# Motion Array Brand Promo (Buff Motion): study for the Lahza motion engine

Source: `Motion Array Brand Promo _ Buff Motion.mp4`, 1280×720 (16:9), 25 fps CFR, 40.72 s video / 40.81 s AAC stereo.
Working files (frame sheets, tracking scripts, audio analysis): `study/ma/` next to this report.

**Time convention.** All frame numbers are **timeline frames T = t × 25**, which line up with the audio. The video stream starts at t = 0.04 s, so T = (ffmpeg/decoder frame index) + 1. The frame sheets in `ma/frames/` are labelled with the decoder index (sheet `f42` = T43).

**Music grid (measured).** The tempo is **90.0 BPM**. A global period/phase search over the onset envelope gave a best period of 0.6665 s, and per-8-beat windows drift less than 5 ms. One beat is **16.667 frames**, one bar (4/4) is **66.67 frames = 2.667 s**, an eighth note is 8.33 frames, a sixteenth is 4.17 frames. The first downbeat, taken from the kick pattern, is at **T27.3 (1.093 s)**. Downbeats: T27.3, 94, 160.7, 227.3, 294, 360.7, 427.3, 494, 560.7, 627.3, 694, 760.7, 827.3, 894, (960.7).

---

## 1. Overview

- **Length / format:** 40.72 s, 1018 frames, 25 fps, 16:9. The music lasts about 15 bars. The beat stops at T947 (37.9 s) with a reverb tail to about 39.7 s, and the end card holds in near-silence until 40.7 s.
- **Style:** Maximalist "toy-box" 3D brand promo. Glossy clay/inflatable 3D objects, chrome and holographic props, and cartoon gloved hands share a dark navy "infinite canvas" with a faint UI grid. Design-tool motifs keep coming back: selection bounding boxes with handles, cursor arrows, a cursor that grabs and drags. Kinetic typography, footage tiles and client-logo keycaps sit in the same space. Almost every transition is a continuous camera move (zoom-through, whip, object wipe, fly-through) rather than a plain cut. The camera never stops drifting.
- **Palette (sampled hex, medians of matching pixels):**
  - Background navy `#000018`; grid lines / glow `#000038`; vignette glow toward `#0c0c54`
  - Cream / off-white (bg and "white" type) `#fdf5d9`; CTA text `#fdfae7`
  - Acid lime (signature accent, serif type, glows) `#dbfd00`
  - Electric blue (bursts, keycaps) `#0057fe` / `#0141e0`; ribbon deep blue `#061ce0`; ribbon light blue `#98d4e1`; balloon-type blue `#2a66f3`
  - Magenta `#c00068`; hot pink (characters) `#d03194`; purple (play triangle) `#7a3cf5`; coral/orange (cross) `#e5573d`
  - By area, about 60% of all pixels are within one bin of `#000018`. The colour is spent in small, saturated hits.
- **Typography:**
  - **Rounded heavy geometric sans** (white/cream, lowercase) for the feature words: "temp-lates", "motion graphics", "foot-age", "AI tools", "all in one place", "from good", "Licensed", "Business". Long words are hyphen-split onto two lines that wrap around a central object cluster. Tracking is normal to slightly tight.
  - **High-contrast italic serif** (display, editorial style) in lime for "Fully", and in cream for "MotionArray" in the end wordmark. Serif italic paired with bold sans is the brand's type pairing.
  - **Inflated 3D balloon type** with an iridescent blue-violet material ("Best").
  - Type animates per glyph almost everywhere: wobble, scatter, converge, typewriter, morph, shatter (see §5).

---

## 2. Shot list

"Beat" columns use the grid above. Δ = event frame − nearest grid point (negative = early).

