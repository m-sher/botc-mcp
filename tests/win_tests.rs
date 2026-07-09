//! Win conditions: DemonDead, SW, Saint, Mayor, EvilTwoAlive, Slayer (Task 11).

use botc_mcp::comms::PublicEvent;
use botc_mcp::game::{
    DayStage, EndReason, Game, NightActionPayload, NightStep, Phase, RoleAssignment, SeatId,
    StartOpts, Winner,
};
use botc_mcp::roles::Character;
use botc_mcp::tools::{
    day_action, end_nominations, night_action, nominate, open_nominations, skip_night_action, vote,
    DayActionPayload,
};

fn names(n: usize) -> Vec<String> {
    (0..n).map(|i| format!("P{i}")).collect()
}

fn five_names() -> Vec<String> {
    names(5)
}

/// First-night skip may poison a seat; clear for ability-dependent day tests.
fn clear_all_poisons(g: &mut Game) {
    for s in &mut g.seats {
        s.poisoned = false;
    }
}

/// Finish first night into Day 1 Discussion.
fn to_day1(g: &mut Game, host: &botc_mcp::auth::Token) {
    while g.pending_night.is_some() || g.pending_host.is_some() {
        skip_night_action(g, host).unwrap();
    }
    assert!(
        matches!(
            g.phase,
            Phase::Day {
                day: 1,
                stage: DayStage::Discussion
            }
        ),
        "expected Day 1 Discussion, got {:?}",
        g.phase
    );
}

/// All living cast `support` on the open nomination (auto-closes when all have voted).
fn all_vote(g: &mut Game, tokens: &[botc_mcp::auth::Token], nominee: SeatId, support: bool) {
    let living: Vec<usize> = g
        .seats
        .iter()
        .filter(|s| s.alive)
        .map(|s| s.id.0 as usize)
        .collect();
    for i in living {
        vote(g, &tokens[i], nominee, support).unwrap();
    }
}

#[test]
fn starpass_does_not_end_game_when_minion_becomes_imp() {
    let lobby = Game::create(five_names(), 5).unwrap();
    let host = lobby.host_token.clone();
    let tokens = lobby.player_tokens.clone();
    let mut g = lobby.game;
    g.start_game(
        &host,
        StartOpts {
            assignments: Some(vec![
                RoleAssignment::normal(SeatId(0), Character::Soldier),
                RoleAssignment::normal(SeatId(1), Character::Imp),
                RoleAssignment::normal(SeatId(2), Character::Poisoner),
                RoleAssignment::normal(SeatId(3), Character::Chef),
                RoleAssignment::normal(SeatId(4), Character::Empath),
            ]),
                ..Default::default()
            },
    )
    .unwrap();
    while g.pending_night.is_some() || g.pending_host.is_some() {
        skip_night_action(&mut g, &host).unwrap();
    }
    g.enter_night(2);
    night_action(
        &mut g,
        &tokens[2],
        NightActionPayload::PickOne { target: SeatId(0) },
    )
    .unwrap();
    let p = g.pending_night.as_ref().unwrap();
    assert!(matches!(p.step, NightStep::DemonKill { .. }));
    night_action(
        &mut g,
        &tokens[1],
        NightActionPayload::PickOne { target: SeatId(1) },
    )
    .unwrap();
    // Starpass requires host pick (or skip → random among minions).
    assert!(g.pending_host.is_some());
    skip_night_action(&mut g, &host).unwrap();
    assert!(g.winner.is_none());
    assert!(!matches!(g.phase, Phase::Ended { .. }));
    assert_eq!(g.seats[2].true_character, Some(Character::Imp));
}

#[test]
fn imp_death_with_no_living_minion_good_wins() {
    let lobby = Game::create(five_names(), 5).unwrap();
    let host = lobby.host_token.clone();
    let tokens = lobby.player_tokens.clone();
    let mut g = lobby.game;
    g.start_game(
        &host,
        StartOpts {
            assignments: Some(vec![
                RoleAssignment::normal(SeatId(0), Character::Soldier),
                RoleAssignment::normal(SeatId(1), Character::Imp),
                RoleAssignment::normal(SeatId(2), Character::Poisoner),
                RoleAssignment::normal(SeatId(3), Character::Chef),
                RoleAssignment::normal(SeatId(4), Character::Empath),
            ]),
                ..Default::default()
            },
    )
    .unwrap();
    while g.pending_night.is_some() || g.pending_host.is_some() {
        skip_night_action(&mut g, &host).unwrap();
    }
    g.seats[2].alive = false;
    g.enter_night(2);
    let p = g.pending_night.as_ref().expect("imp");
    assert!(matches!(p.step, NightStep::DemonKill { .. }));
    night_action(
        &mut g,
        &tokens[1],
        NightActionPayload::PickOne { target: SeatId(1) },
    )
    .unwrap();
    assert_eq!(g.winner, Some(Winner::Good));
    assert!(matches!(
        g.phase,
        Phase::Ended {
            winner: Winner::Good,
            reason: EndReason::DemonDead
        }
    ));
}

