//! Authoritative game state machine (sketch).

pub mod ability;
pub mod day;
mod ids;
pub mod night;
mod phase;
mod seat;
pub mod setup;
mod state;
mod win;

pub use crate::error::GameError;
pub use day::{
    close_vote, end_nominations, execution_leader, meets_threshold, nominate, open_nominations,
    resolve_execution, vote, ClosedNomination, OpenNomination,
};
pub use ids::{GameId, SeatId};
pub use night::{
    build_first_night_queue, build_other_night_queue, ChoiceSchema, NightActionPayload,
    PendingWake,
};
pub use phase::{DayStage, EndReason, NightStep, Phase, Winner};
pub use seat::Seat;
pub use setup::{BagResult, Composition, StartOpts};
pub use state::{
    CreateGameResult, Game, Lobby, PublicSeatView, RoleAssignment, MAX_PLAYERS, MIN_PLAYERS,
};
