# IMEX Innovation Animated Ident (Buff Motion): frame-accurate study

Source: `IMEX Innovation Animated Idents _ Buff Motion.mp4`. Working files, scripts and contact sheets are in
`/tmp/claude-1000/-home-farhan-code-screendrop-rs/b6053da9-9a03-4b7b-96d5-1df3ace8dae5/scratchpad/study/imex/`
(`png0/fNNN.png` = frame NNN, 0-based; `frames/full_XX.png` = every frame in sheets of 30; `frames/z_*.png` = close-up sheets; `scripts/*.py` = measurement scripts).

**Conventions.** "Frame n" is the 0-based video frame (30 fps). The video stream starts at 0.0333 s and the audio at 0, so audio time t maps to **video frame = 30·t − 1**. Every beat/onset time below is already converted to video frames. Pixel values refer to the 1280×720 frame.

---

## 1. Overview

| | |
|---|---|
| Length | 488 frames, 16.27 s at 30 fps. The music runs 0–14.5 s, then a tail and SFX pops play over the end card. |
| Aspect | 1280×720 (16:9) |
| Cuts | **None, except one near-cut at frame 425** (scene score 0.31) when the logo slams in. Everything else is a continuous morph or camera move. ffmpeg scene scores stay below 0.13 everywhere else. |
| Style | Flat, unlit "toon 3D" (solid fills, no gradients or lighting) mixed with 2D shape-layer morphs. Hard contact shadows. Motion blur appears only as smears or capsule stretch, plus one rotational blur. Geometric primitives only: circle, capsule, cylinder, hemisphere, ring. |
| Music | 110.0 BPM 4/4 (beat = 16.358 frames = 0.545 s; bar = 65.43 frames = 2.18 s). Bright percussive pop track with pitched synth glides. |
| Structure | **One scene per bar.** The morph into each new scene starts on beat 1 of a bar (details in §4). |

### Palette (sampled exact pixels)

| Role | Hex |
|---|---|
| Background navy (scenes 1–3) / ink | `#0D0F3B` |
| Coral | `#E85558` |
| White | `#FFFFFF` |
| Cyan A (tubes, cyan background) | `#83CFDB` |
| Cyan B (pillars, tunnel cyan) | `#93CCD9` |
| Cream | `#FBEAD3` (also `#FAECD8`) |
| Blue | `#3A7FC2` |
| Yellow | `#FDC500` |
| Contact shadow (pillars) | `#02030A` |
| Ball shadow on tubes (darker cyan) | `#6CBCCE` |
| Rotational-blur ring (coral × navy mix) | `#A04150` |
| Logo extras | orange `#ED6C00`, dark green `#044C1A`, pale pill `#FED6D7` (frame 425 only) |

The full palette is eight colours plus two logo colours. The backgrounds rotate through navy → cyan → white → (tunnel: yellow / cream / cyan / white) → white.

---

## 2. Shot / beat list

Beat grid (video frames): beat k = 41.6 + 16.358·k. Bar starts (beat 1, inferred from where the bass drops out and returns at frames 270 and 287) fall on frames **25.2, 90.7, 156.1, 221.5, 287.0, 352.4, 417.8**. The music stops at frame **434.2** (beat 24).

