# Lahza motion engine and agent interface — design

Status: proposal. Built from eight studies in `notes/motion-studies/`:
frame-measured breakdowns of IMEX, BOUNC, Motion Array, BeyondTheBlack, a
motion-design showreel and a collage tutorial, plus sourced research on the
Johnny Harris and Vox explainer styles. Numbers quoted here come from those
reports; see them for evidence.

## 0. For the session implementing this

Read this whole file, then `notes/motion-studies/ENGINE_PLAN.md`; open a
study report only when a phase needs its numbers. Build **phase 1 only**
(section 12.1) unless told otherwise, and stop at its acceptance criteria.

### Repository context (verified at commit a900fff)
- Workspace: every crate under `crates/*` is a member; the app binary is the
  root package `lahza` (`src/main.rs`). Project rules are in `CLAUDE.md`
  (performance rules, GPU image release, `just install`).
- Existing pieces to reuse, not rewrite:

| Need | Existing code |
|---|---|
| GPU compositor, layer caches, `blur_plane` | `crates/scene/src/scene.rs`, `scene_gpu.rs` (+ `.wgsl`), `blade-graphics` on Vulkan |
| Export pipeline, overlay hooks (`OverlaySource`, `MediaOverlaySource`), `export_scene` | `crates/export/src/export.rs` |
| Hardware encode (NVENC selection) | `crates/export-encoder/src/export_encoder.rs` |
| Recording project files (`.lahzarec`, `input.json`, `edit.json`) | `crates/model/src/model.rs` |
| Word timestamps (whisper.cpp `whisper-cli`) | `crates/transcription` |
| Annotation marks and SVG rasteriser (`render_svg_layer`) | `crates/annotations` |
| Camera regions, zoom cues, motion presets | `crates/viewport` |
| Editor GPU image release (`Studio::retire_image`) | `src/main.rs` |

### Architecture decision
The motion engine is a **new set of crates beside the existing recording
engine**, not a rewrite of `SceneCompositor`:
- `crates/motion-core`: project document, time model, curves, markers,
  anchors, evaluation of any property at time t. No GPU, no I/O beyond
  load/save. Pure and unit-testable.
- `crates/motion-audio`: music analysis into marker tracks.
- `crates/motion-render`: GPU renderer on `blade-graphics`, reusing
  `blur_plane` ideas and the export encoder.
- `crates/motion-mcp`: the MCP server (`rmcp`), exposed as `lahza mcp`.
- CLI subcommands on the `lahza` binary: `lahza motion check|snapshot|render
  <project>`.

The current editor and exporter keep working unchanged. A Lahza recording
enters the new engine as a media layer (phase 4). Folding the current
"scene" features into a style pack is a later decision.

### Deferred decisions (do not block on these)
- **Asset sourcing** (maps, archives, data, music, TTS) is undecided. Until
  it is decided, the engine only uses files the person supplies or that the
  project already contains; `assets_search`/`assets_fetch` are not built.
- Project folder format (`.lahza` vs inside `.lahzarec`): use a standalone
  `.lahza/` folder for phase 1.

### Reference material on this machine
- Reference videos: `~/Downloads/` (IMEX, BOUNC, Motion Array, BeyondTheBlack
  opener, motion showreel, collage tutorial). Do not copy them.
- Study reports: `notes/motion-studies/*.md`.
- Measurement scripts used by the studies: `notes/motion-studies/scripts/`
  (one-off Python; paths inside point at an old scratch folder, so adapt
  before reuse). Useful parts: beat/onset analysis (`*/audio*.py`,
  `ma/beats.py`), colour-blob tracking (`*/track*.py`), curve fitting
  (`*/fit*.py`).
- Prototypes: `notes/motion-studies/prototypes/` — `ident/` (CPU tiny-skia
  flat ident, 16.3 s at 60 fps with 5× motion blur in 27 s),
  `showcase-film/` (film driven through the current `export_scene`),
  `blender-test-shot.py` (Cycles reference shot).
- `notes/` is git-ignored; these design files are force-added and tracked.

## 1. Goal

