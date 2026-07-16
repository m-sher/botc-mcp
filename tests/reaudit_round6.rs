//! Re-audit round 6: #32 nominate validation order, #33 bluff refilter <7, #34 lie queue day→night.

use botc_mcp::comms::PublicEvent;
use botc_mcp::game::{DayStage, Game, Phase, RoleAssignment, SeatId, StartOpts};
use botc_mcp::roles::Character;
use botc_mcp::tools::{host_queue_lie, nominate, open_nominations, skip_night_action, vote};

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

fn five_tb() -> Vec<RoleAssignment> {
    vec![
        RoleAssignment::normal(SeatId(0), Character::Soldier),
        RoleAssignment::normal(SeatId(1), Character::Chef),
        RoleAssignment::normal(SeatId(2), Character::Empath),
        RoleAssignment::normal(SeatId(3), Character::Poisoner),
        RoleAssignment::normal(SeatId(4), Character::Imp),
    ]
}

/// #32: illegal self-nominate from Discussion must not open Nominations.
#[test]
fn illegal_self_nominate_from_discussion_does_not_open_noms() {
    let (mut g, host, tokens) = start_scripted(3201, five_tb(), |_| {});
    to_day1(&mut g, &host);

    let log_len = g.public_log.since(0).len();
    let err = nominate(&mut g, &tokens[0], SeatId(0)).expect_err("self-nom");
    assert!(
        format!("{err:?}").contains("cannot nominate yourself")
            || format!("{err}").contains("cannot nominate yourself"),
        "unexpected err: {err:?}"
    );
    assert!(
        matches!(
            g.phase,
            Phase::Day {
                day: 1,
                stage: DayStage::Discussion
            }
        ),
        "phase must stay Discussion after rejected self-nom; got {:?}",
        g.phase
    );
    assert_eq!(
        g.public_log.since(0).len(),
        log_len,
        "rejected nominate must not emit public events"
    );
}

/// #32: dead nominator from Discussion must not open Nominations.
#[test]
fn dead_nominator_from_discussion_does_not_open_noms() {
    let (mut g, host, tokens) = start_scripted(3202, five_tb(), |_| {});
    to_day1(&mut g, &host);

    // Kill seat 0 via execution (majority + host end_nominations).
    open_nominations(&mut g, &host).unwrap();
    nominate(&mut g, &tokens[1], SeatId(0)).unwrap();
    // Nominator (seat 1) already auto-yes; remaining seats vote yes.
    for (i, t) in tokens.iter().enumerate() {
        if i == 1 {
            continue;
        }
        vote(&mut g, t, SeatId(0), true).unwrap();
    }
    botc_mcp::tools::end_nominations(&mut g, &host).unwrap();
    // Night 2 → Day 2 Discussion.
    while g.pending_night.is_some() || g.pending_host.is_some() {
        skip_night_action(&mut g, &host).unwrap();
    }
    assert!(
        matches!(
            g.phase,
            Phase::Day {
                day: 2,
                stage: DayStage::Discussion
            }
        ),
        "expected Day 2 Discussion, got {:?}",
        g.phase
    );
    assert!(!g.seats[0].alive);

    let log_len = g.public_log.since(0).len();
    let err = nominate(&mut g, &tokens[0], SeatId(1)).expect_err("dead nom");
    assert!(
        format!("{err:?}").to_lowercase().contains("dead")
            || format!("{err}").to_lowercase().contains("dead"),
        "unexpected err: {err:?}"
    );
    assert!(
        matches!(
            g.phase,
            Phase::Day {
                day: 2,
                stage: DayStage::Discussion
            }
        ),
        "phase must stay Discussion; got {:?}",
        g.phase
    );
    assert_eq!(g.public_log.since(0).len(), log_len);
}

/// #32: legal nominate from Discussion still auto-opens.
#[test]
fn legal_nominate_from_discussion_still_auto_opens() {
    let (mut g, host, tokens) = start_scripted(3203, five_tb(), |_| {});
    to_day1(&mut g, &host);
    nominate(&mut g, &tokens[0], SeatId(1)).unwrap();
    assert!(matches!(
        g.phase,
        Phase::Day {
            day: 1,
            stage: DayStage::Nominations
        }
    ));
    assert!(g.current_nomination.is_some());
}

