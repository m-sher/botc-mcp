//! Stdio MCP proxy: binds a single host/player token and forwards tools to the harness socket.
//!
//! Grok sessions point project MCP config at this binary so every agent shares one game.

use std::io::{self, BufRead, Write};
use std::path::PathBuf;

use clap::Parser;
use serde_json::{json, Value};

use botc_mcp::harness::socket::SocketClient;
use botc_mcp::mcp_server;

#[derive(Parser, Debug)]
#[command(name = "botc-agent-mcp", about = "Token-scoped MCP proxy for botc-tui harness")]
struct Args {
    /// Unix socket path of the harness engine RPC.
    #[arg(long)]
    socket: PathBuf,
    /// File containing the opaque host/player token for this agent.
    #[arg(long)]
    token_file: PathBuf,
    /// Fixed game_id (optional; usually injected into tool args from the token context).
    #[arg(long)]
    game_id: Option<u64>,
}

fn main() {
    let args = Args::parse();
    let token = std::fs::read_to_string(&args.token_file)
        .unwrap_or_else(|e| {
            eprintln!("botc-agent-mcp: read token: {e}");
            std::process::exit(2);
        })
        .trim()
        .to_string();
    if token.is_empty() {
        eprintln!("botc-agent-mcp: empty token");
        std::process::exit(2);
    }

    let mut client = SocketClient::connect(&args.socket).unwrap_or_else(|e| {
        eprintln!("botc-agent-mcp: connect {}: {e}", args.socket.display());
        std::process::exit(2);
    });

    let stdin = io::stdin();
    let mut stdout = io::stdout();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(resp) = handle_line(&mut client, &token, args.game_id, line) {
            let _ = writeln!(stdout, "{resp}");
            let _ = stdout.flush();
        }
    }
}

fn handle_line(
    client: &mut SocketClient,
    token: &str,
    fixed_game_id: Option<u64>,
    line: &str,
) -> Option<String> {
    let req: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => {
            return Some(
                json!({
                    "jsonrpc": "2.0",
                    "id": Value::Null,
                    "error": { "code": -32700, "message": format!("parse error: {e}") }
                })
                .to_string(),
            );
        }
    };
    let id = req.get("id").cloned();
    if id.is_none() {
        return None;
    }
    let id = id.unwrap_or(Value::Null);
    let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let params = req.get("params").cloned().unwrap_or(Value::Null);

    let result: Result<Value, (i64, String)> = match method {
        "initialize" => Ok(json!({
            "protocolVersion": "2024-11-05",
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "botc-agent-mcp", "version": env!("CARGO_PKG_VERSION") }
        })),
        "notifications/initialized" | "initialized" => Ok(Value::Null),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": mcp_server::list_tool_descriptors() })),
        "tools/call" => {
            let Some(name) = params.get("name").and_then(|n| n.as_str()) else {
                return Some(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "error": { "code": -32602, "message": "tools/call requires name" }
                }).to_string());
            };
            let mut arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            inject_auth(&mut arguments, token, fixed_game_id);
            client
                .call_tool(name, arguments)
                .map_err(|e| (-32000_i64, e))
        }
        other => {
            let mut arguments = params;
            inject_auth(&mut arguments, token, fixed_game_id);
            client
                .call_tool(other, arguments)
                .map_err(|e| (-32000_i64, e))
        }
    };

    Some(match result {
        Ok(value) => json!({ "jsonrpc": "2.0", "id": id, "result": value }).to_string(),
        Err((code, message)) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": code, "message": message }
        })
        .to_string(),
    })
}

fn inject_auth(args: &mut Value, token: &str, game_id: Option<u64>) {
    let obj = match args.as_object_mut() {
        Some(o) => o,
        None => {
            *args = json!({});
            args.as_object_mut().unwrap()
        }
    };
    // Always bind this proxy's token (player or host).
    obj.insert("token".into(), json!(token));
    obj.insert("player_token".into(), json!(token));
    obj.insert("host_token".into(), json!(token));
    if let Some(gid) = game_id {
        // Always pin game_id so agents cannot target another table.
        obj.insert("game_id".into(), json!(gid));
    }
}


