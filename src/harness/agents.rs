//! Spawn headless Grok sessions with per-agent MCP proxy config.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde_json::Value;

use crate::game::SeatId;
use crate::harness::prompts;
use crate::harness::scheduler::SchedTarget;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentRole {
    Host,
    Player { seat: SeatId },
}

#[derive(Debug, Clone)]
pub struct AgentConfig {
    pub role: AgentRole,
    pub display_name: String,
    pub token: String,
    pub game_id: u64,
}

#[derive(Debug, Clone)]
pub struct HarnessConfig {
    /// Number of player seats (5–15). Host is +1 session.
    pub player_count: usize,
    pub model: String,
    pub grok_bin: PathBuf,
    pub agent_mcp_bin: PathBuf,
    pub work_root: PathBuf,
    pub socket_path: PathBuf,
    pub max_turns_per_tick: u32,
    pub seed: Option<u64>,
    pub st_choice_mode: String,
}

impl Default for HarnessConfig {
    fn default() -> Self {
        Self {
            player_count: 5,
            model: "grok-build".into(),
            grok_bin: PathBuf::from("grok"),
            agent_mcp_bin: PathBuf::from("botc-agent-mcp"),
            work_root: PathBuf::from("/tmp/botc-harness"),
            socket_path: PathBuf::from("/tmp/botc-harness/engine.sock"),
            max_turns_per_tick: 12,
            seed: Some(42),
            st_choice_mode: "host_first".into(),
        }
    }
}

/// Per-agent child process coordination (#46 / #52).
///
/// Critical: **never** hold the slot mutex across a blocking `Child::wait()`.
/// The waiter uses `try_wait` + short sleeps; `take_and_kill` must be able to
/// acquire the lock while a child is still running and deliver SIGKILL.
#[derive(Debug)]
enum ChildState {
    Empty,
    Running(Child),
    /// Reaped by kill or natural exit; waiter may still consume the status.
    Exited(std::process::ExitStatus),
}

impl Default for ChildState {
    fn default() -> Self {
        Self::Empty
    }
}

#[derive(Debug, Default)]
struct ChildSlot {
    inner: Mutex<ChildState>,
}

impl ChildSlot {
    fn store(&self, child: Child) {
        *self.inner.lock().unwrap() = ChildState::Running(child);
    }

    /// Kill + reap if still running. Must not block on a waiter-held lock (#52).
    fn take_and_kill(&self) {
        let mut g = self.inner.lock().unwrap();
        if let ChildState::Running(mut c) = std::mem::replace(&mut *g, ChildState::Empty) {
            let _ = c.kill();
            match c.wait() {
                Ok(st) => *g = ChildState::Exited(st),
                Err(_) => *g = ChildState::Empty,
            }
        }
    }

    /// Block until the child exits (or is killed/reaped). Only holds the mutex
    /// briefly around `try_wait` / state reads — never across a blocking wait.
    fn wait_exit(&self) -> Option<std::process::ExitStatus> {
        loop {
            let mut g = self.inner.lock().unwrap();
            match &mut *g {
                ChildState::Empty => return None,
                ChildState::Exited(st) => {
                    let st = *st;
                    *g = ChildState::Empty;
                    return Some(st);
                }
                ChildState::Running(c) => match c.try_wait() {
                    Ok(Some(st)) => {
                        *g = ChildState::Empty;
                        return Some(st);
                    }
                    Ok(None) => {
                        drop(g);
                        thread::sleep(Duration::from_millis(20));
                    }
                    Err(_) => {
                        *g = ChildState::Empty;
                        return None;
                    }
                },
            }
        }
    }
}

#[derive(Debug)]
pub struct LiveAgent {
    pub config: AgentConfig,
    /// Mutable so a failed first run can mint a fresh UUID (#47).
    pub session_id: Arc<Mutex<String>>,
    pub workdir: PathBuf,
    /// Live stream buffer (kinded lines for coloured, un-chunked display).
    pub log: Arc<Mutex<Vec<LogLine>>>,
    /// True while a headless Grok child for this agent is alive.
    pub running: Arc<Mutex<bool>>,
    /// True only after a **successful** first headless run (#47).
    pub session_started: Arc<Mutex<bool>>,
    child: Arc<ChildSlot>,
}

pub struct AgentPool {
    pub agents: Vec<LiveAgent>,
    pub cfg: HarnessConfig,
}

