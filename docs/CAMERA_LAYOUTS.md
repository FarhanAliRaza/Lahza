# Timed webcam layouts and local person cutout

Camera layouts are authored independently from screen zooms. The Camera
inspector edits either the project default or the selected purple camera
region. Add a region at the playhead, double-click the camera lane, drag a
region to move it, or drag its edges to trim it. Duplicate, remove, undo,
redo, autosave and reopening use the same saved camera-region data.

Regions use original recording time, matching zoom regions. A cut or speed
change remaps the region with its source footage; repeated footage repeats
its camera layout. The project default applies outside regions. Entry and
return transitions are seek-independent and interpolate geometry, opacity,
corner radius, crop and person-cutout strength. Loaded overlapping ranges
use the latest start and transition back to the underlying range; the editor
prevents creating new overlaps.

Layouts are Overlay, Split screen, Camera only, Screen only and Cutout.
Camera-only fills the canvas. Full split fills both cells with cover crops,
removes the gap and screen decorations, and preserves pixel aspect. Padded
split preserves the camera's selected aspect and project screen styling.
Auto split uses horizontal cells for landscape canvases and vertical cells
for portrait canvases. Camera aspect choices include original, square,
16:9, 9:16, 4:3 and 3:4. Framing, mirrored crop, drag snapping and zoom size
use the same geometry in handles, preview and export.
Paused preview and export decode native webcam pixels, including temporal
shutter samples; export also retains the webcam's native frame rate.
Interactive playback retains a 640-pixel webcam bound;
pausing promotes that proxy even when the playhead timestamp stays unchanged.

## Person cutout

Cutout runs the Apache-2.0 PPHumanSeg model from the official OpenCV Zoo,
locally through a bundled ONNX Runtime CPU backend on Linux x86_64, with
a portable Rust tract backend on other architectures or when the native
runtime cannot load. It does not contact a service
or open a capture device. The exact original model, license, upstream commit
and SHA-256 are in `assets/matting/`. The native runtime and its MIT license
and third-party notices are embedded in the executable. Runtime library
bytes are verified by SHA-256 and written atomically to a versioned user
cache; the license and notices are written beside the library. No system
ONNX installation or runtime download is required. The person mask is a semantic
segmentation probability, refined by guided enlargement against the source
image and adjustable edge softness/inset; it is not a manually drawn shape.
Fine hair, translucent objects and held objects remain subject to the
segmentation model's accuracy.

Inference runs in the existing renderer worker. Its model and a bounded
source-mask cache survive paused edits and temporal shutter samples.
Mask generation is independent from edge controls, crop and layout. Changing
edge controls reuses the neural mask. A separate cache retains up to four
full-resolution coverage frames, targeting 64 MiB while retaining at least
one frame for larger inputs, so entering or leaving Cutout only
blends its strength instead of repeating guided enlargement per shutter
sample. Both CPU and Vulkan composition receive
the same processed camera pixels. A cutout failure is surfaced by the preview
worker or export rather than represented as successful background removal.

The checked-in tract patch corrects empty optional Resize scales when sizes
is provided and avoids dynamic-index allocation in resize. Regression tests
compare its arithmetic with the original resize. The portable backend's final class
axis swap is NWHC (`[0,3,2,1]`); the optimized two-logit sigmoid explicitly
restores spatial orientation and is tested against the full unmodified
model's probabilities. No learned weights are changed.

## Verification

- Camera geometry and transition unit tests cover portrait/landscape,
  full/padded split, hidden fades, adjacent/overlapping transitions, source
  edits, resolution independence and serialization.
- Matting tests run the actual model on a blank frame and a public-domain
  NASA portrait, checking retained person, removed background, soft edges,
  inset/softness edits, original alpha, cache reuse and full-model parity.
- A 1280×720 at 60 fps fixture verifies native paused, export and
  temporal-camera dimensions and native export cadence independently of
  the bounded interactive decoder.
- The ignored display test
  `camera_layout_edit_move_trim_undo_and_reopen_workflow` exercises editor
  state and saved drafts.
- The ignored display test
  `timed_camera_cutout_and_blur_playback_stays_responsive` uses generated video
  and a moving public-domain portrait to exercise the real file transport and
  preview worker with timed layouts and motion blur. It records canvas size,
  advancing frames, preview lag and UI timer responsiveness. It never records
  a screen, camera or microphone.

On the development RTX 3060, the release-build X11/Xvfb playback fixture with
70% motion blur presented 100 distinct sampled frames over five seconds,
stayed within 200 ms of its transport clock, and kept the 25 ms UI polling
timer below 53 ms. The adaptive transition canvas was 784×441. These are measured
fixture results, not a guarantee for other video sizes or hardware; paused
preview and exported output retain full quality. Full split and Cutout
transitions do not allocate an artificial rectangular camera shadow.

Upstream references:

- https://github.com/opencv/opencv_zoo/tree/48b10fa505aeb51fbd127fc60a5c660040e823b7/models/human_segmentation_pphumanseg
- https://github.com/sonos/tract/tree/v0.22.5
- https://github.com/microsoft/onnxruntime/releases/tag/v1.31.0
- https://scikit-image.org/docs/stable/api/skimage.data.html#skimage.data.astronaut
