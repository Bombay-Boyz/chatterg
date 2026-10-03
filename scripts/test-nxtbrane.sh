#!/usr/bin/env bash
# Live check against the real NxTBrane agent. Not part of `cargo test`.
set -euo pipefail

TARGET="${1:-https://flowmarket.social/nxtbrane}"
DB="$(mktemp -u /tmp/chatterg-nxtbrane.XXXXXX.db)"
trap 'rm -f "$DB"' EXIT

echo "== discovery =="
curl -fsS "${TARGET%/}/.well-known/agent-card.json" | python3 -m json.tool | head -40

echo "== run 1 =="
cargo run -q -- "$TARGET" questions.yaml --store "$DB"

echo "== persisted conversation =="
python3 - "$DB" <<'PY'
import json, sqlite3, sys
row = sqlite3.connect(sys.argv[1]).execute("select data from conversations").fetchone()
c = json.loads(row[0])
print("state:", c["state"], "position:", c["position"], "messages_sent:", c["messages_sent"])
PY

echo "== run 2 (must resume, not repeat) =="
cargo run -q -- "$TARGET" questions.yaml --store "$DB"
