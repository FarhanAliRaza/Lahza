"""Score for the Lahza showreel: 120 BPM, A minor, 25 bars + tail.

Every event sits on the beat grid the edit uses (beat = 0.5 s, bar = 2 s),
so the markers written alongside are exact, not detected.

Sections (bar numbers 1-based, bar b starts at (b - 1) * 2 s):
  1-2   intro      clock ticks (quarters, then eighths), pad swell, riser
  3-4   title      impact on 4.0, half-time kick, bass enters on bar 4
  5-8   groove     four on the floor, claps, hats, bass, soft arp; bar 8 builds, beat 4 drops out
  9-13  drop       impact on 16.0, everything, bright arp
  14-17 space      breakdown: pad, plucked melody, ticks; bar 17 builds
  18-20 sync       impact on 34.0, full groove
  21-22 montage    a stab and a kick on every beat (one cut per beat); last beat silent
  23-25 end        impact on 44.0, the chord resolves, ticks fade; tail to 51 s
"""
import json
import sys

import numpy as np
from scipy import signal

SR = 48000
BPM = 120.0
BEAT = 60.0 / BPM
BAR = 4 * BEAT
LENGTH = 51.0
N = int(LENGTH * SR)
rng = np.random.default_rng(7)


def bus():
    return np.zeros((2, N))


def place(dst, src, t, gain=1.0, pan=0.0):
    """Adds mono or stereo `src` at time t (s) with equal-power pan."""
    i = int(round(t * SR))
    if i >= N:
        return
    if src.ndim == 1:
        a = (pan + 1) * np.pi / 4
        src = np.stack([src * np.cos(a), src * np.sin(a)]) * np.sqrt(2)
    n = min(src.shape[1], N - i)
    if i < 0:
        src = src[:, -i:]
        n = min(src.shape[1], N)
        i = 0
    dst[:, i:i + n] += gain * src[:, :n]


def env(n, attack=0.003, decay=None, release=0.01):
    t = np.arange(n) / SR
    e = np.minimum(1.0, t / max(attack, 1e-6))
    if decay:
        e *= np.exp(-t / decay)
    r = int(release * SR)
    if r > 0 and r < n:
        e[-r:] *= np.linspace(1, 0, r)
    return e


def bandpass(x, lo, hi, order=2):
    sos = signal.butter(order, [lo, hi], btype="band", fs=SR, output="sos")
    return signal.sosfilt(sos, x)


def highpass(x, f, order=2):
    return signal.sosfilt(signal.butter(order, f, btype="high", fs=SR, output="sos"), x)


def lowpass(x, f, order=2):
    return signal.sosfilt(signal.butter(order, f, btype="low", fs=SR, output="sos"), x)


def hz(midi):
    return 440.0 * 2 ** ((midi - 69) / 12)


# ---------------------------------------------------------------- voices

def kick(level=1.0):
    n = int(0.55 * SR)
    t = np.arange(n) / SR
    f = 46 + 120 * np.exp(-t / 0.035)
    ph = 2 * np.pi * np.cumsum(f) / SR
    body = np.sin(ph) * np.exp(-t / 0.32)
    body = np.tanh(1.6 * body) / np.tanh(1.6)
    click = highpass(rng.standard_normal(n), 2500) * np.exp(-t / 0.004) * 0.25
    return level * (body + click) * env(n, 0.001, None, 0.02)


def clap():
    n = int(0.45 * SR)
    t = np.arange(n) / SR
    noise = bandpass(rng.standard_normal(n), 900, 4200)
    e = np.zeros(n)
    for k, d in enumerate([0.0, 0.011, 0.022]):
        i = int(d * SR)
        e[i:] += np.exp(-(t[: n - i]) / (0.008 if k < 2 else 0.16))
    body = np.sin(2 * np.pi * 190 * t) * np.exp(-t / 0.05) * 0.3
    return (noise * e * 0.6 + body) * env(n, 0.0005, None, 0.02)


