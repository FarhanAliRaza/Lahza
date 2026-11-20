# Lahza (Rust/GPUI)

The app is named Lahza ("moment" in Urdu).

Linux-first screenshot and recording studio. The macOS Swift app in `Screendrop/` is a reference only — don't build it.

## System dependencies (Ubuntu)

```bash
sudo apt install -y libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev \
  libpipewire-0.3-dev libspa-0.2-dev libclang-dev \
  libxkbcommon-dev libxkbcommon-x11-dev
```

At runtime `ffmpeg`/`ffprobe` are required for recording preview, motion
export (MP4/WebM/GIF), and the export integration tests; `gst-play-1.0` is
needed for synchronized video playback in the editor. Optional: `whisper.cpp`
(`whisper-cli`, `whisper-stream`) for transcription and the teleprompter's
voice following, and `curl` for speech-model downloads.

## Build

```bash
cargo build --release
```

`just install` builds the release binary, stops any running instance, and
replaces `~/.local/bin/lahza`; use it after every change so the launched
app is the new build. `just run` does the same and launches the app.

## Install (desktop integration)

A plain `cargo install` is NOT enough: launching from the application menu
needs a `com.lahza.Lahza.desktop` entry and a `lahza` executable on PATH.
User-local install:

```bash
install -Dm755 target/release/lahza ~/.local/bin/lahza
install -Dm644 packaging/com.lahza.Lahza.desktop \
  ~/.local/share/applications/com.lahza.Lahza.desktop
install -Dm644 assets/Lahza.png \
  ~/.local/share/icons/hicolor/512x512/apps/com.lahza.Lahza.png
update-desktop-database ~/.local/share/applications
gtk-update-icon-cache -t ~/.local/share/icons/hicolor
```

## Optional: GNOME Shell extension (typed captions)

```bash
gnome-extensions pack --force --out-dir /tmp packaging/gnome-shell-extension
gnome-extensions install --force /tmp/lahza-input@com.lahza.shell-extension.zip
gnome-extensions enable lahza-input@com.lahza
```

Requires logging out and back in so GNOME Shell discovers it.

## Testing without a GPU

CI runs `cargo test --release --locked -- --test-threads=1`. On a machine
with no GPU (a container), the motion renderer's GPU and path-tracer tests
run on Mesa's software Vulkan:

```bash
sudo apt install -y mesa-vulkan-drivers libvulkan-dev xvfb \
  gstreamer1.0-plugins-base gstreamer1.0-plugins-good
LAHZA_MOTION_SOFTWARE_GPU=1 cargo test --release -p lahza-motion-render -- --test-threads=1
xvfb-run -a cargo test --release -p lahza -- --ignored --test-threads=1 motion_prompt_tests editor_window
```

- The window tests are the ignored ones of `motion_prompt_tests` and
  `editor_window_edits_and_reloads`; one of them drives the X pointer and
  needs `xdotool`. `a_real_request_is_sent_from_the_bar` sends a request
  to Claude Code, which costs money: it does nothing unless
  `LAHZA_REAL_AGENT=1` is set, and no other test's filter matches it.
- Without `LAHZA_MOTION_SOFTWARE_GPU=1` the GPU tests return early and
  pass without testing anything.
- Keep `--test-threads=1`: several software devices at once exhaust a
  15 GB machine, and on a real card parallel devices have crashed in the
  Vulkan loader.
- The render suite takes about 40 minutes there. Timings on software
  Vulkan say little about a real card: say which numbers are whose.
- `LAHZA_MOTION_VIS_REPORT=1` makes the GPU-against-CPU comparisons report
  where the frames differ instead of failing.

## Performance

Every editor interaction re-renders on the UI thread, so treat performance
as part of correctness when building features:

- Measure before optimizing: time the stages of a change at the preview
  canvas size (roughly 1080 px wide) and keep an interactive tick under
  ~16 ms. Anything dragged or animated must not rebuild work whose inputs
  did not change.
- `SceneCompositor` caches layers (background, vignette, watermark, card)
  and `rebuild` reuses those from a previous compositor; the preview renders
  a half-size proxy while a slider or the media is dragged. Extend these
  paths instead of adding per-frame full-canvas work.
- Blurs go through `blur_plane`, which downsamples for large sigmas; use it
  rather than new full-resolution passes.
- Prefer per-row loops over raw buffers to per-pixel `get_pixel`/`f64`
  math in hot loops, and keep the media paint (`paint_media`) in mind: it
  runs every playback frame.
- GPU images leak unless released explicitly. GPUI never frees a
  `RenderImage` from its sprite atlas when the `Arc` drops; only
  `Window::drop_image` does. Never overwrite an `Arc<RenderImage>` field
  directly: pass the old value to `Studio::retire_image`, which queues it
  for `drop_retired_images` at the top of `render`. A per-frame producer
  (webcam preview, video playback, scene preview) that skips this fills a
  12 GB card in about ten minutes and hard-freezes the Wayland session.