| # | Frames (s) | On screen | Motion | Into next | Beat alignment |
|---|---|---|---|---|---|
| 0 | 0–20 (0–0.67) | Frame 0 is all white. A white disc shrinks to a dot. | Radius 607→33 px, exponential ease-out (§5 E1) | The dot stays as the "nucleus" | Pre-groove. A riser (1.0→2.1 kHz glide, 0.35–1.3 s) starts |
| 1 | 6–32 (0.2–1.07) | A coral blob swoops in from the top-right, shrinks into a pill and spirals round the dot | Constant ~55 px/frame linear speed while the orbit radius shrinks 119→54 px, so angular speed rises 27°→71°/frame. At 25–32 it reads as a motion-blurred ring | Ring shatters | Spiral ends on the bar start at 25.2 |
| 2 | 31–61 (1.03–2.03) | Burst: big coral arcs spiral in from the frame edge (r≈600→300 px, ~100 px/frame). The ring breaks into 5–6 dashes. At 36 everything turns into multicolour capsules that shorten into 16 px dots and lock into a 12×6 grid | Capsule length 118→79→60→41→29→16 px over frames 35–40 (velocity stretch). The grid contracts 5.8% past rest at 47, springs back +1.8% at 54, settles at 61 | Grid holds | **The grid lands on the first downbeat of the groove (41.6)**, which is also where the riser ends |
| 3 | 61–94 (2.03–3.13) | Static 12×6 dot grid. Dots randomly change palette colour (~5–10 of 72 dots per 2 frames) | Hold; camera push/tilt eases in from 82 | Dots become pillar caps | Push starts on an off-beat (82 ≈ beat 2.5) |
| 4 | 94–124 (3.13–4.13) | Dots extrude into cyan cylinders with coloured caps and black diagonal contact shadows; isometric-ish camera | Random stagger over ~8 frames; each pillar takes ~10 frames, ease-out | Whole field rotates | Rise spans exactly one beat (90.7→107.0, +3 frames) |
| 5 | 125–157 (4.17–5.23) | Camera rolls/pitches so the pillars lie down; they lengthen into big horizontal tubes; shadows fade by 131 | Screen axis angle 90°→−13.3° (overshoot) → +1.5°. Coverage 7%→30% (dolly-in) | Tubes settle | **Peak angular velocity at 140 = beat 139.7**; settles at 156 = bar start |
| 6 | 157–221 (5.23–7.37) | Three rows of horizontal tubes. The middle tube telescopes (a cream sleeve is revealed). Balls (blue, cyan, white, cream, coral, yellow) enter from the right and spiral around the tube axis with depth occlusion and darker-cyan contact shadows | Telescoping 164–176, ease-out. Balls spawn at ~167, 173, 177, 183, 189, 196 (≈6-frame stagger). Slow leftward drift of −2 px/frame | Tube yaws to face camera | Telescoping starts on an eighth (164.3) |
| 7 | 222–237 (7.4–7.9) | Tube yaws 90° to point at camera: its white end cap grows from an edge-on ellipse (92×172) into a full disc (r≈150). The body foreshortens away behind the cap | Body length 976→360 px in 222–230, with the whip at 228–230. Satellites keep orbiting and smear into capsules | White disc = "sphere" | **Exactly beat 221.5 → beat 237.9** |
| 8 | 237–266 (7.9–8.87) | White disc with 5 satellites orbiting in perspective (blue ball swings close/large at the bottom-left) | Slow orbit; disc radius drifts 157→162 | Iris | — |
| 9 | 266–281 (8.87–9.37) | Cyan background closes in as a circular iris around the navy. The cyan satellite smears into a capsule along the rim and bites a concave notch into the navy disc. The navy disc becomes a crescent bowl and the white disc squashes into a D shape: a hemisphere with a yellow yolk (the yellow ball) at its centre | Iris radius ~734→597→522→446→383→339→309 (frames 267–273), ease-out | Hemisphere | **Iris centred on beat 270.6, where the bass drops out** |
| 10 | 281–318 (9.37–10.6) | Flat-shaded hemisphere (navy back, white cut face, cream rim, blue inner face, yellow yolk) rocks and tumbles; coral and cream balls float nearby | Settle 276–292 (navy height 506→310, ratio 0.83/frame, no overshoot). Second rock 294–318 | Yolk ejects | Settles as the bass returns at 287 (bar start); rock on beat 303.3 |
| 11 | 318–340 (10.6–11.33) | Bowl flips. Yolk launches on a ballistic arc up-left, while the bowl turns into a ring (navy outline, white face) whose white face grows. The yolk slams back to the centre as a stretched blob, the coral ball hits it, and their overlap turns navy | Yolk launch at 319, apex at ~330, return 334–340 (~100 px per 2 frames). White disc then grows to fill the frame by 349 | White becomes the background | **Launch at beat 319.7, slam at beat 336.0** |
| 12 | 340–356 (11.33–11.87) | Venn chain on white: a big yellow disc; blue discs slide out of the navy "eyes"; small yellow, then cyan discs telescope outward. Every overlap gets its own fill colour | Yellow r 145→264 (overshoot 12%) → 235. Chain width 544→976, ease-out over 9 frames | Camera pushes in | Chain completes at bar start 352.4 |
| 13 | 356–422 (11.87–14.07) | Push into the yellow; a nested-circle tunnel. Each new disc is born at the centre and scales up through the frame. Colours cycle cream → cyan → white → yellow → cream → cyan → white. Rounded "worm" strokes (blue, cream, coral, navy) sweep across the corners | Spawns at 375, 383, 391, 398, 403, 408, 412: intervals 8, 8, 7, 5, 5, 4 (eighths accelerating to sixteenths) | Last white disc fills the frame (422) | Accelerando into the bar start at 417.8 |
| 14 | 422–438 (14.07–14.6) | Three capsule worms (coral, blue, dark green) rise at the bottom of the white frame. At 425 the logo bars slam in, then settle into "IMEX". Pills shoot out of the stems and become dots. The M's V folds in 432–436 | Logo width 777→720 in 6 frames (easeOutSine) | — | **Logo at 425 = off-beat 426.0 (±1). M completes at 434–435 = beat 434.2, the last music hit** |
| 15 | 438–487 (14.6–16.27) | End card. The three dots hang, then fall in sequence and land on the stems with squash; the logo slowly shrinks 724→689 px (−4.8% over 50 frames) | Landings at coral 446, blue 447, green 451 | End | 3 SFX pops at 446.2, 449.7, 452.8 after the music has stopped |

---

## 3. Transition and morph breakdown (frame by frame)

The core principle is that **each object becomes the next object**. Shapes are never swapped by a cut or crossfade. Either one object is reinterpreted (the same pixels read as a different 3D thing after a camera or object rotation), or one object scales up to become the next background.

### T0 Flash → nucleus (0–20)
- 0: full white frame (a flash; the white disc is larger than the frame).
- 1–20: the disc shrinks around a fixed centre (640,360): r = 607, 487, 392, 316, 256, 208, 171, 141, 117, 98, 83, 71, 62, 54, 48, 44, 40, 37, 35, 33. This fits **r = 28 + 579·0.79^(n−1)**, so a pure exponential ease-out.
- 20–30: the dot keeps easing to r≈26 and stays as the nucleus.

### T1 Blob → orbit → spinning ring (6–32)
- 6–10: a large coral organic blob (bean/teardrop path, ~300×380 px) enters from the top-right corner (centroid (1093,22) → (981,173)).
- 10–17: it shrinks (area 85k → 17k px²) and squashes into a short pill while curving round the nucleus (it reaches the bottom-left at 17).
- 17–24: spiral orbit. Radius 119 → 105 → 94 → 84 → 76 → 66 → 59 → 54 px; angle step per frame 27°, 31°, 33°, 38°, 44°, 55°, 71°. Linear speed stays roughly constant at ~55 px/frame, so the spin speeds up as the orbit tightens (angular momentum feel). The pill curls into a C arc (frames 20–23).
- 24–28: the C arc rotates fast enough to smear: rotational motion blur, with the trailing end fading into navy.
- 28–32: a full blurred annulus (inner r 49, outer r 100), mid colour `#A04150` (coral blended with navy).

