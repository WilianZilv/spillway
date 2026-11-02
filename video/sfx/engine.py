"""Tiny procedural sound engine (stdlib only): generators return mono float
lists at SR; Mix places them on a stereo timeline and writes a WAV.
Loudness mastering is done afterwards with ffmpeg's loudnorm (two-pass)."""

import math
import random
import struct
import wave

SR = 48000
rng = random.Random(7)  # reproducible renders


def _env(n, attack, tau):
    a = max(1, int(attack * SR))
    out = []
    for i in range(n):
        if i < a:
            out.append(i / a)
        else:
            out.append(math.exp(-(i - a) / (tau * SR)))
    return out


def bandnoise(dur, f0, f1, q=0.9):
    """Noise through a resonant band-pass whose centre sweeps f0 -> f1."""
    n = int(dur * SR)
    out = []
    low = band = 0.0
    for i in range(n):
        k = i / max(1, n - 1)
        fc = f0 * (f1 / f0) ** k
        f = 2 * math.sin(math.pi * min(fc, SR / 6) / SR)
        x = rng.uniform(-1, 1)
        low += f * band
        high = x - low - q * band
        band += f * high
        out.append(band)
    return out


def whoosh(dur, f0, f1, peak=0.5, q=0.9):
    """Air movement: reveals, transitions. f0->f1 sets the direction."""
    s = bandnoise(dur, f0, f1, q)
    n = len(s)
    out = []
    for i, v in enumerate(s):
        k = i / max(1, n - 1)
        e = (k / peak) ** 1.6 if k < peak else ((1 - k) / (1 - peak)) ** 2.2
        out.append(v * e * 1.6)
    return out


def pop(f0, f1, dur=0.16):
    """Pitched pop: something appearing. Raise f0 across a sequence."""
    n = int(dur * SR)
    e = _env(n, 0.002, dur / 4)
    out, ph = [], 0.0
    for i in range(n):
        f = f0 * (f1 / f0) ** (i / n)
        ph += 2 * math.pi * f / SR
        out.append(math.sin(ph) * e[i])
    return out


def click(freq=1800, dur=0.04):
    """UI tick: a slot, a step, a file landing."""
    n = int(dur * SR)
    e = _env(n, 0.0005, dur / 6)
    return [(math.sin(2 * math.pi * freq * i / SR) * 0.7 + rng.uniform(-1, 1) * 0.3) * e[i] for i in range(n)]


def blip(freq, dur=0.35, tau=0.1):
    """Soft tone: a word landing, a note of a sequence (keep it in key)."""
    n = int(dur * SR)
    e = _env(n, 0.004, tau)
    return [(math.sin(2 * math.pi * freq * i / SR) + 0.25 * math.sin(4 * math.pi * freq * i / SR)) * e[i] for i in range(n)]


def chime(freqs, dur=1.8, tau=0.6):
    """Bell chord: arrival, success."""
    n = int(dur * SR)
    e = _env(n, 0.003, tau)
    out = [0.0] * n
    for j, f in enumerate(freqs):
        for p, g in ((1, 1.0), (2.76, 0.18), (5.4, 0.06)):
            w = 2 * math.pi * f * p / SR
            d = 1 + p * 0.6
            for i in range(n):
                out[i] += math.sin(w * i + j) * g * e[i] ** d / len(freqs)
    return out


def thud(dur=0.35):
    """Low knock: a heavy block landing."""
    n = int(dur * SR)
    e = _env(n, 0.002, 0.07)
    out, ph = [], 0.0
    for i in range(n):
        f = 140 * (55 / 140) ** (i / n)
        ph += 2 * math.pi * f / SR
        out.append((math.sin(ph) + rng.uniform(-1, 1) * 0.15 * (1 - i / n)) * e[i])
    return out


def error(dur=0.5):
    """A refusal: two notes a semitone apart, slightly rough."""
    n = int(dur * SR)
    e = _env(n, 0.003, 0.16)
    return [(math.sin(2 * math.pi * 311.13 * i / SR) + math.sin(2 * math.pi * 329.63 * i / SR) * 0.8 + math.copysign(0.25, math.sin(2 * math.pi * 155.6 * i / SR))) * e[i] * 0.6 for i in range(n)]


def swell(dur, f0=300, f1=2400):
    """Tension that builds without resolving (not the hero riser)."""
    s = bandnoise(dur, f0, f1, 0.6)
    n = len(s)
    return [v * (i / n) ** 2 * 1.4 for i, v in enumerate(s)]


