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
| `Space` | Advance **one turn** — ticks only the agent(s) the engine is waiting on |
| `t` | Toggle auto-tick (~45s), which runs the same turn-routed step |
| `PgUp` / `PgDn` | Scroll selected agent log (away from / toward live tail) |
| `Home` | Jump to live tail |
| `q` | Kill agents, remove workdirs, stop socket, quit |

Left: agents · Center: **host grimoire** (true roles) · Right: selected agent’s stream (**newest lines visible by default**).

## Turn order (scheduling)

Trouble Brewing is strictly **sequential**, so a tick does **not** wake every agent — it
routes to the agent(s) the engine is actually waiting on, with a targeted prompt
(`src/harness/scheduler.rs`, `plan_ticks`):

| Game state | Ticked this turn |
| --- | --- |
| Lobby | Host → `start_game` |
| Night, `pending_host` set | Host → resolve the Storyteller decision |
| Night, `pending_night` set | **only that seat** → its night action (targeted wake prompt) |
| Night, nothing pending | Host → advance the night machine |
| Day / Discussion | Host (pace / open nominations) + one living player (round-robin) |
| Day / Nominations, no open vote | Host (manage) + one living player (round-robin) to nominate |
| Day / Nominations, vote open | the eligible voters who haven't voted (host is **not** co-scheduled to close — it would race the votes) |
| Ended | nobody — auto-tick disarms |

**Stall escalation.** If the engine sits on the same wait for `STALL_ESCALATE` (3)
consecutive cycles — a woken player never submits its night action, or a vote stops
progressing — the plan adds a host fallback (`skip_night_action` for the stuck wake, or
`close_vote` for the stalled vote) so a non-acting agent can't wedge the game. Normal
turns route to only the responsible agent; the host override kicks in only after it
demonstrably fails to act.

The one-time **kickoff** still fans out to every agent (each introduces itself); ongoing
play is turn-routed. This is why, with the host driving, agents act on their turn instead
of all polling tools at once.

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
- Cost: N+1 model sessions, but the turn-router ticks only the 1–2 agents whose turn it is (plus a fan-out during an open vote), not all N+1 each cycle.
- Ticks are **turn-routed** (see *Turn order*), so the night is sequential; day discussion/voting still allows a few agents per cycle, and the engine is mutex-serialized, so conflicting day actions are still possible during discussion but no longer the default.
- Progression depends on the **host** agent driving phase transitions (open/close nominations, advance the night); if it stalls, press `Space` to re-tick or resolve via host tools.

## Tests

| Suite | Command |
| --- | --- |
| Default (no live Grok) | `cargo test` |
| Socket / arg construction / scroll / stop | included in default suite |
| Live Grok smoke (network + auth) | `cargo test --test harness_grok_live -- --ignored --nocapture` |

## Related

- Wire protocol for a single server: [`mcp.md`](mcp.md)
- Eval orchestration notes: [`architecture.md`](architecture.md)
