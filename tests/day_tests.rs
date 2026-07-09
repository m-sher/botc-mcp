//! Day nominations / voting / Virgin / Butler / ghost vote (Task 10).

use botc_mcp::comms::PublicEvent;
use botc_mcp::game::{
    meets_threshold, DayStage, Game, Phase, RoleAssignment, SeatId, StartOpts, Winner,
};
use botc_mcp::roles::Character;
use botc_mcp::game::NightActionPayload;
use botc_mcp::tools::{
    close_vote, end_nominations, nominate, night_action, open_nominations, pass_vote,
    skip_night_action, vote,
};

fn names(n: usize) -> Vec<String> {
    (0..n).map(|i| format!("P{i}")).collect()
}

/// Finish first night (skip pending wakes) into Day 1 Discussion.
fn to_day1(g: &mut Game, host: &botc_mcp::auth::Token) {
    while g.pending_night.is_some() {
        skip_night_action(g, host).unwrap();
    }
    assert!(
        matches!(g.phase, Phase::Day { day: 1, stage: DayStage::Discussion }),
        "expected Day 1 Discussion, got {:?}",
        g.phase
    );
}

#[test]
fn vote_threshold_six_living_needs_three() {
    assert!(!meets_threshold(2, 6));
    assert!(meets_threshold(3, 6));

    let lobby = Game::create(names(6), 10).unwrap();
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
                RoleAssignment::normal(SeatId(3), Character::Butler),
                RoleAssignment::normal(SeatId(4), Character::Poisoner),
                RoleAssignment::normal(SeatId(5), Character::Imp),
            ]),
        },
    )
    .unwrap();
    to_day1(&mut g, &host);

    open_nominations(&mut g, &host).unwrap();
    // Nominate seat 5 (Imp) by seat 0
    nominate(&mut g, &tokens[0], SeatId(5)).unwrap();
    // Two yes from living → not enough; close and check no leader path
    vote(&mut g, &tokens[0], SeatId(5), true).unwrap();
    vote(&mut g, &tokens[1], SeatId(5), true).unwrap();
    // Others no
    for i in 2..6 {
        vote(&mut g, &tokens[i], SeatId(5), false).unwrap();
    }
    // Auto-closed when all living voted
    assert!(g.current_nomination.is_none());
    assert_eq!(g.closed_nominations.len(), 1);
    assert_eq!(g.closed_nominations[0].yes_votes, 2);
    assert!(!meets_threshold(2, 6));

    // New nom with 3 yes should pass threshold
    nominate(&mut g, &tokens[1], SeatId(4)).unwrap();
    vote(&mut g, &tokens[0], SeatId(4), true).unwrap();
    vote(&mut g, &tokens[1], SeatId(4), true).unwrap();
    vote(&mut g, &tokens[2], SeatId(4), true).unwrap();
    for i in 3..6 {
        vote(&mut g, &tokens[i], SeatId(4), false).unwrap();
    }
    assert_eq!(g.closed_nominations.last().unwrap().yes_votes, 3);
    assert!(meets_threshold(3, 6));

    end_nominations(&mut g, &host).unwrap();
    // Seat 4 (Poisoner minion) executed — Imp still alive
    assert!(!g.seats[4].alive);
    assert_eq!(g.executed_today, Some(SeatId(4)));
    // Progressed to night
    assert!(matches!(g.phase, Phase::Night { night: 2, .. }));
}

#[test]
fn virgin_kills_townsfolk_nominator() {
    let lobby = Game::create(names(5), 11).unwrap();
    let host = lobby.host_token.clone();
    let tokens = lobby.player_tokens.clone();
    let mut g = lobby.game;
    g.start_game(
        &host,
        StartOpts {
            assignments: Some(vec![
                RoleAssignment::normal(SeatId(0), Character::Soldier), // townsfolk nominator
                RoleAssignment::normal(SeatId(1), Character::Virgin),
                RoleAssignment::normal(SeatId(2), Character::Chef),
                RoleAssignment::normal(SeatId(3), Character::Poisoner),
                RoleAssignment::normal(SeatId(4), Character::Imp),
            ]),
        },
    )
    .unwrap();
    to_day1(&mut g, &host);
    open_nominations(&mut g, &host).unwrap();

    nominate(&mut g, &tokens[0], SeatId(1)).unwrap();

    // Nominator executed immediately; no open vote
    assert!(!g.seats[0].alive);
    assert!(g.seats[1].alive);
    assert!(g.seats[1].virgin_ability_used);
    assert_eq!(g.executed_today, Some(SeatId(0)));
    assert!(g.current_nomination.is_none());
    assert!(g.public_log.since(0).iter().any(|(_, e)| matches!(
        e,
        PublicEvent::Executed { seat: SeatId(0) }
    )));
}

