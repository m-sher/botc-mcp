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
fn start_game_stores_first_night_queue_and_ticks_to_pending() {
    let game = fixture_drunk_empath_face();
    assert!(matches!(
        game.phase,
        botc_mcp::game::Phase::FirstNight { .. }
    ));
    assert!(!game.night_queue.is_empty());
    // night_tick ran: cursor past SetupMarkers, pending on first choice (Poisoner).
    assert!(game.night_cursor > 0);
    assert_eq!(game.phase.cursor_if_night(), Some(game.night_cursor));
    let p = game.pending_night.as_ref().expect("Poisoner pending");
    assert!(matches!(p.step, NightStep::Poisoner { seat: SeatId(2) }));
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

// ---------------------------------------------------------------------------
// Task 7: night machine — briefings, pending wake, night_action, skip
// ---------------------------------------------------------------------------

use botc_mcp::comms::PrivateMessage;
use botc_mcp::game::{NightActionPayload, Phase};
use botc_mcp::tools::{self, get_private_state, night_action, skip_night_action};

fn seven_names() -> Vec<String> {
    (0..7).map(|i| format!("P{i}")).collect()
}

/// 7p: WW, Lib, Inv, Chef, Poisoner, Imp, Soldier — briefings + Poisoner choice.
fn fixture_7p_poisoner_imp() -> (Game, botc_mcp::auth::Token, Vec<botc_mcp::auth::Token>) {
    let lobby = Game::create(seven_names(), 3).unwrap();
    let host = lobby.host_token.clone();
    let tokens = lobby.player_tokens.clone();
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
    (g, host, tokens)
}

#[test]
fn seven_player_minion_learns_demon_on_start() {
    let (g, _host, tokens) = fixture_7p_poisoner_imp();
    let minion_tok = &tokens[4]; // Poisoner
    let priv_state = get_private_state(&g, minion_tok, 0).unwrap();
    assert!(
        priv_state
            .private_messages_since
            .iter()
            .any(|(_, m)| matches!(m, PrivateMessage::EvilBriefing { .. })),
        "minion must receive EvilBriefing after auto night_tick: {:?}",
        priv_state.private_messages_since
    );
    // Demon also briefed
    let demon = get_private_state(&g, &tokens[5], 0).unwrap();
    assert!(demon
        .private_messages_since
        .iter()
        .any(|(_, m)| matches!(m, PrivateMessage::EvilBriefing { .. })));
}

#[test]
fn start_game_stops_at_poisoner_pending() {
    let (g, _host, tokens) = fixture_7p_poisoner_imp();
    let p = g.pending_night.as_ref().expect("pending Poisoner");
    assert!(matches!(p.step, NightStep::Poisoner { seat: SeatId(4) }));
    assert_eq!(p.seat, SeatId(4));
    assert!(matches!(g.phase, Phase::FirstNight { .. }));

    let poisoner = get_private_state(&g, &tokens[4], 0).unwrap();
    assert!(poisoner.awaiting_action);
    assert!(poisoner.awaiting.is_some());
    assert!(poisoner
        .private_messages_since
        .iter()
        .any(|(_, m)| matches!(m, PrivateMessage::NightPrompt { .. })));

    // Other seats must not see awaiting
    let good = get_private_state(&g, &tokens[0], 0).unwrap();
    assert!(!good.awaiting_action);
    assert!(good.awaiting.is_none());
}

#[test]
fn poisoner_night_action_applies_poison_and_advances() {
    let (mut g, _host, tokens) = fixture_7p_poisoner_imp();
    night_action(
        &mut g,
        &tokens[4],
        NightActionPayload::PickOne { target: SeatId(0) },
    )
    .unwrap();
    assert!(g.seats[0].poisoned);
    assert!(!g.seats[4].poisoned);
    // Pending cleared from Poisoner; next choice or info stubs may run
    if let Some(p) = &g.pending_night {
        assert_ne!(p.seat, SeatId(4), "Poisoner step should be done");
    }
}

#[test]
fn wrong_seat_night_action_rejected() {
    let (mut g, _host, tokens) = fixture_7p_poisoner_imp();
    let err = night_action(
        &mut g,
        &tokens[0],
        NightActionPayload::PickOne { target: SeatId(1) },
    )
    .unwrap_err();
    assert!(matches!(
        err,
        tools::ToolError::Game(botc_mcp::GameError::NotYourWake)
    ));
}

#[test]
fn host_skip_night_action_advances() {
    let (mut g, host, tokens) = fixture_7p_poisoner_imp();
    assert!(g.pending_night.is_some());
    skip_night_action(&mut g, &host).unwrap();
    // Default target is first legal seat (0); poison applied
    assert!(g.seats.iter().any(|s| s.poisoned));
    if let Some(p) = &g.pending_night {
        assert_ne!(p.seat, SeatId(4));
    }
    // Player cannot skip
    let err = skip_night_action(&mut g, &tokens[0]).unwrap_err();
    assert!(matches!(
        err,
        tools::ToolError::Game(botc_mcp::GameError::Unauthorized) | tools::ToolError::Unauthorized
    ));
}

#[test]
fn five_player_no_evil_briefing() {
    let g = fixture_drunk_empath_face();
    let tok = g.tokens.player_token(SeatId(2)).unwrap().clone(); // Poisoner
    let priv_state = get_private_state(&g, &tok, 0).unwrap();
    assert!(!priv_state
        .private_messages_since
        .iter()
        .any(|(_, m)| matches!(m, PrivateMessage::EvilBriefing { .. })));
    assert!(g.pending_night.is_some());
    assert_eq!(g.pending_night.as_ref().unwrap().seat, SeatId(2));
}

// ---------------------------------------------------------------------------
// Task 8: info ability resolution + disabled lies
// ---------------------------------------------------------------------------

fn night_results_for(game: &Game, seat: SeatId) -> Vec<String> {
    game.private_inboxes
        .since(seat, 0)
        .into_iter()
        .filter_map(|(_, m)| match m {
            PrivateMessage::NightResult { text } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// Circle: Good, Imp, Empath, Good, Good — Empath neighbors Imp + Good => 1
#[test]
fn empath_counts_living_evil_neighbors() {
    let lobby = Game::create(five_names(), 42).unwrap();
    let host = lobby.host_token.clone();
    let mut g = lobby.game;
    g.start_game(
        &host,
        StartOpts {
            assignments: Some(vec![
                RoleAssignment::normal(SeatId(0), Character::Soldier), // good
                RoleAssignment::normal(SeatId(1), Character::Imp),     // evil
                RoleAssignment::normal(SeatId(2), Character::Empath),
                RoleAssignment::normal(SeatId(3), Character::Chef), // good
                RoleAssignment::normal(SeatId(4), Character::Soldier), // good
            ]),
        },
    )
    .unwrap();
    // No choice roles → night auto-resolves through Empath to dawn.
    let results = night_results_for(&g, SeatId(2));
    assert!(
        results.iter().any(|t| t.contains("that 1 of")),
        "Empath should learn 1 evil neighbor: {results:?}"
    );
}

#[test]
fn drunk_empath_gets_wrong_info() {
    // Truth neighbors: Imp + Good => 1; disabled always lies => 0 or 2
    let lobby = Game::create(five_names(), 42).unwrap();
    let host = lobby.host_token.clone();
    let mut g = lobby.game;
    g.start_game(
        &host,
        StartOpts {
            assignments: Some(vec![
                RoleAssignment::drunk(SeatId(0), Character::Empath).unwrap(),
                RoleAssignment::normal(SeatId(1), Character::Imp),
                RoleAssignment::normal(SeatId(2), Character::Soldier),
                RoleAssignment::normal(SeatId(3), Character::Chef),
                RoleAssignment::normal(SeatId(4), Character::Soldier),
            ]),
        },
    )
    .unwrap();
    let results = night_results_for(&g, SeatId(0));
    assert!(
        results
            .iter()
            .any(|t| t.contains("that 0 of") || t.contains("that 2 of")),
        "Drunk Empath must get wrong count (not 1): {results:?}"
    );
    assert!(
        !results.iter().any(|t| t.contains("that 1 of")),
        "Drunk Empath must never get the true count 1: {results:?}"
    );
}

#[test]
fn fortune_teller_red_herring_pings_yes() {
    let lobby = Game::create(five_names(), 7).unwrap();
    let host = lobby.host_token.clone();
    let tokens = lobby.player_tokens.clone();
    let mut g = lobby.game;
    g.start_game(
        &host,
        StartOpts {
            assignments: Some(vec![
                RoleAssignment::normal(SeatId(0), Character::FortuneTeller),
                RoleAssignment::normal(SeatId(1), Character::Imp),
                RoleAssignment::normal(SeatId(2), Character::Soldier),
                RoleAssignment::normal(SeatId(3), Character::Chef),
                RoleAssignment::normal(SeatId(4), Character::Soldier),
            ]),
        },
    )
    .unwrap();
    // Force herring to a good non-demon seat and pick herring + good townsfolk.
    g.red_herring = Some(SeatId(2));
    let p = g.pending_night.as_ref().expect("FT pending");
    assert!(matches!(p.step, NightStep::FortuneTeller { seat: SeatId(0) }));
    night_action(
        &mut g,
        &tokens[0],
        NightActionPayload::PickTwo {
            a: SeatId(2),
            b: SeatId(3),
        },
    )
    .unwrap();
    let results = night_results_for(&g, SeatId(0));
    assert!(
        results
            .iter()
            .any(|t| t.contains("YES") || t.contains("yes")),
        "red herring + good should ping yes: {results:?}"
    );
}

#[test]
fn librarian_zero_outsiders_reports_zero() {
    let lobby = Game::create(five_names(), 3).unwrap();
    let host = lobby.host_token.clone();
    let tokens = lobby.player_tokens.clone();
    let mut g = lobby.game;
    // 5p: no Outsiders in bag.
    g.start_game(
        &host,
        StartOpts {
            assignments: Some(vec![
                RoleAssignment::normal(SeatId(0), Character::Librarian),
                RoleAssignment::normal(SeatId(1), Character::Imp),
                RoleAssignment::normal(SeatId(2), Character::Poisoner),
                RoleAssignment::normal(SeatId(3), Character::Chef),
                RoleAssignment::normal(SeatId(4), Character::Soldier),
            ]),
        },
    )
    .unwrap();
    // Poison someone other than the Librarian so info stays truthful.
    night_action(
        &mut g,
        &tokens[2],
        NightActionPayload::PickOne { target: SeatId(4) },
    )
    .unwrap();
    while g.pending_night.is_some() {
        skip_night_action(&mut g, &host).unwrap();
    }
    let results = night_results_for(&g, SeatId(0));
    assert!(
        results.iter().any(|t| t.contains("0 Outsiders")),
        "Librarian with no Outsiders: {results:?}"
    );
}

// ---------------------------------------------------------------------------
// Task 9: demon kill, soldier, monk, starpass, dawn
// ---------------------------------------------------------------------------

use botc_mcp::comms::PublicEvent;
use botc_mcp::game::DayStage;

/// Finish first night via host skips, then enter night 2.
fn finish_n1_enter_n2(
    g: &mut Game,
    host: &botc_mcp::auth::Token,
) {
    while g.pending_night.is_some() {
        skip_night_action(g, host).unwrap();
    }
    assert!(
        matches!(g.phase, Phase::Day { day: 1, stage: DayStage::Discussion }),
        "expected Day 1 Discussion, got {:?}",
        g.phase
    );
    g.enter_night(2);
}

/// 5p other-night: Monk, Imp, Poisoner, Empath, Soldier.
fn fixture_n2_monk_imp_soldier() -> (Game, botc_mcp::auth::Token, Vec<botc_mcp::auth::Token>) {
    let lobby = Game::create(five_names(), 11).unwrap();
    let host = lobby.host_token.clone();
    let tokens = lobby.player_tokens.clone();
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
    finish_n1_enter_n2(&mut g, &host);
    (g, host, tokens)
}

/// Drive N2 until Imp is pending (skip poisoner + monk with given payloads or defaults).
fn advance_to_imp_kill(
    g: &mut Game,
    host: &botc_mcp::auth::Token,
    tokens: &[botc_mcp::auth::Token],
    poison_target: SeatId,
    monk_target: Option<SeatId>,
) {
    // Poisoner
    let p = g.pending_night.as_ref().expect("pending");
    assert!(matches!(p.step, NightStep::Poisoner { .. }));
    night_action(
        g,
        &tokens[p.seat.0 as usize],
        NightActionPayload::PickOne {
            target: poison_target,
        },
    )
    .unwrap();
    // Monk
    let p = g.pending_night.as_ref().expect("monk pending");
    assert!(matches!(p.step, NightStep::Monk { .. }));
    if let Some(t) = monk_target {
        night_action(
            g,
            &tokens[p.seat.0 as usize],
            NightActionPayload::PickOne { target: t },
        )
        .unwrap();
    } else {
        skip_night_action(g, host).unwrap();
    }
    let p = g.pending_night.as_ref().expect("imp pending");
    assert!(
        matches!(p.step, NightStep::DemonKill { seat: SeatId(1) }),
        "expected Imp kill, got {:?}",
        p.step
    );
}

#[test]
fn soldier_survives_imp() {
    let (mut g, host, tokens) = fixture_n2_monk_imp_soldier();
    // Poison someone other than Soldier so Soldier ability stays active.
    advance_to_imp_kill(&mut g, &host, &tokens, SeatId(3), Some(SeatId(3)));
    night_action(
        &mut g,
        &tokens[1],
        NightActionPayload::PickOne { target: SeatId(4) },
    )
    .unwrap();
    assert!(
        g.seats[4].alive,
        "Soldier must survive Imp kill"
    );
    assert!(
        !g.deaths_tonight.contains(&SeatId(4)),
        "Soldier must not be in deaths_tonight"
    );
}

#[test]
fn monk_protects_target() {
    let (mut g, host, tokens) = fixture_n2_monk_imp_soldier();
    // Monk protects Empath; Imp kills Empath → survives.
    advance_to_imp_kill(&mut g, &host, &tokens, SeatId(2), Some(SeatId(3)));
    assert!(g.seats[3].monk_protected_tonight);
    night_action(
        &mut g,
        &tokens[1],
        NightActionPayload::PickOne { target: SeatId(3) },
    )
    .unwrap();
    assert!(g.seats[3].alive, "Monk-protected Empath must live");
    assert!(!g.deaths_tonight.contains(&SeatId(3)));
}

#[test]
fn imp_starpass_transfers_to_minion() {
    let (mut g, host, tokens) = fixture_n2_monk_imp_soldier();
    advance_to_imp_kill(&mut g, &host, &tokens, SeatId(0), Some(SeatId(3)));
    night_action(
        &mut g,
        &tokens[1],
        NightActionPayload::PickOne { target: SeatId(1) }, // self
    )
    .unwrap();
    assert!(!g.seats[1].alive, "old Imp must be dead");
    // Only minion is Poisoner seat 2 → becomes Imp
    assert_eq!(g.seats[2].true_character, Some(Character::Imp));
    assert!(g.seats[2].alive);
    let msgs = g.private_inboxes.since(SeatId(2), 0);
    assert!(
        msgs.iter().any(|(_, m)| matches!(
            m,
            PrivateMessage::YouAre {
                character_label,
                ..
            } if character_label == "Imp"
        )),
        "new Imp must receive YouAre Imp: {msgs:?}"
    );
    // Game continues (living Imp exists). Night may auto-advance past Empath/Dawn.
    assert!(g.winner.is_none());
    assert!(!matches!(g.phase, Phase::Ended { .. }));
    // Public death list (after dawn) includes old Imp, never roles.
    let died_events: Vec<_> = g
        .public_log
        .since(0)
        .into_iter()
        .filter_map(|(_, e)| match e {
            PublicEvent::DiedInNight { seats } if !seats.is_empty() => Some(seats.clone()),
            _ => None,
        })
        .collect();
    assert!(
        died_events.iter().any(|s| s.contains(&SeatId(1))),
        "old Imp should appear in DiedInNight: {died_events:?}"
    );
}

#[test]
fn dawn_announces_deaths_publicly_not_roles() {
    let (mut g, host, tokens) = fixture_n2_monk_imp_soldier();
    advance_to_imp_kill(&mut g, &host, &tokens, SeatId(2), Some(SeatId(4)));
    night_action(
        &mut g,
        &tokens[1],
        NightActionPayload::PickOne { target: SeatId(3) }, // Empath
    )
    .unwrap();
    assert!(!g.seats[3].alive);
    // Finish night (Empath auto if any remaining, Butler none, Dawn)
    while g.pending_night.is_some() && !matches!(g.phase, Phase::Day { .. } | Phase::Ended { .. }) {
        skip_night_action(&mut g, &host).unwrap();
    }
    // If only auto steps left, night_tick should have dawned already after last skip.
    while matches!(g.phase, Phase::Night { .. }) && g.pending_night.is_none() {
        // stuck? shouldn't happen
        break;
    }
    assert!(
        matches!(g.phase, Phase::Day { day: 2, stage: DayStage::Discussion }),
        "expected Day 2 Discussion after dawn, got {:?}",
        g.phase
    );

    let events: Vec<_> = g
        .public_log
        .since(0)
        .into_iter()
        .map(|(_, e)| e.clone())
        .collect();
    // N1 dawn may have empty DiedInNight; N2 should list Empath (seat 3) only.
    let night_deaths: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            PublicEvent::DiedInNight { seats } if !seats.is_empty() => Some(seats.clone()),
            _ => None,
        })
        .collect();
    assert!(
        night_deaths.iter().any(|s| s == &vec![SeatId(3)]),
        "DiedInNight must list seat 3 only (no roles): {night_deaths:?}"
    );
    // Public announce names the player, never character roles.
    let announces: Vec<String> = events
        .iter()
        .filter_map(|e| match e {
            PublicEvent::StorytellerAnnounce { text } => Some(text.clone()),
            _ => None,
        })
        .collect();
    let dawn_line = announces
        .iter()
        .find(|t| t.contains("Died in the night"))
        .expect("dawn death announce");
    assert!(
        dawn_line.contains('D') || dawn_line.contains("seat"),
        "dawn should name player: {dawn_line}"
    );
    for role in ["Empath", "Imp", "Poisoner", "Soldier", "Monk"] {
        assert!(
            !dawn_line.contains(role),
            "dawn must not reveal roles ({role}): {dawn_line}"
        );
    }
}
