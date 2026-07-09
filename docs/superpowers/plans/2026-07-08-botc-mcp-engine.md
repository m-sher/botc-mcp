# botc-mcp Engine Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement the full Trouble Brewing game engine and MCP tool surface so model agents can play end-to-end under a host, per the approved design spec.

**Architecture:** Authoritative in-process `Game` owns Grimoire, phase machine, public log, and private inboxes. Tools authenticate via opaque host/player tokens and never accept client-supplied roles for mechanics. Night steps auto-advance after each resolved wake; host controls day discussion → nominations → end day.

**Tech Stack:** Rust 2021, `rand` + `rand_chacha` (seeded RNG), `thiserror`, `serde`/`serde_json` for tool payloads, `uuid` for tokens, unit tests via `cargo test`. MCP transport (`rmcp` or equivalent) only in the final wiring task—engine API is library-first.

**Spec:** `docs/superpowers/specs/2026-07-08-botc-mcp-engine-design.md`  
**Implementer rules:** `AGENTS.md`

## Global Constraints

- Trouble Brewing only; Imp is the only Demon; no Travellers/other editions.
- Public agent chat only (`say` has no recipient); ST→player private inbox is token-scoped.
- Drunk: player-facing tools never return "Drunk"; always Townsfolk face (`AGENTS.md`).
- No `use_ability(role=...)`; server maps seat → true character.
- Deterministic given `seed` (ChaCha8 or StdRng from seed).
- Vote threshold: `yes * 2 >= living_count`; missing vote = no; ghost vote spent only on yes.
- SW converts if `alive_before >= 5` (count includes dying Demon).
- False info when ability disabled: always lie (deterministic wrong answer).
- Pending night wake visible only to acting player + host, not in general public state.
- One file per character under `docs/roles/`; do not re-bundle role ability text into engine strings beyond short prompts.

---

## File structure (target)

```
src/
  lib.rs                 // module tree + re-exports
  auth.rs                // Token, TokenBook, Actor
  comms.rs               // PublicLog, PrivateInboxes, event enums
  error.rs               // GameError, ToolError
  rng.rs                 // SeededRng wrapper with labeled substreams
  store.rs               // GameStore: HashMap<GameId, Game>
  roles/
    mod.rs
    data.rs              // Character, Team, CharacterType, paths
    night_order.rs       // template steps first/other night
  game/
    mod.rs
    ids.rs               // GameId, SeatId
    seat.rs              // Seat state
    state.rs             // Game aggregate + high-level commands
    setup.rs             // bag, assign, drunk face, herring, bluffs
    phase.rs             // Phase enum + transitions
    night.rs             // queue build, pending wake, advance
    day.rs               // nominate, vote, close_vote, end_nominations
    win.rs               // win_check, demon death, SW, saint, mayor
    ability/
      mod.rs             // dispatch resolve_night / disabled lies
      info.rs            // WW Lib Inv Chef Empath FT Undertaker RK
      evil.rs            // Poisoner Spy Imp briefings
      protect.rs         // Monk Soldier passive
    register.rs          // Spy/Recluse registration draws
  tools/
    mod.rs               // all tool handlers
    views.rs             // PublicStateView, PrivateStateView DTOs
  main.rs                // binary: MCP server entry (Task 12)
tests/
  common/mod.rs          // fixtures: create assigned games
  setup_tests.rs
  identity_tests.rs
  night_tests.rs
  day_tests.rs
  win_tests.rs
  scenario_imp_exec.rs
  scenario_sw_starpass.rs
```

Refactor existing sketch files in place; prefer moving types rather than duplicating.

---

### Task 1: Error types, IDs, seeded RNG

**Files:**
- Create: `src/error.rs`
- Create: `src/rng.rs`
- Create: `src/game/ids.rs`
- Modify: `src/lib.rs`
- Modify: `Cargo.toml` (add `rand`, `rand_chacha`, `thiserror`, `serde`, `serde_json`, `uuid`)
- Test: `tests/rng_tests.rs` (or unit tests in `src/rng.rs`)

