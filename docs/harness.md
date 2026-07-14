# Multi-agent TUI harness

`botc-tui` is a **monitoring + orchestration** front-end for multi-model Trouble Brewing runs.

It:

1. Creates one shared in-process game (Storyteller authority).
2. Exposes that game over a **Unix-socket tool RPC**.
3. Spawns **N player + 1 host** headless [Grok Build](https://grok.x.ai) sessions.
4. Each session loads a project-scoped MCP config pointing at `botc-agent-mcp`, a stdio MCP proxy that injects that agent’s token and forwards tools to the shared engine.
5. Shows a **ratatui** board: fused agents+grimoire panel (with live noms), a live **action feed** (every agent tool call, game actions highlighted), and the selected agent's stream.

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

The setup screen is a roster: the player-count row on top, then **one model row per
session** (Host, P0, P1, …).

| Key | Action |
| --- | --- |
| `↑` / `↓` | Move between rows (count row / Host / P0 / P1 / …) |
| `←` / `→` or `m` / `M` | Change the focused row: player count 5–15, or that **session's model** |
| `a` | Apply the focused row's model to **all** sessions |
| `Enter` | Create game, start night 1, spawn Grok agents |
| `q` | Quit (kills Grok children and removes workdirs) |

**Models are per session** — each agent (host and every seat) can run a different model,
e.g. a strong model as the Storyteller and mixed models across seats. Each pick becomes that
agent's `-m` on every headless `grok` spawn (and `--resume` ticks keep the same model). At
setup the TUI runs `grok models`, parses the available IDs + default, and uses that list for
every row's picker (see `discover_models` / `parse_grok_models_output` in
`src/harness/agents.rs`); the CLI default is every row's starting pick. If the CLI is missing
or returns nothing, the picker falls back to the current `model` string alone. The monitor
shows each agent's model in the agents list and the stream title.

### Monitor screen

Three columns — **left: board** (agents + grimoire + noms) · **center: action feed** · **right: selected agent stream**. The top bar shows live progress: `phase · turn <who> · auto Ns / manual · running: <who>`.

**Board (left).** Per agent, fused status + host grimoire — not a separate grimoire pane:

| Line | Content |
| --- | --- |
| Host | `●/○` running, model, phase, `pending_host` / night wake |
| Seat | `●/○`, true role (Drunk shows face), model; second line: alive/DEAD, team, poison, monk, butler master, ghost/slayer/virgin flags; third line: **token usage** |

**Usage line** (per agent): cumulative spend `Σ…`, last tick total, and context fill from the session (`ctx N% (used / window)`). Cyan under 50%, yellow 50–79%, red ≥80%. Values come from each headless tick’s streaming-json `end.usage` plus `signals.json` after the process exits.

Team colours match setup (Townsfolk green, Outsider yellow, Minion/Demon red). Below the roster, a **live noms tracker**: closed nominations with yes totals (and `≥½` when they meet threshold) plus the open vote with a running tally (`P0✓ P3✗ P1–` for pass).

| Key / mouse | Action |
| --- | --- |
| `Tab` / `Shift+Tab` / **click board row** | Select agent stream (resets to live tail) |
| `Space` | Advance **one turn** manually — ticks the agent(s) the engine is waiting on (only when idle) |
| `t` | Toggle **auto-advance** — next turn when all agents go idle |
| `f` | Feed filter: **all actions ↔ game-only** |
| `h` / **click stream** | Expand/collapse **thinking** for the selected agent (default: collapsed) |
| **click feed row** | Expand/collapse that action: full args (tokens redacted) + result/error |
| **wheel on feed** | Scroll the action feed history |
| `PgUp`/`PgDn`/`↑`/`↓` / **wheel on stream** | Scroll selected agent log |
| `Home` | Jump to live tail |
| `q` | Kill agents, remove workdirs, stop socket, quit |

