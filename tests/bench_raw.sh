#!/usr/bin/env bash
# Raw node PUT speed with curl (no master in the data path).
HERE="$(cd "$(dirname "$0")" && pwd)"
BIN="${SPILLWAY_BIN:-$(cd "$HERE/.." && pwd)/target/release/spillway}"
bash $HERE/demo.sh > /dev/null
mkdir -p /root/spw/t2; mountpoint -q /root/spw/t2 || mount -t tmpfs -o size=3G tmpfs /root/spw/t2
curl -s -XPOST localhost:7070/api/destinations -H 'content-type: application/json' -d '{"id":"tmp-remote","node":"node-b","path":"/root/spw/t2","expect_mount":true}' >/dev/null
sleep 3
mkdir -p /dev/shm/b; head -c 4M /dev/urandom > /dev/shm/b/f4; head -c 512M /dev/urandom > /dev/shm/b/f512
t0=$(date +%s.%N)
for i in $(seq 1 100); do curl -s -o /dev/null -T /dev/shm/b/f4 -H 'authorization: Bearer lab-token' "http://127.0.0.1:7071/v1/dest/tmp-remote/file?path=raw/$i.mp4&mtime_ms=0&mode=420&uid=0&gid=0&size=4194304"; done
t1=$(date +%s.%N); echo "curl 100x4MiB seq: $(python3 -c "print(round(400/($t1-$t0)))") MiB/s"
t0=$(date +%s.%N)
curl -s -T /dev/shm/b/f512 -H 'authorization: Bearer lab-token' "http://127.0.0.1:7071/v1/dest/tmp-remote/file?path=raw/big.mp4&mtime_ms=0&mode=420&uid=0&gid=0&size=536870912"; echo
t1=$(date +%s.%N); echo "curl 1x512MiB: $(python3 -c "print(round(512/($t1-$t0)))") MiB/s"
t0=$(date +%s.%N); cat /dev/shm/b/f512 > /root/spw/t2/cp.bin; t1=$(date +%s.%N); echo "cat to tmpfs: $(python3 -c "print(round(512/($t1-$t0)))") MiB/s"
nproc; ps -o pid,args -C spillway
rm -rf /dev/shm/b
pkill -INT -f "$BIN"; for i in $(seq 1 100); do pgrep -f "$BIN" >/dev/null || break; sleep 0.2; done; pkill -9 -f "$BIN"
umount -l /root/spw/videos /root/spw/t2 2>/dev/null; echo parado
