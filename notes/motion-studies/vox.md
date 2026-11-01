# Vox explainer style: what it is made of, and what Lahza needs to reproduce it

Research date: 2026-11-02. Method: web search and fetch, YouTube transcripts read in the browser, and still frames viewed at chosen timestamps of four Vox videos (nothing was downloaded). Everything fetched was treated as data.

**Evidence labels used below**
- **[V]**: Vox's own material or Vox staff on the record. This includes interviews, Vox video descriptions, vox.com CSS, and frames I viewed from Vox videos.
- **[3P]**: third-party reverse-engineering, such as tutorials and an open-source agent skill. These are concrete, but they are someone's reconstruction, not Vox's own settings.
- **[I]**: my inference.

No source gave measured shot lengths or official After Effects settings for Vox. Treat the numbers here as calibrated starting points.

---

## 1. Style summary

### What makes it recognisable

1. **The narration drives the picture.**
   - Estelle Caswell: the narration "describes what's on screen". A story editor checks each script for whether it is visual. [V] Film Independent
   - Joe Posner's founding rule: tell a story as video only "if it had elements that were distinctly visual". [V] The Long Story
   - The format has no desk and few talking heads. [V] Nieman Lab
2. **Everything on screen reads as found evidence, not as a graphic someone made.**
   - Material comes in as physical objects: photos with torn white borders, newspaper clippings, documents, spreadsheets, and screens that look like screens. [3P] Moran, Flat Pack FX
   - One tutor's summary: a digital graphic "reads as opinion", while a found object reads as "firm evidence". [3P] Flat Pack FX
3. **Imperfection is added on purpose.** [3P] PremiumBeat, Mapal, Moran
   - Animation at 12 fps on a 24 fps timeline.
   - Boiling, roughened edges and slight camera shake.
   - Focus "hunting", chromatic aberration at the frame edges, grain, and a warm off-white ground instead of pure white.
   - Art director Joey Sendaydiego: "You don't want it to look perfect because that might make it look more like an ad than an editorial piece." [V] Storybench
4. **A markup layer directs the eye.** It includes highlighter sweeps, hand-drawn ellipses, underlines, labelled arrows, "contrast callouts" that dim everything except the subject, and a sharp focus region over a blurred map. [V] frames; [3P] Moran
5. **Bold condensed all-caps type over plain sans and serif text.**
   - Headlines are re-typeset as tracked condensed caps, with coloured highlight bars behind chosen lines. The outlet name sits above and the byline and date below. [V] frame, microchip video 2023
   - Lower thirds are a caps name over a sentence-case role line, with a small coloured square marking the corner. [V] frame
   - vox.com itself uses **Balto** (sans) and **Harriet** (serif). The logo yellow is **#FFF200**. [V] vox.com CSS and logo SVG
6. **Charts are drawn as print graphics on paper.**
   - A caps title with a small source line under it (for example "OUR WORLD IN DATA"), plain black marks, and labels joined to marks by leader lines. [V] frame
   - Charts build in the order the narration needs them.
   - Timelines lay down an evenly spaced ruler first, then place events on it. [3P] Flat Pack FX
7. **Each video gets its own art direction inside a shared grammar.**
   - Recent examples include a construction-paper look for a teaching video, a 1960s protest-poster look for the Fred Hampton video, and a "flying text" motif for an AI video. [V] Storybench
   - The house elements (texture, markup, type, stepped time) stay the same while the skin changes.
8. **Restrained, tactile sound.** Music sits under the voice. Sound effects are paper, marker, mechanical clicks and soft whooshes, and only the most important motion in a frame gets a sound. [3P] Moran

### How the series differ

| Series | Lead | What differs |
|---|---|---|
| **Vox Atlas** | Sam Ellis | Maps come first, then the script ("I start with visuals"). The film alternates between a bird's-eye map and on-the-ground footage, which he says "seems to really help people understand the story". Animation sits over aerial or drone footage: red city limits, yellow banners and boxes, a 3D spike graph. He often traces maps from old books. Films run about 10 min; the Egypt film took 5 weeks. [V] Storybench |
| **Earworm** | Estelle Caswell | Music analysis. The aim is a visual language that feels "tactile rather than auditory": beat breakdowns, vinyl records, a tape deck pushed into play, waveforms, beat maps. [V] TNW interview. Sync must be exact; per TNW, if one word lands off the beat the whole piece suffers. Frames viewed: a near-black ground with a faint grid, a bold display serif in pastel pink and yellow, blue and green markers, and experts shown as a framed card over a chart grid. Data work was done with The Pudding. [V] falsetto video. One episode takes about 3.5 weeks, and she researches many at once. [V] TNW |
| **Darkroom** | Coleman Lowndes | Vox's description: "digs into stories of the past, one photograph at a time". [V] video description. Ran 2018–2022, about 19 episodes of 5–11 min. [V] TheTVDB. Frames viewed: full-frame black-and-white photos with a warm sepia lift and a slow push, a tiny credit in the bottom-right corner, and period newspaper pages marked with a **red** hand-drawn ellipse and a **red** underline rather than yellow highlighter. Sources are public collections, such as the Library of Congress Lewis Hine collection. |
| **Explained** (Netflix) | — | About 18-min episodes with celebrity narration and expert interviews. "Yellow highlighting and font" is called a trademark. [3P] Jasper Pictures |
| **Johnny Harris / Borders** (overlap only) | — | He is a Vox alumnus, and his map and annotation grammar comes from the same team. Vox itself is more print-like and evidence-led. Borders added on-location host footage. [V] The Long Story |

