#!/bin/bash
# Run through the installed Snap's command chain; see SNAP.md.
set -euo pipefail
: "${SNAP:?Run this script inside the Lahza Snap environment}"

"$SNAP/bin/lahza" --version
test -d "$LAHZA_ASSETS/icons"
test -f "$PIPEWIRE_CONFIG_DIR/client.conf"
test -f "$PIPEWIRE_MODULE_DIR/libpipewire-module-client-node.so"
test -d "$SPA_PLUGIN_DIR/support"
command -v ffmpeg
command -v ffprobe
command -v gst-play-1.0

for element in pipewiresrc pulsesrc v4l2src vp8enc opusenc matroskamux \
  avenc_ffv1 avdec_ffv1 capssetter videocrop videoscale videobox \
  videoconvert audioconvert audioresample appsink tee; do
  gst-inspect-1.0 "$element" >/dev/null
done

smoke_dir="$(mktemp -d)"
trap 'rm -rf "$smoke_dir"' EXIT

# Window recordings require FFV1 and must retain transparent pixels.
gst-launch-1.0 -q -e videotestsrc num-buffers=3 pattern=ball background-color=0x00000000 \
  ! video/x-raw,format=BGRA,width=64,height=64,framerate=30/1 \
  ! avenc_ffv1 ! matroskamux ! filesink location="$smoke_dir/window.mkv"
ffmpeg -nostdin -v error -i "$smoke_dir/window.mkv" -frames:v 1 \
  -pix_fmt rgba -f rawvideo "$smoke_dir/window.rgba"
test "$(wc -c < "$smoke_dir/window.rgba")" -eq 16384
test "$(od -An -tu1 -j3 -N1 "$smoke_dir/window.rgba" | tr -d ' ')" = 0

# Exercise the recording codecs using synthetic sources, without capturing
# the desktop, microphone, or camera.
gst-launch-1.0 -q -e \
  matroskamux name=mux ! filesink location="$smoke_dir/recording.mkv" \
  videotestsrc num-buffers=30 ! video/x-raw,width=320,height=180,framerate=30/1 \
    ! videoconvert ! vp8enc deadline=1 ! queue ! mux. \
  audiotestsrc num-buffers=50 ! audioconvert ! audioresample ! opusenc ! queue ! mux.

ffmpeg -nostdin -v error -i "$smoke_dir/recording.mkv" \
  -c:v libx264 -pix_fmt yuv420p -c:a aac "$smoke_dir/export.mp4"
ffprobe -v error -show_entries stream=codec_name,width,height \
  -of compact "$smoke_dir/export.mp4"
ffmpeg -nostdin -v error -i "$smoke_dir/export.mp4" -frames:v 1 "$smoke_dir/frame.png"
test -s "$smoke_dir/frame.png"
echo 'Snap runtime smoke test passed.'
