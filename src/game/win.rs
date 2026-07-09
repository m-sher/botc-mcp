//! Win condition checks (minimal stub until Task 11).

use crate::comms::PublicEvent;
use crate::game::phase::{EndReason, Phase, Winner};
use crate::game::state::Game;
use crate::roles::{Character, Team};

/// Check win conditions after a kill chain / execution.
///
/// Task 9 stub: only detects no living Imp (Good). Full SW / 2-alive / Mayor later.
pub fn win_check(game: &mut Game) -> Option<Winner> {
    if game.winner.is_some() {
        return game.winner;
    }
    if matches!(game.phase, Phase::Ended { .. }) {
        return game.winner;
    }

    let living_imp = game
        .seats
        .iter()
        .any(|s| s.alive && s.true_character == Some(Character::Imp));

    if !living_imp {
        let winner = Winner::Good;
        game.winner = Some(winner);
        game.phase = Phase::Ended {
            winner,
            reason: EndReason::DemonDead,
        };
        game.pending_night = None;
        game.public_log.push(PublicEvent::GameEnded {
            winner: Team::Good,
        });
        return Some(winner);
    }

    None
}
