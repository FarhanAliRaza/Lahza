"""Narration for "Why is cancer so hard to cure?": one clip per line,
joined with pauses into vo.wav, with each line's timing in vo.json.

    python3 narrate.py OUT_DIR KOKORO_MODEL KOKORO_VOICES

Uses Kokoro (Apache-2.0, https://github.com/thewh1teagle/kokoro-onnx);
lines already rendered are reused.
"""
import json
import os
import sys

import numpy as np
import soundfile as sf

# (scene, line). A pause follows each line: short inside a scene, longer
# between scenes.
SCRIPT = [
    ("hook", "In 1971, America declared war on cancer."),
    ("hook", "Fifty years and billions of dollars later, there's still no single cure."),
    ("hook", "Why?"),
    ("many", "First, cancer isn't one disease."),
    ("many", "It's more than two hundred, each with its own rules."),
    ("self", "And unlike a virus, cancer isn't an invader."),
    ("self", "It's made of your own cells, so it's hard to hit without hitting you."),
    ("typo", "Every time a cell divides, it copies three billion letters of DNA."),
    ("typo", "A few typos in the wrong genes, and the brakes on growth fail."),
    ("typo", "One cell becomes two, then four, then billions."),
    ("crowd", "Worse, a tumor isn't a copy of one cell. It's a crowd of different ones."),
    ("crowd", "A drug can kill most of them,"),
    ("crowd", "but the few that resist survive, and grow back."),
    ("spread", "And cancer moves. Cells break away and travel through the blood."),
    ("spread", "Most cancer deaths come from tumors that have spread."),
    ("hope", "But this isn't a war we're losing."),
    ("hope", "In the US, five-year survival has climbed from forty-nine percent in the mid-seventies to sixty-nine percent."),
    ("end", "There won't be one cure for cancer. There will be many."),
]
VOICE = "af_heart"
SPEED = 1.0
LEAD = 0.6
PAUSE_LINE = 0.35
PAUSE_SCENE = 0.8
TAIL = 3.2


def main():
    out, model, voices = sys.argv[1], sys.argv[2], sys.argv[3]
    os.makedirs(os.path.join(out, "vo"), exist_ok=True)
    from kokoro_onnx import Kokoro

    kokoro = None
    clips = []
    for i, (scene, text) in enumerate(SCRIPT):
        path = os.path.join(out, "vo", f"{i:02d}.wav")
        stamp = path + ".txt"
        if not (os.path.exists(path) and open(stamp).read() == f"{VOICE}|{SPEED}|{text}"):
            kokoro = kokoro or Kokoro(model, voices)
            samples, sr = kokoro.create(text, voice=VOICE, speed=SPEED, lang="en-us")
            sf.write(path, samples, sr)
            open(stamp, "w").write(f"{VOICE}|{SPEED}|{text}")
            print(f"  line {i:2d} {len(samples) / sr:5.2f}s  {text}")
        audio, sr = sf.read(path)
        # Trim leading/trailing near-silence so pauses are ours.
        loud = np.flatnonzero(np.abs(audio) > 0.01)
        audio = audio[max(0, loud[0] - int(0.03 * sr)): loud[-1] + int(0.08 * sr)]
        clips.append((scene, text, audio, sr))
    sr = clips[0][3]
    t = LEAD
    lines = []
    pieces = [np.zeros(int(LEAD * sr))]
    for i, (scene, text, audio, _) in enumerate(clips):
        lines.append({"scene": scene, "text": text, "start": round(t, 3), "end": round(t + len(audio) / sr, 3)})
        pieces.append(audio)
        t += len(audio) / sr
        nxt = clips[i + 1][0] if i + 1 < len(clips) else None
        pause = TAIL if nxt is None else PAUSE_LINE if nxt == scene else PAUSE_SCENE
        pieces.append(np.zeros(int(pause * sr)))
        t += pause
    vo = np.concatenate(pieces)
    sf.write(os.path.join(out, "vo.wav"), vo, sr)
    json.dump({"duration": round(len(vo) / sr, 3), "lines": lines}, open(os.path.join(out, "vo.json"), "w"), indent=1)
    print(f"vo.wav {len(vo) / sr:.1f}s, {len(lines)} lines")


if __name__ == "__main__":
    main()