| # | T start–end (s) | Len f (beats) | On screen | Motion | In → Out | Beat alignment |
|---|---|---|---|---|---|---|
| 1 | 0–43 (0–1.72) | 43 (2.6) | Logo outline (two loops + play triangle) inside a selection box with tiny triangle cursors on navy | Box scales up slightly. Four 3D gloved hands fly in and grab the box with motion blur: lime from left T6–10, blue from top T10–14, pink from bottom T16–19 (≈4–5 f ≈ one 16th apart). Box stretches and zooms to camera T39–42 | Open on black → hard cut | Cut T43 is 1 f before beat 2 (T44) |
| 2 | 43–98 (1.72–3.92) | 55 (3.3) | Cream bg, black blob-shaped portal holding 3D props (donut, cross, capsule, gem, purple "ma" play-triangle hero) | T43–44 portal scales up. **T45 radial burst** (white/lime rays). Concentric blob rings expand in a colour sequence of navy T45 → blue `#0057fe` T47 → magenta T51 → lime T53 → navy T55, one ring per ~4 f (a 16th). Then a slow push with props bobbing | Hard cut → **fly-through the blob hole** T90–98 (rings blur past camera) | Burst T45 = +1 f after beat. Fly-through is centred on downbeat T94 (kick, 0.95 rel.), new scene readable at T98 (+4) |
| 3a | 98–134 (3.92–5.36) | 36 (2.2) | Spotlight, chrome pedestal, pink clay character with props (painting, chrome arrow, grapes, star badge) | Camera push-in, 15 f ease (§4 E1). Props pop in with ~1 f stagger T100–108 | ← fly-through. Out: **turntable spin swap** T133–137 (whip rotation with motion blur) | Spin peak T135 vs and-of-3 T135.7 (syncopated kick), Δ −0.7 |
| 3b | 134–176 (5.36–7.04) | 42 (2.5) | Same pedestal, purple sitting character, "MOTION / STORYTELLING" sticker type, sunglasses emoji, UFO | Sticker type pops/unrolls T136–142, then slow drift | Spin swap T173–178 | Spin peak T177 vs beat 2 T177.3, Δ 0 |
| 3c | 176–219 (7.04–8.76) | 42 (2.5) | Same pedestal, yellow character, bow, chrome asterisk, photo cards | Settles about T181. **Push into the character** T209–218, accelerating, heavy zoom blur | → **zoom-through** (camera enters the yellow body) | Peak motion T218 vs and-of-4 T219, Δ −1. Hi-band hit (whoosh/clap) at T221. Bass is ducked over T216–226 into the downbeat kick at T227 |
| 4 | 219–243 (8.76–9.72) | 24 (1.4) | "temp- / lates" around a fanned stack of template cards | Scene grows from ~14% scale to ~80% in 4 f (log-space ease-out, §4 E3), then keeps pushing about 2%/frame. Cards rotate like a carousel | ← zoom-through; out: hard cut | Cut T243 vs beat T244, Δ −1 |
| 5 | 243–271 (9.72–10.84) | 28 (1.7) | "motion / graphics" wrapped around a cluster (pink donut, glass "6", football, lime cross, chrome spring, golden bell) | Per-letter wobble (rotation ±5–10°, offset per glyph), objects pop T243–247, slow push | Hard cut | Cut T271 vs and-of-3 T269.3 (kick), Δ +2 |
| 6 | 271–294 (10.84–11.76) | 23 (1.4) | "foot- / age" over a horizontal strip of rounded footage tiles | Tiles pan slowly. **Letter-scatter exit** T286–293: glyphs rotate, drift and fall independently, ~1 f stagger | Hard cut | Cut T294 = downbeat, Δ 0 |
| 7 | 294–362 (11.76–14.48) | 68 (4.1) | "AI / tools" (lime "AI", cream "tools") threaded by a glossy 3D blue tube shaped as a sine wave | Wave amplitude grows T296–300, tube passes in front of and behind glyphs, amplitude collapses T315–322. Then a striped ribbon (blue/white/lime) swoops across with small stickers | Continuous → object wipe | — |
| 8 | 362–384 (14.48–15.36) | 22 (1.3) | Lime 3D blob flies at camera and fills frame (T366–369), revealing a footage clip (boxer) in a rounded-square mask | Mask/tile scales down from ~70% of frame height to a ~20% tile in ~8 f. Other footage tiles pop in with 1 f stagger T377–384 | **Object wipe** | Wipe peak T369 = and-of-1 (T369.0), Δ 0. Downbeat kick T361 launches the blob |
| 9 | 384–437 (15.36–17.48) | 53 (3.2) | "all in / one place" at centre of a wall of footage tiles and floating 3D balloon blobs (purple, lime, pink) | Letters converge from scattered positions into the phrase (T384–394). Blobs bob and parallax. Cursor grabs the text's selection box T413, tilts it and drags it down. Everything slides off-frame T426–437 | Continuous / slide-off | Assembly motion peaks T393 vs beat 3 T394, Δ −1 |
| 10 | 437–502 (17.48–20.08) | 64 (3.8) | Empty grid. 3D primitives (blue squiggle, lime cube, cylinder-with-feet, donut, pink cross, pearl) drop in and stack inside a selection bounding box | Gravity drops: cube falls at ~180 px/frame and stops dead with a ~1% rebound (§4 E6). Objects arrive one by one T441–465. Hold with micro-bob. **Reverse exit**: objects fly up out of frame one by one T487–500 | Continuous | First drop on beat 2 T444; cube impact T447 (+3 ≈ one 16th). Exit centred on downbeat T494 |
| 11 | 502–573 (20.08–22.92) | 71 (4.3) | Ribbons (deep blue, light blue, white, lime) grow upward and twist into a soft-serve swirl, with stickers riding them | Ribbon growth (sweep along spline). Camera tilts up. From the downbeat T561, a push into the swirl. A cursor draws a squiggle. The frame fills with white T569–571 | **Push-in → whip** T572–574 | Push starts at downbeat T561 (bass ducked T551–560, big kick T561). Whip T573 = 16th after "and" (T573.2), Δ 0 |
| 12 | 573–655 (22.92–26.2) | 82 (4.9) | Curved wall of 3D keycaps carrying client logos (Google, Shopify, Microsoft, NBC, Sephora, Accenture, Canal+, Vodafone, BVB…) | Whip-in with directional blur, settles about T584, slow orbit. From T609 a lime light/colour wave spreads across the keys, led by a tiny lime hand. Keys flip to lime in **stepped** increments every ~3.6 f (T609, 612, 616, 619, 623…) | ← whip; out: hard cut to black | Cut T655 is 2 f after an onset at T652.8 (and-of-2 T652.3). This is the loosest sync point |
| 13 | 655–709 (26.2–28.36) | 54 (3.2) | "Fully" (lime italic serif) over "Licensed" (cream bold sans, where the "c" is a 3D lime character) | **Typewriter, 2 f per glyph** (T655–663) while the word **re-centres** with ease-in-out (§4 E5). "Licensed" rises from a line mask, easeOutQuart over 12 f (§4 E2). Hold T679–682. Pull-back reveals the text as a screen on a 3D billboard plus a phone mock-up (T683–694) | Hard cut in; whip out T709–715 | Strongest onset of the section at T663.5 coincides with "Fully" completing (T662–663). The billboard reveal settles into downbeat T694 |
| 14 | 709–770 (28.36–30.8) | 61 (3.7) | Deep space with sparse stars: a cartoon blimp/rocket towing a "Fully Licensed" banner | Flies in from depth T715–728, drifts, then **exits fast** to the right with motion blur T754–766. The banner swings | Whip in / object exit | Whip peak T714 vs beat T710.7 (+3). Exit centred on downbeat T760.7 |
| 15 | 770–827 (30.8–33.08) | 57 (3.4) | Twisted ribbon (blue/light-blue/white/lime) grows left to right carrying stickers, footage cards, cursors and stars | Ribbon write-on (≈8 f to cross half the frame). Camera push and pan along it T804–826. Ribbon tail exits right | Continuous | Growth starts T770 vs and-of-1 T769 (+1). Exit T826 vs downbeat T827.3 (−1) |
| 16 | 827–868 (33.08–34.72) | 41 (2.5) | Black. A single confetti chip tumbles and becomes the "f" of "from good" | Glyphs appear about 1 per frame (from T841–844, good T845–849). Hold. "good" turns blue and squiggles (T858) | Continuous | — |
| 17 | 868–903 (34.72–36.12) | 35 (2.1) | "to Best": "good" morphs into inflated iridescent balloon type | Log-space scale ease-in-out over 10 f (§4 E4), then about 1.4%/frame push. The balloon letters subtly inflate and wobble | **Shatter** T903–906 | Morph begins T868 vs and-of-3 T869 kick (Δ −1). Shatter T903 vs onset T902.7 (+0.3) |
| 18 | 903–923 (36.12–36.92) | 20 (1.2) | Liquid/glass shards of the type fly past camera, blue light leaks, then a few particles | Fly-through with heavy blur, then near-empty frame | Continuous | Bass hits T907/T911 (low-band onsets 0.83/1.0) |
| 19 | 923–1018 (36.92–40.72) | 95 (5.7) | "ma" logo in a lime selection box → wordmark "MotionArray Business" → cursor clicks → "Start now" pill button | Logo flies in from lower right with scale overshoot (spring, §4 E7) T927–933. Logo shrinks T936–940 and the wordmark builds glyph by glyph in scrambled order (T940–950). Cursor click T962, button pops T963–966. Idle cursor/confetti until the end | End card | Logo pop T927 = beat T927.3. **Shrink/transform T936 = largest onset in the track** (T936.1, and-of-3), Δ 0. Music stops T947. Click T962 ≈ the next (silent) downbeat T960.7 |

