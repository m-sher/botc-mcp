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

/// Characters currently in play (true tokens), optionally filtered by type.
fn in_play_of_type(game: &Game, ty: CharacterType) -> Vec<Character> {
    game.seats
        .iter()
        .filter_map(|s| s.true_character)
        .filter(|c| c.character_type() == ty)
        .collect()
}

/// Pick a misregister token from `preferred` (in-play) if non-empty after excludes, else `fallback`.
fn pick_misreg_token(
    preferred: &[Character],
    fallback: &[Character],
    exclude: &[Character],
    rng: &mut impl Rng,
) -> Character {
    let pref: Vec<Character> = preferred
        .iter()
        .copied()
        .filter(|c| !exclude.contains(c) && *c != Character::Drunk)
        .collect();
    if let Some(c) = pref.choose(rng) {
        return *c;
    }
    let fb: Vec<Character> = fallback
        .iter()
        .copied()
        .filter(|c| !exclude.contains(c) && *c != Character::Drunk)
        .collect();
    *fb.choose(rng).unwrap_or(&Character::Soldier)
}

/// Character shown to Undertaker / Ravenkeeper-style "learn their character" abilities.
///
/// - Spy may register as a Townsfolk or Outsider (p=0.5 misregister); prefers in-play tokens
/// - Recluse may register as a Minion or Demon (p=0.5 misregister); prefers in-play tokens
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
                let in_play: Vec<Character> = game
                    .seats
                    .iter()
                    .filter_map(|x| x.true_character)
                    .filter(|ch| {
                        matches!(
                            ch.character_type(),
                            CharacterType::Townsfolk | CharacterType::Outsider
                        ) && *ch != Character::Drunk
                    })
                    .collect();
                let fallback: Vec<Character> = all_townsfolk()
                    .iter()
                    .chain(all_outsiders().iter())
                    .copied()
                    .collect();
                Some(pick_misreg_token(&in_play, &fallback, &[], &mut rng))
            }
        }
        Character::Recluse => {
            let mut rng = game.rng.substream(event_label);
            if rng.gen_bool(0.5) {
                Some(Character::Recluse)
            } else {
                let in_play: Vec<Character> = game
                    .seats
                    .iter()
                    .filter_map(|x| x.true_character)
                    .filter(|ch| {
                        matches!(
                            ch.character_type(),
                            CharacterType::Minion | CharacterType::Demon
                        )
                    })
                    .collect();
                let fallback: Vec<Character> = all_minions()
                    .iter()
                    .chain(all_demons().iter())
                    .copied()
                    .collect();
                Some(pick_misreg_token(&in_play, &fallback, &[], &mut rng))
            }
        }
        other => Some(other),
    }
}

/// Options for [`register_as_type_owner`] token selection.
#[derive(Debug, Clone, Copy, Default)]
pub struct TypeOwnerOpts {
    /// Info role acting seat: never name this seat's true/face character as the pair token
    /// when Spy/Recluse misregisters (Washerwoman must not hear "you are the X").
    pub acting_seat: Option<SeatId>,
}

/// Characters to exclude from named misregister tokens for the acting info role.
fn acting_exclude_chars(game: &Game, acting: Option<SeatId>) -> Vec<Character> {
    let Some(aid) = acting else {
        return Vec::new();
    };
    let Some(s) = game.seats.iter().find(|x| x.id == aid) else {
        return Vec::new();
    };
    let mut ex = Vec::new();
    if let Some(c) = s.true_character {
        ex.push(c);
    }
    if let Some(c) = s.believed_character {
        if !ex.contains(&c) {
            ex.push(c);
        }
    }
    // Also exclude visible face (Drunk: believed; others: true).
    if let Some(c) = s.visible_character() {
        if !ex.contains(&c) {
            ex.push(c);
        }
    }
    ex
}

/// Whether `seat` may appear as an owner of `ty` for pair-info roles (WW/Lib/Inv).
///
/// In addition to true-type seats:
/// - Spy may register as Townsfolk or Outsider (p=0.5 each detection)
/// - Recluse may register as Minion or Demon (p=0.5)
/// - Spy (true Minion) and Recluse (true Outsider) may **hide** from their true type with p=0.5
///
/// Returns the character token to name in the pair message when registering.
/// Named tokens for Spy/Recluse misregister prefer **in-play** characters of that type;
/// exclude Drunk for outsider faces; never name the acting seat's true/face character.
pub fn register_as_type_owner(
    game: &Game,
    seat: SeatId,
    ty: CharacterType,
    event_label: &str,
) -> Option<Character> {
    register_as_type_owner_with(game, seat, ty, event_label, TypeOwnerOpts::default())
}

