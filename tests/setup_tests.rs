#[test]
fn create_game_issues_n_player_tokens() {
    let out = botc_mcp::tools::create_game_in_memory(
        vec!["A".into(), "B".into(), "C".into(), "D".into(), "E".into()],
        1,
    );
    assert_eq!(out.players.len(), 5);
    assert_eq!(out.players[0].name, "A");
    assert!(!out.host_token.as_str().is_empty());
}

#[test]
fn create_game_rejects_too_few_players() {
    let mut store = botc_mcp::store::GameStore::new();
    let err = botc_mcp::tools::create_game(
        &mut store,
        vec!["A".into(), "B".into(), "C".into(), "D".into()],
        1,
        None,
    )
    .unwrap_err();
    assert!(matches!(
        err,
        botc_mcp::ToolError::Game(botc_mcp::GameError::BadRequest(_))
            | botc_mcp::ToolError::BadRequest(_)
    ));
}

#[test]
fn create_game_rejects_too_many_players() {
    let names: Vec<String> = (0..16).map(|i| format!("P{i}")).collect();
    let mut store = botc_mcp::store::GameStore::new();
    let err = botc_mcp::tools::create_game(&mut store, names, 1, None).unwrap_err();
    assert!(matches!(
        err,
        botc_mcp::ToolError::Game(botc_mcp::GameError::BadRequest(_))
            | botc_mcp::ToolError::BadRequest(_)
    ));
}

#[test]
fn create_game_stores_seed_and_lobby_phase() {
    let mut store = botc_mcp::store::GameStore::new();
    let out = botc_mcp::tools::create_game(
        &mut store,
        vec![
            "A".into(),
            "B".into(),
            "C".into(),
            "D".into(),
            "E".into(),
            "F".into(),
        ],
        42,
        None,
    )
    .unwrap();
    let game = store.get_mut(out.game_id).expect("game inserted");
    assert_eq!(game.seed, 42);
    assert!(matches!(game.phase, botc_mcp::game::Phase::Lobby));
    assert_eq!(game.seats.len(), 6);
    assert!(game.winner.is_none());
    assert!(game.public_log.since(0).is_empty());
    // seat tokens resolve
    assert!(game.tokens.resolve(&out.host_token).is_some());
    assert_eq!(out.players[0].seat_id, botc_mcp::game::SeatId(0));
    assert_eq!(out.players[5].name, "F");
}

#[test]
fn composition_8() {
    let c = botc_mcp::game::setup::composition(8);
    assert_eq!(
        (c.townsfolk, c.outsiders, c.minions, c.demons),
        (5, 1, 1, 1)
    );
}

#[test]
fn composition_table_matches_docs() {
    use botc_mcp::game::setup::composition;
    let expected = [
        (5, 3, 0, 1, 1),
        (6, 3, 1, 1, 1),
        (7, 5, 0, 1, 1),
        (8, 5, 1, 1, 1),
        (9, 5, 2, 1, 1),
        (10, 7, 0, 2, 1),
        (11, 7, 1, 2, 1),
        (12, 7, 2, 2, 1),
        (13, 9, 0, 3, 1),
        (14, 9, 1, 3, 1),
        (15, 9, 2, 3, 1),
    ];
    for (n, tf, out, min, dem) in expected {
        let c = composition(n);
        assert_eq!(
            (c.townsfolk, c.outsiders, c.minions, c.demons),
            (tf, out, min, dem),
            "n={n}"
        );
        assert_eq!(c.townsfolk + c.outsiders + c.minions + c.demons, n);
    }
}
