//! Shared socket RPC used by multi-agent MCP proxies.

use botc_mcp::harness::socket::{SocketClient, SocketServer};
use botc_mcp::mcp_server;
use serde_json::json;
use std::path::PathBuf;

#[test]
fn socket_tool_create_and_public_state() {
    let store = mcp_server::new_shared_store();
    let path = PathBuf::from(format!(
        "/tmp/botc-socket-test-{}.sock",
        uuid::Uuid::new_v4()
    ));
    let server = SocketServer::start(store.clone(), &path).expect("bind");
    let mut client = SocketClient::connect(&path).expect("connect");

    let created = client
        .call_tool(
            "create_game",
            json!({ "names": ["A","B","C","D","E"], "seed": 7 }),
        )
        .expect("create_game");
    // call_tool wraps as MCP content + structuredContent
    let sc = created
        .get("structuredContent")
        .cloned()
        .unwrap_or(created.clone());
    let game_id = sc.get("game_id").and_then(|v| v.as_u64()).expect("game_id");
    let host = sc
        .get("host_token")
        .and_then(|v| v.as_str())
        .expect("host_token")
        .to_string();
    assert!(game_id >= 1);

    let pub_state = client
        .call_tool(
            "get_public_state",
            json!({ "game_id": game_id, "token": host }),
        )
        .expect("public");
    let phase = pub_state
        .pointer("/structuredContent/phase")
        .or_else(|| pub_state.get("phase"))
        .map(|v| v.to_string())
        .unwrap_or_default();
    assert!(
        phase.contains("Lobby") || phase.contains("lobby") || !phase.is_empty(),
        "phase={phase} raw={pub_state}"
    );

    server.stop();
}
