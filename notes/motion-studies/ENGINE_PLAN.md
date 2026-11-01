# Lahza motion engine: what the six references require

Sources (full reports beside this file): `imex.md`, `bounc.md`, `motion-array.md`,
`beyond-the-black.md`, `showreel.md`, `collage-hacks.md`.

## 1. Cross-video findings

### Timing and music
Three sync modes appear; the engine must support all three and an agent must choose per section.

| Mode | Where | Evidence |
|---|---|---|
| **Beat grid** — events snap to beat / half / 16th | Motion Array (90 BPM, 21/26 events ±1 f), IMEX (110 BPM, one scene per bar) | cuts land ~1 f *before* the beat |
| **Hit sync** — events snap to audio onsets, not a grid | BOUNC (12/12 impacts within −0.6…+2 f), Showreel stop→hit points (79% ±1 f) | no pulse in the music during the synced section |
| **Free groove** — cuts inside a steady groove are not on the grid | BeyondTheBlack (Rayleigh p≈0.81), Showreel groove cuts | ~1 cut per 1.5–2 beats |

Rules that hold across videos:
- **Sync the peak, not the start.** IMEX tilt peak velocity on the beat; BeyondTheBlack ring brightness peak within 0.3 f of the downbeat while its cut is 3.7 f earlier. A keyframe needs a sync anchor: start, peak velocity, landing, or end.
- **Lead of about −1 f** for cuts (Motion Array, Showreel); up to −3 f at phrase boundaries (BeyondTheBlack).
- **Structure comes from the audio's shape**: stop→hit moments, bass dropouts of about one beat before section downbeats, and the largest hit carrying the logo (Motion Array T936; IMEX M completes on the last hit). BPM alone is not enough.
- **Grids need triplets and accelerando** (BeyondTheBlack 133 BPM triplet feel; IMEX tunnel births 8,8,7,5,5,4 f).
- **Music can be trimmed hard to picture** (BeyondTheBlack stops off-grid at 17.02 s).

### Motion vocabulary (measured)
| Preset | Model | Numbers | Seen in |
|---|---|---|---|
| `snap` | exponential lag, keep r of remaining distance per frame | r≈0.5 (90% in ~3 f) | Showreel pops |
| `glide` | exponential lag | r≈0.7–0.79, τ≈3.4–4.2 f, 90% in 6–9 f | Showreel, IMEX, BeyondTheBlack |
| `settle` | exponential lag | r≈0.8, τ≈6 f | Showreel, BeyondTheBlack |
| `enter` | ease-out quart | 6–12 f, 90% in 4–6 f | Motion Array |
| `gesture` | symmetric ease-in-out | 5–8 f, peak speed ~44–50% | press, riffle, wipe, zoom-through |
| `pop` | stiff spring on scale only | ζ≈0.61, ω≈40 rad/s, 9% overshoot, settled ≤6 f | Motion Array |
| `land` | soft spring | ζ 0.35–0.55, 1.6–2.8 Hz, 6–17% overshoot | IMEX |
| `squash` | spring on a squash deformer | ~5 Hz, ζ≈0.3, 17 f charge-up | BOUNC |
| `bounce` | hand-shaped arcs, not physics | rise/fall exponent ≈2.75, each bounce 0.43× the previous | BOUNC |
| `slam` | accelerate into a dead stop | t² into the hit | BeyondTheBlack opening words |
| `zoom` | interpolate scale in log space | 6–10× zooms | Motion Array, IMEX tunnel |

Global habits:
- **Entrances start at full speed** (no ease-in). Overshoot is rare and reserved for scale and landings.
- **Nothing stops dead.** After settling, drift continues at ~0.5–2% scale per frame (Motion Array, BeyondTheBlack, IMEX). Requires *additive* animation: entrance + drift on one property.
- **Shots open mid-move and cut mid-move** (Showreel: 1–7 f of pre-roll, holds of only 2–10 f).
- **Staggers are tiny and exact**: 1 f (Motion Array), 3 f bars / ~2.7 f cards (Showreel).
- **Speed lines come from motion blur** (BOUNC 180–200° shutter; Motion Array whips), or are deliberately absent (BeyondTheBlack crisp look). Blur must be a per-shot switch.
- **Stop-motion = stepped time, not jitter.** Collage builds at 8 fps (holds 4,4,4,3 at 30 fps), type and strokes at 12 fps (3,2,3,2); held frames pixel-identical.

### Transitions
- **Continuity beats cuts.** IMEX has one near-cut; scenes turn into each other by re-reading the same 3D object from a new camera angle (pillar → tube → disc → hemisphere).
- **Zoom-throughs into nested scenes**, whip pans, object wipes, turntable spins (Motion Array: only 8 hard cuts in 41 s).
- **The click is the transition** (Showreel): cursor travels straight, dwells 0–10 f, the button shrinks to 0.78–0.84 over 4–5 f, and the cut or zoom-through fires at the bottom of the press; button → video window → full frame in 8 f.
- **Action-matched cuts** over one continuous action (BOUNC): exit through frame edge or blur, enter matching direction.

