# Why is cancer so hard to cure? (Vox-style short)

A 68-second vertical (1080×1920) explainer, built entirely with the Lahza
motion engine through its MCP tools: collage on paper, taped cut-out
cards, highlighter swipes, 12 fps stepped animation, captions that light
up word by word with the narration, generated microscope slides, and real
3D where things are objects (gold coins, glass cells, a DNA double helix,
a tumour, a pill), with soft shadows on the paper.

## Build

```bash
pip install kokoro-onnx soundfile numpy pillow
mkdir -p out models
curl -L -o models/kokoro-v1.0.int8.onnx https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.0/kokoro-v1.0.int8.onnx
curl -L -o models/voices-v1.0.bin https://github.com/thewh1teagle/kokoro-onnx/releases/download/model-files-v1.0/voices-v1.0.bin
python3 narrate.py out models/kokoro-v1.0.int8.onnx models/voices-v1.0.bin   # vo.wav + vo.json (line timings)
python3 music.py out                                                          # music.wav, minor → major at "hope"
python3 assets.py out                                                         # out/art: paper, slides, tape
python3 cancer.py out                                                         # project, check, contact sheet, render
```

`SKIP_RENDER=1` stops after the contact sheet; `QUALITY=draft` renders at
half size.

## How it is made

- **Narration** — Kokoro (Apache-2.0) speaks each line separately; the
  lines are joined with measured pauses, so every visual can be timed to
  the line (and, roughly, the word) it illustrates. The line starts are
  also a `line` marker track in the project.
- **Music** — `music.py` synthesises a quiet bed that turns from minor to
  major where the narration turns hopeful; the music ducks under the voice.
- **Art** — `assets.py` paints the paper, an H&E-stained healthy slide and
  a tumour slide (small orderly nuclei vs big crowded ones), and masking tape.
- **Sound effects** — the engine's built-in library (thud, swipe, pop, tick,
  glitch, riser, boom, ding…) placed on the same beats as the visuals.

## Facts

- The National Cancer Act was signed on December 23, 1971.
- There are more than 200 types of cancer (Cancer Research UK).
- The human genome is about three billion DNA letters long.
- US five-year relative survival, all cancers: 49% (1975–77) → 69%
  (2013–19) — American Cancer Society, *Cancer Statistics, 2024*.
- Most cancer deaths are caused by cancer that has spread (metastasis).
