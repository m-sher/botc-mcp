//! Ratatui monitoring UI for the multi-agent harness.

use std::io::{self, Stdout};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind,
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

use crate::game::{Game, GameId, SeatId, StartOpts, StChoiceMode};
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
        }
    }

    fn on_key(&mut self, code: KeyCode) {
        // Only explicit keyboard actions. Mouse wheel / trackpad scroll is ignored
        // in the event loop (EnableMouseCapture + drop Event::Mouse).
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
            KeyCode::Char(' ') if self.agents.is_some() => {
                self.do_tick();
                self.last_tick = Instant::now();
            }
            // Keyboard-only log navigation (not mouse scroll). 0 = live tail.
            KeyCode::PageUp if self.focus == Focus::Monitor => {
                self.scroll_from_bottom = self.scroll_from_bottom.saturating_add(5);
            }
            KeyCode::PageDown if self.focus == Focus::Monitor => {
                self.scroll_from_bottom = self.scroll_from_bottom.saturating_sub(5);
            }
            KeyCode::Home if self.focus == Focus::Monitor => {
                // Jump to live tail.
                self.scroll_from_bottom = 0;
            }
            _ => {}
        }
    }

    fn launch(&mut self) {
        if self.agents.is_some() {
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

        match SocketServer::start(Arc::clone(&self.store), &self.cfg.socket_path) {
            Ok(s) => self.socket = Some(s),
            Err(e) => {
                self.status = format!("socket: {e}");
                return;
            }
        }

        {
            let mut st = self.store.lock().unwrap();
            let g = st.get_mut(GameId(game_id)).unwrap();
            if let Err(e) = tools::start_game(
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
            ) {
                self.status = format!("start_game: {e}");
                return;
            }
        }

        self.game_id = Some(game_id);
        self.host_token = Some(created.host_token.clone());

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
                        "Game {game_id} · kicked {spawned}/{} · Space=tick t=auto Tab=agent q=quit",
                        self.player_count + 1
                    );
                    self.auto_tick = true;
                    self.last_tick = Instant::now();
                    self.focus = Focus::Monitor;
                    self.scroll_from_bottom = 0;
                    self.agents = Some(pool);
                }
                Err(e) => {
                    self.status = format!("kickoff: {e}");
                    self.agents = Some(pool);
                }
            },
            Err(e) => self.status = format!("prepare: {e}"),
        }
    }

    fn do_tick(&mut self) {
        let Some(gid) = self.game_id else {
            return;
        };
        let (summary, hint) = self.public_summary_and_hint(gid);
        if let Some(pool) = self.agents.as_mut() {
            let total = pool.agents.len();
            match pool.tick_all(&summary, &hint) {
                Ok(spawned) => {
                    let skipped = total.saturating_sub(spawned);
                    self.status = if skipped == 0 {
                        format!("Ticked {spawned}/{total} agents.")
                    } else {
                        format!(
                            "Ticked {spawned}/{total} agents ({skipped} still running previous tick)."
                        )
                    };
                }
                Err(e) => self.status = format!("tick error: {e}"),
            }
        }
    }

    fn public_summary_and_hint(&self, game_id: u64) -> (String, String) {
        let st = self.store.lock().unwrap();
        let Some(g) = st.get(GameId(game_id)) else {
            return ("(no game)".into(), String::new());
        };
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

    /// Full agent log as joined text (newest at end). Caller applies tail-anchor scroll (#45).
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
        log[start..].join("\n")
    }

    fn shutdown(&mut self) {
        // Stop agents first (kills children + removes work root with tokens).
        if let Some(mut pool) = self.agents.take() {
            pool.stop_all();
        }
        // Then stop socket (non-blocking accept survives missing path).
        if let Some(s) = self.socket.take() {
            s.stop();
        }
    }
}

/// RAII guard: restores terminal raw-mode / alternate screen on Drop or panic (#49).
struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    restored: bool,
}

