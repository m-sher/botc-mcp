//! Blood on the Clocktower MCP game engine (sketch).
//!
//! See `docs/architecture.md` for the design this module tree implements.

pub mod auth;
pub mod comms;
pub mod error;
pub mod game;
pub mod harness;
pub mod mcp_server;
pub mod rng;
pub mod roles;
pub mod store;
pub mod tools;

pub use error::{GameError, ToolError};
