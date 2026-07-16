//! History-balanced role assignment (issue #70) driven by **match-up coverage**.
//!
//! The bag composition for a player count is fixed (see [`crate::game::setup`]);
//! what we choose is **which model plays which character**.
//!
//! # What the leaderboard actually consumes
//!
//! `scripts/rate_models.py` fits Bradley–Terry over *pairwise* outcomes: each
//! winning-seat model "beats" each losing-seat model, and **same-model pairs are
//! skipped**. Winners and losers are whole teams, so every edge a game can produce
//! is a `(Good-seat model) × (Evil-seat model)` seat-pair with the two models
//! *different*. Two consequences drive this module:
//!
//! * A table where every seat is the same model yields **zero** edges — the game
//!   teaches the ratings nothing at all.
//! * A model can have a perfectly even record and still be barely rateable, because
//!   a per-model count says nothing about *whom it played against*. What pins a
//!   rating is the weight and connectivity of the pair graph.
//!
//! So the primary axis is **pair-coverage deficit**: prefer the model that has been
//! observed least against the opposing side. The per-model count axes from #70 —
//! **team** (Good / Evil), then **role type** (Townsfolk / Outsider / Minion / Demon),
//! then **specific role** (Empath, Poisoner, …) — are kept as lexicographic tie-breaks,
//! so a model still can't drift into always drawing the same team (which would confound
//! its rating with role advantage).
//!
//! Only models with **≥1 completed game** drive the balance (the same games the
//! leaderboard rates; aborted games carry no eval signal). Seats whose model is new
//! are neutral: known-model seats are matched first and new-model seats fill the
//! leftovers at random, so a new model builds its own record rather than distorting
//! everyone else's.

use std::collections::HashMap;
use std::path::Path;

use rand::seq::SliceRandom;
use rand::Rng;
use serde_json::Value;

use crate::game::{RoleAssignment, SeatId};
use crate::roles::{Character, Team};

/// Cost added for putting a model on **both** sides of the table. Those seat-pairs
/// are skipped by the rater, so they carry no information: ranked far above any real
/// pair weight, but finite, so a one-eligible-model table still resolves instead of
/// having no legal assignment. Small enough that summing one per seat cannot overflow.
const SAME_MODEL_PENALTY: u32 = 1 << 20;

/// Cross-team seat-pair coverage, keyed by an **unordered** model pair.
///
/// `pairs[(a, b)]` counts the seat-pairs in which `a` and `b` sat on opposite teams,
/// i.e. exactly the observations the BT fit accumulates for that pair (the rater keys
/// on the ordered winner/loser, but coverage of `a` vs `b` is the sum of both
/// directions — which direction a game landed depends on who won, not on the setup).
pub type PairStats = HashMap<(String, String), u32>;

/// Order-independent key for [`PairStats`].
fn pair_key(a: &str, b: &str) -> (String, String) {
    if a <= b {
        (a.to_string(), b.to_string())
    } else {
        (b.to_string(), a.to_string())
    }
}

fn pair_n(pairs: &PairStats, a: &str, b: &str) -> u32 {
    *pairs.get(&pair_key(a, b)).unwrap_or(&0)
}

/// Everything the balancer reads out of the results corpus.
#[derive(Debug, Default, Clone)]
pub struct History {
    /// Per-model record (team / role type / role counts).
    pub models: HashMap<String, ModelStats>,
    /// Cross-team seat-pair coverage between models — the BT edge weights.
    pub pairs: PairStats,
}

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

