# Multi-agent TUI harness

`botc-tui` is a **monitoring + orchestration** front-end for multi-model Trouble Brewing runs.

It:

1. Creates one shared in-process game (Storyteller authority).
2. Exposes that game over a **Unix-socket tool RPC**.
3. Spawns **N player + 1 host** headless [Grok Build](https://grok.x.ai) sessions.
4. Each session loads a project-scoped MCP config pointing at `botc-agent-mcp`, a stdio MCP proxy that injects that agent’s token and forwards tools to the shared engine.
5. Shows a **ratatui** board: agent list, host grimoire, and live agent streams.

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

```bash
cargo build --bins
# produces: target/debug/botc-tui  target/debug/botc-agent-mcp  target/debug/botc-mcp
```

Ensure `grok` is on your `PATH` (or `~/.grok/bin/grok`) and you are authenticated (`grok login` or `XAI_API_KEY`).

## Run

```bash
cargo run --bin botc-tui
```

### Setup screen

| Key | Action |
| --- | --- |
| `↑` / `↓` | Player count 5–15 (**sessions = players + 1 host**) |
| `Enter` | Create game, start night 1, spawn Grok agents |
| `q` | Quit |

### Monitor screen

| Key | Action |
| --- | --- |
| `Tab` / `Shift+Tab` | Select agent stream |
| `Space` | Kick every agent for another multi-turn tick |
| `t` | Toggle auto-tick (~45s) |
| `PgUp` / `PgDn` | Scroll selected agent log |
| `q` | Stop agents and quit |

Left: agents · Center: **host grimoire** (true roles) · Right: selected agent’s streamed output.

## Workdirs

Each agent gets `/tmp/botc-harness-<uuid>/{host,seat0,…}/`:

- `.grok/config.toml` — project MCP → `botc-agent-mcp --socket … --token-file … --game-id …`
- `agent.token` — opaque seat/host secret (not shared across agents)
- `prompt.txt` — last kickoff/tick prompt

## Security notes

- Tokens are **never** put in another agent’s prompt or MCP config.
- Player proxies always inject their own token; they cannot escalate to host unless given the host token file.
- The TUI process holds the real grimoire for monitoring only.

## Limitations (v1)

- Agents are driven by **periodic headless ticks** (`grok --prompt-file … --resume`), not a single eternal ACP connection.
- Host-first Storyteller pauses require the **host** Grok agent (or skip defaults via host tools) to resolve night info.
- Cost: N+1 model sessions. Start with 5 players.
- Concurrent ticks can race; the engine is mutex-serialized, but agents may issue conflicting day actions.

## Related

- Wire protocol for a single server: [`mcp.md`](mcp.md)
- Eval orchestration notes: [`architecture.md`](architecture.md)