## 2. Feature coverage

✔ = needed by that reference (M = must in its report).

| Feature | IMEX | BOUNC | MotionArr | BtB | Showreel | Collage |
|---|---|---|---|---|---|---|
| Curve library + springs + exp-lag + log-scale | M | M | M | M | M | M |
| Additive layers (entrance + drift), pre-roll | ✔ | | M | M | M | |
| Stepped time per layer/group | | | ✔ | | ✔ | M |
| Audio analysis: grid (incl. triplets), onsets, stop→hit, dropouts; snapping with sync anchor + lead | M | M | M | M | M | |
| Vector shapes (SDF primitives, bezier paths), strokes + trim paths | M | | M | M | M | ✔ |
| Masks/mattes, iris, rounded-rect clips | M | | M | M | M | M |
| Boolean ops with designated fills, smooth (gooey) union | M | | | | | |
| Path morph | ✔ | | ✔ | | ✔ | |
| Per-glyph/word text animators, typewriter, counters, label boxes | | M | M | M | M | M |
| Instancer / repeater with stagger and field effectors | M | ✔ | M | ✔ | M | |
| Motion blur (shutter angle, incl. rotation) | ✔ | M | M | opt | M | |
| Glow/bloom, gradients, glass background blur | | | ✔ | M | M | |
| Grain, Gaussian blur stack, halftone, tint/gradient map | | | | | ✔ | M |
| 2.5D: comps as planes, perspective camera, whip + drift, depth sort | M | | M | ✔ | M | |
| 3D unlit primitives (cylinder, sphere, hemisphere, capsule) + flat shadows | M | M | | | | |
| Squash/stretch along velocity | M | M | ✔ | | | |
| Depth of field | | ✔ | ✔ | ✔ | ✔ | |
| Media layers (video/image in masks), nested comps, zoom-through | | | M | ✔ | M | M |
| Cursor primitive (path, dwell, press, ripple, transition-on-press) | | | ✔ | | M | |
| Multi-camera shot sequencer over one action | | M | | | | |
| PBR + glass + glTF meshes | | M | nice | ✔ | nice | |
| Particles | ✔ | ✔ | ✔ | ✔ | ✔ | |

## 3. Build order

**Phase 1 — Timeline and timing (everything depends on it)**
- Keyframes per property with the preset table above as named curves, plus raw cubic-bezier, power-n, exp-lag (r or τ), spring (ζ, f), log-space scale.
- Additive tracks (entrance + drift + wiggle), hold keys, per-layer/group stepped time with cached held frames, clip pre-roll.
- Audio analysis → marker tracks: BPM + offset grid with straight and triplet subdivisions, onset strength per band, stop→hit and bass-dropout detection, music start/end.
- Snapping with a **sync anchor** per keyframe (start / peak velocity / landing / end) and a lead offset (default −1 f).

**Phase 2 — GPU 2D layer (the bulk of all six references)**
- SDF primitives (circle, rounded rect, capsule, ring, arc) for speed; bezier paths via a GPU path renderer (Vello-style) for letters, blobs and morphs.
- Strokes with trim paths and caps; masks/mattes; boolean ops with designated fills; smooth union; path morph with vertex correspondence.
- Text: per-glyph/word animators with range and stagger selectors, typewriter (with re-centring), counters, label boxes.
- Instancer/repeater with time offsets and field effectors.
- Effects stack: blur, glow/bloom, glass background blur, gradient fills, grain, halftone, tint.
- Shutter-angle motion blur (sub-frame accumulation on GPU).

**Phase 3 — 2.5D and stylised 3D**
- Nested comps as planes in a perspective camera space, depth sorting, whip + drift camera moves, zoom-through.
- Unlit flat-shaded primitives (cylinder, sphere, hemisphere, capsule) with per-surface colour and hard flat shadows — the IMEX "morph by reinterpretation".
- Squash/stretch deformer along velocity; depth of field.

**Phase 4 — Lahza's advantage**
- Screen recording and webcam as media layers; real cursor path, dwell and clicks from `input.json` driving the cursor primitive; "click is the transition" from real clicks; zoom-through from recorded UI.

**Phase 5 — Photoreal (BOUNC level)**
- PBR, area lights, contact shadows, glass/refraction, glTF meshes, rigid bodies. Largest effort; only one reference needs it as a must.

## 4. Not engine work
3D characters and props (BOUNC sculptures, Showreel illustrated worlds, Motion Array characters), filmed footage, illustrations, scanned paper and photos, logos. These need artists, stock, or AI asset generation.

## 5. How to validate
Each report gives numeric targets (overshoot %, settle frames, τ, stagger, sync ±f). Build an **original** benchmark shot per reference in the engine, measure the same quantities from our render with the same scripts, and compare.
