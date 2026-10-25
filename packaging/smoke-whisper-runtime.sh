#!/usr/bin/env bash
# Exercise a relocated runtime against known speech without recording devices.
set -euo pipefail
if [[ $# -ne 3 ]]; then
  echo 'Usage: smoke-whisper-runtime.sh RUNTIME MODEL WAV' >&2
  exit 2
fi
runtime="$(realpath -- "$1")"
model="$(realpath -- "$2")"
sample="$(realpath -- "$3")"
smoke_dir="$(mktemp -d)"
trap 'rm -rf -- "$smoke_dir"' EXIT
cp -a "$runtime" "$smoke_dir/whisper"

# Running away from both source and install directories verifies RPATH and
# relative CPU-backend discovery. Explicit CPU mode must not initialize GPU.
cd "$smoke_dir"
ldd "$smoke_dir/whisper/bin/whisper-cli" > libraries.txt
if grep -E '(libwhisper|libggml).*=>' libraries.txt | grep -vF "$smoke_dir/whisper"; then
  echo 'Runtime resolved Whisper/ggml libraries outside its own bundle' >&2
  exit 1
fi
env -u GGML_BACKEND_PATH ./whisper/bin/whisper-cli \
  -ng -m "$model" -f "$sample" -l en -nf -nt -t 2 \
  > transcript.txt 2> diagnostics.txt
grep -q 'loaded CPU backend' diagnostics.txt
if grep -q 'using Vulkan.* backend' diagnostics.txt; then
  echo 'CPU mode unexpectedly initialized Vulkan' >&2
  exit 1
fi
grep -qi 'ask not what your country can do for you' transcript.txt
# An installed plugin without a working graphics driver must still leave CPU
# inference usable. This also runs on CI machines without a physical GPU.
GGML_BACKEND_PATH="$smoke_dir/whisper/lib/libggml-vulkan.so" \
  VK_ICD_FILENAMES="$smoke_dir/no-driver.json" \
  VK_DRIVER_FILES="$smoke_dir/no-driver.json" \
  ./whisper/bin/whisper-cli -m "$model" -f "$sample" -l en -nf -nt -t 2 \
  > fallback-transcript.txt 2> fallback-diagnostics.txt
if grep -q 'using Vulkan.* backend' fallback-diagnostics.txt; then
  echo 'A nonexistent Vulkan driver unexpectedly enabled GPU inference' >&2
  exit 1
fi
grep -qi 'ask not what your country can do for you' fallback-transcript.txt
env -u GGML_BACKEND_PATH ./whisper/bin/whisper-stream --help > stream-help.txt 2>&1
grep -q 'loaded CPU backend' stream-help.txt
echo 'Relocated Whisper CPU speech, missing-driver fallback and streaming startup tests passed.'
