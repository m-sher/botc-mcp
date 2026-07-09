//! Authoritative game state machine (sketch).

mod ids;
mod phase;
mod seat;
pub mod setup;
mod state;

pub use crate::error::GameError;
pub use ids::{GameId, SeatId};
pub use phase::{DayStage, NightStep, Phase};
pub use seat::Seat;
pub use setup::{BagResult, Composition, StartOpts};
pub use state::{
    CreateGameResult, Game, Lobby, PublicSeatView, RoleAssignment, Winner, MAX_PLAYERS,
    MIN_PLAYERS,
};
