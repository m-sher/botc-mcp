//! Bag composition, character selection, and setup options.

use rand::seq::SliceRandom;
use rand::Rng;

use crate::error::GameError;
use crate::game::ids::SeatId;
use crate::game::state::RoleAssignment;
use crate::rng::SeededRng;
use crate::roles::{
    all_minions, all_outsiders, all_townsfolk, Character, CharacterType, Team,
};

/// Base (pre-modifier) counts for Trouble Brewing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Composition {
    pub townsfolk: u8,
    pub outsiders: u8,
    pub minions: u8,
    pub demons: u8,
}

/// Host options for [`crate::game::Game::start_game`].
#[derive(Debug, Clone, Default)]
pub struct StartOpts {
    /// Full seat overrides for tests / scripted evals. Hard-fail if no Imp.
    pub assignments: Option<Vec<RoleAssignment>>,
}

/// Result of sampling a bag and assigning seats.
#[derive(Debug, Clone)]
pub struct BagResult {
    /// Seat order assignments with optional Drunk face.
    pub assignments: Vec<RoleAssignment>,
    pub red_herring: Option<SeatId>,
    pub demon_bluffs: Vec<Character>,
    /// Characters placed in the bag (order before seat shuffle is undefined).
    pub bag_set: Vec<Character>,
}

/// Base composition for `n` players (5–15). Matches `docs/setup.md`.
///
/// # Panics
/// Panics if `n` is outside 5..=15.
pub fn composition(n: u8) -> Composition {
    try_composition(n).unwrap_or_else(|| panic!("composition requires player count 5..=15, got {n}"))
}

/// Fallible composition lookup.
pub fn try_composition(n: u8) -> Option<Composition> {
    let (townsfolk, outsiders, minions, demons) = match n {
        5 => (3, 0, 1, 1),
        6 => (3, 1, 1, 1),
        7 => (5, 0, 1, 1),
        8 => (5, 1, 1, 1),
        9 => (5, 2, 1, 1),
        10 => (7, 0, 2, 1),
        11 => (7, 1, 2, 1),
        12 => (7, 2, 2, 1),
        13 => (9, 0, 3, 1),
        14 => (9, 1, 3, 1),
        15 => (9, 2, 3, 1),
        _ => return None,
    };
    Some(Composition {
        townsfolk,
        outsiders,
        minions,
        demons,
    })
}

fn sample_n(rng: &mut impl Rng, pool: &[Character], n: usize) -> Vec<Character> {
    debug_assert!(n <= pool.len());
    let mut pool = pool.to_vec();
    pool.shuffle(rng);
    pool.truncate(n);
    pool
}

/// Build a bag for `n` seats using labeled substreams of `rng`.
///
/// Algorithm (design §8.3): sample minions first; if Baron, outsiders += 2 and
/// townsfolk -= 2; sample outsiders and townsfolk; always include Imp; shuffle
/// onto seats; assign Drunk faces uniformly among all 13 Townsfolk.
pub fn build_bag(rng: &SeededRng, n: u8) -> Result<BagResult, GameError> {
    let base = try_composition(n).ok_or(GameError::BadRequest(
        "player count must be between 5 and 15 inclusive",
    ))?;

    let mut need_tf = base.townsfolk as usize;
    let mut need_out = base.outsiders as usize;
    let need_min = base.minions as usize;

    let mut min_rng = rng.substream("setup_minions");
    let minions = sample_n(&mut min_rng, all_minions(), need_min);

    if minions.contains(&Character::Baron) {
        need_out += 2;
        need_tf = need_tf.saturating_sub(2);
    }

    let mut out_rng = rng.substream("setup_outsiders");
    let outsiders = sample_n(&mut out_rng, all_outsiders(), need_out);

    let mut tf_rng = rng.substream("setup_townsfolk");
    let townsfolk = sample_n(&mut tf_rng, all_townsfolk(), need_tf);

    let mut bag: Vec<Character> = Vec::with_capacity(n as usize);
    bag.extend(townsfolk);
    bag.extend(outsiders);
    bag.extend(minions);
    bag.push(Character::Imp);

    if bag.len() != n as usize {
        return Err(GameError::IllegalAction(
            "bag size after setup does not match player count",
        ));
    }

    let bag_set = bag.clone();

    let mut assign_rng = rng.substream("setup_assign");
    bag.shuffle(&mut assign_rng);

    let mut face_rng = rng.substream("drunk_face");
    let townsfolk_pool = all_townsfolk();

    let mut assignments = Vec::with_capacity(bag.len());
    for (i, &ch) in bag.iter().enumerate() {
        let seat = SeatId(i as u8);
        if ch == Character::Drunk {
            let face = townsfolk_pool[face_rng.gen_range(0..townsfolk_pool.len())];
            assignments.push(RoleAssignment::drunk(seat, face)?);
        } else {
            assignments.push(RoleAssignment::normal(seat, ch));
        }
    }

    let red_herring = if bag_set.contains(&Character::FortuneTeller) {
        let good_seats: Vec<SeatId> = assignments
            .iter()
            .filter(|a| a.true_character.team() == Team::Good)
            .map(|a| a.seat)
            .collect();
        if good_seats.is_empty() {
            None
        } else {
            let mut hrng = rng.substream("red_herring");
            Some(good_seats[hrng.gen_range(0..good_seats.len())])
        }
    } else {
        None
    };

    let demon_bluffs = if n >= 7 {
        pick_demon_bluffs(rng, &bag_set)
    } else {
        Vec::new()
    };

    Ok(BagResult {
        assignments,
        red_herring,
        demon_bluffs,
        bag_set,
    })
}

