//! Ratatui monitoring UI for the multi-agent harness.

use std::collections::HashSet;
use std::io::{self, Stdout};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, MouseButton,
    MouseEvent, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::{Frame, Terminal};

use crate::game::{Game, GameId, Phase, RoleAssignment, SeatId, StChoiceMode, StartOpts};
use crate::harness::action_log::{ActionKind, ActionLog, ActorLabel};
use crate::harness::agents::{
    agent_mcp_bin_ok, cycle_in_list, find_agent_mcp_bin, find_claude, find_grok,
    load_claude_models, resolve_agent_mcp_bin_for_display, AgentConfig, AgentPool, AgentRole,
    Backend, HarnessConfig, LineKind, LogLine,
};
use crate::harness::scheduler::{plan_ticks, wait_signature, SchedTarget};
use crate::harness::socket::SocketServer;
use crate::mcp_server::{self, SharedStore};
use crate::roles::{Character, CharacterType};
use crate::tools;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Focus {
    Setup,
    Monitor,
}

/// Which actions the feed shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FeedFilter {
    /// Everything, incl. info reads (dimmed).
    All,
    /// Only game-affecting actions and errors.
    GameOnly,
}

/// One setup-screen session pick: which [`Backend`] runs the seat and on which model.
/// A single struct (not parallel vectors) so backend and model can never desync, and
/// the model is always kept within its backend's list.
#[derive(Debug, Clone)]
struct SeatChoice {
    backend: Backend,
    model: String,
}

struct App {
    focus: Focus,
    player_count: usize,
    /// Per-session backend+model picks: index 0 = Host, 1..=N = players P0..P{N-1}.
    /// Kept in lockstep with `player_count` (+1 for the host row).
    seat_choices: Vec<SeatChoice>,
    /// Previewed role for each player seat (index i = P{i}), shown on the setup
    /// screen so models can be picked per role. Regenerated on count change and on
    /// reroll; passed to `start_game` as fixed assignments so the launched game
    /// matches exactly what was shown. Empty only if the preview draw failed.
    setup_roles: Vec<RoleAssignment>,
    /// Focused setup row: 0 = the player-count row; 1.. = Host, P0, P1, …
    setup_row: usize,
    selected_agent: usize,
    status: String,
    store: SharedStore,
    game_id: Option<u64>,
    host_token: Option<crate::auth::Token>,
    player_names: Vec<String>,
    socket: Option<SocketServer>,
    agents: Option<AgentPool>,
    /// Event-driven auto-advance: when on, the next turn is ticked as soon as all
    /// agents are idle (no fixed timer; a running agent is never skipped).
    auto_tick: bool,
    /// When the last tick was launched — shown as "last turn Ns ago" (staleness).
    last_tick: Instant,
    cfg: HarnessConfig,
    should_quit: bool,
    /// Lines scrolled up from the live tail (0 = stick to newest output) (#45).
    scroll_from_bottom: usize,
    /// Round-robin cursor for discussion/nomination turns in the scheduler (#60).
    /// Reset to 0 whenever the game enters a new phase/stage (see `stage_key`).
    tick_rotation: usize,
    /// Key of the phase/stage the last tick planned for; a change resets `tick_rotation`
    /// so discussion rounds start counting from the first speaker.
    stage_key: String,
    /// Signature of what the engine was waiting on last tick, for stall detection (#60).
    wait_sig: Option<String>,
    /// Consecutive cycles the engine has sat on `wait_sig`; drives host escalation (#60).
    stall: usize,
    /// Fingerprint of the last plan (targets + signature) for the host-retry brake.
    last_plan_key: String,
    /// Consecutive identical host-only plans with no state change; capped to stop
    /// an unbounded host retry loop.
    host_plan_repeats: usize,
    /// Agent indices whose stream shows full `[think]` blocks (default: collapsed).
    thinking_expanded: HashSet<usize>,
    /// Last drawn hit targets for mouse (board / stream / feed).
    hit_agents: Rect,
    hit_stream: Rect,
    /// Hit target of the action-feed pane.
    hit_feed: Rect,
    /// seq of the feed entry rendered on each visible feed row (for click-expand).
    feed_rows: Vec<Option<u64>>,
    /// Feed entries (by seq) whose full args/result are expanded.
    feed_expanded: HashSet<u64>,
    /// Central feed of every agent tool RPC (shared with the socket server) (#UI).
    action_log: Arc<ActionLog>,
    /// Per visible row of the left board pane: agent index to select on click
    /// (`None` = header / nom tracker / spacer — not clickable for selection).
    board_agent_rows: Vec<Option<usize>>,
    /// Labels of the agents targeted by the most recent scheduled tick (for the status bar).
    last_targets: Vec<String>,
    /// Rows scrolled up from the tail of the action feed (0 = live).
    feed_scroll: usize,
    /// Which actions the feed shows (all vs game-only).
    feed_filter: FeedFilter,
    /// Stable id for this TUI process (stamped on every results-log line).
    run_id: String,
    /// Public-log cursor already drained into the ranking results log.
    results_public_cursor: u64,
    /// True after `game_end` or `game_abort` has been written for the current game.
    results_terminal_logged: bool,
}

impl App {
    fn new() -> Self {
        let id = uuid::Uuid::new_v4();
        let work_root = PathBuf::from(format!("/tmp/botc-harness-{id}"));
        let mut cfg = HarnessConfig {
            grok_bin: find_grok(),
            claude_bin: find_claude(),
            agent_mcp_bin: find_agent_mcp_bin(),
            work_root: work_root.clone(),
            socket_path: work_root.join("engine.sock"),
            ..Default::default()
        };
        // Populate the setup picker from `grok models` (CLI default becomes selected).
        let models_note = cfg.load_models_from_grok();
        // Claude models are a curated static list (no `claude models` CLI).
        cfg.claude_models = load_claude_models();
        let mcp_ok = agent_mcp_bin_ok(&cfg);
        let mcp_path = resolve_agent_mcp_bin_for_display(&cfg);
        let status = if mcp_ok {
            format!(
                "↑/↓ row · ←/→ change · a=model to all · Enter launch · q quit | {models_note} | grok={} mcp={}",
                cfg.grok_bin.display(),
                mcp_path.display()
            )
        } else {
            format!(
                "MISSING botc-agent-mcp at {} — run: cargo build --bins",
                mcp_path.display()
            )
        };
        let player_count = 5;
        let mut app = Self {
            focus: Focus::Setup,
            player_count,
            // One pick per session (host + players), all starting grok on the CLI default.
            seat_choices: vec![
                SeatChoice {
                    backend: Backend::Grok,
                    model: cfg.model.clone(),
                };
                player_count + 1
            ],
            setup_roles: Vec::new(),
            setup_row: 0,
            selected_agent: 0,
            status,
            store: mcp_server::new_shared_store(),
            game_id: None,
            host_token: None,
            player_names: Vec::new(),
            socket: None,
            agents: None,
            auto_tick: false,
            last_tick: Instant::now(),
            cfg,
            should_quit: false,
            scroll_from_bottom: 0,
            tick_rotation: 0,
            stage_key: String::new(),
            wait_sig: None,
            stall: 0,
            last_plan_key: String::new(),
            host_plan_repeats: 0,
            thinking_expanded: HashSet::new(),
            hit_agents: Rect::default(),
            hit_stream: Rect::default(),
            hit_feed: Rect::default(),
            feed_rows: Vec::new(),
            feed_expanded: HashSet::new(),
            action_log: Arc::new(ActionLog::default()),
            board_agent_rows: Vec::new(),
            last_targets: Vec::new(),
            feed_scroll: 0,
            feed_filter: FeedFilter::All,
            run_id: uuid::Uuid::new_v4().to_string(),
            results_public_cursor: 0,
            results_terminal_logged: false,
        };
        // Draw an initial role assignment so the setup screen shows roles immediately,
        // then let the balancer pick which eligible models fill those roles — booting
        // straight into a balanced table instead of N copies of the CLI default (which
        // typically has no completed games and so balances nothing). No-op, leaving the
        // default picks in place, when nothing is eligible yet.
        app.reroll_roles();
        app.shuffle_models();
        app
    }

    /// Draw a fresh, valid role assignment for the current player count and stash it
    /// in `setup_roles`. Uses a throwaway game so the same engine bag logic that the
    /// real game will use produces the composition; the bag is then reassigned across
    /// seats so each model keeps a balanced eval record (team → role type → role; see
    /// [`crate::harness::balance`]). `launch()` replays these exact assignments.
    /// Leaves `status` untouched on success (keeps the boot note); only reports failure.
    fn reroll_roles(&mut self) {
        let names: Vec<String> = (0..self.player_count).map(|i| format!("P{i}")).collect();
        let created = match Game::create(names, rand::random()) {
            Ok(c) => c,
            Err(e) => {
                self.setup_roles.clear();
                self.status = format!("role preview failed: {e}");
                return;
            }
        };
        let mut g = created.game;
        if let Err(e) = g.start_game(&created.host_token, StartOpts::default()) {
            self.setup_roles.clear();
            self.status = format!("role preview failed: {e}");
            return;
        }
        // The engine dealt a valid composition; capture it as a bag (characters plus
        // any Drunk face) and the model sitting at each seat, then reassign the bag so
        // models play a balanced mix of teams/roles over time. Permuting the bag keeps
        // the composition intact, so the result is always a valid fixed assignment.
        let seats: Vec<SeatId> = g.seats.iter().map(|s| s.id).collect();
        let bag: Vec<(Character, Option<Character>)> = g
            .seats
            .iter()
            .map(|s| {
                (
                    s.true_character.expect("started game assigns every seat"),
                    s.believed_character,
                )
            })
            .collect();
        // Seat identity lookup (`seat_choices[0]` is the Host, players are `1..=N`).
        // Key by the composed node_key so balance history matches read_model_stats
        // (grok stays bare; claude → "claude:<model>").
        let node_keys: Vec<String> = seats
            .iter()
            .map(|id| {
                self.seat_choices
                    .get(id.0 as usize + 1)
                    .map(|c| crate::harness::balance::node_key(c.backend.as_str(), &c.model))
                    .unwrap_or_default()
            })
            .collect();
        let models: Vec<&str> = node_keys.iter().map(String::as_str).collect();
        let stats =
            crate::harness::balance::read_model_stats(&crate::harness::results_log::log_path());
        let mut rng = rand::thread_rng();
        self.setup_roles =
            crate::harness::balance::balanced_assignment(&seats, &models, &bag, &stats, &mut rng);
    }

    /// The models the balancer may CHOOSE from: everything offered in the pickers
    /// (grok's `grok models` + claude's curated list) that already has **≥1 completed
    /// game**. A model with no valid games has no record to balance — pick it by hand
    /// to give it a first game, after which it becomes eligible.
    fn eligible_models(
        &self,
        stats: &std::collections::HashMap<String, crate::harness::balance::ModelStats>,
    ) -> Vec<SeatChoice> {
        let mut out = Vec::new();
        for (backend, list) in [
            (Backend::Grok, &self.cfg.available_models),
            (Backend::Claude, &self.cfg.claude_models),
        ] {
            for m in list {
                if stats.contains_key(&crate::harness::balance::node_key(backend.as_str(), m)) {
                    out.push(SeatChoice {
                        backend,
                        model: m.clone(),
                    });
                }
            }
        }
        out
    }

