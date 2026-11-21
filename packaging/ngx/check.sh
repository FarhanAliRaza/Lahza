#!/bin/sh
# What `just ngx` needs, and what of it is missing: the DLSS SDK checkout
# ($1, default ignore/DLSS) and Vulkan's headers ($2, default
# ignore/Vulkan-Headers/include). Exit 1 when something is missing.
sdk="${1:-ignore/DLSS}"
headers="${2:-ignore/Vulkan-Headers/include}"
missing=0
need() {
    # (A pattern: the Ray Reconstruction library carries its version.)
    for f in $1; do
        [ -e "$f" ] && return 0
    done
    echo "missing: $1  ($2)"
    missing=1
}
need "$sdk/include/nvsdk_ngx_vk.h" "the DLSS SDK: git clone https://github.com/NVIDIA/DLSS $sdk"
need "$sdk/include/nvsdk_ngx_helpers_vk.h" "the DLSS SDK"
need "$sdk/include/nvsdk_ngx_helpers_dlssd_vk.h" "the DLSS SDK, a version with Ray Reconstruction (DLSS-D)"
need "$sdk/lib/Linux_x86_64/libnvsdk_ngx.a" "the SDK's static library, linked into the shim"
need "$sdk/lib/Linux_x86_64/rel/libnvidia-ngx-dlssd.so.*" "NVIDIA's Ray Reconstruction library (the release one, not dev)"
need "$headers/vulkan/vulkan.h" "Vulkan's headers: git clone https://github.com/KhronosGroup/Vulkan-Headers, or headers=/usr/include with libvulkan-dev"
command -v g++ >/dev/null 2>&1 || { echo "missing: g++"; missing=1; }
[ "$missing" = 0 ] && echo "everything \`just ngx\` needs is there"
exit $missing
