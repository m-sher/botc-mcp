# Multi-agent TUI harness

`botc-tui` is a **monitoring + orchestration** front-end for multi-model Trouble Brewing runs.

It:

1. Creates one shared in-process game (Storyteller authority).
2. Exposes that game over a **Unix-socket tool RPC**.
3. Spawns **N player + 1 host** headless [Grok Build](https://grok.x.ai) sessions.
4. Each session loads a project-scoped MCP config pointing at `botc-agent-mcp`, a stdio MCP proxy that injects that agent’s token and forwards tools to the shared engine.
5. Shows a **ratatui** board: agent list, host grimoire, and live agent streams (tail-anchored).

```
┌────────────────────────────────────────────────────────────┐
│ botc-tui (ratatui)                                         │
│  · SharedStore (one game)                                  │
│  · SocketServer ── engine.sock                             │
│  · polls grimoire / public log for display                 │
└───────────────┬────────────────────────────────────────────┘
                │ Unix socket tool RPC
     ┌──────────┼──────────┬──────────┐
     ▼          ▼          ▼          ▼
 botc-agent-mcp … (stdio MCP, one per Grok session)
     ▲          ▲          ▲          ▲
     │          │          │          │
  grok -p    grok -p    grok -p    grok -p
  (host)     (P0)       (P1)       …
```

## Build

**Both binaries are required.** `botc-tui` spawns `botc-agent-mcp` as the per-agent MCP proxy; if the proxy is missing, agents start but cannot call tools.

```bash
cargo build --bins
# produces: target/debug/botc-tui  target/debug/botc-agent-mcp  target/debug/botc-mcp
```

Ensure `grok` is on your `PATH` (or `~/.grok/bin/grok`) and you are authenticated (`grok login` or `XAI_API_KEY`).

## Run

```bash
# Preferred: build all bins, then run the TUI
cargo build --bins
cargo run --bin botc-tui

# Or run the built binary directly (so sibling botc-agent-mcp is found next to it):
./target/debug/botc-tui
```

`cargo run --bin botc-tui` alone only ensures `botc-tui` is built. The TUI checks that `botc-agent-mcp` exists at launch and refuses to spawn agents with a clear error if it does not.

### Setup screen

| Key | Action |
| --- | --- |
| `↑` / `↓` | Player count 5–15 (**sessions = players + 1 host**) |
| `Enter` | Create game, start night 1, spawn Grok agents |
| `q` | Quit (kills Grok children and removes workdirs) |

### Monitor screen

| Key | Action |
| --- | --- |
| `Tab` / `Shift+Tab` | Select agent stream (resets to live tail) |
| `Space` | Kick every agent for another multi-turn tick |
| `t` | Toggle auto-tick (~45s) |
| `PgUp` / `PgDn` | Scroll selected agent log (away from / toward live tail) |
| `Home` | Jump to live tail |
| `q` | Kill agents, remove workdirs, stop socket, quit |

Left: agents · Center: **host grimoire** (true roles) · Right: selected agent’s stream (**newest lines visible by default**).

## Workdirs

Each agent gets `/tmp/botc-harness-<uuid>/{host,seat0,…}/`:

- `.grok/config.toml` — project MCP → `botc-agent-mcp --socket … --token-file … --game-id … --role …`
- `agent.token` — opaque seat/host secret (not shared across agents)
- `prompt.txt` — last kickoff/tick prompt

On quit (`q`) or process exit, the harness **kills** all Grok children and **removes** the work root (token files do not linger).

## Security notes

- Tokens are **never** put in another agent’s prompt or MCP config.
- Player proxies always inject their own token; they cannot escalate to host unless given the host token file.
- Role ACL in `botc-agent-mcp` denies host-only tools for player agents (JSON-RPC `-32602`).
- The TUI process holds the real grimoire for monitoring only.

## Limitations (v1)

- Agents are driven by **periodic headless ticks** (`grok --prompt-file … --resume`), not a single eternal ACP connection.
- Auto-approve uses a **single** flag (`--yolo`). Do not also pass `--always-approve` (same clap option → CLI error).
- A tick is skipped per agent if the previous Grok process for that seat is still running.
- First-run success is required before `--resume` is used; a failed kickoff retries with a fresh `--session-id`.
- Host-first Storyteller pauses require the **host** Grok agent (or skip defaults via host tools) to resolve night info.
- Cost: N+1 model sessions. Start with 5 players.
- Concurrent agents may issue conflicting day actions; the engine is mutex-serialized but not turn-locked.

## Tests

| Suite | Command |
| --- | --- |
| Default (no live Grok) | `cargo test` |
| Socket / arg construction / scroll / stop | included in default suite |
| Live Grok smoke (network + auth) | `cargo test --test harness_grok_live -- --ignored --nocapture` |

## Related

- Wire protocol for a single server: [`mcp.md`](mcp.md)
- Eval orchestration notes: [`architecture.md`](architecture.md)
