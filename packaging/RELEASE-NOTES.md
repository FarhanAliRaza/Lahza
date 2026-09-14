## Lahza v0.5.9

A native Linux screenshot, recording, and motion studio built with Rust and GPUI.

### Changed in this release

- Keep the timeline scale and parked playhead stable while trimming. Retained frames and neighboring clips stay at their existing times; trimmed space becomes a gap instead of shifting the video.
- Snap trim handles to the playhead so the chosen frame remains an exact trim target. Short clips retain a draggable center between their trim handles.
- Scroll beyond the recording and place clips elsewhere on the timeline without automatically fitting the video to the viewport. Audio, camera, motion, and annotation lanes use the same time scale.
- Click a clip to select it and move the playhead on release, matching Cap's click behavior. Drag the clip body to reposition it without scrubbing, or drag the ruler to scrub.
- Finish clip moves when released outside the clip, preserve undo/redo and saved placement, and keep minimum-length trims intact when saving.

Validation includes automated clip and timeline regression tests plus native X11 mouse interaction under Xvfb, covering click-to-seek, clip dragging, undo/redo, and persisted placement. The release workflow runs the Rust and packaging suites and confined Snap smoke checks for the versioned packages.

Snap builds are installed and tested under strict confinement in CI, including synthetic window recording with alpha, H.264/AAC export, and frame decoding. Desktop source-picker and device behavior still depend on your Linux desktop.

For Debian updates, run `lahza-update --install`. Snap updates are managed by snapd.

### Included

- Screenshot capture and annotation: shapes, arrows, text, numbered steps, highlights, blur, and pixelation.
- Screen recording with pause/resume, microphone and system audio, editable projects, and draft recovery.
- Video cropping, clip trimming, splitting, speed changes, motion regions, and timed annotations.
- Styled backgrounds, window frames, shadows, borders, watermarks, and 3D media transforms.
- Animated screenshots, image sequences, cursor walkthroughs, and eight scene templates.
- PNG, MP4, WebM, and GIF exports.

### Download and install

**Ubuntu 24.04 amd64 — recommended:** download `lahza_0.5.9_amd64.deb`, then run:

```bash
sudo apt install ./lahza_0.5.9_amd64.deb
```

Launch **Lahza** from your app menu, or run `lahza`.

**Snap (alternative, amd64 Linux with snapd):**

```bash
sudo snap install lahza
```

The stable channel receives this version after the release workflow and Store review succeed. Camera and audio recording require additional permission setup. Lahza now prompts when access is missing: choose Copy commands, paste and run them in Terminal, then return and choose Try again.

**Binary bundle:** download `lahza-0.5.9-linux-x86_64.tar.gz`, then run:

```bash
tar -xzf lahza-0.5.9-linux-x86_64.tar.gz
cd lahza-0.5.9-linux-x86_64
./install.sh
```

This installs the binary, assets, desktop entry, and icon into `~/.local`. Ensure `~/.local/bin` is on `PATH`. You can also run `./bin/lahza` from the extracted bundle. The bundle uses system libraries; it is not a statically linked or universally portable build.

On Ubuntu, install the bundle's runtime dependencies with:

```bash
sudo apt install ffmpeg gstreamer1.0-pipewire gstreamer1.0-plugins-good \
  gstreamer1.0-tools gstreamer1.0-libav gstreamer1.0-plugins-base-apps libgstreamer1.0-0 \
  libpipewire-0.3-0 libx11-6 libxkbcommon-x11-0 libxkbcommon0 xdg-desktop-portal
```

A Vulkan-capable desktop, PipeWire, and the portal backend for your desktop are required. Packages are built on Ubuntu 24.04 for x86_64; older distributions may need a source build.

To verify downloaded files, download `SHA256SUMS` into the same directory and run `sha256sum --ignore-missing -c SHA256SUMS`.

### Early-release limitations

Window recordings use lossless FFV1 to preserve pixels and transparency, so their project files can be larger. A window that grows beyond the recording's initial capture surface stops recording rather than being silently reduced or clipped. Shadow-margin detection relies on alpha edges; ambiguous shapes retain their visible bounds. Layout fixes also apply to existing alpha-capable window recordings, but detail lost through older capture-time downscaling cannot be restored.

Desktop capture support depends on your compositor and portals. Editable cursor effects and automatic click zooms depend on input metadata; the optional bundled GNOME helper supplies additional input information. Webcam availability and GPU encoding depend on device permissions and installed drivers. Recovery cannot guarantee that every interrupted recording is salvageable. Automated checks do not replace hands-on testing across Linux desktops.

See the [README](https://github.com/FarhanAliRaza/lahza#readme) for features, source installation, and GNOME helper setup. Please report issues with your distribution, desktop, session type, and reproduction steps.
