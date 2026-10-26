# Recording audio architecture

Lahza retains microphone and system audio independently in new recordings.
Audio preparation produces a separate lossless file which playback and export
consume before applying timeline edits. Microphone presets use offline native
DSP and bundled neural models on Linux x86_64. Noise reduction runs the public
MossFormer2 SE 48K model; Podcast runs the public corrected epoch-19
TF-Restormer model. These are independent public checkpoints, not recovered
Screen Studio weights. Equal model-family names do not establish sound-quality
parity with Screen Studio.

## Capture and compatibility

The native GStreamer capture pipeline encodes each enabled source into its own
Opus stream in `screen.mkv`, sharing the video pipeline's clock. Stream titles
identify the source: `Lahza-Microphone` and `Lahza-System-Audio`. Microphone comes
first when present. Finalization and pause/resume concatenation retain every
stream and its tags.

Finalization stages and validates output before replacing the destination.
Opening an interrupted project recovers all ordered screen segments and the
webcam separately, including recovery from an incomplete final join. A failed
recovery keeps the original segments available for another attempt.

Tagged tracks are selected by role, not by their position. Noise reduction
applies only to the microphone; system-only audio bypasses it. Older untagged
recordings and imported media retain the previous first-audio-stream policy.
Their combined microphone/system audio cannot be separated exactly, so enabling
cleanup on such a source processes the existing combined track.

## Microphone presets

The Audio sidebar exposes five full-row selectable presets. Selection is saved
as `lahzaExtras.audioMode` and is used by playback, video export, audio-only
export, and project CLI export. Older `noiseReduction` booleans migrate to Off
or Noise reduction. Unknown future mode names preserve other saved settings.

| Preset | Processing |
| --- | --- |
| Off | Repair a nearly silent stereo microphone channel without changing the good channel's gain; legacy untagged sources retain their original audio |
| Standard | Stereo channel repair and peak normalization to -6 dB |
| Loud | Channel/level preparation, voice leveling, 70 Hz high-pass filtering, dynamics, pause attenuation, and limiting |
| Noise reduction | Standard preparation, MossFormer2 noise suppression, then the Loud processing stages |
| Podcast | Standard preparation, pause attenuation, TF-Restormer restoration, sibilance restoration, spectrum shaping, dynamic low-mid EQ, leveling, de-essing, high-pass/parametric EQ, and limiting |

Supporting DSP uses deterministic energy and frequency-band activity measures.
It is not Apple's semantic speech classifier or an exact implementation of its
Audio Units. Neither the level targets nor the recovered `targetDb` names are
claimed as calibrated LUFS. See [the study](SCREEN_STUDIO_AUDIO_STUDY.md) for
which stage order and parameters were recovered and what remains uncertain.

MossFormer2 uses four-second windows. TF-Restormer uses one-second windows with
200 ms crossfades, two CPU threads, and no retained CPU arena or memory-pattern
cache to reduce peak memory. Both frontends preserve sample
count. Identical stereo channels share one inference. Models and the CPU runtime
ship inside the application; using a preset does not download weights or send
recordings anywhere. Model identity/checksum and DSP version invalidate stale
prepared audio. Inference runs only during cancellable background preparation;
playback reads its completed lossless asset. Podcast preparation is more
expensive than Noise reduction on CPU. See the [model notes](../assets/audio/README.md)
and [TF-Restormer notes](../assets/audio/TF_RESTORMER.md).

## Source-time preparation

`src/recording/audio_processing.rs` owns source probing, role selection,
processing, synchronization, cache validation, and subprocess cancellation.
When needed, it writes a 48 kHz stereo FLAC asset with 24-bit samples. It does
not copy or re-encode the video. The decoded capture is still limited by its
original codec; lossless preparation avoids adding another lossy generation.

Processing keeps audio in the source recording's time coordinates, including
late audio starts and gaps. For a new recording, it cleans the microphone when
requested, then mixes it with system audio. A limiter protects the combined
recording mix. It retains the source duration, allowing the same trims and rate
changes to address both video and prepared audio.

