#!/usr/bin/env python3
"""Simulates MediaMTX cameras writing recording segments into the mount.

Each camera writes `{uuid}/{YYYY-MM-DD}/{HH-MM-SS-ffffff}.mp4` by streaming a
fixture in small chunks (like fMP4 parts), optionally pausing between chunks,
then closes the file. Prints a JSON summary with the list of files written.
"""
import argparse, datetime, json, os, random, sys, threading, time, uuid

p = argparse.ArgumentParser()
p.add_argument("--mount", required=True)
p.add_argument("--fixtures", default="/root/spw/fixtures")
p.add_argument("--cameras", type=int, default=20)
p.add_argument("--segments", type=int, default=10, help="segments per camera")
p.add_argument("--chunk", type=int, default=64 * 1024)
p.add_argument("--chunk-delay", type=float, default=0.0, help="sleep between chunks (s)")
p.add_argument("--reopen", action="store_true", help="close and reopen (append) mid-segment")
p.add_argument("--manifest", default=None)
p.add_argument("--seed", type=int, default=1)
a = p.parse_args()

random.seed(a.seed)
fixtures = [open(os.path.join(a.fixtures, f), "rb").read() for f in ("frag.mp4", "progressive.mp4", "faststart.mp4")]
cams = [str(uuid.UUID(int=random.getrandbits(128))) for _ in range(a.cameras)]
written, errors, lock = [], [], threading.Lock()
base = datetime.datetime(2026, 10, 22, 8, 0, 0)

def camera(i, cam):
    rnd = random.Random(i)
    t = base + datetime.timedelta(seconds=rnd.randint(0, 3000))
    for s in range(a.segments):
        t += datetime.timedelta(seconds=10, microseconds=rnd.randint(0, 999999))
        rel = f"{cam}{'-sub' if i % 5 == 4 else ''}/{t:%Y-%m-%d}/{t:%H-%M-%S}-{t.microsecond:06d}.mp4"
        data = fixtures[(i + s) % len(fixtures)]
        path = os.path.join(a.mount, rel)
        try:
            os.makedirs(os.path.dirname(path), exist_ok=True)
            f = open(path, "wb")
            half = len(data) // 2
            for off in range(0, len(data), a.chunk):
                if a.reopen and off >= half and not f.closed and off - a.chunk < half:
                    f.close()
                    time.sleep(0.3)
                    f = open(path, "r+b")
                    f.seek(off)
                f.write(data[off:off + a.chunk])
                f.flush()
                if a.chunk_delay:
                    time.sleep(a.chunk_delay)
            f.close()
            with lock:
                written.append({"path": rel, "size": len(data)})
        except OSError as e:
            with lock:
                errors.append({"path": rel, "error": str(e)})

t0 = time.time()
threads = [threading.Thread(target=camera, args=(i, c)) for i, c in enumerate(cams)]
for th in threads: th.start()
for th in threads: th.join()
dt = time.time() - t0
total = sum(w["size"] for w in written)
summary = {"files": len(written), "bytes": total, "errors": len(errors), "first_errors": errors[:5],
           "seconds": round(dt, 2), "files_per_min": round(len(written) / dt * 60), "mb_per_s": round(total / dt / 1048576, 1)}
if a.manifest:
    json.dump({"written": written, "errors": errors}, open(a.manifest, "w"))
print(json.dumps(summary))