---

## 2. Technique catalogue

Each entry gives what it looks like, how it is made (AE = After Effects), typical timing, and the engine primitive it needs.

### 2.1 Charts and data

**Line and scatter build on paper**
- **Looks like:** a warm paper ground with mottled grunge at the edges. A condensed caps title, the source in small tracked caps under it, a bold sans axis, and a log axis where needed. Points arrive in time order, and one label attaches with a thin leader line ("Advanced chips"). [V] frame, microchip video at about 1:35
- **Made with:** shape layers and trim paths for lines. Each point is its own layer, popped in with stepped time. A whoosh plus a stopwatch-tick layer plays as the line moves. [3P] Moran sound video
- **Timing [I]:** points or segments arrive on the words that name them. A line's draw is short (about 0.5–1.5 s) and runs near-linear, so the eye can follow it.
- **Engine needs:**
  - A data-driven chart component: line, scatter, bar and area, with linear or log axes.
  - Tick generation and number formatting.
  - Series reveal by trim path, or a reveal clock driven by the x value.
  - Annotations with leader lines.
  - A slot for the source line.
  - Everything rendered through the paper and texture stack (2.4).

**Bar build** [I, from the general grammar]
- Bars grow from the baseline with stepped ease. The value counter ticks in the same step rhythm.
- The highlighted bar stays at full colour while the others dim to about 0.45. [3P] hyperframes skill, "specimen grid"
- **Engine needs:** a bar or column component, per-element stagger, and a dim-others emphasis state.

**Ruler-first timeline**
- **Looks like:** evenly spaced year ticks, one per year, laid down first. Events then land at their true positions. Vox examples include the space station film (April 2022, 1995–2005 ticks) and a film-strip ruler in the A24 film. [3P] Flat Pack FX, citing the Cleveland & McGill position-accuracy finding
- **Made with:** a repeater of tick strokes, with trim paths on the base line and on each event stem.
- **Engine needs:** a timeline component with time-scaled placement. Equal spacing per event must not be the default.

**Expert inside the chart world** (Earworm)
- **Looks like:** the interview is a framed card over a dark chart grid, with data dots at the edges. [V] frame, falsetto video at 4:22
- **Engine needs:** video as a layer inside a comp, with chart elements sitting behind it.

**Showing the raw data**
- **Looks like:** a full-frame scroll over the actual spreadsheet rows ("TRUE" columns) as proof. [V] frame, falsetto video at 5:30
- **Engine needs:** a screen-capture media layer with a smooth camera scroll and the screen treatment (2.9). This is directly relevant to Lahza.

### 2.2 Documents, quotes and headlines

**Highlighter sweep**
- **Made with:** [3P] Moran, Mapal, Tripo
  - A pen-drawn path across one line of text. Stroke about 25 px at 1080p for body text, because a real marker is narrow.
  - Yellow, multiply blend, trim-path end animating 0→100.
  - A *gentle* ease, so the sweep keeps pace with the reader rather than rushing mid-line.
  - Two stacked Turbulent Displace passes: size about 2 with a high amount, plus a larger size with a low amount, to get a wobbling edge.
  - Posterize Time 12 over the result.
  - For several lines, each next line starts as the previous one finishes.
- **Timing:**
  - About 0.4 s per bar, quantised to 12 fps. [3P] hyperframes
  - Per line, roughly 0.6–1.0 s at reading speed. [I]
- **Colour:**
  - The logo yellow is #FFF200. [V]
  - A measured on-screen highlight yellow is #FFE619. [3P] hyperframes, Cooper Black film
  - Vox also uses blue bars. [V] microchip frame
  - Darkroom uses red ink. [V] frame
- **Engine needs:** stroke on a path with trim, multiply blend, a displacement noise field on the stroke, a stepped clock, and a "highlight text range" helper that finds line boxes from the text layout or from OCR word boxes on an image.

**Re-typeset headline card**
- **Looks like:** [V] frame, microchip video at about 6:20
  - The headline is set in heavy condensed caps.
  - Some lines carry a coloured bar (blue) and turn tan.
  - The outlet name ("BLOOMBERG") sits above, and "author | date" in small tracked caps below.
  - Behind it, a dimmed map from the current scene.
