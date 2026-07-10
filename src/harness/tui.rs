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
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};
use ratatui::{Frame, Terminal};

use crate::game::{Game, GameId, Phase, SeatId, StartOpts, StChoiceMode};
use crate::harness::action_log::{ActionKind, ActionLog, ActorLabel};
use crate::harness::scheduler::{plan_ticks, wait_signature, SchedTarget};
use crate::harness::agents::{
    agent_mcp_bin_ok, find_agent_mcp_bin, find_grok, resolve_agent_mcp_bin_for_display,
    AgentConfig, AgentPool, AgentRole, HarnessConfig,
};
use crate::harness::socket::SocketServer;
use crate::mcp_server::{self, SharedStore};
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

struct App {
    focus: Focus,
    player_count: usize,
    selected_agent: usize,
    status: String,
    store: SharedStore,
    game_id: Option<u64>,
    host_token: Option<crate::auth::Token>,
    player_names: Vec<String>,
    socket: Option<SocketServer>,
    agents: Option<AgentPool>,
    auto_tick: bool,
    last_tick: Instant,
    tick_interval: Duration,
    cfg: HarnessConfig,
    should_quit: bool,
    /// Lines scrolled up from the live tail (0 = stick to newest output) (#45).
    scroll_from_bottom: usize,
    /// Round-robin cursor for discussion/nomination turns in the scheduler (#60).
    tick_rotation: usize,
    /// Signature of what the engine was waiting on last tick, for stall detection (#60).
    wait_sig: Option<String>,
    /// Consecutive cycles the engine has sat on `wait_sig`; drives host escalation (#60).
    stall: usize,
    /// Agent indices whose stream shows full `[think]` blocks (default: collapsed).
    thinking_expanded: HashSet<usize>,
    /// Last drawn hit targets for mouse (agents list + stream pane).
    hit_agents: Rect,
    hit_stream: Rect,
    /// Central feed of every agent tool RPC (shared with the socket server) (#UI).
    action_log: Arc<ActionLog>,
    /// Center pane shows the grimoire (true) or the action feed (false).
    show_grimoire: bool,
    /// Labels of the agents targeted by the most recent scheduled tick (for the status bar).
    last_targets: Vec<String>,
    /// Rows scrolled up from the tail of the action feed (0 = live).
    feed_scroll: usize,
    /// Which actions the feed shows (all vs game-only).
    feed_filter: FeedFilter,
}