def hat(open_=False):
    n = int((0.32 if open_ else 0.06) * SR)
    t = np.arange(n) / SR
    x = highpass(rng.standard_normal(n), 7500, 4)
    # A little metal: a few inharmonic square partials.
    for f in (5400, 7100, 8900, 10600):
        x += 0.15 * np.sign(np.sin(2 * np.pi * f * t + rng.uniform(0, 6)))
    x = highpass(x, 6000)
    return x * env(n, 0.0005, 0.09 if open_ else 0.018, 0.01) * 0.17


def tick(pitch=1.0):
    """A clock tick: a woodblock-like resonance over a short click."""
    n = int(0.12 * SR)
    t = np.arange(n) / SR
    tone = np.sin(2 * np.pi * 1850 * pitch * t) * np.exp(-t / 0.018)
    tone += 0.5 * np.sin(2 * np.pi * 3170 * pitch * t) * np.exp(-t / 0.009)
    click = bandpass(rng.standard_normal(n), 2000, 9000) * np.exp(-t / 0.002)
    return (0.55 * tone + 0.35 * click) * env(n, 0.0003, None, 0.01) * 0.45


def saw(f, n, detune_cents=0.0, harmonics_to=6000.0, phase=0.0):
    t = np.arange(n) / SR
    f = f * 2 ** (detune_cents / 1200)
    x = np.zeros(n)
    k = 1
    while k * f < harmonics_to and k < 80:
        x += np.sin(2 * np.pi * k * f * t + phase * k) / k
        k += 1
    return x * 0.6


def pad_chord(notes, dur, bright=1800):
    n = int(dur * SR)
    out = np.zeros((2, n))
    for m in notes:
        for side, cents in ((0, -7), (1, 7)):
            v = saw(hz(m), n, cents, 4000, rng.uniform(0, 6)) + saw(hz(m), n, -cents * 0.5, 4000, rng.uniform(0, 6))
            out[side] += v
    out = np.stack([lowpass(c, bright, 2) for c in out])
    e = env(n, 0.9, None, 0.9)
    return out * e * (0.09 / max(1, len(notes) / 3))


def pluck(f, dur=0.45, bright=1.0):
    n = int(dur * SR)
    t = np.arange(n) / SR
    x = np.zeros(n)
    k = 1
    while k * f < 9000 and k < 40:
        x += np.sin(2 * np.pi * k * f * t) / k * np.exp(-t * (4 + k * 2.2 / bright))
        k += 1
    return x * env(n, 0.002, None, 0.03) * 0.5


def bass(f, dur):
    n = int(dur * SR)
    t = np.arange(n) / SR
    x = 0.8 * np.sin(2 * np.pi * f * t) + saw(f, n, 0, 900) * 0.6
    x = lowpass(x, 420, 2)
    x = np.tanh(1.8 * x)
    return x * env(n, 0.004, None, 0.03) * 0.42


def stab(notes, dur=0.4):
    n = int(dur * SR)
    out = np.zeros((2, n))
    for m in notes:
        out[0] += saw(hz(m), n, -9, 7000)
        out[1] += saw(hz(m), n, 9, 7000)
    t = np.arange(n) / SR
    cutoff = lowpass(out[0], 3500)  # warm the attack a little
    out[0] = 0.6 * out[0] + 0.4 * cutoff
    return out * np.exp(-t / 0.11) * env(n, 0.002, None, 0.02) * 0.6


def riser(dur, start_hz=300, end_hz=9000):
    n = int(dur * SR)
    x = rng.standard_normal(n)
    # Time-varying band-pass (state variable filter), centre rising exponentially.
    fc = start_hz * (end_hz / start_hz) ** (np.arange(n) / n)
    y = np.zeros(n)
    low = band = 0.0
    q = 0.35
    for i in range(n):
        f = 2 * np.sin(np.pi * fc[i] / SR)
        high = x[i] - low - q * band
        band += f * high
        low += f * band
        y[i] = band
    t = np.arange(n) / SR
    tone = np.sin(2 * np.pi * np.cumsum(200 * (8 ** (t / dur))) / SR) * 0.15
    e = (np.arange(n) / n) ** 2.2
    return (y * 0.5 + tone) * e * env(n, 0.01, None, 0.004)


