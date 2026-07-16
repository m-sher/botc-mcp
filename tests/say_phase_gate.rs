//! `say` (public chat) is a **day** activity: the engine rejects it at night, in
//! the lobby, and after the game ends. Players are silent at night.

use botc_mcp::game::{DayStage, Game, GameError, Phase, SeatId};

fn game7() -> Game {
    let names: Vec<String> = (0..7).map(|i| format!("P{i}")).collect();
    Game::create(names, 42).expect("create game").game
}

#[test]
fn say_is_day_only() {
    let mut g = game7();

    // Lobby (fresh game, not started): no talking.
    assert!(matches!(
        g.say(SeatId(0), "hi".into(), None),
        Err(GameError::WrongPhase)
    ));

    // First night: silent.
    g.phase = Phase::FirstNight { cursor: 0 };
    assert!(matches!(
        g.say(SeatId(0), "psst, who is evil?".into(), None),
        Err(GameError::WrongPhase)
    ));

    // Later night: silent (dead or alive — no night chat).
    g.phase = Phase::Night {
        night: 2,
        cursor: 0,
    };
    assert!(matches!(
        g.say(SeatId(3), "night whisper".into(), None),
        Err(GameError::WrongPhase)
    ));

    // Day / discussion: allowed.
    g.phase = Phase::Day {
        day: 1,
        stage: DayStage::Discussion,
    };
    assert!(g.say(SeatId(0), "good morning".into(), None).is_ok());

    // Day / nominations: allowed (players defend / accuse).
    g.phase = Phase::Day {
        day: 1,
        stage: DayStage::Nominations,
    };
    assert!(g
        .say(SeatId(1), "I think P3 is the Imp".into(), None)
        .is_ok());
}

#[test]
fn directed_say_is_public_and_caps_per_player() {
    use botc_mcp::comms::PublicEvent;
    use botc_mcp::game::DIRECTED_SAY_CAP;

    let mut g = game7();
    g.phase = Phase::Day {
        day: 1,
        stage: DayStage::Discussion,
    };

    g.say(SeatId(0), "P1 what did you see?".into(), Some(SeatId(1)))
        .unwrap();
    assert_eq!(g.pending_directed_wake, Some(SeatId(1)));
    assert_eq!(g.directed_say_sent[0], 1);
    assert_eq!(g.directed_say_received[1], 1);
    assert!(g.public_log.since(0).iter().any(|(_, e)| matches!(
        e,
        PublicEvent::Chat {
            seat: SeatId(0),
            to: Some(SeatId(1)),
            ..
        }
    )));

    // Cap: fill received on seat 1 to the limit, then one more fails.
    g.pending_directed_wake = None;
    g.directed_say_received[1] = DIRECTED_SAY_CAP;
    let err = g
        .say(SeatId(2), "again".into(), Some(SeatId(1)))
        .unwrap_err();
    assert!(
        format!("{err}").to_lowercase().contains("cap")
            || format!("{err:?}").to_lowercase().contains("cap"),
        "expected receive cap error, got {err:?}"
    );

    // Cap: fill sent on seat 0, then further directed fails.
    g.directed_say_received[1] = 0;
    g.directed_say_sent[0] = DIRECTED_SAY_CAP;
    let err = g
        .say(SeatId(0), "too many".into(), Some(SeatId(3)))
        .unwrap_err();
    assert!(
        format!("{err}").to_lowercase().contains("cap")
            || format!("{err:?}").to_lowercase().contains("cap"),
        "expected send cap error, got {err:?}"
    );

    // Undirected still works at send cap.
    assert!(g.say(SeatId(0), "to the table".into(), None).is_ok());
    // Self-target rejected.
    let err = g.say(SeatId(2), "me".into(), Some(SeatId(2))).unwrap_err();
    assert!(format!("{err}").to_lowercase().contains("yourself"));
}

#[test]
fn directed_say_rejected_outside_discussion() {
    let mut g = game7();
    g.phase = Phase::Day {
        day: 1,
        stage: DayStage::Nominations,
    };
    let err = g.say(SeatId(0), "hey".into(), Some(SeatId(1))).unwrap_err();
    assert!(
        format!("{err}").to_lowercase().contains("discussion")
            || format!("{err:?}").to_lowercase().contains("discussion"),
        "expected discussion-only error, got {err:?}"
    );
    // Cap not charged.
    assert_eq!(g.directed_say_sent[0], 0);
    assert_eq!(g.directed_say_received[1], 0);
    // Undirected still ok in nominations.
    assert!(g.say(SeatId(0), "to the table".into(), None).is_ok());
}

#[test]
fn directed_say_may_wake_dead_seat() {
    use botc_mcp::harness::scheduler::{plan_ticks, PlayerTask, SchedTarget};

    let mut g = game7();
    g.phase = Phase::Day {
        day: 1,
        stage: DayStage::Discussion,
    };
    g.seats[3].alive = false;
    g.seats[3].ghost_vote_available = true;

    g.say(SeatId(0), "ghost, any last words?".into(), Some(SeatId(3)))
        .unwrap();
    assert_eq!(g.pending_directed_wake, Some(SeatId(3)));
    assert_eq!(g.directed_say_received[3], 1);

    let plan = plan_ticks(&g, 0, 0);
    match &plan[0] {
        SchedTarget::Player {
            seat,
            task: PlayerTask::Discuss { directed_reply, .. },
        } => {
            assert_eq!(*seat, SeatId(3));
            assert!(*directed_reply, "dead seat still gets directed reply wake");
        }
        t => panic!("expected directed Discuss for ghost, got {t:?}"),
    }
}
