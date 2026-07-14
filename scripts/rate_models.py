#!/usr/bin/env python3
"""Rate models from botc-results.jsonl (finished games only).

Multiplayer team outcomes → pairwise Bradley–Terry:
  each winning-seat model "beats" each losing-seat model (same-model pairs skipped).
Batch MAP fit with Gaussian prior around 1500 Elo. Writes ratings.json for the
publisher (or prints JSON with --stdout).

Stdlib only so it runs without the training / Rust toolchain.
"""

from __future__ import annotations

import argparse
import json
import math
import os
import sys
from collections import defaultdict
from datetime import datetime, timezone
from pathlib import Path

SCALE = 400.0 / math.log(10.0)  # Elo per natural logistic unit
INIT = 1500.0
LEVEL_SIGMA = 350.0  # soft prior σ (Elo) around INIT


def load_game_ends(path: Path) -> list[dict]:
    ends = []
    try:
        text = path.read_text()
    except FileNotFoundError:
        return []
    for i, line in enumerate(text.splitlines(), 1):
        line = line.strip()
        if not line:
            continue
        try:
            rec = json.loads(line)
        except json.JSONDecodeError as exc:
            # Tolerate a torn final line (crash mid-append).
            if i == text.count("\n") + (0 if text.endswith("\n") else 1):
                break
            raise SystemExit(f"bad JSONL at {path}:{i}: {exc}") from exc
        if rec.get("event") == "game_end" and rec.get("seats"):
            ends.append(rec)
    return ends


# Display order for the public role/side matrix (not used in ranking).
SIDE_COLS = ("Good", "Evil")
ROLE_COLS = ("Townsfolk", "Outsider", "Minion", "Demon")
MATRIX_COLS = SIDE_COLS + ROLE_COLS


def expand_pairwise(
    ends: list[dict],
) -> tuple[
    list[tuple[str, str, float]],
    dict[str, int],
    dict[str, list[int]],
    dict[str, dict[str, int]],
]:
    """Return (pairs, games_played, seat_wins/losses, role_side_counts).

    weight is the number of seat-pairs (integer, stored as float for BT).
    role_side_counts is display-only: per-model seat counts by team and
    character_type (does not feed the BT fit).
    """
    pair_w = defaultdict(float)  # (w, l) -> count
    games = defaultdict(int)
    seat_wl = defaultdict(lambda: [0, 0])  # model -> [seat_wins, seat_losses]
    matrix = defaultdict(lambda: {c: 0 for c in MATRIX_COLS})

    for g in ends:
        winners, losers = [], []
        seen = set()
        for s in g["seats"]:
            m = (s.get("model") or "").strip()
            if not m:
                continue
            if m not in seen:
                games[m] += 1
                seen.add(m)
            if s.get("won") is True:
                winners.append(m)
                seat_wl[m][0] += 1
            elif s.get("won") is False:
                losers.append(m)
                seat_wl[m][1] += 1
            # Matrix tallies every seated appearance (independent of win/loss).
            team = (s.get("team") or "").strip()
            if team in matrix[m]:
                matrix[m][team] += 1
            ctype = (s.get("character_type") or "").strip()
            if ctype in matrix[m]:
                matrix[m][ctype] += 1
        for w in winners:
            for l in losers:
                if w == l:
                    continue
                pair_w[(w, l)] += 1.0

    pairs = [(w, l, c) for (w, l), c in pair_w.items() if c > 0]
    return (
        pairs,
        dict(games),
        {k: v for k, v in seat_wl.items()},
        {k: dict(v) for k, v in matrix.items()},
    )


def fit_bt(
    models: list[str],
    pairs: list[tuple[str, str, float]],
    init: float = INIT,
    level_sigma: float = LEVEL_SIGMA,
    max_iter: int = 80,
    tol: float = 1e-9,
) -> tuple[dict[str, float], dict[str, float], bool]:
    """Newton MAP on natural-scale ratings; return (elo, sigma_elo, converged)."""
    if not models:
        return {}, {}, True
    idx = {m: i for i, m in enumerate(models)}
    n = len(models)
    r = [0.0] * n  # natural units relative to INIT
    prior_prec = (SCALE / level_sigma) ** 2

    # Pre-aggregate: for each ordered pair weight of wins of i over j
    W = [[0.0] * n for _ in range(n)]
    for w, l, c in pairs:
        if w not in idx or l not in idx:
            continue
        W[idx[w]][idx[l]] += c

    converged = False
    for _ in range(max_iter):
        g = [-prior_prec * r[i] for i in range(n)]
        H = [[0.0] * n for _ in range(n)]
        for i in range(n):
            H[i][i] = prior_prec

        for i in range(n):
            for j in range(n):
                if i == j:
                    continue
                n_ij = W[i][j] + W[j][i]
                if n_ij <= 0:
                    continue
                # p_ij = P(i beats j) = sigmoid(r_i - r_j)
                diff = r[i] - r[j]
                # stable sigmoid
                if diff >= 0:
                    p = 1.0 / (1.0 + math.exp(-diff))
                else:
                    e = math.exp(diff)
                    p = e / (1.0 + e)
                # gradient of loglik for wins of i over j: W[i][j] * (1 - p)
                # and for wins of j over i contributes when we visit (j,i)
                # Combined: observed wins_i_vs_j - n_ij * p
                obs = W[i][j]
                g[i] += obs - n_ij * p
                # Hessian: -n_ij * p * (1-p) on diag, + same off-diag
                h = n_ij * p * (1.0 - p)
                H[i][i] += h
                H[i][j] -= h

        # Solve H dr = g  (dense Gaussian elimination; n is small)
        try:
            dr = _solve(H, g)
        except ZeroDivisionError:
            break
        step = max(abs(x) for x in dr) if dr else 0.0
        for i in range(n):
            r[i] += dr[i]
        if step < tol:
            converged = True
            break

    # Laplace σ from diag of inv(H) after final assembly
    g = [0.0] * n  # unused
    H = [[0.0] * n for _ in range(n)]
    for i in range(n):
        H[i][i] = prior_prec
    for i in range(n):
        for j in range(n):
            if i == j:
                continue
            n_ij = W[i][j] + W[j][i]
            if n_ij <= 0:
                continue
            diff = r[i] - r[j]
            if diff >= 0:
                p = 1.0 / (1.0 + math.exp(-diff))
            else:
                e = math.exp(diff)
                p = e / (1.0 + e)
            h = n_ij * p * (1.0 - p)
            H[i][i] += h
            H[i][j] -= h

    try:
        inv_diag = _inv_diag(H)
    except ZeroDivisionError:
        inv_diag = [1.0 / prior_prec] * n

    elo = {models[i]: init + SCALE * r[i] for i in range(n)}
    sigma = {models[i]: SCALE * math.sqrt(max(inv_diag[i], 0.0)) for i in range(n)}
    return elo, sigma, converged


