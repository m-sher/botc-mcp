# botc-mcp

Rust **MCP server** for model agents to play **Blood on the Clocktower** —
**Trouble Brewing** only. Authoritative Storyteller state, public table chat,
token-scoped private ST→player info.

## Status

**Implemented:** full TB engine + tool surface + line-delimited JSON-RPC MCP
stdio transport (thin stub; full `rmcp` SDK deferred). See design status in the
spec linked below.

## Docs

| Doc | Purpose |
| --- | --- |
| [`AGENTS.md`](AGENTS.md) | Implementer rules (Drunk face, auth, no whispers) |
| [`docs/architecture.md`](docs/architecture.md) | Architecture & module map |
| [`docs/mcp.md`](docs/mcp.md) | MCP JSON-RPC wire protocol & tools |
| [`docs/superpowers/specs/2026-07-08-botc-mcp-engine-design.md`](docs/superpowers/specs/2026-07-08-botc-mcp-engine-design.md) | Engine design spec |
| [`docs/superpowers/plans/2026-07-08-botc-mcp-engine.md`](docs/superpowers/plans/2026-07-08-botc-mcp-engine.md) | Implementation plan |
| [`docs/README.md`](docs/README.md) | Simulation rules index (roles, night order, …) |

## Development

Requires a recent stable Rust toolchain.

```bash
# Build library + binaries
cargo build --bins

# Unit + integration tests (scenarios under tests/)
cargo test

# MCP server on stdin/stdout (line-delimited JSON-RPC 2.0)
cargo run --bin botc-mcp

# Multi-agent monitoring TUI (spawns headless Grok sessions)
cargo run --bin botc-tui
```

See [`docs/harness.md`](docs/harness.md) for the multi-agent harness architecture.

Protocol and tool arguments: [`docs/mcp.md`](docs/mcp.md).

In-process smoke (no stdio):

```bash
cargo run --example harness_smoke
```

Rust modules: `src/` — `auth`, `comms`, `game`, `roles`, `tools`, `store`,
`mcp_server`, `rng`, `error`.

## License

Code: MIT OR Apache-2.0 (see `Cargo.toml`).  
Rules text: paraphrased reference; game © The Pandemonium Institute.