Hard cuts with scene score ≥ 0.3: T43, T45, T219, T243, T271, T294, T369 (wipe), T655. Every other shot change is a continuous camera or object transition.

---

## 3. Timing analysis

**Cut rhythm.** 21 shots/sub-shots over 1018 frames. Mean 48 f (1.94 s, 2.9 beats), median 43 f (2.6 beats), min 20 f, max 95 f (end card), σ 20 f.
- Phrase structure: the opening (shots 1–3) runs 2–3.3 beats per shot. A **rapid list section** (shots 4–6: temp-lates / motion graphics / foot-age) cuts every **1.5 beats** (24, 28, 23 f). Those are dotted-quarter cuts: −1, +2, +5, +8 eighths around downbeat T227.3, landing on downbeat T294. The middle section (7–15) relaxes to 3.2–4.9 beats per shot, carried by long continuous camera moves. The outro (16–18) tightens again to 1.2–2.5 beats before a 5.7-beat end card.
- Section changes sit on **bar downbeats that follow a bass duck**. The low band (<150 Hz) falls to about 25–35% of median for ~1 beat before T227, T361 and T561, then a big kick (≈2× median) hits. The visuals put their biggest transitions there: zoom-through into "templates" (T209–219), the lime blob wipe (T361–369), and the push into the swirl (T561–573).