**Interfaces:**
- Produces: `GameId(u64)`, `SeatId(u8)`, `GameError`, `ToolError`, `SeededRng::from_seed(u64)`, `SeededRng::substream(&self, label: &str) -> impl Rng`
- Consumes: nothing

- [ ] **Step 1: Add dependencies to `Cargo.toml`**

```toml
[dependencies]
rand = "0.8"
rand_chacha = "0.3"
thiserror = "1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
uuid = { version = "1", features = ["v4"] }
```

- [ ] **Step 2: Write failing test for deterministic substreams**

```rust
#[test]
fn same_seed_same_substream_bytes() {
    let mut a = botc_mcp::rng::SeededRng::from_seed(42);
    let mut b = botc_mcp::rng::SeededRng::from_seed(42);
    let x: u64 = a.substream("washerwoman").gen();
    let y: u64 = b.substream("washerwoman").gen();
    assert_eq!(x, y);
    let z: u64 = a.substream("false_info").gen();
    assert_ne!(x, z); // different labels diverge (with overwhelming probability)
}
```

- [ ] **Step 3: Run test — expect fail (module missing)**

Run: `cargo test same_seed_same_substream_bytes -- --nocapture`  
Expected: compile fail or test fail

- [ ] **Step 4: Implement `error.rs`, `ids.rs`, `rng.rs`**

```rust
// src/error.rs — sketch
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum GameError {
    #[error("no such seat")]
    NoSuchSeat,
    #[error("unauthorized")]
    Unauthorized,
    #[error("wrong phase")]
    WrongPhase,
    #[error("game ended")]
    GameEnded,
    #[error("illegal action: {0}")]
    IllegalAction(&'static str),
    #[error("not your wake")]
    NotYourWake,
    #[error("bad request: {0}")]
    BadRequest(&'static str),
}
```

`SeededRng`: store master `ChaCha8Rng::seed_from_u64(seed)`. `substream(label)` hashes `seed || label` into a new `ChaCha8Rng` (use `std::collections::hash_map::DefaultHasher` or `rand::SeedableRng` from a derived u64).

- [ ] **Step 5: Run test — expect pass**

