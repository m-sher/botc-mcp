//! Night info ability resolution (§9.6) + false info when disabled (§11.1).

use rand::seq::SliceRandom;
use rand::Rng;

use crate::comms::PrivateMessage;
use crate::error::GameError;
use crate::game::ids::SeatId;
use crate::game::night::NightActionPayload;
use crate::game::phase::NightStep;
use crate::game::state::Game;
use crate::roles::{
    all_demons, all_minions, all_outsiders, all_townsfolk, Character, CharacterType,
};

use super::register::{register_demon_for_ft, register_evil};
use super::NightEffect;

/// Resolve an info / passive-choice night step. Mutates game (private msgs, butler master).
pub fn resolve(
    game: &mut Game,
    step: NightStep,
    payload: Option<&NightActionPayload>,
) -> Result<NightEffect, GameError> {
    match step {
        NightStep::Spy { seat } => resolve_spy(game, seat),
        NightStep::Washerwoman { seat } => resolve_washerwoman(game, seat),
        NightStep::Librarian { seat } => resolve_librarian(game, seat),
        NightStep::Investigator { seat } => resolve_investigator(game, seat),
        NightStep::Chef { seat } => resolve_chef(game, seat),
        NightStep::Empath { seat } => resolve_empath(game, seat),
        NightStep::FortuneTeller { seat } => {
            let payload = payload.ok_or(GameError::WrongPayload)?;
            resolve_fortune_teller(game, seat, payload)
        }
        NightStep::Butler { seat } => {
            let payload = payload.ok_or(GameError::WrongPayload)?;
            resolve_butler(game, seat, payload)
        }
        NightStep::Undertaker { seat } => resolve_undertaker(game, seat),
        NightStep::Ravenkeeper { seat } => {
            let payload = payload.ok_or(GameError::WrongPayload)?;
            resolve_ravenkeeper(game, seat, payload)
        }
        _ => Ok(NightEffect::default()),
    }
}

fn stream_label(game: &Game, role: &str) -> String {
    format!("{role}:c{}", game.night_cursor)
}

fn seat_disabled(game: &Game, seat: SeatId) -> bool {
    game.seats
        .iter()
        .find(|s| s.id == seat)
        .map(|s| s.ability_disabled())
        .unwrap_or(true)
}

fn push_result(game: &mut Game, seat: SeatId, text: String) -> NightEffect {
    let msg = PrivateMessage::NightResult { text: text.clone() };
    game.private_inboxes.push(seat, msg.clone());
    NightEffect {
        private: vec![(seat, msg)],
    }
}

fn seat_name(game: &Game, seat: SeatId) -> String {
    game.seats
        .iter()
        .find(|s| s.id == seat)
        .map(|s| s.display_name.clone())
        .unwrap_or_else(|| format!("seat {}", seat.0))
}

fn format_seat(game: &Game, seat: SeatId) -> String {
    format!("{} (seat {})", seat_name(game, seat), seat.0)
}

// ---------------------------------------------------------------------------
// Pair-info helpers (Washerwoman / Librarian / Investigator)
// ---------------------------------------------------------------------------

struct PairInfo {
    character: Character,
    seat_a: SeatId,
    seat_b: SeatId,
}

fn pair_message(game: &Game, ability: &str, info: &PairInfo) -> String {
    format!(
        "{ability}: one of {} and {} is the {}.",
        format_seat(game, info.seat_a),
        format_seat(game, info.seat_b),
        info.character.display_name()
    )
}

fn pick_two_distinct_seats(game: &Game, rng: &mut impl Rng) -> Option<(SeatId, SeatId)> {
    let ids: Vec<SeatId> = game.seats.iter().map(|s| s.id).collect();
    if ids.len() < 2 {
        return None;
    }
    let mut pick = ids;
    pick.shuffle(rng);
    Some((pick[0], pick[1]))
}

fn seats_of_type(game: &Game, ty: CharacterType) -> Vec<(SeatId, Character)> {
    game.seats
        .iter()
        .filter_map(|s| {
            let c = s.true_character?;
            if c.character_type() == ty {
                Some((s.id, c))
            } else {
                None
            }
        })
        .collect()
}

