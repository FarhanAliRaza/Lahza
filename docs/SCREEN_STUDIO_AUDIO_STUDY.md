# Screen Studio 4.1 microphone processing study

Inspection date: 2026-10-26. Target: the user-supplied
`ignore/Screen Studio 4.1.0-5005 Intel.dmg`.

At the time of this investigation, Lahza did not match the inspected audio
pipeline. Its Reduce noise switch applied one FFT denoiser to the recording's combined audio stream.
Screen Studio declares separate microphone presets, including neural speech
enhancement, speech-aware gain control, dynamics, and EQ. The processed
microphone asset is intended to be shared by preview and export.

This is a static investigation, not a listening comparison. No vendor code was
executed, no microphone was opened, and no recording was uploaded. Production
audio code was not changed during this investigation.

Follow-up: the architecture gaps identified here are addressed in
[AUDIO_ARCHITECTURE.md](AUDIO_ARCHITECTURE.md). The comparison below records the
pre-fix implementation. The follow-up now includes public MossFormer2 and
TF-Restormer models, microphone presets, and supporting DSP. These do not prove
exact Screen Studio model weights or subjective sound-quality parity.

## Evidence and limits

REA inventoried and extracted the ASAR, then analyzed selected JavaScript
modules. String-table rotation was decoded as data with the existing static
decoder; native evidence comes from embedded descriptions, imports, and type
metadata. These establish declared behavior, not independently measured sound
quality or exact model weights.

| Artifact | SHA-256 |
| --- | --- |
| Supplied DMG | `52da4f405da480872c3e2b76253445657d4a001e1aec2608336df9a6136169a1` |
| `app.asar` | `69a28dbaca31fcf54bf0249019a9aaaed932525b77f6eec04691c1536a022876` |
| `dist/YEiLxxgG.js`, audio presets/assets | `539edc3c658facb1095c5b27ededd2846fbe24157aa10996c95904e504a4e338` |
| `dist/Cu5_9KY82.js`, microphone mode UI | `7fc1c0da2acd6561c718df7513468b3555f503ae550267cb42222ef1f72bef48` |
| `dist/C0lHueLa2.js`, preparation tasks | `fbbe98b13e2e8114b620dd7c4fcd4747ea7ed04f3d2850805a65a4cbc5b1b410` |
| `dist/B0echNca.js`, render task state | `b87a942ff09a623cfac01c9777d4da2954ac0e24939398c27d5a03de7810bfa2` |
| `bin/screen-studio-native`, observed bytes | `c6c4cf9e67219107f6b5d38d7e5aaf7103874c56cd240506226e459d7779fd94` |

Eight native unpacked files contradict their ASAR-declared hashes. REA retained
these contradictions using `record-and-continue`. The native observations below
refer to these observed, unverified bytes; they do not authenticate the vendor
binary. The cited JavaScript modules were not among those mismatches.

The focused semantic analysis reached its unknown limit. It is not a complete
call graph. The Electron main entry loads V8 cached bytecode; its supported-mode
query and speech-service installation logic were not reconstructed. In
particular, no download URL or exact checkpoint identity has been established.

Local evidence, intentionally outside version control:

- `ignore/screenstudio-audio-analysis/evidence.json`: exported REA ledger.
- `source-identities.json` and `extracted-identities.json` in that directory.
- `decoded/`: statically decoded selected modules.
- `native-audio-findings.md`: native offsets, symbols, and Lahza audit pointers.
- DMG summary: `ev_b263eefc78ff91193c8ef8bded5b6f173591ed95ed01e00676e94c9a789f640c`.
- Extraction: `ev_dded504f96a9f6c36393d123c6da02e027f5269eaa5d0a18d4dbc059488252e2`.
- JavaScript analysis: `ev_ee2ecdc18996d09ae75f3fcaf80b7d7a5f10d8e982616f8309921509ad76859a`.
- Analysis summary: `ev_4265ba0c31b374a5401f5481ad1dabfd0a2747601b289772762770bf5977a358`.

## Modes and platform availability

The microphone UI maps these internal IDs to labels. The backend's
`media.supportedMicrophoneAudioPresets` response determines which appear; finding
a preset in JavaScript does not mean every machine exposes it.

| UI label | ID | Ordered processing |
| --- | --- | --- |
| Off | `channel-fix` | Resample to 48 kHz; repair a sufficiently quiet stereo channel, with no explicit normalization peak target |
| Standard | `voice` | Resample to 48 kHz; peak normalization to -6 dB; quiet-channel repair |
| Loud | `loud` | Standard preparation; speech leveler; high-pass filter; dynamics; limiter; speech-aware pause attenuation |
| Noise reduction | `noise-reduction` | Standard preparation; **MossFormer2**; the Loud processing stages |
| Podcast, Beta | `podcast` | Standard preparation; speech-aware gating; **TF-Restormer**; sibilance restoration; spectrum matching; dynamic EQ; leveler; de-essing; high-pass filter; parametric EQ; limiter |