/// #33: drunk_faces override on <7p must not fabricate demon bluffs.
#[test]
fn small_game_drunk_face_override_no_fabricated_bluffs() {
    // 6p TB with Drunk: no bluff trio per rules.
    let face = Character::Investigator;
    let (g, _, _) = start_scripted(
        3301,
        vec![
            RoleAssignment::drunk(SeatId(0), Character::Washerwoman).unwrap(),
            RoleAssignment::normal(SeatId(1), Character::Soldier),
            RoleAssignment::normal(SeatId(2), Character::Chef),
            RoleAssignment::normal(SeatId(3), Character::Saint),
            RoleAssignment::normal(SeatId(4), Character::Poisoner),
            RoleAssignment::normal(SeatId(5), Character::Imp),
        ],
        |opts| {
            opts.drunk_faces = Some(vec![(SeatId(0), face)]);
        },
    );
    assert_eq!(g.seats[0].believed_character, Some(face));
    assert!(
        g.demon_bluffs.is_empty(),
        "5–6 player games have no bluffs; drunk_faces must not invent any: {:?}",
        g.demon_bluffs
    );
}

/// #33: 7+ still refilters and keeps 3 bluffs after drunk face override.
#[test]
fn seven_plus_drunk_face_override_still_three_bluffs() {
    let face = Character::Investigator;
    let (g, _, _) = start_scripted(
        3302,
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
    assert_eq!(g.demon_bluffs.len(), 3);
    assert!(!g.demon_bluffs.contains(&face));
}

/// #34: lies queued during day survive enter_night for the upcoming night.
#[test]
fn host_lie_queued_during_day_survives_enter_night() {
    let (mut g, host, tokens) = start_scripted(3401, five_tb(), |_| {});
    to_day1(&mut g, &host);

    host_queue_lie(&mut g, &host, "day-queued lie for night 2".into()).unwrap();
    assert_eq!(g.host_lie_queue.len(), 1);

    // Force end day with no noms → night 2.
    open_nominations(&mut g, &host).unwrap();
    // Single nom with all no votes, then exhaust by having everyone nominate someone already used?
    // Simpler: end_nominations host path.
    botc_mcp::tools::end_nominations(&mut g, &host).unwrap();

    assert!(
        matches!(g.phase, Phase::Night { night: 2, .. }),
        "expected Night 2, got {:?}",
        g.phase
    );
    assert_eq!(
        g.host_lie_queue.len(),
        1,
        "day-queued lie must survive enter_night"
    );
    assert_eq!(
        g.host_lie_queue.front().map(|s| s.as_str()),
        Some("day-queued lie for night 2")
    );

    // Advance to day 2 dawn — unused lie clears.
    let _ = &tokens;
    while g.pending_night.is_some() || g.pending_host.is_some() {
        skip_night_action(&mut g, &host).unwrap();
    }
    assert!(g.host_lie_queue.is_empty(), "unused lie must clear at dawn");
}

/// #34 / #28: last closed nomination with no majority → NoExecution.
#[test]
fn auto_end_last_nom_all_no_is_no_execution() {
    let (mut g, host, tokens) = start_scripted(3402, five_tb(), |_| {});
    to_day1(&mut g, &host);

    // Only one nominator can nominate if we use all living as nominees carefully:
    // seat 0 nominates 1; after that 4 living nominees remain but 4 nominators remain —
    // for a true "last nom" path: open, and have each of 0..3 nominate, all no; last
    // is 4→someone. Easier: one nomination then end via host after making no further
    // legal noms impossible — all living have nominated or all targets used.
    //
    // 5 living, chain of 5 noms all voting no (same as round5 test, stricter asserts).
    nominate(&mut g, &tokens[0], SeatId(1)).unwrap();
    // Remaining living vote no (nominator auto-yes already recorded).
    for t in tokens.iter().skip(1) {
        vote(&mut g, t, SeatId(1), false).unwrap();
    }
    for (by, target) in [(1u8, 2u8), (2, 3), (3, 4), (4, 0)] {
        nominate(&mut g, &tokens[by as usize], SeatId(target)).unwrap();
        for t in &tokens {
            if g.current_nomination.is_none() {
                break;
            }
            if !matches!(
                g.phase,
                Phase::Day {
                    day: 1,
                    stage: DayStage::Nominations
                }
            ) {
                break;
            }
            // Nominator already auto-yes; ignore double-vote errors.
            let _ = vote(&mut g, t, SeatId(target), false);
        }
    }

    assert!(matches!(g.phase, Phase::Night { night: 2, .. }));
    assert!(g.seats.iter().all(|s| s.alive));
    assert!(g
        .public_log
        .since(0)
        .iter()
        .any(|(_, e)| matches!(e, PublicEvent::NoExecution)));
}