/// Read play [`History`] from a `botc-results.jsonl` file: the per-model record and
/// the cross-team pair coverage, from every `game_end` event (one per *completed*
/// game). These are the same games the leaderboard rates — an aborted game
/// (`game_abort`) produces no eval signal, so counting it would balance against games
/// that never mattered; `game_start`/`game_abort` are ignored. A missing file or
/// unreadable lines yield an empty history, and the caller falls back to a random
/// assignment. Only models that actually appear are inserted.
///
/// Pair coverage mirrors `expand_pairwise` in `scripts/rate_models.py`: one count per
/// (Good seat, Evil seat) pair of *differing* models. Same-model pairs are skipped
/// there and so are never recorded here.
pub fn read_history(path: &Path) -> History {
    let mut out = History::default();
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
        // Per-game sides, so the pair tally only crosses the Good/Evil divide.
        let (mut good, mut evil): (Vec<String>, Vec<String>) = (Vec::new(), Vec::new());
        for s in seats {
            let model = s.get("model").and_then(Value::as_str).unwrap_or("");
            // Compose-on-read: legacy rows (no backend) default to grok → bare model.
            let backend = s.get("backend").and_then(Value::as_str).unwrap_or("grok");
            let key = node_key(backend, model);
            if key.is_empty() {
                continue;
            }
            let entry = out.models.entry(key.clone()).or_default();
            entry.games += 1;
            if let Some(t) = s.get("team").and_then(Value::as_str) {
                *entry.team.entry(t.to_string()).or_insert(0) += 1;
                match t {
                    "Good" => good.push(key.clone()),
                    "Evil" => evil.push(key.clone()),
                    _ => {}
                }
            }
            if let Some(ct) = s.get("character_type").and_then(Value::as_str) {
                *entry.role_type.entry(ct.to_string()).or_insert(0) += 1;
            }
            if let Some(r) = s.get("true_character").and_then(Value::as_str) {
                *entry.role.entry(r.to_string()).or_insert(0) += 1;
            }
        }
        for g in &good {
            for e in &evil {
                if g != e {
                    *out.pairs.entry(pair_key(g, e)).or_insert(0) += 1;
                }
            }
        }
    }
    out
}

/// Per-model record only — [`read_history`] without the pair coverage.
pub fn read_model_stats(path: &Path) -> HashMap<String, ModelStats> {
    read_history(path).models
}

/// Greedy min-cost matching between `models` and `chars` on the shared lexicographic
/// cost (team → role type → specific role). Returns `out[i]` = the char index matched
/// to model `i`. Only models with ≥1 completed game drive the matching (they are the
/// only ones with an imbalance to correct); models with no history take the leftovers
/// at random. Shuffled order + strict `<` gives a random tie-break.
///
/// Used by [`balanced_assignment`] (`r`), where the seated models are pinned and the
/// drawn roles move onto them. The other direction — letting the harness *choose* which
/// models play a drawn role layout — is [`select_balanced_models`], which is a
/// selection (with repeats) rather than a matching, so it does not share this code.
fn match_models_to_chars(
    models: &[&str],
    chars: &[Character],
    stats: &HashMap<String, ModelStats>,
    rng: &mut impl Rng,
) -> Vec<usize> {
    let n = models.len();
    debug_assert_eq!(n, chars.len());

    let mut char_order: Vec<usize> = (0..n).collect();
    char_order.shuffle(rng);
    let mut model_order: Vec<usize> = (0..n).collect();
    model_order.shuffle(rng);

    let mut known: Vec<usize> = Vec::new();
    let mut fresh: Vec<usize> = Vec::new();
    for &mi in &model_order {
        if stats.contains_key(models[mi]) {
            known.push(mi);
        } else {
            fresh.push(mi);
        }
    }

    let mut assign: Vec<Option<usize>> = vec![None; n];
    let mut remaining: Vec<usize> = char_order;

    // Cost of pairing char c with model m is (m's count of c.team, of c.role_type, of
    // c.role): giving a team/type/role to the model that has the fewest of it corrects
    // the imbalance.
    while !known.is_empty() && !remaining.is_empty() {
        let mut best: Option<(usize, usize, (u32, u32, u32))> = None; // (char_pos, known_pos, cost)
        for (cp, &ci) in remaining.iter().enumerate() {
            let (team, rtype, role) = keys(chars[ci]);
            for (kp, &mi) in known.iter().enumerate() {
                let st = &stats[models[mi]];
                let cost = (st.team_n(&team), st.type_n(&rtype), st.role_n(&role));
                if best.is_none_or(|(_, _, bc)| cost < bc) {
                    best = Some((cp, kp, cost));
                }
            }
        }
        let (cp, kp, _) = best.expect("non-empty");
        let ci = remaining.remove(cp);
        let mi = known.remove(kp);
        assign[mi] = Some(ci);
    }

    // The greedy consumed exactly one char per known model, so the leftovers line up
    // one-to-one with the shuffled no-history models.
    for (&mi, &ci) in fresh.iter().zip(remaining.iter()) {
        assign[mi] = Some(ci);
    }

    assign
        .into_iter()
        .map(|a| a.expect("every model matched a character"))
        .collect()
}

