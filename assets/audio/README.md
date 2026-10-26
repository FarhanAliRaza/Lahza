# Speech enhancement models

Noise reduction uses the MossFormer2 model described here. Podcast restoration
uses the separately documented [TF-Restormer model](TF_RESTORMER.md).

`mossformer2-se-48k-int8.onnx` is an Apache-2.0 ONNX conversion of Alibaba
Speech Lab's public MossFormer2_SE_48K speech enhancement checkpoint. It is
bundled for offline CPU inference. It is not a recovered Screen Studio model.

- Upstream weights: https://huggingface.co/alibabasglab/MossFormer2_SE_48K
- Upstream implementation: https://github.com/modelscope/ClearerVoice-Studio
- Conversion: https://huggingface.co/ipsilondev/MossFormer2-SE-48K-ONNX
- Pinned conversion revision: `40dd49e2d52152d08e5a09a63a5d9465e6bf6329`
- Artifact bytes: 94,268,636
- SHA-256: `2f98c2b3d16cd30f9367181a4328a813a5455121267138836f25218467957b54`
- Input: 48 kHz mono, four-second windows; `[1,496,180]` f32 Kaldi filterbank
  features (60 filters plus first/second deltas).
- Output: `[1,496,961]` real spectral mask.
- Inference: bundled ONNX Runtime CPU, four threads maximum.

The native frontend follows ClearerVoice's 40 ms Hamming / 8 ms hop Kaldi
filterbank (2048-point FFT) and uncentered 1920-point STFT/iSTFT. Dither is zero
for deterministic output. Long files use four-second windows with one-second
crossfades; distinct stereo channels are processed independently, while identical
channels from a mono microphone share one inference. File-level active
sample normalization is reversed after inference, preserving input gain for the
separate editor voice-leveling stage.

Inference runs on the audio preparation worker, with a per-window ONNX Runtime
cancellation token. Cancelling or switching modes interrupts graph execution;
the next request creates a fresh token and can reuse the loaded model. The
model never runs on the playback or UI thread.

The conversion author reports model parity; Lahza's own validation is recorded
separately. Exact Screen Studio weights and perceptual equivalence are not
established. Model license text is retained in `LICENSE` and embedded in the
binary. The existing bundled ONNX Runtime has its own MIT license and notices
under `assets/matting/runtime/`.

`fbank-reference.f32` contains five feature rows (frames 0, 1, 247, 494,
495) generated independently with torchaudio 2.5.1's Kaldi implementation,
dither disabled. The test regenerates the input from a fixed integer LCG; this
checks mel/FFT/window/preemphasis and both delta orders without shipping speech.
The tracked [independent reference tool](../../tools/validate_mossformer_audio.py)
reproduces these features with torchaudio, and full speech processing with
torchaudio STFT/iSTFT and ONNX Runtime. It needs NumPy, PyTorch, torchaudio,
ONNX Runtime, SciPy and SoundFile; no Python packages are needed by the installed
application. The existing golden was produced with torchaudio 2.5.1. The tool
records library versions, model/input hashes, timing and signal metrics for
each validation run. Its default is two numerical threads.

After obtaining the public fixtures identified in
[validation-fixtures.json](validation-fixtures.json), prepare deterministic
noisy/reference pairs and run the independent model:

```sh
python tools/validate_neural_audio.py prepare path/to/fixtures
python tools/validate_mossformer_audio.py path/to/fixtures --output-directory path/to/results
```

The result directory receives WAV and little-endian f32 outputs, matching raw
inputs for the native Rust probe, and `moss-python-metrics.json` plus
`moss-python-run.json`. Native and reference outputs must be compared separately;
running this Python tool alone does not validate the native frontend.

Normal validation never changes the tracked feature golden. Explicit
regeneration uses the fixed integer LCG and can write a review copy:

```sh
python tools/validate_mossformer_audio.py --golden-only --golden-output ignore/fbank-reference.f32
```

With `--golden-only` and no destination, the tool deliberately replaces
`assets/audio/fbank-reference.f32`. This mode computes features without loading
the enhancement model.

The public speech is from the CSTR VCTK corpus by Junichi Yamagishi, Christophe
Veaux and Kirsten MacDonald (2019), shared under CC BY 4.0. The fixture manifest
retains the [corpus attribution](https://doi.org/10.7488/ds/2645),
[license evidence](https://datashare.ed.ac.uk/items/30e7453c-9ea8-48b4-8e18-f96d0dc62928/full),
exact demo URLs and SHA-256 hashes, and descriptions of local modifications.
Speech fixtures are developer validation inputs and are not bundled into Lahza.

Native parity, speech measurements, CPU cost, and validation limits are recorded
in [NEURAL_AUDIO_VALIDATION.md](../../docs/NEURAL_AUDIO_VALIDATION.md).
