//! MCP-shaped JSON-RPC transport over stdio.
//!
//! A thin line-delimited JSON-RPC 2.0 server that exposes engine tools from
//! [`crate::tools`]. Full `rmcp` integration is deferred; the wire shape matches
//! common MCP harness expectations (`initialize`, `tools/list`, `tools/call`).
//! See `docs/mcp.md`.

use std::io::{self, BufRead, Write};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use crate::auth::Token;
use crate::comms::{PrivateMessage, PublicEvent};
use crate::error::ToolError;
use crate::game::{
    ChoiceSchema, GameId, HostDecision, MayorRedirectChoice, NightActionPayload, RegistrationMode,
    RoleAssignment, SeatId, StartOpts, Winner,
};
use crate::roles::Character;
use crate::store::GameStore;
use crate::tools::{self, DayActionPayload};

/// Shared process-local game registry.
pub type SharedStore = Arc<Mutex<GameStore>>;

/// Create an empty shared store for the server process.
pub fn new_shared_store() -> SharedStore {
    Arc::new(Mutex::new(GameStore::new()))
}

/// Run the stdio JSON-RPC loop until stdin EOF.
pub fn run_stdio(store: SharedStore) -> io::Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let mut lines = stdin.lock().lines();
    while let Some(line) = lines.next() {
        let line = line?;
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let response = handle_line(&store, line);
        // Notifications (no id) get no response body.
        if let Some(resp) = response {
            writeln!(stdout, "{resp}")?;
            stdout.flush()?;
        }
    }
    Ok(())
}

/// Handle one JSON-RPC request line. Returns `None` for notifications.
pub fn handle_line(store: &SharedStore, line: &str) -> Option<String> {
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
    // JSON-RPC notification: no id → no response.
    if id.is_none() {
        // Still process initialize-style notifications if ever needed; ignore body.
        return None;
    }
    let id = id.unwrap_or(Value::Null);

    let method = req
        .get("method")
        .and_then(|m| m.as_str())
        .unwrap_or("");
    let params = req.get("params").cloned().unwrap_or(Value::Null);

    let result = dispatch_method(store, method, params);
    Some(match result {
        Ok(value) => json!({ "jsonrpc": "2.0", "id": id, "result": value }).to_string(),
        Err(RpcError { code, message, data }) => {
            let mut err = json!({ "code": code, "message": message });
            if let Some(d) = data {
                err["data"] = d;
            }
            json!({ "jsonrpc": "2.0", "id": id, "error": err }).to_string()
        }
    })
}

struct RpcError {
    code: i64,
    message: String,
    data: Option<Value>,
}

fn dispatch_method(store: &SharedStore, method: &str, params: Value) -> Result<Value, RpcError> {
    match method {
        "initialize" => Ok(json!({
            "protocolVersion": "2024-11-05",
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "botc-mcp", "version": env!("CARGO_PKG_VERSION") }
        })),
        "notifications/initialized" | "initialized" => Ok(Value::Null),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tool_descriptors() })),
        "tools/call" => {
            let name = params
                .get("name")
                .and_then(|n| n.as_str())
                .ok_or_else(|| RpcError {
                    code: -32602,
                    message: "tools/call requires params.name".into(),
                    data: None,
                })?;
            let arguments = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            call_tool(store, name, arguments)
        }
        // Allow direct tool name as method (convenience / simple harnesses).
        other if is_known_tool(other) => call_tool(store, other, params),
        other => Err(RpcError {
            code: -32601,
            message: format!("method not found: {other}"),
            data: None,
        }),
    }
}

fn is_known_tool(name: &str) -> bool {
    TOOL_NAMES.contains(&name)
}

const TOOL_NAMES: &[&str] = &[
    "create_game",
    "start_game",
    "get_public_state",
    "get_public_log",
    "get_private_state",
    "get_character_rules",
    "get_host_state",
    "say",
    "st_announce",
    "night_action",
    "day_action",
    "nominate",
    "vote",
    "pass_vote",
    "open_nominations",
    "close_vote",
    "end_nominations",
    "skip_night_action",
    "host_decide",
    "host_queue_lie",
];

fn tool_descriptors() -> Vec<Value> {
    TOOL_NAMES
        .iter()
        .map(|name| {
            json!({
                "name": name,
                "description": tool_description(name),
                "inputSchema": { "type": "object" }
            })
        })
        .collect()
}

