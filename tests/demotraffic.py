#!/usr/bin/env python3
"""Demo traffic: N cameras each writing a complete, valid MP4 segment every few
seconds (picked from --fixdir), streamed in small chunks like a live recorder.
Every --drop-every'th segment is named drop-* (the demo hook refuses those).
With --burst N it instead writes N files as fast as possible and exits."""
import argparse
import datetime
import glob
import os
import random
import threading
import time
import uuid

p = argparse.ArgumentParser()
p.add_argument("--mount", default="/root/spw/videos")
p.add_argument("--fixdir", default="/root/spw/demofix")
p.add_argument("--cameras", type=int, default=60)
p.add_argument("--every", type=float, default=3.0)
p.add_argument("--drop-every", type=int, default=0)
p.add_argument("--burst", type=int, default=0, help="write this many files at full speed, then exit")
p.add_argument("--threads", type=int, default=32)
p.add_argument("--seconds", type=float, default=0.0, help="spread the burst over this long")
a = p.parse_args()

clips = [open(f, "rb").read() for f in sorted(glob.glob(os.path.join(a.fixdir, "*.mp4")))]
cams = [str(uuid.UUID(int=random.Random(i).getrandbits(128))) + ("-sub" if i % 6 == 5 else "") for i in range(a.cameras)]


def write(cam, data, drop, chunk=32768, pause=0.0):
    now = datetime.datetime.now()
    rel = f"{cam}/{now:%Y-%m-%d}/{'drop-' if drop else ''}{now:%H-%M-%S}-{now.microsecond:06d}.mp4"
    path = os.path.join(a.mount, rel)
    try:
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "wb") as f:
            for off in range(0, len(data), chunk):
                f.write(data[off:off + chunk])
                if pause:
                    f.flush()
                    time.sleep(pause)
    except OSError:
        time.sleep(0.5)


def camera(i):
    rnd = random.Random(i)
    time.sleep(rnd.uniform(0, a.every))
    n = 0
    while True:
        n += 1
        write(cams[i], clips[(i + n) % len(clips)], a.drop_every and n % a.drop_every == 0, pause=0.01)
        time.sleep(a.every * rnd.uniform(0.7, 1.3))


def burster(k, count):
    t0 = time.time()
    for j in range(count):
        if a.seconds:
            # pace each thread evenly across the window
            wait = t0 + a.seconds * j / count - time.time()
            if wait > 0:
                time.sleep(wait)
        write(cams[(k * 7 + j) % len(cams)], clips[(k + j) % len(clips)], False, chunk=1 << 20)


if a.burst:
    per = -(-a.burst // a.threads)
    ts = [threading.Thread(target=burster, args=(k, min(per, a.burst - k * per))) for k in range(a.threads) if a.burst - k * per > 0]
    t0 = time.time()
    for t in ts:
        t.start()
    for t in ts:
        t.join()
    print(f"burst: {a.burst} files in {time.time() - t0:.2f}s")
else:
    for i in range(a.cameras):
        threading.Thread(target=camera, args=(i,), daemon=True).start()
    while True:
        time.sleep(3600)