### T2 Ring → vortex → particles → grid (31–61)
- 30–31: long coral arcs (stroke ~20 px, round caps) appear at r 500–700 px, oriented tangentially like a vortex.
- 31–33: the arcs converge inward ~100 px/frame (radial histogram moves from the 500–700 band to 300–600). The nucleus shrinks 26 → 23 → 17 → 0 (gone at 33, an ease-in "pop out").
- 33–35: the blurred ring is cut into 5 rounded dashes (r 53–94), then 6 smaller dashes that shrink. The vortex arcs shorten into dashes.
- 36: **colour switch.** Every dash becomes a capsule in the grid palette (white, yellow, blue, cyan, coral, light grey), many of them two-tone (one colour per end).
- 36–40: the capsules keep their direction of travel and shorten as they slow down: median length 79 → 60 → 41 → 29 → 16 px (= dot diameter). This is velocity-proportional stretch, i.e. fake motion blur.
- 40–47: the dots fly to the 12×6 grid slots, overshooting inward: grid width 1168 → 1110 → 1078 → **1064** (frame 47, −5.8% vs. rest).
- 47–61: the spring rebounds 1076 → 1094 → 1114 → 1130 → 1142 → **1150** (frame 54, +1.8%) and settles at 1130 by 61.

### T3 Dots → pillars (82–110)
- 82–94: the camera eases into a push and tilt (grid width 1136 → 1269 at 94, accelerating). The grid takes on perspective.
- 94–104: individual dots extrude. **The dot becomes the coloured top cap and the cyan body grows downward**, with a hard black shadow (`#02030A`) pointing lower-left. Stagger is random: at 96 about 12 pillars are up, at 100 about 23, at 104 about 34.
- Single pillar (the tallest track): height 22, 42, 46, 60, 76, 82, 92, 96, 100, 102, 104 over frames 95–105, an ease-out lasting ~10 frames. The last pillars finish at ~110 (p10 height 90).
- Total shadow area follows an S-curve from 94 to 110: peak growth at 101–103, symmetric ease-in-out in aggregate.

### T4 Pillars → tubes (125–157), the main 3D reinterpretation
The same cylinders are kept. The camera rolls about 90° (screen-space axis angle measured from the structure tensor of the cyan mask) and dollies in hard, and the cylinders lengthen.
- 125–131: slow start (87° → 80°). Shadows fade out by 131.
- 131–139: accelerating (76° → 38°). Caps turn into ellipses facing right.
- 139–142: peak speed, 12–14°/frame (38° → 24° → 13° → 1°).
- 143–146: overshoot to −13.3° at 146.
- 146–157: spring back through 0 to +1.7° (157) and settle at ≈+1.4°.
- Cyan coverage 7% → 21% (143) → 30% (160). The camera keeps dollying slowly after the rotation.
- Result: three rows of long horizontal tubes, with white or blue end caps (the old dot colours) and coloured coral caps on the next row.

### T5 Tube telescoping and orbiting balls (157–221)
- 163: the middle row is two tubes butted end to end (a white cap shows in the gap).
- 164: the left tube's cap slides into the right tube (only a thin white crescent is left). At 165 they read as one continuous tube.
- 166–176: the right segment slides out to the right (cap x 912 → 942 → 974 → 990 → 996), revealing a cream sleeve: telescoping, ease-out over ~12 frames.
- 167–197: balls enter one at a time from the right edge (blue 167, cyan 173, white 177, cream 183, coral 189, yellow 196). Each follows a **helix around the tube's long axis**: it passes in front of the tube (occluding it, with a darker-cyan `#6CBCCE` contact-shadow ellipse offset on the tube), goes over the top, then behind (occluded).
- The outer rows of tubes slide horizontally at different speeds (parallax conveyor).
- 194–215: the middle tube drifts left at ~2 px/frame (keeps the frame alive).

### T6 Tube → sphere (222–237), the morph the first attempt missed
- 216–226: the right white cap is an ellipse, 118 → 88 px wide and 188 → 160 px tall. The tube starts yawing toward camera and slides right, so its left end comes into view at 228.
- 227–230: **a whip.** Body length 790 → 710 → 492 → 360 px. The cap opens from an ellipse into a near circle (aspect 0.53 → ~1.0). The cream sleeve shows as a ring around the body.
- 230–236: the body is hidden behind the now-circular cap, with only a cyan rim and the cream ring peeking out at the left. The cap grows from r≈93 to r≈145.
- 231–236: the satellites keep orbiting. The cyan and blue balls **stretch into capsules** (about 2.5:1) along their orbital paths because their angular speed jumps with the yaw.
- 237: a pure white disc (r≈150) with satellites at varied depth. The tube has become a "planet".

### T7 Sphere + navy → hemisphere with yolk, on a cyan background (264–281)
- 264–266: the cyan satellite passes over the top of the disc, stretching (smear).
- 266/267: cyan background appears at the corners. A **circular iris** (the navy region becomes a disc that shrinks) closes in, centred near (645,361).
- 268–273: iris r ≈ 597, 522, 446, 383, 339, 309. Steps 75, 76, 63, 44, 30, so a fast start that decelerates.
- 269–272: the cyan satellite slides around the right rim as a capsule. Where it overlaps the navy disc it becomes background, which **bites a concave notch** into the disc (a boolean subtract with a rounded edge). The notch travels round the rim (272–274).
- 273–276: the navy disc loses its circular shape and becomes a crescent bowl seen at three-quarter view. The white disc flattens on top into a D or half-disc, the cut face of a hemisphere seen edge-on.
- 277: a yellow yolk dot appears at the centre of the cut face (the yellow ball was "inside" the egg).
- 278–281: the inner surface of the bowl rotates into view as a blue ellipse (`#3A7FC2`), and a cream rim line appears. The coral ball squashes against the bowl's bottom (275–281) and wobbles like jelly.