    /// Let the harness **choose which models play**, given the current roles, so the
    /// eval corpus balances. It draws from [`Self::eligible_models`] — the *available*
    /// models that have a valid (completed) game — rather than merely permuting your
    /// picks, which would defeat the purpose. Having fewer eligible models than seats
    /// is normal, so a model may take several seats. Counterpart to
    /// [`Self::reroll_roles`] (`r`), which redraws the roles against whatever models
    /// are currently seated. The Host has no role and is left untouched.
    fn shuffle_models(&mut self) {
        if self.setup_roles.len() != self.player_count
            || self.seat_choices.len() < self.player_count + 1
        {
            self.status = "no role preview yet — press r first".into();
            return;
        }
        let stats =
            crate::harness::balance::read_model_stats(&crate::harness::results_log::log_path());
        let pool = self.eligible_models(&stats);
        if pool.is_empty() {
            self.status =
                "no available model has a completed game yet — pick models by hand to seed one"
                    .into();
            return;
        }
        let seat_chars: Vec<Character> =
            self.setup_roles.iter().map(|a| a.true_character).collect();
        let keys: Vec<String> = pool
            .iter()
            .map(|c| crate::harness::balance::node_key(c.backend.as_str(), &c.model))
            .collect();
        let key_refs: Vec<&str> = keys.iter().map(String::as_str).collect();
        let mut rng = rand::thread_rng();
        let pick = crate::harness::balance::select_balanced_models(
            &seat_chars,
            &key_refs,
            &stats,
            &mut rng,
        );
        for (i, &ci) in pick.iter().enumerate() {
            self.seat_choices[1 + i] = pool[ci].clone();
        }
        let used: std::collections::BTreeSet<&str> =
            pick.iter().map(|&ci| pool[ci].model.as_str()).collect();
        self.status = format!(
            "balancer picked {}/{} eligible models: {}",
            used.len(),
            pool.len(),
            used.into_iter().collect::<Vec<_>>().join(" · ")
        );
    }

    fn selected_thinking_expanded(&self) -> bool {
        self.thinking_expanded.contains(&self.selected_agent)
    }

    fn toggle_thinking_selected(&mut self) {
        if self.agents.is_none() {
            return;
        }
        let idx = self.selected_agent;
        if self.thinking_expanded.contains(&idx) {
            self.thinking_expanded.remove(&idx);
            self.status = format!("agent {idx}: thinking collapsed (h / click stream)");
        } else {
            self.thinking_expanded.insert(idx);
            self.status = format!("agent {idx}: thinking expanded (h / click stream)");
        }
        self.scroll_from_bottom = 0;
    }

    /// Scroll the selected agent stream by `delta` visual rows (positive = older / up).
    fn scroll_stream(&mut self, delta: i32) {
        if self.focus != Focus::Monitor || self.agents.is_none() {
            return;
        }
        if delta > 0 {
            self.scroll_from_bottom = self.scroll_from_bottom.saturating_add(delta as usize);
        } else {
            self.scroll_from_bottom = self.scroll_from_bottom.saturating_sub((-delta) as usize);
        }
    }

    fn on_mouse(&mut self, m: MouseEvent) {
        let x = m.column;
        let y = m.row;
        match m.kind {
            // Wheel / trackpad: scroll the stream or the action feed under the pointer.
            MouseEventKind::ScrollUp if point_in_rect(self.hit_stream, x, y) => {
                self.scroll_stream(3);
            }
            MouseEventKind::ScrollDown if point_in_rect(self.hit_stream, x, y) => {
                self.scroll_stream(-3);
            }
            MouseEventKind::ScrollUp if point_in_rect(self.hit_feed, x, y) => {
                self.feed_scroll = self.feed_scroll.saturating_add(3);
            }
            MouseEventKind::ScrollDown if point_in_rect(self.hit_feed, x, y) => {
                self.feed_scroll = self.feed_scroll.saturating_sub(3);
            }
            // Left click: select agent (board row), expand a feed action, or toggle thinking.
            MouseEventKind::Down(MouseButton::Left) => {
                if point_in_rect(self.hit_agents, x, y) {
                    // Row under the top border → board_agent_rows mapping (multi-line board).
                    let inner_y = y.saturating_sub(self.hit_agents.y.saturating_add(1)) as usize;
                    if let Some(Some(idx)) = self.board_agent_rows.get(inner_y).copied() {
                        self.selected_agent = idx;
                        self.scroll_from_bottom = 0;
                        self.status = format!("selected agent {idx}");
                    }
                } else if point_in_rect(self.hit_feed, x, y) {
                    // Row under the top border → visible feed row → entry seq.
                    let row = y.saturating_sub(self.hit_feed.y.saturating_add(1)) as usize;
                    if let Some(Some(seq)) = self.feed_rows.get(row) {
                        let seq = *seq;
                        if !self.feed_expanded.remove(&seq) {
                            self.feed_expanded.insert(seq);
                        }
                    }
                } else if point_in_rect(self.hit_stream, x, y) {
                    self.toggle_thinking_selected();
                }
            }
            // Ignore other mouse noise (drags, right-click, wheel outside panes).
            _ => {}
        }
    }

    /// Setup ←/→: adjust the focused row. Row 0 changes the player count (the
    /// per-seat model list grows/shrinks with it, new rows on the default model);
    /// rows 1.. cycle that session's model through `grok models`.
    fn setup_adjust(&mut self, delta: i32) {
        if self.setup_row == 0 {
            let pc = self.player_count as i32 + delta;
            self.player_count = pc.clamp(5, 15) as usize;
            self.seat_choices.resize(
                self.player_count + 1,
                SeatChoice {
                    backend: Backend::Grok,
                    model: self.cfg.model.clone(),
                },
            );
            // Roles are count-specific; redraw the assignment so the picker stays valid.
            self.reroll_roles();
            self.status = format!(
                "players: {}  ({} sessions incl. host)",
                self.player_count,
                self.player_count + 1
            );
        } else {
            let idx = self.setup_row - 1;
            let backend = self.seat_choices[idx].backend;
            let cur = self.seat_choices[idx].model.clone();
            // Cycle within the seat's *own* backend's model list.
            let next = {
                let list = self.cfg.models_for(backend);
                cycle_in_list(&cur, list, delta)
            };
            self.seat_choices[idx].model = next;
            let who = if idx == 0 {
                "Host".to_string()
            } else {
                format!("P{}", idx - 1)
            };
            self.status = format!(
                "{who}: [{}] {}  (b = backend · a = apply to all)",
                backend.as_str(),
                self.seat_choices[idx].model
            );
        }
    }

