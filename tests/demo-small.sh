#!/usr/bin/env bash
# Long-running dashboard demo: master + remote node + small-file traffic + retention.
HERE="$(cd "$(dirname "$0")" && pwd)"
BIN="${SPILLWAY_BIN:-$(cd "$HERE/.." && pwd)/target/release/spillway}"
bash $HERE/demo.sh
bash $HERE/mkfixtures.sh /root/spw/fixtures >/dev/null
pkill -f smalltraffic.py; pkill -f demo-retention
setsid nohup python3 $HERE/smalltraffic.py --cameras 40 --every 4 > /root/spw/smalltraffic.log 2>&1 < /dev/null &
# retention: keep the small lab disks from filling up (files older than 10 min)
setsid nohup bash -c 'exec -a demo-retention bash -c "while true; do for d in /root/spw/d1 /root/spw/d2 /root/spw/d3; do find \$d -name \"*.mp4\" -mmin +10 -delete 2>/dev/null; find \$d -mindepth 1 -type d -empty -not -path \"*/.spillway*\" -delete 2>/dev/null; done; sleep 30; done"' > /dev/null 2>&1 < /dev/null &
sleep 2
pgrep -af "smalltraffic|spillway" | grep -v pgrep
