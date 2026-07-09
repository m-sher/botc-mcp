# botc-mcp Engine Design Spec

**Status:** Draft for review  
**Date:** 2026-07-08  
**Scope:** Full playable Trouble Brewing simulation behind an MCP tool surface for model agents  
**Related:** `AGENTS.md`, `docs/architecture.md`, `docs/*` rules reference  

---

## 1. Purpose

Build an **authoritative game engine** exposed as an **MCP server** so multiple model agents can play **Blood on the Clocktower — Trouble Brewing** end-to-end:

- Server assigns roles and resolves all abilities.
- Agents speak **only** on a public channel.
- Storyteller→player secrets are private and token-scoped.
- Drunk and similar false-identity effects are enforced by the engine (see `AGENTS.md`).

Success criteria for this design:

1. A host process can create a game, seat N agents, start, and run until Good or Evil wins.
2. No player tool can read another seat’s private state or true Grimoire role.
3. Every Trouble Brewing character ability is specified for setup, night, day, and edge cases.
4. Behavior is **deterministic given a seed** (reproducible evals).
5. Implementation can be staged without rewriting the tool contract.

---

## 2. Non-goals

| Out of scope | Why |
| --- | --- |
| Sects & Violets, Bad Moon Rising, Travellers, Fabled | Product scope |
| Player–player private messages | Explicit product choice |
| Physical table / hand signals | Digital sim only |
| LLM-as-Storyteller creativity required for v1 | Seeded policy instead (extensible later) |
| Real-time push / WebSockets | Polling via cursors is enough |
| Ranking / matchmaking / multi-tenant SaaS | Single-process multi-game is enough |
| Perfect human ST “fun balancing” heuristics beyond a documented policy | Eval fairness first |

---

## 3. Decided product constraints (from prior work)

These are **locked** unless the user revises them:

1. **Script:** Trouble Brewing only; Imp is the only Demon.
2. **Comms:** Public `say` only between agents; ST private inbox per seat.
3. **Auth:** Opaque `host_token` / `player_token`; never trust seat id alone.
4. **Roles:** Server-assigned; no mechanical `claim_role`.
5. **Drunk:** Player-facing identity is Townsfolk face; tools never say “Drunk”.
6. **Ability tools:** No `role=` parameter; server maps seat → true character.
7. **Public character sheet:** Agents may read any character’s rules text (pool knowledge).
8. **Docs:** One markdown file per character under `docs/roles/<type>/`.

---

## 4. Design approaches considered

### A. Thin MCP + fat external harness (rules in harness)

Harness implements BotC; MCP only relays chat.  
**Reject:** Duplicates authority; agents can be lied to inconsistently; harder to share.

### B. Fully automatic engine (no host advances)

Engine auto-runs night order and day timers with no host.  
**Reject for v1 control:** Evals need pause points; discussion length is agent-driven.

### C. Authoritative engine + host phase control + auto-complete within phase (**chosen**)

- Engine owns Grimoire, resolution, win checks, public/private logs.
- Host starts game, may force-advance discussion → nominations → night.
- **Within night:** when the current wake has received a valid action (or is auto/skip), engine advances to the next night step automatically.
- **Within nomination:** vote window closes when all living voters have voted **or** host calls `close_votes` / next nomination / `end_nominations`.

**Why:** Fits multi-agent evals, keeps ST authority in-process, allows harness pacing without implementing rules twice.

---

## 5. System overview

```
┌─────────────┐  host_token   ┌──────────────────────────────┐
│ Eval harness│──────────────▶│ botc-mcp (MCP server)        │
│ / orchestr. │  player_tok   │  GameStore: game_id → Game   │
└──────┬──────┘──────────────▶│  tools → engine commands     │
       │                      └──────────────┬───────────────┘
       │ get_public_log (shared feed)        │
       │ get_private_state (per agent)       ▼
       │ say / nominate / vote / night_action
       ▼                              Grimoire + PhaseMachine
   Model agents                       PublicLog + PrivateInboxes
```

**Processes:**

- One MCP server process can hold many concurrent `Game`s.
- Each agent connection presents its `player_token` on every tool call (or session-bind token at connect — see §12).
- Harness is responsible for not mixing tokens across agent contexts.

---

## 6. Identity & security model

### 6.1 Tokens

| Token | Powers |
| --- | --- |
| `host_token` | `start_game`, bag overrides, phase controls, ST public announce, **host-only** grimoire debug (optional, off by default in eval), force-skip stuck wakes |
| `player_token` | `say`, reads (public + own private), night/day actions, nominate, vote |

Tokens are cryptographically random (v1: 128-bit+ from OS RNG). Log and error messages never echo other seats’ tokens or true roles.

### 6.2 Player-facing vs true identity