/// Reassign a fixed bag of characters to the given player seats so each model's
/// record stays balanced (team → role type → role, lexicographically). The models are
/// PINNED to their seats and the roles move — this is what `r` (reroll roles) does.
/// The bag (`true_character`, `believed_character` pairs — the latter carries a Drunk's
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
    debug_assert_eq!(seats.len(), bag.len());
    debug_assert_eq!(seats.len(), seat_models.len());
    let chars: Vec<Character> = bag.iter().map(|(c, _)| *c).collect();
    // Models are indexed by seat, so pick[i] is the char for seat i.
    let pick = match_models_to_chars(seat_models, &chars, stats, rng);
    (0..seats.len())
        .map(|i| {
            let (true_character, believed_character) = bag[pick[i]];
            RoleAssignment {
                seat: seats[i],
                true_character,
                believed_character,
            }
        })
        .collect()
}

/// Indices (into `candidates`) already placed on the team **opposing** `seat`'s — the
/// seats this one will form BT edges with.
fn opposite_picks(seat_chars: &[Character], out: &[usize], seat: usize) -> Vec<usize> {
    let mine = seat_chars[seat].team();
    (0..seat_chars.len())
        .filter(|&j| out[j] != usize::MAX && seat_chars[j].team() != mine)
        .map(|j| out[j])
        .collect()
}

/// How well `ci` is **already** known against the current opposition — the axis we
/// minimise, so the pick is the match-up the leaderboard has seen least.
///
/// A model facing *itself* across the divide is charged [`SAME_MODEL_PENALTY`] rather
/// than the `0` a never-observed pair scores: the rater skips same-model pairs, so
/// those seats produce no edge. Without this, "never seen" and "cannot ever be seen"
/// would tie at zero and a single model could sweep the table.
fn pair_cost(ci: usize, opposite: &[usize], candidates: &[&str], pairs: &PairStats) -> u32 {
    opposite.iter().fold(0u32, |acc, &oi| {
        let add = if oi == ci {
            SAME_MODEL_PENALTY
        } else {
            pair_n(pairs, candidates[ci], candidates[oi])
        };
        acc.saturating_add(add)
    })
}

/// Fold a pick into the running record so later seats see the updated coverage.
fn commit_pick(
    live: &mut [ModelStats],
    pairs: &mut PairStats,
    candidates: &[&str],
    ci: usize,
    opposite: &[usize],
    (team, rtype, role): (String, String, String),
) {
    let st = &mut live[ci];
    st.games += 1;
    *st.team.entry(team).or_insert(0) += 1;
    *st.role_type.entry(rtype).or_insert(0) += 1;
    *st.role.entry(role).or_insert(0) += 1;
    for &oi in opposite {
        if oi != ci {
            *pairs
                .entry(pair_key(candidates[ci], candidates[oi]))
                .or_insert(0) += 1;
        }
    }
}

