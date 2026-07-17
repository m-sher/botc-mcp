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

/// Day-time Virgin/Spy registration is immediate (no day-blocking pause).
#[test]
fn virgin_spy_registration_immediate_no_day_pause() {
    let (mut g, host, tokens) = start_scripted(
        14,
        vec![
            RoleAssignment::normal(SeatId(0), Character::Virgin),
            RoleAssignment::normal(SeatId(1), Character::Spy),
            RoleAssignment::normal(SeatId(2), Character::Soldier),
            RoleAssignment::normal(SeatId(3), Character::Chef),
            RoleAssignment::normal(SeatId(4), Character::Imp),
        ],
        |opts| {
            // Force Spy to register as Townsfolk for Virgin.
            opts.registration_mode = botc_mcp::game::RegistrationMode::AlwaysMisreg;
        },
    );
    to_day1(&mut g, &host);
    // From Discussion: public path is atomic (open noms + Nominated + outcome).
    nominate(&mut g, &tokens[1], SeatId(0)).unwrap();
    assert!(
        g.pending_host.is_none(),
        "day registration must not set pending_host"
    );
    assert!(g
        .public_log
        .since(0)
        .iter()
        .any(|(_, e)| matches!(e, botc_mcp::comms::PublicEvent::Nominated { .. })));
    // AlwaysMisreg → Spy registers as Townsfolk → executed.
    assert!(!g.seats[1].alive);
}

/// Slayer→Recluse does not block the day; registration_mode controls outcome.
#[test]
fn slayer_recluse_immediate_registration_mode() {
    let (mut g, host, tokens) = start_scripted(
        36,
        vec![
            RoleAssignment::normal(SeatId(0), Character::Slayer),
            RoleAssignment::normal(SeatId(1), Character::Recluse),
            RoleAssignment::normal(SeatId(2), Character::Soldier),
            RoleAssignment::normal(SeatId(3), Character::Poisoner),
            RoleAssignment::normal(SeatId(4), Character::Imp),
        ],
        |opts| {
            opts.registration_mode = botc_mcp::game::RegistrationMode::AlwaysMisreg;
        },
    );
    to_day1(&mut g, &host);
    for s in &mut g.seats {
        s.poisoned = false;
    }
    botc_mcp::tools::day_action(
        &mut g,
        &tokens[0],
        botc_mcp::tools::DayActionPayload::Slay { target: SeatId(1) },
    )
    .unwrap();
    assert!(g.pending_host.is_none());
    assert!(!g.seats[1].alive, "AlwaysMisreg → Recluse dies as Demon");
    // Day remains fully usable.
    open_nominations(&mut g, &host).unwrap();
    botc_mcp::tools::end_nominations(&mut g, &host).unwrap();
    assert!(matches!(g.phase, Phase::Night { night: 2, .. }));
}

/// Night host pause still blocks day mutations.
#[test]
fn night_info_pending_blocks_day_mutations() {
    let (mut g, host, tokens) = start_scripted(
        37,
        vec![
            RoleAssignment::normal(SeatId(0), Character::Washerwoman),
            RoleAssignment::normal(SeatId(1), Character::Soldier),
            RoleAssignment::normal(SeatId(2), Character::Chef),
            RoleAssignment::normal(SeatId(3), Character::Poisoner),
            RoleAssignment::normal(SeatId(4), Character::Imp),
        ],
        |_| {},
    );
    // Reach WW host pause (after poisoner).
    while g.pending_night.is_some() {
        skip_night_action(&mut g, &host).unwrap();
    }
    assert!(
        matches!(
            g.pending_host,
            Some(botc_mcp::game::PendingHostDecision::NightInfo { .. })
        ),
        "expected night_info pause, got {:?}",
        g.pending_host
    );
    // Player nominate must fail while night ST decision pending.
    let err = nominate(&mut g, &tokens[0], SeatId(1));
    assert!(err.is_err());
    let _ = host;
}

/// Fortune Teller always pauses in host-first (not only when Recluse is picked).
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
