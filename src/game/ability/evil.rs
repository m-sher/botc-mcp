//! Poisoner, Imp kill, starpass, Mayor bounce (§9.4–9.5, §11.3–11.4).

use rand::seq::SliceRandom;

use crate::comms::PrivateMessage;
use crate::game::ids::SeatId;
use crate::game::phase::NightStep;
use crate::game::state::Game;
use crate::game::win;
use crate::roles::{Character, CharacterType, Team};

use super::protect::{is_demon_killable, is_monk_protected, is_soldier_immune};

/// Outcome of [`try_demon_kill`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KillResult {
    /// Target already dead — kill sinks with no public death.
    Sank,
    /// No death (Monk, Soldier, Mayor bounce nowhere, or Imp poisoned).
    Survived,
    /// A seat died to the demon (normal kill or Mayor bounce victim).
    Died(SeatId),
    /// Imp self-kill; a living Minion became the Imp.
    Starpass {
        dead_imp: SeatId,
        new_imp: SeatId,
    },
}

/// Clear `poisoned` on every seat (start of Poisoner step).
pub fn clear_poisons(game: &mut Game) {
    for seat in &mut game.seats {
        seat.poisoned = false;
    }
}

/// Mark `target` poisoned (Poisoner ability not disabled).
pub fn apply_poison(game: &mut Game, target: SeatId) {
    if let Some(t) = game.seats.iter_mut().find(|s| s.id == target) {
        t.poisoned = true;
    }
}

/// Resolve Imp night kill / starpass (spec §9.5).
///
/// `demon_seat` is the acting Imp. Side-effects: deaths, Ravenkeeper queue insert,
/// private `YouAre` on starpass, optional win check.
pub fn try_demon_kill(game: &mut Game, demon_seat: SeatId, target: SeatId) -> KillResult {
    let imp_disabled = game
        .seats
        .iter()
        .find(|s| s.id == demon_seat)
        .map(|s| s.ability_disabled())
        .unwrap_or(true);
    if imp_disabled {
        return KillResult::Survived;
    }

    if target == demon_seat {
        return resolve_starpass(game, demon_seat);
    }

    kill_chain(game, target)
}

fn resolve_starpass(game: &mut Game, imp_seat: SeatId) -> KillResult {
    // Imp dies first.
    mark_dead(game, imp_seat);

    let mut living_minions: Vec<SeatId> = game
        .seats
        .iter()
        .filter(|s| {
            s.alive
                && s.true_character
                    .is_some_and(|c| c.character_type() == CharacterType::Minion)
        })
        .map(|s| s.id)
        .collect();

    if living_minions.is_empty() {
        // §11.4: no minion → Imp dies and Good wins.
        win::win_check(game);
        return KillResult::Died(imp_seat);
    }

    living_minions.sort_by_key(|id| id.0);
    let label = format!("starpass:c{}", game.night_cursor);
    let mut rng = game.rng.substream(&label);
    let new_imp = *living_minions.choose(&mut rng).expect("non-empty minions");

    if let Some(seat) = game.seats.iter_mut().find(|s| s.id == new_imp) {
        seat.true_character = Some(Character::Imp);
        seat.believed_character = None;
        seat.is_drunk_outsider = false;
    }
    game.private_inboxes.push(
        new_imp,
        PrivateMessage::YouAre {
            character_label: Character::Imp.display_name().to_string(),
            team: Team::Evil,
            rules_path: Character::Imp.rules_doc_path().to_string(),
            note: Some("You are now the Imp.".to_string()),
        },
    );

    win::win_check(game);
    KillResult::Starpass {
        dead_imp: imp_seat,
        new_imp,
    }
}

/// Apply death chain for a non-self demon target (steps 1–6 of §9.5).
fn kill_chain(game: &mut Game, target: SeatId) -> KillResult {
    let Some(seat) = game.seats.iter().find(|s| s.id == target) else {
        return KillResult::Sank;
    };
    if !seat.alive {
        return KillResult::Sank;
    }

    if is_monk_protected(game, target) {
        return KillResult::Survived;
    }
    if is_soldier_immune(game, target) {
        return KillResult::Survived;
    }

    // Mayor bounce (§11.3).
    let is_mayor = seat.true_character == Some(Character::Mayor) && !seat.ability_disabled();
    if is_mayor {
        return mayor_bounce(game, target);
    }

    die_from_demon(game, target);
    KillResult::Died(target)
}

fn mayor_bounce(game: &mut Game, mayor: SeatId) -> KillResult {
    let mut candidates: Vec<SeatId> = game
        .seats
        .iter()
        .filter(|s| s.id != mayor && is_demon_killable(game, s.id))
        .map(|s| s.id)
        .collect();
    // Exclude other living Mayors with active ability? Spec: redirect to living
    // non-soldier/monk-protected. A second Mayor would bounce again in theory;
    // v1: treat active Mayor as unkillable for bounce candidates.
    candidates.retain(|id| {
        game.seats
            .iter()
            .find(|s| s.id == *id)
            .map(|s| !(s.true_character == Some(Character::Mayor) && !s.ability_disabled()))
            .unwrap_or(false)
    });
    candidates.sort_by_key(|id| id.0);

    if candidates.is_empty() {
        return KillResult::Survived;
    }

    let label = format!("mayor_bounce:c{}", game.night_cursor);
    let mut rng = game.rng.substream(&label);
    let bounce = *candidates.choose(&mut rng).expect("non-empty candidates");
    die_from_demon(game, bounce);
    KillResult::Died(bounce)
}

/// Mark dead, track night death, maybe insert Ravenkeeper wake.
fn die_from_demon(game: &mut Game, seat: SeatId) {
    // Snapshot ability/character before death mutations.
    let (is_rk, was_disabled, is_poisoner) = game
        .seats
        .iter()
        .find(|s| s.id == seat)
        .map(|s| {
            (
                s.true_character == Some(Character::Ravenkeeper),
                s.ability_disabled(),
                s.true_character == Some(Character::Poisoner),
            )
        })
        .unwrap_or((false, true, false));

    mark_dead(game, seat);

    if is_poisoner {
        clear_poisons(game);
    }

    // Ravenkeeper: true character, ability not disabled at death → insert wake.
    if is_rk && !was_disabled {
        insert_ravenkeeper_wake(game, seat);
    }

    win::win_check(game);
}

fn mark_dead(game: &mut Game, seat: SeatId) {
    if let Some(s) = game.seats.iter_mut().find(|x| x.id == seat) {
        if !s.alive {
            return;
        }
        s.alive = false;
        // Ghost vote available remains true until a yes vote is cast (day).
    }
    if !game.deaths_tonight.contains(&seat) {
        game.deaths_tonight.push(seat);
    }
}

/// Insert Ravenkeeper step immediately after the current night cursor step.
fn insert_ravenkeeper_wake(game: &mut Game, seat: SeatId) {
    // Avoid duplicate if queue already has this RK wake.
    let already = game
        .night_queue
        .iter()
        .any(|s| matches!(s, NightStep::Ravenkeeper { seat: rk } if *rk == seat));
    if already {
        return;
    }
    let insert_at = (game.night_cursor + 1).min(game.night_queue.len());
    game.night_queue
        .insert(insert_at, NightStep::Ravenkeeper { seat });
}
