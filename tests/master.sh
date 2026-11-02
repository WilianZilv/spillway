#!/usr/bin/env bash
# (Re)starts the lab master in the background. Usage: tests/master.sh [config]
HERE="$(cd "$(dirname "$0")" && pwd)"
BIN="${SPILLWAY_BIN:-$(cd "$HERE/.." && pwd)/target/release/spillway}"
CFG=${1:-/root/spw/spillway.toml}
pkill -INT -f "$BIN master" && sleep 2
pkill -9 -f "$BIN master" 2>/dev/null
umount -l /root/spw/videos 2>/dev/null
cd /root/spw
setsid nohup $BIN master -c "$CFG" >> /root/spw/master.log 2>&1 < /dev/null &
for i in $(seq 1 50); do curl -sf localhost:7070/healthz >/dev/null && break; sleep 0.1; done
mountpoint -q /root/spw/videos && echo "master up" || { echo "MOUNT FAILED"; tail -20 /root/spw/master.log; }
