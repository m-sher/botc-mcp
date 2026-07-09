# Agent / developer rules (botc-mcp)

Rules for humans and coding agents **working on this repository**.  
Game rules for play are under `docs/`. This file is about **how to build the system**.

## Scope of the product

- One gameplay loop: **Trouble Brewing** only.
- MCP for **model agents to play**, with server-side Storyteller authority.
- Public agent chat only (no player–player private channels).
- Private Storyteller → player info is token-scoped.

Do not reintroduce other editions, Travellers, Fabled, or whisper/DM features unless the user explicitly asks.

## Authoritative state vs player view

The engine is the source of truth (Grimoire). Agents never set their own mechanical role.

| Layer | Storage | Exposed to player tools? |
| --- | --- | --- |
| True character / effects | Grimoire | **No** (except via legal ability *results*, not raw dumps) |
| Player-facing identity | Derived / `believed_character` | **Yes** — `get_private_state` and private briefings |

### Drunk and similar false-identity effects (required)

**The Drunk must not know they are the Drunk.** The same pattern applies to any effect where the rules say the player is unaware.

When implementing role info (including `get_private_state`, join/start briefings, “who am I?”, ability doc paths for *my* role):

1. If the seat is the **Drunk** Outsider, return the assigned **Townsfolk face** (name, type presentation, rules path for that Townsfolk) — **never** the string or type “Drunk”.
2. Do **not** tell them their ability is fake, that they are an Outsider, or that info may be false *because* they are Drunk.
3. Ability **resolution** still uses true character + no real effect (and may feed false info). Prompts/scheduling follow the face role so the illusion holds.
4. Other seats’ abilities that learn a **true** character still see Drunk (Undertaker, etc.).
5. There must be **no** player-facing tool that returns raw `true_character` for the caller when it differs from the face.

Poisoned / temporary drunk: do not announce the status; they usually still know their real character name. Only the Drunk Outsider gets a permanent false character **label**.

If you add a feature that can leak true identity to a Drunk seat (logs, errors, debug fields on player responses, “related rules” dumps), fix it before merge.

## MCP tools

- Auth every mutating/read tool with `host_token` or `player_token` (opaque secrets).
- No `use_ability(role=...)`. Infer ability from the seat’s true Grimoire character.
- `say` is public only — no `to_seat` / whisper parameters.
- Public sheet tools (`get_character_rules` for the pool) are fine; they are not “you are this role.”

## Docs layout

- Simulation rules: `docs/` (split by topic; **one file per character** under `docs/roles/<type>/`).
- When adding a character ability, update the single role file — do not re-bundle all roles into one doc.
- Architecture intent: `docs/architecture.md`.

## Code

- Prefer small modules: `auth`, `comms`, `game`, `roles`, `tools`, `store`, `mcp_server`.
- Transport is line-delimited JSON-RPC on stdio (`docs/mcp.md`); keep handlers in `tools`, not in the transport layer.
- Keep player responses free of other seats’ secrets.
- Tests should cover at least: public chat shared; private roles isolated; **Drunk face on private state**.

## Don’t

- Don’t trust agent natural-language claims for mechanics.
- Don’t put Grimoire dumps in player tool results (Spy is an explicit ability path to that seat only).
- Don’t expand script scope without a clear product request.