**Sync accuracy.** Across 26 measured visual events (cuts, transition motion peaks, pops), the error against the 90-BPM grid is **median |Δ| ≈ 0.7 frame, max 3 frames (120 ms)**. 21 of 26 land within ±1 frame (11 exactly on a grid point). The outliers are the T447 impact (+3, which is −1 against the following 16th), the T714 whip (+3), the T655 cut (+2.7) and the T271 cut (+1.7).
- **Cuts land on the beat or one frame early** (T43 → beat T44, T218 → T219, T243 → T244). This looks deliberate: the new image is already on screen when the transient hits.
- Events land on **downbeats** (T94, T294, T494, T561, T694, T760, T927), on the **"and" of beat 3, the syncopated kick** (T135, T269, T869, T936), and on **sixteenths** (T573 whip, the ring colour steps every ~4 f in shot 2, the hand entries in shot 1).
- The largest audio accent (T936.1) is the logo-to-wordmark transform. The music cutoff (T947) is the wordmark finishing (≈T950).
- Sound design: high-band transients separate from the drums at T221 (5.3× median, right after the zoom-through: a whoosh/clap hit) and at T206–214 (rising highs during the push-in, a riser). Highs drop out over T564–578 (filter sweep under the whip). There is no audible separate pop for each object pop; the music carries everything.

