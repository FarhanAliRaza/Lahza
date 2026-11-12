# Narration adapters

Voices are made outside the engine. An adapter turns one provider's output
into a **narration manifest**, which `narration_import` reads:

```json
{
  "lead": 0.5,
  "pause": 0.35,
  "clips": [
    {"id": "hook", "audio": "hook.wav", "text": "One bomb. Over [strike]Iran.",
     "words": [{"text": "One", "start": 0.04, "end": 0.29}, ...]},
    {"id": "s02", "audio": "s02.mp3", "text": "It hit 81 cities.", "at": 4.0,
     "chars": [{"char": "I", "start": 0.0, "end": 0.06}, ...]}
  ]
}
```

- **One clip per line.** Line bounds are then exact by construction. `id`
  is how scenes refer to the line (`line:hook`).
- **`text`** is the script as shown ("81", "U.S."). Cues in square brackets
  name moments that survive script edits: `[strike]Iran` gives `cue:strike`,
  and `[map]a span[/map]` gives a span. Cues are not spoken.
- **Timings** are seconds from the clip's own start, in either form:
  - `words`, which may be in the spoken form ("eighty-one");
  - `chars`, which the engine groups into words.

  The engine aligns them to the script and reports any word it can't place.
- **`at`** places a clip on the timeline. Without it, a clip follows the
  previous one after `pause`; the first clip starts at `lead`.

| Provider | Timings | Adapter |
|---|---|---|
| Kokoro (`kokoro-onnx` `create_timed`) | phoneme durations; needs an export with a duration output, e.g. `onnx-community/Kokoro-82M-v1.0-ONNX-timestamped` (`onnx/model_quantized.onnx`); `kokoro-v1.0.int8.onnx` has none | `kokoro_manifest.py` |
| ElevenLabs `with-timestamps` | per character | `elevenlabs_manifest.py` |
| Azure Speech | word boundary events (offset and duration, 100 ns ticks) | convert the events to `words` |
| OpenAI TTS, most others | none | an external aligner (for example Montreal Forced Aligner, aeneas or whisper with forced alignment) produces `words` |
| A recording made in Lahza | its transcript | `markers_import kind=transcript`, or an aligner for exact script words |

Re-running an adapter after a script edit regenerates only the changed
lines. Re-importing reuses unchanged clips; everything timed to `line:`,
`word:` or `cue:` references moves with the narration.
