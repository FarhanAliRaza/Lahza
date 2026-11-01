# Johnny Harris–style documentary motion design: study for the Lahza engine and agent

Date: 2026-11-02. Method: WebSearch and WebFetch only. I could not frame-step any video. YouTube rate-limited and bot-checked direct page fetches, and many tutorial hosts (aescripts.com, motionarray, beehiiv, podwise, medium) sit behind Cloudflare 403s, so their content is known only from search-engine extracts.

**Evidence tags.** Every claim carries one of these:
- **[S]** sourced, with the URL.
- **[S2]** secondary or third-party source (tutorial, vendor, AI summary). Treat it as indicative.
- **[I]** inferred: my own reading of the style and general motion-design practice, not verified here. Every **[I]** number is a starting default for a later frame study, not a measurement.

---

## 1. Style summary

1. **The map is the main character.** His maps are custom and often 3D, used to anchor geopolitical and historical stories. They are built in After Effects with the GEOlayers plugin, drawing on Mapbox styles and Google Earth Studio [S] https://www.premiumbeat.com/blog/making-maps-for-johnny-harris/ ; [S] https://vmacke.com/johnny-harris ; [S2] https://outlierkit.com/channel/johnnyharris.
2. **A house map style is used like a brand.** Since mid-2022 a dedicated art direction (by Valentin Macke) has supplied map styles "built from the ground up" in Mapbox and GEOlayers. It also supplied rigs to "animate camera moves, sketch borders and areas, and tag places", plus Notion style guides covering typography, maps, colour, animated elements, rigs and titles [S] https://vmacke.com/johnny-harris.
3. **Visual anchor first, context second.** He opens a beat on a striking image ("look at this thing!") and only then explains it. He keeps returning to the same anchors [S2] https://chengweihu.com/io/visual-before-context/. He rarely says a line without a matching visual [S2] RNDR newsletter, via search extract.
4. **Detective-story structure.** A provocative question comes first. Then clues: maps, documents and footage, often posed as spoken questions ("why here?"). Then the payoff [S2] https://blog.editorduel.com/blog/johnny-harris-detective-story-formula-explainer-videos.
5. **Texture everywhere.** Paper grain, film dust, burns, light leaks, noise and grain overlay most graphics and photos [S2] Motion Array tutorial (search extract). Maps look printed, folded or scanned: muted base colours, one strong highlight colour, soft vignette [S2] https://motionvid.ai/blog/johnny-harris-style-map-animation.
6. **Documents as evidence.** Scanned memos, newspapers and papers are zoomed into a specific line, then highlighted or circled [S2] editorduel; [S2] gist.ly CapCut breakdown https://gist.ly/youtube-summarizer/master-johnny-harris-style-editing-in-capcut.
7. **Cinematic presenter footage mixed with scrappy footage.** He walks and talks on location (Borders), with drone shots, and also cuts in grainy phone clips of himself at work [S] https://thelongstory.substack.com/p/why-the-best-journalists-on-youtube ; [S] https://www.niemanlab.org/?p=162382. His own description of the style is "motion graphics with cinematic videography" [S] https://www.johnnyharris.ch/about.
8. **Long form, built on a craft pipeline.** Recent uploads run 30–50 min, and some compilations run 2–3 h (channel listing fetched 2026-11-02, e.g. "India vs China, Explained" 31:38, "How billionaires buy American elections" 44:18). Each video takes about a four-month cycle [S2] podcast summary (see section 4). Newpress won the News Emmy for Outstanding Graphic Design in 2026 [S] https://en.wikipedia.org/wiki/Johnny_Harris_(journalist).

---

## 2. Technique catalogue

Each entry gives what it looks like, how it is made, typical timing, and the engine primitive needed.

### 2.1 Map animation