/// Result of attempting to tick one agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickOutcome {
    Spawned,
    SkippedStillRunning,
}

impl AgentPool {
    pub fn prepare(cfg: &HarnessConfig, agents: Vec<AgentConfig>) -> std::io::Result<Self> {
        fs::create_dir_all(&cfg.work_root)?;
        let mut live = Vec::new();
        for a in agents {
            let label = match a.role {
                AgentRole::Host => "host".to_string(),
                AgentRole::Player { seat } => format!("seat{}", seat.0),
            };
            let workdir = cfg.work_root.join(&label);
            fs::create_dir_all(workdir.join(".grok"))?;
            write_agent_mcp_config(&workdir, cfg, &a.token, a.game_id, a.role)?;
            let session_id = uuid::Uuid::new_v4().to_string();
            live.push(LiveAgent {
                config: a,
                session_id: Arc::new(Mutex::new(session_id)),
                workdir,
                log: Arc::new(Mutex::new(Vec::new())),
                running: Arc::new(Mutex::new(false)),
                session_started: Arc::new(Mutex::new(false)),
                child: Arc::new(ChildSlot::default()),
            });
        }
        Ok(Self {
            agents: live,
            cfg: cfg.clone(),
        })
    }

    /// Kick off every agent. One spawn failure does not abort the rest (#48 borderline).
    pub fn kickoff_all(&mut self, n_players: usize) -> std::io::Result<usize> {
        let mut n = 0;
        let mut last_err: Option<std::io::Error> = None;
        for agent in &mut self.agents {
            let prompt = match agent.config.role {
                AgentRole::Host => prompts::host_kickoff(
                    agent.config.game_id,
                    n_players,
                    &self.cfg.st_choice_mode,
                ),
                AgentRole::Player { seat } => prompts::player_kickoff(
                    &agent.config.display_name,
                    seat,
                    agent.config.game_id,
                    n_players,
                ),
            };
            match spawn_grok_tick(&self.cfg, agent, &prompt) {
                Ok(TickOutcome::Spawned) => n += 1,
                Ok(TickOutcome::SkippedStillRunning) => {}
                Err(e) => last_err = Some(e),
            }
        }
        if n == 0 {
            if let Some(e) = last_err {
                return Err(e);
            }
        }
        Ok(n)
    }

    /// One more multi-turn tick for every agent.
    pub fn tick_all(&mut self, public_summary: &str, host_hint: &str) -> std::io::Result<usize> {
        let mut n = 0;
        let mut last_err: Option<std::io::Error> = None;
        for agent in &mut self.agents {
            let prompt = match agent.config.role {
                AgentRole::Host => {
                    prompts::host_tick(agent.config.game_id, public_summary, host_hint)
                }
                AgentRole::Player { seat } => prompts::player_tick(
                    &agent.config.display_name,
                    seat,
                    agent.config.game_id,
                    public_summary,
                ),
            };
            match spawn_grok_tick(&self.cfg, agent, &prompt) {
                Ok(TickOutcome::Spawned) => n += 1,
                Ok(TickOutcome::SkippedStillRunning) => {}
                Err(e) => last_err = Some(e),
            }
        }
        if n == 0 {
            if let Some(e) = last_err {
                return Err(e);
            }
        }
        Ok(n)
    }

    /// Turn-routed tick (#60): run only the agents the scheduler selected this
    /// cycle, each with a targeted role/phase prompt. Skips agents whose previous
    /// tick is still running. Returns how many were spawned.
    pub fn tick_scheduled(
        &mut self,
        targets: &[SchedTarget],
        public_summary: &str,
        host_hint: &str,
    ) -> std::io::Result<usize> {
        let mut n = 0;
        let mut last_err: Option<std::io::Error> = None;
        for target in targets {
            let idx = match target {
                SchedTarget::Host(_) => self
                    .agents
                    .iter()
                    .position(|a| matches!(a.config.role, AgentRole::Host)),
                SchedTarget::Player { seat, .. } => {
                    let seat = *seat;
                    self.agents
                        .iter()
                        .position(|a| matches!(a.config.role, AgentRole::Player { seat: s } if s == seat))
                }
            };
            let Some(idx) = idx else { continue };
            let prompt = match target {
                SchedTarget::Host(task) => {
                    let a = &self.agents[idx];
                    prompts::host_task_tick(a.config.game_id, task, public_summary, host_hint)
                }
                SchedTarget::Player { seat, task } => {
                    let a = &self.agents[idx];
                    prompts::player_task_tick(
                        &a.config.display_name,
                        *seat,
                        a.config.game_id,
                        task,
                        public_summary,
                    )
                }
            };
            match spawn_grok_tick(&self.cfg, &mut self.agents[idx], &prompt) {
                Ok(TickOutcome::Spawned) => n += 1,
                Ok(TickOutcome::SkippedStillRunning) => {}
                Err(e) => last_err = Some(e),
            }
        }
        if n == 0 {
            if let Some(e) = last_err {
                return Err(e);
            }
        }
        Ok(n)
    }