    fn on_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
            // Setup: ↑/↓ move the focused row (row 0 = player count; 1.. = one
            // model row per session), ←/→ change the focused row's value.
            KeyCode::Up | KeyCode::Char('k') if self.focus == Focus::Setup => {
                self.setup_row = self.setup_row.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') if self.focus == Focus::Setup => {
                // Rows: 0 (count) + player_count+1 model rows.
                self.setup_row = (self.setup_row + 1).min(self.player_count + 1);
            }
            KeyCode::Right | KeyCode::Char('l') | KeyCode::Char('m')
                if self.focus == Focus::Setup =>
            {
                self.setup_adjust(1);
            }
            KeyCode::Left | KeyCode::Char('h') | KeyCode::Char('M')
                if self.focus == Focus::Setup =>
            {
                self.setup_adjust(-1);
            }
            // Reroll the whole-table role assignment (rebuilds the bag).
            KeyCode::Char('r') if self.focus == Focus::Setup => {
                self.reroll_roles();
                if !self.setup_roles.is_empty() {
                    self.status = "rerolled roles".into();
                }
            }
            // Shuffle the picked models across seats, balanced against the CURRENT
            // roles — the inverse of `r` (which redraws roles against current models).
            KeyCode::Char('s') | KeyCode::Char('S') if self.focus == Focus::Setup => {
                self.shuffle_models();
            }
            // Cycle the focused seat's backend (grok ↔ claude); snap model to that
            // backend's default so the pick is always valid.
            KeyCode::Char('b') | KeyCode::Char('B') if self.focus == Focus::Setup => {
                if self.setup_row >= 1 {
                    let idx = self.setup_row - 1;
                    let nb = self.seat_choices[idx].backend.cycle(1);
                    let model = self.cfg.default_model(nb);
                    self.seat_choices[idx].backend = nb;
                    self.seat_choices[idx].model = model;
                    let who = if idx == 0 {
                        "Host".to_string()
                    } else {
                        format!("P{}", idx - 1)
                    };
                    self.status = format!(
                        "{who} backend: [{}] {}",
                        nb.as_str(),
                        self.seat_choices[idx].model
                    );
                }
            }
            // Apply the focused row's backend + model to every session.
            KeyCode::Char('a') if self.focus == Focus::Setup => {
                if self.setup_row >= 1 {
                    let src = self.seat_choices[self.setup_row - 1].clone();
                    for slot in self.seat_choices.iter_mut() {
                        *slot = src.clone();
                    }
                    self.status = format!("ALL sessions: [{}] {}", src.backend.as_str(), src.model);
                }
            }
            KeyCode::Enter if self.focus == Focus::Setup => self.launch(),
            KeyCode::Tab if self.agents.is_some() => {
                let n = self
                    .agents
                    .as_ref()
                    .map(|a| a.agents.len())
                    .unwrap_or(1)
                    .max(1);
                self.selected_agent = (self.selected_agent + 1) % n;
                self.scroll_from_bottom = 0;
            }
            KeyCode::BackTab if self.agents.is_some() => {
                let n = self
                    .agents
                    .as_ref()
                    .map(|a| a.agents.len())
                    .unwrap_or(1)
                    .max(1);
                self.selected_agent = (self.selected_agent + n - 1) % n;
                self.scroll_from_bottom = 0;
            }
            KeyCode::Char('t') if self.agents.is_some() => {
                self.auto_tick = !self.auto_tick;
                self.status = format!("auto_tick={}", self.auto_tick);
            }
            // Toggle the feed filter: all actions ↔ game-only.
            KeyCode::Char('f') if self.agents.is_some() => {
                self.feed_filter = match self.feed_filter {
                    FeedFilter::All => FeedFilter::GameOnly,
                    FeedFilter::GameOnly => FeedFilter::All,
                };
                self.status = match self.feed_filter {
                    FeedFilter::All => "feed: all actions (f = game-only)".into(),
                    FeedFilter::GameOnly => "feed: game actions only (f = all)".into(),
                };
            }
            // Expand/collapse [think] for the selected agent (default collapsed).
            // Same as left-click on the stream pane.
            KeyCode::Char('h') if self.agents.is_some() => {
                self.toggle_thinking_selected();
            }
            KeyCode::Char(' ') if self.agents.is_some() => {
                // Never skip a running agent: only advance when everything is idle.
                if self.any_agent_running() {
                    self.status =
                        "still running — advances automatically when this turn finishes".into();
                } else {
                    self.do_tick();
                }
            }
            // Stream scroll: keyboard and mouse wheel (over stream). 0 = live tail.
            KeyCode::PageUp if self.focus == Focus::Monitor => {
                self.scroll_stream(5);
            }
            KeyCode::PageDown if self.focus == Focus::Monitor => {
                self.scroll_stream(-5);
            }
            KeyCode::Up if self.focus == Focus::Monitor => {
                self.scroll_stream(1);
            }
            KeyCode::Down if self.focus == Focus::Monitor => {
                self.scroll_stream(-1);
            }
            KeyCode::Home if self.focus == Focus::Monitor => {
                // Jump to live tail.
                self.scroll_from_bottom = 0;
            }
            _ => {}
        }
    }

    fn launch(&mut self) {
        // #55: also gate on socket — a failed prior launch may have left socket=Some.
        if self.agents.is_some() || self.socket.is_some() {
            self.status = "Already launched — restart the TUI for a new table.".into();
            return;
        }
        // #50: refuse to launch without a real botc-agent-mcp binary.
        if !agent_mcp_bin_ok(&self.cfg) {
            let p = resolve_agent_mcp_bin_for_display(&self.cfg);
            self.status = format!(
                "botc-agent-mcp not found (looked for {}). Run: cargo build --bins",
                p.display()
            );
            return;
        }
        // #69: a claude seat needs the claude binary + a model from the claude list.
        if self
            .seat_choices
            .iter()
            .any(|c| c.backend == Backend::Claude)
        {
            let bin = &self.cfg.claude_bin;
            if !(bin.is_absolute() && bin.exists()) {
                self.status = format!(
                    "claude backend selected but `claude` was not found ({}). Install Claude Code or switch the seat to grok (b).",
                    bin.display()
                );
                return;
            }
            if let Some((slot, c)) = self.seat_choices.iter().enumerate().find(|(_, c)| {
                c.backend == Backend::Claude
                    && !self
                        .cfg
                        .models_for(Backend::Claude)
                        .iter()
                        .any(|m| m == &c.model)
            }) {
                let who = if slot == 0 {
                    "Host".to_string()
                } else {
                    format!("P{}", slot - 1)
                };
                self.status = format!(
                    "{who}: claude model '{}' not in the claude list — re-pick with ←/→.",
                    c.model
                );
                return;
            }
        }
        // Refresh resolved path so workdir config gets an absolute sibling if possible.
        self.cfg.agent_mcp_bin = find_agent_mcp_bin();

        self.player_names = (0..self.player_count).map(|i| format!("P{i}")).collect();
        let seed = self.cfg.seed.unwrap_or(42);
        let created = match Game::create(self.player_names.clone(), seed) {
            Ok(c) => c,
            Err(e) => {
                self.status = format!("create failed: {e}");
                return;
            }
        };

        let game_id = {
            let mut st = self.store.lock().unwrap();
            st.insert(created.game).0
        };

        match SocketServer::start_with_log(
            Arc::clone(&self.store),
            Arc::clone(&self.action_log),
            &self.cfg.socket_path,
        ) {
            Ok(s) => self.socket = Some(s),
            Err(e) => {
                self.status = format!("socket: {e}");
                return;
            }
        }

        let start_err = {
            let mut st = self.store.lock().unwrap();
            let g = st.get_mut(GameId(game_id)).unwrap();
            // Launch with exactly the roles shown on the setup screen. Fall back to a
            // fresh random bag only if the preview is missing/mismatched (shouldn't
            // happen — it's regenerated on every count change).
            let assignments = if self.setup_roles.len() == self.player_count {
                Some(self.setup_roles.clone())
            } else {
                None
            };
            tools::start_game(
                g,
                &created.host_token,
                StartOpts {
                    assignments,
                    st_choice_mode: if self.cfg.st_choice_mode == "random" {
                        StChoiceMode::Random
                    } else {
                        StChoiceMode::HostFirst
                    },
                    ..Default::default()
                },
            )
            .err()
        };
        if let Some(e) = start_err {
            self.status = format!("start_game: {e}");
            // #55/#58: roll back partial launch state (socket + work_root).
            self.abort_partial_launch();
            return;
        }

        self.game_id = Some(game_id);
        self.host_token = Some(created.host_token.clone());

        // Register token → actor labels so the action feed can name each caller.
        let mut labels = std::collections::HashMap::new();
        labels.insert(
            created.host_token.as_str().to_string(),
            ActorLabel {
                name: "Host".into(),
                seat: None,
                is_host: true,
            },
        );
        for (i, tok) in created.player_tokens.iter().enumerate() {
            labels.insert(
                tok.as_str().to_string(),
                ActorLabel {
                    name: format!("P{i}"),
                    seat: Some(i as u8),
                    is_host: false,
                },
            );
        }
        self.action_log.set_labels(labels);
        crate::dlog!(
            "LAUNCH game_id={game_id} players={} st_mode={} host_token={}… players_tokens={}",
            self.player_count,
            self.cfg.st_choice_mode,
            &created.host_token.as_str()[..8.min(created.host_token.as_str().len())],
            created.player_tokens.len()
        );

        // Per-session backend+model from the setup picker (index 0 = host, 1+i = P{i}).
        // Fall back to the default grok model if the picks are out of sync.
        let choice_for = |slot: usize| -> SeatChoice {
            self.seat_choices
                .get(slot)
                .cloned()
                .unwrap_or_else(|| SeatChoice {
                    backend: Backend::Grok,
                    model: self.cfg.model.clone(),
                })
        };
        let host_choice = choice_for(0);
        let mut configs = vec![AgentConfig {
            role: AgentRole::Host,
            display_name: "Storyteller".into(),
            token: created.host_token.as_str().to_string(),
            game_id,
            model: host_choice.model,
            backend: host_choice.backend,
        }];
        for (i, tok) in created.player_tokens.iter().enumerate() {
            let c = choice_for(1 + i);
            configs.push(AgentConfig {
                role: AgentRole::Player {
                    seat: SeatId(i as u8),
                },
                display_name: self.player_names[i].clone(),
                token: tok.as_str().to_string(),
                game_id,
                model: c.model,
                backend: c.backend,
            });
        }

        match AgentPool::prepare(&self.cfg, configs) {
            Ok(mut pool) => {
                // Ranking corpus: log the full seat↔model↔role table once at start.
                {
                    let st = self.store.lock().unwrap();
                    if let Some(g) = st.get(GameId(game_id)) {
                        let agent_cfgs: Vec<_> =
                            pool.agents.iter().map(|a| a.config.clone()).collect();
                        crate::harness::results_log::log_game_start(
                            &self.run_id,
                            game_id,
                            g,
                            &agent_cfgs,
                            seed,
                            &self.cfg.st_choice_mode,
                        );
                        // Catch any public events already emitted during start_game.
                        self.results_public_cursor =
                            crate::harness::results_log::drain_public_events(
                                &self.run_id,
                                game_id,
                                g,
                                &agent_cfgs,
                                0,
                            );
                    }
                }
                self.results_terminal_logged = false;
                match pool.kickoff_all(self.player_count) {
                    Ok(spawned) => {
                        self.status = format!(
                            "Game {game_id} · kicked {spawned}/{} · Space=tick · click stream=think · wheel=scroll · q=quit",
                            self.player_count + 1
                        );
                        self.auto_tick = true;
                        self.last_tick = Instant::now();
                        self.focus = Focus::Monitor;
                        self.scroll_from_bottom = 0;
                        self.agents = Some(pool);
                        crate::dlog!("LAUNCH kickoff spawned {spawned}/{}", self.player_count + 1);
                    }
                    Err(e) => {
                        // Partial pool still owns workdirs — keep it so shutdown/stop_all cleans up.
                        self.status = format!("kickoff: {e}");
                        self.agents = Some(pool);
                        crate::dlog!("LAUNCH kickoff ERROR {e}");
                    }
                }
            }
            Err(e) => {
                self.status = format!("prepare: {e}");
                // #55/#58: socket + partial work_root without agents.
                self.abort_partial_launch();
            }
        }
    }

    /// Agent configs for results logging (empty if no pool).
    fn agent_configs_for_results(&self) -> Vec<AgentConfig> {
        self.agents
            .as_ref()
            .map(|p| p.agents.iter().map(|a| a.config.clone()).collect())
            .unwrap_or_default()
    }

    /// Per-agent usage snapshot for results log / game_end.
    fn agent_usage_for_results(&self) -> Vec<(String, crate::harness::agents::AgentUsage)> {
        let Some(pool) = self.agents.as_ref() else {
            return Vec::new();
        };
        pool.agents
            .iter()
            .map(|a| {
                let label = match a.config.role {
                    AgentRole::Host => "Host".to_string(),
                    AgentRole::Player { seat } => format!("P{}", seat.0),
                };
                (label, a.usage.lock().unwrap().clone())
            })
            .collect()
    }

    /// Drain ranking-relevant public events; if the game just ended, write `game_end` once.
    fn results_poll(&mut self) {
        let Some(gid) = self.game_id else {
            return;
        };
        let agent_cfgs = self.agent_configs_for_results();
        let usage = self.agent_usage_for_results();
        let st = self.store.lock().unwrap();
        let Some(g) = st.get(GameId(gid)) else {
            return;
        };
        self.results_public_cursor = crate::harness::results_log::drain_public_events(
            &self.run_id,
            gid,
            g,
            &agent_cfgs,
            self.results_public_cursor,
        );
        if !self.results_terminal_logged && matches!(g.phase, Phase::Ended { .. }) {
            crate::harness::results_log::log_game_end(&self.run_id, gid, g, &agent_cfgs, &usage);
            self.results_terminal_logged = true;
            crate::dlog!("RESULTS game_end logged run_id={}", self.run_id);
        }
    }

    /// Tear down socket + work_root after a launch that never got a live AgentPool (#55/#58).
    fn abort_partial_launch(&mut self) {
        if let Some(s) = self.socket.take() {
            s.stop();
        }
        if self.cfg.work_root.exists() {
            let _ = std::fs::remove_dir_all(&self.cfg.work_root);
        }
    }

    /// One turn-routed tick (#60): plan from engine state, then tick only the
    /// agent(s) the game is waiting on. Stops (and disarms auto-tick) at `Ended`.
    fn do_tick(&mut self) {
        let Some(gid) = self.game_id else {
            return;
        };
        // Read all we need under one short lock: the plan, the prompt context,
        // and whether the game is over. Do NOT hold the lock while spawning grok.
        let (plan, summary, hint, ended, sig, stall, state_dbg) = {
            let st = self.store.lock().unwrap();
            let Some(g) = st.get(GameId(gid)) else {
                return;
            };
            // New phase/stage => restart the turn rotation (round counting depends
            // on rotation starting at 0 when discussion/nominations begin).
            let key = stage_key_of(g);
            if key != self.stage_key {
                crate::dlog!("STAGE {} -> {} (rotation reset)", self.stage_key, key);
                self.stage_key = key;
                self.tick_rotation = 0;
            }
            // Stall detection: same wait as last cycle => it's not progressing.
            let sig = wait_signature(g);
            let stall = if sig.is_some() && sig == self.wait_sig {
                self.stall + 1
            } else {
                0
            };
            let plan = plan_ticks(g, self.tick_rotation, stall);
            let (summary, hint) = game_summary_and_hint(g);
            let state_dbg = format!(
                "phase={:?} pending_host={:?} pending_night={:?} nom={:?}",
                g.phase,
                g.pending_host.as_ref().map(|p| p.kind_str()),
                g.pending_night.as_ref().map(|w| w.seat.0),
                g.current_nomination
                    .as_ref()
                    .map(|n| (n.by.0, n.target.0, n.votes.len())),
            );
            (
                plan,
                summary,
                hint,
                matches!(g.phase, Phase::Ended { .. }),
                sig,
                stall,
                state_dbg,
            )
        };
        self.wait_sig = sig.clone();
        self.stall = stall;

        let plan_dbg: Vec<String> = plan.iter().map(target_label).collect();
        crate::dlog!(
            "TICK rotation={} sig={:?} stall={} {} plan=[{}]",
            self.tick_rotation,
            sig,
            stall,
            state_dbg,
            plan_dbg.join(", ")
        );

        // Always drain ranking events (deaths / noms) even on the end tick.
        self.results_poll();

        if ended {
            self.auto_tick = false;
            self.status =
                "Game ended — agents idle. Press q to quit or Tab to review streams.".into();
            crate::dlog!("TICK -> game ended, auto_tick off");
            return;
        }

        // Host-retry brake: a host-only fallback plan that repeats with no state
        // change means the host agent runs but never makes the required call.
        // Without a cap, event-driven auto would re-spawn it forever (token burn).
        let host_only = plan.iter().all(|t| matches!(t, SchedTarget::Host(_)));
        let plan_key = format!("{}|{sig:?}", plan_dbg.join(","));
        if host_only && plan_key == self.last_plan_key {
            self.host_plan_repeats += 1;
        } else {
            self.host_plan_repeats = 0;
        }
        self.last_plan_key = plan_key;
        const HOST_RETRY_CAP: usize = 5;
        if host_only && self.host_plan_repeats >= HOST_RETRY_CAP {
            self.auto_tick = false;
            self.status = format!(
                "auto-advance stopped: host repeated '{}' {HOST_RETRY_CAP}x without progress (see debug log). t=resume",
                self.last_targets.join(",")
            );
            crate::dlog!(
                "TICK -> host plan repeated {HOST_RETRY_CAP}x with no progress, auto_tick disabled"
            );
            return;
        }

        self.last_targets = plan_dbg;
        self.last_tick = Instant::now();
        if let Some(pool) = self.agents.as_mut() {
            match pool.tick_scheduled(&plan, &summary, &hint) {
                Ok(spawned) => {
                    self.status = format!(
                        "Turn tick → {}: {spawned} of {} spawned.",
                        self.last_targets.join(", "),
                        plan.len()
                    );
                    crate::dlog!("TICK -> tick_scheduled spawned {spawned}/{}", plan.len());
                    if spawned > 0 {
                        // Consume the turn slot only when someone actually ran —
                        // a failed spawn must not silently skip a speaker.
                        self.tick_rotation = self.tick_rotation.wrapping_add(1);
                    } else {
                        // Event-driven auto would re-fire immediately while idle; if a
                        // tick spawns nothing (broken grok / all errored), stop auto to
                        // avoid a hot retry loop. Re-enable with `t` after fixing.
                        self.auto_tick = false;
                        self.status =
                            "auto-advance stopped: tick spawned no agent (check grok / debug log). t=resume".into();
                        crate::dlog!("TICK -> 0 spawned, auto_tick disabled");
                    }
                }
                Err(e) => {
                    self.auto_tick = false;
                    self.status = format!("tick error (auto stopped): {e}");
                    crate::dlog!("TICK -> tick_scheduled ERROR {e}, auto_tick disabled");
                }
            }
        }
    }

    /// Concise phase label for the status bar (`Night 1`, `Day 2·noms`, `Ended: Good`).
    fn phase_label(&self) -> String {
        let Some(gid) = self.game_id else {
            return "Lobby".into();
        };
        let st = self.store.lock().unwrap();
        let Some(g) = st.get(GameId(gid)) else {
            return "—".into();
        };
        match &g.phase {
            Phase::Lobby => "Lobby".into(),
            Phase::FirstNight { .. } => "Night 1".into(),
            Phase::Night { night, .. } => format!("Night {night}"),
            Phase::Day { day, stage } => {
                let s = match stage {
                    crate::game::DayStage::Discussion => "disc",
                    crate::game::DayStage::Nominations => "noms",
                };
                format!("Day {day}·{s}")
            }
            Phase::Ended { winner, .. } => format!("Ended: {winner:?}"),
        }
    }

    /// True if any agent's Grok child is currently running.
    fn any_agent_running(&self) -> bool {
        self.agents
            .as_ref()
            .map(|p| p.agents.iter().any(|a| *a.running.lock().unwrap()))
            .unwrap_or(false)
    }

    /// Labels of agents whose Grok child is currently running.
    fn running_labels(&self) -> Vec<String> {
        let Some(pool) = self.agents.as_ref() else {
            return Vec::new();
        };
        pool.agents
            .iter()
            .filter(|a| *a.running.lock().unwrap())
            .map(|a| match a.config.role {
                AgentRole::Host => "Host".into(),
                AgentRole::Player { seat } => format!("P{}", seat.0),
            })
            .collect()
    }

    /// Game-progress spans for the top bar: phase · whose turn · tick mode · running.
    fn status_spans(&self) -> Vec<Span<'static>> {
        let phase = self.phase_label();
        let running = self.running_labels();
        let turn = if self.last_targets.is_empty() {
            "—".to_string()
        } else {
            self.last_targets.join(",")
        };
        let ago = self.last_tick.elapsed().as_secs();
        let tick = if self.auto_tick {
            "auto (on turn end)".to_string()
        } else {
            "manual (Space)".to_string()
        };
        let mut spans = vec![
            Span::styled(
                format!(" {phase} "),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("· turn "),
            Span::styled(format!("{turn} "), Style::default().fg(Color::Yellow)),
            Span::raw("· "),
            Span::styled(
                format!("{tick} "),
                if self.auto_tick {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default().fg(Color::Yellow)
                },
            ),
            Span::raw("· "),
        ];
        if running.is_empty() {
            spans.push(Span::styled(
                format!("idle · last turn {ago}s ago"),
                Style::default().fg(Color::DarkGray),
            ));
        } else {
            spans.push(Span::styled(
                format!("▶ running: {}", running.join(",")),
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            ));
        }
        spans
    }

    /// Agent stream for the selected seat: thinking collapsed by default so game
    /// actions (text / tools / errors) stay visible; `h` expands `[think]` blocks.
    fn agent_log_lines(&self) -> Vec<Line<'static>> {
        let Some(pool) = self.agents.as_ref() else {
            return Vec::new();
        };
        if pool.agents.is_empty() {
            return Vec::new();
        }
        let idx = self.selected_agent.min(pool.agents.len() - 1);
        let agent = &pool.agents[idx];
        let log = agent.log.lock().unwrap();
        let start = log.len().saturating_sub(600);
        stream_lines(&log[start..], self.thinking_expanded.contains(&idx))
    }

    fn shutdown(&mut self) {
        // Ranking: if the game never reached Ended, record an abort before we
        // tear down the store/pool (so we still have models + grimoire).
        if let Some(gid) = self.game_id {
            if !self.results_terminal_logged {
                let agent_cfgs = self.agent_configs_for_results();
                let usage = self.agent_usage_for_results();
                let st = self.store.lock().unwrap();
                let g = st.get(GameId(gid));
                // Final drain of any unlogged public events.
                if let Some(game) = g {
                    self.results_public_cursor = crate::harness::results_log::drain_public_events(
                        &self.run_id,
                        gid,
                        game,
                        &agent_cfgs,
                        self.results_public_cursor,
                    );
                    if matches!(game.phase, Phase::Ended { .. }) {
                        crate::harness::results_log::log_game_end(
                            &self.run_id,
                            gid,
                            game,
                            &agent_cfgs,
                            &usage,
                        );
                    } else {
                        crate::harness::results_log::log_game_abort(
                            &self.run_id,
                            gid,
                            Some(game),
                            &agent_cfgs,
                            "tui_quit",
                            &usage,
                        );
                    }
                } else {
                    crate::harness::results_log::log_game_abort(
                        &self.run_id,
                        gid,
                        None,
                        &agent_cfgs,
                        "tui_quit",
                        &usage,
                    );
                }
                self.results_terminal_logged = true;
                crate::dlog!(
                    "RESULTS terminal event logged on shutdown run_id={}",
                    self.run_id
                );
            }
        }

        // Stop agents first (kills children + removes work root with tokens).
        if let Some(mut pool) = self.agents.take() {
            pool.stop_all();
        }
        // Then stop socket (non-blocking accept; inode-safe remove).
        if let Some(s) = self.socket.take() {
            s.stop();
        }
        // #58: even if agents never built (failed launch), remove work_root/tokens.
        if self.cfg.work_root.exists() {
            let _ = std::fs::remove_dir_all(&self.cfg.work_root);
        }
    }
}

