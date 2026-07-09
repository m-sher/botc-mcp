//! Spawn headless Grok sessions with per-agent MCP proxy config.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde_json::Value;

use crate::game::SeatId;
use crate::harness::prompts;

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

/// Shared child handle so stop_all can kill while a waiter reaps.
#[derive(Debug, Default)]
struct ChildSlot {
    inner: Mutex<Option<Child>>,
}

impl ChildSlot {
    fn store(&self, child: Child) {
        *self.inner.lock().unwrap() = Some(child);
    }

    fn take_and_kill(&self) {
        if let Some(mut c) = self.inner.lock().unwrap().take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }

    fn wait_exit(&self) -> Option<std::process::ExitStatus> {
        let mut g = self.inner.lock().unwrap();
        let child = g.as_mut()?;
        match child.wait() {
            Ok(st) => {
                *g = None;
                Some(st)
            }
            Err(_) => {
                *g = None;
                None
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
    pub log: Arc<Mutex<Vec<String>>>,
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
    if *agent.running.lock().unwrap() {
        return Ok(TickOutcome::SkippedStillRunning);
    }

    let prompt_file = agent.workdir.join("prompt.txt");
    fs::write(&prompt_file, prompt)?;

    let session_started = *agent.session_started.lock().unwrap();
    // If a previous first-run failed, mint a fresh session id so --session-id is not a collision (#47).
    if !session_started {
        // Keep existing id for first attempt; regenerate only after a failed attempt (see waiter).
    }
    let session_id = agent.session_id.lock().unwrap().clone();

    let args = build_grok_tick_args(
        cfg,
        &agent.workdir,
        &prompt_file,
        &session_id,
        session_started,
    );

    let mut cmd = Command::new(&cfg.grok_bin);
    cmd.args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            push_log_line(
                &agent.log,
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
    *running_flag.lock().unwrap() = true;

    if let Some(stdout) = child.stdout.take() {
        let log = Arc::clone(&log);
        thread::spawn(move || {
            use std::io::BufRead;
            let reader = std::io::BufReader::new(stdout);
            let mut asm = StreamAssembler::default();
            for line in reader.lines().flatten() {
                for piece in asm.push_line(&line) {
                    push_log_line(&log, piece);
                }
            }
            for piece in asm.finish() {
                push_log_line(&log, piece);
            }
        });
    }
    if let Some(stderr) = child.stderr.take() {
        let log = Arc::clone(&agent.log);
        thread::spawn(move || {
            use std::io::BufRead;
            let reader = std::io::BufReader::new(stderr);
            for line in reader.lines().flatten() {
                push_log_line(&log, format!("[stderr] {line}"));
            }
        });
    }

    child_slot.store(child);

    // Waiter: set session_started only on success; regenerate id on failed first run (#47).
    let child_slot_w = Arc::clone(&child_slot);
    thread::spawn(move || {
        let status = child_slot_w.wait_exit();
        let ok = status.map(|s| s.success()).unwrap_or(false);
        if ok {
            *session_started_flag.lock().unwrap() = true;
        } else if !*session_started_flag.lock().unwrap() {
            // First run never succeeded — next tick uses a fresh --session-id.
            *session_id_slot.lock().unwrap() = uuid::Uuid::new_v4().to_string();
        }
        *running_flag.lock().unwrap() = false;
    });

    Ok(TickOutcome::Spawned)
}

fn push_log_line(log: &Mutex<Vec<String>>, msg: String) {
    if msg.is_empty() {
        return;
    }
    let mut g = log.lock().unwrap();
    g.push(msg);
    if g.len() > 400 {
        let drain = g.len() - 400;
        g.drain(0..drain);
    }
}

/// Coalesce NDJSON `streaming-json` chunks into readable log lines.
#[derive(Debug, Default)]
pub struct StreamAssembler {
    kind: Option<StreamKind>,
    buf: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StreamKind {
    Text,
    Thought,
}

impl StreamAssembler {
    /// Ingest one stdout line; returns zero or more completed log lines.
    pub fn push_line(&mut self, line: &str) -> Vec<String> {
        let line = line.trim();
        if line.is_empty() {
            return Vec::new();
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            let mut out = self.flush();
            out.push(line.to_string());
            return out;
        };
        match v.get("type").and_then(|t| t.as_str()) {
            Some("text") => {
                let data = v.get("data").and_then(|d| d.as_str()).unwrap_or("");
                self.push_chunk(StreamKind::Text, data)
            }
            Some("thought") => {
                let data = v.get("data").and_then(|d| d.as_str()).unwrap_or("");
                self.push_chunk(StreamKind::Thought, data)
            }
            Some("end") => {
                let mut out = self.flush();
                out.push("[turn end]".into());
                out
            }
            Some("error") => {
                let mut out = self.flush();
                let msg = v
                    .get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("?");
                out.push(format!("ERROR {msg}"));
                out
            }
            _ => {
                let mut out = self.flush();
                out.push(line.to_string());
                out
            }
        }
    }

    pub fn finish(&mut self) -> Vec<String> {
        self.flush()
    }

    fn push_chunk(&mut self, kind: StreamKind, data: &str) -> Vec<String> {
        if data.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        if self.kind != Some(kind) {
            out.extend(self.flush());
            self.kind = Some(kind);
        }
        self.buf.push_str(data);
        out
    }

    fn flush(&mut self) -> Vec<String> {
        if self.buf.is_empty() {
            self.kind = None;
            return Vec::new();
        }
        let text = std::mem::take(&mut self.buf);
        let line = match self.kind.take() {
            Some(StreamKind::Thought) => format!("[think] {text}"),
            Some(StreamKind::Text) | None => text,
        };
        vec![line]
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
    fn stream_assembler_coalesces_thought_chunks() {
        let mut a = StreamAssembler::default();
        let mut out = Vec::new();
        for line in [
            r#"{"type":"thought","data":"The"}"#,
            r#"{"type":"thought","data":" task"}"#,
            r#"{"type":"thought","data":" is"}"#,
            r#"{"type":"text","data":"Hello"}"#,
            r#"{"type":"text","data":" world"}"#,
            r#"{"type":"end","stopReason":"EndTurn","sessionId":"x"}"#,
        ] {
            out.extend(a.push_line(line));
        }
        out.extend(a.finish());
        assert_eq!(
            out,
            vec![
                "[think] The task is".to_string(),
                "Hello world".to_string(),
                "[turn end]".to_string(),
            ],
            "got {out:?}"
        );
        assert!(!out.iter().any(|s| s.contains("[think] The[think]")));
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
}
