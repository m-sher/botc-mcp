# BOTC model leaderboard

Live web leaderboard of model ratings from the multi-agent harness results log.

## Pieces

| Piece | Path | Role |
| --- | --- | --- |
| **Results log** | `botc-results.jsonl` (TUI) | Append-only finished games (`game_end` with per-seat `model` + `won`) |
| **Rater** | `scripts/rate_models.py` | Batch Bradley–Terry MAP over team-vs-team game outcomes → `leaderboard/ratings.json` |
| **Publisher** | `scripts/publish_leaderboard.py` | Re-rate + POST to Worker when display content changes |
| **Worker** | `leaderboard/worker.js` | `GET /api/leaderboard`, `POST /api/publish`, static page |
| **Page** | `leaderboard/public/index.html` | Polls API every 30s, ranked table + ± bars |

### Rating logic (short)

1. Take finished games only (`event == game_end`).
2. Treat each game as one team-vs-team contest; a team's strength is the mean rating of its seats, so P(winning team wins) = sigmoid(mean_win − mean_lose). A strong teammate raises the team mean, so a weaker seat earns little credit for an expected win.
3. Fit a stationary Bradley–Terry model on the Elo scale with a soft Gaussian prior at 1500.
4. Report MAP rating ± Laplace 1σ and games played.

This is the multiplayer analogue of QTris’s 1v1 whole-history rating: same publish path, simpler stationary BT because models are not generation trajectories.


## Name privacy

Public entry ids are **aliases** (`Agent-01`, `Agent-02`, …), not raw model strings.

| File | Public? | Contents |
| --- | --- | --- |
| `leaderboard/aliases.json` | **No** (gitignored) | `{ "models": { "real-model-id": "Agent-01", … } }` |
| `leaderboard/ratings.json` | **No** (gitignored) | Ratings keyed by **real** model ids |
| Cloudflare KV payload | **Yes** | Ratings keyed by **alias** only |

New models get the next `Agent-NN` the first time they appear at publish time. Assignments are stable and never recycled. Inspect or hand-edit labels locally:

```bash
python3 scripts/model_aliases.py --from-ratings leaderboard/ratings.json --from-log botc-results.jsonl
```

Override a label by editing `aliases.json` (keep values unique). Do **not** pass `--reveal` to the publisher unless you intentionally want real names on the public Worker.

---

## One-time Cloudflare setup

