use botc_mcp::error::{GameError, ToolError};
use botc_mcp::game::{Game, RoleAssignment, SeatId, StartOpts};
use botc_mcp::roles::Character;
use botc_mcp::tools;

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
    let view = tools::get_private_state(&g, &tok, 0).unwrap();
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
fn player_forbidden_from_host_state() {
    let g = fixture_assigned_drunk();
    let player_tok = g.tokens.player_token(SeatId(0)).unwrap().clone();
    let err = tools::get_host_state(&g, &player_tok).unwrap_err();
    assert!(matches!(
        err,
        ToolError::Unauthorized | ToolError::Game(GameError::Unauthorized)
    ));
}

#[test]
fn host_state_shows_true_roles_including_drunk() {
    let g = fixture_assigned_drunk();
    let host = g.tokens.host_token().unwrap().clone();
    let view = tools::get_host_state(&g, &host).unwrap();
    assert_eq!(view.seed, 99);
    let s0 = view.seats.iter().find(|s| s.seat_id == SeatId(0)).unwrap();
    assert_eq!(s0.true_character, Some("Drunk"));
    assert_eq!(s0.believed_character, Some("Empath"));
    assert!(s0.is_drunk_outsider);
}

#[test]
fn get_character_rules_loads_markdown() {
    let rules = tools::get_character_rules(Character::Monk).unwrap();
    assert_eq!(rules.name, "Monk");
    assert!(rules.path.contains("monk.md"));
    assert!(rules.text.contains("Monk"));
    assert!(rules.text.contains("safe") || rules.text.contains("Demon"));
}

#[test]
fn public_state_omits_pending_night_seat() {
    let g = fixture_assigned_drunk();
    let player = g.tokens.player_token(SeatId(0)).unwrap().clone();
    let pub_view = tools::get_public_state(&g, &player).unwrap();
    let dump = format!("{pub_view:?}");
    // Must not leak which seat is pending a night action.
    assert!(
        !dump.to_lowercase().contains("pending"),
        "public state must not include pending wake: {dump}"
    );
    // Host may still see pending.
    let host = g.tokens.host_token().unwrap().clone();
    let host_view = tools::get_host_state(&g, &host).unwrap();
    // First night should have some pending or at least grimoire seats.
    assert_eq!(host_view.seats.len(), 5);
    let _ = host_view.pending; // available field for host only
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
    // Fixed salt so bag is deterministic for this seed (secret_salt is otherwise CSPRNG).
    let lobby = Game::create_with_salt(five_names(), 12345, 0).unwrap();
    let host = lobby.host_token.clone();
    let mut g = lobby.game;
    g.start_game(&host, StartOpts::default()).unwrap();
    // night_tick may already be pending a choice (FirstNight) or have finished to Day
    // if the bag has no choice-required N1 roles.
    assert!(
        matches!(
            g.phase,
            botc_mcp::game::Phase::FirstNight { .. } | botc_mcp::game::Phase::Day { day: 1, .. }
        ),
        "unexpected phase {:?}",
        g.phase
    );
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