    /// Kill all grok children and remove workdirs containing tokens (#46).
    pub fn stop_all(&mut self) {
        for agent in &mut self.agents {
            *agent.running.lock().unwrap() = false;
            agent.child.take_and_kill();
        }
        // Best-effort secret cleanup.
        if self.cfg.work_root.exists() {
            let _ = fs::remove_dir_all(&self.cfg.work_root);
        }
    }
}

impl Drop for AgentPool {
    fn drop(&mut self) {
        self.stop_all();
    }
}

fn write_agent_mcp_config(
    workdir: &Path,
    cfg: &HarnessConfig,
    token: &str,
    game_id: u64,
    role: AgentRole,
) -> std::io::Result<()> {
    let mcp_bin = resolve_agent_mcp_bin(cfg);
    let token_path = workdir.join("agent.token");
    fs::write(&token_path, token)?;
    let sock = cfg.socket_path.display();
    let bin = mcp_bin.display();
    let tok = token_path.display();
    let role_s = match role {
        AgentRole::Host => "host",
        AgentRole::Player { .. } => "player",
    };
    let toml = format!(
        r#"# Auto-generated by botc-tui — do not commit.
[mcp_servers.botc]
command = "{bin}"
args = ["--socket", "{sock}", "--token-file", "{tok}", "--game-id", "{game_id}", "--role", "{role_s}"]
enabled = true
startup_timeout_sec = 60
"#
    );
    fs::write(workdir.join(".grok/config.toml"), toml)?;
    Ok(())
}

fn resolve_agent_mcp_bin(cfg: &HarnessConfig) -> PathBuf {
    if cfg.agent_mcp_bin.is_absolute() && cfg.agent_mcp_bin.exists() {
        return cfg.agent_mcp_bin.clone();
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sibling = dir.join("botc-agent-mcp");
            if sibling.exists() {
                return sibling;
            }
        }
    }
    if let Ok(p) = which("botc-agent-mcp") {
        return p;
    }
    cfg.agent_mcp_bin.clone()
}

/// Build the `grok` argv for one headless tick (pure — unit-tested without spawning).
///
/// Uses a single auto-approve flag (`--yolo`). Do **not** also pass `--always-approve`
/// (alias of the same clap flag → "cannot be used multiple times").
pub fn build_grok_tick_args(
    cfg: &HarnessConfig,
    workdir: &Path,
    prompt_file: &Path,
    session_id: &str,
    session_started: bool,
) -> Vec<String> {
    let mut args = vec![
        "--prompt-file".into(),
        prompt_file.display().to_string(),
        "-m".into(),
        cfg.model.clone(),
        "--cwd".into(),
        workdir.display().to_string(),
        "--max-turns".into(),
        cfg.max_turns_per_tick.to_string(),
        "--output-format".into(),
        "streaming-json".into(),
        // Single auto-approve flag only (--yolo == --always-approve).
        "--yolo".into(),
        "--no-subagents".into(),
        "--disable-web-search".into(),
    ];
    if session_started {
        args.push("--resume".into());
        args.push(session_id.into());
    } else {
        args.push("--session-id".into());
        args.push(session_id.into());
    }
    args
}

