#!/usr/bin/env python3
"""Golden + corpus-regression test for the cross-language node_key (issue #69).

Run: python3 scripts/test_node_key.py  (exits non-zero on failure)

Mirrors the Rust golden test in src/harness/balance.rs (node_key_golden /
read_model_stats_folds_legacy_grok_and_splits_claude). Keeps the Python raters'
node_key in lockstep with the Rust one and proves the existing corpus rates
identically (no leaderboard node splits).
"""
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from model_aliases import node_key as alias_key  # noqa: E402
from rate_models import node_key as rate_key  # noqa: E402


def check(cond: bool, msg: str) -> None:
    if not cond:
        print(f"FAIL: {msg}")
        sys.exit(1)


def main() -> None:
    for node_key in (rate_key, alias_key):
        # Legacy grok (no backend) and explicit grok both stay bare → same node.
        check(node_key("", "grok-build") == "grok-build", "legacy grok bare")
        check(node_key("grok", "grok-build") == "grok-build", "explicit grok bare")
        # Claude is namespaced and never conflates with grok:same-name.
        check(
            node_key("claude", "claude-opus-4-8") == "claude:claude-opus-4-8",
            "claude namespaced",
        )
        check(node_key("claude", "x") != node_key("grok", "x"), "claude != grok same model")
        # Empty model → "" (skipped by callers).
        check(node_key("claude", "") == "", "empty model → empty")
        check(node_key(None, None) == "", "None → empty")

    # rate_models and model_aliases must agree on every case.
    for backend in ("", "grok", "claude", "codex", None):
        for model in ("", "m", "grok-build", "claude-opus-4-8", None):
            check(
                rate_key(backend, model) == alias_key(backend, model),
                f"cross-script agree {backend!r}/{model!r}",
            )

    # Corpus regression: the committed corpus predates #69 (no `backend` field, or
    # "grok"), so every node_key equals the bare model — the leaderboard is unchanged.
    corpus = Path(__file__).parent.parent / "botc-results.jsonl"
    if corpus.is_file():
        rows = 0
        for line in corpus.read_text().splitlines():
            if not line.strip():
                continue
            try:
                rec = json.loads(line)
            except json.JSONDecodeError:
                continue
            for s in rec.get("seats") or []:
                model = (s.get("model") or "").strip()
                if model:
                    rows += 1
                    check(
                        rate_key(s.get("backend"), model) == model,
                        f"legacy corpus row must stay bare: {model!r} backend={s.get('backend')!r}",
                    )
        print(f"OK: node_key golden + corpus regression ({rows} legacy seat rows unchanged)")
    else:
        print("OK: node_key golden (no corpus file to regress)")


if __name__ == "__main__":
    main()