/// Hard floor: a table must field **at least two models**, because a monolithic table
/// produces zero BT edges and so contributes nothing to the leaderboard.
///
/// [`pair_cost`] already charges [`SAME_MODEL_PENALTY`] per Good seat for reusing the
/// Evil model, so with ≥2 candidates and both sides present the greedy never builds
/// one. This is the belt-and-braces guarantee for shapes that reasoning cannot cover
/// (a bag with no Evil seat at all), and is necessarily a no-op when only one model is
/// eligible — there is then no second model to field.
fn enforce_two_models(
    out: &mut [usize],
    seat_chars: &[Character],
    candidates: &[&str],
    live: &[ModelStats],
    rng: &mut impl Rng,
) {
    if candidates.len() < 2 || out.is_empty() {
        return;
    }
    let incumbent = out[0];
    if out.iter().any(|&c| c != incumbent) {
        return;
    }
    // Flip a seat on the *smaller* side: with 5 Good / 2 Evil, moving one Evil seat to
    // a second model creates 5 edges where moving one Good seat would create only 2.
    let good: Vec<usize> = (0..out.len())
        .filter(|&i| seat_chars[i].team() == Team::Good)
        .collect();
    let evil: Vec<usize> = (0..out.len())
        .filter(|&i| seat_chars[i].team() != Team::Good)
        .collect();
    let side = match (good.is_empty(), evil.is_empty()) {
        (false, false) if evil.len() <= good.len() => &evil,
        (false, false) => &good,
        (true, false) => &evil,
        (false, true) => &good,
        (true, true) => return,
    };
    let si = *side.choose(rng).expect("side is non-empty");
    let (team, rtype, role) = keys(seat_chars[si]);
    let mut alts: Vec<usize> = (0..candidates.len()).filter(|&c| c != incumbent).collect();
    alts.shuffle(rng);
    // `min_by_key` keeps the first minimum, so the shuffle is the tie-break.
    let best = alts
        .into_iter()
        .min_by_key(|&ci| {
            let st = &live[ci];
            (st.team_n(&team), st.type_n(&rtype), st.role_n(&role))
        })
        .expect("candidates.len() >= 2");
    out[si] = best;
}

