#!/usr/bin/env python3
"""Private real-model → public display-name mapping for the leaderboard.

The aliases file lives only on the rating machine (gitignored). The Cloudflare
payload never includes raw model ids — only the assigned public labels.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys
from pathlib import Path

_REPO_ROOT = Path(__file__).resolve().parent.parent
DEFAULT_PATH = _REPO_ROOT / "leaderboard" / "aliases.json"

# Public labels: Agent-01, Agent-02, … (stable once assigned; never recycled).
_LABEL_RE = re.compile(r"^Agent-(\d+)$")


def default_path() -> Path:
    env = os.environ.get("BOTC_ALIASES")
    return Path(env) if env else DEFAULT_PATH


def load(path: Path | None = None) -> dict:
    path = path or default_path()
    try:
        data = json.loads(path.read_text())
    except FileNotFoundError:
        return {"version": 1, "models": {}, "next_index": 1}
    except json.JSONDecodeError as exc:
        raise SystemExit(f"bad aliases file {path}: {exc}") from exc
    data.setdefault("version", 1)
    data.setdefault("models", {})
    if "next_index" not in data:
        used = []
        for label in data["models"].values():
            m = _LABEL_RE.match(str(label))
            if m:
                used.append(int(m.group(1)))
        data["next_index"] = (max(used) + 1) if used else 1
    return data


def save(data: dict, path: Path | None = None) -> Path:
    path = path or default_path()
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_suffix(path.suffix + ".tmp")
    text = json.dumps(data, indent=2, sort_keys=True) + "\n"
    tmp.write_text(text)
    os.replace(tmp, path)
    return path


def ensure_aliases(real_ids: list[str] | set[str], path: Path | None = None) -> dict[str, str]:
    """Return {real_id: public_label}, assigning new Agent-NN labels as needed."""
    path = path or default_path()
    book = load(path)
    models: dict = book["models"]
    next_i = int(book["next_index"])
    changed = False

    # Stable assignment order for brand-new ids.
    for real in sorted(set(real_ids)):
        if not real:
            continue
        if real in models:
            continue
        label = f"Agent-{next_i:02d}"
        # Avoid colliding with a hand-edited label that already uses this number.
        while label in models.values():
            next_i += 1
            label = f"Agent-{next_i:02d}"
        models[real] = label
        next_i += 1
        changed = True

    if changed or book.get("next_index") != next_i:
        book["models"] = models
        book["next_index"] = next_i
        save(book, path)

    return {r: models[r] for r in real_ids if r in models}


def public_id(real_id: str, mapping: dict[str, str]) -> str:
    return mapping.get(real_id, real_id)


def invert(mapping: dict[str, str]) -> dict[str, str]:
    """public → real (last write wins if duplicate labels)."""
    return {pub: real for real, pub in mapping.items()}


def node_key(backend, model) -> str:
    """Compose the ranking node key for a seat (issue #69). Identical to `node_key` in
    src/harness/balance.rs and scripts/rate_models.py: grok — and legacy rows with no
    `backend` — stay bare; other backends namespace as `<backend>:<model>`."""
    model = (model or "").strip()
    backend = (backend or "").strip()
    if not model:
        return ""
    if backend in ("", "grok"):
        return model
    return f"{backend}:{model}"


def main() -> None:
    ap = argparse.ArgumentParser(description="Show / maintain private model aliases.")
    ap.add_argument(
        "--path",
        type=Path,
        default=None,
        help=f"aliases file (default {DEFAULT_PATH} or $BOTC_ALIASES)",
    )
    ap.add_argument(
        "--from-ratings",
        type=Path,
        default=_REPO_ROOT / "leaderboard" / "ratings.json",
        help="ensure every model in this ratings book has an alias",
    )
    ap.add_argument(
        "--from-log",
        type=Path,
        default=None,
        help="also scan a results JSONL for model ids",
    )
    ap.add_argument("--json", action="store_true", help="print full aliases file as JSON")
    args = ap.parse_args()
    path = args.path or default_path()

    reals: set[str] = set()
    if args.from_ratings and args.from_ratings.is_file():
        book = json.loads(args.from_ratings.read_text())
        reals.update(book.get("ratings", {}).keys())
    if args.from_log and args.from_log.is_file():
        for line in args.from_log.read_text().splitlines():
            if not line.strip():
                continue
            try:
                rec = json.loads(line)
            except json.JSONDecodeError:
                continue
            if rec.get("event") != "game_end":
                continue
            for s in rec.get("seats") or []:
                m = node_key(s.get("backend"), s.get("model"))
                if m:
                    reals.add(m)

    mapping = ensure_aliases(reals, path) if reals else load(path)["models"]
    if args.json:
        print(json.dumps(load(path), indent=2, sort_keys=True))
        return

    if not mapping:
        print(f"(no aliases yet — {path})", file=sys.stderr)
        return
    width = max(len(r) for r in mapping)
    print(f"{'real model':{width}}  public")
    print(f"{'-' * width}  ------")
    for real in sorted(mapping, key=lambda r: mapping[r]):
        print(f"{real:{width}}  {mapping[real]}")
    print(f"\nprivate map: {path}", file=sys.stderr)


if __name__ == "__main__":
    main()
