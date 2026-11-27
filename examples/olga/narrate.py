"""Narration for "Olga of Kyiv" with word timings, as a Lahza narration
manifest.

    python3 narrate.py OUT_DIR KOKORO_MODEL KOKORO_VOICES [--voice bm_george] [--speed 1.25]

Kokoro (Apache-2.0) speaks each line. The int8 model gives no timings, so
each word's time is estimated: the line's speech is spread over its words by
their phoneme counts, anchored to the pauses actually heard in the audio at
commas and full stops. Writes OUT_DIR/vo/<id>.wav and OUT_DIR/manifest.json.
"""
import argparse
import json
import os
import re

import numpy as np
import soundfile as sf

CUE = re.compile(r"\[/?[A-Za-z0-9_-]+\]")

SCRIPT = [
    ("l01", "Twenty men sit in a [boat]boat at the bottom of a [pit]pit, and the first shovel of [dirt]dirt comes down."),
    ("l02", "The woman watching is [olga]Olga, princess of Kyiv. The year is [year]945."),
    ("l03", "Weeks ago, these men's tribe, the [drev]Drevlians, killed her husband, Prince [igor]Igor, for coming back to take their [tribute]tribute twice."),
    ("l04", "Now they want her to marry their own prince, [mal]Mal."),
    ("l05", "So Olga [smile]smiles. Stay in your boat, she says. My people will [carry]carry you in, as an [honour]honour."),
    ("l06", "They carry the boat straight into a [pit2]pit."),
    ("l07", "She leans over the edge and asks if the honour [pleases]pleases them. Then the [dirt2]dirt comes down."),
    ("l08", "The Drevlians never [hear]hear of it. So she [writes]writes again: send me your [best]best men."),
    ("l09", "They send their best. She offers them a [bath]bathhouse to wash off the road. She [bars]bars the door. She [burns]burns it."),
    ("l10", "At Igor's [grave]grave she holds a funeral [feast]feast. The Drevlians [drink]drink with her. [five]Five thousand of them never get [up]up."),
    ("l11", "Their city, [isk]Iskorosten, still holds out. So Olga offers [peace]peace, at a tiny price: three [pigeons]pigeons and three [sparrows]sparrows from every house."),
    ("l12", "Her soldiers tie [sulphur]sulphur and rags to every bird and [light]light them at dusk."),
    ("l13", "The birds fly [home]home, to their nests under dry [eaves]eaves. Every house [catches]catches at once."),
    ("l14", "Later, the Church made Olga a [saint]saint."),
]

# How the voice should say a shown word (the captions keep the spelling).
SAY = {"Kyiv": "Keev", "Kyiv.": "Keev.", "945.": "nine forty-five.", "Drevlians": "Drevleeans", "Drevlians,": "Drevleeans,",
       "Iskorosten,": "Iskoros-ten,", "Mal.": "Mahl.", "Igor,": "Eegor,", "Igor's": "Eegor's", "honour": "honor", "honour.": "honor."}

PAUSE_AFTER = set(",.:;!?")


def words_of(text):
    return CUE.sub("", text).split()


def phoneme_weight(tok, word):
    ph = tok.phonemize(word, "en-gb") if word.strip(".,:;!?") else ""
    sounds = [c for c in ph if c.isalpha() or c in "əɪʊɛæɑɔʌɜɐːθðʃʒŋ"]
    return max(1.0, len(sounds)) + 1.2


def pauses(audio, sr, min_len=0.07, thresh=0.012):
    """Silent stretches inside the speech: [(start, end)] seconds."""
    hop = int(0.01 * sr)
    rms = np.array([np.sqrt(np.mean(audio[i:i + hop] ** 2)) for i in range(0, len(audio) - hop, hop)])
    quiet = rms < thresh
    out, i = [], 0
    while i < len(quiet):
        if quiet[i]:
            j = i
            while j < len(quiet) and quiet[j]:
                j += 1
            if (j - i) * 0.01 >= min_len and i > 0 and j < len(quiet):
                out.append((i * 0.01, j * 0.01))
            i = j
        else:
            i += 1
    return out


def time_words(shown, weights, dur, gaps):
    """Each word's (start, end): speech time spread by weight, anchored to the
    heard pauses at punctuation."""
    n = len(shown)
    total = sum(weights)
    # Words after which a pause is expected, and when (by weight).
    cum = np.cumsum(weights) / total * dur
    anchors = {}  # word index -> (pause start, pause end)
    used = set()
    for i, w in enumerate(shown[:-1]):
        if w[-1] in PAUSE_AFTER:
            best, bd = None, 0.45
            for k, (a, b) in enumerate(gaps):
                if k in used:
                    continue
                d = abs((a + b) / 2 - cum[i])
                if d < bd:
                    best, bd = k, d
            if best is not None:
                used.add(best)
                anchors[i] = gaps[best]
    out = [None] * n
    seg_start, t0 = 0, 0.0
    bounds = sorted(anchors.items()) + [(n - 1, (dur, dur))]
    for last, (pa, pb) in bounds:
        ws = weights[seg_start:last + 1]
        span = pa - t0
        t = t0
        for k, w in enumerate(ws):
            d = span * w / sum(ws)
            out[seg_start + k] = (round(t, 3), round(t + d, 3))
            t += d
        seg_start, t0 = last + 1, pb
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("out")
    ap.add_argument("model")
    ap.add_argument("voices")
    ap.add_argument("--voice", default="bm_george")
    ap.add_argument("--speed", type=float, default=1.25)
    args = ap.parse_args()
    from kokoro_onnx import Kokoro
    kokoro = Kokoro(args.model, args.voices)
    os.makedirs(os.path.join(args.out, "vo"), exist_ok=True)
    clips = []
    total_words, total_time = 0, 0.0
    for cid, text in SCRIPT:
        shown = words_of(text)
        said = [SAY.get(w, w) for w in shown]
        spoken = " ".join(said)
        path = os.path.join(args.out, "vo", f"{cid}.wav")
        stamp = path + ".txt"
        key = f"{args.voice}|{args.speed}|{spoken}"
        if not (os.path.exists(path) and os.path.exists(stamp) and open(stamp).read() == key):
            audio, sr = kokoro.create(spoken, voice=args.voice, speed=args.speed, lang="en-gb")
            loud = np.flatnonzero(np.abs(audio) > 0.01)
            audio = audio[max(0, loud[0] - int(0.02 * sr)): loud[-1] + int(0.06 * sr)]
            sf.write(path, audio, sr)
            open(stamp, "w").write(key)
        audio, sr = sf.read(path)
        dur = len(audio) / sr
        weights = [phoneme_weight(kokoro.tokenizer, w) for w in said]
        times = time_words(shown, weights, dur, pauses(audio, sr))
        words = [{"text": w, "start": a, "end": b} for w, (a, b) in zip(shown, times)]
        clips.append({"id": cid, "audio": f"vo/{cid}.wav", "text": text, "words": words})
        total_words += len(shown)
        total_time += dur
        print(f"  {cid} {dur:5.2f}s {len(shown) / dur:4.2f} w/s  {' '.join(shown)}")
    manifest = {"lead": 0.15, "pause": 0.12, "clips": clips}
    json.dump(manifest, open(os.path.join(args.out, "manifest.json"), "w"), indent=1)
    film = 0.15 + total_time + 0.12 * (len(clips) - 1)
    print(f"{total_words} words, speech {total_time:.1f}s, film ≈ {film:.1f}s, {total_words / film:.2f} words/s")


if __name__ == "__main__":
    main()
