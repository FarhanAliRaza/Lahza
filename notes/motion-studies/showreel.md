# Motion Design Showreel 2025 (Bohdan Martovskyi): breakdown for the Lahza engine

Source: `Motion Design Showreel 2025 – Bohdan Martovskyi.mp4`. Everything below was measured from the file. Working files (frames, scripts, plots) are in
`/tmp/claude-1000/-home-farhan-code-screendrop-rs/b6053da9-9a03-4b7b-96d5-1df3ace8dae5/scratchpad/study/sr/`
(`scripts/` has the trackers and fitters, `frames/` has the contact sheets, `frames/audio_plot.png` has the onset bands plotted against the cuts).

**Frame convention.** The video stream starts at pts 0.04 s, so **F = pts × 25**. The first frame is F1 (0.04 s) and the last is F875 (35.00 s). All times are player/pts times, and the audio starts at 0, so audio and video times line up directly. Pixel values are at 1280×720.

---

## 1. Overview

| | |
|---|---|
| Length | 35.0 s video, 35.1 s audio; 875 frames |
| Frame rate | 25 fps constant (h264) |
| Aspect | 16:9, 1280×720. Two inner formats: DTEK is letterboxed to ~2.39:1, and the neon performer shot is pillarboxed to roughly 4:3 |
| Audio | AAC stereo, 44.1 kHz. Music only, no VO. **~105 BPM** (fitted 104.9–105.3 across sections). Quarter note = 0.571 s = 14.28 f; 8th = 0.2855 s = 7.14 f; 16th = 3.57 f. Dense 16th hi-hat texture. 8th grid phase: t = 0.150 + k·0.2855 s |
| Shots | **34 shots / 33 cuts.** 31 are hard cuts; 3 are designed transitions (white canvas → phone screen, button → video zoom-through, ticket UI → same UI on a stage LED wall) |
| Shot length | mean 25.8 f (1.03 s), median 23 f, min 8 f, max 83 f. Groove body (2.08–29.84 s, 28 shots): mean 24.8 f ± 8.3 = **1.74 beats**, not quantised to beats |

**Structure, five musical blocks:**

| Block | Time | Music | Picture |
|---|---|---|---|
| Intro | 0.00–2.08 | Sparse hits separated by **silence "stops"** (0.52–0.60, 1.00–1.04) | Title type: SHOWREEL / 2025, 4 shots of 11–15 f, black/white flips |
| A | 2.08–10.80 | Groove enters (bass from ~1.1 s); **bass fades out 10.2–10.76** (a break) | UI and product (Create button, dashboard, social cards, phones, icon ring, resize) and a 3D world; ends with the kinetic "Never / looked / so" composited into live action during the bass fade-out |
| B | 10.80–20.60 | **Bass returns exactly at 10.80** | VAPI (a literal audio-waveform logo on the drop), design-token physics, collage, stop-motion, 3D world, globe, profile cards, ticket brand → stage |
| C | 20.60–29.84 | Full groove | Light/energy (DTEK), finance UI (charts, OPEN ACCOUNT → video), UK map, neon live action, logo stings |
| Outro | 29.84–35.00 | Music drops to **stutter hits over silence** (29.52–29.76 is silent), then the tail | Tiny centred "Never / looked / so" (a callback to block A) and client logos flashing in on each hit; logo wall on the final hit at 33.16, held 1.84 s |

Consecutive shots alternate brightness hard. Mean shot luminance jumps by 73/255 on average at each cut, and 15 of 33 cuts jump more than 60/255 (white↔black↔saturated). This contrast does a lot of the cutting work that beat-sync would otherwise do.

---

## 2. Shot list

Offset = cut time minus the nearest strong audio onset, in frames (negative means the picture leads the sound). "Groove" means the cut sits inside continuous music, where offsets are statistically random (see §3).