`voice` aliases the `normalized` pipeline. Quiet-channel repair uses
`silentChannelBelowDb: 30`. Standard is peak normalization, not the adaptive
speech leveler used by Loud and the advanced modes.

The preset renderer chooses `speechModels` when the chain contains
`mossFormer2` or `tfRestormer`; otherwise it chooses `nativeClient`.
The native embedded descriptions explicitly require the speech-models service,
macOS 14, and Apple silicon for both models. These paths are present in the
supplied Intel package but are not described as Intel-capable. The precise
runtime-supported list was not queried on macOS.

The [official homepage](https://screen.studio/) describes Standard, Noise
reduction, and Podcast, with processing performed locally after recording.
The [official changelog](https://preview.screen.studio/changelog) describes the
4.0 microphone-mode UI as unavailable on Intel and macOS 13 or earlier. That
changelog detail was available through its indexed excerpt; direct retrieval
returned the page shell. It is corroborating product documentation, not a
substitute for the 4.1 runtime query.

## Noise reduction details

After 48 kHz resampling and peak normalization, the declared chain is:

1. MossFormer2. The native description identifies noise and room-sound removal.
2. Speech leveler with `targetDb: -16`, gain range -20 to +30 dB, floor -60 dB,
   and `holdBelowSpeechDb: 35`. Window 400 ms, lookahead 50 ms, fall 150 ms,
   rise 400 ms. Native documentation says gain holds steady during pauses.
3. Apple high-pass AudioUnit, cutoff 70 Hz, resonance 0.
4. Apple dynamics AudioUnit: compression threshold -40, headroom 4, expansion
   threshold -50, expansion ratio 2, attack 0.005, release 0.12, master gain 25.
5. Apple limiter AudioUnit: attack 0.001, release 0.05, pre-gain 0.
6. Speech-aware keep stage: pause level -20 dB and fades of 400 ms.

The leveler's `targetDb` is not established as LUFS. Apple AudioUnit parameters
and their response must not be assumed equivalent to similarly named FFmpeg
parameters without verification.

Speech classification includes speech, whispering, shouting, singing, and
rapping. A second classifier preserves nearby human sounds such as breaths,
laughter, and sighs within 1,500 ms of speech. The Noise reduction/Loud keep
configuration uses speech confidence 0.1, nearby-sound confidence 0.35, and
`shortestCutMs: 1000`. The native documentation describes 1-second sound
classification windows every half second using Apple's built-in classifier.
This keep stage preserves timeline length; it attenuates or silences pauses.

## Podcast details

Podcast uses TF-Restormer in its declared chain, not an additional MossFormer2
stage. Native documentation describes restoring speech bandwidth up to 17 kHz.
This is a different enhancement treatment from the Noise reduction preset.

After resampling and normalization:

1. Speech-aware keep: speech confidence 0.25, nearby-sound confidence 0.35,
   nearby range 1,500 ms, and `shortestCutMs: 3000`.
2. TF-Restormer: spectral cutoff detection `bandEndsBelowPeakDb: 60`;
   `pauseBelowSpeechDb: 30`; pause-level range -75 to -45 dB; shortest silence
   400 ms; 300 ms retained around speech; fades 10 ms.
3. Restore sibilance: detector 5–10 kHz, reference 300–3,000 Hz, fill 4–16 kHz;
   thresholds 3 dB over reference and 20 dB over floor; floor window 4 seconds
   with share 0.2; event length 40–200 ms; near speech 150 ms;
   `pauseBelowSpeechDb: 20`, `underRecordingDb: 12`, fades 30 ms. The native
   description says this synthesizes band-limited noise where processing
   removed an original S sound.
4. Match EQ: bounded correction -15 to +12 dB. Native documentation specifies
   25 third-octave bands from 63 Hz to 16 kHz, relative to 1 kHz. Target dB:
   `[-15.3,-1.3,4.6,5.9,4.7,7.7,6.7,6,6.2,8.2,6.9,4.3,2,2,-0.2,-4.3,-3.9,-6.9,-8.2,-7.3,-8.9,-7.8,-12.1,-20,-29.1]`.
5. Low-mid dynamic EQ: frame 2048; detector 250–500 Hz relative to 1–4 kHz;
   allowed 7.5 dB; ratio `Number.MAX_VALUE`; maximum cut 4 dB across 200–650 Hz;
   taper 0.4 octaves; attack 15 ms; release 80 ms.
6. Speech leveler: target -18 dB; gain range -30 to +30 dB; floor -68 dB;
   `holdBelowSpeechDb: 20`; window 1,000 ms; lookahead 50 ms; fall 800 ms;
   rise 2,000 ms.
7. De-essing dynamic EQ: frame 1024; detector 5–9 kHz; allowed -23 dB; ratio 4;
   maximum cut 8 dB across 3.5–12.5 kHz; taper 0.3 octaves; attack 1 ms;
   release 50 ms.
8. High-pass filter: 70 Hz, resonance 0.
9. Parametric EQ: 4 kHz, -3 dB, bandwidth 0.3 in the AudioUnit's parameter units.
10. Limiter: attack 0.002, release 0.1, pre-gain 0.

Configuration alone cannot reproduce the model weights, classifier behavior,
Apple DSP implementations, or the resulting perceptual quality.

## Processing lifecycle and UI

The renderer treats microphone audio separately from system/external-device
audio. It prepares each microphone session with the selected preset through
`media.renderAudio`. The UI stores the selection in both project settings and
client preferences and provides task progress and pause/resume behavior.

For an unmuted microphone, Standard preparation is started as a baseline.
Muting cancels the preset jobs. Advanced presets can use the
ready Standard asset while processing, falling back to the original if needed.
`bestReadyPath` selects a completed processed file; `getOrRenderPath` awaits
processing and attempts fallback on failure.

Output names contain a hash of the serialized processor configuration, giving
different chains distinct cached assets. This is a noncryptographic rolling
hash, not proof of source-content or model-version cache invalidation.

Native embedded documentation describes:

- A processed microphone copy used by both editor preview and export.
- Processing in a separate process to avoid competing with editor video work
  on its main process.
- Ordered stages producing 48 kHz stereo AAC.
- A partial file, one writer per destination, progress every whole percent,
  and cancellation at block boundaries with resumable work preserved.
- Shared audio composition for editor playback and export.

These lifecycle details are static documentation and corroborating renderer
behavior, not a cancellation/performance test on macOS.

## Lahza comparison and implementation work

| Area | Lahza evidence at the initial audit | Gap at that time |
| --- | --- | --- |
| Cleaning | `src/recording/video.rs:293`: `afftdn=nr=20:tn=1` | One FFT noise-reduction filter; no equivalent neural enhancement, speech leveler, speech classifier, room-sound model, or Podcast chain |
| Capture separation | `src/recording/native.rs:1377`: microphone and system sources feed one mixer and 160 kbps Opus stream | Cannot clean or level the microphone independently after these sources are mixed |
| Controls | `src/scene_ui.rs:2731`: Reduce noise boolean | No Standard/Loud/Noise reduction/Podcast selection or per-microphone processing |
| Preview | `src/recording/video.rs:298`: denoise full source, copy video, re-encode audio to Opus | Extra lossy audio generation and video copying for an audio-only operation |
| Export | `src/recording/export.rs:502`: clip composition then denoise, then mix added audio | Adaptive denoiser sees different history/order than preview; it does not reuse the prepared microphone samples |
| Final mix | `src/recording/export.rs:539`: limiter after mixing | Peak protection exists, but is not speech leveling |
| Job handling | `src/video.rs:662`: pause playback, background command, busy state | No progress or cancellation handle; error/completion state lacks consistent project-identity checks |
| Cache | `src/video.rs:651` and `src/recording/video.rs:298`: existence checks | No source/configuration/model-version validation |
| Validation | Existing export/cache tests | No matched noisy-speech quality benchmark or processed preview/export sample comparison |

The export path uses the original source; this audit did not find a
double-denoising bug. The discrepancy is processing order, different adaptive
history, and the additional preview encode. Imported music/narration layers
bypass the current cleaning filter.

To close the gap completely:

1. Preserve microphone and system audio separately in new captures, including
   pause/resume, recovery, project save/load, and export. Existing combined
   recordings need an explicit legacy path: independent original tracks cannot
   be recovered exactly from their mix.
2. Implement explicit microphone modes backed by an evaluated local speech
   enhancement model and speech-aware DSP. Verify model and runtime licensing,
   CPU/GPU support, latency, and sound quality before shipping a mode.
3. Build one versioned processed microphone asset per source/preset, consumed
   by both preview and export before timeline edits and final mixing.
4. Add cancellable/resumable work, useful progress, safe fallback, and
   project-scoped job completion. Version source, model, and DSP configuration
   in cache identities.
5. Validate voice preservation, fan/keyboard noise, room reverberation, breaths,
   silence, stereo microphones, clipping, cut boundaries, speed changes, and
   system-audio preservation. Compare preview/export audio and conduct listening
   A/B tests. Recovered parameter names alone are not a quality-parity result.

Original UTF-8 byte starts in `YEiLxxgG.js`: AudioUnit descriptor `G` 49861,
renderer selection `q` 52870, shared normalization `J` 53330, classifiers `It`
53380, shared Loud/Noise reduction chain `Ht` 54411, presets `Ut` 54851,
audio-asset wrapper class `nn` 69983. Its render bootstrap is assigned later in
the same file. These differ from decoded character offsets.

Key native file offsets: classifier `0x529000`, normalization `0x529240`,
match EQ `0x5292e0`, leveler `0x529430`, keep/gate `0x529490`, sibilance
`0x5295a0`, TF-Restormer `0x529640`, MossFormer2 `0x529820`, shared processed
microphone copy `0x529c60`, job lifecycle `0x529d60`, audio composition
`0x52a140`.
