#!/usr/bin/env bash
# Runs the whole e2e suite N times (flakiness check). Usage: tests/e2e-loop.sh [N]
HERE="$(cd "$(dirname "$0")" && pwd)"
BIN="${SPILLWAY_BIN:-$(cd "$HERE/.." && pwd)/target/release/spillway}"
cd /root/spw
for i in $(seq 1 "${1:-2}"); do
  echo "== run $i"
  python3 -u $HERE/e2e.py 2>&1 | grep -E '^(PASS|FAIL)|scenarios passed'
done
