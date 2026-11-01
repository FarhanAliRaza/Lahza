# BOUNC (Buff Motion): motion study

Source: `/home/farhan/Downloads/BOUNC _ Buff Motion.mp4`. Working files: `study/bounc/` next to this report. It holds `track.tsv` (per-frame blue-ball tracker), `align.tsv` (event-to-audio table), `audio_out.txt` and `bands.txt` (librosa output), `spec.png`, the contact sheets in `frames/`, and the scripts in `scripts/`.

**Frame convention.** The video stream starts at pts 1/30 s and the audio starts at 0. All frame numbers here are **timeline frames = pts × 30**, so t = f/30 s and the numbers line up directly with audio time. The `s_*.png` sheets are labelled with the decode index, which is timeline − 1. The `t_*.png` sheets are labelled in timeline frames. Pixel measurements come from a 640×360 downscale ("640-px"); double them for 1280×720.

---

## 1. Overview

| | |
|---|---|
| Length | 29.80 s video (894 frames) / 29.89 s audio |
| Frame rate / size | 30 fps, 1280×720, 16:9 |
| Cuts | 12 hard cuts, 13 shots including the end card. No dissolves, wipes or transitions of any other kind. |
| Audio | AAC stereo. A designed score (music plus impact SFX). Pulse ≈ 108–112 BPM from 4 s onward. It fades to silence by about 27.4 s. |

**Style.** This is a photoreal 3D studio piece in the C4D + Redshift/Octane look. A single flat, unshaded **electric-blue ball** is the "character". It bounces, rolls, squashes and rides through a series of minimal sculptures on a seamless warm-grey cyclorama: clay-like faceted stones, a glass sphere cluster, and a looping tube. Shots are joined by match-on-action cuts that follow the ball. The opening title and the closing logo are hard cut-ons timed to impacts. The whole piece is "one ball's journey" told through physics and timing, with almost no graphic animation.

**Palette** (sampled from frames, sRGB):

| Role | Hex |
|---|---|
| Hero ball (flat, unlit) | `#1000FE` (range `#0000FE`–`#1302FF`). Effectively pure blue. |
| Title / logo white | `#F9F9F9` – `#FAFAFA` |
| Shot 1 backdrop (warm grey, slight red cast) | `#968C8C`, `#A79D9D`, `#B2ABAB` |
| Stone shadow side (shot 1) | `#746C6D`, `#70676A` |
| Shot 2 backdrop | `#C5BEBD`, `#C9C2C1`, `#BFB8B7` |
| Glass-cluster / rope backdrop | `#D3D1D2`, `#DFDCDD`, `#E4E0E2` |
| Floor / contact shadows | `#0F0A0C`, `#231C1E` |
| End card | `#000000` + ball blue `#1000FE` |

All the greys are warm and low in chroma (R slightly > G ≈ B, saturation about 5–7%). Blue is the only saturated colour in the film.

**Typography.** There is one face: an ultra-heavy (Black) neo-grotesque sans in all caps, of the Helvetica Now Display Black / Neue Haas Black kind. Tracking is very tight: the U and N touch. The opening title "BOUNC" measures 1052×193 px (82% of frame width, cap height 27% of frame height). It is centred horizontally (x 122–1174) and sits slightly above centre (y 226–419). It is pure white and flat, with no shadow, and the ball passes **in front of** it. The end-card wordmark is the same face at about 14% of frame width, set over the stone. **Neither text element animates.** Both are hard cut-ons: the title appears on the ball's first impact and is held static for 187 frames (its bounding box is identical at frames 13, 100 and 198). The logo cuts on 17 frames after the final impact.

---

## 2. Shot list

Beat references use the librosa beat track (≈ 108–112 BPM). "Hit" is a measured audio transient: either the sharpest HF attack after a 1.5 kHz high-pass, or a librosa onset. Δ is in frames; + means the sound comes after the picture.

