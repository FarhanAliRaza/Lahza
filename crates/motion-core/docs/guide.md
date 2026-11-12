# Making a video with Lahza motion

## The loop

1. **Brief → visual script.** One row per spoken line or musical section:
   the words, the visual, the sync anchor (`word:canal`, `bar:5`, `hit:2`)
   and the style, plus a `BANNED:` line (see
   [A film, not a page](#a-film-not-a-page)). Get it approved before
   building.
2. **Audio first.** Put the music (and voiceover) in `assets/`, register it
   with `audio_analyze` (writes `beat`, `bar`, `half`, `sixteenth`,
   `triplet`, `onset.*`, `hit`, `stop`, `drop`, `music.start`,
   `music.end`). Narration comes in as a manifest (`narration_import`, see
   [Narration](#narration)).
   Read the result with `markers_query`; structure comes from the audio's
   shape (stops, drops, the biggest hit), not from BPM alone. Check
   `feel`: with `triplet` (a shuffle or 12/8 swing) cut and key on
   `triplet` markers, not `half`/`sixteenth`. A low `tempo_confidence`
   means another grouping of the same pulse (listed in `alternatives`,
   e.g. 100 BPM straight for 133 BPM triplets) fits the onsets as well:
   pick the one the reference cuts on. A `stop` labelled "the music cuts"
   is a hard stop with a tail ringing on; land the final title on it.
3. **Scene plan.** One scene per section; cut on bars or hits. Choose a
   sync mode per section:
   - `grid`: snap events to beat subdivisions (promos).
   - `hits`: snap to onsets, no grid.
   - `free`: only stop→hit moments bind; cut every 1.5–2 beats.
   - `words`: anchor to narration (explainers).
4. **Breakdown** (required, per shot, before building it): what each
   element is, how it is lit and moves, and how you will build it. See
   [Breakdown](#breakdown-before-building-a-shot) below, or the MCP prompt
   `breakdown`.
5. **Build** with `edit_apply`, one scene at a time, as the breakdown
   says. Batches are atomic and undoable (`history`). Geometry is built,
   never traced from a reference (see [No tracing](#no-tracing-as-content)).
6. **Review**: `check` (no errors, sync drift ≤ ±1 f, no `tracing`
   warnings, every craft warning — `first_frame_empty` and
   `empty_after_cut` included — fixed or `allow`ed: see
   [What check verifies](#what-check-verifies-and-measures)),
   `sync_check {flagged: true}` (every cut, entrance, key
   landing and jump against its nearest word, cue, beat or sound effect:
   `drift` means it misses the marker it was keyed to, `unsynced` means a
   hit with nothing to hit; key it to the marker instead of nudging
   seconds), `snapshot` with `sheet: true` at the scene's sync points, and
   `review` for the whole film (scenes, every cut, phone width). Look at the
   frames; fix; repeat.
7. **Whole-film review** with `review` before calling it done (see
   [Whole-film review](#whole-film-review)).
8. **Render** a draft (`quality: "draft"`), then the final.

## A film, not a page

A viewer sees the film once, at full speed, often on a phone. When nothing
is decided, generated video falls back on web defaults: a header and a
footer, labels on everything, feature lists, stat blocks, a paragraph of
small print. Decide instead.

- **One idea per shot, shown by the picture.** With the words covered,
  the frames should still tell the story. Text names what the picture
  cannot (a number, a name, a quote); it never describes what is on
  screen.
- **Few words, large.** Besides captions, at most about 12 words on
  screen at once, and no line smaller than 2.6% of the frame's short side
  (28 px on a 1080-wide Short). A line stays up long enough to read: 0.4 s
  for one or two words, otherwise about 0.75 × (0.5 s + 0.3 s a word); the
  film's last line at least 2 s. (`text_crowded`, `text_below_floor`,
  `line_unread`.)
- **Don't say it twice.** On-screen text that repeats the caption at the
  same moment is noise; show the picture the words describe instead
  (`repeats_caption`).
- **No chrome.** Nothing sits unchanged in every scene: no header or
  footer, section index ("07 / 10"), frame counter, progress bar or docked
  logo. A video player already has all of these (`persistent_chrome`).
- **Fill the frame for its shape.** A vertical film is laid out for the
  tall frame (a map fills it, it is not a landscape strip with bands
  above and below).
- **Cuts have a cause.** Use three to five kinds of transition in a film,
  each caused by something on screen (a push through an object, an object
  that becomes the next world, a carry across, a colour flood, a whip).
  Two unrelated worlds never crossfade into each other, and a cut never
  lands on an empty frame: the first frame after a cut already shows the
  new idea. Frame 0 is the thumbnail: never empty. (`world_crossfade`,
  `empty_after_cut`, `first_frame_empty`; `review` shows every cut.)
- **Say what each layer is.** Give layers a `role` (`background`,
  `subject`, `foreground`, `caption`, `decoration`) and anything that
  travels a `facing`. `check` then proves depth (scenery never in front
  of the subject) and direction (nothing walks or flies backwards), and
  exempts captions and deliberate small print from the text rules. Keep
  a rule broken on purpose with `allow` on that layer, not by ignoring
  `check` (see [Declaring intent](#declaring-intent-role-facing-allow)).
- **Maps.** Flat-map routes are `bow` arcs between the two cities as they
  sit on the map; `great_circle` only when the true path is the point
  (and then prefer a camera that shows it). Both ends of a route are in
  the frame when it lands (`draws_off_frame`).

### Banned by default

These are where generated video lands when nothing else is decided. Each
is banned unless the brand's own material or the person's reference uses
it, and then only as they use it:

- a dark void with small elements floating in its middle;
- glow, bloom, neon, lens flares and floating particles as decoration;
- purple or blue gradients, glass cards;
- a cream paper ground with a serif display, numbered "01 / 02" labels and
  small monospace caps labels everywhere;
- corner labels, frame borders, HUD readouts, frame counters as
  decoration;
- three cards in a row, KPI grids, spec tables, feature lists;
- everything centred; everything fading and sliding up; titles that keep
  drifting after they land;
- the logo docked in a corner.

After the first render, name every default the film used anyway, from
this list or not, write it on the film's `BANNED:` line in the visual
script, and fix it. The list grows with every film.

## Breakdown (before building a shot)

Write this for every shot before the first `edit_apply` for it, and keep
it next to the visual script. One block per element; a few words each:

```
shot <n> (<from>–<to>, sync: <anchors>)
- <element>: kind (2D shape | 3D object | text | footage | particles)
  structure: counts, segments, faces, thickness
  material: flat | glossy | coated | glass | emissive (+ colours)
  lighting: key direction, rim, highlights, shadows
  motion: what moves, eases, sync points
  build: engine 2D | engine 3D mesh | Blender (+ the layer kinds/tools)
```

Example, a logo sting:

```
shot 2 (3.82–5.10 s, sync: hit:2 lands, bar:3 out)
- ring: 3D object
  structure: torus split into 4 equal arc segments (90° each), thick tube
  material: glossy coated plastic, segments #ff3d7f, #ffd23f, #2d5bff, #00d4a6
  lighting: warm key from the top left, cool rim from behind right,
    one soft specular streak per segment, contact shadow below
  motion: segments fly in from 4 directions ("snap"), meet on hit:2
    (anchor: land), ring turns 20° on its y axis with a drift; out on bar:3
  build: engine 3D mesh: torus, segments 4, gap 6, colors ×4, material
    preset coated; one layer per segment (angles keyed) so each can fly
    in; default key + rim lights, studio environment
- title: text, "LAHZA", 1 line, heavy weight
  material: flat white; lighting: none (edge light from the ring's key side)
  motion: per-glyph enter, 1 f stagger, starting at hit:2 + 2f
  build: engine 2D (text, animators)
```

Measurements of a reference (`track`, `fit_curve`, sampled colours) fill in
the numbers: when things move, where they sit, colours, sizes, the camera.
They never replace the build line.

## Measuring a reference

`measure {video, kind}` (CLI `lahza motion measure <video> <kind>`) reads
numbers off a reference instead of eyeballing them:

- `cuts`: every cut, flash (a brief jump in brightness that comes back),
  dissolve (frames that are blends of the frames around them) and whip (a
  run of blurred, fast-changing frames), with times, from frame
  differences at 64 × 36.
- `events`: where the picture starts and stops changing (cuts left out),
  with the box of what changed: when a move begins and lands.
- `ink`: text size at `at` inside `region`: each line's top, bottom and
  height in px, and the stroke width; set a text layer's size from it.
- `captions`: when the caption band (`region`, default the bottom fifth)
  holds caption text (pixels near `color`, default white) and when it
  changes: caption timing to match.
- `curve`: track a target (`color`, `point` or `box`, as `track`) and fit
  one of the engine's eases to its x and y.

## Matching a reference camera

`fit_camera` (CLI `lahza motion fit-camera <project> <marks.json>`) solves
the scene camera instead of guessing it: for each reference frame, mark a
few 3D points of the scene (px, like a layer's position and z: corners of
a box, the tip of a rod) and where they appear in the frame. It solves
`fov`, `orbit`, `dolly` and `position` (`shift` too unless fixed; `fix`
keeps others) through the engine's own camera, frame by frame starting
from the last, and reports each frame's rms error in px. With `scene` it
returns `set` ops putting the result on that scene's camera as keys
(`apply: true` applies them). Six or more marks spread across the frame
and in depth pin it down; three to five work with more fixed.

## Comparing with a reference

`compare {against: "ref.mp4"}` checks every frame from first to last
(`offset` = reference seconds minus ours; `from`/`to` limit the range). It
returns a per-scene summary (how much each side moves, how different it
looks), the worst stretches with a sheet of them, a timeline graph and a
side-by-side video (ours | reference | difference). With `at: [...]` it
shows only those moments.

Every frame is scored by SSIM (structural similarity of the luminance,
Gaussian window σ 1.5; 1 is identical, around 0 unrelated), at the
comparison width (`width`, default 270 px). Each scene and the whole run
report `ssim: {mean, p05, min, worst_cell}`: `p05` is the 5th percentile
of the frames (the bad moments a mean hides), and `worst_cell` the cell of
a 4×4 grid whose mean SSIM over those frames is lowest, with its `box`
in project px, so it says *where* the replica differs. `drawn: [x, y, w,
h]` (project px) scores one element's box as well (`drawn_ssim` per
scene, `drawn` for the run, `ssim_drawn` per moment) and outlines it in
the pictures and the video. On the command line:

```bash
lahza motion compare film.lahza --against original.mp4 --video out/side_by_side.mp4
lahza motion compare film.lahza --against original.mp4 --drawn 640,300,420,180
```

The mean difference is dominated by large areas: thin lines, cages and
outlines barely move it, and leaving them out can even lower it. Each
scene also reports `missing_detail` and `extra_detail` (the share of edges
in one picture with nothing near them in the other); a verdict saying
"ours lacks detail" means something visible is missing, whatever the
difference says.

Each frame and scene also reports `shading_ours`, `shading_reference` and
`shading_ratio` (ours ÷ reference): how much light lives *inside* shapes.
It is the mean luminance slope, in levels per 1% of the frame width, over
interior pixels: pixels away from drawn edges (Sobel above 60, a step of
15+ levels, widened by about 1% of the width; thin lit tubes and rings
still have an interior) and not the background (the
most common colour ± 24); slopes under 0.5 count as flat. A lit 3D object
reads several units; flat fills, posterised polygons and line art read
about 0; gradient fills, glows and soft backgrounds count as shading too.
When the reference has real shading (≥ 1.0) and the ratio is under 0.5,
the verdict says "ours is flatter than the reference (shading 34% of the
reference's)" and those stretches show up as "ours is flatter". Flat 2D
graphics against flat graphics, or line art against line art, are never
flagged (`shading_ratio` is null when the reference is flat). A flat
verdict means the shot needs lighting (3D materials, gradients,
highlights), not a better match of pixels.

### No tracing as content

A low difference is not the goal; a shot built the way the reference was
made is. Measurements of a reference (`track`, `fit_curve`, sampled
pixels) may set **parameters**: timing, positions of objects, colours,
sizes, the camera. The final **geometry** is built from engine shapes and
text, 3D meshes or Blender, never from traced or rotoscoped reference
pixels: no per-frame polygons cut from the reference, no tracked colour
blobs drawn as shapes, no outlines keyed to the reference frame by frame.

`check` warns (`kind: "tracing"`) when a scene looks traced:

- 12 or more path layers drawn from SVG data (3+ points) that are each on
  screen for 2 frames or less (a flipbook of polygons), or
- path layers whose `d` is keyed on (nearly) every frame (12+ keys, median
  gap ≤ 1.5 frames): 3 such layers, or 48 such keys in the scene.

Ordinary morphs (a few eased keys), paths made from primitives
(`{"shape": …}`), short spark streaks, instancer copies and long-lived
hand-drawn paths don't count. Fix the warning by rebuilding the element
as the breakdown says, not by spreading the same tracing over more frames.

## Measuring a reference's motion

Don't eyeball frames or write scripts to measure a reference: use the tools.
What they measure sets parameters (timing, positions, colours, sizes,
camera), never the geometry itself (see
[No tracing](#no-tracing-as-content)).

- `track {video, color: "#ff3355", tolerance}` follows a coloured object
  (`region` limits the search); `point: [x, y]` follows the colour blob
  under a point; `box: [x, y, w, h]` follows a textured patch (position
  only). Per frame you get x/y, bbox, area, angle and mean colour in the
  reference's pixels; `from`/`to` limit the range.
- To copy a move frame for frame (a hand-held drift, a wobble), ask for
  `keys: {scale, offset, at, layer}`: canvas = reference × scale + offset,
  scale from the area, rotation from the angle, and `ops` to pass to
  `edit_apply`.
- To rebuild a designed move, give its samples to `fit_curve` (e.g.
  `[[t, [x, y]], …]` from track): it returns the ease (a preset when one
  fits), start, duration, from/to and a `key` to drop into the property's
  keys. Prefer this over per-frame keys: one eased key stays editable and
  snaps to beats.

## 3D objects (meshes)

Anything that reads as a solid object (rings, tori, discs, coins, petals,
cubes, pills, tubes that grow, extruded logos and letters) is a `mesh`
layer: real geometry, lit and shaded in the engine, live in the preview.
Never fake one with flat shapes, gradients or a traced outline; a flat
shape has no shading and `compare` will report it flatter than the
reference.

- **Geometry**: `sphere`, `box` (bevel rounds the edges), `cylinder`
  (`top` makes cones), `capsule`, `torus` (`angles` makes an arc, `caps`
  flat or round), `extrude` (any outline, holes kept, `bevel`), `revolve`
  (lathe a profile about y or z), `sweep` (a tube along a path, `trim`
  grows it, `head` puts a ball on the front).
- **Structure**: `segments` + `gap` split a ring, revolve or sweep into
  pieces; `colors` colours them in turn. Measure the count from the
  reference (count the pieces, don't guess).
- **Placement**: the usual transform; `rotation_x`/`rotation_y` tilt and
  turn it, `z` moves it in depth, `scale` scales it (depth follows x unless
  `scale_z`). Parent several meshes to a null to move them as one object.
  Mesh layers next to each other in the list share a depth buffer, so
  interlocking rings and objects passing through each other are right.
- **Material**: start from a `preset`: `glossy` (default) for plastic
  toys and UI objects, `coated` for lacquered/car-paint looks with crisp
  reflections, `metal`/`chrome`, `glass`, `matte`, `emissive` (glows
  without light), `flat` (unlit flat colour for flat-shaded idents). Then
  set `color`, `roughness`, `clearcoat`. `parts` gives the front, back,
  side, bevel or caps their own material (a coin with a gold rim).
- **Lighting**: the scene's `lights` (default: a key from the upper left
  in front and a rim from behind on the right) and `environment` (what
  glossy surfaces reflect: `studio`, `soft`, `sunset`, `night`, `none`).
  The key casts soft shadows (`softness`); add a `plane` floor (`rotation_x:
  90`) to catch them, or a plane with the `shadow` preset to drop them
  onto a 2D background. Glass (`preset: glass`) refracts the layers behind
  it (`ior`, `thickness`); rough glass is frosted.
- **Depth**: meshes, and flat 3D layers next to them (text and shapes with
  `three_d`), share one depth test: text can pass through a ring, a card
  can cut through a box, whatever the layer order.
  Match the reference: where the highlights sit gives the key direction,
  a bright outline on the far side means a rim light, dark glossy objects
  with thin streaks mean the `night` environment.
- **Camera**: the scene camera's `fov`, `orbit`, `dolly`, depth of field
  (`aperture`, `focus`) and motion blur all apply.
- **Viewport looks**: `preset: wireframe` draws a mesh's edges (`fill`
  for hidden-line); a wire cage over a glossy copy of the same mesh reads
  as a 3D app's viewport.
- Blender (`blender_run`) is for what meshes can't do: fluids, cloth,
  sculpted or imported models, caustics and global illumination.

```json
{"id": "ring", "kind": "mesh", "mesh": {"type": "torus", "radius": 220, "tube": 46},
 "segments": 4, "gap": 6, "colors": ["#ff3d7f", "#ffd23f", "#2d5bff", "#00d4a6"],
 "material": {"preset": "coated"},
 "transform": {"position": [960, 540], "rotation_x": 60,
               "rotation": {"keys": [{"at": 0, "v": 0}, {"at": 4, "v": 90}]}}}
```

## Footage and Blender

- Footage: `media_import` a clip or image sequence, then add a
  `{"kind": "video", "src": …}` layer (trim, speed, loop, time_remap).
- Blender (fluids, complex models): write a `bpy` script and run it with `blender_run`. It renders
  a transparent PNG sequence into `assets/blender/<name>/` that you place as
  a video layer, so 3D objects composite with type, shapes and effects.
  Match `frames` to the scene's span; use `fps` of the project.
  - Sync 3D keys to the timeline from `LAHZA`: `LAHZA["markers"]["beat"]`
    (`"seconds"`, `"frames"`, and `"labels"` for words), and
    `LAHZA["elements"]["title"]` (`start_frame`, `end_frame`) for scenes and
    layers; `frame_of(seconds)` converts any time. Frames count from
    `frame_start` at project time `at`: place the footage layer at `at`.
  - Iterate with `quality: "preview"` (EEVEE, half size, no motion blur, a
    sheet of `preview_frames` frames in seconds), then render
    `quality: "final"` once (every frame, `samples`, denoised, motion blur).
    It renders on the GPU (OptiX/CUDA) when there is one; `seconds_per_frame`
    in the report tells you what a longer range will cost.

## Sound effects

Sound sells motion. Give arrivals and big moves a sound with `sfx_add`,
anchored to the animation so it follows it (built-ins need no files):

- a layer flying in: `whoosh` at `"#card.start - 3f"` (air leads the move);
  leaving: `whoosh-out` at its exit;
- appearing or scaling up: `pop`; a card settling: `thud`; a title slam or
  a cut on a downbeat: `hit`; the drop: `boom`;
- a reveal: `riser` ending on it (`"#reveal.start - 1.6"`), then `ding` or
  `sparkle`; typing: `type` per word or glyph; clicks: `click`;
- keep effects under the music: `gain_db` -6 to -10, and fewer is better:
  one sound per idea, not per layer.

On a screen recording's own timeline use `recording_sound_add`;
`recording_info` lists its clicks, keystrokes and zooms.

## Motion inside a screen recording

A person editing a Lahza screen recording (`*.lahzarec`) can ask for a
motion design in it. Work inside the recording so it shows up on their
video timeline:

1. `recording_info {recording}`: size, the edited video's length, shared
   media, pieces already placed.
2. `project_create {path: "intro", recording}`: the piece is made inside
   the recording, at its shape. For a piece drawn over the video, keep the
   background clear (`"#00000000"`); coordinates are the recording's pixels.
3. Use the recording's files where they are: the raw recording is
   `"../../screen.mkv"`; `media_import` of any other file copies it into
   the recording's shared `media/`.
4. `edit_apply`, `check`, `snapshot` as usual.
5. `recording_place {start, placement}`: `overlay` draws it on the
   recording (it follows the video's zooms and camera), `full` covers the
   frame like an inserted clip. Call again to move it; `remove: true` takes
   it off. The person sees it live and can open it in the editor.

The recording itself is edited with `recording_edit`: its look (`scene`:
background, padding, corners, shadow, aspect, camera, pointer, keystroke
overlays, subtitles), its zooms (`zoomCues`), its cuts and speed (`clips`)
and its audio (`audioMode`, `audioMuted`). Read what is there first with
`recording_info {recording, edit: true}`; zoom and clip times are seconds
on the raw recording, and `source_clicks` gives the clicks in those
seconds. `scene` and a
zoom cue with a known id are merged, so send only what changes.
`recording_export {recording, path}` then writes the finished video
through the studio's own export; the studio must be showing the
recording.

## Repeaters: many copies of one thing

Dot fields, rings of petals or shards, rows of bars: build one `instancer`
(see the schema), never hundreds of layers with their own keys. Animate
the template once; `stagger` cascades it across the copies, `random`
breaks the regularity and `fields` move, grow, fade or tint copies by
where they are. Thousands of copies render in real time.

**Halftone ripple.** A dot grid pops on from the centre, then a ring
leaves the centre at 3.82 s and swells the dots as it passes, with a
slow noise twinkle throughout:

```json
{"id": "halftone", "kind": "instancer", "transform": {"position": [960, 540]},
 "layout": {"type": "grid", "columns": 49, "rows": 28, "spacing": [40, 40], "hex": true},
 "stagger": {"total": "18f", "order": "center"},
 "fields": [
   {"type": "wave", "center": [0, 0], "at": 3.82, "speed": 1100, "wavelength": 320, "scale": 2.2, "ease": "gesture"},
   {"type": "noise", "size": 220, "speed": 0.6, "seed": 7, "scale": 0.25, "opacity": -0.35}
 ],
 "layers": [{"id": "halftone-dot", "kind": "shape", "shape": "circle", "size": 10, "fill": "#f4f1ea",
   "transform": {"scale": {"keys": [{"at": "#halftone.start", "from": 0, "v": 1, "ease": "pop", "anchor": "start"}]}}}]}
```

**Burst ring of petals.** Twelve petals shoot out from a small ring one
frame apart, each pointing outward (`align`), coloured from a palette,
then fade:

```json
{"id": "burst", "kind": "instancer", "transform": {"position": [960, 540]},
 "layout": {"type": "radial", "count": 12, "radius": 40, "align": true},
 "stagger": {"each": "1f"},
 "random": {"seed": 4, "scale": [0.8, 1.1], "rotation": [-6, 6]},
 "palette": ["#ff3d7f", "#ffd23f", "#7b61ff"],
 "layers": [{"id": "petal", "kind": "shape", "shape": "capsule", "size": [26, 90], "fill": "#ffffff",
   "transform": {"anchor": [0, 45],
     "position": {"keys": [{"at": "#burst.start", "from": [0, 0], "v": [0, -260], "ease": "snap", "anchor": "start"}]},
     "scale": {"keys": [{"at": "#burst.start", "from": 0, "v": 1, "ease": "pop", "anchor": "start"}]},
     "opacity": {"keys": [{"at": "#burst.start + 0.5", "v": 1}, {"at": "#burst.start + 0.9", "v": 0}]}}}]}
```

Tips: key the template relative to the instancer (`#burst.start`), since
each copy replays those keys `delay` later. With `align`, the template's
up is outward (radial) or its x axis runs along the line or path, so a
template moving toward −y flies outward. A field's `amount` keyed 0 → 1
fades an effect in; `invert` on a `radial` field affects everything
outside the radius instead.

## Timing rules that make it look professional

- **Sync the peak, not the start.** Put `"anchor": "peak"` on transitions
  and moves that should hit a beat. Use `"lead": -1` on cuts and whips.
- **Entrances start at full speed**: `snap`, `glide`, `settle` (exponential
  lag) or `enter`. Avoid ease-in on entrances.
- **Overshoot only on scale and landings** (`pop`, `land`).
- **Nothing stops dead.** Add a drift track after the entrance:
  `{"type": "drift", "rate": 0.005, "per": "frame", "mode": "mul"}` on scale
  or a few px/s on position.
- **Staggers are tiny**: 1–3 frames (`"stagger": "1f"`).
- **Shots open mid-move**: start the entrance a few frames before the cut
  or set `pre_roll` on the scene.
- **Stop-motion is stepped time**, not jitter: `"time": {"step_fps": 12}`.
  The camera keeps its own smooth clock.
- **Zooms over large ratios** use `zoom` (log space).
- **A handle on a stroke's drawing head** (pen tip, ring, arrowhead) is one
  field: `"follow": {"layer": "<path id>"}` on the handle (`"rotate": true`
  for arrows). Animate only the path's `trim`; never key the handle's
  position frame by frame.
- **A stroke that changes colour as it draws** is `stroke.gradient`
  (`{"stops": […], "along": "length"}`) on a path; the colours stay on the
  path while `trim` reveals it.
- **Gradients on growing shapes** use `"units": "box"` so they stretch
  with an animated `size` instead of staying pinned.
- **Soft colour-blob backgrounds** are one `shape: "field"` layer with a
  few moving `points` (wiggle their positions), never dozens of blurred
  layers.
- **Rack focus** in 2.5D: give the camera an `aperture` (2–4) and key its
  `focus` from one layer's depth to another's (or set `focus` to a layer
  id); spread 3D layers in `z` so depth reads.

## Display type

- **Outlines** are one field: `"stroke": {"color", "width"}` on the text
  (`"align": "center"` or `"inside"` for hollow type with a transparent
  `color`). Never stack offset copies of a layer to fake an outline.
- **Bolder or thinner** type is `"weight"` (px, animatable), not
  duplicated layers.
- **Rim-lit or bevelled** letters: `"edge": {"color", "width", "angle"}`
  lights the edges that face the light.
- **Reveals that sweep across the text** use a positional `order`
  (`{"axis": "x"}`, `{"radial": [0.5, 0.5]}`) on one animator, not one
  layer per letter; add `"feather"` for a soft wipe through the glyphs.

## Whole-film review

Per-scene snapshots miss what only shows at cuts and on a phone: a cut to
half a second of blank paper, a transition showing a half-faded box with
ghosted text, a 3D object drawn over the next scene's map in a crossfade,
labels too small to read on a phone. `review` renders three sheets of the
whole film (small, no motion blur), every frame labelled with its scene and
time:

- **scenes**: each scene 0.4 s after it starts and at its middle;
- **cuts**: a strip at −0.2, −0.1, 0, +0.1, +0.2 s around every scene
  boundary, spanning the transition when it is longer;
- **phone**: the scene frames 360 px wide, in pages.

Then, sheet by sheet:

1. Describe each frame in one line as a stranger would, naming every
   object in it. Anything you would have to explain is a P0.
2. Cover the words: do the frames alone still tell the story?
3. At every cut: no blank or near-blank frame, no half-faded box or
   ghosted text, nothing of one scene drawn over the other.
4. At 360 px: can every line be read? If not, make it bigger or cut it.

One note per problem, worst first: P0 (the viewer is lost or sees a broken
frame), P1 (it costs attention), P2 (polish), as
`P0 t=5.2s · s01 opening · bare paper for half a second after the cut → bring the 81 on with the cut`.

`check` measures two of these on small renders: `first_frame_empty` (frame
0, the thumbnail social players show, is only background) and
`empty_after_cut` (0.1–0.4 s after a cut nothing but background is on
screen: dead air). Only background means under 0.2% of a 270 px render's
pixels on a drawn edge: flat colour, gradients, paper grain and vignettes
don't count, one small word or a dot does. A flash that clears within
0.1 s of the cut is not dead air.

## Declaring intent: role, facing, allow

You write the film from your intent and never watch it as a viewer;
`check` does, frame by frame, but only knows what a layer *is* if you say
so. Declare it while you build:

- **`"role"`**: `background` (the map, the room, the backdrop: behind
  everything it overlaps), `subject` (what the shot is about),
  `foreground` (in front of the subject: a label on it, a frame around
  it), `caption` (spoken words on screen) or `decoration` (deliberately
  small or brief: credits, a texture label; no text checks). A comp's role
  passes to its layers unless they set their own. `captions` children are
  `caption` and the map's water, countries and lakes are `background`
  without asking. Plain text layers used as captions need
  `"role": "caption"`, or the text checks count them as on-screen text.
- **`"facing"`**: the way the art points in its own space: `"right"`
  (+x), `"left"`, `"up"`, `"down"` or degrees clockwise from +x. Give it to
  anything that travels: cars, planes, ships, fish, arrows. A layer that
  `follow`s a path with `rotate: true` faces along the path unless
  `facing` says the art points elsewhere. On a comp it is measured on the
  comp's first layer, so keep that layer unturned.
- **`"allow"`**: check kinds the layer (and its children) breaks on
  purpose: `["text_below_floor"]` on deliberate small print,
  `["persistent_chrome"]` on a watermark. Scenes take
  `["world_crossfade"]`. Keep the thing *and* say so; a warning you leave
  standing hides the next one.

```json
{"id": "world", "kind": "comp", "role": "background", "layers": [
  {"id": "iran-label", "kind": "text", "text": "IRAN", "role": "foreground"},
  {"id": "tanker", "kind": "comp", "facing": "right", "follow": {"layer": "lane", "rotate": true}, "layers": […]}]}
```

## What check verifies and measures

Errors: the picture contradicts what you declared.

- **`depth_order`**: a `background` drawn in front of a `subject`, or a
  `foreground` drawn behind one, where their screen boxes overlap by at
  least 20% of the one hidden, on two samples in a row (¼ s apart). 3D
  layers are compared by camera depth, not list order. Fix the order (or
  `z`), or the role if you declared the wrong one.
- **`moves_backwards`**: a layer with a facing whose motion points against
  it (dot < −0.3) faster than 3% of the frame's short side per second for
  0.3 s. Motion is measured in the layer's own world (its follow path, its
  parent or a sibling that does not move), so a camera or comp carrying it
  doesn't count; flips (negative scale) and rotation do. Flip it
  (`scale` x −1), turn it, or set `facing` to the way the art points.

Warnings: measured craft. One finding per cause and scene, listing the
layers; `role` and `allow` are respected.

- **`text_below_floor`**: text under 2.6% of the frame's short side tall
  on screen (28 px at 1080; its `size` times its on-screen scale) for
  0.5 s or more. Captions are judged too. Make it bigger, say it in the
  voice-over, or mark deliberate small print `decoration`.
- **`text_crowded`**: more than 12 words or more than 5 text layers on
  screen at once (captions and decoration aside) for a second: the web-page
  tell. Keep one short idea on screen; the voice carries the rest.
- **`line_unread`**: a line (not a caption) on screen shorter than it
  takes to read: 0.75 × (0.5 s + 0.3 s per word), 0.4 s for one or two
  words, 2 s for the film's last line. Numbers replaced at one place (a
  count) and words the next scene carries on don't count. Hold it or cut
  it.
- **`repeats_caption`**: text saying the caption shown with it (three or
  more words in a row in common): read twice. Show what the words can't (a
  number, a place, a face), or drop one.
- **`persistent_chrome`**: the same text or small shape at the same place
  in the frame's outer 15% in 3 or more scenes, or for 60% of the film: a
  header, footer, frame counter or progress bar. Films have no chrome.
- **`world_crossfade`**: a scene that dissolves (`fade`) into another,
  both showing something. Worlds never crossfade: cut on a cause (a hit, a
  word), carry one element across, flood the frame with a colour or whip.
- **`draws_off_frame`**: a stroke drawn on with `trim` (open, not an
  outline) whose head is off the frame when its draw ends, or on its
  scene's last frame, while its start is on it: an arc to a city the
  viewer cannot see. Frame the destination before the draw lands.

## Review checklist

- `check` has no errors; every anchored key's drift is within ±1 frame;
  no `tracing` warnings; craft warnings (`first_frame_empty` and
  `empty_after_cut` included) fixed or `allow`ed.
- `review`: every frame describable without explanation, the cuts clean,
  every line readable at 360 px.
- `sync_check {flagged: true}` lists nothing you did not mean (ambient
  motion may land between markers on purpose).
- Every shot was built from its breakdown; against a reference, no scene
  says "ours is flatter" or "ours lacks detail".
- No still picture longer than the style's hold limit.
- Text inside title-safe, readable contrast, no overlapping labels.
- Every asset has a source and a licence.
- Contact sheet at the sync points reads as the visual script intends.

## Narration

Narrate before building scenes, and time everything from the narration.

1. **Script with cues.** One line per clip (id it: `hook`, `s03`). Mark
   the moments visuals hit with `[cue]` markup: `One bomb. Over
   [strike]Iran.` gives `cue:strike` (the cued word: start and `.end`);
   `[map]Over Iran[/map]` spans words. Write numbers and names as they
   should be shown ("81", "U.S."); the voice may say them differently.
2. **Voice with timings.** Any provider, through an adapter in
   `tools/narration` that writes a manifest: each clip's audio, script text
   and the provider's `words` or `chars` timings. Kokoro needs a model
   export with a duration output; ElevenLabs `with-timestamps` gives
   characters; voices without timings need an external aligner.
3. **`narration_import`** places the clips (one after another unless `at`
   is given), builds `line`, `sentence`, `word` and `cue` tracks, and
   reports timing problems — words with no time, out of order, outside
   their clip or in silence. These are errors in `check`; fix the
   timings, not the references.
4. **Time with references, never seconds.** Scene starts on `line:s03`,
   a reveal on `cue:strike`, a caption until `sentence#4.end`, a sound
   effect on `word:'81'` (quote numbers: `word:81` is the 81st word).
   Indices (`word#42`) shift when the script changes; cues and line ids
   do not. Re-narrating — a new hook line, a reworded sentence — then
   moves every scene with no other edits; unchanged lines are reused.
