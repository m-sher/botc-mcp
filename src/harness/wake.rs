//! Durable wake mailbox + long-poll `await_turn` for continuous agent sessions.
//!
//! Agents stay in one headless process and call [`WakeCoordinator::await_turn`]
//! when idle. The coordinator plans from engine state ([`plan_ticks`]) and either
//! returns a wake payload, a soft `idle` before the client tool timeout, or
//! `game_over`. Wakes stay outstanding until a completing tool succeeds, so a
//! client timeout mid-delivery cannot lose the turn (redelivered on re-await).

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::auth::Actor;
use crate::game::{Game, GameId, Phase, SeatId};
use crate::harness::prompts;
use crate::harness::scheduler::{plan_ticks, wait_signature, HostTask, PlayerTask, SchedTarget};
use crate::mcp_server::SharedStore;

/// Default server-side long-poll budget (seconds). Must stay well under Grok's
/// per-tool `tool_timeouts.await_turn` (harness sets 3600s).
pub const AWAIT_SERVER_BUDGET_SECS: u64 = 300;

/// Grok client timeout for `await_turn` written into per-agent MCP config.
pub const AWAIT_CLIENT_TIMEOUT_SECS: u64 = 3600;

/// How often an unchanged wait signature increments the stall counter while
/// agents are long-polling (wall-clock, not process ticks).
pub const STALL_BUMP_SECS: u64 = 45;

/// Who is waiting for a wake (host or a seat).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum WakeActor {
    Host,
    Player(SeatId),
}

impl WakeActor {
    pub fn from_auth(actor: Actor) -> Self {
        match actor {
            Actor::Host => WakeActor::Host,
            Actor::Player { seat } => WakeActor::Player(seat),
        }
    }

    pub fn label(self) -> String {
        match self {
            WakeActor::Host => "Host".into(),
            WakeActor::Player(s) => format!("P{}", s.0),
        }
    }
}

#[derive(Debug, Clone)]
struct WakeEnvelope {
    seq: u64,
    wake_id: String,
    /// Fingerprint of the scheduled target (for still-valid checks).
    plan_key: String,
    target: SchedTarget,
    /// Full wake text (same content as the old per-tick prompt).
    prompt_text: String,
    kind: String,
}

struct Inner {
    game_id: Option<u64>,
    rotation: usize,
    stall: usize,
    wait_sig: Option<String>,
    stage_key: String,
    last_stall_bump: Option<Instant>,
    next_seq: u64,
    /// Delivered wakes not yet completed by a resolving tool.
    outstanding: HashMap<WakeActor, WakeEnvelope>,
    /// Actors currently blocked inside `await_turn` (for UI).
    waiters: HashMap<WakeActor, Instant>,
    /// Shut down all waiters (TUI quit).
    stopped: bool,
}

/// Shared scheduler + mailbox for harness long-poll wakes.
#[derive(Clone)]
pub struct WakeCoordinator {
    inner: Arc<Mutex<Inner>>,
    cv: Arc<Condvar>,
}

impl Default for WakeCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

