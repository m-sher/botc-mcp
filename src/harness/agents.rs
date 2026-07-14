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
    /// Model this agent's grok sessions run on (picked per seat in setup).
    pub model: String,
}

#[derive(Debug, Clone)]
pub struct HarnessConfig {
    /// Number of player seats (5–15). Host is +1 session.
    pub player_count: usize,
    /// Default model: the starting pick for every session row in setup and the
    /// fallback when a seat has no pick. The model actually used by an agent is
    /// per-session ([`AgentConfig::model`]).
    pub model: String,
    /// Models available in the setup pickers — filled from `grok models` at TUI start.
    pub available_models: Vec<String>,
    pub grok_bin: PathBuf,
    pub agent_mcp_bin: PathBuf,
    pub work_root: PathBuf,
    pub socket_path: PathBuf,
    pub max_turns_per_tick: u32,
    pub seed: Option<u64>,
    pub st_choice_mode: String,
    /// Built-in grok tools to REMOVE from each agent (`--disallowed-tools`). The game
    /// is played only through the `botc` MCP server (reached via search_tool/use_tool),
    /// so agents don't need shell/file tools — removing them stops them exploring the
    /// filesystem for source / other seats' tokens. NEVER remove search_tool/use_tool.
    pub disallowed_tools: Vec<String>,
    /// grok `--sandbox` profile confining filesystem/network (defense in depth).
    pub grok_sandbox: Option<String>,
    /// Pass `--no-memory` so agents don't inherit the user's global coding context.
    pub no_memory: bool,
}

impl Default for HarnessConfig {
    fn default() -> Self {
        Self {
            player_count: 5,
            // Placeholder until `load_models_from_grok` (or the caller) fills it.
            model: "grok-build".into(),
            available_models: Vec::new(),
            grok_bin: PathBuf::from("grok"),
            agent_mcp_bin: PathBuf::from("botc-agent-mcp"),
            work_root: PathBuf::from("/tmp/botc-harness"),
            socket_path: PathBuf::from("/tmp/botc-harness/engine.sock"),
            max_turns_per_tick: 12,
            seed: Some(42),
            st_choice_mode: "host_first".into(),
            // grok-build is a software-engineering agent; with a shell it hunts for
            // source/tokens instead of playing. Strip every file/shell/edit/search
            // built-in, keeping only the MCP dispatch tools (search_tool/use_tool)
            // and todo_write. NOTE: removals must be self-consistent — search_replace
            // (edit) requires read_file, so both go together, or grok won't start.
            disallowed_tools: [
                "run_terminal_command",
                "read_file",
                "list_dir",
                "search_replace",
                "grep",
                "get_command_or_subagent_output",
                "kill_command_or_subagent",
                "image_edit",
                "web_search",
                "x_keyword_search",
                "x_semantic_search",
                "x_thread_fetch",
                "x_user_search",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
            grok_sandbox: Some("workspace".into()),
            no_memory: true,
        }
    }
}

/// Parse the human-readable output of `grok models`.
///
/// Expected shape (from the CLI today):
/// ```text
/// Default model: grok-code-fast-1
///
/// Available models:
///   - v9-stickynote
///   * grok-code-fast-1 (default)
/// ```
///
/// Returns `(models, default_id)`. Models keep the order from the CLI. The default
/// is taken from the `Default model:` line when present, else from a `* … (default)`
/// bullet.
pub fn parse_grok_models_output(stdout: &str) -> (Vec<String>, Option<String>) {
    let mut models = Vec::new();
    let mut default: Option<String> = None;
    for line in stdout.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("Default model:") {
            let id = rest.trim();
            if !id.is_empty() {
                default = Some(id.to_string());
            }
            continue;
        }
        let rest = if let Some(r) = t.strip_prefix("* ") {
            r
        } else if let Some(r) = t.strip_prefix("- ") {
            r
        } else {
            continue;
        };
        // First token is the model id; ignore a trailing "(default)" label.
        let id = rest
            .split_whitespace()
            .next()
            .unwrap_or("")
            .trim()
            .to_string();
        if id.is_empty() {
            continue;
        }
        if !models.iter().any(|m| m == &id) {
            models.push(id);
        }
    }
    (models, default)
}