def reverse_cymbal(dur):
    n = int(dur * SR)
    x = highpass(rng.standard_normal(n), 4000)
    return x * (np.arange(n) / n) ** 3 * 0.35 * env(n, 0.01, None, 0.003)


def impact(size=1.0):
    n = int(2.8 * SR)
    t = np.arange(n) / SR
    f = 32 + 70 * np.exp(-t / 0.12)
    sub = np.sin(2 * np.pi * np.cumsum(f) / SR) * np.exp(-t / (0.9 * size))
    crack = lowpass(rng.standard_normal(n), 3000) * np.exp(-t / 0.05) * 0.5
    air = highpass(rng.standard_normal(n), 3000) * np.exp(-t / 0.6) * 0.06
    x = np.tanh(1.4 * (sub + crack)) + air
    return x * env(n, 0.001, None, 0.3) * 0.8 * size


def downlifter(dur):
    n = int(dur * SR)
    t = np.arange(n) / SR
    tone = np.sin(2 * np.pi * np.cumsum(1400 * (0.05 ** (t / dur))) / SR)
    noise = lowpass(rng.standard_normal(n), 2500)
    return (0.25 * tone + 0.2 * noise) * np.exp(-t / (dur / 3)) * env(n, 0.005, None, 0.05)


def reverb_ir(seconds=2.4, damp=5000):
    n = int(seconds * SR)
    t = np.arange(n) / SR
    ir = np.stack([rng.standard_normal(n), rng.standard_normal(n)])
    decay = np.exp(-t / (seconds / 6.9))
    ir = np.stack([lowpass(c, damp, 1) for c in ir]) * decay
    pre = int(0.018 * SR)
    ir = np.concatenate([np.zeros((2, pre)), ir], axis=1)
    return ir / np.sqrt(np.sum(ir ** 2) / 2)


def reverb(x, ir):
    out = np.stack([signal.fftconvolve(x[0] + x[1], ir[c])[:N] for c in range(2)])
    return out * 0.5


# ---------------------------------------------------------------- arrangement

def bar_t(b, beat=1.0):
    """Time of `beat` (1-based, fractional) in bar b (1-based)."""
    return (b - 1) * BAR + (beat - 1) * BEAT


# A minor: Am - F - C - G, two bars each, from bar 1.
CHORDS = [[57, 60, 64, 69], [53, 57, 60, 65], [55, 60, 64, 67], [55, 59, 62, 67]]
ROOTS = [45, 41, 48, 43]


def chord_at(b):
    return (b - 1) // 2 % 4


drums, perc, music, fx, send = bus(), bus(), bus(), bus(), bus()
TICKS = []
kicks = []  # times, for side-chain ducking

# Intro ticks: quarters in bar 1, eighths in bar 2 (time speeding up).
for b, step in ((1, 1.0), (2, 0.5)):
    k = 0.0
    while k < 4:
        accent = (k % 1) == 0
        place(perc, tick(1.0 if accent else 1.33), bar_t(b, 1 + k), 0.55 if accent else 0.32, pan=-0.25 if accent else 0.25)
        TICKS.append(bar_t(b, 1 + k))
        k += step

# Ticks under the title and the breakdown, and the end.
for b in (3, 4, 14, 15, 16, 17):
    for q in range(4):
        place(perc, tick(1.0 if q == 0 else 1.33), bar_t(b, 1 + q), 0.22 if q else 0.3, pan=-0.3 if q % 2 else 0.3)
        TICKS.append(bar_t(b, 1 + q))
for i, b in enumerate([23, 23, 23, 23, 24, 24, 24, 24, 25]):
    q = i % 4
    place(perc, tick(1.0 if q == 0 else 1.33), bar_t(b, 1 + q), 0.3 * 0.82 ** i, pan=0.3 if i % 2 else -0.3)
    TICKS.append(bar_t(b, 1 + q))

# Pads: every bar from 1 to 22, then the resolve.
for b in range(1, 23, 2):
    if b in (21,):
        continue
    gain = 0.7 if b < 3 else (1.0 if b < 9 else 0.85)
    if 14 <= b <= 17:
        gain = 0.75
    place(music, pad_chord(CHORDS[chord_at(b)], 2 * BAR + 0.6, 1400 if b < 9 else 3200), bar_t(b), gain)