| Field | Meaning |
| --- | --- |
| `true_character` | Grimoire truth |
| `believed_character` | Set for Drunk (Townsfolk face); `None` otherwise |
| `player_facing_character()` | `believed_character.unwrap_or(true_character)` |

**Invariant:** Every player-visible role field uses `player_facing_character()`.  
**Invariant:** Ability **effect** resolution uses `true_character` + ability-disabled flags (Drunk outsider, poisoned).

Poisoned players still see their true name (not a face), but are not told they are poisoned.

### 6.3 Information firewall

Player tool responses MUST NOT include:

- Other seats’ `true_character` / face / poison / drunk flags
- Red herring seat id (except indirectly via Fortune Teller yes/no)
- Private inbox of any other seat
- Host debug grimoire

Spy receives a **structured grimoire snapshot** only as their night result private message.

---

## 7. Domain model

### 7.1 Identifiers

```text
GameId: u64 or UUID
SeatId: u8 in 0..n-1   // circle order; (i+1)%n and (i+n-1)%n are neighbors
EventId: u64 monotonic per game (public log) and per-seat (private inbox) — or global per game
```

Neighbor helpers **skip dead** seats when rules say “living neighbors” (Empath).  
Chef evil pairs use **seating adjacency including dead**? Official: neighbors are seating; evil pairs typically count living alignment in seats that exist. Spec: **Chef uses physical seating circle of all seats that started the game; dead players still occupy seats for adjacency.** Alignment of dead is still good/evil for pair counting. (Matches common digital implementations; document as such.)

**Empath:** closest **living** clockwise and counterclockwise.

### 7.2 Seat state

```text
Seat {
  id, display_name
  alive: bool
  ghost_vote_available: bool   // true until spent after death; false if already used
  true_character: Character
  believed_character: Option<Character>  // Drunk face
  alignment: Good | Evil       // TB: fixed from type except always matches team of true char
  poisoned: bool
  poison_ends_at: PoisonEpoch  // see §9.2
  ability_disabled: bool       // true if Drunk outsider OR poisoned (computed)
  monk_protected: bool         // clears each dawn
  butler_master: Option<SeatId>
  slayer_used: bool
  virgin_used: bool            // first nomination happened
  once_per_game_spent: set     // generic guard
}
```

### 7.3 Game state

```text
Game {
  id, seed: u64
  phase: Phase
  seats: Vec<Seat>
  tokens: TokenBook
  public_log: PublicLog
  private: PrivateInboxes
  // Setup
  bag: Vec<Character>          // in-play set (unique characters)
  not_in_play_good: Vec<Character>  // for Imp bluffs pool
  red_herring: Option<SeatId>
  demon_bluffs: [Character; 3] // fixed after N1 briefing
  // Day politics
  day_number: u32              // 0 before first dawn; Day 1 after first night
  night_number: u32            // 1 = first night
  nominations_today: Vec<NominationRecord>
  current_nomination: Option<OpenNomination>
  // Night machine
  night_queue: Vec<NightStepId>
  night_index: usize
  pending_night: Option<PendingWake>
  deaths_tonight: Vec<SeatId>
  // Outcome
  winner: Option<Good|Evil>
  config: GameConfig
}
```

### 7.4 Phase enum (normative)

```text
Phase =
  | Lobby
  | FirstNight { at: NightCursor }
  | Day { day: u32, stage: Discussion | Nominations }
  | Night { night: u32, at: NightCursor }   // night >= 2
  | Ended { winner: Good | Evil, reason: EndReason }
```

`NightCursor` = index into that night’s concrete step list (after filtering absent roles).

### 7.5 EndReason

```text
DemonDead
EvilTwoAlive
SaintExecuted
MayorThreeNoExec
```

---

## 8. Setup specification

### 8.1 Player count

- Seats: **5–15** inclusive.
- Host is not a seat.

### 8.2 Base composition

Use the table in `docs/setup.md` (Townsfolk / Outsiders / Minions / Demon counts).

### 8.3 Bag selection

**Default algorithm (seeded):**

1. Let `need_tf, need_out, need_min, need_dem = composition(n)`.
2. Pool lists: all TB Townsfolk, Outsiders, Minions, Demons (Imp only).
3. Sample without replacement using RNG(seed):
   - Always include Imp as the Demon.
   - Sample `need_min` Minions from 4 minions.
   - Sample `need_out` Outsiders from 4 outsiders.
   - Sample `need_tf` Townsfolk from 13 townsfolk.
4. If **Baron** is among selected Minions: `need_out += 2`, `need_tf -= 2`, then **re-sample or top-up** outsiders and remove two townsfolk so final counts match (see below).

**Baron handling (normative):**

1. First select minions (may include Baron).
2. If Baron in bag: target outsiders = base_out + 2, target townsfolk = base_tf - 2.
3. Select outsiders and townsfolk to those targets.
4. Assert `len(bag) == n`.

