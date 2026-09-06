## Lahza v0.4.4

A native Linux screenshot, recording, and motion studio built with Rust and GPUI.

### Changed in this release

- Added eight shadow colors across Soft, Long, Glow, and Crisp styles, with matching preview and export rendering.
- Saved presets and projects now retain the shadow color; older projects keep their original black shadows.
- Fixed dragging the Shadow Amount control after its label changed. Sliders now use stable targets independent of their display labels.

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

**Snap (amd64 Linux with snapd):**

```bash
sudo snap install lahza
```

The stable channel receives this version after the release workflow and Store review succeed. Microphone capture and webcam access require `sudo snap connect lahza:audio-record` and `sudo snap connect lahza:camera` respectively.

**Ubuntu 24.04 amd64 — recommended:** download `lahza_0.4.4_amd64.deb`, then run:

```bash
sudo apt install ./lahza_0.4.4_amd64.deb
```

Launch **Lahza** from your app menu, or run `lahza`.

**Binary bundle:** download `lahza-0.4.4-linux-x86_64.tar.gz`, then run:

```bash
tar -xzf lahza-0.4.4-linux-x86_64.tar.gz
cd lahza-0.4.4-linux-x86_64
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

Desktop capture support depends on your compositor and portals. Editable cursor effects and automatic click zooms depend on input metadata; the optional bundled GNOME helper supplies additional input information. Live webcam capture/device selection is unfinished; camera-file overlays are supported. Recovery cannot guarantee that every interrupted recording is salvageable. Automated checks do not replace hands-on testing across Linux desktops.

See the [README](https://github.com/FarhanAliRaza/lahza#readme) for features, source installation, and GNOME helper setup. Please report issues with your distribution, desktop, session type, and reproduction steps.
