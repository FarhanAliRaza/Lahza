# Motion engine (phase 1)

A timing-first motion-graphics engine an AI agent drives through MCP, built
from [`notes/MOTION_ENGINE_DESIGN.md`](../notes/MOTION_ENGINE_DESIGN.md).
Phase 1 is the time and motion core with a GPU 2D renderer and the agent
interface; the editor UI comes in the next phase. The recording editor and
exporter are unchanged.

| Crate | What it does |
|---|---|
| [`crates/motion-core`](../crates/motion-core) | Project document, rational time, stepped clocks, the measured curve presets, marker tracks, time references with sync anchors, additive tracks, per-glyph text animators, precomps, track mattes, effects, scene transitions, atomic edit ops with inverses, `check`. Pure Rust, no GPU, no I/O beyond project folders. |
| [`crates/motion-audio`](../crates/motion-audio) | Music → marker tracks: tempo (with a tempo map when it changes), meter (3/4, 4/4), first downbeat, beat/bar/half/sixteenth/triplet grids, straight vs triplet feel, onsets per band (refined to ~1 ms), `hit`, `stop` (silence→hit), `drop` (bass dropout before a downbeat), `music.start/end`. |
| [`crates/motion-render`](../crates/motion-render) | Display list of SDF shapes, Bézier paths, HarfBuzz-shaped SDF glyphs and images binned into 16×16 tiles; Vulkan compute rasteriser on `blade-graphics` with a CPU path that runs the same math; offscreen groups for precomps, mattes, blur/shadow/glow (large blurs at reduced resolution); shutter motion blur; frame cache; export through ffmpeg (NVENC/VAAPI/libx264, VP9, GIF) with audio mix and ducking; contact sheets. |
| [`crates/motion-mcp`](../crates/motion-mcp) | `lahza mcp` (MCP server on the official `rmcp` SDK) and `lahza motion …` (CLI). Both run one transport-free `Toolbox`. |

## Use it from an agent

```bash
claude mcp add lahza -- lahza mcp
```

Any MCP client works: the server speaks stdio. `lahza-motion mcp` (from
`cargo build -p lahza-motion-mcp`) is the same server without the desktop
dependencies. Pass `--cpu` to force the CPU renderer.

Tools: `project_create`, `project_open`, `project_info`, `timeline_get`,
`markers_query`, `markers_import`, `edit_apply`, `check`, `snapshot`,
`render`, `render_status`, `history`, `audio_analyze`.
Resources: `lahza://guide` (the production loop and timing rules),
`lahza://schema` (the project format), `lahza://catalog` (presets with
their measured numbers, curves, anchors, references, layer kinds, ops),
`lahza://styles` and `lahza://styles/<pack>`.
Prompts: `make_promo`, `make_explainer`, `make_product_demo`, `make_ident`.

The loop: create a project with a style pack and the music → `audio_analyze`
→ `markers_query` → `edit_apply` scene by scene, anchoring keys to markers
(`"at": "beat:13", "anchor": "peak", "lead": -1`) → `check` (no errors,
drift ≤ ±1 frame) → `snapshot` contact sheet at the sync points → `render`.

## Use it from the shell

```bash
lahza motion create promo --style promo.kinetic --duration 20 --music track.wav
lahza motion analyze promo.lahza
lahza motion apply promo.lahza ops.json
lahza motion check promo.lahza
lahza motion snapshot promo.lahza --sheet -o sheet.png
lahza motion render promo.lahza -o promo.mp4          # --draft, --from/--to, --no-audio
```

## Timing model in one paragraph

