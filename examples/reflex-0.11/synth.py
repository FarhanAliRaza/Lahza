import sys

import numpy as np
from scipy import signal

SR = 48000
BPM = 120.0
BEAT = 60.0 / BPM
BAR = 4 * BEAT
LENGTH = 47.0
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

