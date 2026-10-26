# Neural microphone processing validation

Measurements on 2026-10-26, Linux x86_64, Ryzen 5 5600. Builds and model runs
were executed sequentially. All speech fixtures are public VCTK demo utterances;
[provenance and exact hashes](../assets/audio/validation-fixtures.json) are
tracked. Synthetic fan, hiss, and typing-like noise is deterministic.

## Model execution

Lahza embeds public MossFormer2 and corrected epoch-19 TF-Restormer checkpoints
and their licenses. Neither checkpoint is a recovered Screen Studio asset.
The native Rust frontends and actual bundled CPU runtime were tested against
independent torchaudio/NumPy plus ONNX Runtime references.

| Model / fixture | Native/reference RMSE | Error relative to reference signal |
| --- | ---: | ---: |
| MossFormer2, p360 | 0.0000540 | -61.71 dB |
| MossFormer2, p362 | 0.0000440 | -60.44 dB |
| MossFormer2, p363 | 0.0000422 | -63.45 dB |
| TF-Restormer, p360 | 0.0000000387 | -131.29 dB |

Native results have exact input sample counts and finite samples. Regression
tests separately check Kaldi features against torchaudio golden vectors,
MossFormer2 reconstruction against an identity mask, and TF-Restormer analysis
and synthesis against the upstream convolution kernels, including DC/Nyquist.
Cancellation interrupts inference; a subsequent request still succeeds.

TF-Restormer's packed f16-storage/f32-compute model was also compared with the
full-f32 export: waveform RMSE 0.0000189, error/signal -77.39 dB. Packaging
compression does not change the runtime to half-precision arithmetic.

## Speech evidence and finishing DSP

The **raw neural stage**, before deliberate leveling/EQ, improved unshifted
SI-SDR on the three noisy utterances:

| Fixture | Input SI-SDR | MossFormer2 SI-SDR | Improvement |
| --- | ---: | ---: | ---: |
| p360 | -0.74 dB | 1.05 dB | +1.80 dB |
| p362 | 0.13 dB | 14.28 dB | +14.16 dB |
| p363 | 1.38 dB | 3.11 dB | +1.72 dB |

All three have zero estimated lag. These numbers describe these fixtures, not
all noise types or recordings, and are not scores for the entire preset.

Stage ablation exposed a genuine leveler problem: the generous sound-retention
detector also treated periodic residual fan noise as quiet speech, and the first
quiet block could seed excessive gain. The leveler now has a separate confidence
threshold based on the measured noise floor and starts from the recording's
robust speech level. A synthetic fan/first-word regression covers this behavior.

In the DSP ablation and subsequent complete native preparation runs,
speech-envelope correlation improved from
0.451 to 0.887 (p360), 0.971 to 0.975 (p362), 0.553 to 0.946 (p363), and 0.971 to
0.993 (Podcast). Podcast peak fell from 0.95 to 0.578 and its limiter no longer
had to reduce 89,207 frames. The complete Noise reduction outputs peaked at
0.608, 0.559, and 0.499; all retained exact source frame counts. The prescribed
filtering remains in place.

Whole-preset SI-SDR against an unprocessed clean reference is not sufficient to
judge this chain: intentional high-pass/EQ changes phase and leveling changes
amplitude. For example, the Podcast raw output scores 20.46 dB; applying only
the 70 Hz high-pass changes the score to 1.85 dB against the unfiltered reference
but 20.88 dB against the same-filtered reference. Validation therefore reports
waveform metrics, envelope behavior, timing, peaks, and stage ablations separately.

For the authors' 16-to-48 kHz p360 input, TF-Restormer restores missing high-band
energy: the 8–20 kHz / 300–3400 Hz ratio changes from -70.92 to -30.09 dB, versus
-26.29 dB in the clean reference. Waveform SI-SDR decreases from 30.25 to
20.46 dB: bandwidth reconstruction changes the waveform. This does not establish
perceptual superiority on already good microphone audio.

## Preparation cost and memory

Cold native Noise reduction preparation, including normalization, inference,
finishing DSP, and FLAC writing, took approximately 2.3–2.4 seconds for the
3.0–3.8 second clips. The loaded model is reused within the application.

Podcast is substantially slower on CPU: native TF-Restormer took 36.34 seconds
for a 3.028-second clip, and full preparation took approximately 35 seconds in a
separate run. Playback reuses the prepared audio; inference is not performed in
its callback. Progress, cancellation, mode changes, and source-specific caching
are part of the editor workflow.

The initial three-second TF window briefly used about 5.3 GB in the reference
process. One-second windows with 200 ms crossfades, two CPU threads, and disabled
CPU arena/memory-pattern caching reduced measured peak RSS to 1,498,524 KiB.
This is a processing-process measurement, not total editor/GPU memory. The
smaller window preserved exact duration and zero measured lag on the fixture.

A separate native full-band stress run used one second of the noisy 48 kHz
p360 fixture. It completed in 24.10 seconds with 2,448,420 KiB peak RSS, zero
swaps, finite output, and exact sample count. Memory and processing cost depend
on input bandwidth: the 1.5 GB narrow-band measurement is not a universal ceiling.
The measured CPU range was approximately 12–24 seconds per audio second.

## Reproduction and limits

- `tools/validate_neural_audio.py` prepares deterministic noisy fixtures and
  measures waveform/envelope properties.
- `tools/validate_mossformer_audio.py` runs the independent MossFormer2 reference
  and explicitly regenerates the Kaldi golden only when requested.
- `tools/validate_tf_restormer.py` checks TF export precision and native/reference
  parity. `tools/export_tf_restormer.py` documents the pinned conversion.
- Ignored Rust fixture probes require explicit input/output paths and run the
  production model and complete preset APIs. They never open recording devices.

The portable speech-activity detector and DSP are not Apple's semantic classifier
or AudioUnits. Three utterances and regression fixtures are useful engineering
evidence, not a broad listening study, room-reverberation benchmark, or verified
perceptual match with Screen Studio. See [the architecture](AUDIO_ARCHITECTURE.md)
and [artifact study](SCREEN_STUDIO_AUDIO_STUDY.md) for these boundaries.

Final regression run: `cargo test --release -j 1 -- --test-threads=1` passed
510 tests, with 57 explicit-environment/fixture tests ignored by that command.
The four newly added neural/fixture probes ran separately on the public inputs;
the isolated Xvfb project-switch/cancel/playback lifecycle check also passed.
The full five-preset preview/export comparison now passes. It caught and fixed
unity-speed `atempo` modifying samples, and multiple models attempting to create
separate ONNX environments. Runtime initialization is now shared and covered by
a concurrent acquisition regression.

The release build passed and was installed atomically at
`/home/farhan/.local/bin/lahza`, without restarting the running editor. SHA-256:
`fdb7bfdede6d4ad4f15c01ffd78caa109c3a395adf27984146914acb1d27ef13`.
Both model license texts were verified inside the release binary. All application
builds and model validation jobs in this continuation were serialized.