Run: `cargo test same_seed_same_substream_bytes`

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock src/error.rs src/rng.rs src/game/ids.rs src/lib.rs
git commit -m "feat: add errors, ids, and seeded RNG"
```

---

### Task 2: Auth tokens (CSPRNG)

**Files:**
- Modify: `src/auth.rs`
- Test: unit tests in `src/auth.rs`

**Interfaces:**
- Produces: `Token::generate() -> Token` using `uuid::Uuid::new_v4()`, `TokenBook::{issue_host, issue_player, resolve, player_token, host_token}`
- Consumes: `SeatId`

- [ ] **Step 1: Write failing test**

```rust
#[test]
fn tokens_unique_and_resolve() {
    let mut book = TokenBook::default();
    let h = book.issue_host();
    let p0 = book.issue_player(SeatId(0));
    let p1 = book.issue_player(SeatId(1));
    assert_ne!(h.as_str(), p0.as_str());
    assert!(matches!(book.resolve(&h), Some(Actor::Host)));
    assert!(matches!(book.resolve(&p0), Some(Actor::Player { seat: SeatId(0) })));
    assert!(book.resolve(&Token::from_shared("nope")).is_none());
}
```

- [ ] **Step 2: Run — expect fail if still using weak tokens**

- [ ] **Step 3: Replace `Token::generate` with UUID v4; keep `Debug` redaction as `Token(***)`**

- [ ] **Step 4: Run — expect pass**

- [ ] **Step 5: Commit**

```bash
git commit -am "feat: CSPRNG player/host tokens"
```

---

### Task 3: Comms — public log & private inbox

**Files:**
- Modify: `src/comms.rs`
- Test: unit tests in `src/comms.rs`

**Interfaces:**
- Produces: `PublicEvent` variants per spec §12; `PrivateMessage` variants (`YouAre`, `NightPrompt`, `NightResult`, `EvilBriefing`, `System`); `PublicLog::push/since`; `PrivateInboxes::push/since`
- Consumes: `SeatId`, `Team`

- [ ] **Step 1: Write failing test for cursor isolation**

```rust
#[test]
fn private_inbox_is_per_seat() {
    let mut boxes = PrivateInboxes::default();
    boxes.push(SeatId(0), PrivateMessage::System { text: "only-0".into() });
    boxes.push(SeatId(1), PrivateMessage::System { text: "only-1".into() });
    assert_eq!(boxes.since(SeatId(0), 0).len(), 1);
    assert!(format!("{:?}", boxes.since(SeatId(0), 0)[0].1).contains("only-0"));
    assert!(!format!("{:?}", boxes.since(SeatId(1), 0)[0].1).contains("only-0"));
}
```

- [ ] **Step 2–4: Implement full event enums (serde later ok); pass tests; commit**

```bash
git commit -am "feat: public log and private inbox events"
```

---

### Task 4: Seat + Game lobby + create_game API

**Files:**
- Modify: `src/game/seat.rs`, `src/game/state.rs`, `src/game/mod.rs`
- Create: `src/store.rs`
- Modify: `src/tools/mod.rs` — `create_game`
- Test: `tests/setup_tests.rs`

**Interfaces:**
- Produces:
  - `Game::create(player_names: Vec<String>, seed: u64) -> CreateGameResult { game, host_token, player_tokens }`
  - `GameStore::insert/get_mut`
  - `tools::create_game(store, names, seed) -> CreateGameResponse`
- Consumes: `TokenBook`, `Seat`, `SeededRng` stored on `Game`

- [ ] **Step 1: Failing test**

```rust
#[test]
fn create_game_issues_n_player_tokens() {
    let out = botc_mcp::tools::create_game_in_memory(
        vec!["A".into(), "B".into(), "C".into(), "D".into(), "E".into()],
        1,
    );
    assert_eq!(out.players.len(), 5);
    assert_eq!(out.players[0].name, "A");
    assert!(!out.host_token.as_str().is_empty());
}
```

- [ ] **Step 2: Implement `Game` lobby fields: `id`, `seed`, `rng`, `phase: Lobby`, `seats`, `tokens`, empty logs, `winner: None`**

- [ ] **Step 3: Reject `player_names.len() < 5 || > 15` with `GameError::BadRequest`**

- [ ] **Step 4: Pass tests; commit**

```bash
git commit -am "feat: create_game lobby with tokens"
```

---

### Task 5: Setup — composition, bag, Drunk face, start_game

**Files:**
- Create: `src/game/setup.rs`
- Modify: `src/game/state.rs` — `start_game`
- Modify: `src/tools/mod.rs`
- Test: `tests/setup_tests.rs`, `tests/identity_tests.rs`

**Interfaces:**
- Produces:
  - `composition(n: u8) -> Composition { townsfolk, outsiders, minions, demons: 1 }`
  - `build_bag(rng, n, overrides?) -> BagResult { assignments: Vec<(SeatId, Character)>, drunk_faces, red_herring, demon_bluffs, bag_set }`
  - `Game::start_game(&mut self, host: &Token, opts: StartOpts) -> Result<()>`
- Consumes: `Character` pool helpers `all_townsfolk()`, etc.

**Composition table** (copy exactly from `docs/setup.md`):

| n | TF | Out | Min | Dem |
| 5–6 | as table | | | 1 |
| … through 15 | | | | |

Baron: after minion sample includes Baron, outsiders = base+2, townsfolk = base-2.

Drunk face: uniform among 13 Townsfolk via `rng.substream("drunk_face")`.

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn composition_8() {
    let c = botc_mcp::game::setup::composition(8);
    assert_eq!((c.townsfolk, c.outsiders, c.minions, c.demons), (5, 1, 1, 1));
}

#[test]
fn drunk_private_state_never_says_drunk() {
    // fixed assignments: seat0 Drunk face Empath, seat1 Imp, ... fill 5 seats legally
    let mut g = fixture_assigned(...);
    let tok = g.tokens.player_token(SeatId(0)).unwrap().clone();
    let view = botc_mcp::tools::get_private_state(&g, &tok, 0).unwrap();
    assert_eq!(view.character_label.as_deref(), Some("Empath"));
    assert!(!format!("{view:?}").to_lowercase().contains("drunk") || view.character_label.as_deref() != Some("Drunk"));
    assert_ne!(view.character_label.as_deref(), Some("Drunk"));
    assert_eq!(g.seats[0].true_character, Some(Character::Drunk));
}
```

