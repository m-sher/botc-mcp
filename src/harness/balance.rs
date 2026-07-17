//! History-balanced role assignment driven by **rating uncertainty**.
//!
//! The bag composition for a player count is fixed (see [`crate::game::setup`]);
//! what we choose is **which model plays which character**.
//!
//! # What the leaderboard actually consumes
//!
//! `scripts/rate_models.py` fits Bradley–Terry on **one team-vs-team contest per
//! game**: a team's strength is the mean rating of its seats, and the fit's precision
//! is its Fisher information `M = τ·I + Σ_games z zᵀ`, where `z[m]` is model `m`'s
//! (Good-share − Evil-share) that game. The reported ± is the diagonal of the
//! posterior covariance `Σ = M⁻¹`. Two consequences drive this module:
//!
//! * A table where every seat is the same model has `z = 0` — the game shrinks no
//!   variance and teaches the ratings nothing at all.
//! * A model can have a perfectly even record and still be barely rateable, because a
//!   per-model count says nothing about *whom it played against*. What pins a rating is
//!   the connectivity of the contest graph, i.e. the off-diagonals of `Σ`.
//!
//! So the primary axis is **uncertainty**: field the table whose team-contrast probes
//! the direction of greatest remaining variance, i.e. give a seat to the model least
//! **covariant** with the opposition already placed — the least-resolved match-up,
//! counting indirect connectivity a raw pair-count cannot see. The per-model count
//! axes — **team** (Good / Evil), then **role type** (Townsfolk / Outsider / Minion /
//! Demon), then **specific role** (Empath, Poisoner, …) — are kept as lexicographic
//! tie-breaks, so a model still can't drift into always drawing the same team (which
//! would confound its rating with role advantage).
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

/// Cost added for putting a model on **both** sides of the table. Splitting a model
/// across teams cancels its own contrast, so those seats resolve no variance: ranked
/// far above any real covariance term, but finite, so a one-eligible-model table still
/// resolves instead of having no legal assignment.
const SAME_MODEL_PENALTY: f64 = 1_048_576.0;

/// Prior width (Elo) around the 1500 anchor, and the per-game Fisher weight at the
/// rating-agnostic operating point `p = 0.5` (`p·(1−p)`). Both identical to
/// `scripts/rate_models.py` so the balancer's information matrix matches the fit's.
const LEVEL_SIGMA_ELO: f64 = 350.0;
const GAME_WEIGHT: f64 = 0.25;

/// Prior precision `τ = (SCALE / LEVEL_SIGMA)²` on the natural-rating scale, where
/// `SCALE = 400 / ln 10` is Elo per natural logistic unit (matches the rater).
fn prior_precision() -> f64 {
    let s = (400.0 / std::f64::consts::LN_10) / LEVEL_SIGMA_ELO;
    s * s
}

/// Everything the balancer reads out of the results corpus.
#[derive(Debug, Default, Clone)]
pub struct History {
    /// Per-model record (team / role type / role counts).
    pub models: HashMap<String, ModelStats>,
    /// One entry per completed game: the team-contrast weights `model → (Good-share −
    /// Evil-share)`, i.e. the `z` vector whose `z zᵀ` the Fisher information sums.
    pub contests: Vec<Vec<(String, f64)>>,
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

/// Compose the ranking/identity node key for a seat. Grok — and legacy
/// rows with no `backend` field — stay **bare**, so grok and legacy rows resolve to
/// the same bare key and no leaderboard node splits. Other backends are namespaced
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

/// Read play [`History`] from a `botc-results.jsonl` file: the per-model record and the
/// per-game team contrasts, from every `game_end` event (one per *completed* game).
/// These are the same games the leaderboard rates — an aborted game (`game_abort`)
/// produces no eval signal, so counting it would balance against games that never
/// mattered; `game_start`/`game_abort` are ignored. A missing file or unreadable lines
/// yield an empty history, and the caller falls back to a random assignment.
///
/// Each contest is the game's `z` vector: `model → (its Good seats / Good total) − (its
/// Evil seats / Evil total)`, matching the rater's mean-team contrast. A game with an
/// empty side, or a single model spanning both, contributes no contrast.
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
        // Per-team seat counts per model, so the contrast is the mean-team difference.
        let mut good: HashMap<String, u32> = HashMap::new();
        let mut evil: HashMap<String, u32> = HashMap::new();
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
                    "Good" => *good.entry(key.clone()).or_insert(0) += 1,
                    "Evil" => *evil.entry(key.clone()).or_insert(0) += 1,
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
        if let Some(contest) = contest_of(&good, &evil) {
            out.contests.push(contest);
        }
    }
    out
}

