//! Stdio MCP proxy: binds a single host/player token and forwards tools to the harness socket.

use std::io::{self, BufRead, Write};
use std::path::PathBuf;

use clap::Parser;
use serde_json::{json, Value};

use botc_mcp::harness::proxy_acl;
use botc_mcp::harness::socket::SocketClient;
use botc_mcp::mcp_server;

#[derive(Parser, Debug)]
#[command(
    name = "botc-agent-mcp",
    about = "Token-scoped MCP proxy for botc-tui harness"
)]
struct Args {
    #[arg(long)]
    socket: PathBuf,
    #[arg(long)]
    token_file: PathBuf,
    #[arg(long)]
    game_id: Option<u64>,
    /// `host` or `player` (default player).
    #[arg(long, default_value = "player")]
    role: String,
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
    let is_host = args.role.eq_ignore_ascii_case("host");

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
        if let Some(resp) = handle_line(&mut client, &token, args.game_id, is_host, line) {
            let _ = writeln!(stdout, "{resp}");
            let _ = stdout.flush();
        }
    }
}

fn handle_line(
    client: &mut SocketClient,
    token: &str,
    fixed_game_id: Option<u64>,
    is_host: bool,
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
    let id = req.get("id").cloned()?;
    let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let params = req.get("params").cloned().unwrap_or(Value::Null);

    let result: Result<Value, (i64, String)> = match method {
        // Echo the client's requested protocol version. Our wire use (initialize +
        // tools/list + tools/call) is version-agnostic, so mirroring the client keeps
        // every client happy. Newer clients (Claude Code speaks e.g. "2025-11-25")
        // reject a server that unilaterally downgrades to an old fixed version — that
        // left the botc server stuck at "pending" so its tools never loaded. Fall back
        // to a known-good version when the client sends none.
        "initialize" => Ok(json!({
            "protocolVersion": params
                .get("protocolVersion")
                .and_then(|v| v.as_str())
                .unwrap_or("2024-11-05"),
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "botc-agent-mcp", "version": env!("CARGO_PKG_VERSION") }
        })),
        "notifications/initialized" | "initialized" => Ok(Value::Null),
        "ping" => Ok(json!({})),
        "tools/list" => {
            let tools: Vec<Value> = mcp_server::list_tool_descriptors()
                .into_iter()
                .filter(|t| {
                    let name = t.get("name").and_then(|n| n.as_str()).unwrap_or("");
                    proxy_acl::tool_allowed(name, is_host)
                })
                .collect();
            Ok(json!({ "tools": tools }))
        }
        "tools/call" => {
            let Some(name) = params.get("name").and_then(|n| n.as_str()) else {
                return Some(
                    json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": { "code": -32602, "message": "tools/call requires name" }
                    })
                    .to_string(),
                );
            };
            // Policy denial is Invalid params (-32602), not Method not found (-32601).
            // -32601 would let a strict client conclude tools/call itself is unsupported (#51).
            if !proxy_acl::tool_allowed(name, is_host) {
                return Some(
                    json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": {
                            "code": proxy_acl::ACL_DENY_JSONRPC_CODE,
                            "message": format!("tool not available for this agent role: {name}")
                        }
                    })
                    .to_string(),
                );
            }
            let mut arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            inject_auth(&mut arguments, token, fixed_game_id, is_host);
            client
                .call_tool(name, arguments)
                .map_err(|e| (-32000_i64, e))
        }
        other => {
            // #54/#59: only forward genuine bare tool names. Namespaced MCP methods
            // (resources/list, …) and unknown bare typos (nominatee, foobar) must get
            // local -32601 — never a tools/call-shaped isError success via the engine.
            if other.is_empty() || other.contains('/') || !mcp_server::is_known_tool(other) {
                Err((-32601, format!("method not found: {other}")))
            } else if !proxy_acl::tool_allowed(other, is_host) {
                // Known tool but denied for this agent role → Invalid params.
                Err((
                    proxy_acl::ACL_DENY_JSONRPC_CODE,
                    format!("tool not available for this agent role: {other}"),
                ))
            } else {
                let mut arguments = params;
                inject_auth(&mut arguments, token, fixed_game_id, is_host);
                client
                    .call_tool(other, arguments)
                    .map_err(|e| (-32000_i64, e))
            }
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

fn inject_auth(args: &mut Value, token: &str, game_id: Option<u64>, is_host: bool) {
    let obj = match args.as_object_mut() {
        Some(o) => o,
        None => {
            *args = json!({});
            args.as_object_mut().unwrap()
        }
    };
    obj.insert("token".into(), json!(token));
    if is_host {
        obj.insert("host_token".into(), json!(token));
        obj.remove("player_token");
    } else {
        obj.insert("player_token".into(), json!(token));
        obj.remove("host_token");
    }
    if let Some(gid) = game_id {
        obj.insert("game_id".into(), json!(gid));
    }
}
