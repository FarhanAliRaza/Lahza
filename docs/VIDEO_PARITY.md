# Video parity contract

## Recording lifecycle

- [x] Explicit Display / Window / Area recording modes, portal source selection,
  countdown, cancel, and close-on-start controls. Area recording has an
  app-owned rectangle picker after portal monitor selection; it crops the
  stream before encoding and uses the embedded system cursor. Cross-desktop
  portal and live-device validation remains outstanding.
- [x] Starting, recording, paused, finishing, and idle states.
- [x] Fixed-width elapsed clock with visible recording/paused status.
- [x] Pause/resume, restart, stop-and-save, and discard controls backed by the
  same recording controller rather than UI-only placeholders.
- [x] Protected recording pill and camera monitor start together on the
  launcher's physical monitor, including unequal-height and portrait layouts.
  A selected display moves both overlays together; window capture and missing
  portal geometry retain their initial visible placement. Helper failures
  preserve launcher controls and report the failure reason.
- [ ] A recording that started is never silently lost after an encoder,
  compositor, display, sleep, or disk error.
- [x] Stop opens the persistent project without forcing a flattened export,
  retaining the chosen scene style.

## Project package

- [x] Persistent `.lahzarec` directory rather than a bare temporary file.
- [x] Screen master, capture manifest, input sidecar, saved edit, and draft paths.
- [x] Atomic JSON sidecar writes.
- [ ] Camera master, replacement audio, poster, render stamp, and metadata.
  Real decoded poster generation, lossless JSON render stamps, and project
  metadata and the optional camera master are implemented; replacement-audio
  persistence remains.
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
- [x] Deterministic cursor path planner with 1 ms integration, cut-bounded
  anticipation, grouped input accuracy constraints, bounded speed search,
  compressed idle spans, and deterministic arbitrary seeking. Smooth /
  Medium / Rapid / None are selectable; old motion values still deserialize.
- [x] Spring click scale (90% while pressed), idle blur/fade, typing hide,
  velocity tilt, directional steering, and a configurable loop return.
  Available capture metadata supplies button, drag, release, and artwork
  constraints; dedicated hover-change metadata is not currently recorded.
- [x] Deterministic click-pulse geometry and keystroke caption timeline shared
  as renderer-ready data.
- [x] Clip-aware pointer remapping before spring integration, including
  half-open cut boundaries, removed-event filtering, speed scaling, and a
  deterministic travel seed at every retained clip boundary.

## Automatic zoom

- [x] Compact click zoom: 0.6 s pre-roll, 1.6 s post-roll, 0.6 s join
  tolerance, 1 s tail exclusion, 0.8 s trailing guard, and 1.5× default
  magnification. Cinematic is an explicit alternative with 0.3 s pre-roll,
  2.5 s post-roll/join tolerance, 2× zoom, and Focused camera response.
  New captures seed Cinematic when no authored cues exist; existing/imported
  projects keep the Compact fallback. New manual cues use Focused, while
  missing response fields in older saved cues continue to load as Standard.
  See [the Screen Studio study](SCREEN_STUDIO_ANIMATION_STUDY.md) for evidence
  and remaining differences; these options do not establish full parity.
- [x] Pointer, smart, and pinned anchor modes represented as project data.
- [x] Editable add/remove/move/resize/enable controls with undo/redo. Motion
  regions live on an orange timeline lane: double-click adds one at that
  time, edges retime it, and the selection-aware inspector edits its style
  (hold, zoom in, zoom out), magnification, target mode, focus point, pan
  destination, and enabled state.
- [x] Deterministic 120 Hz viewport spring used by both preview and export.
  `ViewportTimeline` is the single camera source; the GPUI preview and the
  CPU `SceneCompositor` read the same `visible_rect` per frame.
- [x] Per-region Standard, Smooth, and Focused camera response, persisted with
  the cue and retained during release. Older projects retain Standard.
- [x] Duration-weighted activity clusters after 10 px travel thinning, with
  button events retained and bounds scaled by zoom, crop, and Fill on both axes.
