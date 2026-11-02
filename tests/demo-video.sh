#!/usr/bin/env bash
# Lab for the demo video: master (2 local disks) + remote node (disk3),
# readable demo hook, small-file traffic from 60 cameras, 1 GB RAM ceiling.
HERE="$(cd "$(dirname "$0")" && pwd)"
BIN="${SPILLWAY_BIN:-$(cd "$HERE/.." && pwd)/target/release/spillway}"
pkill -f demotraffic.py; pkill -f smalltraffic.py
pkill -INT -f "$BIN"; for i in $(seq 1 50); do pgrep -f "$BIN" >/dev/null || break; sleep 0.2; done; pkill -9 -f "$BIN"
# start from an empty spool (it is a tmpfs that survives restarts on purpose)
mountpoint -q /root/spw/ram && rm -rf /root/spw/ram/* /root/spw/ram/.[!.]* 2>/dev/null
rm -rf /root/spw/outbox
bash $HERE/demo.sh > /dev/null
bash $HERE/mkdemofix.sh /root/spw/demofix > /dev/null
pkill -f smalltraffic.py; pkill -f demotraffic.py; pkill -f hookserver.py; pkill -f demohook.py; pkill -f demo-retention
setsid nohup python3 $HERE/demohook.py 7091 > /root/spw/demohook.log 2>&1 < /dev/null &
sleep 0.5
curl -s -XPUT localhost:7070/api/settings -H 'content-type: application/json' -d '{"ram":{"thief_max_gb":1.0},"hooks":{
  "validate":{"url":"http://127.0.0.1:7091/spillway/validate"},
  "moved":{"url":"http://127.0.0.1:7091/spillway/moved"}}}' > /dev/null
setsid nohup python3 $HERE/demotraffic.py --cameras 60 --every 3 --drop-every 9 > /root/spw/demotraffic.log 2>&1 < /dev/null &
setsid nohup bash -c 'exec -a demo-retention bash -c "while true; do for d in /root/spw/d1 /root/spw/d2 /root/spw/d3; do find \$d -name \"*.mp4\" -mmin +2 -delete 2>/dev/null; find \$d -mindepth 1 -type d -empty -not -path \"*/.spillway*\" -delete 2>/dev/null; done; sleep 20; done"' > /dev/null 2>&1 < /dev/null &
echo "demo-video lab up"
