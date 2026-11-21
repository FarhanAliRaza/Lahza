"""ElevenLabs narration with character timings, as a Lahza narration manifest.

    ELEVENLABS_API_KEY=... python3 elevenlabs_manifest.py SCRIPT.json OUT_DIR --voice VOICE_ID [--model eleven_multilingual_v2]

SCRIPT.json is {"lines": [{"id": "hook", "text": "One bomb. Over [strike]Iran."}, ...]}.
Each line goes to the `with-timestamps` endpoint, which returns the audio
and per-character start and end times; those go into the manifest as
`chars` (the engine groups them into words). Cue markup is not sent.
Lines already synthesised with the same text and voice are reused.
"""
import argparse
import base64
import hashlib
import json
import os
import re
import sys
import urllib.request

CUE = re.compile(r"\[/?[A-Za-z0-9_-]+\]")


def spoken(text):
    return " ".join(CUE.sub(" ", text).split())


def synthesize(text, voice, model, key):
    url = f"https://api.elevenlabs.io/v1/text-to-speech/{voice}/with-timestamps?output_format=mp3_44100_128"
    body = json.dumps({"text": text, "model_id": model}).encode()
    req = urllib.request.Request(url, data=body, headers={"xi-api-key": key, "Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=120) as r:
        return json.load(r)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("script")
    ap.add_argument("out")
    ap.add_argument("--voice", required=True)
    ap.add_argument("--model", default="eleven_multilingual_v2")
    args = ap.parse_args()
    key = os.environ.get("ELEVENLABS_API_KEY") or sys.exit("set ELEVENLABS_API_KEY")
    lines = json.load(open(args.script))["lines"]
    os.makedirs(args.out, exist_ok=True)
    clips = []
    for line in lines:
        text = spoken(line["text"])
        audio = os.path.join(args.out, f"{line['id']}.mp3")
        stamp = hashlib.sha256(f"{args.voice}|{args.model}|{text}".encode()).hexdigest()
        cached = audio + ".json"
        if os.path.exists(audio) and os.path.exists(cached) and json.load(open(cached)).get("stamp") == stamp:
            chars = json.load(open(cached))["chars"]
        else:
            r = synthesize(text, args.voice, args.model, key)
            open(audio, "wb").write(base64.b64decode(r["audio_base64"]))
            a = r["alignment"]
            chars = [
                {"char": c, "start": round(s, 3), "end": round(e, 3)}
                for c, s, e in zip(a["characters"], a["character_start_times_seconds"], a["character_end_times_seconds"])
            ]
            json.dump({"stamp": stamp, "chars": chars}, open(cached, "w"))
        clips.append({"id": line["id"], "audio": os.path.basename(audio), "text": line["text"], "chars": chars})
        print(f"  {line['id']}: {len(chars)} characters", file=sys.stderr)
    json.dump({"clips": clips}, open(os.path.join(args.out, "manifest.json"), "w"), indent=1)
    print(os.path.join(args.out, "manifest.json"))


if __name__ == "__main__":
    main()
