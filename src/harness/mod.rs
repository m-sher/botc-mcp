//! Multi-agent TUI harness: shared engine + headless CLI-agent sessions + monitor UI.
//!
//! Architecture:
//! - The TUI owns a [`crate::mcp_server::SharedStore`] (one game).
//! - A Unix-socket RPC server exposes the same tools as the MCP stdio server.
//! - Each agent seat runs a headless CLI ([`agents::Backend`]: grok or Claude Code)
//!   with a project-scoped MCP config that launches `botc-agent-mcp` (stdio proxy)
//!   bound to that agent's token. Backends are per-seat, so one game can mix them.
//! - The TUI polls host/public state and tails agent stdout for monitoring.

pub mod action_log;
pub mod agents;
pub mod balance;
pub mod debug_log;
pub mod prompts;
pub mod proxy_acl;
pub mod results_log;
pub mod scheduler;
pub mod socket;
pub mod tui;

pub use agents::{AgentConfig, AgentRole, Backend, HarnessConfig};
pub use tui::run_tui;