- **Engine needs:** a "quote/headline card" template with title, highlighted line ranges, an outlet slot and a byline slot. Bars sweep on and lines step in.

**Article or document zoom**
- **Looks like:** a scanned page or PDF placed in a pre-comp, rotated a little to straighten a crooked line, then pushed in with a scale and position move. Highlights draw on and a pull-up reveal follows. [3P] Mapal, who notes that After Effects imports PDFs directly
- **Paper realism:** two paper layers, with the top one at about 90% opacity so the page beneath bleeds through. [3P] Moran
- **Engine needs:** an image or PDF page as a layer, a camera push with ease, and the paper texture stack.

**Pencil and pen markup** (underline, circle, arrow)
- **Made with:** [3P] Moran
  - A path drawn with deliberately uneven points. Stroke about 5 px.
  - Trim paths draw it on over about 1 s with a slight ease, and a second trim keyframe on the start draws it off.
  - Roughen Edges with border about 3.5.
  - Boil: the random seed is driven by `time*4`, so the jitter changes 4 times per second.
  - Multiply blend and Posterize Time 12.
- **Labels on arrows:** labels are short bold caps of 3 words or fewer, and the arrow tip touches the subject. [3P] hyperframes
- **Engine needs:** a hand-drawn stroke style with edge roughness, a boil rate (Hz), a seed, and draw-on and draw-off.

**Contrast callout**
- **Made with:** [3P] Moran
  - An adjustment layer with Curves that lifts the blacks, so everything goes faded white, inside an inverted, feathered mask around the subject.
  - It **hard-cuts on**; it does not animate.
  - The hard cut is sold with a "circuit breaker" sound.
  - A camera punch-in runs under it, timed so the curve's peak velocity lands on the cut.
- **Text variant:**
  - Duplicate the text. Fade the base copy from 100 to about 20 opacity while the scene punches in about 10%.
  - Reveal the chosen words on the duplicate with a Linear Wipe (feather about 55) a moment after the zoom.
- **Engine needs:** a masked adjustment region with a lift/fade grade, hard on and off, and a feather setting.

**Focal (depth) callout**
- **Made with:** Compound Blur driven by a hidden radial-gradient map. The two gradient stops are pulled close together for a tight sharp region, and the gradient centre is animated from place to place as the narrator names them. Use a long, strong ease. [3P] Moran
- **Engine needs:** a variable blur driven by a mask or gradient (focus centre, radius, falloff, maximum blur), animatable along a path. This is like depth of field, but in 2D.

### 2.3 Archival photos and footage

**Torn white border ("cut-out object")**
- **Made with:** [3P] Flat Pack FX
  - Convert the photo to black and white.
  - Fill with off-white (not pure white), expand the matte with Simple Choker, and composite the photo back on top with CC Composite.
  - Roughen Edges in "spiky" mode, followed by a second, fine Roughen Edges.
  - A texture layer in Hard Light, alpha-matted to the photo.
  - The border is always white, because white is the colour of paper. Black separates the photo just as well but does not read as an object.
- **Engine needs:**
  - A matte-dilate border generator (width and colour).
  - A two-scale edge-roughen noise.
  - A per-layer texture multiply or hard light restricted to the layer's alpha.

**Parallax (2.5D) photo**
- **Made with:** [3P] Mapal
  - Cut the subject out in Photoshop, and fill the hole in the background plate with content-aware fill.
  - In AE, both layers go 3D. Over about 3 s the subject moves forward and to the left, while the plate drifts back slightly.
  - Add a fast blur of about 1 on the plate and a small Curves darken.
- **Engine needs:** 2.5D planes and a camera. Ideally also an automatic subject cut-out and inpainting step to make the two layers; that is an asset-pipeline task, not engine work.

**Zoom-isolation**
- **Looks like:** the camera pushes into a wide photo where the subject is small. The background drops to white, leaving the cut-out subject with a labelled arrow.
- **Timing:**
  - Push: power4-in over about 1.2 s to 4.5–7× scale.
  - Arrival from inverse zoom: expo-out over about 1.1 s from scale about 3.2. [3P] hyperframes
- **Engine needs:** nested zoom-through (already in the plan) plus a background-dropout state.

**Full-bleed photo hold** (Darkroom)
- **Looks like:** warm monochrome with a slow push, a tiny credit in the bottom-right corner, and the Vox mark in the top-right. [V] frames
- **Motion:** a camera creep of about 3–4% over the beat. [3P] hyperframes
- **Engine needs:** a slow linear push and a grade (tint or gradient map).

