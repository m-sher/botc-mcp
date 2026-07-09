//! Night queue construction (wake list only; machine in later tasks).

use crate::game::ids::SeatId;
use crate::game::phase::NightStep;
use crate::game::state::Game;
use crate::roles::night_order::{
    FirstNightSlot, OtherNightSlot, FIRST_NIGHT_CHARACTER_ORDER, OTHER_NIGHT_CHARACTER_ORDER,
};
use crate::roles::Character;

/// Build the ordered first-night step list for the current grimoire (spec §9.1).
pub fn build_first_night_queue(game: &Game) -> Vec<NightStep> {
    let mut q = Vec::new();
    q.push(NightStep::SetupMarkers);

    let n = game.seats.len();
    if n >= 7 {
        if has_living_minion(game) {
            q.push(NightStep::MinionBriefing);
        }
        q.push(NightStep::DemonBriefing);
    }

    for slot in FIRST_NIGHT_CHARACTER_ORDER {
        if let Some(seat) = find_wake_seat(game, slot.character(), slot.uses_true_character()) {
            q.push(first_night_step(*slot, seat));
        }
    }

    q.push(NightStep::Dawn);
    q
}

/// Build the ordered other-night step list (no N1 setup/briefings; includes Imp kill).
///
/// Ravenkeeper is included only when a seat in `deaths_tonight` faces as Ravenkeeper
/// (typically filled after the demon kill resolves). Undertaker is included when the
/// seat is alive **and** there was an execution today (`executed_today`).
pub fn build_other_night_queue(game: &Game) -> Vec<NightStep> {
    let mut q = Vec::new();

    for slot in OTHER_NIGHT_CHARACTER_ORDER {
        match slot {
            OtherNightSlot::Ravenkeeper => {
                // Spec: true Ravenkeeper who died to the demon (Drunk face is not true RK).
                for &dead in &game.deaths_tonight {
                    if seat_matches_wake(game, dead, Character::Ravenkeeper, true) {
                        q.push(NightStep::Ravenkeeper { seat: dead });
                    }
                }
            }
            OtherNightSlot::Undertaker => {
                if game.executed_today.is_some() {
                    if let Some(seat) =
                        find_wake_seat(game, Character::Undertaker, false /* face */)
                    {
                        q.push(NightStep::Undertaker { seat });
                    }
                }
            }
            OtherNightSlot::Imp => {
                if let Some(seat) = find_wake_seat(game, Character::Imp, true) {
                    q.push(NightStep::DemonKill { seat });
                }
            }
            other => {
                let role = other.character();
                if let Some(seat) = find_wake_seat(game, role, other.uses_true_character()) {
                    q.push(other_night_step(*other, seat));
                }
            }
        }
    }

    q.push(NightStep::Dawn);
    q
}

fn first_night_step(slot: FirstNightSlot, seat: SeatId) -> NightStep {
    use FirstNightSlot::*;
    match slot {
        Poisoner => NightStep::Poisoner { seat },
        Spy => NightStep::Spy { seat },
        Washerwoman => NightStep::Washerwoman { seat },
        Librarian => NightStep::Librarian { seat },
        Investigator => NightStep::Investigator { seat },
        Chef => NightStep::Chef { seat },
        Empath => NightStep::Empath { seat },
        FortuneTeller => NightStep::FortuneTeller { seat },
        Butler => NightStep::Butler { seat },
    }
}

fn other_night_step(slot: OtherNightSlot, seat: SeatId) -> NightStep {
    use OtherNightSlot::*;
    match slot {
        Poisoner => NightStep::Poisoner { seat },
        Monk => NightStep::Monk { seat },
        Spy => NightStep::Spy { seat },
        Imp => NightStep::DemonKill { seat },
        Ravenkeeper => NightStep::Ravenkeeper { seat },
        Undertaker => NightStep::Undertaker { seat },
        Empath => NightStep::Empath { seat },
        FortuneTeller => NightStep::FortuneTeller { seat },
        Butler => NightStep::Butler { seat },
    }
}

fn has_living_minion(game: &Game) -> bool {
    game.seats.iter().any(|s| {
        s.alive
            && s.true_character
                .is_some_and(|c| c.character_type() == crate::roles::CharacterType::Minion)
    })
}

/// First living seat matching role via true character or player-facing character.
fn find_wake_seat(game: &Game, role: Character, use_true: bool) -> Option<SeatId> {
    game.seats.iter().find_map(|s| {
        if !s.alive {
            return None;
        }
        if seat_matches_wake(game, s.id, role, use_true) {
            Some(s.id)
        } else {
            None
        }
    })
}

fn seat_matches_wake(game: &Game, seat: SeatId, role: Character, use_true: bool) -> bool {
    let Some(s) = game.seats.iter().find(|x| x.id == seat) else {
        return false;
    };
    if use_true {
        s.true_character == Some(role)
    } else {
        game.player_facing_character(seat) == Some(role)
    }
}

#[cfg(test)]
mod unit_tests {
    use super::*;
    use crate::game::{RoleAssignment, StartOpts};
    use crate::roles::Character;

    fn tiny_game() -> Game {
        let names = vec![
            "A".into(),
            "B".into(),
            "C".into(),
            "D".into(),
            "E".into(),
        ];
        let lobby = Game::create(names, 1).unwrap();
        let host = lobby.host_token.clone();
        let mut g = lobby.game;
        g.start_game(
            &host,
            StartOpts {
                assignments: Some(vec![
                    RoleAssignment::drunk(SeatId(0), Character::FortuneTeller).unwrap(),
                    RoleAssignment::normal(SeatId(1), Character::Imp),
                    RoleAssignment::normal(SeatId(2), Character::Poisoner),
                    RoleAssignment::normal(SeatId(3), Character::Butler),
                    RoleAssignment::normal(SeatId(4), Character::Spy),
                ]),
            },
        )
        .unwrap();
        g
    }

    #[test]
    fn drunk_fortune_teller_face_wakes_as_ft() {
        let g = tiny_game();
        let q = build_first_night_queue(&g);
        assert!(q
            .iter()
            .any(|s| matches!(s, NightStep::FortuneTeller { seat: SeatId(0) })));
        assert!(q
            .iter()
            .any(|s| matches!(s, NightStep::Butler { seat: SeatId(3) })));
        assert!(q
            .iter()
            .any(|s| matches!(s, NightStep::Spy { seat: SeatId(4) })));
    }
}
