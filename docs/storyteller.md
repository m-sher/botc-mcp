# Storyteller

The Storyteller is the moderator (in this project: the MCP / server). Not on a team.

## Responsibilities

- Setup bag composition and Grimoire
- Run night order and deliver private ability results
- Open/close day discussion and nominations; count votes
- Apply registration (Spy, Recluse), false info (drunk/poison), Mayor bounce, starpass target
- Check win conditions after deaths
- Never reveal Grimoire contents except via legal abilities (e.g. Spy)

## Discretion (legal)

| Allowed | Not allowed |
| --- | --- |
| False info to drunk/poisoned | Breaking ability or win rules |
| Red herring placement | Running without a Demon |
| Spy / Recluse registration per detection | Revealing secrets outside abilities |
| Mayor night-death bounce target | Ignoring thresholds or night order |
| Which Minion receives Imp starpass | |

Prefer false info that creates a coherent, playable bluff environment without
hard-coding a winner.

### Engine host knobs (v1)

| Knob | Default | Tool / start option |
| --- | --- | --- |
| Spy/Recluse registration | random p=0.5 | `start_game.registration_mode`: `random` / `always_true` / `always_misreg` |
| Drunk face, red herring, demon bluffs | seeded-random | `start_game.drunk_faces`, `red_herring`, `demon_bluffs` |
| Mayor night bounce | host pending | `host_decide` type `mayor_redirect`; `skip_night_action` → nobody dies when alternatives exist |
| Imp starpass minion | host pending | `host_decide` type `starpass_pick`; skip → random living minion |
| Disabled-role false info text | seeded-random | `host_queue_lie` FIFO free-text consumed by next disabled info result |

Structured host-authored lies (e.g. pick exact Empath count) beyond the free-text queue are deferred.

## Private channels

Night and secret day questions use private Storyteller ↔ player messages.
Other players must not see those contents.


## False identity (Drunk)

- Never tell a Drunk seat they are the Drunk.
- Private role tools and briefings show only the Townsfolk face.
- Still run face-schedule wakes; give false info as needed; resolve no real effect.
- True character remains Drunk for Grimoire and for abilities that learn characters.