### T8 Hemisphere tumble → yolk eject → ring → white fill (281–349)
- 276–292: the bowl settles. Navy bbox height 506 → 452 → 406 → 374 → 348 → 334 → 322 → 318 → 310, a geometric decay of ~0.83 per frame. The yolk is still at (648,336) from 282.
- 294–318: the bowl rocks. The navy back sweeps from right → bottom-left → bottom → right (centroid x 732 → 624 → 686), like a moon phase. The yolk grows slightly (60 → 72 px) as the face turns toward camera.
- 318–326: **flip.** The cut face rotates to face down. The yolk launches off the face on a ballistic arc: (657,342) → (615,250) → (528,171) → (435,160) → (391,183) → (376,207) → (374,241) over frames 318–332. It stretches along its velocity (96×76 at 324). The coral ball is flung off the other way as a stretched capsule.
- 324–330: the bowl is seen from below, then from the top. 330–336: a navy ring with a white face. The white face grows inside the ring: r 68, 85, 100, 114, 126, 135, 143, 149, 152 (frames 326–334), an ease-out. The navy ring stays at outer r≈200, then thins to 0 by 337 as the white swallows it.
- 334–340: the yolk comes back in and accelerates into the centre (centroid (393,279) → (443,317) → (536,349) → (639,359)). It is smeared into a gooey blob with a trailing lobe (337–339) that resolves into a circle. The coral capsule flies in from the right at the same time. **Where coral overlaps yellow, the overlap is drawn navy** (338). That navy spot becomes one "eye", and a second navy eye appears mirrored (339).
- 338–349: the white disc grows to fill the frame. Radius along the diagonal: 228, 285, 357, 431, 496, 551, 596, 634, 666, 694, 718, then ≥734 (fills) at 349. Steps 57, 72, 74, 65, 55, 45, 38, 32, 28, 24: a quick ramp then ease-out. The old background (cyan) is pushed out of the corners.

### T9 Eyes → Venn chain (339–356)
- 339–341: yellow disc r 145 → 189 → 236. The navy eyes grow, and tiny yellow dots appear inside them (another intersection colour).
- 342–347: each navy eye turns out to be the **intersection of the big yellow disc with a blue disc** sliding outward from behind. The visible blue crescent grows outside the yellow. Next a small yellow disc slides out of the blue (its overlap with blue = coral), then a small cyan disc (its overlap with the small yellow = navy). The chain is mirror-symmetric left/right.
- Yellow radius: 253, 263, **264** (344, peak), 261, 255, 247, 241, 237, **235** (350). Overshoot 12.3%, settles in 6 frames.
- Chain total width: 544, 648, 736, 804, 858, 904, 938, 960, 972, 976 (351), then 956 (355). Ease-out, with a ~2% settle-back.
- The rule for every overlap: **fill = a designated third colour from the palette, not a blend mode.** yellow∩blue = navy, blue∩small-yellow = coral, small-yellow∩cyan = navy. The same rule shows up in the tunnel (coral∩yellow = navy at 409–411).

### T10 Venn → zoom tunnel (356–422)
- 356–372: the camera pushes into the yellow disc, accelerating (equivalent radius 214 → 297; per-frame growth 1.0 → 7.9 px). The chain tilts slightly (the left circles rise, the right ones drop, about 5–8°) and slides off-frame.
- 375 onward: a new disc is born at the centre at r≈0 and scales up; the previous disc grows ahead of it. Births: cream 375, cyan 383, white 391, yellow 398, cream 403, cyan 408, white 412.
- Example radius track (white, born 391): 2, 5, 9, 15, 24, 36, 53, 76, 107, 145, 181, 227, 285, 357 (frames 391–404). Per-frame growth falls from ×2.5 to ×1.25, roughly r ∝ (n−390)^2.5. **Each layer has its own scale curve.** Ring radii are not in a fixed ratio, so this is not a uniform camera zoom.
- 393–419: in the corners, thick round-capped worms (blue, then cream, coral, navy, blue, cream) swing across. They are 2D strokes on circular arcs, and their overlaps with the discs follow the intersection-colour rule.
- 412–422: the last white disc grows 4 → 8 → 14 → 24 → 40 → 64 → 101 → 157 → 240 → 332 px and fills the frame at 422.

### T11 Tunnel → logo (421–438)
- 421–424: on the white frame, a coral round-capped stroke (upright pill), a blue pill and a dark-green S-curve worm rise into the bottom of the frame. These are the **logo's three colours arriving as the same capsule vocabulary**. The green worm flattens.
- 425 (the only near-cut): the logo appears as five tall solid bars (coral, blue, green, orange, coral with a K/X-ish notch), oversized and condensed. A pale pink pill (`#FED6D7`) sits inside the coral bar.
- 426: the logo snaps toward its final size; letter counters cut in (E gets its bars, X its notches). The coral bar's pill is now a stretched coral capsule above the I (h 212 px). A dark-green pill rides inside the blue bar.
- 426–431: overall width 777 → 763 → 749 → 736 → 726 → 721 → 720. Pills **shoot up out of the stems** (blue at 427, green at 428) and detach at 429. They shrink from capsule to circle (coral h 212 → 100 → 92 → 86 → 82 → 80 at 436). The rise decelerates to an apex at 434 (cy 120 → 114).
- 432–436: the **M is folded in.** Two vertical bars (blue, green) get a V: a green triangle hinges down from the top of the blue bar and a blue triangle from the top of the green bar (a thin sliver at 433, deep V at 434, complete at 435–436). The colours cross over.

