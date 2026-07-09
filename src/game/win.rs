//! Win conditions, demon death (Scarlet Woman), and related end-game helpers.
//!
//! Design §10.3–10.4, §13; `docs/win-conditions.md`.

use crate::comms::{PrivateMessage, PublicEvent};
use crate::game::ids::SeatId;
use crate::game::phase::{EndReason, Phase, Winner};
use crate::game::state::Game;
use crate::roles::{Character, Team};

/// End the game with a winner and public announcement.
pub fn end_game(game: &mut Game, winner: Winner, reason: EndReason) {
    if game.winner.is_some() || matches!(game.phase, Phase::Ended { .. }) {
        return;
    }
    game.winner = Some(winner);
    game.phase = Phase::Ended { winner, reason };
    game.pending_night = None;
    game.public_log.push(PublicEvent::GameEnded {
        winner: Team::from(winner),
    });
}

/// Check win conditions after a kill chain / execution / day death / end of nominations.
///
/// Order: no living Imp → Good (`DemonDead`); else living==2 with Imp → Evil (`EvilTwoAlive`).
/// Simultaneous DemonDead + two-alive → Good (first branch).
/// Saint / Mayor set winner at event time and short-circuit via [`end_game`].
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
        end_game(game, Winner::Good, EndReason::DemonDead);
        return Some(Winner::Good);
    }

    let living = game.seats.iter().filter(|s| s.alive).count();
    // Defensive: deaths are normally one-at-a-time, but `<= 2` covers multi-death edge cases.
    if living <= 2 {
        end_game(game, Winner::Evil, EndReason::EvilTwoAlive);
        return Some(Winner::Evil);
    }

    None
}

/// Handle Imp death that is **not** a starpass (execution, Slayer, Mayor bounce onto Imp, …).
///
/// `alive_before` counts living seats **including** the dying Imp (design §10.4).
/// If a living, non-disabled Scarlet Woman is in play and `alive_before >= 5`, she becomes Imp.
/// Otherwise leave no living Imp so [`win_check`] awards Good.
pub fn apply_demon_death(game: &mut Game, _dead_imp: SeatId, alive_before: u32) {
    if game.winner.is_some() || matches!(game.phase, Phase::Ended { .. }) {
        return;
    }

    if alive_before < 5 {
        return;
    }

    let sw_id = game.seats.iter().find_map(|s| {
        if s.alive
            && s.true_character == Some(Character::ScarletWoman)
            && !s.ability_disabled()
        {
            Some(s.id)
        } else {
            None
        }
    });

    let Some(sw_id) = sw_id else {
        return;
    };

    if let Some(seat) = game.seats.iter_mut().find(|s| s.id == sw_id) {
        seat.true_character = Some(Character::Imp);
        seat.believed_character = None;
        seat.is_drunk_outsider = false;
    }
    game.private_inboxes.push(
        sw_id,
        PrivateMessage::YouAre {
            character_label: Character::Imp.display_name().to_string(),
            team: Team::Evil,
            rules_path: Character::Imp.rules_doc_path().to_string(),
            note: Some("You are now the Imp.".to_string()),
        },
    );
}

/// Living-seat count (helper for callers computing `alive_before`).
pub fn living_count(game: &Game) -> u32 {
    game.seats.iter().filter(|s| s.alive).count() as u32
}
