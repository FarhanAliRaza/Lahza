"""Original 120 BPM track for the showcase: 18 bars from 0.25 s.

Am–F–C–G, four-on-the-floor from bar 1 (soft), full groove from bar 3,
a one-beat stop before the drop on bar 9, a final hit on bar 18.
Usage: python3 music.py out.wav
"""
import sys
import wave

import numpy as np

SR = 44100
BPM = 120.0
BEAT = 60.0 / BPM
T0 = 0.25
BARS = 18
LENGTH = T0 + BARS * 4 * BEAT + 2.5
rng = np.random.default_rng(7)
out = np.zeros(int(LENGTH * SR))


def at(t):
    return int(t * SR)


def add(t, sig, gain=1.0):
    i = at(t)
    j = min(len(out), i + len(sig))
    if j > i:
        out[i:j] += sig[: j - i] * gain


def env(n, attack, decay):
    t = np.arange(n) / SR
    return np.minimum(1.0, t / max(attack, 1e-4)) * np.exp(-t / decay)


def kick(amp):
    n = at(0.45)
    t = np.arange(n) / SR
    f = 45 + 110 * np.exp(-t * 38)
    return amp * np.sin(2 * np.pi * np.cumsum(f) / SR) * np.exp(-t * 7)


def noise_hit(dur, decay, hp=True):
    n = at(dur)
    x = rng.standard_normal(n)
    if hp:
        x = np.diff(x, prepend=0)
    return x * env(n, 0.001, decay)


def tone(freq, dur, kind="saw", attack=0.01, decay=0.4, partials=8):
    n = at(dur)
    t = np.arange(n) / SR
    if kind == "sine":
        s = np.sin(2 * np.pi * freq * t)
    else:
        s = sum(np.sin(2 * np.pi * freq * k * t) / k for k in range(1, partials + 1))
    return s * env(n, attack, decay)


def beat_time(bar, beat):
    return T0 + ((bar - 1) * 4 + (beat - 1)) * BEAT


def silent(bar, beat):
    # The stop: beat 4 of bar 8.
    return bar == 8 and beat >= 4


chords = [(220.0, 261.63, 329.63), (174.61, 220.0, 261.63), (130.81, 164.81, 196.0), (196.0, 246.94, 293.66)]
bass = [110.0, 87.31, 65.41, 98.0]

for bar in range(1, BARS + 1):
    ch = chords[(bar - 1) % 4]
    full = bar >= 3
    # Pad.
    if not (bar == 8):
        for f in ch:
            add(beat_time(bar, 1), tone(f, 4 * BEAT, "saw", attack=0.4, decay=3.0, partials=4), 0.025)
    for beat in range(1, 5):
        if silent(bar, beat):
            continue
        t = beat_time(bar, beat)
        downbeat = beat == 1
        if full or downbeat:
            add(t, kick(0.9 if full else 0.6))
        if full and beat in (2, 4):
            add(t, noise_hit(0.25, 0.07), 0.35)
        for half in (0.0, 0.5):
            if not silent(bar, beat):
                add(t + half * BEAT, noise_hit(0.05, 0.012), 0.12 if half else 0.07)
        if full:
            add(t + 0.5 * BEAT, tone(bass[(bar - 1) % 4], 0.45 * BEAT, "saw", attack=0.005, decay=0.18, partials=6), 0.22)
        # Arpeggio in sixteenths on the busy sections.
        if bar in (5, 6, 7, 10, 11, 12, 13, 14, 15, 16, 17):
            for s in range(4):
                f = ch[(beat + s) % 3] * 2
                add(t + s * BEAT / 4, tone(f, 0.18, "saw", attack=0.002, decay=0.06, partials=5), 0.05)

# Impacts: the logo hit (bar 2), the drop (bar 9), the end (bar 18).
for bar, gain in ((2, 0.6), (9, 1.0), (18, 1.0)):
    t = beat_time(bar, 1)
    add(t, noise_hit(1.6, 0.5, hp=False), 0.25 * gain)
    add(t, tone(55.0, 1.6, "sine", attack=0.001, decay=0.9), 0.8 * gain)
    add(t, noise_hit(0.6, 0.15), 0.3 * gain)
# Riser into the drop.
n = at(2 * BEAT + 3 * BEAT)
r = rng.standard_normal(n) * np.linspace(0, 1, n) ** 2
add(beat_time(8, 1) - 2 * BEAT, np.diff(r, prepend=0) * (np.arange(n) < at(4 * BEAT)), 0.25)

out = np.tanh(out * 0.9)
out /= np.max(np.abs(out)) + 1e-9
stereo = np.stack([out, out], axis=1)
pcm = (stereo * 0.9 * 32767).astype(np.int16)
with wave.open(sys.argv[1], "wb") as w:
    w.setnchannels(2)
    w.setsampwidth(2)
    w.setframerate(SR)
    w.writeframes(pcm.tobytes())
print(f"{LENGTH:.2f} s")