/// Owners eligible for WW/Lib/Inv "correct" seat.
///
/// True seats of `ty`, plus for Townsfolk: Spy may register as a Townsfolk with p=0.5
/// (design §11.2 / §11.5) and become a valid correct-seat owner for that character.
fn pair_owners(
    game: &Game,
    ty: CharacterType,
    pool: &[Character],
    rng: &mut impl Rng,
) -> Vec<(SeatId, Character)> {
    let mut owners = seats_of_type(game, ty);
    if ty == CharacterType::Townsfolk {
        if let Some(spy_id) = game
            .seats
            .iter()
            .find(|s| s.true_character == Some(Character::Spy))
            .map(|s| s.id)
        {
            if rng.gen_bool(0.5) {
                // Prefer an in-play Townsfolk token; else any from the townsfolk pool.
                let character = if !owners.is_empty() {
                    owners[rng.gen_range(0..owners.len())].1
                } else if let Some(&c) = pool.choose(rng) {
                    c
                } else {
                    Character::Soldier
                };
                owners.push((spy_id, character));
            }
        }
    }
    owners
}

fn truthful_pair_info(
    game: &Game,
    ty: CharacterType,
    pool: &[Character],
    rng: &mut impl Rng,
) -> Option<PairInfo> {
    let owners = pair_owners(game, ty, pool, rng);
    if owners.is_empty() {
        return None;
    }
    let (correct_seat, character) = owners[rng.gen_range(0..owners.len())];
    let others: Vec<SeatId> = game
        .seats
        .iter()
        .map(|s| s.id)
        .filter(|id| *id != correct_seat)
        .collect();
    if others.is_empty() {
        return None;
    }
    let wrong = others[rng.gen_range(0..others.len())];
    let (seat_a, seat_b) = if rng.gen_bool(0.5) {
        (correct_seat, wrong)
    } else {
        (wrong, correct_seat)
    };
    Some(PairInfo {
        character,
        seat_a,
        seat_b,
    })
}

fn lie_pair_info(game: &Game, pool: &[Character], rng: &mut impl Rng) -> Option<PairInfo> {
    let (seat_a, seat_b) = pick_two_distinct_seats(game, rng)?;
    let character = *pool.choose(rng)?;
    Some(PairInfo {
        character,
        seat_a,
        seat_b,
    })
}

fn resolve_pair_role(
    game: &mut Game,
    seat: SeatId,
    ability: &str,
    ty: CharacterType,
    pool: &[Character],
    stream: &str,
    zero_message: Option<&str>,
) -> Result<NightEffect, GameError> {
    let label = stream_label(game, stream);
    let mut rng = game.rng.substream(&label);
    let disabled = seat_disabled(game, seat);

    if !disabled {
        // Zero-path uses true bag count only (Spy registration does not invent Outsiders).
        if seats_of_type(game, ty).is_empty() {
            if let Some(z) = zero_message {
                return Ok(push_result(game, seat, z.to_string()));
            }
        }
        if let Some(info) = truthful_pair_info(game, ty, pool, &mut rng) {
            let text = pair_message(game, ability, &info);
            return Ok(push_result(game, seat, text));
        }
    }

    // Disabled, or no owners and no zero path: lie.
    let info = lie_pair_info(game, pool, &mut rng).ok_or(GameError::IllegalAction(
        "cannot generate pair info",
    ))?;
    let text = pair_message(game, ability, &info);
    Ok(push_result(game, seat, text))
}

// ---------------------------------------------------------------------------
// Spy grimoire
// ---------------------------------------------------------------------------

/// Structured grimoire snapshot for the Spy (true if ability active; seeded fake if disabled).
fn resolve_spy(game: &mut Game, seat: SeatId) -> Result<NightEffect, GameError> {
    let disabled = seat_disabled(game, seat);
    let text = if disabled {
        let label = stream_label(game, "spy_lie");
        let mut rng = game.rng.substream(&label);
        format_fake_grimoire(game, &mut rng)
    } else {
        format_true_grimoire(game)
    };
    Ok(push_result(game, seat, text))
}