A key is a *move* that arrives at its value. Without `dur` it is a classic
keyframe; with `dur` (or an anchor other than `end`) it is placed so its
`start`, `peak` speed, `land` (98%) or `end` falls on `at + lead`. Times are
numbers or references: `beat:17`, `bar:5`, `word:canal`, `hit:2`,
`#title.end + 0.25`, `bar:5 - 2f`. Cuts on markers lead by one frame. Moves
that overlap take over from the current value, so motion never jumps.
Additive `drift`, `wiggle` and `sine` tracks sit on top ("nothing stops
dead"). `step_fps` on a layer or scene gives stop-motion with bit-identical
held frames while the camera stays smooth. `check` measures each anchor on
the real frame grid, through stepping and pre-roll, and reports the drift.

## Verified in this phase

- Curve presets reproduce the study numbers (tests in `motion-core/src/curve.rs`):
  snap/glide/settle 90% frames, `enter` 90% in 4–6 f, `pop` 9% overshoot
  settling ≤ 6 f with its peak at 2.5 f, `land` 6–17% overshoot, bounce
  durations ×0.43 and heights ×0.18.
- Anchored peaks land within ±0.5 frame of their marker after lead.
- Audio: synthetic rebuilds of the study tracks — 90 BPM with the first
  downbeat at 1.093 s, 110 BPM, 133.3 BPM with triplet feel, stop→hit,
  bass drop before a downbeat — within one 25 fps frame.
- Rendering is deterministic; GPU (Vulkan) and CPU output match within
  3/255 per channel (run with a Vulkan device; on llvmpipe set
  `LAHZA_MOTION_SOFTWARE_GPU=1`).
- Benchmark `crates/motion-mcp/tests/promo.rs`: an original 20 s
  kinetic-type promo on a supplied track, built only through tool calls;
  `check` passes with every anchor within ±1 frame; the draft renders with
  audio. `LAHZA_PROMO_OUT=<dir>` keeps the contact sheet and a full render.

Performance, 1080p, 200 animated shapes and four per-glyph text layers
(`cargo test -p lahza-motion-render --release -- --ignored --nocapture benchmark`):
frame evaluation 0.16 ms and display-list build 0.8 ms per frame; raster on a
4-core container is 47 ms (llvmpipe Vulkan) / 69 ms (CPU path). Hardware GPU
numbers (the RTX 3060 target) still need measuring on that machine. The full
20 s promo exported at 0.85× real time on the 4-core CPU path.

## Compared with After Effects

What an agent can make today: beat- and word-synced motion graphics and
kinetic typography — shapes and Bézier paths with draw-on, per-glyph text
(shaped, any script with a suitable font), images, precomps, track mattes,
blur/shadow/glow, transitions, motion blur — rendered on the GPU and
checked frame by frame for sync. That covers promos, idents, title
sequences, lower thirds, explainer graphics without footage, and
logo reveals.

| After Effects | Lahza motion |
|---|---|
| Keyframes, graph-editor easing, springs | ✔ measured presets, raw bezier/power/exp/spring/bounce, moves anchored by start/peak/land/end to markers |
| Audio-driven timing (markers by hand) | ✔ automatic: beats, bars (3/4, 4/4), tempo maps, hits, stops, drops, words |
| Wiggle / looping expressions | ✔ drift, wiggle, sine additive tracks; no general expressions yet |
| Shape layers, trim paths | ✔ primitives + SVG Bézier paths, fill, stroke, trim |
| Text animators | ✔ per glyph/word/line, stagger orders, ranges; HarfBuzz shaping, bidi |
| Precomps, parenting, time offset | ✔ comps render as one picture when they need to; parenting; time offsets; stepped time |
| Track mattes | ✔ alpha, luma, inverted |
| Effects | ✔ Gaussian blur (repeat edges), drop shadow, glow — ✘ everything else (colour correction, LUTs, grain, distortions, fractal noise, …) |
| Blend modes | ✔ normal, add, multiply, screen — ✘ the other ~20 |
| Transitions | ✔ fade, push, slide, zoom, wipe, whip — ✘ object/matte wipes, zoom-through into a nested scene |
| Motion blur | ✔ shutter angle per scene |
| Footage | ✔ stills — ✘ **video layers** (Lahza recordings arrive in phase 4) |
| 3D | ✘ **3D layers, cameras, lights**, extruded text (phases 3/5) |
| Masks on a layer | ◐ via track mattes (a shape layer as matte); no per-layer mask paths with feather/expansion |
| Path morph, booleans, repeaters, particles, text on a path | ✘ (phase 2) |
| Expressions / scripting | ✘ (planned as `code_put`: expressions, shaders, WASM) |
| Motion tracking, rotoscoping, keying | ✘ (camera matting exists in the recorder, not wired in) |
| Timeline UI | ✘ — agent and CLI only; the editor UI is the next phase |

The biggest gaps for "AE-class" in order of value: **video layers**, **the
editor UI**, **2.5D/3D camera with layers in depth**, **more effects
(colour, grain, displacement) and blend modes**, **path morph and
repeaters**, **expressions**.

## Known limits

- The MCP server works on project files (it re-reads changes made on disk);
  a live link into a running editor comes with the editor UI.
- Effects on a matted layer: the shadow and glow use the unmatted picture.
- Trim on a filled path trims the stroke only.
- Tools from the design that depend on undecided asset sourcing or later
  phases: `code_put`, `voiceover`, `ocr`, `assets_search`/`assets_fetch`.
