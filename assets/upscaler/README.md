# UI reconstruction models

Lahza UI CNN v2 enlarges zoomed screen recordings. It is trained on natively
rendered screen content: documentation pages, code editors, terminals and
procedural UI. Separate 2× and 4× models run on the GPU in
`src/recording/screen_upscale.wgsl`.

| File | Role |
| --- | --- |
| `ui-cnn-v2-2x.f32` | 2× model: 16 channels, 6 layers, 11,468 parameters |
| `ui-cnn-v2-4x.f32` | 4× model: 24 channels, 7 layers, 37,128 parameters |
| `ui-training-v2.json` | Validation, test and real-recording metrics for both |
| `ui-cnn-*.f32`, `anime4k-*` | UI CNN v1 and its initialization, kept as the comparison baseline |

## Architecture

Each model is a plain stack of 3×3 convolutions with ReLU: a head (RGB to
features), hidden layers, and a tail fed by the last hidden features plus the
head features. The tail predicts an RGB residual that is pixel-shuffled and
added to bilinear RGB. Alpha always comes from the original source.

Coefficients are little-endian float32. Per layer, for each output group of
four channels, for each 3×3 tap (row-major), for each input group of four
channels, one column-major 4×4 matrix (column = input channel) follows; the
layer's biases follow as one vec4 per output group. Missing input channels
are zero. `tools/export_ui_upscaler_v2.py` writes this layout and checks it
against PyTorch before writing.

Supported Vulkan devices use native f16 products with f32 accumulation in the
hidden layers; the head and tail stay f32. `LAHZA_UPSCALE_F32=1` selects f32
throughout. On an RTX 3060 the GPU output matches an independent CPU oracle
within 1/255 at both scales, including tiled and edge regions.

## Training data

`tools/build_upscaler_data.py` renders every source at 1×, 2× and 4×:

- Headless Chrome screenshots of the local Rust documentation, light and
  dark, each book in exactly one split.
- Code-editor pages (Rust library and Python standard library source,
  highlighted with pygments in 14 editor themes and five monospace fonts) and
  terminal pages (prompts, directory and package listings). This
  repository's own source and git log form the code test split only.
- Procedural UI scenes rasterized natively at each scale; validation and test
  use three fonts excluded from training.

Inputs are mostly the high-resolution render area-downscaled and passed
through Lahza's recording codec (VP8 realtime, yuv420p). Natively rasterized
1× inputs, clean and VP8, make up the rest. No user recordings are training
inputs.

The loss is Charbonnier RGB error plus gradient L1 against the HR render,
plus L1 between the area-downscaled output and the clean input. The last term
keeps glyph structure from the source instead of rounding it.

## Results

Held-out test split, 4× (PSNR, dB):

| Input | Bilinear | v1 | v2 |
| --- | ---: | ---: | ---: |
| Code, downscaled + VP8 | 21.59 | 23.43 | 26.50 |
| Docs, downscaled + VP8 | 24.22 | 25.61 | 30.82 |
| UI scenes, downscaled + VP8 | 20.92 | 20.87 | 22.53 |
| Code, native 1× + VP8 | 19.78 | 19.46 | 20.16 |

2× validation on downscaled + VP8 input: code 22.43 → 26.47 dB, docs
23.58 → 29.89 dB (bilinear → v2). On natively rasterized input, which hints
glyphs differently from the HR render, all methods are within about 0.4 dB.

PSNR rewards soft output, so sharpness is also measured on frames from local
Lahza recordings (evaluation only, never trained on or distributed): edge
energy and halo (output outside its source neighbourhood's range).

| 2× on real recordings | Edge energy | Halo |
| --- | ---: | ---: |
| Bilinear | 47.7 | 0.0 |
| v1 | 106.9 | 16.0 |
| v2 | 61.6 | 1.2 |

v1 is sharper only through overshoot halos.

## Cost

GPU time on an RTX 3060 for one enhanced region: 960×540 at 2× takes about
18 ms and 480×270 at 4× about 16 ms (v1: 2.1 ms and 2.8 ms). Regions are
cached across nearby frames, so the cost is paid when the visible region
changes.

## Reproducing

From the repository root, with Chrome, ffmpeg and a CUDA PyTorch environment
that has numpy, Pillow and pygments:

```sh
python tools/build_upscaler_data.py
python tools/train_ui_upscaler_v2.py --scale 2 --channels 16 --layers 6 --name=-sharp
python tools/train_ui_upscaler_v2.py --scale 4 --channels 24 --layers 7 --name=-sharp --test
python tools/export_ui_upscaler_v2.py ignore/upscaler-v2-runs/2x-c16-l6-sharp ignore/upscaler-v2-runs/4x-c24-l7-sharp
```

## Rendering policy

Only enlarged source regions are enhanced. Output is bounded to 24 million
pixels and 8192 pixels per side; 4× falls back to 2× when necessary. Strength
ramps over 1.02×–1.2× enlargement, attenuates with motion, and is quantized to
1/16. Strength below 0.2 and source footprints requiring minification skip
neural reconstruction. An extra 32-source-pixel inference margin avoids
re-running the network for every small viewport change. Unsupported GPUs keep
original-source sampling.

## v1 baseline provenance

UI CNN v1 was initialized from **Anime4K v3.2 Upscale CNN x2 S** by bloc97,
under MIT:
<https://github.com/bloc97/Anime4K/blob/28535a4aff3deb55707a36c2f2d05009e0cb7fb3/glsl/Upscale/Anime4K_Upscale_CNN_x2_S.glsl>.
Its training and metrics are in `tools/train_ui_upscaler.py` and
`ui-training.json`. The v2 models are trained from scratch. Copyright notices
are retained in [LICENSE](LICENSE).
