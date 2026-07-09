//! Re-audit round 5: #27 starpass pause leak, #28 day auto-end, #29 bluffs vs drunk faces, #30 lie queue clear.

use botc_mcp::game::{
    DayStage, Game, HostDecision, NightActionPayload, NightStep, Phase, RoleAssignment, SeatId,
    StartOpts,
};
use botc_mcp::roles::Character;
use botc_mcp::tools::{
    get_public_state, host_decide, host_queue_lie, nominate, open_nominations, skip_night_action,
    vote,
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

fn advance_to_imp_kill(g: &mut Game, host: &botc_mcp::auth::Token, tokens: &[botc_mcp::auth::Token]) {
    loop {
        let Some(p) = g.pending_night.clone() else {
            panic!("no pending {:?}", g.phase);
        };
        match p.step {
            NightStep::DemonKill { .. } => break,
            NightStep::Poisoner { seat } => {
                botc_mcp::tools::night_action(
                    g,
                    &tokens[seat.0 as usize],
                    NightActionPayload::PickOne { target: SeatId(0) },
                )
                .unwrap();
            }
            _ => skip_night_action(g, host).unwrap(),
        }
    }
}

/// #27: during starpass host pause, get_public_state shows Imp still alive.
#[test]
fn starpass_pause_imp_still_public_alive() {
    let (mut g, host, tokens) = start_scripted(
        2701,
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
    advance_to_imp_kill(&mut g, &host, &tokens);

    botc_mcp::tools::night_action(
        &mut g,
        &tokens[1],
        NightActionPayload::PickOne { target: SeatId(1) },
    )
    .unwrap();
    assert!(g.pending_host.is_some());
    assert!(g.seats[1].alive);

    let pub_view = get_public_state(&g, &tokens[0]).unwrap();
    let imp_pub = pub_view
        .seats
        .iter()
        .find(|s| s.id == SeatId(1))
        .expect("imp seat");
    assert!(
        imp_pub.alive,
        "public state must not show Imp dead during starpass pause"
    );

    host_decide(
        &mut g,
        &host,
        HostDecision::StarpassPick {
            minion: SeatId(2),
        },
    )
    .unwrap();
    assert!(!g.seats[1].alive);
    assert_eq!(g.seats[2].true_character, Some(Character::Imp));
}

/// #28: day ends without host end_nominations once nominations are exhausted.
#[test]
fn day_auto_ends_when_nominations_exhausted() {
    // 5 living: each nominates once (chain). All votes no → NoExecution → night 2.
    let (mut g, host, tokens) = start_scripted(
        2801,
        vec![
            RoleAssignment::normal(SeatId(0), Character::Soldier),
            RoleAssignment::normal(SeatId(1), Character::Chef),
            RoleAssignment::normal(SeatId(2), Character::Empath),
            RoleAssignment::normal(SeatId(3), Character::Poisoner),
            RoleAssignment::normal(SeatId(4), Character::Imp),
        ],
        |_| {},
    );
    to_day1(&mut g, &host);

    // Nominate from Discussion (auto-open).
    nominate(&mut g, &tokens[0], SeatId(1)).unwrap();
    assert!(matches!(
        g.phase,
        Phase::Day {
            day: 1,
            stage: DayStage::Nominations
        }
    ));
    // All living vote no → close; more noms remain.
    for t in &tokens {
        vote(&mut g, t, SeatId(1), false).unwrap();
    }
    assert!(
        matches!(g.phase, Phase::Day { day: 1, .. }),
        "day should continue after first nom: {:?}",
        g.phase
    );

    // Exhaust remaining nominators: 1→2, 2→3, 3→4, 4→0 (each target once).
    let chain = [(1u8, 2u8), (2, 3), (3, 4), (4, 0)];
    for (by, target) in chain {
        assert!(
            matches!(g.phase, Phase::Day { day: 1, stage: DayStage::Nominations }),
            "expected still day 1 noms before {by}->{target}, got {:?}",
            g.phase
        );
        nominate(&mut g, &tokens[by as usize], SeatId(target)).unwrap();
        for t in &tokens {
            // After last vote of last nom, day may already have advanced.
            if !matches!(g.phase, Phase::Day { day: 1, stage: DayStage::Nominations }) {
                break;
            }
            if g.current_nomination.is_none() {
                break;
            }
            let _ = vote(&mut g, t, SeatId(target), false);
        }
    }

    // All five closed noms had 0 yes → no execution; everyone still alive; enter night 2.
    assert!(
        matches!(g.phase, Phase::Night { night: 2, .. }),
        "day must auto-end into Night 2 with no execution; got {:?}",
        g.phase
    );
    assert!(g.executed_today.is_none());
    assert!(g.seats.iter().all(|s| s.alive), "no one should have died");
    assert!(
        g.public_log
            .since(0)
            .iter()
            .any(|(_, e)| matches!(e, botc_mcp::comms::PublicEvent::NoExecution)),
        "expected NoExecution in public log"
    );
}

/// #29: drunk face override is excluded from demon bluffs.
#[test]
fn drunk_face_override_not_in_demon_bluffs() {
    // 7p so bluffs are generated. Drunk face override to a TF not in bag.
    let face = Character::Investigator; // not in this bag
    let (g, _, _) = start_scripted(
        2901,
        vec![
            RoleAssignment::drunk(SeatId(0), Character::Washerwoman).unwrap(),
            RoleAssignment::normal(SeatId(1), Character::Soldier),
            RoleAssignment::normal(SeatId(2), Character::Chef),
            RoleAssignment::normal(SeatId(3), Character::Empath),
            RoleAssignment::normal(SeatId(4), Character::Monk),
            RoleAssignment::normal(SeatId(5), Character::Poisoner),
            RoleAssignment::normal(SeatId(6), Character::Imp),
        ],
        |opts| {
            opts.drunk_faces = Some(vec![(SeatId(0), face)]);
        },
    );
    assert_eq!(g.seats[0].believed_character, Some(face));
    assert!(
        !g.demon_bluffs.contains(&face),
        "bluffs must not include drunk face {face:?}: {:?}",
        g.demon_bluffs
    );
    assert_eq!(g.demon_bluffs.len(), 3);
    for b in &g.demon_bluffs {
        assert_eq!(b.team(), botc_mcp::roles::Team::Good);
    }
}

/// #30: host_lie_queue cleared at dawn.
#[test]
fn host_lie_queue_cleared_at_dawn() {
    let (mut g, host, _) = start_scripted(
        3001,
        vec![
            RoleAssignment::normal(SeatId(0), Character::Soldier),
            RoleAssignment::normal(SeatId(1), Character::Chef),
            RoleAssignment::normal(SeatId(2), Character::Empath),
            RoleAssignment::normal(SeatId(3), Character::Poisoner),
            RoleAssignment::normal(SeatId(4), Character::Imp),
        ],
        |_| {},
    );
    host_queue_lie(&mut g, &host, "unused lie for night 1".into()).unwrap();
    assert_eq!(g.host_lie_queue.len(), 1);
    to_day1(&mut g, &host);
    assert!(
        g.host_lie_queue.is_empty(),
        "lie queue must clear at dawn"
    );
}
