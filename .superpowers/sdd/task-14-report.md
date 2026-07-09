# Task 14 Report — MCP server binary (transport)

## Summary

Stdio JSON-RPC 2.0 transport exposing all Task-12 tools via shared `Arc<Mutex<GameStore>>`. Full `rmcp` deferred (dependency-light stub documented in `docs/mcp.md`).

## Deliverables

| File | What |
| --- | --- |
| `src/mcp_server.rs` | Line-delimited JSON-RPC: `initialize`, `tools/list`, `tools/call`, tool-name aliases; wires all 17 tools |
| `src/main.rs` | `run_stdio` entry + existing smoke tests retained |
| `src/lib.rs` | `pub mod mcp_server` |
| `docs/mcp.md` | Protocol, tool args, payload shapes, smoke commands |
| `examples/harness_smoke.rs` | In-process create_game + start_game smoke |

## Tools wired

`create_game`, `start_game`, `get_public_state`, `get_public_log`, `get_private_state`, `get_character_rules`, `get_host_state`, `say`, `st_announce`, `night_action`, `day_action`, `nominate`, `vote`, `open_nominations`, `close_vote`, `end_nominations`, `skip_night_action`

## Research note

`rmcp` 2.2.0 is on crates.io (tokio + schemars, `transport-io`). Not pulled: view DTOs lack `Serialize`/schemars, and a thin MCP-shaped stub is enough for harnesses. Handlers stay in `tools::*` for a later swap.

## Verification

- `cargo build` — ok
- `cargo test` — green (incl. 3 mcp_server unit tests)
- `cargo run --example harness_smoke` — create_game + start_game OK
- Stdio pipe: `initialize` + `create_game` return structured tokens

## Commit

`feat: MCP server transport wiring`
