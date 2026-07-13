#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$(readlink -f "$0")")/.."

if [[ -f .env ]]; then
  set -a
  # shellcheck source=/dev/null
  source ./.env
  set +a
fi

: "${LEADERBOARD_URL:?set LEADERBOARD_URL (in .env or the environment)}"
: "${LEADERBOARD_TOKEN:?set LEADERBOARD_TOKEN (in .env or the environment)}"

mkdir -p logs
ts="$(date +%Y%m%d_%H%M%S)"
log="logs/leaderboard_${ts}.log"
pidfile="logs/leaderboard_${ts}.pid"

setsid python3 scripts/publish_leaderboard.py --interval 120 \
  >"$log" 2>&1 </dev/null &

pid=$!
echo "$pid" >"$pidfile"

echo "Publishing BOTC model leaderboard"
echo "  PID:  $pid   (pidfile: $pidfile)"
echo "  Log:  $log"
