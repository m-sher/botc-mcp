//! Focused regression tests for GitHub issue #1 (TB rules audit remediation).

use botc_mcp::comms::PrivateMessage;
use botc_mcp::game::ability::{register, try_demon_kill, KillResult};
use botc_mcp::game::night::build_first_night_queue;
use botc_mcp::game::{
    Game, NightActionPayload, NightStep, Phase, RoleAssignment, SeatId, StartOpts, DayStage,
};
use botc_mcp::roles::Character;
use botc_mcp::tools::{
    close_vote, nominate, open_nominations, skip_night_action, vote,
};

fn names(n: usize) -> Vec<String> {
    (0..n).map(|i| format!("P{i}")).collect()
}

fn start_scripted(seed: u64, salt: u64, assignments: Vec<RoleAssignment>) -> (Game, botc_mcp::auth::Token, Vec<botc_mcp::auth::Token>) {
    let lobby = Game::create_with_salt(names(assignments.len()), seed, salt).unwrap();
    let host = lobby.host_token.clone();
    let tokens = lobby.player_tokens.clone();
    let mut g = lobby.game;
    g.start_game(
        &host,
        StartOpts {
            assignments: Some(assignments),
        },
    )
    .unwrap();
    (g, host, tokens)
}

fn to_day1(g: &mut Game, host: &botc_mcp::auth::Token) {
    while g.pending_night.is_some() {
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

/// 1. Drunk face not in bag (also covered in setup unit tests).
#[test]
fn drunk_face_not_duplicating_in_bag_many_seeds() {
    use botc_mcp::game::setup::build_bag;
    use botc_mcp::rng::SeededRng;
    use botc_mcp::roles::CharacterType;

    let mut found = 0;
    for seed in 0..200u64 {
        let bag = build_bag(&SeededRng::from_seed(seed), 9).unwrap();
        let set: std::collections::HashSet<_> = bag.bag_set.iter().copied().collect();
        for a in &bag.assignments {
            if a.true_character == Character::Drunk {
                found += 1;
                let face = a.believed_character.unwrap();
                assert_eq!(face.character_type(), CharacterType::Townsfolk);
                assert!(
                    !set.contains(&face),
                    "seed {seed}: Drunk face {face:?} collides with bag"
                );
            }
        }
    }
    assert!(found > 0);
}

/// 2. Two Empaths (real + Drunk face) both queued on first night.
#[test]
fn two_empaths_real_and_drunk_face_both_wake() {
    let (g, host, tokens) = start_scripted(
        50,
        0,
        vec![
            RoleAssignment::normal(SeatId(0), Character::Empath),
            RoleAssignment::drunk(SeatId(1), Character::Empath).unwrap(),
            RoleAssignment::normal(SeatId(2), Character::Poisoner),
            RoleAssignment::normal(SeatId(3), Character::Soldier),
            RoleAssignment::normal(SeatId(4), Character::Imp),
        ],
    );
    let q = build_first_night_queue(&g);
    let empath_seats: Vec<_> = q
        .iter()
        .filter_map(|s| match s {
            NightStep::Empath { seat } => Some(*seat),
            _ => None,
        })
        .collect();
    assert!(
        empath_seats.contains(&SeatId(0)) && empath_seats.contains(&SeatId(1)),
        "both Empath faces must wake: {empath_seats:?} in {q:?}"
    );

    // Drive night; both should get Empath night results.
    let mut g = g;
    while g.pending_night.is_some() {
        skip_night_action(&mut g, &host).unwrap();
    }
    let _ = tokens;
    let results0 = g
        .private_inboxes
        .since(SeatId(0), 0)
        .iter()
        .filter(|(_, m)| matches!(m, PrivateMessage::NightResult { .. }))
        .count();
    let results1 = g
        .private_inboxes
        .since(SeatId(1), 0)
        .iter()
        .filter(|(_, m)| matches!(m, PrivateMessage::NightResult { .. }))
        .count();
    assert!(results0 >= 1, "real Empath needs a night result");
    assert!(results1 >= 1, "Drunk-face Empath needs a night result");
}

/// 3. Drunk face Ravenkeeper dies at night → gets wake.
#[test]
fn drunk_face_ravenkeeper_dies_at_night_wakes() {
    let (mut g, host, tokens) = start_scripted(
        51,
        0,
        vec![
            RoleAssignment::drunk(SeatId(0), Character::Ravenkeeper).unwrap(),
            RoleAssignment::normal(SeatId(1), Character::Soldier),
            RoleAssignment::normal(SeatId(2), Character::Chef),
            RoleAssignment::normal(SeatId(3), Character::Poisoner),
            RoleAssignment::normal(SeatId(4), Character::Imp),
        ],
    );
    to_day1(&mut g, &host);
    // Enter night 2 via host end path: open noms, no execution.
    open_nominations(&mut g, &host).unwrap();
    botc_mcp::tools::end_nominations(&mut g, &host).unwrap();
    assert!(matches!(g.phase, Phase::Night { night: 2, .. }));

    // Advance to Imp kill.
    loop {
        let p = g.pending_night.as_ref().expect("pending");
        match p.step {
            NightStep::DemonKill { .. } => break,
            _ => skip_night_action(&mut g, &host).unwrap(),
        }
    }
    botc_mcp::tools::night_action(
        &mut g,
        &tokens[4],
        NightActionPayload::PickOne { target: SeatId(0) },
    )
    .unwrap();

    assert!(
        !g.seats[0].alive,
        "Drunk-face RK should have died to Imp"
    );
    let pending = g.pending_night.as_ref().expect("RK wake pending");
    assert!(
        matches!(pending.step, NightStep::Ravenkeeper { seat: SeatId(0) }),
        "expected Ravenkeeper wake for face-RK, got {:?}",
        pending.step
    );
}

/// Poisoned true Ravenkeeper also wakes (disabled path).
#[test]
fn poisoned_ravenkeeper_still_wakes_on_death() {
    let (mut g, host, tokens) = start_scripted(
        52,
        0,
        vec![
            RoleAssignment::normal(SeatId(0), Character::Ravenkeeper),
            RoleAssignment::normal(SeatId(1), Character::Soldier),
            RoleAssignment::normal(SeatId(2), Character::Chef),
            RoleAssignment::normal(SeatId(3), Character::Poisoner),
            RoleAssignment::normal(SeatId(4), Character::Imp),
        ],
    );
    // N1: poison the RK.
    botc_mcp::tools::night_action(
        &mut g,
        &tokens[3],
        NightActionPayload::PickOne { target: SeatId(0) },
    )
    .unwrap();
    to_day1(&mut g, &host);
    assert!(g.seats[0].poisoned);
    open_nominations(&mut g, &host).unwrap();
    botc_mcp::tools::end_nominations(&mut g, &host).unwrap();

    loop {
        let p = g.pending_night.as_ref().expect("pending");
        match p.step {
            NightStep::DemonKill { .. } => break,
            NightStep::Poisoner { .. } => {
                // Re-poison RK so still disabled at death.
                botc_mcp::tools::night_action(
                    &mut g,
                    &tokens[3],
                    NightActionPayload::PickOne { target: SeatId(0) },
                )
                .unwrap();
            }
            _ => skip_night_action(&mut g, &host).unwrap(),
        }
    }
    assert!(g.seats[0].ability_disabled());
    botc_mcp::tools::night_action(
        &mut g,
        &tokens[4],
        NightActionPayload::PickOne { target: SeatId(0) },
    )
    .unwrap();
    let pending = g.pending_night.as_ref().expect("RK wake");
    assert!(matches!(
        pending.step,
        NightStep::Ravenkeeper { seat: SeatId(0) }
    ));
}

/// 4. Mayor bounce never kills Imp (or minions).
#[test]
fn mayor_bounce_never_kills_imp() {
    // 5 seats: Mayor, Soldier, Chef, Poisoner, Imp — only good bounce targets are Soldier/Chef.
    let (mut g, _host, _tokens) = start_scripted(
        53,
        0,
        vec![
            RoleAssignment::normal(SeatId(0), Character::Mayor),
            RoleAssignment::normal(SeatId(1), Character::Soldier),
            RoleAssignment::normal(SeatId(2), Character::Chef),
            RoleAssignment::normal(SeatId(3), Character::Poisoner),
            RoleAssignment::normal(SeatId(4), Character::Imp),
        ],
    );
    // Clear N1 poison; simulate night 2 kill chain directly.
    for s in &mut g.seats {
        s.poisoned = false;
    }
    g.night_cursor = 3;
    for _ in 0..40 {
        // Re-alive all if needed and re-try bounce.
        for s in &mut g.seats {
            if s.id != SeatId(0) {
                s.alive = true;
            }
        }
        g.seats[0].alive = true;
        g.deaths_tonight.clear();
        g.winner = None;
        g.phase = Phase::Night { night: 2, cursor: 3 };
        let r = try_demon_kill(&mut g, SeatId(4), SeatId(0));
        match r {
            KillResult::Died(victim) => {
                assert_ne!(victim, SeatId(4), "must never bounce onto Imp");
                assert_ne!(victim, SeatId(3), "must never bounce onto Minion");
                assert!(g.seats[4].alive, "Imp must remain alive after bounce");
                assert!(
                    victim == SeatId(1) || victim == SeatId(2),
                    "bounce victim should be good townsfolk, got {victim:?}"
                );
            }
            KillResult::Survived => {
                // Soldier immune / empty candidates ok for some RNG; continue
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}

/// 5. Ghost vote: living finish voting first does not auto-close before dead votes.
#[test]
fn ghost_vote_not_auto_closed_when_only_living_voted() {
    let (mut g, host, tokens) = start_scripted(
        54,
        0,
        vec![
            RoleAssignment::normal(SeatId(0), Character::Soldier),
            RoleAssignment::normal(SeatId(1), Character::Chef),
            RoleAssignment::normal(SeatId(2), Character::Empath),
            RoleAssignment::normal(SeatId(3), Character::Poisoner),
            RoleAssignment::normal(SeatId(4), Character::Imp),
        ],
    );
    to_day1(&mut g, &host);
    // Dead seat with ghost vote remaining.
    g.seats[2].alive = false;
    g.seats[2].ghost_vote_available = true;

    open_nominations(&mut g, &host).unwrap();
    nominate(&mut g, &tokens[0], SeatId(4)).unwrap();

    // All living vote — must NOT auto-close while dead ghost voter has not voted.
    for i in [0usize, 1, 3, 4] {
        vote(&mut g, &tokens[i], SeatId(4), i % 2 == 0).unwrap();
    }
    assert!(
        g.current_nomination.is_some(),
        "vote window must stay open for ghost voter"
    );

    // Dead casts no — now auto-close is allowed.
    vote(&mut g, &tokens[2], SeatId(4), false).unwrap();
    assert!(g.current_nomination.is_none(), "should auto-close after ghost voted");

    // Host can still force-close early on a fresh nom.
    nominate(&mut g, &tokens[1], SeatId(3)).unwrap();
    vote(&mut g, &tokens[0], SeatId(3), true).unwrap();
    close_vote(&mut g, &host).unwrap();
    assert!(g.current_nomination.is_none());
}

/// 6. Poisoned Virgin first nom spends ability without executing.
#[test]
fn poisoned_virgin_first_nom_spends_ability() {
    let (mut g, host, tokens) = start_scripted(
        55,
        0,
        vec![
            RoleAssignment::normal(SeatId(0), Character::Soldier),
            RoleAssignment::normal(SeatId(1), Character::Virgin),
            RoleAssignment::normal(SeatId(2), Character::Chef),
            RoleAssignment::normal(SeatId(3), Character::Poisoner),
            RoleAssignment::normal(SeatId(4), Character::Imp),
        ],
    );
    to_day1(&mut g, &host);
    g.seats[1].poisoned = true;
    assert!(g.seats[1].ability_disabled());

    open_nominations(&mut g, &host).unwrap();
    nominate(&mut g, &tokens[0], SeatId(1)).unwrap();

    assert!(g.seats[1].virgin_ability_used, "ability spent even when poisoned");
    assert!(g.seats[0].alive, "nominator not executed while Virgin disabled");
    assert!(g.current_nomination.is_some());
    assert_eq!(g.executed_today, None);
}

/// 7. Chef: single registration per seat → Evil–Recluse–Evil cannot yield count 1.
#[test]
fn chef_single_reg_evil_recluse_evil_never_one() {
    // Circle: Imp, Recluse, Poisoner, Soldier, Chef — pairs Imp-Recluse, Recluse-Poisoner,
    // Poisoner-Soldier, Soldier-Chef, Chef-Imp. Only the two Imp/Poisoner–Recluse edges
    // can form evil pairs depending on Recluse registration (0 or 2 when Recluse evil).
    let (g, _, _) = start_scripted(
        56,
        0,
        vec![
            RoleAssignment::normal(SeatId(0), Character::Imp),
            RoleAssignment::normal(SeatId(1), Character::Recluse),
            RoleAssignment::normal(SeatId(2), Character::Poisoner),
            RoleAssignment::normal(SeatId(3), Character::Soldier),
            RoleAssignment::normal(SeatId(4), Character::Chef),
        ],
    );
    // Probe many night_cursor labels by temporarily adjusting and calling chef_true_count
    // via resolve — use private API by re-running register consistency check inline.
    for cursor in 0..64u32 {
        let evil: Vec<bool> = g
            .seats
            .iter()
            .map(|s| {
                let lab = format!("chef_reg:{}:{}", cursor, s.id.0);
                register::register_evil(&g, s.id, &lab)
            })
            .collect();
        let n = evil.len();
        let mut count = 0u8;
        for i in 0..n {
            if evil[i] && evil[(i + 1) % n] {
                count += 1;
            }
        }
        // With single reg: Recluse either evil (→ 2 pairs with Imp and Poisoner) or good (→ 0).
        // Imp-Poisoner not adjacent. Soldier/Chef good.
        assert!(
            count == 0 || count == 2,
            "cursor {cursor}: impossible chef count {count} with single reg; evil={evil:?}"
        );
    }
}

/// 8. Seed default not 0 when omitted from MCP create_game — covered in mcp_server unit tests.
/// 9. Substream with salt differs — covered in rng_tests.

#[test]
fn host_state_exposes_secret_salt_not_private() {
    let lobby = Game::create_with_salt(names(5), 7, 99).unwrap();
    let host = lobby.host_token.clone();
    let player = lobby.player_tokens[0].clone();
    let g = lobby.game;
    let hv = botc_mcp::tools::get_host_state(&g, &host).unwrap();
    assert_eq!(hv.secret_salt, 99);
    assert_eq!(hv.seed, 7);
    let pv = botc_mcp::tools::get_private_state(&g, &player, 0).unwrap();
    // Private view has no seed/salt fields (compile-time); just ensure it works.
    assert_eq!(pv.seat, SeatId(0));
}