/// The mean-team contrast `z` for one game from its per-team seat counts, or `None` when
/// a side is empty (no cross-team contrast to learn from). Models whose Good and Evil
/// shares cancel drop out.
fn contest_of(
    good: &HashMap<String, u32>,
    evil: &HashMap<String, u32>,
) -> Option<Vec<(String, f64)>> {
    let (gt, et) = (good.values().sum::<u32>(), evil.values().sum::<u32>());
    if gt == 0 || et == 0 {
        return None;
    }
    let (gt, et) = (gt as f64, et as f64);
    let mut z: HashMap<&str, f64> = HashMap::new();
    for (m, c) in good {
        *z.entry(m).or_insert(0.0) += f64::from(*c) / gt;
    }
    for (m, c) in evil {
        *z.entry(m).or_insert(0.0) -= f64::from(*c) / et;
    }
    let contest: Vec<(String, f64)> = z
        .into_iter()
        .filter(|(_, w)| *w != 0.0)
        .map(|(m, w)| (m.to_string(), w))
        .collect();
    // A single model spanning both sides cancels to nothing — no contrast to learn.
    (!contest.is_empty()).then_some(contest)
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
/// Invert a matrix by Gauss–Jordan with partial pivoting. `None` if singular — not
/// expected here, since the `τ` prior keeps the information matrix full-rank.
#[allow(clippy::needless_range_loop)]
fn invert(mat: &[Vec<f64>]) -> Option<Vec<Vec<f64>>> {
    let n = mat.len();
    // Augment [mat | I], then reduce the left half to identity.
    let mut a: Vec<Vec<f64>> = (0..n)
        .map(|i| {
            let mut row = vec![0.0; 2 * n];
            row[..n].copy_from_slice(&mat[i]);
            row[n + i] = 1.0;
            row
        })
        .collect();
    for col in 0..n {
        let mut piv = col;
        for r in (col + 1)..n {
            if a[r][col].abs() > a[piv][col].abs() {
                piv = r;
            }
        }
        if a[piv][col].abs() < 1e-12 {
            return None;
        }
        a.swap(col, piv);
        let d = a[col][col];
        for j in 0..2 * n {
            a[col][j] /= d;
        }
        for r in 0..n {
            if r == col {
                continue;
            }
            let f = a[r][col];
            if f != 0.0 {
                for j in 0..2 * n {
                    a[r][j] -= f * a[col][j];
                }
            }
        }
    }
    Some(a.into_iter().map(|row| row[n..].to_vec()).collect())
}

/// Posterior covariance `Σ = M⁻¹` restricted to `candidates`, where
/// `M = τ·I + GAME_WEIGHT · Σ_contests z zᵀ` is the team-contest Fisher information over
/// every model in `candidates` and in the history. Off-diagonals carry the indirect
/// connectivity a raw pair-count misses. Falls back to the prior-only covariance if the
/// (always full-rank) inverse cannot be formed.
#[allow(clippy::needless_range_loop)]
fn candidate_covariance(candidates: &[&str], hist: &History) -> Vec<Vec<f64>> {
    let tau = prior_precision();
    // Universe index: candidates first, then any other model seen in a contest.
    let mut idx: HashMap<&str, usize> = HashMap::new();
    for &c in candidates {
        let next = idx.len();
        idx.entry(c).or_insert(next);
    }
    for game in &hist.contests {
        for (m, _) in game {
            let next = idx.len();
            idx.entry(m.as_str()).or_insert(next);
        }
    }
    let n = idx.len();
    let mut m = vec![vec![0.0; n]; n];
    for i in 0..n {
        m[i][i] = tau;
    }
    for game in &hist.contests {
        let terms: Vec<(usize, f64)> = game
            .iter()
            .filter_map(|(name, z)| idx.get(name.as_str()).map(|&i| (i, *z)))
            .collect();
        for &(i, zi) in &terms {
            for &(j, zj) in &terms {
                m[i][j] += GAME_WEIGHT * zi * zj;
            }
        }
    }
    let nc = candidates.len();
    let mut sigma = vec![vec![0.0; nc]; nc];
    match invert(&m) {
        Some(inv) => {
            for a in 0..nc {
                for b in 0..nc {
                    sigma[a][b] = inv[idx[candidates[a]]][idx[candidates[b]]];
                }
            }
        }
        None => {
            for a in 0..nc {
                sigma[a][a] = 1.0 / tau;
            }
        }
    }
    sigma
}

/// Lexicographic order on the selection cost: match-up cost first (with a relative
/// tolerance so float noise falls through), then the team / role-type / role counts.
fn cost_less(a: (f64, u32, u32, u32), b: (f64, u32, u32, u32)) -> bool {
    let eps = 1e-9 * (1.0 + a.0.abs().max(b.0.abs()));
    if (a.0 - b.0).abs() > eps {
        a.0 < b.0
    } else {
        (a.1, a.2, a.3) < (b.1, b.2, b.3)
    }
}

/// Hard floor: a table must field **at least two models**, because a monolithic table
/// has zero team contrast and so contributes nothing to the leaderboard.
///
/// The match-up cost already charges [`SAME_MODEL_PENALTY`] for reusing the Evil model
/// on a Good seat, so with ≥2 candidates and both sides present the greedy never builds
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
    // Flip a seat on the *smaller* side: with 5 Good / 2 Evil, a second model on one
    // Evil seat is measured against all 5 Good seats, where one on a Good seat faces 2.
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
/// seats and some may go unused — repeats **within** a side are free, since only the
/// Good-vs-Evil contrast carries rating signal.
///
/// Greedy, cost `(match-up cost, team, role type, role)`, over the posterior covariance
/// `Σ` of games already played:
/// * **Evil seats first.** They are the scarce side, and pinning them gives every Good
///   seat a concrete opposition to measure against. While they are placed there is no
///   opposition yet, so the match-up cost is 0 for all and the tuple degrades to the
///   count axes — exactly what we want for "who is owed Evil".
/// * **Good seats** then go to whoever is least **covariant** with the placed Evil side
///   — the least-resolved match-up. A model already seated opposite itself is charged
///   [`SAME_MODEL_PENALTY`] (its split contrast resolves nothing), which forbids the
///   monolithic table; the count axes then spread the remaining seats.
///
/// Over successive games the team axis rotates who draws Evil while the covariance axis
/// picks their opposition, so the two together drive down the largest rating variances
/// rather than re-measuring match-ups already pinned. `candidates` must be non-empty.
#[allow(clippy::needless_range_loop)]
pub fn select_balanced_models(
    seat_chars: &[Character],
    candidates: &[&str],
    hist: &History,
    rng: &mut impl Rng,
) -> Vec<usize> {
    assert!(!candidates.is_empty(), "need at least one eligible model");
    let n = seat_chars.len();
    // Running per-model record = real history + what this game has already handed out.
    let mut live: Vec<ModelStats> = candidates
        .iter()
        .map(|k| hist.models.get(*k).cloned().unwrap_or_default())
        .collect();
    // Covariance from games already played; this game is not folded into it.
    let sigma = candidate_covariance(candidates, hist);
    let mut out = vec![usize::MAX; n];

    let good_total = seat_chars.iter().filter(|c| c.team() == Team::Good).count() as f64;
    let evil_total = n as f64 - good_total;

    // Shuffle, then *stable* sort Evil-first so order within a side stays random.
    let mut order: Vec<usize> = (0..n).collect();
    order.shuffle(rng);
    order.sort_by_key(|&i| seat_chars[i].team() == Team::Good);

    for &si in &order {
        let (team, rtype, role) = keys(seat_chars[si]);
        let my_team = seat_chars[si].team();
        let opp_total = if my_team == Team::Good {
            evil_total
        } else {
            good_total
        };
        // Opposition seat-share per candidate: each placed seat on the other team adds
        // 1 / (its team's seat total). u stays zero while the scarce side is still being
        // filled, so those seats fall through to the count axes (owed-team balance).
        let mut u = vec![0.0f64; candidates.len()];
        if opp_total > 0.0 {
            for j in 0..n {
                if out[j] != usize::MAX && seat_chars[j].team() != my_team {
                    u[out[j]] += 1.0 / opp_total;
                }
            }
        }
        // Random candidate order so equal-cost ties resolve arbitrarily.
        let mut cand_order: Vec<usize> = (0..candidates.len()).collect();
        cand_order.shuffle(rng);
        let mut best: Option<(usize, (f64, u32, u32, u32))> = None;
        for ci in cand_order {
            // Match-up cost = covariance-weighted overlap with the opposition (lower =
            // less-resolved match-up); a candidate already opposite itself resolves
            // nothing and is ranked out via SAME_MODEL_PENALTY.
            let mut info = 0.0;
            for m in 0..candidates.len() {
                if u[m] != 0.0 {
                    info += sigma[ci][m] * u[m];
                }
            }
            if u[ci] != 0.0 {
                info += SAME_MODEL_PENALTY;
            }
            let st = &live[ci];
            let cost = (info, st.team_n(&team), st.type_n(&rtype), st.role_n(&role));
            if best.is_none_or(|(_, bc)| cost_less(cost, bc)) {
                best = Some((ci, cost));
            }
        }
        let (ci, _) = best.expect("candidates is non-empty");
        out[si] = ci;
        let st = &mut live[ci];
        st.games += 1;
        *st.team.entry(team).or_insert(0) += 1;
        *st.role_type.entry(rtype).or_insert(0) += 1;
        *st.role.entry(role).or_insert(0) += 1;
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

    /// History with per-model records and, optionally, a set of games given as
    /// `(model, team)` seat lists — each becomes one mean-team contest, exactly as
    /// [`read_history`] would build it.
    fn hist_with(rows: Vec<(&str, ModelStats)>, games: &[&[(&str, &str)]]) -> History {
        let mut contests = Vec::new();
        for game in games {
            let mut good: HashMap<String, u32> = HashMap::new();
            let mut evil: HashMap<String, u32> = HashMap::new();
            for (m, team) in *game {
                match *team {
                    "Good" => *good.entry((*m).to_string()).or_insert(0) += 1,
                    "Evil" => *evil.entry((*m).to_string()).or_insert(0) += 1,
                    _ => {}
                }
            }
            if let Some(c) = contest_of(&good, &evil) {
                contests.push(c);
            }
        }
        History {
            models: stats_with(rows),
            contests,
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

    /// A single model must never field **every** seat.
    ///
    /// A model with far less history is the cheapest pick on every count axis, and
    /// folding a single seat in per pick never closes a gap that large, so a pure-count
    /// cost would hand it all seven. The rater skips same-model pairs, so such a table
    /// produces *zero* BT edges: the model with the least data seated in a game that
    /// could teach the leaderboard nothing about it.
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

    /// Match-up cost is the **primary** axis: with the count axes tied, the Good seats
    /// must go to whoever is least covariant with the placed Evil side.
    #[test]
    fn select_targets_the_least_observed_matchup() {
        let seat_chars = vec![Character::Empath, Character::Chef, Character::Imp];
        // `owed_evil` has never been Evil, so it takes the Demon seat. `stranger` and
        // `rival` have *identical* records — only the covariance separates them, and
        // `rival` is heavily coupled to `owed_evil` by 30 shared contests.
        let even = || {
            model(&[
                ("Good", "Townsfolk", "Empath", 5),
                ("Evil", "Demon", "Imp", 9),
            ])
        };
        let paired: &[(&str, &str)] = &[("owed_evil", "Evil"), ("rival", "Good")];
        let games: Vec<&[(&str, &str)]> = vec![paired; 30];
        let hist = hist_with(
            vec![
                ("owed_evil", model(&[("Good", "Townsfolk", "Empath", 9)])),
                ("stranger", even()),
                ("rival", even()),
            ],
            &games,
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

    /// The ≥2 floor still holds for a shape the match-up axis cannot reason about: a bag
    /// with no Evil seat at all leaves the match-up cost at 0 for everyone.
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
    fn read_history_builds_team_contrasts() {
        // One game: A and B are Good, C is Evil → z is each Good seat's share (½) and
        // −1 for the lone Evil seat. A second game seats the SAME model on both sides:
        // its shares cancel, so it yields no contrast at all.
        let path = std::env::temp_dir().join("botc_balance_read_history_contest_test.jsonl");
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

        // Only the contested game contributes a contrast; the same-model game drops out.
        assert_eq!(hist.contests.len(), 1);
        let z = &hist.contests[0];
        let w = |name: &str| z.iter().find(|(m, _)| m == name).map(|(_, w)| *w);
        assert!((w("A").unwrap() - 0.5).abs() < 1e-9, "A share ½");
        assert!((w("B").unwrap() - 0.5).abs() < 1e-9, "B share ½");
        assert!(
            (w("C").unwrap() + 1.0).abs() < 1e-9,
            "C lone Evil seat = −1"
        );
        // The per-model record still counts both of D's seats.
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
