#!/usr/bin/env python3
"""Rate models from botc-results.jsonl (finished games only).

Multiplayer team outcomes → additive Bradley–Terry:
  each game is one team-vs-team contest whose winner is the positive class, and a
  team's strength is the mean rating of its seats, so
  P(winning team wins) = sigmoid(mean_win_rating − mean_lose_rating).
  A strong teammate raises the team mean, so a weaker seat earns little credit for
  a win the team was already expected to take.
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


def node_key(backend, model) -> str:
    """Compose the ranking node key for a seat (issue #69). Must stay identical to
    `node_key` in src/harness/balance.rs and scripts/model_aliases.py: grok — and
    legacy rows with no `backend` — stay bare; other backends namespace as
    `<backend>:<model>`. Empty model → "" (caller skips)."""
    model = (model or "").strip()
    backend = (backend or "").strip()
    if not model:
        return ""
    if backend in ("", "grok"):
        return model
    return f"{backend}:{model}"


def build_contests(
    ends: list[dict],
) -> tuple[
    list[dict[str, float]],
    dict[str, int],
    dict[str, list[int]],
    dict[str, dict[str, int]],
]:
    """Return (designs, games_played, seat_wins/losses, role_side_counts).

    Each contested game yields one design vector mapping model → (share of
    winning-team seats) − (share of losing-team seats), where share is a model's
    seat count over its team's seat count. The linear predictor Σ weight·rating is
    then mean_win_rating − mean_lose_rating. Games with no cross-team contrast are
    skipped. role_side_counts is display-only: per-model seat counts by team and
    character_type (does not feed the BT fit).
    """
    designs = []  # list of {model: weight} per contested game
    games = defaultdict(int)
    seat_wl = defaultdict(lambda: [0, 0])  # model -> [seat_wins, seat_losses]
    matrix = defaultdict(lambda: {c: 0 for c in MATRIX_COLS})

    for g in ends:
        win_ct = defaultdict(int)  # model -> winning-seat count this game
        los_ct = defaultdict(int)  # model -> losing-seat count this game
        seen = set()
        for s in g["seats"]:
            m = node_key(s.get("backend"), s.get("model"))
            if not m:
                continue
            if m not in seen:
                games[m] += 1
                seen.add(m)
            if s.get("won") is True:
                win_ct[m] += 1
                seat_wl[m][0] += 1
            elif s.get("won") is False:
                los_ct[m] += 1
                seat_wl[m][1] += 1
            # Matrix tallies every seated appearance (independent of win/loss).
            team = (s.get("team") or "").strip()
            if team in matrix[m]:
                matrix[m][team] += 1
            ctype = (s.get("character_type") or "").strip()
            if ctype in matrix[m]:
                matrix[m][ctype] += 1
        nwin, nlose = sum(win_ct.values()), sum(los_ct.values())
        if nwin == 0 or nlose == 0:
            continue
        x = defaultdict(float)
        for m, c in win_ct.items():
            x[m] += c / nwin
        for m, c in los_ct.items():
            x[m] -= c / nlose
        # A model on both teams partially cancels; drop exact zeros.
        designs.append({m: w for m, w in x.items() if w != 0.0})

    return (
        designs,
        dict(games),
        {k: v for k, v in seat_wl.items()},
        {k: dict(v) for k, v in matrix.items()},
    )


def fit_bt(
    models: list[str],
    designs: list[dict[str, float]],
    init: float = INIT,
    level_sigma: float = LEVEL_SIGMA,
    max_iter: int = 200,
    tol: float = 1e-10,
) -> tuple[dict[str, float], dict[str, float], bool]:
    """Newton MAP on natural-scale ratings; return (elo, sigma_elo, converged).

    Each design is one game's {model: team-share weight}; the winning team is the
    positive class, so the log-likelihood is logistic in eta = Σ weight·rating.
    """
    if not models:
        return {}, {}, True
    idx = {m: i for i, m in enumerate(models)}
    n = len(models)
    r = [0.0] * n  # natural units relative to INIT
    prior_prec = (SCALE / level_sigma) ** 2
    # Pre-resolve each game's weights to (index, weight) term lists.
    games = [[(idx[m], w) for m, w in d.items() if m in idx] for d in designs]

    def assemble() -> tuple[list[float], list[list[float]]]:
        # Gradient and negative Hessian of the log-posterior at the current r.
        g = [-prior_prec * r[i] for i in range(n)]
        H = [[0.0] * n for _ in range(n)]
        for i in range(n):
            H[i][i] = prior_prec
        for terms in games:
            eta = sum(w * r[i] for i, w in terms)
            # stable sigmoid; p = P(winning team wins)
            if eta >= 0:
                p = 1.0 / (1.0 + math.exp(-eta))
            else:
                e = math.exp(eta)
                p = e / (1.0 + e)
            resid = 1.0 - p  # observed (win) minus expected
            for i, wi in terms:
                g[i] += resid * wi
            h = p * (1.0 - p)
            for i, wi in terms:
                for j, wj in terms:
                    H[i][j] += h * wi * wj
        return g, H

    converged = False
    for _ in range(max_iter):
        g, H = assemble()
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

    # Laplace σ from diag of inv(H) at the final estimate.
    _, H = assemble()
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
    designs, games, seat_wl, matrix = build_contests(ends)
    models = sorted(games.keys())
    elo, sigma, ok = fit_bt(models, designs, init=init)
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
        "contests": len(designs),
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
