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
   shape (stops, drops, the biggest hit), not from BPM alone.
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

## Footage and 3D

- Footage: `media_import` a clip or image sequence, then add a
  `{"kind": "video", "src": …}` layer (trim, speed, loop, time_remap).
- 3D: write a Blender `bpy` script and run it with `blender_run`. It renders
  a transparent PNG sequence into `assets/blender/<name>/` that you place as
  a video layer, so 3D objects composite with type, shapes and effects.
  Match `frames` to the scene's span; use `fps` of the project.

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

## Review checklist

- `check` has no errors; every anchored key's drift is within ±1 frame.
- No still picture longer than the style's hold limit.
- Text inside title-safe, readable contrast, no overlapping labels.
- Every asset has a source and a licence.
- Contact sheet at the sync points reads as the visual script intends.