fn tool_description(name: &str) -> &'static str {
    match name {
        "create_game" => "Create a Trouble Brewing lobby (5–15 players); optional seed/secret_salt (omit = CSPRNG); returns game_id, host_token, player tokens",
        "start_game" => "Host: lock lobby, assign bag (or fixed assignments), enter first night",
        "get_public_state" => "Public phase/seats/winner snapshot (no roles, no pending night seat)",
        "get_public_log" => "Public event log since cursor",
        "get_private_state" => "Player private view: face identity, inbox, awaiting action",
        "get_character_rules" => "Public character sheet markdown for one TB character",
        "get_host_state" => "Host-only grimoire (true roles, markers, pending wake, seed, secret_salt)",
        "say" => "Player public table talk (no whispers)",
        "st_announce" => "Host public storyteller announcement",
        "night_action" => "Player night choice for current pending wake",
        "day_action" => "Player day ability (Slayer slay)",
        "nominate" => "Player nominate a living seat",
        "vote" => "Player cast yes/no on open nomination",
        "pass_vote" => "Dead player: abstain without spending ghost vote",
        "open_nominations" => "Host: Discussion → Nominations",
        "close_vote" => "Host: close current vote window",
        "end_nominations" => "Host: execute vote leader (if any), begin next night",
        "skip_night_action" => "Host: default pending wake or host decision and continue night",
        "host_decide" => "Host: resolve Mayor redirect or starpass pick",
        "host_queue_lie" => "Host: enqueue free-text false info for next disabled info result",
        _ => "botc-mcp tool",
    }
}

/// Invoke a named tool; wraps success as MCP `content` + structured `structuredContent`.
fn call_tool(store: &SharedStore, name: &str, args: Value) -> Result<Value, RpcError> {
    match invoke_tool(store, name, args) {
        Ok(structured) => {
            let text = structured.to_string();
            Ok(json!({
                "content": [{ "type": "text", "text": text }],
                "structuredContent": structured,
                "isError": false
            }))
        }
        Err(e) => {
            // Tool-level errors are returned as successful RPC with isError (MCP style),
            // except protocol/arg parse failures which stay as RPC errors.
            if e.code == -32602 {
                return Err(e);
            }
            Ok(json!({
                "content": [{ "type": "text", "text": e.message }],
                "isError": true
            }))
        }
    }
}

fn invoke_tool(store: &SharedStore, name: &str, args: Value) -> Result<Value, RpcError> {
    match name {
        "create_game" => tool_create_game(store, args),
        "start_game" => tool_start_game(store, args),
        "get_public_state" => tool_get_public_state(store, args),
        "get_public_log" => tool_get_public_log(store, args),
        "get_private_state" => tool_get_private_state(store, args),
        "get_character_rules" => tool_get_character_rules(args),
        "get_host_state" => tool_get_host_state(store, args),
        "say" => tool_say(store, args),
        "st_announce" => tool_st_announce(store, args),
        "night_action" => tool_night_action(store, args),
        "day_action" => tool_day_action(store, args),
        "nominate" => tool_nominate(store, args),
        "vote" => tool_vote(store, args),
        "pass_vote" => tool_pass_vote(store, args),
        "open_nominations" => tool_open_nominations(store, args),
        "close_vote" => tool_close_vote(store, args),
        "end_nominations" => tool_end_nominations(store, args),
        "skip_night_action" => tool_skip_night_action(store, args),
        "host_decide" => tool_host_decide(store, args),
        "host_queue_lie" => tool_host_queue_lie(store, args),
        other => Err(RpcError {
            code: -32601,
            message: format!("unknown tool: {other}"),
            data: None,
        }),
    }
}

// ── helpers ──────────────────────────────────────────────────────────────────

fn tool_err(e: ToolError) -> RpcError {
    let code = match &e {
        ToolError::Unauthorized => -32001,
        ToolError::BadRequest(_) => -32002,
        ToolError::Game(_) => -32003,
    };
    RpcError {
        code,
        message: e.to_string(),
        data: None,
    }
}

fn invalid_params(msg: impl Into<String>) -> RpcError {
    RpcError {
        code: -32602,
        message: msg.into(),
        data: None,
    }
}

fn require_game_id(args: &Value) -> Result<GameId, RpcError> {
    let id = args
        .get("game_id")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| invalid_params("missing game_id (u64)"))?;
    Ok(GameId(id))
}

fn require_token(args: &Value, key: &str) -> Result<Token, RpcError> {
    let s = args
        .get(key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| invalid_params(format!("missing {key} (string)")))?;
    Ok(Token::from_shared(s))
}

/// Prefer `token`, fall back to `host_token` / `player_token`.
fn any_token(args: &Value) -> Result<Token, RpcError> {
    if let Some(s) = args.get("token").and_then(|v| v.as_str()) {
        return Ok(Token::from_shared(s));
    }
    if let Some(s) = args.get("host_token").and_then(|v| v.as_str()) {
        return Ok(Token::from_shared(s));
    }
    if let Some(s) = args.get("player_token").and_then(|v| v.as_str()) {
        return Ok(Token::from_shared(s));
    }
    Err(invalid_params(
        "missing token (or host_token / player_token)",
    ))
}

fn seat_id(v: &Value) -> Result<SeatId, RpcError> {
    let n = v
        .as_u64()
        .ok_or_else(|| invalid_params("seat id must be number"))?;
    if n > u8::MAX as u64 {
        return Err(invalid_params("seat id out of range"));
    }
    Ok(SeatId(n as u8))
}

