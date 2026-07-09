# Game rules (simulation reference)

Rules for the **botc-mcp** play loop: models play **Trouble Brewing** Blood on the
Clocktower under a Storyteller (server / moderator).

Only this character set and loop are in scope. Start here when implementing or
prompting players.

## Document map

| Topic | File |
| --- | --- |
| Overview & player rules | [overview.md](overview.md) |
| Gameplay loop (night / day) | [gameplay-loop.md](gameplay-loop.md) |
| Setup & bag composition | [setup.md](setup.md) |
| Win conditions | [win-conditions.md](win-conditions.md) |
| Nominations, voting, execution | [voting-and-nominations.md](voting-and-nominations.md) |
| Death & ghost votes | [death-and-ghosts.md](death-and-ghosts.md) |
| States (drunk, poisoned, register) | [states.md](states.md) |
| Ability resolution | [abilities-rules.md](abilities-rules.md) |
| Night wake order | [night-order.md](night-order.md) |
| Storyteller (moderator) | [storyteller.md](storyteller.md) |
| Character types (no abilities) | [roles/character-types.md](roles/character-types.md) |
| Character pool index | [characters.md](characters.md) |
| **Roles (one file each)** | [roles/README.md](roles/README.md) |
| └ Townsfolk | [roles/townsfolk/](roles/townsfolk/) |
| └ Outsiders | [roles/outsiders/](roles/outsiders/) |
| └ Minions | [roles/minions/](roles/minions/) |
| └ Demons | [roles/demons/](roles/demons/) |
| Glossary | [glossary.md](glossary.md) |
| Sources | [sources.md](sources.md) |
| **Engine architecture sketch** | [architecture.md](architecture.md) |
| **Engine implementation plan** | [superpowers/plans/2026-07-08-botc-mcp-engine.md](superpowers/plans/2026-07-08-botc-mcp-engine.md) |
| **Engine design spec (full)** | [superpowers/specs/2026-07-08-botc-mcp-engine-design.md](superpowers/specs/2026-07-08-botc-mcp-engine-design.md) |

## Retrieval tips

- Day/night cycle → `gameplay-loop.md`
- Who wins → `win-conditions.md`
- Vote thresholds → `voting-and-nominations.md`
- **A player’s role only** → `roles/<type>/<name>.md` (never the whole type folder’s contents at once unless indexing)
- Wake order → `night-order.md`
- Drunk / poison → `states.md`
- Bag composition → `setup.md`