place(music, pad_chord([45, 57, 59, 60, 64, 71], 7.5, 1600), bar_t(23), 1.3)  # Am(add9) resolve

# Risers into each impact.
place(fx, riser(1.6), 4.0 - 1.6, 0.55)
place(fx, reverse_cymbal(1.4), 4.0 - 1.4, 0.6)
place(fx, riser(1.9), 16.0 - 1.9, 0.6)
place(fx, reverse_cymbal(1.0), 16.0 - 1.0, 0.6)
place(fx, riser(1.8), 34.0 - 1.8, 0.5)
place(fx, riser(1.4), 44.0 - 1.9, 0.6)
place(fx, reverse_cymbal(0.5), 44.0 - 0.5, 0.7)

# Impacts.
for t, s in ((4.0, 1.0), (16.0, 1.1), (34.0, 0.9), (44.0, 1.25)):
    place(fx, impact(s), t, 0.9)
    place(fx, downlifter(1.6), t + 0.02, 0.35)
    place(send, impact(s) * 0.4, t)
    kicks.append(t)

# Drums.
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

# Title: half-time kick, bar 4 hats.
for b in (3, 4):
    for q in (0, 2):
        if b == 3 and q == 0:
            continue  # the impact is the downbeat
        place(drums, kick(), bar_t(b, 1 + q), 0.85)
        kicks.append(bar_t(b, 1 + q))
for s in range(16):
    place(perc, hat(False), bar_t(4) + s * BEAT / 4, (0.18, 0.07, 0.12, 0.07)[s % 4], pan=-0.25)

for b in range(5, 9):
    groove(b, full=b >= 6)
# Bar 8: snare roll accelerating into the drop; the last beat drops out.
for i, d in enumerate(np.cumsum([0.25] * 4 + [0.125] * 8 + [0.0625] * 8)):
    t = bar_t(8) + d - 0.25
    if t < bar_t(8, 4):
        place(drums, clap(), t, 0.12 + 0.3 * i / 20, pan=0.1)
for b in range(9, 14):
    groove(b, hats16=True)
for b in (17,):
    groove(b, full=False, clapit=False)
for i, d in enumerate(np.cumsum([0.125] * 8 + [0.0625] * 8)):
    t = bar_t(17, 3) + d - 0.125
    if t < 34.0 - 0.02:
        place(drums, clap(), t, 0.1 + 0.3 * i / 16)
for b in range(18, 21):
    groove(b, hats16=True)
# Montage: a kick and a stab on every beat; the last beat drops out.
for b in (21, 22):
    for q in range(4):
        t = bar_t(b, 1 + q)
        if t >= 43.5:
            continue
        place(drums, kick(), t, 1.0)
        kicks.append(t)
        place(music, stab(CHORDS[(q + 2 * (b - 21)) % 4]), t, 1.0)
        place(send, stab(CHORDS[(q + 2 * (b - 21)) % 4]), t, 0.6)
        place(perc, hat(True), t + BEAT / 2, 0.22, pan=0.3)

# Bass.
for b in list(range(4, 14)) + list(range(18, 21)):
    root = hz(ROOTS[chord_at(b)])
    if b == 4:
        place(music, bass(root, BAR - 0.05), bar_t(b), 0.8)
        continue
    sixteenths = range(16) if b >= 9 else range(0, 16, 2)
    for s in sixteenths:
        if b >= 9 and s % 4 == 0:
            continue  # leave the kick alone, roll the off-beats
        f = root * (2 if s % 8 == 6 and b >= 9 else 1)
        place(music, bass(f, BEAT / 4 * 0.9 if b >= 9 else BEAT / 2 * 0.85), bar_t(b) + s * BEAT / 4, 0.75)
for b in range(14, 18):
    place(music, bass(hz(ROOTS[chord_at(b)]), BAR * 0.95), bar_t(b), 0.6)
place(music, bass(hz(33), 6.0), bar_t(23), 0.7)