/// Run `grok models` and return the available model ids plus the CLI default.
///
/// On failure (binary missing, non-zero exit, empty parse) returns an empty list
/// and `None` — the caller should keep whatever `model` it already has.
pub fn discover_models(grok_bin: &Path) -> (Vec<String>, Option<String>) {
    let output = match Command::new(grok_bin)
        .arg("models")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            crate::dlog!("models: failed to run `{} models`: {e}", grok_bin.display());
            return (Vec::new(), None);
        }
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        crate::dlog!(
            "models: `{} models` exit={:?} stderr={}",
            grok_bin.display(),
            output.status.code(),
            clip_for_log(&stderr, 200)
        );
        // Some builds still print the list on stdout even with a weird exit — try parse.
    }
    let (models, default) = parse_grok_models_output(&stdout);
    if models.is_empty() {
        // Fallback: sometimes the list is on stderr (or mixed).
        let (m2, d2) = parse_grok_models_output(&stderr);
        if !m2.is_empty() {
            return (m2, d2.or(default));
        }
        crate::dlog!(
            "models: no models parsed from `{} models` (stdout={})",
            grok_bin.display(),
            clip_for_log(&stdout, 200)
        );
    }
    (models, default)
}

fn clip_for_log(s: &str, max: usize) -> String {
    let s = s.replace('\n', "\\n");
    if s.chars().count() <= max {
        s
    } else {
        let cut: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{cut}…")
    }
}

impl HarnessConfig {
    /// Fill [`Self::available_models`] (and optionally [`Self::model`]) from
    /// `grok models`. Returns a short status note for the setup screen.
    pub fn load_models_from_grok(&mut self) -> String {
        let (models, default) = discover_models(&self.grok_bin);
        if models.is_empty() {
            // Keep a one-entry picker so ←/→ is not a no-op, using the current model.
            if self.available_models.is_empty() {
                self.available_models = vec![self.model.clone()];
            }
            return format!(
                "model list: could not read `{} models` — using {}",
                self.grok_bin.display(),
                self.model
            );
        }
        self.available_models = models;
        if let Some(d) = default {
            if self.available_models.iter().any(|m| m == &d) {
                self.model = d;
            } else {
                self.model = self.available_models[0].clone();
            }
        } else if !self.available_models.iter().any(|m| m == &self.model) {
            self.model = self.available_models[0].clone();
        }
        format!(
            "models: {} from `{} models` · selected {}",
            self.available_models.len(),
            self.grok_bin.display(),
            self.model
        )
    }

    /// Index of `self.model` in [`Self::available_models`], or `None` if custom.
    pub fn model_index(&self) -> Option<usize> {
        self.available_models.iter().position(|m| m == &self.model)
    }

    /// Cycle the model picker by `delta` steps (±1 for next/prev).
    ///
    /// If the current model is not in the list, `delta > 0` jumps to the first
    /// entry and `delta < 0` to the last.
    pub fn cycle_model(&mut self, delta: i32) {
        self.model = cycle_in_list(&self.model, &self.available_models, delta);
    }
}

/// Step `current` through `list` by `delta` (wrapping). An unknown `current`
/// lands on the first entry going forward, the last going backward. Used by the
/// per-seat model pickers in setup.
pub fn cycle_in_list(current: &str, list: &[String], delta: i32) -> String {
    if list.is_empty() || delta == 0 {
        return current.to_string();
    }
    let n = list.len() as i32;
    let idx = match list.iter().position(|m| m == current) {
        Some(i) => {
            let next = ((i as i32 + delta) % n + n) % n;
            next as usize
        }
        None if delta > 0 => 0,
        None => list.len() - 1,
    };
    list[idx].clone()
}