**Typical durations (frames at 25 fps).**
| Action | Duration | Notes |
|---|---|---|
| Hero entrance (move/scale) | 6–12 f (0.24–0.48 s); 90% done in 4–6 f | E2, E3, E7 |
| Camera settle after transition | 10–15 f | E1 |
| Small prop pop-in | 4–6 f each, **1 f stagger** | pedestal props, footage tiles |
| Per-glyph type reveal | 1–2 f per glyph | "Fully" 2 f, "from good" ~1 f, wordmark ~0.6 f |
| Per-glyph scatter exit | ~8 f total, ~1 f stagger | "foot-age" |
| Zoom-through / fly-through | 8–10 f of acceleration + 1–3 f peak blur | T90–98, T209–219 |
| Whip pan | 2–6 f of blur, settle by +10 f | T572–584, T709–715 |
| Object wipe | 6 f cover, reveal on the beat | T363–369 |
| Holds | Rarely > 1 beat truly still: after an entrance there is a 0.5–2%/frame scale drift (slow push) or orbit | "never static" |
| Stepped colour/state changes | every ~4 f (16th) or ~3.6 f | rings in shot 2, keycap wave |

**Phrasing.** A typical shot runs: transition-in centred on the beat → hero settles within half a beat (≤ 8 f) → slow drift while secondary elements (props, tiles, particles) keep arriving on 1-frame staggers → the next transition starts accelerating 4–10 f *before* the next accent so its peak lands on the accent. Movement is front-loaded (fast-out) on entrances and back-loaded (accelerating) into exits and transitions.

---

## 4. Easing catalogue (measured)

Measured with colour-blob tracking on half-resolution frames (`ma/scripts/track.py`), normalised to 0→1 and fitted with a cubic-bezier grid search (`ma/scripts/fit.py`). Frames are T.

| ID | Move | Frames | Progress per frame | Peak speed | Overshoot | Model |
|---|---|---|---|---|---|---|
| E1 | Camera push into pedestal after fly-through (bbox height 84→270 px) T97–112 | 15 (90% at 10) | 0, .13, .28, .44, .57, .66, .73, .78, .84, .88, .91, .95, .96, .98, .99, 1 | step 3 (0.16/f), ≈20% into the move | none | **cubic-bezier(0.20, 0.30, 0.20, 0.90)**, RMSE 0.005: soft start, long exponential tail |
| E2 | "Licensed" rising from mask (y) T667–679 | 12 (90% at 6) | 0, .26, .52, .75, .85, .89, .92, .95, .96, .98, .99, 1 | first 2 frames | none (<0.5 px) | **easeOutQuart** (1−(1−t)⁴ matches within 0.03); bezier(0.30, 0.90, 0.15, 0.90) |
| E3 | "temp-lates" scale-in from tiny (log of height, 34→206 px) T219–224 | 4–5 (94% at 3) | 0, .40, .73, .94, .97, 1 (×2.06, ×1.80, ×1.46, ×1.07 per frame) | first frame | none | **ease-out (quad) in log-scale space**, ~4 f, then hand-off to a linear drift of ≈ +2%/frame. Scale must be interpolated logarithmically |
| E4 | "good"→"Best" balloon grow (log of width, 46→358 px) T866–876 | 10 (90% at 8) | 0, .02, .11, .26, .38, .54, .74, .88, .95, .98, 1 | step 6 (60%) | none in bbox | **log-space ease-in-out, bezier(0.45, 0.10, 0.50, 1.00)**, then linear push ≈ 1.4%/frame |
| E5 | Typewriter re-centring: "F" x-position while "ully" are added T655–661 | 6 | 0, .19, .44, .72, .89, .99, 1 | step 3 (mid) | none | **ease-in-out, bezier(0.50, 0.50, 0.45, 1.00)** (≈ easeInOutSine). The word anchor eases to the new centre each time a glyph is added |
| E6 | Lime cube gravity drop T443–450 (cy 17→296 half-res px) | 3 f of fall + 1 f impact | velocity 85, 97, 82, then 13, 3, 4, −5 px/f | constant ~90 px/f half-res (≈180 px/f full-res ≈ 25% of frame height per frame) | rebound ≈ 4 px half-res (~1.5% of fall) | **near-linear fall (no visible acceleration) + hard stop**, 1-frame micro-rebound. Model: fall at constant v, then a critically damped (ζ≈1) spring at impact with a squash pulse |
| E7 | "ma" end-logo fly-in: position (x: 223→122) and box width (22→~99) T927–934 | position 7 (90% at 5); scale peaks at 2 f | x: 0, .25, .47, .72, .86, .95, .99, 1. Width: 22, 82, **108**, 104, 100, 98, 98 | position: first frames | **scale +9% overshoot**, settled in 3 f | Position: bezier(0.35, 0.60, 0.50, 1.00) ≈ easeOutCubic. Scale: **underdamped spring**, ζ ≈ 0.61, t_peak ≈ 2.5 f → ωn ≈ 40 rad/s (mass 1: stiffness ≈ 1600, damping ≈ 49), settle ≈ 6 f |
| E8 | Templates/Best/MOTION holds: continuous push | whole hold | +0.9–2%/frame scale | — | — | **linear (constant-rate) dolly**; it starts exactly where the entrance ease ends, with no velocity discontinuity visible because the ease tails into it |
| E9 | Zoom-through into yellow character T209–218 (motion energy 14→37→120) | 9–10 | motion energy grows ~×1.2 per frame, peaks on the cut frame | last frame | — | **ease-in (expo/quart) to the cut**, mirrored by an ease-out (E3) on the other side: an expo-in/expo-out pair split across two scenes |