#[test]
fn drunk_nominator_does_not_trigger_virgin() {
    let lobby = Game::create(names(5), 12).unwrap();
    let host = lobby.host_token.clone();
    let tokens = lobby.player_tokens.clone();
    let mut g = lobby.game;
    g.start_game(
        &host,
        StartOpts {
            assignments: Some(vec![
                // Drunk faces as Soldier (Townsfolk label) but is Outsider
                RoleAssignment::drunk(SeatId(0), Character::Soldier).unwrap(),
                RoleAssignment::normal(SeatId(1), Character::Virgin),
                RoleAssignment::normal(SeatId(2), Character::Chef),
                RoleAssignment::normal(SeatId(3), Character::Poisoner),
                RoleAssignment::normal(SeatId(4), Character::Imp),
            ]),
        },
    )
    .unwrap();
    to_day1(&mut g, &host);
    open_nominations(&mut g, &host).unwrap();

    nominate(&mut g, &tokens[0], SeatId(1)).unwrap();

    // Virgin spent, but Drunk nominator lives; vote opens
    assert!(g.seats[0].alive);
    assert!(g.seats[1].virgin_ability_used);
    assert!(g.current_nomination.is_some());
    assert_eq!(g.executed_today, None);
}

#[test]
fn ghost_yes_only_once() {
    let lobby = Game::create(names(5), 13).unwrap();
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
        },
    )
    .unwrap();
    to_day1(&mut g, &host);

    // Kill seat 2 during day setup (simulate prior death)
    g.seats[2].alive = false;
    g.seats[2].ghost_vote_available = true;

    open_nominations(&mut g, &host).unwrap();
    nominate(&mut g, &tokens[0], SeatId(4)).unwrap();

    // Ghost yes spends token
    vote(&mut g, &tokens[2], SeatId(4), true).unwrap();
    assert!(!g.seats[2].ghost_vote_available);

    // Close first nom manually (not all living voted yet)
    close_vote(&mut g, &host).unwrap();

    // Second nomination — spent ghost rejects yes and no
    nominate(&mut g, &tokens[1], SeatId(3)).unwrap();
    let err = vote(&mut g, &tokens[2], SeatId(3), true).unwrap_err();
    assert!(
        format!("{err:?}").to_lowercase().contains("ghost")
            || format!("{err}").to_lowercase().contains("ghost"),
        "expected ghost vote error, got {err:?}"
    );
    let err_no = vote(&mut g, &tokens[2], SeatId(3), false).unwrap_err();
    assert!(
        format!("{err_no:?}").to_lowercase().contains("ghost")
            || format!("{err_no}").to_lowercase().contains("ghost"),
        "spent ghost must reject no votes too, got {err_no:?}"
    );
}

#[test]
fn butler_yes_requires_master_yes() {
    let lobby = Game::create(names(5), 14).unwrap();
    let host = lobby.host_token.clone();
    let tokens = lobby.player_tokens.clone();
    let mut g = lobby.game;
    g.start_game(
        &host,
        StartOpts {
            assignments: Some(vec![
                RoleAssignment::normal(SeatId(0), Character::Butler),
                RoleAssignment::normal(SeatId(1), Character::Soldier), // master
                RoleAssignment::normal(SeatId(2), Character::Chef),
                RoleAssignment::normal(SeatId(3), Character::Poisoner),
                RoleAssignment::normal(SeatId(4), Character::Imp),
            ]),
        },
    )
    .unwrap();
    to_day1(&mut g, &host);
    // N1 Poisoner skip may poison seat 0; clear so Butler ability is active.
    g.seats[0].poisoned = false;
    g.seats[0].butler_master = Some(SeatId(1));

    open_nominations(&mut g, &host).unwrap();
    nominate(&mut g, &tokens[2], SeatId(4)).unwrap();

    // Butler yes before master → reject
    let err = vote(&mut g, &tokens[0], SeatId(4), true).unwrap_err();
    assert!(
        format!("{err}").to_lowercase().contains("butler")
            || format!("{err:?}").to_lowercase().contains("butler"),
        "expected butler error, got {err:?}"
    );

    // Master yes, then Butler yes ok
    vote(&mut g, &tokens[1], SeatId(4), true).unwrap();
    vote(&mut g, &tokens[0], SeatId(4), true).unwrap();
}

