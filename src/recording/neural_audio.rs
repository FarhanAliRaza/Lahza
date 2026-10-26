//! Local MossFormer2 speech enhancement. Model provenance is in assets/audio/README.md.
//! Four-second inference windows and streaming PCM I/O bound memory independently
//! of recording length; the caller owns atomic publication of the output file.
use rustfft::{num_complex::Complex32, Fft, FftPlanner};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    sync::{Arc, Mutex, OnceLock, TryLockError},
    time::Duration,
};

pub const MODEL_ID: &str = "mossformer2-se48k-int8-2f98c2b3-v1";
pub const MODEL_SHA256: &str = "2f98c2b3d16cd30f9367181a4328a813a5455121267138836f25218467957b54";
const MODEL: &[u8] = include_bytes!("../../assets/audio/mossformer2-se-48k-int8.onnx");
#[used]
static LICENSE: &str = include_str!("../../assets/audio/LICENSE");
const RATE: usize = 48_000;
const WINDOW: usize = RATE * 4;
const OVERLAP: usize = RATE;
const STRIDE: usize = WINDOW - OVERLAP;
const FRAME: usize = 1920;
const HOP: usize = 384;
const FRAMES: usize = (WINDOW - FRAME) / HOP + 1;
const MEL_FFT: usize = 2048;
const MELS: usize = 60;
const BINS: usize = FRAME / 2 + 1;

struct Frontend {
    window: Vec<f32>,
    mel: Vec<Vec<(usize, f32)>>,
    fbank_fft: Arc<dyn Fft<f32>>,
    analysis_fft: Arc<dyn Fft<f32>>,
    synthesis_fft: Arc<dyn Fft<f32>>,
}
impl Frontend {
    fn new() -> Self {
        let mut planner = FftPlanner::new();
        let window = (0..FRAME)
            .map(|i| {
                0.54 - 0.46 * (2.0 * std::f32::consts::PI * i as f32 / (FRAME - 1) as f32).cos()
            })
            .collect();
        let mel_hz = |hz: f32| 1127.0 * (1.0 + hz / 700.0).ln();
        let lo = mel_hz(20.0);
        let step = (mel_hz(RATE as f32 / 2.0) - lo) / (MELS + 1) as f32;
        let mel = (0..MELS)
            .map(|band| {
                let center = lo + (band + 1) as f32 * step;
                // Kaldi's Nyquist bin is zero-padded after filter construction.
                (0..MEL_FFT / 2)
                    .filter_map(|bin| {
                        let weight = (1.0
                            - (mel_hz(bin as f32 * RATE as f32 / MEL_FFT as f32) - center).abs()
                                / step)
                            .max(0.0);
                        (weight > 0.0).then_some((bin, weight))
                    })
                    .collect()
            })
            .collect();
        Self {
            window,
            mel,
            fbank_fft: planner.plan_fft_forward(MEL_FFT),
            analysis_fft: planner.plan_fft_forward(FRAME),
            synthesis_fft: planner.plan_fft_inverse(FRAME),
        }
    }

    fn features(&self, audio: &[f32]) -> Vec<f32> {
        debug_assert_eq!(audio.len(), WINDOW);
        let mut fbanks = vec![0.0; FRAMES * MELS];
        let mut spectrum = vec![Complex32::default(); MEL_FFT];
        let mut frame = vec![0.0; FRAME];
        for time in 0..FRAMES {
            let slice = &audio[time * HOP..time * HOP + FRAME];
            let mean = slice.iter().copied().sum::<f32>() / FRAME as f32;
            for (out, &value) in frame.iter_mut().zip(slice) {
                *out = value - mean;
            }
            for i in (1..FRAME).rev() {
                frame[i] -= 0.97 * frame[i - 1];
            }
            frame[0] *= 0.03;
            spectrum.fill(Complex32::default());
            for i in 0..FRAME {
                spectrum[i].re = frame[i] * self.window[i];
            }
            self.fbank_fft.process(&mut spectrum);
            for band in 0..MELS {
                let energy: f32 = self.mel[band]
                    .iter()
                    .map(|&(bin, weight)| spectrum[bin].norm_sqr() * weight)
                    .sum();
                fbanks[time * MELS + band] = energy.max(f32::EPSILON).ln();
            }
        }
        let first = deltas(&fbanks);
        let second = deltas(&first);
        let mut features = vec![0.0; FRAMES * MELS * 3];
        for time in 0..FRAMES {
            for (order, values) in [&fbanks, &first, &second].iter().enumerate() {
                features[time * MELS * 3 + order * MELS..time * MELS * 3 + (order + 1) * MELS]
                    .copy_from_slice(&values[time * MELS..(time + 1) * MELS]);
            }
        }
        features
    }