impl WakeCoordinator {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                game_id: None,
                rotation: 0,
                stall: 0,
                wait_sig: None,
                stage_key: String::new(),
                last_stall_bump: None,
                next_seq: 1,
                outstanding: HashMap::new(),
                waiters: HashMap::new(),
                stopped: false,
            })),
            cv: Arc::new(Condvar::new()),
        }
    }

    pub fn set_game_id(&self, game_id: u64) {
        let mut g = self.inner.lock().unwrap();
        g.game_id = Some(game_id);
        g.rotation = 0;
        g.stall = 0;
        g.wait_sig = None;
        g.stage_key.clear();
        g.outstanding.clear();
        g.last_stall_bump = None;
        g.stopped = false;
        self.cv.notify_all();
    }

    /// Current bound game id, if any.
    pub fn game_id(&self) -> Option<u64> {
        self.inner.lock().unwrap().game_id
    }

    pub fn stop(&self) {
        let mut g = self.inner.lock().unwrap();
        g.stopped = true;
        self.cv.notify_all();
    }

    /// Wake all long-pollers (e.g. after external state change).
    pub fn notify_all(&self) {
        self.cv.notify_all();
    }

    pub fn rotation(&self) -> usize {
        self.inner.lock().unwrap().rotation
    }

    pub fn stall(&self) -> usize {
        self.inner.lock().unwrap().stall
    }

    /// Actors currently blocked in `await_turn`.
    pub fn waiting_labels(&self) -> Vec<String> {
        let g = self.inner.lock().unwrap();
        g.waiters.keys().map(|a| a.label()).collect()
    }

    pub fn is_waiting(&self, actor: WakeActor) -> bool {
        self.inner.lock().unwrap().waiters.contains_key(&actor)
    }

    /// True when every listed actor is blocked in `await_turn` (all idle).
    pub fn all_waiting(&self, actors: &[WakeActor]) -> bool {
        let g = self.inner.lock().unwrap();
        !actors.is_empty() && actors.iter().all(|a| g.waiters.contains_key(a))
    }

    /// After a successful game tool: clear outstanding if this action completes
    /// the wake, advance discussion/nomination rotation, notify waiters.
    pub fn note_tool_success(&self, actor: WakeActor, tool: &str) {
        {
            let mut g = self.inner.lock().unwrap();
            if let Some(env) = g.outstanding.get(&actor).cloned() {
                if tool_completes_wake(tool, &env.target) {
                    match &env.target {
                        SchedTarget::Player {
                            task: PlayerTask::Discuss { .. },
                            ..
                        }
                        | SchedTarget::Player {
                            task: PlayerTask::Nominate,
                            ..
                        } => {
                            g.rotation = g.rotation.wrapping_add(1);
                        }
                        _ => {}
                    }
                    g.outstanding.remove(&actor);
                }
            }
        }
        self.cv.notify_all();
    }

    /// Long-poll until this actor has a wake, the server budget elapses (`idle`),
    /// the game ends, or the coordinator is stopped.
    pub fn await_turn(
        &self,
        store: &SharedStore,
        actor: WakeActor,
        display_name: &str,
        budget: Duration,
    ) -> Value {
        let deadline = Instant::now() + budget;
        let mut guard = self.inner.lock().unwrap();
        guard.waiters.insert(actor, Instant::now());

        let result = loop {
            if guard.stopped {
                break json!({
                    "status": "stopped",
                    "retry": false,
                    "hint": "Harness is shutting down.",
                });
            }

            match try_deliver(store, &mut guard, actor, display_name) {
                Deliver::GameOver(v) | Deliver::Wake(v) | Deliver::Error(v) => break v,
                Deliver::Idle => {
                    let now = Instant::now();
                    if now >= deadline {
                        let since = guard.outstanding.get(&actor).map(|e| e.seq).unwrap_or(0);
                        break json!({
                            "status": "idle",
                            "next_since_seq": since,
                            "retry": true,
                            "hint": "No wake yet (server poll budget). Call await_turn again immediately. If the tool times out instead, also re-call await_turn — wakes are durable.",
                        });
                    }
                    let remaining = deadline.saturating_duration_since(now);
                    let slice = remaining.min(Duration::from_millis(500));
                    let (g2, _) = self.cv.wait_timeout(guard, slice).unwrap();
                    guard = g2;
                }
            }
        };

        guard.waiters.remove(&actor);
        drop(guard);
        result
    }
}

enum Deliver {
    GameOver(Value),
    Wake(Value),
    Idle,
    /// Permanent failure — do not retry in a hot loop.
    Error(Value),
}

