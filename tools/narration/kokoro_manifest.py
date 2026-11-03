"""Kokoro narration with word timings, as a Lahza narration manifest.

    python3 kokoro_manifest.py SCRIPT.json OUT_DIR MODEL.onnx VOICES.bin [--voice af_heart] [--speed 1.0]

SCRIPT.json is {"lines": [{"id": "hook", "text": "One bomb. Over [strike]Iran."}, ...]}
(cues in square brackets are kept for the engine and not spoken). Writes
OUT_DIR/<id>.wav per line and OUT_DIR/manifest.json for `narration_import`.

Timings come from Kokoro's own phoneme durations (`create_timed`). That
needs a model export with a `duration` output, such as the
"Kokoro-82M-v1.0-ONNX-timestamped" export; `kokoro-v1.0.int8.onnx` only
outputs audio, and this script stops rather than guess. Lines already
synthesised with the same text, voice and speed are reused.
"""
import argparse
import hashlib
import json
import os
import re
import sys

import numpy as np
import soundfile as sf

CUE = re.compile(r"\[/?[A-Za-z0-9_-]+\]")


def spoken(text):
    """The script without cue markup."""
    return " ".join(CUE.sub(" ", text).split())


def phoneme_words(timings):
    """Phoneme timings grouped into words at the spaces between them."""
    words, cur = [], None
    for t in timings:
        if t.phoneme.isspace():
            if cur:
                words.append(cur)
            cur = None
            continue
        if cur is None:
            cur = [t.phoneme, t.start, t.end]
        else:
            cur[0] += t.phoneme
            cur[2] = t.end
    if cur:
        words.append(cur)
    # Punctuation is spoken as silence: it is not a word of its own.
    return [w for w in words if re.search(r"\w", w[0])]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("script")
    ap.add_argument("out")
    ap.add_argument("model")
    ap.add_argument("voices")
    ap.add_argument("--voice", default="af_heart")
    ap.add_argument("--speed", type=float, default=1.0)
    ap.add_argument("--lang", default="en-us")
    args = ap.parse_args()
    from kokoro_onnx import Kokoro

    kokoro = Kokoro(args.model, args.voices)
    if not getattr(kokoro, "has_timings", True):
        sys.exit(f"{args.model} has no duration output: use a timestamped export, or another voice with timings")
    lines = json.load(open(args.script))["lines"]
    os.makedirs(args.out, exist_ok=True)
    clips = []
    for line in lines:
        text = spoken(line["text"])
        wav = os.path.join(args.out, f"{line['id']}.wav")
        stamp = hashlib.sha256(f"{args.voice}|{args.speed}|{text}".encode()).hexdigest()
        cached = wav + ".json"
        if os.path.exists(wav) and os.path.exists(cached) and json.load(open(cached)).get("stamp") == stamp:
            words = json.load(open(cached))["words"]
        else:
            # Phonemise once, so the timings index exactly these phonemes.
            phonemes = kokoro.tokenizer.phonemize(text, args.lang)
            audio, sr, timings = kokoro.create_timed(phonemes, voice=args.voice, speed=args.speed, lang=args.lang, is_phonemes=True)
            if not timings:
                sys.exit(f"{args.model} returned no timings: the export has no duration output")
            sf.write(wav, audio, sr)
            pw = phoneme_words(timings)
            # Each script word gives as many phoneme words as it does on its
            # own ("81" → two); when that holds the words map one to one,
            # else the engine aligns the spoken words to the script.
            per_word = [len([w for w in kokoro.tokenizer.phonemize(t, args.lang).split() if re.search(r"\w", w)]) for t in text.split()]
            words = []
            if sum(per_word) == len(pw):
                i = 0
                for t, n in zip(text.split(), per_word):
                    if n:
                        words.append({"text": t, "start": round(pw[i][1], 3), "end": round(pw[i + n - 1][2], 3)})
                    i += n
            else:
                normal = kokoro.tokenizer.normalize_text(text).split()
                names = normal if len(normal) == len(pw) else [w[0] for w in pw]
                words = [{"text": n, "start": round(w[1], 3), "end": round(w[2], 3)} for n, w in zip(names, pw)]
            json.dump({"stamp": stamp, "words": words}, open(cached, "w"))
        clips.append({"id": line["id"], "audio": os.path.basename(wav), "text": line["text"], "words": words})
        print(f"  {line['id']}: {len(words)} words", file=sys.stderr)
    json.dump({"clips": clips}, open(os.path.join(args.out, "manifest.json"), "w"), indent=1)
    print(os.path.join(args.out, "manifest.json"))


if __name__ == "__main__":
    main()