Takeaways for the engine:
- Entrances are **ease-out quart/expo** taking 6–12 f; 90% of travel happens in the first half.
- **Springs appear only on scale** (logo, pops) and are stiff: about 9% overshoot, ≤ 6 f settle. They are not floaty.
- **Scale is interpolated in log space** (zoom-ins of 6–10× read as constant-rate).
- Transitions use **in-to-cut / out-from-cut expo pairs**.
- Everything ends in a **linear drift** rather than a dead stop.

---

## 5. Techniques catalogue

| Technique | Where (T) | How it's made in AE/C4D | Engine primitive needed |
|---|---|---|---|
| Continuous "infinite canvas" camera (push, orbit, tilt) | all shots | C4D/AE 3D camera with eased keyframes and slow drift | 3D (or 2.5D) camera with keyframed transform, log-scale zoom, constant drift |
| **Zoom-through / fly-through** transitions | 90–98, 209–219, 561–573, 903–911 | Camera flies into an object or hole; next comp nested inside at tiny scale; motion blur | Nested compositions placed in parent space ("comp as layer" at arbitrary scale/depth), expo-in/out camera, per-pixel motion blur |
| **Whip pan** with directional blur | 572–584, 709–715 | Fast camera rotation plus Directional Blur / RSMB | Camera-velocity-driven directional/motion blur, sub-frame sampling |
| **Object wipe** (foreground object fills frame) | 363–369 (lime blob), 214–218 | 3D object animated at camera; next shot revealed behind it | Layer ordering with a full-frame occluder; transition authored as an object, not as a wipe preset |
| **Turntable spin swap** | 133–137, 173–178 | C4D object rotation with motion blur; model swapped at peak blur | Model/asset swap at a blur peak; rotational motion blur |
| Shape-mask portal (blob hole) | 43–98 | Blob-shaped track matte, scaled; content inside | Arbitrary vector masks (bezier/blob) with feather, animated scale |
| Radial burst rays + expanding concentric rings with stepped colours | 45–55 | Shape layers (repeater/ripple), colours keyed on 16ths | Procedural radial rays, offset-path/ripple rings, stepped (hold) keyframes on colour |
| **Per-glyph kinetic type**: wobble, scatter exit, converge, typewriter with re-centring, scrambled build | 243–271, 286–293, 384–394, 655–663, 841–849, 940–950 | AE Text Animators (range selectors with offset, wiggly selector), stagger by index/random | Glyph-level text layout with per-glyph transforms, range/stagger selectors (index, random order), wiggle, auto re-centre animation |
| Line reveal from mask | 667–679 | Text rising behind a track matte | Clip/mask layers, line-level animation |
| Mixed-media type (3D object replacing a glyph: the "c" in Licensed; "AI tools" tube threading through letters) | 296–322, 667+ | 3D render with depth-sorted text, or text extruded in C4D | Depth interleaving between text and 3D (z-buffer or split layers), glyph replacement by an image/model |
| **Word morph** (good → Best) | 858–872 | Shape morph or 3D blend shapes, cross-dissolve hidden by scale | Path/shape morphing between glyph outlines (point matching), or a fast dissolve covered by scale and blur |
| **Inflated balloon 3D type** with iridescent material | 868–903 | C4D Cloth inflation / Volume Builder plus thin-film shader | 3D extruded/inflated text mesh with PBR + thin-film (or pre-rendered asset) |
| **Shatter / liquid break-up** of type | 903–911 | C4D Voronoi Fracture plus fly-through, or AE Shatter / Displacement | Mesh fracture, or displacement + particle shards (possibly a pre-rendered element) |
| Ribbon / twisted tape growth (sweep along spline) | 325–360, 503–573, 770–827 | C4D Sweep with growth parameter and twist; stickers on splines | Spline sweep (tube/ribbon) with animatable **trim (start/end)**, twist and gradient stripes; attach-to-path for riders |
| 3D sine-wave tube with animated amplitude | 294–322 | Sweep on a Formula spline, amplitude keyed | Parametric curve with animatable params → tube mesh |
| Gravity drop and stack, reverse fly-out | 441–500 | C4D dynamics, or keyframes with squash/stretch | Simple rigid-body/drop preset (fall + impact spring + squash), or keyframe presets |
| Floating bob / parallax of 3D blobs | 98–211, 384–437 | Wiggle / sine expressions on position and rotation, depth layering | Procedural noise/sine drivers on transforms; depth-based parallax |
| Staggered pop-in (scale spring) | 100–108, 377–384, 441–465 | Scale 0→1 with overshoot, offset by index | Stagger by index (1 f), spring scale preset |
| Footage in rounded masks, tile grids | 271–294, 369–437 | Precomps with rounded-rect masks, grid layout | Video layers with rounded-rect clip, grid/strip layout, per-tile pop |
| Device/billboard mock-up pull-back | 683–709 | Comp projected onto a 3D plane (billboard, phone) in a 3D camera | Comp-as-texture on 3D planes with perspective; camera pull-back |
| Keycap wall with logos + light-sweep colour wave | 573–655 | C4D cloner (grid on curved surface), MoGraph effector sweeping colour, texture per clone | Instancer/cloner (grid on curve), per-instance texture, effector-style field (sweep) driving colour/height, optionally stepped |
| Design-tool UI motifs (selection box with handles, cursor grab and drag, click, button pop) | 0–43, 384–437, 441–500, 923–1000 | Shape layers plus cursor sprite, parented | Built-in "UI overlay" primitives: bbox with handles bound to a layer, cursor sprite with path and click pulse |
| Cartoon gloved hands grabbing | 6–42, 609–655 | Pre-rendered 3D hand assets | Asset layers (image-sequence/alpha video or glTF) |
| Character/prop 3D renders (clay, chrome, glass, holographic) | throughout | C4D/Redshift/Octane renders | Either a real PBR renderer or alpha-video/image-sequence playback (see §7) |
| Motion blur | every fast move | Render MB / RSMB | Must-have: per-layer transform motion blur with shutter angle, sub-frame sampling |
| Depth of field / defocused foreground | 96–98, 368–372 | Camera DOF / Camera Lens Blur | Depth-aware blur, or per-layer blur keyed to depth |
| Glow / bloom, vignette glow | ribbons, spotlight, blue backlight | Glow, radial gradients | Bloom pass, radial gradient layers, additive blending |
| Spotlight cone | 98–211 | Volumetric light or gradient shape | Gradient shapes with additive blend |
| Particles / confetti / stars | 826–860, 903–960, scattered | Particular / MoGraph | Lightweight 2D/3D particle emitter (sprites, tumble rotation, gravity) |
| Faint UI grid background | 219–940 | Grid effect at low opacity, in camera space | Procedural grid layer that moves with the camera |
| Grain / texture | none visible | — | not needed |