fn pick_demon_bluffs(rng: &SeededRng, bag_set: &[Character]) -> Vec<Character> {
    let in_bag: std::collections::HashSet<Character> = bag_set.iter().copied().collect();
    let mut good_not_in_play: Vec<Character> = all_townsfolk()
        .iter()
        .chain(all_outsiders().iter())
        .copied()
        .filter(|c| !in_bag.contains(c))
        .collect();
    let mut brng = rng.substream("demon_bluffs");
    good_not_in_play.shuffle(&mut brng);
    good_not_in_play.truncate(3.min(good_not_in_play.len()));
    good_not_in_play
}

/// Validate a full fixed assignment list for start.
pub fn validate_fixed_assignments(
    n_seats: usize,
    assignments: &[RoleAssignment],
) -> Result<(), GameError> {
    if assignments.len() != n_seats {
        return Err(GameError::BadRequest(
            "assignments must cover every seat exactly once",
        ));
    }
    let mut seen = vec![false; n_seats];
    for a in assignments {
        let i = a.seat.0 as usize;
        if i >= n_seats {
            return Err(GameError::NoSuchSeat);
        }
        if seen[i] {
            return Err(GameError::BadRequest("duplicate seat in assignments"));
        }
        seen[i] = true;
        if a.true_character == Character::Drunk {
            let face = a.believed_character.ok_or(GameError::IllegalAction(
                "Drunk assignment requires a Townsfolk believed_character face",
            ))?;
            if face.character_type() != CharacterType::Townsfolk {
                return Err(GameError::IllegalAction(
                    "Drunk face must be a Townsfolk character",
                ));
            }
        } else if a.believed_character.is_some() {
            return Err(GameError::IllegalAction(
                "believed_character only valid for Drunk",
            ));
        }
    }
    if !assignments
        .iter()
        .any(|a| a.true_character == Character::Imp)
    {
        return Err(GameError::IllegalAction("bag must include Imp"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn composition_8_unit() {
        let c = composition(8);
        assert_eq!((c.townsfolk, c.outsiders, c.minions, c.demons), (5, 1, 1, 1));
    }

    #[test]
    fn baron_increases_outsiders() {
        // Seed hunt is brittle; instead call build with many seeds and check invariant
        // when Baron appears: outsiders count == base+2.
        let mut found_baron = false;
        for seed in 0..200u64 {
            let rng = SeededRng::from_seed(seed);
            let bag = build_bag(&rng, 8).unwrap();
            let base = composition(8);
            let outs = bag
                .bag_set
                .iter()
                .filter(|c| c.character_type() == CharacterType::Outsider)
                .count();
            let tfs = bag
                .bag_set
                .iter()
                .filter(|c| c.character_type() == CharacterType::Townsfolk)
                .count();
            if bag.bag_set.contains(&Character::Baron) {
                found_baron = true;
                assert_eq!(outs, base.outsiders as usize + 2);
                assert_eq!(tfs, base.townsfolk as usize - 2);
            } else {
                assert_eq!(outs, base.outsiders as usize);
                assert_eq!(tfs, base.townsfolk as usize);
            }
            assert_eq!(bag.bag_set.len(), 8);
            assert!(bag.bag_set.contains(&Character::Imp));
        }
        assert!(found_baron, "expected some seed to include Baron");
    }

    #[test]
    fn drunk_gets_townsfolk_face() {
        let mut found = false;
        for seed in 0..300u64 {
            let rng = SeededRng::from_seed(seed);
            let bag = build_bag(&rng, 8).unwrap();
            for a in &bag.assignments {
                if a.true_character == Character::Drunk {
                    found = true;
                    let face = a.believed_character.unwrap();
                    assert_eq!(face.character_type(), CharacterType::Townsfolk);
                }
            }
        }
        assert!(found, "expected Drunk in some 8p bags");
    }
}
