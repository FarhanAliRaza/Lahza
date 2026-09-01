# Screendrop for Linux

This directory contains the Linux-first Rust and GPUI port of Screendrop. The
original macOS Swift application remains in `Screendrop/` as the behavioral and
visual reference.

## Download a Debian package

Every push to `main` produces a `.deb` in the GitHub Actions run artifacts.
Pushing a version tag such as `v0.1.0` also creates a GitHub Release and attaches
the package to it. Install a downloaded package on Debian or Ubuntu with:

```bash
sudo apt install ./screendrop_0.1.0_amd64.deb
```

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

## Motion editing and export

Screenshots and recordings share one scene model: a background, the media
surface (padding, corners, border, shadow), and a camera. Camera motion is
edited as orange regions on the timeline's motion lane rather than keyframes:

- Recordings open with regions synthesized from clicks. Double-click the lane
  to add one, drag its edges to retime it, and use the inspector to change
  its style (hold, zoom in, zoom out), magnification, target (cursor, auto,
  pinned), focus point, and pan destination. Click the video to set the
  focus.
- Screenshots start static. **Animate** turns the capture into a 3–10 second
  scene with presets (slow zoom, pan, focus, sweep) that expand into the same
  editable regions.

The media surface is a 3D object: click it to select it, drag to move,
Shift-drag to tilt, Ctrl-drag to spin, scroll to scale, and double-click to
reset; the Transform panel exposes scale, position, X/Y/Z rotation,
perspective, and anchor with per-value reset plus Fit, Fill, and Actual size.
Backgrounds gain blur, grain, and vignette, and a text watermark can sit in
any corner. Recordings also get a pointer panel (cursor size, shadow, idle
hiding, click effects and colour, removable clicks), an audio lane with a
mute toggle, and clip thumbnails. Annotations in an animated screenshot are
timed (draggable lane, entrance and exit effects such as draw-on and
type-on). The inspector has Quick, Customize, and Advanced levels, and any
look can be saved to a personal preset library.

Export renders the whole scene, not only the source clip, with the same
compositor that drives the preview (the preview *is* the compositor's
output). MP4 (H.264/AAC), WebM (VP9/Opus), and looping GIF are available for
both recordings and animated screenshots at original, 720p, 1080p, 1440p, or
4K, 30 or 60 fps, with a size estimate; FFmpeg is required and progress can
be cancelled.

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