**Drunk handling:**

1. Drunk is an Outsider token in the bag (not a Townsfolk).
2. After seat assignment, choose `believed_character` uniformly from Townsfolk that are **in the bag OR not in the bag**?  
   **Decision:** Face is chosen from **all Townsfolk in the script pool**, **excluding** any constraint that the face must be in play. Prefer faces that **are in play** with probability policy:  
   **v1 policy:** choose uniformly among Townsfolk **not equal to a face already used** and prefer in-play:  
   - Candidate set = in-play Townsfolk seats’ characters ∪ (if empty, all Townsfolk).  
   Simpler **v1:** uniform among **all 13 Townsfolk**. Document that face may not be in play (legal and common).

3. Player never sees Drunk.

**Fortune Teller red herring:**

- If Fortune Teller in bag: pick uniform random seat with **good** true alignment (Townsfolk or Outsider, including Drunk). Store `red_herring`.
- Red herring is not told anything special.

**Seat assignment:**

- Shuffle bag with RNG; assign to seats 0..n-1 in order.

**Not-in-play bluffs for Imp (7+):**

- After assignment, `good_not_in_play` = all Townsfolk∪Outsiders in pool minus those in bag (Drunk counts as Outsider in bag; face is irrelevant).
- Sample 3 distinct good characters from not-in-play; if fewer than 3, sample with remaining pool expansion from any good not in bag — TB pool is large enough for normal counts.

### 8.4 Host bag override (optional)

`start_game` may accept:

```text
{
  seed?: u64,
  assignments?: [ { seat_id, character, drunk_face? } ],  // full override
  bag?: [ character, ... ]  // set only; still random assign
}
```

If `assignments` provided, skip random bag; validate counts roughly (warn-only vs hard fail: **hard fail** on invalid TB set, e.g. no Imp).

### 8.5 Start sequence

1. Validate lobby size 5–15 and all seats joined (or host creates seats with names in `create_game`).
2. Build bag + assign + Drunk faces + red herring + bluff trio.
3. Phase → `FirstNight { at: 0 }` with concrete night queue built.
4. For each seat: private `YouAre { facing, team, rules_path }` (**facing** never Drunk).
5. Public: `StorytellerAnnounce("The first night begins.")` (wording flexible).
6. Run night machine from step 0 (may auto-complete pure ST steps).

### 8.5.1 Create / join API

**Option chosen:** Host creates all seats up front (eval-friendly):

```text
create_game({ player_names: string[], seed?: u64 })
  → { game_id, host_token, players: [ { seat_id, name, player_token } ] }
```

No open lobby race. `join_game` optional later for human UX; **not required for v1**.

---

## 9. Night machine

### 9.1 Concrete queue construction

Build a list of `NightStep` values for First Night vs Other Night from the **roles in play**, not the full sheet blanks.

#### First night queue (ordered)

1. `SetupMarkers` — ST internal (already done at start); no player wait if complete.
2. `MinionBriefing` — if `n >= 7` and ≥1 minion alive (all are).
3. `DemonBriefing` — if `n >= 7`.
4. `Poisoner` — if Poisoner in play and alive.
5. `Spy` — if Spy in play and alive.
6. `Washerwoman` — if in play and alive.
7. `Librarian` — if in play and alive.
8. `Investigator` — if in play and alive.
9. `Chef` — if in play and alive.
10. `Empath` — if in play and alive.
11. `FortuneTeller` — if in play and alive.
12. `Butler` — if in play and alive.
13. `Dawn`

**Note:** Drunk with face X is **woken on X’s slots**, not on Drunk (no slot). If face is Empath, they appear in Empath step. True character remains Drunk → info false / no effect.

**Implementation of face wakes:** Night queue is built from **player-facing character for wake eligibility of info roles**, but wait — resolution uses true character. Spec:

- **Wake eligibility** for night roles: use `player_facing_character()` for Townsfolk/Outsider info/protect roles so Drunk-as-Empath wakes on Empath.
- **Effect resolution:** if `true_character == Drunk` OR `poisoned`, apply **disabled path** (fake info / no protect / etc.).
- Minions/Demon wake on **true** character only (Drunk is never evil).

#### Other nights queue

1. `Poisoner` (if in play, alive)
2. `Monk` (if in play, alive)
3. `Spy` (if in play, alive)
4. `Imp` (if Imp seat alive — after starpass, new Imp)
5. `Ravenkeeper` — only if that seat is in `deaths_tonight` from Demon kill **and** true/face is Ravenkeeper with ability active
6. `Undertaker` (if in play, alive) — skip if no execution death today
7. `Empath`
8. `FortuneTeller`
9. `Butler`
10. `Dawn`

Ravenkeeper is typically queued dynamically after Imp kill rather than fixed index: **after Imp resolves**, if kill target is Ravenkeeper (true character Ravenkeeper, not disabled), insert immediate Ravenkeeper wake before continuing.

