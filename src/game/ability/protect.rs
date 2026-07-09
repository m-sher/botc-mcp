//! Monk protection and passive Demon-kill immunities (Soldier).

use crate::game::ids::SeatId;
use crate::game::state::Game;
use crate::roles::Character;

/// True if the seat has active Monk protection for tonight's demon attack.
pub fn is_monk_protected(game: &Game, seat: SeatId) -> bool {
    game.seats
        .iter()
        .find(|s| s.id == seat)
        .map(|s| s.monk_protected_tonight)
        .unwrap_or(false)
}

/// True if the seat is a living true Soldier whose ability is not disabled.
pub fn is_soldier_immune(game: &Game, seat: SeatId) -> bool {
    game.seats
        .iter()
        .find(|s| s.id == seat)
        .map(|s| {
            s.alive
                && s.true_character == Some(Character::Soldier)
                && !s.ability_disabled()
        })
        .unwrap_or(false)
}

/// Apply Monk protect to a living other seat (caller already checked Monk not disabled).
pub fn apply_monk_protect(game: &mut Game, target: SeatId) {
    if let Some(t) = game.seats.iter_mut().find(|s| s.id == target) {
        t.monk_protected_tonight = true;
    }
}

/// Clear all monk protection markers (dawn).
pub fn clear_monk_protection(game: &mut Game) {
    for seat in &mut game.seats {
        seat.monk_protected_tonight = false;
    }
}

/// Whether a living seat is a legal Mayor-bounce victim (not soldier/monk protected).
pub fn is_demon_killable(game: &Game, seat: SeatId) -> bool {
    let Some(s) = game.seats.iter().find(|x| x.id == seat) else {
        return false;
    };
    if !s.alive {
        return false;
    }
    if is_monk_protected(game, seat) {
        return false;
    }
    if is_soldier_immune(game, seat) {
        return false;
    }
    true
}
