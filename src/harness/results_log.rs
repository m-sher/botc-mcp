//! Append-only JSONL log of ranking-relevant game outcomes.
//!
//! Separate from the verbose TUI debug log: this is a durable, machine-readable
//! corpus for a future model ranking system. One JSON object per line.
//!
//! Default path: `botc-results.jsonl` at the repo root (override with `BOTC_RESULTS_LOG`).
//! Opens in **append** mode so multiple TUI runs accumulate.
//!
//! Schema version is the `v` field on every record. Current: **1**.
//!
//! ## Events (`event` field)
//!
//! | event | When | Ranking relevance |
//! | --- | --- | --- |
//! | `game_start` | After launch / start_game | Models ↔ seats, true roles, seed |
//! | `death` | Seat dies (exec / night / day ability) | Survival, who killed whom |
//! | `nomination` | A nomination opens | Social targeting metrics later |
//! | `game_end` | Engine reaches Ended | Team win + per-seat outcome |
//! | `game_abort` | TUI quit mid-game | Incomplete run filtering |
//! | `tick_usage` | Headless grok tick reports `usage` | Token spend / context growth |
//!
//! Chat / tool spam is intentionally **not** logged here.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::comms::PublicEvent;
use crate::game::{EndReason, Game, Phase, SeatId, Winner};
use crate::harness::agents::{
    AgentConfig, AgentRole, AgentUsage, ContextWindow, TickUsage,
};

static FILE: OnceLock<Mutex<Option<File>>> = OnceLock::new();
static PATH: OnceLock<PathBuf> = OnceLock::new();
/// Set by the TUI so tick_usage lines share the same run id without threading it
/// through every spawn.
static RUN_ID: OnceLock<Mutex<String>> = OnceLock::new();

/// Schema version stamped on every line.
pub const SCHEMA_VERSION: u32 = 1;