An AI agent, given a brief (and optionally a Lahza screen recording, a
voiceover, data or documents), produces a finished video at professional
motion-design level: script, voiceover, picture timed to words and music,
maps, documents, charts, UI demos, transitions, sound and final render. A
person can open the same project in Lahza and keep editing.

Non-goals for the first versions: photoreal character animation, fluid and
cloth simulation, replacing a 3D modelling tool. Those enter as assets.

## 2. Principles from the studies

1. **Timing is the craft.** Every reference is defined more by *when* things
   move than by what they look like. Time is a first-class system, not a
   number on a keyframe (section 4).
2. **A small measured motion vocabulary covers almost everything.** Entrances
   start at full speed and decay exponentially; overshoot is rare and only
   on scale or landings; nothing stops dead (residual drift); staggers are
   1–3 frames (section 5).
3. **Continuity over cuts.** Scenes become each other: an object re-read by a
   3D camera (IMEX), zoom-through into a nested scene (Motion Array), the
   click as the transition (showreel), action-matched cuts (BOUNC).
4. **Evidence, not decoration** (Vox, Johnny Harris). Maps, documents,
   photos and screens are treated as physical objects and marked up by hand.
   Lahza owns the richest evidence source there is: the screen recording,
   with real cursor, clicks, keys and OCR-able pixels.
5. **The document is the source of truth.** The agent, the Lahza editor and
   the renderer all read and write the same project. No hidden state.
6. **Fast enough to iterate.** Preview plays in real time; 2D exports render
   faster than real time; the agent looks at frames, not whole videos.

## 3. The production pipeline an agent runs

```
brief ──▶ script + visual script ──▶ voiceover ──▶ word timestamps
                                                       │
music choice ──▶ music analysis (beats, hits, stops) ──┤
                                                       ▼
            scene plan (each line → shot, style, sync anchors)
                                                       ▼
            build scenes (edit ops) ──▶ check ──▶ snapshot review ──▶ fix
                                                       ▼
                         mix (VO, music ducking, SFX) ──▶ render
```

- **Visual script first.** A table with one row per spoken line: the words,
  the visual, the sync anchor, the style. The person approves this before
  any building (Johnny Harris's colour-coded "visual scripts"; Vox's rule
  that narration describes what is on screen).
- **Voiceover:** recorded in Lahza (teleprompter, mic, RE-USE denoise
  already exist) or synthesised by a TTS provider the user configures.
- **Word timestamps** come from the existing whisper.cpp integration.
- **Music analysis** produces marker tracks (section 4.2).
- **Building** is a sequence of edit operations against the project
  (section 8). **Review** is `check` plus contact sheets of frames at the
  sync points.

## 4. Time model

### 4.1 Clocks
- **Timeline time** in seconds, rational frame rates (24, 25, 30, 60).
- **Per-layer or per-group stepped clock**: `step_fps` (8, 12) and a
  phase offset. Animation is evaluated at
  `t_q = floor((t - t0) * step_fps) / step_fps + t0`; held frames are
  bit-identical so the compositor caches them. At 30 fps, 8 fps gives
  holds of 4,4,4,3 and 12 fps gives 3,2,3,2 (collage study).
- **The camera keeps its own smooth clock** while elements step (Vox).
- **Clip pre-roll**: a shot may start mid-animation (showreel: 1–7 frames
  before the cut).

### 4.2 Marker tracks
Every project carries named marker tracks the agent can query and anchor to:

| Track | Source |
|---|---|
| `line`, `sentence`, `word`, `cue` | narration manifest (`narration_import`): the voice provider's own word or character timings, aligned to the script |
| `word`, `sentence` (recordings) | transcription (whisper.cpp), for recordings that have no script |
| `beat`, `bar`, `half`, `sixteenth`, `triplet` | music analysis: BPM, offset, swing/triplet feel |
| `onset.low`, `onset.mid`, `onset.high` | per-band onset detection |
| `hit` | strongest onsets; impact-style sounds |
| `stop`, `drop` | silence→hit points; bass dropouts before section downbeats |
| `music.start`, `music.end` | first sound; hard end (may be trimmed to picture) |
| `click`, `key`, `scroll`, `window` | Lahza recording input data |
| `ocr` | text boxes found in recordings, images and PDFs, with times |
| `user` | markers placed by hand |