/// **Choose which models play**, given the drawn roles — the counterpart to
/// [`balanced_assignment`]. This does NOT permute a hand-picked set: it *selects* from
/// `candidates` (the models offered in the pickers that already have a completed game)
/// so the corpus improves. Returns `out[seat_i]` = index into `candidates`.
///
/// There are usually fewer eligible models than seats, so a candidate may take several
/// seats and some may go unused — repeats **within** a side are free, since only
/// cross-team pairs produce BT edges.
///
/// Greedy, cost `(pair deficit, team, role type, role)`:
/// * **Evil seats first.** They are the scarce side, and pinning them gives every Good
///   seat a concrete opposition to measure against. While they are placed there is no
///   opposition yet, so `pair_cost` is 0 for all and the tuple degrades to the #70
///   count axes — exactly what we want for "who is owed Evil".
/// * **Good seats** then go to whoever has faced that Evil side least, which both
///   forbids the monolithic table (via [`SAME_MODEL_PENALTY`]) and spreads the seats:
///   each pick raises its own pair weight, so the next seat prefers someone else.
///
/// Over successive games the team axis rotates who draws Evil while the pair axis picks
/// their opposition, so the two together sweep the pair graph rather than deepening the
/// match-ups it already knows. `candidates` must be non-empty.
pub fn select_balanced_models(
    seat_chars: &[Character],
    candidates: &[&str],
    hist: &History,
    rng: &mut impl Rng,
) -> Vec<usize> {
    assert!(!candidates.is_empty(), "need at least one eligible model");
    let n = seat_chars.len();
    // Running record = real history + what this game has already handed out.
    let mut live: Vec<ModelStats> = candidates
        .iter()
        .map(|k| hist.models.get(*k).cloned().unwrap_or_default())
        .collect();
    let mut live_pairs = hist.pairs.clone();
    let mut out = vec![usize::MAX; n];

    // Shuffle, then *stable* sort Evil-first so order within a side stays random.
    let mut order: Vec<usize> = (0..n).collect();
    order.shuffle(rng);
    order.sort_by_key(|&i| seat_chars[i].team() == Team::Good);

    for &si in &order {
        let (team, rtype, role) = keys(seat_chars[si]);
        let opposite = opposite_picks(seat_chars, &out, si);
        // Random candidate order so equal-cost ties resolve arbitrarily.
        let mut cand_order: Vec<usize> = (0..candidates.len()).collect();
        cand_order.shuffle(rng);
        let mut best: Option<(usize, (u32, u32, u32, u32))> = None;
        for ci in cand_order {
            let st = &live[ci];
            let cost = (
                pair_cost(ci, &opposite, candidates, &live_pairs),
                st.team_n(&team),
                st.type_n(&rtype),
                st.role_n(&role),
            );
            if best.is_none_or(|(_, bc)| cost < bc) {
                best = Some((ci, cost));
            }
        }
        let (ci, _) = best.expect("candidates is non-empty");
        out[si] = ci;
        commit_pick(
            &mut live,
            &mut live_pairs,
            candidates,
            ci,
            &opposite,
            (team, rtype, role),
        );
    }

    enforce_two_models(&mut out, seat_chars, candidates, &live, rng);
    out
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

    /// History with per-model records and, optionally, cross-team pair coverage
    /// (`(a, b, seat_pairs)` — order-independent).
    fn hist_with(rows: Vec<(&str, ModelStats)>, pairs: &[(&str, &str, u32)]) -> History {
        History {
            models: stats_with(rows),
            pairs: pairs.iter().map(|(a, b, w)| (pair_key(a, b), *w)).collect(),
        }
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
    fn select_picks_owed_evil_model_for_the_evil_seats() {
        // Roles pinned: seats 0-2 Good/Townsfolk, seat 3 Minion, seat 4 Demon.
        let seat_chars = vec![
            Character::Empath,
            Character::FortuneTeller,
            Character::Chef,
            Character::Poisoner,
            Character::Imp,
        ];
        // Only TWO eligible candidates, so they must repeat across the five seats:
        // A has never been Evil (owed Evil); B has only ever been Evil (owed Good).
        let hist = hist_with(
            vec![
                ("A", model(&[("Good", "Townsfolk", "Empath", 8)])),
                ("B", model(&[("Evil", "Minion", "Poisoner", 8)])),
            ],
            &[],
        );
        let pool = ["A", "B"];
        let mut rng = StdRng::seed_from_u64(21);
        let pick = select_balanced_models(&seat_chars, &pool, &hist, &mut rng);
        let evil: Vec<&str> = [3usize, 4].iter().map(|&s| pool[pick[s]]).collect();
        assert!(
            evil.iter().all(|m| *m == "A"),
            "the never-Evil model must take the Evil seats, got {evil:?}"
        );
        // B (owed Good) should be picked up for Good seats rather than left unused.
        let good: Vec<&str> = [0usize, 1, 2].iter().map(|&s| pool[pick[s]]).collect();
        assert!(
            good.contains(&"B"),
            "the owed-Good model should take Good seats, got {good:?}"
        );
    }

    #[test]
    fn select_spreads_seats_instead_of_giving_one_model_everything() {
        // All-Townsfolk seats + two identical-history candidates: counting each
        // assignment as we go must spread them rather than hand every seat to one.
        let seat_chars = vec![
            Character::Empath,
            Character::FortuneTeller,
            Character::Chef,
            Character::Washerwoman,
        ];
        let hist = hist_with(
            vec![
                ("A", model(&[("Good", "Townsfolk", "Empath", 1)])),
                ("B", model(&[("Good", "Townsfolk", "Empath", 1)])),
            ],
            &[],
        );
        let pool = ["A", "B"];
        let mut rng = StdRng::seed_from_u64(3);
        let pick = select_balanced_models(&seat_chars, &pool, &hist, &mut rng);
        let used: std::collections::BTreeSet<usize> = pick.iter().copied().collect();
        assert_eq!(
            used.len(),
            2,
            "both candidates used, not one hogging all seats"
        );
    }

    /// Regression: the pure-count cost let one model take **every** seat.
    ///
    /// A model with far less history is the cheapest pick on every count axis, and
    /// folding a single seat in per pick never closes a gap that large — so all seven
    /// went to it. The rater skips same-model pairs, so that table produced *zero* BT
    /// edges: the model with the least data was seated in a game that could teach the
    /// leaderboard nothing about it.
    #[test]
    fn select_never_fields_one_model_when_a_second_is_eligible() {
        let seat_chars = vec![
            Character::Washerwoman,
            Character::Librarian,
            Character::Chef,
            Character::Empath,
            Character::Monk,
            Character::Poisoner, // Evil
            Character::Imp,      // Evil
        ];
        let hist = hist_with(
            vec![
                ("fresh", model(&[("Good", "Townsfolk", "Empath", 1)])),
                (
                    "veteran",
                    model(&[
                        ("Good", "Townsfolk", "Empath", 40),
                        ("Evil", "Demon", "Imp", 40),
                    ]),
                ),
            ],
            &[],
        );
        let pool = ["fresh", "veteran"];
        for seed in 0..64u64 {
            let mut rng = StdRng::seed_from_u64(seed);
            let pick = select_balanced_models(&seat_chars, &pool, &hist, &mut rng);
            let used: std::collections::BTreeSet<usize> = pick.iter().copied().collect();
            assert!(
                used.len() >= 2,
                "seed {seed}: a one-model table yields zero BT edges, got {pick:?}"
            );
        }
    }

    /// Match-up coverage is the **primary** axis: with the count axes tied, the Good
    /// seats must go to whoever has faced the Evil side least.
    #[test]
    fn select_targets_the_least_observed_matchup() {
        let seat_chars = vec![Character::Empath, Character::Chef, Character::Imp];
        // `owed_evil` has never been Evil, so it takes the Demon seat. `stranger` and
        // `rival` have *identical* records — only pair coverage can separate them, and
        // `rival` is already heavily observed against `owed_evil`.
        let even = || {
            model(&[
                ("Good", "Townsfolk", "Empath", 5),
                ("Evil", "Demon", "Imp", 9),
            ])
        };
        let hist = hist_with(
            vec![
                ("owed_evil", model(&[("Good", "Townsfolk", "Empath", 9)])),
                ("stranger", even()),
                ("rival", even()),
            ],
            &[("owed_evil", "rival", 30)],
        );
        let pool = ["owed_evil", "stranger", "rival"];
        for seed in 0..32u64 {
            let mut rng = StdRng::seed_from_u64(seed);
            let pick = select_balanced_models(&seat_chars, &pool, &hist, &mut rng);
            assert_eq!(
                pool[pick[2]], "owed_evil",
                "seed {seed}: never-Evil takes Demon"
            );
            let good: Vec<&str> = [0usize, 1].iter().map(|&s| pool[pick[s]]).collect();
            assert!(
                good.iter().all(|m| *m == "stranger"),
                "seed {seed}: Good seats must target the unobserved match-up, got {good:?}"
            );
        }
    }

    /// A model on **both** sides of the table produces no edge for those seats, so it
    /// must lose to any real alternative even when its counts are the most owed.
    #[test]
    fn select_will_not_put_the_evil_model_on_good_seats() {
        let seat_chars = vec![Character::Empath, Character::Chef, Character::Imp];
        // `hog` is owed on every axis, so pure counts would hand it all three seats.
        let hist = hist_with(
            vec![
                ("hog", model(&[("Good", "Townsfolk", "Empath", 0)])),
                (
                    "other",
                    model(&[
                        ("Good", "Townsfolk", "Empath", 50),
                        ("Evil", "Demon", "Imp", 50),
                    ]),
                ),
            ],
            &[],
        );
        let pool = ["hog", "other"];
        for seed in 0..32u64 {
            let mut rng = StdRng::seed_from_u64(seed);
            let pick = select_balanced_models(&seat_chars, &pool, &hist, &mut rng);
            let evil = pool[pick[2]];
            let good: Vec<&str> = [0usize, 1].iter().map(|&s| pool[pick[s]]).collect();
            assert!(
                good.iter().all(|m| *m != evil),
                "seed {seed}: {evil} on both sides yields no edge for those seats, got \
                 good={good:?} evil={evil}"
            );
        }
    }

    /// The ≥2 floor still holds for a shape the pair axis cannot reason about: a bag
    /// with no Evil seat at all leaves `pair_cost` at 0 for everyone.
    #[test]
    fn min_two_models_holds_even_with_no_evil_seat() {
        let seat_chars = vec![Character::Empath, Character::Chef];
        // Identical, maximally-lopsided records so counts alone would pick one twice.
        let hist = hist_with(
            vec![
                ("a", model(&[("Good", "Townsfolk", "Empath", 5)])),
                ("b", model(&[("Good", "Townsfolk", "Empath", 5)])),
            ],
            &[],
        );
        let pool = ["a", "b"];
        for seed in 0..32u64 {
            let mut rng = StdRng::seed_from_u64(seed);
            let pick = select_balanced_models(&seat_chars, &pool, &hist, &mut rng);
            let used: std::collections::BTreeSet<usize> = pick.iter().copied().collect();
            assert_eq!(used.len(), 2, "seed {seed}: got {pick:?}");
        }
    }

    /// One eligible model is the one case the floor cannot satisfy — it must still
    /// resolve rather than panic or loop (the TUI warns instead).
    #[test]
    fn select_with_a_single_candidate_still_resolves() {
        let seat_chars = vec![Character::Empath, Character::Imp];
        let hist = hist_with(
            vec![("solo", model(&[("Good", "Townsfolk", "Empath", 1)]))],
            &[],
        );
        let mut rng = StdRng::seed_from_u64(7);
        let pick = select_balanced_models(&seat_chars, &["solo"], &hist, &mut rng);
        assert_eq!(pick, vec![0, 0]);
    }

    #[test]
    fn read_history_counts_only_cross_team_pairs() {
        // One game: A and B are Good, C is Evil. Only the cross-team pairs {A,C} and
        // {B,C} can ever become BT edges — {A,B} share an outcome, so the rater never
        // compares them. A second game seats the SAME model on both sides: the rater
        // skips same-model pairs, so it must contribute nothing.
        let path = std::env::temp_dir().join("botc_balance_read_history_pairs_test.jsonl");
        let seat = |m: &str, team: &str, ct: &str, role: &str| {
            format!(
                r#"{{"model":"{m}","team":"{team}","character_type":"{ct}","true_character":"{role}"}}"#
            )
        };
        let g1 = format!(
            r#"{{"event":"game_end","seats":[{},{},{}]}}"#,
            seat("A", "Good", "Townsfolk", "Empath"),
            seat("B", "Good", "Townsfolk", "Chef"),
            seat("C", "Evil", "Demon", "Imp"),
        );
        let g2 = format!(
            r#"{{"event":"game_end","seats":[{},{}]}}"#,
            seat("D", "Good", "Townsfolk", "Empath"),
            seat("D", "Evil", "Demon", "Imp"),
        );
        std::fs::write(&path, [g1, g2].join("\n")).unwrap();
        let hist = read_history(&path);
        let _ = std::fs::remove_file(&path);

        assert_eq!(pair_n(&hist.pairs, "A", "C"), 1, "Good×Evil pair counted");
        assert_eq!(pair_n(&hist.pairs, "B", "C"), 1, "order-independent lookup");
        assert_eq!(pair_n(&hist.pairs, "C", "B"), 1, "key is unordered");
        assert_eq!(
            pair_n(&hist.pairs, "A", "B"),
            0,
            "same-team seats never form a BT edge"
        );
        assert_eq!(
            pair_n(&hist.pairs, "D", "D"),
            0,
            "same-model cross-team pairs carry no information"
        );
        // The per-model record is unaffected by the pair tally.
        assert_eq!(hist.models["D"].games, 2);
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
