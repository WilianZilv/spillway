#!/usr/bin/env python3
"""Transport benchmark: how fast does RAM drain into a local destination
(copy_file_range) vs a remote node (HTTP PUT)? Both destinations are tmpfs so
the disk does not hide the transport cost. Needs tests/demo.sh running.

Method: with every destination disabled, write N files into the mount (they
stay in RAM), enable exactly one destination and time how long the master
takes to move everything out.
"""
import json
import os
import subprocess
import sys
import time
import urllib.request

API = "http://127.0.0.1:7070"
MNT = "/root/spw/videos"
N = int(os.environ.get("N", "500"))
SIZE = int(os.environ.get("SIZE_MB", "4")) * 1024 * 1024


def api(path, method="GET", body=None):
    req = urllib.request.Request(API + path, data=json.dumps(body).encode() if body is not None else None, method=method)
    if body is not None:
        req.add_header("content-type", "application/json")
    with urllib.request.urlopen(req, timeout=30) as r:
        t = r.read()
        return json.loads(t) if t else None


def sh(c):
    subprocess.run(c, shell=True, check=False, capture_output=True)


def ov():
    return api("/api/overview")


def only(dest, inflight):
    for d in ov()["dests"]:
        want = d["id"] == dest
        api(f"/api/destinations/{d['id']}", "PATCH", {"draining": not want, **({"max_inflight": inflight} if want else {})})


def run(dest, inflight, tag):
    for d in ov()["dests"]:
        api(f"/api/destinations/{d['id']}", "PATCH", {"draining": True})
    data = os.urandom(SIZE)
    for i in range(N):
        p = f"{MNT}/bench-{tag}/2026-10-22/{i:04d}.mp4"
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, "wb") as f:
            f.write(data)
    # all sealed and waiting in RAM
    while ov()["ram"]["queued"] < N:
        time.sleep(0.1)
    m0 = ov()["totals"]["moved_bytes"]
    only(dest, inflight)
    # a re-enabled remote destination waits for the node's next heartbeat to
    # be assigned; the clock starts at the first byte moved, not at the PATCH
    while ov()["totals"]["moved_bytes"] == m0:
        time.sleep(0.002)
    t0 = time.time()
    peak = 0
    while True:
        o = ov()
        d = next(x for x in o["dests"] if x["id"] == dest)
        peak = max(peak, d["inflight"])
        if o["ram"]["files"] == 0:
            break
        time.sleep(0.005)
    dt = time.time() - t0
    moved = ov()["totals"]["moved_bytes"] - m0
    sh(f"rm -rf /root/spw/t1/bench-{tag} /root/spw/t2/bench-{tag}")
    return moved / dt / 1048576, dt, peak


def main():
    sh("mkdir -p /root/spw/t1 /root/spw/t2")
    for t in ("t1", "t2"):
        sh(f"mountpoint -q /root/spw/{t} || mount -t tmpfs -o size=3G tmpfs /root/spw/{t}")
    api("/api/settings", "PUT", {"ram": {"thief_max_gb": 3.0, "settle_secs": 0.5}})
    for d, node, path in (("tmp-local", "local", "/root/spw/t1"), ("tmp-remote", "node-b", "/root/spw/t2")):
        try:
            api("/api/destinations", "POST", {"id": d, "node": node, "path": path, "expect_mount": True})
        except Exception:  # noqa: BLE001  already there
            pass
    time.sleep(3)
    print(f"{N} files x {SIZE // 1048576} MiB = {N * SIZE / 1048576:.0f} MiB per run")
    print(f"{'destino':<12} {'cópias':>6} {'MiB/s':>9} {'tempo':>8}")
    for inflight in (1, 4, 8):
        for dest in ("tmp-local", "tmp-remote"):
            rate, dt, peak = run(dest, inflight, f"{dest}-{inflight}")
            print(f"{dest:<12} {inflight:>6} {rate:>9.0f} {dt:>7.2f}s  (pico {peak})", flush=True)


if __name__ == "__main__":
    sys.exit(main())