Narration is made outside the engine by any voice (Kokoro, ElevenLabs,
Azure, OpenAI, a recording). The engine never transcribes it: small
adapters (`tools/narration`) turn each provider's timings into one manifest
format — one clip per line with its audio, script text and `words` or
`chars` timings — and the engine validates them without listening (order,
inside the clip, every script word timed, no word in silence). Script text
is what is shown ("81"); timings may be what was spoken ("eighty-one").
`[cue]` markup in the script names moments (`cue:strike`) that survive
script edits. Clips are hashed: re-importing after an edit re-imports only
changed lines, and everything anchored to the narration moves with it.
Transcription (whisper.cpp) remains an import source for recordings with
no script.

### 4.3 Anchoring
Any time field accepts a reference instead of a number:

```
"cue:strike", "cue:strike.end"   a cue in the narration script
"line:hook"               a narration line by id
"word:borders"            first occurrence of the word (or its spoken form)
"word#42"                 by index (breaks when the script changes)
"sentence#3.end"
"beat:17", "bar:5", "triplet:12"
"hit:3", "stop:2", "click:4", "ocr:'Deploy'"
"#title.end + 0.25"       relative to another element
```

Each keyframe or clip edge also has an **anchor point**: which moment of the
animation lands on the reference: `start`, `peak` (maximum velocity or
brightness), `land` (settle), or `end`. Plus a `lead` in frames (default −1
for cuts). This is required by IMEX (tilt peak on the beat), BeyondTheBlack
(ring brightness peak on the downbeat, cut 3.7 f earlier) and the showreel
(cuts lead stop→hit points by about half a frame).

### 4.4 Sync modes per section
- `grid`: snap to beat subdivisions (Motion Array: 21/26 events within ±1 f).
- `hits`: snap to onsets, no grid (BOUNC: 12/12 contacts within −0.6…+2 f).
- `free`: no snapping; only stop→hit points are binding (showreel groove,
  BeyondTheBlack).
- `words`: anchor to narration (Vox, Johnny Harris).

`check` reports drift between requested and achieved sync per anchor.

## 5. Motion vocabulary

Named curves, all available raw as well:

| Preset | Model | Measured |
|---|---|---|
| `snap` | exponential lag, keep r of remaining distance per frame | r≈0.5 |
| `glide` | exponential lag | r≈0.7–0.79, 90% in 6–9 f |
| `settle` | exponential lag | r≈0.8, τ≈6 f |
| `enter` | ease-out quart | 6–12 f, 90% in 4–6 f |
| `gesture` | symmetric ease-in-out | 5–8 f |
| `pop` | spring, scale only | ζ≈0.61, ω≈40 rad/s, ~9% overshoot, settles ≤ 6 f |
| `land` | spring | ζ 0.35–0.55, 1.6–2.8 Hz, 6–17% overshoot |
| `squash` | spring on squash deformer | ~5 Hz, ζ≈0.3 |
| `bounce` | shaped arcs | apex exponent ≈2.75, each bounce 0.43× previous |
| `slam` | accelerate into a dead stop | t² |
| `zoom` | interpolate scale in log space | for 6–10× zooms |
| `push` | slow constant camera push | ~3–4% per beat (Vox), "rarely snaps" (Harris) |

Raw curves: cubic-bezier, power-n, exp-lag (r or τ), spring (ζ, f or k, c),
hold.

Modifiers:
- **Additive tracks**: entrance + drift (0.5–2% scale per frame) + wiggle.
- **Stagger**: by index, random, centre-out, along a path; 1–3 f typical.
- **Velocity stretch**: squash/stretch along the velocity vector, area
  preserving (IMEX particles 118 → 16 px in 5 f; balls up to 3:1).
- **Hand-drawn boil**: displacement noise reseeded at a set rate (Vox markup
  4 Hz). Off by default; collage held frames are pixel-identical.

## 6. Scene model