### 9.2 Pending wake protocol

```text
PendingWake {
  step: NightStep
  seat: Option<SeatId>     // None for group briefings handled as ST push-only
  kind: InfoOnly | ChoiceRequired | BriefingAuto
  deadline: optional       // unused v1
}
```

**Flow:**

1. `begin_step(step)`:
   - If role absent/dead/skip → advance immediately.
   - If BriefingAuto (minion/demon info) → push private messages, advance.
   - If ChoiceRequired → set `pending_night`, push `NightPrompt` private message describing legal choice schema.
   - If InfoOnly with no choice (Chef, Empath after compute) → compute, push `NightResult`, advance.
2. Player calls `night_action` matching pending seat.
3. Validate payload; resolve; clear pending; advance.

**Stuck recovery:** Host `skip_night_action` applies a documented default (see each role).

### 9.3 Ability-disabled path

When `seat.ability_disabled()` (Drunk outsider OR poisoned):

| Ability class | Behavior |
| --- | --- |
| Info (Chef, Empath, FT, WW, Lib, Inv, Undertaker, Ravenkeeper) | Still requires same choice shape if any; return **false** info per §11 policy |
| Monk protect | Accept target; **no** protection applied |
| Poisoner | Accept target; **no** poison applied (rare: Poisoner poisoned) |
| Butler | Accept master; **no** voting restriction (or apply fake restriction? **Decision: no real restriction**) |
| Imp kill | If Imp disabled (poisoned), kill **fails** (no death) — still accept choice |
| Spy grimoire | Show **false/altered** grimoire per policy OR true? **Decision: show true grimoire only if not disabled; if disabled, show a seeded plausible fake snapshot** |

### 9.4 Poison timing

- Poisoner each night: clear previous poison on all seats, then apply new target: `poisoned=true` until next Poisoner step (or Poisoner dies → clear immediately).
- Covers “tonight and tomorrow day”.

### 9.5 Death at night

`try_demon_kill(target)`:

1. If target dead → no public death (sink); still “resolved”.
2. If target monk_protected → no death.
3. If target true Soldier and ability not disabled → no death.
4. If target is Mayor and ability not disabled → **Mayor bounce policy** (§11): may redirect to another living non-unprotected seat; if bounce target also unkillable → nobody dies.
5. Else mark target dead, add to `deaths_tonight`, shroud, ghost vote available.
6. If target was Ravenkeeper and ability not disabled → wake Ravenkeeper for choose player → learn **true** character (with registration if Recluse/Spy).
7. Win check after kill chain (usually not on N1).

**Imp starpass:** if target == self:

1. Imp dies.
2. Host/engine picks living Minion via policy §11 (`starpass_target`).
3. That seat `true_character = Imp`; private `YouAre` Imp; clear old minion ability.
4. No second kill tonight.
5. Win check: Demon still exists → no good win.

**Scarlet Woman:** checked on Demon death from **execution or other non-starpass** causes (§10). Starpass is not “Demon dies for good win”; it’s conversion of role. If Imp dies by execution and SW eligible → SW becomes Imp.

### 9.6 Per-role night resolution (normative)

Notation: `register_evil(seat)`, `register_character(seat, for_ability)` — §11.

#### MinionBriefing (n≥7)

For each Minion seat: private message listing Demon seat id/name and fellow Minion seat ids/names.

#### DemonBriefing (n≥7)

Imp: list Minion seats; list 3 bluff character names.

#### Poisoner

- Choice: one seat (any, including self).
- Effect: poison target (if not disabled).

#### Spy

- Choice: none (ack) or automatic.
- Effect: private structured grimoire: for each seat `{ name, alive, true_character, poisoned, drunk_face?, reminders }`.

#### Washerwoman

- ST picks (seeded): one in-play Townsfolk character T that exists on some seat (true Townsfolk, or Spy registering as that Townsfolk — policy).
- Picks two seats: correct owner of T (or registered) + one wrong.
- Player sees two names + character T.
- Disabled: two random seats + random Townsfolk token (seeded lie).

#### Librarian

- If zero Outsiders in play (true bag count): show 0.
- Else: like WW for an Outsider character (Drunk shows as Drunk to the ability, not face).
- Disabled: lie.

#### Investigator

- Like WW for a Minion character in play.
- Disabled: lie.

#### Chef

- Count pairs of adjacent seats (circle) where both **register as evil** for Chef (default: true alignment evil; Recluse/Spy per §11).
- Disabled: random 0–2 or seeded wrong count.

#### Empath

- Living neighbors CW/CCW; count how many register evil.
- Disabled: wrong 0–2.

#### Fortune Teller

