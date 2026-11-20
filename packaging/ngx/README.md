# The Ray Reconstruction shim

`lahza_ngx.cpp` is Lahza's C surface over NVIDIA's NGX SDK (DLSS Ray
Reconstruction). The renderer loads it at run time when it is there
(`crates/motion-render/src/ngx.rs`): `$LAHZA_NGX`, else
`~/.local/lib/lahza/liblahza_ngx.so`, with NVIDIA's own library beside it.
Without it, or off NVIDIA, the fast path-traced mode (`--fast`) still
runs: it accumulates and denoises a few paths a pixel itself, and a
warning says so.

Nothing of NVIDIA's is in this repository, and no build of Lahza
(`cargo build`, `just install`, the snap) makes or ships the shim.

## Building it

`just ngx` (an NVIDIA RTX card and its driver are needed to use it, not to
build it). `sh packaging/ngx/check.sh [sdk] [headers]` lists what is
missing. It needs:

- **The DLSS SDK**, a version with Ray Reconstruction (DLSS-D), checked
  out at `ignore/DLSS` (`git clone https://github.com/NVIDIA/DLSS
  ignore/DLSS`; `ignore/` is not committed). Of it:
  - `include/nvsdk_ngx_vk.h`, `nvsdk_ngx_helpers_vk.h`,
    `nvsdk_ngx_helpers_dlssd_vk.h` and the headers they include;
  - `lib/Linux_x86_64/libnvsdk_ngx.a`, linked into the shim;
  - `lib/Linux_x86_64/rel/libnvidia-ngx-dlssd.so.<version>`, which NGX
    loads from the shim's folder at run time. The release one: `dev/` is
    a developer tool and is not to be shipped.
- **Vulkan's headers** at `ignore/Vulkan-Headers/include`
  (github.com/KhronosGroup/Vulkan-Headers), or `just ngx ignore/DLSS
  /usr/include` with `libvulkan-dev`.
- `g++` (C++17) and `libvulkan.so.1`.

It installs `liblahza_ngx.so` and `libnvidia-ngx-dlssd.so.*` into
`~/.local/lib/lahza`. The device must be made with the extensions NGX
asks for; the vendored blade-graphics does that on NVIDIA
(`Context::ngx_handles`).

## Shipping it: NVIDIA's licence

Read on 2026-11-20 from the SDK's `LICENSE.txt` ("NVIDIA RTX SDKs
LICENSE", v. March 14, 2024, with its RTX supplement). This is a reading,
not legal advice; the licence is to be read again at the SDK version that
is shipped.

NVIDIA's library may be shipped, with conditions:

- **Inside the application, as object code** (1c): the shim (which holds
  `libnvsdk_ngx.a`) and the release `libnvidia-ngx-dlssd.so`, as part of
  Lahza's package. Not as a download of their own (4b: "you may not
  distribute or sublicense the SDK as a stand-alone product"), and not the
  SDK's developer tools (`dev/` libraries).
- **Under terms at least as protective as NVIDIA's** (2c), and never so
  that the SDK becomes subject to an open-source licence (4e). Lahza is
  MIT: NVIDIA's files cannot be under it. They need their own notice in
  the package, they stay out of the repository, and they are not linked
  into `lahza` itself (the shim is a separate library, loaded when
  present, which is how it is built now).
- **NVIDIA GPUs only** (supplement 1). The shim is only loaded where NGX
  says Ray Reconstruction is available.
- **NVIDIA is told before a commercial release** (supplement 4):
  developer.nvidia.com/sw-notification, with the company, the SDK, the
  application's name, platform, ship date and a link.
- **Attribution** (supplement 7.1b): the use of the SDK is named, with
  NVIDIA's marks, on splash screens and in the about box. Lahza has
  neither line today. Saying more than that (that NVIDIA endorses it)
  needs their agreement (4c, 7.1a).
- **Not in a cloud service**, but for systems running NVIDIA vGPU
  software (supplement 3): a render service on rented GPUs is outside the
  licence.
- NVIDIA may update the library over the air (supplement 6.1) and, as a
  last resort, disable an integration with unfixed problems (6.2).

To settle before any package ships it:

1. Whether to ship it at all, given the above (the owner's decision).
2. The application id. `lahza_ngx_init` passes a made-up one
   (`0x4c41485a41`, "LAHZA"). NVIDIA assigns application ids; the SDK's
   programming guide also describes starting NGX with a project id
   instead (`NVSDK_NGX_VULKAN_Init_with_ProjectID`). Which of the two a
   shipped Lahza must use is to be read there: the guide is in the SDK's
   `doc/` folder, which was not at hand when this was written.
3. The notice file and the about-box line.
4. Where the build gets the SDK: the snap is built from this repository
   alone, so the SDK would be a pinned source of the snap's build
   (`snap/snapcraft.yaml`), and the release's `.so` a file of the snap.
   `just install` would run `just ngx` when `ignore/DLSS` is there.
