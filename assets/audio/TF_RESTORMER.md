# TF-Restormer restoration model

`tf-restormer-weights-f16.onnx` is a local conversion of the corrected public
TF-Restormer baseline (epoch 19). The model and its MIT license are embedded in
Lahza; inference uses the bundled ONNX Runtime on Linux x86_64 without a server
or a first-use download.

- Paper: <https://arxiv.org/abs/2509.21003>
- Authors' source: <https://github.com/dmlguq456/TF_Restormer>
- Source revision: `eaefe1cb3811c13f324eee34a81564cff8b7da7e`
- Checkpoint: <https://huggingface.co/shinuh/tf-restormer-baseline>
- Checkpoint revision: `bd8ffc3105299ea5497797783ed1ce750e089523`
- Downloaded `model.pt` SHA-256:
  `d67115273bed06cb1ab06de57168e6ce6e56dfcf3c626b3e3fbc6eb945b5424e`
- Authors' original `epoch.0019.pth` SHA-256:
  `f41a075ee3edf4c747f7626a0c64b09c361cd8a3058201d24f6572313c3501a4`
- Config SHA-256:
  `d94e7509dfa84a09d76a81a71a4861fc8cf3684b753d45eb24b7ff5ef27daf8e`
- Bundled ONNX bytes: 61,746,447
- Bundled ONNX SHA-256:
  `f3f1cf158d5eae68c2402e8022da829cec763c8ada3d8258ee514a1d662f8e31`
- License: [LICENSE-TF-RESTORMER](LICENSE-TF-RESTORMER).

The authors replaced the earlier epoch-25 Hub checkpoint after discovering
that it suppressed almost all output above 8 kHz. This bundle uses their
corrected epoch-19 checkpoint. It is not Screen Studio's recovered model, and
the model name alone does not establish perceptual parity with Screen Studio.

## Conversion and signal handling

The conversion uses ONNX opset 17, dynamic input frequency/time dimensions,
and a fixed 961-bin output. Python-only input bandwidth detection runs in
Lahza's native frontend. Rotary embedding caches are disabled for export and
negative ONNX transpose axes are canonicalized. Large floating-point weights
are stored as f16, cast to f32 in the graph, and computed in f32 on CPU. This
reduces packaging size; it does not imply accelerated half-precision compute.

`tools/export_tf_restormer.py` reproduces this transformation from the pinned
source, checkpoint and config. Conversion and quality measurement are developer
tasks and are not dependencies of the installed application.

The native frontend consumes 48 kHz PCM, uses the author's periodic square-root
Hann window (1920 samples), 960-sample hop, and normalized one-sided convolution
kernel. The equivalent FFT scale is `1/sqrt(960)`. Synthesis includes the
author's DC/Nyquist convention; substituting a conventional inverse STFT would
change the signal. Direct convolution-kernel regression tests cover both
directions independently of the FFT implementation.

Input bandwidth detection trims bins whose mean magnitude lies 60 dB below
the strongest bin, with at least 161 input bins. The traced graph always
retains a generated high band, so a full-band input reserves the Nyquist bin
for generation (maximum 960 input bins). Lahza adds one hop of boundary context
and crops the result to preserve the first/last samples and exact duration.
Long recordings use one-second windows with 200 ms crossfades. Distinct
stereo channels are processed separately; identical channels share inference.
Cancellation interrupts ORT between graph operators and is checked during file
processing. Its CPU session uses at most two threads and disables both the
memory arena and memory pattern cache so large attention allocations can be
released between runs. Camera and noise-reduction runtime settings are
unchanged. The prepared result is cached before playback/export.

## Validation boundaries

`tools/validate_tf_restormer.py` compares packed weights against the full-f32
export, measures reference speech, and can compare the complete native file
result with independent NumPy/ONNX Runtime processing using the same chunk
boundaries. It reports sample count, latency, signal-to-distortion change,
high-band energy and processing time. The public test utterances and their
hashes are recorded in [`validation-fixtures.json`](validation-fixtures.json).
They are public VCTK demo clips, not the user's recordings.

CPU restoration is substantially more expensive than noise reduction. It
runs during explicit preparation, with progress/cancellation, rather than on
the playback callback. A completed conversion or a successful inference is
not by itself evidence of audio quality; measured results belong in the audio
architecture validation report. Exact Screen Studio weights, Apple audio-unit
behavior and perceptual equivalence remain unverified.

The original three-second CPU run exceeded 5 GB of process memory in a local
test. Frequency attention has a
`[time_frames, 4, frequency_bins, 512]` score tensor, so shortening the window
reduces this cost. The one-second window is an intentional bounded-memory CPU
tradeoff; it must be evaluated as that configuration, rather than claiming
quality results from longer-context offline inference apply unchanged.

For the public `p360_072_mic1` demo (3.028 s of speech with missing high-band
content), the one-second CPU configuration at two threads took 34.88 s of
inference and peaked at 1,498,524 KiB process RSS. The 8–20 kHz / 0.3–3.4 kHz
energy ratio moved from −70.92 dB in the input to −30.09 dB after restoration;
the clean reference is −26.29 dB. Output was finite, retained all 145,346
samples, and had zero estimated lag. This establishes bandwidth restoration on
that fixture, not perceptual superiority: SI-SDR against the clean reference
fell from 30.25 to 20.46 dB, showing that generated restoration also changes
the original waveform. These are raw-model results before the preset's
normalization, leveler and limiter.

A separate three-second-window check compared the packed weights with the
full-f32 export: waveform RMSE was `1.8883e-5`, or −77.39 dB relative to the
full-f32 output RMS. Its results do not imply that either model is identical
to Screen Studio. Evidence is saved in
`ignore/audio-enhancement-analysis/tf-validation-p360/metrics.json` and
`ignore/audio-enhancement-analysis/tf-validation-1s-p360/metrics.json`.
