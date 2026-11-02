#!/usr/bin/env bash
# Continuous demo traffic: waves of cameras writing into the mount + a crude
# retention on the lab destinations so the small loop disks never fill up.
# Usage: tests/traffic.sh [seconds]   (default: forever)
HERE="$(cd "$(dirname "$0")" && pwd)"
BIN="${SPILLWAY_BIN:-$(cd "$HERE/.." && pwd)/target/release/spillway}"
END=$(( $(date +%s) + ${1:-999999} ))
i=0
while [ "$(date +%s)" -lt "$END" ]; do
  i=$((i+1))
  python3 $HERE/camsim.py --mount /root/spw/videos --cameras $((20 + RANDOM % 30)) \
    --segments 3 --chunk-delay 0.004 --seed $i > /dev/null 2>&1
  for d in /root/spw/d1 /root/spw/d2 /root/spw/d3; do
    find $d -name '*.mp4' -mmin +2 -delete 2>/dev/null
    find $d -mindepth 1 -type d -empty -not -path '*/.spillway*' -delete 2>/dev/null
  done
  sleep $((1 + RANDOM % 3))
done
