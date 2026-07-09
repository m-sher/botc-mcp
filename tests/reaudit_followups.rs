//! Follow-up re-audit fixes (#3–#20).

use botc_mcp::comms::PrivateMessage;
use botc_mcp::game::ability::{register, resolve_night_step, try_demon_kill, KillResult};
use botc_mcp::game::night::build_other_night_queue;
use botc_mcp::game::{
    DayStage, Game, NightActionPayload, NightStep, Phase, RoleAssignment, SeatId, StartOpts,
};
use botc_mcp::roles::{Character, CharacterType};
use botc_mcp::tools::{
    day_action, nominate, open_nominations, pass_vote, skip_night_action, vote, DayActionPayload,
};
use rand::Rng;

fn names(n: usize) -> Vec<String> {
    (0..n).map(|i| format!("P{i}")).collect()
}

fn start_scripted(
    seed: u64,
    salt: u64,
    assignments: Vec<RoleAssignment>,
) -> (Game, botc_mcp::auth::Token, Vec<botc_mcp::auth::Token>) {
    let lobby = Game::create_with_salt(names(assignments.len()), seed, salt).unwrap();
    let host = lobby.host_token.clone();
    let tokens = lobby.player_tokens.clone();
    let mut g = lobby.game;
    g.start_game(
        &host,
        StartOpts {
            assignments: Some(assignments),
                ..Default::default()
            },
    )
    .unwrap();
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

/// #3 Acting seat never appears in WW pair seats.
#[test]
fn washerwoman_pair_excludes_acting_seat() {
    let (mut g, host, tokens) = start_scripted(
        100,
        0,
        vec![
            RoleAssignment::normal(SeatId(0), Character::Washerwoman),
            RoleAssignment::normal(SeatId(1), Character::Soldier),
            RoleAssignment::normal(SeatId(2), Character::Chef),
            RoleAssignment::normal(SeatId(3), Character::Poisoner),
            RoleAssignment::normal(SeatId(4), Character::Imp),
        ],
    );
    // Drive until day so Washerwoman has resolved (skip N1).
    // Capture WW night result by draining night carefully.
    // First pending is Poisoner; skip until Washerwoman result is in inbox.
    while g.pending_night.is_some() || g.pending_host.is_some() {
        skip_night_action(&mut g, &host).unwrap();
    }
    let msgs = g.private_inboxes.since(SeatId(0), 0);
    let text = msgs
        .iter()
        .find_map(|(_, m)| match m {
            botc_mcp::comms::PrivateMessage::NightResult { text } if text.contains("Washerwoman") => {
                Some(text.clone())
            }
            _ => None,
        })
        .expect("Washerwoman night result");
    // Must mention two seats that are not seat 0.
    assert!(
        !text.contains("seat 0"),
        "acting WW seat must not appear in pair: {text}"
    );
    let _ = tokens;
}

/// #4 same seed+salt → same bag / same substream.
#[test]
fn same_seed_and_salt_same_bag_and_substream() {
    use botc_mcp::game::setup::build_bag;
    use botc_mcp::rng::SeededRng;

    let seed = 4242u64;
    let salt = 777u64;
    let a = SeededRng::from_seed_and_salt(seed, salt);
    let b = SeededRng::from_seed_and_salt(seed, salt);
    let bag_a = build_bag(&a, 7).unwrap();
    let bag_b = build_bag(&b, 7).unwrap();
    assert_eq!(bag_a.bag_set, bag_b.bag_set);
    assert_eq!(
        bag_a
            .assignments
            .iter()
            .map(|x| (x.seat, x.true_character, x.believed_character))
            .collect::<Vec<_>>(),
        bag_b
            .assignments
            .iter()
            .map(|x| (x.seat, x.true_character, x.believed_character))
            .collect::<Vec<_>>()
    );
    let x: u64 = a.substream("washerwoman").gen();
    let y: u64 = b.substream("washerwoman").gen();
    assert_eq!(x, y);

    // tools path with explicit salt
    let mut store1 = botc_mcp::store::GameStore::new();
    let mut store2 = botc_mcp::store::GameStore::new();
    let r1 = botc_mcp::tools::create_game(&mut store1, names(5), seed, Some(salt)).unwrap();
    let r2 = botc_mcp::tools::create_game(&mut store2, names(5), seed, Some(salt)).unwrap();
    let g1 = store1.get_mut(r1.game_id).unwrap();
    let g2 = store2.get_mut(r2.game_id).unwrap();
    assert_eq!(g1.secret_salt, salt);
    assert_eq!(g2.secret_salt, salt);
    assert_eq!(
        g1.rng.substream("setup").gen::<u64>(),
        g2.rng.substream("setup").gen::<u64>()
    );
}

/// #5 Spy misreg as Townsfolk prefers in-play TF tokens (not WW face when excluded).
#[test]
fn spy_type_owner_prefers_in_play_and_excludes_actor_face() {
    let (g, _, _) = start_scripted(
        101,
        0,
        vec![
            RoleAssignment::normal(SeatId(0), Character::Washerwoman),
            RoleAssignment::normal(SeatId(1), Character::Spy),
            RoleAssignment::normal(SeatId(2), Character::Soldier),
            RoleAssignment::normal(SeatId(3), Character::Chef),
            RoleAssignment::normal(SeatId(4), Character::Imp),
        ],
    );
    let opts = register::TypeOwnerOpts {
        acting_seat: Some(SeatId(0)),
    };
    let in_play_tf = [Character::Washerwoman, Character::Soldier, Character::Chef];
    let mut named = std::collections::HashSet::new();
    for i in 0..128u32 {
        let lab = format!("spy_tf:{i}");
        if let Some(c) = register::register_as_type_owner_with(
            &g,
            SeatId(1),
            CharacterType::Townsfolk,
            &lab,
            opts,
        ) {
            assert_ne!(c, Character::Washerwoman, "must not name acting WW");
            named.insert(c);
            // Prefer in-play: when we name something, it should often be Soldier/Chef.
            assert!(
                in_play_tf.contains(&c) || all_tf_contains(c),
                "unexpected token {c:?}"
            );
        }
    }
    // Should have named at least one in-play non-WW townsfolk across trials.
    assert!(
        named.contains(&Character::Soldier) || named.contains(&Character::Chef),
        "expected in-play TF tokens, got {named:?}"
    );
}

fn all_tf_contains(c: Character) -> bool {
    botc_mcp::roles::all_townsfolk().contains(&c)
}

/// #6 pass_vote auto-close path covered in day_tests; ensure ghost retained after pass + later yes.
#[test]
fn pass_then_ghost_yes_on_later_nomination() {
    let (mut g, host, tokens) = start_scripted(
        102,
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
    g.seats[2].alive = false;
    g.seats[2].ghost_vote_available = true;

    open_nominations(&mut g, &host).unwrap();
    nominate(&mut g, &tokens[0], SeatId(4)).unwrap();
    for i in [0usize, 1, 3, 4] {
        vote(&mut g, &tokens[i], SeatId(4), false).unwrap();
    }
    pass_vote(&mut g, &tokens[2]).unwrap();
    assert!(g.seats[2].ghost_vote_available);
    assert!(g.current_nomination.is_none());

    nominate(&mut g, &tokens[1], SeatId(3)).unwrap();
    vote(&mut g, &tokens[2], SeatId(3), true).unwrap();
    assert!(!g.seats[2].ghost_vote_available);
}

/// #7 Mayor bounce can kill Minion (host kill_other).
#[test]
fn mayor_bounce_can_kill_minion() {
    use botc_mcp::game::{HostDecision, MayorRedirectChoice};

    let (mut g, host, _) = start_scripted(
        103,
        0,
        vec![
            RoleAssignment::normal(SeatId(0), Character::Mayor),
            RoleAssignment::normal(SeatId(1), Character::Soldier), // immune
            RoleAssignment::normal(SeatId(2), Character::Poisoner),
            RoleAssignment::normal(SeatId(3), Character::Spy),
            RoleAssignment::normal(SeatId(4), Character::Imp),
        ],
    );
    for s in &mut g.seats {
        s.poisoned = false;
    }
    g.night_cursor = 1;
    g.phase = Phase::Night { night: 2, cursor: 1 };
    assert_eq!(
        try_demon_kill(&mut g, SeatId(4), SeatId(0)),
        KillResult::NeedsHost
    );
    g.host_decide(
        &host,
        HostDecision::MayorRedirect {
            choice: MayorRedirectChoice::KillOther {
                target: SeatId(2),
            },
        },
    )
    .unwrap();
    assert!(!g.seats[2].alive, "host bounce onto Poisoner minion");
    assert!(g.seats[0].alive, "Mayor survives bounce");
}

/// #8 Slayer kills Recluse when Storyteller registers them as Demon.
#[test]
fn slayer_can_kill_recluse_registering_as_demon() {
    let (mut g, host, tokens) = start_scripted(
        104,
        0,
        vec![
            RoleAssignment::normal(SeatId(0), Character::Slayer),
            RoleAssignment::normal(SeatId(1), Character::Recluse),
            RoleAssignment::normal(SeatId(2), Character::Soldier),
            RoleAssignment::normal(SeatId(3), Character::Poisoner),
            RoleAssignment::normal(SeatId(4), Character::Imp),
        ],
    );
    to_day1(&mut g, &host);
    g.seats[0].poisoned = false;
    // Host-first: Recluse-as-Demon is ST discretion.
    day_action(
        &mut g,
        &tokens[0],
        DayActionPayload::Slay { target: SeatId(1) },
    )
    .unwrap();
    assert!(matches!(
        g.pending_host,
        Some(botc_mcp::game::PendingHostDecision::SlayerRecluseReg { .. })
    ));
    botc_mcp::tools::host_decide(
        &mut g,
        &host,
        botc_mcp::game::HostDecision::Registration { register: true },
    )
    .unwrap();
    assert!(!g.seats[1].alive, "Recluse dies when ST registers as Demon");
    assert!(g.seats[4].alive, "Imp still alive; no SW path from Recluse");
}

/// #12 spent ghost rejects all votes (also day_tests).
#[test]
fn spent_ghost_rejects_all_votes() {
    let (mut g, host, tokens) = start_scripted(
        105,
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
    g.seats[2].alive = false;
    g.seats[2].ghost_vote_available = false; // already spent
    open_nominations(&mut g, &host).unwrap();
    nominate(&mut g, &tokens[0], SeatId(4)).unwrap();
    assert!(vote(&mut g, &tokens[2], SeatId(4), true).is_err());
    assert!(vote(&mut g, &tokens[2], SeatId(4), false).is_err());
    assert!(pass_vote(&mut g, &tokens[2]).is_err());
}

/// #13 other-night queue does not pre-list Ravenkeeper from deaths_tonight.
#[test]
fn other_night_queue_omits_prelisted_ravenkeeper() {
    let (mut g, _, _) = start_scripted(
        106,
        0,
        vec![
            RoleAssignment::normal(SeatId(0), Character::Ravenkeeper),
            RoleAssignment::normal(SeatId(1), Character::Soldier),
            RoleAssignment::normal(SeatId(2), Character::Chef),
            RoleAssignment::normal(SeatId(3), Character::Poisoner),
            RoleAssignment::normal(SeatId(4), Character::Imp),
        ],
    );
    g.seats[0].alive = false;
    g.deaths_tonight = vec![SeatId(0)];
    let q = build_other_night_queue(&g);
    assert!(
        !q.iter()
            .any(|s| matches!(s, NightStep::Ravenkeeper { .. })),
        "RK must not be pre-queued from deaths_tonight: {q:?}"
    );
}

/// #11 / #19 stable mix differs by salt/label; pin golden mix(1,2,"setup").
#[test]
fn mix_stable_and_salt_sensitive() {
    use botc_mcp::rng::mix;
    assert_eq!(mix(9, 8, "a"), mix(9, 8, "a"));
    assert_ne!(mix(9, 8, "a"), mix(9, 9, "a"));
    assert_ne!(mix(9, 8, "a"), mix(9, 8, "b"));
    assert_eq!(mix(1, 2, "setup"), 0x7351_1a5b_7da1_f833);
}

/// #15 Ravenkeeper never learns their own face from Spy misreg.
#[test]
fn ravenkeeper_spy_misreg_excludes_viewer_face() {
    let (g, _, _) = start_scripted(
        201,
        0,
        vec![
            RoleAssignment::normal(SeatId(0), Character::Ravenkeeper),
            RoleAssignment::normal(SeatId(1), Character::Spy),
            RoleAssignment::normal(SeatId(2), Character::Soldier),
            RoleAssignment::normal(SeatId(3), Character::Chef),
            RoleAssignment::normal(SeatId(4), Character::Imp),
        ],
    );
    for i in 0..200u32 {
        let lab = format!("rk_spy:{i}");
        let shown = register::register_character(&g, SeatId(1), &lab, Some(SeatId(0)))
            .expect("Spy registers");
        assert_ne!(
            shown,
            Character::Ravenkeeper,
            "RK viewer must never see Ravenkeeper token from Spy misreg (draw {i})"
        );
    }
}

fn last_night_result(g: &Game, seat: SeatId) -> String {
    g.private_inboxes
        .since(seat, 0)
        .into_iter()
        .rev()
        .find_map(|(_, m)| match m {
            PrivateMessage::NightResult { text } => Some(text.clone()),
            _ => None,
        })
        .expect("night result")
}

/// #16 Investigator sole Spy always gets a real minion (Spy) — never pure lie.
#[test]
fn investigator_sole_spy_always_truthful_minion() {
    for seed in 300..380u64 {
        let (mut g, _, _) = start_scripted(
            seed,
            seed,
            vec![
                RoleAssignment::normal(SeatId(0), Character::Investigator),
                RoleAssignment::normal(SeatId(1), Character::Spy),
                RoleAssignment::normal(SeatId(2), Character::Soldier),
                RoleAssignment::normal(SeatId(3), Character::Chef),
                RoleAssignment::normal(SeatId(4), Character::Imp),
            ],
        );
        // Direct resolve — skip full night so Poisoner cannot disable Inv.
        for s in &mut g.seats {
            s.poisoned = false;
        }
        g.night_cursor = seed as usize % 7;
        resolve_night_step(&mut g, NightStep::Investigator { seat: SeatId(0) }, None).unwrap();
        let text = last_night_result(&g, SeatId(0));
        assert!(
            text.contains("Spy"),
            "sole Spy must be named as minion token, got: {text}"
        );
        assert!(
            text.contains("seat 1"),
            "correct Spy seat should appear in pair: {text}"
        );
    }
}

/// #16 Librarian sole Recluse is truthful (cannot hide as sole outsider).
#[test]
fn librarian_sole_recluse_truthful() {
    let (mut g, _, _) = start_scripted(
        203,
        0,
        vec![
            RoleAssignment::normal(SeatId(0), Character::Librarian),
            RoleAssignment::normal(SeatId(1), Character::Recluse),
            RoleAssignment::normal(SeatId(2), Character::Soldier),
            RoleAssignment::normal(SeatId(3), Character::Poisoner),
            RoleAssignment::normal(SeatId(4), Character::Imp),
        ],
    );
    for s in &mut g.seats {
        s.poisoned = false;
    }
    resolve_night_step(&mut g, NightStep::Librarian { seat: SeatId(0) }, None).unwrap();
    let text = last_night_result(&g, SeatId(0));
    assert!(
        !text.contains("0 Outsiders"),
        "sole Recluse must not yield 0 outsiders: {text}"
    );
    assert!(
        text.contains("Recluse"),
        "sole Recluse should be named: {text}"
    );
}

/// #16 Librarian with no outsiders gets 0 message (not a lie pair).
#[test]
fn librarian_zero_outsiders_message() {
    let (mut g, _, _) = start_scripted(
        204,
        0,
        vec![
            RoleAssignment::normal(SeatId(0), Character::Librarian),
            RoleAssignment::normal(SeatId(1), Character::Soldier),
            RoleAssignment::normal(SeatId(2), Character::Chef),
            RoleAssignment::normal(SeatId(3), Character::Poisoner),
            RoleAssignment::normal(SeatId(4), Character::Imp),
        ],
    );
    for s in &mut g.seats {
        s.poisoned = false;
    }
    resolve_night_step(&mut g, NightStep::Librarian { seat: SeatId(0) }, None).unwrap();
    let text = last_night_result(&g, SeatId(0));
    assert!(
        text.contains("0 Outsiders"),
        "expected 0 Outsiders message, got: {text}"
    );
}

/// #16 sole TF Washerwoman still gets a pair naming their character (not pure lie).
#[test]
fn washerwoman_sole_tf_names_self_token() {
    let (mut g, _, _) = start_scripted(
        205,
        0,
        vec![
            RoleAssignment::normal(SeatId(0), Character::Washerwoman),
            RoleAssignment::normal(SeatId(1), Character::Recluse),
            RoleAssignment::normal(SeatId(2), Character::Butler),
            RoleAssignment::normal(SeatId(3), Character::Poisoner),
            RoleAssignment::normal(SeatId(4), Character::Imp),
        ],
    );
    for s in &mut g.seats {
        s.poisoned = false;
    }
    resolve_night_step(&mut g, NightStep::Washerwoman { seat: SeatId(0) }, None).unwrap();
    let text = last_night_result(&g, SeatId(0));
    assert!(
        text.contains("is the Washerwoman"),
        "sole TF should name Washerwoman token: {text}"
    );
}

/// #17 poisoned Spy/Recluse cannot misregister (unit covered in register.rs; integration).
#[test]
fn poisoned_spy_register_character_always_spy() {
    let (mut g, _, _) = start_scripted(
        206,
        0,
        vec![
            RoleAssignment::normal(SeatId(0), Character::Ravenkeeper),
            RoleAssignment::normal(SeatId(1), Character::Spy),
            RoleAssignment::normal(SeatId(2), Character::Soldier),
            RoleAssignment::normal(SeatId(3), Character::Chef),
            RoleAssignment::normal(SeatId(4), Character::Imp),
        ],
    );
    g.seats[1].poisoned = true;
    for i in 0..40u32 {
        let lab = format!("pspy:{i}");
        assert_eq!(
            register::register_character(&g, SeatId(1), &lab, Some(SeatId(0))),
            Some(Character::Spy)
        );
        assert!(register::register_evil(&g, SeatId(1), &format!("pe:{i}")));
        assert_eq!(
            register::register_as_type_owner(&g, SeatId(1), CharacterType::Townsfolk, &format!("pt:{i}")),
            None
        );
    }
}

/// #18 Mayor bounce host kill_mayor when no soft bounce targets.
#[test]
fn mayor_bounce_empty_kills_mayor() {
    use botc_mcp::game::{HostDecision, MayorRedirectChoice};

    let (mut g, host, _) = start_scripted(
        207,
        0,
        vec![
            RoleAssignment::normal(SeatId(0), Character::Mayor),
            RoleAssignment::normal(SeatId(1), Character::Soldier), // immune
            RoleAssignment::normal(SeatId(2), Character::Monk),
            RoleAssignment::normal(SeatId(3), Character::Poisoner),
            RoleAssignment::normal(SeatId(4), Character::Imp),
        ],
    );
    // Kill everyone except Mayor + Imp so only Imp remains as living_other.
    g.seats[1].alive = false;
    g.seats[2].alive = false;
    g.seats[3].alive = false;
    for s in &mut g.seats {
        s.poisoned = false;
    }
    g.night_cursor = 1;
    g.deaths_tonight.clear();
    assert_eq!(
        try_demon_kill(&mut g, SeatId(4), SeatId(0)),
        KillResult::NeedsHost
    );
    g.host_decide(
        &host,
        HostDecision::MayorRedirect {
            choice: MayorRedirectChoice::KillMayor,
        },
    )
    .unwrap();
    assert!(!g.seats[0].alive, "Mayor dies on host kill_mayor");
}

/// #16 healthy Investigator path still works with two minions.
#[test]
fn investigator_healthy_names_a_minion_token() {
    let (mut g, _, _) = start_scripted(
        208,
        0,
        vec![
            RoleAssignment::normal(SeatId(0), Character::Investigator),
            RoleAssignment::normal(SeatId(1), Character::Spy),
            RoleAssignment::normal(SeatId(2), Character::Soldier),
            RoleAssignment::normal(SeatId(3), Character::Poisoner),
            RoleAssignment::normal(SeatId(4), Character::Imp),
        ],
    );
    for s in &mut g.seats {
        s.poisoned = false;
    }
    resolve_night_step(&mut g, NightStep::Investigator { seat: SeatId(0) }, None).unwrap();
    let text = last_night_result(&g, SeatId(0));
    assert!(
        text.contains("Spy") || text.contains("Poisoner"),
        "should name a minion token: {text}"
    );
}

/// Direct register_character viewer exclusion (#15) via resolve_night_step Ravenkeeper.
#[test]
fn ravenkeeper_resolve_excludes_own_token_from_spy() {
    let (mut g, _, _) = start_scripted(
        209,
        0,
        vec![
            RoleAssignment::normal(SeatId(0), Character::Ravenkeeper),
            RoleAssignment::normal(SeatId(1), Character::Spy),
            RoleAssignment::normal(SeatId(2), Character::Soldier),
            RoleAssignment::normal(SeatId(3), Character::Chef),
            RoleAssignment::normal(SeatId(4), Character::Imp),
        ],
    );
    g.seats[0].alive = false;
    g.night_cursor = 0;
    for trial in 0..80u32 {
        // Clear prior results for seat 0 by reading high watermark after each resolve.
        let before = g.private_inboxes.since(SeatId(0), 0).len();
        g.rng = botc_mcp::rng::SeededRng::from_seed_and_salt(500 + trial as u64, trial as u64);
        resolve_night_step(
            &mut g,
            NightStep::Ravenkeeper { seat: SeatId(0) },
            Some(&NightActionPayload::PickOne {
                target: SeatId(1),
            }),
        )
        .unwrap();
        let msgs = g.private_inboxes.since(SeatId(0), 0);
        let text = msgs[before..]
            .iter()
            .find_map(|(_, m)| match m {
                PrivateMessage::NightResult { text } => Some(text.as_str()),
                _ => None,
            })
            .expect("RK result");
        // Message is "Ravenkeeper: {seat} is the {character}." — assert character body only.
        assert!(
            !text.contains("is the Ravenkeeper"),
            "Spy must not show as Ravenkeeper to RK viewer: {text}"
        );
    }
}