/// Stable key for the current phase/stage; a change means the turn rotation restarts.
/// Includes the living count so a mid-stage death (Slayer shot) restarts the round
/// with the current roster instead of mis-indexing speakers/rounds.
fn stage_key_of(g: &Game) -> String {
    let living = g.seats.iter().filter(|s| s.alive).count();
    match &g.phase {
        Phase::Lobby => "lobby".into(),
        Phase::FirstNight { .. } => "night1".into(),
        Phase::Night { night, .. } => format!("night{night}"),
        Phase::Day { day, stage } => format!(
            "day{day}-{}-{living}",
            match stage {
                crate::game::DayStage::Discussion => "disc",
                crate::game::DayStage::Nominations => "noms",
            }
        ),
        Phase::Ended { .. } => "ended".into(),
    }
}

/// Readable one-line rendering of a public event (for agent prompts, not Debug).
fn fmt_public_event(e: &crate::comms::PublicEvent) -> String {
    use crate::comms::PublicEvent::*;
    // Each event MUST be a single line: the snapshot lists one event per line, so a
    // message with embedded newlines would blur where one speaker ends and the next
    // begins — which makes agents think a message was "cut off" or reorder events.
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

/// Public snapshot + host hint string for tick prompts, computed from a locked game.
/// Free function so the scheduler can build it under the store lock without a second lock.
fn game_summary_and_hint(g: &Game) -> (String, String) {
    let phase = format!("{:?}", g.phase);
    // Player-facing roster: use PUBLICLY-known alive so a night kill isn't leaked
    // into a later night-order agent's prompt before the dawn announcement.
    let living: Vec<_> = g
        .seats
        .iter()
        .filter(|s| g.seat_publicly_alive(s))
        .map(|s| format!("P{}", s.id.0))
        .collect();
    let dead: Vec<_> = g
        .seats
        .iter()
        .filter(|s| !g.seat_publicly_alive(s))
        .map(|s| format!("P{}", s.id.0))
        .collect();
    let recent: Vec<_> = g
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
    let hint = format!(
        "pending_night={} pending_host={:?}",
        g.pending_night.is_some(),
        g.pending_host.as_ref().map(|p| p.kind_str())
    );
    (summary, hint)
}

/// Label a scheduler target for the status bar.
fn target_label(t: &SchedTarget) -> String {
    match t {
        SchedTarget::Host(_) => "Host".into(),
        SchedTarget::Player { seat, .. } => format!("P{}", seat.0),
    }
}

/// Stable per-agent colour (host magenta; players cycle a palette by seat).
fn actor_color(is_host: bool, seat: Option<u8>) -> Color {
    if is_host {
        return Color::Magenta;
    }
    const PAL: [Color; 6] = [
        Color::Cyan,
        Color::Green,
        Color::Yellow,
        Color::Blue,
        Color::LightRed,
        Color::LightGreen,
    ];
    PAL[(seat.unwrap_or(0) as usize) % PAL.len()]
}

/// `say` / `st_announce` text is never truncated in the feed — it always wraps
/// in full. Other tools keep a short inline summary on the header row.
fn is_speech_tool(tool: &str) -> bool {
    matches!(tool, "say" | "st_announce")
}

fn summary_style(kind: ActionKind) -> Style {
    if kind == ActionKind::Game {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    }
}

/// Chunk `text` into indented feed rows at `width` (pane inner width).
fn feed_chunk_lines(text: &str, width: usize, style: Style) -> Vec<Line<'static>> {
    let indent = "      ";
    let body_w = width.saturating_sub(indent.len()).max(8);
    let chars: Vec<char> = text.chars().collect();
    chars
        .chunks(body_w)
        .map(|c| {
            Line::from(Span::styled(
                format!("{indent}{}", c.iter().collect::<String>()),
                style,
            ))
        })
        .collect()
}

