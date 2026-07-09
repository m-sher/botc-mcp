//! Multi-agent TUI harness: shared engine + headless Grok sessions + monitor UI.
//!
//! Architecture:
//! - The TUI owns a [`crate::mcp_server::SharedStore`] (one game).
//! - A Unix-socket RPC server exposes the same tools as the MCP stdio server.
//! - Each Grok agent is a headless `grok` process with a project-scoped MCP config
//!   that launches `botc-agent-mcp` (stdio proxy) bound to that agent's token.
//! - The TUI polls host/public state and tails agent stdout for monitoring.

pub mod agents;
pub mod prompts;
pub mod proxy_acl;
pub mod socket;
pub mod tui;

pub use agents::{AgentConfig, AgentRole, HarnessConfig};
pub use tui::run_tui;
