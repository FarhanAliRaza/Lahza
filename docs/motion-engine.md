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
`render`, `render_status`, `history`, `audio_analyze`, `media_import`,
`blender_run`, `recording_info`, `recording_place`, `recording_edit`,
`recording_export`, `sfx_add`, `recording_sound_add`.
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
lahza motion sync-check promo.lahza --tracks beat    # events off the beat; --all lists every event
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

## Sound effects

Sound effects are clips with ids, placed by any time reference, so an
effect anchored to an animation (`"#card.start - 3f"`) moves when the
animation moves; `set` and `remove` work on them by id, with undo. Fourteen
effects are built in, synthesised at 48 kHz with nothing to source or
license: whoosh, whoosh-out, swipe, pop, click, tick, type, thud, hit, boom,
riser, ding, sparkle and glitch (`lahza://catalog` lists what each is for).

- **Motion editor:** one row per sound with its waveform. **Add sound**
  places an effect at the playhead (on a selected layer's start it anchors
  to that layer). Drag a sound to retime it: it snaps to a layer's start or
  end and then follows that layer, or to a key; Shift snaps to beats.
  Drag a bar's ends to trim it and the dots on its top corners to fade it
  in and out (the envelope is drawn over the waveform); trimming the start
  of an anchored sound keeps the anchor (`#card.start + 0.120`). Selecting
  one shows its time, what plays of the file, gain (±3 dB), fade in and out
  (50 ms steps), Reset trim and fades, and Delete.
- **Video timeline:** **+ Media → Sound effect…** adds one at the playhead
  on the Music / SFX lane, where it can be moved, trimmed and faded like any
  audio clip.
- **Agents:** `sfx_add` in a motion project; `recording_sound_add` on a
  recording, whose `recording_info` lists the clicks, keystrokes and zooms
  in edited-video seconds. The studio shows an agent's sounds live.

## Motion inside screen recordings

Motion designs live inside the recording they belong to, at
`<recording>.lahzarec/motion/<name>.lahza/`. The recording's edit document
lists where each plays (`motionPieces`: start on the edited video,
placement, opacity). Pieces share the recording's files: `../../screen.mkv`
is the raw recording, and media imported into a piece goes to the
recording's `media/` folder.

- **Over the video** pieces are drawn in the recording's own pixels, under
  the annotations, so the video's zooms, crop and camera moves carry them.
  Titles, callouts, highlights, stickers.
- **Full screen** pieces cover the frame like an inserted clip: intros,
  chapter cards, outros. A piece can use the recording itself as footage.

In the studio the complexity stays out of the way: a **Motion** lane shows
one bar per piece (drag to move it), and selecting one shows its placement,
opacity, **Edit design** and Delete. **Edit design** opens the motion editor
in the same window, with the recording under an over-the-video piece as a
guide, and **‹ Video** returns. Pieces play in the preview with their
sound, and export renders them at export size. An agent adds and moves
pieces with `project_create {recording}` and `recording_place`; the studio
picks the change up live.

## Editor

`lahza project.lahza` opens a standalone motion project in the editor; pieces
inside a recording open from the studio instead:
- **Preview:** rendered on a worker thread (GPU when available), with a half-size proxy while playing or dragging.
- **Timeline:** the ruler, beat/bar/hit and word markers, the sound's waveform, scenes, and layer bars with key diamonds.
- **Sound:** music, narration, effects and footage sound play in sync while playing.
- **Inspector:** scrubbable transform values (3D values on 3D layers).
- **Shortcuts:** Space plays, arrows step frames, Home/End jump, Delete removes the selected layer, Ctrl+Z / Ctrl+Shift+Z undo and redo.

Click a layer in the preview or the timeline and drag it to move it. Drag a value to change it; on an animated property this adds a key at the playhead. Drag a key diamond to retime it; hold Shift to snap it to the nearest beat, which writes `beat:N` so the key stays in sync.

Every edit goes through the same tools the MCP server uses, so a person and an agent share one history and undo stack. Edits an agent makes over MCP appear live.

## 3D objects in the engine