| # | Frames (s) | Len | On screen | Motion | In / out | Audio alignment |
|---|---|---|---|---|---|---|
| 1 | 0–199 (0.00–6.63) | 200 f / 6.67 s | Close, low angle on the top of a large faceted clay stone. Static centred title "BOUNC". | Ball drops from the top edge (f4–12) and hits the stone at f13. **The title cuts on that same frame.** Rebound apex f30, 2nd contact f48, settles about f63, then **holds still** f70–117 (1.6 s). Small drop at f118→124, hop f124→141, then a long accelerating roll off the stone's shoulder to the bottom-right (f141–199). The stone slowly yaws and pushes in (about 2.5% over 6 s). | Opens on the stone alone (f0–3). Out: the ball exits bottom-right → cut. | Impact/title f13: HF attack at 0.439 s (**+0.2 f**), sub boom 0.441 s. f48: +1.4 f (strongest early onset). f124: +0.8 f. f141: +0.9 f. |
| 2 | 200–380 (6.67–12.67) | 181 f / 6.03 s | Wide: a sculpture of three stones plus a small glass bubble, floating. | Ball enters top-left (f200, match on action from shot 1). It lands on the left stone at f208 and bounces. Rest f212–248. Hop f249–261. **Charge squash f262–278.** At f279 the left stone turns to glass and **bursts into bubbles** while the ball launches. It flies in an arc (f282–307), lands on the big stone at f308, re-bounces at f319 and rests f320–345. Then it rolls down the stone's face and out of the bottom of frame (f346–380). | In: match cut. Out: the ball exits the bottom edge → cut. | f208 land: +1.1 f. f249 takeoff: −1.9 f. Pop f279: **+2.0 f, largest HF jump in the film (×7.1)**, with a sub drop at 9.36 s. f308 land: +1.5 f (the strongest sub-band onset in the piece is 10.31 s). f319: +1.5 f. |
| 3 | 381–395 (12.70–13.17) | 15 f / 0.50 s | Medium: a single dodecahedral stone on a floor, darker light. | f381–382 empty. The ball drops in from the top at f383 with heavy motion blur and impacts at f386 (**squash 1.75:1**). It rebounds with stretch (f388) to an apex at f391. | Out: cut while the ball is falling again. | f386: −0.4 f. Cut f381: onset +1.4 f. |
| 4 | 396–408 (13.20–13.60) | 13 f / 0.43 s | Macro: the ball pressed onto a stone tip. Shallow DOF. | The ball is squashed (about 1.4:1) and keeps spreading, 22% wider over 13 f, while the camera slowly tilts. This reads as a slow-motion insert of pressure building. | Cut-in on an impact (same action, closer). | Cut: +1.1 f to a sub onset. |
| 5 | 409–441 (13.63–14.70) | 33 f / 1.10 s | Medium wide: the dodecahedral stone with the squashed ball on top. | Ball held squashed (f409–412). **The stone cracks along a vertical seam (f412–415) and splits into two halves** with flat blue cut faces. The ball pops up and **hangs** f416–432 (17 f), then falls between the halves (f433–441). The halves drift apart slowly and linearly. | Out: cut while the ball is falling. | Crack: +2.7 f. Cut f409: +1.3 f. |
| 6 | 442–467 (14.73–15.57) | 26 f / 0.87 s | Low angle near the floor; the halves land. | The ball comes in heavily blurred (stretch about 3.5:1 from blur), glances off the right half (f445–450), arcs to the floor and hits at f458 (**flat squash ≈ 2.5:1**), then rebounds. | In: cut on action (mid-fall). Out: cut on the rebound. | f458: +1.5 f. |
| 7 | 468–537 (15.60–17.90) | 70 f / 2.33 s | High angle on the floor; stone halves in the background, soft contact shadow. | The ball lands (about f472) and rolls toward the camera and left, slowing to a rest (about f500–519). Wobble squash f520–522 (anticipation). Then it **accelerates hard to the right with horizontal motion-blur smear** (f531–537) and exits right. | Out: smear exit → cut. | Wobble: HF attack −5.5 f (weak). Whip: an onset at 17.83 s sits **3 f before** the cut (sound leads). |
| 8 | 538–571 (17.93–19.03) | 34 f / 1.13 s | Clean: a white plinth edge, with the ball large in frame (about 224 px diameter). | The ball enters from the left already fast and **glides to a stop with an exponential decel** (f538–556), then keeps drifting at a constant rate. At f566–571 it rolls over the edge and drops out the bottom with sharp acceleration. | In: direction-matched cut (left → right continuity, large scale jump). Out: drop exit → cut. | Cut: −3 f relative to the 17.83 s HF hit. |
| 9 | 572–603 (19.07–20.10) | 32 f / 1.07 s | Wide: a white tube swept along a looping 3D spline. | The ball drops from the top-left, bounces once on the tube (contact about f575, apex f580), lands on the tube at f587 and **rides the spline** with blur (f587–595). It decelerates as it climbs the loop and reaches the top at f603. | In: falls in from the top (matches the exit at the bottom of shot 8). | Landing: +3.6 f (weak). |
| 10 | 604–633 (20.13–21.10) | 30 f / 1.00 s | Close on the top of the loop (thick dark tube). | The ball arrives at the crest, settles with an exponential decel (f604–612), drifts (f612–628), then tips over and accelerates left (f629–633). | In: match cut on position (ball at the top of the loop). | No distinct hit (cut at 20.13 s; nearest onset +12 f). |
| 11 | 634–664 (21.13–22.13) | 31 f / 1.03 s | Same wide as shot 9. | The ball rolls down the loop (accelerating to about 45 px/f at 640-px), decelerates as it climbs the next hump (f645–655: 31→9 px/f), then accelerates down and off to the left. Speed follows height, like a roller coaster. | In: match cut back to the wide. | Cut: +2.7 f. |
| 12 | 665–757 (22.17–25.23) | 93 f / 3.10 s | Floating cluster: large clear glass spheres (with inner bubbles), matte white spheres, a faceted rock, and flat-blue spheres. Shallow DOF. | The ball drops in from the top and pinballs between spheres (contacts around f680–720). It exits up (f722–732), falls back and **passes into the large glass sphere** (f733–736), where it is seen refracted and comes to rest at the centre (f737–757). The cluster drifts slightly. | In: falls in from the top. Out: hard cut to black. | Strong HF onset 22.384 s (f671.5). Glass entry: +1.2 f. Cut to black: no hit. |
| 13 | 758–894 (25.27–29.80) | 137 f / 4.57 s | End card on pure black: a totem of flat-blue faceted blobs. | The upper blue blob separates from the lower one and comes back (about a 19 f cycle, f758–776). A grey rendered stone **drops onto the stack at f777** and squashes it, with a 7 f settle (f777–784). Frozen from f785. **The "BOUNC" logo cuts on at f794** and holds to the end. | In: hard cut. Out: end. | Stone impact f777: **−0.6 f, the loudest onset of the film (strength 6.35)**. The logo comes 17 f (0.57 s ≈ one beat) after the impact, with no audio hit. Music decays to silence about 1.5 s after the last hit. |