def _solve(A: list[list[float]], b: list[float]) -> list[float]:
    """Solve A x = b with Gaussian elimination + partial pivoting."""
    n = len(b)
    M = [row[:] + [b[i]] for i, row in enumerate(A)]
    for col in range(n):
        pivot = max(range(col, n), key=lambda r: abs(M[r][col]))
        if abs(M[pivot][col]) < 1e-18:
            raise ZeroDivisionError("singular")
        M[col], M[pivot] = M[pivot], M[col]
        div = M[col][col]
        for j in range(col, n + 1):
            M[col][j] /= div
        for row in range(n):
            if row == col:
                continue
            f = M[row][col]
            if f == 0:
                continue
            for j in range(col, n + 1):
                M[row][j] -= f * M[col][j]
    return [M[i][n] for i in range(n)]


def _inv_diag(A: list[list[float]]) -> list[float]:
    """Diagonal of A^{-1} via solving A e_k for each k (n small)."""
    n = len(A)
    out = [0.0] * n
    for k in range(n):
        e = [0.0] * n
        e[k] = 1.0
        x = _solve(A, e)
        out[k] = x[k]
    return out


def build_book(ends: list[dict], init: float = INIT) -> dict:
    pairs, games, seat_wl, matrix = expand_pairwise(ends)
    models = sorted(games.keys())
    elo, sigma, ok = fit_bt(models, pairs, init=init)
    ratings = {}
    for m in models:
        sw, sl = seat_wl.get(m, [0, 0])
        denom = sw + sl
        counts = matrix.get(m) or {c: 0 for c in MATRIX_COLS}
        ratings[m] = {
            "rating": round(elo.get(m, init), 1),
            "sigma": round(sigma.get(m, LEVEL_SIGMA), 1),
            "games": int(games.get(m, 0)),
            "seat_wins": sw,
            "seat_losses": sl,
            "win_rate": round(sw / denom, 4) if denom else None,
            # Display-only seat tallies; not used by the BT fit.
            "sides": {c: int(counts.get(c, 0)) for c in SIDE_COLS},
            "roles": {c: int(counts.get(c, 0)) for c in ROLE_COLS},
        }
    return {
        "fitted_at": datetime.now(timezone.utc).isoformat(timespec="seconds"),
        "games_rated": len(ends),
        "pairwise_edges": len(pairs),
        "converged": ok,
        "init": init,
        "matrix_columns": {
            "sides": list(SIDE_COLS),
            "roles": list(ROLE_COLS),
        },
        "ratings": ratings,
    }


def main() -> None:
    ap = argparse.ArgumentParser(description="Rate BOTC models from results JSONL.")
    ap.add_argument(
        "--log",
        default=os.environ.get("BOTC_RESULTS_LOG", "botc-results.jsonl"),
        help="path to botc-results.jsonl",
    )
    ap.add_argument(
        "--out",
        default="leaderboard/ratings.json",
        help="write ratings book here (default leaderboard/ratings.json)",
    )
    ap.add_argument("--stdout", action="store_true", help="print book JSON to stdout")
    ap.add_argument("--init", type=float, default=INIT, help="prior mean Elo")
    args = ap.parse_args()

    ends = load_game_ends(Path(args.log))
    book = build_book(ends, init=args.init)
    text = json.dumps(book, indent=2, sort_keys=True) + "\n"
    if args.stdout:
        sys.stdout.write(text)
    out = Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    tmp = out.with_suffix(out.suffix + ".tmp")
    tmp.write_text(text)
    os.replace(tmp, out)
    n = len(book["ratings"])
    print(
        f"rated {book['games_rated']} games → {n} models "
        f"(converged={book['converged']}) → {out}",
        flush=True,
    )


if __name__ == "__main__":
    main()
