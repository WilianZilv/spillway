"""Sound design for DemoVideo. Every time comes from demo-timeline.json.

    uv run python sfx/demo.py      -> public/demo-sfx-raw.wav (mastered by scripts/master.mjs)
"""

import json
import os
import sys

sys.path.insert(0, os.path.dirname(__file__))
from engine import Mix, blip, chime, click, envelope, error, impact, pad, pop, riser, swell, thud, whoosh  # noqa: E402

ROOT = os.path.dirname(os.path.dirname(__file__))
tl = json.load(open(os.path.join(ROOT, "demo-timeline.json")))

# Same key as the intro film (E major) so the two read as a pair.
BED_A = [82.41, 123.47, 164.81, 207.65, 311.13, 369.99]
BED_B = [82.41, 123.47, 164.81, 277.18, 369.99, 493.88]
BED_C = [73.42, 110.0, 146.83, 220.0, 277.18, 369.99]  # D6/9 under the failure scenes
SCALE = [659.25, 739.99, 830.61, 987.77, 1108.73, 1318.51, 1479.98, 1661.22]
BPM = 112
BEAT = 60 / BPM

PANEL_SCENES = ["burst", "fail", "cluster", "hooks", "thief", "settings"]


def words(mix, start, n, notes, gain=0.08, pan0=-0.3):
    for i in range(n):
        t = start + 0.15 + i * 0.07
        mix.put(t, whoosh(0.26, 600, 2600, peak=0.45), gain, pan0 + i * 0.15)
        mix.put(t + 0.1, blip(notes[i % len(notes)] / 4, dur=0.28, tau=0.07), gain * 1.5)


def ui_click(mix, t, pan=0.2):
    mix.put(t, click(2400, 0.03), 0.12, pan)
    mix.put(t + 0.015, click(1300, 0.04), 0.08, pan)