fn with_store_mut<F, R>(store: &SharedStore, f: F) -> Result<R, RpcError>
where
    F: FnOnce(&mut GameStore) -> Result<R, RpcError>,
{
    let mut guard = store
        .lock()
        .map_err(|_| RpcError {
            code: -32000,
            message: "store lock poisoned".into(),
            data: None,
        })?;
    f(&mut guard)
}

fn parse_character(name: &str) -> Result<Character, RpcError> {
    let norm = name.trim().to_ascii_lowercase().replace([' ', '_', '-'], "");
    use Character::*;
    let c = match norm.as_str() {
        "washerwoman" => Washerwoman,
        "librarian" => Librarian,
        "investigator" => Investigator,
        "chef" => Chef,
        "empath" => Empath,
        "fortuneteller" => FortuneTeller,
        "undertaker" => Undertaker,
        "monk" => Monk,
        "ravenkeeper" => Ravenkeeper,
        "virgin" => Virgin,
        "slayer" => Slayer,
        "soldier" => Soldier,
        "mayor" => Mayor,
        "butler" => Butler,
        "drunk" => Drunk,
        "recluse" => Recluse,
        "saint" => Saint,
        "poisoner" => Poisoner,
        "spy" => Spy,
        "scarletwoman" => ScarletWoman,
        "baron" => Baron,
        "imp" => Imp,
        _ => {
            return Err(invalid_params(format!("unknown character: {name}")));
        }
    };
    Ok(c)
}

// ── tool handlers ────────────────────────────────────────────────────────────

fn tool_create_game(store: &SharedStore, args: Value) -> Result<Value, RpcError> {
    let names: Vec<String> = args
        .get("names")
        .and_then(|v| v.as_array())
        .ok_or_else(|| invalid_params("create_game requires names: string[]"))?
        .iter()
        .map(|v| {
            v.as_str()
                .map(|s| s.to_string())
                .ok_or_else(|| invalid_params("names entries must be strings"))
        })
        .collect::<Result<_, _>>()?;
    // Never default to seed 0: omit → CSPRNG secret so bag/draws are not offline-reproducible.
    let seed = args
        .get("seed")
        .and_then(|v| v.as_u64())
        .unwrap_or_else(rand::random);
    // Optional salt: omit → CSPRNG (production); provide with seed for full deterministic replay.
    let secret_salt = args.get("secret_salt").and_then(|v| v.as_u64());

    with_store_mut(store, |st| {
        let resp = tools::create_game(st, names, seed, secret_salt).map_err(tool_err)?;
        Ok(json!({
            "game_id": resp.game_id.0,
            "host_token": resp.host_token.as_str(),
            "players": resp.players.iter().map(|p| json!({
                "seat_id": p.seat_id.0,
                "name": p.name,
                "player_token": p.player_token.as_str(),
            })).collect::<Vec<_>>()
        }))
    })
}

fn tool_start_game(store: &SharedStore, args: Value) -> Result<Value, RpcError> {
    let game_id = require_game_id(&args)?;
    let host = require_token(&args, "host_token").or_else(|_| any_token(&args))?;
    let assignments = if let Some(arr) = args.get("assignments").and_then(|v| v.as_array()) {
        let mut out = Vec::with_capacity(arr.len());
        for item in arr {
            let seat = seat_id(
                item.get("seat")
                    .or_else(|| item.get("seat_id"))
                    .ok_or_else(|| invalid_params("assignment.seat required"))?,
            )?;
            let ch_name = item
                .get("character")
                .or_else(|| item.get("true_character"))
                .and_then(|v| v.as_str())
                .ok_or_else(|| invalid_params("assignment.character required"))?;
            let true_c = parse_character(ch_name)?;
            let believed = item
                .get("believed")
                .or_else(|| item.get("believed_character"))
                .and_then(|v| v.as_str())
                .map(parse_character)
                .transpose()?;
            let ra = if true_c == Character::Drunk {
                let face = believed.ok_or_else(|| {
                    invalid_params("Drunk assignment requires believed/believed_character Townsfolk face")
                })?;
                RoleAssignment::drunk(seat, face).map_err(|e| tool_err(ToolError::from(e)))?
            } else {
                RoleAssignment::normal(seat, true_c)
            };
            out.push(ra);
        }
        Some(out)
    } else {
        None
    };
    let registration_mode = match args
        .get("registration_mode")
        .and_then(|v| v.as_str())
        .unwrap_or("random")
        .to_ascii_lowercase()
        .as_str()
    {
        "random" | "" => RegistrationMode::Random,
        "alwaystrue" | "always_true" | "true" => RegistrationMode::AlwaysTrue,
        "alwaysmisreg" | "always_misreg" | "misreg" => RegistrationMode::AlwaysMisreg,
        other => {
            return Err(invalid_params(format!(
                "unknown registration_mode: {other}"
            )))
        }
    };

    let drunk_faces = if let Some(arr) = args.get("drunk_faces").and_then(|v| v.as_array()) {
        let mut out = Vec::new();
        for item in arr {
            let seat = seat_id(
                item.get("seat")
                    .or_else(|| item.get("seat_id"))
                    .ok_or_else(|| invalid_params("drunk_faces.seat required"))?,
            )?;
            let face_name = item
                .get("face")
                .or_else(|| item.get("character"))
                .and_then(|v| v.as_str())
                .ok_or_else(|| invalid_params("drunk_faces.face required"))?;
            out.push((seat, parse_character(face_name)?));
        }
        Some(out)
    } else {
        None
    };

    let red_herring = args
        .get("red_herring")
        .map(seat_id)
        .transpose()?;

    let demon_bluffs = if let Some(arr) = args.get("demon_bluffs").and_then(|v| v.as_array()) {
        let mut out = Vec::new();
        for item in arr {
            let name = item
                .as_str()
                .ok_or_else(|| invalid_params("demon_bluffs entries must be character names"))?;
            out.push(parse_character(name)?);
        }
        Some(out)
    } else {
        None
    };

    let opts = StartOpts {
        assignments,
        drunk_faces,
        red_herring,
        demon_bluffs,
        registration_mode,
    };

    with_store_mut(store, |st| {
        let game = st
            .get_mut(game_id)
            .ok_or_else(|| tool_err(ToolError::BadRequest("unknown game_id")))?;
        tools::start_game(game, &host, opts).map_err(tool_err)?;
        Ok(json!({ "ok": true, "phase": format!("{:?}", game.phase) }))
    })
}

