# Study: "BeyondTheBlack Event Opener #shorts" (Buffmotion)

Source: `/home/farhan/Downloads/BeyondTheBlack Event Opener #shorts.mp4`
Working files: `/tmp/claude-1000/-home-farhan-code-screendrop-rs/b6053da9-9a03-4b7b-96d5-1df3ace8dae5/scratchpad/study/btb/` (contact sheets in `frames/`, full-res frames in `raw/`, measurement scripts in `scripts/`).

Conventions: `n` = 0-based video frame index; the stream starts at pts 0.0333, so frame `n` is displayed at **t = (n+1)/30 s**. Audio starts at 0.000. Beat grid fitted to the librosa beats: **t_beat(k) = 0.2546 + 0.45006·k s** (133.3 BPM, 13.50 frames per beat, residual ≤ ±12 ms).

---

## 1. Overview

- **Length / format:** 22.33 s, 670 frames, 720×900 (4:5 vertical), 30 fps H.264, with AAC stereo 44.1 kHz. Real content runs 0–17.0 s. From 17.03 s to the end (5.3 s) a static end card ("Watch the full project at / buffmotion.com") holds over silence.
- **What it is:** a social cut of an event-opener project. It mixes **finished renders** (2D line/type animation and C4D-style glossy 3D) with **behind-the-scenes Cinema 4D viewport captures** (grey viewport, gizmos, a split-screen of render over viewport, and a before/after wipe from viewport to render). The style is dark, minimal, premium "tech keynote": black void, a single phrase per beat, and blue/amber glossy geometry. Accents come from glow, a halftone dot field and emissive tubes.
- **Copy (one fragment per shot):** "it's a decision" / "[Beyon]d The" / "is" / "where" / "BlackLine unites" / "break free" / "now".
- **Palette (sampled):**

| Role | Hex |
|---|---|
| Base black / near-black | `#000000`, `#06030A`, `#03010B` |
| Deep navy (line-drawing bg) | `#0A0F47`, `#07092F` |
| Ultramarine blue (3D, gradient) | `#0602B8`, `#0839C4`, `#1819BE`, highlight `#5A68DA` |
| Violet / purple (gradient, sphere) | `#8A00AA`, `#9825DA`, sphere `#1A0550` |
| Lavender stroke | `#8F77F5` (glow core), `#261E8C` (dim) |
| Amber / gold | `#F5B61D`, `#F3B912`, `#ECAF16`, render gold `#C87F0A` |
| Orange / burnt | `#F07F2B`, `#CE6B14`, `#AB3804` |
| Cream highlight | `#FCEEA4`, `#F8E19B` |
| Salmon/pink petals | `#E29B74` |
| Maroon tubes | `#7A2239` |
| Type / glow white | `#FDFEFC` |
| C4D viewport grey (BTS) | `#393939` |

- **Typography:** a geometric sans with single-storey `a` (Outfit, Urbanist or Gilroy class), Regular weight, all lowercase, normal tracking, set small (cap height about 3–4 % of frame height). It is always centred, one short phrase at a time. "BlackLine" uses a rim-lit or bevelled glyph look with a gradient fill. The end card is a small sans line above an italic URL.

---

## 2. Shot list

Beat column: the position on the fitted grid (beat index; beat 0 = first downbeat; a bar is 4 beats, about 54 f) and the offset in frames to the nearest beat.