#[test]
fn execute_imp_good_wins() {
    let lobby = Game::create(five_names(), 20).unwrap();
    let host = lobby.host_token.clone();
    let tokens = lobby.player_tokens.clone();
    let mut g = lobby.game;
    g.start_game(
        &host,
        StartOpts {
            assignments: Some(vec![
                RoleAssignment::normal(SeatId(0), Character::Soldier),
                RoleAssignment::normal(SeatId(1), Character::Chef),
                RoleAssignment::normal(SeatId(2), Character::Empath),
                RoleAssignment::normal(SeatId(3), Character::Poisoner),
                RoleAssignment::normal(SeatId(4), Character::Imp),
            ]),
                ..Default::default()
            },
    )
    .unwrap();
    to_day1(&mut g, &host);
    open_nominations(&mut g, &host).unwrap();
    nominate(&mut g, &tokens[0], SeatId(4)).unwrap();
    all_vote(&mut g, &tokens, SeatId(4), true);
    end_nominations(&mut g, &host).unwrap();

    assert!(!g.seats[4].alive);
    assert_eq!(g.winner, Some(Winner::Good));
    assert!(matches!(
        g.phase,
        Phase::Ended {
            winner: Winner::Good,
            reason: EndReason::DemonDead
        }
    ));
}

#[test]
fn sw_converts_when_alive_before_ge_5() {
    // 5 alive including Imp; execute Imp → SW becomes Imp, game continues.
    let lobby = Game::create(five_names(), 21).unwrap();
    let host = lobby.host_token.clone();
    let tokens = lobby.player_tokens.clone();
    let mut g = lobby.game;
    g.start_game(
        &host,
        StartOpts {
            assignments: Some(vec![
                RoleAssignment::normal(SeatId(0), Character::Soldier),
                RoleAssignment::normal(SeatId(1), Character::Chef),
                RoleAssignment::normal(SeatId(2), Character::Empath),
                RoleAssignment::normal(SeatId(3), Character::ScarletWoman),
                RoleAssignment::normal(SeatId(4), Character::Imp),
            ]),
                ..Default::default()
            },
    )
    .unwrap();
    to_day1(&mut g, &host);
    assert_eq!(g.seats.iter().filter(|s| s.alive).count(), 5);
    open_nominations(&mut g, &host).unwrap();
    nominate(&mut g, &tokens[0], SeatId(4)).unwrap();
    all_vote(&mut g, &tokens, SeatId(4), true);
    end_nominations(&mut g, &host).unwrap();

    assert!(!g.seats[4].alive);
    assert_eq!(g.seats[3].true_character, Some(Character::Imp));
    assert!(g.winner.is_none());
    assert!(matches!(g.phase, Phase::Night { night: 2, .. }));
}

#[test]
fn sw_no_convert_at_4_alive_before() {
    // 4 alive including Imp; SW does not convert → Good.
    let lobby = Game::create(five_names(), 22).unwrap();
    let host = lobby.host_token.clone();
    let tokens = lobby.player_tokens.clone();
    let mut g = lobby.game;
    g.start_game(
        &host,
        StartOpts {
            assignments: Some(vec![
                RoleAssignment::normal(SeatId(0), Character::Soldier),
                RoleAssignment::normal(SeatId(1), Character::Chef),
                RoleAssignment::normal(SeatId(2), Character::Empath),
                RoleAssignment::normal(SeatId(3), Character::ScarletWoman),
                RoleAssignment::normal(SeatId(4), Character::Imp),
            ]),
                ..Default::default()
            },
    )
    .unwrap();
    to_day1(&mut g, &host);
    g.seats[0].alive = false; // 4 living left
    open_nominations(&mut g, &host).unwrap();
    nominate(&mut g, &tokens[1], SeatId(4)).unwrap();
    all_vote(&mut g, &tokens, SeatId(4), true);
    end_nominations(&mut g, &host).unwrap();

    assert!(!g.seats[4].alive);
    assert_eq!(g.seats[3].true_character, Some(Character::ScarletWoman));
    assert_eq!(g.winner, Some(Winner::Good));
    assert!(matches!(
        g.phase,
        Phase::Ended {
            winner: Winner::Good,
            reason: EndReason::DemonDead
        }
    ));
}

