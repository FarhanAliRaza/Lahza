# UI reconstruction models

Lahza UI CNN v1 is trained on generated UI panels, text, thin lines and controls.
It uses no user screenshots, Screen Studio source, or Screen Studio weights.
This is an independent reconstruction model; the measured results below do not
establish pixel parity with Screen Studio or recovery of missing source detail.

The 2× model has four 3×3 convolutions, four features per layer, positive/negative
feature splitting, and a pixel-shuffled luma residual added to bilinear RGB.
The 4× model is a separately trained two-stage cascade with its own first and
second stage. Both stages are jointly optimized; the RGBA8 intermediate is
included in training using straight-through quantization. Runtime transparency
always comes from the original source, including at 4×.

| File | Role |
| --- | --- |
| `ui-cnn-2x.f32` | Complete 2× model |
| `ui-cnn-4x-first.f32` | First stage of the independent 4× model |
| `ui-cnn-4x.f32` | Second stage of the independent 4× model |

Each file is 4,672 bytes, little-endian float32. Each of four layers contains
18 column-major 4×4 matrices (positive then negative input, x-major 3×3 tap
order) followed by four biases. The first layer's unused negative matrices are
zero. The tiled GPU implementation is in `src/recording/screen_upscale.wgsl`.
Supported Vulkan devices use native f16 products in the two hidden
convolutions with f32 accumulation; the input and reconstruction convolutions
retain f32 to protect small glyph residuals. The 4× cascade also retains f32
through its first stage to stabilize its quantized intermediate. Other devices
use f32 throughout. Storage remains f32/u32. `LAHZA_UPSCALE_F32=1` selects the f32 validation
path. The minimal optional-feature Blade patch and its MIT license are in
`vendor/blade-graphics`.

## Training and validation

Training used 512 procedural 192×192 scenes, 96×96 random crops, batch size 24,
and fixed seed 1317. The training scene seeds are 100000–100511. The 2× model
uses 3,000 steps with RGB MSE + 0.015 × gradient L1; the 4× cascade uses 6,000
steps with RGB MSE + 0.6 × gradient L1. No GAN or perceptual hallucination loss
is used. Data and model definitions are fully reproducible in
[`train_ui_upscaler.py`](../../tools/train_ui_upscaler.py).

A separate set of 64 scenes (seeds 900000–900063) was used for objective
selection. A final 64-scene test set (950000–950063) was generated after
selection and evaluated without further optimization. Both output scales must
improve PSNR by at least 0.15 dB and reduce gradient error against bilinear
sampling; failed runs are not published as model artifacts.

| Fresh test set | Bilinear PSNR | UI CNN PSNR | Bilinear gradient error | UI CNN gradient error |
| --- | ---: | ---: | ---: | ---: |
| 2× | 18.987 dB | 21.991 dB | 0.097192 | 0.089715 |
| 4× | 17.047 dB | 17.525 dB | 0.103021 | 0.100952 |

A separate glyph fixture uses DejaVu Serif Bold, excluded from training fonts.
Its PSNR improves from 18.428 to 24.936 dB at 2× and 15.413 to 16.818 dB at 4×;
its stroke contrast errors decrease at both scales. Full validation/test
metrics, software versions, font and coefficient hashes are in
[`ui-training.json`](ui-training.json). Generated comparison PNGs are retained
beside the models. These measurements cover synthetic screen artwork, not all
recordings; 4× enlargement has substantially less recoverable detail than 2×.

On an RTX 3060, native mixed-precision GPU output matches the independent f32
CPU oracle within 1/255 across 2×/4×, full-frame, edge and tiled-ROI fixtures.
A separate generated UI fixture also matches PyTorch's actual forward output
within 1/255 at both scales, verifying coefficient layout and pixel-shuffle
mapping end to end.

To reproduce from the repository root (DejaVu fonts installed in
`/usr/share/fonts/truetype/dejavu`):

```sh
uv venv --python 3.12 ignore/ui-upscale-venv
uv pip install --python ignore/ui-upscale-venv/bin/python -r tools/ui-upscaler-requirements.txt
ignore/ui-upscale-venv/bin/python tools/train_ui_upscaler.py
ignore/ui-upscale-venv/bin/python tools/validate_ui_upscaler.py
```

The fixed CPU training environment is pinned in
[`ui-upscaler-requirements.txt`](../../tools/ui-upscaler-requirements.txt).
Training also supports PyTorch CUDA when available. Font binaries are not
bundled; their source license is [DejaVu Fonts](https://dejavu-fonts.github.io/License.html).

## Rendering policy

Only enlarged source regions are enhanced. Output is bounded to 24 million
pixels and 8192 pixels per side; 4× falls back to 2× when necessary. Strength
ramps over 1.02×–1.2× enlargement, attenuates across 4–28 physical pixels of
motion and 4–8 pixels of shutter trail, and is quantized to 1/16. Strength below
0.2 and source footprints requiring minification skip neural reconstruction.
GPU-generated alpha-aware mipmaps with trilinear, four-tap anisotropic sampling
handle minification, including perspective shear. The CPU renderer shares the
same filter. Cropped-out source colors cannot enter mip levels.

Unchanged source uploads, mipmaps and nearby neural regions are reused across
shutter samples. An extra bounded 32-source-pixel inference margin avoids
re-running the network for every small viewport change. Learned RGB remains
beneath annotations; alpha, cursor artwork, camera and canvas overlays retain
their independent composition. Unsupported GPUs retain original-source CPU
sampling.

## Upstream provenance

Initialization uses **Anime4K v3.2 Upscale CNN x2 S** by bloc97, under MIT:
<https://github.com/bloc97/Anime4K/blob/28535a4aff3deb55707a36c2f2d05009e0cb7fb3/glsl/Upscale/Anime4K_Upscale_CNN_x2_S.glsl>.
The original GLSL SHA-256 is
`4c53ec2e287908f7ee7bcb266b0170421626d663576468b7d7dafc62962649a4`.
The original shader and unchanged coefficients are retained as provenance and
a test oracle; production inference uses the trained UI coefficients above.
The shader is not executed or compiled by Lahza. Copyright notices for bloc97
and the UI coefficient modifications are retained in [LICENSE](LICENSE) and
embedded in the executable alongside the Blade license.
