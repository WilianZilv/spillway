#!/usr/bin/env python3
"""Continuous demo traffic with small files: N cameras each writing a segment
every few seconds (64-512 KiB, real MP4 header so the parser has something to
read), in small chunks like a live recorder. Runs until killed."""
import argparse
import datetime
import os
import random
import threading
import time
import uuid

p = argparse.ArgumentParser()
p.add_argument("--mount", default="/root/spw/videos")
p.add_argument("--fixtures", default="/root/spw/fixtures")
p.add_argument("--cameras", type=int, default=40)
p.add_argument("--every", type=float, default=4.0, help="seconds between segments per camera")
p.add_argument("--drop-every", type=int, default=0, help="name every Nth segment drop-*.mp4 (the test hook refuses those)")
a = p.parse_args()

heads = [open(os.path.join(a.fixtures, f), "rb").read() for f in ("frag.mp4", "progressive.mp4", "faststart.mp4")]


def camera(i):
    rnd = random.Random(i)
    cam = str(uuid.UUID(int=rnd.getrandbits(128)))
    sub = "-sub" if i % 6 == 5 else ""
    time.sleep(rnd.uniform(0, a.every))
    n = 0
    while True:
        n += 1
        now = datetime.datetime.now()
        drop = "drop-" if a.drop_every and n % a.drop_every == 0 else ""
        rel = f"{cam}{sub}/{now:%Y-%m-%d}/{drop}{now:%H-%M-%S}-{now.microsecond:06d}.mp4"
        size = rnd.randint(64, 512) * 1024
        src = heads[i % len(heads)]
        data = (src * (size // len(src) + 1))[:size] if size > len(src) else src[:size]
        path = os.path.join(a.mount, rel)
        try:
            os.makedirs(os.path.dirname(path), exist_ok=True)
            with open(path, "wb") as f:
                for off in range(0, len(data), 32768):
                    f.write(data[off:off + 32768])
                    f.flush()
                    time.sleep(0.01)
        except OSError:
            time.sleep(1)
        time.sleep(a.every * rnd.uniform(0.7, 1.3))


for i in range(a.cameras):
    threading.Thread(target=camera, args=(i,), daemon=True).start()
while True:
    time.sleep(3600)