| # | Frames (n) | Seconds | Len (f) | On screen | Motion | In → Out | Beat alignment |
|---|---|---|---|---|---|---|---|
| 0 | 0 | 0.03 | 1 | black | none | — | music is silent until 0.220 s |
| 1 | 1–40 | 0.07–1.37 | 40 | "it's" / "a" / "decision" fly in from three directions and lock into one line. At n6 a black disc mask reveals an animated blurred gradient (blue/violet/amber blobs) around the text | Words: ease-in (accelerating) to a hard stop at n6, with fade-up. Disc: snap shrink from r≈412 to ≈250 px (ease-out, τ≈3.5 f), then slow shrink drift. Blobs drift continuously | in from black; out by hard cut | **Lock + reveal at n6 = 0.233 s; music starts at 0.220 s (first onset 0.232). Within 1 frame.** |
| 2 | 41–72 | 1.40–2.43 | 32 | Navy blueprint: dashed guide lines, lavender stroke drawn with dot "handles" at the ends (letter construction) | Stroke draw-on in segments: draw 8 f, hold 6 f, draw 5 f, hold 3 f, draw 7 f | hard cut in and out | beat 2.54 (half-beat, −6 f from beat 3), not beat-locked |
| 3 | 73–110 | 2.47–3.70 | 38 | Closer framing: "V" and "C/O" strokes drawing over a diagonal dashed guide. A soft light glint sweeps across the V (n104–110) | Strokes draw on, then a glint (radial light) moves L→R. Stroke count peaks at n98, then fades | hard cut / hard cut | beat 4.92 (−1 f, beat 2 of bar 2) |
| 4 | 111–146 | 3.73–4.90 | 36 | One frame of tiny "dThe" collapsing into a point. A glowing white ring with "is" inside. A halftone dot field ripples outward from centre. At n130 "is" rolls up and out, "where" rolls up in (masked by the ring) and the ring thins to a plain stroke | Ring: hard-in/soft-out scale (56→285 px Ø; 50 % at 2 f, 90 % at 9 f). Dot field: radial wave of dot scale, n112–119. Roll: about 7 f. Exit: ring scales up and accelerates (Ø 283→289→297→313→359, n142–146) | match cut in (text→point→ring); **match cut out (ring→circular mask of shot 5)** | cut −3.7 f before downbeat 8. **Ring brightness peak (luma max) at n115 = 3.867 s, +0.3 f from downbeat 8.** Roll at +1.8 f from beat 9 |
| 5 | 147–184 | 4.93–6.17 | 38 | "BlackLine / unites" in black. An amber/blue 3D ring arc fills the top of the frame | Black disc expands to fill (ease-out cubic, 5 f). Arc then drifts back down about 80 px over 30 f. Text fill sweeps centre-out (letters "ckL" first, edges last) over about 25 f on rim-lit glyph outlines | match cut in / hard cut out | +5.3 f after beat 10 (on the triplet ⅓). Source is 24 fps (duplicate frames at 150, 155, 159, 164…) |
| 6 | 185–204 | 6.20–6.83 | 20 | Huge dark violet sphere edge. An orange polyline with dot end draws down, then right | Trim-path draw in two segments (vertical, then a tick) | hard / hard | +2.8 f from beat 13 |
| 7 | 205–218 | 6.87–7.30 | 14 | Orange arc with dot end on a dark amber vignette | Arc draws on with ease-out over about 12 f | hard / hard | −4.2 f before beat 15 |
| 8a | 219–256 | 7.33–8.57 | 38 | **BTS:** C4D perspective viewport (grey, axis gizmos, spline cages). Gold petals fly along paths | Viewport playback | hard / hard | cut −3.7 f before downbeat 16 |
| 8b | 257–294 | 8.60–9.83 | 38 | **BTS split screen:** render on top (blue/gold petals orbiting), viewport below | Viewport playback | hard / hard | −6 f before beat 19 (n257 ≈ onset at 8.649, −1.5 f) |
| 9 | 295–327 | 9.87–10.93 | 33 | Render: a blue petal flower bursts radially and leaves frame. A radial ring of gold bars and blue shards expands. "break free" is revealed centre-out through the expanding ring of petals. Glossy pastel petals (gold, salmon, cream) | Burst: radius 44→450 px in 5 f (near-linear, peak 0.26 of travel per frame). Second shard ring about 35 px/f. Text reveal: radial soft mask, about 8 f (n304–311) | hard / hard | +4.8 f after beat 21 |
| 10 | 328–363 | 10.97–12.13 | 36 | Macro close-up of glossy blue and gold extruded semicircle shapes with shallow DOF | Slow tumbling and drift, rack-focus feel | hard / hard | cut −2.7 f before downbeat 24 |
| 11a | 364–396 | 12.17–13.23 | 33 | **BTS:** viewport of concentric segmented rings (blue/gold) with spline cages | Rings rotate | hard / wipe | +6.3 f (off-beat) |
| 11b | 397–404 | 13.27–13.50 | 8 | **Before/after wipe:** viewport slides off to the left, revealing the final render | Wipe edge x: 699→677→619→516→301→159→81→28→0 (ease-in-out, peak at 44 % of duration) | — | wipe midpoint +1.8 f from beat 29 |
| 11c | 404–430 | 13.50–14.37 | 27 | Render: 4 concentric 3D rings with alternating blue/gold segments counter-rotating. Thin 2D orbit arcs with dot ends circle around | Continuous rotation | wipe in / hard out | — |
| 12 | 431–466 | 14.40–15.57 | 36 | Glowing extruded amber tubes in letter-like "r"/"h" shapes. A white sphere head leads the growing tube. Maroon tubes out of focus behind. Bottom fades to black | Ball/tube head rises 872→49 px (ease-out, τ≈6 f; 50 % at about 3.5 f, 90 % at 18 f, 99 % at 29 f). Other tubes slide in from the left | hard / transform | +5.8 f after beat 31 (−7.7 f before downbeat 32) |
| 13 | 467–472 | 15.60–15.77 | 6 | Tubes collapse into a small glowing amber cross, which shrinks | Fast scale-down | transform / hard | +1.3 f from beat 34 |
| 14 | 473–509 | 15.80–17.00 | 37 | "now" in white. A dark gradient sweeps across it L→R and it dims to nearly black | Bright on the first frame, then a luminance sweep/fade over about 35 f (mean frame luma 29→3) | hard in / fade out | "now" −1.4 f before the strongest late onset (15.848 s, strength 0.47) |
| 15 | 510–669 | 17.03–22.33 | 160 | End card: "Watch the full project at" + "buffmotion.com" | L→R blur/opacity sweep reveal, about 13 f (n510–523), then a static hold | — | **Music hard-stops at 17.02 s (−8 dB in 10 ms, then a 1.5 s reverb tail to 18.5 s). The card appears 0–1 f later.** |

