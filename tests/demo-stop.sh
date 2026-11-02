#!/usr/bin/env bash
# Stops every demo lab process (master, node, traffic, hooks, retention) and unmounts.
HERE="$(cd "$(dirname "$0")" && pwd)"
BIN="${SPILLWAY_BIN:-$(cd "$HERE/.." && pwd)/target/release/spillway}"
pkill -f demotraffic.py; pkill -f smalltraffic.py; pkill -f demohook.py; pkill -f hookserver.py; pkill -f demo-retention
pkill -INT -f "$BIN"; for i in $(seq 1 50); do pgrep -f "$BIN" >/dev/null || break; sleep 0.2; done; pkill -9 -f "$BIN"
umount -l /root/spw/videos 2>/dev/null
pgrep -af "spillway|demotraffic|demohook|demo-retention" | grep -v pgrep || echo "lab parado"