/// Resolve log path: `$BOTC_RESULTS_LOG`, else `botc-results.jsonl` at the repo
/// root (pinned via `CARGO_MANIFEST_DIR` so the location is stable regardless of
/// the working directory the TUI is launched from).
pub fn log_path() -> PathBuf {
    if let Ok(p) = std::env::var("BOTC_RESULTS_LOG") {
        return PathBuf::from(p);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("botc-results.jsonl")
}

/// Open the results log for append. Idempotent. Returns the path in use.
pub fn init() -> PathBuf {
    let path = log_path();
    let _ = PATH.set(path.clone());
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok();
    let _ = FILE.set(Mutex::new(file));
    path
}

/// Stamp every subsequent `tick_usage` / game event with this TUI run id.
pub fn set_run_id(run_id: &str) {
    let slot = RUN_ID.get_or_init(|| Mutex::new(String::new()));
    *slot.lock().unwrap() = run_id.to_string();
}

fn current_run_id() -> String {
    RUN_ID
        .get()
        .and_then(|m| {
            let s = m.lock().unwrap();
            if s.is_empty() {
                None
            } else {
                Some(s.clone())
            }
        })
        .unwrap_or_else(|| "unknown".into())
}

fn tick_usage_json(t: &TickUsage) -> Value {
    json!({
        "input_tokens": t.input_tokens,
        "cache_read_input_tokens": t.cache_read_input_tokens,
        "output_tokens": t.output_tokens,
        "reasoning_tokens": t.reasoning_tokens,
        "total_tokens": t.total_tokens,
        "num_turns": t.num_turns,
    })
}

fn context_json(c: &ContextWindow) -> Value {
    json!({
        "tokens_used": c.tokens_used,
        "window_tokens": c.window_tokens,
        "usage_pct": c.usage_pct,
    })
}

/// One headless tick's spend (from streaming-json `end.usage`).
pub fn log_tick_usage(
    game_id: u64,
    agent: &str,
    model: &str,
    tick: &TickUsage,
    context: Option<&ContextWindow>,
    cumulative: &TickUsage,
    ticks_with_usage: u32,
) {
    emit(json!({
        "event": "tick_usage",
        "run_id": current_run_id(),
        "game_id": game_id,
        "agent": agent,
        "model": model,
        "tick": tick_usage_json(tick),
        "cumulative": tick_usage_json(cumulative),
        "ticks_with_usage": ticks_with_usage,
        "context": context.map(context_json),
    }));
}

/// Usage snapshot for `game_end` / `game_abort` (agent label → usage).
pub fn usage_snapshot_json(entries: &[(String, AgentUsage)]) -> Vec<Value> {
    entries
        .iter()
        .map(|(label, u)| {
            json!({
                "agent": label,
                "ticks_with_usage": u.ticks_with_usage,
                "last_tick": u.last_tick.as_ref().map(tick_usage_json),
                "game_total": tick_usage_json(&u.game_total),
                "context": u.context.as_ref().map(context_json),
            })
        })
        .collect()
}

/// True if a file handle is open.
pub fn enabled() -> bool {
    FILE.get()
        .map(|m| m.lock().unwrap().is_some())
        .unwrap_or(false)
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Append one JSON object as a single line (flushed). No-op if not initialised.
pub fn emit(mut record: Value) {
    if let Some(obj) = record.as_object_mut() {
        obj.entry("v").or_insert_with(|| json!(SCHEMA_VERSION));
        obj.entry("ts_unix_ms").or_insert_with(|| json!(now_unix_ms()));
    }
    let Some(m) = FILE.get() else {
        return;
    };
    let mut g = m.lock().unwrap();
    let Some(f) = g.as_mut() else {
        return;
    };
    if let Ok(line) = serde_json::to_string(&record) {
        let _ = writeln!(f, "{line}");
        let _ = f.flush();
    }
}

/// Build the ranking-facing seat roster from the live grimoire + agent models.
pub fn seats_snapshot(game: &Game, agents: &[AgentConfig]) -> Vec<Value> {
    game.seats
        .iter()
        .map(|s| {
            let model = agents
                .iter()
                .find(|a| matches!(a.role, AgentRole::Player { seat } if seat == s.id))
                .map(|a| a.model.as_str())
                .unwrap_or("");
            let true_c = s.true_character;
            let believed = s.believed_character.or(true_c);
            json!({
                "seat": s.id.0,
                "name": s.display_name,
                "model": model,
                "true_character": true_c.map(|c| c.display_name()),
                "believed_character": believed.map(|c| c.display_name()),
                "team": true_c.map(|c| format!("{:?}", c.team())),
                "character_type": true_c.map(|c| format!("{:?}", c.character_type())),
                "alive": s.alive,
                "is_drunk_outsider": s.is_drunk_outsider,
            })
        })
        .collect()
}

fn host_model(agents: &[AgentConfig]) -> String {
    agents
        .iter()
        .find(|a| matches!(a.role, AgentRole::Host))
        .map(|a| a.model.clone())
        .unwrap_or_default()
}

fn model_for_seat(agents: &[AgentConfig], seat: SeatId) -> String {
    agents
        .iter()
        .find(|a| matches!(a.role, AgentRole::Player { seat: s } if s == seat))
        .map(|a| a.model.clone())
        .unwrap_or_default()
}

fn phase_fields(game: &Game) -> Value {
    match &game.phase {
        Phase::Lobby => json!({ "kind": "lobby" }),
        Phase::FirstNight { .. } => json!({ "kind": "night", "night": 1 }),
        Phase::Night { night, .. } => json!({ "kind": "night", "night": night }),
        Phase::Day { day, stage } => json!({
            "kind": "day",
            "day": day,
            "stage": format!("{:?}", stage),
        }),
        Phase::Ended { winner, reason } => json!({
            "kind": "ended",
            "winner": format!("{winner:?}"),
            "reason": format!("{reason:?}"),
        }),
    }
}

/// `game_start` — full assignment for ranking join keys.
pub fn log_game_start(
    run_id: &str,
    game_id: u64,
    game: &Game,
    agents: &[AgentConfig],
    seed: u64,
    st_choice_mode: &str,
) {
    emit(json!({
        "event": "game_start",
        "run_id": run_id,
        "game_id": game_id,
        "seed": seed,
        "player_count": game.seats.len(),
        "st_choice_mode": st_choice_mode,
        "host": {
            "model": host_model(agents),
            "display_name": "Storyteller",
        },
        "seats": seats_snapshot(game, agents),
        "phase": phase_fields(game),
    }));
}

/// Map a public death-related event into a ranking `death` record (if applicable).
pub fn death_records_from_public(
    run_id: &str,
    game_id: u64,
    game: &Game,
    agents: &[AgentConfig],
    ev: &PublicEvent,
) -> Vec<Value> {
    let phase = phase_fields(game);
    let enrich = |seat: SeatId, cause: &str| -> Value {
        let s = game.seats.iter().find(|x| x.id == seat);
        json!({
            "event": "death",
            "run_id": run_id,
            "game_id": game_id,
            "seat": seat.0,
            "cause": cause,
            "model": model_for_seat(agents, seat),
            "true_character": s.and_then(|x| x.true_character).map(|c| c.display_name()),
            "team": s.and_then(|x| x.true_character).map(|c| format!("{:?}", c.team())),
            "phase": phase,
        })
    };
    match ev {
        PublicEvent::Executed { seat } => vec![enrich(*seat, "executed")],
        PublicEvent::DiedInNight { seats } => seats
            .iter()
            .map(|seat| enrich(*seat, "night"))
            .collect(),
        // PlayerDied also fires after Executed — skip duplicates by only logging
        // PlayerDied when it is not immediately an execution companion. Callers
        // pass events in order; we log PlayerDied as `day` only if the seat is
        // already dead and this isn't pure noise. Safer: log PlayerDied as
        // "day" and let consumers de-dupe exec+PlayerDied pairs by seat+time.
        PublicEvent::PlayerDied { seat } => vec![enrich(*seat, "day")],
        _ => vec![],
    }
}

/// `nomination` — who put whom on the block (for future social metrics).
pub fn log_nomination_if_any(
    run_id: &str,
    game_id: u64,
    game: &Game,
    agents: &[AgentConfig],
    ev: &PublicEvent,
) {
    let PublicEvent::Nominated { by, target } = ev else {
        return;
    };
    emit(json!({
        "event": "nomination",
        "run_id": run_id,
        "game_id": game_id,
        "by_seat": by.0,
        "target_seat": target.0,
        "by_model": model_for_seat(agents, *by),
        "target_model": model_for_seat(agents, *target),
        "by_team": game.seats.iter().find(|s| s.id == *by)
            .and_then(|s| s.true_character).map(|c| format!("{:?}", c.team())),
        "target_team": game.seats.iter().find(|s| s.id == *target)
            .and_then(|s| s.true_character).map(|c| format!("{:?}", c.team())),
        "phase": phase_fields(game),
    }));
}

fn winner_str(w: Winner) -> &'static str {
    match w {
        Winner::Good => "Good",
        Winner::Evil => "Evil",
    }
}

fn reason_str(r: EndReason) -> &'static str {
    match r {
        EndReason::DemonDead => "DemonDead",
        EndReason::EvilTwoAlive => "EvilTwoAlive",
        EndReason::SaintExecuted => "SaintExecuted",
        EndReason::MayorThreeNoExec => "MayorThreeNoExec",
    }
}