- [x] Classic and Glass Loupe are separate animated tracks. The lens includes
  source-anchored focus, size, magnification, spring appearance/disappearance,
  bevel refraction, RGB dispersion, reflected rim, directional highlights,
  colored shine, shadow, and camera fading. CPU preview and GPU/CPU export
  use the same geometry and optical equations; canvas overlays stay above it.
- [x] Motion overview exposes illustrated **Classic zoom** and **Glass loupe**
  creation cards at the playhead, plus a selectable list of regions. Creation
  takes one undo step. Selected zooms follow the recovered inspector order:
  style, Instant animation or lens size, Auto/Manual position, and zoom level.
  Manual positioning has a clickable source preview; the zoom track supports
  click and drag. Advanced optics/animation/timing and Pan & transform are
  separate disclosures. Automatic generation and new-region defaults are
  grouped in the overview; image-scene presets have their own section.
- [x] Playback composition runs outside the UI thread with a single replaceable
  waiting frame, preventing a backlog. Paused composition also runs off-thread
  and rejects stale source, time, and scene results. Large CPU lens passes use
  at most four parallel workers.
  Playback preview size is bounded to 1.44 million pixels while preserving
  aspect ratio; settled preview and export use their requested full resolution,
  including the window's device scale for HiDPI previews.
- [x] Native source detail survives playback, settled seeks, and export
  preparation. Interactive scrubbing uses a temporary proxy. The lens
  reprojects continuous samples into the original source, including bevel and
  rim reflections, instead of enlarging a downsampled canvas. CPU/GPU raw-source
  mapping preserves annotations, cursor artwork, alpha, and layer order.
  Synthetic one-pixel text/stroke probes now have zero mean source error,
  versus approximately 68.4/255 with the former reduced-canvas path.