Docs used: [Workers static assets](https://developers.cloudflare.com/workers/static-assets/), [Workers KV get started](https://developers.cloudflare.com/kv/get-started/), [Wrangler config](https://developers.cloudflare.com/workers/wrangler/configuration/).

### 0. Prerequisites

- Cloudflare account (free tier is enough)
- Node.js 16.17+ (for Wrangler)
- Python 3.10+ (rater / publisher are stdlib-only)

### 1. Log in with Wrangler

From the repo root (or any dir):

```bash
cd leaderboard
npx wrangler login
# opens a browser; approve the API token
npx wrangler whoami   # confirm account
```

### 2. Create a KV namespace

```bash
cd leaderboard
npx wrangler kv namespace create LEADERBOARD
```

Copy the printed **id**. Paste it into `wrangler.toml`:

```toml
[[kv_namespaces]]
binding = "LEADERBOARD"
id = "paste-the-id-here"
```

Optional (local `wrangler dev` with real KV preview):

```bash
npx wrangler kv namespace create LEADERBOARD --preview
# add preview_id = "..." under the same [[kv_namespaces]] block
```

### 3. Set the publish secret

Pick a long random token (do **not** commit it):

```bash
# generate one
python3 -c 'import secrets; print(secrets.token_urlsafe(32))'

cd leaderboard
npx wrangler secret put PUBLISH_TOKEN
# paste the token when prompted
```

For local dev only, you can put the same value in `leaderboard/.dev.vars` (gitignored):

```
PUBLISH_TOKEN=your-token-here
```

### 4. Deploy the Worker + page

```bash
cd leaderboard
npx wrangler deploy
```

Wrangler prints a URL like:

```text
https://botc-leaderboard.<your-subdomain>.workers.dev
```

Open it — you should see “Waiting for the first publish…”.

Optional custom domain: Cloudflare dashboard → Workers → `botc-leaderboard` → Triggers → Custom Domains.

### 5. Wire the publisher on the machine that runs games

In the **repo root** `.env` (gitignored — create it):

```bash
LEADERBOARD_URL=https://botc-leaderboard.<your-subdomain>.workers.dev
LEADERBOARD_TOKEN=the-same-token-you-put-in-PUBLISH_TOKEN
```

Rate once from existing results and publish:

```bash
python3 scripts/rate_models.py --log botc-results.jsonl --out leaderboard/ratings.json
python3 scripts/publish_leaderboard.py --once
```

Or loop (re-rates every 2 minutes, writes KV at most every 5 minutes, force-refreshes hourly):

```bash
./scripts/run_leaderboard_publisher.sh
# or foreground:
python3 scripts/publish_leaderboard.py --interval 120
```

Refresh the Worker URL — the table should fill in.

---

## Day-to-day

1. Play games with `botc-tui` → appends `botc-results.jsonl`.
2. Leave the publisher running (or cron `--once` after a session).
3. Page auto-polls; no redeploy needed for new ratings.

Redeploy the Worker only when you change `worker.js` or `public/index.html`:

```bash
cd leaderboard && npx wrangler deploy
```

---

## Local preview without Cloudflare

Serve the static page and mock the API:

```bash
# Terminal A: rate current log
python3 scripts/rate_models.py

# Terminal B: tiny static server that injects ratings into the API shape
python3 - <<'PY'
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path
import json
from datetime import datetime, timezone

ROOT = Path("leaderboard/public")
BOOK = Path("leaderboard/ratings.json")

class H(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path.startswith("/api/leaderboard"):
            book = json.loads(BOOK.read_text())
            entries = [
                {
                    "id": k,
                    "rating": v["rating"],
                    "sigma": v["sigma"],
                    "games": v["games"],
                    "win_rate": v.get("win_rate"),
                    "present": True,
                }
                for k, v in book["ratings"].items()
            ]
            entries.sort(key=lambda e: -e["rating"])
            body = json.dumps({
                "updated_at": datetime.now(timezone.utc).isoformat(timespec="seconds"),
                "games_rated": book.get("games_rated"),
                "entries": entries,
            }).encode()
            self.send_response(200)
            self.send_header("content-type", "application/json")
            self.send_header("cache-control", "no-store")
            self.end_headers()
            self.wfile.write(body)
            return
        path = ROOT / ("index.html" if self.path in ("/", "") else self.path.lstrip("/"))
        if not path.is_file():
            self.send_error(404); return
        data = path.read_bytes()
        ctype = "text/html" if path.suffix == ".html" else "application/octet-stream"
        self.send_response(200)
        self.send_header("content-type", ctype)
        self.end_headers()
        self.wfile.write(data)
    def log_message(self, *a): pass

HTTPServer(("127.0.0.1", 8787), H).serve_forever()
PY
# open http://127.0.0.1:8787/
```

Or full Worker locally (needs KV binding + `.dev.vars`):

```bash
cd leaderboard
npx wrangler dev
```

---

## API contract

**`GET /api/leaderboard`** → JSON:

```json
{
  "updated_at": "2026-07-13T05:00:00+00:00",
  "games_rated": 12,
  "entries": [
    { "id": "model-name", "rating": 1542.3, "sigma": 80.1, "games": 8, "win_rate": 0.55, "present": true }
  ]
}
```

Empty KV: `{ "status": "waiting" }`.

**`POST /api/publish`** — `Authorization: Bearer <PUBLISH_TOKEN>`, body = full payload JSON. Invalid token → 401.

---

## Free-tier notes

- KV free tier ~1k writes/day. The publisher digests integer-quantized ratings and enforces `--min-interval` (default 300s) plus hourly liveness writes.
- Worker requests on the free plan are plenty for a private leaderboard.