/// Per-agent child process coordination (#46 / #52).
///
/// Critical: **never** hold the slot mutex across a blocking `Child::wait()`.
/// The waiter uses `try_wait` + short sleeps; `take_and_kill` must be able to
/// acquire the lock while a child is still running and deliver SIGKILL.
#[derive(Debug, Default)]
enum ChildState {
    #[default]
    Empty,
    Running(Child),
    /// Reaped by kill or natural exit; waiter may still consume the status.
    Exited(std::process::ExitStatus),
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

/// Token spend for one headless tick, from streaming-json `end.usage`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TickUsage {
    pub input_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_tokens: u64,
    /// Full prompt+output including cache (per Grok headless docs).
    pub total_tokens: u64,
    pub num_turns: Option<u32>,
}

impl TickUsage {
    pub fn accumulate(&mut self, other: &TickUsage) {
        self.input_tokens += other.input_tokens;
        self.cache_read_input_tokens += other.cache_read_input_tokens;
        self.output_tokens += other.output_tokens;
        self.reasoning_tokens += other.reasoning_tokens;
        self.total_tokens += other.total_tokens;
        // num_turns: sum tool-loop rounds across ticks.
        self.num_turns = match (self.num_turns, other.num_turns) {
            (Some(a), Some(b)) => Some(a.saturating_add(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        };
    }
}

/// Context window fill from session `signals.json` (post-tick).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ContextWindow {
    pub tokens_used: u64,
    pub window_tokens: u64,
    /// Integer percent from Grok (`contextWindowUsage`), 0–100.
    pub usage_pct: u32,
}

impl ContextWindow {
    pub fn short(&self) -> String {
        if self.window_tokens > 0 {
            format!(
                "ctx {}% ({} / {})",
                self.usage_pct,
                format_token_count(self.tokens_used),
                format_token_count(self.window_tokens)
            )
        } else {
            format!("ctx {}%", self.usage_pct)
        }
    }
}

/// Per-agent cumulative spend + latest context window for UI / ranking.
#[derive(Debug, Clone, Default)]
pub struct AgentUsage {
    pub last_tick: Option<TickUsage>,
    /// Sum of per-tick totals over the whole game.
    pub game_total: TickUsage,
    pub ticks_with_usage: u32,
    pub context: Option<ContextWindow>,
}

impl AgentUsage {
    pub fn record_tick(&mut self, tick: TickUsage) {
        self.game_total.accumulate(&tick);
        self.ticks_with_usage = self.ticks_with_usage.saturating_add(1);
        self.last_tick = Some(tick);
    }

