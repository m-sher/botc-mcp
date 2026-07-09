//! Spy / Recluse registration draws (§11.2).

use rand::seq::SliceRandom;
use rand::Rng;

use crate::game::ids::SeatId;
use crate::game::state::Game;
use crate::roles::{
    all_demons, all_minions, all_outsiders, all_townsfolk, Character, CharacterType, Team,
};

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

/// Whether the nominator registers as Townsfolk for the Virgin.
///
/// - True Townsfolk: always
/// - Spy: Townsfolk with p=0.5 (may register as good Townsfolk)
/// - Others (incl. Drunk Outsider, Recluse): never as Townsfolk
pub fn registers_as_townsfolk(game: &Game, seat: SeatId, event_label: &str) -> bool {
    let Some(s) = game.seats.iter().find(|x| x.id == seat) else {
        return false;
    };
    let Some(c) = s.true_character else {
        return false;
    };
    match c {
        Character::Spy => {
            let mut rng = game.rng.substream(event_label);
            rng.gen_bool(0.5)
        }
        other => other.character_type() == CharacterType::Townsfolk,
    }
}

/// Character shown to Undertaker / Ravenkeeper-style "learn their character" abilities.
///
/// - Spy may register as a Townsfolk or Outsider (p=0.5 misregister)
/// - Recluse may register as a Minion or Demon (p=0.5 misregister)
/// - Others: true token
pub fn register_character(game: &Game, seat: SeatId, event_label: &str) -> Option<Character> {
    let s = game.seats.iter().find(|x| x.id == seat)?;
    let c = s.true_character?;
    match c {
        Character::Spy => {
            let mut rng = game.rng.substream(event_label);
            if rng.gen_bool(0.5) {
                Some(Character::Spy)
            } else {
                let pool: Vec<Character> = all_townsfolk()
                    .iter()
                    .chain(all_outsiders().iter())
                    .copied()
                    .filter(|x| *x != Character::Drunk) // avoid leaking Drunk token as face-like
                    .collect();
                Some(*pool.choose(&mut rng).unwrap_or(&Character::Soldier))
            }
        }
        Character::Recluse => {
            let mut rng = game.rng.substream(event_label);
            if rng.gen_bool(0.5) {
                Some(Character::Recluse)
            } else {
                let pool: Vec<Character> = all_minions()
                    .iter()
                    .chain(all_demons().iter())
                    .copied()
                    .collect();
                Some(*pool.choose(&mut rng).unwrap_or(&Character::Imp))
            }
        }
        other => Some(other),
    }
}

/// Whether `seat` may appear as an owner of `ty` for pair-info roles (WW/Lib/Inv).
///
/// In addition to true-type seats:
/// - Spy may register as Townsfolk or Outsider (p=0.5 each detection)
/// - Recluse may register as Minion or Demon (p=0.5)
///
/// Returns the character token to name in the pair message when registering.
pub fn register_as_type_owner(
    game: &Game,
    seat: SeatId,
    ty: CharacterType,
    event_label: &str,
) -> Option<Character> {
    let s = game.seats.iter().find(|x| x.id == seat)?;
    let c = s.true_character?;
    // True type always owns that type.
    if c.character_type() == ty {
        return Some(c);
    }
    match (c, ty) {
        (Character::Spy, CharacterType::Townsfolk) => {
            let mut rng = game.rng.substream(event_label);
            if rng.gen_bool(0.5) {
                let pool = all_townsfolk();
                Some(*pool.choose(&mut rng).unwrap_or(&Character::Soldier))
            } else {
                None
            }
        }
        (Character::Spy, CharacterType::Outsider) => {
            let mut rng = game.rng.substream(event_label);
            if rng.gen_bool(0.5) {
                // Prefer non-Drunk outsider faces for the named token.
                let pool: Vec<Character> = all_outsiders()
                    .iter()
                    .copied()
                    .filter(|x| *x != Character::Drunk)
                    .collect();
                Some(*pool.choose(&mut rng).unwrap_or(&Character::Recluse))
            } else {
                None
            }
        }
        (Character::Recluse, CharacterType::Minion) => {
            let mut rng = game.rng.substream(event_label);
            if rng.gen_bool(0.5) {
                let pool = all_minions();
                Some(*pool.choose(&mut rng).unwrap_or(&Character::Poisoner))
            } else {
                None
            }
        }
        (Character::Recluse, CharacterType::Demon) => {
            let mut rng = game.rng.substream(event_label);
            if rng.gen_bool(0.5) {
                Some(Character::Imp)
            } else {
                None
            }
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::{RoleAssignment, StartOpts};
    use crate::roles::Character;

    fn game_with(assignments: Vec<RoleAssignment>, seed: u64) -> Game {
        let names: Vec<String> = (0..assignments.len()).map(|i| format!("P{i}")).collect();
        let lobby = Game::create_with_salt(names, seed, 0).unwrap();
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
