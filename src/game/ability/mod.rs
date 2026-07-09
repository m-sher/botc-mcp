//! Night ability dispatch and resolution.

mod info;
pub mod register;

use crate::comms::PrivateMessage;
use crate::error::GameError;
use crate::game::ids::SeatId;
use crate::game::night::NightActionPayload;
use crate::game::phase::NightStep;
use crate::game::state::Game;

/// Result of resolving a night step (private side-effects already applied to `Game`).
#[derive(Debug, Clone, Default)]
pub struct NightEffect {
    pub private: Vec<(SeatId, PrivateMessage)>,
}

/// Resolve a night step that is either automatic info or a submitted player choice.
///
/// Applies private messages and seat mutations (e.g. Butler master) on `game`.
/// Does not advance the night cursor or clear pending wakes.
pub fn resolve_night_step(
    game: &mut Game,
    step: NightStep,
    payload: Option<&NightActionPayload>,
) -> Result<NightEffect, GameError> {
    info::resolve(game, step, payload)
}

pub use info::empath_true_count;