    /// Compact board line: `Σ48k · last 12k · ctx 7% (36k/512k)`.
    pub fn board_short(&self) -> String {
        let mut parts = Vec::new();
        if self.game_total.total_tokens > 0 {
            parts.push(format!(
                "Σ{}",
                format_token_count(self.game_total.total_tokens)
            ));
        }
        if let Some(t) = &self.last_tick {
            if t.total_tokens > 0 {
                parts.push(format!("last {}", format_token_count(t.total_tokens)));
            }
        }
        if let Some(c) = &self.context {
            parts.push(c.short());
        }
        if parts.is_empty() {
            "—".into()
        } else {
            parts.join(" · ")
        }
    }
}

/// Human token count: `940`, `12k`, `1.2M`.
pub fn format_token_count(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 10_000 {
        format!("{}k", (n + 500) / 1_000)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

/// Parse `usage` (+ optional `num_turns`) from a streaming-json `end` / result object.
pub fn parse_tick_usage(v: &Value) -> Option<TickUsage> {
    let usage = v.get("usage")?;
    let u64_field = |key: &str| -> u64 {
        usage
            .get(key)
            .and_then(|x| x.as_u64().or_else(|| x.as_f64().map(|f| f as u64)))
            .unwrap_or(0)
    };
    let total = u64_field("total_tokens");
    let input = u64_field("input_tokens");
    let cache = u64_field("cache_read_input_tokens");
    let output = u64_field("output_tokens");
    let reasoning = u64_field("reasoning_tokens");
    // Some payloads omit total_tokens — reconstruct.
    let total = if total > 0 {
        total
    } else {
        input.saturating_add(cache).saturating_add(output)
    };
    if total == 0 && input == 0 && output == 0 {
        return None;
    }
    let num_turns = v
        .get("num_turns")
        .and_then(|x| x.as_u64())
        .map(|n| n as u32);
    Some(TickUsage {
        input_tokens: input,
        cache_read_input_tokens: cache,
        output_tokens: output,
        reasoning_tokens: reasoning,
        total_tokens: total,
        num_turns,
    })
}

/// Parse usage from one streaming-json stdout line (`type: end` preferred).
pub fn parse_stream_line_usage(line: &str) -> Option<TickUsage> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let v: Value = serde_json::from_str(line).ok()?;
    let ty = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
    // `end` is the documented spend carrier; also accept a bare result-shaped line.
    if matches!(ty, "end" | "max_turns_reached" | "result" | "") {
        return parse_tick_usage(&v);
    }
    // Nested usage under some wrappers.
    if let Some(inner) = v.get("result") {
        return parse_tick_usage(inner);
    }
    None
}

/// Read context-window fill from Grok's session `signals.json` for this workdir + id.
pub fn read_session_context(workdir: &Path, session_id: &str) -> Option<ContextWindow> {
    let path = grok_session_signals_path(workdir, session_id)?;
    let text = fs::read_to_string(&path).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    let tokens_used = v
        .get("contextTokensUsed")
        .and_then(|x| x.as_u64())
        .unwrap_or(0);
    let window_tokens = v
        .get("contextWindowTokens")
        .and_then(|x| x.as_u64())
        .unwrap_or(0);
    let usage_pct = v
        .get("contextWindowUsage")
        .and_then(|x| x.as_u64().or_else(|| x.as_f64().map(|f| f as u64)))
        .unwrap_or(0) as u32;
    if tokens_used == 0 && window_tokens == 0 && usage_pct == 0 {
        return None;
    }
    Some(ContextWindow {
        tokens_used,
        window_tokens,
        usage_pct,
    })
}

/// `~/.grok/sessions/{url-encoded-abs-cwd}/{session_id}/signals.json`
fn grok_session_signals_path(workdir: &Path, session_id: &str) -> Option<PathBuf> {
    if session_id.is_empty() {
        return None;
    }
    let abs = workdir
        .canonicalize()
        .unwrap_or_else(|_| workdir.to_path_buf());
    let encoded = abs.to_string_lossy().replace('/', "%2F");
    let home = std::env::var_os("HOME")?;
    Some(
        PathBuf::from(home)
            .join(".grok/sessions")
            .join(encoded)
            .join(session_id)
            .join("signals.json"),
    )
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
    /// Token spend + context window (updated each tick).
    pub usage: Arc<Mutex<AgentUsage>>,
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
                usage: Arc::new(Mutex::new(AgentUsage::default())),
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
                AgentRole::Host => {
                    prompts::host_kickoff(agent.config.game_id, n_players, &self.cfg.st_choice_mode)
                }
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
                    self.agents.iter().position(
                        |a| matches!(a.config.role, AgentRole::Player { seat: s } if s == seat),
                    )
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
    model: &str,
    workdir: &Path,
    prompt_file: &Path,
    session_id: &str,
    session_started: bool,
) -> Vec<String> {
    let mut args = vec![
        "--prompt-file".into(),
        prompt_file.display().to_string(),
        "-m".into(),
        model.to_string(),
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
    // Confine the agent to *playing* (not exploring the repo): remove built-in
    // file/shell tools, drop the global coding context, and sandbox the fs.
    if !cfg.disallowed_tools.is_empty() {
        args.push("--disallowed-tools".into());
        args.push(cfg.disallowed_tools.join(","));
    }
    if cfg.no_memory {
        args.push("--no-memory".into());
    }
    if let Some(profile) = &cfg.grok_sandbox {
        args.push("--sandbox".into());
        args.push(profile.clone());
    }
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
        &agent.config.model,
        &agent.workdir,
        &prompt_file,
        &session_id,
        session_started,
    );
    crate::dlog!(
        "SPAWN {label} model={} mode={} session={} prompt_first_line={:?} argv=[{}]",
        agent.config.model,
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
    let usage_slot = Arc::clone(&agent.usage);
    let workdir = agent.workdir.clone();
    let child_slot = Arc::clone(&agent.child);
    let game_id = agent.config.game_id;
    let model = agent.config.model.clone();
    let agent_role_label = label.clone();
    // #56: grok exits 1 on normal --max-turns; gate resume on stream evidence of a
    // real session (end / max_turns_reached), not process exit code.
    let session_established = Arc::new(AtomicBool::new(false));
    *running_flag.lock().unwrap() = true;

    if let Some(stdout) = child.stdout.take() {
        let log = Arc::clone(&log);
        let usage_slot = Arc::clone(&usage_slot);
        let established = Arc::clone(&session_established);
        thread::spawn(move || {
            use std::io::BufRead;
            let reader = std::io::BufReader::new(stdout);
            // Process each streaming-json event as it arrives → live display + usage.
            for line in reader.lines().map_while(Result::ok) {
                if stream_line_establishes_session(&line) {
                    established.store(true, Ordering::SeqCst);
                }
                if let Some(tick) = parse_stream_line_usage(&line) {
                    let mut u = usage_slot.lock().unwrap();
                    u.record_tick(tick.clone());
                    crate::dlog!(
                        "USAGE {agent_role_label} tick_total={} Σ={} turns={:?}",
                        tick.total_tokens,
                        u.game_total.total_tokens,
                        tick.num_turns
                    );
                    // Ranking corpus: one line per spend-bearing tick.
                    crate::harness::results_log::log_tick_usage(
                        game_id,
                        &agent_role_label,
                        &model,
                        &tick,
                        u.context.as_ref(),
                        &u.game_total,
                        u.ticks_with_usage,
                    );
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
            for line in reader.lines().map_while(Result::ok) {
                let mut g = log.lock().unwrap();
                push_full_line(&mut g, LineKind::Stderr, line);
            }
        });
    }

    child_slot.store(child);

    // Waiter: session_started from stream evidence (#47/#56), not exit code alone.
    let child_slot_w = Arc::clone(&child_slot);
    let exit_label = label.clone();
    let usage_slot_w = Arc::clone(&usage_slot);
    let session_id_for_ctx = Arc::clone(&session_id_slot);
    thread::spawn(move || {
        let status = child_slot_w.wait_exit();
        // stdout reader may still be finishing; give it a brief moment.
        for _ in 0..10 {
            if session_established.load(Ordering::SeqCst) {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        // Context window: read signals.json after the process exits (file is final).
        let sid = session_id_for_ctx.lock().unwrap().clone();
        if let Some(ctx) = read_session_context(&workdir, &sid) {
            usage_slot_w.lock().unwrap().context = Some(ctx.clone());
            crate::dlog!(
                "CTX {exit_label} {}% used={} window={}",
                ctx.usage_pct,
                ctx.tokens_used,
                ctx.window_tokens
            );
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
        Some("end") => {
            let note = parse_tick_usage(&v)
                .map(|u| format!("— turn end · {} tok —", format_token_count(u.total_tokens)))
                .unwrap_or_else(|| "— turn end —".into());
            push_full_line(&mut guard, LineKind::System, note);
        }
        Some("max_turns_reached") => {
            let note = parse_tick_usage(&v)
                .map(|u| format!("— max turns · {} tok —", format_token_count(u.total_tokens)))
                .unwrap_or_else(|| "— max turns reached —".into());
            push_full_line(&mut guard, LineKind::System, note);
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
    fn parse_tick_usage_from_end_event() {
        let line = r#"{"type":"end","stopReason":"EndTurn","usage":{"input_tokens":100,"cache_read_input_tokens":900,"output_tokens":50,"reasoning_tokens":10,"total_tokens":1050},"num_turns":3}"#;
        let u = parse_stream_line_usage(line).expect("usage");
        assert_eq!(u.input_tokens, 100);
        assert_eq!(u.cache_read_input_tokens, 900);
        assert_eq!(u.output_tokens, 50);
        assert_eq!(u.reasoning_tokens, 10);
        assert_eq!(u.total_tokens, 1050);
        assert_eq!(u.num_turns, Some(3));
        // Reconstruct total when omitted.
        let bare = r#"{"type":"end","usage":{"input_tokens":10,"cache_read_input_tokens":5,"output_tokens":2}}"#;
        let u2 = parse_stream_line_usage(bare).unwrap();
        assert_eq!(u2.total_tokens, 17);
        assert!(parse_stream_line_usage(r#"{"type":"text","data":"hi"}"#).is_none());
    }

    #[test]
    fn agent_usage_accumulates_and_formats_board() {
        let mut u = AgentUsage::default();
        assert_eq!(u.board_short(), "—");
        u.record_tick(TickUsage {
            total_tokens: 12_000,
            input_tokens: 10_000,
            output_tokens: 2_000,
            ..Default::default()
        });
        u.record_tick(TickUsage {
            total_tokens: 3_500,
            ..Default::default()
        });
        assert_eq!(u.ticks_with_usage, 2);
        assert_eq!(u.game_total.total_tokens, 15_500);
        u.context = Some(ContextWindow {
            tokens_used: 35_586,
            window_tokens: 512_000,
            usage_pct: 6,
        });
        let s = u.board_short();
        assert!(s.contains('Σ'), "{s}");
        assert!(s.contains("ctx 6%"), "{s}");
        assert!(s.contains("last"), "{s}");
    }

    #[test]
    fn format_token_count_scales() {
        assert_eq!(format_token_count(940), "940");
        assert_eq!(format_token_count(1_500), "1.5k");
        assert_eq!(format_token_count(12_400), "12k");
    }

    #[test]
    fn parse_grok_models_output_reads_list_and_default() {
        let sample = r#"
You are logged in with grok.com.

Default model: grok-code-fast-1

Available models:
  - v9-stickynote
  - grok-build
  - sxs-claude-opus-4-6
  * grok-code-fast-1 (default)
  - opus-4-5-caching
"#;
        let (models, default) = parse_grok_models_output(sample);
        assert_eq!(
            models,
            vec![
                "v9-stickynote",
                "grok-build",
                "sxs-claude-opus-4-6",
                "grok-code-fast-1",
                "opus-4-5-caching",
            ]
        );
        assert_eq!(default.as_deref(), Some("grok-code-fast-1"));
    }

    #[test]
    fn cycle_model_wraps_and_recovers_from_custom() {
        let mut cfg = HarnessConfig {
            available_models: vec!["grok-build".into(), "v9-stickynote".into(), "other".into()],
            model: "grok-build".into(),
            ..Default::default()
        };
        cfg.cycle_model(1);
        assert_eq!(cfg.model, "v9-stickynote");
        cfg.cycle_model(-1);
        assert_eq!(cfg.model, "grok-build");
        // Wrap past ends.
        cfg.cycle_model(-1);
        assert_eq!(cfg.model, "other");
        cfg.cycle_model(1);
        assert_eq!(cfg.model, "grok-build");
        // Custom / unknown id lands on an edge of the list.
        cfg.model = "my-custom-model".into();
        cfg.cycle_model(1);
        assert_eq!(cfg.model, "grok-build");
        cfg.model = "my-custom-model".into();
        cfg.cycle_model(-1);
        assert_eq!(cfg.model, "other");
    }

    #[test]
    fn load_models_applies_parsed_default() {
        let mut cfg = HarnessConfig::default();
        // Simulate what load_models_from_grok does after a successful parse,
        // without spawning the real binary.
        let (models, default) = parse_grok_models_output(
            "Default model: alpha\nAvailable models:\n  - beta\n  * alpha (default)\n",
        );
        cfg.available_models = models;
        if let Some(d) = default {
            cfg.model = d;
        }
        assert_eq!(cfg.model, "alpha");
        assert_eq!(cfg.available_models, vec!["beta", "alpha"]);
    }

    #[test]
    fn grok_args_use_the_per_agent_model() {
        // Models are picked per seat: the argv must carry the CALLER's model,
        // not the config default.
        let cfg = HarnessConfig {
            model: "config-default-model".into(),
            ..Default::default()
        };
        for model in ["host-model-a", "seat-model-b"] {
            let args = build_grok_tick_args(
                &cfg,
                model,
                Path::new("/tmp/wd"),
                Path::new("/tmp/wd/prompt.txt"),
                "11111111-1111-1111-1111-111111111111",
                false,
            );
            let mi = args.iter().position(|a| a == "-m").expect("-m present");
            assert_eq!(args[mi + 1], model);
            assert!(!args.contains(&"config-default-model".to_string()));
        }
    }

    #[test]
    fn cycle_in_list_steps_and_wraps() {
        let list: Vec<String> = vec!["a".into(), "b".into(), "c".into()];
        assert_eq!(cycle_in_list("a", &list, 1), "b");
        assert_eq!(cycle_in_list("c", &list, 1), "a");
        assert_eq!(cycle_in_list("a", &list, -1), "c");
        // Unknown current: forward → first, backward → last.
        assert_eq!(cycle_in_list("zzz", &list, 1), "a");
        assert_eq!(cycle_in_list("zzz", &list, -1), "c");
        // Empty list / zero delta are no-ops.
        assert_eq!(cycle_in_list("keep", &[], 1), "keep");
        assert_eq!(cycle_in_list("keep", &list, 0), "keep");
    }

    #[test]
    fn grok_args_confine_agent_to_playing() {
        let cfg = HarnessConfig::default();
        let args = build_grok_tick_args(
            &cfg,
            "grok-build",
            Path::new("/tmp/wd"),
            Path::new("/tmp/wd/prompt.txt"),
            "11111111-1111-1111-1111-111111111111",
            false,
        );
        // Built-in file/shell tools removed (so it can't hunt for source / tokens)…
        let di = args
            .iter()
            .position(|a| a == "--disallowed-tools")
            .expect("--disallowed-tools present");
        let list = &args[di + 1];
        assert!(list.contains("run_terminal_command"));
        assert!(list.contains("read_file"));
        assert!(list.contains("list_dir"));
        // …but the MCP dispatch tools are NEVER removed (they ARE the game tools).
        assert!(!list.contains("search_tool"));
        assert!(!list.contains("use_tool"));
        // Global coding context dropped + filesystem sandboxed.
        assert!(args.contains(&"--no-memory".into()));
        let sb = args
            .iter()
            .position(|a| a == "--sandbox")
            .expect("--sandbox");
        assert_eq!(args[sb + 1], "workspace");
    }

    #[test]
    fn grok_args_use_yolo_once_not_always_approve() {
        let cfg = HarnessConfig::default();
        let args = build_grok_tick_args(
            &cfg,
            "grok-build",
            Path::new("/tmp/wd"),
            Path::new("/tmp/wd/prompt.txt"),
            "11111111-1111-1111-1111-111111111111",
            false,
        );
        let yolo_count = args.iter().filter(|a| *a == "--yolo").count();
        let always = args.iter().filter(|a| *a == "--always-approve").count();
        assert_eq!(yolo_count, 1, "expected single --yolo: {args:?}");
        assert_eq!(
            always, 0,
            "must not pass --always-approve (alias): {args:?}"
        );
        assert!(args.contains(&"--session-id".into()));
        assert!(!args.contains(&"--resume".into()));
    }

    #[test]
    fn grok_args_resume_after_session_started() {
        let cfg = HarnessConfig::default();
        let args = build_grok_tick_args(
            &cfg,
            "grok-build",
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
            "grok-build",
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
            "grok-build",
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
        let cfg = HarnessConfig {
            work_root: root.clone(),
            socket_path: root.join("engine.sock"),
            ..Default::default()
        };
        let pool = AgentPool::prepare(
            &cfg,
            vec![AgentConfig {
                role: AgentRole::Host,
                display_name: "ST".into(),
                token: "tok".into(),
                game_id: 1,
                model: "grok-build".into(),
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
        let cfg = HarnessConfig {
            work_root: root.clone(),
            socket_path: root.join("engine.sock"),
            ..Default::default()
        };
        let mut pool = AgentPool::prepare(
            &cfg,
            vec![AgentConfig {
                role: AgentRole::Host,
                display_name: "ST".into(),
                token: "tok".into(),
                game_id: 1,
                model: "grok-build".into(),
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
