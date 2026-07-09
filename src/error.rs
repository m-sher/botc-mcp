//! Engine and tool error types (safe for client-facing messages).

/// Authoritative game-engine failures (phase, legality, seats, etc.).
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum GameError {
    #[error("no such seat")]
    NoSuchSeat,
    #[error("unauthorized")]
    Unauthorized,
    #[error("wrong phase")]
    WrongPhase,
    #[error("game ended")]
    GameEnded,
    #[error("illegal action: {0}")]
    IllegalAction(&'static str),
    #[error("not your wake")]
    NotYourWake,
    #[error("bad request: {0}")]
    BadRequest(&'static str),
}

/// Errors returned to the MCP client (safe strings; no other seats' secrets).
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum ToolError {
    #[error("unauthorized")]
    Unauthorized,
    #[error(transparent)]
    Game(#[from] GameError),
    #[error("bad request: {0}")]
    BadRequest(&'static str),
}
