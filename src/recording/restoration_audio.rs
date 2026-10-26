//! TF-Restormer speech restoration, using the public corrected epoch-19 model.
//! The bundled ONNX stores weights as f16 and computes in f32 on the CPU.
//! See assets/audio/TF_RESTORMER.md for provenance and validation boundaries.
use rustfft::{num_complex::Complex32, Fft, FftPlanner};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    sync::{Arc, Mutex, OnceLock, TryLockError},
    time::Duration,
};

pub const MODEL_ID: &str = "tf-restormer-epoch19-f3f1cf15-v2-w1s";
const MODEL_SHA256: &str = "f3f1cf158d5eae68c2402e8022da829cec763c8ada3d8258ee514a1d662f8e31";
const MODEL: &[u8] = include_bytes!("../../assets/audio/tf-restormer-weights-f16.onnx");
#[used]
static LICENSE: &str = include_str!("../../assets/audio/LICENSE-TF-RESTORMER");
const RATE: usize = 48_000;
// Frequency-attention scores scale with the number of time frames. Keeping
// CPU windows to one second bounds this workspace even for long recordings.
const WINDOW: usize = RATE;
const OVERLAP: usize = RATE / 5;
const STRIDE: usize = WINDOW - OVERLAP;
const FRAME: usize = 1920;
const HOP: usize = 960;
const BINS: usize = FRAME / 2 + 1;

struct Frontend {
    window: Vec<f32>,
    analysis: Arc<dyn Fft<f32>>,
    synthesis: Arc<dyn Fft<f32>>,
}
impl Frontend {
    fn new() -> Self {
        let mut planner = FftPlanner::new();
        let window = (0..FRAME)
            .map(|i| {
                (0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / FRAME as f64).cos()).sqrt()
                    as f32
            })
            .collect();
        Self {
            window,
            analysis: planner.plan_fft_forward(FRAME),
            synthesis: planner.plan_fft_inverse(FRAME),
        }
    }
    fn analyze(&self, audio: &[f32]) -> (usize, usize, Vec<f32>) {
        // A hop of boundary context preserves the first/last samples. The
        // native FFT implements the author's normalized convolution kernels.
        let frames = audio.len().div_ceil(HOP) + 1;
        let mut all = vec![0.0; BINS * frames * 2];
        let mut profile = vec![0.0; BINS];
        let mut fft = vec![Complex32::default(); FRAME];
        let scale = (HOP as f32).sqrt();
        for time in 0..frames {
            for i in 0..FRAME {
                let at = (time * HOP + i).checked_sub(HOP);
                fft[i] = Complex32::new(
                    at.and_then(|j| audio.get(j)).copied().unwrap_or(0.0) * self.window[i],
                    0.0,
                );
            }
            self.analysis.process(&mut fft);
            for frequency in 0..BINS {
                let value = fft[frequency] / scale;
                let at = (frequency * frames + time) * 2;
                all[at] = value.re;
                all[at + 1] = value.im;
                profile[frequency] += value.norm();
            }
        }
        let threshold = profile.iter().copied().fold(0.0_f32, f32::max) * 0.001;
        // Always reserve at least the Nyquist bin for reconstruction. The
        // traced cross-attention graph requires a nonempty generated band;
        // omitting precisely 24kHz does not remove audible content.
        let frequencies = profile
            .iter()
            .rposition(|&v| v > threshold)
            .map_or(161, |i| i + 1)
            .clamp(161, BINS - 1);
        all.truncate(frequencies * frames * 2);
        (frequencies, frames, all)
    }
    fn reconstruct(&self, spectrum: &[f32], frames: usize, length: usize) -> Vec<f32> {
        let mut output = vec![0.0; (frames - 1) * HOP + FRAME];
        let mut fft = vec![Complex32::default(); FRAME];
        let scale = 2.0 * (HOP as f32).sqrt();
        for time in 0..frames {
            for frequency in 0..BINS {
                let at = (frequency * frames + time) * 2;
                fft[frequency] = Complex32::new(spectrum[at], spectrum[at + 1]);
            }
            // Upstream conv_transpose uses one-sided FFT kernels, with the
            // endpoint bins contributing twice their conventional iFFT weight.
            fft[0] *= 2.0;
            fft[BINS - 1] *= 2.0;
            for frequency in 1..BINS - 1 {
                fft[FRAME - frequency] = fft[frequency].conj();
            }
            self.synthesis.process(&mut fft);
            for i in 0..FRAME {
                output[time * HOP + i] += fft[i].re * self.window[i] / scale;
            }
        }
        output[HOP..HOP + length].to_vec()
    }
}