---

## 6. Engine requirements (prioritised)

**MUST**
1. **Beat-grid timeline.** Import BPM, offset and bars; snap keyframes and cuts to beat, half-beat and sixteenth; a "cut lead" setting (default −1 frame); phrase-level syncopation patterns (dotted-quarter cuts). All shots. Plus **audio analysis** (onsets, kick/hi bands, bass-duck detection) so an agent can find the accents (shots 3c, 8, 11, 19).
2. **Easing library with numeric control:** cubic-bezier, power (quad…quint, expo), in/out pairs split across a cut, spring (stiffness/damping/mass) with ≤ 6 f settle presets, and **log-space interpolation for scale/zoom**. Entrances need easeOutQuart 6–12 f; springs on scale (≈9% overshoot). All shots.
3. **Hierarchical comps in a camera space:** nested comps as layers at any scale/depth, parenting, and a 2.5D/3D camera with keyframes and constant drift. Shots 2→3, 3c→4, 11, 13, 17→18.
4. **Motion blur** (shutter-angle, sub-frame) and **directional/whip blur**. Shots 1, 3, 11–14, 17–18.
5. **Per-glyph text engine:** layout and two-font pairing, per-glyph transforms, stagger selectors (index/random/centre-out), typewriter with auto re-centring, wiggle, scatter/converge, line masks. Shots 4–9, 13, 16, 19.
6. **Vector shapes and masks:** bezier/blob masks with feather, rounded-rect clips for video, radial rays, ripple rings, stepped (hold) keyframes. Shots 2, 6, 8–9, 19.
7. **Media layers:** video in masks, image sequences or video with alpha for pre-rendered 3D assets. Most shots.
8. **Stagger/instancing:** pop-in N items with 1-frame offsets and a spring scale. Shots 3, 8, 9, 10, 12.

