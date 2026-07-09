# MCP transport (botc-mcp)

## Overview

The `botc-mcp` binary speaks **line-delimited JSON-RPC 2.0** on **stdin/stdout**.

This is a thin MCP-compatible stub (not the full `rmcp` SDK). It implements the
methods a harness needs to discover and call tools. Semantic handlers live in
`src/tools/`; transport wiring is `src/mcp_server.rs`.

```
stdin  → one JSON-RPC request per line
stdout → one JSON-RPC response per line (except notifications with no `id`)
```

Shared process state: `Arc<Mutex<GameStore>>` (multiple concurrent games by `game_id`).

## Methods

| Method | Purpose |
| --- | --- |
| `initialize` | Returns `protocolVersion`, `capabilities.tools`, `serverInfo` |
| `tools/list` | Lists tool names + short descriptions |
| `tools/call` | `{ "name": "<tool>", "arguments": { ... } }` |
| `ping` | Health check |
| *tool name* | Convenience alias: method may be the tool name with args as `params` |

### `tools/call` result shape

Successful tool execution:

```json
{
  "content": [{ "type": "text", "text": "<json string of structuredContent>" }],
  "structuredContent": { /* tool-specific object */ },
  "isError": false
}
```

Engine / auth failures return RPC success with `"isError": true` and a text
message (MCP tool-error style). Malformed arguments use JSON-RPC error code
`-32602`.

## Tools

Auth: game tools take `game_id` plus `token` (or `host_token` / `player_token`).

| Tool | Key arguments | Notes |
| --- | --- | --- |
| `create_game` | `names: string[]`, `seed?: u64` | Returns `game_id`, `host_token`, `players[]`. If `seed` is omitted, the server draws a CSPRNG seed (never defaults to `0`). Each game also gets a host-only `secret_salt` mixed into every RNG substream so public labels alone cannot reconstruct draws; `get_host_state` exposes `seed` + `secret_salt`. |
| `start_game` | `game_id`, `host_token`, `assignments?` | Optional fixed roles; Drunk needs `believed` |
| `get_public_state` | `game_id`, `token` | No roles / no pending night seat |
| `get_public_log` | `game_id`, `token`, `cursor?` | Events with id > cursor |
| `get_private_state` | `game_id`, `token`, `private_cursor?` | Drunk → Townsfolk face only |
| `get_host_state` | `game_id`, `host_token` | Full grimoire + `seed` + `secret_salt` (never on player views) |
| `get_character_rules` | `character` | Public sheet markdown (no `game_id`) |
| `say` | `game_id`, `token`, `text` | Public chat only |
| `st_announce` | `game_id`, `host_token`, `text` | Host public ST line |
| `night_action` | `game_id`, `token`, payload | See below |
| `day_action` | `game_id`, `token`, `target` | Slayer slay |
| `nominate` | `game_id`, `token`, `target` | Seat id |
| `vote` | `game_id`, `token`, `nominee`, `support` | bool |
| `open_nominations` | `game_id`, `host_token` | Host |
| `close_vote` | `game_id`, `host_token` | Host |
| `end_nominations` | `game_id`, `host_token` | Host |
| `skip_night_action` | `game_id`, `host_token` | Host default for pending wake |

### `night_action` payload

Accepts nested `payload` or flat fields:

- Ack: `{ "kind": "ack" }` or `{ "ack": true }` (also default if no targets)
- Pick one: `{ "kind": "pick_one", "target": 0 }` or `{ "target": 0 }`
- Pick two: `{ "kind": "pick_two", "a": 0, "b": 1 }` or `{ "targets": [0, 1] }`
- Pick character: `{ "kind": "pick_character", "name": "Empath" }`

### `start_game` assignments

```json
{
  "game_id": 1,
  "host_token": "...",
  "assignments": [
    { "seat": 0, "character": "Empath" },
    { "seat": 1, "character": "Drunk", "believed": "Soldier" },
    { "seat": 4, "character": "Imp" }
  ]
}
```

Omit `assignments` for seeded random bag.

## Smoke example

```bash
cargo build
printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"create_game","arguments":{"names":["A","B","C","D","E"],"seed":1}}}' \
  | cargo run -q
```

Or: `cargo run --example harness_smoke`.

## Why not `rmcp`?

`rmcp` 2.x is available on crates.io (tokio + schemars). This crate pins a
minimal stdio JSON-RPC surface so the engine stays dependency-light and tests
remain sync. A future task may swap the transport for official `rmcp` while
keeping `tools::*` handlers unchanged.
