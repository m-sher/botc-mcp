//! Re-audit round 4: #21 poisoned Spy+Virgin, ST policy, #26 cleanup covered in unit tests.

use botc_mcp::game::ability::register;
use botc_mcp::game::{
    DayStage, Game, HostDecision, NightActionPayload, NightStep, Phase, RegistrationMode,
    RoleAssignment, SeatId, StartOpts,
};
use botc_mcp::roles::{Character, CharacterType};
use botc_mcp::tools::{
    host_decide, host_queue_lie, nominate, open_nominations, skip_night_action,
};

fn names(n: usize) -> Vec<String> {
    (0..n).map(|i| format!("P{i}")).collect()
}

fn start_scripted(
    seed: u64,
    salt: u64,
    assignments: Vec<RoleAssignment>,
    opts_extra: impl FnOnce(&mut StartOpts),
) -> (Game, botc_mcp::auth::Token, Vec<botc_mcp::auth::Token>) {
    let lobby = Game::create_with_salt(names(assignments.len()), seed, salt).unwrap();
    let host = lobby.host_token.clone();
    let tokens = lobby.player_tokens.clone();
    let mut g = lobby.game;
    let mut opts = StartOpts {
        assignments: Some(assignments),
        ..Default::default()
    };
    opts_extra(&mut opts);
    g.start_game(&host, opts).unwrap();
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

/// #21 poisoned Spy nominating Virgin never triggers execution.
#[test]
fn poisoned_spy_never_triggers_virgin() {
    for seed in 0..40u64 {
        let (mut g, host, tokens) = start_scripted(
            seed,
            seed.wrapping_mul(17),
            vec![
                RoleAssignment::normal(SeatId(0), Character::Virgin),
                RoleAssignment::normal(SeatId(1), Character::Spy),
                RoleAssignment::normal(SeatId(2), Character::Soldier),
                RoleAssignment::normal(SeatId(3), Character::Poisoner),
                RoleAssignment::normal(SeatId(4), Character::Imp),
            ],
            |_| {},
        );
        to_day1(&mut g, &host);
        g.seats[1].poisoned = true;
        // Direct registration check for many labels.
        for i in 0..16u32 {
            assert!(
                !register::registers_as_townsfolk(&g, SeatId(1), &format!("v:{seed}:{i}")),
                "poisoned Spy must never register as Townsfolk (seed {seed} i {i})"
            );
        }
        open_nominations(&mut g, &host).unwrap();
        nominate(&mut g, &tokens[1], SeatId(0)).unwrap();
        assert!(
            g.seats[1].alive,
            "poisoned Spy must not be executed by Virgin (seed {seed})"
        );
        assert!(
            g.current_nomination.is_some(),
            "vote should proceed (seed {seed})"
        );
        // Close the day cleanly for next seed.
        let _ = tokens;
    }
}

/// Mayor attack requires host_decide; skip defaults to nobody dies when others live.
#[test]
fn mayor_pending_requires_host_decide_skip_nobody() {
    let (mut g, host, tokens) = start_scripted(
        900,
        0,
        vec![
            RoleAssignment::normal(SeatId(0), Character::Mayor),
            RoleAssignment::normal(SeatId(1), Character::Soldier),
            RoleAssignment::normal(SeatId(2), Character::Chef),
            RoleAssignment::normal(SeatId(3), Character::Poisoner),
            RoleAssignment::normal(SeatId(4), Character::Imp),
        ],
        |_| {},
    );
    to_day1(&mut g, &host);
    open_nominations(&mut g, &host).unwrap();
    botc_mcp::tools::end_nominations(&mut g, &host).unwrap();
    assert!(matches!(g.phase, Phase::Night { night: 2, .. }));

    // Drive to Imp kill; poison someone other than Mayor so bounce stays active.
    loop {
        let Some(p) = g.pending_night.clone() else {
            if g.pending_host.is_some() {
                break;
            }
            panic!("stuck {:?}", g.phase);
        };
        match p.step {
            NightStep::Poisoner { seat } => {
                botc_mcp::tools::night_action(
                    &mut g,
                    &tokens[seat.0 as usize],
                    NightActionPayload::PickOne { target: SeatId(2) },
                )
                .unwrap();
            }
            NightStep::DemonKill { .. } => break,
            _ => skip_night_action(&mut g, &host).unwrap(),
        }
    }
    assert!(
        !g.seats[0].ability_disabled(),
        "Mayor must not be poisoned for bounce test"
    );
    botc_mcp::tools::night_action(
        &mut g,
        &tokens[4],
        NightActionPayload::PickOne { target: SeatId(0) },
    )
    .unwrap();
    assert!(g.pending_host.is_some(), "Mayor attack must pause for host");
    assert!(g.seats[0].alive);
    // Skip → nobody dies.
    skip_night_action(&mut g, &host).unwrap();
    assert!(g.seats[0].alive, "Mayor kept on skip default");
    assert!(!g.deaths_tonight.contains(&SeatId(0)));
}

/// Starpass pending + explicit host pick.
#[test]
fn starpass_pending_host_pick() {
    let (mut g, host, tokens) = start_scripted(
        901,
        0,
        vec![
            RoleAssignment::normal(SeatId(0), Character::Soldier),
            RoleAssignment::normal(SeatId(1), Character::Imp),
            RoleAssignment::normal(SeatId(2), Character::Poisoner),
            RoleAssignment::normal(SeatId(3), Character::Spy),
            RoleAssignment::normal(SeatId(4), Character::Chef),
        ],
        |_| {},
    );
    to_day1(&mut g, &host);
    open_nominations(&mut g, &host).unwrap();
    botc_mcp::tools::end_nominations(&mut g, &host).unwrap();

    loop {
        let Some(p) = g.pending_night.clone() else {
            panic!("no pending {:?}", g.phase);
        };
        match p.step {
            NightStep::DemonKill { .. } => break,
            _ => skip_night_action(&mut g, &host).unwrap(),
        }
    }
    botc_mcp::tools::night_action(
        &mut g,
        &tokens[1],
        NightActionPayload::PickOne { target: SeatId(1) },
    )
    .unwrap();
    // #27: Imp stays publicly alive during host starpass pause.
    assert!(g.seats[1].alive, "Imp must stay alive until host resolves starpass");
    match g.pending_host.clone().expect("starpass pending") {
        botc_mcp::game::PendingHostDecision::StarpassPick { minions, .. } => {
            assert!(minions.contains(&SeatId(2)) && minions.contains(&SeatId(3)));
        }
        other => panic!("{other:?}"),
    }
    host_decide(
        &mut g,
        &host,
        HostDecision::StarpassPick {
            minion: SeatId(3),
        },
    )
    .unwrap();
    assert!(!g.seats[1].alive, "Imp dies when starpass completes");
    assert_eq!(g.seats[3].true_character, Some(Character::Imp));
    assert_eq!(g.seats[2].true_character, Some(Character::Poisoner));
}

/// StartOpts red_herring override.
#[test]
fn start_opts_red_herring_override() {
    let (g, _, _) = start_scripted(
        902,
        0,
        vec![
            RoleAssignment::normal(SeatId(0), Character::FortuneTeller),
            RoleAssignment::normal(SeatId(1), Character::Soldier),
            RoleAssignment::normal(SeatId(2), Character::Chef),
            RoleAssignment::normal(SeatId(3), Character::Poisoner),
            RoleAssignment::normal(SeatId(4), Character::Imp),
        ],
        |opts| {
            opts.red_herring = Some(SeatId(2));
        },
    );
    assert_eq!(g.red_herring, Some(SeatId(2)));
}

/// RegistrationMode::AlwaysTrue → Spy always true minion for Investigator type owner.
#[test]
fn registration_mode_always_true_spy() {
    let (g, _, _) = start_scripted(
        903,
        0,
        vec![
            RoleAssignment::normal(SeatId(0), Character::Investigator),
            RoleAssignment::normal(SeatId(1), Character::Spy),
            RoleAssignment::normal(SeatId(2), Character::Soldier),
            RoleAssignment::normal(SeatId(3), Character::Poisoner),
            RoleAssignment::normal(SeatId(4), Character::Imp),
        ],
        |opts| {
            opts.registration_mode = RegistrationMode::AlwaysTrue;
        },
    );
    for i in 0..32u32 {
        let lab = format!("at:{i}");
        assert_eq!(
            register::register_as_type_owner(&g, SeatId(1), CharacterType::Minion, &lab),
            Some(Character::Spy),
            "AlwaysTrue Spy never hides"
        );
        assert!(
            register::register_evil(&g, SeatId(1), &format!("ev:{i}")),
            "AlwaysTrue Spy always evil"
        );
        assert!(
            !register::registers_as_townsfolk(&g, SeatId(1), &format!("tf:{i}")),
            "AlwaysTrue Spy never Townsfolk for Virgin"
        );
    }
}

/// Host queue lie consumed by disabled info role.
#[test]
fn host_queue_lie_used_for_disabled_empath() {
    use botc_mcp::comms::PrivateMessage;
    use botc_mcp::game::ability::resolve_night_step;

    let (mut g, host, _) = start_scripted(
        904,
        0,
        vec![
            RoleAssignment::normal(SeatId(0), Character::Empath),
            RoleAssignment::normal(SeatId(1), Character::Soldier),
            RoleAssignment::normal(SeatId(2), Character::Chef),
            RoleAssignment::normal(SeatId(3), Character::Poisoner),
            RoleAssignment::normal(SeatId(4), Character::Imp),
        ],
        |_| {},
    );
    g.seats[0].poisoned = true;
    host_queue_lie(&mut g, &host, "Empath: canned lie 0".into()).unwrap();
    resolve_night_step(&mut g, NightStep::Empath { seat: SeatId(0) }, None).unwrap();
    let text = g
        .private_inboxes
        .since(SeatId(0), 0)
        .into_iter()
        .rev()
        .find_map(|(_, m)| match m {
            PrivateMessage::NightResult { text } => Some(text),
            _ => None,
        })
        .expect("result");
    assert_eq!(text, "Empath: canned lie 0");
    assert!(g.host_lie_queue.is_empty());
}

/// Direct register unit: Ability disabled Spy is never Townsfolk for Virgin (#21).
#[test]
fn registers_as_townsfolk_disabled_spy_unit() {
    let (mut g, _, _) = start_scripted(
        905,
        0,
        vec![
            RoleAssignment::normal(SeatId(0), Character::Virgin),
            RoleAssignment::normal(SeatId(1), Character::Spy),
            RoleAssignment::normal(SeatId(2), Character::Soldier),
            RoleAssignment::normal(SeatId(3), Character::Poisoner),
            RoleAssignment::normal(SeatId(4), Character::Imp),
        ],
        |opts| {
            opts.registration_mode = RegistrationMode::AlwaysMisreg;
        },
    );
    // Healthy AlwaysMisreg Spy always townsfolk.
    assert!(register::registers_as_townsfolk(&g, SeatId(1), "m0"));
    g.seats[1].poisoned = true;
    for i in 0..20u32 {
        assert!(
            !register::registers_as_townsfolk(&g, SeatId(1), &format!("m{i}")),
            "disabled Spy false even under AlwaysMisreg"
        );
    }
}