fn tool_get_public_state(store: &SharedStore, args: Value) -> Result<Value, RpcError> {
    let game_id = require_game_id(&args)?;
    let token = any_token(&args)?;
    with_store_mut(store, |st| {
        let game = st
            .get(game_id)
            .ok_or_else(|| tool_err(ToolError::BadRequest("unknown game_id")))?;
        let view = tools::get_public_state(game, &token).map_err(tool_err)?;
        Ok(json!({
            "phase": view.phase,
            "seats": view.seats.iter().map(|s| json!({
                "id": s.id.0,
                "name": s.name,
                "alive": s.alive,
                "ghost_vote_available": s.ghost_vote_available,
            })).collect::<Vec<_>>(),
            "winner": view.winner.map(winner_json),
        }))
    })
}

fn tool_get_public_log(store: &SharedStore, args: Value) -> Result<Value, RpcError> {
    let game_id = require_game_id(&args)?;
    let token = any_token(&args)?;
    let cursor = args.get("cursor").and_then(|v| v.as_u64()).unwrap_or(0);
    with_store_mut(store, |st| {
        let game = st
            .get(game_id)
            .ok_or_else(|| tool_err(ToolError::BadRequest("unknown game_id")))?;
        let log = tools::get_public_log(game, &token, cursor).map_err(tool_err)?;
        let events: Vec<Value> = log
            .iter()
            .map(|(id, e)| json!({ "id": id, "event": public_event_json(e) }))
            .collect();
        Ok(json!({ "events": events }))
    })
}

