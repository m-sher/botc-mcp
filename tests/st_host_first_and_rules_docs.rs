//! Host-first Storyteller defaults + public rules doc tools.

use botc_mcp::game::{
    DayStage, Game, HostDecision, Phase, RoleAssignment, SeatId, StChoiceMode, StartOpts,
};
use botc_mcp::roles::Character;
use botc_mcp::tools::{
    get_character_list, get_host_state, get_rules_topic, get_rules_topics, host_decide, nominate,
    open_nominations, skip_night_action, start_game,
};

fn names(n: usize) -> Vec<String> {
    (0..n).map(|i| format!("P{i}")).collect()
}

fn start_scripted(
    seed: u64,
    assignments: Vec<RoleAssignment>,
    opts_extra: impl FnOnce(&mut StartOpts),
) -> (Game, botc_mcp::auth::Token, Vec<botc_mcp::auth::Token>) {
    let lobby = Game::create_with_salt(names(assignments.len()), seed, 0).unwrap();
    let host = lobby.host_token.clone();
    let tokens = lobby.player_tokens.clone();
    let mut g = lobby.game;
    let mut opts = StartOpts {
        assignments: Some(assignments),
        ..Default::default()
    };
    opts_extra(&mut opts);
    start_game(&mut g, &host, opts).unwrap();
    (g, host, tokens)
}

fn to_day1(g: &mut Game, host: &botc_mcp::auth::Token) {
    while g.pending_night.is_some() || g.pending_host.is_some() {
        skip_night_action(g, host).unwrap();
    }
    assert!(matches!(
        g.phase,
        Phase::Day {
            day: 1,
            stage: DayStage::Discussion
        }
    ));
}

#[test]
fn default_st_choice_mode_is_host_first() {
    let lobby = Game::create(names(5), 1).unwrap();
    assert_eq!(lobby.game.st_choice_mode, StChoiceMode::HostFirst);
}

#[test]
fn washerwoman_host_first_pauses_for_storyteller() {
    let (mut g, host, _tokens) = start_scripted(
        11,
        vec![
            RoleAssignment::normal(SeatId(0), Character::Washerwoman),
            RoleAssignment::normal(SeatId(1), Character::Soldier),
            RoleAssignment::normal(SeatId(2), Character::Chef),
            RoleAssignment::normal(SeatId(3), Character::Poisoner),
            RoleAssignment::normal(SeatId(4), Character::Imp),
        ],
        |_| {},
    );
    // Skip poisoner; next auto-info is Washerwoman → host pending.
    while g.pending_night.is_some() {
        skip_night_action(&mut g, &host).unwrap();
    }
    let ph = g.pending_host.as_ref().expect("host pending for WW");
    assert!(
        matches!(
            ph,
            botc_mcp::game::PendingHostDecision::NightInfo { ability, .. }
                if ability == "Washerwoman"
        ),
        "got {ph:?}"
    );
    host_decide(
        &mut g,
        &host,
        HostDecision::NightInfo {
            text: "Washerwoman: one of P1 (seat 1) and P2 (seat 2) is the Chef.".into(),
        },
    )
    .unwrap();
    let msgs = g.private_inboxes.since(SeatId(0), 0);
    assert!(msgs.iter().any(|(_, m)| match m {
        botc_mcp::comms::PrivateMessage::NightResult { text } => text.contains("Chef"),
        _ => false,
    }));
}

#[test]
fn night_info_skip_uses_engine_random_fallback() {
    let (mut g, host, _) = start_scripted(
        12,
        vec![
            RoleAssignment::normal(SeatId(0), Character::Washerwoman),
            RoleAssignment::normal(SeatId(1), Character::Soldier),
            RoleAssignment::normal(SeatId(2), Character::Chef),
            RoleAssignment::normal(SeatId(3), Character::Poisoner),
            RoleAssignment::normal(SeatId(4), Character::Imp),
        ],
        |_| {},
    );
    while g.pending_night.is_some() {
        skip_night_action(&mut g, &host).unwrap();
    }
    assert!(g.pending_host.is_some());
    // Skip applies random pair-info resolution.
    skip_night_action(&mut g, &host).unwrap();
    let msgs = g.private_inboxes.since(SeatId(0), 0);
    assert!(
        msgs.iter().any(|(_, m)| matches!(
            m,
            botc_mcp::comms::PrivateMessage::NightResult { text } if text.contains("Washerwoman")
        )),
        "skip should deliver WW result"
    );
}