Implement `Debug` on `PrivateStateView` or assert field-by-field only.

- [ ] **Step 2: Implement composition + bag builder + `start_game`**

On start:
1. Auth host
2. Build/assign characters
3. Private `YouAre` per seat using **player-facing** character only
4. Phase → `FirstNight { cursor: 0 }` (night queue may be empty until Task 6)
5. Public announce first night

- [ ] **Step 3: `StartOpts` supports `assignments: Option<Vec<RoleAssignment>>` for tests (hard-fail if no Imp)**

- [ ] **Step 4: Pass tests; commit**

```bash
git commit -am "feat: setup bag assignment and drunk faces"
```

---

### Task 6: Phase types + night queue builder

**Files:**
- Modify: `src/game/phase.rs`
- Create: `src/roles/night_order.rs`
- Create: `src/game/night.rs` (queue only first)
- Test: `tests/night_tests.rs`

**Interfaces:**
- Produces:
  - `Phase::{Lobby, FirstNight { cursor }, Day { day, stage }, Night { night, cursor }, Ended { winner, reason }}`
  - `DayStage::{Discussion, Nominations}`
  - `NightStep` enum covering all steps in spec §9.1
  - `build_first_night_queue(game) -> Vec<NightStep>`
  - `build_other_night_queue(game) -> Vec<NightStep>`
- Consumes: in-play characters via true + face for townsfolk wakes

**Wake eligibility rule (spec):**
- Info/protect townsfolk-outsider steps: include seat if `player_facing_character() == Role` and alive (or special cases).
- Minion/Demon steps: `true_character` match and alive.
- Drunk face Empath → Empath step present for that seat.

- [ ] **Step 1: Failing test**

```rust
#[test]
fn first_night_queue_includes_faced_empath_not_drunk_token() {
    // seat0 Drunk face Empath, poisoner, imp, ...
    let q = build_first_night_queue(&game);
    assert!(q.iter().any(|s| matches!(s, NightStep::Empath { seat: SeatId(0) })));
    assert!(!q.iter().any(|s| matches!(s, NightStep::DemonKill { .. }))); // no N1 kill
}
```

- [ ] **Step 2: Implement queue builders; store `night_queue` + `night_cursor` on Game when entering night**

- [ ] **Step 3: Pass; commit**

```bash
git commit -am "feat: night order queue construction"
```

---

### Task 7: Night machine — pending wake, auto ST steps, night_action stub path

**Files:**
- Modify: `src/game/night.rs`, `src/game/state.rs`
- Modify: `src/tools/mod.rs` — `night_action`, `skip_night_action`, `get_private_state.awaiting`
- Test: `tests/night_tests.rs`

**Interfaces:**
- Produces:
  - `Game::night_tick(&mut self)` — process from cursor until pending choice or dawn
  - `Game::night_action(&mut self, token, payload) -> Result<()>`
  - `PendingWake { seat, step, schema }`
  - `NightActionPayload::{Ack, PickOne{target}, PickTwo{a,b}}`