- Choice: two seats (can be self/dead).
- Yes if either is true Demon **OR** is red_herring.
- Recluse may register as Demon; Spy does not register as Demon unless policy says so (**v1: Spy does not ping FT as Demon; Recluse may**).
- Disabled: random yes/no seeded.

#### Butler

- Choice: one other seat as master (living or dead).
- Effect: store master for next day votes.

#### Monk (not N1)

- Choice: one other living seat.
- Effect: set monk_protected on target for tonight.

#### Imp (not N1)

- Choice: one seat (any).
- Effect: kill/starpass as §9.5.

#### Ravenkeeper

- Choice: one seat; learn **true** character name (Spy/Recluse registration for character reveal: show registered character if policy applies — **v1: show true character always for Ravenkeeper**, except Recluse may show evil character type name? **Decision: Ravenkeeper learns true character token; Recluse still true Recluse; Spy true Spy.** Registration primarily for alignment/type pings like Empath/WW.)

#### Undertaker

- If a player died by **execution** today (and died): show their **true** character (Drunk → Drunk).
- If execution killed via Virgin bounce, that death counts.
- No execution death → skip wake.

### 9.7 Dawn

1. Clear `monk_protected`.
2. Public announce deaths (names only) or “nobody died”.
3. Clear `deaths_tonight` after announce.
4. `day_number += 1` (after first night, day becomes 1).
5. Phase → `Day { day, stage: Discussion }`.
6. Win check if needed (rare at dawn).

---

## 10. Day machine

### 10.1 Discussion stage

- Any player (alive or dead) may `say`.
- Host calls `open_nominations` → stage Nominations.
- Optional: host `st_announce`.

### 10.2 Nominations stage

Rules from `docs/voting-and-nominations.md`, refined:

**Nominate(`by`, `target`):**

- Phase must be Nominations.
- `by` alive; not already nominated today; `target` not already nominated today.
- **Virgin:** if `target` is Virgin (true) and `!virgin_used`:
  - Set `virgin_used = true` always on first nomination.
  - If `by.true_character` is Townsfolk **and** `!by.ability_disabled()` for Virgin purposes: Virgin checks **true** type of nominator. Drunk is Outsider → no bounce. Poisoned Townsfolk nominator: ability disabled means Virgin does not trigger? **Decision: Virgin checks true character type of nominator; poison/drunk on nominator does not change type; only Drunk outsider is not Townsfolk. Poison does not change character type → poisoned Townsfolk still dies to Virgin.**
  - If nominator is Townsfolk: execute nominator immediately (no vote); that is the day’s execution; end nominations; resolve death; win check; if game continues, host may still end day or auto-progress to night after execution.
  - If not Townsfolk: continue to normal vote.

**Vote on current nomination:**

- Model: when nomination created, open vote.
- Each living seat may vote yes/no once per nomination (default **no** if not cast by close? **Decision: explicit vote required for yes; missing vote = no**).
- Dead: at most one yes vote in the entire game (`ghost_vote_available`); spending a yes consumes it; voting no does not consume? **Decision: ghost vote token is only spent when casting **support (yes)**.**
- Butler: if living Butler with ability active and master set, Butler may only vote yes if master has voted yes on this nomination; if master votes no or hasn’t yes, Butler yes is rejected or forced no.

**Vote count:** number of yes votes.

**Threshold:** `yes * 2 >= living_count` (i.e. yes >= ceil(living/2)? For even 6, half is 3: `yes >= (living + 1) / 2` integer: use **`yes * 2 >= living`** → 3/6 ok, 2/6 no). Confirm: 5 living need 3: `3*2 >= 5` ok; 2*2 >= 5 false. Good.

**Leader:** nomination is current execution candidate if threshold met AND yes strictly greater than any other nomination’s yes total today.

**Ties for highest:** no execution candidate.

**end_nominations (host):**

1. If candidate exists → execute that seat.
2. Else → `NoExecution`.
3. If no execution and living == 3 and Mayor in play alive with ability: Mayor team wins (good).
4. Else if execution: `resolve_execution(seat)`.
5. If game not ended → Phase Night (next night number) or allow host `begin_night`.

**Auto path:** After end_nominations and not ended, engine auto-starts next night queue.

### 10.3 Execution resolution

```text
resolve_execution(seat):
  if already dead: still "executed" for undertaker? Decision: cannot nominate dead in v1.
  mark dead, ghost vote, clear abilities
  public Executed
  if true Saint and not ability_disabled: Evil wins (Saint executed)
  else apply_demon_death_if_needed(seat)
  win_check
```

**Slayer (day_action):**

- Once per game; public; any day stage? **Decision: allowed in Discussion or Nominations.**
- If target true Demon and Slayer not disabled: kill Demon via `apply_demon_death`; public announce death (Storyteller: “X dies” / slayer success public without revealing roles — **public: target dies immediately**).
- If wrong: public nothing (or “nothing happens”); spend slayer_used anyway.
- Disabled Slayer: spend and fail, nothing happens.

