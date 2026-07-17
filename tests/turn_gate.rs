//! Turn gate: phase-advancing tools may only be called by a seat that currently
//! holds the matching `await_turn` wake. Prevents an always-on session from
//! advancing game state out of turn (#66 P3 race: Host ended nominations while a
//! player was still mid-nomination).

use botc_mcp::harness::socket::{SocketClient, SocketServer};
use botc_mcp::mcp_server;
use serde_json::{json, Value};
use std::path::PathBuf;

fn sc(v: &Value) -> Value {
    v.get("structuredContent")
        .cloned()
        .unwrap_or_else(|| v.clone())
}

fn new_game(client: &mut SocketClient) -> (u64, String) {
    let created = client
        .call_tool(
            "create_game",
            json!({ "names": ["A","B","C","D","E"], "seed": 7 }),
        )
        .expect("create_game");
    let created = sc(&created);
    let game_id = created
        .get("game_id")
        .and_then(|v| v.as_u64())
        .expect("game_id");
    let host = created
        .get("host_token")
        .and_then(|v| v.as_str())
        .expect("host_token")
        .to_string();
    (game_id, host)
}

#[test]
fn end_nominations_blocked_when_host_holds_no_endday_wake() {
    let store = mcp_server::new_shared_store();
    let path = PathBuf::from(format!("/tmp/botc-gate-{}.sock", uuid::Uuid::new_v4()));
    let server = SocketServer::start(store.clone(), &path).expect("bind");
    let mut client = SocketClient::connect(&path).expect("connect");
    let (game_id, host) = new_game(&mut client);

    // The Host has never polled await_turn, so it holds no EndDay wake. The gate
    // must reject the phase-advance BEFORE it reaches the engine.
    let res = client.call_tool(
        "end_nominations",
        json!({ "game_id": game_id, "token": host, "host_token": host }),
    );
    let msg = match res {
        Err(e) => e,
        Ok(v) => v.to_string(),
    };
    assert!(
        msg.contains("not your turn"),
        "out-of-turn end_nominations must be gate-blocked, got: {msg}"
    );

    server.stop();
}

#[test]
fn end_nominations_allowed_once_host_holds_the_endday_wake() {
    use botc_mcp::game::{DayStage, GameId, Phase};

    let store = mcp_server::new_shared_store();
    let path = PathBuf::from(format!("/tmp/botc-gate-ok-{}.sock", uuid::Uuid::new_v4()));
    let server = SocketServer::start(store.clone(), &path).expect("bind");
    let mut client = SocketClient::connect(&path).expect("connect");
    let (game_id, host) = new_game(&mut client);

    // Put the game in Day/Nominations with every living seat having already
    // nominated, so `plan_ticks` schedules Host(EndDay{in_discussion:false}) —
    // i.e. it genuinely IS the Host's turn to close the day.
    {
        let mut st = store.lock().unwrap();
        let g = st.get_mut(GameId(game_id)).expect("game");
        g.phase = Phase::Day {
            day: 1,
            stage: DayStage::Nominations,
        };
        g.pending_host = None;
        g.pending_night = None;
        g.current_nomination = None;
        for s in &mut g.seats {
            s.alive = true;
        }
        g.day_nominators = g.seats.iter().map(|s| s.id).collect();
    }

    // Host long-polls and must receive the end-day wake.
    let wake = client
        .call_tool(
            "await_turn",
            json!({ "game_id": game_id, "token": host, "budget_secs": 2 }),
        )
        .expect("await_turn");
    let wake = sc(&wake);
    assert_eq!(
        wake.get("kind").and_then(|v| v.as_str()),
        Some("host_end_day"),
        "host should be woken to end the day: {wake}"
    );

    // Now that the Host holds the matching wake, the gate must let it through to
    // the engine (result may be Ok or an engine-level outcome, but NOT the gate).
    let res = client.call_tool(
        "end_nominations",
        json!({ "game_id": game_id, "token": host, "host_token": host }),
    );
    let msg = match &res {
        Err(e) => e.clone(),
        Ok(v) => v.to_string(),
    };
    assert!(
        !msg.contains("not your turn"),
        "on-turn end_nominations must NOT be gate-blocked, got: {msg}"
    );

    server.stop();
}

#[test]
fn ungated_tools_still_pass_without_a_wake() {
    // Reads and create_game are not turn-gated; they must work with no wake held.
    let store = mcp_server::new_shared_store();
    let path = PathBuf::from(format!("/tmp/botc-gate-ro-{}.sock", uuid::Uuid::new_v4()));
    let server = SocketServer::start(store.clone(), &path).expect("bind");
    let mut client = SocketClient::connect(&path).expect("connect");
    let (game_id, host) = new_game(&mut client);

    let pub_state = client
        .call_tool(
            "get_public_state",
            json!({ "game_id": game_id, "token": host }),
        )
        .expect("get_public_state must not be gated");
    assert!(
        sc(&pub_state).get("phase").is_some(),
        "read returns state: {pub_state}"
    );

    server.stop();
}
