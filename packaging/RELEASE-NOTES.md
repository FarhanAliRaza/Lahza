## Lahza v1.0.1

A native Linux screenshot, recording, and motion studio built with Rust and GPUI.

### Changed in this release

- Redesign the editor around one design system: Inter typography, consistent spacing, shared buttons, sliders, toggles, and fields, and an ink accent with red reserved for recording and the playhead.
- Add dark mode. Choose System, Light, or Dark under Appearance in the launcher, or cycle it from the editor's top bar; the choice is remembered.
- Improve keyboard access: visible focus, Tab navigation, arrow keys in segmented controls, `[` and `]` to trim to the playhead, Alt+arrow nudging, and global capture shortcuts (Ctrl+Alt+S, Ctrl+Alt+P) through the desktop portal.
- Rework the timeline: a readable time ruler, labelled regions, clearer trim handles, right-click menus, and a simpler toolbar with visible Snap and Skim states.
- Make destructive actions safer with two-step Discard and Restart and undo from notifications. Notifications now explain failures and offer a follow-up action.
- Show the current selection in the inspector header, simplify the export panel with a summary of what will be written, and widen the inspector on large windows.
- Remove the share-to-server upload. Export saves files locally.
- Keep sliders responsive while dragging: cursor and zoom sliders no longer rebuild motion timelines or re-read pointer data on every move.

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

**Ubuntu 24.04 amd64 — recommended:** download `lahza_1.0.1_amd64.deb`, then run:

```bash
sudo apt install ./lahza_1.0.1_amd64.deb
```

Launch **Lahza** from your app menu, or run `lahza`.

**Snap (alternative, amd64 Linux with snapd):**

```bash
sudo snap install lahza
```

The stable channel receives this version after the release workflow and Store review succeed. Camera and audio recording require additional permission setup. Lahza now prompts when access is missing: choose Copy commands, paste and run them in Terminal, then return and choose Try again.

**Binary bundle:** download `lahza-1.0.1-linux-x86_64.tar.gz`, then run:

```bash
tar -xzf lahza-1.0.1-linux-x86_64.tar.gz
cd lahza-1.0.1-linux-x86_64
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