fn tool_get_private_state(store: &SharedStore, args: Value) -> Result<Value, RpcError> {
    let game_id = require_game_id(&args)?;
    let token = any_token(&args)?;
    let cursor = args
        .get("private_cursor")
        .or_else(|| args.get("cursor"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    with_store_mut(store, |st| {
        let game = st
            .get(game_id)
            .ok_or_else(|| tool_err(ToolError::BadRequest("unknown game_id")))?;
        let view = tools::get_private_state(game, &token, cursor).map_err(tool_err)?;
        Ok(json!({
            "seat": view.seat.0,
            "name": view.name,
            "alive": view.alive,
            "character_label": view.character_label,
            "team_label": view.team_label,
            "rules_path": view.rules_path,
            "private_messages_since": view.private_messages_since.iter().map(|(id, m)| {
                json!({ "id": id, "message": private_message_json(m) })
            }).collect::<Vec<_>>(),
            "awaiting_action": view.awaiting_action,
            "awaiting": view.awaiting.as_ref().map(|a| json!({
                "action": a.action,
                "prompt": a.prompt,
                "schema": choice_schema_json(&a.schema),
            })),
        }))
    })
}

fn tool_get_character_rules(args: Value) -> Result<Value, RpcError> {
    let name = args
        .get("character")
        .or_else(|| args.get("name"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| invalid_params("get_character_rules requires character name"))?;
    let c = parse_character(name)?;
    let view = tools::get_character_rules(c).map_err(tool_err)?;
    Ok(json!({
        "name": view.name,
        "path": view.path,
        "team": view.team,
        "character_type": view.character_type,
        "text": view.text,
    }))
}

fn tool_get_host_state(store: &SharedStore, args: Value) -> Result<Value, RpcError> {
    let game_id = require_game_id(&args)?;
    let token = any_token(&args)?;
    with_store_mut(store, |st| {
        let game = st
            .get(game_id)
            .ok_or_else(|| tool_err(ToolError::BadRequest("unknown game_id")))?;
        let view = tools::get_host_state(game, &token).map_err(tool_err)?;
        Ok(json!({
            "seed": view.seed,
            "secret_salt": view.secret_salt,
            "phase": view.phase,
            "seats": view.seats.iter().map(|s| json!({
                "seat_id": s.seat_id.0,
                "name": s.name,
                "alive": s.alive,
                "ghost_vote_available": s.ghost_vote_available,
                "true_character": s.true_character,
                "believed_character": s.believed_character,
                "poisoned": s.poisoned,
                "is_drunk_outsider": s.is_drunk_outsider,
                "monk_protected_tonight": s.monk_protected_tonight,
                "slayer_used": s.slayer_used,
                "virgin_ability_used": s.virgin_ability_used,
                "butler_master": s.butler_master.map(|x| x.0),
            })).collect::<Vec<_>>(),
            "pending": view.pending.as_ref().map(|p| json!({
                "seat_id": p.seat_id.0,
                "prompt": p.prompt,
                "schema": choice_schema_json(&p.schema),
                "step_debug": p.step_debug,
            })),
            "pending_host": view.pending_host.as_ref().map(|p| json!({
                "kind": p.kind,
                "detail": p.detail,
                "seats": p.seats.iter().map(|s| s.0).collect::<Vec<_>>(),
            })),
            "registration_mode": view.registration_mode,
            "host_lie_queue_len": view.host_lie_queue_len,
            "red_herring": view.red_herring.map(|s| s.0),
            "demon_bluffs": view.demon_bluffs,
            "winner": view.winner.map(winner_json),
        }))
    })
}

fn tool_say(store: &SharedStore, args: Value) -> Result<Value, RpcError> {
    let game_id = require_game_id(&args)?;
    let token = any_token(&args)?;
    let text = args
        .get("text")
        .and_then(|v| v.as_str())
        .ok_or_else(|| invalid_params("say requires text"))?
        .to_string();
    with_store_mut(store, |st| {
        let game = st
            .get_mut(game_id)
            .ok_or_else(|| tool_err(ToolError::BadRequest("unknown game_id")))?;
        let event_id = tools::say(game, &token, text).map_err(tool_err)?;
        Ok(json!({ "event_id": event_id }))
    })
}

fn tool_st_announce(store: &SharedStore, args: Value) -> Result<Value, RpcError> {
    let game_id = require_game_id(&args)?;
    let host = require_token(&args, "host_token").or_else(|_| any_token(&args))?;
    let text = args
        .get("text")
        .and_then(|v| v.as_str())
        .ok_or_else(|| invalid_params("st_announce requires text"))?
        .to_string();
    with_store_mut(store, |st| {
        let game = st
            .get_mut(game_id)
            .ok_or_else(|| tool_err(ToolError::BadRequest("unknown game_id")))?;
        let event_id = tools::st_announce(game, &host, text).map_err(tool_err)?;
        Ok(json!({ "event_id": event_id }))
    })
}

fn tool_night_action(store: &SharedStore, args: Value) -> Result<Value, RpcError> {
    let game_id = require_game_id(&args)?;
    let token = any_token(&args)?;
    let payload = parse_night_payload(&args)?;
    with_store_mut(store, |st| {
        let game = st
            .get_mut(game_id)
            .ok_or_else(|| tool_err(ToolError::BadRequest("unknown game_id")))?;
        tools::night_action(game, &token, payload).map_err(tool_err)?;
        Ok(json!({ "ok": true }))
    })
}

fn parse_night_payload(args: &Value) -> Result<NightActionPayload, RpcError> {
    // Nested `payload` object or flat fields.
    let p = args.get("payload").unwrap_or(args);
    if let Some(kind) = p.get("kind").or_else(|| p.get("type")).and_then(|v| v.as_str()) {
        return match kind {
            "ack" | "Ack" => Ok(NightActionPayload::Ack),
            "pick_one" | "PickOne" => {
                let t = p
                    .get("target")
                    .ok_or_else(|| invalid_params("PickOne needs target"))?;
                Ok(NightActionPayload::PickOne {
                    target: seat_id(t)?,
                })
            }
            "pick_two" | "PickTwo" => {
                let a = p
                    .get("a")
                    .or_else(|| p.get("target_a"))
                    .ok_or_else(|| invalid_params("PickTwo needs a"))?;
                let b = p
                    .get("b")
                    .or_else(|| p.get("target_b"))
                    .ok_or_else(|| invalid_params("PickTwo needs b"))?;
                Ok(NightActionPayload::PickTwo {
                    a: seat_id(a)?,
                    b: seat_id(b)?,
                })
            }
            "pick_character" | "PickCharacter" => {
                let name = p
                    .get("name")
                    .or_else(|| p.get("character"))
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| invalid_params("PickCharacter needs name"))?;
                Ok(NightActionPayload::PickCharacter {
                    name: name.to_string(),
                })
            }
            other => Err(invalid_params(format!("unknown night payload kind: {other}"))),
        };
    }
    if p.get("ack").and_then(|v| v.as_bool()) == Some(true)
        || p.get("action").and_then(|v| v.as_str()) == Some("ack")
    {
        return Ok(NightActionPayload::Ack);
    }
    if let Some(t) = p.get("target") {
        return Ok(NightActionPayload::PickOne {
            target: seat_id(t)?,
        });
    }
    if let (Some(a), Some(b)) = (p.get("a").or_else(|| p.get("targets")), p.get("b")) {
        // targets: [a,b] handled below
        let _ = a;
        let _ = b;
    }
    if let Some(arr) = p.get("targets").and_then(|v| v.as_array()) {
        if arr.len() == 2 {
            return Ok(NightActionPayload::PickTwo {
                a: seat_id(&arr[0])?,
                b: seat_id(&arr[1])?,
            });
        }
    }
    if let (Some(a), Some(b)) = (p.get("a"), p.get("b")) {
        return Ok(NightActionPayload::PickTwo {
            a: seat_id(a)?,
            b: seat_id(b)?,
        });
    }
    if let Some(name) = p
        .get("character")
        .or_else(|| p.get("character_guess"))
        .and_then(|v| v.as_str())
    {
        return Ok(NightActionPayload::PickCharacter {
            name: name.to_string(),
        });
    }
    // Default: ack (info-only wakes)
    Ok(NightActionPayload::Ack)
}

fn tool_day_action(store: &SharedStore, args: Value) -> Result<Value, RpcError> {
    let game_id = require_game_id(&args)?;
    let token = any_token(&args)?;
    let payload = {
        let p = args.get("payload").unwrap_or(&args);
        let action = p
            .get("action")
            .or_else(|| p.get("kind"))
            .and_then(|v| v.as_str())
            .unwrap_or("slay");
        match action {
            "slay" | "Slay" | "slayer" => {
                let t = p
                    .get("target")
                    .ok_or_else(|| invalid_params("day_action slay needs target"))?;
                DayActionPayload::Slay {
                    target: seat_id(t)?,
                }
            }
            other => {
                return Err(invalid_params(format!("unknown day action: {other}")));
            }
        }
    };
    with_store_mut(store, |st| {
        let game = st
            .get_mut(game_id)
            .ok_or_else(|| tool_err(ToolError::BadRequest("unknown game_id")))?;
        tools::day_action(game, &token, payload).map_err(tool_err)?;
        Ok(json!({ "ok": true }))
    })
}

fn tool_nominate(store: &SharedStore, args: Value) -> Result<Value, RpcError> {
    let game_id = require_game_id(&args)?;
    let token = any_token(&args)?;
    let target = seat_id(
        args.get("target")
            .ok_or_else(|| invalid_params("nominate requires target"))?,
    )?;
    with_store_mut(store, |st| {
        let game = st
            .get_mut(game_id)
            .ok_or_else(|| tool_err(ToolError::BadRequest("unknown game_id")))?;
        tools::nominate(game, &token, target).map_err(tool_err)?;
        Ok(json!({ "ok": true }))
    })
}

fn tool_vote(store: &SharedStore, args: Value) -> Result<Value, RpcError> {
    let game_id = require_game_id(&args)?;
    let token = any_token(&args)?;
    let nominee = seat_id(
        args.get("nominee")
            .or_else(|| args.get("target"))
            .ok_or_else(|| invalid_params("vote requires nominee"))?,
    )?;
    let support = args
        .get("support")
        .or_else(|| args.get("yes"))
        .and_then(|v| v.as_bool())
        .ok_or_else(|| invalid_params("vote requires support (bool)"))?;
    with_store_mut(store, |st| {
        let game = st
            .get_mut(game_id)
            .ok_or_else(|| tool_err(ToolError::BadRequest("unknown game_id")))?;
        tools::vote(game, &token, nominee, support).map_err(tool_err)?;
        Ok(json!({ "ok": true }))
    })
}

fn tool_pass_vote(store: &SharedStore, args: Value) -> Result<Value, RpcError> {
    let game_id = require_game_id(&args)?;
    let token = any_token(&args)?;
    with_store_mut(store, |st| {
        let game = st
            .get_mut(game_id)
            .ok_or_else(|| tool_err(ToolError::BadRequest("unknown game_id")))?;
        tools::pass_vote(game, &token).map_err(tool_err)?;
        Ok(json!({ "ok": true }))
    })
}

fn tool_open_nominations(store: &SharedStore, args: Value) -> Result<Value, RpcError> {
    host_phase_tool(store, args, tools::open_nominations)
}

fn tool_close_vote(store: &SharedStore, args: Value) -> Result<Value, RpcError> {
    host_phase_tool(store, args, tools::close_vote)
}

fn tool_end_nominations(store: &SharedStore, args: Value) -> Result<Value, RpcError> {
    host_phase_tool(store, args, tools::end_nominations)
}

fn tool_skip_night_action(store: &SharedStore, args: Value) -> Result<Value, RpcError> {
    host_phase_tool(store, args, tools::skip_night_action)
}

fn tool_host_queue_lie(store: &SharedStore, args: Value) -> Result<Value, RpcError> {
    let game_id = require_game_id(&args)?;
    let host = require_token(&args, "host_token").or_else(|_| any_token(&args))?;
    let text = args
        .get("text")
        .and_then(|v| v.as_str())
        .ok_or_else(|| invalid_params("host_queue_lie requires text"))?
        .to_string();
    with_store_mut(store, |st| {
        let game = st
            .get_mut(game_id)
            .ok_or_else(|| tool_err(ToolError::BadRequest("unknown game_id")))?;
        tools::host_queue_lie(game, &host, text).map_err(tool_err)?;
        Ok(json!({
            "ok": true,
            "host_lie_queue_len": game.host_lie_queue.len()
        }))
    })
}

fn tool_host_decide(store: &SharedStore, args: Value) -> Result<Value, RpcError> {
    let game_id = require_game_id(&args)?;
    let host = require_token(&args, "host_token").or_else(|_| any_token(&args))?;
    let ty = args
        .get("type")
        .or_else(|| args.get("decision"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| invalid_params("host_decide requires type"))?;
    let decision = match ty {
        "mayor_redirect" => {
            let choice_s = args
                .get("choice")
                .and_then(|v| v.as_str())
                .ok_or_else(|| invalid_params("mayor_redirect requires choice"))?;
            let choice = match choice_s {
                "kill_mayor" => MayorRedirectChoice::KillMayor,
                "nobody" => MayorRedirectChoice::Nobody,
                "kill_other" => {
                    let target = args
                        .get("target")
                        .ok_or_else(|| invalid_params("kill_other requires target seat"))?;
                    // Allow string "nobody" as alias.
                    if target.as_str() == Some("nobody") {
                        MayorRedirectChoice::Nobody
                    } else {
                        MayorRedirectChoice::KillOther {
                            target: seat_id(target)?,
                        }
                    }
                }
                other => {
                    return Err(invalid_params(format!(
                        "unknown mayor choice: {other}"
                    )))
                }
            };
            HostDecision::MayorRedirect { choice }
        }
        "starpass_pick" => {
            let minion = args
                .get("target")
                .or_else(|| args.get("minion"))
                .or_else(|| args.get("seat"))
                .ok_or_else(|| invalid_params("starpass_pick requires target/minion seat"))?;
            HostDecision::StarpassPick {
                minion: seat_id(minion)?,
            }
        }
        other => {
            return Err(invalid_params(format!(
                "unknown host_decide type: {other}"
            )))
        }
    };
    with_store_mut(store, |st| {
        let game = st
            .get_mut(game_id)
            .ok_or_else(|| tool_err(ToolError::BadRequest("unknown game_id")))?;
        tools::host_decide(game, &host, decision).map_err(tool_err)?;
        Ok(json!({ "ok": true, "phase": format!("{:?}", game.phase) }))
    })
}

fn host_phase_tool<F>(store: &SharedStore, args: Value, f: F) -> Result<Value, RpcError>
where
    F: FnOnce(&mut crate::game::Game, &Token) -> Result<(), ToolError>,
{
    let game_id = require_game_id(&args)?;
    let host = require_token(&args, "host_token").or_else(|_| any_token(&args))?;
    with_store_mut(store, |st| {
        let game = st
            .get_mut(game_id)
            .ok_or_else(|| tool_err(ToolError::BadRequest("unknown game_id")))?;
        f(game, &host).map_err(tool_err)?;
        Ok(json!({ "ok": true, "phase": format!("{:?}", game.phase) }))
    })
}

// ── JSON serializers for domain types ────────────────────────────────────────

fn winner_json(w: Winner) -> Value {
    match w {
        Winner::Good => json!("Good"),
        Winner::Evil => json!("Evil"),
    }
}

fn choice_schema_json(s: &ChoiceSchema) -> Value {
    match s {
        ChoiceSchema::Ack => json!({ "kind": "ack" }),
        ChoiceSchema::PickOne {
            any_seat,
            living_only,
            exclude_self,
        } => json!({
            "kind": "pick_one",
            "any_seat": any_seat,
            "living_only": living_only,
            "exclude_self": exclude_self,
        }),
        ChoiceSchema::PickTwo { any_seat } => json!({
            "kind": "pick_two",
            "any_seat": any_seat,
        }),
    }
}

fn public_event_json(e: &PublicEvent) -> Value {
    match e {
        PublicEvent::Chat { seat, name, text } => json!({
            "type": "chat", "seat": seat.0, "name": name, "text": text
        }),
        PublicEvent::StorytellerAnnounce { text } => {
            json!({ "type": "storyteller_announce", "text": text })
        }
        PublicEvent::Nominated { by, target } => {
            json!({ "type": "nominated", "by": by.0, "target": target.0 })
        }
        PublicEvent::VoteCast {
            seat,
            nominee,
            support,
        } => json!({
            "type": "vote_cast", "seat": seat.0, "nominee": nominee.0, "support": support
        }),
        PublicEvent::Executed { seat } => json!({ "type": "executed", "seat": seat.0 }),
        PublicEvent::NoExecution => json!({ "type": "no_execution" }),
        PublicEvent::DiedInNight { seats } => json!({
            "type": "died_in_night",
            "seats": seats.iter().map(|s| s.0).collect::<Vec<_>>()
        }),
        PublicEvent::PlayerDied { seat } => json!({ "type": "player_died", "seat": seat.0 }),
        PublicEvent::SlayerMiss { slayer, target } => json!({
            "type": "slayer_miss", "slayer": slayer.0, "target": target.0
        }),
        PublicEvent::PhaseChanged { summary } => {
            json!({ "type": "phase_changed", "summary": summary })
        }
        PublicEvent::GameEnded { winner } => json!({
            "type": "game_ended",
            "winner": format!("{winner:?}")
        }),
    }
}

fn private_message_json(m: &PrivateMessage) -> Value {
    match m {
        PrivateMessage::YouAre {
            character_label,
            team,
            rules_path,
            note,
        } => json!({
            "type": "you_are",
            "character_label": character_label,
            "team": format!("{team:?}"),
            "rules_path": rules_path,
            "note": note,
        }),
        PrivateMessage::NightPrompt { text } => json!({ "type": "night_prompt", "text": text }),
        PrivateMessage::NightResult { text } => json!({ "type": "night_result", "text": text }),
        PrivateMessage::EvilBriefing { text } => json!({ "type": "evil_briefing", "text": text }),
        PrivateMessage::System { text } => json!({ "type": "system", "text": text }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(store: &SharedStore, name: &str, args: Value) -> Value {
        let line = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": { "name": name, "arguments": args }
        })
        .to_string();
        let resp = handle_line(store, &line).expect("response");
        serde_json::from_str(&resp).expect("json")
    }

    #[test]
    fn create_game_via_tools_call() {
        let store = new_shared_store();
        let resp = call(
            &store,
            "create_game",
            json!({
                "names": ["A", "B", "C", "D", "E"],
                "seed": 42
            }),
        );
        assert!(resp.get("error").is_none(), "{resp}");
        let sc = &resp["result"]["structuredContent"];
        assert_eq!(sc["game_id"], 1);
        assert!(sc["host_token"].as_str().unwrap().len() == 36);
        assert_eq!(sc["players"].as_array().unwrap().len(), 5);
    }

    #[test]
    fn create_game_omitted_seed_is_not_zero() {
        let store = new_shared_store();
        let resp = call(
            &store,
            "create_game",
            json!({
                "names": ["A", "B", "C", "D", "E"]
            }),
        );
        assert!(resp.get("error").is_none(), "{resp}");
        let sc = &resp["result"]["structuredContent"];
        let host = sc["host_token"].as_str().unwrap();
        let game_id = sc["game_id"].as_u64().unwrap();
        let host_resp = call(
            &store,
            "get_host_state",
            json!({ "game_id": game_id, "host_token": host }),
        );
        assert!(host_resp.get("error").is_none(), "{host_resp}");
        let seed = host_resp["result"]["structuredContent"]["seed"]
            .as_u64()
            .expect("seed");
        let salt = host_resp["result"]["structuredContent"]["secret_salt"]
            .as_u64()
            .expect("secret_salt");
        // CSPRNG seed: must not silently default to 0 (issue #1 #4).
        assert_ne!(seed, 0, "omitted seed must not default to 0");
        // secret_salt is present for host (player views never get it).
        let _ = salt;
    }

    #[test]
    fn tools_list_includes_create_game() {
        let store = new_shared_store();
        let line = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
        let resp: Value = serde_json::from_str(&handle_line(&store, line).unwrap()).unwrap();
        let tools = resp["result"]["tools"].as_array().unwrap();
        assert!(tools.iter().any(|t| t["name"] == "create_game"));
    }

    #[test]
    fn initialize_ok() {
        let store = new_shared_store();
        let line = r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{}}"#;
        let resp: Value = serde_json::from_str(&handle_line(&store, line).unwrap()).unwrap();
        assert_eq!(resp["result"]["serverInfo"]["name"], "botc-mcp");
    }
}
