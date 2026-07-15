//! History-balanced role assignment (issue #70).
//!
//! The bag composition for a player count is fixed (see [`crate::game::setup`]);
//! what we choose is **which model plays which character**. To keep the eval
//! corpus balanced, we steer the assignment so each model accumulates an even
//! record, lexicographically:
//!   1. **team** (Good / Evil) — the primary axis the rater scores on,
//!   2. **role type** (Townsfolk / Outsider / Minion / Demon) — tie-breaks (1),
//!   3. **specific role** (Empath, Poisoner, …) — tie-breaks (2).
//!
//! Only models that have **≥1 completed game** drive the balance (the same games
//! the leaderboard rates; aborted games carry no eval signal and are not counted).
//! They are the only ones with an imbalance to correct. Seats whose model is new are
//! neutral: known-model seats are matched first, and new-model seats fill the
//! leftover characters at random — a new model simply builds its own record from
//! the games it plays rather than distorting everyone else's.

use std::collections::HashMap;
use std::path::Path;

use rand::seq::SliceRandom;
use rand::Rng;
use serde_json::Value;

use crate::game::{RoleAssignment, SeatId};
use crate::roles::Character;

/// Per-model play history. Only models with ≥1 recorded game get an entry.
#[derive(Debug, Default, Clone)]
pub struct ModelStats {
    pub games: u32,
    /// "Good" / "Evil" → count.
    pub team: HashMap<String, u32>,
    /// "Townsfolk" / "Outsider" / "Minion" / "Demon" → count.
    pub role_type: HashMap<String, u32>,
    /// Role display name ("Fortune Teller", …) → count.
    pub role: HashMap<String, u32>,
}

impl ModelStats {
    fn team_n(&self, k: &str) -> u32 {
        *self.team.get(k).unwrap_or(&0)
    }
    fn type_n(&self, k: &str) -> u32 {
        *self.role_type.get(k).unwrap_or(&0)
    }
    fn role_n(&self, k: &str) -> u32 {
        *self.role.get(k).unwrap_or(&0)
    }
}

/// String keys for a character, matching the forms written to the results log
/// (`format!("{:?}", team())`, `format!("{:?}", character_type())`, `display_name()`).
fn keys(c: Character) -> (String, String, String) {
    (
        format!("{:?}", c.team()),
        format!("{:?}", c.character_type()),
        c.display_name().to_string(),
    )
}

/// Compose the ranking/identity node key for a seat (issue #69). Grok — and legacy
/// rows with no `backend` field — stay **bare**, so pre-#69 corpus and ratings keys
/// are unchanged and no leaderboard node splits. Other backends are namespaced
/// `<backend>:<model>`, so a native claude seat never conflates with claude weights
/// served through grok. Empty model → empty key (the caller skips it). Must stay
/// identical to `node_key` in scripts/rate_models.py and scripts/model_aliases.py.
pub fn node_key(backend: &str, model: &str) -> String {
    if model.is_empty() {
        String::new()
    } else if backend.is_empty() || backend == "grok" {
        model.to_string()
    } else {
        format!("{backend}:{model}")
    }
}

/// Tally per-model history from a `botc-results.jsonl` file. Counts each seat of
/// every `game_end` event (one per *completed* game) by team / role type / role.
/// These are the same games the leaderboard rates: an aborted game (`game_abort`)
/// produces no eval signal, so counting it would balance against games that never
/// mattered — hence `game_start`/`game_abort` are ignored. Missing file or
/// unreadable lines yield an empty map — the caller then falls back to a random
/// assignment. Only models that actually appear are inserted.
pub fn read_model_stats(path: &Path) -> HashMap<String, ModelStats> {
    let mut out: HashMap<String, ModelStats> = HashMap::new();
    let Ok(text) = std::fs::read_to_string(path) else {
        return out;
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if v.get("event").and_then(Value::as_str) != Some("game_end") {
            continue;
        }
        let Some(seats) = v.get("seats").and_then(Value::as_array) else {
            continue;
        };
        for s in seats {
            let model = s.get("model").and_then(Value::as_str).unwrap_or("");
            // Compose-on-read: legacy rows (no backend) default to grok → bare model.
            let backend = s.get("backend").and_then(Value::as_str).unwrap_or("grok");
            let key = node_key(backend, model);
            if key.is_empty() {
                continue;
            }
            let entry = out.entry(key).or_default();
            entry.games += 1;
            if let Some(t) = s.get("team").and_then(Value::as_str) {
                *entry.team.entry(t.to_string()).or_insert(0) += 1;
            }
            if let Some(ct) = s.get("character_type").and_then(Value::as_str) {
                *entry.role_type.entry(ct.to_string()).or_insert(0) += 1;
            }
            if let Some(r) = s.get("true_character").and_then(Value::as_str) {
                *entry.role.entry(r.to_string()).or_insert(0) += 1;
            }
        }
    }
    out
}

