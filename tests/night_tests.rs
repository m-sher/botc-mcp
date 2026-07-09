use botc_mcp::game::night::{build_first_night_queue, build_other_night_queue};
use botc_mcp::game::{Game, NightStep, RoleAssignment, SeatId, StartOpts};
use botc_mcp::roles::Character;

fn five_names() -> Vec<String> {
    vec![
        "A".into(),
        "B".into(),
        "C".into(),
        "D".into(),
        "E".into(),
    ]
}

/// seat0 Drunk face Empath, Imp, Poisoner, Chef, Soldier.
fn fixture_drunk_empath_face() -> Game {
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
    .expect("start_game");
    g
}

#[test]
fn first_night_queue_includes_faced_empath_not_drunk_token() {
    let game = fixture_drunk_empath_face();
    let q = build_first_night_queue(&game);
    assert!(
        q.iter()
            .any(|s| matches!(s, NightStep::Empath { seat: SeatId(0) })),
        "Drunk face Empath must wake as Empath: {q:?}"
    );
    assert!(
        !q.iter().any(|s| matches!(s, NightStep::DemonKill { .. })),
        "no Imp kill on first night: {q:?}"
    );
    // True Drunk must not produce a Drunk step (there is none) or omit Empath.
    assert!(!format!("{q:?}").to_lowercase().contains("drunk"));
}

#[test]
fn first_night_queue_order_and_minion_by_true_character() {
    let game = fixture_drunk_empath_face();
    let q = build_first_night_queue(&game);
    // Poisoner (true) before info roles; Chef face; Empath face; Dawn last.
    let poisoner = q
        .iter()
        .position(|s| matches!(s, NightStep::Poisoner { seat: SeatId(2) }))
        .expect("Poisoner step");
    let chef = q
        .iter()
        .position(|s| matches!(s, NightStep::Chef { seat: SeatId(3) }))
        .expect("Chef step");
    let empath = q
        .iter()
        .position(|s| matches!(s, NightStep::Empath { seat: SeatId(0) }))
        .expect("Empath step");
    assert!(matches!(q.first(), Some(NightStep::SetupMarkers)));
    assert!(matches!(q.last(), Some(NightStep::Dawn)));
    assert!(poisoner < chef);
    assert!(chef < empath);
    // n=5: no minion/demon briefing
    assert!(!q.iter().any(|s| matches!(s, NightStep::MinionBriefing)));
    assert!(!q.iter().any(|s| matches!(s, NightStep::DemonBriefing)));
}

#[test]
fn start_game_stores_first_night_queue_and_cursor() {
    let game = fixture_drunk_empath_face();
    assert!(matches!(
        game.phase,
        botc_mcp::game::Phase::FirstNight { cursor: 0 }
    ));
    assert!(!game.night_queue.is_empty());
    assert_eq!(game.night_queue, build_first_night_queue(&game));
    assert_eq!(game.night_cursor, 0);
}

#[test]
fn other_night_queue_has_demon_kill_and_monk_not_n1_setup() {
    let lobby = Game::create(five_names(), 7).unwrap();
    let host = lobby.host_token.clone();
    let mut g = lobby.game;
    g.start_game(
        &host,
        StartOpts {
            assignments: Some(vec![
                RoleAssignment::normal(SeatId(0), Character::Monk),
                RoleAssignment::normal(SeatId(1), Character::Imp),
                RoleAssignment::normal(SeatId(2), Character::Poisoner),
                RoleAssignment::normal(SeatId(3), Character::Empath),
                RoleAssignment::normal(SeatId(4), Character::Soldier),
            ]),
        },
    )
    .unwrap();
    // Kill monk for eligibility checks later; queue uses alive seats.
    let q = build_other_night_queue(&g);
    assert!(!q.iter().any(|s| matches!(s, NightStep::SetupMarkers)));
    assert!(!q.iter().any(|s| matches!(s, NightStep::MinionBriefing)));
    assert!(q
        .iter()
        .any(|s| matches!(s, NightStep::DemonKill { seat: SeatId(1) })));
    assert!(q
        .iter()
        .any(|s| matches!(s, NightStep::Monk { seat: SeatId(0) })));
    assert!(q
        .iter()
        .any(|s| matches!(s, NightStep::Empath { seat: SeatId(3) })));
    assert!(matches!(q.last(), Some(NightStep::Dawn)));
    let poisoner = q
        .iter()
        .position(|s| matches!(s, NightStep::Poisoner { .. }))
        .unwrap();
    let monk = q.iter().position(|s| matches!(s, NightStep::Monk { .. })).unwrap();
    let kill = q
        .iter()
        .position(|s| matches!(s, NightStep::DemonKill { .. }))
        .unwrap();
    assert!(poisoner < monk && monk < kill);
}

#[test]
fn seven_player_first_night_includes_briefings() {
    let names: Vec<String> = (0..7).map(|i| format!("P{i}")).collect();
    let lobby = Game::create(names, 3).unwrap();
    let host = lobby.host_token.clone();
    let mut g = lobby.game;
    g.start_game(
        &host,
        StartOpts {
            assignments: Some(vec![
                RoleAssignment::normal(SeatId(0), Character::Washerwoman),
                RoleAssignment::normal(SeatId(1), Character::Librarian),
                RoleAssignment::normal(SeatId(2), Character::Investigator),
                RoleAssignment::normal(SeatId(3), Character::Chef),
                RoleAssignment::normal(SeatId(4), Character::Poisoner),
                RoleAssignment::normal(SeatId(5), Character::Imp),
                RoleAssignment::normal(SeatId(6), Character::Soldier),
            ]),
        },
    )
    .unwrap();
    let q = build_first_night_queue(&g);
    let setup = q.iter().position(|s| matches!(s, NightStep::SetupMarkers)).unwrap();
    let minion = q.iter().position(|s| matches!(s, NightStep::MinionBriefing)).unwrap();
    let demon = q.iter().position(|s| matches!(s, NightStep::DemonBriefing)).unwrap();
    let poisoner = q
        .iter()
        .position(|s| matches!(s, NightStep::Poisoner { seat: SeatId(4) }))
        .unwrap();
    assert!(setup < minion && minion < demon && demon < poisoner);
}

#[test]
fn dead_seats_omitted_from_queues() {
    let mut g = fixture_drunk_empath_face();
    g.seats[2].alive = false; // Poisoner dead
    g.seats[3].alive = false; // Chef dead
    let q = build_first_night_queue(&g);
    assert!(!q.iter().any(|s| matches!(s, NightStep::Poisoner { .. })));
    assert!(!q.iter().any(|s| matches!(s, NightStep::Chef { .. })));
    // Empath face still alive
    assert!(q
        .iter()
        .any(|s| matches!(s, NightStep::Empath { seat: SeatId(0) })));
}
