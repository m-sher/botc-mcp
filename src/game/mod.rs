//! Authoritative game state machine (sketch).

mod ids;
pub mod night;
mod phase;
mod seat;
pub mod setup;
mod state;

pub use crate::error::GameError;
pub use ids::{GameId, SeatId};
pub use night::{build_first_night_queue, build_other_night_queue};
pub use phase::{DayStage, EndReason, NightStep, Phase, Winner};
pub use seat::Seat;
pub use setup::{BagResult, Composition, StartOpts};
pub use state::{
    CreateGameResult, Game, Lobby, PublicSeatView, RoleAssignment, MAX_PLAYERS, MIN_PLAYERS,
};
