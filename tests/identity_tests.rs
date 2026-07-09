use botc_mcp::game::{Game, RoleAssignment, SeatId, StartOpts};
use botc_mcp::roles::Character;

fn five_names() -> Vec<String> {
    vec![
        "Dana".into(),
        "Eve".into(),
        "C".into(),
        "D".into(),
        "E".into(),
    ]
}

/// Fixed 5-seat game: seat0 Drunk (Empath face), Imp, Poisoner, Chef, Soldier.
fn fixture_assigned_drunk() -> Game {
    let lobby = Game::create(five_names(), 99).unwrap();
    let host = lobby.host_token.clone();
    let mut g = lobby.game;
    g.start_game(
        &host,
        StartOpts {
            assignments: Some(vec![
                RoleAssignment::drunk(SeatId(0), Character::Empath).unwrap(),
                RoleAssignment::normal(SeatId(1), Character::Imp),
                RoleAssignment::normal(SeatId(2), Character::Poisoner),
                RoleAssignment::normal(SeatId(3), Character::Chef),
                RoleAssignment::normal(SeatId(4), Character::Soldier),
            ]),
        },
    )
    .expect("start_game with fixed assignments");
    g
}

#[test]
fn drunk_private_state_never_says_drunk() {
    let g = fixture_assigned_drunk();
    let tok = g.tokens.player_token(SeatId(0)).unwrap().clone();
    let view = botc_mcp::tools::get_private_state(&g, &tok, 0).unwrap();
    assert_eq!(view.character_label.as_deref(), Some("Empath"));
    assert_ne!(view.character_label.as_deref(), Some("Drunk"));
    let dump = format!("{view:?}").to_lowercase();
    assert!(
        !dump.contains("drunk"),
        "private state debug dump must not mention drunk: {dump}"
    );
    assert_eq!(g.seats[0].true_character, Some(Character::Drunk));
    assert_eq!(g.seats[0].believed_character, Some(Character::Empath));
}

#[test]
fn start_game_requires_host_and_imp() {
    let lobby = Game::create(five_names(), 1).unwrap();
    let player = lobby.player_tokens[0].clone();
    let host = lobby.host_token.clone();
    let mut g = lobby.game;

    let err = g
        .start_game(
            &player,
            StartOpts {
                assignments: Some(vec![
                    RoleAssignment::normal(SeatId(0), Character::Chef),
                    RoleAssignment::normal(SeatId(1), Character::Imp),
                    RoleAssignment::normal(SeatId(2), Character::Poisoner),
                    RoleAssignment::normal(SeatId(3), Character::Empath),
                    RoleAssignment::normal(SeatId(4), Character::Soldier),
                ]),
            },
        )
        .unwrap_err();
    assert!(matches!(err, botc_mcp::GameError::Unauthorized));

    let err = g
        .start_game(
            &host,
            StartOpts {
                assignments: Some(vec![
                    RoleAssignment::normal(SeatId(0), Character::Chef),
                    RoleAssignment::normal(SeatId(1), Character::Empath),
                    RoleAssignment::normal(SeatId(2), Character::Poisoner),
                    RoleAssignment::normal(SeatId(3), Character::Soldier),
                    RoleAssignment::normal(SeatId(4), Character::Monk),
                ]),
            },
        )
        .unwrap_err();
    assert!(matches!(err, botc_mcp::GameError::IllegalAction(_)));
}

#[test]
fn start_game_random_bag_assigns_all_seats() {
    let lobby = Game::create(five_names(), 12345).unwrap();
    let host = lobby.host_token.clone();
    let mut g = lobby.game;
    g.start_game(&host, StartOpts::default()).unwrap();
    assert!(matches!(
        g.phase,
        botc_mcp::game::Phase::FirstNight { .. }
    ));
    assert_eq!(g.seats.len(), 5);
    assert!(g.seats.iter().all(|s| s.true_character.is_some()));
    assert!(g
        .seats
        .iter()
        .any(|s| s.true_character == Some(Character::Imp)));
    // bag size equals seats
    assert_eq!(
        g.seats
            .iter()
            .filter(|s| s.true_character.is_some())
            .count(),
        5
    );
}
