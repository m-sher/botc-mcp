//! Authoritative game state machine (sketch).

mod ids;
mod phase;
mod seat;
mod state;

pub use crate::error::GameError;
pub use ids::{GameId, SeatId};
pub use phase::{DayStage, NightStep, Phase};
pub use seat::Seat;
pub use state::{Game, Lobby, PublicSeatView, RoleAssignment, Winner};
