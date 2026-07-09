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

**Default policy:** Storyteller discretion that is **player-visible mid-pause** is
**host-first** (night info authorship, Mayor bounce, starpass). `skip_night_action`
applies the documented **random/default fallback**. Set `start_game.st_choice_mode` to
`random` to force immediate seeded-random policy for those pauses (eval harness).

**Day-time Spy/Recluse registration** (Virgin nominator, Slayer→Recluse) resolves
**immediately** via `registration_mode` — never a day-blocking host pause — so the day
stays live with no limbo or probe channel (#39 / #41). Control it with
`registration_mode`: `random` / `always_true` / `always_misreg`.

**Isolation (night pauses only):** While `pending_host` is set, other gameplay mutations
are rejected. Only `host_decide` / `skip_night_action` may proceed. Night info pauses are
**uniform** in host-first so result timing does not leak Spy/Recluse.

| Knob | Default | Tool / start option |
| --- | --- | --- |
| ST choice policy (night pauses) | **host-first** | `start_game.st_choice_mode`: `host_first` (default) / `random` |
| Pair info + all ST night results | host pending | `host_decide` type `night_info` with `text`; skip → engine |
| False info (drunk/poisoned) | host pending | `host_decide` `night_info` **or** `host_queue_lie`; skip → seeded lie |
| Spy/Recluse registration (incl. Virgin / Slayer day) | immediate policy | `start_game.registration_mode`: `random` / `always_true` / `always_misreg` |
| Drunk face, red herring, demon bluffs | seeded-random | `start_game.drunk_faces`, `red_herring`, `demon_bluffs` |
| Mayor night bounce | host pending | `host_decide` type `mayor_redirect`; skip → **nobody dies** |
| Imp starpass minion | host pending | `host_decide` type `starpass_pick`; skip → random living minion |
| Disabled-role free-text queue | optional | `host_queue_lie` FIFO; **cleared at dawn** |

## Private channels

Night and secret day questions use private Storyteller ↔ player messages.
Other players must not see those contents.


## False identity (Drunk)

- Never tell a Drunk seat they are the Drunk.
- Private role tools and briefings show only the Townsfolk face.
- Still run face-schedule wakes; give false info as needed; resolve no real effect.
- True character remains Drunk for Grimoire and for abilities that learn characters.