- Consumes: ability dispatch (Task 8–9); for this task, briefings + ack-only steps can be fully implemented

**Behavior:**
1. After `start_game`, call `night_tick`.
2. `SetupMarkers` / completed setup: skip.
3. `MinionBriefing` / `DemonBriefing` (n>=7): push private messages, advance (no player action).
4. Choice steps: set pending, push `NightPrompt`, **stop**.
5. On `night_action` from correct seat: resolve (Task 8), clear pending, `night_tick` again.
6. Host `skip_night_action`: apply role default (pick seat 0 or random living via seed), then continue.

- [ ] **Step 1: Failing test — 7p minion briefing without player call**

```rust
#[test]
fn seven_player_minion_learns_demon_on_start() {
    let mut g = fixture_7p_with_poisoner_imp(...);
    g.start_game(...).unwrap();
    // night_tick ran briefings
    let minion_tok = ...;
    let priv = get_private_state(&g, &minion_tok, 0).unwrap();
    assert!(priv.messages.iter().any(|m| matches!(m, PrivateMessage::EvilBriefing { .. })));
}
```

- [ ] **Step 2: Implement briefing auto steps + pending for Poisoner**

- [ ] **Step 3: Pass; commit**

```bash
git commit -am "feat: night tick, briefings, pending wakes"
```

---

### Task 8: Ability resolution — info roles + disabled lies

**Files:**
- Create: `src/game/ability/mod.rs`, `info.rs`, `register.rs`
- Modify: `src/game/night.rs` to call dispatch
- Test: `tests/night_tests.rs`

**Interfaces:**
- Produces: `resolve_night_step(game, step, payload) -> Result<NightEffect>`
- `NightEffect` may include private messages, no public death yet
- Consumes: `SeededRng` substreams, `register.rs`

Implement for: Washerwoman, Librarian, Investigator, Chef, Empath, FortuneTeller, Butler (store master), Undertaker, Ravenkeeper (learn true name).

Disabled path (`true_character == Drunk || poisoned`): always lie per spec §11.1.

Registration: Empath/Chef/FT as spec §11.2 (seeded p=0.5).

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn empath_counts_living_evil_neighbors() {
    // circle: Good, Imp, Empath, Good, Good — Empath neighbors Imp + Good => 1
}

#[test]
fn drunk_empath_gets_wrong_info() {
    // truth would be 1; disabled always lie => 0 or 2
}

#[test]
fn fortune_teller_red_herring_pings_yes() {
    // pick herring + good townsfolk => yes even without demon
}
```

- [ ] **Step 2: Implement info abilities + librarian zero outsiders**

- [ ] **Step 3: Pass; commit**

```bash
git commit -am "feat: night info abilities and false info policy"
```

---

### Task 9: Poisoner, Monk, Imp kill, Soldier, Mayor bounce, Ravenkeeper insert, Dawn

**Files:**
- Create: `src/game/ability/evil.rs`, `protect.rs`
- Modify: `src/game/night.rs`, `src/game/win.rs` (minimal)
- Test: `tests/night_tests.rs`, `tests/win_tests.rs`

**Interfaces:**
- Produces:
  - `apply_poison(game, target)`
  - `clear_poisons(game)` at start of Poisoner step
  - `try_demon_kill(game, target) -> KillResult`
  - `dawn(game)` → Day Discussion
- Consumes: `win_check` (can stub returning None until Task 11 for pure night tests)

**Kill order (spec §9.5):** dead sink; monk protect; soldier; mayor bounce; else die; if Ravenkeeper trigger insert wake; starpass if self.

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn soldier_survives_imp() { ... }

#[test]
fn monk_protects_target() { ... }

#[test]
fn imp_starpass_transfers_to_minion() {
    // imp picks self; minion becomes imp; private YouAre Imp; old imp dead
}

#[test]
fn dawn_announces_deaths_publicly_not_roles() { ... }
```

- [ ] **Step 2: Implement kill + dawn; other-night queue includes Imp**

