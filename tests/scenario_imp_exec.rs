//! Integration: 5p scripted start → execute Imp on day 1 → Good wins.
//!
//! Legal 5p composition: 3 Townsfolk + 0 Outsiders + 1 Minion + 1 Demon.
//! (Poisoner minion, not SW — matches bag counts.)

mod common;

use botc_mcp::comms::PublicEvent;
use botc_mcp::game::{EndReason, Phase, RoleAssignment, SeatId, Winner};
use botc_mcp::roles::Character;
use botc_mcp::tools::{get_public_log, get_public_state};

use common::{assert_good_wins_demon_dead, execute_seat, start_scripted, to_day1};

#[test]
fn scenario_5p_execute_imp_day1_good_wins() {
    // Seats: Soldier, Empath, Monk, Poisoner, Imp
    let (mut g, host, tokens) = start_scripted(
        1301,
        vec![
            RoleAssignment::normal(SeatId(0), Character::Soldier),
            RoleAssignment::normal(SeatId(1), Character::Empath),
            RoleAssignment::normal(SeatId(2), Character::Monk),
            RoleAssignment::normal(SeatId(3), Character::Poisoner),
            RoleAssignment::normal(SeatId(4), Character::Imp),
        ],
    );

    to_day1(&mut g, &host);

    let pub_state = get_public_state(&g, &tokens[0]).expect("public state");
    assert!(pub_state.phase.contains("Day"));
    assert!(pub_state.winner.is_none());

    // Good nominate + execute the Imp.
    execute_seat(&mut g, &host, &tokens, SeatId(0), SeatId(4));

    assert!(!g.seats[4].alive);
    assert_good_wins_demon_dead(&g);
    assert!(matches!(
        g.phase,
        Phase::Ended {
            winner: Winner::Good,
            reason: EndReason::DemonDead
        }
    ));

    let log = get_public_log(&g, &host, 0).expect("public log");
    assert!(
        log.iter()
            .any(|(_, e)| matches!(e, PublicEvent::Executed { seat: SeatId(4) })),
        "expected Executed Imp seat: {log:?}"
    );
}
