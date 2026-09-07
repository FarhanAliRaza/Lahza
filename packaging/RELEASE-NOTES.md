## Lahza v0.5.2

A native Linux screenshot, recording, and motion studio built with Rust and GPUI.

### Changed in this release

- MP4 exports automatically detect working NVIDIA NVENC or Intel/AMD VAAPI encoders, with a faster CPU fallback when hardware encoding is unavailable.
- Edited recordings and webcam clips stream directly into export, removing the full intermediate-video preparation pass. Audio edits are applied during final encoding.
- Export runs in the background with a collapsible progress panel, filename, encoder status, estimated time remaining, and cancellation. Continue editing and previewing while the original project snapshot renders.
- Cached webcam shadows avoid repeating a full-canvas blur on every frame.
- Webcam playback reuses a decoder, pairs camera frames with screen timestamps, and waits for camera readiness before starting audio. A regular playback cadence keeps webcams and motion smooth when the screen recording has sparse frames.
- Timeline playback applies cuts and speed changes without rebuilding preview videos, preserves audio pitch, and reuses playback across unchanged splits.
- Clip edits keep media annotations aligned with retained footage and preserve annotation state for undo. Deleted motion regions no longer leave a held transform on later clips.

Validation: 160 automated Rust tests and 15 packaging tests passed locally; five manual/device-dependent Rust tests remain excluded from the default suite. Added regression tests for sparse screen recordings, repeated playback starts, webcam synchronization, export streaming, and time estimates.

Snap builds are installed and tested under strict confinement in CI, including synthetic recording, H.264/AAC export, and frame decoding. Desktop source-picker and device behavior still depend on your Linux desktop.

For Debian updates, run `lahza-update --install`. Snap updates are managed by snapd.

### Included

- Screenshot capture and annotation: shapes, arrows, text, numbered steps, highlights, blur, and pixelation.
- Screen recording with pause/resume, microphone and system audio, editable projects, and draft recovery.
- Clip trimming, splitting, speed changes, motion regions, and timed annotations.
- Styled backgrounds, window frames, shadows, borders, watermarks, and 3D media transforms.
- Animated screenshots, image sequences, cursor walkthroughs, and eight scene templates.
- PNG, MP4, WebM, and GIF exports.

### Download and install

**Ubuntu 24.04 amd64 — recommended:** download `lahza_0.5.2_amd64.deb`, then run:

```bash
sudo apt install ./lahza_0.5.2_amd64.deb
```

Launch **Lahza** from your app menu, or run `lahza`.

**Snap (alternative, amd64 Linux with snapd):**

```bash
sudo snap install lahza
```

The stable channel receives this version after the release workflow and Store review succeed. Camera and audio recording require additional permission setup. Lahza now prompts when access is missing: choose Copy commands, paste and run them in Terminal, then return and choose Try again.

**Binary bundle:** download `lahza-0.5.2-linux-x86_64.tar.gz`, then run:

```bash
tar -xzf lahza-0.5.2-linux-x86_64.tar.gz
cd lahza-0.5.2-linux-x86_64
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

Desktop capture support depends on your compositor and portals. Editable cursor effects and automatic click zooms depend on input metadata; the optional bundled GNOME helper supplies additional input information. Webcam availability and GPU encoding depend on device permissions and installed drivers. Recovery cannot guarantee that every interrupted recording is salvageable. Automated checks do not replace hands-on testing across Linux desktops.

See the [README](https://github.com/FarhanAliRaza/lahza#readme) for features, source installation, and GNOME helper setup. Please report issues with your distribution, desktop, session type, and reproduction steps.