**Footage homogenisation** (mixed sources forced into one look)
- **Purpose:** archive.org, YouTube, phone video and fair-use clips all differ, so one shared treatment hides the cuts between them.
- **Made with:** [3P] Moran
  - Edge blur radius about 6 outside a feathered, inverted radial mask.
  - CC Ball Action with grid spacing 0, for a slight pixel-grid look.
  - Chromatic aberration about 2.
  - Exposure flicker `wiggle(24, 0.05)`.
  - Grain size about 1.4.
  - Optionally force black and white.
- **Period map look:** desaturate, Lumetri vignette, Posterize Time 10 fps, 16 mm grain overlay at 35%. [3P] No Film School
- **Engine needs:** a reusable "look" preset applied over a group of clips: edge blur, RGB split, exposure flicker, grain, desaturate, and frame-rate posterize.

**"Lens" adjustment over flat 2D clippings**
- **Made with:** Quick Chromatic Aberration with a slight RGB split, Gaussian blur 3.5, and a centre circle subtracted with feather 50 so the effect hits only the edges. [3P] PremiumBeat
- **Engine needs:** a radial edge-mask on effects.

### 2.4 Paper and texture overlays

**Recipe** [3P] Moran:
1. Warm the background off pure white by about 4%.
2. Roughen Edges about 2–3 on grid lines.
3. A halftone-grunge texture in Overlay (the mid-grey vanishes).
4. A light leak in Add.
5. Lumetri vignette.
6. Chromatic aberration about 0.8 (1.5 looked "amateur").
7. Posterize Time 12 on top.

**Other textures:**
- A "motion background" built from 5–6 distressed textures that swap 2–3 times per second, looped over about 10 s and placed behind static holds. [3P] PremiumBeat
- A thin grid background: a Grid effect with border about 1, low opacity, slightly rotated, with Turbulent Displace so the grid boils. [3P] Mapal
- Newsprint layering: a pale clipping strip under the words, plus a low-opacity copy on top so the texture rides the glyphs. [3P] hyperframes

**Texture sources:** texturelabs.org, free and commercial-safe. [3P] Moran

**Engine needs:**
- Texture layers with blend modes (Overlay, Multiply, Add, Hard Light, Screen).
- Texture "flip-book" cycling at N Hz with random flip and rotate.
- Paper ground colour and vignette.
- Seeded displacement noise for boil.
- Stepped time as a group or adjustment property.

### 2.5 Maps

**Outline and place callout**
- **Made with:** [3P] Mapal
  - The map image goes in a pre-comp.
  - A white pen outline, 3 px stroke, drawn by trim paths over about 1.5–2 s with an ease.
  - Place-name text in a period serif, coloured to match the map, revealed through a rectangle matte, starting before the outline finishes.
  - A location circle: ellipse size 0→target with opacity fading to 0 at the end (a radar ping), in multiply.
  - Turbulent Displace amount 40, size 2 on the outline; amount 8, size 8 on the text; evolution driven by `time*5`.
  - Optional Deep Glow in Overlay.
  - The whole map zooms in over about 2 s.
- **Atmosphere** [3P] Mapal:
  - Adjustment layers with a fast blur keyed 0→1→0→3→0 over a few frames, so focus "hunts" during the zoom.
  - Posterize Time 12.
  - A Transform at scale 101 with a wiggle on position, for camera shake.

**Highlighting a region**
- The contrast callout (2.2) with a label. [3P] Moran
- Red outlines for city limits, yellow banners and boxes for landmarks. [V] Ellis

**Emanation from a point**
- After Effects' built-in Radio Waves: white, start width 8, end width 0–1, "saw-in" profile. Laid flat in 3D, or flat on the map. [3P] Moran, copying a Vox radio-station map

**Globe and terrain flyovers**
- Google Earth Studio renders a frame sequence and exports AE tracking data, including nulls per track point. Annotations parent to those nulls. [3P] No Film School, Moran

**Map sources:**
- Traced from old printed maps, or built on existing cartography. [V] Ellis
- Map accuracy is checked by other staff, because borders change. [V] Sendaydiego

**Engine needs:**
- Vector map layers (GeoJSON) projected to the canvas.
- Path draw-on.
- A camera with eased zoom and pan to a bounding box.
- Region fill and dim.
- Pins with labels and pings.
- Geo-anchored annotations that follow the camera.
- Optional terrain or satellite tiles: either a tile fetcher or imported Google Earth Studio sequences plus tracking.

### 2.6 Kinetic type and lower thirds

**Lower third (current house style)** [V] frame, microchip video at 4:10
- No bar. Name in tracked condensed bold caps (white), with a sentence-case role line under it.
- A small coloured square at the top-left corner of the name. The colour changes per section (green, blue).
- Over full-frame archival footage.

**Older textured lower third** [3P] PremiumBeat
- A rough, jagged-edged PNG plate, revealed by a mask that grows irregularly frame by frame, skipping frames.
- The text uses the same reveal, offset later.

**Collage-style variant** [3P] hyperframes
- A black bar at about 0.94 opacity, with the role line as a yellow chip that sweeps on.