### 10.4 Demon death helper

```text
apply_demon_death(seat):
  // seat was Imp
  living = count alive AFTER this death applied
  if ScarletWoman in play and SW.alive and living >= 5 and SW not disabled:
     convert SW to Imp; private YouAre; return
  else:
     Good wins
```

Note: living count for SW is after Imp removed. Spec: “5 or more players alive when the Demon dies” — count living **excluding the just-dead Demon**, i.e. remaining living ≥ 5? Official: if 5+ alive when demon dies, SW transforms. Usually interpreted as alive count **including** demon before death ≥ 5, or remaining ≥ 4? Standard TB: SW works if **5+ players still alive** after? Wiki: “If there are 5 or more alive when the Demon dies”. Common reading: count alive **before** removing demon ≥ 5, or after?  
**Decision used in community tools:** If alive players **≥ 5 before death**, SW converts (Demon is among the 5). Example: 5 alive including Imp → Imp executed → 4 left + SW becomes Imp.  
**Normative:** Let `alive_before` include the dying Demon. SW converts if `alive_before >= 5` and SW alive and not disabled.

Starpass: not a “good wins” demon death.

---

## 11. Storyteller policy (seeded, deterministic)

All random ST choices use `Rng` stream from `game.seed` with labeled substreams (`"washerwoman"`, `"false_info"`, etc.).

### 11.1 False info (disabled abilities)

| Ability | Lie generator |
| --- | --- |
| Binary yes/no (FT) | Flip true answer with p=1.0 (**always lie** when disabled) |
| Count 0–2 (Empath) | Uniform wrong value among {0,1,2} \ {truth} |
| Chef count | Uniform in 0..=min(4, n) except truth if possible |
| WW/Lib/Inv | Random two seats + random plausible token of correct class |
| Undertaker | Random in-play or any character token from pool |
| Ravenkeeper | Random character from pool |

### 11.2 Registration (Spy / Recluse)

Per detection event, draw from seed:

**Recluse (good):**

- Empath/Chef evil ping: register evil with p=0.5  
- FT demon ping: register as demon with p=0.5  
- WW/Lib/Inv type: not applicable as townsfolk/outsider/minion owner unless needed  
- Default character reveal: true Recluse  

**Spy (evil):**

- Empath/Chef: register good with p=0.5  
- WW as Townsfolk: may appear as townsfolk for WW with p=0.5 when Spy is the “wrong” or “right” slot — keep ST picks consistent with registration chosen first  

Document registration draws in host debug log only.

### 11.3 Mayor bounce

When Mayor would die at night and not disabled: redirect kill to uniform random other living seat that is not Soldier-protected/monk-protected; if none, nobody dies.

### 11.4 Starpass target

Uniform random living Minion; if none (shouldn’t happen), Imp dies and Good wins.

### 11.5 Washerwoman “correct” pick

Prefer real Townsfolk in play; if Spy registers as that Townsfolk, may use Spy as correct seat.

---

## 12. MCP tool specification

Transport: MCP tools with JSON arguments. Auth: `token` on every call (or session-scoped metadata — **v1 argument `token`** for simplicity).

### 12.1 Lifecycle

#### `create_game`

```json
{ "player_names": ["A","B",...], "seed": 12345 }
→ { "game_id", "host_token", "players": [{"seat_id", "name", "player_token"}] }
```

#### `start_game`

```json
{ "token": host, "game_id", "assignments"?: [...], "bag"?: [...] }
→ { "ok": true, "phase": "..." }
```

#### `open_nominations` / `close_vote` / `end_nominations` / `begin_night` (if not auto)

Host only.

- `close_vote`: finalize current nomination tally; do not execute yet.
- `end_nominations`: execute leader or none; Mayor check; start night if not ended.

#### `skip_night_action`

Host only; applies default for pending wake.

#### `st_announce`

```json
{ "token": host, "text": "..." }
```

### 12.2 Reads

#### `get_public_state`

```json
{ "token", "game_id" }
→ {
  "phase": { "type": "day", "day": 1, "stage": "discussion" },
  "seats": [{"seat_id","name","alive","ghost_vote_available"}],
  "living_count": 7,
  "nominations_today": [...],
  "current_nomination": null | { "nominee", "yes_votes", "voters_yes": [seat_id], "threshold_met": bool },
  "winner": null | "good" | "evil",
  "pending": null | { "kind": "night_action", "seat_id": 3 } // only if token is that seat or host
}
```

Note: `pending.seat_id` visible to all? **Decision: public that someone must act would leak night order. Pending is only returned in `get_private_state` for the acting seat; host sees all pending in `get_host_state`.**

#### `get_public_log`

```json
{ "token", "game_id", "cursor": 0 }
→ { "events": [{"id", "event": ...}], "next_cursor": 12 }
```