### T12 End card (438–487)
- 434–440: the dots hang at their apex. Coral and blue start falling at 437–438; green stays up until 441.
- Coral fall, centre y: 116, 118, 122, 124, 130, 136, 147, 162, 182 (437–445). Displacements 2, 6, 8, 14, 20, 31, 46, 66: quadratic, g ≈ 2 px/frame².
- 445: stretch, h 112 / w 74 (1.5:1). 446: impact on the stem top, squash h 66 / w 80 (0.86 × 1.05). 447: 72. 448: 76. 449: 77, rest. No second bounce. Rest is ~20 px above the stem (the dot of the "i").
- Blue lands at 447 (h 65), green at 451 (h 56 / w 98, a stronger squash of 0.73).
- 437–487: the whole logo shrinks slowly (width 724 → 689, −0.1%/frame), the "alive" end-card drift.

---

## 4. Timing analysis

### Macro structure: one scene per bar
| Bar (start frame) | Scene | Event on beat 1 | beat 2 | beat 3 | beat 4 |
|---|---|---|---|---|---|
| intro (0–25) | flash, blob | flash at 0 (riser starts) | — | — | — |
| −1 (25.2) | ring → grid | spiral becomes ring spin (24–25) | grid lands, groove downbeat (41.6) | grid spring settles (57.9→61) | hold (74.3) |
| 3 (90.7) | pillars → tubes | pillars rise 94 | rise done 107–110 | tilt starts 125 (123.4) | peak tilt velocity 140 (139.7) |
| 7 (156.1) | tubes + balls | tilt settled 156 | telescope 164 (eighth), balls start 167 | balls 177–189 | ball 196, drift |
| 11 (221.5) | tube → sphere → hemisphere | yaw starts 222 | sphere complete 237 (237.9) | satellites orbit 254 | iris 266–273 (270.6), bass drops |
| 15 (287.0) | hemisphere | bowl settled, bass returns | rock 294–318 (303.3) | yolk launch 319 (319.7) | yolk slam, white fill 336 (336.0) |
| 19 (352.4) | Venn → tunnel | chain completes 351–352 | push accelerates (368.7) | tunnel births at ~eighths 375/383/391 | births at sixteenths 398/403/408/412 |
| 23 (417.8) | logo | white fill 422 | logo slam 425 (off-beat 426.0) | M complete 435 = last music hit (434.2) | dot landings 446–451 (+ SFX) |

### Sync tightness
| Visual event | Frame | Nearest grid point | Error |
|---|---|---|---|
| Grid convergence (dots rounded / lock-in) | 40–42 | beat 41.6 | 0 to −2 f |
| Pillar rise start | 94 | beat 90.7 | +3 f |
| Pillar rise end | 110 | beat 107.0 | +3 f |
| Tilt: max angular velocity | 140 | beat 139.7 | 0 f |
| Tilt settle | 155–157 | beat 156.1 | ±1 f |
| Telescope start | 164 | eighth 164.3 | 0 f |
| Tube yaw start | 222 | beat 221.5 | +0.5 f |
| Sphere complete | 237 | beat 237.9 | −1 f |
| Iris mid-close | 270 | beat 270.6 | −0.6 f |
| Yolk launch | 319 | beat 319.7 | −0.7 f |
| Yolk slam / white starts filling | 336–338 | beat 336.0 | 0 to +2 f |
| Venn chain complete | 351 | beat 352.4 | −1.4 f |
| Logo slam | 425 | off-beat 426.0 | −1 f |
| M complete / music stops | 434–435 | beat 434.2 | 0 to +1 f |
| Dot landing pops | 446, 447, 451 | SFX onsets 446.2, 449.7, 452.8 | 0, −2.7, −1.8 f |

**Sync is ±1 frame on the hero beats.** Mid-scene events (pillar rise) sit +3 frames late, which reads as "on the beat" because the motion's peak speed falls ~5 frames into a 10-frame ease-out. Two placement rules come out of this:
1. **Place the peak velocity, or the moment the shape reads, on the beat**, not the start of the motion. The tilt peaks exactly on beat 139.7 and crosses horizontal 2 frames after.
2. **Hits (landings, slams, launches) go on the beat; preparation goes on the beat before.** For example, the yolk launches on beat 3 and slams on beat 4.

### Durations (frames)
| Move | Frames | Note |
|---|---|---|
| Opening shrink to 95% | 13 (τ = 4.2) | exponential |
| Spiral orbit | 8 | 17–24 |
| Ring spin (blurred) | 8 | 25–32 |
| Particle deceleration (streak → dot) | 5 | 35–40 |
| Grid spring | 14 (47 → 61) | 1 full oscillation |
| Grid hold | ~21 | 61–82 |
| Pillar rise (one pillar) | ~10 | ease-out |
| Pillar stagger spread | ~8 | random |
| Pillar field rise | 16 = 1 beat | 94–110 |
| Tilt to tubes | 17 to crossing + 15 settle | 125–142–157 |
| Telescoping | 12 | 164–176 |
| Ball spawn stagger | ~6 (4–7) | 167…196 |
| Tube → sphere yaw | 15 (whip 3) | 222–237, whip 227–230 |
| Iris close | 7 | 266–273 |
| Hemisphere settle | 16 | 276–292 |
| Yolk airborne | ~17 | 319–336 |
| White fill (r 228 → full) | 11 | 338–349 |
| Venn chain build | 12 | 340–352 |
| Tunnel disc lifetime (birth → off-screen) | ~13–14 | e.g. 391–404 |
| Tunnel birth interval | 8, 8, 7, 5, 5, 4 | accelerando |
| Logo settle | 6 | 425–431 |
| M fold | 4 | 432–436 |
| Dot toss (up, hang, fall, squash) | ~20 | 426–446 |
| Squash recovery | 3 | 446–449 |

