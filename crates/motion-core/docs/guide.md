# Making a video with Lahza motion

## The loop

1. **Brief → visual script.** One row per spoken line or musical section:
   the words, the visual, the sync anchor (`word:canal`, `bar:5`, `hit:2`)
   and the style. Get it approved before building.
2. **Audio first.** Put the music (and voiceover) in `assets/`, register it
   with `audio_analyze` (writes `beat`, `bar`, `half`, `sixteenth`,
   `triplet`, `onset.*`, `hit`, `stop`, `drop`, `music.start`,
   `music.end`). Import word timestamps for narration (`markers_import`).
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
4. **Build** with `edit_apply`, one scene at a time. Batches are atomic and
   undoable (`history`).
5. **Review**: `check` (no errors, sync drift ≤ ±1 f) and `snapshot` with
   `sheet: true` at the scene's sync points. Look at the frames; fix; repeat.
6. **Render** a draft (`quality: "draft"`), then the final.

## Comparing with a reference

`compare {against: "ref.mp4"}` checks every frame from first to last
(`offset` = reference seconds minus ours; `from`/`to` limit the range). It
returns a per-scene summary (how much each side moves, how different it
looks), the worst stretches with a sheet of them, a timeline graph and a
side-by-side video (ours | reference | difference). With `at: [...]` it
shows only those moments.

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
15+ levels, widened by the slope's reach) and not the background (the
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

## Measuring a reference's motion

Don't eyeball frames or write scripts to measure a reference: use the tools.

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

## Footage and 3D

- Footage: `media_import` a clip or image sequence, then add a
  `{"kind": "video", "src": …}` layer (trim, speed, loop, time_remap).
- 3D: write a Blender `bpy` script and run it with `blender_run`. It renders
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

## Review checklist

- `check` has no errors; every anchored key's drift is within ±1 frame.
- Against a reference, no scene says "ours is flatter" or "ours lacks
  detail".
- No still picture longer than the style's hold limit.
- Text inside title-safe, readable contrast, no overlapping labels.
- Every asset has a source and a licence.
- Contact sheet at the sync points reads as the visual script intends.
