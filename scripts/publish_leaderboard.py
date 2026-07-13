#!/usr/bin/env python3
"""Publish BOTC model ratings to the Cloudflare leaderboard Worker.

Reads leaderboard/ratings.json (from scripts/rate_models.py), remaps model ids
through the private aliases file (leaderboard/aliases.json), and POSTs a
compact ranked payload to the Worker's /api/publish. Raw model names never leave
this machine.

Env (or flags): LEADERBOARD_URL, LEADERBOARD_TOKEN.
Publishes when the DISPLAYED content changes (ratings quantized to integers for
the digest), at most once per --min-interval, with an hourly force-publish so
updated_at stays an honest liveness signal.
"""

from __future__ import annotations

import argparse
import hashlib
import http.client
import json
import math
import os
import subprocess
import sys
import time
import urllib.request
from datetime import datetime, timezone
from pathlib import Path

_REPO_ROOT = Path(__file__).resolve().parent.parent
_SCRIPTS = Path(__file__).resolve().parent
if str(_SCRIPTS) not in sys.path:
    sys.path.insert(0, str(_SCRIPTS))

_USER_AGENT = "botc-leaderboard-publisher/1.0"
DEFAULT_RATINGS = _REPO_ROOT / "leaderboard" / "ratings.json"
DEFAULT_LOG = _REPO_ROOT / "botc-results.jsonl"
DEFAULT_ALIASES = _REPO_ROOT / "leaderboard" / "aliases.json"


def _load_dotenv(path: Path) -> None:
    try:
        lines = path.read_text().splitlines()
    except FileNotFoundError:
        return
    for line in lines:
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, _, val = line.partition("=")
        key = key.removeprefix("export ").strip()
        os.environ.setdefault(key, val.strip().strip("\"'"))


def build_payload(
    ratings_path: Path,
    aliases_path: Path | None = None,
    *,
    reveal: bool = False,
) -> dict | None:
    """Build the public ranked payload.

    By default, entry ids are remapped through the private aliases file so raw
    model names never leave this machine. Pass reveal=True only for local dumps.
    """
    try:
        book = json.loads(ratings_path.read_text())
    except (FileNotFoundError, json.JSONDecodeError):
        return None
    ratings = book.get("ratings") or {}

    # Late import so `python scripts/publish_leaderboard.py` works from any cwd.
    from model_aliases import ensure_aliases  # type: ignore

    real_ids = list(ratings.keys())
    mapping = {} if reveal else ensure_aliases(real_ids, aliases_path)

    entries = []
    for pid, rec in ratings.items():
        pub = pid if reveal else mapping.get(pid, pid)
        sides = rec.get("sides") or {}
        roles = rec.get("roles") or {}
        entries.append(
            {
                "id": pub,
                "rating": round(float(rec["rating"]), 1),
                "sigma": round(float(rec.get("sigma", 0)), 1),
                "games": int(rec.get("games", 0)),
                "win_rate": rec.get("win_rate"),
                "present": True,
                # Display-only seat tallies (aliased id); not used for ranking.
                "sides": {k: int(sides.get(k, 0) or 0) for k in ("Good", "Evil")},
                "roles": {
                    k: int(roles.get(k, 0) or 0)
                    for k in ("Townsfolk", "Outsider", "Minion", "Demon")
                },
            }
        )
    entries.sort(key=lambda e: (-e["rating"], e["id"]))
    return {
        "updated_at": datetime.now(timezone.utc).isoformat(timespec="seconds"),
        "games_rated": book.get("games_rated"),
        "converged": book.get("converged"),
        "entries": entries,
        "matrix_columns": book.get("matrix_columns")
        or {
            "sides": ["Good", "Evil"],
            "roles": ["Townsfolk", "Outsider", "Minion", "Demon"],
        },
        # Never include real model ids on the public payload.
        "anonymized": not reveal,
    }


def _half_up(x: float) -> int:
    return int(math.floor(x + 0.5))


def _digest(payload: dict) -> str:
    body = [
        (
            e["id"],
            _half_up(e["rating"]),
            _half_up(e["sigma"]),
            e["games"] // 5,
            e.get("win_rate"),
            # Include seat matrix so role/side redistributions republish.
            e.get("sides"),
            e.get("roles"),
        )
        for e in payload["entries"]
    ]
    body.append(payload.get("games_rated"))
    return hashlib.sha256(json.dumps(body, sort_keys=True).encode()).hexdigest()


