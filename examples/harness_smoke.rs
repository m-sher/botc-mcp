//! Manual smoke: create_game via in-process JSON-RPC (no subprocess).
//!
//! ```bash
//! cargo run --example harness_smoke
//! ```

use botc_mcp::mcp_server::{self, SharedStore};
use serde_json::{json, Value};

fn rpc(store: &SharedStore, id: u64, method: &str, params: Value) -> Value {
    let line = json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params
    })
    .to_string();
    let resp = mcp_server::handle_line(store, &line).expect("response");
    serde_json::from_str(&resp).expect("parse")
}

fn main() {
    let store = mcp_server::new_shared_store();

    let init = rpc(&store, 1, "initialize", json!({}));
    assert_eq!(init["result"]["serverInfo"]["name"], "botc-mcp");
    println!("initialize: ok");

    let created = rpc(
        &store,
        2,
        "tools/call",
        json!({
            "name": "create_game",
            "arguments": {
                "names": ["Alice", "Bob", "Cara", "Dan", "Eve"],
                "seed": 7
            }
        }),
    );
    assert_eq!(created["result"]["isError"], false);
    let sc = &created["result"]["structuredContent"];
    let game_id = sc["game_id"].as_u64().expect("game_id");
    let host = sc["host_token"].as_str().expect("host").to_string();
    println!("create_game: game_id={game_id} players={}", sc["players"].as_array().unwrap().len());

    let list = rpc(&store, 3, "tools/list", json!({}));
    let n = list["result"]["tools"].as_array().unwrap().len();
    println!("tools/list: {n} tools");

    let started = rpc(
        &store,
        4,
        "tools/call",
        json!({
            "name": "start_game",
            "arguments": {
                "game_id": game_id,
                "host_token": host,
            }
        }),
    );
    assert_eq!(started["result"]["isError"], false, "{started}");
    println!("start_game: {}", started["result"]["structuredContent"]);

    println!("harness_smoke: OK");
}