fn try_deliver(
    store: &SharedStore,
    guard: &mut Inner,
    actor: WakeActor,
    display_name: &str,
) -> Deliver {
    let game_id = match guard.game_id {
        Some(id) => GameId(id),
        None => return Deliver::Idle,
    };

    // try_lock: never block on the store while holding the coordinator mutex.
    // Completing tools hold the store then call note_tool_success (coord) — if we
    // waited here we'd deadlock with that order inverted.
    let st = match store.try_lock() {
        Ok(s) => s,
        Err(std::sync::TryLockError::WouldBlock) => return Deliver::Idle,
        Err(std::sync::TryLockError::Poisoned(_)) => {
            // Permanent: poisoned mutex stays poisoned — do not invite a hot retry loop.
            return Deliver::Error(json!({
                "status": "error",
                "retry": false,
                "hint": "Store lock poisoned; harness must restart.",
            }));
        }
    };
    let Some(game) = st.get(game_id) else {
        return Deliver::Idle;
    };

    if matches!(game.phase, Phase::Ended { .. }) {
        let winner = format!("{:?}", game.winner);
        return Deliver::GameOver(json!({
            "status": "game_over",
            "retry": false,
            "winner": winner,
            "phase": format!("{:?}", game.phase),
        }));
    }

    // Stage change resets discussion rotation (same as old TUI).
    let key = stage_key_of(game);
    if key != guard.stage_key {
        guard.stage_key = key;
        guard.rotation = 0;
        guard.stall = 0;
        guard.wait_sig = None;
        guard.last_stall_bump = None;
        guard.outstanding.clear();
    }

    // Stall bump on stable wait signature (wall clock).
    let sig = wait_signature(game);
    let now = Instant::now();
    if sig.is_some() && sig == guard.wait_sig {
        let due = guard
            .last_stall_bump
            .map(|t| now.duration_since(t) >= Duration::from_secs(STALL_BUMP_SECS))
            .unwrap_or(true);
        if due {
            guard.stall = guard.stall.saturating_add(1);
            guard.last_stall_bump = Some(now);
        }
    } else {
        guard.wait_sig = sig;
        guard.stall = 0;
        guard.last_stall_bump = Some(now);
    }

    let plan = plan_ticks(game, guard.rotation, guard.stall);

    // Redeliver outstanding if still planned for this actor.
    if let Some(env) = guard.outstanding.get(&actor).cloned() {
        if plan_still_contains(&plan, &env) {
            // Summary only when delivering a wake (idle polls skip O(log) string build).
            let (summary, _) = public_summary(game);
            return Deliver::Wake(wake_json(&env, &summary));
        }
        guard.outstanding.remove(&actor);
    }

    // Fresh assignment if we are in the plan.
    if let Some(target) = plan
        .iter()
        .find(|t| target_matches_actor(t, actor))
        .cloned()
    {
        let (summary, host_hint) = public_summary(game);
        let plan_key = plan_key_of(&target);
        let seq = guard.next_seq;
        guard.next_seq = guard.next_seq.saturating_add(1);
        let prompt_text = render_prompt(display_name, game.id.0, &target, &summary, &host_hint);
        let kind = kind_of(&target);
        let env = WakeEnvelope {
            seq,
            wake_id: format!("w-{}-{}", seq, plan_key),
            plan_key,
            target,
            prompt_text,
            kind,
        };
        let out = wake_json(&env, &summary);
        guard.outstanding.insert(actor, env);
        return Deliver::Wake(out);
    }

    Deliver::Idle
}

fn target_matches_actor(t: &SchedTarget, actor: WakeActor) -> bool {
    match (t, actor) {
        (SchedTarget::Host(_), WakeActor::Host) => true,
        (SchedTarget::Player { seat, .. }, WakeActor::Player(s)) => *seat == s,
        _ => false,
    }
}

fn plan_still_contains(plan: &[SchedTarget], env: &WakeEnvelope) -> bool {
    plan.iter()
        .any(|t| plan_key_of(t) == env.plan_key && target_matches_actor(t, actor_of(&env.target)))
}

fn actor_of(t: &SchedTarget) -> WakeActor {
    match t {
        SchedTarget::Host(_) => WakeActor::Host,
        SchedTarget::Player { seat, .. } => WakeActor::Player(*seat),
    }
}

fn plan_key_of(t: &SchedTarget) -> String {
    match t {
        SchedTarget::Host(h) => format!("host:{h:?}"),
        SchedTarget::Player { seat, task } => format!("p{}:{task:?}", seat.0),
    }
}