fn format_true_grimoire(game: &Game) -> String {
    let mut lines = vec!["Spy: Grimoire".to_string()];
    for s in &game.seats {
        lines.push(format_grimoire_seat_line(
            &s.display_name,
            s.id,
            s.alive,
            s.true_character,
            s.poisoned,
            s.believed_character,
        ));
    }
    lines.join("\n")
}

fn format_fake_grimoire(game: &Game, rng: &mut impl Rng) -> String {
    let pool: Vec<Character> = all_townsfolk()
        .iter()
        .chain(all_outsiders().iter())
        .chain(all_minions().iter())
        .chain(all_demons().iter())
        .copied()
        .collect();
    let mut lines = vec!["Spy: Grimoire".to_string()];
    for s in &game.seats {
        // Keep name/alive/poisoned shape; scramble character tokens.
        let fake_char = *pool.choose(rng).unwrap_or(&Character::Soldier);
        // Occasionally invent a believed face for variety.
        let believed = if rng.gen_bool(0.2) {
            Some(*pool.choose(rng).unwrap_or(&Character::Soldier))
        } else {
            None
        };
        lines.push(format_grimoire_seat_line(
            &s.display_name,
            s.id,
            s.alive,
            Some(fake_char),
            s.poisoned,
            believed,
        ));
    }
    lines.join("\n")
}

fn format_grimoire_seat_line(
    name: &str,
    id: SeatId,
    alive: bool,
    true_character: Option<Character>,
    poisoned: bool,
    believed: Option<Character>,
) -> String {
    let char_name = true_character
        .map(|c| c.display_name())
        .unwrap_or("unknown");
    let alive_s = if alive { "alive" } else { "dead" };
    let mut line = format!(
        "- {name} (seat {}): {alive_s}, {char_name}, poisoned={poisoned}",
        id.0
    );
    if let Some(b) = believed {
        line.push_str(&format!(", believed={}", b.display_name()));
    }
    line
}

fn resolve_washerwoman(game: &mut Game, seat: SeatId) -> Result<NightEffect, GameError> {
    resolve_pair_role(
        game,
        seat,
        "Washerwoman",
        CharacterType::Townsfolk,
        all_townsfolk(),
        "washerwoman",
        None,
    )
}

fn resolve_librarian(game: &mut Game, seat: SeatId) -> Result<NightEffect, GameError> {
    resolve_pair_role(
        game,
        seat,
        "Librarian",
        CharacterType::Outsider,
        all_outsiders(),
        "librarian",
        Some("Librarian: there are 0 Outsiders in play."),
    )
}

fn resolve_investigator(game: &mut Game, seat: SeatId) -> Result<NightEffect, GameError> {
    resolve_pair_role(
        game,
        seat,
        "Investigator",
        CharacterType::Minion,
        all_minions(),
        "investigator",
        None,
    )
}

// ---------------------------------------------------------------------------
// Chef / Empath
// ---------------------------------------------------------------------------

fn chef_true_count(game: &Game) -> u8 {
    let n = game.seats.len();
    if n < 2 {
        return 0;
    }
    let mut count = 0u8;
    for i in 0..n {
        let a = game.seats[i].id;
        let b = game.seats[(i + 1) % n].id;
        let lab_a = format!("chef_reg:{}:{}:a", game.night_cursor, a.0);
        let lab_b = format!("chef_reg:{}:{}:b", game.night_cursor, b.0);
        if register_evil(game, a, &lab_a) && register_evil(game, b, &lab_b) {
            count += 1;
        }
    }
    count
}

fn resolve_chef(game: &mut Game, seat: SeatId) -> Result<NightEffect, GameError> {
    let truth = chef_true_count(game);
    let shown = if seat_disabled(game, seat) {
        let label = stream_label(game, "chef_lie");
        let mut rng = game.rng.substream(&label);
        lie_chef_count(&mut rng, truth, game.seats.len())
    } else {
        truth
    };
    let text = format!("Chef: you learn that there are {shown} pairs of evil neighbors.");
    Ok(push_result(game, seat, text))
}

