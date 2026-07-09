//! Authoritative game state machine (sketch).

mod phase;
mod seat;
mod state;

pub use phase::{DayStage, NightStep, Phase};
pub use seat::{Seat, SeatId};
pub use state::{Game, GameError, GameId, Lobby, PublicSeatView, RoleAssignment, Winner};
