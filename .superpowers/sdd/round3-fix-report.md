# PR re-audit round 3 (#15–#20) fix report

**Branch:** `fix/issue-1-tb-rules-audit`  
**Date:** 2026-07-08  
**Scope:** Second re-audit follow-ups after #3–#14.

## Status

All requested items **#15–#20** implemented. `cargo test` green.

## Fixes

| # | Severity | Change |
| --- | --- | --- |
| 15 | High | `register_character(game, seat, label, viewer)` excludes viewer true/face/believed via `acting_exclude_chars`; Undertaker/RK pass `Some(seat)` |
| 16 | High | `resolve_pair_role`: truth → zero_message if empty → Inv force-true minions → sole-TF WW includes actor; Spy/Recluse hide only when another true owner exists; Lib zero no longer requires empty `seats_of_type` |
| 17 | Medium | Poisoned/Drunk seats: `register_evil`, `register_demon_for_ft`, `register_character`, `register_as_type_owner_with` return true alignment/type/token only (no misreg/hide) |
| 18 | Low | Mayor bounce with empty candidates → `die_from_demon(mayor)` |
| 19 | Low | Pin golden `mix(1, 2, "setup") == 0x73511a5b7da1f833` in `rng.rs` + reaudit test |
| 20 | Low | `pick_misreg_token(..., default: Character)` explicit fallback per call site |

## Tests

- `tests/reaudit_followups.rs` — #15 RK Spy exclude viewer; #16 sole Spy Inv, sole Recluse Lib, Lib zero, sole TF WW, healthy Inv; #17 poisoned Spy; #18 empty bounce; #19 golden
- `src/game/ability/register.rs` unit — sole Spy cannot hide; ability_disabled blocks misreg; multi-minion hide still works

## Verification

```bash
cargo test
```