def publish(payload: dict, url: str, token: str) -> int:
    req = urllib.request.Request(
        url.rstrip("/") + "/api/publish",
        data=json.dumps(payload).encode(),
        headers={
            "Authorization": f"Bearer {token}",
            "Content-Type": "application/json",
            "User-Agent": _USER_AGENT,
        },
        method="POST",
    )
    with urllib.request.urlopen(req, timeout=15) as resp:
        return resp.status


def recompute(log_path: Path, ratings_path: Path) -> None:
    cmd = [
        sys.executable,
        str(_REPO_ROOT / "scripts" / "rate_models.py"),
        "--log",
        str(log_path),
        "--out",
        str(ratings_path),
    ]
    subprocess.check_call(cmd)


def main() -> None:
    ap = argparse.ArgumentParser(description="Publish BOTC ratings to the Worker.")
    ap.add_argument("--ratings", type=Path, default=DEFAULT_RATINGS)
    ap.add_argument("--log", type=Path, default=Path(os.environ.get("BOTC_RESULTS_LOG", DEFAULT_LOG)))
    ap.add_argument(
        "--aliases",
        type=Path,
        default=Path(os.environ.get("BOTC_ALIASES", DEFAULT_ALIASES)),
        help="private real→public name map (default leaderboard/aliases.json)",
    )
    ap.add_argument(
        "--reveal",
        action="store_true",
        help="LOCAL ONLY: put real model names in the payload (never use for public publish)",
    )
    ap.add_argument("--url", default=None, help="Worker base URL (or LEADERBOARD_URL)")
    ap.add_argument("--token", default=None, help="publish secret (or LEADERBOARD_TOKEN)")
    ap.add_argument(
        "--env-file",
        type=Path,
        default=_REPO_ROOT / ".env",
        help="dotenv with LEADERBOARD_URL / LEADERBOARD_TOKEN",
    )
    ap.add_argument("--once", action="store_true", help="publish once and exit")
    ap.add_argument("--no-rate", action="store_true", help="skip re-rating; publish existing ratings.json")
    ap.add_argument("--interval", type=int, default=120, help="loop poll seconds")
    ap.add_argument("--min-interval", type=int, default=300, help="min seconds between KV writes")
    ap.add_argument("--force-interval", type=int, default=3600, help="liveness republish interval")
    args = ap.parse_args()

    _load_dotenv(args.env_file)
    url = args.url or os.environ.get("LEADERBOARD_URL")
    token = args.token or os.environ.get("LEADERBOARD_TOKEN")
    if not url or not token:
        ap.error(
            "set LEADERBOARD_URL and LEADERBOARD_TOKEN (in .env, the environment, "
            "or via --url/--token)."
        )

    last_digest = None
    last_write = 0.0
    while True:
        published = False
        if not args.no_rate:
            try:
                recompute(args.log, args.ratings)
            except subprocess.CalledProcessError as exc:
                print(f"rate_models failed ({exc}); skipping cycle", flush=True)
                if args.once:
                    raise SystemExit(1)
                time.sleep(args.interval)
                continue

        if args.reveal:
            print("WARNING: --reveal publishes real model names", flush=True)
        payload = build_payload(args.ratings, args.aliases, reveal=args.reveal)
        if payload is None:
            print(f"no readable ratings at {args.ratings}, skipping", flush=True)
        else:
            digest = _digest(payload)
            now = time.monotonic()
            stale = last_write and now - last_write >= args.force_interval
            changed = digest != last_digest
            if (changed or stale or args.once) and (
                now - last_write >= args.min_interval or args.once or not last_write
            ):
                try:
                    status = publish(payload, url, token)
                except (OSError, http.client.HTTPException) as exc:
                    retry = "" if args.once else "; retrying next cycle"
                    print(f"publish failed ({exc}){retry}", flush=True)
                else:
                    published = True
                    last_digest, last_write = digest, now
                    n = len(payload["entries"])
                    print(
                        f"published {n} models / {payload.get('games_rated')} games "
                        f"-> {status} at {payload['updated_at']}",
                        flush=True,
                    )

        if args.once:
            raise SystemExit(0 if published else 1)
        time.sleep(args.interval)


if __name__ == "__main__":
    main()