| # | Start–end s (F) | Len f | Category | On screen | Motion | Transition in | Audio alignment |
|---|---|---|---|---|---|---|---|
| 1 | 0.00–0.60 (F1–14) | 14 | type | Small bold grotesk "SHOWREEL" centred on white. Four corner credit labels (@MARTOVSKYI / MOTION DESIGNER / ART DIRECTOR) in micro mono, with characters scrambling | Letters pop on in 2 frames at 0.08. Hold. Scale-up plus bulge warp starts at F12 | Open | Letters on the first hit at 0.08 (+0.4 f) |
| 2 | 0.60–1.04 (F15–25) | 11 | type | Giant fisheye/bulge-warped "SHOWREEL", black on white, bleeding off frame | Wobble | Scale/warp punch | **−1.8 f**: lands at the end of the silence stop, before the hit at 0.67 |
| 3 | 1.04–1.60 (F26–39) | 14 | type | Inverted (white on black) "SHOWREEL" with a sine-wave warp | Warp amplitude relaxes to flat over 13 f (ease-out) | Hard cut plus invert | **−1.7 f** (silence stop 1.00–1.04, then hit at 1.11) |
| 4 | 1.60–2.08 (F40–51) | 12 | type | "2025" black on white | Per-glyph scale wave: "2" and "5" grow, "02" shrinks | Hard | Kick at 1.62 (−0.5 f) |
| 5 | 2.08–2.88 (F52–71) | 20 | **UI** | Glossy glass "Create" pill on near-black plum; 3D glass cursor | Slide-in and light-up (6 f), halo ring, cursor approach (9 f), press plus ripple rings (4 f) | 1-frame dim-in | Groove (+6.1 vs strong; −1.7 vs 8th grid). **Press starts F68 = 2.72, on the onset at 2.70** |
| 6 | 2.88–3.72 (F72–92) | 21 | **UI** | Dark-mode coaching-app window (macOS chrome), tilted in 3D, blue/purple rim glow | Whip-in with motion blur (4 f) then slow drift; **linear counters** | Hard (on the click) | −2.6 |
| 7 | 3.72–4.56 (F93–113) | 21 | 3D world | Clay-like illustrated garden: topiary, horse statue, then fountain with brand "COMFY", floating products | Camera whip (pan/tilt): frame-diff 30,26,21,12,6,4,3, then drift | Hard | −2.9 |
| 8 | 4.56–5.64 (F114–140) | 27 | **UI/social** | 3D cluster of rounded photo/video cards (BMX), profile header, location tag, emoji stickers, "#BIKELIFE" | Cluster spins ~180° in Y (back faces visible F117–F121), settles by F128, then keeps orbiting (S-curve, peak 6 px/f, 3 px/f at the cut) | Hard, mid-spin | −0.5 |
| 9 | 5.64–6.56 (F141–163) | 23 | **product** | Three iPhone models on black | Fast Y-spins that strobe front/back each frame, decelerating; staggered settle (left ~F149, middle ~F151, right ~F153); specular glare sweeps across the screens; slow drift | Hard | −2.0 |
| 10 | 6.56–7.96 (F164–198) | 35 | **UI** | Ring of app icons (Slack, Notion, Trello, Asana, Todoist, GitHub, Google…) on frosted glass tiles, orbiting the word "Smooth" in perspective; light blue (#89ddfc–#b1dcfc) | Ring orbit ≈ 6–7°/frame (~90° in 14 f) plus pull-back; DoF on near icons; the "o" glyphs morph/loop | Hard | −3.4 |
| 11 | 7.96–8.96 (F199–223) | 25 | **UI** | Figma-style selection box (corner handles) with emoji on a white grid; black arrow cursor | Drag-resize from the bottom-right handle, top-left anchored, proportional; 18 f ease-out; emoji swaps 😐→🤯 at F215; 4 f hold | Hard | +3.0 |
| 12 | 8.96–10.80 (F224–269) | 46 | type + live action | "Never" (purple–blue gradient) letters converge; 1-frame word swap to "looked" (F239); the white canvas becomes a **phone screen** in live footage (woman holding phone); 3D iridescent extruded "so" flies out of the screen | Converge 9 f; screen pull-back F241–247; "so" fly-out 2 f plus 14 f settle; "so" exits upward (ease-in) F266–268 | Hard into type; **match transition** into footage | Cut +6.6 (groove), **but letters land at 9.28 on the onset at 9.28 (0 f)** |
| 13 | 10.80–11.48 (F270–286) | 17 | logo | VAPI wordmark inside a circle; iris/ring reveal expands past frame; multicolour audio-waveform bars either side | Ring expands over 3 f; bars pulse (loosely audio-reactive) | Hard | **Bass returns 10.80–10.84: 0 to −1 f** |
| 14 | 11.48–12.24 (F287–305) | 19 | **UI/design-system** | Pill tags (Design System, Token, HEX, RGB, #7B61FF) on a transparency checkerboard | **2D rigid-body drop**: tags fall at a near-constant 70–83 px/f, land in ~5 f, small rebound, then slide; 5 f stagger; a red ball drops in last | Hard | −9.1 (no hit nearby) |
| 15 | 12.24–13.44 (F306–335) | 30 | collage | Paper texture, cut-out B&W archival photos, ruled document, "1952", Cyrillic text typing on, flying horseshoe, Muybridge film strip | Irregular stepped (mixed 1s and holds) | Hard | +2.8 |
| 16 | 13.44–14.44 (F336–360) | 25 | stop-motion | Top-down table with papers and props | **Strictly on 2s** (frame-diff alternates 0/11); camera drift is also stepped | Hard | −2.9 |
| 17 | 14.44–14.88 (F361–371) | 11 | stop-motion | Second table, closer | On 2s, push-in | Hard | +0.8 |
| 18 | 14.88–15.20 (F372–379) | 8 | collage | Old photo/document on the table | Fast pull | Hard | −2.6 |
| 19 | 15.20–16.52 (F380–412) | 33 | 3D world | COMFY world: fountain, cat on sofa, elevator opens on a dog carrying a box | Smooth dolly (on 1s, diff 17→10 decelerating) | Hard | −1.6 |
| 20 | 16.52–17.52 (F413–437) | 25 | cel/vector | Flat illustrated globe with shipping containers dispersing, clouds, pink swirl background | Containers scatter, globe turns | Hard | +2.7 |
| 21 | 17.52–18.48 (F438–461) | 24 | **UI** | "Monitor who saw which ads and notify sales in real-time" with a stack of colourful profile cards (front: yellow #fdee84 "Saul McGill") | Word-by-word grey→black reveal; back cards **riffle** up and down one by one | Hard | −0.6 |
| 22 | 18.48–19.92 (F462–497) | 36 | logo/UI | Blue sunburst "S" tiles plus a purple (#9258e2) pill: "Buy and sell tickets for concerts" | Circle→pill morph (5 f) with layout recentring (10 f); word-by-word type-on, one word every ~3.4 f; sunburst rays rotate | Hard | +1.6 |
| 23 | 19.92–20.60 (F498–514) | 17 | live action + UI | Same brand UI on concert LED walls | Slow drift | **Graphic match cut** | +2.1 |
| 24 | 20.60–21.48 (F515–536) | 22 | light/logo | DTEK logo, anamorphic flare, gold light streaks over live action (hand, energy) | Light streak sweeps; letterboxed | Hard | groove (next strong +9.5) |
| 25 | 21.48–22.16 (F537–553) | 17 | light/type | X-shaped streaks converge to reveal gold "200 МВт" | Bloom, streaks | Hard | −1.9 |
| 26 | 22.16–23.40 (F554–584) | 31 | **UI** | Tilted dark dashboard with a green header; bar chart; tooltip | Camera whip-in then drift; **bars pop every 3 f exactly**; tooltip hops between bars | Hard | +1.0 |
| 27 | 23.40–24.64 (F585–615) | 31 | **UI → live action** | Floating dark ticker cards (Nasdaq, KASE…) with DoF; one card morphs into a green "OPEN ACCOUNT" button; black arrow cursor; click; the button becomes a video window (man at laptop) that zooms to full frame | Morph 5 f, hover 10 f, press 4 f, zoom-through 8 f | Hard | +3.4 |
| 28 | 24.64–25.44 (F616–635) | 20 | infographic | Hatched UK map on purple (~#531c52), "United Kingdom" pill and "England" label | Map slides in; label letters converge; triangle→pill morph | Hard | −1.3 |
| 29 | 25.44–27.00 (F636–674) | 39 | live action + type | Singer with neon slanted italic "FAMILY / TECH / SUPPORT" (pink glow) around her | Words smear in horizontally (streak blur, ~10 f); orbiting pink light leaks; neon flicker/brighten | Hard | +0.1 |
| 30 | 27.00–27.88 (F675–696) | 22 | logo/type | "KAMYSHAN" in a runic display face, white on black | Glitch type-on, glyph by glyph | Hard | +0.8 |
| 31 | 27.88–28.76 (F697–718) | 22 | logo | "ОХОРОНА БЕЗПЕКА" collapses into the "016" mark | Letter-collapse morph | Hard (black→black) | +1.0 |
| 32 | 28.76–29.84 (F719–745) | 27 | logo | "sunsama" on #f5f7fd; icon morphs moon→sun→cloud | Icon morph, subtle | Hard | +1.5 |
| 33 | 29.84–33.16 (F746–828) | 83 | type/outro | Tiny centred "Never" → "looked" (F759) → "so" (F771); client logos flash in at their wall positions, 2–4 at a time | Each change is a hard pop on a hit | Hard after the silence | **+0.3; each word or flash is within 0 to −1 f of a hit** |
| 34 | 33.16–35.00 (F829–875) | 47 | logo wall | Full grid of ~90 client logos, black on white | Static hold while the music tail decays | Hard | +0.2 |

---

## 3. Timing analysis

### 3.1 Audio sync: two different regimes

I tested the cuts against the onset envelope (5 ms hop, librosa) with a random-placement null (3000 draws):

| Event set | n | within ±1 f of a strong onset | random baseline (mean / 95th pct) | within ±2 f | random | mean abs offset |
|---|---|---|---|---|---|---|
| **Groove cuts** (2.08–28.76 s) | 28 | **0.21** | 0.30 / 0.46 | 0.43 | 0.58 / 0.71 | 2.83 f |
| **Intro/break/outro events** (stop→hit) | 14 | **0.79** | 0.30 / 0.50 | **1.00** | 0.55 / 0.79 | **0.67 f** (mean signed −0.44 f) |

Cross-correlating per-frame visual change with onset strength across lags of −50…+50 f gives nothing above the null at any lag (max mean z 0.29 against a 95th percentile of 0.31). The audio is not offset; groove cuts are simply not beat-locked. Shot lengths in beats are non-integer (0.56–3.22 beats).

**How the edit is phrased:**
- **Structural hits are frame-tight and lead the sound by 0–2 frames.** Every place the music stops or drops (0.60, 1.04, 10.80 bass return, 29.84, the outro stutters, 33.16 logo wall) gets a cut or pop within ±1 f, usually 1 f early. Leading by a frame is standard editing practice; the eye reads the change in time with the sound.
- **Inside the groove, cuts float.** They fall at about one cut per 1.7 beats (~1 s), and what drives them is the animation (cut as soon as a move reads, often mid-motion, see §3.3) and big luminance flips. The constant 16th hi-hat (3.5 strong onsets/s) means any cut "feels" on the beat, so the editor doesn't need to quantise.
- **Key in-shot hits are synced even when the cut isn't.** The Create press starts on the onset at 2.70 (+0.5 f). The "Never" letters land on 9.28 (0 f). The VAPI waveform swells on the strong onsets at F275, F282/283 and F286 (0 to +1 f), but it also dips at F280–281 while the audio is steady, so it is hand-keyed or loosely audio-reactive rather than a strict audio spectrum.
- **Breaks hold back the visual energy.** During the bass fade-out (10.2–10.76) the picture shows the slowest, simplest moment in block A ("so" settling on the phone). The outro mirrors the intro: white minimal type, one change per hit.
- **Callback.** "Never / looked / so" appears at 8.96 (big, coloured, composited) and again at 29.84 (tiny, black, one word per hit). The "Smooth" icon shot implies the full tagline "Never looked so smooth".

### 3.2 Cut rhythm numbers
- Intro: 14, 11, 14, 12 f, so about one shot per beat (14.3 f).
- Groove: sorted lengths 8, 11, 17, 17, 17, 19, 20, 20, 21, 21, 21, 22, 23, 23, 24, 25, 25, 25, 27, 27, 30, 31, 31, 33, 35, 36, 39, 46 f. Interquartile range 20–31 f (0.8–1.24 s). Short 8–11 f shots only occur in pairs inside a sub-sequence (stop-motion → photo).
- Outro: 83 f of type and logo pops (events every 5–13 f), then a 47 f hold.

### 3.3 Entrance / hold / exit durations (frames at 25 fps)

| Element | Entrance (to settle ≤1.5 px) | 90 % point | Hold | Exit |
|---|---|---|---|---|
| Create button slide and light-up | 6 f | 4 f | ~2 f before the cursor enters | Press 4 f, cut on the press |
| Halo ring emitted on landing | ~8 f expand and fade | | | |
| Cursor approach (Create) | visible 9 f | 6 f | **0–1 f dwell before press** | |
| Dashboard whip-in | 4–5 f | 4 f | none: linear drift ~1–1.5 px/f to the cut | cut while moving |
| Counters | linear for the whole shot | | | cut mid-count |
| BMX card cluster settle | ~14 f of spin-down | | none: orbit continues, 3 px/f at the cut | cut while moving |
| Phones spin-down | 8–12 f, staggered ~2 f per phone | | ~10 f with drift | |
| Resize drag | 18 f | 11 f | **4 f** | cut |
| "Never" letter converge | 9 f | 5 f | 5 f | 1-frame word swap |
| 3D "so" fly-out | 2 f attack plus 14 f settle | | 2 f | 3 f ease-in exit upward |
| Tag drop (each) | fall ~6 f, land ~5 f | | | |
| Profile card riffle | 5–6 f up, 5–6 f down (11 f cycle) | | | |
| Circle→pill morph | 5 f | 4 f | | |
| Word-by-word type-on | 1 word per ~3.4 f (tickets) or ~1 word/f (Monitor…); each word fades grey→full in ~3 f | | | |
| Bars | each pops to ~95 % in 1 f and finishes in 2 f | | | |
| OPEN ACCOUNT morph | 5 f | | **10 f hover** | press 4 f, then zoom-through 8 f |

**Staggers (measured):** bars 3 f (exact); tag drops 5 f; card riffle ~2.7 f; phones ~2 f; type-on 1–3.4 f per word; ripple rings ~1 f apart.

**General rule seen everywhere:** the first visible frame is already mid-move (the best fits need a phase offset of 1–7 f, see §4), and most shots cut while something is still moving. Explicit holds are short (2–10 f). This is what makes the reel feel fast with only ~1 s shots.

---

## 4. Easing catalogue (measured)

Method: track position or size per frame (colour masks, NCC template tracking, or connected components). Then fit (a) exponential decay toward a target, v(i) = end + A·rⁱ, and (b) standard named cubic-beziers with free duration D and free phase (frames already elapsed at the first visible frame). The RMS fit error is in pixels. Raw series are in `scripts/fit.py`; the fitters are `fit.py` and `fit2.py`.

| Move | Series (per frame) | Peak speed | 90 % / settle | Overshoot | Best models (RMS px) |
|---|---|---|---|---|---|
| Create label slide-in (x) | offset from rest: 108, 60, 32, 16, 6, 2, 0 | frame 0, 48 px/f | 4 f / 6 f | 0 % | **exp r = 0.55** (1.1); AE linear-out + 90 % ease-in, D = 9, phase 3 (0.3); easeOutQuint D = 13, phase 5 (0.4) |
| Cursor approach (Create) | y: 719, 641, 586, 547, 520, 500, 485, 476, 470, 467 | frame 0, 78 px/f | 6 f / 9 f | 0 % | **exp r = 0.70** (0.9); easeOutExpo D = 20, phase 7 (0.6) |
| Dashboard whip-in (y) | 89, 100, 108, 113, 116, 117 (+ motion blur: sharpness 153→474 over 4 f) | frame 0 | 4 f / 4 f | 0 % | exp r = 0.59; cubic-bezier(0, 0, 0, 1) D = 12, phase 7 (0.1) |
| Resize drag (box width) | Δw: 88, 62, 46, 36, 32, 26, 20, 20, 16, 12, 12, 10, 8, 6, 6, 4, 2, 4, 0 | frame 0, 88 px/f | 11 f / 18 f | 0 % | bezier(0, 0, 0, 1) D = 20 or AE linOut+90 % D = 27 (2.4); exp r = 0.82 (3.9) |
| "Never" letters converge (spread) | excess: 220, 225, 155, 97, 58, 19, 7, 9, 3, 1, 0 | frame 1, 70 px/f | 5 f / 9 f | ~0 (2 px blur artefact) | in-out start then fast settle: easeInOutCubic D = 11 (7.4). Letters have per-glyph offsets, so this is an aggregate |
| Circle→pill width | 230, 446, 536, 582, 610, 622 | frame 0, 216 px/f | 4 f / 5 f | 0 % | **exp r = 0.47**; easeOutExpo D = 9 (3.2) |
| Layout recentre (tickets) | 556, 476, 462, 450, 441, 435, 430, 427, 424, 422, 421, 420 | frame 0 | 6 f / 10 f | 0 % | bezier(0, 0, 0, 1) D = 9 (4.5); tail ratio ≈ 0.70 |
| Chart camera whip (x) | 507, 449, 409, 385, 370, 361, 352, 343, … then ~6 px/f drift | frame 0 | — | 0 % | bezier(0, 0, 0, 1) D = 19 (3.5); decay settling into linear drift |
| 3D "so" settle (after the 2 f attack) | top y: 414, 415, 419, 424, 429, 433, 437, 440, 442, 445, 446, 448, 449, 450 | — | 11 f / 12 f | **flies past rest in 2 f, then settles** | settle ≈ exp r = 0.80; whole move ≈ impulse into a critically damped system (no oscillation) |
| Button press (width) | 741, 728, 695, 655, 621 (scale 1→0.84) | frame 2–3 | 4 f | — | **easeInOutSine / AE Easy Ease, D = 5** (0.4). Create press: height 139→137→133→126→109 (→0.78), accelerating |
| Zoom-through window (top edge) | 245, 236, 211, 174, 132, 86, 43, 7, 0; velocity 9, 25, 37, 42, 46, 43, 36, 7 | frame 4 | 7 f / 8 f | 0 % | **AE Easy Ease cubic-bezier(.33, 0, .67, 1), D = 8** (4.0) |
| Card riffle (up phase) | 266, 248, 236, 210, 188, 180 | frame 2–3 | 5 f | 0 % | Easy Ease D = 5 (2.8) |
| Tag fall (physics) | vy: 67, 69, 70, 71 then landing 56, 37, 23, 15, 6 | constant | — | rebound −9 px (pink tag) | constant-velocity fall (≈1–2 px/f² acceleration, far below real gravity), soft landing, restitution ≈ 0.1, friction slide (vx −10 → −5 px/f) |
| Counters | Grammar +5/f, Filler +5.85/f, Mispronunciation +0.27/f | — | — | — | **linear**, stopped by the cut |
| BMX orbit drift | vx: −4, −1, 1, 3, 4, 5, 5, 6, 6, 6, 6, 5, 5, 4, 4, 3 | middle | — | — | sine in-out drift (a continuous camera orbit) |

**What this means for the engine:**
1. **No springy overshoot anywhere in the UI motion.** Every UI entrance is a hard ease-out with peak velocity at or before the first visible frame. The house curves are:
   - **"Snap"** (element pops: pill morph, button slide): exponential retention **r ≈ 0.47–0.55 per frame** (time constant 1.3–1.7 f). Equivalent to easeOutExpo over ~9–12 f.
   - **"Glide"** (cursor, layout, camera whips, resize): **r ≈ 0.70 per frame** (time constant 2.8 f, 90 % in 6 f). Equivalent to easeOutExpo over ~20 f, or AE "linear out, 90 % influence in".
   - **"Slow settle"** (large drags, 3D settle): **r ≈ 0.80–0.82** (90 % in 11 f).

   In code, the simplest faithful model is `x += (target − x)·(1 − r)` per 25 fps frame, i.e. a first-order lag with τ = −1/ln r frames. As a cubic-bezier, use **cubic-bezier(0, 0, 0, 1)** or **(0.16, 1, 0.3, 1)** with a pre-roll.
2. **Symmetric Easy Ease (0.33, 0, 0.67, 1)** is used only for short, self-contained gestures: the press (5 f), the riffle (5 f up and 5 f down) and the zoom-through (8 f).
3. **Cuts start mid-curve.** The best fits put 1–7 f of each move before the first visible frame. Elements should support a negative start time (pre-roll), so a clip can open with its fastest frame.
4. **Motion blur is coupled to speed.** Whip-ins are blurred on the fast frames and sharp by frame 4 (dashboard sharpness 153→474). Without shutter blur the r = 0.6–0.7 whips look like a stutter.
5. Overshoot appears only for 3D objects coming toward camera ("so": past rest in 2 f, then a 12 f settle) and in physics (tags). If the engine offers a spring, the reel's look is **critically or over-damped (ζ ≥ 1)**, not bouncy.

---

## 5. UI / product-motion deep dive

### 5.1 "Create" button (2.08–2.88, F52–71)
- **Material:** pill, about 300×140 px at rest (at this zoom). Fill is a horizontal gradient from magenta #c842a9 (left) to translucent plum #58265c (right), with a frosted/glass look. A **thin white-pink rim light** (#fea2f0–#ffb8fa) sits on the edge and travels around the pill while it moves (top-right at F52 → bottom-right at F57 → bottom-left at F60), like a rotating specular or conic-gradient stroke. A faint outer glow. Label is white medium-weight sans "Create". The background is near-black plum (#1a021f) with a faint dot grid.
- **Entrance:** slides left 108 px with r = 0.55 (6 f). It starts dark (~35 % fill) and lights up over 2 f. A **ghost outline** of the pill trails to the right on F53–F56 (an echo/motion trail). On landing (F55) a **halo ring**, a faint pill-shaped stroke, expands outward for about 8 f.
- **Cursor:** a 3D **glass cursor**: an extruded, bevelled translucent pink triangle (body #5c2d5c, rim #fea2f0). It enters from bottom-right at F58 already at peak speed (78 px/f), on a straight path up-left (~68° from horizontal). Exponential r = 0.70, rests on the button's lower-right edge at F67.
- **Click:** the press starts the next frame (F68, on the audio onset at 2.70). The button scales from 1 to 0.78 over 4 f with acceleration. **Concentric pill-shaped ripple strokes** spawn ~1 f apart (1 ring at F68, 3 at F69, 4 at F70) and expand past the frame. **The cut (F72) happens at the bottom of the press**, so the release is the cut. "Click = cut" is the transition grammar.

### 5.2 Coaching-app dashboard (2.88–3.72, F72–92)
- Dark UI window (black #000 with #1c1c1e-style cards), macOS traffic lights, left icon rail, title "Testing application to assess functionality…", metric cards on the right with **progress-ring icons** in blue, red and cyan, "Sounds like you said library [ˈlaɪbrəri]" cards, a white "Practice" pill, and a list of words with partial blue highlights ("create", "personalized", "speaking", "ready-made").
- **3D:** strong perspective tilt (rotateY about −20° to −25°, rotateX about 10°). The window corner recedes toward the upper right. A blue/purple **backlight glow** sits behind the right edge.
- **Motion:** a whip-in with heavy directional **motion blur** decays over 4 f (y offsets 11, 8, 5, 3, 1 px/f), then a slow linear camera drift of ~1–1.5 px/f with slight push-in. The right-hand list items fan out at different depths.
- **Data animation:** **linear counters** at different rates (Grammar 10→103 at +5/f, stopping at F91; Filler 0→117 at +5.85/f, still counting at the cut; Mispronunciation 1→6; Speaking styles 0→2). No easing on the numbers.

### 5.3 Social card cluster (4.56–5.64)
- Rounded-corner photo and video cards (BMX clips **playing inside the cards**) arranged at several Z depths around a centre. Overlays: profile header (avatar, follower counts), a rainbow-gradient location tag "WILSON SKATE PARK", emoji stickers 🔥😱👏, a "#BIKELIFE" hashtag, and an Instagram-style action row.
- The cluster rotates about Y (back faces are visible with mirrored text F117–F121), settles front-on by ~F128, then orbits continuously (sine velocity profile). Layers parallax by depth. A white background with soft shadows.

### 5.4 Phones (5.64–6.56)
- Three phone 3D models (white, graphite and titanium finishes) on black. Their screens show app UI (a portrait-photo app with purple CTA buttons "Experiment with New Looks", "Make your photos alive").
- **Spin-in:** fast Y rotation that strobes between front and back per frame, decelerates, and settles with a ~2 f stagger left→right. Then a **specular glare** sweeps across the screen glass (F152–F157) while the phones drift with a slight tilt. PBR materials with environment reflections (metal edges, glass).

### 5.5 App-icon ring "Smooth" (6.56–7.96)
- ~14 brand icons, each on a **frosted white translucent rounded tile** (glassmorphism, ~60–70 % white, soft shadow), arranged on a tilted ring around blue text "Smooth".
- Ring orbit ≈ 6–7°/f clockwise while the camera pulls back. Text width shrinks to ~0.65× over ~16 f, then holds. Icons near the camera are larger and **defocused (DoF)**. The "oo" in Smooth animates (rotate/morph loop). Light-blue gradient background (#89ddfc → #b1dcfc).

### 5.6 Resize with emoji (7.96–8.96)
- White canvas with a light-grey grid (~40 px cells). A lime-gradient box (#cbf686) with black square corner handles and a black macOS arrow cursor locked to the bottom-right handle.
- Proportional drag 108×80 → 518×388 px. Velocity starts at 88 px/f and decays (r ≈ 0.78–0.82), settling in 18 f. **The emoji scales with the box** and **swaps 😐→🤯 at F215**, the punchline, timed when the drag is ~95 % done. 4 f hold, then cut.

### 5.7 Design tokens (11.48–12.24)
- Pill tags with saturated fills: Design System red-orange, Token green #1ac17c, HEX blue #179ef8, RGB magenta, #7B61FF purple, and yellow Token. White or black medium sans. Background is a **transparency checkerboard** (#858585 / lighter grey), a design-tool joke.
- **Physics:** tags fall in from the top at 70–83 px/f with a 5 f stagger, collide, tilt and slide (rigid bodies with friction, low restitution). A red ball drops in at the end. Pure 2D physics; the background doesn't move.

### 5.8 Profile cards (17.52–18.48)
- Left: a three-line headline whose words are revealed one by one, grey (#bbb) to black. Right: a stack of 5 offset cards in yellow, orange, lavender, teal and pink, each with avatar, "PROSPECT" label, name and title, and "Impressions 45 / Clicks 3" chips below.
- **Riffle:** each back card in turn lifts ~80–90 px and drops back (Easy Ease, 11 f cycle), staggered ~2.7 f, so names flash past (Maria Bartra, David Li, Priya Patel, John Brooks). The front card stays still.

### 5.9 Ticket brand (18.48–20.60)
- Blue (#177bfa) square tiles with a white "S" in a rotating sunburst, plus a purple pill (#9258e2). The pill **morphs from a circle** (230 px) to a 622 px pill in 5 f (r = 0.47) while the row recentres (r ≈ 0.7, 10 f). The text types on word by word with a grey→white fade.
- Then a **graphic match cut** to the same layout on concert LED screens, showing the design "in situ".

### 5.10 Finance UI (22.16–24.64)
- **Chart dashboard:** a dark slate UI with a green header bar "CALENDAR" and a left icon rail, strongly tilted in perspective. **Bars pop in left to right every 3 f exactly**, each at ~95 % height on its first frame and full by the second. A white **tooltip with a dot marker** appears at F576 and hops bar to bar. The camera whips in (bezier(0, 0, 0, 1)) then drifts at ~4–6 px/f. Background: a teal-green radial glow (#087952).
- **OPEN ACCOUNT:** many floating **dark-glass ticker cards** (Nasdaq "$1,000,000", sparklines in green and red, "KASE $200,150", "AIX") at different depths and angles, the far ones blurred (DoF). A standard black-with-white-outline arrow cursor glides in (y deltas 50, 46, 38, 27, 15: ease-out). The central Nasdaq card **morphs into the button** through a left-to-right green fill wipe plus a resize (F588–F593, 5 f). The button has a horizontal gradient lime #4ccf1f → emerald #10d870, a ~12 px radius, an **outer green glow/bloom**, and bold white caps.
- **Hover** 10 f. **Press:** width 741→621 (0.84) over 4 f, Easy Ease. Then the button is **replaced by a video window of the same size** (a white-lit scene of a man at a laptop) with a **thin green rim line**, which expands to full frame in 8 f with Easy Ease (edge velocity peaks at 46 px/f). **Total click → full-frame: 12 f (0.48 s).** The UI element becomes the next shot (a zoom-through).

### 5.11 UK map (24.64–25.44)
- A diagonal-**hatched** UK silhouette that colours from dark to green as it slides in, on purple (~#531c52). The label pill "United Kingdom" grows out of a small triangle, and "England" letters converge from scatter (the same per-glyph converge animator as "Never").

### 5.12 UI cursor behaviour summary
- Two cursor styles: a **3D glass cursor** (stylised brand piece) and a **standard OS arrow**.
- The path is always a **straight line**, entering at full speed from off-screen or off-layout, with an exponential ease-out (r ≈ 0.70). No arcs, no overshoot, no wobble.
- Dwell before the click is **0–1 f** (Create) or **10 f** (OPEN ACCOUNT, where the button is the hero).
- The press is a 4–5 f scale-down (to 0.78–0.84) with Easy Ease or acceleration, and **the transition happens at the bottom of the press**: a cut (Create) or a zoom-through (OPEN ACCOUNT). Feedback is ripple rings (Create) or brightening and a gradient shift (OPEN ACCOUNT).
- During a drag the cursor is locked to the handle it drags (resize shot).

---

## 6. Techniques catalogue

| Technique | Where | AE / C4D build | Engine primitive needed |
|---|---|---|---|
| Strong exponential ease-out entrances with pre-roll | almost every UI shot | Graph editor: outgoing high velocity, incoming 80–100 % influence | Keyframes with cubic-bezier and exp-lag curves; negative clip start / time remap |
| Easy Ease gestures (press, riffle, zoom-through) | 5, 21, 27 | F9 Easy Ease | Same |
| Linear counters | 6 | Slider control → Source Text expression | Animated number text with format, rate and stop |
| Per-glyph converge / scatter, word-by-word reveal, typewriter, character scramble | 1, 12, 21, 22, 28, 30, 33 | Text animators with range selectors (position, opacity, blur), Offset by character or word | Text layout with per-glyph and per-word transform/opacity/colour animators and a stagger selector |
| Text warp (bulge, wave, per-glyph scale wave) | 2, 3, 4 | Mesh Warp / Bulge / Wave Warp, or animator scale with a wiggly selector | Mesh/vertex deform on text or layer (grid warp, bulge, sine) |
| Colour inversion cut | 3 | Invert / new comp | Trivial (separate shot) |
| 2.5D tilted UI planes with perspective camera | 6, 8, 10, 26, 27 | 3D layers plus camera, or C4D planes | 3D transforms (rotateX/Y/Z, z), perspective camera with position, point-of-interest, FOV |
| Camera whip-in with motion blur, then drift | 6, 7, 26 | Camera keyframes plus layer motion blur (180° shutter) | Camera animation; **shutter-based motion blur** (multi-sample or velocity buffer) |
| Depth of field | 10, 27 (6 rack) | AE camera DoF / C4D | Depth-based blur (per-layer blur by distance is enough) |
| Glassmorphism tiles and windows | 5, 10, 27 | Fast Box Blur on an adjustment layer under a mask, or C4D glass | **Background blur under a rounded-rect mask**, translucent fill, inner stroke |
| Gradient pill with travelling rim light | 5 | Gradient Ramp, stroke with a trim-paths-animated gradient, glow | Rounded rect with linear/conic gradient fill, stroke with an animated gradient offset |
| Halo / ripple rings | 5 | Repeater on a shape stroke, scale and opacity | Shape **repeater** (copies with time offset, scale, opacity) |
| Outer glow / bloom / neon | 5, 27, 29, 24, 25 | Glow, Deep Glow, Saber | Glow (threshold + blur + add), multi-radius |
| Shape morph (circle→pill, card→button, triangle→pill) | 22, 27, 28 | Rect size and roundness keys, path morph | Animatable rect size and corner radius; path interpolation |
| Fill wipe (colour sweeps across a shape) | 27 | Linear Wipe / gradient | Animated gradient or wipe mask |
| Zoom-through (UI element becomes the next shot) | 27 | Precomp the video into the button shape, scale to full frame | Nested comps / video in a masked layer, animated mask rect |
| Click ripple and press | 5, 27 | Shape layers | Cursor primitive with click event (press scale, ripple emitter) |
| Synthetic cursor, path and drag | 5, 11, 27 | Cursor PNG keyframed, parented to a handle | **Cursor layer** (OS or 3D glass skin), path with exp ease, parenting to target |
| Figma-style selection box resize | 11 | Shape layer with handles, expressions | Rect with handle decorations; anchor-relative size animation |
| Image / emoji swap on a beat | 11 | Layer cut / time remap | Hold keyframes (step) on content |
| Rigid-body 2D physics | 14 | Newton plugin | 2D physics (gravity or initial velocity, collisions, friction, restitution), baked to keys |
| Card riffle / staggered sequence | 21 | Sequence layers / expression offset | Stagger utility (n children, delay per index) |
| Bar chart growth, tooltip hopping | 26 | Shape layers or Data-driven | Chart primitive (bars with per-bar delay), callout layer |
| 3D card cluster orbit with playing video | 8 | 3D layers, null orbit | 3D group transform; video textures |
| 3D phone models spin, glare sweep | 9 | C4D / Element 3D, HDRI | glTF mesh with PBR and environment map; screen texture = video or comp |
| 3D icon ring orbit | 10 | Repeater / C4D cloner radial | Radial array (cloner) in 3D |
| 3D extruded iridescent text flying out of a screen | 12 | C4D extrude / Element 3D, Fresnel iridescence | Extruded text mesh with Fresnel or thin-film shader; or pre-rendered |
| Screen replacement on live footage | 12 | Mocha corner pin | Planar tracking / corner-pin of a comp onto video |
| Iris / circle reveal | 13 | Mask expansion, track matte | Animated masks and track mattes |
| Audio waveform bars | 13 | Audio Spectrum / keyed bars | **Audio-reactive** bars (per-band amplitude), or keyed |
| Stop-motion on 2s, paper collage | 15–18 | Posterize Time, photographed cut-outs | Posterize-time (step frame rate per layer); paper texture overlay |
| Light streaks, anamorphic flare, gold bloom | 24, 25 | Optical Flares, Saber, Trapcode | Additive streak/flare generators, bloom |
| Letterbox / pillarbox | 24, 29 | Solids | Trivial |
| Neon type around performer, light leaks | 29 | Glow plus roto, light-leak footage | Glow; video compositing with alpha/roto mattes; blend modes (screen/add) |
| Glitch / stutter type-on, letter-collapse logo morph | 30, 31 | Animators, displacement | Per-glyph step-random animator; path morph |
| Icon morph (moon→sun→cloud) | 32 | Shape morph | Path interpolation |
| Logo-wall build on hits | 33–34 | Hold keyframes on markers | **Audio-marker-driven** reveals (snap events to onsets) |
| Luminance-alternating editing | everywhere | Editorial choice | Agent rule, not an engine feature |

---

## 7. Engine requirements (prioritised)

### Must (the UI/product category and the edit cannot be reproduced without these)
1. **Curve system:** cubic-bezier keyframes plus an **exponential-lag ease** (parameter r per frame, or τ), Easy Ease presets, hold/step keys, linear. House presets from §4: `snap` r = 0.5, `glide` r = 0.7, `settle` r = 0.8, `gesture` = Easy Ease 5–8 f. *Shots 5, 6, 11, 21, 22, 26, 27, 28.*
2. **Pre-roll / clip time offset** so a shot can begin mid-animation, and the ability to cut while layers are still moving. *All.*
3. **Frame-accurate timeline with audio analysis:** onset detection, BPM / 8th grid, and detection of **silence→hit "stops"**; snapping of cuts and keyed events to hits with a configurable **lead of −1 f**. Agents should treat stop→hit points as mandatory sync points and groove cuts as free (about one cut per 1.5–2 beats). *1–4, 13, 33, 34; in-shot hits in 5, 12.*
4. **2.5D layers with a perspective camera:** rotateX/Y/Z, z-depth, camera position, look-at and FOV, and camera moves (whip + drift). *6, 8, 10, 26, 27.*
5. **Motion blur** (shutter angle, sub-frame sampling) on layers and camera. *6, 7, 8, 9, 26, 29.*
6. **Rounded-rect shape primitive** with animatable size and radius, linear/radial/conic gradient fills, strokes with animated gradients, inner and outer glow, drop shadow, and **background blur (glass)** under the shape. *5, 10, 21, 22, 26, 27.*
7. **Text engine with per-glyph and per-word animators:** position, scale, rotation, opacity, blur and colour, with a stagger/range selector; word-by-word reveal; typewriter; **number counters**. *1, 6, 12, 21, 22, 28, 30, 33.*
8. **Video/image layers inside masks and nested comps:** video in cards and windows, and **zoom-through** (a shape grows to full frame revealing the next comp). *8, 12, 23, 27.* For Lahza this is the screen-recording-in-a-window path.
9. **Cursor primitive:** OS-arrow and stylised skins; straight-line exp-ease approach; dwell; press (scale 0.8 over 4–5 f); ripple emitter; lock to a drag target; "transition on press". *5, 11, 27.*
10. **Repeater / stagger utilities:** n copies with time offset and transform deltas (ripple rings, halos, riffle, bar sequences, icon rings). *5, 10, 21, 26.*

### Should
11. **Depth of field** (blur by z). *10, 27, 6.*
12. **Glow/bloom** with neon presets. *5, 24, 25, 27, 29.*
13. **Shape/path morph** (circle↔pill↔card, triangle→pill, icon morph). *22, 27, 28, 32.*
14. **Masks and track mattes** (iris reveal, wipes). *13, 27.*
15. **Mesh/text warp deformers** (bulge, wave). *2–4.*
16. **2D rigid-body physics** baked to keyframes. *14.*
17. **Posterize time** (animate on 2s). *15–18.*
18. **Audio-reactive parameters** (band amplitude → scale or height). *13.*
19. **3D group transforms** for card clusters with playing video textures. *8.*
20. **Simple chart primitives** (bars with stagger, tooltips) and **animated counters** bound to data. *6, 26.*

### Nice
21. glTF mesh rendering with PBR and an environment map (phone spins, glare sweeps). *9.*
22. Extruded 3D text with iridescent/Fresnel material. *12.*
23. Light streak / anamorphic flare / light-leak generators and particles. *24, 25, 29.*
24. Planar tracking / corner-pin onto footage (screen replacement). *12, 23.*
25. Glitch/scramble text effects, paper/grain overlays. *1, 15, 30.*

---

## 8. Content the engine cannot make on its own

- **Live-action footage:** woman holding a phone (12), singer (29), man at a laptop (27), concert stage with LED walls (23), BMX clips (8), hand with light (24).
- **Pre-rendered 3D worlds and characters:** COMFY garden (topiary, horse statue, fountain, mermaid, floating products), interior (cat, sofa, elevator, dog with box) (7, 19); DTEK energy/light renders (24, 25).
- **3D product models:** phone models with app-screen textures (9), glass cursor model (5), extruded "so" (12; could be engine-made with extrusion).
- **Photography and stop-motion:** top-down table stop-motion sequences (16, 17), archival photos, documents, horseshoe, film strip, paper scans (15, 18).
- **Illustration:** vector globe with containers and pink swirl background (20), hatched UK map artwork (28; engine-makeable given the SVG).
- **Brand and app assets:** app icons (Slack, Notion, Trello, Asana, GitHub, Google, Todoist…), Apple emoji, client logos (~90 on the wall), wordmarks (VAPI, DTEK, KAMYSHAN, 016, sunsama, COMFY, ticket "S").
- **UI designs:** the actual app screens (coaching dashboard, finance dashboard, ticker cards, profile cards, social cards). In Lahza these would come from screen recordings or screenshots; the engine supplies the staging (tilt, glass, cursor, counters, zoom-through).
- **Fonts:** grotesk display, runic display face (KAMYSHAN), slanted heavy italic (neon), monospace micro type.
- **Music track** (~105 BPM with designed silence stops).
