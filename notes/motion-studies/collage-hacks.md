# "3 hacks for Collage Style Animation": study notes

Source: `/home/farhan/Downloads/3 hacks for Collage Style Animation.mp4`
Working files (frames, sheets, scripts): `/tmp/claude-1000/-home-farhan-code-screendrop-rs/b6053da9-9a03-4b7b-96d5-1df3ace8dae5/scratchpad/study/collage/`
(sheets are in `frames/`, measurement scripts in `scripts/`, per-frame difference table in `diffs.tsv`, transcript in `transcript.srt`).

Method: whisper.cpp (ggml-base) word-level transcript, 1 fps overview sheet, per-frame
mean-absolute-difference over the whole clip (180x320 gray), so holds and step cadence are counted
rather than eyeballed. Full-frame-rate sheets around each demo, normalised cross-correlation
tracking of elements, and full-res crops of every After Effects (AE) panel shown. I also
sampled pixel colours and noise statistics with numpy.

Frame indices below are 0-based at 30 fps (frame n = n/30 s). ffmpeg's `select` and the
`-frame_pts` numbering differ by one frame in a couple of places, so treat individual frame
numbers as ±1. Cadences and spacings are exact.

---

## 1. Overview

- 49.96 s, 720x1280 vertical, 30 fps H.264 (some inserts are 24 fps footage with every 5th
  frame duplicated), AAC 44.1 kHz stereo.
- It is a talking-head tutorial with a voiceover throughout and burned-in word-by-word captions
  (white bold sans in a translucent grey pill, lower third). It intercuts collage demos, AE
  screen captures, hand-cutting B-roll credited "@MrCutandStich" and a presenter to camera.
- **Thesis (0–18.8 s):** "Collage style animation is all about layering. And without these three
  details, it can look pretty flat and digital … perfect collage usually looks awful. That's
  because the animation style traditionally comes from cutting, stacking, and assembling things
  by hand."
- **The three hacks:**
  1. **Tint the paper off-white** (18.8–25.1 s). AE *Tint*, Map White To H60° S6% B90%, which
     lowers and warms the highlights so stacked paper layers separate.
  2. **Fragment one photo** (28.9–41.3 s). Cut one image into mask fragments, offset them
     slightly so they overlap, drop their opacity to 82%, and weave text and shapes between the
     pieces for an "assembled editorial look".
  3. **Texture variance** (41.3–49.9 s). Give each layer a different texture: paper grain,
     halftones, or simply AE *Noise HLS* (Grain, Lightness 53%) plus *Gaussian Blur* (0.6 → 11.8)
     on different layers.

Timeline map (measured from the cut list and the difference table):

| t (s) | frames | content | motion cadence |
|---|---|---|---|
| 0.0–3.8 | 0–113 | Hero collage builds on: title, newspaper scraps, photo, pink phone, word labels, arrow | **8 fps stepped** (holds 4,4,4,3 = 30/8) |
| 3.8–6.8 | 114–204 | Counterexample: phone alone on pure white, typed "FLAT and DIGITAL" | 24 fps smooth, text typed per word chunk |
| 6.8–10.3 | 205–308 | Counterexample: "perfect" clean vector collage (green circle, flowers, suit man, waves, dots); dimmed to grey, "LOOKS AWFUL" | smooth slow push |
| 10.3–15.3 | 309–457 | Real hand cutting with a scalpel (B-roll, 24 fps) | — |
| 15.3–18.8 | 458–563 | Presenter to camera (30 fps) | — |
| 18.8–25.1 | 564–752 | **Hack 1** demo and AE capture | — |
| 25.1–26.8 | 753–803 | Hero collage build replayed | 8 fps stepped |
| 26.8–28.9 | 804–867 | Presenter | — |
| 28.9–41.3 | 868–1238 | **Hack 2** (Capitol photo) | smooth ease *and* 8 fps stepped fly-in |
| 41.3–49.9 | 1239–1496 | **Hack 3** (texture plates, Supreme Court collage) | 12 fps stepped titles and strokes, plus smooth 24 fps pushes |

---

## 2. The hacks in detail

### Hack 1: Tint the paper and push its highlights off-white

**Spoken (18.96–26.96 s):** "First, if I'm using paper, I'll tint it, and push the highlights
slightly off white. It's subtle, but it adds depth, especially when you have multiple paper
layers."

**AE recipe as shown on screen:**
- Effects & Presets search "Tint" → *Color Correction ▸ Tint* (f630, 21.0 s) is dragged onto the
  scanned-newspaper fragment layer. The layer is a torn scrap of old book/newspaper text.
