"""Sound design for SpillwayVideo. Every time comes from timeline.json.

    uv run python sfx/spillway.py      -> public/sfx-raw.wav (mastered later with loudnorm)
"""

import json
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from engine import Mix, blip, chime, click, envelope, error, impact, pad, pop, riser, swell, thud, whoosh  # noqa: E402

ROOT = os.path.dirname(os.path.dirname(__file__))
tl = json.load(open(os.path.join(ROOT, "timeline.json")))

# E major throughout: bed Emaj9 -> E6/9 after the hit; melodic notes from the scale.
BED_A = [82.41, 123.47, 164.81, 207.65, 311.13, 369.99]
BED_B = [82.41, 123.47, 164.81, 277.18, 369.99, 493.88]
SCALE = [659.25, 739.99, 830.61, 987.77, 1108.73, 1318.51, 1479.98, 1661.22]
DISK_PAN = [-0.6, 0.0, 0.6]


def words(mix, start, n, notes, gain=0.09, pan0=-0.3):
    for i in range(n):
        t = start + i * 0.09
        mix.put(t, whoosh(0.28, 600, 2600, peak=0.45), gain, pan0 + i * 0.2)
        mix.put(t + 0.12, blip(notes[i % len(notes)] / 4, dur=0.3, tau=0.08), gain * 1.6)


