#!/usr/bin/env bash
# Build a relocatable, matched Whisper/ggml runtime. Never mix these libraries
# with a distribution's ggml plugins: their backend ABI can differ.
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo 'Usage: packaging/build-whisper-runtime.sh OUTPUT_DIRECTORY' >&2
  exit 2
fi
runtime_output="$(realpath -m -- "$1")"
runtime_work="${LAHZA_WHISPER_BUILD_DIR:-$(mktemp -d)}"
mkdir -p "$runtime_work"
runtime_work="$(realpath -- "$runtime_work")"
if [[ -z "${LAHZA_WHISPER_BUILD_DIR:-}" ]]; then
  trap 'rm -rf -- "$runtime_work"' EXIT
fi

# whisper.cpp v1.8.3, including the ggml revision shipped in that source tree.
source_commit=2eeeba56e9edd762b4b38467bab96c2517163158
source_sha256=089b898aa83b24a8321e0fd554eeb0967fb03dd687e27f6374c72d3363b5b429
source_archive="$runtime_work/whisper-source.tar.gz"
if [[ ! -f "$source_archive" ]]; then
  curl --fail --location --silent --show-error --retry 3 \
    "https://codeload.github.com/ggml-org/whisper.cpp/tar.gz/$source_commit" \
    --output "$source_archive"
fi
printf '%s  %s\n' "$source_sha256" "$source_archive" | sha256sum --check --status
tar -xzf "$source_archive" -C "$runtime_work"
source_dir="$runtime_work/whisper.cpp-$source_commit"
build_dir="$runtime_work/build"

cmake -S "$source_dir" -B "$build_dir" \
  -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_INSTALL_LIBDIR=lib \
  '-DCMAKE_INSTALL_RPATH=$ORIGIN;$ORIGIN/../lib' \
  -DCMAKE_BUILD_WITH_INSTALL_RPATH=ON \
  -DBUILD_SHARED_LIBS=ON \
  -DWHISPER_BUILD_TESTS=OFF \
  -DWHISPER_BUILD_SERVER=OFF \
  -DWHISPER_SDL2=ON \
  -DGGML_BACKEND_DL=ON \
  -DGGML_NATIVE=OFF \
  -DGGML_CPU_ALL_VARIANTS=ON \
  -DGGML_VULKAN=ON
cmake --build "$build_dir" --config Release \
  --parallel "${CMAKE_BUILD_PARALLEL_LEVEL:-2}" --target whisper-cli whisper-stream

mkdir -p "$runtime_output/bin" "$runtime_output/lib"
install -m755 "$build_dir/bin/whisper-cli" "$build_dir/bin/whisper-stream" "$runtime_output/bin/"
cp -a "$build_dir/src/"libwhisper.so* "$runtime_output/lib/"
cp -a "$build_dir/ggml/src/"libggml.so* "$build_dir/ggml/src/"libggml-base.so* "$runtime_output/lib/"
cp -a "$build_dir/bin/"libggml-cpu*.so "$runtime_output/bin/"
# CPU plugins are discovered beside the executables. Vulkan is deliberately
# outside that directory: CPU mode must not initialize a broken GPU driver.
install -m755 "$build_dir/bin/libggml-vulkan.so" "$runtime_output/lib/"
install -m644 "$source_dir/LICENSE" "$runtime_output/LICENSE.whisper.cpp"
if [[ -f "$source_dir/ggml/LICENSE" ]]; then
  install -m644 "$source_dir/ggml/LICENSE" "$runtime_output/LICENSE.ggml"
fi
printf '{"whisperVersion":"1.8.3","sourceCommit":"%s","sourceSha256":"%s","backend":"Vulkan","architecture":"%s"}\n' \
  "$source_commit" "$source_sha256" "$(uname -m)" > "$runtime_output/manifest.json"

# Help initializes backend discovery, catching misplaced CPU plugins and
# missing shared-library dependencies without opening a microphone.
env -u GGML_BACKEND_PATH "$runtime_output/bin/whisper-cli" --help > "$runtime_work/cli-help.log" 2>&1
env -u GGML_BACKEND_PATH "$runtime_output/bin/whisper-stream" --help > "$runtime_work/stream-help.log" 2>&1
grep -q 'loaded CPU backend' "$runtime_work/cli-help.log"
grep -q 'loaded CPU backend' "$runtime_work/stream-help.log"
printf 'Built Whisper CPU/Vulkan runtime in %s\n' "$runtime_output"