**Label chips**
- A tracked caps label with a small square swatch ("FAIRCHILD SEMICONDUCTOR"), placed beside a photo panel. [V] frame at 1:44

**Promoted sentence**
- At most one per act. Two or three lines in a staircase layout, with words stepping in at their own fast cadence; they do not wait for slow VO.
- Per-line treatments vary.
- The block settles from 1.24× to 1.0×. [3P] hyperframes

**Engine needs:**
- A text layout engine with tracking.
- Per-line and per-word reveal: mask wipe, step-in, highlight bar.
- Lower-third and label-chip templates with an accent swatch.
- Templates are parameters, not baked art.

### 2.7 Illustrated explainer diagrams

**Looks like:** flat, paper-cut or construction-paper shapes per video. For example, a paper clock turns into a pie chart for work hours. [V] Storybench

**Made with:** shape layers, sometimes real paper photographed (construction paper). [V] Storybench

**Motion:**
- Elements step at 12 fps over smooth eases.
- Card pops scale 1.10→1.0 in 0.3 s with back.out(1.6), stepped.
- Two-stage moves overlap by about 39% with an ease near cubic-bezier(.857, 0, .143, 1). [3P] hyperframes
- This matches the existing measured data, so there is nothing new here.

**Engine needs:** SDF and bezier shapes, path morph (clock to pie), paper fill textures per shape, cast "paper" drop shadows, and stepped time.

### 2.8 Transitions

**Match cut by motion**
- Animate a parent null of scene A out (for example, up) and scene B in on the same vector, then hard-cut mid-motion. [3P] Mapal
- The general rule: same axis, same direction, matched speed. [3P] hyperframes motion-continuity

**Tracking transition**
- A 3D camera moves across the edit point with an eased curve that peaks at the cut.
- An adjustment blur rises into the cut and falls off over a few frames, shorter than the camera move. [3P] PremiumBeat

**Contrast hard cut with a sound effect** (2.2).

**Cuts are mostly hard.** Fades between scenes are uncommon. [I, consistent with all tutorials]

**Engine needs:**
- Cut-on-motion, with a velocity-continuity check across scenes.
- A transition blur pulse locked to the cut.
- An "align ease peak to cut" option on camera keyframes.

### 2.9 Screen capture of websites and apps (most relevant to Lahza)

**Looks like:** articles, websites and spreadsheets "rarely feel like static screenshots". They are treated as a physical monitor, and the camera glides smoothly across the page to steer the eye. One tutor says it feels like "participating in the journalism". [3P] Moran. Vox also shows raw spreadsheets full-frame. [V] falsetto frame

**Screen treatment** [3P] Moran:
1. An adjustment layer with Venetian Blinds: completion 5%, rotated 90° so the lines run horizontal, width 8, feather 1. Duplicate at 180° for a pixel grid.
2. Solid Composite black behind it.
3. Quick Chromatic Aberration about 1.2.
4. Lumetri vignette.
5. Exposure flicker `wiggle(24, 0.08)`.

**Camera:** smooth eased pans and pushes. The camera stays smooth even when the elements on it step at 12 fps. [3P] hyperframes

**Headlines:** important headlines are often **re-typeset** rather than shown as screenshots (2.2). [V]

**Engine needs (Lahza has a natural lead here):**
- A recorded-screen media layer.
- A "monitor look" preset: scanline or pixel-grid, RGB split, vignette, flicker.
- A smooth camera path over the recording, auto-generated from Lahza's cursor and click data or from DOM or OCR text positions.
- Highlight and underline tied to text found by OCR in the recording.
- A dim-the-rest contrast callout over a UI region.
- A "zoom into this paragraph" that targets a text box.

### 2.10 Sound design

**Levels** [3P] Moran:
- VO about −6 dB, music about −20 dB, sound effects −10 to −20 dB.
- Duck the music in the voice frequencies ("make room for vocals", about −20 dB in that band).

**Palette** [3P] Moran:
- Tactile: paper movement, page turn, book slide, pencil, pen, marker.
- Mechanical: stopwatch tick, toggle, rotary dial, mechanical camera shutter, lever, circuit breaker.
- Whooshes: soft, low, fabric.
- Tech: keyboard, mouse click, trackpad, subtle UI click, electrical hum.
- Analog ambience: vinyl crackle, tape hiss, projector hum, room tone.

**Rules** [3P] Moran:
- Give sound only to the most important motion in a busy frame (hierarchy and restraint).
- Stack a texture sound with a motion sound (whoosh plus paper).
- Vary repeats by small pitch and length stretches.
- Sound effects may sit a few frames off the motion; this suits the stop-motion feel.
- J-cut: the marker sound starts slightly before its visual.

**Music:** documentary-style library tracks, MusicBed and similar, appear in tutorials [3P]. Licensing for music analysis is a real cost: Film Independent notes one rap deconstruction would have cost about $20k to license. [V]