def score(mix):
    s1, s2, s3, s4, s5, s6 = (tl[k] for k in ("s1", "s2", "s3", "s4", "s5", "s6"))
    dur = tl["duration"]
    put = mix.put

    duck, lift = s6["merge"] + 0.2, s6["hit"] + 0.25
    vol = envelope([(0, 0.0), (1.2, 1.0), (duck, 1.0), (s6["hit"], 0.3), (lift, 0.3), (lift + 0.6, 1.2), (dur - 1.3, 1.2), (dur, 0.0)])
    mix.put_stereo(0, *pad(dur, [(0, BED_A), (s6["hit"], BED_B)], volume=vol), gain=0.05)

    # scene 1: cameras come up, files start pouring
    put(0.0, whoosh(0.9, 300, 1800, peak=0.3), 0.12)
    words(mix, s1["line1"], 3, SCALE[:3])
    words(mix, s1["line2"], 4, SCALE[2:6])
    for i in range(70):  # files dropping: a light rain of ticks, thinning out
        t = s1["flow"] + 0.15 + i * 0.05 + (i * 37 % 11) * 0.004
        if t > s1["exit"]:
            break
        put(t, click(2600 + (i * 53 % 7) * 180, 0.025), 0.035 * (1 - i / 90), -0.8 + (i * 29 % 17) / 10)
    for i in range(8):  # the counter rolling up
        put(s1["counter"] + i * 0.18, blip(SCALE[i % 8], dur=0.18, tau=0.05), 0.05 + i * 0.006, 0.0)
    put(s1["counter"] + 1.55, chime([SCALE[0], SCALE[3], SCALE[5]], dur=1.4, tau=0.45), 0.12)
    put(s1["exit"], whoosh(0.45, 2400, 500, peak=0.35), 0.14)

    # scene 2: the old stack lands heavy, then gets struck through
    for i, t in enumerate(s2["stack"]):
        put(t + 0.05, thud(), 0.3, -0.2 + i * 0.2)
    words(mix, s2["line"], 2, SCALE[:2])
    words(mix, s2["line"] + 0.4, 2, [SCALE[1], SCALE[0]])
    for i in range(3):
        put(s2["strike"] + i * 0.12, whoosh(0.32, 1800, 5200, peak=0.25, q=0.5), 0.13, -0.4 + i * 0.4)
    put(s2["exit"], whoosh(0.6, 1600, 220, peak=0.3), 0.18)

    # scene 3: the mark, the RAM, the disks, the flow
    put(s3["logo"], chime([SCALE[0], SCALE[2], SCALE[4]], dur=1.8, tau=0.55), 0.14)
    put(s3["logo"], whoosh(0.8, 200, 1600, peak=0.3), 0.12)
    put(s3["word"], whoosh(0.5, 500, 2400, peak=0.4), 0.1)
    put(s3["line1"] - 0.25, whoosh(0.6, 2200, 700, peak=0.5), 0.1, -0.3)
    words(mix, s3["line1"], 3, SCALE[:3])
    put(s3["ram"], pop(330, 165, dur=0.22), 0.26)
    words(mix, s3["line2"], 4, SCALE[3:7])
    for i, t in enumerate(s3["disks"]):
        put(t, pop(440 + i * 110, 220 + i * 55, dur=0.2), 0.24, DISK_PAN[i])
    for i in range(3):
        put(s3["shares"] + i * 0.12, blip(SCALE[4 + i], dur=0.3, tau=0.08), 0.08, DISK_PAN[i])
    # files landing on the disks (every other one, panned to its disk)
    pattern = [0, 1, 0, 2, 0, 1, 0]
    for n in range(0, 200, 2):
        ts = s3["streams"] + n * 0.085 + 0.85
        if ts > s4["exit"]:
            break
        if s4["rise"] <= ts - 0.85 < s4["thief"] + 0.9:
            continue
        d = pattern[n % 7]
        if d == 1 and s4["fail"] <= ts - 0.85 < s4["recover"] + 0.2:
            d = 0
        put(ts, click(1500 + d * 300, 0.03), 0.045, DISK_PAN[d])

    # scene 4: a disk goes down and comes back; the RAM fills and the thief trims
    words(mix, s4["q1"], 2, [SCALE[2], SCALE[1]])
    put(s4["fail"], error(), 0.22, 0.0)
    put(s4["fail"], thud(0.3), 0.22, 0.0)
    put(s4["reroute"], whoosh(0.5, 900, 3000, peak=0.5), 0.1, -0.5)
    words(mix, s4["a1"], 3, SCALE[3:6])
    put(s4["recover"], blip(SCALE[3], dur=0.4, tau=0.12), 0.1)
    put(s4["recover"] + 0.12, blip(SCALE[5], dur=0.5, tau=0.16), 0.11)
    words(mix, s4["q2"], 3, SCALE[:3])
    put(s4["rise"], swell(s4["ceiling"] - s4["rise"] + 0.1), 0.12)
    put(s4["thief"], pop(1200, 300, dur=0.25), 0.18, -0.2)
    for i in range(2):
        put(s4["thief"] + 0.15 + i * 0.25, pop(900 - i * 200, 180, dur=0.2), 0.16, -0.5 + i * 0.1)
    words(mix, s4["a2"], 3, SCALE[3:6])
    put(s4["exit"], whoosh(0.5, 2600, 500, peak=0.4), 0.15)

    # scene 5: the live panel
    put(s5["panel"], whoosh(0.8, 300, 2000, peak=0.35), 0.13)
    words(mix, s5["line"], 3, SCALE[2:5])
    for i, t in enumerate(s5["stats"]):
        put(t, pop(700 + i * 160, 380 + i * 80, dur=0.18), 0.22, -0.5 + i * 0.5)
    put(s5["exit"], whoosh(0.4, 2200, 600, peak=0.4), 0.12)

    # scene 6: everything converges into the mark — the one riser and the one impact
    put(s6["riser"], riser(s6["hit"] - s6["riser"]), 0.2)
    put(s6["merge"], whoosh(s6["hit"] - s6["merge"], 400, 5000, peak=0.85), 0.16)
    put(s6["hit"], impact(), 0.42)
    put(s6["hit"], chime([SCALE[0] / 2, SCALE[2] / 2, SCALE[4] / 2, SCALE[5] / 2], dur=3.0, tau=1.0), 0.16)
    words(mix, s6["word"], 1, [SCALE[0]], gain=0.1)
    words(mix, s6["tag"], 4, SCALE[2:6], gain=0.07)
    words(mix, s6["tag"] + 0.4, 3, SCALE[3:6], gain=0.07)
    put(s6["footer"], click(2200), 0.08)


if __name__ == "__main__":
    out = os.path.join(ROOT, "public", "sfx-raw.wav")
    m = Mix(tl["duration"])
    score(m)
    peak = m.write(out)
    print(f"wrote {out} (raw peak {peak:.2f})")