fn kind_of(t: &SchedTarget) -> String {
    match t {
        SchedTarget::Host(HostTask::StartGame) => "host_start_game".into(),
        SchedTarget::Host(HostTask::ResolveDecision { .. }) => "host_decide".into(),
        SchedTarget::Host(HostTask::AdvanceNight) => "host_advance_night".into(),
        SchedTarget::Host(HostTask::SkipStuckWake { .. }) => "host_skip_stuck".into(),
        SchedTarget::Host(HostTask::CloseVoting) => "host_close_vote".into(),
        SchedTarget::Host(HostTask::EndDay { .. }) => "host_end_day".into(),
        SchedTarget::Player {
            task: PlayerTask::NightWake { .. },
            ..
        } => "night_action".into(),
        SchedTarget::Player {
            task: PlayerTask::Discuss { .. },
            ..
        } => "discuss".into(),
        SchedTarget::Player {
            task: PlayerTask::Nominate,
            ..
        } => "nominate".into(),
        SchedTarget::Player {
            task: PlayerTask::Vote { .. },
            ..
        } => "vote".into(),
    }
}

fn render_prompt(
    display_name: &str,
    game_id: u64,
    target: &SchedTarget,
    summary: &str,
    host_hint: &str,
) -> String {
    match target {
        SchedTarget::Host(task) => prompts::host_task_tick(game_id, task, summary, host_hint),
        SchedTarget::Player { seat, task } => {
            prompts::player_task_tick(display_name, *seat, game_id, task, summary)
        }
    }
}

fn wake_json(env: &WakeEnvelope, public_summary: &str) -> Value {
    json!({
        "status": "wake",
        "wake_id": env.wake_id,
        "seq": env.seq,
        "next_since_seq": env.seq,
        "kind": env.kind,
        "prompt": env.prompt_text,
        "public_summary": public_summary,
        "retry": false,
        "hint": "Take the legal action(s) described in prompt, then call await_turn again. If you see the same wake_id twice, finish the action (redelivery after timeout is normal).",
    })
}

fn tool_completes_wake(tool: &str, target: &SchedTarget) -> bool {
    match target {
        SchedTarget::Player {
            task: PlayerTask::NightWake { .. },
            ..
        } => tool == "night_action",
        SchedTarget::Player {
            task: PlayerTask::Discuss { .. },
            ..
        } => tool == "say" || tool == "nominate",
        SchedTarget::Player {
            task: PlayerTask::Nominate,
            ..
        } => tool == "nominate" || tool == "say",
        SchedTarget::Player {
            task: PlayerTask::Vote { .. },
            ..
        } => tool == "vote" || tool == "pass_vote",
        SchedTarget::Host(HostTask::StartGame) => tool == "start_game",
        SchedTarget::Host(HostTask::ResolveDecision { .. }) => {
            tool == "host_decide" || tool == "skip_night_action"
        }
        SchedTarget::Host(HostTask::AdvanceNight) => tool == "skip_night_action",
        SchedTarget::Host(HostTask::SkipStuckWake { .. }) => tool == "skip_night_action",
        SchedTarget::Host(HostTask::CloseVoting) => tool == "close_vote",
        SchedTarget::Host(HostTask::EndDay {
            in_discussion: true,
        }) => tool == "open_nominations" || tool == "end_nominations",
        SchedTarget::Host(HostTask::EndDay {
            in_discussion: false,
        }) => tool == "end_nominations",
    }
}

/// Stage fingerprint for rotation reset. Day keys include **living count** so a
/// mid-discussion death (e.g. Slayer) restarts the round against the new roster
/// instead of mis-indexing `rotation / living.len()` in `plan_ticks`.
fn stage_key_of(game: &Game) -> String {
    let living = game.seats.iter().filter(|s| s.alive).count();
    match &game.phase {
        Phase::Lobby => "lobby".into(),
        Phase::FirstNight { .. } => "n1".into(),
        Phase::Night { night, .. } => format!("n{night}"),
        Phase::Day { day, stage } => format!("d{day}-{stage:?}-{living}"),
        Phase::Ended { .. } => "ended".into(),
    }
}