# Arp: chord tones an octave up, 16ths; soft in the groove, bright in the drops.
for b in list(range(6, 14)) + list(range(18, 21)):
    tones = [m + 12 for m in CHORDS[chord_at(b)]]
    pattern = [0, 1, 2, 3, 2, 1, 3, 2]
    for s in range(16):
        m = tones[pattern[s % 8]] + (12 if s % 8 == 4 and b >= 9 else 0)
        bright = 0.5 if b < 9 else 1.4
        place(music, pluck(hz(m), 0.32, bright), bar_t(b) + s * BEAT / 4, 0.3 if b < 9 else 0.5, pan=0.45 * np.sin(s * 0.7))
        place(send, pluck(hz(m), 0.32, bright), bar_t(b) + s * BEAT / 4, 0.12)

# Breakdown melody: plucked, sparse, long reverb.
MELODY = [(14, 1, 76), (14, 2.5, 74), (14, 3.5, 72), (15, 1, 72), (15, 3, 69), (16, 1, 71), (16, 2.5, 72), (16, 3.5, 74), (17, 1, 76), (17, 3, 79)]
for b, beat, m in MELODY:
    place(music, pluck(hz(m), 1.0, 2.0), bar_t(b, beat), 0.4, pan=0.2)
    place(send, pluck(hz(m), 1.0, 2.0), bar_t(b, beat), 0.5)
# A last bell-like pluck with the logo.
for m, d in ((69, 0), (76, 0.02), (81, 0.04)):
    place(music, pluck(hz(m), 3.0, 3.0), 44.0 + d, 0.3)
    place(send, pluck(hz(m), 3.0, 3.0), 44.0 + d, 0.45)

# ---------------------------------------------------------------- mix

ir = reverb_ir(2.6)
wet = reverb(send + 0.25 * music + 0.15 * perc, ir)

# Side-chain: duck music and wet under every kick.
duck = np.ones(N)
t_ = np.arange(int(0.3 * SR)) / SR
shape = 1 - 0.7 * np.exp(-t_ / 0.09)
for k in kicks:
    i = int(k * SR)
    n = min(len(shape), N - i)
    if n > 0:
        duck[i:i + n] = np.minimum(duck[i:i + n], shape[:n])
section = np.ones(N)
section[int(8 * SR):int(16 * SR)] = 0.72  # the groove sits under the drop
mix = drums * section * 0.25 + perc * 1.7 + (music * duck) * 0.56 + fx * 0.42 + wet * duck * 0.33

# Glue: gentle bus saturation, low cut, then peak-normalise to -1 dBFS.
mix = np.stack([highpass(c, 28, 2) for c in mix])
# Fade the tail.
fade = int(1.5 * SR)
mix[:, -fade:] *= np.linspace(1, 0, fade) ** 2
# Loudness: aim the loud sections at about -14 LUFS, then a soft limiter
# keeps peaks under -1 dBFS.
loud = mix[:, int(16 * SR):int(26 * SR)]
mix *= 10 ** (-13.5 / 20) / np.sqrt(np.mean(loud ** 2))
ceiling = 10 ** (-1.2 / 20)
mix = ceiling * np.tanh(mix / ceiling)
peak = np.max(np.abs(mix)) / ceiling

out = sys.argv[1] if len(sys.argv) > 1 else "track.wav"
from scipy.io import wavfile
wavfile.write(out, SR, (mix.T * 32767).astype(np.int16))

# Exact markers for the project.
beats = [{"t": round(i * BEAT, 4)} for i in range(int(LENGTH / BEAT))]
bars = [{"t": round(i * BAR, 4)} for i in range(int(LENGTH / BAR) + 1)]
hits = [{"t": t, "label": l} for t, l in ((4.0, "title"), (16.0, "drop"), (34.0, "sync"), (44.0, "end"))]
ticks = [{"t": round(t, 4)} for t in sorted(set(TICKS))]
json.dump({"beat": beats, "bar": bars, "hit": hits, "tick": ticks}, open(out.replace(".wav", "-markers.json"), "w"))
print(out, f"{LENGTH:.1f}s peak {20*np.log10(peak):.1f} dB pre-normalise, rms {20*np.log10(np.sqrt(np.mean(mix**2))):.1f} dBFS")