**a) Globe to country push-in, eased "fly-to".**
- **Looks like:** A 3D globe or satellite view pushes into the region the narrator names. The camera "rarely snaps" [S2] motionvid. Real satellite or terrain shots come from Google Earth Studio (GES), and stylised shots from GEOlayers.
- **How it is made:** A producer named Jack does much of the channel's map animation. GES is used to place track points, export the 3D camera data to AE, and run a script that builds a 3D camera plus nulls with attached text [S] premiumbeat.
- **GES limits:**
  - Output is image sequences, up to 4096×2304 [S2] (search extract of GES rendering docs).
  - Google and third-party imagery attribution must stay on screen for the whole shot [S] https://www.google.com/earth/studio/faq/.
  - Google offers no licence for commercial use [S] same FAQ.
- **Timing [I]:**
  - Push-in: 2–4 s with ease-in-out, landing on the stressed noun of the sentence.
  - Hold: 1–2 s after landing, so labels and borders can resolve.
  - Long continent-to-city moves: 4–6 s, often with a mid-flight zoom-out (the "van Wijk" zoom–pan arc).
- **Engine primitive:** A geographic camera `GeoCamera {lon, lat, zoom|altitude, bearing, pitch}`.
  - Interpolated with the van Wijk–Nuij optimal zoom-and-pan, as in Mapbox `flyTo`, not with linear lon/lat.
  - Fed by eased keyframes.
  - Renders a globe at low zoom and a projected plane at high zoom, cross-fading between the two.

**b) Borders drawing on, country highlight.**
- **Looks like:** An outline is sketched on, then the area fills with the single accent colour or a pattern.
- **How it is made:**
  - The art-direction rigs "sketch borders and areas" [S] vmacke.
  - The National-Geographic-style glowing border uses a GEOlayers country shape with an Inner Glow layer style and the fill removed [S2] https://aescripts.com/learn/make-maps-like-national-geographic-using-geolayers-in-after-effects/ (search extract).
  - The WW2 occupation reveal duplicates the map, gives the lower copy patterned (hatch) fills, and reveals it through an animated track matte that is scaled to read as a zoom-out [S2] https://community.adobe.com/t5/after-effects-discussions/how-does-johnny-harris-do-this-mask-reveal-animation-in-after-effects/td-p/15191636.
- **Timing [I]:** Stroke draw-on 0.6–1.5 s; fill fade 0.3–0.5 s after the stroke closes.
- **Engine primitive:** A vector polygon layer from GeoJSON or TopoJSON, projected through the camera, with:
  - Stroke trim (start/end), plus a dash pattern and an optional hand-drawn wobble (noise displacement on the stroke offset).
  - Inner glow and outer glow.
  - Fill as solid, hatch pattern or texture.
  - A matte or reveal mask shaped as a wipe, a radial, or a "spreading front".

**c) Routes, arrows, frontlines and invasions.**
- **Looks like:** Dashed shipping lanes, invasion arrows, troop paths, symbols moving along a path.
- **How it is made:**
  - Route lines "draw on at roughly the pace of the narration" [S2] motionvid.
  - Boone's course, endorsed by Harris, has lessons on "3 Ways to Add Arrows", "Put Symbols on a Path", "Visualize an Invasion with Shape Layers / Solid Layers", military symbols, flags and "How to Visualize Disputed Areas" [S] https://boonelovesvideo.teachable.com/p/geolayers-3-war.
  - GPX/KML/GeoJSON route import is common in this tool space [S2] motionvid (Mult.dev).
- **Timing [I]:** Route length is tied to a narration span: the agent sets the start word and the end word. Arrowheads arrive with 0.15–0.25 s of overshoot.
- **Engine primitive:**
  - A `GeoPath` covering great-circle or rhumb lines and arbitrary polylines.
  - Trim-path animation, a dash offset that can march, and an arrowhead that follows the tangent.
  - Symbol-on-path sampling at arc length.
  - A "front" polygon morph between keyframed shapes, for invasions.

**d) Labels, pins and tags.**
- **Looks like:** Place names appear exactly as the narrator says them [S2] motionvid. Tags look like small paper slips pinned to the map with drop shadows [S2] motionvid. Labels track the camera in 3D.
- **How it is made:**
  - Rigs "tag places according to the style" [S] vmacke.
  - Boone's course covers "Pin and Parent Elements", "Custom Label Templates", "Animate Labels", "Label Locations Quickly" and "Battle Map Fonts" [S] teachable.
  - A 3D blinking map marker over a monument [S] premiumbeat.