impl TerminalGuard {
    fn enter() -> io::Result<Self> {
        enable_raw_mode()?;
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
        guard.terminal_mut().draw(|f| draw(f, &app))?;
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
                    // Press only — ignore Release/Repeat (and anything scroll-related).
                    if key.kind == KeyEventKind::Press {
                        app.on_key(key.code);
                    }
                }
                // Explicit no-op: wheel, trackpad, clicks, resize, paste, focus.
                Event::Mouse(_)
                | Event::Resize(_, _)
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

fn draw(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(3),
        ])
        .split(f.area());

    let title = Paragraph::new(Line::from(vec![
        Span::styled(
            " botc-tui ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(format!(
            " players={} sessions={}  [{}]",
            app.player_count,
            app.player_count + 1,
            match app.focus {
                Focus::Setup => "SETUP",
                Focus::Monitor => "MONITOR",
            }
        )),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title("Trouble Brewing · multi-agent monitor"),
    );
    f.render_widget(title, chunks[0]);

    match app.focus {
        Focus::Setup => draw_setup(f, chunks[1], app),
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

fn draw_monitor(f: &mut Frame, area: Rect, app: &App) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(26),
            Constraint::Percentage(34),
            Constraint::Percentage(40),
        ])
        .split(area);

    let items: Vec<ListItem> = if let Some(pool) = app.agents.as_ref() {
        pool.agents
            .iter()
            .enumerate()
            .map(|(i, a)| {
                let label = match a.config.role {
                    AgentRole::Host => "HOST  Storyteller".into(),
                    AgentRole::Player { seat } => {
                        format!("SEAT{} {}", seat.0, a.config.display_name)
                    }
                };
                let running = *a.running.lock().unwrap();
                let mark = if i == app.selected_agent { "▶" } else { " " };
                let style = if i == app.selected_agent {
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                ListItem::new(format!(
                    "{mark} {label} {}",
                    if running { "…run" } else { "idle" }
                ))
                .style(style)
            })
            .collect()
    } else {
        vec![ListItem::new("no agents")]
    };
    let list =
        List::new(items).block(Block::default().borders(Borders::ALL).title("agents (Tab)"));
    f.render_widget(list, cols[0]);

    let host_p = Paragraph::new(app.snapshot_host())
        .block(Block::default().borders(Borders::ALL).title("grimoire (host)"))
        .wrap(Wrap { trim: false });
    f.render_widget(host_p, cols[1]);

    let agent_title = app
        .agents
        .as_ref()
        .and_then(|p| p.agents.get(app.selected_agent))
        .map(|a| match a.config.role {
            AgentRole::Host => "stream: Storyteller".into(),
            AgentRole::Player { seat } => format!("stream: seat{}", seat.0),
        })
        .unwrap_or_else(|| "stream".into());

    // #45: tail-anchor — keep newest lines in view via Paragraph::scroll.
    let log_text = app.agent_log_text();
    let line_count = if log_text.is_empty() {
        0
    } else {
        log_text.lines().count()
    };
    // Inner height excludes borders (top+bottom).
    let view_h = cols[2].height.saturating_sub(2) as usize;
    let max_scroll_top = line_count.saturating_sub(view_h);
    // scroll_from_bottom=0 → show the end; larger → look further up.
    let scroll_y = max_scroll_top.saturating_sub(app.scroll_from_bottom.min(max_scroll_top));
    let tail_mark = if app.scroll_from_bottom == 0 {
        "·live"
    } else {
        "·scroll"
    };
    let log_p = Paragraph::new(log_text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!("{agent_title} {tail_mark}")),
        )
        .wrap(Wrap { trim: false })
        .scroll((scroll_y as u16, 0));
    f.render_widget(log_p, cols[2]);
}

/// Pure helper for unit tests: compute vertical scroll that keeps the live tail
/// (or an offset from it) in view (#45).
pub fn stream_scroll_y(line_count: usize, view_h: usize, scroll_from_bottom: usize) -> usize {
    let max_scroll_top = line_count.saturating_sub(view_h);
    max_scroll_top.saturating_sub(scroll_from_bottom.min(max_scroll_top))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_scroll_defaults_to_tail() {
        // 50 lines, 10-row pane, scroll_from_bottom=0 → scroll so last 10 show.
        assert_eq!(stream_scroll_y(50, 10, 0), 40);
        assert_eq!(stream_scroll_y(5, 10, 0), 0); // fits entirely
        assert_eq!(stream_scroll_y(50, 10, 5), 35); // look 5 lines up from tail
        assert_eq!(stream_scroll_y(50, 10, 999), 0); // clamped to top
    }
}
