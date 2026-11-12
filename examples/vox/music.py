"""An original, quiet explainer bed for the cancer short.

Minor and pensive (Dm–B♭–F–C) under the problem, lifting to major
(F–C–Dm–B♭ with a brighter pluck and a soft pulse) where the narration
turns hopeful. Reads the timing from vo.json.

    python3 music.py OUT_DIR
"""
import json
import os
import sys

import numpy as np
import soundfile as sf

SR = 44100
rng = np.random.default_rng(3)


def notes(names):
    base = {"C": 0, "D": 2, "E": 4, "F": 5, "G": 7, "A": 9, "B": 11}
    out = []
    for n in names:
        semis = base[n[0]] + (-1 if "b" in n[1:-1] else 0) + 12 * (int(n[-1]) + 1)
        out.append(440.0 * 2 ** ((semis - 69) / 12))
    return out


def env(n, attack, release):
    t = np.arange(n) / SR
    a = np.minimum(1.0, t / attack)
    r = np.minimum(1.0, (n / SR - t) / release)
    return a * np.clip(r, 0.0, 1.0)


def pad(freqs, dur, gain):
    n = int(dur * SR)
    t = np.arange(n) / SR
    s = np.zeros(n)
    for f in freqs:
        for detune in (-0.6, 0.6):
            ff = f + detune
            # A soft saw: few partials, gentle.
            s += sum(np.sin(2 * np.pi * ff * k * t + rng.uniform(0, 6.28)) / k ** 1.6 for k in range(1, 6))
    return s * env(n, 1.2, 1.4) * gain / len(freqs)


def pluck(f, dur, gain, bright=0.5):
    n = int(dur * SR)
    t = np.arange(n) / SR
    s = sum(np.sin(2 * np.pi * f * k * t) * (bright ** (k - 1)) / k for k in range(1, 7))
    return s * np.exp(-t * 3.2) * np.minimum(1.0, t / 0.004) * gain


def kick(gain):
    n = int(0.35 * SR)
    t = np.arange(n) / SR
    f = 42 + 70 * np.exp(-t * 30)
    return np.sin(2 * np.pi * np.cumsum(f) / SR) * np.exp(-t * 9) * gain


def main():
    out = sys.argv[1]
    vo = json.load(open(os.path.join(out, "vo.json")))
    length = vo["duration"]
    hope = next(l["start"] for l in vo["lines"] if l["scene"] == "hope")
    bpm = 84.0
    beat = 60.0 / bpm
    bar = 4 * beat
    mix = np.zeros(int((length + 2) * SR))

    def put(t, sig):
        i = int(t * SR)
        j = min(len(mix), i + len(sig))
        if j > i:
            mix[i:j] += sig[: j - i]

    sad = [notes(["D3", "F3", "A3", "D4"]), notes(["Bb2", "D3", "F3", "Bb3"]), notes(["F2", "A2", "C3", "F3"]), notes(["C3", "E3", "G3", "C4"])]
    lift = [notes(["F2", "A2", "C3", "F3"]), notes(["C3", "E3", "G3", "C4"]), notes(["D3", "F3", "A3", "D4"]), notes(["Bb2", "D3", "F3", "Bb3"])]
    # Arpeggio patterns (indices into the chord, an octave up).
    arp = [0, 2, 1, 3, 2, 1, 3, 2]
    t = 0.0
    k = 0
    while t < length:
        hopeful = t >= hope - 0.2
        chords = lift if hopeful else sad
        chord = chords[k % 4]
        put(t, pad(chord, bar + 1.2, 0.05 if not hopeful else 0.06))
        for i, a in enumerate(arp):
            f = chord[a] * 2
            put(t + i * beat / 2, pluck(f, 1.4, 0.05 if not hopeful else 0.07, 0.45 if not hopeful else 0.6))
        # A deep root on each bar, and a soft pulse once things turn.
        put(t, pluck(chord[0] / 2, 2.5, 0.08, 0.2))
        if hopeful:
            for b in range(4):
                put(t + b * beat, kick(0.18))
        t += bar
        k += 1
    # Fade in, and out at the end.
    n = len(mix)
    tt = np.arange(n) / SR
    mix *= np.minimum(1.0, tt / 1.5) * np.clip((length + 1.5 - tt) / 2.5, 0.0, 1.0)
    # Gentle saturation and a little stereo width.
    mix = np.tanh(mix * 1.6) / 1.6
    delay = int(0.012 * SR)
    stereo = np.stack([mix, np.concatenate([np.zeros(delay), mix[:-delay]])], axis=1) * 0.9
    sf.write(os.path.join(out, "music.wav"), stereo, SR)
    print(f"music.wav {len(mix) / SR:.1f}s, lift at {hope:.2f}s")


if __name__ == "__main__":
    main()