    fn reconstruct(&self, audio: &[f32], mask: &[f32]) -> Vec<f32> {
        let mut out = vec![0.0; WINDOW];
        let mut weights = vec![0.0; WINDOW];
        let mut spectrum = vec![Complex32::default(); FRAME];
        for time in 0..FRAMES {
            let offset = time * HOP;
            for i in 0..FRAME {
                spectrum[i] = Complex32::new(audio[offset + i] * self.window[i], 0.0);
            }
            self.analysis_fft.process(&mut spectrum);
            for frequency in 0..BINS {
                spectrum[frequency] *= mask[time * BINS + frequency];
            }
            for frequency in 1..FRAME / 2 {
                spectrum[FRAME - frequency] = spectrum[frequency].conj();
            }
            self.synthesis_fft.process(&mut spectrum);
            for i in 0..FRAME {
                out[offset + i] += spectrum[i].re / FRAME as f32 * self.window[i];
                weights[offset + i] += self.window[i] * self.window[i];
            }
        }
        for (value, weight) in out.iter_mut().zip(weights) {
            *value /= weight.max(1e-12);
        }
        out
    }
}

fn deltas(values: &[f32]) -> Vec<f32> {
    let mut out = vec![0.0; values.len()];
    for time in 0..FRAMES {
        for band in 0..MELS {
            for distance in 1..=2 {
                let previous = time.saturating_sub(distance);
                let next = (time + distance).min(FRAMES - 1);
                out[time * MELS + band] += distance as f32
                    * (values[next * MELS + band] - values[previous * MELS + band])
                    / 10.0;
            }
        }
    }
    out
}

struct Enhancer {
    frontend: Frontend,
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    session: ort::session::Session,
}
impl Enhancer {
    fn load() -> Result<Self, String> {
        if format!("{:x}", Sha256::digest(MODEL)) != MODEL_SHA256 {
            return Err("Bundled speech-enhancement model checksum mismatch".into());
        }
        #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
        {
            Ok(Self {
                frontend: Frontend::new(),
                session: super::camera_matting_runtime::load(MODEL)?,
            })
        }
        #[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
        {
            Err("Neural speech enhancement currently requires Linux x86_64".into())
        }
    }