- [ ] **Step 3: Pass; commit**

```bash
git commit -am "feat: demon kills, protection, starpass, dawn"
```

---

### Task 10: Day — nominate, vote, close_vote, end_nominations, Virgin, Butler, ghost vote

**Files:**
- Create: `src/game/day.rs`
- Modify: `src/tools/mod.rs`
- Test: `tests/day_tests.rs`

**Interfaces:**
- Produces:
  - `open_nominations(host)`
  - `nominate(player, target)`
  - `vote(player, nominee, support: bool)`
  - `close_vote(host | auto)`
  - `end_nominations(host)`
- Consumes: `resolve_execution` (Task 11), Virgin rules

**Rules checklist:**
- Living nominate once/day; target nominated once/day
- Vote threshold `yes * 2 >= living`
- Leader strict majority of yes totals
- Auto close_vote when all living voted
- Ghost: yes spends token; no does not
- Butler: yes only if master already yes
- Virgin first nom: Townsfolk nominator executed immediately

- [ ] **Step 1: Failing tests**

```rust
#[test]
fn vote_threshold_six_living_needs_three() { ... }

#[test]
fn virgin_kills_townsfolk_nominator() { ... }

#[test]
fn drunk_nominator_does_not_trigger_virgin() { ... }

#[test]
fn ghost_yes_only_once() { ... }
```

- [ ] **Step 2: Implement day.rs + wire tools**

- [ ] **Step 3: Pass; commit**

```bash
git commit -am "feat: nominations voting virgin butler ghost"
```

---

### Task 11: Win conditions — execution, SW, Saint, Mayor, Slayer, two-alive

**Files:**
- Create/modify: `src/game/win.rs`
- Modify: `src/game/day.rs`, `src/game/ability/evil.rs`
- Modify: `src/tools/mod.rs` — `day_action` slay
- Test: `tests/win_tests.rs`, `tests/scenario_*.rs`

**Interfaces:**
- Produces:
  - `win_check(game)`
  - `resolve_execution(game, seat)`
  - `apply_demon_death(game, seat, alive_before)`
  - `day_action_slay(game, slayer, target)`
- Consumes: seat flags, SW eligibility

**Order in `win_check`:** if no living Imp → Good; if living==2 && living Imp → Evil; (Saint/Mayor set winner at event time).

- [ ] **Step 1: Failing tests for each EndReason**

```rust
#[test]
fn execute_imp_good_wins() { ... }

#[test]
fn sw_converts_when_alive_before_ge_5() { ... }

#[test]
fn sw_no_convert_at_4_alive_before() { ... }

#[test]
fn saint_executed_evil_wins() { ... }

#[test]
fn mayor_three_no_exec_good_wins() { ... }

#[test]
fn two_living_with_imp_evil_wins() { ... }

#[test]
fn slayer_hits_imp() { ... }
```

- [ ] **Step 2: Implement; ensure simultaneous demon death + 2 alive → Good**

- [ ] **Step 3: Pass; commit**

```bash
git commit -am "feat: win conditions slayer scarlet woman saint mayor"
```

---

### Task 12: Tool surface completeness + host state + character rules loader

**Files:**
- Modify: `src/tools/mod.rs`, `src/tools/views.rs`
- Create: `src/tools/rules_text.rs` — read `docs/roles/...` from crate-relative path or `CARGO_MANIFEST_DIR`
- Test: `tests/identity_tests.rs` (isolation), tool auth tests

**Interfaces:**
- Produces all tools from spec §12:
  - create_game, start_game, get_public_state, get_public_log, get_private_state, get_character_rules, get_host_state, say, night_action, day_action, nominate, vote, open_nominations, close_vote, end_nominations, skip_night_action, st_announce
- `get_public_state` must **not** include other seats’ pending night identity; host sees pending via `get_host_state`

- [ ] **Step 1: Failing test — player cannot call get_host_state**