**Engine needs:**
- An audio timeline with a sound-effect library indexed by tag.
- Automatic sound-effect placement from animation events: draw-on → marker or pencil; push → whoosh; hard callout → breaker click; card slide → paper or book.
- Per-event offset, and pitch or rate variation.
- Ducking in the voice frequency band.
- LUFS and level targets.

---

## 3. Pacing

### Sourced

- **Visuals follow the words.** Caswell: the narration "describes what's on screen". [V] In Earworm the sync is beat-exact; per TNW, one word off the beat hurts the whole piece. [V]
- **Length and retention:**
  - Atlas films run about 10 min [V Ellis]; Darkroom 5–11 min [V]; Explained about 18 min [3P].
  - Vox optimises for retention, not views; Sendaydiego says "I think we average four minutes". [V]
- **Story arc:** Ellis prefers "a story arc, with a beginning, middle, and end", with the story first and an entertaining script to keep viewers. [V]
- **Hold rules**, taken from an agent skill calibrated on a Vox film [3P] hyperframes:
  - No visually still stretch longer than 3 s.
  - No 3 s window without an authored event: a pop, swap, highlight, zoom or promoted caption.
  - A deliberate 0.3–0.75 s stillness before a reveal.
  - The visual may lead slow VO by about 1–1.5 s, and reactions may lead their cause by about 100 ms, never trail it.
  - Plate pushes of 3–4% per beat count as camera intent, not as idle motion.
  - About 150 words of VO per minute.
- **Highlighter speed** matches reading speed; there is no fast middle. [3P] Moran

### Inferred [I]

- A typical visual unit lasts one VO clause, about 2–4 s. The camera usually keeps moving within a shot (push or pan) while elements step in, so a cut every 4–8 s still feels busy.
- Emphasis is shown by a markup gesture (highlight, circle, contrast dim, focal blur) landing **on** the stressed word. It is not shown with bigger text.
- Music is a bed, with occasional swells at chapter cards. Sound effects carry the micro-rhythm.

**What an agent should set:** word-level timestamps are the clock. Every markup event is anchored to a word with a lead offset of −0.1 s by default, and up to −1.5 s for anchor labels.

---

## 4. Production pipeline [V unless marked]

1. **Pitch.** Three ideas, each with a headline and a short synopsis; a story editor greenlights one. Producers own their own ideas, and one person carries a video from pitch to publish. (Caswell / Film Independent; Nieman Lab)
2. **Reporting.** Expert interviews, and sometimes an original dataset. Caswell logged lyric databases in a spreadsheet; the falsetto film was made with The Pudding. Archive services are licensed. (Film Independent; Ellis; video description)
3. **Script.** Takes 4–5 days, up to about 1.5 weeks. A story editor makes sure it is visual. Caswell: the editor slashes "my really bad script". Atlas reverses the order: visuals first, then writing. (Film Independent; TNW; Ellis)
4. **VO.** The producer records and "stitches" it. Early on, Joss Fong wrote scripts and had writers narrate them. (Film Independent; The Long Story)
5. **Design and animatic.** No Vox source describes a formal animatic step. The art director joins early in scripting and decides the per-video look. [V Sendaydiego]. One reconstruction suggests a still per beat as a contact sheet before animation [3P].
6. **Animation and edit.** About 1–1.5 weeks of animation and post. (Film Independent)
7. **Total time.** 2–3 weeks to publish (Sendaydiego), about 3 weeks (Caswell), 3.5 weeks for Earworm, 5 weeks for an Atlas film. One to two videos a week across the team. Maps get an accuracy check by other staff.

**Roles:**
- Producer: reporter, writer, narrator and animator in one. Vox says hires "cover all production skills rather than specialize".
- Story editor.
- Associate producer, occasionally.
- Art director (Joey Sendaydiego).
- Motion designers.
- Executive producer.
- The founding team was about 11 people (Nieman Lab). Fong built an internal After Effects training curriculum (The Long Story).

**Tools:**
- After Effects at the core; Premiere, Photoshop and Illustrator. Figma appears in a motion-design-lead job ad.
- Physical craft materials.
- Tutorial-reported plugins [3P]: Quick Chromatic Aberration, Deep Glow, Motion by Mt. Mograph (easing), GEOlayers, Google Earth Studio, Soundly.

---

## 5. Engine requirements specific to Vox style

### Must

1. **Look stack ("analog pass")** applied per comp or group.
   - Parameters: `ground_color` (off-white, warmth 0–10%), `texture[]` (each with asset, blend, opacity, scale, flip-book Hz), `vignette`, `rgb_split` (0–2 px, edge-weighted by radial mask), `grain` (size, amount, seed), `exposure_flicker` (Hz, amplitude ±0.05), `edge_blur` (radius, mask feather), `posterize_fps` (8, 10 or 12, with 24 or 25 output).
   - Elements and camera need separate clocks: elements stepped, camera smooth.
