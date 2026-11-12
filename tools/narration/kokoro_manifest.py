"""Kokoro narration with word timings, as a Lahza narration manifest.

    python3 kokoro_manifest.py SCRIPT.json OUT_DIR MODEL.onnx VOICES.bin [--voice af_heart] [--speed 1.0]

SCRIPT.json is {"lines": [{"id": "hook", "text": "One bomb. Over [strike]Iran."}, ...]}
(cues in square brackets are kept for the engine and not spoken). Writes
OUT_DIR/<id>.wav per line and OUT_DIR/manifest.json for `narration_import`.

Timings come from Kokoro's own phoneme durations (`create_timed`). That
needs a model export with a duration output, such as
onnx-community/Kokoro-82M-v1.0-ONNX-timestamped (`onnx/model_quantized.onnx`,
output "durations"); `kokoro-v1.0.int8.onnx` only outputs audio, and this
script stops rather than guess. Lines already synthesised with the same
text, voice and speed are reused.
"""
import argparse
import difflib
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


# Not sounds: spaces, stress marks and punctuation.
SILENT = set(" \t\nˈˌ.,!?;:—–…'\"()-")
# Where a spoken word ends: a space or punctuation (a pause), not a stress mark.
EDGE = SILENT - set("ˈˌ")


def word_times(words, word_phonemes, timings):
    """Each script word's (start, end), or None when it can't be placed.
    espeak joins words when it phonemises a line ("in the" → "ɪnðə") and
    splits others ("81" → "eɪti wʌn"), so the words' own phonemes are
    aligned to the spoken ones instead of counting words. A word then grows
    to the spaces around it in the spoken stream (taking its stress mark and
    sounds that differ in context), never into a neighbour; a word with no
    match takes the sounds between its neighbours ("a": "eɪ" alone, "ɐ" in
    the line)."""
    script, owner = [], []
    for i, ph in enumerate(word_phonemes):
        for c in ph:
            if c not in SILENT:
                script.append(c)
                owner.append(i)
    sounds = [k for k, t in enumerate(timings) if t[0] not in SILENT]
    span = [None] * len(words)  # first and last index into timings
    matcher = difflib.SequenceMatcher(None, script, [timings[k][0] for k in sounds], autojunk=False)
    for a, b, n in matcher.get_matching_blocks():
        for k in range(n):
            i, idx = owner[a + k], sounds[b + k]
            span[i] = (idx, idx) if span[i] is None else (min(span[i][0], idx), max(span[i][1], idx))
    found = [i for i, s_ in enumerate(span) if s_]
    out = list(span)
    for j, i in enumerate(found):
        lo, hi = span[i]
        floor = span[found[j - 1]][1] if j else -1
        ceil = span[found[j + 1]][0] if j + 1 < len(found) else len(timings)
        while lo - 1 > floor and timings[lo - 1][0] not in EDGE:
            lo -= 1
        while hi + 1 < ceil and timings[hi + 1][0] not in EDGE:
            hi += 1
        out[i] = (lo, hi)
    # Unplaced words share the sounds between the placed words around them.
    i = 0
    while i < len(words):
        if out[i] is not None:
            i += 1
            continue
        j = i
        while j < len(words) and out[j] is None:
            j += 1
        a = out[i - 1][1] + 1 if i else 0
        b = out[j][0] - 1 if j < len(words) else len(timings) - 1
        gap = [k for k in range(a, b + 1) if timings[k][0] not in SILENT]
        n = j - i
        if len(gap) >= n:
            for m in range(n):
                part = gap[m * len(gap) // n:(m + 1) * len(gap) // n]
                out[i + m] = (part[0], part[-1])
        i = j
    return [(timings[s_[0]][1], timings[s_[1]][2]) if s_ else None for s_ in out]


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
    # kokoro-onnx looks for an output named "duration"; the timestamped
    # export calls it "durations" (same values, read by position).
    if "durations" in {o.name for o in kokoro.sess.get_outputs()}:
        kokoro.has_timings = True
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
        # The cache keeps Kokoro's phoneme timings, so words are re-derived
        # from them without synthesising again.
        prev = json.load(open(cached)) if os.path.exists(wav) and os.path.exists(cached) else {}
        if prev.get("stamp") == stamp and "phonemes" in prev:
            timings = prev["phonemes"]
        else:
            # Phonemise once, so the timings index exactly these phonemes.
            phonemes = kokoro.tokenizer.phonemize(text, args.lang)
            audio, sr, timings = kokoro.create_timed(phonemes, voice=args.voice, speed=args.speed, lang=args.lang, is_phonemes=True)
            if not timings:
                sys.exit(f"{args.model} returned no timings: the export has no duration output")
            sf.write(wav, audio, sr)
            timings = [[t.phoneme, t.start, t.end] for t in timings]
            json.dump({"stamp": stamp, "phonemes": timings}, open(cached, "w"))
        script = text.split()
        found = word_times(script, [kokoro.tokenizer.phonemize(w, args.lang) for w in script], timings)
        # A word with no phonemes found is left out: narration_import reports
        # it rather than this script guessing a time.
        words = [{"text": w, "start": round(f[0], 3), "end": round(f[1], 3)} for w, f in zip(script, found) if f]
        clips.append({"id": line["id"], "audio": os.path.basename(wav), "text": line["text"], "words": words})
        print(f"  {line['id']}: {len(words)} words", file=sys.stderr)
    json.dump({"clips": clips}, open(os.path.join(args.out, "manifest.json"), "w"), indent=1)
    print(os.path.join(args.out, "manifest.json"))


if __name__ == "__main__":
    main()