```rust
#[test]
fn player_forbidden_from_host_state() {
    let err = get_host_state(&g, &player_tok).unwrap_err();
    assert!(matches!(err, ToolError::Unauthorized | ToolError::Game(GameError::Unauthorized)));
}
```

- [ ] **Step 2: Implement remaining tools; load markdown for rules**

- [ ] **Step 3: Pass; commit**

```bash
git commit -am "feat: complete MCP semantic tool surface"
```

---

### Task 13: Integration scenarios

**Files:**
- Create: `tests/common/mod.rs`
- Create: `tests/scenario_imp_exec.rs`
- Create: `tests/scenario_sw_starpass.rs`
- Create: `tests/scenario_full_night_day.rs`

**Interfaces:**
- Consumes: full tool/game API
- Produces: confidence the engine runs start→win

- [ ] **Step 1: Write scenario — 5p scripted, execute Imp on day 1**

Fixed assignments: Imp, Soldier, Empath, Monk, Virgin (adjust composition: 5p = 3TF 0Out 1Min 1Dem — use Poisoner not SW).

Script: start → skip/finish N1 → open noms → nominate imp → all yes → end noms → winner Good.

- [ ] **Step 2: Write scenario — SW conversion then later win**

- [ ] **Step 3: Write scenario — starpass mid-game**

- [ ] **Step 4: `cargo test` all green; commit**

```bash
git commit -am "test: end-to-end TB scenarios"
```

---

### Task 14: MCP server binary (transport)

**Files:**
- Modify: `src/main.rs`
- Modify: `Cargo.toml` — add MCP SDK when chosen (`rmcp` or `mcp-server` crate current in ecosystem)
- Create: `src/mcp_server.rs` — map JSON tools to `tools::*`
- Create: `examples/harness_smoke.rs` optional

**Interfaces:**
- Produces: stdio MCP server exposing tools from Task 12
- Consumes: `GameStore` behind `Mutex`

- [ ] **Step 1: Research/add working Rust MCP server dependency (pin version in Cargo.toml)**

If no stable crate works in-environment, implement a thin JSON-RPC stdio stub that matches MCP tool call shape used by the harness, documented in `docs/mcp.md`.

- [ ] **Step 2: Wire each tool name to handler; shared `Arc<Mutex<GameStore>>`**

- [ ] **Step 3: Manual smoke: start server, create_game via client, assert response**

- [ ] **Step 4: Commit**

```bash
git commit -am "feat: MCP server transport wiring"
```

---

### Task 15: Docs + AGENTS checklist polish

**Files:**
- Modify: `README.md`, `docs/architecture.md` (point to implemented modules)
- Modify: `AGENTS.md` if any new invariants
- Mark design spec status Implemented-in-progress → done when Task 13 passes

- [ ] **Step 1: Update README with run/test/MCP instructions**

- [ ] **Step 2: Ensure `cargo test` and `cargo build` documented**

- [ ] **Step 3: Commit**

```bash
git commit -am "docs: engine usage and architecture sync"
```

---

## Spec coverage checklist (self-review)

| Spec area | Task |
| --- | --- |
| §6 Identity / Drunk | 5, 12 |
| §8 Setup / Baron / bag | 5 |
| §9 Night machine | 6–9 |
| §9 per-role night | 8–9 |
| §10 Day / votes / Virgin | 10 |
| §10 Slayer | 11 |
| §11 ST policy | 8–9 |
| §12 Tools | 4, 7, 10–12, 14 |
| §13 Win check | 11 |
| §14 Character matrix | 8–11 |
| §17 Tests | 1–13 |
| §18 Harness contract | 13–14 |
| §23 Edge cases | 10–11 |

## Execution notes

- Prefer **TDD**: failing test → implement → pass → commit per task.
- Keep player tool responses free of secrets (`AGENTS.md`).
- Do not implement other editions.
- If a task grows too large mid-flight, split at the next commit boundary but do not skip tests.