2. **Hand-drawn markup primitives:** `highlight(text_range | rect, color, width, ease, dur)`, `underline`, `ellipse`, `arrow(label)`.
   - Shared parameters: roughness (two-scale displacement: size and amount), boil Hz (default 4), seed, blend (multiply), draw-on and draw-off.
   - Each must target **text boxes** from the engine's text layout, from PDF text, or from OCR word boxes on images and screen recordings.
3. **Callouts:** `contrast_dim(region, feather, lift)` with a hard cut, and `focus_blur(center path, radius, falloff, max_blur)`.
4. **Paper object treatment:** `torn_border(width, color≈#F7F4EE, rough_coarse, rough_fine)`, `paper_card` (rotation ±1.5–3°, soft shadow, radius), black-and-white or tint grade, and a per-layer texture clipped to alpha.
5. **Chart component**, driven by data (CSV or JSON): line, scatter, bar, area, timeline.
   - Linear or log scales, ticks and formatting.
   - Build order (by x, by series, or manual), dim-others emphasis, value counters.
   - Annotations with leader lines, title and source-line slots.
   - A ruler-first timeline mode.
6. **Document and headline layer.**
   - PDF or image page import, deskew, and a camera push to a text box.
   - A **re-typeset headline card** template with outlet, title (highlighted line ranges), byline and date.
   - A pull-quote template.
7. **Screen-recording "monitor look"** (scanlines or pixel grid, RGB split, flicker, vignette), plus camera paths over recordings generated from Lahza cursor and click data or OCR targets.
8. **Word-timestamp timeline.** VO transcription (whisper) gives word times, and every event can anchor to a word with a lead offset.
9. **Hard-cut-mid-motion transitions** with "ease peak at cut", a blur pulse at the cut, and a check on velocity continuity.
10. **Templates:** lower third (name caps, role line, accent square, optional bar), label chip, source credit (tiny, bottom corner), section or chapter card.

### Should

1. **Map module:**
   - GeoJSON layers in Natural Earth style, projection, and region fill or outline draw-on.
   - Camera fit-to-bounds, pins with ping (radio-wave rings), geo-anchored labels.
   - Optional tile fetch, or import of a Google Earth Studio sequence plus 3D track data.
2. **2.5D photo parallax** with a subject cut-out layer and an inpainted plate, a slow push, and plate blur and darken.
3. **Zoom-isolation** (background dropout to a cut-out) and **inverse zoom arrival**, built on the nested zoom-through already planned.
4. **Footage homogenisation** preset over mixed clips: black-and-white or tint, grain, edge blur, flicker.
5. **Sound-effect auto-placement** from motion events, with a tagged library, offsets, pitch variation, voice-band ducking, and level targets (VO −6, music −20, sound effects −10 to −20).
6. **Art-direction skins:** swap the palette, fonts and texture set per video (protest poster, construction paper, Earworm dark grid) while keeping the grammar.

### Nice

1. Earworm music tools: waveform and spectrogram layers, beat-grid markers, notation and piano-roll renderer, sync to the music track's onsets (reusing the planned audio analysis).
2. Period-film looks: gate weave of 0.5 px at 8 Hz, ±0.04 stop flicker, 16 mm grain, and a 10 fps posterize. [3P] hyperframes, No Film School
3. Paper-craft diagram shapes with real paper fill textures and cast shadows.
4. A CRT emulation preset.

### Agent-facing parameters (summary)

```
style: vox | vox.atlas | vox.earworm | vox.darkroom
stepped_fps: 12
ground: #FBFAF8
ink: #1A1A1A
highlight: #FFE619 (or brand #FFF200 / blue / red-ink)
markup.roughness: 2..4
markup.boil_hz: 4
markup.draw_dur: 0.4..1.5 s
callout.kind: highlight | ellipse | underline | arrow | contrast | focus
callout.anchor_word: <id>
callout.lead_s: -0.1..-1.5
camera.push_pct_per_beat: 3..4
camera.zoom_iso.scale: 4.5..7
camera.zoom_iso.ease: power4.in
camera.zoom_iso.dur: 1.2 s
look.texture_set
look.rgb_split
look.grain
look.flicker
chart.type
chart.data
chart.build_order
chart.emphasis
chart.source
lower_third.name
lower_third.role
lower_third.accent
sfx.auto: on
```

---

## 6. Data and asset requirements

### Automatic sources an agent can fetch

