"""Score for the Reflex 0.11 reel: 120 BPM, D minor, 23 bars + tail.

Every event sits on the grid the edit uses (beat 0.5 s, bar 2 s), so the
markers written alongside are exact. Synth voices come from synth.py (the
Lahza showreel's instruments).

  1-2   open      ticks, pad swell, riser
  3-4   mark      impact on 4.0, half-time kick
  5-8   idea      four on the floor, bass, soft arp; bar 8 builds, last beat out
  9-10  0.11      impact on 16.0, wide chord, half-time
  11-18 features  full groove, bright arp; a stab on every second bar (20, 24, 28, 32)
  19-20 montage   a kick and a stab on every beat; last beat silent
  21-23 end       impact on 40.0, the chord resolves; tail to 47 s
"""
import json
import sys

import numpy as np

from synth import *  # noqa: F401,F403
from synth import BAR, BEAT, LENGTH, N, SR, bus, place

# D minor: Dm - Bb - F - C, two bars each.
CHORDS = [[62, 65, 69, 74], [58, 62, 65, 70], [60, 65, 69, 72], [60, 64, 67, 72]]
ROOTS = [38, 34, 41, 36]


def bar_t(b, beat=1.0):
    return (b - 1) * BAR + (beat - 1) * BEAT


def chord_at(b):
    return (b - 1) // 2 % 4


drums, perc, music, fx, send = bus(), bus(), bus(), bus(), bus()
TICKS, kicks = [], []

# Open: quarter ticks, then eighths.
for b, step in ((1, 1.0), (2, 0.5)):
    q = 0.0
    while q < 4:
        acc = q % 1 == 0
        place(perc, tick(1.0 if acc else 1.5), bar_t(b, 1 + q), 0.5 if acc else 0.3, pan=-0.25 if acc else 0.25)
        TICKS.append(bar_t(b, 1 + q))
        q += step
for b in (3, 4, 9, 10):
    for q in range(4):
        place(perc, tick(1.0 if q == 0 else 1.5), bar_t(b, 1 + q), 0.2 if q else 0.28, pan=-0.3 if q % 2 else 0.3)
        TICKS.append(bar_t(b, 1 + q))
for i in range(9):
    b, q = 21 + i // 4, i % 4
    place(perc, tick(1.0 if q == 0 else 1.5), bar_t(b, 1 + q), 0.3 * 0.82 ** i, pan=0.3 if i % 2 else -0.3)
    TICKS.append(bar_t(b, 1 + q))

# Pads.
for b in range(1, 19, 2):
    gain = 0.7 if b < 3 else (0.95 if b < 9 else 0.85)
    place(music, pad_chord(CHORDS[chord_at(b)], 2 * BAR + 0.6, 1400 if b < 9 else 3000), bar_t(b), gain)
place(music, pad_chord([38, 50, 57, 62, 64, 69, 76], 7.0, 1700), bar_t(21), 1.3)  # Dm(add9)

IMPACTS = ((4.0, 1.0, "mark"), (16.0, 1.15, "version"), (40.0, 1.25, "end"))
STABS = (20.0, 24.0, 28.0, 32.0)
place(fx, riser(1.6), 4.0 - 1.6, 0.55)
place(fx, reverse_cymbal(1.4), 4.0 - 1.4, 0.6)
place(fx, riser(1.9), 16.0 - 1.9, 0.6)
place(fx, reverse_cymbal(1.0), 16.0 - 1.0, 0.6)
place(fx, riser(1.4), 40.0 - 1.9, 0.6)
place(fx, reverse_cymbal(0.5), 40.0 - 0.5, 0.7)
for t in STABS:
    place(fx, reverse_cymbal(0.5), t - 0.5, 0.3)
    place(fx, impact(0.55), t, 0.5)
for t, s, _ in IMPACTS:
    place(fx, impact(s), t, 0.9)
    place(fx, downlifter(1.6), t + 0.02, 0.35)
    place(send, impact(s) * 0.4, t)
    kicks.append(t)


def groove(b, full=True, hats16=False, clapit=True):
    for q in range(4):
        t = bar_t(b, 1 + q)
        place(drums, kick(), t, 0.95)
        kicks.append(t)
        if clapit and q in (1, 3):
            place(drums, clap(), t, 0.5, pan=0.05)
            place(send, clap(), t, 0.35)
        if full:
            place(perc, hat(True), t + BEAT / 2, 0.28, pan=0.35)
        for s in range(4 if hats16 else 2):
            off = s * BEAT / (4 if hats16 else 2)
            if hats16 or s == 0 or not full:
                vel = (0.32, 0.14, 0.22, 0.14)[s] if hats16 else 0.24
                if not (full and off == BEAT / 2):
                    place(perc, hat(False), t + off, vel, pan=-0.3)


# Half-time under the mark and the version.
for b in (3, 4, 9, 10):
    for q in (0, 2):
        if q == 0 and b in (3, 9):
            continue
        place(drums, kick(), bar_t(b, 1 + q), 0.85)
        kicks.append(bar_t(b, 1 + q))
    if b in (4, 10):
        for s in range(16):
            place(perc, hat(False), bar_t(b) + s * BEAT / 4, (0.18, 0.07, 0.12, 0.07)[s % 4], pan=-0.25)
