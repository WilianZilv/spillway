#!/usr/bin/env bash
# Takes thread stacks of the master in the middle of a burst.
HERE="$(cd "$(dirname "$0")" && pwd)"
BIN="${SPILLWAY_BIN:-$(cd "$HERE/.." && pwd)/target/release/spillway}"
bash $HERE/demo-video.sh > /dev/null
sleep 12
python3 $HERE/demotraffic.py --burst 3000 --threads 24 --seconds 8 > /root/spw/burst.out &
sleep 4
PID=$(pgrep -f "release/spillway master")
for k in 1 2; do
  gdb -p $PID -batch -ex "thread apply all bt 25" > /root/spw/stacks-$k.txt 2>/dev/null
  sleep 1
done
wait
cat /root/spw/burst.out
bash $HERE/demo-stop.sh > /dev/null
for k in 1 2; do
  echo "=== snapshot $k: runtime worker threads"
  awk '/^Thread /{name=$0} /^Thread .*tokio-runtime-w/{p=1} p' /root/spw/stacks-$k.txt | head -0
  python3 - /root/spw/stacks-$k.txt <<'PY'
import re, sys, collections
txt = open(sys.argv[1]).read()
threads = re.split(r'\n(?=Thread \d+ )', txt)
c = collections.Counter()
for t in threads:
    head = t.split('\n', 1)[0]
    m = re.search(r'"([^"]+)"', head)
    name = m.group(1) if m else '?'
    frames = re.findall(r'#\d+\s+(?:0x[0-9a-f]+ in )?([^\s(]+)', t)
    # first frame from our crate, else top frame
    ours = next((f for f in frames if 'spillway' in f), None)
    top = frames[0] if frames else '?'
    c[(name, top[:60], (ours or '-')[:110])] += 1
for (name, top, ours), n in c.most_common(25):
    print(f"{n:3d}  {name:22s} {top:45s} {ours}")
PY
done