fn spawn_grok_tick(
    cfg: &HarnessConfig,
    agent: &mut LiveAgent,
    prompt: &str,
) -> std::io::Result<TickOutcome> {
    let label = agent_label(&agent.config.role);
    if *agent.running.lock().unwrap() {
        crate::dlog!("SPAWN {label} SKIPPED (previous tick still running)");
        return Ok(TickOutcome::SkippedStillRunning);
    }

    let prompt_file = agent.workdir.join("prompt.txt");
    fs::write(&prompt_file, prompt)?;

    let session_started = *agent.session_started.lock().unwrap();
    let session_id = agent.session_id.lock().unwrap().clone();

    let args = build_grok_tick_args(
        cfg,
        &agent.workdir,
        &prompt_file,
        &session_id,
        session_started,
    );
    crate::dlog!(
        "SPAWN {label} mode={} session={} prompt_first_line={:?} argv=[{}]",
        if session_started { "resume" } else { "fresh" },
        session_id,
        prompt.lines().next().unwrap_or(""),
        args.join(" ")
    );

    let mut cmd = Command::new(&cfg.grok_bin);
    cmd.args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            let mut g = agent.log.lock().unwrap();
            push_full_line(
                &mut g,
                LineKind::System,
                format!("ERROR failed to spawn {}: {e}", cfg.grok_bin.display()),
            );
            return Err(e);
        }
    };

    let log = Arc::clone(&agent.log);
    let running_flag = Arc::clone(&agent.running);
    let session_started_flag = Arc::clone(&agent.session_started);
    let session_id_slot = Arc::clone(&agent.session_id);
    let child_slot = Arc::clone(&agent.child);
    // #56: grok exits 1 on normal --max-turns; gate resume on stream evidence of a
    // real session (end / max_turns_reached), not process exit code.
    let session_established = Arc::new(AtomicBool::new(false));
    *running_flag.lock().unwrap() = true;

    if let Some(stdout) = child.stdout.take() {
        let log = Arc::clone(&log);
        let established = Arc::clone(&session_established);
        thread::spawn(move || {
            use std::io::BufRead;
            let reader = std::io::BufReader::new(stdout);
            // Process each streaming-json event as it arrives → live display.
            for line in reader.lines().flatten() {
                if stream_line_establishes_session(&line) {
                    established.store(true, Ordering::SeqCst);
                }
                apply_stream_event(&log, &line);
            }
        });
    }
    if let Some(stderr) = child.stderr.take() {
        let log = Arc::clone(&agent.log);
        thread::spawn(move || {
            use std::io::BufRead;
            let reader = std::io::BufReader::new(stderr);
            for line in reader.lines().flatten() {
                let mut g = log.lock().unwrap();
                push_full_line(&mut g, LineKind::Stderr, line);
            }
        });
    }

    child_slot.store(child);

    // Waiter: session_started from stream evidence (#47/#56), not exit code alone.
    let child_slot_w = Arc::clone(&child_slot);
    let exit_label = label.clone();
    thread::spawn(move || {
        let status = child_slot_w.wait_exit();
        // stdout reader may still be finishing; give it a brief moment.
        for _ in 0..10 {
            if session_established.load(Ordering::SeqCst) {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let established = session_established.load(Ordering::SeqCst);
        let was_started = *session_started_flag.lock().unwrap();
        let mut regenerated = false;
        if established {
            *session_started_flag.lock().unwrap() = true;
        } else if !was_started {
            // No session was created (auth/spawn death) — next tick uses a fresh --session-id.
            *session_id_slot.lock().unwrap() = uuid::Uuid::new_v4().to_string();
            regenerated = true;
        }
        *running_flag.lock().unwrap() = false;
        crate::dlog!(
            "EXIT {exit_label} status={:?} established={established} session_started(now)={} regenerated_id={regenerated}",
            status.map(|s| s.code()),
            established || was_started
        );
    });

    Ok(TickOutcome::Spawned)
}

/// Short label for an agent role (for logs / feed).
fn agent_label(role: &AgentRole) -> String {
    match role {
        AgentRole::Host => "Host".to_string(),
        AgentRole::Player { seat } => format!("P{}", seat.0),
    }
}

/// True if a streaming-json line means grok created/used a session (#56).
///
/// `max_turns_reached` and `end` both mean the session is on disk and resumable,
/// even when the process exits non-zero.
pub fn stream_line_establishes_session(line: &str) -> bool {
    let line = line.trim();
    if line.is_empty() {
        return false;
    }
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        return false;
    };
    matches!(
        v.get("type").and_then(|t| t.as_str()),
        Some("end") | Some("max_turns_reached")
    )
}

/// Kind of a stream line — used only for colouring the display (no in-text tags).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    /// Model assistant text.
    Text,
    /// Model reasoning ("thinking").
    Thought,
    /// Child process stderr.
    Stderr,
    /// Harness / turn notices (turn end, errors).
    System,
}