**Action feed (center).** Always the action feed — no grimoire toggle. Every agent tool RPC, newest last, colour-coded by caller. **Game actions** highlighted (`▶` + cyan summary); info reads dimmed; errors red. **`f`** filters to game-only. **`say` / `st_announce`** show the full quote. **Click** a row to expand args/result.

**Stream pane:** the selected agent's raw output streams **live, token by token** (no
buffering until a block finishes) and is **coloured by kind** rather than tagged — model
text is default, **thinking is dim grey**, **stderr yellow**, and turn/error notices cyan/red.
Model thinking is **collapsed by default** (a one-line `· thinking… N line(s) …` stand-in) so
you see game-facing output first; **`h`** or **left-click the stream** expands that agent's
full thinking; again collapses. Expansion is **per agent**; the title shows `·think▾`/`·think▸`.
**Mouse wheel over the stream** scrolls history (same as PgUp/PgDn); wheel elsewhere is ignored.

## Turn order (scheduling)

Trouble Brewing is strictly **sequential**, so each tick wakes **exactly one agent** (plus,
rarely, a host fallback) with a targeted prompt that says why it was woken, which actions are
legal, and what ends the turn (`src/harness/scheduler.rs`, `plan_ticks`).

The host is **minimal**: it is woken only for genuine Storyteller decisions and stall
fallbacks. The engine self-drives the rest — a player's `nominate` auto-opens the vote,
`vote` auto-closes once everyone has acted, and the day auto-ends into night.

| Game state | Ticked this turn |
| --- | --- |
| Lobby | Host → `start_game` (normally already done by the TUI at launch) |
| Night, `pending_host` set | Host → resolve that one Storyteller decision |
| Night, `pending_night` set | **only that seat** → its night action (targeted wake prompt) |
| Day / Discussion | **one living player at a time**, seat order, `DISCUSSION_ROUNDS` (2) full table rounds — no host |
| Day / Discussion, rounds spent | Host → `open_nominations` + `end_nominations` (close the day) |
| Day / Nominations, no open vote | one living player who hasn't nominated yet (rotating) — no host |
| Day / Nominations, vote open | **one voter at a time, clockwise from the nominee**, shown the tally so far |
| Ended | nobody — auto-tick disarms |

**Stall escalation.** If the engine sits on the same wait for several cycles, the scheduler
escalates: a stuck night wake → host `skip_night_action`; a stalled **voter** first yields the
floor (each stalled cycle offers the *next* pending voter, so nobody blocks the queue) and only
after every pending voter has been offered does the host `close_vote` (missing votes count
"no"); nobody nominating after everyone had a turn → host `end_nominations`. **The escalation
tick is host-only** — the stuck agent is not co-scheduled, so the host override can never race
a recovering player. Signatures include progress counts and the living roster, so every landed
vote, new nomination, or death resets the window. A host that repeats the same fallback 5×
without progress stops auto-advance (`t` resumes).

The one-time **kickoff** still fans out to every agent (orientation only — the first night has
no talking); ongoing play is one turn at a time, so every speaker sees everything said before
them and votes are cast in clockwise order like the tabletop game.

## Results log (ranking corpus)

Alongside the debug log, the TUI **appends** ranking-relevant outcomes to a JSONL file
(default `botc-results.jsonl` at the repo root, override with `BOTC_RESULTS_LOG`). One JSON object per
line, schema field `v` (currently `1`). Survives quit; not under the deleted work root.

| `event` | When | Useful for |
| --- | --- | --- |
| `game_start` | After launch | Per-seat `model`, `true_character`, `team`, seed, host model |
| `death` | Exec / night / day kill | Survival metrics (`cause`: `executed` \| `night` \| `day`) |
| `nomination` | Someone is put on the block | Social targeting later |
| `game_end` | Engine `Ended` | `winner`, `reason`, per-seat `won` |
| `game_abort` | Quit before end | Filter incomplete runs |
| `tick_usage` | Each headless tick with `end.usage` | Token spend + cumulative + optional context window |