- **Timing [I]:** Label in 0.25–0.4 s with an ease-out pop or a typewriter effect, synced to word onset ±2 frames. Out when the beat ends.
- **Engine primitive:**
  - Billboarded text or label nodes anchored at lon/lat and screen-projected each frame, with a leader line and pin.
  - Optional collision avoidance.
  - Style templates (font, case, tracking, paper-tag background).
  - Triggers keyed to word timestamps.

**e) Paper-styled vs. satellite vs. terrain maps.**
- **What exists:**
  - **Stylised vector basemaps** from Mapbox, styled to the house palette [S] vmacke.
  - **GES satellite and 3D-city flyovers** [S] premiumbeat.
  - **Relief/terrain maps** ("Vox Map - Vintage: relief map with badges and route lines") [S2] motionvid.
  - **Old historical maps**, georeferenced: Boone's course teaches "Basic/Advanced Georeferencing", "Georeferencing with Graticules" and "Digitizing" [S] teachable. Vox's Sam Ellis traced maps from old books for Vox Atlas [S] https://www.storybench.org/?p=14818.
- **Engine primitives:**
  - A raster tile layer (XYZ/WMTS) with LOD selection.
  - A DEM-based hillshade and an extruded terrain mesh (3D extruded terrain needs a heightmap mesh with vertical exaggeration and lighting).
  - Georeferenced image overlays: a scanned historical map warped by control points or graticule affine/TPS onto lon/lat.
  - A style system that recolours vector layers from a palette.

**f) Texture that moves with the map.** The texture stays still while the map zooms, which a forum user reported as a pitfall [S2] aescripts forum extract. Boone's fix is to "attach the texture to the map" [S2] https://aescripts.com/learn/how-to-add-texture-to-maps-in-geolayers/ (extract).
- **Engine primitive:** Two texture spaces:
  - **Map-locked** paper texture, in world UVs, that scales with zoom and tiles or mip-blends across zoom levels.
  - **Screen-locked** grain and dust.
  - Both need to be selectable per layer.

### 2.2 Archival photo and document treatment

**a) Document zoom and highlight.**
- **Looks like:** A scanned page, slightly angled in 3D. The camera pushes to one line, and a yellow highlighter sweeps across it.
- **How it is made:**
  - The highlighter is a solid with a Stroke effect, Paint Style "On Transparent", in Multiply blend [S2] https://aejuice.com/blog/how-to-highlight-text-in-after-effects (extract).
  - Turbulent Displace gives the marker a natural, uneven edge, per Mapal's "How To Make Vox Style Videos" at about 15:20 [S2] (search extract of an avp.stackexchange mirror).
  - Harris "zooms in on a specific line and asks what it means" [S2] editorduel.
- **Timing [I]:** Push-in 1–2 s, then highlight 0.4–0.8 s per line, left to right, with ease-out. The highlight starts on the word being quoted.
- **Engine primitive:**
  - A `DocumentPlane`: an image or PDF page as a 3D card with paper shading, shadow and slight curl.
  - A camera "focus on rect" move.
  - A `Highlighter` stroke shape with a multiply blend, noise-displaced edges, ends that vary in width, and colour presets.
  - For agents, word- and line-level bounding boxes from OCR, so they can say "highlight the phrase X" instead of giving pixel rects.

**b) Circling, underlining, red-pen marks and arrows.** [I] These are hand-drawn ellipses and underlines in red or marker colours that draw on with trim paths and a slight overshoot, so the ellipse does not close perfectly.
- **Engine primitive:** A procedural "hand-drawn" stroke built from a shape plus per-vertex noise, a pressure-width profile, an overshoot parameter and a draw-on time. It must work in both document space and map space.

