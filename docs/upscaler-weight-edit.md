# 2× upscaler weight-edit experiment

This experiment tests a narrow application of [weight-space programming](https://rikiyat.github.io/blog/weight-space-programming/): reducing false color on gray text by directly editing the existing CNN's final convolution. It does not claim that a language model can generally improve super-resolution without training.

The source is the shipped 11,468-parameter Lahza UI CNN v2 2× model. The 4× model is outside this experiment. All runs use CPU PyTorch 2.5.1; the independent exporter checks the flat shader layout against PyTorch. No architecture, shader, or inference operation is added.

## Result

The selected edit reduces false color on held-out grayscale text by **24.6%** in channel-spread RMS. Its worst-case channel spread falls from 21 to 15 output levels. This is a small artifact correction: it does not establish better text recognition or sharper reconstruction.

| Held-out measurement | Original v2 | Edited v2 | 300-step fine-tune |
| --- | ---: | ---: | ---: |
| Gray-text channel-spread RMS, 8-bit levels | 1.2162 | 0.9170 | 1.2101 |
| Gray-text maximum channel spread, 8-bit levels | 21 | 15 | 22 |
| Real-recording halo, existing metric ×1000 | 1.1872 | 1.1726 | 1.1229 |
| Synthetic pan error, 8-bit levels | 2.0384 | 2.0319 | 2.0211 |

Across the nine full test groups, the edited model's PSNR changes range from −0.0150 to +0.0162 dB. On real-recording crops, edge energy changes from 61.5607 to 61.5102, and source contrast retention from 0.97057 to 0.97060. Thus the tint reduction does not come with a meaningful sharpness gain. Fine-tuning did better on the halo and synthetic pan metrics, but reduced grayscale-text tint RMS by only 0.5% in this run.

Exactly **1,727 of 11,468 parameters** change (15.1%), all in the final layer. The exported layout agrees with an independent NumPy evaluation to a maximum absolute error of 1.57×10⁻⁷. Flat-gray and integer-pan checks remain exact after output quantization. Five numerical regression tests cover loading/round-trip identity, projection invariants, cached inference, gray preservation, and blank-target handling.

The candidate remains separate from the shipped models. CPU evaluation and shader-layout verification are complete; actual GPU execution, OCR accuracy, and real-video temporal evaluation remain unverified. This result supports a constrained chroma calibration inspired by the article, not a general claim about LLMs understanding arbitrary weights.

Full metrics and hashes are in [the results JSON](upscaler-weight-edit-results.json). Local candidate weights are written to `ignore/upscaler-weight-edit/edited-2x.f32`.

## Observed defect and edit

Uniform gray inputs are reproduced exactly at all 256 8-bit levels. Nevertheless, a gray diagonal with input values 51 and 204 produces an RGB channel spread of up to 18 levels. Rendered grayscale text produces a spread of up to 27 levels. The output should remain neutral in both cases.

The tail has 12 output channels: RGB × four pixel-shuffle positions. For each position, input feature, and kernel tap, the selected edit is:

```text
mean = (W_red + W_green + W_blue) / 3
W_edited = mean + 0.75 * (W_original - mean)
```

Biases and all earlier layers are unchanged. This reduces the chromatic part of the residual by 25% and preserves its RGB arithmetic mean before clipping. It is not luminance-preserving in a perceptually weighted color space. It also attenuates legitimate chromatic corrections, so color and reconstruction checks are necessary.

The edit is a deterministic tensor projection, with no gradients or fitted replacement weights. Its strength is selected using validation examples. It is not a zero-data or zero-feedback result.

## Protocol

- **Development probes:** neutral geometric patterns and DejaVu Sans/Mono text at four font sizes in both polarities.
- **Selection:** one deterministic 48×48 crop per validation image in nine source/degradation groups. There are 552 crops across docs, code, and procedural scenes; each source is evaluated as native, native+VP8, and downsampled+VP8.
- **Candidate families:** 52 RGB-neutrality projections (four global strengths and three strengths for each of 16 channels), followed by four global pixel-shuffle phase projections. No single-channel candidate qualified. The phase family preserves each output block's mean chroma but did not qualify either.
- **Gates:** at least 10% less grayscale-text channel-spread RMS; exact quantized flat grays; in every validation group, PSNR loss ≤0.05 dB, edge/stroke error increases ≤1%, chroma MSE increase ≤2%, and halo increase ≤3% + 0.002 output levels. These are practical tolerances, not statistical significance thresholds.
- **Fine-tuning baseline:** 300 Adam steps, learning rate 1e-5, batch 16, 3,072 cached training crops, seed 2601103. It uses the original degradation proportions, inversion/channel permutations, Charbonnier + gradient + backprojection objective, and gradient clipping. This is one short continuation, not an optimized comparison against all fine-tuning methods.
- **Final evaluation:** full validation and held-out test images, fresh grayscale geometry and text probes, existing real-recording crops, integer-pixel translation checks, and a synthetic quarter-pixel text pan. Candidate hashes are written before test data is opened. No edits are selected using test results.

The first flat-field check compared floating-point intensities directly and incorrectly reported an error of 0.000015 output levels even for unchanged v2. This was corrected to compare integer 8-bit outputs, and all candidates' diagnostics and acceptance were recomputed with the same thresholds and cached image metrics. A regression test covers it.

Full-image stroke error is averaged over two-image batches with eligible target edges; blank batches are excluded. The original helper returned NaN for blank documentation batches. After correcting the aggregation, the affected test documentation groups were reevaluated with the same frozen weights. No candidate was changed in response to test results.

## Reproduce

Requires the upscaler's existing NumPy/Pillow/PyTorch environment and dataset from `tools/build_upscaler_data.py`. The baseline is loaded directly from the shipped `.f32` file and round-tripped byte-for-byte; no original training checkpoint is needed. Use a fresh output directory for an independent run:

```sh
PY=ignore/ui-upscale-cpu-venv/bin/python
$PY tools/test_ui_upscaler_weight_edits.py
$PY tools/experiment_ui_upscaler_weights.py search --projection rgb --out ignore/upscaler-weight-edit-reproduction
$PY tools/experiment_ui_upscaler_weights.py search --projection phase --out ignore/upscaler-weight-edit-reproduction
$PY tools/experiment_ui_upscaler_weights.py finetune --out ignore/upscaler-weight-edit-reproduction
$PY tools/experiment_ui_upscaler_weights.py evaluate --split valid --out ignore/upscaler-weight-edit-reproduction
$PY tools/experiment_ui_upscaler_weights.py evaluate --split test --out ignore/upscaler-weight-edit-reproduction
```

The script saves candidate checkpoints, an exported `.f32` candidate, per-group metrics, hashes, and comparison panels under its output directory. Panels run left-to-right: nearest-neighbor source, original v2, edited v2, fine-tuned v2. Real recording pixels are not copied into this report.

## Limits

Channel neutrality is an objective property of grayscale input, but it is not a measure of reading accuracy. Stroke error is a proxy; OCR character error rate was not measured. The motion check measures synthetic resampling sensitivity, not flicker in encoded recordings. GPU execution of the new weights has not been tested. The test split was unused in this experiment's selection; it is the repository's existing holdout, not a newly collected dataset.