A project is a folder (`*.lahza/`) holding `project.json`, `scenes/*.json`,
`assets/`, `markers/*.json`, `history/`, and the existing recording files
when a recording is the source. JSON because agents and the editor both edit
it reliably and diffs stay readable. Every node has a stable `id`.

```
Project
 ├─ settings: size, fps, duration, colour space, style pack
 ├─ audio: vo, music (with ducking), sfx[]
 ├─ markers: tracks (section 4.2)
 └─ timeline: scenes[] (start, duration, transition in/out, pre-roll)
      Scene = Comp
       ├─ camera (2D, 2.5D or 3D; geo camera for maps)
       └─ layers[] (tree; any layer can hold a nested comp)
```

Layer fields shared by all kinds: transform (2D or 3D), opacity, blend, mask
and matte, effects stack, time (`start`, `duration`, `step_fps`,
`pre_roll`, `time_remap`), parent, and per-property animation (keyframes,
additive tracks, expressions).

### 6.1 Layer kinds
| Kind | Notes |
|---|---|
| `shape` | SDF primitives (circle, rounded rect, capsule, ring, arc, polygon) and bezier paths; fill (solid, gradient, pattern/hatch, texture); stroke with trim, dash, caps; boolean groups with designated intersection fills (IMEX overlap colours); smooth union; path morph with vertex correspondence |
| `text` | per-glyph, per-word and per-line animators with range selectors; typewriter with re-centring; counters; label boxes; gradient fill, outline, blur reveal; text on path |
| `markup` | highlighter, underline, ellipse, arrow with label, redact, contrast dim, focus blur; targets a text box, OCR box, rect or point; roughness, boil Hz, draw-on/off, multiply blend |
| `image` / `video` | stills and footage; the Lahza **recording** as a source with its cursor, clicks and OCR; webcam with existing matting |
| `document` | PDF or image page in 3D, deskew, OCR word boxes, camera push to a phrase |
| `chart` | data-driven line, bar, area, scatter, timeline (ruler first); scales, ticks, leader-line labels, title and source-line slots, build order, dim-others emphasis |
| `map` | projections (globe, Web Mercator, Equal Earth), vector layers (Natural Earth, GeoJSON, OSM), raster tiles (satellite, relief), DEM terrain; per-feature stroke trim, wobble, hatch fill, reveal; geodesic routes and arrows; geo-anchored labels and pins with collision nudging |
| `mesh` | unlit flat-shaded primitives (cylinder, sphere, hemisphere, capsule, box) with per-surface colours and hard flat shadows first; glTF with PBR later |
| `instancer` | grid, radial, random, along-path layouts; per-instance stagger, palette, time offset; field effectors |
| `particles` | GPU particles with gravity, drag, tumble, trails |
| `cursor` | OS or stylised skin; path, dwell, press (scale ~0.8 over 4–5 f), ripple; driven by recorded pointer data or by keys |
| `comp` | nested scene; enables zoom-through and portal transitions |

### 6.2 Effects
Gaussian and directional blur (through `blur_plane`), glow and bloom,
background blur (glass), drop and inner shadow, gradient map and tint (HSB
input), LUT, grain (static or boiling), halftone, paper and texture overlays
locked to layer or screen, vignette, RGB split, scanlines and flicker
("monitor look"), film burns and light leaks, torn-edge border, depth of
field. Each effect is a parameter list plus readable shader source
(section 9).

### 6.3 Transitions
`cut` (with "peak at cut" and a blur pulse), `zoom_through(target)`,
`portal(target)`, `whip(direction)`, `wipe(object | shape)`,
`match(object)` (keep screen position and direction), `on_click(click)`.

## 7. Style packs

A style pack bundles palette tokens, fonts, textures, default presets and
**rules** that `check` enforces. Agents choose a pack, then override.

| Pack | Defaults and rules |
|---|---|
| `explainer.evidence` (Vox-like) | off-white ground, ink black, one highlight colour; elements stepped at 12 fps, camera smooth; markup boil 4 Hz; no still held over 3 s; charts print-style with title and source line |
| `explainer.map` (Harris-like) | muted paper map, one accent; slow pushes, no snaps; labels appear as places are named; grain, dust, burns |
| `ident.flat` (IMEX-like) | flat palette with intersection table; unlit 3D; one scene per bar; springs on landings |
| `product.ui` (showreel UI) | exp-lag snaps, glass cards, 1–3 f staggers, click as transition, residual drift |
| `collage` | tinted paper (never pure white), fragments at ~82% opacity, 8/12 fps stepping, per-layer textures |
| `promo.kinetic` (Motion Array-like) | strict beat grid, −1 f cut lead, per-glyph type, zoom-throughs |