fn fmt_public_event(e: &crate::comms::PublicEvent) -> String {
    use crate::comms::PublicEvent::*;
    let one_line = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
    match e {
        Chat { seat, text, .. } => format!("P{}: {}", seat.0, one_line(text)),
        StorytellerAnnounce { text } => format!("Storyteller: {}", one_line(text)),
        Nominated { by, target } => format!("P{} nominated P{}", by.0, target.0),
        VoteCast {
            seat,
            nominee,
            support,
        } => format!(
            "P{} voted {} on P{}",
            seat.0,
            if *support { "YES" } else { "no" },
            nominee.0
        ),
        Executed { seat } => format!("P{} was executed", seat.0),
        NoExecution => "No one was executed today".to_string(),
        DiedInNight { seats } => {
            if seats.is_empty() {
                "No one died in the night".to_string()
            } else {
                format!(
                    "Died in the night: {}",
                    seats
                        .iter()
                        .map(|s| format!("P{}", s.0))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
        }
        PlayerDied { seat } => format!("P{} died", seat.0),
        SlayerMiss { slayer, target } => {
            format!(
                "P{} tried to slay P{} — nothing happened",
                slayer.0, target.0
            )
        }
        PhaseChanged { summary } => summary.clone(),
        GameEnded { winner } => format!("Game over — {winner:?} wins"),
    }
}

fn public_summary(game: &Game) -> (String, String) {
    let phase = format!("{:?}", game.phase);
    let living: Vec<_> = game
        .seats
        .iter()
        .filter(|s| s.alive)
        .map(|s| format!("P{}", s.id.0))
        .collect();
    let dead: Vec<_> = game
        .seats
        .iter()
        .filter(|s| !s.alive)
        .map(|s| format!("P{}", s.id.0))
        .collect();
    let recent: Vec<_> = game
        .public_log
        .since(0)
        .into_iter()
        .rev()
        .take(16)
        .map(|(_, e)| fmt_public_event(e))
        .collect();
    let recent: Vec<_> = recent.into_iter().rev().collect();
    let recent_str = if recent.is_empty() {
        "(nothing public has happened yet)".to_string()
    } else {
        recent.join("\n")
    };
    let summary = format!(
        "phase: {phase}\nliving: {}\ndead: {}\nrecent public events:\n{}",
        living.join(", "),
        if dead.is_empty() {
            "none".to_string()
        } else {
            dead.join(", ")
        },
        recent_str
    );
    let hint = if game.pending_host.is_some() {
        "pending_host set — resolve with host_decide or skip_night_action".into()
    } else if let Some(w) = &game.pending_night {
        format!("pending night wake seat {}", w.seat.0)
    } else {
        "no pending host/night wait".into()
    };
    (summary, hint)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::{Game, StartOpts};
    use crate::harness::scheduler::STALL_ESCALATE;
    use crate::store::GameStore;
    use std::thread;

    fn five_names() -> Vec<String> {
        (0..5).map(|i| format!("P{i}")).collect()
    }

    fn started_store() -> (SharedStore, u64) {
        let mut games = GameStore::new();
        let created = Game::create(five_names(), 42).unwrap();
        let host = created.host_token.clone();
        let mut game = created.game;
        game.start_game(&host, StartOpts::default()).unwrap();
        let id = games.insert(game);
        (Arc::new(Mutex::new(games)), id.0)
    }

    #[test]
    fn await_returns_idle_or_wake_quickly() {
        let (store, gid) = started_store();
        let coord = WakeCoordinator::new();
        coord.set_game_id(gid);
        let v = coord.await_turn(
            &store,
            WakeActor::Player(SeatId(4)),
            "P4",
            Duration::from_millis(80),
        );
        let status = v["status"].as_str().unwrap();
        assert!(
            status == "idle" || status == "wake",
            "unexpected status {status}: {v}"
        );
        if status == "idle" {
            assert_eq!(v["retry"], true);
        }
    }

    fn first_planned_actor(store: &SharedStore, gid: u64, coord: &WakeCoordinator) -> WakeActor {
        let st = store.lock().unwrap();
        let g = st.get(GameId(gid)).unwrap();
        let plan = plan_ticks(g, coord.rotation(), coord.stall());
        match &plan[0] {
            SchedTarget::Host(_) => WakeActor::Host,
            SchedTarget::Player { seat, .. } => WakeActor::Player(*seat),
        }
    }

    #[test]
    fn redelivery_after_wake_without_action() {
        let (store, gid) = started_store();
        let coord = WakeCoordinator::new();
        coord.set_game_id(gid);
        let actor = first_planned_actor(&store, gid, &coord);
        let name = actor.label();
        let v1 = coord.await_turn(&store, actor, &name, Duration::from_secs(2));
        assert_eq!(v1["status"], "wake", "{v1}");
        let id1 = v1["wake_id"].as_str().unwrap().to_string();
        let v2 = coord.await_turn(&store, actor, &name, Duration::from_secs(2));
        assert_eq!(v2["status"], "wake", "{v2}");
        assert_eq!(
            v2["wake_id"].as_str().unwrap(),
            id1,
            "same wake redelivered"
        );
    }

    #[test]
    fn note_tool_clears_outstanding() {
        let (store, gid) = started_store();
        let coord = WakeCoordinator::new();
        coord.set_game_id(gid);
        let actor = first_planned_actor(&store, gid, &coord);
        let name = actor.label();
        let v1 = coord.await_turn(&store, actor, &name, Duration::from_secs(2));
        assert_eq!(v1["status"], "wake", "{v1}");
        let kind = v1["kind"].as_str().unwrap();
        let tool = match kind {
            "night_action" => "night_action",
            "discuss" | "nominate" => "say",
            "vote" => "vote",
            k if k.starts_with("host_") => "skip_night_action",
            other => panic!("unexpected kind {other}"),
        };
        // Host end-day / close-vote need different tools.
        let tool = match kind {
            "host_close_vote" => "close_vote",
            "host_end_day" => "end_nominations",
            "host_start_game" => "start_game",
            _ => tool,
        };
        coord.note_tool_success(actor, tool);
        assert!(
            !coord.inner.lock().unwrap().outstanding.contains_key(&actor),
            "kind={kind} tool={tool} still outstanding"
        );
    }

    #[test]
    fn concurrent_waiter_wakes_on_notify() {
        let (store, gid) = started_store();
        let coord = WakeCoordinator::new();
        coord.set_game_id(gid);
        let coord2 = coord.clone();
        let store2 = Arc::clone(&store);
        let h = thread::spawn(move || {
            coord2.await_turn(&store2, WakeActor::Host, "Host", Duration::from_millis(200))
        });
        thread::sleep(Duration::from_millis(30));
        coord.notify_all();
        let v = h.join().unwrap();
        let status = v["status"].as_str().unwrap();
        assert!(status == "idle" || status == "wake", "{v}");
    }

    #[test]
    fn tool_completes_wake_matrix() {
        let night = SchedTarget::Player {
            seat: SeatId(0),
            task: PlayerTask::NightWake { prompt: "x".into() },
        };
        assert!(tool_completes_wake("night_action", &night));
        assert!(!tool_completes_wake("say", &night));
        let disc = SchedTarget::Player {
            seat: SeatId(0),
            task: PlayerTask::Discuss {
                round: 0,
                last_round: false,
            },
        };
        assert!(tool_completes_wake("say", &disc));
        assert!(tool_completes_wake("nominate", &disc));
    }

    #[test]
    fn host_gets_wake_when_planned() {
        let (store, gid) = started_store();
        let coord = WakeCoordinator::new();
        coord.set_game_id(gid);
        // host_first start usually has pending_host; if not, force stall skip path.
        {
            let mut g = coord.inner.lock().unwrap();
            g.stall = STALL_ESCALATE;
            let sig = {
                let st = store.lock().unwrap();
                wait_signature(st.get(GameId(gid)).unwrap())
            };
            g.wait_sig = sig;
            g.last_stall_bump = Some(Instant::now());
        }
        let v = coord.await_turn(&store, WakeActor::Host, "Host", Duration::from_secs(2));
        // Host is planned on pending_host, advance-night, or stall skip — not always,
        // but with STALL_ESCALATE and a night wait it should be.
        let status = v["status"].as_str().unwrap();
        if status == "wake" {
            assert!(v["kind"].as_str().unwrap().starts_with("host_"), "{v}");
        } else {
            // No host work in this seed/state — acceptable; still a valid soft idle.
            assert_eq!(status, "idle", "{v}");
        }
    }

    #[test]
    fn stage_key_includes_living_count_on_day() {
        let (store, gid) = started_store();
        // Force Day Discussion with known living roster.
        {
            let mut st = store.lock().unwrap();
            let g = st.get_mut(GameId(gid)).unwrap();
            g.phase = Phase::Day {
                day: 1,
                stage: crate::game::DayStage::Discussion,
            };
            g.pending_night = None;
            g.pending_host = None;
            for s in &mut g.seats {
                s.alive = true;
            }
            let k5 = stage_key_of(g);
            assert!(
                k5.ends_with("-5") || k5.contains("-5"),
                "living=5 should appear in stage key: {k5}"
            );
            g.seats[2].alive = false;
            let k4 = stage_key_of(g);
            assert_ne!(k5, k4, "death must change stage key so rotation resets");
            assert!(
                k4.contains("-4"),
                "living=4 should appear in stage key: {k4}"
            );
        }
    }

    #[test]
    fn mid_discussion_death_resets_rotation() {
        let (store, gid) = started_store();
        let coord = WakeCoordinator::new();
        coord.set_game_id(gid);
        {
            let mut st = store.lock().unwrap();
            let g = st.get_mut(GameId(gid)).unwrap();
            g.phase = Phase::Day {
                day: 1,
                stage: crate::game::DayStage::Discussion,
            };
            g.pending_night = None;
            g.pending_host = None;
            for s in &mut g.seats {
                s.alive = true;
            }
        }
        // Deliver a discuss wake so stage_key is stamped with living=5.
        let v = coord.await_turn(
            &store,
            WakeActor::Player(SeatId(0)),
            "P0",
            Duration::from_secs(1),
        );
        assert_eq!(v["status"], "wake", "{v}");
        assert_eq!(v["kind"], "discuss");
        // Simulate several speakers having already gone (high rotation).
        {
            let mut g = coord.inner.lock().unwrap();
            g.rotation = 7;
        }
        // Mid-day death shrinks living 5→4; next deliver must reset rotation.
        {
            let mut st = store.lock().unwrap();
            st.get_mut(GameId(gid)).unwrap().seats[1].alive = false;
        }
        let _ = coord.await_turn(
            &store,
            WakeActor::Player(SeatId(0)),
            "P0",
            Duration::from_secs(1),
        );
        assert_eq!(
            coord.rotation(),
            0,
            "rotation must reset when living count changes mid-discussion"
        );
    }

    /// Regression: await_turn must not block on the store while holding the
    /// coordinator (inverted order vs note_tool_success / TUI maintain).
    #[test]
    fn try_lock_avoids_deadlock_under_store_hold() {
        let (store, gid) = started_store();
        let coord = WakeCoordinator::new();
        coord.set_game_id(gid);

        let store_hold = Arc::clone(&store);
        let blocker = thread::spawn(move || {
            let _guard = store_hold.lock().unwrap();
            thread::sleep(Duration::from_millis(400));
        });

        // Give the blocker the store first.
        thread::sleep(Duration::from_millis(20));

        let coord2 = coord.clone();
        let store2 = Arc::clone(&store);
        let waiter = thread::spawn(move || {
            // Must return (idle) without hanging while store is held.
            coord2.await_turn(
                &store2,
                WakeActor::Player(SeatId(0)),
                "P0",
                Duration::from_millis(150),
            )
        });

        // Concurrent note_tool while store is held by blocker — coord-only path.
        thread::sleep(Duration::from_millis(30));
        coord.note_tool_success(WakeActor::Player(SeatId(0)), "say");
        // TUI-style: hold store then read coordinator (would deadlock if await blocked).
        {
            let _st = store.lock().unwrap();
            let _ = coord.rotation();
            let _ = coord.waiting_labels();
        }

        let v = waiter
            .join()
            .expect("await_turn thread must not hang/panic");
        let status = v["status"].as_str().unwrap();
        assert!(
            status == "idle" || status == "wake",
            "expected soft result under store contention: {v}"
        );
        blocker.join().unwrap();
    }
}
