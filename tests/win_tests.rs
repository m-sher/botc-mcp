//! Minimal win_check coverage for Task 9 (starpass / demon dead stub).

use botc_mcp::game::{
    Game, NightActionPayload, NightStep, Phase, RoleAssignment, SeatId, StartOpts, Winner,
};
use botc_mcp::roles::Character;
use botc_mcp::tools::{night_action, skip_night_action};

fn five_names() -> Vec<String> {
    vec![
        "A".into(),
        "B".into(),
        "C".into(),
        "D".into(),
        "E".into(),
    ]
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
        },
    )
    .unwrap();
    while g.pending_night.is_some() {
        skip_night_action(&mut g, &host).unwrap();
    }
    g.enter_night(2);
    // Poisoner
    night_action(
        &mut g,
        &tokens[2],
        NightActionPayload::PickOne { target: SeatId(0) },
    )
    .unwrap();
    // Imp pending (no Monk)
    let p = g.pending_night.as_ref().unwrap();
    assert!(matches!(p.step, NightStep::DemonKill { .. }));
    night_action(
        &mut g,
        &tokens[1],
        NightActionPayload::PickOne { target: SeatId(1) },
    )
    .unwrap();
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
        },
    )
    .unwrap();
    while g.pending_night.is_some() {
        skip_night_action(&mut g, &host).unwrap();
    }
    // Kill the only minion during day simulation
    g.seats[2].alive = false;
    g.enter_night(2);
    // No poisoner (dead); Imp should be pending
    let p = g.pending_night.as_ref().expect("imp");
    assert!(matches!(p.step, NightStep::DemonKill { .. }));
    night_action(
        &mut g,
        &tokens[1],
        NightActionPayload::PickOne { target: SeatId(1) },
    )
    .unwrap();
    assert_eq!(g.winner, Some(Winner::Good));
    assert!(matches!(g.phase, Phase::Ended { winner: Winner::Good, .. }));
}