### Secondary motion (numbers)
- **Velocity stretch** (capsule length ∝ speed): particles 118 → 16 px (7.4:1 → 1:1) over 5 frames. Satellites and yolk up to ~2.5–3:1 during whips. Falling dot 1.5:1 on the frame before impact.
- **Squash on impact**: 0.86 height × 1.05 width (coral), 0.73 (green). Back to round in 3 frames, no rebound.
- **Overshoot**: grid −5.8% / +1.8%. Tilt 16.7% of travel. Yellow disc +12.3%. Logo ~0.5%. Pillars ≤2%. Hemisphere none.
- **Follow-through / overlap**: the camera keeps dollying after every move (cyan coverage keeps rising 143 → 160; disc keeps growing 157 → 162 px; end-card shrink). Nothing is ever fully static except the 21-frame grid hold, and even that has colour twinkle.
- **Stagger**: random (pillars, ±4 f), sequential (balls ~6 f, tunnel 8 → 4 f), near-simultaneous (logo pills 1 f apart, landings 0/1/5 f).
- **Angular acceleration from radius**: the spiral keeps linear speed constant (~55 px/f) while the radius halves, so the spin rate more than doubles.

---

## 5. Easing catalogue (measured)

Spring parameters assume mass = 1, time in seconds (ω in rad/s). ζ = damping ratio.

| ID | Move (frames) | Data | Shape | Model |
|---|---|---|---|---|
| E1 | Opening disc shrink (1–20) | r−28 decays ×0.79/frame | exponential ease-out, no overshoot | `r = r∞ + (r0−r∞)·0.79^n`; τ = 4.24 f (0.141 s); ≈ easeOutExpo `1−2^(−10t)` over 29 f; ≈ cubic-bezier(0.16, 1, 0.3, 1) |
| E2 | Particle stop / grid lock (40–61) | width 1064 (−5.8%) at 47, 1150 (+1.8%) at 54, rest 1130 at 61 | underdamped spring, half-period 7 f | ζ ≈ 0.36–0.4, ω_n ≈ 0.48 rad/f = 14.4 rad/s (2.3 Hz) → k ≈ 207, c ≈ 11 |
| E3 | Pillar extrude, single (95–105) | 22, 42, 46, 60, 76, 82, 92, 96, 100, 102, 104 | ease-out (quad/cubic out) | cubic-bezier(0.25, 0.8, 0.4, 1) over ~10 f; aggregate field = ease-in-out over 16 f |
| E4 | Pillars → tubes roll (125–157) | angle 87 → 80 (131) → 38 (139) → 1 (142) → −13.3 (146) → +1.7 (157) | ease-in to peak (≈14°/f) then spring overshoot 16.7% | keyframed ease-in (cubic-bezier(0.55, 0, 1, 0.45), 17 f) handing off to a spring: ζ ≈ 0.5, ω_n ≈ 0.33 rad/f = 9.9 rad/s (1.6 Hz) → k ≈ 98, c ≈ 9.8 |
| E5 | Telescope (164–176) | +30, +32, +16, +6 px per 3 f | ease-out | easeOutCubic, 12 f |
| E6 | Tube yaw to camera (222–237) | body length 976 → 908 → 848 → 790 → 710 → 492 → 360 (222…230) | slow-in, sudden whip, ease-out | ease-in (220–228) + fast middle (228–230) + ease-out (230–237): cubic-bezier(0.7, 0, 0.2, 1) over 15 f (strong in-out) |
| E7 | Iris close (267–273) | step 75, 76, 63, 44, 30 | ease-out (starts at full speed) | easeOutQuad/cubic, 7 f, then handed to a shape morph |
| E8 | Hemisphere settle (276–292) | height 506 → 310, ratio 0.83/f | exponential ease-out, no overshoot | τ ≈ 5.4 f; critically damped spring ω_n ≈ 0.2 rad/f |
| E9 | Yolk ballistic (318–332) | parabola up-left, ~45 px/f initial | projectile + gravity | linear velocity + constant g; stretch along velocity |
| E10 | Ring white face (326–334) | +17, 15, 14, 12, 9, 8, 6, 3 | ease-out | easeOutQuad, 9 f |
| E11 | White → background fill (338–349) | +57, 72, 74, 65, 55, 45, 38, 32, 28, 24 | short accel, then long ease-out | cubic-bezier(0.3, 0.6, 0.3, 1), 11 f |
| E12 | Yellow disc pop (338–350) | r 145, 189, 236, 253, 263, 264 (peak), 261, 255, 247, 241, 237, 235 | spring, single overshoot 12.3% | ζ ≈ 0.55, t_peak ≈ 6.5 f → ω_n ≈ 0.58 rad/f = 17.4 rad/s (2.8 Hz) → k ≈ 303, c ≈ 19 |
| E13 | Venn chain extend (342–351) | +104, 88, 68, 54, 46, 34, 22, 12, 4 | ease-out (geometric ~0.8/f), 2% settle-back | easeOutExpo-ish, 9 f |
| E14 | Camera push into tunnel (356–372) | per-frame growth 1.0 → 7.9 px, accelerating | ease-in | easeInQuad/cubic over ~20 f, flowing into the tunnel |
| E15 | Tunnel disc scale (per layer) | r ∝ (t)^~2.5; growth ×2.5/f → ×1.25/f | power ease-in in linear radius | `r(t) = R·(t/T)^2.5`, T ≈ 14 f to exit the frame |
| E16 | Logo settle (425–431) | progress 0.25, 0.49, 0.72, 0.89, 0.98, 1.0 | easeOutSine | cubic-bezier(0.61, 1, 0.88, 1), 6 f |
| E17 | Dot toss (426–449) | rise ease-out (8 f) → hang 3–6 f → fall s ≈ ½·2·t² → squash 0.86 → 3 f recovery | gravity arc with squash/stretch | Apex hold, then ease-in quad fall, g ≈ 2 px/f² (1800 px/s²); impact squash 14%, recovery 3 f (critically damped, ζ ≈ 1) |
| E18 | End-card drift (437–487) | −0.7 px/f in width | linear (slight ease-out) | linear, −0.1% scale/frame |