impl App {
    fn new() -> Self {
        let mut cfg = HarnessConfig::default();
        cfg.grok_bin = find_grok();
        cfg.agent_mcp_bin = find_agent_mcp_bin();
        let id = uuid::Uuid::new_v4();
        cfg.work_root = PathBuf::from(format!("/tmp/botc-harness-{id}"));
        cfg.socket_path = cfg.work_root.join("engine.sock");
        let mcp_ok = agent_mcp_bin_ok(&cfg);
        let mcp_path = resolve_agent_mcp_bin_for_display(&cfg);
        let status = if mcp_ok {
            format!(
                "↑/↓ players · Enter launch · q quit | grok={} mcp={}",
                cfg.grok_bin.display(),
                mcp_path.display()
            )
        } else {
            format!(
                "MISSING botc-agent-mcp at {} — run: cargo build --bins",
                mcp_path.display()
            )
        };
        Self {
            focus: Focus::Setup,
            player_count: 5,
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
            tick_interval: Duration::from_secs(45),
            cfg,
            should_quit: false,
            scroll_from_bottom: 0,
            tick_rotation: 0,
            wait_sig: None,
            stall: 0,
            thinking_expanded: HashSet::new(),
            hit_agents: Rect::default(),
            hit_stream: Rect::default(),
            action_log: Arc::new(ActionLog::default()),
            show_grimoire: false,
            last_targets: Vec::new(),
            feed_scroll: 0,
            feed_filter: FeedFilter::All,
        }
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
            self.scroll_from_bottom = self
                .scroll_from_bottom
                .saturating_add(delta as usize);
        } else {
            self.scroll_from_bottom = self
                .scroll_from_bottom
                .saturating_sub((-delta) as usize);
        }
    }

    fn on_mouse(&mut self, m: MouseEvent) {
        let x = m.column;
        let y = m.row;
        match m.kind {
            // Wheel / trackpad: scroll the stream when the pointer is over it.
            MouseEventKind::ScrollUp if point_in_rect(self.hit_stream, x, y) => {
                self.scroll_stream(3);
            }
            MouseEventKind::ScrollDown if point_in_rect(self.hit_stream, x, y) => {
                self.scroll_stream(-3);
            }
            // Left click: select agent, or toggle thinking on the stream pane.
            MouseEventKind::Down(MouseButton::Left) => {
                if point_in_rect(self.hit_agents, x, y) {
                    if let Some(pool) = self.agents.as_ref() {
                        let n = pool.agents.len();
                        if n == 0 {
                            return;
                        }
                        // Row under the top border → agent index.
                        let inner_y = y.saturating_sub(self.hit_agents.y.saturating_add(1));
                        let idx = inner_y as usize;
                        if idx < n {
                            self.selected_agent = idx;
                            self.scroll_from_bottom = 0;
                            self.status = format!("selected agent {idx}");
                        }
                    }
                } else if point_in_rect(self.hit_stream, x, y) {
                    self.toggle_thinking_selected();
                }
            }
            // Ignore other mouse noise (drags, right-click, wheel outside stream).
            _ => {}
        }
    }

    fn on_key(&mut self, code: KeyCode) {
        match code {
            KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
            KeyCode::Up | KeyCode::Char('k') if self.focus == Focus::Setup => {
                self.player_count = (self.player_count + 1).min(15);
            }
            KeyCode::Down | KeyCode::Char('j') if self.focus == Focus::Setup => {
                self.player_count = (self.player_count - 1).max(5);
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
            // Toggle the center pane between the action feed and the host grimoire.
            KeyCode::Char('g') if self.agents.is_some() => {
                self.show_grimoire = !self.show_grimoire;
                self.status = if self.show_grimoire {
                    "center: grimoire (g = action feed)".into()
                } else {
                    "center: action feed (g = grimoire)".into()
                };
            }
            // Toggle the feed filter: all actions ↔ game-only.
            KeyCode::Char('f') if self.agents.is_some() => {
                self.feed_filter = match self.feed_filter {
                    FeedFilter::All => FeedFilter::GameOnly,
                    FeedFilter::GameOnly => FeedFilter::All,
                };
                self.show_grimoire = false;
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
                self.do_tick();
                self.last_tick = Instant::now();
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
            tools::start_game(
                g,
                &created.host_token,
                StartOpts {
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

        let mut configs = vec![AgentConfig {
            role: AgentRole::Host,
            display_name: "Storyteller".into(),
            token: created.host_token.as_str().to_string(),
            game_id,
        }];
        for (i, tok) in created.player_tokens.iter().enumerate() {
            configs.push(AgentConfig {
                role: AgentRole::Player {
                    seat: SeatId(i as u8),
                },
                display_name: self.player_names[i].clone(),
                token: tok.as_str().to_string(),
                game_id,
            });
        }

        match AgentPool::prepare(&self.cfg, configs) {
            Ok(mut pool) => match pool.kickoff_all(self.player_count) {
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
                }
                Err(e) => {
                    // Partial pool still owns workdirs — keep it so shutdown/stop_all cleans up.
                    self.status = format!("kickoff: {e}");
                    self.agents = Some(pool);
                }
            },
            Err(e) => {
                self.status = format!("prepare: {e}");
                // #55/#58: socket + partial work_root without agents.
                self.abort_partial_launch();
            }
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
        let (plan, summary, hint, ended, sig, stall) = {
            let st = self.store.lock().unwrap();
            let Some(g) = st.get(GameId(gid)) else {
                return;
            };
            // Stall detection: same wait as last cycle => it's not progressing.
            let sig = wait_signature(g);
            let stall = if sig.is_some() && sig == self.wait_sig {
                self.stall + 1
            } else {
                0
            };
            let plan = plan_ticks(g, self.tick_rotation, stall);
            let (summary, hint) = game_summary_and_hint(g);
            (
                plan,
                summary,
                hint,
                matches!(g.phase, Phase::Ended { .. }),
                sig,
                stall,
            )
        };
        self.wait_sig = sig;
        self.stall = stall;

        if ended {
            self.auto_tick = false;
            self.status = "Game ended — agents idle. Press q to quit or Tab to review streams.".into();
            return;
        }

        self.tick_rotation = self.tick_rotation.wrapping_add(1);
        self.last_targets = plan.iter().map(target_label).collect();
        if let Some(pool) = self.agents.as_mut() {
            match pool.tick_scheduled(&plan, &summary, &hint) {
                Ok(spawned) => {
                    self.status = format!(
                        "Turn tick → {}: {spawned} of {} spawned.",
                        self.last_targets.join(", "),
                        plan.len()
                    );
                }
                Err(e) => self.status = format!("tick error: {e}"),
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
        let tick = if self.auto_tick {
            let rem = self
                .tick_interval
                .saturating_sub(self.last_tick.elapsed())
                .as_secs();
            format!("auto {rem}s")
        } else {
            "manual (Space)".to_string()
        };
        let mut spans = vec![
            Span::styled(
                format!(" {phase} "),
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
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
                "idle (nothing running)",
                Style::default().fg(Color::DarkGray),
            ));
        } else {
            spans.push(Span::styled(
                format!("▶ running: {}", running.join(",")),
                Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
            ));
        }
        spans
    }

    fn snapshot_host(&self) -> String {
        let Some(gid) = self.game_id else {
            return "No game.".into();
        };
        let Some(host) = self.host_token.as_ref() else {
            return "No host token.".into();
        };
        let st = self.store.lock().unwrap();
        let Some(g) = st.get(GameId(gid)) else {
            return "Missing game.".into();
        };
        match tools::get_host_state(g, host) {
            Ok(v) => {
                let mut lines = vec![
                    format!("phase {}", v.phase),
                    format!("seed {}  salt {}", v.seed, v.secret_salt),
                    format!(
                        "pending_host {:?}",
                        v.pending_host.as_ref().map(|p| &p.kind)
                    ),
                    format!("st_choice {}", v.st_choice_mode),
                    "seats:".into(),
                ];
                for s in &v.seats {
                    lines.push(format!(
                        "  #{} {:8} alive={:5} true={:?} face={:?} poison={}",
                        s.seat_id.0,
                        s.name,
                        s.alive,
                        s.true_character,
                        s.believed_character,
                        s.poisoned
                    ));
                }
                lines.join("\n")
            }
            Err(e) => format!("host_state err: {e}"),
        }
    }

    /// Agent stream for the selected seat: thinking collapsed by default so game
    /// actions (text / tools / errors) stay visible; `h` expands `[think]` blocks.
    fn agent_log_text(&self) -> String {
        let Some(pool) = self.agents.as_ref() else {
            return String::new();
        };
        if pool.agents.is_empty() {
            return String::new();
        }
        let idx = self.selected_agent.min(pool.agents.len() - 1);
        let agent = &pool.agents[idx];
        let log = agent.log.lock().unwrap();
        // Cap retained lines for the pane (keep a generous buffer for PgUp).
        let start = log.len().saturating_sub(400);
        let slice = &log[start..];
        format_stream_for_display(slice, self.thinking_expanded.contains(&idx))
    }

    fn shutdown(&mut self) {
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

/// Public snapshot + host hint string for tick prompts, computed from a locked game.
/// Free function so the scheduler can build it under the store lock without a second lock.
fn game_summary_and_hint(g: &Game) -> (String, String) {
    let phase = format!("{:?}", g.phase);
    let living: Vec<_> = g
        .seats
        .iter()
        .filter(|s| s.alive)
        .map(|s| format!("{}#{}", s.display_name, s.id.0))
        .collect();
    let chat: Vec<_> = g
        .public_log
        .since(0)
        .into_iter()
        .rev()
        .take(12)
        .map(|(id, e)| format!("#{id} {e:?}"))
        .collect();
    let chat: Vec<_> = chat.into_iter().rev().collect();
    let summary = format!(
        "phase={phase}\nliving={}\nrecent_log:\n{}",
        living.join(", "),
        chat.join("\n")
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

/// Render one action-feed entry as a styled line. Game actions are bright with a
/// `▶` marker; info reads are dimmed; errors are red. The actor is colour-coded.
fn feed_line(e: &crate::harness::action_log::ActionEntry) -> Line<'static> {
    let ac = actor_color(e.actor.is_host, e.actor.seat);
    let (tool_style, marker) = match e.kind {
        ActionKind::Game => (
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
            "▶ ",
        ),
        ActionKind::Info => (Style::default().fg(Color::DarkGray), "  "),
        ActionKind::Meta => (Style::default().fg(Color::Gray), "  "),
    };
    let mut spans = vec![
        Span::styled(format!("{:>4}s ", e.secs), Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("{:<5}", e.actor.name),
            Style::default().fg(ac).add_modifier(Modifier::BOLD),
        ),
        Span::styled(marker, tool_style),
        Span::styled(e.tool.clone(), tool_style),
    ];
    if !e.summary.is_empty() {
        let sty = if e.kind == ActionKind::Game {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        spans.push(Span::raw(" "));
        spans.push(Span::styled(e.summary.clone(), sty));
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

/// Draw the global action feed (all agents), tail-anchored with `feed_scroll`.
fn draw_action_feed(f: &mut Frame, area: Rect, app: &App) {
    let inner_h = area.height.saturating_sub(2).max(1) as usize;
    // Game-only pulls a wider window so real actions aren't starved by info reads.
    let pull = match app.feed_filter {
        FeedFilter::All => inner_h + app.feed_scroll,
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
    let end = entries.len().saturating_sub(app.feed_scroll);
    let start = end.saturating_sub(inner_h);
    let lines: Vec<Line> = if entries.is_empty() {
        let msg = match app.feed_filter {
            FeedFilter::All => "no actions yet — agents haven't called any tools",
            FeedFilter::GameOnly => "no game actions yet (f = show all)",
        };
        vec![Line::from(Span::styled(msg, Style::default().fg(Color::DarkGray)))]
    } else {
        entries[start..end].iter().map(feed_line).collect()
    };
    let filt = match app.feed_filter {
        FeedFilter::All => "all",
        FeedFilter::GameOnly => "game-only",
    };
    let tail = if app.feed_scroll == 0 { "·live" } else { "·scroll" };
    let title = format!(
        "actions · {filt} · {} total · f=filter g=grimoire {tail}",
        app.action_log.len()
    );
    let p = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(title))
        .wrap(Wrap { trim: false });
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
    install_panic_hook();
    let mut guard = TerminalGuard::enter()?;
    let mut app = App::new();

    let result = loop {
        guard.terminal_mut().draw(|f| draw(f, &mut app))?;
        if app.should_quit {
            break Ok(());
        }
        if app.auto_tick && app.agents.is_some() && app.last_tick.elapsed() >= app.tick_interval {
            app.do_tick();
            app.last_tick = Instant::now();
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
                Event::Resize(_, _)
                | Event::FocusGained
                | Event::FocusLost
                | Event::Paste(_) => {}
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

fn draw_setup(f: &mut Frame, area: Rect, app: &App) {
    let mcp = resolve_agent_mcp_bin_for_display(&app.cfg);
    let mcp_note = if agent_mcp_bin_ok(&app.cfg) {
        "ok"
    } else {
        "MISSING — cargo build --bins"
    };
    let text = format!(
        "Player count: {pc}   →  {tot} headless Grok sessions (host + players)\n\n\
         Model:       {model}\n\
         Grok binary: {grok}\n\
         Agent MCP:   {mcp}  ({mcp_note})\n\
         Work root:   {work}\n\n\
         Controls:  ↑/↓  change player count\n\
                    Enter  create game + spawn agents\n\
                    q      quit (kills agents, removes workdirs)\n\n\
         Each agent workdir gets .grok/config.toml → botc-agent-mcp\n\
         (token-scoped) → Unix socket → shared in-process engine.\n\
         Build first: cargo build --bins && cargo run --bin botc-tui",
        pc = app.player_count,
        tot = app.player_count + 1,
        model = app.cfg.model,
        grok = app.cfg.grok_bin.display(),
        mcp = mcp.display(),
        work = app.cfg.work_root.display(),
    );
    let p = Paragraph::new(text)
        .block(Block::default().borders(Borders::ALL).title("setup"))
        .wrap(Wrap { trim: false });
    f.render_widget(p, area);
}

fn draw_monitor(f: &mut Frame, area: Rect, app: &mut App) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(22),
            Constraint::Percentage(38),
            Constraint::Percentage(40),
        ])
        .split(area);

    // Mouse hit targets (click agents / click-or-scroll stream).
    app.hit_agents = cols[0];
    app.hit_stream = cols[2];

    // Agents list with a live status glyph: ● running (green) / ○ idle (grey).
    let items: Vec<ListItem> = if let Some(pool) = app.agents.as_ref() {
        pool.agents
            .iter()
            .enumerate()
            .map(|(i, a)| {
                let (label, seat) = match a.config.role {
                    AgentRole::Host => ("Host".to_string(), None),
                    AgentRole::Player { seat } => {
                        (format!("P{} {}", seat.0, a.config.display_name), Some(seat.0))
                    }
                };
                let running = *a.running.lock().unwrap();
                let (glyph, gstyle) = if running {
                    ("●", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))
                } else {
                    ("○", Style::default().fg(Color::DarkGray))
                };
                let selected = i == app.selected_agent;
                let label_style = if selected {
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(actor_color(seat.is_none(), seat))
                };
                let mark = if selected { "▶ " } else { "  " };
                ListItem::new(Line::from(vec![
                    Span::styled(glyph, gstyle),
                    Span::raw(" "),
                    Span::styled(mark, label_style),
                    Span::styled(label, label_style),
                ]))
            })
            .collect()
    } else {
        vec![ListItem::new("no agents")]
    };
    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title("agents · ●=running"),
    );
    f.render_widget(list, cols[0]);

    // Center: action feed by default, host grimoire on `g`.
    if app.show_grimoire {
        let host_p = Paragraph::new(app.snapshot_host())
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("grimoire (host) · g=feed"),
            )
            .wrap(Wrap { trim: false });
        f.render_widget(host_p, cols[1]);
    } else {
        draw_action_feed(f, cols[1], app);
    }

    let agent_title = app
        .agents
        .as_ref()
        .and_then(|p| p.agents.get(app.selected_agent))
        .map(|a| match a.config.role {
            AgentRole::Host => "stream: Storyteller".into(),
            AgentRole::Player { seat } => format!("stream: seat{}", seat.0),
        })
        .unwrap_or_else(|| "stream".into());

    // #45/#53: tail-anchor in *wrapped visual rows* (Paragraph::scroll unit), not
    // logical lines — long Grok prose wraps in the ~40% stream column constantly.
    let log_text = app.agent_log_text();
    let inner_w = cols[2].width.saturating_sub(2);
    let view_h = cols[2].height.saturating_sub(2) as usize;
    let row_count = stream_wrapped_row_count(&log_text, inner_w);
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
    let log_p = Paragraph::new(log_text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(
                    "{agent_title} {tail_mark} {think_mark}  click·h  wheel·scroll"
                )),
        )
        .wrap(Wrap { trim: false })
        .scroll((scroll_y_u16, 0));
    f.render_widget(log_p, cols[2]);
}

/// Format agent log lines for the stream pane.
///
/// When `expand_think` is false (default), consecutive `[think] …` blocks collapse
/// to a one-line summary so text / tool / error / turn-end lines (game actions)
/// stay visible while tabbing agents. Press `h` to expand.
pub fn format_stream_for_display(entries: &[String], expand_think: bool) -> String {
    if expand_think {
        return entries
            .iter()
            .map(|e| {
                if e.starts_with("[think]") {
                    format!("▸ {e}")
                } else {
                    e.clone()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
    }

    let mut out: Vec<String> = Vec::new();
    let mut think_n = 0usize;
    let mut preview = String::new();

    let flush_think = |out: &mut Vec<String>, think_n: &mut usize, preview: &mut String| {
        if *think_n == 0 {
            return;
        }
        let snip = if preview.is_empty() {
            String::new()
        } else {
            let mut s: String = preview.chars().take(48).collect();
            if preview.chars().count() > 48 {
                s.push('…');
            }
            format!(" “{s}”")
        };
        let label = if *think_n == 1 {
            format!("▾ [think] 1 block{snip}  (h expand)")
        } else {
            format!("▾ [think] {think_n} blocks{snip}  (h expand)")
        };
        out.push(label);
        *think_n = 0;
        preview.clear();
    };

    for e in entries {
        if let Some(rest) = e.strip_prefix("[think]") {
            think_n += 1;
            if preview.is_empty() {
                preview = rest.trim_start().to_string();
            }
        } else {
            flush_think(&mut out, &mut think_n, &mut preview);
            out.push(e.clone());
        }
    }
    flush_think(&mut out, &mut think_n, &mut preview);
    out.join("\n")
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

    #[test]
    fn format_stream_collapses_think_by_default() {
        let entries = vec![
            "[think] I should poison someone".into(),
            "[think] maybe seat 2".into(),
            "Calling night_action on seat 2".into(),
            "[turn end]".into(),
        ];
        let collapsed = format_stream_for_display(&entries, false);
        assert!(
            collapsed.contains("▾ [think] 2 blocks"),
            "collapsed summary missing: {collapsed}"
        );
        // Only a one-line summary (optional short preview), not the second think line.
        assert!(
            !collapsed.contains("maybe seat 2"),
            "collapsed stream must not dump every think body: {collapsed}"
        );
        assert_eq!(
            collapsed.lines().filter(|l| l.starts_with("▾ [think]")).count(),
            1,
            "consecutive thinks must merge to one summary line: {collapsed}"
        );
        assert!(
            collapsed.contains("Calling night_action on seat 2"),
            "action text must remain visible: {collapsed}"
        );
        assert!(collapsed.contains("[turn end]"));

        let expanded = format_stream_for_display(&entries, true);
        assert!(expanded.contains("▸ [think] I should poison someone"));
        assert!(expanded.contains("▸ [think] maybe seat 2"));
        assert!(expanded.contains("Calling night_action on seat 2"));
    }

    #[test]
    fn format_stream_interleaves_think_and_action() {
        let entries = vec![
            "[think] first".into(),
            "action A".into(),
            "[think] second".into(),
            "action B".into(),
        ];
        let collapsed = format_stream_for_display(&entries, false);
        let lines: Vec<_> = collapsed.lines().collect();
        assert_eq!(lines.len(), 4, "got {lines:?}");
        assert!(lines[0].starts_with("▾ [think] 1 block"));
        assert_eq!(lines[1], "action A");
        assert!(lines[2].starts_with("▾ [think] 1 block"));
        assert_eq!(lines[3], "action B");
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
}
