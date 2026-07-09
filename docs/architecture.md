# Architecture sketch — botc-mcp

MCP server that runs one **Trouble Brewing** game for model agents: authoritative
Storyteller state, public agent-to-agent chat, private Storyteller→player channels only.

This document is the design target. Implementation lives under `src/`.

---

## Goals

1. **Server assigns roles.** Agents never pick a mechanical role.
2. **Private identity & ST info** are token-scoped; other agents cannot read them.
3. **Public speech is fully public.** No whispers, private DMs, or side channels between agents.
4. **Abilities resolve from Grimoire truth**, not from what a player claims to be.
5. **Bluffing is free in chat.** Claims are not state.

---

## Actors

| Actor | Credential | Powers |
| --- | --- | --- |
| **Host / Storyteller** | `host_token` | Create game, assign seats, advance phase, broadcast ST public events |
| **Player agent** | `player_token` (one per seat) | Chat publicly, nominate/vote, submit night/day actions when eligible, read own private view + public state |

There is no “spectator write” path. Eval harnesses use the host token for setup/control.

---

## Session & isolation

```
create_game  →  game_id + host_token
join_game    →  player_token bound to (game_id, seat_id)
every tool   →  requires host_token XOR player_token
```

- Tokens are opaque random secrets (not seat indices).
- Tool handlers resolve token → `Actor` → authorized views only.
- **Never** accept `seat_id` alone as proof of identity.
- One MCP process may host multiple games; state is keyed by `game_id`.

### What a player token can see

| Data | Visible? |
| --- | --- |
| Public phase, living/dead, nomination/vote log | Yes |
| Full public chat transcript | Yes |
| ST public announcements (dawn deaths, “open nominations”, execution result) | Yes |
| Own believed character + ability text | Yes (`get_private_state`) |
| Own night/day private results from ST | Yes (same + action results) |
| Other seats’ true characters | **No** |
| Other seats’ private ST messages | **No** |
| Grimoire | **No** (except Spy via ability resolution, delivered only to Spy’s private view) |
| Other agents’ tool calls / private_state | **No** |

### What chat can do

| Channel | Allowed? |
| --- | --- |
| Public table talk (`say`) | Yes — all players + host see it |
| Player ↔ player private | **No** (not implemented; reject if ever requested) |
| Storyteller → one player (ability info, briefing) | Yes — private inbox on that seat only |
| Storyteller → all (dawn, nominations open, etc.) | Yes — public event log |

---

## Game loop (server-driven)

```
Lobby → Setup → FirstNight → Day → Night → Day → … → Ended
```

Host (or auto-rules later) calls `advance_phase` when the current step is complete.

### Night (sketch)

1. Phase = `Night { index, step }` walking [night-order](night-order.md).
2. For each wake: mark seat `AwaitingNightAction` (or skip if dead / N/A).
3. Eligible player calls `night_action(player_token, payload)`.
4. Server validates against **true** character + drunk/poisoned; writes private result to that seat’s inbox; may update Grimoire.
5. When order complete → host advances → public dawn announcement (who died only).

### Day (sketch)

1. Public discussion via `say` (no time limit enforced initially; host advances).
2. `nominate` / `vote` with public visibility of nominations and tallies.
3. At most one execution; win check; host advances to night or `Ended`.

---

## Tool surface (MCP)

Auth: every tool takes `token` (host or player). Server rejects wrong role for the tool.

### Lifecycle

| Tool | Auth | Notes |
| --- | --- | --- |
| `create_game` | none or deploy secret | Returns `game_id`, `host_token`, seat list placeholders |
| `join_game` | none | `{ game_id, seat_id or display_name }` → `player_token` + public seat map |
| `start_game` | host | Locks joins, builds bag, assigns characters, enters First Night |
| `advance_phase` | host | Dawn, open nominations, close day, etc., when legal |

### Read models

| Tool | Auth | Returns |
| --- | --- | --- |
| `get_public_state` | any token in game | Phase, seats (name, alive, ghost_vote_available), nominations today, vote history summary, winner if any |
| `get_public_log` | any | Append-only **public** events: chat + ST broadcasts + nominations/votes/executions/deaths |
| `get_private_state` | player | Believed character, team as known, ability summary/path, private inbox, whether ST expects an action now |
| `get_character_rules` | any | Public sheet entry for one character name (pool knowledge; not “who is it”) |