/// Header row: time · actor · tool · status. Speech tools leave the quote off the
/// header (it wraps in full below). Other tools put the short summary inline.
fn feed_line(e: &crate::harness::action_log::ActionEntry) -> Line<'static> {
    let ac = actor_color(e.actor.is_host, e.actor.seat);
    let (tool_style, marker) = match e.kind {
        ActionKind::Game => (
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
            "▶ ",
        ),
        ActionKind::Info => (Style::default().fg(Color::DarkGray), "  "),
        ActionKind::Meta => (Style::default().fg(Color::Gray), "  "),
    };
    let mut spans = vec![
        Span::styled(
            format!("{:>4}s ", e.secs),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(
            format!("{:<5}", e.actor.name),
            Style::default().fg(ac).add_modifier(Modifier::BOLD),
        ),
        Span::styled(marker, tool_style),
        Span::styled(e.tool.clone(), tool_style),
    ];
    if !is_speech_tool(&e.tool) && !e.summary.is_empty() {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(e.summary.clone(), summary_style(e.kind)));
    }
    if e.ok {
        spans.push(Span::styled("  ✓", Style::default().fg(Color::Green)));
    } else {
        spans.push(Span::styled(
            format!("  ✗ {}", e.error.as_deref().unwrap_or("")),
            Style::default().fg(Color::Red),
        ));
    }
    Line::from(spans)
}

/// Full `say` / `st_announce` quote, wrapped — always shown (not gated on expand).
fn feed_speech_lines(
    e: &crate::harness::action_log::ActionEntry,
    width: usize,
) -> Vec<Line<'static>> {
    if e.summary.is_empty() {
        return Vec::new();
    }
    feed_chunk_lines(&e.summary, width, summary_style(e.kind))
}

/// Expanded-only detail: args JSON, then result / error. Speech text is already
/// visible above and is not repeated here.
fn feed_detail_lines(
    e: &crate::harness::action_log::ActionEntry,
    width: usize,
) -> Vec<Line<'static>> {
    use crate::harness::action_log::clip_chars;
    const MAX_DETAIL_ROWS: usize = 16;
    let indent = "      ";
    let mut out = Vec::new();
    out.extend(feed_chunk_lines(
        &format!("args: {}", e.args),
        width,
        Style::default().fg(Color::Gray),
    ));
    if let Some(err) = &e.error {
        out.extend(feed_chunk_lines(
            &format!("error: {err}"),
            width,
            Style::default().fg(Color::Red),
        ));
    } else if let Some(res) = &e.result {
        out.extend(feed_chunk_lines(
            &format!("result: {}", clip_chars(res, 600)),
            width,
            Style::default().fg(Color::DarkGray),
        ));
    }
    if out.len() > MAX_DETAIL_ROWS {
        out.truncate(MAX_DETAIL_ROWS);
        out.push(Line::from(Span::styled(
            format!("{indent}… (truncated)"),
            Style::default().fg(Color::DarkGray),
        )));
    }
    out
}

/// Draw the global action feed (all agents), tail-anchored with `feed_scroll`.
/// Rows are pre-clipped (no wrapping) so every visible row maps to exactly one
/// entry — that mapping (`app.feed_rows`) makes rows click-expandable.
fn draw_action_feed(f: &mut Frame, area: Rect, app: &mut App) {
    let inner_h = area.height.saturating_sub(2).max(1) as usize;
    let inner_w = area.width.saturating_sub(2).max(8) as usize;
    // Game-only pulls a wider window so real actions aren't starved by info reads.
    let pull = match app.feed_filter {
        FeedFilter::All => inner_h + app.feed_scroll + 50,
        FeedFilter::GameOnly => 1500,
    };
    let entries: Vec<_> = app
        .action_log
        .recent(pull)
        .into_iter()
        .filter(|e| match app.feed_filter {
            FeedFilter::All => true,
            FeedFilter::GameOnly => e.kind == ActionKind::Game || !e.ok,
        })
        .collect();

    // Flatten entries into visual rows: header (▸/▾) + full speech text always,
    // plus args/result when expanded. Every row of a block maps to that seq so
    // any click toggles expand/collapse.
    let mut rows: Vec<(Line<'static>, Option<u64>)> = Vec::new();
    for e in &entries {
        let expanded = app.feed_expanded.contains(&e.seq);
        let marker = if expanded { "▾" } else { "▸" };
        let mut line = feed_line(e);
        line.spans.insert(
            0,
            Span::styled(format!("{marker} "), Style::default().fg(Color::DarkGray)),
        );
        rows.push((line, Some(e.seq)));
        // say / st_announce: full quote always, never truncated (may wrap).
        if is_speech_tool(&e.tool) {
            for l in feed_speech_lines(e, inner_w) {
                rows.push((l, Some(e.seq)));
            }
        }
        if expanded {
            for l in feed_detail_lines(e, inner_w) {
                rows.push((l, Some(e.seq)));
            }
        }
    }
    if rows.is_empty() {
        let msg = match app.feed_filter {
            FeedFilter::All => "no actions yet — agents haven't called any tools",
            FeedFilter::GameOnly => "no game actions yet (f = show all)",
        };
        rows.push((
            Line::from(Span::styled(msg, Style::default().fg(Color::DarkGray))),
            None,
        ));
    }

    // Tail-anchored window over the visual rows.
    let end = rows.len().saturating_sub(app.feed_scroll);
    let start = end.saturating_sub(inner_h);
    let window = &rows[start..end];
    let lines: Vec<Line> = window.iter().map(|(l, _)| l.clone()).collect();
    app.feed_rows = window.iter().map(|(_, seq)| *seq).collect();
    app.hit_feed = area;

    let filt = match app.feed_filter {
        FeedFilter::All => "all",
        FeedFilter::GameOnly => "game-only",
    };
    let tail = if app.feed_scroll == 0 {
        "·live"
    } else {
        "·scroll"
    };
    let title = format!(
        "actions · {filt} · {} total · click=expand f=filter {tail}",
        app.action_log.len()
    );
    let p = Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(title));
    f.render_widget(p, area);
}

/// RAII guard: restores terminal raw-mode / alternate screen on Drop or panic (#49).
struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    restored: bool,
}

impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        // #57: enable raw mode first, then finish setup under a restore-on-err path
        // so a later failure cannot leave the terminal in raw mode without a guard.
        enable_raw_mode()?;
        match Self::enter_after_raw() {
            Ok(g) => Ok(g),
            Err(e) => {
                let _ = disable_raw_mode();
                let mut out = io::stdout();
                let _ = execute!(out, DisableMouseCapture, LeaveAlternateScreen);
                Err(e)
            }
        }
    }

    fn enter_after_raw() -> io::Result<Self> {
        let mut stdout = io::stdout();
        // Capture mouse so the terminal delivers wheel/trackpad as Event::Mouse
        // (which we ignore) instead of synthesizing Enter/Space/arrow keypresses.
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;
        Ok(Self {
            terminal,
            restored: false,
        })
    }

    fn terminal_mut(&mut self) -> &mut Terminal<CrosstermBackend<Stdout>> {
        &mut self.terminal
    }

    fn restore(&mut self) {
        if self.restored {
            return;
        }
        self.restored = true;
        let _ = disable_raw_mode();
        let _ = execute!(
            self.terminal.backend_mut(),
            DisableMouseCapture,
            LeaveAlternateScreen
        );
        let _ = self.terminal.show_cursor();
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        self.restore();
    }
}

fn install_panic_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Best-effort restore before printing the panic (stdout may be alt screen).
        let _ = disable_raw_mode();
        let mut out = io::stdout();
        let _ = execute!(out, DisableMouseCapture, LeaveAlternateScreen);
        prev(info);
    }));
}

pub fn run_tui() -> io::Result<()> {
    let log_path = crate::harness::debug_log::log_path();
    crate::harness::debug_log::init(&log_path);
    let results_path = crate::harness::results_log::init();
    install_panic_hook();
    let mut guard = TerminalGuard::enter()?;
    let mut app = App::new();
    crate::harness::results_log::set_run_id(&app.run_id);
    app.status = format!(
        "{}  · debug: {log_path}  · results: {}",
        app.status,
        results_path.display()
    );
    crate::dlog!(
        "run_tui: grok={} model={} agent_mcp={} results={}",
        app.cfg.grok_bin.display(),
        app.cfg.model,
        app.cfg.agent_mcp_bin.display(),
        results_path.display()
    );

    let result = loop {
        guard.terminal_mut().draw(|f| draw(f, &mut app))?;
        if app.should_quit {
            break Ok(());
        }
        // Event-driven auto-advance: tick the next turn as soon as every agent is
        // idle (a running agent is never skipped; there is no fixed timer).
        if app.auto_tick && app.agents.is_some() && !app.any_agent_running() {
            app.do_tick();
        } else if app.agents.is_some() && app.game_id.is_some() && !app.any_agent_running() {
            // Between turns (manual mode / idle), still drain deaths & noms into
            // the ranking log without spawning agents.
            app.results_poll();
        }
        // Drain the whole queue so scroll floods don't lag behind real keys.
        while event::poll(Duration::from_millis(0))? {
            match event::read()? {
                Event::Key(key) => {
                    // Press only — ignore Release/Repeat.
                    if key.kind == KeyEventKind::Press {
                        app.on_key(key.code);
                    }
                }
                Event::Mouse(m) => app.on_mouse(m),
                Event::Resize(_, _) | Event::FocusGained | Event::FocusLost | Event::Paste(_) => {}
            }
        }
        // Idle wait so we don't busy-spin when the queue is empty.
        let _ = event::poll(Duration::from_millis(200));
    };

    app.shutdown();
    guard.restore();
    result
}

fn point_in_rect(r: Rect, x: u16, y: u16) -> bool {
    x >= r.x && x < r.x.saturating_add(r.width) && y >= r.y && y < r.y.saturating_add(r.height)
}