---

## 3. Timing analysis

### Cut rhythm
- Shot lengths in frames, shots 1–12: 200, 181, 15, 13, 33, 26, 70, 34, 32, 30, 31, 93. The end card is 137.
- Mean 63 f (2.1 s), **median 32.5 f (1.08 s)**, minimum 13 f (0.43 s), maximum 200 f (6.67 s).
- **Three-act rhythm:**
  1. **Setup, 0–12.7 s.** Two long takes (6.7 s and 6.0 s). Long holds: in shot 1 the ball sits motionless for 48 f.
  2. **Rupture, 12.7–15.6 s.** Four cuts in 2.9 s (15, 13, 33, 26 f), all built around impact → crack → split.
  3. **Journey, 15.6–25.3 s.** A steady cadence of about 1 s (70 f, then 34, 32, 30, 31), then one 3.1 s settle shot.
- The coda is a 4.6 s end card, of which 3.6 s is a frozen hold.
- From 17.9 to 22.2 s the cuts arrive every **31.75 f on average (1.06 s)**. That equals the 2-beat period at the local tempo of 112.3 BPM (1.07 s) within 1%. However, the cuts *lead* the tracked beats by 3–7 frames, and the gap drifts. So the cadence is musical (half-bar), but each cut is placed on the ball's action, not quantised to the grid.

