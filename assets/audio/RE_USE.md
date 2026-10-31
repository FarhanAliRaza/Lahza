# RE-USE speech enhancement (Enhance audio)

The Enhance audio preset runs NVIDIA NemotronLabs RE-USE, a 9.6 M-parameter SEMamba
model (convolutional encoder/decoder around 30 bidirectional time/frequency
Mamba v1 blocks), on the GPU through blade compute shaders. It needs a Vulkan
GPU and no ONNX Runtime or CUDA.

- Upstream: https://huggingface.co/nvidia/NVIDIA-NemotronLabs-RE-USE
- Pinned revision: `31f82f2efad1095034492f236e8dac9239ba713b`
- `re-use/model.safetensors`: upstream bytes, unmodified; 38,592,940 bytes;
  SHA-256 `06badaf4029b297f97f7fe48d62990b48d5482608210df577354e0d4e0ed70e1`
  (equal to Hugging Face's LFS record). Checked at load.
- License: OpenMDW-1.1, retained in `re-use/LICENSE` and embedded in the binary.
- Implementation: `src/recording/reuse_audio.rs` and `reuse_audio.wgsl`.

## Processing

Each segment is one `inference.py` forward pass at 48 kHz: centered Hann STFT
(n_fft 1920, hop 240), `log1p` magnitude and phase in, magnitude and phase out,
the upstream "sweep artifact" frame silencing, and iSTFT. No input gain
normalization is applied, as upstream. Audio up to 10 s is a single pass.
Longer audio uses `inference_chunk.py`'s combination (periodic-Hann-weighted
overlap-add, the last segment aligned to the end) with 10 s segments and a
quarter overlap instead of the scripted 5 s / half overlap, giving InstanceNorm
more context at 1.33× rather than 2× compute. Output is streamed, so host
memory is bounded by two segments. Identical stereo channels share one pass.

On devices with native f16 the full-resolution encoder activations are stored
as f16 (f32 arithmetic). Mamba intermediates exceed the f16 range and always
stay f32. `LAHZA_REUSE_F32=1` forces f32 storage throughout.

Without a GPU it still runs, on a CPU Vulkan implementation (Mesa lavapipe),
and preparation shows "no GPU found, this is very slow": measured ~53 s per
audio second on a Ryzen 5 5600, versus ~0.37 s on the RTX 3060. Hardware
devices are always preferred over software ones (a vendored blade change), so
lavapipe is only used when nothing else is available. On macOS, blade's Metal
backend runs the same shaders on the Apple GPU.

Inference runs on the audio preparation worker with its own Vulkan context.
Work is submitted per stage and every GEMM dispatch is capped at ~4.3 G
multiply-accumulates (~3 ms on an RTX 3060), so other GPU clients interleave.
Cancellation is checked between stages.

## Validation (RTX 3060, driver 595.91)

Reference: the official PyTorch code (torch 2.9.1+cu128, mamba_ssm 2.3.2.post1)
on the first 10 s of a Lahza microphone recording, dumped per layer with
`ignore/re-use-tools/dump_reference.py` in strict fp32 (cuDNN TF32 off). The
full 19.5 s file needed ~8 GB in PyTorch and did not fit beside another job.
The unmodified `inference.py` was also run (default TF32, 24-bit FLAC output);
it differs from strict fp32 by rel-L2 2.3e-4.

Each layer is checked with PyTorch's input for that layer:

| Check | f32 rel-L2 | f16-encoder rel-L2 |
| --- | ---: | ---: |
| STFT log1p magnitude | 7.4e-7 | 2.1e-4 |
| Encoder layers | ≤ 2.2e-6 | ≤ 6.3e-4 |
| Block 0 Mamba internals (in_proj, x_proj, scan, out) | ≤ 2.1e-6 | ≤ 8.4e-4 |
| Each of the 30 TF-Mamba blocks | ≤ 9.2e-6 | f32 |
| Decoder layers and outputs | ≤ 1.4e-6 | f32 |
| iSTFT | 4.1e-7 | f32 |
| End-to-end waveform vs strict fp32 | 1.9e-5 | 3.5e-4 |
| End-to-end waveform vs official `.flac` | 2.3e-4 | 4.2e-4 |

STFT phase is excluded from the table: near-silent bins have noise-dominated
phase in any f32 FFT. Against an exact float64 STFT, phase errors above 1e-3
occur at 12,658 bins here (20,110 with f16 input storage) versus 53,429 for
cuFFT. The test asserts this, the table's limits, and runs with
`LAHZA_REUSE_REF=<dump> cargo test --release reuse_audio -- --ignored --nocapture --test-threads=1`.

Performance on the full 19.5 s, 48 kHz recording (one pass, GPU otherwise idle):

| Storage | Time | Real-time factor | Device memory |
| --- | ---: | ---: | ---: |
| f16 encoder | 7.25 s | 0.37 | 2.67 GB |
| f32 | 7.17 s (before dispatch capping) | 0.37 | 4.18 GB |

A 10 s segment, the production maximum, holds 1.39 GB (f16) or 2.16 GB (f32)
plus 38 MB of weights. A separate Vulkan client submitting a 2 M-element compute
pass every 16 ms measured p50/p99/max completion of 0.12/0.40/1.07 ms idle and
0.81/1.51/1.57 ms during enhancement, with no frame over 16 ms.

Files longer than one segment pay for the overlap: about 1.33× the single-pass
cost (RTF ≈ 0.5), and up to 1.5× for lengths just over a segment boundary,
where the end-aligned last segment overlaps more. Through `enhance_file`, the
19.5 s recording as duplicated-mono stereo (three segments, one inference per
segment) matches the official `inference_chunk.py --chunk_size_in_seconds 10
--hop_length_portion 0.75` output with rel-L2 4.2e-4.
