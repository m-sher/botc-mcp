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

**Default policy:** Storyteller discretion is **host-first**. Whenever the rules require a
Storyteller choice, the engine pauses for `host_decide`. `skip_night_action` applies the
documented **random/default fallback** for that decision. Set
`start_game.st_choice_mode` to `random` to force immediate seeded-random policy (eval harness).

| Knob | Default | Tool / start option |
| --- | --- | --- |
| ST choice policy | **host-first** | `start_game.st_choice_mode`: `host_first` (default) / `random` |
| Pair info (Washerwoman / Librarian / Investigator) | host pending | `host_decide` type `night_info` with `text`; skip → seeded pair |
| False info (drunk/poisoned) | host pending | `host_decide` `night_info` **or** pre-queue via `host_queue_lie`; skip → seeded lie |
| Spy/Recluse affecting Chef / Empath / FT / UT / RK | host pending | `host_decide` `night_info` with the private result text; skip → registration draws |
| Spy/Recluse registration (random path) | p=0.5 when skip/`random` mode | `start_game.registration_mode`: `random` / `always_true` / `always_misreg` |
| Drunk face, red herring, demon bluffs | seeded-random | `start_game.drunk_faces`, `red_herring`, `demon_bluffs` (host overrides at setup) |
| Mayor night bounce | host pending | `host_decide` type `mayor_redirect`; skip → **nobody dies** |
| Imp starpass minion | host pending | `host_decide` type `starpass_pick`; skip → random living minion |
| Virgin: Spy nominator as Townsfolk? | host pending | `host_decide` type `registration` with `register: bool`; skip → random |
| Slayer: Recluse as Demon? | host pending | `host_decide` type `registration` with `register: bool`; skip → random |
| Disabled-role free-text queue | optional | `host_queue_lie` FIFO; consumed by next disabled info; **cleared at dawn** |

## Private channels

Night and secret day questions use private Storyteller ↔ player messages.
Other players must not see those contents.


## False identity (Drunk)

- Never tell a Drunk seat they are the Drunk.
- Private role tools and briefings show only the Townsfolk face.
- Still run face-schedule wakes; give false info as needed; resolve no real effect.
- True character remains Drunk for Grimoire and for abilities that learn characters.