### Picture–sound sync (most important finding)
- **Every on-screen contact that has a designed SFX lands within −0.6 to +2.0 frames of the audio attack.** Over the 12 clean contacts (f13, 48, 124, 141, 208, 279, 308, 319, 386, 458, 734, 777), the median Δ is **+1.1 f**: the sound comes about one frame after the first contact frame. That fits the physical contact happening between frames, with the sound placed on the true contact instant. Tolerance is ±1 f for hits and ±2 f for big events.
- **Sync is to the hits, not to a beat grid.** The early bounces (0.44, 1.60, 4.13, 4.70 s) have irregular spacing: 1.17 s, 2.53 s, then 0.57 s. The librosa beat tracker locks onto *these SFX* from 1.67 s onward, so the score was written around the animation, or the hits are part of the score. Before 4.1 s there is no metric pulse: autocorrelation gives about 50 BPM noise, and the only sustained element is a tonal pad at 250–450 Hz.
- **The two biggest dramatic events are given sub-bass booms:** the stone-to-glass pop at 9.30 s (sub drop at 9.36 s) and the landing on the big stone at 10.27 s (strongest sub onset, 10.31 s). There are smaller sub hits under the title (0.44 s), the floor hit (15.3 s) and the final stone (25.96 s).
- **Dynamics follow the action.** Loudness per second (RMS): 0.19 (title hit) → 0.03 during the charge-up at 8 s (near silence) → 0.21 at the pop at 9 s → 0.03 at 11 s → 0.20–0.26 during the journey act at 13–22 s → 0.02 by 26 s → silent at 28 s. Quiet before the release is a deliberate tension device.
- **Cuts with no hit** (20.13, 25.27 s) are motion-matched cuts in the middle of a musical phrase. The cut to the end card is silent, and the next sound is the stone impact 19 f later.
- **Text timing.** The title is cut on *on the impact frame* (f13, sound at +0.2 f). The logo comes 17 f (0.57 s, about one beat at 107 BPM) after the last impact, during the ring-out.

### Entrances, holds, exits (frames at 30 fps)
- **Drop-in from the frame edge to first contact:** 8–9 f (shot 1: f4→13; shot 2: f200→208; shot 3: f383→386 is 3 f, a fast and short drop).
- **Bounce cycle**, contact to contact: 35 f (shot 1, first rebound). Then **each following bounce lasts about 0.42–0.43× the one before** (35→15 f in shot 1, 26→11 f in shot 2). That is a constant coefficient of restitution of e ≈ 0.43, with height dropping to about 18% per bounce.
- **Holds (ball still):** 48 f (shot 1), 36 f (shot 2), 26 f (shot 2), about 20 f (shots 7 and 10). The end card has a 110 f freeze.
- **Hang at the apex:** 17 f (shot 5 hover). In ordinary bounces the ball spends about 39% of its airtime in the top 10% of its height. A true parabola would give 32%.
- **Anticipation before a release:** 17 f (shot 2 charge squash), 4 f (shot 5 squash before the crack), about 3 f (shot 7 wobble before the whip).
- **Exits** always leave through a frame edge or a strong blur smear, 5–7 f from the start of acceleration to off-screen (shot 7 whip: 7 f; shot 8 drop: 6 f). **Every cut happens while the ball is moving.** No shot ends on a still ball, except the cut to the end card.
- **Stagger:** there is essentially none. The film has a single hero, the bubble burst is simultaneous within 2–3 f, and the text has no per-letter animation. This is a timing piece built on one object's physics, not on layered stagger.

---

## 4. Easing catalogue (measured)

The curves come from the blue-ball centroid and bounding box per frame (`scripts/fit.py`). "rms" is the residual of the normalised progress curve against the model.