**c) Redactions.** [I] Black bars over text, sometimes wiping on, used in declassified-document beats.
- **Engine primitive:** A rect shape with a wipe-on animation, anchored to OCR boxes.

**d) Newspaper clippings and memos.**
- **How it is made:**
  - Ripped newspaper overlays come from paper-texture PNGs plus masks, with sepia, grain, and a "typewriter" text effect. Fonts in one tutorial are Bebas Neue and Times New Roman [S2] https://indianfunmedia.com/master-the-johnny-harris-style-newspaper-animation-in-after-effects/.
  - The CapCut recreation [S2] gist.ly does this:
    - memos are inverted or cut out with a white stroke border;
    - multi-page memos are duplicated per page;
    - photos are desaturated to B&W with a "90s" look;
    - film-roll overlays are composited in Screen blend at transitions;
    - contrast is set to −18 on an adjustment layer;
    - text uses a typewriter font with a 1–3 s typewriter in-animation;
    - zoom keyframes use ease curves.
- **Engine primitive:**
  - Torn-edge masks, procedural or from an alpha library.
  - Cut-out with an outline stroke (dilate the alpha, then fill white).
  - Drop shadow.
  - B&W, sepia and duotone grading.
  - Halftone [I].

**e) 2.5D photo parallax and cut-outs.** [S2] Standard practice: isolate the subject in Photoshop, fill the background behind it, and move the layers on a 3D camera. Moving parts of the image separately distinguishes this from Ken Burns, which moves the whole picture (https://blog.motionarray.com/learn/after-effects/animate-flat-2d-images-after-effects/ extract). I found no source tying a specific parallax recipe to Harris.
- **Engine primitive:**
  - A layered photo: foreground, mid and background planes at Z depths, with background inpainting.
  - A slow dolly or push of 3–8 % scale over 3–6 s [I].
  - For agents: automatic subject segmentation plus monocular depth to make the planes, and inpainting for the hole, if Lahza adds ML.

### 2.3 Collage, paper and tape

- **Looks like:** Photos, documents and small tags pinned or taped onto the map or a paper background, with drop shadows [S2] motionvid. A "scrapbook" look that pays homage to manual, early-filmmaking inputs, and physical props are reported in his work [S2] RNDR newsletter (search extract).
- **Commercial packs:** Paper, scribble and tape packs for AE and Premiere exist (e.g. 19 AE projects plus .mogrts) [S2] https://gfxreplica.gumroad.com/l/yzjoq.
- **Engine primitive:**
  - A collage stack: cards with rotation jitter, tape strips (semi-transparent textured quads), pins, contact shadows, and slight paper bend.
  - "Slam" or "drop" entrance presets with a short settle.
  - Stepped time (2s or 3s) as an optional collage feel. This is already measured elsewhere.

### 2.4 Grain and film looks

- **How it is made:**
  - **Premiere:** texture over footage with a changed blend mode, nested, with blur to soften the edges [S2] Motion Array (extract).
  - **Burn-through paper:** Fractal Noise → Colorama → Glow [S2] https://community.adobe.com/t5/after-effects-discussions/tutorial-on-burning-through-paper-effect/m-p/11135508.
  - **Light leaks and film burns:** Screen or Add blend overlays [S2].
  - **Grade:** cinematic and slightly desaturated [S2] editorduel.
- **Engine primitive:** A finishing stack:
  - Animated grain, luminance-weighted, temporally random per frame.
  - Dust and scratch sprites.
  - Gate weave of ±0.5 px [I].
  - Vignette.
  - Overlay clips for light leaks and burns in Screen/Add blend.
  - LUT grade and halation [I].
  - Defaults [I]: grain RMS about 2–4 % luma at 1080p; vignette −10 to −20 % at the corners.

### 2.5 On-camera presenter, B-roll and "evidence" segments

- **What is sourced:**
  - Walk-and-talk on location; a camera style built on years of handling the camera and looking for images that explain something [S2] documentarytube.
  - Grainy phone clips of himself to show frustration and process [S] thelongstory.
  - Split screens contrasting his approach with "boring" news anchors [S2] editorduel.
  - Vox's founding rule was "no desks" [S] https://www.niemanlab.org/?p=124626. Harris's channel does use a studio, but the cable-news desk look is avoided [I].
- **Physical maps or whiteboards on camera:** not confirmed by any source I could reach (searches returned nothing) → [I] only. If an agent wants this look, it would have to be a composited "map on table" shot: a top-down plane plus a hand-drawn stroke.
- **Engine primitive:**
  - Picture-in-picture and split-screen layouts.
  - A presenter clip with a light grade match.
  - Rotoscoping or background removal to put the presenter over a map. A recording studio like Lahza can already supply the webcam or presenter track.

### 2.6 Kinetic captions and quotes

- **Looks like:** Typewriter title cards [S2] gist.ly, and text overlays stamping key phrases ("question", "report") [S2] editorduel. Elsewhere this vocabulary is described as "bold typography stamping onto archival footage … a yellow highlighter sweeps across a document" [S2] https://easy-peasy.ai/blog/how-to-make-vox-style-videos-with-ai.
- **Fonts:** The house typography lives in private Notion guides [S] vmacke, so it is not public. Tutorials use typewriter faces, Bebas Neue and Times New Roman [S2]. Treat any specific face as [I].
- **Engine primitive:** Per-glyph text (exists already), typewriter reveal with a cursor, "stamp" scale-down with a hard settle, quote cards with attribution, and word-synced emphasis.

### 2.7 Data and chart moments

- **Looks like:** Simple, functional graphics: formulas appearing, flow loops [S2] editorduel. Vox's data-viz lineage [S] thelongstory. Plotting geolocated dataset points on maps (Vox Atlas) [S] storybench.
- **Engine primitive:** Line, bar and area charts with draw-on, counter numbers, proportional symbols or dot-density on maps, and choropleth from a CSV joined on ISO codes.

### 2.8 Transitions

- **Sourced:**
  - Match cuts across locations; jump cuts to condense speech [S2] editorduel.
  - Luma fades and matte transitions; push-ins paired with sound cues and VO pauses [S2] indianfunmedia.
  - Film-roll overlays in Screen blend at transitions [S2] gist.ly.
  - Map shapes scaled to suggest a zoom [S2] adobe forum.
- **Inferred [I]:**
  - A zoom-through from map to photo to document: the camera flies into a pinned photo, which fills the frame and becomes the next shot. This is a nested-comp zoom-through, already measured elsewhere.
  - Whip-pans across the map.
- **Engine primitive:** Nested scene "portal" transitions, where the camera enters a child scene's card; luma/matte wipes; overlay transitions.

### 2.9 Sound design

- **Sourced:**
  - The channel employs a full-time music composer [S] thelongstory.
  - Sound design punctuates on-screen actions (doors, taps) [S2] editorduel.
  - Tutorials pair typewriter clicks, grain noise and subtle swells with text and push-ins [S2] indianfunmedia.
  - Sound design is one of his explicit process stages [S] Perell interview chapter list, https://socialcounts.org/youtube-video-live-view-count/zq4b96m1AvM.
- **Inferred [I]:** Whooshes on camera moves, paper rustle on collage drops, pen squeak on highlights and circles, stamp thuds, map "tick" pops on labels, and music that drops out before a reveal.
- **Engine primitive:**
  - An SFX track whose clips are automatically attached to visual events (label-in → tick, highlight → marker squeak, camera flight → whoosh scaled to its duration).
  - Music ducking under VO (sidechain).
  - Music "hit" markers.

---

## 3. Pacing

- **Unit of cutting: the sentence or phrase.** Almost every spoken claim has a visual [S2] RNDR extract. Place names appear as they are spoken [S2] motionvid, and routes draw on at narration pace [S2] motionvid.
- **Anchor → question → context.** He shows the visual, often asks "why here?" or "look at this", lets it sit, and then answers [S2] editorduel; chengweihu. This produces holds of 1–3 s on maps before the explanation lands [I].
- **Variable cut rate:**
  - Opening montages change shots "well under 1 second" [S2] editorduel; this is qualitative, not measured.
  - Cuts slow down when a new concept is introduced and speed up toward a revelation [S2] editorduel.
- **Shot length defaults [I]:** None was measured, because no frame access was possible.
  - Montage: 0.4–0.8 s.
  - Typical B-roll or graphic: 2–5 s.
  - Map establishing moves: 3–8 s.
  - Presenter A-roll: 5–20 s, broken by inserts.
  - Narration: about 150–170 wpm [I].
  - **Must verify with a frame study before baking these into presets.**
- **Emphasis words:**
  - Shown on screen as stamped text overlays [S2] editorduel.
  - Shown as document highlights on the quoted words [S2].
  - Shown as accent colour on the region named [S2] motionvid.
  - The "push-in on emphasis + sound cue + VO pause" pairing [S2] indianfunmedia.
- **Retention writing:** His interview has chapters titled "Every story is a promise", "Finding the paradoxes", "Writing for retention" and "The art of surprise" [S] Perell chapter list (socialcounts link above). Agents should plan beats as promise → paradox → reveal.
- **Music:** Original score from an in-house composer [S]. Pacing against music is not documented; [I] the score follows story sections rather than cut-to-beat.

---

## 4. Production pipeline

- **Cycle:** About four months per video: idea → research → script → filming → post → release [S2] podwise summary of David Perell's "How I Write" episode (12 Mar 2025, 1 h 26 m), via search extract. For comparison, a Vox Atlas video took five weeks [S] storybench.
- **Script:**
  - "Writing visual scripts": he writes the script and visually directs at the same time, with colour-coded notes for visual assets and fact-checking [S2] podwise extract.
  - The team's script template is "a large document with tables, color coding" to structure team communication [S2] https://craftbyzen.com/curation/stream/link/2025-03-07t181915788z/.
  - Interview chapters include "Writing visual scripts", "Is he writing or coding?", "Video-first writer" and "New art direction" [S] Perell chapter list.
- **Research:**
  - Deeply researched and fact-checked [S2] craftbyzen.
  - Researcher/Writer is a hiring role [S] https://www.johnnyharris.ch/join-the-team.
  - Borders used crowdsourced local networks: 7,000 suggestions; more than 700 local helpers in Hong Kong; 15-minute meet-up slots [S] https://www.niemanlab.org/?p=162382.
- **VO and animatic:** Not documented in anything I could reach. [I] Given the colour-coded visual script, the likely order is: script locked → scratch VO → rough cut with placeholders → map and graphics animation → final VO, mix and score.
- **Team:**
  - More than 20 people, including producers, animators, editors and a full-time composer. Iz Harris is executive producer and runs operations [S] thelongstory.
  - Map animation sits partly with a producer named Jack [S] premiumbeat.
  - Freelance map animators include Jason Boone [S] premiumbeat and Jeremy Shuback [S] teachable testimonial.
  - Art direction and the map-rig system came from Valentin Macke, from mid-2022 [S] vmacke.
  - Newpress, launched Feb 2026, adds operations, thumbnail designers and publishing [S2] craftbyzen; [S] Wikipedia.
  - Async briefing happens by voice notes, Loom and Marco Polo [S2] craftbyzen.
- **Tools:**
  - After Effects plus GEOlayers 3. Hiring posts ask for "After Effects & Geolayers" [S] https://x.com/johnnywharris/status/1569713094229692418 ; [S] https://www.johnnyharris.ch/editor-animator-application.
  - Mapbox Studio [S] vmacke.
  - Google Earth Studio [S] premiumbeat.
  - Illustrator and Photoshop, listed in the course software [S] teachable.
  - Editor: not confirmed. Premiere is commonly assumed [I].

---

## 5. Engine requirements, prioritised

### Must

1. **Geo scene graph and GeoCamera.**
   - Projections: orthographic globe, Web Mercator and equal-earth/Robinson for world views.
   - A fly-to with van Wijk zoom–pan.
   - Pitch and bearing.
   - Ease presets (existing).
   - **Agent parameters:** `from`/`to` as `{center:[lon,lat], zoom|bbox|feature_id, pitch, bearing}`, `duration`, `ease`, `path: auto|direct|arc`.
2. **Vector geo layers.** Load GeoJSON or TopoJSON (Natural Earth admin-0/1, coastlines, rivers, cities). Feature lookup by ISO code or name.
   - **Per feature:** `stroke {color, width, trim_start/end anim, dash, wobble_amp, wobble_freq}`, `fill {color|pattern:hatch(angle,spacing)|texture, opacity}`, `glow {inner|outer, radius, color}`, `reveal {type: wipe|radial|grow_from(point), duration}`.
3. **Geo paths and arrows.**
   - **Parameters:** `points|from/to`, `geodesic: true`, `trim anim`, `dash/march speed`, `arrowhead style`, `width taper`, `symbol_on_path`.
   - Front morph for invasions.
4. **Anchored labels, pins and tags.**
   - **Parameters:** `text`, `lonlat`, `style template` (paper tag, outline text, pin), `appear_at` (time or word index), `out_at`, `leader`.
   - Screen projection every frame, with collision nudging.
5. **Paper-map style system.** Palette tokens (land, water, borders, accent), a map-locked paper texture, vignette, and a muted-base-plus-one-accent rule as the default.
6. **DocumentPlane, highlighter and hand-drawn annotations.**
   - Page in 3D; focus on a rect or OCR phrase.
   - `highlight {phrase|rect, color, duration, edge_noise}`, `circle/underline/arrow {target, color, overshoot, duration}`, `redact {target}`.
7. **Finishing stack.** Grain, dust, vignette, light-leak and burn overlays in Screen/Add, LUT and desaturation. Each must be selectable as screen-locked or layer/map-locked.
8. **Word-timed timeline.** Ingest VO plus word timestamps (Lahza already has whisper.cpp). Every event can be keyed to `word_index` with an offset in frames. This is the core of the narration-driven pacing.
9. **Raster tile and imagery layer.** XYZ tiles for satellite or relief, with LOD and caching, so terrain/satellite push-ins do not depend on GES.

### Should

10. **DEM terrain.** Hillshade, plus an extruded mesh with exaggeration and sun angle, for 3D terrain shots.
11. **Georeferenced historical map overlay.** Control points to an affine or TPS warp; cross-fade from old map to modern map.
12. **Collage primitives.** Photo card with white border, tape, pin, drop shadow, rotation jitter, torn-edge mask; drop/slam entrance; optional stepped time.
13. **Photo treatments.** B&W, sepia and duotone, halftone, cut-out outline, slow push.
14. **2.5D parallax photo.** Manual layers first; segmentation, depth and inpainting later.
15. **Typewriter, stamp and quote-card text presets** on the existing per-glyph engine.
16. **SFX auto-attach.** Event type → SFX category, scaled to duration; music ducking under VO.
17. **Portal transition.** Camera flies into a card or pin that becomes the next scene (nested comps exist already).

### Nice

18. Choropleth and dot-density from CSV joined on ISO codes; chart draw-ons.
19. A 3D globe with atmosphere glow and a day/night terminator.
20. Presenter-over-map compositing (background removal on the webcam track).
21. Split-screen and PiP layouts.
22. Map-on-table pseudo-practical: a top-down paper plane with a hand-held camera shake and a marker stroke.

**Performance note for Lahza.** Map vector tessellation, projected labels and textures must be cached per zoom band. Only the camera transform should change per frame during a fly-to. This follows the project's "do not rebuild unchanged inputs" rule, with `blur_plane` for glows and the proxy path while scrubbing.

---

## 6. Data and asset requirements

| Asset | Source | Licence / terms | Agent can fetch automatically? |
|---|---|---|---|
| Country, admin-1, coast, river and city vectors | Natural Earth 1:10m/50m/110m | Public domain; attribution not required [S] https://www.naturalearthdata.com/about/terms-of-use/ | Yes. Bundle it. Note that disputed borders follow a single de-facto view; NE offers point-of-view variants on its site [I]. A human should check sensitive borders. |
| Detailed roads, buildings, local features | OpenStreetMap (Overpass, Geofabrik extracts) | ODbL. For video: when the map is a major element, credit a corner of the map plus the credits or description with the openstreetmap.org/copyright URL. It need not stay on screen during pans [S] https://osmfoundation.org/wiki/Licence/Attribution_Guidelines | Yes, with an attribution-overlay helper |
| Styled basemaps | Mapbox | Logo plus "© Mapbox © OpenStreetMap" required. The docs are silent on video; print export needs sales contact [S] https://docs.mapbox.com/help/dive-deeper/attribution/ | Needs the user's API key and ToS acceptance → human |
| Satellite / 3D city flyovers | Google Earth Studio | On-screen attribution for the whole shot; no commercial licence offered; Chrome plus an access request [S] GES FAQ | No. Manual export by a human. Avoid in an automated pipeline. |
| Open satellite imagery | NASA Blue Marble / Black Marble, Landsat; ESA Sentinel-2 (Copernicus) | NASA media are generally not copyrighted, acknowledge NASA, no logos or endorsement [S] https://www.nasa.gov/nasa-brand-center/images-and-media/ ; Copernicus data is free with attribution [I] | Yes, for global and regional mosaics. Pre-tile them. |
| Terrain DEM | SRTM / Copernicus DEM GLO-30, AWS Terrain Tiles | Generally open with attribution [I]. Verify the Copernicus DEM licence. | Yes |
| Historical maps | Library of Congress, David Rumsey, OldMapsOnline | LoC "Free to Use and Reuse" sets are described as rights-free [S] https://www.loc.gov/free-to-use/ ; other items are per-item [I] | Partly. Rights vary per item, so the agent must record the source and rights. Georeferencing needs human checks. |
| Archival photos and documents | LoC, US National Archives (US-government works), Wikimedia Commons, CIA FOIA reading room | US federal works are public domain [I]. Commons is per-file CC or PD [I]. | Yes, for PD/CC with a metadata log. News-agency photos (AP, Getty, Reuters) and broadcast clips need licences or a fair-use judgement → human. |
| Newspaper clippings | Chronicling America (LoC, pre-1963 US papers), other national archives | Mostly PD by age [I] | Yes, for PD papers. Modern articles → screenshot under fair use → human judgement. |
| Fonts | Google Fonts (OFL): typewriter faces (Special Elite, Courier Prime), condensed display (Bebas Neue, Oswald), serif (Libre Caslon, Source Serif) | OFL | Yes, bundle them. Do not imitate the channel's private house fonts. |
| Paper, tape, dust and burn textures | CC0 sources (ambientCG, Poly Haven, own scans) | CC0 | Yes. Bundle a curated pack. Better still, generate procedurally (noise-based fibre, crease and grain) so nothing needs licensing. |
| Music | His channel uses a full-time composer [S] | — | An agent needs a licensed or royalty-free library, or generated music → human sign-off. |
| SFX | CC0 (Freesound CC0 filter) or own recordings | Mixed | Partly. Curate a small CC0 set (whoosh, paper, pen, stamp, tick, typewriter). |
| VO | User recording or TTS | — | TTS automatic; final VO is usually a human. Word timestamps come from whisper.cpp (already in Lahza). |

**Needs a human:**
- Map API keys (Mapbox) and GES exports.
- Licensing for news photos, agency footage and broadcast clips, and any fair-use decisions.
- Border-dispute choices.
- Fact-checking of the script, given public accuracy criticism of the genre [S] Wikipedia section on Jarry and the Kyiv Independent.
- Final music.

**Not reachable in this session, worth a follow-up:**
- Johnny Harris's own "How to Create Animated Maps with AfterEffects" (2020).
- The aescripts "How Johnny Harris Makes Maps" (2022) talk.
- The Perell interview transcript.
- A frame-level study of 2–3 recent videos to replace every [I] timing above.