- [x] Bounded learned enhancement for enlarged source regions in settled
  previews and export, with ordinary original-source fallback. It uses public
  MIT Anime4K x2 S coefficients; 4× repeats the network with an attenuated
  second pass. Final blending is capped at 85% for 2× and 65% for 4×.
  Playback skips neural inference. GPU/independent CPU model
  tests agree within 1/255 across full images, cropped regions, and tile seams.
  This generic line-art model is not Screen Studio's UI-trained network;
  exact visual parity and improved recognition of arbitrary text are unproven.
  See [the clarity study](SCREEN_STUDIO_ANIMATION_STUDY.md#screen-super-resolution-and-source-clarity)
  and [model provenance](../assets/upscaler/README.md).
- [x] Per-region early animation (350 ms) and exact instant boundaries. The
  instant spring window extends 50 ms beyond the authored end to snap release;
  the visible zoom itself does not extend past the region.
- [x] Clip cuts and speed changes remap camera and lens regions deterministically;
  removed regions cannot leave a held pose on retained footage.
- [ ] Long-travel comfort widening beyond the bounded camera target.

## Native Linux backend

- [x] Select a monitor or window through the desktop ScreenCast portal.
- [x] Consume the granted PipeWire stream without launching another application.
- [x] Encode VP8/Matroska directly into the Lahza project package.
- [x] Pause and resume with finalized segments and join them on stop.
- [x] Remove OBS/WebSocket and all profile/output-directory mutation.
- [x] Capture pointer travel, clicks, drag state, and privacy-filtered shortcut
  keys into `input.json` through the packaged GNOME compositor helper. The
  master is cursor-free when the helper handshake succeeds and safely embeds
  the cursor when it is unavailable.
- [x] Optional system-audio and microphone sources share the native media clock,
  mix into an Opus track, survive pause/resume, and are recorded in the manifest.
- [x] Optional native camera capture and device selection, persisted as a
  camera master and mapped through the same clip timeline as the screen.
  Live physical-device validation remains outstanding in this implementation
  session; synthetic media tests do not establish desktop hardware reliability.
- [x] Overlay / Fullscreen / Split camera layouts, native/square/wide/tall
  aspect choices, source crop and focus, mirror, corner radius, shadow, and
  separate normal/zoomed size. Shared layout math drives CPU preview, GPU
  export, focus picking, drag limits, and the framing preview.

## Studio and export

- [ ] Video preview and seekable clip timeline. FFprobe media inspection,
  arbitrary seek-frame decoding, consecutive RGBA frame streaming, fitted
  preview sizing, process cleanup, and atomic poster generation are verified.
  GPUI now opens `.lahzarec` directly and provides synchronized
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
- [x] Camera/background/aspect controls. The video canvas now lives in the
  standard Lahza shell and uses the shared color/gradient/wallpaper
  library, padding, corners, four shadow styles, aspect presets, border, and
  collapsible inspector. The preview is the compositor's own output, so
  background blur, grain, vignette, watermark, and the 3D media transform
  (scale, position, X/Y/Z rotation, perspective, anchor, with drag, tilt,
  spin, scroll-to-scale, corner handles, and double-click reset) look the
  same in the editor and in the file. Scene settings autosave into the
  project's edit document under `scene`; recorded or imported camera media
  uses the shared camera compositor and layout controls.
- [x] Preview and export share the same immutable pointer and viewport timelines.
  `recording::export::export_scene` renders the full scene (background,
  padding, corners, border, shadow, camera motion, glass loupe, camera media,
  reconstructed cursor and click pulses) through `recording::scene::SceneCompositor`, then encodes
  MP4 (H.264/AAC), WebM (VP9/Opus), or GIF with FFmpeg. Layout is derived
  from `SceneGeometry`, which the preview canvas also uses.
- [ ] Timestamped export with progress, cancellation, and stale-render detection.
  Frame-level progress, cancellation, resolution presets (original, 720p,
  1080p, 1440p, 4K), 30/60 fps, GIF looping, audio include/mute, and a size
  estimate are implemented; stale-render detection remains.
- [x] Pointer inspector: show/hide cursor, cursor size, cursor shadow, hide
  when idle, spring response, typing hide, steering, loop return, click effects
  and colour; click markers on the ruler select
  individual presses, which can be removed (persisted as
  `lahzaExtras.removedPressTimes`).
- [x] Audio lane with an RMS waveform mapped through the clip timeline and a
  mute toggle; clip thumbnails sampled from the master.
- [x] Preset library (`~/.config/lahza/presets.json`) saving background,
  layout, border, shadow, effects, pointer look, aspect, and default zoom
  strength for screenshots and recordings; Quick / Customize / Advanced
  inspector levels.
- [x] Timed annotations on recordings: the screenshot drawing tools (text,
  rectangle, filled rectangle, ellipse, line, arrow, pen, number, highlight)
  work on the recording canvas, through the viewport crop and the 3D
  projection. Each mark has a start/end on a stacking annotation lane with
  entrance and exit effects; marks persist as `annotations` in the edit
  document and export as a per-frame overlay through the same compositor
  path the preview uses (verified by an FFmpeg integration test). Blur and
  pixelate stay screenshot-only because they are baked into the still.
- [x] Template gallery (`recording::templates`): eight curated looks that
  set background, layout, 3D pose, motion preset with easing, and timed
  captions; applied to a screenshot they create the animated scene, applied
  to a recording they restyle it and add an intro while keeping later motion
  regions. Template captions are ordinary annotations afterwards.

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

- 2026-10-25: Screen Studio static study and independent Rust reconstruction
  added constrained cursor planning, duration-weighted camera focus, lens optics,
  camera layouts, and recording controls. Synthetic render/animation tests and
  a glass-lens gallery exercise the implementation; no macOS reference runtime
  or pixel-equivalence claim is made. See [the study](SCREEN_STUDIO_ANIMATION_STUDY.md).
- 2026-10-25: the final default suite passed 421 tests, with 48 opt-in tests
  ignored. Native UHD and camera/cursor/glass/export editor workflows separately
  passed with process exit 0 after source-promotion and GPU-shutdown fixes. A 20-second
  excerpt of the user's recording exported at 2580×1080/30 fps in 22.94 seconds
  on RTX 3060 with NVENC. Its glass cue falls outside that excerpt, so this
  does not measure real-recording glass-export performance.