`mesh` layers are real lit 3D objects rendered by the engine itself, live in the preview and identical on GPU and CPU: spheres, rounded boxes, cylinders and cones, capsules, tori and torus arcs split into coloured segments, outlines extruded with bevels (logos, letters, coins), profiles revolved about an axis, and tubes swept along paths that grow with `trim`. Materials are physically based (presets glossy, coated, metal, chrome, glass, emissive, matte, flat, shadow; roughness, clear coat, per-part materials) under the scene's lights and a procedural studio environment. Lights cast soft shadows (CPU-built shadow maps, filtered on the GPU), glass refracts and frosts what is behind it, and flat 3D layers join the meshes' depth test as cards. Meshes next to each other share a depth buffer, so they intersect correctly; they sort by depth with 2.5D planes and take effects, mattes, depth of field and motion blur like any layer. The display list bins each batch's triangles into the renderer's 16×16 tiles; each pixel finds the nearest triangle at 8 samples (watertight edges) and shades each winner once.

## 3D with Blender

`blender_run` runs Blender headless. It takes an agent-written `bpy` script and/or a `.blend` file, and renders a transparent PNG sequence into `assets/blender/<name>/`. A `video` layer then places it, so 3D assets composite with type, shapes, effects, mattes and 2.5D layers in one timeline. Blender handles real 3D (geometry, lights, materials); Lahza handles motion, timing and composition. Cycles works without a display. EEVEE is much faster with a GPU and a display.

## Compared with After Effects and browser editors

| Capability | Lahza motion |
|---|---|
| Keyframes, easing, springs | ✔ measured presets plus raw curves; moves anchored by start, peak, land or end to markers |
| Audio-driven timing | ✔ automatic beats, bars (3/4 and 4/4), tempo maps, hits, stops, drops and words |
| Wiggle and loop expressions | ✔ drift, wiggle and sine additive tracks; no general expressions yet |
| Shape layers, trim paths, **morphs** | ✔ primitives and Bézier paths with fill, stroke and trim; keyed path morphs between any shapes |
| Text animators | ✔ per glyph, word or line, with stagger orders and ranges; HarfBuzz shaping and bidi |
| Precomps, parenting, time offset | ✔ comps render as one picture when they need to; parenting; time offsets; stepped time |
| Track mattes | ✔ alpha, luma and inverted |
| **Footage** | ✔ video and image sequences with alpha: trim, speed, loop, time remap, sound in the mix |
| **Blend modes** | ✔ all of AE's separable and non-separable modes (19) |
| **Effects** | ✔ blur, shadow, glow, exposure, brightness/contrast, levels, hue/saturation, tint, gradient map, invert, fill, `.cube` LUT, displacement, directional blur, RGB split, vignette, grain |
| Transitions | ✔ fade, push, slide, zoom, wipe and whip |
| Motion blur | ✔ shutter angle per scene |
| **3D** | ✔ lit 3D meshes in the engine (primitives, extrude, revolve, sweep; PBR materials, lights, environment, depth buffer) plus 2.5D layers (z, x/y rotation, perspective camera with orbit and dolly, depth sort); Blender for fluids and complex models |
| **Timeline UI** | ✔ editor window (see above) |
| Masks on a layer | ◐ through track mattes; no per-layer mask paths with feather |
| Expressions, scripting | ✘ (planned: `code_put` for expressions, shaders, WASM) |
| Particles, repeaters, booleans, text on a path | ✘ |
| Motion tracking, rotoscoping, keying | ✘ |

Against Chrome-based editors (Remotion, Hyperframes and other React or canvas tools), Lahza has these advantages:
- Native GPU compute rendering, with the CPU path drawing the same picture.
- Deterministic frames, and exports that pipe straight to a hardware encoder.
- Timing derived from the audio.
- Sync checked frame by frame.
- One project format that agents and people edit together.

## Known limits

- **Editor:** it doesn't yet edit text or effects in the inspector; agents and JSON do that. There are no graph editor, multi-select or copy/paste yet.
- **Shadow and glow on a matted layer:** they use the unmatted picture.
- **Trim on a filled path:** it trims only the stroke.
- **3D depth:** meshes and flat 3D layers in one layer list share a depth test (flat layers as alpha-tested cards), so they intersect in any order; 3D layers in different comps still sort by origin. Cards are cut at half opacity: a plane with layer opacity below 1, a blend mode or effects is drawn as before (sorted by origin).
- **Meshes:** shadows are shadow maps (soft, from directional and point lights); there is no global illumination or caustics. Glass refracts what is drawn before it in its layer list (screen space: nothing outside the frame, no glass behind glass).
- **Motion pieces:** an over-the-video piece takes the recording's shape; the
  editor's guide shows the raw recording from the piece's start, so cuts
  inside a piece's range are not reflected there. Speed-changed footage and
  music ducking play only in exports.
- **Missing design tools:** `code_put`, `voiceover`, `ocr` and `assets_search`/`assets_fetch`. These depend on asset sourcing, which is undecided, or on later phases.