Public event types: `chat`, `st_announce`, `nominated`, `vote_cast`, `executed`, `no_execution`, `died_in_night`, `phase_changed`, `game_ended`, `slayer_miss` (optional silence), `player_died` (slayer/virgin).

#### `get_private_state`

```json
{ "token": player, "game_id", "cursor": 0 }
→ {
  "seat_id", "name", "alive",
  "character": "Empath",           // player-facing only
  "team": "good",
  "rules_path": "docs/roles/townsfolk/empath.md",
  "awaiting": null | { "action": "night", "prompt": "...", "schema": {...} },
  "messages": [{"id", "msg": ...}],
  "next_cursor": 3
}
```

#### `get_character_rules`

```json
{ "token", "character": "Monk" }
→ { "name", "team", "type", "text": "..." }  // load markdown body
```

#### `get_host_state` (host only)

Full grimoire + pending + seed — **eval/debug only**. Never give host token to player agents.

### 12.3 Communication

#### `say`

```json
{ "token": player, "text": "..." }
→ { "event_id" }
```

Reject empty/whitespace; max length 4000 chars.

### 12.4 Actions

#### `night_action`

```json
{
  "token": player,
  "payload":
    | { "type": "ack" }
    | { "type": "pick_one", "target": seat_id }
    | { "type": "pick_two", "a": seat_id, "b": seat_id }
}
```

Must match pending wake for that seat; else error `not_your_wake` / `wrong_payload`.

#### `day_action`

```json
{ "token", "type": "slay", "target": seat_id }
```

#### `nominate`

```json
{ "token", "target": seat_id }
```

#### `vote`

```json
{ "token", "nominee": seat_id, "support": true|false }
```

Nominee must be current open nomination.

### 12.5 Errors

```json
{ "error": "unauthorized" | "wrong_phase" | "illegal_action" | "not_your_wake" | "bad_request",
  "message": "human readable, no secrets" }
```

---

## 13. Win check algorithm (single function)

Call after: night kill, execution, slayer kill, virgin kill, end_nominations (mayor).

```text
fn win_check(game):
  if game.winner.is_some(): return
  if no living seat with true_character Imp:
     // SW should have converted already if eligible
     good wins DemonDead
  if living_count == 2 and living Imp exists:
     evil wins EvilTwoAlive
  // Saint handled at execution time
  // Mayor handled at end_nominations
```

Simultaneous DemonDead and EvilTwoAlive → Good wins (DemonDead first check order: if Imp dead, good wins even if 2 alive).

---

## 14. Character coverage matrix

| Character | Setup | Night | Day | Notes |
| --- | --- | --- | --- | --- |
| Washerwoman | — | N1 info | — | |
| Librarian | — | N1 info | — | 0 outsiders |
| Investigator | — | N1 info | — | |
| Chef | — | N1 info | — | |
| Empath | — | each night | — | living neighbors |
| Fortune Teller | red herring | each night | — | |
| Undertaker | — | other nights | — | execution only |
| Monk | — | other nights | — | protect |
| Ravenkeeper | — | on night death | — | |
| Virgin | — | — | on nominate | |
| Slayer | — | — | day_action | |
| Soldier | — | passive | — | |
| Mayor | — | bounce | 3-alive no exec | |
| Butler | — | each night | vote restrict | |
| Drunk | face | face schedule | face day | never revealed |
| Recluse | — | passive reg | — | |
| Saint | — | — | exec lose | |
| Poisoner | — | each night | — | |
| Spy | — | each night | reg | grimoire |
| Scarlet Woman | — | on demon death | — | |
| Baron | +2 out | — | — | |
| Imp | bluffs N1 | kill * | — | starpass |

---

## 15. Module architecture (implementation)

```text
src/
  auth.rs          // tokens
  comms.rs         // public/private logs
  roles/           // enum + static data + night order templates
  game/
    mod.rs
    state.rs       // Game aggregate
    setup.rs       // bag, assign, drunk face, herring, bluffs
    phase.rs       // transitions
    night.rs       // queue, wakes, resolve
    day.rs         // nominate, vote, execute
    win.rs         // win_check, demon death, SW
    ability.rs     // per-character resolve + disabled lies
    register.rs    // spy/recluse draws
  tools/           // MCP handlers → game API
  store.rs         // game_id → Game mutex
  main.rs          // MCP server entry (later)
```

No circular deps: `tools` → `game` → `roles`/`comms`/`auth`.

---

## 16. Concurrency

- `Arc<Mutex<GameStore>>` or per-game tokio Mutex.
- One command at a time per game (serialize tool calls on `game_id`).
- v1 single-threaded accept loop acceptable.

---

## 17. Testing strategy

### 17.1 Unit tests