Summary: the vocabulary is **expo/cubic ease-out for arrivals**, **ease-in for departures and pushes**, **springs with ζ ≈ 0.35–0.55 at 1.6–2.8 Hz for "lands"**, **gravity arcs for tossed objects**, and **easeOutSine for the logo** (calm, no overshoot on the brand mark).

---

## 6. Techniques catalogue

| Technique | Where | How it would be built in AE / C4D | Engine primitive needed |
|---|---|---|---|
| Shrinking flash disc (scale-to-nucleus) | 0–20 | AE shape ellipse, scale keyframes with exponential ease | Circle shape + per-property easing (expo) |
| Organic blob → pill → C-arc | 6–24 | AE shape path morph (Bezier vertices, same vertex count), or a trim-path on a fat round-capped stroke | Bezier path morph with vertex correspondence; round-capped stroke with animatable trim start/end |
| Spiral orbit with increasing angular speed | 17–24 | Position on a spiral expression; motion follows the path | Parametric path motion (polar r(t), θ(t)) or physics orbit |
| Rotational motion blur ring | 24–32 | AE motion blur (high shutter angle) or CC Radial blur | **Per-object motion blur (shutter-angle sampling)** — must support rotation, not only translation |
| Vortex of arcs, ring shatter into dashes | 30–36 | Trim paths on concentric circles with offset + dash pattern; Repeater with rotation | Stroke dash/trim on arcs; repeater/instancer with radial distribution |
| Particles with velocity stretch → dots → grid | 36–61 | Trapcode Particular / AE Repeater + expressions (stretch by velocity), or C4D MoGraph cloner with an Inheritance effector | **Instancer (cloner)** with per-instance target slots, spring to target, **capsule length = f(speed)**, random palette per instance |
| Per-dot colour twinkle | 47–94 | Random colour effector with time offset | Random/noise field driving instance colour (discrete palette, with a hold) |
| Extruded pillars with coloured caps, hard flat shadows | 94–124 | C4D cloner of cylinders (Toon/Luminance material), Plain effector on scale Y with random delay; shadow as a projected flat shape | **3D primitives (cylinder) with unlit flat materials** per face (cap colour ≠ body colour), **hard projected shadows** as flat shapes, cloner with staggered effector |
| Camera roll/dolly turning a field of pillars into tubes | 125–157 | C4D camera rotation + dolly with overshoot in the F-curve; or AE 3D layers | **Real 3D camera** (perspective, keyframed position/rotation/FOV) with spring easing on the camera itself; object length animatable |
| Telescoping tube, cream sleeve | 164–176 | Two cylinders, one slides along the axis; the sleeve is a third cylinder | Hierarchy (parenting) + 3D transform; occlusion (depth) |
| Balls in a helix around a tube with occlusion and contact shadow | 167–237 | C4D: spheres parented to rotating nulls; toon render. AE: 2D circles with z-order swap + fake shadow ellipse | **Depth sorting/occlusion in a mixed 2D/3D scene**; flat-shaded spheres (= circles); **contact shadow as a darker-tint ellipse projected on the receiving surface** |
| Tube yaw → end-on disc | 222–237 | C4D rotation of the cylinder (toon render: the cap becomes a circle) | Same 3D cylinder rendered flat; the read-change happens automatically if the renderer is truly 3D |
| Satellite smear (capsule stretch along orbital velocity) | 231–236, 264–271, 322–336 | AE: motion-blur expression or manual stretch/rotate toward velocity | **Automatic velocity-aligned stretch** for circles (scale along motion direction by \|v\|·k, conserving area) |
| Circular iris background change | 266–273 | AE: shape layer circle mask on the old background, scale-down keyframes | Circle mask / matte with animated radius; layer mattes |
| Liquid "bite" where a ball touches a disc | 269–274 | AE: Gooey (blur + levels) or a boolean subtract with a rounded edge; manual path animation | **Metaball / SDF smooth union & smooth subtraction** between shapes |
| Sphere → hemisphere (egg) with tumble | 273–330 | C4D: half-sphere with toon materials (outer navy, inner blue, cut face white, rim cream), rotating; or AE path morph faking it | Either a 3D hemisphere primitive with per-surface flat colours **or** 2D path morph between authored keyshapes. A real 3D primitive + flat render is cheaper for agents to drive |
| Ballistic yolk with stretch | 318–340 | AE: position keyframes + motion-blur stretch | Physics-lite: projectile motion (v0, g) + velocity stretch |
| Gooey merge (yolk splat) | 337–339 | AE: Gooey or path morph | SDF smooth-union between circles (k parameter) |
| Background fill by growing disc | 338–349, 412–422 | Scale a circle beyond the frame | Circle with radius > frame diagonal; background layer as a normal shape |
| Venn chain with designated intersection colours | 339–372, 404–416 | AE: duplicate the circles, intersect them via track mattes (alpha matte) and fill the overlap with a third colour; or the Merge Paths "Intersect" operator | **Boolean shape ops (intersect, subtract, union) as live, animatable layers**, with their own fill. Must handle N-way chains and stay live while the operands move |
| Telescoping circles sliding out from behind | 342–352 | Position + depth order | Z-order + position springs |
| Nested-circle zoom tunnel with per-layer scale | 356–422 | AE: shape layers with staggered scale keyframes (ease-in), colour cycle; or an expression `scale = f(time − index·delay)` | **Time-offset instancer** (birth interval, per-instance curve, palette cycle), exponential/power scale curve; birth times quantised to the beat grid with accelerando |
| Thick round-capped "worm" strokes | 393–424 | AE shape stroke with round caps + trim paths, path animated | **Variable stroke along a Bezier path with round caps, trim start/end animation** |
| Logo assembly from bars + pills | 425–437 | AE: letters as separate shape layers; rectangles morph to glyphs via masks; dots are pills parented to the stems; the V of the M is two triangles rotating about a hinge | Path morph rect → glyph (or reveal by animated masks); **anchor/pivot control** for hinge rotations; per-letter layers |
| Dot toss with gravity + squash & stretch | 426–451 | AE: position keyframes (or Newton), scale keys for squash | **Squash/stretch tied to velocity and impact** (an auto-squash helper); gravity motion |
| End-card slow scale drift | 437–487 | Linear scale keyframes | Linear interpolation |
| Music sync | throughout | Markers from audio, keyframes snapped | **Beat grid (BPM + offset) as a first-class timeline**; snap keyframe peaks (not starts) to beats; audio markers for SFX |

