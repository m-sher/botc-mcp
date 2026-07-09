//! Spy / Recluse registration draws (§11.2).

use rand::Rng;

use crate::game::ids::SeatId;
use crate::game::state::Game;
use crate::roles::{Character, Team};

/// Whether `seat` registers as **evil** for Empath / Chef this detection.
///
/// - Recluse (good): evil with p=0.5
/// - Spy (evil): good with p=0.5 (hence evil with p=0.5)
/// - Others: true alignment
pub fn register_evil(game: &Game, seat: SeatId, event_label: &str) -> bool {
    let Some(s) = game.seats.iter().find(|x| x.id == seat) else {
        return false;
    };
    let Some(c) = s.true_character else {
        return false;
    };
    match c {
        Character::Recluse => {
            let mut rng = game.rng.substream(event_label);
            rng.gen_bool(0.5)
        }
        Character::Spy => {
            let mut rng = game.rng.substream(event_label);
            // Register as good with p=0.5 → evil otherwise.
            !rng.gen_bool(0.5)
        }
        other => other.team() == Team::Evil,
    }
}

/// Whether `seat` pings the Fortune Teller as Demon (excluding red herring).
///
/// - True Demon: always
/// - Recluse: demon with p=0.5
/// - Spy: never (v1 policy §9.6)
pub fn register_demon_for_ft(game: &Game, seat: SeatId, event_label: &str) -> bool {
    let Some(s) = game.seats.iter().find(|x| x.id == seat) else {
        return false;
    };
    let Some(c) = s.true_character else {
        return false;
    };
    match c {
        Character::Imp => true,
        Character::Recluse => {
            let mut rng = game.rng.substream(event_label);
            rng.gen_bool(0.5)
        }
        Character::Spy => false,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::{RoleAssignment, StartOpts};
    use crate::roles::Character;

    fn game_with(assignments: Vec<RoleAssignment>, seed: u64) -> Game {
        let names: Vec<String> = (0..assignments.len()).map(|i| format!("P{i}")).collect();
        let lobby = Game::create(names, seed).unwrap();
        let host = lobby.host_token.clone();
        let mut g = lobby.game;
        g.start_game(
            &host,
            StartOpts {
                assignments: Some(assignments),
            },
        )
        .unwrap();
        g
    }

    #[test]
    fn ordinary_evil_and_good_register_true() {
        let g = game_with(
            vec![
                RoleAssignment::normal(SeatId(0), Character::Empath),
                RoleAssignment::normal(SeatId(1), Character::Imp),
                RoleAssignment::normal(SeatId(2), Character::Poisoner),
                RoleAssignment::normal(SeatId(3), Character::Soldier),
                RoleAssignment::normal(SeatId(4), Character::Chef),
            ],
            1,
        );
        assert!(!register_evil(&g, SeatId(0), "t0"));
        assert!(register_evil(&g, SeatId(1), "t1"));
        assert!(register_evil(&g, SeatId(2), "t2"));
    }
}