Packs only use public-domain or user-supplied fonts and textures; they
reproduce a grammar, never another outlet's branding.

## 8. Agent interface (MCP)

One server, `lahza mcp`, built on the official Rust SDK (`rmcp`). When the
Lahza editor is open it connects to it through a local socket, so edits
appear live and land in the editor's undo history; otherwise it works on
project files directly.

### 8.1 Tools
| Tool | Purpose |
|---|---|
| `project_open` / `project_create` | open a folder or recording; create from a style pack |
| `project_info` | size, fps, duration, scenes, style, assets with licences |
| `timeline_get` | compact JSON of scenes, layers and keyframes with ids; filters by scene or time range |
| `markers_query` | words, beats, hits, stops, clicks, OCR boxes, by kind and range |
| `edit_apply` | batch of operations (`add`, `set`, `remove`, `move`, `key`, `anchor`, `group`) applied as one undoable step; returns the diff and new ids |
| `code_put` | add or update an expression, shader or plugin; compiles and returns errors with line numbers |
| `check` | structural, timing (sync drift per anchor, holds over the pack limit), layout (safe areas, overlaps, contrast), licensing |
| `snapshot` | frames at times or markers, single or contact sheet, small by default; returned as images |
| `render` / `render_status` | export with progress; draft or final quality |
| `history` | recent edits by person and agent; undo to a point |
| `voiceover` | record prompt in Lahza or synthesise with the configured TTS; then transcribe |
| `audio_analyze` | run music analysis and write marker tracks |
| `assets_search` / `assets_fetch` | maps (Natural Earth, OSM, NASA), archives (Library of Congress, Wikimedia), data (Our World in Data), SFX library; every asset stored with source and licence |
| `ocr` | text boxes for an image, PDF page or recording frame range |

### 8.2 Resources
- `lahza://schema`: project format.
- `lahza://catalog`: every layer kind, effect, transition and preset with
  parameters, ranges, defaults and a one-line description.
- `lahza://catalog/<item>/source`: the readable shader or preset source.
- `lahza://styles`, `lahza://styles/<pack>`: packs with their rules.
- `lahza://guide`: the production loop, the visual-script format, sync
  rules and review checklist.

### 8.3 Prompts
`make_explainer`, `make_product_demo`, `make_ident`, `make_promo`: each
seeds the visual-script template and the right style pack.

### 8.4 Control levels
Catalog items → raw document properties → expressions → custom shaders →
WASM plugins → the Rust library → pixel passes in and out (EXR). Built-in
items are made from the same pieces and can be ejected into the project and
edited (`effects/`, `presets/`).

### 8.5 Edit example
```json
{"ops": [
  {"op": "add", "kind": "map", "id": "m1", "projection": "globe",
   "layers": [{"source": "naturalearth:admin0", "feature": "ISO:EGY",
               "stroke": {"trim": {"from": 0, "to": 1,
                          "at": "word:Egypt", "anchor": "peak", "ease": "glide"}},
               "fill": {"pattern": "hatch", "angle": 45}}]},
  {"op": "key", "target": "m1.camera",
   "to": {"bbox": "ISO:EGY", "pitch": 20}, "at": "word:Egypt", "ease": "push"},
  {"op": "add", "kind": "markup", "type": "highlight",
   "target": "ocr:'canal revenue'", "at": "word:revenue", "lead": -3}
]}
```

## 9. Renderer

- **GPU-first** on the existing `blade-graphics`/Vulkan stack used by
  `scene_gpu`. CPU paths only as fallback.