| # | Move | Frames | Measurement | Best fit | Suggested model |
|---|---|---|---|---|---|
| E1 | Rebound rise (shot 1) | 14→30, 16 f | Velocity peaks on frame 1 (3.0× average) and decays monotonically to 0.06×. | cubic-bezier(0.15, 0.5, 0.45, 1), rms 0.002. Expo-out k=3.0, rms 0.011. Power-out exponent 2.75. | Ease-out cubic/expo, **steeper than gravity** (a parabola would be power 2). |
| E2 | Fall to contact (shot 1) | 30→47, 17 f | Velocity 0.05× → 3.2×, peaking on the last frame. | cubic-bezier(0.8, 0, 1, 1), rms 0.003. Power-in t^2.9. Expo-in k=3.25, rms 0.008. | Ease-in cubic. Together E1 and E2 give a **keyframed bounce with "hang" at the apex**: exponent about 2.6–2.9 rather than 2. Model it as y = apex − h·\|τ\|^2.75, or with AE easy-ease at about 75% influence. |
| E3 | Launch rise (shot 2) | 282→294, 12 f | Peak velocity at the start (3.0×). | cubic-bezier(0.25, 0.9, 0.7, 1), rms 0.004. Power-out 2.95. | Same as E1, a shorter version (12 f). |
| E4 | Arc fall (shot 2) | 294→307, 13 f | Peak at the end (2.6×). | cubic-bezier(0.8, 0, 0.95, 1). Power-in 2.7. | Same as E2. The arc is asymmetric: 12 f up, 13 f down. |
| E5 | Arc horizontal (shot 2) | 282→307, 25 f | Velocity 1.45× → settles at 0.86× after 10 f, then constant. | Power-in t^0.8, rms 0.007. | Linear plus a decaying launch impulse: v = v∞ + Δv·e^(−t/0.17 s). |
| E6 | Entrance glide (shot 8) | 538→565, 27 f | v0 = 57 px/f at 640-px (114 px/f at full size). Frame-to-frame velocity ratio 0.80–0.87, about 0.83 on average. After about 18 f it reaches a constant drift of 2.4 px/f (640-px). No overshoot. | cubic-bezier(0.05, 0.2, 0.1, 1), rms 0.013. Expo-out k=4.5. | **Linear-drag glide:** v(t) = v_d + (v0 − v_d)·e^(−t/τ), with τ = 0.18 s (5.4 f). Equivalent to an overdamped spring with ζ ≥ 1. Do not use a bouncy spring here. |
| E7 | Roll-off acceleration (shot 1) | 150→198, 48 f | Velocity grows about ×1.09 per frame (0.4 → 15 px/f at 640-px). | Expo-in k=4.25, rms 0.007. Power-in 3.6. | Expo ease-in over 1.6 s (gravity on a curved surface). |
| E8 | Drop off edge (shot 8) | 565→571, 6 f | Increments 0.3, 1, 2.9, 10, 25, 51 px. | Power-in t^4.5, rms 0.003. | Very sharp quartic/expo-in: "tip over, then gone". |
| E9 | Impact squash recovery (shot 3) | 386→393 | w/h aspect 1.75 (contact frame) → 1.20 → 0.77 → 0.72 (peak stretch, +3 f) → 0.78 → 0.90 → 0.95 → 1.00 (+7 f). | Damped oscillation: half-period about 3 f, amplitude ratio 0.37. | **Spring: f ≈ 5 Hz, ζ ≈ 0.30** (unit mass: k ≈ 1080, c ≈ 20). Apply it to the scale axis, preserving volume. Part of the stretch is motion blur. |
| E10 | Charge squash (shot 2) | 262→278, 17 f | Height 16 → 10 px, width 16 → 20 (w/h 1.0 → 2.0), roughly linear through to ease-out. Released in 1 frame into a vertical stretch of about 1:2 (f280–281), round again by f287. | Linear / ease-out (in), step (release). | Anticipation: about 0.55 s of slow compression, then an instant release into stretch. |
| E11 | Rebound rise (shot 3) | 386→391, 5 f | Velocity 1.37 → 1.72 → 1.2 → 0.61 → 0.11 (peaks on step 2 because of squash recovery). | cubic-bezier(0.35, 0.4, 0.45, 1). | Short ease-out. |
| E12 | End-card settle | 777→784, 7 f | Centroid y 223.7 → 225.4 → 223.9 (sub-pixel). | Critically damped, no visible ring. | Spring ζ ≈ 0.8–1. |
| E13 | Motion-blur length | throughout | Blob length = diameter + s·(displacement per frame), with s = 0.51–0.58 (shot 1, f9–12). | | **Shutter ≈ 180–200°**. Stretch on fast moves is mostly blur, not a deformer. |