fn lie_chef_count(rng: &mut impl Rng, truth: u8, n: usize) -> u8 {
    let max = 4u8.min(n as u8);
    let options: Vec<u8> = (0..=max).filter(|&x| x != truth).collect();
    if options.is_empty() {
        truth
    } else {
        options[rng.gen_range(0..options.len())]
    }
}

/// Living neighbors CW / CCW (skip dead). Same seat counted once if only one other living.
pub fn empath_true_count(game: &Game, seat: SeatId) -> u8 {
    let Some(idx) = game.seats.iter().position(|s| s.id == seat) else {
        return 0;
    };
    let Some(cw) = next_living_index(&game.seats, idx, 1) else {
        return 0;
    };
    let Some(ccw) = next_living_index(&game.seats, idx, -1) else {
        return 0;
    };
    // Alone: both walks return self.
    if cw == idx {
        return 0;
    }
    if cw == ccw {
        let id = game.seats[cw].id;
        let lab = format!("empath_reg:{}:{}", game.night_cursor, id.0);
        return if register_evil(game, id, &lab) { 1 } else { 0 };
    }
    let mut count = 0u8;
    for ni in [cw, ccw] {
        let id = game.seats[ni].id;
        let lab = format!("empath_reg:{}:{}", game.night_cursor, id.0);
        if register_evil(game, id, &lab) {
            count += 1;
        }
    }
    count
}

fn next_living_index(seats: &[crate::game::seat::Seat], from: usize, dir: isize) -> Option<usize> {
    let n = seats.len() as isize;
    if n == 0 {
        return None;
    }
    for step in 1..=n {
        let i = (from as isize + dir * step).rem_euclid(n) as usize;
        if seats[i].alive {
            return Some(i);
        }
    }
    None
}

fn lie_count_0_2(rng: &mut impl Rng, truth: u8) -> u8 {
    let options: Vec<u8> = (0u8..=2).filter(|&x| x != truth).collect();
    options[rng.gen_range(0..options.len())]
}

fn resolve_empath(game: &mut Game, seat: SeatId) -> Result<NightEffect, GameError> {
    let truth = empath_true_count(game, seat);
    let shown = if seat_disabled(game, seat) {
        let label = stream_label(game, "empath_lie");
        let mut rng = game.rng.substream(&label);
        lie_count_0_2(&mut rng, truth)
    } else {
        truth
    };
    let text = format!("Empath: you learn that {shown} of your living neighbors are evil.");
    Ok(push_result(game, seat, text))
}

// ---------------------------------------------------------------------------
// Fortune Teller
// ---------------------------------------------------------------------------

fn ft_true_yes(game: &Game, a: SeatId, b: SeatId) -> bool {
    for (seat, tag) in [(a, "a"), (b, "b")] {
        if game.red_herring == Some(seat) {
            return true;
        }
        let lab = format!("ft_reg:{}:{}:{}", game.night_cursor, seat.0, tag);
        if register_demon_for_ft(game, seat, &lab) {
            return true;
        }
    }
    false
}

fn resolve_fortune_teller(
    game: &mut Game,
    seat: SeatId,
    payload: &NightActionPayload,
) -> Result<NightEffect, GameError> {
    let NightActionPayload::PickTwo { a, b } = payload else {
        return Err(GameError::WrongPayload);
    };
    let truth = ft_true_yes(game, *a, *b);
    let shown = if seat_disabled(game, seat) {
        // §11.1: always lie when disabled.
        !truth
    } else {
        truth
    };
    let answer = if shown { "yes" } else { "no" };
    let text = format!(
        "Fortune Teller: {} — at least one of {} and {} is a Demon? {answer}.",
        if shown { "YES" } else { "NO" },
        format_seat(game, *a),
        format_seat(game, *b),
    );
    Ok(push_result(game, seat, text))
}

// ---------------------------------------------------------------------------
// Butler / Undertaker / Ravenkeeper
// ---------------------------------------------------------------------------