def score(mix):
    dur = tl["duration"]
    put = mix.put
    I, T, O = tl["intro"], tl["term"], tl["outro"]
    S = {k: tl[k] for k in PANEL_SCENES}

    # bed: E through the intro and panel tour, a darker turn for the failures, home again for the finale
    vol = envelope([(0, 0.0), (1.0, 1.0), (O["at"] - 0.3, 1.0), (O["at"] + 0.2, 0.4), (O["at"] + 0.9, 1.25), (dur - 1.0, 1.25), (dur, 0.0)])
    chords = [(0, BED_A), (S["fail"]["at"], BED_C), (S["cluster"]["at"], BED_A), (S["thief"]["at"], BED_C), (S["settings"]["at"], BED_A), (O["at"], BED_B)]
    mix.put_stereo(0, *pad(dur, chords, volume=vol), gain=0.05)

    # a soft pulse drives the panel tour (kick on the beat, tick off it)
    t = S["burst"]["at"]
    beat = 0
    while t < O["at"] - 0.4:
        put(t, thud(0.22), 0.07, 0.0)
        put(t + BEAT / 2, click(5200, 0.018), 0.022, 0.35 if beat % 2 else -0.35)
        t += BEAT
        beat += 1

    # intro
    put(0.1, whoosh(0.9, 250, 1600, peak=0.35), 0.12)
    put(0.2, chime([SCALE[0], SCALE[2], SCALE[4]], dur=1.6, tau=0.5), 0.12)
    words(mix, I["l1"] - 0.15, 3, SCALE[:3])
    words(mix, I["l2"] - 0.15, 5, SCALE[2:7])
    put(I["end"] - 0.45, whoosh(0.5, 2400, 500, peak=0.4), 0.14)

    # terminal: the window lands, keys type, outputs tick in
    put(T["at"], pop(330, 165, dur=0.22), 0.2)
    words(mix, T["at"], 6, SCALE[:6], gain=0.06)
    for i, c in enumerate(T["cmds"]):
        at = T["at"] + c
        n = [21, 44, 60][i]
        for j in range(0, n, 2):
            put(at + j / 46, click(3000 + (j * 37 % 9) * 140, 0.012), 0.03, -0.1 + (j % 5) * 0.05)
        put(at + n / 46 + 0.1, click(1600, 0.03), 0.07)
        put(at + 0.75, blip(SCALE[3 + i], dur=0.25, tau=0.06), 0.06)
    put(T["end"] - 0.35, whoosh(0.45, 2200, 600, peak=0.4), 0.14)

    # every panel scene: the screen arrives, the headline, the exit
    for k, s in S.items():
        put(s["at"], whoosh(0.7, 300, 2200, peak=0.35), 0.13)
        put(s["at"] + 0.15, pop(440, 220, dur=0.2), 0.14)
        words(mix, s["at"], 4, SCALE[1:5], gain=0.06)
        put(s["end"] - 0.35, whoosh(0.45, 2400, 500, peak=0.4), 0.13)

    # burst: a rain of files while it runs, a chime when the counter lands
    b = S["burst"]
    for i in range(110):
        at = b["at"] + b["burst"] + 0.1 + i * 0.075 + (i * 37 % 11) * 0.003
        put(at, click(2600 + (i * 53 % 7) * 200, 0.02), 0.03 * (1 - i / 140), -0.8 + (i * 29 % 17) / 10)
    put(b["at"] + b["burst"] + 0.5, chime([SCALE[0], SCALE[3], SCALE[5]], dur=1.4, tau=0.45), 0.1)
    put(b["at"] + b["c1"], blip(SCALE[4], dur=0.3, tau=0.08), 0.08, -0.4)
    put(b["at"] + b["c2"], blip(SCALE[5], dur=0.3, tau=0.08), 0.08, 0.4)

    # disk failure: the hit, the reroute, the recovery
    f = S["fail"]
    put(f["at"] + f["down"], error(), 0.22, 0.3)
    put(f["at"] + f["down"], thud(0.3), 0.22, 0.3)
    put(f["at"] + f["reroute"] + 0.5, whoosh(0.5, 900, 3000, peak=0.5), 0.1, -0.5)
    put(f["at"] + f["back"] + 0.2, blip(SCALE[3], dur=0.4, tau=0.12), 0.1, 0.3)
    put(f["at"] + f["back"] + 0.32, blip(SCALE[5], dur=0.5, tau=0.16), 0.11, 0.3)

    # cluster: a server joins, the cursor clicks over to Destinos
    k = S["cluster"]
    put(k["at"] + k["join"], chime([SCALE[2], SCALE[4], SCALE[6]], dur=1.2, tau=0.35), 0.11)
    ui_click(mix, k["at"] + k["nav"] - 0.1, -0.3)

    # hooks: cut to files, cut to groups, the two select clicks
    hk = S["hooks"]
    for c in (hk["files"], hk["rec"]):
        put(hk["at"] + c, whoosh(0.3, 1800, 4800, peak=0.3, q=0.5), 0.08)
    ui_click(mix, hk["at"] + hk["select"], 0.4)
    ui_click(mix, hk["at"] + hk["option"], 0.4)

    # thief: RAM rising, the trim, the drain
    th = S["thief"]
    put(th["at"] + th["banner"], error(0.4), 0.14)
    put(th["at"] + th["banner"] + 0.4, swell(th["thief"] - th["banner"] - 0.3), 0.12)
    put(th["at"] + th["thief"], pop(1200, 300, dur=0.25), 0.18, -0.2)
    for i in range(3):
        put(th["at"] + th["thief"] + 0.15 + i * 0.22, pop(900 - i * 180, 180, dur=0.2), 0.14, -0.5 + i * 0.2)
    put(th["at"] + th["drain"], whoosh(0.8, 3000, 400, peak=0.3), 0.12)
    put(th["at"] + th["drain"] + 0.4, blip(SCALE[4], dur=0.4, tau=0.12), 0.09)

    # settings: the drag, the save, the resize lands
    st = S["settings"]
    put(st["at"] + st["drag"], whoosh(0.9, 400, 2400, peak=0.7, q=0.6), 0.1, 0.2)
    ui_click(mix, st["at"] + st["save"], 0.5)
    put(st["at"] + st["save"] + 0.8, chime([SCALE[1], SCALE[3], SCALE[5]], dur=1.0, tau=0.3), 0.1)

    # finale: the one riser and the one impact
    put(O["at"] - 1.6, riser(1.6), 0.16)
    put(O["at"] + O["logo"], impact(), 0.38)
    put(O["at"] + O["logo"], chime([SCALE[0] / 2, SCALE[2] / 2, SCALE[4] / 2, SCALE[5] / 2], dur=3.0, tau=1.0), 0.15)
    words(mix, O["at"] + O["word"] - 0.15, 1, [SCALE[0]], gain=0.09)
    words(mix, O["at"] + O["tag"] - 0.15, 7, SCALE[1:8], gain=0.05)
    for i, s in enumerate(O["stats"]):
        put(O["at"] + s, pop(700 + i * 160, 380 + i * 80, dur=0.18), 0.18, -0.5 + i * 0.5)
    put(O["at"] + O["footer"], click(2200), 0.07)


if __name__ == "__main__":
    out = os.path.join(ROOT, "public", "demo-sfx-raw.wav")
    m = Mix(tl["duration"])
    score(m)
    peak = m.write(out)
    print(f"wrote {out} (raw peak {peak:.2f})")
