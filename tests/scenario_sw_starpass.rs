//! Integration: Scarlet Woman conversion and Imp starpass, then eventual win.

mod common;

use botc_mcp::comms::PrivateMessage;
use botc_mcp::game::{
    DayStage, EndReason, NightActionPayload, Phase, RoleAssignment, SeatId, Winner,
};
use botc_mcp::roles::Character;
use botc_mcp::tools::{get_private_state, night_action, skip_night_action};

use common::{
    advance_to_imp_kill, assert_good_wins_demon_dead, execute_seat, finish_night, living_count,
    start_scripted, to_day1,
};

/// SW converts when Imp is executed with ≥5 alive; new Imp then dies → Good.
#[test]
fn scenario_sw_converts_then_new_imp_executed_good_wins() {
    // 5p: 3 TF + SW + Imp (legal bag shape; SW in place of Poisoner).
    let (mut g, host, tokens) = start_scripted(
        1302,
        vec![
            RoleAssignment::normal(SeatId(0), Character::Soldier),
            RoleAssignment::normal(SeatId(1), Character::Chef),
            RoleAssignment::normal(SeatId(2), Character::Empath),
            RoleAssignment::normal(SeatId(3), Character::ScarletWoman),
            RoleAssignment::normal(SeatId(4), Character::Imp),
        ],
    );

    to_day1(&mut g, &host);
    assert_eq!(living_count(&g), 5);

    // Execute original Imp → SW becomes Imp; game continues into N2.
    execute_seat(&mut g, &host, &tokens, SeatId(0), SeatId(4));

    assert!(!g.seats[4].alive);
    assert_eq!(g.seats[3].true_character, Some(Character::Imp));
    assert!(g.winner.is_none());
    assert!(matches!(g.phase, Phase::Night { night: 2, .. }));

    // Finish N2 with no deaths of consequence (skip wakes; default Imp kill via skip).
    finish_night(&mut g, &host);
    assert!(
        matches!(
            g.phase,
            Phase::Day {
                day: 2,
                stage: DayStage::Discussion
            }
        ),
        "expected Day 2, got {:?}",
        g.phase
    );

    // Living should still include new Imp (seat 3). Execute them → Good.
    assert!(g.seats[3].alive);
    assert_eq!(g.seats[3].true_character, Some(Character::Imp));
    execute_seat(&mut g, &host, &tokens, SeatId(0), SeatId(3));

    assert!(!g.seats[3].alive);
    assert_good_wins_demon_dead(&g);
}

/// Imp starpasses to living minion mid-game; new Imp is later executed → Good.
#[test]
fn scenario_starpass_then_execute_new_imp_good_wins() {
    let (mut g, host, tokens) = start_scripted(
        1303,
        vec![
            RoleAssignment::normal(SeatId(0), Character::Soldier),
            RoleAssignment::normal(SeatId(1), Character::Imp),
            RoleAssignment::normal(SeatId(2), Character::Poisoner),
            RoleAssignment::normal(SeatId(3), Character::Chef),
            RoleAssignment::normal(SeatId(4), Character::Empath),
        ],
    );

    to_day1(&mut g, &host);
    // Skip day 1 execution → night 2.
    botc_mcp::tools::open_nominations(&mut g, &host).unwrap();
    botc_mcp::tools::end_nominations(&mut g, &host).unwrap();
    assert!(matches!(g.phase, Phase::Night { night: 2, .. }));

    // Starpass: Imp kills self → host picks (or skips →) Poisoner becomes Imp.
    advance_to_imp_kill(&mut g, &host, &tokens, SeatId(0), None);
    night_action(
        &mut g,
        &tokens[1],
        NightActionPayload::PickOne { target: SeatId(1) },
    )
    .unwrap();
    assert!(g.pending_host.is_some());
    skip_night_action(&mut g, &host).unwrap();

    assert!(!g.seats[1].alive);
    assert_eq!(g.seats[2].true_character, Some(Character::Imp));
    assert!(g.seats[2].alive);
    assert!(g.winner.is_none());

    let priv_new = get_private_state(&g, &tokens[2], 0).unwrap();
    assert!(
        priv_new
            .private_messages_since
            .iter()
            .any(|(_, m)| matches!(
                m,
                PrivateMessage::YouAre {
                    character_label,
                    ..
                } if character_label == "Imp"
            )),
        "starpass recipient should learn they are Imp: {:?}",
        priv_new.private_messages_since
    );

    finish_night(&mut g, &host);
    assert!(
        matches!(
            g.phase,
            Phase::Day {
                day: 2,
                stage: DayStage::Discussion
            }
        ),
        "expected Day 2 after starpass night, got {:?}",
        g.phase
    );

    // Execute the new Imp → Good (no living minion left to convert).
    execute_seat(&mut g, &host, &tokens, SeatId(0), SeatId(2));
    assert!(!g.seats[2].alive);
    assert_good_wins_demon_dead(&g);
    assert!(matches!(
        g.phase,
        Phase::Ended {
            winner: Winner::Good,
            reason: EndReason::DemonDead
        }
    ));
}

/// SW converts on Imp execution; evil then wins via two-alive with living Imp.
#[test]
fn scenario_sw_converts_then_evil_two_alive() {
    let (mut g, host, tokens) = start_scripted(
        1304,
        vec![
            RoleAssignment::normal(SeatId(0), Character::Soldier),
            RoleAssignment::normal(SeatId(1), Character::Chef),
            RoleAssignment::normal(SeatId(2), Character::Empath),
            RoleAssignment::normal(SeatId(3), Character::ScarletWoman),
            RoleAssignment::normal(SeatId(4), Character::Imp),
        ],
    );

    to_day1(&mut g, &host);
    // Execute Imp → SW becomes Imp (5 alive before death).
    execute_seat(&mut g, &host, &tokens, SeatId(0), SeatId(4));
    assert_eq!(g.seats[3].true_character, Some(Character::Imp));
    assert!(matches!(g.phase, Phase::Night { night: 2, .. }));

    // N2: new Imp kills a townsfolk.
    advance_to_imp_kill(&mut g, &host, &tokens, SeatId(0), None);
    // pending is DemonKill for seat 3 now
    let imp_seat = g.pending_night.as_ref().unwrap().seat;
    assert_eq!(imp_seat, SeatId(3));
    night_action(
        &mut g,
        &tokens[3],
        NightActionPayload::PickOne { target: SeatId(1) }, // Chef
    )
    .unwrap();
    finish_night(&mut g, &host);
    assert!(!g.seats[1].alive);
    // Living: Soldier, Empath, Imp (ex-SW) = 3
    assert_eq!(living_count(&g), 3);

    // Day 2: execute Empath → 2 alive with Imp → Evil.
    execute_seat(&mut g, &host, &tokens, SeatId(0), SeatId(2));
    assert!(!g.seats[2].alive);
    assert!(g.seats[3].alive);
    assert_eq!(g.winner, Some(Winner::Evil));
    assert!(matches!(
        g.phase,
        Phase::Ended {
            winner: Winner::Evil,
            reason: EndReason::EvilTwoAlive
        }
    ));
}