fn draw(f: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(3),
        ])
        .split(f.area());

    let mut title_spans = vec![Span::styled(
        " botc-tui ",
        Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    )];
    match app.focus {
        Focus::Setup => title_spans.push(Span::raw(format!(
            " players={} sessions={}  [SETUP]",
            app.player_count,
            app.player_count + 1,
        ))),
        // Live game-progress: phase · whose turn · tick mode · what's running.
        Focus::Monitor => {
            title_spans.push(Span::raw(" "));
            title_spans.extend(app.status_spans());
        }
    }
    let title = Paragraph::new(Line::from(title_spans)).block(
        Block::default()
            .borders(Borders::ALL)
            .title("Trouble Brewing · multi-agent monitor"),
    );
    f.render_widget(title, chunks[0]);

    match app.focus {
        Focus::Setup => {
            app.hit_agents = Rect::default();
            app.hit_stream = Rect::default();
            draw_setup(f, chunks[1], app);
        }
        Focus::Monitor => draw_monitor(f, chunks[1], app),
    }

    let status = Paragraph::new(app.status.as_str())
        .block(Block::default().borders(Borders::ALL).title("status"))
        .wrap(Wrap { trim: true });
    f.render_widget(status, chunks[2]);
}

/// Setup-screen label for a previewed seat role. Drunk shows both its true role and
/// the Townsfolk it believes it is, since the operator picks models on the true role.
fn role_label(a: &RoleAssignment) -> String {
    match a.believed_character {
        Some(face) => format!(
            "{} (as {})",
            a.true_character.display_name(),
            face.display_name()
        ),
        None => a.true_character.display_name().to_string(),
    }
}

/// Colour a role by team so the operator can see the evil seats at a glance.
fn role_style(c: Character) -> Style {
    match c.character_type() {
        CharacterType::Demon => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        CharacterType::Minion => Style::default().fg(Color::Red),
        CharacterType::Outsider => Style::default().fg(Color::Yellow),
        CharacterType::Townsfolk => Style::default().fg(Color::Green),
    }
}

fn draw_setup(f: &mut Frame, area: Rect, app: &App) {
    let mcp = resolve_agent_mcp_bin_for_display(&app.cfg);
    let mcp_note = if agent_mcp_bin_ok(&app.cfg) {
        "ok"
    } else {
        "MISSING — cargo build --bins"
    };
    let focused = Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(Color::DarkGray);

    let mut lines: Vec<Line> = Vec::new();

    // Row 0: player count.
    let count_mark = if app.setup_row == 0 { "▶ " } else { "  " };
    lines.push(Line::from(Span::styled(
        format!(
            "{count_mark}Players: {}    →  {} headless agent sessions (host + players)",
            app.player_count,
            app.player_count + 1
        ),
        if app.setup_row == 0 {
            focused
        } else {
            Style::default()
        },
    )));
    lines.push(Line::raw(""));

    // One row per session: Host, P0, P1, … each showing seat · role · model.
    lines.push(Line::from(Span::styled(
        "  Seat · role · [backend] model   (←/→ model · b backend · a apply-all · r roles→models · s models→roles)",
        dim,
    )));
    for slot in 0..=app.player_count {
        let row = slot + 1;
        let who = if slot == 0 {
            "Host".to_string()
        } else {
            format!("P{}", slot - 1)
        };
        let (backend, model) = app
            .seat_choices
            .get(slot)
            .map(|c| (c.backend, c.model.clone()))
            .unwrap_or((Backend::Grok, app.cfg.model.clone()));
        let list = app.cfg.models_for(backend);
        let known = list.iter().any(|m| m == &model);
        let mark = if app.setup_row == row { "▶ " } else { "  " };
        let mut spans = vec![Span::styled(
            format!("{mark}{who:<5} "),
            if app.setup_row == row {
                focused
            } else {
                Style::default()
            },
        )];
        // Role column: Host runs the game; players show their previewed character.
        let (role_text, role_st) = if slot == 0 {
            ("Storyteller".to_string(), dim)
        } else if let Some(a) = app.setup_roles.get(slot - 1) {
            (role_label(a), role_style(a.true_character))
        } else {
            ("—".to_string(), dim)
        };
        spans.push(Span::styled(format!("{role_text:<26}"), role_st));
        spans.push(Span::styled(format!("[{}] ", backend.as_str()), dim));
        spans.push(Span::styled(
            model.clone(),
            if app.setup_row == row {
                focused
            } else {
                Style::default().fg(actor_color(slot == 0, slot.checked_sub(1).map(|s| s as u8)))
            },
        ));
        if !known && !list.is_empty() {
            spans.push(Span::styled(
                format!("  (not in {} models)", backend.as_str()),
                dim,
            ));
        }
        lines.push(Line::from(spans));
    }

    lines.push(Line::raw(""));
    // Show the model list for the focused seat's backend (grok vs claude).
    let focus_backend = app
        .setup_row
        .checked_sub(1)
        .and_then(|i| app.seat_choices.get(i))
        .map(|c| c.backend)
        .unwrap_or(Backend::Grok);
    let focus_models = app.cfg.models_for(focus_backend);
    lines.push(Line::from(Span::styled(
        format!(
            "  {} models: {}",
            focus_backend.as_str(),
            if focus_models.is_empty() {
                "(none found)".to_string()
            } else {
                focus_models.join(" · ")
            }
        ),
        dim,
    )));
    lines.push(Line::raw(""));
    lines.push(Line::raw(format!(
        "  Grok binary: {}",
        app.cfg.grok_bin.display()
    )));
    lines.push(Line::raw(format!(
        "  Claude bin:  {}",
        app.cfg.claude_bin.display()
    )));
    lines.push(Line::raw(format!(
        "  Agent MCP:   {}  ({mcp_note})",
        mcp.display()
    )));
    lines.push(Line::raw(format!(
        "  Work root:   {}",
        app.cfg.work_root.display()
    )));
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "  Controls:  ↑/↓ row · ←/→ change · b backend · a apply→all · r reroll roles (balance vs models) · s shuffle models (balance vs roles)",
        dim,
    )));
    lines.push(Line::from(Span::styled(
        "             Enter create game + spawn agents · q quit",
        dim,
    )));
    lines.push(Line::raw(""));
    lines.push(Line::from(Span::styled(
        "  Each agent workdir gets a per-backend MCP config → botc-agent-mcp (token-scoped)",
        dim,
    )));
    lines.push(Line::from(Span::styled(
        "  → Unix socket → shared in-process engine. Build first: cargo build --bins",
        dim,
    )));

    let p = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title("setup"))
        .wrap(Wrap { trim: false });
    f.render_widget(p, area);
}

/// Left monitor pane: agents + grimoire fused per seat, plus a live nomination tracker.
///
/// Each agent is 1–2 lines (identity/status on the first, markers on the second).
/// Clickable rows map through `app.board_agent_rows` (agent index or `None`).
fn draw_board_panel(f: &mut Frame, area: Rect, app: &mut App) {
    let dim = Style::default().fg(Color::DarkGray);
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut row_map: Vec<Option<usize>> = Vec::new();

    let push = |lines: &mut Vec<Line<'static>>,
                row_map: &mut Vec<Option<usize>>,
                line: Line<'static>,
                agent: Option<usize>| {
        lines.push(line);
        row_map.push(agent);
    };

    // Snapshot grimoire + noms under a short store lock (display only).
    let board = board_snapshot(app);

    if let Some(pool) = app.agents.as_ref() {
        for (i, a) in pool.agents.iter().enumerate() {
            let running = *a.running.lock().unwrap();
            let (glyph, gstyle) = if running {
                (
                    "●",
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                ("○", dim)
            };
            let selected = i == app.selected_agent;
            let mark = if selected { "▶" } else { " " };
            let model = crate::harness::action_log::clip_chars(&a.config.model, 12);
            let (usage_short, usage_style) = {
                let u = a.usage.lock().unwrap();
                let short = u.board_short();
                let pct = u.context.as_ref().map(|c| c.usage_pct).unwrap_or(0);
                let style = if pct >= 80 {
                    Style::default().fg(Color::Red)
                } else if pct >= 50 {
                    Style::default().fg(Color::Yellow)
                } else if pct > 0 || u.game_total.total_tokens > 0 {
                    Style::default().fg(Color::Cyan)
                } else {
                    dim
                };
                (short, style)
            };

            match a.config.role {
                AgentRole::Host => {
                    let name_st = if selected {
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::Magenta)
                    };
                    push(
                        &mut lines,
                        &mut row_map,
                        Line::from(vec![
                            Span::styled(glyph, gstyle),
                            Span::raw(" "),
                            Span::styled(format!("{mark} Host"), name_st),
                            Span::styled("  Storyteller", dim),
                            Span::styled(format!(" · {model}"), dim),
                        ]),
                        Some(i),
                    );
                    // Host second line: phase + pending waits (ST-only).
                    let mut detail = String::new();
                    if let Some(ref b) = board {
                        detail.push_str(&b.phase_short);
                        if let Some(ref ph) = b.pending_host {
                            detail.push_str(&format!(" · host:{ph}"));
                        }
                        if let Some(seat) = b.pending_night {
                            detail.push_str(&format!(" · wake:P{seat}"));
                        }
                        if b.pending_host.is_none() && b.pending_night.is_none() {
                            detail.push_str(" · idle");
                        }
                    } else {
                        detail.push('—');
                    }
                    push(
                        &mut lines,
                        &mut row_map,
                        Line::from(Span::styled(format!("    {detail}"), dim)),
                        Some(i),
                    );
                    push(
                        &mut lines,
                        &mut row_map,
                        Line::from(Span::styled(format!("    {usage_short}"), usage_style)),
                        Some(i),
                    );
                }
                AgentRole::Player { seat } => {
                    let seat_n = seat.0;
                    let seat_info = board
                        .as_ref()
                        .and_then(|b| b.seats.iter().find(|s| s.seat == seat_n));
                    let (role_text, role_st) = match seat_info {
                        Some(s) => {
                            let label = if s.is_drunk_outsider {
                                match s.believed.as_deref() {
                                    Some(face) => format!("Drunk (as {face})"),
                                    None => "Drunk".into(),
                                }
                            } else {
                                s.true_role.clone().unwrap_or_else(|| "?".into())
                            };
                            let st = s
                                .true_char
                                .map(role_style)
                                .unwrap_or(Style::default().fg(Color::White));
                            (label, st)
                        }
                        None => ("—".into(), dim),
                    };
                    let name_st = if selected {
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(actor_color(false, Some(seat_n)))
                    };
                    push(
                        &mut lines,
                        &mut row_map,
                        Line::from(vec![
                            Span::styled(glyph, gstyle),
                            Span::raw(" "),
                            Span::styled(format!("{mark} P{seat_n}"), name_st),
                            Span::raw(" "),
                            Span::styled(role_text, role_st),
                            Span::styled(format!(" · {model}"), dim),
                        ]),
                        Some(i),
                    );
                    // Markers: life, poison, monk, butler, ghost.
                    let mut marks: Vec<Span<'static>> = vec![Span::raw("    ")];
                    if let Some(s) = seat_info {
                        if s.alive {
                            marks.push(Span::styled("alive", Style::default().fg(Color::Green)));
                        } else {
                            marks.push(Span::styled(
                                "DEAD",
                                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                            ));
                        }
                        if let Some(team) = s.team.as_deref() {
                            let tsty = if team == "Evil" {
                                Style::default().fg(Color::Red)
                            } else {
                                Style::default().fg(Color::Green)
                            };
                            marks.push(Span::styled(format!(" · {team}"), tsty));
                        }
                        if s.poisoned {
                            marks.push(Span::styled(
                                " · poison",
                                Style::default().fg(Color::Magenta),
                            ));
                        }
                        if s.monk_protected {
                            marks.push(Span::styled(" · monk", Style::default().fg(Color::Cyan)));
                        }
                        if let Some(m) = s.butler_master {
                            marks.push(Span::styled(
                                format!(" · butler→P{m}"),
                                Style::default().fg(Color::Yellow),
                            ));
                        }
                        if !s.alive && s.ghost_vote_available {
                            marks.push(Span::styled(" · ghost✓", dim));
                        } else if !s.alive && !s.ghost_vote_available {
                            marks.push(Span::styled(" · ghost✗", dim));
                        }
                        if s.slayer_used {
                            marks.push(Span::styled(" · slayer✓", dim));
                        }
                        if s.virgin_used {
                            marks.push(Span::styled(" · virgin✓", dim));
                        }
                    } else {
                        marks.push(Span::styled("—", dim));
                    }
                    push(&mut lines, &mut row_map, Line::from(marks), Some(i));
                    push(
                        &mut lines,
                        &mut row_map,
                        Line::from(Span::styled(format!("    {usage_short}"), usage_style)),
                        Some(i),
                    );
                }
            }
        }
    } else {
        push(
            &mut lines,
            &mut row_map,
            Line::from(Span::styled("no agents", dim)),
            None,
        );
    }

    // ── live nomination tracker ──────────────────────────────────────────
    push(&mut lines, &mut row_map, Line::from(Span::raw("")), None);
    push(
        &mut lines,
        &mut row_map,
        Line::from(Span::styled("── noms ──", dim)),
        None,
    );
    if let Some(ref b) = board {
        if b.closed_noms.is_empty() && b.open_nom.is_none() {
            push(
                &mut lines,
                &mut row_map,
                Line::from(Span::styled("  (none today)", dim)),
                None,
            );
        }
        for c in &b.closed_noms {
            let thr = if c.meets_threshold {
                Style::default().fg(Color::Cyan)
            } else {
                dim
            };
            push(
                &mut lines,
                &mut row_map,
                Line::from(vec![
                    Span::raw(format!("  P{}→P{}  ", c.by, c.target)),
                    Span::styled("closed", dim),
                    Span::styled(format!("  {} yes", c.yes), thr),
                    if c.meets_threshold {
                        Span::styled("  ≥½", thr)
                    } else {
                        Span::raw("")
                    },
                ]),
                None,
            );
        }
        if let Some(ref o) = b.open_nom {
            let mut vote_bits: Vec<String> = o
                .votes
                .iter()
                .map(|(s, yes)| format!("P{s}{}", if *yes { "✓" } else { "✗" }))
                .collect();
            for s in &o.passes {
                vote_bits.push(format!("P{s}–"));
            }
            let votes = if vote_bits.is_empty() {
                "no votes yet".into()
            } else {
                vote_bits.join(" ")
            };
            push(
                &mut lines,
                &mut row_map,
                Line::from(vec![
                    Span::raw(format!("  P{}→P{}  ", o.by, o.target)),
                    Span::styled(
                        "OPEN",
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!("  {votes}"), Style::default().fg(Color::White)),
                ]),
                None,
            );
        }
    } else {
        push(
            &mut lines,
            &mut row_map,
            Line::from(Span::styled("  (no game)", dim)),
            None,
        );
    }

    app.board_agent_rows = row_map;
    let title = if let Some(ref b) = board {
        format!("board · {} · ●=run", b.phase_short)
    } else {
        "board · ●=run".into()
    };
    let p = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(title))
        .wrap(Wrap { trim: false });
    f.render_widget(p, area);
}