**Summary of the curve language.** Vertical bounces are keyframed with cubic-strength easing (exponent about 2.75, apex hang). Horizontal glides use exponential drag. Releases accelerate exponentially. Squash is spring-damped (5 Hz, ζ 0.3) and used only for emphasis: the shot 1 contacts show essentially *no* squash, while shots 2, 3, 6, 7 and 13 show strong squash. There are no overshooting position springs anywhere: positions never overshoot their rest point.

---

## 5. Techniques catalogue

| Technique | Where | How it is built in AE/C4D | Engine primitive needed |
|---|---|---|---|
| Photoreal studio render (GI, soft area light, warm cyc, contact shadows, AO) | All 3D shots | C4D with Redshift/Octane, large softbox, infinite cyc | PBR renderer: area lights, soft shadows, GI/AO (path traced, or high-quality raster with SSGI/RT shadows); seamless cyc primitive |
| Flat, unlit "graphic" ball that still casts shadows and occludes | Every shot | Emissive/constant material with "cast shadows" on and no reflections received; or a matte pass recoloured in comp | Material mode **"unlit flat colour, shadow-casting, depth-occluding"** |
| Squash and stretch (volume preserving) | Shots 2, 3, 4, 6, 7, 13 | C4D Squash & Stretch deformer or keyed scale; AE expressions | Per-object scale on an arbitrary axis aligned to the velocity/contact normal, with volume preservation and spring-driven recovery |
| Keyframed bounces with apex hang | Shots 1, 2, 3, 5 | Hand-keyed position with eased graph-editor curves | Keyframes with per-segment bezier / power / expo easing; a **bounce generator** (restitution e ≈ 0.43, apex exponent 2.75) |
| Path-follow on a spline with energy-based speed | Shots 9–11 | Align-to-Spline tag with keyed or Xpresso position; or a dynamics ball on a collider | Spline path constraint; speed driven by a function of height (v = √(2g(h0 − h))) or keyed |
| Rolling contact (rotation matched to distance) | Shots 1, 2, 7, 8 | Roll by expression | Auto-roll: rotation = distance / radius |
| Rigid fracture / split into halves | Shot 5 (crack and split), shot 6 (halves land) | Voronoi Fracture or a modelled split with keyed halves; flat-blue cut faces | Pre-split mesh pieces plus an interior-face material; keyed or simulated rigid bodies |
| Material morph (stone → glass) plus burst into bubbles | Shot 2, f279–282 | Material/visibility swap at the impact frame, then emit glass spheres (MoGraph cloner with random scale, dynamics/explosion) | Material switch at a keyframe; instanced sphere emitter with a radial burst and drag |
| Glass with refraction, dispersion and inner bubbles | Shots 2 and 12 | Redshift glass, dispersion (Abbe), bubble geometry inside | Refraction and dispersion (RT or screen-space refraction fallback), transparent shadows / caustics approximation |
| Ball passing into a glass sphere (seen refracted) | Shot 12, f733–757 | Real geometry inside the glass | Correct refraction of objects inside transmissive volumes |
| Displaced, faceted clay stones | Shots 1–7, 13 | Sculpted low-poly rocks plus noise displacement / normal maps | Mesh import with normal/displacement maps; procedural noise displacement |
| Swept tube on a 3D spline | Shots 9–11 | Sweep NURBS (circle along spline) | Spline sweep / tube generator |
| Depth of field (shallow on macro shots) | Shots 4, 6, 7, 12 | Physical camera f-stop | Bokeh DOF post (gather or scatter) |
| 3D motion blur, about 180° shutter | All fast moves (shots 1, 3, 6, 7, 9) | Render motion blur (transform + deformation) | Velocity-buffer motion blur or sub-frame accumulation with a configurable shutter angle |
| Subtle camera moves: push-in, slow yaw, drift | Shots 1, 4, 12 | Keyed camera with linear/ease moves | Camera keyframes; multiple cameras per scene |
| Multi-camera editing with match-on-action cuts | All cuts | C4D Stage object / take system, or an AE edit of renders | **Shot sequencer**: a timeline of camera switches over one continuous simulation; tools to match ball position and direction across a cut |
| Speed / time remap (macro pressure insert) | Shot 4 | Time remapping in AE/C4D | Per-shot time-remap curve |
| Kinetic type as hard cut-ons on hits | Title f13, logo f794 | AE text layer with an in-point on a marker | 2D text layer over 3D (heavy grotesque, custom tracking); in-points snapped to audio markers |
| Flat-blue silhouettes and mask cuts (end card) | Shot 13 | Flat materials on black, or shapes in AE; a stone dropped in from 3D | Unlit materials, clip/mask planes; composite of 3D over a solid background |
| Final freeze plus ring-out | Shot 13 | Hold the last frame | Hold / freeze frame |
| Audio-locked hits | Every contact | SFX laid against frame markers | Audio onset → **marker track**; snap keyframes and cuts to markers, and markers to keyframes |

