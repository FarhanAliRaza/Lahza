## Lahza v1.0.0

A native Linux screenshot, recording, and motion studio built with Rust and GPUI.

### Changed in this release

- Add five microphone presets: Off, Standard, Loud, Noise reduction, and Podcast. Noise reduction uses bundled MossFormer2 inference; Podcast uses bundled TF-Restormer restoration with voice leveling, EQ, and de-essing. Processing runs locally without downloading enhancement models.
- Keep microphone and system audio as separate tracks in new recordings, so microphone cleanup leaves system audio unaffected. Preserve both tracks through pause/resume, finalization, and interrupted-recording recovery.
- Prepare audio once in the background and share the lossless result between playback and export. Add preparation progress, cancellation, retry, and caches for each source and preset. Fix audio changes at normal export speed and shared neural-runtime initialization.
- Add timed webcam regions with animated transitions between Overlay, Split screen, Camera only, Screen only, and Cutout layouts. Support full or padded splits, portrait and landscape arrangements, camera aspect ratios, and local neural person cutout.
- Preserve native webcam detail in paused previews and exports. Improve enlarged screen text with independently trained 2× and 4× UI reconstruction models, alpha-aware sampling, and cached GPU rendering.
- Add a dedicated Animation panel with hover previews for screen and cursor presets, adjustable motion blur, and a project-wide option to start zooms early.
- Improve Glass loupe and motion-blur playback with shared frame decoding and cancellation of obsolete requests. Refine cursor smoothing, hover handling, idle fade, and independent typing-based cursor hiding.

Validation before release passed 510 Rust tests, with model and editor lifecycle probes run separately. Neural output was compared with independent references on public speech fixtures; rendering checks cover native camera detail and GPU/CPU sampling. The release workflow reruns the Rust and packaging suites, tests the bundled Whisper runtime, and runs confined Snap smoke checks. Compilation jobs are limited to one at a time.

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

**Ubuntu 24.04 amd64 — recommended:** download `lahza_1.0.0_amd64.deb`, then run:

```bash
sudo apt install ./lahza_1.0.0_amd64.deb
```

Launch **Lahza** from your app menu, or run `lahza`.

**Snap (alternative, amd64 Linux with snapd):**

```bash
sudo snap install lahza
```

The stable channel receives this version after the release workflow and Store review succeed. Camera and audio recording require additional permission setup. Lahza now prompts when access is missing: choose Copy commands, paste and run them in Terminal, then return and choose Try again.

**Binary bundle:** download `lahza-1.0.0-linux-x86_64.tar.gz`, then run:

```bash
tar -xzf lahza-1.0.0-linux-x86_64.tar.gz
cd lahza-1.0.0-linux-x86_64
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

### Early-release limitations

Podcast preparation is expensive on CPU: measured public fixtures took roughly 12–24 seconds per second of audio, with peak processing memory up to about 2.4 GiB. Completed audio is cached for playback and export; progress and cancellation remain available during preparation. These measurements are hardware- and input-dependent. Older recordings with already-mixed microphone and system audio cannot be separated, so cleanup processes their combined track. Neural audio is supported on Linux x86_64; confined Snap smoke checks do not yet exercise neural inference.

Window recordings use lossless FFV1 to preserve pixels and transparency, so their project files can be larger. A window that grows beyond the recording's initial capture surface stops recording rather than being silently reduced or clipped. Shadow-margin detection relies on alpha edges; ambiguous shapes retain their visible bounds. Layout fixes also apply to existing alpha-capable window recordings, but detail lost through older capture-time downscaling cannot be restored.

Desktop capture support depends on your compositor and portals. Editable cursor effects and automatic click zooms depend on input metadata; the optional bundled GNOME helper supplies additional input information. Webcam availability and GPU encoding depend on device permissions and installed drivers. Recovery cannot guarantee that every interrupted recording is salvageable. Automated checks do not replace hands-on testing across Linux desktops.

See the [README](https://github.com/FarhanAliRaza/lahza#readme) for features, source installation, and GNOME helper setup. Please report issues with your distribution, desktop, session type, and reproduction steps.
