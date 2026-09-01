# Video parity contract

The macOS Swift implementation is the behavioral specification for the Rust
video workflow. A control appearing in the Rust UI is not evidence of parity;
the underlying state transition, saved project data, recovery behavior,
preview, and exported output must agree.

## Recording lifecycle

- [ ] Full-screen and window source selection; area selection where supported.
- [x] Starting, recording, paused, finishing, and idle states.
- [x] Fixed-width elapsed clock with visible recording/paused status.
- [x] Pause/resume, restart, stop-and-save, and discard controls backed by the
  same recording controller rather than UI-only placeholders.
- [ ] A recording that started is never silently lost after an encoder,
  compositor, display, sleep, or disk error.
- [ ] Stop opens a persistent project without forcing a flattened export.
- [ ] Global shortcut can start capture while the editor is unfocused.

## Project package

- [x] Persistent `.screendroprec` directory rather than a bare temporary file.
- [x] Screen master, capture manifest, input sidecar, saved edit, and draft paths.
- [x] Atomic JSON sidecar writes.
- [ ] Camera master, replacement audio, poster, render stamp, and metadata.
  Real decoded poster generation, lossless JSON render stamps, and project
  metadata are implemented; camera and replacement-audio persistence remain.
- [x] Draft autosave remains separate from explicit save, reopens first after
  recovery, and preserves unknown fields written by newer project versions.
- [x] Recovery scans unfinished packages after restart and adopts usable raw
  native MKV/MOV/MP4 output without deleting incomplete work.

## Pointer and timing

- [x] Normalized top-left-origin pointer coordinates.
- [x] Travel/drag, press/release, keystroke, artwork, and pause data model.
- [x] Monotonic capture clock starts with the first native encoder segment.
- [ ] Dynamic window/source geometry mapping.
- [ ] Wayland cursor collector with explicit permission/fallback behavior.
- [ ] Pause intervals removed from event time without moving earlier events.
- [x] Swift-equivalent deterministic pointer sanitizer: stable ordering,
  tolerance clamping, isolated-spike removal, travel thinning, input cleanup,
  artwork validation, and travel/press/release stream priority.
- [x] Deterministic 120 Hz reconstructed cursor timeline with Swift's glide,
  intercept, drag-track, press-settle, inactivity reveal, tilt, interpolation,
  and exact press-target behavior.
- [x] Deterministic click-pulse geometry and keystroke caption timeline shared
  as renderer-ready data.
- [x] Clip-aware pointer remapping before spring integration, including
  half-open cut boundaries, removed-event filtering, speed scaling, and a
  deterministic travel seed at every retained clip boundary.

## Automatic zoom

- [x] Swift cue synthesis constants: 0.3 s pre-roll, 2.5 s post-roll,
  2.5 s transitive join tolerance, 1 s tail exclusion, 0.8 s trailing guard,
  and 1.5× default magnification.
- [x] Pointer, smart, and pinned anchor modes represented as project data.
- [x] Editable add/remove/move/resize/enable controls with undo/redo. Motion
  regions live on an orange timeline lane: double-click adds one at that
  time, edges retime it, and the selection-aware inspector edits its style
  (hold, zoom in, zoom out), magnification, target mode, focus point, pan
  destination, and enabled state.
- [x] Deterministic 120 Hz viewport spring used by both preview and export.
  `ViewportTimeline` is the single camera source; the GPUI preview and the
  CPU `SceneCompositor` read the same `visible_rect` per frame.
- [ ] Bounds-aware framing and long-travel comfort widening.
- [ ] Clip cuts and speed changes preserve continuous camera motion.

## Native Linux backend

- [x] Select a monitor or window through the desktop ScreenCast portal.
- [x] Consume the granted PipeWire stream without launching another application.
- [x] Encode VP8/Matroska directly into the Screendrop project package.
- [x] Pause and resume with finalized segments and join them on stop.
- [x] Remove OBS/WebSocket and all profile/output-directory mutation.
- [x] Capture pointer travel, clicks, drag state, and privacy-filtered shortcut
  keys into `input.json` through the packaged GNOME compositor helper. The
  master is cursor-free when the helper handshake succeeds and safely embeds
  the cursor when it is unavailable.
- [x] Optional system-audio and microphone sources share the native media clock,
  mix into an Opus track, survive pause/resume, and are recorded in the manifest.
- [ ] Optional camera master and device selection. A camera clip can be
  added to a project from a file (`camera.mkv`), is cut with the same clip
  timeline as the master, and is composited as a picture-in-picture bubble
  (corner, size, circle/rounded/square, mirror, margin, shadow) in preview
  and export; live camera capture and device selection remain.

## Studio and export