/// Host-only board snapshot for the left pane (grimoire + noms). Display only.
struct BoardSnapshot {
    phase_short: String,
    pending_host: Option<String>,
    pending_night: Option<u8>,
    seats: Vec<BoardSeat>,
    closed_noms: Vec<BoardClosedNom>,
    open_nom: Option<BoardOpenNom>,
}

struct BoardSeat {
    seat: u8,
    alive: bool,
    true_role: Option<String>,
    believed: Option<String>,
    true_char: Option<Character>,
    team: Option<String>,
    poisoned: bool,
    is_drunk_outsider: bool,
    monk_protected: bool,
    butler_master: Option<u8>,
    ghost_vote_available: bool,
    slayer_used: bool,
    virgin_used: bool,
}

struct BoardClosedNom {
    by: u8,
    target: u8,
    yes: u32,
    meets_threshold: bool,
}

struct BoardOpenNom {
    by: u8,
    target: u8,
    votes: Vec<(u8, bool)>,
    passes: Vec<u8>,
}

fn board_snapshot(app: &App) -> Option<BoardSnapshot> {
    let gid = app.game_id?;
    let st = app.store.lock().unwrap();
    let g = st.get(GameId(gid))?;
    let living = g.seats.iter().filter(|s| s.alive).count() as u32;
    let phase_short = match &g.phase {
        Phase::Lobby => "lobby".into(),
        Phase::FirstNight { .. } => "N1".into(),
        Phase::Night { night, .. } => format!("N{night}"),
        Phase::Day { day, stage } => format!(
            "D{day}·{}",
            match stage {
                crate::game::DayStage::Discussion => "disc",
                crate::game::DayStage::Nominations => "noms",
            }
        ),
        Phase::Ended { winner, .. } => format!("end:{winner:?}"),
    };
    let seats = g
        .seats
        .iter()
        .map(|s| BoardSeat {
            seat: s.id.0,
            alive: s.alive,
            true_role: s.true_character.map(|c| c.display_name().to_string()),
            believed: s.believed_character.map(|c| c.display_name().to_string()),
            true_char: s.true_character,
            team: s.true_character.map(|c| format!("{:?}", c.team())),
            poisoned: s.poisoned,
            is_drunk_outsider: s.is_drunk_outsider,
            monk_protected: s.monk_protected_tonight,
            butler_master: s.butler_master.map(|m| m.0),
            ghost_vote_available: s.ghost_vote_available,
            slayer_used: s.slayer_used,
            virgin_used: s.virgin_ability_used,
        })
        .collect();
    let closed_noms = g
        .closed_nominations
        .iter()
        .map(|c| BoardClosedNom {
            by: c.by.0,
            target: c.target.0,
            yes: c.yes_votes,
            meets_threshold: crate::game::meets_threshold(c.yes_votes, living),
        })
        .collect();
    let open_nom = g.current_nomination.as_ref().map(|o| BoardOpenNom {
        by: o.by.0,
        target: o.target.0,
        votes: o.votes.iter().map(|(s, y)| (s.0, *y)).collect(),
        passes: o.passes.iter().map(|s| s.0).collect(),
    });
    Some(BoardSnapshot {
        phase_short,
        pending_host: g.pending_host.as_ref().map(|p| p.kind_str().to_string()),
        pending_night: g.pending_night.as_ref().map(|w| w.seat.0),
        seats,
        closed_noms,
        open_nom,
    })
}

fn draw_monitor(f: &mut Frame, area: Rect, app: &mut App) {
    // Left board is denser (grimoire + noms + agent status) — give it more width;
    // center is always the action feed (no grimoire toggle).
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(32),
            Constraint::Percentage(33),
            Constraint::Percentage(35),
        ])
        .split(area);

    // Mouse hit targets (click board / click-or-scroll stream / feed).
    app.hit_agents = cols[0];
    app.hit_stream = cols[2];

    draw_board_panel(f, cols[0], app);
    draw_action_feed(f, cols[1], app);

    let agent_title = app
        .agents
        .as_ref()
        .and_then(|p| p.agents.get(app.selected_agent))
        .map(|a| match a.config.role {
            AgentRole::Host => format!("stream: Storyteller [{}]", a.config.model),
            AgentRole::Player { seat } => format!("stream: seat{} [{}]", seat.0, a.config.model),
        })
        .unwrap_or_else(|| "stream".into());

    // #45/#53: tail-anchor in *wrapped visual rows* (Paragraph::scroll unit), not
    // logical lines — long Grok prose wraps in the stream column constantly.
    let log_lines = app.agent_log_lines();
    let inner_w = cols[2].width.saturating_sub(2);
    let view_h = cols[2].height.saturating_sub(2) as usize;
    let para = Paragraph::new(log_lines).wrap(Wrap { trim: false });
    // Wrapped-row count for the exact rendered width (tail-anchor scroll, #53).
    let row_count = if inner_w == 0 {
        0
    } else {
        para.line_count(inner_w)
    };
    // scroll_from_bottom=0 → show the end; larger → look further up (row units).
    let scroll_y = stream_scroll_y(row_count, view_h, app.scroll_from_bottom);
    let scroll_y_u16 = u16::try_from(scroll_y).unwrap_or(u16::MAX);
    let tail_mark = if app.scroll_from_bottom == 0 {
        "·live"
    } else {
        "·scroll"
    };
    let think_mark = if app.selected_thinking_expanded() {
        "·think▸"
    } else {
        "·think▾"
    };
    let log_p = para
        .block(Block::default().borders(Borders::ALL).title(format!(
            "{agent_title} {tail_mark} {think_mark}  click·h  wheel·scroll"
        )))
        .scroll((scroll_y_u16, 0));
    f.render_widget(log_p, cols[2]);
}

/// Format agent log lines for the stream pane.
///
/// When `expand_think` is false (default), consecutive `[think] …` blocks collapse
/// to a one-line summary so text / tool / error / turn-end lines (game actions)
/// stay visible while tabbing agents. Press `h` to expand.
/// Colour a stream line by kind — no in-text tags. Errors show red.
fn styled_log_line(e: &LogLine) -> Line<'static> {
    let style = match e.kind {
        LineKind::Text => Style::default(),
        LineKind::Thought => Style::default().fg(Color::DarkGray),
        LineKind::Stderr => Style::default().fg(Color::Yellow),
        LineKind::System => {
            if e.text.to_lowercase().contains("error") {
                Style::default().fg(Color::Red)
            } else {
                Style::default().fg(Color::Cyan)
            }
        }
    };
    Line::from(Span::styled(e.text.clone(), style))
}

/// A collapsed-thinking summary line standing in for a run of hidden thought.
fn think_summary(n: usize, preview: &str) -> Line<'static> {
    let snip = if preview.trim().is_empty() {
        String::new()
    } else {
        let s: String = preview.trim().chars().take(48).collect();
        let ell = if preview.trim().chars().count() > 48 {
            "…"
        } else {
            ""
        };
        format!(" “{s}{ell}”")
    };
    Line::from(Span::styled(
        format!("· thinking… {n} line(s){snip}  (h/click = show)"),
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::ITALIC),
    ))
}