/// `game_end` — terminal outcome + per-seat win flags for ranking.
pub fn log_game_end(
    run_id: &str,
    game_id: u64,
    game: &Game,
    agents: &[AgentConfig],
    usage: &[(String, AgentUsage)],
) {
    let (winner, reason) = match &game.phase {
        Phase::Ended { winner, reason } => (*winner, *reason),
        _ => return,
    };
    let win_team = match winner {
        Winner::Good => "Good",
        Winner::Evil => "Evil",
    };
    let seats: Vec<Value> = seats_snapshot(game, agents)
        .into_iter()
        .map(|mut seat| {
            if let Some(obj) = seat.as_object_mut() {
                let team = obj
                    .get("team")
                    .and_then(|t| t.as_str())
                    .unwrap_or("");
                obj.insert("won".into(), json!(team == win_team));
            }
            seat
        })
        .collect();
    emit(json!({
        "event": "game_end",
        "run_id": run_id,
        "game_id": game_id,
        "winner": winner_str(winner),
        "reason": reason_str(reason),
        "host": { "model": host_model(agents) },
        "seats": seats,
        "usage": usage_snapshot_json(usage),
        "phase": phase_fields(game),
    }));
}

/// `game_abort` — TUI quit (or crash path) before a clean Ended phase.
pub fn log_game_abort(
    run_id: &str,
    game_id: u64,
    game: Option<&Game>,
    agents: &[AgentConfig],
    why: &str,
    usage: &[(String, AgentUsage)],
) {
    let seats = game
        .map(|g| seats_snapshot(g, agents))
        .unwrap_or_default();
    let phase = game.map(phase_fields).unwrap_or(json!({ "kind": "unknown" }));
    emit(json!({
        "event": "game_abort",
        "run_id": run_id,
        "game_id": game_id,
        "why": why,
        "host": { "model": host_model(agents) },
        "seats": seats,
        "usage": usage_snapshot_json(usage),
        "phase": phase,
    }));
}

