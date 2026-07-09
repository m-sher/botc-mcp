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

#[derive(Debug)]
pub struct LiveAgent {
    pub config: AgentConfig,
    pub session_id: String,
    pub workdir: PathBuf,
    pub log: Arc<Mutex<Vec<String>>>,
    /// Background tick loop handle (optional).
    pub child: Option<Child>,
    pub running: Arc<Mutex<bool>>,
}

pub struct AgentPool {
    pub agents: Vec<LiveAgent>,
    pub cfg: HarnessConfig,
}

impl AgentPool {
    pub fn prepare(
        cfg: &HarnessConfig,
        agents: Vec<AgentConfig>,
    ) -> std::io::Result<Self> {
        fs::create_dir_all(&cfg.work_root)?;
        let mut live = Vec::new();
        for a in agents {
            let label = match a.role {
                AgentRole::Host => "host".to_string(),
                AgentRole::Player { seat } => format!("seat{}", seat.0),
            };
            let workdir = cfg.work_root.join(&label);
            fs::create_dir_all(workdir.join(".grok"))?;
            write_agent_mcp_config(&workdir, cfg, &a.token, a.game_id)?;
            let session_id = uuid::Uuid::new_v4().to_string();
            live.push(LiveAgent {
                config: a,
                session_id,
                workdir,
                log: Arc::new(Mutex::new(Vec::new())),
                child: None,
                running: Arc::new(Mutex::new(false)),
            });
        }
        Ok(Self {
            agents: live,
            cfg: cfg.clone(),
        })
    }

    /// Kick off every agent with its role prompt (one headless grok invocation each).
    pub fn kickoff_all(&mut self, n_players: usize) -> std::io::Result<()> {
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
            spawn_grok_tick(&self.cfg, agent, &prompt, true)?;
        }
        Ok(())
    }

    /// One more multi-turn tick for every agent (resume session).
    pub fn tick_all(&mut self, public_summary: &str, host_hint: &str) -> std::io::Result<()> {
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
            spawn_grok_tick(&self.cfg, agent, &prompt, false)?;
        }
        Ok(())
    }

    pub fn stop_all(&mut self) {
        for agent in &mut self.agents {
            *agent.running.lock().unwrap() = false;
            if let Some(mut c) = agent.child.take() {
                let _ = c.kill();
                let _ = c.wait();
            }
        }
    }
}

fn write_agent_mcp_config(
    workdir: &Path,
    cfg: &HarnessConfig,
    token: &str,
    game_id: u64,
) -> std::io::Result<()> {
    let mcp_bin = if cfg.agent_mcp_bin.is_absolute() {
        cfg.agent_mcp_bin.clone()
    } else {
        // Resolve relative to current_exe dir or PATH later; write absolute if possible.
        std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.join("botc-agent-mcp")))
            .filter(|p| p.exists())
            .unwrap_or_else(|| cfg.agent_mcp_bin.clone())
    };
    let token_path = workdir.join("agent.token");
    fs::write(&token_path, token)?;
    let sock = cfg.socket_path.display();
    let bin = mcp_bin.display();
    let tok = token_path.display();
    let toml = format!(
        r#"# Auto-generated by botc-tui — do not commit.
[mcp_servers.botc]
command = "{bin}"
args = ["--socket", "{sock}", "--token-file", "{tok}", "--game-id", "{game_id}"]
enabled = true
startup_timeout_sec = 60
"#
    );
    fs::write(workdir.join(".grok/config.toml"), toml)?;
    Ok(())
}

fn spawn_grok_tick(
    cfg: &HarnessConfig,
    agent: &mut LiveAgent,
    prompt: &str,
    first: bool,
) -> std::io::Result<()> {
    // Avoid overlapping ticks on the same agent.
    if let Some(mut c) = agent.child.take() {
        // If previous still running, leave it.
        match c.try_wait()? {
            None => {
                agent.child = Some(c);
                return Ok(());
            }
            Some(_) => {}
        }
    }

    let prompt_file = agent.workdir.join("prompt.txt");
    fs::write(&prompt_file, prompt)?;

    let mut cmd = Command::new(&cfg.grok_bin);
    cmd.arg("--prompt-file")
        .arg(&prompt_file)
        .arg("-m")
        .arg(&cfg.model)
        .arg("--cwd")
        .arg(&agent.workdir)
        .arg("--max-turns")
        .arg(cfg.max_turns_per_tick.to_string())
        .arg("--output-format")
        .arg("streaming-json")
        .arg("--yolo")
        .arg("--always-approve")
        .arg("--no-subagents")
        .arg("--disable-web-search")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    if first {
        cmd.arg("--session-id").arg(&agent.session_id);
    } else {
        cmd.arg("--resume").arg(&agent.session_id);
    }

    let mut child = cmd.spawn()?;
    let log = Arc::clone(&agent.log);
    let running_flag = Arc::clone(&agent.running);
    *running_flag.lock().unwrap() = true;
    if let Some(stdout) = child.stdout.take() {
        let log = Arc::clone(&log);
        thread::spawn(move || {
            use std::io::BufRead;
            let reader = std::io::BufReader::new(stdout);
            for line in reader.lines().flatten() {
                if let Ok(v) = serde_json::from_str::<Value>(&line) {
                    let msg = match v.get("type").and_then(|t| t.as_str()) {
                        Some("text") => v
                            .get("data")
                            .and_then(|d| d.as_str())
                            .unwrap_or("")
                            .to_string(),
                        Some("thought") => format!(
                            "[think] {}",
                            v.get("data").and_then(|d| d.as_str()).unwrap_or("")
                        ),
                        Some("end") => "[turn end]".into(),
                        Some("error") => format!(
                            "ERROR {}",
                            v.get("message").and_then(|m| m.as_str()).unwrap_or("?")
                        ),
                        _ => line,
                    };
                    if !msg.is_empty() {
                        let mut g = log.lock().unwrap();
                        g.push(msg);
                        if g.len() > 400 {
                            let drain = g.len() - 400;
                            g.drain(0..drain);
                        }
                    }
                } else {
                    let mut g = log.lock().unwrap();
                    g.push(line);
                }
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
                g.push(format!("[stderr] {line}"));
            }
        });
    }
    // Reap child in background and clear running flag.
    let running_flag2 = Arc::clone(&running_flag);
    thread::spawn(move || {
        let _ = child.wait();
        *running_flag2.lock().unwrap() = false;
    });
    // Don't store Child — reaped above. Keep slot free for next tick.
    agent.child = None;
    Ok(())
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

/// Small delay helper for UI loops.
pub fn sleep_ms(ms: u64) {
    thread::sleep(Duration::from_millis(ms));
}
