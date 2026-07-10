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
        g.say(SeatId(0), "hi".into()),
        Err(GameError::WrongPhase)
    ));

    // First night: silent.
    g.phase = Phase::FirstNight { cursor: 0 };
    assert!(matches!(
        g.say(SeatId(0), "psst, who is evil?".into()),
        Err(GameError::WrongPhase)
    ));

    // Later night: silent (dead or alive — no night chat).
    g.phase = Phase::Night {
        night: 2,
        cursor: 0,
    };
    assert!(matches!(
        g.say(SeatId(3), "night whisper".into()),
        Err(GameError::WrongPhase)
    ));

    // Day / discussion: allowed.
    g.phase = Phase::Day {
        day: 1,
        stage: DayStage::Discussion,
    };
    assert!(g.say(SeatId(0), "good morning".into()).is_ok());

    // Day / nominations: allowed (players defend / accuse).
    g.phase = Phase::Day {
        day: 1,
        stage: DayStage::Nominations,
    };
    assert!(g.say(SeatId(1), "I think P3 is the Imp".into()).is_ok());
}