/// Process new public-log events since `cursor` (exclusive); returns new cursor.
///
/// Emits `death` / `nomination` records. Skips `PlayerDied` when the previous
/// event in this batch was `Executed` for the same seat (engine emits both).
pub fn drain_public_events(
    run_id: &str,
    game_id: u64,
    game: &Game,
    agents: &[AgentConfig],
    cursor: u64,
) -> u64 {
    let events = game.public_log.since(cursor);
    let mut last = cursor;
    let mut prev_executed: Option<u8> = None;
    for (id, ev) in events {
        last = id;
        match ev {
            PublicEvent::Executed { seat } => {
                for rec in death_records_from_public(run_id, game_id, game, agents, ev) {
                    emit(rec);
                }
                prev_executed = Some(seat.0);
            }
            PublicEvent::PlayerDied { seat } if prev_executed == Some(seat.0) => {
                // Duplicate of Executed — skip.
                prev_executed = None;
            }
            PublicEvent::PlayerDied { .. } | PublicEvent::DiedInNight { .. } => {
                for rec in death_records_from_public(run_id, game_id, game, agents, ev) {
                    emit(rec);
                }
                prev_executed = None;
            }
            PublicEvent::Nominated { .. } => {
                log_nomination_if_any(run_id, game_id, game, agents, ev);
                prev_executed = None;
            }
            _ => {
                prev_executed = None;
            }
        }
    }
    last
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::{Game, RoleAssignment, SeatId, StartOpts};
    use crate::roles::Character;

    fn five_seat_game() -> Game {
        let lobby = Game::create((0..5).map(|i| format!("P{i}")).collect(), 99).unwrap();
        let host = lobby.host_token.clone();
        let mut game = lobby.game;
        game.start_game(
            &host,
            StartOpts {
                assignments: Some(vec![
                    RoleAssignment::normal(SeatId(0), Character::Empath),
                    RoleAssignment::normal(SeatId(1), Character::Soldier),
                    RoleAssignment::normal(SeatId(2), Character::Chef),
                    RoleAssignment::normal(SeatId(3), Character::Poisoner),
                    RoleAssignment::normal(SeatId(4), Character::Imp),
                ]),
                ..Default::default()
            },
        )
        .unwrap();
        game
    }

    #[test]
    fn seats_snapshot_includes_model_and_true_role() {
        let game = five_seat_game();
        let agents = vec![
            AgentConfig {
                role: AgentRole::Host,
                display_name: "Storyteller".into(),
                token: "h".into(),
                game_id: 1,
                model: "host-model".into(),
            },
            AgentConfig {
                role: AgentRole::Player { seat: SeatId(0) },
                display_name: "P0".into(),
                token: "p0".into(),
                game_id: 1,
                model: "model-a".into(),
            },
            AgentConfig {
                role: AgentRole::Player { seat: SeatId(4) },
                display_name: "P4".into(),
                token: "p4".into(),
                game_id: 1,
                model: "model-b".into(),
            },
        ];
        let seats = seats_snapshot(&game, &agents);
        assert_eq!(seats.len(), 5);
        assert_eq!(seats[0]["model"], "model-a");
        assert_eq!(seats[0]["true_character"], "Empath");
        assert_eq!(seats[0]["team"], "Good");
        assert_eq!(seats[4]["model"], "model-b");
        assert_eq!(seats[4]["team"], "Evil");
        // Seats without a matching agent get empty model.
        assert_eq!(seats[1]["model"], "");
    }

    #[test]
    fn death_records_tag_executed() {
        let game = five_seat_game();
        let agents = vec![AgentConfig {
            role: AgentRole::Player { seat: SeatId(4) },
            display_name: "P4".into(),
            token: "x".into(),
            game_id: 1,
            model: "imp-model".into(),
        }];
        let recs = death_records_from_public(
            "run",
            1,
            &game,
            &agents,
            &PublicEvent::Executed { seat: SeatId(4) },
        );
        assert_eq!(recs.len(), 1);
        assert_eq!(recs[0]["event"], "death");
        assert_eq!(recs[0]["cause"], "executed");
        assert_eq!(recs[0]["seat"], 4);
        assert_eq!(recs[0]["model"], "imp-model");
    }

    #[test]
    fn drain_dedupes_executed_player_died_pair() {
        let mut game = five_seat_game();
        game.public_log.push(PublicEvent::Executed { seat: SeatId(2) });
        game.public_log.push(PublicEvent::PlayerDied { seat: SeatId(2) });
        game.public_log.push(PublicEvent::DiedInNight {
            seats: vec![SeatId(3)],
        });
        let agents = vec![
            AgentConfig {
                role: AgentRole::Player { seat: SeatId(2) },
                display_name: "P2".into(),
                token: "a".into(),
                game_id: 1,
                model: "m2".into(),
            },
            AgentConfig {
                role: AgentRole::Player { seat: SeatId(3) },
                display_name: "P3".into(),
                token: "b".into(),
                game_id: 1,
                model: "m3".into(),
            },
        ];
        // Drain without an open FILE still advances cursor; we only assert cursor
        // and that death_records shapes are correct (emit is a no-op until init).
        let end = drain_public_events("run", 1, &game, &agents, 0);
        assert!(end >= 3);
        // Direct shape checks for night death.
        let night = death_records_from_public(
            "run",
            1,
            &game,
            &agents,
            &PublicEvent::DiedInNight {
                seats: vec![SeatId(3)],
            },
        );
        assert_eq!(night[0]["cause"], "night");
        assert_eq!(night[0]["model"], "m3");
    }

    #[test]
    fn json_line_roundtrip_shape() {
        let v = json!({
            "event": "game_end",
            "run_id": "r",
            "game_id": 1,
            "winner": "Good",
            "reason": "DemonDead",
            "seats": [],
        });
        let line = serde_json::to_string(&v).unwrap();
        assert!(!line.contains('\n'));
        let parsed: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(parsed["event"], "game_end");
        assert_eq!(parsed["winner"], "Good");
    }
}