Scene-detection cuts (`gt(scene,0.2)`): 1.400, 7.333, 9.867, 10.967, 12.167, 14.400. Visual inspection of every-frame sheets added the dark-to-dark cuts the detector misses (2.467, 3.733, 4.933, 6.200, 6.867, 15.800, 17.033).

---

## 3. Timing analysis

### Cut rhythm
- Content shot lengths (frames, excluding the 6 f transition shot 13 and the end card): 40, 32, 38, 36, 38, 20, 14, 38, 38, 33, 36, 33, 27, 36, 37. **Median 36 f (1.20 s), mean 33.7 f, range 14–40.**
- That is about 2.7 beats per shot. Shot lengths are not multiples of the beat (13.5 f) or the half-beat. The editor works on a roughly constant ~1.2 s visual pulse, not on the beat count.
- Sections: 2D type/line act (0.23–7.33 s, 16 beats = 4 bars exactly from music start), BTS + 3D act (7.33–14.4 s), light-tube finale + "now" (14.4–17.0 s), then the end card.

### Audio sync (measured)
- **Music:** 133.3 BPM, constant level (RMS ≈ −12 dB throughout, heavily compressed). The feel is triplet: mean onset strength at beat fractions 0 / ⅓ / ⅔ = 1.14 / 1.00 / 1.01, against only 0.15 at ½. The kick lands every 4 beats (bar = 1.80 s, downbeats at 0.255, 2.055, 3.855, 5.655, 7.456 …). There are no separate SFX: no whooshes, impacts or risers above the music bed, and the spectrum is flat-busy throughout. The music starts hard at full level at 0.220 s and ends with a hard edit plus reverb tail at 17.02 s.
- **Beat-phase test of 19 visual events** (cuts, hits, reveals): mean |phase error| 0.272 beat, against 0.25 for random timing. Rayleigh R = 0.105, p ≈ 0.81 for the beat and p ≈ 0.82 for half-beats. **Most cuts are not beat-locked.** With triplet onsets every 4.5 f, any event is within ±2.25 f of some onset, so "near an onset" is not evidence.
- **What is locked (within ±1 frame):**
  1. Music start (0.220 s, first onset 0.232) is the text lock + disc reveal at n6 (0.233 s).
  2. The "is" ring's brightness peak (n115, 3.867 s) lands on downbeat 8 (+0.3 f). The cut into that shot comes 3.7 f earlier, so the entrance animation's **peak, not its start**, sits on the beat.
  3. Music stop (17.02 s) is the end-card reveal (17.03–17.07 s, 0–1 f late). The stop is off the beat grid (beat 37.25), so **the music was edited to the picture**.
  4. "now" comes 1.4 f before the strongest late onset (15.848 s).
- **Weak phrase pattern:** cuts at the 2-bar boundaries lead the downbeat by 2.7–3.7 f (beat 8: −3.7 f, beat 16: −3.7 f, beat 24: −2.7 f). Beat 32 breaks the pattern (−7.7 f). This matches the editorial habit of cutting about 3 frames early so the change reads on the beat, but n=3 is suggestive, not proof.