#[test]
fn saint_executed_evil_wins() {
    let lobby = Game::create(five_names(), 23).unwrap();
    let host = lobby.host_token.clone();
    let tokens = lobby.player_tokens.clone();
    let mut g = lobby.game;
    g.start_game(
        &host,
        StartOpts {
            assignments: Some(vec![
                RoleAssignment::normal(SeatId(0), Character::Soldier),
                RoleAssignment::normal(SeatId(1), Character::Chef),
                RoleAssignment::normal(SeatId(2), Character::Saint),
                RoleAssignment::normal(SeatId(3), Character::Poisoner),
                RoleAssignment::normal(SeatId(4), Character::Imp),
            ]),
                ..Default::default()
            },
    )
    .unwrap();
    to_day1(&mut g, &host);
    open_nominations(&mut g, &host).unwrap();
    nominate(&mut g, &tokens[0], SeatId(2)).unwrap();
    all_vote(&mut g, &tokens, SeatId(2), true);
    end_nominations(&mut g, &host).unwrap();

    assert!(!g.seats[2].alive);
    assert_eq!(g.winner, Some(Winner::Evil));
    assert!(matches!(
        g.phase,
        Phase::Ended {
            winner: Winner::Evil,
            reason: EndReason::SaintExecuted
        }
    ));
}

#[test]
fn mayor_three_no_exec_good_wins() {
    let lobby = Game::create(five_names(), 24).unwrap();
    let host = lobby.host_token.clone();
    let mut g = lobby.game;
    g.start_game(
        &host,
        StartOpts {
            assignments: Some(vec![
                RoleAssignment::normal(SeatId(0), Character::Mayor),
                RoleAssignment::normal(SeatId(1), Character::Chef),
                RoleAssignment::normal(SeatId(2), Character::Soldier),
                RoleAssignment::normal(SeatId(3), Character::Poisoner),
                RoleAssignment::normal(SeatId(4), Character::Imp),
            ]),
                ..Default::default()
            },
    )
    .unwrap();
    to_day1(&mut g, &host);
    // Kill two so three remain (Mayor + one townsfolk + Imp).
    g.seats[1].alive = false;
    g.seats[3].alive = false;
    assert_eq!(g.seats.iter().filter(|s| s.alive).count(), 3);

    open_nominations(&mut g, &host).unwrap();
    // No nomination → no execution.
    clear_all_poisons(&mut g);
    end_nominations(&mut g, &host).unwrap();

    assert!(g.public_log.since(0).iter().any(|(_, e)| matches!(e, PublicEvent::NoExecution)));
    assert_eq!(g.winner, Some(Winner::Good));
    assert!(matches!(
        g.phase,
        Phase::Ended {
            winner: Winner::Good,
            reason: EndReason::MayorThreeNoExec
        }
    ));
}

#[test]
fn two_living_with_imp_evil_wins() {
    let lobby = Game::create(five_names(), 25).unwrap();
    let host = lobby.host_token.clone();
    let tokens = lobby.player_tokens.clone();
    let mut g = lobby.game;
    g.start_game(
        &host,
        StartOpts {
            assignments: Some(vec![
                RoleAssignment::normal(SeatId(0), Character::Soldier),
                RoleAssignment::normal(SeatId(1), Character::Chef),
                RoleAssignment::normal(SeatId(2), Character::Empath),
                RoleAssignment::normal(SeatId(3), Character::Poisoner),
                RoleAssignment::normal(SeatId(4), Character::Imp),
            ]),
                ..Default::default()
            },
    )
    .unwrap();
    to_day1(&mut g, &host);
    // Leave Soldier + Imp (+ others dead). Execute a townsfolk while 3 alive → 2 left with Imp.
    g.seats[1].alive = false;
    g.seats[2].alive = false;
    assert_eq!(g.seats.iter().filter(|s| s.alive).count(), 3);

    open_nominations(&mut g, &host).unwrap();
    // Execute the Poisoner (not Imp) → 2 living with Imp → Evil.
    nominate(&mut g, &tokens[0], SeatId(3)).unwrap();
    all_vote(&mut g, &tokens, SeatId(3), true);
    end_nominations(&mut g, &host).unwrap();

    assert!(!g.seats[3].alive);
    assert!(g.seats[4].alive);
    assert_eq!(g.winner, Some(Winner::Evil));
    assert!(matches!(
        g.phase,
        Phase::Ended {
            winner: Winner::Evil,
            reason: EndReason::EvilTwoAlive
        }
    ));
}

