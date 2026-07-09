# PR re-audit round 4 (#21–#26) fix report

**Branch:** `fix/issue-1-tb-rules-audit`  
**Date:** 2026-07-08  
**Scope:** Third re-audit follow-ups after #15–#20.

## Status

All requested items **#21–#26** implemented. `cargo test` green.

## Fixes

| # | Severity | Change |
| --- | --- | --- |
| 21 | Medium | `registers_as_townsfolk`: if `ability_disabled()`, return true type only (`Spy`/`Minion` → false). Poisoned Spy never triggers Virgin. |
| 22 | ST host | Imp→Mayor sets `pending_host: MayorRedirect`; `host_decide` (`kill_mayor` / `kill_other` / `nobody`); skip default → nobody dies when living others exist, else kill Mayor. |
| 23 | ST host | Imp starpass sets `pending_host: StarpassPick`; host picks minion; skip → random living minion (prior default). |
| 24 | ST host | `StartOpts`: `drunk_faces`, `red_herring`, `demon_bluffs`, `registration_mode`. Wired through `start_game` + MCP. |
| 25 | ST host | `RegistrationMode` on all 5 register paths; `host_queue_lie` FIFO free-text for disabled info. Structured lie authoring deferred (docs note). |
| 26 | Low | Removed dead `pair_owners_force_true` + Inv force-true branch; updated `resolve_pair_role` comments; fixed tautological RK assertion. |

## New API

- `Game.pending_host` / `registration_mode` / `host_lie_queue`
- Tools: `host_decide`, `host_queue_lie`
- `start_game` args: `registration_mode`, `drunk_faces`, `red_herring`, `demon_bluffs`
- `get_host_state`: `pending_host`, `registration_mode`, `host_lie_queue_len`
- Module: `src/game/st_policy.rs`

## Tests

- `tests/reaudit_round4.rs` — #21 Virgin×Spy, Mayor pending, starpass host pick, red_herring override, AlwaysTrue Spy, host_queue_lie
- Updated mayor/starpass integration tests for host pending + skip defaults

## Verification

```bash
cargo test
```
