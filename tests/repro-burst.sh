#!/usr/bin/env bash
# Repro: the demo lab plus a paced burst of new files; prints how the remote
# destination behaved (breaker trips, node-side errors, lookup counts).
HERE="$(cd "$(dirname "$0")" && pwd)"
BIN="${SPILLWAY_BIN:-$(cd "$HERE/.." && pwd)/target/release/spillway}"
bash $HERE/demo-video.sh > /dev/null
sleep 3
if [ -n "$NOMOVED" ]; then curl -s -XPUT localhost:7070/api/settings -H "content-type: application/json" -d "{\"hooks\":{\"moved\":{\"url\":\"\"}}}" > /dev/null; echo "(moved hook off)"; fi
sleep 12
: > /root/spw/burst.out
n0=$(grep -c 'dest_unresponsive' /root/spw/master.log)
python3 $HERE/demotraffic.py --burst ${1:-3000} --threads 24 --seconds ${2:-8} > /root/spw/burst.out
sleep 3
cat /root/spw/burst.out
echo "breaker trips during burst: $(( $(grep -c 'dest_unresponsive' /root/spw/master.log) - n0 ))"
grep 'dest_unresponsive' /root/spw/master.log | tail -3 | sed -E 's/.*WARN //'
echo "--- node log (warn/error)"; grep -iE 'warn|error' /root/spw/node.log | tail -5
curl -s localhost:7070/api/overview | python3 -c "import json,sys; d=json.load(sys.stdin); print([(x['id'], x['status'], x['failures']) for x in d['dests']])"
bash $HERE/demo-stop.sh > /dev/null
