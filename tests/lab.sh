#!/usr/bin/env bash
# Test lab: two ext4 loop filesystems as destinations, a master with
# a small RAM ceiling. Usage: tests/lab.sh up|down
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
BIN="${SPILLWAY_BIN:-$(cd "$HERE/.." && pwd)/target/release/spillway}"
LAB=/root/spw
case "${1:-up}" in
up)
  mkdir -p $LAB
  for d in d1:4000 d2:2000 d3:1000; do
    n=${d%%:*}; size=${d##*:}
    mkdir -p $LAB/$n
    if ! mountpoint -q $LAB/$n; then
      [ -f $LAB/$n.img ] || { truncate -s ${size}M $LAB/$n.img; mkfs.ext4 -q -F $LAB/$n.img; }
      mount -o loop $LAB/$n.img $LAB/$n
    fi
  done
  ;;
down)
  pkill -f "$BIN" || true
  sleep 1
  for m in $LAB/videos $LAB/ram $LAB/d1 $LAB/d2 $LAB/d3; do umount -l $m 2>/dev/null || true; done
  ;;
esac
