# Screendrop for Linux

This directory contains the Linux-first Rust and GPUI port of Screendrop. The
original macOS Swift application remains in `Screendrop/` as the behavioral and
visual reference.

## Capture shortcut

While Screendrop is running, press **Ctrl+Shift+3** from any application. Linux
opens its secure screenshot picker; after a screen, window, or area is selected,
Screendrop loads the result into the annotation editor and brings its window
forward. Wayland desktops show a one-time system dialog to approve or change the
shortcut.

GNOME requires a host build to have the matching
`com.screendrop.Screendrop.desktop` entry and a discoverable `screendrop`
executable. For a user-local development install:

```bash
cargo build
install -Dm755 target/debug/screendrop ~/.local/bin/screendrop
install -Dm644 packaging/com.screendrop.Screendrop.desktop \
  ~/.local/share/applications/com.screendrop.Screendrop.desktop
update-desktop-database ~/.local/share/applications
```

## Run

GPUI needs a Vulkan-capable Linux desktop and the standard Wayland/X11
runtime libraries. Both display backends are compiled so the same binary works
in native Wayland sessions and under X11/XWayland.

```bash
cargo run
```

## Native video recording

Video recording uses the desktop ScreenCast portal and its restricted
PipeWire file descriptor directly. Screendrop launches GStreamer only as its
encoder; it does not launch or reconfigure OBS. The Linux transport requires
`gst-launch-1.0`, the `pipewiresrc`, `vp8enc`, and `matroskamux` plugins, plus
FFmpeg for joining recordings across pause/resume.

On Debian/Ubuntu these are supplied by `gstreamer1.0-tools`,
`gstreamer1.0-pipewire`, `gstreamer1.0-plugins-good`, and FFmpeg. Recording
creates a `.screendroprec` package directly and keeps unfinished encoder
segments inside it for crash recovery.

The Record toolbar exposes separate System audio and Microphone toggles. When
enabled, the selected sources are mixed into an Opus track on the same media
clock as the screen stream.

Wayland cursor movement and click coordinates come from the compositor's
PipeWire cursor metadata, so they remain correct with display scaling and
pointer acceleration. The optional GNOME helper adds modifier/special-key
captions; install it and log out/in once so GNOME Shell discovers it:

```bash
gnome-extensions pack --force --out-dir /tmp packaging/gnome-shell-extension
gnome-extensions install --force \
  /tmp/screendrop-input@com.screendrop.shell-extension.zip
gnome-extensions enable screendrop-input@com.screendrop
```

The helper remains idle unless Screendrop creates its private runtime control
file. Plain typed text is never logged.

On Debian/Ubuntu, the complete native development dependency set includes
`libpipewire-0.3-dev`, `libspa-0.2-dev`, `libxkbcommon-dev`, and
`libxkbcommon-x11-dev`.