Preview uses original video and prepared audio in a single GStreamer pipeline
with a shared clock and segment seeks. Export awaits that same prepared source
asset before trimming, changing speed, concatenating clips, and mixing imported
music. Export no longer applies a second adaptive denoiser after composition.
Both video and audio-only exports use this path. Unchanged-speed cuts skip
FFmpeg atempo: even atempo=1.0 modifies samples. Actual speed edits retain
time stretching. GStreamer `scaletempo` and
FFmpeg `atempo` remain different time-stretch implementations, so sharing the
prepared samples does not imply byte-identical final output at altered speeds.

Media-bank entries are prepared independently; importing another clip reuses
finished work for the existing entries. Silent images require no audio asset.
Audio imports and waveform extraction also use the source's audible mix.

## Cache and jobs

Camera and speech sessions share one process-lifetime ONNX environment.
Initialization is serialized and retries after failure; model sessions, CPU
settings, and cancellation tokens remain independent. Switching from one
neural preset to the other does not create a second runtime environment.

Project caches live under `.lahza-audio` in the `.lahzarec` project. External
media uses the user's cache directory, allowing imports from read-only sources.
Cache identities include the source's identity, a content fingerprint for
processed sources, selected stream roles, processing version, and filter
configuration, including the selected mode, model identity, and DSP version. A completed output is validated before its stamp is published.
Metadata changes, corrupt outputs, and chain changes invalidate cached work.
Ready checks use small metadata records rather than scanning video on the UI
thread.

An operating-system file lock serializes writers for the same source/mode.
Workers write unique temporary files and publish completed files atomically.
Cancellation is checked during lock waits, hashing, probing, and FFmpeg
processing, DSP blocks, and neural execution. Neural RunOptions allow cancellation
between graph operators. Pipes are drained while the child is polled; cancellation kills and
reaps the subprocess and discards its incomplete output. Retrying reuses
completed source assets. It does not resume an arbitrary half-written FLAC.

The editor has an audio-specific progress/cancel/retry state. Preparation does
not set the timeline's editing-busy flag. Project changes and superseded
requests cancel their worker; completion must match the active job, project,
source, and cleanup selection before changing the UI. A processing failure does
not silently change the user's cleanup setting.

## Validation

[Neural audio validation](NEURAL_AUDIO_VALIDATION.md) records native/reference
parity, public speech results, stage ablations, and CPU/memory costs.


Tests use generated tones and synthetic GStreamer sources; they do not open
microphones, speakers, or a desktop capture session. Coverage includes source
isolation, role persistence through finalization, delayed microphone timing,
system-audio preservation, legacy behavior, cache invalidation, concurrent
requests, cancellation, and shared prepared audio across playback/export cuts
and speed changes. Independent frontend references and public speech fixtures
check the neural path; sound-quality parity with Screen Studio still requires
matched output and listening evaluation.

Architecture baseline before the neural implementation, 2026-10-26: `cargo test --release -- --test-threads=2` passed
490 tests, with 53 environment-specific tests left ignored. The isolated Xvfb
test `audio_preparation_is_project_scoped_and_does_not_lock_editing` also passed,
covering stale failure, project switching, queued Play/Pause, loading another
document, and closing during preparation. `cargo build --release` and
`git diff --check` passed. The installed binary was updated atomically without
restarting the running editor.

Neural implementation validation on 2026-10-26: `cargo test --release -j 1 --
--test-threads=1` passed 510 tests (469 application, 40 annotations, one public
API); 57 tests requiring explicit fixtures/devices/displays were ignored by that
command. All four new neural/fixture probes were exercised separately with
public speech inputs, including cancellation. The isolated Xvfb audio job
lifecycle test passed in 3.21 seconds. The five-mode integration test verifies
shared prepared audio across cuts, speed edits, media-bank boundaries, and
switching between both neural models. No user recording devices were opened.

`cargo build --release -j 1` and `git diff --check` passed. The release binary
was installed atomically without restarting the editor. Full-band native Podcast
validation also passed; its measured preparation cost and memory are higher than
the band-limited fixture, as recorded in the neural validation report.