#[test]
fn simultaneous_imp_death_and_two_alive_good_wins() {
    // Execute Imp leaving exactly 2 living → Good (DemonDead before EvilTwoAlive).
    let lobby = Game::create(five_names(), 26).unwrap();
    let host = lobby.host_token.clone();
    let tokens = lobby.player_tokens.clone();
    let mut g = lobby.game;
    g.start_game(
        &host,
        StartOpts {
            assignments: Some(vec![
                RoleAssignment::normal(SeatId(0), Character::Soldier),
                RoleAssignment::normal(SeatId(1), Character::Chef),
                RoleAssignment::normal(SeatId(2), Character::Empath),
                RoleAssignment::normal(SeatId(3), Character::Poisoner),
                RoleAssignment::normal(SeatId(4), Character::Imp),
            ]),
                ..Default::default()
            },
    )
    .unwrap();
    to_day1(&mut g, &host);
    g.seats[1].alive = false;
    g.seats[2].alive = false;
    // Living: 0 Soldier, 3 Poisoner, 4 Imp (3 alive; execute Imp → 2 left).
    open_nominations(&mut g, &host).unwrap();
    nominate(&mut g, &tokens[0], SeatId(4)).unwrap();
    all_vote(&mut g, &tokens, SeatId(4), true);
    end_nominations(&mut g, &host).unwrap();

    assert_eq!(g.winner, Some(Winner::Good));
    assert!(matches!(
        g.phase,
        Phase::Ended {
            winner: Winner::Good,
            reason: EndReason::DemonDead
        }
    ));
}

#[test]
fn slayer_hits_imp() {
    let lobby = Game::create(five_names(), 27).unwrap();
    let host = lobby.host_token.clone();
    let tokens = lobby.player_tokens.clone();
    let mut g = lobby.game;
    g.start_game(
        &host,
        StartOpts {
            assignments: Some(vec![
                RoleAssignment::normal(SeatId(0), Character::Slayer),
                RoleAssignment::normal(SeatId(1), Character::Chef),
                RoleAssignment::normal(SeatId(2), Character::Empath),
                RoleAssignment::normal(SeatId(3), Character::Poisoner),
                RoleAssignment::normal(SeatId(4), Character::Imp),
            ]),
                ..Default::default()
            },
    )
    .unwrap();
    to_day1(&mut g, &host);
    clear_all_poisons(&mut g);

    day_action(
        &mut g,
        &tokens[0],
        DayActionPayload::Slay { target: SeatId(4) },
    )
    .unwrap();

    assert!(g.seats[0].slayer_used);
    assert!(!g.seats[4].alive);
    assert_eq!(g.winner, Some(Winner::Good));
    assert!(matches!(
        g.phase,
        Phase::Ended {
            winner: Winner::Good,
            reason: EndReason::DemonDead
        }
    ));
    assert!(g
        .public_log
        .since(0)
        .iter()
        .any(|(_, e)| matches!(e, PublicEvent::PlayerDied { seat: SeatId(4) })));
}

#[test]
fn slayer_miss_spends_and_continues() {
    let lobby = Game::create(five_names(), 28).unwrap();
    let host = lobby.host_token.clone();
    let tokens = lobby.player_tokens.clone();
    let mut g = lobby.game;
    g.start_game(
        &host,
        StartOpts {
            assignments: Some(vec![
                RoleAssignment::normal(SeatId(0), Character::Slayer),
                RoleAssignment::normal(SeatId(1), Character::Chef),
                RoleAssignment::normal(SeatId(2), Character::Empath),
                RoleAssignment::normal(SeatId(3), Character::Poisoner),
                RoleAssignment::normal(SeatId(4), Character::Imp),
            ]),
                ..Default::default()
            },
    )
    .unwrap();
    to_day1(&mut g, &host);

    day_action(
        &mut g,
        &tokens[0],
        DayActionPayload::Slay { target: SeatId(3) },
    )
    .unwrap();

    assert!(g.seats[0].slayer_used);
    assert!(g.seats[3].alive);
    assert!(g.seats[4].alive);
    assert!(g.winner.is_none());
    // Second attempt illegal.
    let err = day_action(
        &mut g,
        &tokens[0],
        DayActionPayload::Slay { target: SeatId(4) },
    );
    assert!(err.is_err());
}
