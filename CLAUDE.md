# Lahza (Rust/GPUI)

The app is named Lahza ("moment" in Urdu); the crate, binary, app id, and
session extension keep the historical `screendrop` identifiers.

Linux-first screenshot and recording studio. The macOS Swift app in `Screendrop/` is a reference only — don't build it.

## System dependencies (Ubuntu)

```bash
sudo apt install -y libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev \
  libpipewire-0.3-dev libspa-0.2-dev libclang-dev \
  libxkbcommon-dev libxkbcommon-x11-dev
```

At runtime `ffmpeg`/`ffprobe` are required for recording preview, motion
export (MP4/WebM/GIF), and the export integration tests; `gst-play-1.0` is
needed for synchronized video playback in the editor.

## Build

```bash
cargo build --release
```

`just install` builds the release binary, stops any running instance, and
replaces `~/.local/bin/screendrop`; use it after every change so the launched
app is the new build. `just run` does the same and launches the app.

## Install (desktop integration)

A plain `cargo install` is NOT enough: the global capture shortcut (Ctrl+Shift+3)
only works when GNOME can match the running app to a
`com.screendrop.Screendrop.desktop` entry and find a `screendrop` executable on
PATH. User-local install:

```bash
install -Dm755 target/release/screendrop ~/.local/bin/screendrop
install -Dm644 packaging/com.screendrop.Screendrop.desktop \
  ~/.local/share/applications/com.screendrop.Screendrop.desktop
install -Dm644 Lahza.png \
  ~/.local/share/icons/hicolor/512x512/apps/com.screendrop.Screendrop.png
update-desktop-database ~/.local/share/applications
gtk-update-icon-cache -t ~/.local/share/icons/hicolor
```

Wayland shows a one-time system dialog to approve the shortcut on first use.

## Optional: GNOME Shell extension (typed captions)

```bash
gnome-extensions pack --force --out-dir /tmp packaging/gnome-shell-extension
gnome-extensions install --force /tmp/screendrop-input@com.screendrop.shell-extension.zip
gnome-extensions enable screendrop-input@com.screendrop
```

Requires logging out and back in so GNOME Shell discovers it.

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