Not used: liquid blobs, trim paths, shape-layer morphs, particles beyond the one bubble burst, film grain (the end-card hold has a frame difference of exactly 0, and shot 1 holds are near 0), chromatic aberration, light leaks, glitch, camera shake, and transitions other than hard cuts.

---

## 6. Engine requirements (prioritised)

**Must**
1. **PBR 3D renderer** with soft area lights, contact shadows, AO/GI and a cyc backdrop. Needed for every shot except 13.
2. **Keyframe system with rich per-segment easing**: bezier, power-n, expo-in/out (parameter k), and damped springs (f, ζ). Measured values are E1–E13. Needed in every shot.
3. **Squash/stretch deformer** along an arbitrary axis, with volume preservation and spring recovery. Shots 2, 3, 4, 6, 7, 13.
4. **Motion blur with a shutter-angle control** (default 180°). This is what sells the speed. Shots 1, 3, 6, 7, 9, 11.
5. **Shot sequencer / multi-camera cuts** over one continuous action, plus a match-cut helper that keeps screen position and direction continuous. All 12 cuts.
6. **Audio analysis → marker track** (onsets per band, beat grid) with snapping of keyframes, cuts and text in-points to markers. This is required to reach the observed **±1 frame hit sync**.
7. **Unlit flat material that still casts shadows and occludes.** The hero ball, the end card, and the crack faces in shot 5.
8. **2D text layer composited over 3D**: heavy grotesque, negative tracking, hard cut-on. Title and logo.

**Should**
9. **Glass**: refraction, dispersion, objects seen through glass. Shots 2 and 12.
10. **DOF** (bokeh). Shots 4, 6, 7, 12.
11. **Spline path constraint** with speed from height or easing, plus **auto-roll** (rotation from distance). Shots 1, 2, 7–11.
12. **Physics-assisted bounce generator**: restitution, apex-hang exponent, auto contact markers that emit SFX cue points. Shots 1, 2, 3, 5, 6.
13. **Pre-fractured meshes and a material swap at a keyframe**, plus an instanced burst emitter. Shots 2 and 5.
14. **Spline sweep (tube) generator**. Shots 9–11.
15. **Per-shot time remap**. Shot 4.

**Nice**
16. Real rigid-body simulation (for fracture and pinball in shot 12) instead of hand keys.
17. Caustics and transparent shadows from glass.
18. Procedural rock generator (faceted, displaced).
19. Agent-facing "bounce choreography" macros: drop → bounce ×n (e = 0.43) → hold → anticipation squash → release, with automatic SFX markers.

---

## 7. Non-engine content

- **Music/score and SFX**: the bounce taps, the glass pop, sub-bass booms and the pad. These need to be composed or licensed. The engine should only place and align them.
- **Typeface**: a licensed heavy neo-grotesque (Black weight) for the "BOUNC" wordmark.
- **Sculpted stone meshes**: faceted clay rocks with surface-detail normal/displacement maps. A procedural generator could approximate them, but a hero-quality look needs authored assets.
- **Scene layouts / art direction**: the floating stone sculpture, the sphere cluster arrangement, the loop-tube spline shape.
- **Lighting rig / HDRI**: a studio softbox setup. Presets could ship with the engine.
- No live footage, illustration or screen captures are used.