**SHOULD**
9. **Spline sweep (ribbon/tube) with trim start/end, twist and stripe gradients**, plus attach-to-path for riders. Shots 7, 11, 15.
10. **Shape/path morph** between glyph outlines (good → Best, logo → wordmark). Shots 17, 19.
11. **Cloner/instancer + field effector** (sweeping colour/scale across a grid on a curved surface). Shot 12.
12. **Bloom/glow, radial gradients, additive blending, depth-of-field blur.** Shots 3, 7, 11–12, 15, 17.
13. **UI-motif kit:** selection bbox bound to a layer, cursor with path/click/drag, button pop. Shots 1, 9, 10, 19.
14. **Particles** (confetti/stars/shards with gravity and tumble). Shots 16, 18, 19.
15. **Comp-on-3D-plane** (billboard/phone mock-ups) with perspective. Shot 13.
16. **Simple physics presets:** drop + impact + squash, reverse fly-out. Shot 10.

**NICE**
17. Real-time PBR mesh rendering (clay, chrome, thin-film iridescence, inflated text) so 3D props need not be pre-rendered. Shots 2–3, 5, 10, 17.
18. Mesh fracture/shatter. Shot 17→18.
19. Parametric-curve tube (sine wave with animated amplitude) with depth interleaving against text. Shot 7.
20. Volumetric spotlight. Shot 3.

---

## 7. Non-engine content

These come from a 3D/illustration pipeline or a stock library, not from the engine:
- **3D character renders:** three clay pedestal characters (pink, purple, yellow), the "c" character in "Licensed", cartoon gloved hands (lime, blue, pink), blimp/rocket with banner.
- **3D props:** donut, crosses, capsule, gem, chrome arrow, chrome asterisk, grapes, bow, golden bell, football, glass "6", emoji (sunglasses), UFO, balloon blobs, primitives (cube, cylinder with feet, pearl), chrome pedestal. These come pre-rendered with matched lighting and materials (Redshift/Octane look).
- **Stickers/illustration:** "MOTION / STORYTELLING" sticker type, small icon stickers riding the ribbons, starbursts.
- **Stock footage clips:** boxer, portraits, food, eye close-up, etc. for the tiles, plus template preview thumbnails (shot 4).
- **Brand assets:** Motion Array "ma" logo, wordmark, and third-party client logos on the keycaps.
- **Fonts:** a rounded geometric heavy sans and a high-contrast italic display serif (licensed typefaces).
- **Music track** with mixed stingers/risers (sound design is mostly baked into the track).
- Optional, if the engine lacks PBR: inflated "Best" type and its shatter simulation as pre-rendered alpha sequences.