Chat and full tool traces are **not** in this file (see the debug log / action feed for those).

## Debug log

The TUI writes a **verbose, timestamped** trace to a file (the full-screen UI can't use
stdout). Default `/tmp/botc-tui-debug.log` (override with `BOTC_TUI_LOG`); the path is shown
in the status line at launch, and it survives quit (it is **not** under the deleted work root).

Each line is `[HH:MM:SS.mmm +elapsedms] …`. It records the state machine end to end:

- `LAUNCH` — game id, player count, ST mode, token count.
- `TICK` — every scheduler cycle: `sig` (what the engine waits on), `stall`, live `phase` /
  `pending_host` / `pending_night` / nomination state, and the routed `plan=[…]`.
- `SPAWN` / `EXIT` — per agent: resume-vs-fresh, session id, first prompt line, full argv;
  and on exit the code, whether the session was `established`, and any id regeneration.
  (`SPAWN … SKIPPED` is a defensive no-op that should not appear now that ticks only fire when idle.)
- `RPC` — every tool call through the socket: `actor tool args=… -> ok/ERR`.

So a stuck host shows up plainly: a `SPAWN Host` with no following `EXIT Host` for a long time
means the tick hung (the game waits for it — it is never skipped); repeated `TICK … plan=[Host]`
with no `RPC Host …` between them means the host agent isn't calling any tools.

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
- **Agents are confined to *playing*, not exploring.** `grok-build` is a software-engineering
  agent by default — given a shell it will `find`/`read` the filesystem to locate the game's
  source or *other seats' `agent.token` files*. So each headless agent is launched with the
  built-in **file/shell tools removed** (`--disallowed-tools run_terminal_command,read_file,
  list_dir,search_replace,grep,…`), leaving only the MCP dispatch tools (`search_tool`/`use_tool`)
  and planning — the game is played entirely through the `botc` MCP server, so nothing is lost.
  It also runs with `--no-memory` (don't inherit the user's global `~/.grok/AGENTS.md` / skills /
  MCP coding context) and `--sandbox` (fs/network confinement, defense in depth). All three are
  fields on `HarnessConfig` (`disallowed_tools`, `no_memory`, `grok_sandbox`). Removals must be
  self-consistent — `search_replace` (edit) requires `read_file`, so both are removed together or
  grok refuses to start.

## Limitations (v1)

- Agents are driven by **event-driven headless ticks** (`grok --prompt-file … --resume`), not a single eternal ACP connection: the next turn is ticked when all agents go idle, and a running agent is never skipped (a hung agent will hold the game until it exits — watch the debug log).
- Auto-approve uses a **single** flag (`--yolo`). Do not also pass `--always-approve` (same clap option → CLI error).
- The next turn is only ticked once **all** agents are idle, so a per-seat tick is never spawned on top of a still-running one.
- First-run success is required before `--resume` is used; a failed kickoff retries with a fresh `--session-id`.
- Host-first Storyteller pauses require the **host** Grok agent (or skip defaults via host tools) to resolve night info.
- Cost: N+1 model sessions, but the turn-router ticks only the 1–2 agents whose turn it is (plus a fan-out during an open vote), not all N+1 each cycle.
- Ticks are **turn-routed** (see *Turn order*): one agent per tick in every phase, so there are no concurrent day actions to conflict.
- Progression is player/engine-driven; the host is only needed for `pending_host` decisions and stall fallbacks. Dead players don't get speaking turns (they still vote with their ghost vote); day length is bounded by `DISCUSSION_ROUNDS`.

## Tests

| Suite | Command |
| --- | --- |
| Default (no live Grok) | `cargo test` |
| Socket / arg construction / scroll / stop | included in default suite |
| Live Grok smoke (network + auth) | `cargo test --test harness_grok_live -- --ignored --nocapture` |

## Related

- Wire protocol for a single server: [`mcp.md`](mcp.md)
- Eval orchestration notes: [`architecture.md`](architecture.md)
