#!/usr/bin/env bash
# Demo lab: master (2 local destinations) + remote node serving d3, continuous traffic.
set -e
HERE="$(cd "$(dirname "$0")" && pwd)"
BIN="${SPILLWAY_BIN:-$(cd "$HERE/.." && pwd)/target/release/spillway}"
LAB=/root/spw
pkill -INT -f "$BIN" || true
for i in $(seq 1 50); do pgrep -f "$BIN" >/dev/null || break; sleep 0.2; done
pkill -9 -f "$BIN" || true
umount -l $LAB/videos 2>/dev/null || true
bash $HERE/lab.sh up
for d in d1 d2 d3; do find $LAB/$d -mindepth 1 -maxdepth 1 -not -name lost+found -exec rm -rf {} +; done
cp $HERE/lab.toml $LAB/spillway.toml
cd $LAB
setsid nohup $BIN master -c $LAB/spillway.toml > $LAB/master.log 2>&1 < /dev/null &
for i in $(seq 1 50); do curl -sf localhost:7070/healthz >/dev/null && break; sleep 0.1; done
setsid nohup $BIN node --id node-b --master http://127.0.0.1:7070 --token lab-token --listen 127.0.0.1:7071 --advertise http://127.0.0.1:7071 > $LAB/node.log 2>&1 < /dev/null &
sleep 3
curl -s -XPOST localhost:7070/api/destinations -H 'content-type: application/json' \
  -d '{"id":"disk3","node":"node-b","path":"/root/spw/d3","expect_mount":true}' ; echo
echo demo up