struct Enhancer {
    frontend: Frontend,
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    session: ort::session::Session,
}
impl Enhancer {
    fn load() -> Result<Self, String> {
        if format!("{:x}", Sha256::digest(MODEL)) != MODEL_SHA256 {
            return Err("Bundled restoration model checksum mismatch".into());
        }
        #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
        {
            Ok(Self {
                frontend: Frontend::new(),
                session: super::camera_matting_runtime::load_audio(MODEL)?,
            })
        }
        #[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
        {
            Err("Neural speech restoration currently requires Linux x86_64".into())
        }
    }
    fn window(&mut self, audio: &[f32], cancelled: &dyn Fn() -> bool) -> Result<Vec<f32>, String> {
        if audio.iter().all(|value| value.abs() < 1e-9) {
            return Ok(vec![0.0; audio.len()]);
        }
        let (frequencies, frames, values) = self.frontend.analyze(audio);
        #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
        {
            let tensor = ort::value::Tensor::from_array(([1usize, frequencies, frames, 2], values))
                .map_err(|e| e.to_string())?;
            let options = ort::session::RunOptions::new().map_err(|e| e.to_string())?;
            let session = &mut self.session;
            // The caller's cancellation closure stays on its calling thread.
            // ORT execution can be interrupted between graph operators even
            // when a CPU restoration window takes several seconds.
            let restored = std::thread::scope(|scope| {
                let options_ref = &options;
                let handle = scope.spawn(move || -> Result<Vec<f32>, String> {
                    let result = session
                        .run_with_options(ort::inputs![tensor], options_ref)
                        .map_err(|e| format!("Speech restoration inference failed: {e}"))?;
                    let (shape, values) = result[0]
                        .try_extract_tensor::<f32>()
                        .map_err(|e| e.to_string())?;
                    if shape[..] != [1, BINS as i64, frames as i64, 2]
                        || values.iter().any(|v| !v.is_finite())
                    {
                        return Err("Restoration model returned an invalid spectrum".into());
                    }
                    Ok(values.to_vec())
                });
                while !handle.is_finished() {
                    if cancelled() {
                        let _ = options.terminate();
                    }
                    std::thread::sleep(Duration::from_millis(30));
                }
                let result = handle
                    .join()
                    .map_err(|_| "Speech restoration worker panicked".to_string())?;
                check(cancelled)?;
                result
            })?;
            let output = self.frontend.reconstruct(&restored, frames, audio.len());
            if output.iter().any(|v| !v.is_finite()) {
                return Err("Speech restoration produced invalid samples".into());
            }
            Ok(output)
        }
        #[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
        {
            let _ = (frequencies, frames, values, cancelled);
            Err("Neural speech restoration currently requires Linux x86_64".into())
        }
    }
}
fn check(cancelled: &dyn Fn() -> bool) -> Result<(), String> {
    if cancelled() {
        Err("Audio enhancement cancelled".into())
    } else {
        Ok(())
    }
}
fn engine() -> &'static Mutex<Option<Enhancer>> {
    static ENGINE: OnceLock<Mutex<Option<Enhancer>>> = OnceLock::new();
    ENGINE.get_or_init(|| Mutex::new(None))
}
fn read_samples(file: &mut File, frames: usize, channels: usize) -> Result<Vec<f32>, String> {
    let mut bytes = vec![0; frames * channels * 4];
    file.read_exact(&mut bytes).map_err(|e| e.to_string())?;
    let values: Vec<_> = bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
        .collect();
    if values.iter().any(|v| !v.is_finite()) {
        return Err("Audio contains non-finite samples".into());
    }
    Ok(values)
}
/// Input/output are interleaved 48kHz f32 PCM; output duration and channels are exact.
/// Processing is bounded to a one-second window, independent of file duration.
pub fn enhance_file(
    input: &Path,
    output: &Path,
    channels: usize,
    cancelled: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(f64),
) -> Result<(), String> {
    check(cancelled)?;
    if !(1..=2).contains(&channels) {
        return Err("Speech restoration expects mono or stereo audio".into());
    }
    let same_path = input == output
        || output
            .canonicalize()
            .ok()
            .is_some_and(|p| input.canonicalize().ok().as_ref() == Some(&p));
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
        return Err("Speech restoration output must differ from its input".into());
    }
    let mut source = File::open(input).map_err(|e| e.to_string())?;
    let bytes = source.metadata().map_err(|e| e.to_string())?.len();
    if bytes % (4 * channels) as u64 != 0 {
        return Err("Audio has an incomplete PCM frame".into());
    }
    let total = usize::try_from(bytes / (4 * channels) as u64)
        .map_err(|_| "Audio is too long for this platform")?;
    let mut target = File::create(output).map_err(|e| e.to_string())?;
    if total == 0 {
        progress(1.0);
        return Ok(());
    }
    let mut shared = loop {
        check(cancelled)?;
        match engine().try_lock() {
            Ok(guard) => break guard,
            Err(TryLockError::WouldBlock) => std::thread::sleep(Duration::from_millis(20)),
            Err(TryLockError::Poisoned(e)) => break e.into_inner(),
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
            .map_err(|e| e.to_string())?;
        let values = read_samples(&mut source, frames, channels)?;
        let identical = channels == 2 && values.chunks_exact(2).all(|v| v[0] == v[1]);
        let mut enhanced = vec![0.0; frames * channels];
        for channel in 0..if identical { 1 } else { channels } {
            check(cancelled)?;
            let mono: Vec<_> = (0..frames)
                .map(|i| values[i * channels + channel])
                .collect();
            let restored = enhancer.window(&mono, cancelled)?;
            for i in 0..frames {
                enhanced[i * channels + channel] = restored[i];
            }
        }
        if identical {
            for frame in enhanced.chunks_exact_mut(2) {
                frame[1] = frame[0];
            }
        }
        if let Some(previous) = tail.take() {
            for frame in 0..OVERLAP.min(frames) {
                let mix = frame as f32 / OVERLAP as f32;
                for channel in 0..channels {
                    let i = frame * channels + channel;
                    enhanced[i] = previous[i] * (1.0 - mix) + enhanced[i] * mix;
                }
            }
        }
        check(cancelled)?;
        let last = start + frames == total;
        let emit = if last { frames } else { STRIDE };
        let mut bytes = Vec::with_capacity(emit * channels * 4);
        for value in &enhanced[..emit * channels] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        target.write_all(&bytes).map_err(|e| e.to_string())?;
        progress((start + emit) as f64 / total as f64);
        if last {
            break;
        }
        tail = Some(enhanced[STRIDE * channels..].to_vec());
        start += STRIDE;
    }
    target.flush().map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[cfg(unix)]
    fn hardlinked_output_is_rejected_before_truncating_input() {
        let directory = std::env::temp_dir().join(format!(
            "lahza-restoration-hardlink-{}-{}",
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
    fn analysis_detects_audible_band_and_preserves_tensor_layout() {
        let audio: Vec<_> = (0..RATE)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / RATE as f32).sin() * 0.1)
            .collect();
        let (frequencies, frames, spectrum) = Frontend::new().analyze(&audio);
        assert!((161..BINS).contains(&frequencies));
        assert_eq!(frames, 51);
        assert_eq!(spectrum.len(), frequencies * frames * 2);
        assert!(spectrum.iter().all(|v| v.is_finite()));
    }
    #[test]
    fn analysis_matches_the_upstream_normalized_convolution_kernel() {
        // Evaluate the author's convolution kernel directly for a sparse
        // signal. This independently checks FFT scaling, imaginary sign,
        // boundary padding, periodic Hann coefficients and tensor layout.
        let impulses = [(0, 0.2_f64), (617, -0.37), (1033, 0.11), (2046, 0.08)];
        let mut audio = vec![0.0; 2047];
        for &(at, value) in &impulses {
            audio[at] = value as f32;
        }
        let (frequencies, frames, spectrum) = Frontend::new().analyze(&audio);
        assert_eq!(frames, 4);
        for frequency in [0, 1, 17, 159, 319, 511, 959] {
            assert!(frequency < frequencies);
            for time in 0..frames {
                let mut expected = (0.0_f64, 0.0_f64);
                for &(at, value) in &impulses {
                    let local = at as isize + HOP as isize - (time * HOP) as isize;
                    if !(0..FRAME as isize).contains(&local) {
                        continue;
                    }
                    let angle = std::f64::consts::TAU * local as f64 / FRAME as f64;
                    let weight = (0.5 - 0.5 * angle.cos()).sqrt() / (HOP as f64).sqrt();
                    expected.0 += value * weight * (angle * frequency as f64).cos();
                    expected.1 -= value * weight * (angle * frequency as f64).sin();
                }
                let at = (frequency * frames + time) * 2;
                assert!((spectrum[at] as f64 - expected.0).abs() < 2e-8);
                assert!((spectrum[at + 1] as f64 - expected.1).abs() < 2e-8);
            }
        }
    }
    #[test]
    fn synthesis_matches_the_upstream_one_sided_transposed_kernel() {
        // Include DC and Nyquist: a conventional iFFT normalization would
        // incorrectly halve these bins, and a missing scale would amplify
        // every speech sample by roughly sqrt(960).
        let coefficients = [
            (0, 0, 0.11_f64, 0.0_f64),
            (7, 0, -0.13, 0.17),
            (439, 1, 0.02, -0.03),
            (BINS - 1, 1, 0.07, 0.0),
        ];
        let frames = 3;
        let mut spectrum = vec![0.0; BINS * frames * 2];
        for &(frequency, time, real, imaginary) in &coefficients {
            let at = (frequency * frames + time) * 2;
            spectrum[at] = real as f32;
            spectrum[at + 1] = imaginary as f32;
        }
        let output = Frontend::new().reconstruct(&spectrum, frames, FRAME);
        for at in [0, 1, 117, 501, 959, 960, 1227, 1919] {
            let mut expected = 0.0_f64;
            for &(frequency, time, real, imaginary) in &coefficients {
                let local = at as isize + HOP as isize - (time * HOP) as isize;
                if !(0..FRAME as isize).contains(&local) {
                    continue;
                }
                let angle = std::f64::consts::TAU * local as f64 / FRAME as f64;
                let weight = (0.5 - 0.5 * angle.cos()).sqrt() / (HOP as f64).sqrt();
                expected += weight
                    * (real * (angle * frequency as f64).cos()
                        - imaginary * (angle * frequency as f64).sin());
            }
            assert!((output[at] as f64 - expected).abs() < 2e-8);
        }
    }
    #[test]
    #[ignore = "real model integration/benchmark; requires explicit PCM paths"]
    fn restoration_audio_file_probe() {
        let input = std::env::var_os("LAHZA_RESTORATION_INPUT").expect("explicit PCM input");
        let output = std::env::var_os("LAHZA_RESTORATION_OUTPUT").expect("explicit PCM output");
        let channels = std::env::var("LAHZA_RESTORATION_CHANNELS")
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
            "TF-Restormer runtime: {:.3}s",
            start.elapsed().as_secs_f64()
        );
        assert_eq!(
            std::fs::metadata(input).unwrap().len(),
            std::fs::metadata(output).unwrap().len()
        );
    }
}