#[test]
fn st_choice_mode_random_auto_resolves_pair_info() {
    let (mut g, host, _) = start_scripted(
        13,
        vec![
            RoleAssignment::normal(SeatId(0), Character::Washerwoman),
            RoleAssignment::normal(SeatId(1), Character::Soldier),
            RoleAssignment::normal(SeatId(2), Character::Chef),
            RoleAssignment::normal(SeatId(3), Character::Poisoner),
            RoleAssignment::normal(SeatId(4), Character::Imp),
        ],
        |opts| {
            opts.st_choice_mode = StChoiceMode::Random;
        },
    );
    while g.pending_night.is_some() || g.pending_host.is_some() {
        // With Random mode, WW should not set pending_host.
        if g.pending_host.is_some() {
            // Mayor/starpass only later nights.
            skip_night_action(&mut g, &host).unwrap();
            continue;
        }
        if g.pending_night.is_some() {
            skip_night_action(&mut g, &host).unwrap();
        }
    }
    let msgs = g.private_inboxes.since(SeatId(0), 0);
    assert!(msgs.iter().any(|(_, m)| matches!(
        m,
        botc_mcp::comms::PrivateMessage::NightResult { text } if text.contains("Washerwoman")
    )));
}

#[test]
fn virgin_spy_host_first_pauses_registration() {
    let (mut g, host, tokens) = start_scripted(
        14,
        vec![
            RoleAssignment::normal(SeatId(0), Character::Virgin),
            RoleAssignment::normal(SeatId(1), Character::Spy),
            RoleAssignment::normal(SeatId(2), Character::Soldier),
            RoleAssignment::normal(SeatId(3), Character::Chef),
            RoleAssignment::normal(SeatId(4), Character::Imp),
        ],
        |_| {},
    );
    to_day1(&mut g, &host);
    open_nominations(&mut g, &host).unwrap();
    nominate(&mut g, &tokens[1], SeatId(0)).unwrap();
    assert!(matches!(
        g.pending_host,
        Some(botc_mcp::game::PendingHostDecision::VirginSpyReg { .. })
    ));
    // #39: no public Nominated / vote limbo until host resolves.
    assert!(g.current_nomination.is_none());
    assert!(
        !g.public_log
            .since(0)
            .iter()
            .any(|(_, e)| matches!(e, botc_mcp::comms::PublicEvent::Nominated { .. })),
        "Nominated must wait until host resolves VirginSpyReg"
    );
    // #37: concurrent nominate blocked while pending.
    let err = nominate(&mut g, &tokens[2], SeatId(4));
    assert!(err.is_err(), "nominate blocked during host pause");
    // Host: Spy registers as Townsfolk → execute Spy.
    host_decide(
        &mut g,
        &host,
        HostDecision::Registration { register: true },
    )
    .unwrap();
    assert!(!g.seats[1].alive);
    assert!(g
        .public_log
        .since(0)
        .iter()
        .any(|(_, e)| matches!(e, botc_mcp::comms::PublicEvent::Nominated { .. })));
}

/// #36: end_nominations cannot drop a pending Slayer Recluse decision.
#[test]
fn end_nominations_blocked_while_slayer_recluse_pending() {
    let (mut g, host, tokens) = start_scripted(
        36,
        vec![
            RoleAssignment::normal(SeatId(0), Character::Slayer),
            RoleAssignment::normal(SeatId(1), Character::Recluse),
            RoleAssignment::normal(SeatId(2), Character::Soldier),
            RoleAssignment::normal(SeatId(3), Character::Poisoner),
            RoleAssignment::normal(SeatId(4), Character::Imp),
        ],
        |_| {},
    );
    to_day1(&mut g, &host);
    // Clear residual N1 poison so Recluse registration is legal.
    for s in &mut g.seats {
        s.poisoned = false;
    }
    botc_mcp::tools::day_action(
        &mut g,
        &tokens[0],
        botc_mcp::tools::DayActionPayload::Slay { target: SeatId(1) },
    )
    .unwrap();
    assert!(
        matches!(
            g.pending_host,
            Some(botc_mcp::game::PendingHostDecision::SlayerRecluseReg { .. })
        ),
        "expected SlayerRecluseReg, got {:?}",
        g.pending_host
    );
    open_nominations(&mut g, &host).expect_err("open_noms blocked");
    botc_mcp::tools::end_nominations(&mut g, &host).expect_err("end_noms blocked");
    // Still day; decision still pending; Recluse still alive.
    assert!(matches!(g.phase, Phase::Day { day: 1, .. }));
    assert!(g.pending_host.is_some());
    assert!(g.seats[1].alive);
    // Resolve then day can advance.
    host_decide(
        &mut g,
        &host,
        HostDecision::Registration { register: false },
    )
    .unwrap();
    assert!(g.seats[1].alive);
    open_nominations(&mut g, &host).unwrap();
    botc_mcp::tools::end_nominations(&mut g, &host).unwrap();
    assert!(matches!(g.phase, Phase::Night { night: 2, .. }));
}