/// Turn the kinded log into styled display lines. When `expand` is false, runs of
/// `Thought` lines collapse to a one-line summary (default); otherwise they show
/// dimmed. Everything else streams verbatim, coloured by kind.
pub fn stream_lines(entries: &[LogLine], expand: bool) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut think_n = 0usize;
    let mut preview = String::new();
    for e in entries {
        if e.kind == LineKind::Thought && !expand {
            think_n += 1;
            if preview.is_empty() {
                preview = e.text.clone();
            }
            continue;
        }
        if think_n > 0 {
            out.push(think_summary(think_n, &preview));
            think_n = 0;
            preview.clear();
        }
        out.push(styled_log_line(e));
    }
    if think_n > 0 {
        out.push(think_summary(think_n, &preview));
    }
    out
}

/// Wrapped visual row count at `inner_w` — same unit as `Paragraph::scroll` (#53).
///
/// Uses ratatui's wrap math (no Block) so the count matches what the renderer
/// produces inside a bordered pane of that inner width.
pub fn stream_wrapped_row_count(text: &str, inner_w: u16) -> usize {
    if text.is_empty() || inner_w == 0 {
        return 0;
    }
    Paragraph::new(text)
        .wrap(Wrap { trim: false })
        .line_count(inner_w)
}

/// Vertical scroll offset that keeps the live tail (or an offset from it) in view.
/// `row_count` / `scroll_from_bottom` are in **wrapped visual rows** (#45/#53).
pub fn stream_scroll_y(row_count: usize, view_h: usize, scroll_from_bottom: usize) -> usize {
    let max_scroll_top = row_count.saturating_sub(view_h);
    max_scroll_top.saturating_sub(scroll_from_bottom.min(max_scroll_top))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;

    #[test]
    fn point_in_rect_hit_test() {
        let r = Rect {
            x: 10,
            y: 5,
            width: 20,
            height: 8,
        };
        assert!(point_in_rect(r, 10, 5));
        assert!(point_in_rect(r, 29, 12));
        assert!(!point_in_rect(r, 9, 5));
        assert!(!point_in_rect(r, 30, 5));
        assert!(!point_in_rect(r, 10, 13));
    }

    #[test]
    fn stream_scroll_defaults_to_tail() {
        // 50 rows, 10-row pane, scroll_from_bottom=0 → scroll so last 10 show.
        assert_eq!(stream_scroll_y(50, 10, 0), 40);
        assert_eq!(stream_scroll_y(5, 10, 0), 0); // fits entirely
        assert_eq!(stream_scroll_y(50, 10, 5), 35); // look 5 rows up from tail
        assert_eq!(stream_scroll_y(50, 10, 999), 0); // clamped to top
    }

    fn lk(kind: LineKind, text: &str) -> LogLine {
        LogLine {
            kind,
            text: text.into(),
            closed: true,
        }
    }
    fn line_text(l: &Line) -> String {
        l.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn stream_collapses_think_by_default() {
        let entries = vec![
            lk(LineKind::Thought, "I should poison someone"),
            lk(LineKind::Thought, "maybe seat 2"),
            lk(LineKind::Text, "Calling night_action on seat 2"),
            lk(LineKind::System, "— turn end —"),
        ];
        let texts: Vec<String> = stream_lines(&entries, false)
            .iter()
            .map(line_text)
            .collect();
        assert_eq!(
            texts.iter().filter(|t| t.contains("thinking…")).count(),
            1,
            "consecutive thinks must merge to one summary: {texts:?}"
        );
        assert!(texts.iter().any(|t| t.contains("2 line")));
        assert!(
            !texts.iter().any(|t| t.contains("maybe seat 2")),
            "collapsed must not dump think bodies: {texts:?}"
        );
        assert!(texts
            .iter()
            .any(|t| t.contains("Calling night_action on seat 2")));
        assert!(texts.iter().any(|t| t.contains("turn end")));

        let etexts: Vec<String> = stream_lines(&entries, true).iter().map(line_text).collect();
        assert!(etexts.iter().any(|t| t.contains("I should poison someone")));
        assert!(etexts.iter().any(|t| t.contains("maybe seat 2")));
        // No in-text tags — colour, not text markers.
        assert!(!etexts.iter().any(|t| t.contains("[think]")));
    }

    #[test]
    fn stream_interleaves_think_and_action() {
        let entries = vec![
            lk(LineKind::Thought, "first"),
            lk(LineKind::Text, "action A"),
            lk(LineKind::Thought, "second"),
            lk(LineKind::Text, "action B"),
        ];
        let texts: Vec<String> = stream_lines(&entries, false)
            .iter()
            .map(line_text)
            .collect();
        assert_eq!(texts.len(), 4, "got {texts:?}");
        assert!(texts[0].contains("thinking… 1 line"));
        assert_eq!(texts[1], "action A");
        assert!(texts[2].contains("thinking… 1 line"));
        assert_eq!(texts[3], "action B");
    }

    #[test]
    fn wrapped_row_count_exceeds_logical_when_lines_wrap() {
        // 11 logical lines that each wrap to >1 row at width 10.
        let mut lines: Vec<String> = (0..10)
            .map(|i| format!("LINE{i}-ABCDEFGHIJKLMNOPQRSTUVWXYZ"))
            .collect();
        lines.push("MARKER-LAST-LINE".into());
        let text = lines.join("\n");
        let logical = text.lines().count();
        let wrapped = stream_wrapped_row_count(&text, 18); // inner_w of a w=20 bordered pane
        assert_eq!(logical, 11);
        assert!(
            wrapped > logical,
            "expected wrap to inflate row count: logical={logical} wrapped={wrapped}"
        );
        // Issue repro numbers: logical scroll under-shoots; wrap-aware is larger.
        assert!(
            stream_scroll_y(wrapped, 6, 0) > stream_scroll_y(logical, 6, 0),
            "wrap-aware tail scroll must be deeper than logical-only"
        );
    }

    /// #53: with wrapping lines, live-tail scroll must keep MARKER on screen.
    #[test]
    fn wrap_aware_tail_anchor_shows_last_line_on_test_backend() {
        // Pane w=20 / h=8 → inner 18×6 (borders). Matches issue repro shape.
        let backend = TestBackend::new(20, 8);
        let mut terminal = Terminal::new(backend).expect("terminal");

        let mut lines: Vec<String> = (0..10)
            .map(|i| format!("LINE{i}-ABCDEFGHIJKLMNOPQRSTUVWXYZ"))
            .collect();
        lines.push("MARKER-LAST-LINE".into());
        let log_text = lines.join("\n");

        terminal
            .draw(|f| {
                let area = f.area();
                let inner_w = area.width.saturating_sub(2);
                let view_h = area.height.saturating_sub(2) as usize;
                let rows = stream_wrapped_row_count(&log_text, inner_w);
                let scroll_y = stream_scroll_y(rows, view_h, 0);
                // Sanity: logical-only would under-scroll (issue repro).
                let logical = log_text.lines().count();
                assert!(
                    scroll_y > stream_scroll_y(logical, view_h, 0),
                    "wrap-aware scroll_y={scroll_y} should exceed logical"
                );
                let p = Paragraph::new(log_text.as_str())
                    .block(Block::default().borders(Borders::ALL).title("stream ·live"))
                    .wrap(Wrap { trim: false })
                    .scroll((u16::try_from(scroll_y).unwrap_or(u16::MAX), 0));
                f.render_widget(p, area);
            })
            .expect("draw");

        let buf = terminal.backend().buffer();
        let mut screen = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                screen.push_str(buf[(x, y)].symbol());
            }
            screen.push('\n');
        }
        assert!(
            screen.contains("MARKER"),
            "last line MARKER must be visible with wrap-aware tail scroll; screen:\n{screen}"
        );

        // Control: logical-only scroll must leave MARKER off-screen (documents the bug).
        let backend2 = TestBackend::new(20, 8);
        let mut terminal2 = Terminal::new(backend2).expect("terminal2");
        terminal2
            .draw(|f| {
                let area = f.area();
                let view_h = area.height.saturating_sub(2) as usize;
                let logical = log_text.lines().count();
                let bad_y = stream_scroll_y(logical, view_h, 0);
                let p = Paragraph::new(log_text.as_str())
                    .block(Block::default().borders(Borders::ALL).title("bad"))
                    .wrap(Wrap { trim: false })
                    .scroll((u16::try_from(bad_y).unwrap_or(u16::MAX), 0));
                f.render_widget(p, area);
            })
            .expect("draw2");
        let buf2 = terminal2.backend().buffer();
        let mut screen2 = String::new();
        for y in 0..buf2.area.height {
            for x in 0..buf2.area.width {
                screen2.push_str(buf2[(x, y)].symbol());
            }
            screen2.push('\n');
        }
        assert!(
            !screen2.contains("MARKER"),
            "logical-only scroll should still clip MARKER (proves wrap matters); screen:\n{screen2}"
        );
    }

    #[test]
    fn previewed_assignment_replays_as_fixed_assignments() {
        // The parity contract behind reroll_roles() → launch(): a role assignment
        // drawn off a throwaway game must replay verbatim as fixed `assignments`, so
        // the launched game shows exactly the roles the operator picked models for.
        let names: Vec<String> = (0..7).map(|i| format!("P{i}")).collect();
        let created = Game::create(names.clone(), 42).unwrap();
        let mut preview = created.game;
        preview
            .start_game(&created.host_token, StartOpts::default())
            .unwrap();
        let roles: Vec<RoleAssignment> = preview
            .seats
            .iter()
            .map(|s| RoleAssignment {
                seat: s.id,
                true_character: s.true_character.unwrap(),
                believed_character: s.believed_character,
            })
            .collect();

        // A legal Trouble Brewing bag: exactly one Imp, and a face iff Drunk.
        assert_eq!(
            roles
                .iter()
                .filter(|a| a.true_character == Character::Imp)
                .count(),
            1
        );
        for a in &roles {
            assert_eq!(
                a.believed_character.is_some(),
                a.true_character == Character::Drunk
            );
        }

        // Different seed — proves the launched roles come from the fixed assignments,
        // not a fresh random bag.
        let launched = Game::create(names, 999).unwrap();
        let mut g = launched.game;
        g.start_game(
            &launched.host_token,
            StartOpts {
                assignments: Some(roles.clone()),
                ..Default::default()
            },
        )
        .unwrap();
        for (s, a) in g.seats.iter().zip(&roles) {
            assert_eq!(s.true_character, Some(a.true_character));
            assert_eq!(s.believed_character, a.believed_character);
        }
    }

    #[test]
    fn role_label_and_style_surface_team_and_drunk_face() {
        let imp = RoleAssignment::normal(SeatId(0), Character::Imp);
        assert_eq!(role_label(&imp), "Imp");
        assert_eq!(role_style(Character::Imp).fg, Some(Color::Red));

        // Drunk shows both its true role and the Townsfolk it believes it is.
        let drunk = RoleAssignment::drunk(SeatId(1), Character::Chef).unwrap();
        assert_eq!(role_label(&drunk), "Drunk (as Chef)");
        // …but is coloured as the Outsider it truly is, not as its face.
        assert_eq!(role_style(drunk.true_character).fg, Some(Color::Yellow));

        assert_eq!(role_style(Character::Empath).fg, Some(Color::Green));
        assert_eq!(role_style(Character::Poisoner).fg, Some(Color::Red));
    }
}