#[test]
fn host_close_vote_without_all_living() {
    let lobby = Game::create(names(5), 15).unwrap();
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
        },
    )
    .unwrap();
    to_day1(&mut g, &host);
    open_nominations(&mut g, &host).unwrap();
    nominate(&mut g, &tokens[0], SeatId(3)).unwrap();
    vote(&mut g, &tokens[0], SeatId(3), true).unwrap();
    // Host closes early — missing living votes count as no
    close_vote(&mut g, &host).unwrap();
    assert!(g.current_nomination.is_none());
    assert_eq!(g.closed_nominations[0].yes_votes, 1);
    // 1*2 >= 5? false → no execution on end
    end_nominations(&mut g, &host).unwrap();
    assert!(g.seats.iter().all(|s| s.alive) || g.winner == Some(Winner::Good));
    // Without execution of imp, night should start with all still alive
    assert!(matches!(g.phase, Phase::Night { night: 2, .. }));
    assert!(g.public_log.since(0).iter().any(|(_, e)| matches!(e, PublicEvent::NoExecution)));
}

#[test]
fn poisoner_executed_clears_active_poison() {
    let lobby = Game::create(names(5), 17).unwrap();
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
        },
    )
    .unwrap();
    // N1: Poisoner poisons Soldier (seat 0).
    let p = g.pending_night.as_ref().expect("Poisoner pending");
    assert_eq!(p.seat, SeatId(3));
    night_action(
        &mut g,
        &tokens[3],
        NightActionPayload::PickOne { target: SeatId(0) },
    )
    .unwrap();
    assert!(g.seats[0].poisoned, "Soldier should be poisoned overnight");
    to_day1(&mut g, &host);
    assert!(g.seats[0].poisoned, "poison lasts through the day");

    open_nominations(&mut g, &host).unwrap();
    nominate(&mut g, &tokens[0], SeatId(3)).unwrap();
    // 5 living → need 3 yes (yes*2 >= living).
    vote(&mut g, &tokens[0], SeatId(3), true).unwrap();
    vote(&mut g, &tokens[1], SeatId(3), true).unwrap();
    vote(&mut g, &tokens[2], SeatId(3), true).unwrap();
    vote(&mut g, &tokens[3], SeatId(3), false).unwrap();
    vote(&mut g, &tokens[4], SeatId(3), false).unwrap();
    end_nominations(&mut g, &host).unwrap();

    assert!(!g.seats[3].alive, "Poisoner must be executed");
    assert!(
        !g.seats[0].poisoned,
        "poison must clear when Poisoner dies by execution"
    );
}

#[test]
fn dead_pass_vote_keeps_ghost_and_allows_auto_close() {
    let lobby = Game::create(names(5), 18).unwrap();
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
        },
    )
    .unwrap();
    to_day1(&mut g, &host);
    g.seats[2].alive = false;
    g.seats[2].ghost_vote_available = true;

    open_nominations(&mut g, &host).unwrap();
    nominate(&mut g, &tokens[0], SeatId(4)).unwrap();

    // Living cannot pass (before they vote).
    let err = pass_vote(&mut g, &tokens[1]).unwrap_err();
    assert!(
        format!("{err}").to_lowercase().contains("dead")
            || format!("{err:?}").to_lowercase().contains("dead"),
        "expected dead-only pass error, got {err:?}"
    );

    for i in [0usize, 1, 3, 4] {
        vote(&mut g, &tokens[i], SeatId(4), false).unwrap();
    }
    assert!(g.current_nomination.is_some(), "open until ghost responds");

    pass_vote(&mut g, &tokens[2]).unwrap();
    assert!(g.seats[2].ghost_vote_available, "pass must not spend ghost");
    assert!(g.current_nomination.is_none(), "auto-close after pass");
}