- Next the *Map White To* colour picker opens (f678–752, 22.6–25.1 s). The user drags from
  **H 60°, S 0%, B 100%** (16-bpc RGB 32768/32768/32768, which is pure white) to
  **H 60°, S 6%, B 90%** (16-bpc RGB 29424/29426/27626, about **#E5E5D7**). Map Black To is never
  touched, so it stays at the default black.
- Tint's Amount to Tint is never shown changing, so it is presumably the default 100%.

**Measured before and after.** Paper pixels are sampled from the 60th–95th luminance percentile
of the scrap, which excludes the ink:
- Untinted (picker at white, f700): paper **#D8D9D3**.
- Tinted (H60/S6/B90, f750): paper **#C6C7B9**, about 8.5% darker, and its blue channel falls
  from 211 to 186 (warm cream).
- The grey paper background behind the scrap is unchanged (#AAAAAA), so the effect is per
  layer. That is the point of the hack: the scrap now sits visibly on its own plane instead of
  merging with the other light layers.

**What it does:** it is a two-point gradient map (luminance → lerp(black, tintWhite)). It
compresses the top of the range so no paper layer reaches pure white, and each layer gets a
slightly different warm off-white. With several stacked papers this produces tonal separation,
which reads as depth without shadows.

**Before/after context:** the "flat and digital" counterexample (3.8–6.8 s) is the phone
cut-out on pure #FFFFFF with no paper and no texture.

**Engine primitive:**
- `tint` / `gradient_map` per layer, with `map_black` and `map_white` colours and an `amount`.
- In linear or sRGB space: `out = mix(map_black, map_white, luma(in))`, then
  `mix(in, out, amount)`.
- Agents should get an HSB colour input, because "H60 S6 B90" is how artists talk about it.
  A helper `off_white(hue_deg, sat≈0.03–0.08, bright≈0.88–0.94)` makes the off-white.
- Cheap: a single-pass per-pixel LUT, cacheable per static layer.

### Hack 2: Fragment one photo, overlap the pieces, lower their opacity, weave in text and shapes

**Spoken (27.0–40.6 s):** "Now photos are where it gets a little more interesting. I like to cut
one image into fragments, slightly overlap them, and change their opacity. And then between the
pieces, I'll weave in text and shapes, and it gives you this assembled editorial look."

**Construction, as shown:**
- **Source:** one vintage magazine-cover scan of the US Capitol (masthead "The Capitol …
  Symbol … Freedom", red band at the bottom).
- **Fragment shapes (f880–940):** dashed outlines are drawn over the photo. They are simple
  geometric masks:
  - arch / round-top "tombstone" rectangles,
  - quarter- and half-circles,
  - plain rectangles,
  - lobed shapes (a scalloped club shape).

  There are roughly 6–8 pieces, all cut from the same image.
- **AE layer stack (f997–1041):** many duplicates of the same footage, named
  "Capital… to Cut.tif". Each layer has fx enabled (masks), Mode Normal and no track matte.
  Opacity is scrubbed on all of them together from 100% to **82%**. With 82% opacity,
  overlapping duplicates build up density, so the seams show as faint lighter and darker
  bands, and the dark background shows through a little.
- **Overlap offsets:** in the "overlap them" beat (f945–995) the pieces drift apart and come
  back with a smooth ease (24 fps, no stepping).
  - Normalised cross-correlation tracking puts the dome piece about **44 px** (≈6% of frame
    width) left and 8 px up at maximum, and a lower-left piece about **45 px** left.
  - Ease-out over about 16 frames (0.53 s), hold about 8 frames, ease back over about 12 frames
    (0.4 s).
  - In the final state the pieces stay slightly misregistered: the dome appears doubled and edge
    steps are visible (f962, f1225).
- **Woven elements:**
  - A large black serif glyph-like shape (curly, like a giant "S"/"3" swash) sits *between*
    fragment layers (f950–990), so some pieces cover it and it covers others.
  - Then the italic serif title "**Editorial Look**" (light grey, ~70 px cap height, two lines,
    left-aligned) is placed behind and over pieces. In AE it is shown with red layer-handle
    squares (f1050–1100).
- **Background:** near-black #101010, flat. There are a few slightly lighter dark-grey paper
  panels behind the cut-out, so the background is not pure black.
- **Final assembly (f1131–1170, 37.7–39.0 s):** fragments fly in from scattered, enlarged
  (about 2–3x) and rotated positions.
  - The motion is **posterized to 8 fps**: changes land on f1132, 1136, 1140, 1144, 1147, 1151,
    1155, 1159, 1162, 1166, 1170, i.e. holds of 4,4,4,3,4,4,4,3,4,4.
  - Mean frame difference per step decays 53 → 37 → 41 → 29 → 19 → 14 → 10 → 8 → 6 → 3 → 2, a
    strong ease-out (expo/quart-like) settling over about 1.3 s.
  - "Editorial" then pops on in one frame at f1170 with no fade, and "Look" follows a step later.

**Engine primitives:**
- **Shape masks on duplicated image layers.** A cheap way to express this is `fragment(source,
  shapes[])`: each fragment references the same texture and has its own mask path and its own
  transform offset from its "home" position. Internally it is one texture with N masked quads,
  with no N copies of pixels.
- **Per-layer opacity with Normal over-compositing**, so overlaps double up (82% → about 97%
  coverage where two overlap).
- **Arbitrary z-interleaving** of text and vector shapes between fragments, so the layer order
  must be free and not grouped by type.
- **Stepped (posterized) time per layer or group.** Evaluate the eased animation at
  `floor(t*fps_step)/fps_step` with fps_step = 8. The visual cadence is the stepping of an eased
  curve, not jitter.
- **Smooth eases** for the "drift apart and back" (ease-out/in-out, 0.4–0.6 s).

### Hack 3: Texture variance (paper grain, halftones, noise and blur per layer)

**Spoken (40.6–49.2 s):** "And then a little texture variance can help sell it too. Paper grain,
halftones [whisper: "haptomes"], or even just stacking a bit of blur and noise to different
layers, can get you what you need."

**Shown, beat by beat:**
- **Title card (41.3–43.0 s).** "TEXTURE VARIANCE" is typed in chunks on a black label (white
  bold grotesk). Chunks land at f1243 "T", f1245 "TEXTURE", f1248 "TEXTURE VARI", f1250 "…VARIAN",
  f1251 "…VARIANC", f1254 "TEXTURE VARIANCE". Holds alternate 2 and 3 frames, which is **12 fps
  stepped**.
  - Behind it, three diagonal paper strips sweep in from the left, also at 12 fps steps with an
    ease-out: a near-black grainy strip, a mid-grey speckled strip and crumpled white paper.
  - The texture variance is the visual itself: three tones, three surfaces.
- **"Paper grain" (43.0–44.1 s).** A full-frame pale grey paper plate (≈#CACCCA). The grain
  measures high-pass σ≈1.8/255 with overall σ≈4/255, so it is very low contrast. It moves
  smoothly with a slow push or brightening (24 fps), and the grain itself does not re-randomise.
- **"Halftones" (44.1–44.7 s).**
  - The Supreme Court building is cut out, carries a visible halftone and sits on cream newsprint
    (paper **#E9E8D4**).
  - The newsprint carries large faded serif body copy at low contrast, with letter-spaced words
    ("a  p  a  r  t").
  - Then a gabled ("house"-shaped) grey halftone/mottle plate (f1344–1356) is shown on its own
    as the texture layer, and is then revealed as the building.
- **"Or even just stacking a bit of blur and noise" (45.4–47.3 s).** An AE Effect Controls capture
  on a sky-photo layer:
  - **Noise HLS**: Noise = *Grain*, Hue 0.0%, **Lightness 53.0%**, Saturation 0.0%,
    **Grain Size 1.00**, Noise Phase 0x+0.0° (not animated).
  - **Gaussian Blur**, *below* the noise in the stack: Blurriness scrubbed **0.6 → 3.7 → 7.7 →
    11.8** (f1376 → 1415), Blur Dimensions Horizontal and Vertical, **Repeat Edge Pixels on**.
  - **Measured result on the sky:**
    - at blur 0.6 the high-pass σ is **29.9**/255 (harsh monochrome grain, overall σ 34);
    - at blur ≈11.8 the high-pass σ is **3.5** (overall σ 15.3), so the grain becomes soft
      mottled "printed sky" blotches;
    - the mean colour lifts slightly, from (131,177,188) to (145,183,193).
  - Static frames show zero temporal difference, so the grain is **static, not boiling**.
- **Payoff (47.3–49.9 s).** A stacked editorial collage. From the back:
  1. Cream newsprint with faded serif text, softly blurred and darkened toward the edges
     (≈#C2BFB3 at the edges against #E9E8D4 clean, about 14% darker).
  2. A tilted strip of the noise+blur sky.
  3. The halftoned building cut-out with a hedge strip.
  4. Serif text "Can get you / what you need".
  5. A **red hand-drawn ellipse** plus a black hand-drawn arrow.

  The ellipse and arrow draw on at **12 fps** (changes at f1433, 1435, 1438, 1440, 1443, 1445,
  1448, 1450, 1453, 1455, alternating 2 and 3 frames). Underneath, there is a smooth 24 fps
  push on the whole frame, so two cadences are layered.

**Engine primitives:**
- **Per-layer procedural noise** (monochrome/HLS grain): amount (lightness %), grain size (px),
  seed, `animate: bool` / phase. It must come *before* blur in a per-layer effect stack, so
  **effect order matters**.
- **Per-layer Gaussian blur** with a repeat-edge mode. In Lahza, route this through `blur_plane`.
- **Texture plates:** paper-grain, crumpled-paper and halftone images used as layers, mattes or
  overlays.
- **Halftone filter:** dot screen, angle and cell size. The phone in the intro shows a fine 45°
  screen. FFT on f100 gives the dominant frequency at 45°, period ≈1.5 px diagonal, i.e. about a
  2 px pitch at 720 px width. That is essentially print-scan texture rather than a stylised big
  dot.
- **Edge vignette / darken** on the background paper.
- **Write-on stroke** (trim path) for hand-drawn ellipses and arrows, stepped at 12 fps.

---

## 3. Collage look catalogue (whole video, with numbers)

- **Stop-motion cadence:**
  - **8 fps** for collage builds and fly-ins (holds 4,4,4,3 at 30 fps; that is 3 frames at
    24 fps).
  - **12 fps** for typed titles and hand-drawn strokes (holds 3,2,3,2 at 30 fps; 2 frames at
    24 fps).
  - Eased motion is *sampled* at those rates: the telephone settles 10 px over about 6 steps
    (f21→f44, ≈0.75 s), and the fragment fly-in decays 53→2 over 10 steps.
  - **There is no random per-step jitter on held layers.** Holds are pixel-identical: f82–113 has
    zero difference, and the title text tracks at 0 px offset across every step. The handmade
    feel comes from stepping plus imperfect placement, not from wiggle.
  - Smooth 24/30 fps motion is kept for camera pushes and for the "overlap" drift demo.
- **Build-on by replacement:**
  - Each element appears whole in one frame (no fade, no scale-pop), roughly one element per
    step.
  - Intro order: "Collage" (f0) → "Style" (f10) → newspaper scrap (f14) → photo plus "Free World
    Alliance" headline scrap (f18) → telephone (f20/21) → labels ALL (f44) / ABOUT (f51) /
    LAYERING (f55).
  - The arrow writes on in 4 steps (f59–74) and a loose white hand-drawn ellipse circles
    "LAYERING".
  - The background (grey paper with vertical strips of slightly different tone) is revealed
    top-down in steps (f6–33).
- **Word labels:**
  - Each word is set in white heavy grotesk on its own black rectangle (no padding radius,
    slight per-word rotation and offset). The words are staggered like cut-out ransom-note
    strips.
  - Pop timing leads the spoken words by about 0.25–0.5 s: ALL at 1.47 s against the voice at
    1.71 s, ABOUT 1.70 against 1.94, LAYERING 1.83 against 2.33.
- **Paper:**
  - Background grey paper is #AAAAAA–#B5B5B5 with a fine fibre texture.
  - Off-white tinted scraps are #C6C7B9; cream newsprint is #E9E8D4; the pale paper plate is
    #CACCCA.
  - None of the papers is pure white. Even the counterexamples use pure white, deliberately.
- **Edges:**
  - Torn-paper edges on the newspaper scraps are irregular and slightly ragged, with a light fibre
    rim.
  - Photo cut-outs (telephone, buildings) have clean scalpel-cut silhouettes.
  - The Capitol fragments are clean geometric vector masks (arches, circles, rectangles).
- **Shadows:** a luminance profile across scrap and phone edges finds no clear drop shadow.
  Separation comes from tone (tint), texture and occasional dark rims. Any shadow is ≤1–2 px or
  absent.
- **Print and texture:** fine 45° halftone screen on photos (~2 px pitch at 720 w); static
  monochrome grain on some layers; noise and blur mottling; crumpled-paper plates; faded,
  letter-spaced serif body text used as texture.
- **Colour:**
  - Mostly desaturated neutrals and creams, plus one saturated accent per composition: the pink
    phone, red pen ellipse, red cover band, or green circle in the "too perfect" example.
  - The dark variant uses a #101010 background with light grey serif type.
- **Type:**
  - A classic serif (old-style/didone-ish) for titles: "Collage Style" and "Editorial Look" in
    italic.
  - A heavy grotesk on black strips for labels.
  - A small serif for handwritten-note captions.
- **Audio (for reference):**
  - Voice-led. No stable musical tempo was detectable: the tempogram is flat, and percussive beat
    intervals are 0.578 ± 0.043 s, i.e. inconsistent.
  - A low-frequency impact hits at about 5.0–6.5 s (the "flat and digital" beat) and again at
    about 48.8–49.3 s.
  - Percussive onsets at 0.19/0.33/0.49/0.70 s line up with the first pop-ons (0.20/0.33/0.47/
    0.70 s), which suggests click or paper SFX per pop-on. This is confounded by speech
    onsets, so treat it as likely, not proven.
  - **The stepped animation is fixed-rate (8/12 fps), not locked to a beat.**

---

## 4. Engine requirements (prioritised)

### Must

1. **Stepped time (posterize time) per layer, group or comp.**
   - Parameters: `step_fps` (8, 12, 6), `phase_offset_frames`, `enabled`.
   - Evaluate the layer's whole animation (transform, write-on, typewriter, effect params) at
     `t_q = floor((t - t0) * step_fps) / step_fps + t0`.
   - It must work across 24, 30 and 60 fps outputs. At 30 fps, 8 fps produces 4,4,4,3 holds and
     12 fps produces 3,2,3,2, which is what this video shows.
   - Held frames must be bit-identical, so the compositor can cache them (aligns with
     `SceneCompositor` layer caching).
2. **Layer stack with free z-order, opacity and Normal blend,** with masks on any layer.
   - Parameters: `opacity` (0–1, e.g. 0.82), `blend_mode` (normal first; multiply and screen are
     useful for textures).
   - Text, shape and image layers must interleave arbitrarily.
3. **Masks / cut shapes.**
   - Primitives: rectangle, rounded rectangle, ellipse, arch (rect with semicircular top),
     quarter-circle, polygon/bezier path.
   - Parameters: `path`, `feather_px` (0 for cut paper), `invert`, `expansion_px`.
   - A **fragment** helper: `source_image`, a list of `{mask_path, home_offset(dx,dy),
     rotation, opacity, z}`. All fragments share one texture.
4. **Tint / gradient map effect.**
   - Parameters: `map_black` (colour, default #000), `map_white` (colour, e.g. HSB(60°,6%,90%)),
     `amount` (0–1).
   - Accept HSB input.
5. **Keyframed transforms with easing curves.**
   - Parameters per keyframe: position, scale, rotation, anchor and opacity, each with `ease`
     (linear, ease-out quad/cubic/expo, in-out). The 8 fps fly-in starts at about 2–3x scale
     with rotation, travels hundreds of px and settles over 1.0–1.3 s.
   - Combined with (1), the result reads as stop-motion.
6. **Text layers:** serif and grotesk fonts, italic, tracking, per-word boxes ("label" style:
   black rectangle padding ≈0.15 em, white text), per-word rotation, and a **typewriter/chunk
   reveal** quantised by the step rate.

### Should

7. **Procedural grain (Noise HLS equivalent).**
   - Parameters: `mode` (grain / uniform / squares), `lightness_amount` (0–100%, e.g. 53),
     `hue_amount`, `sat_amount`, `grain_size_px` (≈1.0), `seed`, `animated: bool`,
     `phase_speed`.
   - The default should be static per layer (as shown). An optional boil re-seeds every step
     (tie it to `step_fps`).
8. **Per-layer Gaussian blur** in an ordered effect stack (noise → blur).
   - Parameters: `sigma_px` (0.6–12 at 720 w; scale with resolution), `dimensions` (h, v or
     both), `repeat_edge: bool`.
   - Must go through the downsampling `blur_plane` path.
9. **Halftone filter.**
   - Parameters: `cell_px` (2–3 at 720 w for scan realism; 6–20 for stylised), `angle_deg`
     (45), `shape` (round, ellipse, line), `mono / cmyk`, `contrast`, `ink_color`,
     `paper_color`.
10. **Texture overlay / matte.**
    - Apply an image (paper grain, crumple) to a layer with `blend` (multiply, overlay, soft
      light), `amount`, `scale` and `offset`, optionally as a luma matte.
    - Parameters: `texture_id`, `blend`, `opacity` (0.1–0.6), `tile: bool`, `random_offset_per_step`.
11. **Write-on strokes (trim paths)** for hand-drawn arrows, circles and ellipses.
    - Parameters: `path`, `stroke_width` (2–4 px at 720 w), `color` (black or red #C0302A-ish),
      `start/end` keyframes, `roughness` (slight wobble / non-closing overshoot), stepped at 12 fps.
12. **Background-paper vignette / edge darken.**
    - Parameters: `strength` (≈0.14 luminance at edges), `softness`, `shape`.

### Nice

13. **Torn-edge generator.**
    - A procedural ragged mask edge (fractal displacement of a path) plus a light fibre rim.
    - Parameters: `roughness_px` (2–8), `frequency`, `rim_width_px` (1–3), `rim_color`
      (off-white), `seed`.
14. **Hairline drop shadow / contact shadow** (absent here, common in the genre).
    - Parameters: `offset` (1–4 px), `blur` (1–6 px), `opacity` (0.15–0.35), `color`.
15. **Per-layer random placement imperfection.**
    - A one-time seeded offset and rotation (±2–6 px, ±0.5–2°) applied at layout time, which
      produces the slightly crooked "assembled by hand" placement of word labels and scraps.
    - This is separate from boil (re-seeding per step), which this video does *not* use.
16. **Typed/chunked caption track** synchronised to transcript word timings with a negative lead
    (about −0.25 s).
17. **Audio cue hooks:** a paper or click SFX on each pop-on step, and an impact on section cuts.

**What an agent would set for a "collage shot" preset (derived from this video):**

```
comp: 720x1280 (or 1080x1920), 30 fps
group "collage": step_fps=8
  background: paper_texture(grey #AAAAAA, fibre), vignette 0.1
  scraps: image + torn_edge(roughness 4px) + tint(map_white=HSB(60,6,90))
  hero cutout: image + halftone(cell 2–3px, 45°)
  build: each element pops on at successive steps (1 step = 125 ms)
  settle: transform ease-out expo, 6–10 steps, overshoot none
labels: grotesk white on black box, per-word pop, step_fps=8, rotation ±2°
annotations: write-on stroke, step_fps=12, red #C8302C, 3 px
photo layer variant: fragment(source, 6–8 geometric masks, offset ≤6% width, opacity 0.82)
texture variance: per-layer noise(lightness 0.53, size 1.0, static) -> gaussian(σ 0.6–12)
```

---

## 5. Non-engine content (assets the engine cannot synthesise well)

| Asset | Seen as | How to source or generate |
|---|---|---|
| Scanned old book / newspaper pages with real body text | Tinted torn scraps, newsprint background | Public-domain scans (Internet Archive, Library of Congress Chronicling America, Wikimedia Commons). Alternatively, typeset public-domain text in an old-style serif and run it through the halftone, ink-spread and paper-texture effects. |
| Paper textures: fibre grey card, crumpled white, cream newsprint, dark grainy stock | Backgrounds, texture plates, the diagonal strips | Scan or photograph real paper (flat lit, 600 dpi) or use CC0 texture sites (e.g. Poly Haven, ambientCG, texturelabs-style packs). Procedural fallback: fBm noise plus a fibre streak layer plus a crumple normal-map lit at a low angle. |
| Photo cut-outs (vintage phone, buildings, flowers, suited figure) | Hero objects with clean silhouettes | Public-domain or CC0 photos, background removed (rembg/SAM locally), then the halftone filter. A cut-out pipeline (image → matte → optional white cut border) is worth building in the tool even though the images are external. |
| Vintage magazine cover / poster | The source of the Hack 2 fragments | Public-domain magazine scans; or a generated "vintage editorial" photo composited with a serif masthead. |
| Hand-cutting B-roll | Thesis montage | Live footage; not engine content. |
| Hand-drawn strokes (arrows, loose ellipses) | Annotations | Can be procedural (wobbly bezier with uneven pressure and non-closed ends). A small library of real pen-stroke SVGs adds authenticity. |
| Halftone screens / print textures | Overlays | Procedural (engine). Scanned print-dot textures add misregistration and dot gain realism. |
| Fonts | Serif titles, grotesk labels | Open fonts such as old-style serifs (EB Garamond, Libre Caslon, Instrument Serif italic) and heavy grotesks (Inter Black, Archivo Black). |
| SFX (paper rustle, clicks, impact) | Pop-ons, section hits | CC0 SFX libraries (freesound CC0) or a recorded foley pack. |