- [ ] Video preview and seekable clip timeline. FFprobe media inspection,
  arbitrary seek-frame decoding, consecutive RGBA frame streaming, fitted
  preview sizing, process cleanup, and atomic poster generation are verified.
  GPUI now opens `.screendroprec` directly and provides synchronized
  GStreamer video/audio playback, play/pause, ±5-second seeking, and a
  draggable seek bar. The lane now renders and selects every retained clip;
  thumbnail sampling remains.
- [x] Trim/split/speed editing and synchronized pointer remapping. This
  includes live mouse edge trimming with neighbor/minimum-duration limits,
  integer 1x-8x speed, split/delete, undo/redo, crash-safe draft autosave,
  explicit save, cut-aware pointer remapping, and real video/audio
  composition.
- [x] Timeline zoom and horizontal navigation with Swift's playhead-anchored
  scaling, 240-points/second and 100,000-pixel caps, Ctrl/Super-wheel zoom,
  wheel panning, visible zoom controls, and fit reset.
- [ ] Camera/background/aspect controls. The video canvas now lives in the
  standard Screendrop shell and uses the shared color/gradient/wallpaper
  library, padding, corners, four shadow styles, aspect presets, border, and
  collapsible inspector. The preview is the compositor's own output, so
  background blur, grain, vignette, watermark, and the 3D media transform
  (scale, position, X/Y/Z rotation, perspective, anchor, with drag, tilt,
  spin, scroll-to-scale, corner handles, and double-click reset) look the
  same in the editor and in the file. Scene settings autosave into the
  project's edit document under `scene`. Camera composition remains (no
  camera master is recorded yet).
- [x] Preview and export share the same immutable pointer and viewport timelines.
  `recording::export::export_scene` renders the full scene (background,
  padding, corners, border, shadow, camera motion, reconstructed cursor and
  click pulses) through `recording::scene::SceneCompositor`, then encodes
  MP4 (H.264/AAC), WebM (VP9/Opus), or GIF with FFmpeg. Layout is derived
  from `SceneGeometry`, which the preview canvas also uses.
- [ ] Timestamped export with progress, cancellation, and stale-render detection.
  Frame-level progress, cancellation, resolution presets (original, 720p,
  1080p, 1440p, 4K), 30/60 fps, GIF looping, audio include/mute, and a size
  estimate are implemented; stale-render detection remains.
- [x] Pointer inspector: show/hide cursor, cursor size, cursor shadow, hide
  when idle, click effects and colour; click markers on the ruler select
  individual presses, which can be removed (persisted as
  `screendropExtras.removedPressTimes`).
- [x] Audio lane with an RMS waveform mapped through the clip timeline and a
  mute toggle; clip thumbnails sampled from the master.
- [x] Preset library (`~/.config/screendrop/presets.json`) saving background,
  layout, border, shadow, effects, pointer look, aspect, and default zoom
  strength for screenshots and recordings; Quick / Customize / Advanced
  inspector levels.

## Animated screenshots

- [x] A screenshot opens static; **Animate** turns it into a timed scene
  (3/5/8/10 s) edited with the same motion lane, inspector, and exporter as a
  recording.
- [x] Presets (slow zoom in/out, pan left/right, focus, sweep, 3D tilt,
  floating card) expand into ordinary editable motion regions.
- [x] Cursor walkthrough: click the spots a synthetic cursor should visit;
  the cursor glides between them with clicks, click zooms are synthesized,
  and the cursor is drawn by the same reconstruction as recordings.
- [x] Annotations are flattened onto the capture before compositing so the
  animated export matches the static one.
- [x] MP4, WebM, and looping GIF export with progress and cancellation.
- [x] Timed annotations: marks placed while animating get a start/end,
  entrance (cut, fade, pop, slide, draw-on, type-on) and exit (cut, fade,
  shrink, slide) with a draggable annotation lane and a timing inspector;
  the exporter renders them frame by frame through the same animation code
  as the preview.
- [x] 3D transforms and tilt presets (3D tilt, floating card); motion
  regions can carry an animated tilt and an easing curve.
- [x] Background blur, noise, vignette, and a text watermark.

Rust video support must not be described as complete until all required rows are
implemented and a manual Swift-versus-Rust behavior pass has been recorded.

## Live validation log

- 2026-08-24: the visible GNOME portal chooser produced a persistent 36-byte
  PipeWire restore token. Reuse, active-stream validation, full
  start/pause/resume/stop/import, and a decoded non-blank screen frame passed
  (`YMIN=3`, `YMAX=245`, `YAVG=50.82`).
- 2026-08-24: the Swift v5 clip model was ported with normalization,
  legacy-trim migration, split/delete/range removal, 1x-8x speed, inclusive
  playhead and half-open event boundary contracts, plus unknown-field-safe
  draft persistence. An FFmpeg integration test cut a synthetic A/V source,
  accelerated one retained clip to 2x, concatenated both streams, and decoded
  the resulting preview successfully with audio present.