/// #40: Fortune Teller always pauses in host-first (not only when Recluse is picked).
#[test]
fn fortune_teller_always_pauses_host_first() {
    let (mut g, host, tokens) = start_scripted(
        40,
        vec![
            RoleAssignment::normal(SeatId(0), Character::FortuneTeller),
            RoleAssignment::normal(SeatId(1), Character::Imp),
            RoleAssignment::normal(SeatId(2), Character::Soldier),
            RoleAssignment::normal(SeatId(3), Character::Chef),
            RoleAssignment::normal(SeatId(4), Character::Soldier),
        ],
        |_| {},
    );
    // Skip until FT wake (Chef may host-pause first).
    loop {
        if g.pending_host.is_some() {
            skip_night_action(&mut g, &host).unwrap();
            continue;
        }
        match g.pending_night.as_ref().map(|p| p.step) {
            Some(botc_mcp::game::NightStep::FortuneTeller { seat: SeatId(0) }) => break,
            Some(_) => skip_night_action(&mut g, &host).unwrap(),
            None => panic!("stuck {:?}", g.phase),
        }
    }
    botc_mcp::tools::night_action(
        &mut g,
        &tokens[0],
        botc_mcp::game::NightActionPayload::PickTwo {
            a: SeatId(2),
            b: SeatId(3),
        },
    )
    .unwrap();
    // No Recluse in bag — still pauses so delay is non-informative.
    assert!(
        matches!(
            g.pending_host,
            Some(botc_mcp::game::PendingHostDecision::NightInfo { .. })
        ),
        "FT must always pause in host-first; got {:?}",
        g.pending_host
    );
    assert!(
        g.private_inboxes
            .since(SeatId(0), 0)
            .iter()
            .filter(|(_, m)| matches!(m, botc_mcp::comms::PrivateMessage::NightResult { .. }))
            .count()
            == 0,
        "no FT result until host resolves"
    );
}

#[test]
fn list_rules_topics_and_get_gameplay_loop() {
    let topics = get_rules_topics();
    assert!(topics.iter().any(|t| t.id == "gameplay_loop"));
    assert!(topics.iter().any(|t| t.id == "voting"));
    let (t, text) = get_rules_topic("gameplay_loop").unwrap();
    assert_eq!(t.id, "gameplay_loop");
    assert!(!text.is_empty());
    assert!(text.to_lowercase().contains("night") || text.contains("Day"));
}

#[test]
fn list_characters_includes_pool() {
    let list = get_character_list();
    assert!(list.iter().any(|c| c.name == "Washerwoman"));
    assert!(list.iter().any(|c| c.name == "Imp"));
    assert!(list.iter().any(|c| c.character_type == "Minion"));
}

#[test]
fn host_state_exposes_st_choice_mode() {
    let (g, host, tokens) = start_scripted(
        15,
        vec![
            RoleAssignment::normal(SeatId(0), Character::Soldier),
            RoleAssignment::normal(SeatId(1), Character::Chef),
            RoleAssignment::normal(SeatId(2), Character::Empath),
            RoleAssignment::normal(SeatId(3), Character::Poisoner),
            RoleAssignment::normal(SeatId(4), Character::Imp),
        ],
        |_| {},
    );
    let view = get_host_state(&g, &host).unwrap();
    assert!(
        view.st_choice_mode.contains("HostFirst"),
        "got {}",
        view.st_choice_mode
    );
    let _ = tokens;
}