### Communication

| Tool | Auth | Behavior |
| --- | --- | --- |
| `say` | player (alive or dead) | Appends `{ seat, name, text, ts }` to **public** log. Visible to all via `get_public_log` / subscriptions. **No** `to_seat` parameter. |

Future (explicitly out of scope now): `whisper`, private rooms, evil-only chat.

### Actions

| Tool | Auth | Behavior |
| --- | --- | --- |
| `night_action` | player | Payload is generic choices (`target`, `targets`, `character_guess`, …). Server maps seat → true role → validate → resolve. |
| `day_action` | player | e.g. `slay { target }` for Slayer once-per-game when legal |
| `nominate` | player | Public; living only; once per day per nominator/nominee rules |
| `vote` | player | Public; living unlimited per day; dead one ghost vote total |

No `use_ability(role=...)`. Role is never a client-supplied mechanical field.

---

## Public log vs private inbox

### Public log (`PublicEvent`)

Everyone with a game token can read the full sequence:

- `Chat { seat, name, text }`
- `StorytellerAnnounce { text }` — e.g. “Night has fallen”, “X died in the night”, “Nominations are open”
- `Nominated { by, target }`
- `VoteCast { seat, target, hand: Up/Down }` (or only tallies if we prefer less spam)
- `Executed { seat }` / `NoExecution`
- `GameEnded { winner: Good | Evil }`

### Private inbox (`PrivateMessage`)

Only `get_private_state` for that `player_token`:

- Role assignment / Drunk false identity
- First-night evil briefing
- Night ability prompts and results (“Empath: 1”, “Fortune Teller: yes”)
- “You are now the Imp” (SW / starpass)
- Spy Grimoire snapshot (structured, not shared publicly)

Storyteller **never** posts private content into the public log.

---

## Role assignment (init)

On `start_game`:

1. Host supplies player count seats already joined (or bag seed).
2. Server builds composition from [setup](setup.md), applies Baron/Drunk, shuffles.
3. Each seat gets `true_character` in Grimoire.
4. Drunk: `believed_character = Some(Townsfolk)`; player private view shows believed, not Drunk.
5. Red herring seat if Fortune Teller in play.
6. Players call `get_private_state` — that is their “init”.

Agents do not call `claim_role` / `set_role`.

---

## Ability resolution principle

```
night_action(token, choice)
  seat = auth(token)
  true = grimoire[seat].character
  if drunk/poisoned → fake path (false info / no effect, spend once-per-game if attempted)
  else match true ability → validate choice → mutate grimoire
  push PrivateMessage to seat
  // public log unchanged unless ability has public day effect (Slayer kill, Virgin bounce)
```

Registration (Spy/Recluse/red herring) applied when **reading** for an info ability, not when the target chats.

---

## Module map (`src/`)

| Module | Responsibility |
| --- | --- |
| `auth` | Tokens, `Actor::Host | Player { seat }` |
| `roles` | Character enum, team, type, static data (night\* flags) |
| `game` | `Game`, phase machine, grimoire, seats, win checks |
| `comms` | Public log + per-seat private inbox |
| `tools` | MCP tool handlers → game methods (thin) |
| `main` | Process entry / later MCP transport |

Rules text remains in `docs/`; engine may load role markdown by path for `get_character_rules` / private ability blurb.

---

## Multi-agent orchestration (eval harness)

Suggested outer loop (not necessarily inside this repo):

```
1. create_game + N × join_game → tokens
2. start_game
3. Each agent: get_private_state once
4. Loop:
     a. Each agent: get_public_log (cursor) + get_public_state
     b. Agents may say(...) any number of times  // all public
     c. If private prompt waiting: night_action / day_action / nominate / vote
     d. Host: advance_phase when constraints met
5. Until GameEnded
```

Because chat is public-only, the harness can feed **the same public log** to every agent. Private state is fetched **per agent** with that agent’s token only—never pasted into a shared context.

---

## Non-goals (for this sketch)

- Private player–player communication
- Travellers / other editions
- Perfect Storyteller AI policy (false-info heuristics can be stubbed)
- Real-time push (polling `get_public_log` since cursor is enough for v1)

---

## Security / eval integrity notes

- Treat `player_token` like a password; harness must not put player A’s token in player B’s prompt.
- Log redaction: host debug dumps are not player tools.
- Deterministic seed option on `create_game` for reproducible evals.