/// One display line of an agent's stream (kinded for colour; grown live).
#[derive(Debug, Clone)]
pub struct LogLine {
    pub kind: LineKind,
    pub text: String,
    /// True once a newline closed the line (no further appends).
    pub closed: bool,
}

const STREAM_LOG_CAP: usize = 800;

fn cap_log(log: &mut Vec<LogLine>) {
    if log.len() > STREAM_LOG_CAP {
        let drain = log.len() - STREAM_LOG_CAP;
        log.drain(0..drain);
    }
}

/// Append a streaming chunk **live**: extend the current open line of the same
/// kind, breaking to a new line on `\n`. The visible tail updates on every chunk
/// (no buffering until a block ends), so the stream shows text as it arrives.
pub fn append_chunk(log: &mut Vec<LogLine>, kind: LineKind, data: &str) {
    if data.is_empty() {
        return;
    }
    let cont = matches!(log.last(), Some(l) if l.kind == kind && !l.closed);
    if !cont {
        log.push(LogLine {
            kind,
            text: String::new(),
            closed: false,
        });
    }
    let mut parts = data.split('\n');
    if let Some(first) = parts.next() {
        if let Some(last) = log.last_mut() {
            last.text.push_str(first);
        }
    }
    for part in parts {
        if let Some(last) = log.last_mut() {
            last.closed = true;
        }
        log.push(LogLine {
            kind,
            text: part.to_string(),
            closed: false,
        });
    }
    cap_log(log);
}

/// Push a complete standalone line (stderr line / system notice).
pub fn push_full_line(log: &mut Vec<LogLine>, kind: LineKind, text: String) {
    if let Some(last) = log.last_mut() {
        last.closed = true;
    }
    log.push(LogLine {
        kind,
        text,
        closed: true,
    });
    cap_log(log);
}

/// Parse one grok `streaming-json` line and append it to `log` live.
pub fn apply_stream_event(log: &Mutex<Vec<LogLine>>, line: &str) {
    let line = line.trim();
    if line.is_empty() {
        return;
    }
    let mut guard = log.lock().unwrap();
    let Ok(v) = serde_json::from_str::<Value>(line) else {
        push_full_line(&mut guard, LineKind::Text, line.to_string());
        return;
    };
    let data = |v: &Value| {
        v.get("data")
            .and_then(|d| d.as_str())
            .unwrap_or("")
            .to_string()
    };
    match v.get("type").and_then(|t| t.as_str()) {
        Some("text") => append_chunk(&mut guard, LineKind::Text, &data(&v)),
        Some("thought") => append_chunk(&mut guard, LineKind::Thought, &data(&v)),
        Some("end") => push_full_line(&mut guard, LineKind::System, "— turn end —".into()),
        Some("max_turns_reached") => {
            push_full_line(&mut guard, LineKind::System, "— max turns reached —".into())
        }
        Some("error") => {
            let msg = v.get("message").and_then(|m| m.as_str()).unwrap_or("?");
            push_full_line(&mut guard, LineKind::System, format!("error: {msg}"));
        }
        Some(other) => {
            // Surface other events (tool calls, etc.) compactly rather than dropping them.
            let name = v
                .get("name")
                .or_else(|| v.get("tool"))
                .and_then(|x| x.as_str());
            let note = match name {
                Some(n) => format!("· {other}: {n}"),
                None => format!("· {other}"),
            };
            push_full_line(&mut guard, LineKind::System, note);
        }
        None => {}
    }
}

/// Best-effort: resolve grok binary.
pub fn find_grok() -> PathBuf {
    if let Ok(p) = which("grok") {
        return p;
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    let candidate = PathBuf::from(home).join(".grok/bin/grok");
    if candidate.exists() {
        candidate
    } else {
        PathBuf::from("grok")
    }
}

fn which(name: &str) -> Result<PathBuf, ()> {
    let path = std::env::var_os("PATH").ok_or(())?;
    for dir in std::env::split_paths(&path) {
        let p = dir.join(name);
        if p.is_file() {
            return Ok(p);
        }
    }
    Err(())
}

pub fn find_agent_mcp_bin() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        let sibling = exe.parent().map(|d| d.join("botc-agent-mcp"));
        if let Some(p) = sibling {
            if p.exists() {
                return p;
            }
        }
    }
    if let Ok(p) = which("botc-agent-mcp") {
        return p;
    }
    PathBuf::from("botc-agent-mcp")
}