---

## 7. Engine requirements (prioritised)

### MUST
1. **Morph-by-reinterpretation via a real 3D camera with flat (unlit) shading.** Cylinder, sphere, hemisphere and capsule primitives, with a flat colour per surface (cap vs. body, inner vs. outer). This is what makes pillars → tubes (125–157), tube → disc (222–237) and sphere → egg (273–330) work without hand-drawn morphs. A perspective camera with keyframed rotation, dolly and FOV, and spring easing on camera channels. *Shots 4, 5, 6, 7, 10, 11.*
2. **Per-property easing library + damped springs.** Expo/cubic/sine in-out (cubic-bezier) plus springs parametrised by (ζ, f) or (k, c), and a hybrid "keyframed ease-in → spring settle" (E4). Measured targets: ζ 0.35–0.55, 1.6–2.8 Hz. *All shots.*
3. **Beat-grid timeline.** BPM + offset, with bar/beat/eighth/sixteenth snapping, and the ability to anchor a keyframe's **peak velocity or "read" moment** (not its start) to a beat. Accelerando quantisation (8 → 4 f births). *All shots; essential for agents.*
4. **Live boolean shape ops with designated fills** (intersect, subtract) that stay correct while the operands animate, including N-circle chains. *Shots 11, 12, 13.*
5. **Velocity-aware squash & stretch / smear for circles and capsules.** Stretch along the velocity vector, area-conserving, with an impact squash and recovery. *Shots 2, 7, 8, 9, 11, 14, 15.*
6. **Instancer / cloner** with grid and random layouts, per-instance stagger (random or sequential), per-instance palette and colour twinkle, spring-to-target, and a time-offset mode for the tunnel. *Shots 2, 3, 4, 13.*
7. **Circle/ellipse/rect/capsule shapes, Bezier paths, round-capped strokes with trim-path animation.** *Shots 1, 2, 13, 14.*
8. **Masks/mattes (iris) and full-frame shape backgrounds** (a background is just a big shape that can grow or shrink). *Shots 0, 9, 11, 13.*

### SHOULD
9. **SDF smooth union/subtraction (metaball "gooey")** between 2D shapes. *The bite (269–274), the yolk splat (337–339), the jelly coral ball (275–281).*
10. **Motion blur with shutter-angle sampling, including rotation** (the spinning ring is rendered entirely as blur). *Shot 1.* Can be approximated with sub-frame accumulation on the GPU.
11. **Bezier path morph with vertex correspondence** (blob → pill → arc; rect → letterform; logo assembly). *Shots 1, 14.*
12. **Hard flat projected shadows and contact-shadow tints** (shadow = darker tint of the receiving surface). *Shots 4, 6.*
13. **Depth sorting for mixed 2D/3D**, with orbiting objects passing in front of and behind a 3D body. *Shots 6, 7, 8.*
14. **Physics-lite channels**: projectile/gravity (v0, g), orbit/spiral (constant linear speed, shrinking radius). *Shots 1, 11, 15.*
15. **Parenting/hierarchy with pivot (anchor) control** for hinge folds (the M) and telescoping. *Shots 6, 14.*

### NICE
16. Audio analysis to auto-place the beat grid and detect onsets (BPM 110, hits on 1/8 notes), plus SFX auto-alignment for landings (pops on dot impacts). *Shot 15.*
17. A "camera never stops" helper: residual dolly/drift after each move (cyan coverage keeps rising after the tilt; end-card −0.1%/frame). *Shots 5, 8, 15.*
18. Palette-constrained colour rules (an intersection colour lookup table, random palette assignment). *Shots 2, 3, 12, 13.*
19. A half-resolution proxy preview for the tunnel and instancer scenes (many full-frame circles per frame).

---

## Appendix: audio

- **Tempo** 110.04 BPM (linear fit of librosa beats 1.42–13.96 s; jitter < 1 frame). Beat k at audio t = 1.419 + 0.5453·k s.
- **Form**: intro riser 0–1.4 s (rising glide ~1.0 → 2.1 kHz, 0.35–1.3 s). The groove enters at 1.42 s (video 41.6), the grid lock. Bass drops out on beat 14 (9.05 s, video 270.6; low band 198 vs. ~450 typical) during the iris, and comes back hard on beat 15 (9.60 s, video 287; low band 554). A brief mid-band gap at ~9.8 s (video ~293). The music stops on beat 24 (14.51 s, video 434.2) and the tail decays to silence at ~15.8 s.
- **Sound design**: pitched upward glides (slide-whistle/synth "whoop" chirps) at 4.3–4.6 s (pillar tilt), 6.5–8.4 s (many ascending lines 0.6 → 3 kHz: the ball orbit and tube whip, video 194–251), and a sustained ~2.2 kHz tone with lower partials at 10.85–11.35 s (video 325–340: bowl → ring → yolk slam). After the music: **three dry pops at 14.91, 15.02 and 15.13 s (video 446.2, 449.7, 452.8)**, one per logo dot landing (coral 446, blue 447, green 451), with the sound lagging the squash frame by 0–3 frames.
- Hi-hat/clap energy sits on every beat, with alternating off-beats. Most visual hits land on beats; only the logo slam sits on an off-beat (426.0), and the M completes on the final downbeat hit.