- Composition counts 5–15 + Baron.
- Drunk face never leaks in `get_private_state`.
- Public log isolation.
- Vote threshold math.
- Virgin Townsfolk vs Drunk nominator.
- SW conversion boundary at 5 alive_before.
- Starpass transfers Imp.
- Soldier/Monk/Mayor bounce block kills.
- Poison disables FT truth (always lie policy).
- Win: 2 alive evil; demon exec good; saint exec evil; mayor 3 no exec.

### 17.2 Scenario tests (scripted seats)

Fixture assignments + scripted action lists → assert winner and key private messages.

### 17.3 Property / fuzz (optional later)

Random legal action sequences don’t panic; seed replay identical.

---

## 18. Eval harness contract

```text
1. create_game(names, seed) → tokens
2. start_game(host)
3. Each agent: get_private_state(player_token)  // isolated context
4. Loop until ended:
   a. Shared: get_public_log(cursor), get_public_state
   b. Agents: say as desired (public)
   c. If private awaiting night/day: submit action
   d. Host: open_nominations / end_nominations when policy says
      (night auto-advances on actions)
5. Record public log + host grimoire for scoring
```

Harness MUST NOT place multiple player tokens in one model context.

---

## 19. Implementation phases (for later planning)

Not part of runtime behavior; order of build:

| Phase | Deliverable |
| --- | --- |
| P0 | Game store, create/start, identity, public say, private YouAre, host grimoire |
| P1 | Night queue + briefings + all N1 info roles + dawn |
| P2 | Other nights: poison, monk, imp kill, RK, undertaker |
| P3 | Day: nominate/vote/execute, virgin, ghost vote, butler |
| P4 | Slayer, SW, saint, mayor, win_check completeness |
| P5 | MCP transport wiring + harness example |
| P6 | Registration polish + scenario suite |

---

## 20. Open questions / defaults (defaults ARE normative for v1)

| Topic | v1 default |
| --- | --- |
| Who creates seats | `create_game` returns all player tokens |
| Night advance | Automatic after each resolved wake |
| Day discussion end | Host `open_nominations` |
| Nomination vote close | Host `close_vote` or auto when all living voted |
| Missing vote | Counts as no |
| Ghost vote spend | Only on yes |
| False info when disabled | Always lie (deterministic wrong) |
| Chef adjacency | Full circle including dead seats |
| Empath neighbors | Skip dead |
| SW threshold | `alive_before >= 5` |
| Mayor bounce | Random other living legal target |
| Drunk face pool | Uniform among 13 Townsfolk |
| Ravenkeeper learns | True character name |
| Spy grimoire if poisoned | Seeded fake snapshot |
| Pending wake visibility | Private to actor + host only |

Revising a default requires updating this spec and tests.

---

## 21. Success definition (done)

The engine is complete when:

1. Scripted 8-player scenario can run start→win with all major roles exercised.
2. MCP tools match §12.
3. Tests in §17.1 pass.
4. `AGENTS.md` Drunk invariant holds under adversarial tool use.
5. No other-edition code paths.

---



---

## 23. Edge cases (normative)

### 23.1 Game ends mid-day or mid-night

When `winner` is set:

- Reject further `night_action`, `nominate`, `vote`, `day_action` with `wrong_phase` / `game_ended`.
- Allow `get_*` and `say` (dead and living may still talk for postgame; optional harness mute).
- **v1:** allow `say` after end; reject mechanical actions.

### 23.2 Closing votes

When a nomination is open:

1. Players cast `vote` until host calls `close_vote`, OR
2. When every **living** seat has cast a vote on this nomination, auto-`close_vote`.

`close_vote`:

- Finalize yes tally for this nomination.
- Update leader candidate for the day.
- Clear `current_nomination` so a new `nominate` may occur.
- Do **not** execute until `end_nominations`.

### 23.3 Multiple nominations

After `close_vote`, another living player may nominate a different target (subject to once-per-day limits). Each closed nomination’s yes total remains on the record for leader comparison.

### 23.4 Hostless night

If pending wake exists and the acting player never acts, host must `skip_night_action` or eval harness times out and skips. Engine does not wall-clock timeout in v1.

### 23.5 Character not in play

Night queue omits steps for characters not in the bag. Passive with face role of a not-in-play Townsfolk still wakes on that face’s step (queue built from faces + true minions/demon).

### 23.6 Imp seat after SW / starpass

Only one Imp true character at a time. Old Imp seat is dead (starpass) or was SW (converted from minion while Imp seat dead).

## 22. Spec self-review notes

- No TBD left without a default in §20.
- Architecture matches tool surface.
- Scope is one script, one process model, eval-first ST policy.
- Ambiguities resolved with explicit Decisions above.
- Edge cases §23 cover mid-game end, vote close, faces not in play.
- Tool list includes `close_vote` (host) as used in §23.2; add to §12.1:
  `close_vote` host-only.