fn resolve_butler(
    game: &mut Game,
    seat: SeatId,
    payload: &NightActionPayload,
) -> Result<NightEffect, GameError> {
    let NightActionPayload::PickOne { target } = payload else {
        return Err(GameError::WrongPayload);
    };
    // Always store master; day vote enforces ability_disabled (no real restriction).
    if let Some(s) = game.seats.iter_mut().find(|s| s.id == seat) {
        s.butler_master = Some(*target);
    }
    let text = format!(
        "Butler: your master is {}.",
        format_seat(game, *target)
    );
    Ok(push_result(game, seat, text))
}

fn resolve_undertaker(game: &mut Game, seat: SeatId) -> Result<NightEffect, GameError> {
    let executed = game.executed_today;
    let shown = if seat_disabled(game, seat) {
        let label = stream_label(game, "undertaker_lie");
        let mut rng = game.rng.substream(&label);
        random_pool_character(&mut rng)
    } else if let Some(ex) = executed {
        game.seats
            .iter()
            .find(|s| s.id == ex)
            .and_then(|s| s.true_character)
            .unwrap_or(Character::Imp)
    } else {
        // Should not wake without execution; soft fallback.
        return Ok(push_result(
            game,
            seat,
            "Undertaker: nobody was executed today.".into(),
        ));
    };
    let text = format!(
        "Undertaker: the player executed today was the {}.",
        shown.display_name()
    );
    Ok(push_result(game, seat, text))
}

fn resolve_ravenkeeper(
    game: &mut Game,
    seat: SeatId,
    payload: &NightActionPayload,
) -> Result<NightEffect, GameError> {
    let NightActionPayload::PickOne { target } = payload else {
        return Err(GameError::WrongPayload);
    };
    let shown = if seat_disabled(game, seat) {
        let label = stream_label(game, "ravenkeeper_lie");
        let mut rng = game.rng.substream(&label);
        random_pool_character(&mut rng)
    } else {
        // v1: true character token (Drunk → Drunk; Spy → Spy).
        game.seats
            .iter()
            .find(|s| s.id == *target)
            .and_then(|s| s.true_character)
            .unwrap_or(Character::Soldier)
    };
    let text = format!(
        "Ravenkeeper: {} is the {}.",
        format_seat(game, *target),
        shown.display_name()
    );
    Ok(push_result(game, seat, text))
}

fn random_pool_character(rng: &mut impl Rng) -> Character {
    let pool: Vec<Character> = all_townsfolk()
        .iter()
        .chain(all_outsiders().iter())
        .chain(all_minions().iter())
        .chain(all_demons().iter())
        .copied()
        .collect();
    *pool.choose(rng).unwrap_or(&Character::Soldier)
}

#[cfg(test)]
mod unit_tests {
    use super::*;
    use crate::game::{RoleAssignment, StartOpts};

    fn five(
        a0: Character,
        a1: Character,
        a2: Character,
        a3: Character,
        a4: Character,
        seed: u64,
    ) -> Game {
        let lobby = Game::create(
            vec![
                "A".into(),
                "B".into(),
                "C".into(),
                "D".into(),
                "E".into(),
            ],
            seed,
        )
        .unwrap();
        let host = lobby.host_token.clone();
        let mut g = lobby.game;
        g.start_game(
            &host,
            StartOpts {
                assignments: Some(vec![
                    RoleAssignment::normal(SeatId(0), a0),
                    RoleAssignment::normal(SeatId(1), a1),
                    RoleAssignment::normal(SeatId(2), a2),
                    RoleAssignment::normal(SeatId(3), a3),
                    RoleAssignment::normal(SeatId(4), a4),
                ]),
            },
        )
        .unwrap();
        g
    }

    #[test]
    fn empath_neighbors_count_unit() {
        // A Soldier, B Imp, C Empath, D Soldier, E Soldier — Empath neighbors Imp + Soldier => 1
        let g = five(
            Character::Soldier,
            Character::Imp,
            Character::Empath,
            Character::Soldier,
            Character::Chef,
            11,
        );
        assert_eq!(empath_true_count(&g, SeatId(2)), 1);
    }
}