### Entrance / exit / hold durations
- **Snap entrances** (masks, rings, disc wipes): 5–9 f to 90 % (disc wipe 5 f to 98 %; ring 9 f to 90 %; opening disc about 8 f).
- **Travel entrances** (tube head): about 18 f to 90 %, about 29 f to 99 %.
- **Text reveals:** about 7 f (roll), about 8 f (radial reveal of "break free"), about 25 f (centre-out fill sweep on "BlackLine"), about 13 f (end-card sweep).
- **Exits:** mostly none; shots end mid-motion on a hard cut. Where animated, an exit is a short accelerating scale-up into a match cut (ring, 4 f) or a long dimming sweep ("now", about 35 f).
- **Holds:** there are no static holds before the end card. Every "settled" element keeps drifting: the ring grows about 0.6 %/frame after settling, the opening disc shrinks about 0.5 %/frame, and the BlackLine arc drifts about 3 px/frame. Line drawings hold between segments for 3–6 f.
- **Stagger:** the opening words converge simultaneously from three directions (0 f stagger, all lock at n6). Stroke segments are drawn sequentially with 3–6 f holds. Text reveals are spatial (centre-out) rather than per-letter time offsets: the effect of a stagger without discrete delays.

### Frame cadence
Several renders were made at 23.976 fps and conformed to 30 fps by frame repetition. Shot 5 duplicates frames 150, 155, 159, 164, 169, 173, 178, 183 (period 4.71 f, so source ≈ 23.6–24 fps). The tube shot shows alternating long/short steps (140, 93, 143, 85 … px). Shots 2–4 and 9 are judder-free.

---

## 4. Easing catalogue

Progress is normalised 0→1. "Velocity" is the fraction of total travel per frame.

| Move | Where | Data (progress per frame) | Shape | Suggested model |
|---|---|---|---|---|
| **Ring scale-in** ("is") | n111–125, Ø 56→285 px | 0, .34, .50, .62, .69, .76, .80, .84, .87, .90, .91, .94, .95, .96, .97 | Max velocity on frame 1 (34 % of travel), monotonic decay, no overshoot. 90 % at 9 f, 97 % at 14 f, then blends into drift | `1-exp(-t/3.4f)` (rms .035), or best `cubic-bezier(0.10,1.05,1.00,0.90)` over 14 f (rms .009). In practice: easeOutExpo/Quint over about 14 f, starting from 20 % scale. Critically damped spring ω≈0.3 rad/frame (≈9 rad/s) also fits |
| **Disc wipe** (BlackLine) | n147–153 | 0, .48, .81, .93, .98, .995, 1 | Very fast hard-in/soft-out, 5 f | easeOutCubic over 5 f (power-out k=3.1, rms .011); `cubic-bezier(0.25,0.30,0,1.05)` (rms .003) |
| **Opening disc shrink** | n6–20, r 412→≈250 | ≈0, .22, .36, .49, .59, ~.78, .85, .88, .91 … | Same family as the ring (τ≈3.5 f), then −0.5 %/f drift | exp-out τ≈3.5 f plus a linear drift layer |
| **Tube head rise** | n431–463, y 872→49 | 0, .17, .28, .46, .56, .60, .66, .69, .73, .75, .78, .81 … .95 (n454) … 1.0 (n463) | Starts at full speed (enters from off-frame). Velocity peaks frames 1–3 (17 % of travel per frame), long tail. Uneven steps from the 24→30 conform | exp-out τ≈6.3 f (rms .033); `cubic-bezier(0.1,0.4,0,0.9)` over 32 f (rms .011). Effectively easeOutExpo over about 1 s |
| **Viewport→render wipe** | n396–404, edge x 699→0 | 0, .03, .11, .26, .57, .77, .88, .96, 1.0 | Symmetric-ish S-curve. Peak velocity .31/frame at frame 3–4 (44 % of duration). No overshoot | easeInOutSine/Quad over 8 f; best `cubic-bezier(0.65,0.15,0.25,0.75)` (rms .006) |
| **Petal burst radius** | n295–300, 44→450 px | 0, .10, .31, .58, .83, 1.0 (exits frame) | Short ease-in, then near-constant about 115 px/f. Elements leave frame before decelerating | linear with 1-frame ease-in; `cubic-bezier(0.35,0.05,0.65,0.8)` over 5 f |
| **Word converge** ("decision") | n1–6 | 0, .08, .20, .37, .64, 1.0 (hard stop) | **Ease-in (accelerating) into a dead stop.** Steps 6, 9, 13, 20, 27 thumb-px, then 0. Matches t² | easeInQuad over 5 f + opacity ramp, ending on the music hit ("slam-in") |
| **Ring exit** (anticipation) | n142–146, Ø 283→359 | +6, +8, +16, +46 px per frame | Ease-in (exponential acceleration) straight into a match cut | easeInExpo over 4–5 f; the cut happens at peak velocity |
| **Text roll** ("is"→"where") | n130–137, about 76 px | ≈0, .11, .32, .53, .82, .89, .95, 1 | Quick ease-in-out with a short tail | easeInOutCubic about 7 f, masked to the ring interior |