/// Reassign a fixed bag of characters to the given player seats so each model's
/// record stays balanced (team → role type → role, lexicographically). The bag
/// (`true_character`, `believed_character` pairs — the latter carries a Drunk's
/// Townsfolk face) is preserved exactly; only *which seat* gets each entry changes,
/// so the result is always a valid setup for the same composition.
///
/// `seat_models[i]` is the model at `seats[i]`. `bag.len()` must equal `seats.len()`.
pub fn balanced_assignment(
    seats: &[SeatId],
    seat_models: &[&str],
    bag: &[(Character, Option<Character>)],
    stats: &HashMap<String, ModelStats>,
    rng: &mut impl Rng,
) -> Vec<RoleAssignment> {
    let n = seats.len();
    debug_assert_eq!(n, bag.len());
    debug_assert_eq!(n, seat_models.len());

    // Random order up front so ties (and new-seat fill) resolve arbitrarily.
    let mut char_order: Vec<usize> = (0..n).collect();
    char_order.shuffle(rng);
    let mut seat_order: Vec<usize> = (0..n).collect();
    seat_order.shuffle(rng);

    // Split seats into known (model has history) and new.
    let mut known: Vec<usize> = Vec::new();
    let mut fresh: Vec<usize> = Vec::new();
    for &si in &seat_order {
        if stats.contains_key(seat_models[si]) {
            known.push(si);
        } else {
            fresh.push(si);
        }
    }

    let mut assign: Vec<Option<usize>> = vec![None; n];
    let mut chars: Vec<usize> = char_order; // remaining char indices, shuffled

    // Greedy min-cost matching over known seats: repeatedly take the (char, seat)
    // pair whose lexicographic cost is lowest. Cost of putting char c on model m is
    // (m's count of c.team, of c.role_type, of c.role): assigning a team/type/role
    // to the model that has the fewest of it corrects the imbalance. Shuffled order
    // + strict `<` gives a random tie-break.
    while !known.is_empty() && !chars.is_empty() {
        let mut best: Option<(usize, usize, (u32, u32, u32))> = None; // (char_pos, known_pos, cost)
        for (cp, &ci) in chars.iter().enumerate() {
            let (team, rtype, role) = keys(bag[ci].0);
            for (kp, &si) in known.iter().enumerate() {
                let st = &stats[seat_models[si]];
                let cost = (st.team_n(&team), st.type_n(&rtype), st.role_n(&role));
                if best.is_none_or(|(_, _, bc)| cost < bc) {
                    best = Some((cp, kp, cost));
                }
            }
        }
        let (cp, kp, _) = best.expect("non-empty");
        let ci = chars.remove(cp);
        let si = known.remove(kp);
        assign[si] = Some(ci);
    }

    // The greedy consumed exactly one character per known seat, so the remaining
    // characters line up one-to-one with the shuffled new-model seats.
    for (&si, &ci) in fresh.iter().zip(chars.iter()) {
        assign[si] = Some(ci);
    }

    (0..n)
        .map(|i| {
            let ci = assign[i].expect("every seat assigned a character");
            let (true_character, believed_character) = bag[ci];
            RoleAssignment {
                seat: seats[i],
                true_character,
                believed_character,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::StdRng;
    use rand::SeedableRng;

    /// Build one model's history from `(team, role_type, role, count)` rows.
    fn model(rows: &[(&str, &str, &str, u32)]) -> ModelStats {
        let mut s = ModelStats::default();
        for (team, rt, role, c) in rows {
            s.games += c;
            *s.team.entry(team.to_string()).or_insert(0) += c;
            *s.role_type.entry(rt.to_string()).or_insert(0) += c;
            *s.role.entry(role.to_string()).or_insert(0) += c;
        }
        s
    }

    fn stats_with(rows: Vec<(&str, ModelStats)>) -> HashMap<String, ModelStats> {
        rows.into_iter().map(|(m, s)| (m.to_string(), s)).collect()
    }

    fn team_of(assigns: &[RoleAssignment], seat: u8) -> String {
        let a = assigns.iter().find(|a| a.seat.0 == seat).unwrap();
        format!("{:?}", a.true_character.team())
    }

    // 5p bag: 3 Townsfolk (Good), 1 Minion + 1 Demon (Evil).
    fn bag_5p() -> Vec<(Character, Option<Character>)> {
        vec![
            (Character::Empath, None),
            (Character::FortuneTeller, None),
            (Character::Chef, None),
            (Character::Poisoner, None), // Minion
            (Character::Imp, None),      // Demon
        ]
    }

    #[test]
    fn team_balance_gives_evil_to_the_model_owed_evil() {
        // Strict owed-evil ordering by evil count: A(0) < D(1) < E(2) < C(3) < B(6).
        // With 2 Evil slots, the two lowest (A and D) must get Evil; B (most evil,
        // owed Good) must get Good. Asserting A + B is then deterministic.
        let stats = stats_with(vec![
            ("A", model(&[("Good", "Townsfolk", "Empath", 6)])),
            ("B", model(&[("Evil", "Minion", "Poisoner", 6)])),
            (
                "C",
                model(&[
                    ("Good", "Townsfolk", "Chef", 3),
                    ("Evil", "Demon", "Imp", 3),
                ]),
            ),
            (
                "D",
                model(&[
                    ("Good", "Townsfolk", "Chef", 5),
                    ("Evil", "Minion", "Poisoner", 1),
                ]),
            ),
            (
                "E",
                model(&[
                    ("Good", "Townsfolk", "Empath", 4),
                    ("Evil", "Demon", "Imp", 2),
                ]),
            ),
        ]);
        let seats: Vec<SeatId> = (0..5).map(SeatId).collect();
        let models = ["A", "B", "C", "D", "E"];
        let mut rng = StdRng::seed_from_u64(1);
        let a = balanced_assignment(&seats, &models, &bag_5p(), &stats, &mut rng);
        // A (never Evil) must be Evil; B (always Evil) must be Good.
        assert_eq!(team_of(&a, 0), "Evil", "A owed evil");
        assert_eq!(team_of(&a, 1), "Good", "B owed good");
        // D (evil=1) is the other owed-evil seat.
        assert_eq!(team_of(&a, 3), "Evil", "D is the second-most owed evil");
        // exactly 2 evil seats overall (valid composition preserved)
        let evil = (0..5).filter(|&s| team_of(&a, s) == "Evil").count();
        assert_eq!(evil, 2);
    }

    #[test]
    fn role_type_breaks_team_ties_demon_to_least_demon() {
        // Two models both owed Evil equally (0 evil each) but one has played the
        // Demon a lot and the other a Minion a lot -> Demon should go to the
        // model with fewer Demon games (secondary axis).
        let stats = stats_with(vec![
            (
                "A",
                model(&[
                    ("Good", "Townsfolk", "Empath", 4),
                    ("Evil", "Demon", "Imp", 4),
                ]),
            ),
            (
                "B",
                model(&[
                    ("Good", "Townsfolk", "Chef", 4),
                    ("Evil", "Minion", "Poisoner", 4),
                ]),
            ),
            // fillers owed Good so A and B take the two Evil slots
            ("C", model(&[("Evil", "Minion", "Poisoner", 5)])),
            ("D", model(&[("Evil", "Demon", "Imp", 5)])),
            ("E", model(&[("Evil", "Minion", "Baron", 5)])),
        ]);
        let seats: Vec<SeatId> = (0..5).map(SeatId).collect();
        let models = ["A", "B", "C", "D", "E"];
        let mut rng = StdRng::seed_from_u64(7);
        let a = balanced_assignment(&seats, &models, &bag_5p(), &stats, &mut rng);
        // A and B are the two owed-evil seats; among them the Demon (Imp) must go to
        // B (fewer demon games) and the Minion (Poisoner) to A.
        let a_char = a.iter().find(|x| x.seat.0 == 0).unwrap().true_character;
        let b_char = a.iter().find(|x| x.seat.0 == 1).unwrap().true_character;
        assert_eq!(
            a_char,
            Character::Poisoner,
            "A (played Demon) gets the Minion"
        );
        assert_eq!(b_char, Character::Imp, "B (played Minion) gets the Demon");
    }

    #[test]
    fn specific_role_breaks_type_ties() {
        // A, B, C are all owed Good (heavy Evil history) and have played Townsfolk
        // equally — they differ only in *which* Townsfolk. A has played the Empath a
        // lot; B and C have not. With team and role type tied among them, the Empath
        // must avoid A (the tertiary axis). D and E (never Evil) take the Evil slots.
        let stats = stats_with(vec![
            (
                "A",
                model(&[
                    ("Evil", "Minion", "Poisoner", 8),
                    ("Good", "Townsfolk", "Empath", 4),
                ]),
            ),
            (
                "B",
                model(&[
                    ("Evil", "Minion", "Poisoner", 8),
                    ("Good", "Townsfolk", "Chef", 4),
                ]),
            ),
            (
                "C",
                model(&[
                    ("Evil", "Minion", "Poisoner", 8),
                    ("Good", "Townsfolk", "Chef", 4),
                ]),
            ),
            ("D", model(&[("Good", "Townsfolk", "Chef", 8)])),
            ("E", model(&[("Good", "Townsfolk", "Chef", 8)])),
        ]);
        let seats: Vec<SeatId> = (0..5).map(SeatId).collect();
        let models = ["A", "B", "C", "D", "E"];
        let mut rng = StdRng::seed_from_u64(5);
        let a = balanced_assignment(&seats, &models, &bag_5p(), &stats, &mut rng);
        let a_char = a.iter().find(|x| x.seat.0 == 0).unwrap().true_character;
        assert_eq!(team_of(&a, 0), "Good", "A is owed Good");
        assert_ne!(
            a_char,
            Character::Empath,
            "A over-played the Empath; it must go to a model that hasn't"
        );
        // D and E (evil=0) are the two owed-evil seats.
        assert_eq!(team_of(&a, 3), "Evil");
        assert_eq!(team_of(&a, 4), "Evil");
    }

    #[test]
    fn only_known_models_drive_balance_new_models_fill_leftovers() {
        // One known model owed Good (all-evil history); the rest brand new (no
        // history). The known model must NOT be forced into Evil; a new seat takes
        // an Evil slot instead.
        let stats = stats_with(vec![("KNOWN", model(&[("Evil", "Demon", "Imp", 5)]))]);
        let seats: Vec<SeatId> = (0..5).map(SeatId).collect();
        let models = ["KNOWN", "new1", "new2", "new3", "new4"];
        let mut rng = StdRng::seed_from_u64(3);
        let a = balanced_assignment(&seats, &models, &bag_5p(), &stats, &mut rng);
        assert_eq!(
            team_of(&a, 0),
            "Good",
            "the known owed-Good model must get Good"
        );
        // still exactly 2 evil among the new seats
        let evil = (0..5).filter(|&s| team_of(&a, s) == "Evil").count();
        assert_eq!(evil, 2);
    }

    #[test]
    fn no_history_is_a_valid_random_assignment() {
        let stats: HashMap<String, ModelStats> = HashMap::new();
        let seats: Vec<SeatId> = (0..5).map(SeatId).collect();
        let models = ["m", "m", "m", "m", "m"];
        let mut rng = StdRng::seed_from_u64(9);
        let a = balanced_assignment(&seats, &models, &bag_5p(), &stats, &mut rng);
        assert_eq!(a.len(), 5);
        let evil = (0..5).filter(|&s| team_of(&a, s) == "Evil").count();
        assert_eq!(evil, 2, "composition preserved even with no history");
    }

    #[test]
    fn read_model_stats_counts_only_completed_games() {
        // Only `game_end` seats count. A `game_abort` or `game_start` for the same
        // model must be ignored — those games never reached the leaderboard, so
        // they carry no eval signal to balance against.
        let path = std::env::temp_dir().join("botc_balance_read_model_stats_test.jsonl");
        let lines = [
            r#"{"event":"game_end","seats":[{"model":"grok","team":"Good","character_type":"Townsfolk","true_character":"Empath"}]}"#,
            r#"{"event":"game_abort","seats":[{"model":"grok","team":"Evil","character_type":"Demon","true_character":"Imp"}]}"#,
            r#"{"event":"game_start","seats":[{"model":"grok","team":"Evil","character_type":"Minion","true_character":"Poisoner"}]}"#,
        ];
        std::fs::write(&path, lines.join("\n")).unwrap();
        let stats = read_model_stats(&path);
        let _ = std::fs::remove_file(&path);

        let g = stats.get("grok").expect("grok has one completed game");
        assert_eq!(g.games, 1, "only the game_end seat counts");
        assert_eq!(g.team_n("Good"), 1);
        assert_eq!(g.team_n("Evil"), 0, "aborted/started games must not count");
        assert_eq!(g.type_n("Townsfolk"), 1);
        assert_eq!(g.role_n("Empath"), 1);
    }

    #[test]
    fn node_key_golden() {
        // Legacy grok (no backend) and explicit grok both stay bare → same node.
        assert_eq!(node_key("", "grok-build"), "grok-build");
        assert_eq!(node_key("grok", "grok-build"), "grok-build");
        // Claude is namespaced and never conflates with grok:same-name.
        assert_eq!(
            node_key("claude", "claude-opus-4-8"),
            "claude:claude-opus-4-8"
        );
        assert_ne!(node_key("claude", "x"), node_key("grok", "x"));
        // Empty model → empty key (skipped by the caller).
        assert_eq!(node_key("claude", ""), "");
        assert_eq!(node_key("grok", ""), "");
    }

    #[test]
    fn read_model_stats_folds_legacy_grok_and_splits_claude() {
        let path = std::env::temp_dir().join("botc_balance_nodekey_test.jsonl");
        let lines = [
            // legacy grok row (no backend field) — Good
            r#"{"event":"game_end","seats":[{"model":"m","team":"Good","character_type":"Townsfolk","true_character":"Empath"}]}"#,
            // explicit grok row — Evil — must fold onto the SAME node as the legacy row
            r#"{"event":"game_end","seats":[{"model":"m","backend":"grok","team":"Evil","character_type":"Minion","true_character":"Poisoner"}]}"#,
            // native claude row with the SAME model name — a DISTINCT node
            r#"{"event":"game_end","seats":[{"model":"m","backend":"claude","team":"Good","character_type":"Townsfolk","true_character":"Chef"}]}"#,
        ];
        std::fs::write(&path, lines.join("\n")).unwrap();
        let stats = read_model_stats(&path);
        let _ = std::fs::remove_file(&path);
        let g = stats.get("m").expect("bare grok node");
        assert_eq!(g.games, 2, "legacy + explicit grok fold to one node");
        assert_eq!(g.team_n("Good"), 1);
        assert_eq!(g.team_n("Evil"), 1);
        let c = stats.get("claude:m").expect("claude node is separate");
        assert_eq!(c.games, 1);
        assert_eq!(c.team_n("Good"), 1);
    }
}