- **Layer graph with caching**: each layer caches its last output keyed by
  its inputs; stepped layers reuse held frames; map tessellation, label
  projection and textures cache per zoom band so a fly-to only changes the
  camera transform (follows the project's performance rules).
- **2D**: SDF shaders for primitives; a compute-based path renderer
  (Vello-style) for bezier paths, glyphs and morphs; boolean groups via
  stencil or SDF combination.
- **2.5D/3D**: rasterised scene with depth sorting of planes and meshes;
  unlit and flat-shaded materials first; ray-traced shadows and PBR later
  (blade supports ray queries on the RTX path).
- **Effects** as compute passes; blur through the downsampling `blur_plane`.
- **Motion blur**: shutter-angle accumulation of sub-frame samples on the
  GPU; per-scene switch (BOUNC needs ~180°, BeyondTheBlack wants none).
- **Media**: hardware decode into GPU textures; recording frames reuse the
  existing decoders and UI CNN upscaler.
- **Output**: GPU frames straight to NVENC (or software encoders); audio mix
  through the existing processing chain with ducking.
- **Preview**: half-resolution proxy while scrubbing; full quality when
  paused; same code path as export so preview equals render.
- **Determinism**: time is the only input; seeded randomness; no clock or
  network during render.
- **GPU images in the editor** go through `Studio::retire_image`.

Targets: 1080p60 preview in real time for 2D/2.5D scenes; 2D exports faster
than real time. Measured reference: a CPU prototype rendered a 16.3 s flat
ident at 60 fps with 5× motion blur in 27 s; the GPU path should be well
under that.

### 9.1 Incremental rendering

Renders reuse work whose inputs did not change, like an incremental compiler:
frames, then render passes (mesh pass, isolated groups), then upstream stages
(Blender, audio mix) are cached by a hash of all their inputs, so an edit
re-renders only the frames and passes it affects. Correctness rests on the
keys being complete, not on tracking edits. Full plan with phases and
acceptance: `notes/MOTION_INCREMENTAL_RENDER_PLAN.md`.

## 10. Quality loop

1. Visual script approved.
2. Build scene by scene; after each, `check` and a contact sheet at its
   sync points.
3. `check` rules: sync drift > ±1 f against requested anchors, holds over
   the pack limit, text outside title-safe, contrast, overlapping labels,
   missing licences, missing credits.
4. Motion lint: measured overshoot, settle time and stagger compared with
   the chosen preset (the study scripts already measure these).
5. Draft render, person review, final render. Credits generated from asset
   licences (OSM, NASA, archives).

## 11. Licensing and provenance

**Deferred**: where assets come from is not decided (section 0). What
follows is the research to inform that decision; build none of it yet
except recording source and licence fields on assets.

Every asset records source URL, licence and required credit. Fetchable
without a person: Natural Earth (public domain), OpenStreetMap (ODbL, credit
required), NASA imagery, Library of Congress free-to-use sets, Wikimedia
items with compatible licences, Our World in Data. Needs a person: Mapbox
(user key and terms), Google Earth Studio exports, news and agency photos,
licensed footage and music, fair-use decisions, disputed borders. Export
refuses unlicensed assets unless the person approves them.

## 12. Build phases and benchmarks

Each phase ends with an **original** benchmark video measured with the study
scripts against the reference numbers.

| Phase | Scope | Benchmark |
|---|---|---|
| 1. Time and motion core | project format, keyframes, presets, additive tracks, stepped time, pre-roll, marker tracks, anchors, audio analysis, `check`, `snapshot`, MCP skeleton | a 20 s beat-synced kinetic-type promo |
| 2. GPU 2D layer | shapes, paths, booleans, morph, text animators, markup, instancer, effects, motion blur, transitions | IMEX-style flat ident (2D parts), collage piece |
| 3. Evidence layers | documents and OCR, charts, maps (vectors, labels, routes, geo camera, tiles), looks (paper, grain, monitor) | 60 s Vox/Harris-style explainer with VO |
| 4. Lahza recordings | recording layer, cursor from real input, click transitions, zoom-through into recorded UI, OCR on recordings | 45 s product demo from a real recording |
| 5. 2.5D and stylised 3D | comps as planes, perspective camera, unlit mesh primitives, depth sort, squash/stretch, DOF | full IMEX-level ident with re-reading morphs |
| 6. Photoreal | PBR, area lights, glass, glTF, rigid bodies | BOUNC-style shot |

### 12.1 Phase 1 specification

**Deliverables**
1. `crates/motion-core`
   - Document types with serde (section 6): `Project`, `Scene`, `Layer`
     (phase 1 kinds: `shape` with SDF primitives, `text` with per-glyph
     animators, `image`, `comp`), `Transform`, `Property<T>`.
   - `Property<T>`: constant, keyframes (each with curve, sync anchor,
     lead), additive tracks, optional `step_fps`; `value_at(t, &Markers)`.
   - Curves: every preset in section 5 plus raw cubic-bezier, power-n,
     exp-lag (r or τ), spring (ζ, f), hold; log-space interpolation for
     scale. Each preset defined once with its measured numbers.
   - Time references (section 4.3) parsed and resolved against marker
     tracks; anchor `start|peak|land|end`; `peak` found numerically from
     the curve's velocity.
   - Edit operations (section 8.5) applied atomically with an inverse for
     undo; stable ids.
2. `crates/motion-audio`
   - Decode audio through ffmpeg; BPM and first downbeat; straight and
     triplet grids; per-band onset strength; `hit`, `stop` (silence→hit)
     and `drop` (bass dropout) detection; writes marker tracks.
   - Validate against the studies: Motion Array 90 BPM (beat 16.67 f at
     25 fps, first downbeat 1.093 s), IMEX 110 BPM, BeyondTheBlack 133.3
     BPM with triplet feel, BOUNC impact onsets.
3. Word markers from the existing `crates/transcription` output.
4. `crates/motion-render` (phase 1 subset): GPU SDF shapes, glyph rendering
   for text, image layers, nested comps, Gaussian blur, shutter motion blur,
   stepped-time frame caching; render to RGBA and to the existing encoder.
5. CLI: `lahza motion check <project>`, `snapshot <project> --at <refs>
   [--sheet]`, `render <project> -o out.mp4`.
6. `crates/motion-mcp`: tools `project_create`, `project_open`,
   `project_info`, `timeline_get`, `markers_query`, `edit_apply`, `check`,
   `snapshot`, `render`, `render_status`, `history`, `audio_analyze`;
   resources `lahza://schema`, `lahza://catalog`, `lahza://guide`.

**Acceptance**
- Determinism: rendering the same project twice gives identical frames.
- Curve tests: each preset reproduces its measured numbers (overshoot %,
  settle frames, 90%-complete frame) within ±1 frame / ±1 percentage point.
- Sync tests: an anchored keyframe's `peak` lands within ±0.5 frame of its
  marker after `lead`.
- Performance: a 1080p60 scene with 200 animated shapes and text previews
  in real time; the 20 s benchmark exports faster than real time on the
  RTX 3060.
- Benchmark: an agent using only the MCP tools builds an original 20 s
  kinetic-type promo synced to a supplied music track; `check` reports no
  sync drift over ±1 f.
- Follow the project rules in `CLAUDE.md`; tests run in CI.

### 12.2 Phase 1 status

Built: `crates/motion-core`, `motion-audio`, `motion-render` (Vulkan compute
+ CPU fallback with identical math), `motion-mcp` (`lahza mcp`, `lahza
motion …`). Usage, verification results, measured performance and what is
still missing are in [`docs/motion-engine.md`](../docs/motion-engine.md).
Open against the acceptance list: real-time 1080p60 and faster-than-real-time
export need measuring on the RTX 3060 (only llvmpipe and a 4-core CPU were
available while building); the agent benchmark exists as the scripted test
`crates/motion-mcp/tests/promo.rs` and still wants a run by a live agent
over MCP. Next: the editor UI for motion projects (phase 1b), then phase 2.

## 13. Open questions

- Asset sourcing (maps, archives, data, TTS voice, music): deferred, see section 0.
- Whether projects live inside `.lahzarec` folders or a new `.lahza` folder
  that references recordings.
- How much of the editor UI exposes the new layer kinds in phase 1 versus
  agent-only access.
- Fonts: which open fonts ship with the style packs.
