# Screendrop (Rust/GPUI)

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

## Install (desktop integration)

A plain `cargo install` is NOT enough: the global capture shortcut (Ctrl+Shift+3)
only works when GNOME can match the running app to a
`com.screendrop.Screendrop.desktop` entry and find a `screendrop` executable on
PATH. User-local install:

```bash
install -Dm755 target/release/screendrop ~/.local/bin/screendrop
install -Dm644 packaging/com.screendrop.Screendrop.desktop \
  ~/.local/share/applications/com.screendrop.Screendrop.desktop
install -Dm644 Screendrop.png \
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
