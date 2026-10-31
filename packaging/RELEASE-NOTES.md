## Lahza v2.0.0

A native Linux screenshot, recording, and motion studio built with Rust and GPUI.

### Changed in this release

- Replace the microphone presets with **Enhance audio**, which runs NVIDIA's RE-USE speech enhancement model on the GPU (any Vulkan GPU) to remove background noise and room echo, then levels the voice. The model is bundled and runs locally; without a GPU it falls back to the CPU, very slowly, with a warning. The timeline waveform switches to the cleaned audio once it is ready. Projects saved with Standard or Loud open as Off; Noise reduction and Podcast open as Enhance audio.
- Animate camera layout edits. Switching layout, position, shape, or size, and undo/redo, now eases from the camera on screen instead of jumping. Dragging still follows the pointer, and releasing a drag eases into the snapped spot. Respects GNOME's Animations setting.
- Split screen now fills its half of the canvas instead of shrinking the recording to fit, cropping the overflow. On a 9:16 canvas the screen is no longer a thin strip.
- Add **Avoid empty area** to the Overlay and Cutout camera layouts. It appears when fitting the whole recording would leave background beside it, such as a landscape recording on 9:16, and crops the recording to fill the canvas while the camera follows your cursor. It is the same setting as Fill under Style → Canvas.
- Keep the preview responsive while dragging. Resizing or moving the recording on the canvas, scrubbing sliders, and dragging the camera no longer cancel every preview frame, so the canvas follows the pointer instead of catching up when you pause.
- Upscale zoomed screen footage with a new model trained on interface content.

The release workflow reruns the Rust and packaging suites, tests the bundled Whisper runtime, and runs confined Snap smoke checks.

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

**Ubuntu 24.04 amd64 — recommended:** download `lahza_2.0.0_amd64.deb`, then run:

```bash
sudo apt install ./lahza_2.0.0_amd64.deb
```

Launch **Lahza** from your app menu, or run `lahza`.

**Snap (alternative, amd64 Linux with snapd):**

```bash
sudo snap install lahza
```

The stable channel receives this version after the release workflow and Store review succeed. Camera and audio recording require additional permission setup. Lahza now prompts when access is missing: choose Copy commands, paste and run them in Terminal, then return and choose Try again.

**Binary bundle:** download `lahza-2.0.0-linux-x86_64.tar.gz`, then run:

```bash
tar -xzf lahza-2.0.0-linux-x86_64.tar.gz
cd lahza-2.0.0-linux-x86_64
./install.sh
```

This installs the binary, bundled Whisper runtime, assets, desktop entry, and icon into `~/.local`. Whisper transcription models download during speech setup; microphone enhancement and camera cutout models are included. Ensure `~/.local/bin` is on `PATH`. You can also run `./bin/lahza` from the extracted bundle. The bundle uses system libraries; it is not a statically linked or universally portable build.

On Ubuntu, install the bundle's runtime dependencies with:

```bash
sudo apt install ffmpeg gstreamer1.0-pipewire gstreamer1.0-plugins-good \
  gstreamer1.0-tools gstreamer1.0-libav gstreamer1.0-plugins-base-apps libgstreamer1.0-0 \
  libpipewire-0.3-0 libx11-6 libxkbcommon-x11-0 libxkbcommon0 \
  libsdl2-2.0-0 libvulkan1 libgomp1 xdg-desktop-portal
```

A Vulkan-capable desktop, PipeWire, and the portal backend for your desktop are required. Packages are built on Ubuntu 24.04 for x86_64; older distributions may need a source build.

To verify downloaded files, download `SHA256SUMS` into the same directory and run `sha256sum --ignore-missing -c SHA256SUMS`.

### Known limitations

Enhance audio needs a Vulkan GPU for practical speed; on the CPU it is very slow. Completed audio is cached for playback and export; progress and cancellation remain available during preparation. Older recordings with already-mixed microphone and system audio cannot be separated, so cleanup processes their combined track. Confined Snap smoke checks do not yet exercise neural inference.

Window recordings use lossless FFV1 to preserve pixels and transparency, so their project files can be larger. A window that grows beyond the recording's initial capture surface stops recording rather than being silently reduced or clipped. Shadow-margin detection relies on alpha edges; ambiguous shapes retain their visible bounds. Layout fixes also apply to existing alpha-capable window recordings, but detail lost through older capture-time downscaling cannot be restored.

Desktop capture support depends on your compositor and portals. Editable cursor effects and automatic click zooms depend on input metadata; the optional bundled GNOME helper supplies additional input information. Webcam availability and GPU encoding depend on device permissions and installed drivers. Recovery cannot guarantee that every interrupted recording is salvageable. Automated checks do not replace hands-on testing across Linux desktops.

See the [README](https://github.com/FarhanAliRaza/lahza#readme) for features, source installation, and GNOME helper setup. Please report issues with your distribution, desktop, session type, and reproduction steps.
