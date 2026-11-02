#!/usr/bin/env bash
# Transport benchmark: local copy vs remote HTTP node (both tmpfs). Leaves nothing running.
HERE="$(cd "$(dirname "$0")" && pwd)"
BIN="${SPILLWAY_BIN:-$(cd "$HERE/.." && pwd)/target/release/spillway}"
bash $HERE/demo.sh > /dev/null
python3 -u $HERE/bench_transport.py
pkill -INT -f "$BIN"; for i in $(seq 1 100); do pgrep -f "$BIN" >/dev/null || break; sleep 0.2; done; pkill -9 -f "$BIN"
umount -l /root/spw/videos /root/spw/t1 /root/spw/t2 2>/dev/null
echo "lab parado"