def riser(dur, f0=180, f1=4200):
    """The one build-up: ends exactly on the hero hit."""
    n = int(dur * SR)
    nz = bandnoise(dur, f0 * 2, f1, 0.5)
    out, ph = [], 0.0
    for i in range(n):
        k = i / n
        f = f0 * (f1 / 4 / f0) ** (k ** 1.5)
        ph += 2 * math.pi * f / SR
        out.append((math.sin(ph) * 0.5 + nz[i] * 0.9) * k ** 2.4)
    return out


def impact(dur=1.6):
    """Hero hit: sub drop + noise burst + long tail. Once per video."""
    n = int(dur * SR)
    out, ph = [], 0.0
    for i in range(n):
        k = i / n
        f = 90 * (34 / 90) ** min(1, k * 3)
        ph += 2 * math.pi * f / SR
        sub = math.sin(ph) * math.exp(-k * 4.5)
        nz = rng.uniform(-1, 1) * math.exp(-k * 28) * 0.8
        out.append((sub + nz) * (min(1, i / 60)))
    return out


def envelope(points):
    """Piecewise-linear gain curve over time, [(t, v), ...] -> f(t)."""
    def f(t):
        if t <= points[0][0]:
            return points[0][1]
        for (t0, v0), (t1, v1) in zip(points, points[1:]):
            if t0 <= t <= t1:
                return v0 + (v1 - v0) * (t - t0) / max(1e-9, t1 - t0)
        return points[-1][1]
    return f


def pad(duration, chords, volume=lambda t: 1.0, detune=0.5, xfade=0.6):
    """Warm bed under everything: detuned sines with slow tremolo. Returns (left, right)."""
    n = int(duration * SR)
    left = [0.0] * n
    right = [0.0] * n
    step = 32  # control-rate for envelopes
    for ci, (start, freqs) in enumerate(chords):
        end = chords[ci + 1][0] if ci + 1 < len(chords) else duration
        a = int(max(0, start - xfade) * SR)
        b = min(n, int((end + xfade) * SR))
        for fi, f in enumerate(freqs):
            for side, dt in ((left, -detune), (right, detune)):
                w = 2 * math.pi * f * (1 + dt / 1200 * 10) / SR
                ph = fi * 1.3
                g = 0.0
                for i in range(a, b):
                    if (i - a) % step == 0:
                        t = i / SR
                        fade_in = min(1, (t - (start - xfade)) / (2 * xfade)) if ci else 1
                        fade_out = min(1, ((end + xfade) - t) / (2 * xfade)) if ci + 1 < len(chords) else 1
                        trem = 0.85 + 0.15 * math.sin(2 * math.pi * 0.23 * t + fi)
                        g = max(0.0, fade_in) * max(0.0, fade_out) * trem * volume(t) / len(freqs)
                    side[i] += math.sin(w * i + ph) * g
    return left, right


class Mix:
    def __init__(self, duration):
        self.n = int(duration * SR)
        self.L = [0.0] * self.n
        self.R = [0.0] * self.n

    def put(self, t, samples, gain=0.2, pan=0.0):
        """Place a mono sound at time t (seconds) with equal-power pan (-1..1)."""
        a = math.cos((pan + 1) * math.pi / 4) * gain
        b = math.sin((pan + 1) * math.pi / 4) * gain
        start = int(t * SR)
        for i, v in enumerate(samples):
            j = start + i
            if 0 <= j < self.n:
                self.L[j] += v * a
                self.R[j] += v * b

    def put_stereo(self, t, left, right, gain=1.0):
        start = int(t * SR)
        for i in range(len(left)):
            j = start + i
            if 0 <= j < self.n:
                self.L[j] += left[i] * gain
                self.R[j] += right[i] * gain

    def write(self, path):
        peak = max(1e-9, max(max(abs(v) for v in self.L), max(abs(v) for v in self.R)))
        norm = 0.7 / peak  # headroom; loudnorm sets the final level
        with wave.open(path, "wb") as w:
            w.setnchannels(2)
            w.setsampwidth(2)
            w.setframerate(SR)
            frames = bytearray()
            for l, r in zip(self.L, self.R):
                frames += struct.pack("<hh", int(max(-1, min(1, l * norm)) * 32767), int(max(-1, min(1, r * norm)) * 32767))
            w.writeframes(bytes(frames))
        return peak