**No overshoot or spring bounce** appears in any measured move. The house curve is "hard-in, soft-out": maximum velocity on the first frame, then a long exponential tail that hands off to a constant slow drift. Fast moves show **no motion blur** (petals moving 115 px/frame are crisp).

---

## 5. Techniques catalogue

| Technique | Where | AE / C4D build | Engine primitive needed |
|---|---|---|---|
| **Animated blurred gradient field** (blue/violet/amber blobs) | Shot 1 | Several solid blobs with heavy Gaussian blur and wiggle/position keys, or the Turbulent Displace / "Gradient" plugin | Mesh gradient or N metaball blobs with large blur (`blur_plane` downsampled), animated by noise |
| **Circle mask / track matte with animated radius** | Shots 1, 4, 5, 9 | Ellipse mask or shape-layer matte; Mask Expansion keys | Shape mask layer (SDF circle) with feather; alpha/inverted matte between layers |
| **Match cut via shape continuity** (ring → disc mask, text → point → ring) | 3→4, 4→5 | Pre-comp hand-off; final ring frame scaled to match the next shot's mask | Transition = shape morph/scale across a cut; shared-geometry hand-off between scenes |
| **Stroke draw-on with endpoint dot markers** + dashed construction guides | Shots 2, 3, 6, 7, 11c | Shape layers + Trim Paths; repeater/ellipse attached to path end (expression); dashed strokes | Vector paths with trim start/end, dash patterns, an "attach to path end" child, per-segment sequential timing |
| **Light glint / sweep across strokes** | Shot 3 (n104–110) | Radial gradient solid in Add mode, or CC Light Sweep | Additive radial light layer, path-animated |
| **Glow / bloom** | Shot 4 ring, shot 12 tubes, lavender strokes | Deep Glow / Glow; C4D Glow post | Multi-scale bloom (threshold + downsampled blur pyramid) |
| **Halftone dot field with radial ripple** (LED/dot-matrix look) | Shot 4 | Ball Action / CC Ball Action, or a repeater grid driven by a radial ramp; Plexus/Trapcode | Instanced dot grid whose per-dot scale/opacity comes from a field (radial wave, noise, falloff) |
| **Masked text roll** | Shot 4 | Position keys + track matte to ring interior | Per-layer transform + clip to shape |
| **Rim-lit / bevelled type with gradient fill sweep** | Shot 5 ("BlackLine") | Text with Bevel Alpha + Gradient Ramp, animated Range Selector (centre-out), or extruded C4D text | Text rendering with stroke/rim + gradient fill; reveal driven by a spatial ramp (centre-out) instead of per-glyph delays |
| **Blur/opacity sweep reveal and dim sweep** | "now", end card | Linear Wipe with feather, or Range Selector with blur/opacity | Feathered linear-gradient matte with animated offset; directional blur on text |
| **Glossy extruded 3D shapes** (petals, semicircles, segmented tori) | Shots 8–11 | C4D Extrude/Sweep, Redshift/Octane glossy plastic with soft area lights, studio HDRI | PBR (or matcap-like) lit extrusions: 2D shape → bevelled extrude; spec + Fresnel |
| **MoGraph cloner radial arrays + burst** | Shot 9 (petal flower, gold bars ring, shards) | Radial Cloner + Plain/Random Effector with falloff, keyed radius | Instancer with radial/grid layouts and effector fields (offset, rotation, scale by time/falloff) |
| **Segmented concentric rings, counter-rotating** | Shot 11 | Tube primitives with segment selections and two materials; rotation keys | Arc/ring primitive with segment colouring; parented rotations |
| **Depth of field / defocus** | Shots 10, 12 (maroon tubes) | Redshift DOF / Camera Lens Blur | Depth-based bokeh blur, or per-layer blur by z (2.5D) |
| **Emissive tubes growing along splines with a leading sphere** | Shot 12 | C4D Sweep with End Growth keyed + sphere aligned to spline; emissive material + bloom | Path-swept tube (or thick stroke with gradient shading) with grow parameter and head object; additive emission |
| **Vertical gradient vignettes / fade-to-black edges** | Shots 7, 12 | Gradient masks | Gradient matte layer |
| **2D orbit arcs with dot handles over 3D** | Shot 11c | Shape layers composited over the C4D render | 2D vector overlay composited over a 3D pass |
| **BTS: viewport capture, split screen, before/after wipe** | Shots 8, 11 | Screen recording of C4D; split-screen comp; linear wipe | Screen footage + multi-panel layout + wipe transition (Lahza already has screen recording) |
| **Continuous drift layer** | every shot | `wiggle`/`time*k` expressions or slow keys | Additive "drift" animation (constant velocity/scale) layered over keyed entrances |