/// True if the resolved agent MCP binary exists on disk (#50).
pub fn agent_mcp_bin_ok(cfg: &HarnessConfig) -> bool {
    resolve_agent_mcp_bin(cfg).exists()
}

/// Resolved path used for setup UI / error messages (#50).
pub fn resolve_agent_mcp_bin_for_display(cfg: &HarnessConfig) -> PathBuf {
    resolve_agent_mcp_bin(cfg)
}

pub fn sleep_ms(ms: u64) {
    thread::sleep(Duration::from_millis(ms));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grok_args_use_yolo_once_not_always_approve() {
        let cfg = HarnessConfig::default();
        let args = build_grok_tick_args(
            &cfg,
            Path::new("/tmp/wd"),
            Path::new("/tmp/wd/prompt.txt"),
            "11111111-1111-1111-1111-111111111111",
            false,
        );
        let yolo_count = args.iter().filter(|a| *a == "--yolo").count();
        let always = args.iter().filter(|a| *a == "--always-approve").count();
        assert_eq!(yolo_count, 1, "expected single --yolo: {args:?}");
        assert_eq!(always, 0, "must not pass --always-approve (alias): {args:?}");
        assert!(args.contains(&"--session-id".into()));
        assert!(!args.contains(&"--resume".into()));
    }

    #[test]
    fn grok_args_resume_after_session_started() {
        let cfg = HarnessConfig::default();
        let args = build_grok_tick_args(
            &cfg,
            Path::new("/tmp/wd"),
            Path::new("/tmp/wd/prompt.txt"),
            "11111111-1111-1111-1111-111111111111",
            true,
        );
        assert!(args.contains(&"--resume".into()));
        assert!(!args.contains(&"--session-id".into()));
        assert_eq!(args.iter().filter(|a| *a == "--yolo").count(), 1);
        assert_eq!(args.iter().filter(|a| *a == "--always-approve").count(), 0);
    }

    #[test]
    fn no_duplicate_flag_pairs() {
        let cfg = HarnessConfig::default();
        let args = build_grok_tick_args(
            &cfg,
            Path::new("/tmp/wd"),
            Path::new("/tmp/p"),
            "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
            false,
        );
        for flag in [
            "--prompt-file",
            "-m",
            "--cwd",
            "--max-turns",
            "--output-format",
            "--yolo",
            "--no-subagents",
            "--disable-web-search",
            "--session-id",
        ] {
            let c = args.iter().filter(|a| a.as_str() == flag).count();
            assert!(c <= 1, "flag {flag} appears {c} times: {args:?}");
        }
    }

    #[test]
    fn max_turns_stream_establishes_session() {
        // #56: exit 1 on max-turns still means the session exists and is resumable.
        assert!(stream_line_establishes_session(
            r#"{"type":"max_turns_reached"}"#
        ));
        assert!(stream_line_establishes_session(
            r#"{"type":"end","stopReason":"Cancelled","sessionId":"x"}"#
        ));
        assert!(!stream_line_establishes_session(
            r#"{"type":"error","message":"auth failed"}"#
        ));
        assert!(!stream_line_establishes_session("not json"));
        // After establish, next tick must use --resume.
        let cfg = HarnessConfig::default();
        let args = build_grok_tick_args(
            &cfg,
            Path::new("/tmp/wd"),
            Path::new("/tmp/wd/prompt.txt"),
            "11111111-1111-1111-1111-111111111111",
            true, // session_started after max-turns establish
        );
        assert!(args.contains(&"--resume".into()));
        assert!(!args.contains(&"--session-id".into()));
    }

    #[test]
    fn stream_events_append_live_and_coalesce_by_kind() {
        let log = Mutex::new(Vec::<LogLine>::new());
        for line in [
            r#"{"type":"thought","data":"The"}"#,
            r#"{"type":"thought","data":" task"}"#,
            r#"{"type":"thought","data":" is"}"#,
            r#"{"type":"text","data":"Hello"}"#,
            r#"{"type":"text","data":" world"}"#,
            r#"{"type":"end","stopReason":"EndTurn","sessionId":"x"}"#,
        ] {
            apply_stream_event(&log, line);
        }
        let g = log.lock().unwrap();
        // Consecutive same-kind chunks coalesce into one growing line; kinds are
        // kept (for colour) rather than tagged in-text.
        assert_eq!(g.len(), 3);
        assert_eq!(g[0].kind, LineKind::Thought);
        assert_eq!(g[0].text, "The task is");
        assert_eq!(g[1].kind, LineKind::Text);
        assert_eq!(g[1].text, "Hello world");
        assert_eq!(g[2].kind, LineKind::System);
        // No in-text tags.
        assert!(!g.iter().any(|l| l.text.contains("[think]")));
    }

    #[test]
    fn append_chunk_breaks_on_newlines() {
        let mut log = Vec::new();
        append_chunk(&mut log, LineKind::Text, "line one\nline ");
        append_chunk(&mut log, LineKind::Text, "two");
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].text, "line one");
        assert!(log[0].closed);
        assert_eq!(log[1].text, "line two"); // second chunk continued the open line
    }

    #[test]
    fn stop_all_removes_work_root() {
        let id = uuid::Uuid::new_v4();
        let root = std::env::temp_dir().join(format!("botc-stop-test-{id}"));
        let mut cfg = HarnessConfig::default();
        cfg.work_root = root.clone();
        cfg.socket_path = root.join("engine.sock");
        let pool = AgentPool::prepare(
            &cfg,
            vec![AgentConfig {
                role: AgentRole::Host,
                display_name: "ST".into(),
                token: "tok".into(),
                game_id: 1,
            }],
        )
        .unwrap();
        assert!(root.exists());
        assert!(root.join("host/agent.token").exists());
        drop(pool); // Drop → stop_all → remove work root
        assert!(!root.exists(), "work root should be removed on stop");
    }

    /// #52 regression: waiter must not hold the slot lock across blocking wait,
    /// or take_and_kill / stop_all deadlocks while a child is still running.
    #[test]
    fn take_and_kill_while_waiter_running_does_not_deadlock() {
        let slot = Arc::new(ChildSlot::default());
        let child = Command::new("sleep")
            .arg("30")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn sleep");
        let pid = child.id();
        slot.store(child);

        let slot_w = Arc::clone(&slot);
        let waiter = thread::spawn(move || slot_w.wait_exit());

        // Give the waiter time to enter its poll loop.
        thread::sleep(Duration::from_millis(50));

        let start = std::time::Instant::now();
        slot.take_and_kill();
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "take_and_kill hung while waiter was running (deadlock #52)"
        );

        let status = waiter.join().expect("waiter join");
        // Either we observed the killed exit status, or kill already cleared it.
        if let Some(st) = status {
            assert!(!st.success(), "killed child should not report success");
        }
        // Process must be gone (best-effort — already reaped by wait).
        let _ = pid;
    }

    #[test]
    fn stop_all_with_running_child_returns_quickly() {
        let id = uuid::Uuid::new_v4();
        let root = std::env::temp_dir().join(format!("botc-stop-run-{id}"));
        let mut cfg = HarnessConfig::default();
        cfg.work_root = root.clone();
        cfg.socket_path = root.join("engine.sock");
        let mut pool = AgentPool::prepare(
            &cfg,
            vec![AgentConfig {
                role: AgentRole::Host,
                display_name: "ST".into(),
                token: "tok".into(),
                game_id: 1,
            }],
        )
        .unwrap();

        // Inject a long-running child + waiter the same way spawn_grok_tick does.
        let agent = &mut pool.agents[0];
        let child = Command::new("sleep")
            .arg("30")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn sleep");
        let _ = child.id();
        agent.child.store(child);
        *agent.running.lock().unwrap() = true;
        let slot = Arc::clone(&agent.child);
        let running = Arc::clone(&agent.running);
        let started = Arc::clone(&agent.session_started);
        let sid = Arc::clone(&agent.session_id);
        thread::spawn(move || {
            let status = slot.wait_exit();
            let ok = status.map(|s| s.success()).unwrap_or(false);
            if ok {
                *started.lock().unwrap() = true;
            } else if !*started.lock().unwrap() {
                *sid.lock().unwrap() = uuid::Uuid::new_v4().to_string();
            }
            *running.lock().unwrap() = false;
        });
        thread::sleep(Duration::from_millis(50));

        let start = std::time::Instant::now();
        pool.stop_all();
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "stop_all hung with running child (#52)"
        );
        assert!(!root.exists());
    }
}
