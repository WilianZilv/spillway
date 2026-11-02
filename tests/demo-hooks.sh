#!/usr/bin/env bash
# Dashboard demo with hooks: master + node + test hook server (validate + moved)
# + small-file traffic where every 7th segment is refused by the hook.
HERE="$(cd "$(dirname "$0")" && pwd)"
BIN="${SPILLWAY_BIN:-$(cd "$HERE/.." && pwd)/target/release/spillway}"
bash $HERE/demo.sh
bash $HERE/mkfixtures.sh /root/spw/fixtures >/dev/null
pkill -f smalltraffic.py; pkill -f hookserver.py; pkill -f demo-retention
setsid nohup python3 $HERE/hookserver.py 7090 > /root/spw/hookserver.log 2>&1 < /dev/null &
sleep 1
curl -s -XPUT localhost:7070/api/settings -H 'content-type: application/json' -d '{"hooks":{
  "validate":{"url":"http://127.0.0.1:7090/spillway/validate","token":"hook-token"},
  "moved":{"url":"http://127.0.0.1:7090/spillway/moved","token":"hook-token"}}}'; echo
setsid nohup python3 $HERE/smalltraffic.py --cameras 40 --every 4 --drop-every 7 > /root/spw/smalltraffic.log 2>&1 < /dev/null &
setsid nohup bash -c 'exec -a demo-retention bash -c "while true; do for d in /root/spw/d1 /root/spw/d2 /root/spw/d3; do find \$d -name \"*.mp4\" -mmin +10 -delete 2>/dev/null; find \$d -mindepth 1 -type d -empty -not -path \"*/.spillway*\" -delete 2>/dev/null; done; sleep 30; done"' > /dev/null 2>&1 < /dev/null &
sleep 1; pgrep -af "smalltraffic|hookserver|spillway (master|node)" | grep -v pgrep