| Need | Source | Notes |
|---|---|---|
| Chart data | Our World in Data (CSV/API) | Credited on screen in a Vox chart [V]. CC BY, so keep the source line. |
| Chart data | FRED, World Bank, UN, census APIs | [I] |
| Chart data | The Pudding open datasets | Vox collaborated with them [V]. |
| Archival photos, newspapers | Library of Congress | Collections such as the Hine NCLC photos used in Darkroom [V]; Chronicling America newspapers [3P Mapal]. |
| Archival photos, newspapers | Wikimedia Commons API, Internet Archive book images (Flickr) | Per-asset licence check needed; LOC blocks direct jpg, so use IIIF. [3P hyperframes] |
| Scanned maps | Harvard Library scanned maps | [3P Mapal] |
| Scanned maps | David Rumsey (CC BY-NC, so check), Natural Earth vectors (public domain) | [I] |
| Map data | GeoJSON from Natural Earth or OSM extracts | OSM is ODbL, needs attribution. [I] |
| Map data | Google Earth Studio | Renders need on-screen attribution, and it requires a Google account. A human runs it. [I] |
| Textures | texturelabs.org | Free, commercial use allowed [3P]. Or a procedurally generated paper, halftone and grain library in-engine. [I] |
| Fonts | Open stand-ins | Condensed caps: Archivo Narrow, Archivo Black, Bebas Neue, Oswald, Anton. Serif: Source Serif, Playfair Display. Sans: Inter or Work Sans. [I] Real Vox faces Balto (Type Supply) and Harriet (Okay Type) are commercial licences and should not be used to imitate Vox. |
| Sound effects | freesound.org (licences vary), a bundled CC0 set | [3P] / [I] |
| VO timing | whisper.cpp word timestamps | Already a Lahza dependency. |

### Needs a human

- Licensed news footage and photo agencies (Getty, AP, Reuters). Vox uses "lots of archive services". [V Ellis]
- Fair-use judgements on clips and screenshots of other outlets' articles.
- Music licensing, especially analysis of commercial songs: about $20k for one Earworm episode. [V Film Independent]
- Expert interviews and filmed footage.
- Map border and disputed-territory accuracy review, which Vox does as a separate check. [V]
- Commercial fonts.
- Final editorial fact-check.

### Guardrail [I]

Agents should reproduce the *grammar* (paper, markup, stepped motion, evidence-first), not Vox branding. That means no Vox logo, no Vox watermark, and no claim of Vox authorship. The open hyperframes skill states the same rule.

---

## Sources

**Vox and Vox staff**
- Storybench, Joey Sendaydiego interview: https://www.storybench.org/how-vox-uses-animation-to-make-complicated-topics-digestible-for-everyone/ (same article: https://www.storybench.org/?p=21229)
- Storybench, Sam Ellis / Vox Atlas: https://www.storybench.org/?p=14818
- Film Independent, Estelle Caswell interview: https://www.filmindependent.org/blog/explainer-an-interview-with-vox-pop-video-essayist-estelle-caswell/
- TNW 2019, Caswell interview transcript: https://www.youtube.com/watch?v=aPwi5rNKpMk and article https://thenextweb.com/news/earworm-youtube-music-dancing-about-architecture
- Nieman Lab on the Vox video team: https://www.niemanlab.org/?p=124626
- The Long Story, Vox alumni: https://thelongstory.substack.com/p/why-the-best-journalists-on-youtube
- Darkroom episode description and frames: https://www.youtube.com/watch?v=ddiOJLuu2mo
- Darkroom series data: https://thetvdb.com/series/vox-darkroom
- Earworm falsetto episode, description and frames: https://www.youtube.com/watch?v=qJT2h5uGAC0
- Microchip film (Atlas era), frames: https://www.youtube.com/watch?v=Uh4QGey2zTk
- vox.com CSS and logo SVG: Balto, Harriet, Roboto Mono, #FFF200 (fetched https://www.vox.com/)
- Video Lab membership: https://niemanlab.org/2019/02/vox-com-tries-a-membership-program-with-a-twist-its-focused-on-video-and-entirely-on-youtube

**Third-party reconstructions [3P]**
- PremiumBeat: https://www.premiumbeat.com/blog/replicating-vox-motion-graphic/
- Mapal, "How To Make Vox Style Videos": https://www.youtube.com/watch?v=F7A503llNBA
- Chris Moran:
  - Analog look: https://www.youtube.com/watch?v=sACZlG7z35Q
  - Callouts: https://www.youtube.com/watch?v=CqHb6bVoxgI
  - Sound: https://www.youtube.com/watch?v=UgVgzSZVu-8
- Flat Pack FX:
  - Torn border: https://www.youtube.com/watch?v=NunIxSHQSZg
  - Timelines: https://www.youtube.com/watch?v=BYEvQoG_fm8
- No Film School, maps with Google Earth Studio: https://nofilmschool.com/how-create-vox-style-map-animations-after-effects
- hyperframes community skill "vox-explainer" (measured from Vox's Cooper Black film; palette, timing, gates): https://github.com/heygen-com/hyperframes-community-skills/tree/main/skills/vox-explainer
- Jasper Pictures on Explained: https://jasperpictures.com.au/blog/vox-explained-a-case-study/