/// Like [`register_as_type_owner`] with acting-seat token exclusions.
pub fn register_as_type_owner_with(
    game: &Game,
    seat: SeatId,
    ty: CharacterType,
    event_label: &str,
    opts: TypeOwnerOpts,
) -> Option<Character> {
    let s = game.seats.iter().find(|x| x.id == seat)?;
    let c = s.true_character?;
    let exclude = acting_exclude_chars(game, opts.acting_seat);

    // True type: Spy/Recluse may flip p=0.5 to hide from their true type detection.
    if c.character_type() == ty {
        match c {
            Character::Spy if ty == CharacterType::Minion => {
                let mut rng = game.rng.substream(event_label);
                if rng.gen_bool(0.5) {
                    return Some(Character::Spy);
                }
                return None;
            }
            Character::Recluse if ty == CharacterType::Outsider => {
                let mut rng = game.rng.substream(event_label);
                if rng.gen_bool(0.5) {
                    return Some(Character::Recluse);
                }
                return None;
            }
            other => return Some(other),
        }
    }

    match (c, ty) {
        (Character::Spy, CharacterType::Townsfolk) => {
            let mut rng = game.rng.substream(event_label);
            if rng.gen_bool(0.5) {
                let in_play = in_play_of_type(game, CharacterType::Townsfolk);
                let fallback = all_townsfolk().to_vec();
                Some(pick_misreg_token(&in_play, &fallback, &exclude, &mut rng))
            } else {
                None
            }
        }
        (Character::Spy, CharacterType::Outsider) => {
            let mut rng = game.rng.substream(event_label);
            if rng.gen_bool(0.5) {
                let in_play: Vec<Character> = in_play_of_type(game, CharacterType::Outsider)
                    .into_iter()
                    .filter(|x| *x != Character::Drunk)
                    .collect();
                let fallback: Vec<Character> = all_outsiders()
                    .iter()
                    .copied()
                    .filter(|x| *x != Character::Drunk)
                    .collect();
                Some(pick_misreg_token(&in_play, &fallback, &exclude, &mut rng))
            } else {
                None
            }
        }
        (Character::Recluse, CharacterType::Minion) => {
            let mut rng = game.rng.substream(event_label);
            if rng.gen_bool(0.5) {
                let in_play = in_play_of_type(game, CharacterType::Minion);
                let fallback = all_minions().to_vec();
                Some(pick_misreg_token(&in_play, &fallback, &exclude, &mut rng))
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

    #[test]
    fn spy_true_minion_can_hide_from_investigator() {
        let g = game_with(
            vec![
                RoleAssignment::normal(SeatId(0), Character::Investigator),
                RoleAssignment::normal(SeatId(1), Character::Spy),
                RoleAssignment::normal(SeatId(2), Character::Soldier),
                RoleAssignment::normal(SeatId(3), Character::Chef),
                RoleAssignment::normal(SeatId(4), Character::Imp),
            ],
            2,
        );
        let mut saw_some = false;
        let mut saw_none = false;
        for i in 0..64u32 {
            let lab = format!("spy_hide:{i}");
            match register_as_type_owner(&g, SeatId(1), CharacterType::Minion, &lab) {
                Some(Character::Spy) => saw_some = true,
                None => saw_none = true,
                Some(other) => panic!("Spy true minion owner should be Spy or None, got {other:?}"),
            }
        }
        assert!(saw_some && saw_none, "Spy should flip hide ~50% as minion owner");
    }

    #[test]
    fn recluse_true_outsider_can_hide_from_librarian() {
        let g = game_with(
            vec![
                RoleAssignment::normal(SeatId(0), Character::Librarian),
                RoleAssignment::normal(SeatId(1), Character::Recluse),
                RoleAssignment::normal(SeatId(2), Character::Soldier),
                RoleAssignment::normal(SeatId(3), Character::Poisoner),
                RoleAssignment::normal(SeatId(4), Character::Imp),
            ],
            3,
        );
        let mut saw_some = false;
        let mut saw_none = false;
        for i in 0..64u32 {
            let lab = format!("rec_hide:{i}");
            match register_as_type_owner(&g, SeatId(1), CharacterType::Outsider, &lab) {
                Some(Character::Recluse) => saw_some = true,
                None => saw_none = true,
                Some(other) => panic!("expected Recluse or None, got {other:?}"),
            }
        }
        assert!(saw_some && saw_none);
    }
}