    fn window(
        &mut self,
        samples: &[f32],
        gain: f32,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Vec<f32>, String> {
        check(cancelled)?;
        if samples.iter().all(|sample| sample.abs() < 1e-9) {
            return Ok(vec![0.0; WINDOW]);
        }
        let scale = gain * 32768.0;
        let scaled: Vec<_> = samples.iter().map(|value| value * scale).collect();
        let features = self.frontend.features(&scaled);
        #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
        {
            let tensor = ort::value::Tensor::from_array(([1usize, FRAMES, MELS * 3], features))
                .map_err(|error| error.to_string())?;
            let options = ort::session::RunOptions::new().map_err(|error| error.to_string())?;
            let session = &mut self.session;
            // Keep the cancellation callback on its caller's thread, while ORT
            // runs with an interruptible per-window token. Slow CPUs must not
            // make switching a microphone preset wait for a whole inference.
            let mask = std::thread::scope(|scope| {
                let options_ref = &options;
                let handle = scope.spawn(move || -> Result<Vec<f32>, String> {
                    let result = session
                        .run_with_options(ort::inputs![tensor], options_ref)
                        .map_err(|error| format!("Speech enhancement inference failed: {error}"))?;
                    let (shape, mask) = result[0]
                        .try_extract_tensor::<f32>()
                        .map_err(|error| error.to_string())?;
                    if shape[..] != [1, FRAMES as i64, BINS as i64]
                        || mask.iter().any(|value| !value.is_finite())
                    {
                        return Err("Speech model returned an invalid mask".into());
                    }
                    Ok(mask.to_vec())
                });
                while !handle.is_finished() {
                    if cancelled() {
                        let _ = options.terminate();
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                let result = handle
                    .join()
                    .map_err(|_| "Speech enhancement worker panicked".to_string())?;
                check(cancelled)?;
                result
            })?;
            let mut audio = self.frontend.reconstruct(&scaled, &mask);
            for value in &mut audio {
                *value /= scale;
            }
            if audio.iter().any(|value| !value.is_finite()) {
                return Err("Speech enhancement produced invalid samples".into());
            }
            Ok(audio)
        }
        #[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
        {
            let _ = features;
            Err("Neural speech enhancement currently requires Linux x86_64".into())
        }
    }
}

fn engine() -> &'static Mutex<Option<Enhancer>> {
    static ENGINE: OnceLock<Mutex<Option<Enhancer>>> = OnceLock::new();
    ENGINE.get_or_init(|| Mutex::new(None))
}

fn check(cancelled: &dyn Fn() -> bool) -> Result<(), String> {
    if cancelled() {
        Err("Audio enhancement cancelled".into())
    } else {
        Ok(())
    }
}

fn read_samples(file: &mut File, frames: usize, channels: usize) -> Result<Vec<f32>, String> {
    let mut bytes = vec![0; frames * channels * 4];
    file.read_exact(&mut bytes)
        .map_err(|error| error.to_string())?;
    let values: Vec<_> = bytes
        .chunks_exact(4)
        .map(|bytes| f32::from_le_bytes(bytes.try_into().unwrap()))
        .collect();
    if values.iter().any(|value| !value.is_finite()) {
        return Err("Audio contains non-finite samples".into());
    }
    Ok(values)
}

/// Input and output are interleaved, little-endian 48 kHz f32 PCM. Both paths
/// must differ. The output has exactly the input's frame count and channels.
pub fn enhance_file(
    input: &Path,
    output: &Path,
    channels: usize,
    cancelled: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(f64),
) -> Result<(), String> {
    check(cancelled)?;
    if !(1..=2).contains(&channels) {
        return Err("Speech enhancement expects mono or stereo audio".into());
    }
    let same_path = input == output
        || output
            .canonicalize()
            .ok()
            .is_some_and(|path| input.canonicalize().ok().as_ref() == Some(&path));
    #[cfg(unix)]
    let same_file = {
        use std::os::unix::fs::MetadataExt;
        match (std::fs::metadata(input), std::fs::metadata(output)) {
            (Ok(input), Ok(output)) => input.dev() == output.dev() && input.ino() == output.ino(),
            _ => false,
        }
    };
    #[cfg(not(unix))]
    let same_file = false;
    if same_path || same_file {
        return Err("Speech enhancement output must differ from its input".into());
    }
    let mut source = File::open(input).map_err(|error| error.to_string())?;
    let bytes = source.metadata().map_err(|error| error.to_string())?.len();
    if bytes % (4 * channels) as u64 != 0 {
        return Err("Audio has an incomplete PCM frame".into());
    }
    let total = usize::try_from(bytes / (4 * channels) as u64)
        .map_err(|_| "Audio is too long for this platform")?;
    let mut sums = vec![0.0_f64; channels];
    let mut count = 0;
    while count < total {
        check(cancelled)?;
        let frames = (total - count).min(RATE * 5);
        for (i, value) in read_samples(&mut source, frames, channels)?
            .iter()
            .enumerate()
        {
            sums[i % channels] += f64::from(*value).powi(2);
        }
        count += frames;
        progress(0.025 * count as f64 / total.max(1) as f64);
    }
    let mean: Vec<_> = sums
        .iter()
        .map(|value| value / total.max(1) as f64)
        .collect();
    sums.fill(0.0);
    let mut above = vec![0usize; channels];
    source.rewind().map_err(|error| error.to_string())?;
    count = 0;
    while count < total {
        check(cancelled)?;
        let frames = (total - count).min(RATE * 5);
        for (i, value) in read_samples(&mut source, frames, channels)?
            .iter()
            .enumerate()
        {
            let channel = i % channels;
            let energy = f64::from(*value).powi(2);
            if energy > mean[channel] {
                sums[channel] += energy;
                above[channel] += 1;
            }
        }
        count += frames;
        progress(0.025 + 0.025 * count as f64 / total.max(1) as f64);
    }
    // Match the upstream active-sample normalization while restoring the
    // original gain after enhancement. This is not the editor voice leveler.
    let gain: Vec<_> = (0..channels)
        .map(|channel| {
            let rms = if above[channel] > 0 {
                (sums[channel] / above[channel] as f64).sqrt()
            } else {
                mean[channel].sqrt()
            };
            (10f64.powf(-25.0 / 20.0) / (rms + 1e-6)).clamp(0.001, 1000.0) as f32
        })
        .collect();
    let mut target = File::create(output).map_err(|error| error.to_string())?;
    if total == 0 {
        progress(1.0);
        return Ok(());
    }
    let mut shared = loop {
        check(cancelled)?;
        match engine().try_lock() {
            Ok(guard) => break guard,
            Err(TryLockError::WouldBlock) => std::thread::sleep(Duration::from_millis(20)),
            Err(TryLockError::Poisoned(error)) => break error.into_inner(),
        }
    };
    if shared.is_none() {
        *shared = Some(Enhancer::load()?);
    }
    let enhancer = shared.as_mut().unwrap();
    let mut start = 0;
    let mut tail: Option<Vec<f32>> = None;
    loop {
        check(cancelled)?;
        let frames = (total - start).min(WINDOW);
        source
            .seek(SeekFrom::Start((start * channels * 4) as u64))
            .map_err(|error| error.to_string())?;
        let values = read_samples(&mut source, frames, channels)?;
        let mut enhanced = vec![0.0; frames * channels];
        for channel in 0..channels {
            check(cancelled)?;
            // Captured mono microphones are commonly duplicated into stereo.
            // Reuse identical inference without changing true stereo content.
            if channel == 1
                && gain[0].to_bits() == gain[1].to_bits()
                && values
                    .chunks_exact(2)
                    .all(|frame| frame[0].to_bits() == frame[1].to_bits())
            {
                for frame in 0..frames {
                    enhanced[frame * 2 + 1] = enhanced[frame * 2];
                }
                continue;
            }
            let mut mono = vec![0.0; WINDOW];
            for frame in 0..frames {
                mono[frame] = values[frame * channels + channel];
            }
            let output = enhancer.window(&mono, gain[channel], cancelled)?;
            for frame in 0..frames {
                enhanced[frame * channels + channel] = output[frame];
            }
        }
        check(cancelled)?;
        if let Some(previous) = tail.take() {
            for frame in 0..OVERLAP.min(frames) {
                let mix = frame as f32 / OVERLAP as f32;
                for channel in 0..channels {
                    let i = frame * channels + channel;
                    enhanced[i] = previous[i] * (1.0 - mix) + enhanced[i] * mix;
                }
            }
        }
        let last = start + frames == total;
        let emit = if last { frames } else { STRIDE };
        let mut bytes = Vec::with_capacity(emit * channels * 4);
        for value in &enhanced[..emit * channels] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        target
            .write_all(&bytes)
            .map_err(|error| error.to_string())?;
        progress(0.05 + 0.95 * (start + emit) as f64 / total as f64);
        if last {
            break;
        }
        tail = Some(enhanced[STRIDE * channels..].to_vec());
        start += STRIDE;
    }
    target.flush().map_err(|error| error.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(unix)]
    fn hardlinked_output_is_rejected_before_truncating_input() {
        let directory = std::env::temp_dir().join(format!(
            "lahza-neural-hardlink-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        struct RemoveDirectory(std::path::PathBuf);
        impl Drop for RemoveDirectory {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = RemoveDirectory(directory.clone());
        let input = directory.join("input.f32");
        let output = directory.join("alias.f32");
        let original = 0.25_f32.to_le_bytes();
        std::fs::write(&input, original).unwrap();
        std::fs::hard_link(&input, &output).unwrap();
        let result = enhance_file(&input, &output, 1, &|| false, &mut |_| {});
        assert!(result.unwrap_err().contains("must differ"));
        assert_eq!(std::fs::read(&input).unwrap(), original);
    }

    #[test]
    fn spectral_synthesis_preserves_timing_gain_and_all_window_samples() {
        let frontend = Frontend::new();
        let audio: Vec<_> = (0..WINDOW)
            .map(|i| ((i as f32 * 0.173).sin() + (i as f32 * 0.039).cos()) * 0.2)
            .collect();
        let output = frontend.reconstruct(&audio, &vec![1.0; FRAMES * BINS]);
        assert_eq!(output.len(), audio.len());
        assert!(output
            .iter()
            .zip(&audio)
            .all(|(a, b)| (a - b).abs() < 0.000003));
    }
    #[test]
    fn native_features_match_independent_torchaudio_kaldi_reference() {
        let mut state = 1u32;
        let audio: Vec<_> = (0..WINDOW)
            .map(|_| {
                state = state.wrapping_mul(1664525).wrapping_add(1013904223);
                ((state >> 8) as f32 / 16777216.0 - 0.5) * 4000.0
            })
            .collect();
        let actual = Frontend::new().features(&audio);
        let golden = include_bytes!("../../assets/audio/fbank-reference.f32");
        let expected: Vec<_> = golden
            .chunks_exact(4)
            .map(|bytes| f32::from_le_bytes(bytes.try_into().unwrap()))
            .collect();
        let selected: Vec<_> = [0, 1, 247, 494, 495]
            .into_iter()
            .flat_map(|frame| {
                actual[frame * MELS * 3..(frame + 1) * MELS * 3]
                    .iter()
                    .copied()
            })
            .collect();
        assert_eq!(selected.len(), expected.len());
        let error = selected
            .iter()
            .zip(expected)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0_f32, f32::max);
        assert!(
            error < 0.003,
            "Kaldi frontend differs from torchaudio: {error}"
        );
    }
    #[test]
    #[ignore = "real model integration/benchmark; requires explicit PCM input/output environment paths"]
    fn neural_audio_file_probe() {
        let input = std::env::var_os("LAHZA_NEURAL_AUDIO_INPUT").expect("explicit PCM input");
        let output = std::env::var_os("LAHZA_NEURAL_AUDIO_OUTPUT").expect("explicit PCM output");
        let channels = std::env::var("LAHZA_NEURAL_AUDIO_CHANNELS")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(1);
        let start = std::time::Instant::now();
        enhance_file(
            Path::new(&input),
            Path::new(&output),
            channels,
            &|| false,
            &mut |_| {},
        )
        .unwrap();
        eprintln!(
            "MossFormer2 runtime: {:.3} s",
            start.elapsed().as_secs_f64()
        );
        assert_eq!(
            std::fs::metadata(input).unwrap().len(),
            std::fs::metadata(output).unwrap().len()
        );
    }

    #[test]
    #[ignore = "loads the bundled model; run serially with the neural validation tests"]
    fn cancelled_inference_does_not_poison_the_next_request() {
        use std::cell::Cell;
        let directory = std::env::temp_dir().join(format!(
            "lahza-neural-cancel-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        struct RemoveDirectory(std::path::PathBuf);
        impl Drop for RemoveDirectory {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = RemoveDirectory(directory.clone());
        let input = directory.join("input.f32");
        let cancelled_output = directory.join("cancelled.f32");
        let output = directory.join("complete.f32");
        let mut samples = Vec::with_capacity(WINDOW * 4);
        for i in 0..WINDOW {
            let value = (i as f32 * 0.0576).sin() * 0.1;
            samples.extend_from_slice(&value.to_le_bytes());
        }
        std::fs::write(&input, samples).unwrap();
        let inference_ready = Cell::new(false);
        let checks = Cell::new(0usize);
        let cancelled = || {
            if !inference_ready.get() {
                return false;
            }
            checks.set(checks.get() + 1);
            // Lock acquisition, window and channel checks happen first. The
            // later polling checks terminate an inference already in flight.
            checks.get() > 6
        };
        let result = enhance_file(&input, &cancelled_output, 1, &cancelled, &mut |fraction| {
            if fraction >= 0.05 {
                inference_ready.set(true);
            }
        });
        assert!(result.unwrap_err().contains("cancelled"));
        enhance_file(&input, &output, 1, &|| false, &mut |_| {}).unwrap();
        assert_eq!(
            std::fs::metadata(&input).unwrap().len(),
            std::fs::metadata(&output).unwrap().len()
        );
    }
}
