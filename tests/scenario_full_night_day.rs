//! Integration: full first night info + day nomination/vote path (+ N2 kill).

mod common;

use botc_mcp::comms::{PrivateMessage, PublicEvent};
use botc_mcp::game::{
    DayStage, NightActionPayload, NightStep, Phase, RoleAssignment, SeatId,
};
use botc_mcp::roles::Character;
use botc_mcp::tools::{
    get_private_state, get_public_log, get_public_state, night_action, say, skip_night_action,
};

use common::{advance_to_imp_kill, execute_seat, finish_night, start_scripted, to_day1};

fn night_results(game: &botc_mcp::game::Game, seat: SeatId) -> Vec<String> {
    game.private_inboxes
        .since(seat, 0)
        .into_iter()
        .filter_map(|(_, m)| match m {
            PrivateMessage::NightResult { text } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// Empath + Fortune Teller receive N1 info; day vote executes minion; N2 Imp kill.
#[test]
fn scenario_full_n1_info_day_vote_n2_kill() {
    // 5p: FortuneTeller, Empath, Soldier, Poisoner, Imp
    let (mut g, host, tokens) = start_scripted(
        1305,
        vec![
            RoleAssignment::normal(SeatId(0), Character::FortuneTeller),
            RoleAssignment::normal(SeatId(1), Character::Empath),
            RoleAssignment::normal(SeatId(2), Character::Soldier),
            RoleAssignment::normal(SeatId(3), Character::Poisoner),
            RoleAssignment::normal(SeatId(4), Character::Imp),
        ],
    );

    // --- First night: Poisoner choice, then FT pick-two, Empath auto ---
    let p = g.pending_night.as_ref().expect("poisoner");
    assert!(matches!(p.step, NightStep::Poisoner { seat: SeatId(3) }));
    night_action(
        &mut g,
        &tokens[3],
        NightActionPayload::PickOne { target: SeatId(2) }, // poison Soldier, not info roles
    )
    .unwrap();

    // Fortune Teller may be next (or after host-first ST info pauses).
    loop {
        if let Some(ph) = g.pending_host.as_ref() {
            // Skip pre-FT ST info pauses. FT host pause only happens after the pick below.
            let is_ft_result = matches!(
                ph,
                botc_mcp::game::PendingHostDecision::NightInfo { ability, .. }
                    if ability == "Fortune Teller"
            );
            if is_ft_result {
                break;
            }
            skip_night_action(&mut g, &host).unwrap();
            continue;
        }
        match g.pending_night.as_ref().map(|p| p.step) {
            Some(NightStep::FortuneTeller { seat: SeatId(0) }) => {
                night_action(
                    &mut g,
                    &tokens[0],
                    NightActionPayload::PickTwo {
                        a: SeatId(4), // Imp
                        b: SeatId(2), // Soldier
                    },
                )
                .unwrap();
                // Host-first: FT result is authored by ST; deliver the true yes via host_decide.
                if g.pending_host.is_some() {
                    botc_mcp::tools::host_decide(
                        &mut g,
                        &host,
                        botc_mcp::game::HostDecision::NightInfo {
                            text: "Fortune Teller: YES — at least one of P4 (seat 4) and P2 (seat 2) is a Demon? yes.".into(),
                        },
                    )
                    .unwrap();
                }
                break;
            }
            Some(_) => skip_night_action(&mut g, &host).unwrap(),
            None => panic!("stuck before FT: {:?}", g.phase),
        }
    }

    // Finish any remaining N1 wakes into Day 1.
    to_day1(&mut g, &host);

    // Empath neighbors: FT (good) and Soldier (good) if seats are 0-1-2-3-4 circle
    // Seat1 Empath neighbors 0 and 2 → both good → 0 evil.
    let empath_results = night_results(&g, SeatId(1));
    assert!(
        empath_results.iter().any(|t| t.contains("0 of") || t.contains("that 0")),
        "Empath should learn 0 evil neighbors: {empath_results:?}"
    );

    let ft_results = night_results(&g, SeatId(0));
    assert!(
        ft_results
            .iter()
            .any(|t| t.contains("YES") || t.contains("yes")),
        "FT reading Imp should ping yes: {ft_results:?}"
    );

    // Private state for Empath shows Empath face.
    let emp_priv = get_private_state(&g, &tokens[1], 0).unwrap();
    assert_eq!(emp_priv.character_label.as_deref(), Some("Empath"));
    assert!(emp_priv.alive);

    // Public chat during day.
    say(&mut g, &tokens[0], "I checked the Imp and Soldier".into()).unwrap();
    say(&mut g, &tokens[1], "Empath zero".into()).unwrap();

    let pub_day = get_public_state(&g, &tokens[0]).unwrap();
    assert!(pub_day.winner.is_none());
    assert!(pub_day.phase.contains("Day"));

    // Day vote: nominate Poisoner (not Imp) → execution → night 2.
    execute_seat(&mut g, &host, &tokens, SeatId(0), SeatId(3));
    assert!(!g.seats[3].alive);
    assert!(g.seats[4].alive);
    assert!(g.winner.is_none());
    assert!(matches!(g.phase, Phase::Night { night: 2, .. }));

    // N2: Imp kills Empath (no Monk in bag).
    advance_to_imp_kill(&mut g, &host, &tokens, SeatId(0), None);
    night_action(
        &mut g,
        &tokens[4],
        NightActionPayload::PickOne { target: SeatId(1) },
    )
    .unwrap();
    finish_night(&mut g, &host);

    assert!(!g.seats[1].alive);
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

    let log = get_public_log(&g, &host, 0).unwrap();
    assert!(
        log.iter().any(|(_, e)| matches!(
            e,
            PublicEvent::DiedInNight { seats } if seats.contains(&SeatId(1))
        )),
        "Empath should be in DiedInNight: {log:?}"
    );
    assert!(
        log.iter()
            .any(|(_, e)| matches!(e, PublicEvent::Chat { .. })),
        "public chat events expected: {log:?}"
    );

    // Day 2: execute Imp → Good.
    execute_seat(&mut g, &host, &tokens, SeatId(0), SeatId(4));
    assert!(!g.seats[4].alive);
    common::assert_good_wins_demon_dead(&g);
}