Not present: particles in the effects sense (no sprites or sparks), glitch, chromatic aberration, camera shake, motion blur, LED-screen pixel grid (the halftone dots are the closest thing), lens flares.

---

## 6. Engine requirements (prioritised)

**Must**
1. **Frame-accurate timeline with per-keyframe easing**: custom cubic-bezier, exponential/expo-out with a τ parameter, ease-in for anticipation, linear. Shapes start at full velocity (no ease-in) on entrances. *All shots.*
2. **Additive animation layers**: keyed entrance + constant drift on the same property, plus ease-in "push into cut". *Shots 1, 4, 5, 12.*
3. **Audio analysis and markers**: music start/stop detection, beat grid with **triplet and straight subdivisions**, onset strength. Events snap by their **peak/landing frame**, not their start, with a lead offset (about −3 f for cuts). Option to trim the music hard to the picture's end. *Shots 1, 4, 14, 15.*
4. **Vector shapes with trim-path draw-on, dashes, end-cap dot children, sequential segment timing.** *Shots 2, 3, 6, 7, 11c.*
5. **Shape masks/mattes** (circle SDF with feather, linear gradient matte), inverted and alpha mattes, clip-to-shape for text. *Shots 1, 4, 5, 9, 11b, 14, 15.*
6. **Text engine**: per-word transforms/opacity, gradient fill, outline/rim, spatial (centre-out, L→R) reveal selectors, blur reveal. *Shots 1, 4, 5, 9, 14, 15.*
7. **Bloom/glow** (thresholded, multi-scale). *Shots 3, 4, 12.*
8. **Hard cuts and match-cut hand-off** between scenes (shape continuity), plus linear wipe transition. *3→4, 4→5, 11b.*

**Should**
9. **Instancer/cloner with field effectors** (radial/grid layouts; scale/offset by radial wave, falloff, time). *Shots 4 (halftone), 9 (burst).*
10. **Animated mesh/blob gradient background.** *Shot 1.*
11. **2.5D/3D layer of extruded shapes** with glossy shading (bevel, spec, Fresnel), orthographic or perspective camera. *Shots 9–11c.*
12. **Swept tubes along paths with grow and head object, emissive gradient shading.** *Shot 12.*
13. **Depth-of-field / per-layer defocus.** *Shots 10, 12.*
14. **Split-screen / multi-panel layout for footage** (BTS). *Shots 8b, 11.*

**Nice**
15. Light sweep / glint layer presets. *Shot 3, end card.*
16. Mixed-fps source handling (24 fps animation conformed to 30 fps without judder; this source shows duplicate-frame judder, so the engine should render natively at output fps).
17. Optional motion blur (unused here; the crisp look is intentional, so it should be off by default for this style).

---

## 7. Non-engine content

- **Music**: a licensed 133 BPM triplet-feel electronic bed. No SFX layer exists; a library track or an AI-music source is needed.
- **Fonts**: geometric sans (Outfit/Urbanist/Gilroy class) and an italic for the URL.
- **3D assets** (if not built procedurally): petal/teardrop and semicircle extrusions, segmented tori and swept letter tubes. All are simple primitives a procedural 3D layer could generate. Materials and lighting (glossy plastic, studio HDRI) would otherwise need an offline renderer such as Redshift or Octane.
- **BTS footage**: Cinema 4D viewport screen captures. These are real application recordings, not engine output (Lahza's recorder would supply the equivalent).
- **Brand assets**: the "BlackLine" name and the end-card URL.