for b in range(5, 9):
    groove(b, full=b >= 6)
for i, d in enumerate(np.cumsum([0.25] * 4 + [0.125] * 8 + [0.0625] * 8)):
    t = bar_t(8) + d - 0.25
    if t < bar_t(8, 4):
        place(drums, clap(), t, 0.12 + 0.3 * i / 20, pan=0.1)
for b in range(11, 19):
    groove(b, hats16=True)
for b in (19, 20):
    for q in range(4):
        t = bar_t(b, 1 + q)
        if t >= 39.5:
            continue
        place(drums, kick(), t, 1.0)
        kicks.append(t)
        c = CHORDS[(q + 2 * (b - 19)) % 4]
        place(music, stab(c), t, 1.0)
        place(send, stab(c), t, 0.6)
        place(perc, hat(True), t + BEAT / 2, 0.22, pan=0.3)

# Bass.
for b in list(range(4, 9)) + list(range(10, 19)):
    root = hz(ROOTS[chord_at(b)])
    if b in (4, 10):
        place(music, bass(root, BAR - 0.05), bar_t(b), 0.8)
        continue
    fast = b >= 11
    for s in (range(16) if fast else range(0, 16, 2)):
        if fast and s % 4 == 0:
            continue
        f = root * (2 if s % 8 == 6 and fast else 1)
        place(music, bass(f, BEAT / 4 * 0.9 if fast else BEAT / 2 * 0.85), bar_t(b) + s * BEAT / 4, 0.75)
place(music, bass(hz(26), 6.0), bar_t(21), 0.7)

# Arp.
for b in list(range(6, 9)) + list(range(11, 19)):
    tones = [m + 12 for m in CHORDS[chord_at(b)]]
    pattern = [0, 2, 1, 3, 2, 1, 3, 0]
    for s in range(16):
        m = tones[pattern[s % 8]] + (12 if s % 8 == 4 and b >= 11 else 0)
        bright = 0.5 if b < 9 else 1.4
        place(music, pluck(hz(m), 0.32, bright), bar_t(b) + s * BEAT / 4, 0.3 if b < 9 else 0.5, pan=0.45 * np.sin(s * 0.7))
        place(send, pluck(hz(m), 0.32, bright), bar_t(b) + s * BEAT / 4, 0.12)

# The version: a slow plucked line over the wide chord.
for b, beat, m in [(9, 1, 74), (9, 3, 77), (10, 1, 81), (10, 2.5, 79), (10, 3.5, 77)]:
    place(music, pluck(hz(m), 1.0, 2.0), bar_t(b, beat), 0.4, pan=0.2)
    place(send, pluck(hz(m), 1.0, 2.0), bar_t(b, beat), 0.5)
for m, d in ((74, 0), (81, 0.02), (86, 0.04)):
    place(music, pluck(hz(m), 3.0, 3.0), 40.0 + d, 0.3)
    place(send, pluck(hz(m), 3.0, 3.0), 40.0 + d, 0.45)

# Mix.
wet = reverb(send + 0.25 * music + 0.15 * perc, reverb_ir(2.6))
duck = np.ones(N)
t_ = np.arange(int(0.3 * SR)) / SR
shape = 1 - 0.7 * np.exp(-t_ / 0.09)
for kt in kicks:
    i = int(kt * SR)
    n = min(len(shape), N - i)
    if n > 0:
        duck[i:i + n] = np.minimum(duck[i:i + n], shape[:n])
section = np.ones(N)
section[int(8 * SR):int(16 * SR)] = 0.72
mix = drums * section * 0.25 + perc * 1.7 + (music * duck) * 0.56 + fx * 0.42 + wet * duck * 0.33
mix = np.stack([highpass(c, 28, 2) for c in mix])
fade = int(1.5 * SR)
mix[:, -fade:] *= np.linspace(1, 0, fade) ** 2
loud = mix[:, int(20 * SR):int(30 * SR)]
mix *= 10 ** (-13.5 / 20) / np.sqrt(np.mean(loud ** 2))
ceiling = 10 ** (-1.2 / 20)
mix = ceiling * np.tanh(mix / ceiling)

out = sys.argv[1] if len(sys.argv) > 1 else "track.wav"
from scipy.io import wavfile  # noqa: E402

wavfile.write(out, SR, (mix.T * 32767).astype(np.int16))
beats = [{"t": round(i * BEAT, 4)} for i in range(int(LENGTH / BEAT))]
bars = [{"t": round(i * BAR, 4)} for i in range(int(LENGTH / BAR) + 1)]
hits = [{"t": t, "label": l} for t, _, l in IMPACTS] + [{"t": t, "label": "feature"} for t in STABS]
hits.sort(key=lambda m: m["t"])
ticks = [{"t": round(t, 4)} for t in sorted(set(TICKS))]
json.dump({"beat": beats, "bar": bars, "hit": hits, "tick": ticks}, open(out.replace(".wav", "-markers.json"), "w"))
print(out, f"{LENGTH:.1f}s rms {20*np.log10(np.sqrt(np.mean(mix**2))):.1f} dBFS")
