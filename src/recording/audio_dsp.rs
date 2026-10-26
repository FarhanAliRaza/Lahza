//! Local speech post-processing, independent of the neural model runtime.
//!
//! The stage ordering and voice-level targets follow the recovered microphone
//! presets. These portable filters are not Apple's AudioUnits or SoundAnalysis:
//! speech activity uses energy, modulation, and periodicity, with nearby breaths
//! retained. In particular target dB here means linked-channel RMS, not LUFS.
//! Biquad equations follow https://www.w3.org/TR/audio-eq-cookbook/.
//!
//! Files contain little-endian f32 samples. Global scans use fixed histograms;
//! rendering retains at most four seconds of audio plus fixed filter state.
//! The neural model runs BETWEEN normalize_file and finish_file. This module
//! does not implement, emulate, or silently replace neural enhancement.
use super::audio_mode::AudioMode;
use std::{
    collections::VecDeque,
    fs::{self, File},
    io::{BufReader, BufWriter, Cursor, Read, Seek, SeekFrom, Write},
    path::Path,
};

pub const DSP_VERSION: &str = "speech-dsp-v3-conservative-leveler-48k";
const BLOCK_MS: usize = 20;
const CONTEXT_BLOCKS: usize = 200;
const BANDS: [f64; 25] = [
    63., 80., 100., 125., 160., 200., 250., 315., 400., 500., 630., 800., 1000., 1250., 1600.,
    2000., 2500., 3150., 4000., 5000., 6300., 8000., 10000., 12500., 16000.,
];
const PODCAST_TARGET: [f64; 25] = [
    -15.3, -1.3, 4.6, 5.9, 4.7, 7.7, 6.7, 6., 6.2, 8.2, 6.9, 4.3, 2., 2., -0.2, -4.3, -3.9, -6.9,
    -8.2, -7.3, -8.9, -7.8, -12.1, -20., -29.1,
];

#[derive(Clone, Debug, Default)]
pub struct NormalizationReport {
    pub frames: u64,
    pub input_peak: f32,
    pub output_peak: f32,
    pub gain_db: f32,
    pub repaired_channel: Option<usize>,
}
#[derive(Clone, Debug, Default)]
pub struct DspReport {
    pub frames: u64,
    pub input_peak: f32,
    pub output_peak: f32,
    pub active_fraction: f32,
    pub minimum_gain_db: f32,
    pub maximum_gain_db: f32,
    pub limited_frames: u64,
}
fn cancelled(check: &dyn Fn() -> bool) -> Result<(), String> {
    if check() {
        Err("Audio processing cancelled".into())
    } else {
        Ok(())
    }
}
fn validate(channels: usize, sample_rate: u32) -> Result<(), String> {
    if !(1..=2).contains(&channels) {
        return Err("Microphone processing requires mono or stereo audio".into());
    }
    if !(8000..=192000).contains(&sample_rate) {
        return Err("Unsupported microphone sample rate".into());
    }
    Ok(())
}
fn sample_count(path: &Path, channels: usize) -> Result<u64, String> {
    let bytes = fs::metadata(path).map_err(|e| e.to_string())?.len();
    if channels == 0 || bytes % (4 * channels) as u64 != 0 {
        return Err("Audio file ends inside a sample frame".into());
    }
    Ok(bytes / 4)
}
fn separate_output(input: &Path, output: &Path) -> Result<(), String> {
    let same_path = input == output
        || input
            .canonicalize()
            .ok()
            .is_some_and(|path| output.canonicalize().ok().as_ref() == Some(&path));
    #[cfg(unix)]
    let same_file = {
        use std::os::unix::fs::MetadataExt;
        match (fs::metadata(input), fs::metadata(output)) {
            (Ok(input), Ok(output)) => input.dev() == output.dev() && input.ino() == output.ino(),
            _ => false,
        }
    };
    #[cfg(not(unix))]
    let same_file = false;
    if same_path || same_file {
        Err("Audio processing needs a separate output file".into())
    } else {
        Ok(())
    }
}
fn read_samples(reader: &mut dyn Read, count: usize) -> Result<Vec<f32>, String> {
    let mut bytes = vec![0u8; count * 4];
    let mut length = 0;
    while length < bytes.len() {
        match reader.read(&mut bytes[length..]) {
            Ok(0) => break,
            Ok(n) => length += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.to_string()),
        }
    }
    if length % 4 != 0 {
        return Err("Audio file ends inside a sample".into());
    }
    let values: Vec<_> = bytes[..length]
        .chunks_exact(4)
        .map(|v| f32::from_le_bytes(v.try_into().unwrap()))
        .collect();
    if values.iter().any(|x| !x.is_finite()) {
        return Err("Audio contains non-finite samples".into());
    }
    Ok(values)
}
fn write_samples(writer: &mut dyn Write, samples: &[f32]) -> Result<(), String> {
    let mut bytes = Vec::with_capacity(samples.len() * 4);
    for value in samples {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    writer.write_all(&bytes).map_err(|e| e.to_string())
}
fn db(amplitude: f64) -> f64 {
    20.0 * amplitude.max(1e-12).log10()
}
fn amp(decibels: f64) -> f64 {
    10.0_f64.powf(decibels / 20.0)
}
fn coefficient(ms: f64, rate: f64) -> f64 {
    1.0 - (-1.0 / (ms.max(0.01) * 0.001 * rate)).exp()
}
fn fade_toward(value: f64, target: f64, step: f64) -> f64 {
    value + (target - value).clamp(-step, step)
}

/// Peak normalization and dead-channel repair precede inference. Off repairs
/// a dead/very quiet stereo side but leaves microphone gain untouched. Legacy
/// combined audio bypasses this stage when Off is selected. No normalization
/// is repeated by finish_file.
pub fn normalize_file(
    input: &Path,
    output: &Path,
    channels: usize,
    mode: AudioMode,
    check: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(f64),
) -> Result<NormalizationReport, String> {
    validate(channels, 48000)?;
    separate_output(input, output)?;
    let count = sample_count(input, channels)?;
    let mut reader = BufReader::new(File::open(input).map_err(|e| e.to_string())?);
    let mut peak = [0.0f64; 2];
    let mut energy = [0.0f64; 2];
    let mut seen = 0;
    loop {
        cancelled(check)?;
        let values = read_samples(&mut reader, 8192 * channels)?;
        if values.is_empty() {
            break;
        }
        for (i, x) in values.iter().enumerate() {
            let x = f64::from(*x);
            peak[i % channels] = peak[i % channels].max(x.abs());
            energy[i % channels] += x * x;
        }
        seen += values.len() as u64;
        progress(0.45 * seen as f64 / count.max(1) as f64);
    }
    if seen != count {
        return Err("Audio input changed during normalization".into());
    }
    let input_peak = peak.into_iter().fold(0.0, f64::max);
    let repaired = if channels == 2 {
        let quieter = usize::from(energy[1] < energy[0]);
        let louder = 1 - quieter;
        (energy[louder] > 1e-16 && energy[quieter] < energy[louder] * 0.001).then_some(quieter)
    } else {
        None
    };
    let gain = if mode == AudioMode::Off || input_peak < 1e-12 {
        1.0
    } else {
        amp(-6.0) / input_peak
    };
    reader.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    let mut writer = BufWriter::new(File::create(output).map_err(|e| e.to_string())?);
    seen = 0;
    loop {
        cancelled(check)?;
        let mut values = read_samples(&mut reader, 8192 * channels)?;
        if values.is_empty() {
            break;
        }
        for frame in values.chunks_exact_mut(channels) {
            if let Some(channel) = repaired {
                frame[channel] = frame[1 - channel];
            }
            for value in frame {
                *value = (f64::from(*value) * gain) as f32;
            }
        }
        seen += values.len() as u64;
        write_samples(&mut writer, &values)?;
        progress(0.45 + 0.55 * seen as f64 / count.max(1) as f64);
    }
    writer.flush().map_err(|e| e.to_string())?;
    if seen != count {
        return Err("Audio input changed during normalization".into());
    }
    cancelled(check)?;
    progress(1.0);
    Ok(NormalizationReport {
        frames: count / channels as u64,
        input_peak: input_peak as f32,
        output_peak: (input_peak * gain) as f32,
        gain_db: db(gain) as f32,
        repaired_channel: repaired,
    })
}

#[derive(Clone, Copy)]
struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
    z: [f64; 2],
}
impl Biquad {
    fn identity() -> Self {
        Self {
            b: [1., 0., 0.],
            a: [0., 0.],
            z: [0., 0.],
        }
    }
    fn new(b: [f64; 3], a: [f64; 3]) -> Self {
        Self {
            b: b.map(|x| x / a[0]),
            a: [a[1] / a[0], a[2] / a[0]],
            z: [0., 0.],
        }
    }
    fn lowpass(rate: f64, hz: f64) -> Self {
        Self::pass(rate, hz, false)
    }
    fn highpass(rate: f64, hz: f64) -> Self {
        Self::pass(rate, hz, true)
    }
    fn pass(rate: f64, hz: f64, high: bool) -> Self {
        let w = 2.0 * std::f64::consts::PI * hz.min(rate * 0.45) / rate;
        let c = w.cos();
        let alpha = w.sin() / 2.0_f64.sqrt();
        let b = if high {
            [(1. + c) / 2., -(1. + c), (1. + c) / 2.]
        } else {
            [(1. - c) / 2., 1. - c, (1. - c) / 2.]
        };
        Self::new(b, [1. + alpha, -2. * c, 1. - alpha])
    }
    fn bandpass(rate: f64, hz: f64, q: f64) -> Self {
        let w = 2.0 * std::f64::consts::PI * hz.min(rate * 0.45) / rate;
        let alpha = w.sin() / (2.0 * q);
        Self::new([alpha, 0., -alpha], [1. + alpha, -2. * w.cos(), 1. - alpha])
    }
    fn peak(rate: f64, hz: f64, q: f64, gain_db: f64) -> Self {
        if hz >= rate * 0.45 {
            return Self::identity();
        }
        let w = 2.0 * std::f64::consts::PI * hz / rate;
        let alpha = w.sin() / (2.0 * q);
        let a = amp(gain_db / 2.0);
        Self::new(
            [1. + alpha * a, -2. * w.cos(), 1. - alpha * a],
            [1. + alpha / a, -2. * w.cos(), 1. - alpha / a],
        )
    }
    fn tick(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.z[0];
        self.z[0] = self.b[1] * x - self.a[0] * y + self.z[1];
        self.z[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}

#[derive(Clone, Copy, Default)]
struct Features {
    rms: f64,
    periodicity: f64,
    high: f64,
    mid: f64,
    low_mid: f64,
    upper_mid: f64,
}
fn features(samples: &[f32], channels: usize, rate: u32) -> Features {
    if samples.is_empty() {
        return Features::default();
    }
    let energy: f64 = samples.iter().map(|x| f64::from(*x).powi(2)).sum();
    // Analyze the stronger channel instead of cancelling antiphase stereo.
    let channel = if channels == 2 {
        let mut e = [0.; 2];
        for frame in samples.chunks_exact(2) {
            e[0] += f64::from(frame[0]).powi(2);
            e[1] += f64::from(frame[1]).powi(2);
        }
        usize::from(e[1] > e[0])
    } else {
        0
    };
    let decimate = (rate as usize / 8000).max(1);
    let mono: Vec<f64> = samples
        .chunks_exact(channels)
        .step_by(decimate)
        .map(|frame| f64::from(frame[channel]))
        .collect();
    let effective_rate = rate as usize / decimate;
    let mut periodicity = 0.0f64;
    for lag in
        (effective_rate / 500).max(1)..=(effective_rate / 80).min(mono.len().saturating_sub(16))
    {
        let (mut xy, mut xx, mut yy) = (0., 0., 0.);
        for i in lag..mono.len() {
            xy += mono[i] * mono[i - lag];
            xx += mono[i] * mono[i];
            yy += mono[i - lag] * mono[i - lag];
        }
        if xx * yy > 1e-20 {
            periodicity = periodicity.max(xy / (xx * yy).sqrt());
        }
    }
    let mut high = Biquad::bandpass(f64::from(rate), 7000., 1.4);
    let mut mid = Biquad::bandpass(f64::from(rate), 1000., 0.6);
    let mut low_mid = (
        Biquad::highpass(f64::from(rate), 250.),
        Biquad::lowpass(f64::from(rate), 500.),
    );
    let mut upper_mid = (
        Biquad::highpass(f64::from(rate), 1000.),
        Biquad::lowpass(f64::from(rate), 4000.),
    );
    let (mut h, mut m, mut lo, mut hi) = (0., 0., 0., 0.);
    for frame in samples.chunks_exact(channels) {
        let x = f64::from(frame[channel]);
        h += high.tick(x).powi(2);
        m += mid.tick(x).powi(2);
        lo += low_mid.1.tick(low_mid.0.tick(x)).powi(2);
        hi += upper_mid.1.tick(upper_mid.0.tick(x)).powi(2);
    }
    let frames = (samples.len() / channels).max(1) as f64;
    Features {
        rms: (energy / samples.len() as f64).sqrt(),
        periodicity,
        high: (h / frames).sqrt(),
        mid: (m / frames).sqrt(),
        low_mid: (lo / frames).sqrt(),
        upper_mid: (hi / frames).sqrt(),
    }
}

struct Analysis {
    histogram: [u64; 121],
    voiced: [u64; 121],
    peak: f64,
    band_energy: [f64; 25],
    samples: u64,
}
impl Default for Analysis {
    fn default() -> Self {
        Self {
            histogram: [0; 121],
            voiced: [0; 121],
            peak: 0.,
            band_energy: [0.; 25],
            samples: 0,
        }
    }
}
fn percentile(histogram: &[u64; 121], q: f64) -> f64 {
    let total: u64 = histogram.iter().sum();
    if total == 0 {
        return -120.0;
    }
    let threshold = (total as f64 * q).ceil() as u64;
    let mut n = 0;
    for (i, count) in histogram.iter().enumerate() {
        n += count;
        if n >= threshold.max(1) {
            return i as f64 - 120.;
        }
    }
    0.
}
fn scan(
    reader: &mut dyn Read,
    rate: u32,
    channels: usize,
    mode: AudioMode,
    count: u64,
    check: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(f64),
) -> Result<Analysis, String> {
    let mut result = Analysis::default();
    let mut bands: Vec<Vec<_>> = (0..channels)
        .map(|_| {
            BANDS
                .iter()
                .map(|hz| Biquad::bandpass(f64::from(rate), *hz, 4.318))
                .collect()
        })
        .collect();
    loop {
        cancelled(check)?;
        let values = read_samples(reader, rate as usize * BLOCK_MS / 1000 * channels)?;
        if values.is_empty() {
            break;
        }
        let f = features(&values, channels, rate);
        let slot = (db(f.rms).round() + 120.).clamp(0., 120.) as usize;
        result.histogram[slot] += 1;
        if f.periodicity > 0.38 {
            result.voiced[slot] += 1;
        }
        result.peak = result.peak.max(
            values
                .iter()
                .map(|x| f64::from(*x).abs())
                .fold(0., f64::max),
        );
        if mode == AudioMode::Podcast {
            for frame in values.chunks_exact(channels) {
                // Average channel energies, not antiphase waveforms.
                for (channel, x) in frame.iter().enumerate() {
                    for (i, band) in bands[channel].iter_mut().enumerate() {
                        result.band_energy[i] += band.tick(f64::from(*x)).powi(2) / channels as f64;
                    }
                }
            }
        }
        result.samples += values.len() as u64;
        progress(0.2 * result.samples as f64 / count.max(1) as f64);
    }
    Ok(result)
}

#[derive(Clone, Copy)]
struct Profile {
    target: f64,
    window: usize,
    look: usize,
    rise: f64,
    fall: f64,
    floor: f64,
    hold: f64,
    gain_min: f64,
    gain_max: f64,
    fade: f64,
    minimum_pause: usize,
    pause_gain: f64,
}
impl Profile {
    fn for_mode(mode: AudioMode) -> Self {
        if mode == AudioMode::Podcast {
            Self {
                target: -18.,
                window: 50,
                look: 3,
                rise: 2000.,
                fall: 800.,
                floor: -68.,
                hold: 20.,
                gain_min: -30.,
                gain_max: 30.,
                fade: 30.,
                minimum_pause: 150,
                pause_gain: 0.001,
            }
        } else {
            Self {
                target: -16.,
                window: 20,
                look: 3,
                rise: 400.,
                fall: 150.,
                floor: -60.,
                hold: 35.,
                gain_min: -20.,
                gain_max: 30.,
                fade: 400.,
                minimum_pause: 50,
                pause_gain: 0.1,
            }
        }
    }
}
struct Activity {
    reference_db: f64,
    noise_db: f64,
    threshold: f64,
    modulation: f64,
    level_floor_db: f64,
}
impl Activity {
    fn new(analysis: &Analysis, profile: Profile) -> Self {
        let voiced_count: u64 = analysis.voiced.iter().sum();
        let reference_db = if voiced_count > 0 {
            percentile(&analysis.voiced, 0.85)
        } else {
            percentile(&analysis.histogram, 0.90)
        };
        let measured_floor = percentile(&analysis.histogram, 0.1);
        let modulation = percentile(&analysis.histogram, 0.95) - measured_floor;
        Self {
            reference_db,
            noise_db: measured_floor.min(reference_db - 15.),
            threshold: profile.floor.max(reference_db - profile.hold),
            modulation,
            // Retention is deliberately generous toward uncertain sounds and
            // breaths. Leveling uses the measured floor: a residual periodic
            // fan must not be amplified as if it were a quiet voice.
            level_floor_db: if modulation > 8. {
                measured_floor + 4.
            } else {
                profile.floor
            },
        }
    }
    fn active(&self, f: Features) -> bool {
        db(f.rms) > self.threshold
            && (f.periodicity > 0.38 || (self.modulation > 8. && db(f.rms) > self.noise_db + 8.))
    }
    fn nearby_human(&self, f: Features, near: bool) -> bool {
        near && db(f.rms) > (self.noise_db + 4.).max(self.reference_db - 35.)
    }
    fn level_active(&self, f: Features) -> bool {
        self.active(f) && db(f.rms) > self.level_floor_db
    }
}

/// Apply Podcast's pre-restoration pause treatment to normalized microphone
/// samples. This is bounded, deterministic activity detection, not a semantic
/// speech classifier. Short pauses and nearby breaths are retained; long quiet
/// regions fade toward silence without removing samples or changing gain.
/// Keep the ungated normalized file as finish_file's sibilance reference.
pub fn prepare_restoration_file(
    input: &Path,
    output: &Path,
    channels: usize,
    check: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(f64),
) -> Result<(), String> {
    const RATE: u32 = 48_000;
    validate(channels, RATE)?;
    separate_output(input, output)?;
    let count = sample_count(input, channels)?;
    let mut reader = BufReader::new(File::open(input).map_err(|e| e.to_string())?);
    let analysis = scan(
        &mut reader,
        RATE,
        channels,
        AudioMode::Loud,
        count,
        check,
        progress,
    )?;
    if analysis.samples != count {
        return Err("Audio input changed during pause analysis".into());
    }
    reader.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    let profile = Profile::for_mode(AudioMode::Podcast);
    let activity = Activity::new(&analysis, profile);
    let mut writer = BufWriter::new(File::create(output).map_err(|e| e.to_string())?);
    let mut pending = VecDeque::new();
    let mut eof = false;
    let mut index = 0u64;
    let mut last_active: Option<u64> = None;
    let mut inactive_past = 0;
    let mut gain = 1.0_f64;
    let fade_step = 1.0 / (profile.fade * 0.001 * f64::from(RATE));
    let fade_blocks = (profile.fade / BLOCK_MS as f64).ceil() as usize;
    let mut written = 0u64;
    loop {
        cancelled(check)?;
        while !eof && pending.len() < CONTEXT_BLOCKS {
            let samples = read_samples(&mut reader, RATE as usize * BLOCK_MS / 1000 * channels)?;
            if samples.is_empty() {
                eof = true;
                break;
            }
            let f = features(&samples, channels, RATE);
            pending.push_back(Block {
                samples,
                reference: None,
                f,
                rf: f,
                active: activity.active(f),
                level_active: activity.level_active(f),
            });
        }
        let Some(mut block) = pending.pop_front() else {
            break;
        };
        if block.active {
            last_active = Some(index);
        }
        let next_active = pending.iter().position(|block| block.active).map(|n| n + 1);
        let near = last_active.is_some_and(|last| index - last <= 75)
            || next_active.is_some_and(|n| n <= 75);
        let human = block.active || activity.nearby_human(block.f, near);
        let future_silence = pending.iter().take_while(|block| !block.active).count();
        let keep = human
            || next_active.is_some_and(|n| n <= fade_blocks)
            || inactive_past + future_silence + 1 < profile.minimum_pause;
        if human {
            inactive_past = 0;
        } else {
            inactive_past += 1;
        }
        for frame in block.samples.chunks_exact_mut(channels) {
            gain = fade_toward(gain, if keep { 1.0 } else { 0.0 }, fade_step);
            for sample in frame {
                *sample *= gain as f32;
            }
        }
        write_samples(&mut writer, &block.samples)?;
        written += block.samples.len() as u64;
        index += 1;
        progress(0.2 + 0.8 * written as f64 / count.max(1) as f64);
    }
    if written != count {
        return Err("Audio input changed during pause preparation".into());
    }
    writer.flush().map_err(|e| e.to_string())?;
    cancelled(check)?;
    progress(1.0);
    Ok(())
}

struct Block {
    samples: Vec<f32>,
    reference: Option<Vec<f32>>,
    f: Features,
    rf: Features,
    active: bool,
    level_active: bool,
}
struct VoiceState {
    channels: usize,
    rate: f64,
    profile: Profile,
    mode: AudioMode,
    filters: Vec<Biquad>,
    eq: Vec<Vec<Biquad>>,
    midband: Vec<Biquad>,
    highband: Vec<Biquad>,
    presence: Vec<Biquad>,
    noise_band: Vec<(Biquad, Biquad)>,
    gain_db: f64,
    started: bool,
    gate: f64,
    compression: f64,
    low_cut: f64,
    high_cut: f64,
    random: u64,
    min_gain: f64,
    max_gain: f64,
    sibilance_gain: f64,
}
impl VoiceState {
    fn new(rate: u32, channels: usize, mode: AudioMode, analysis: &Analysis) -> Self {
        let rate = f64::from(rate);
        let profile = Profile::for_mode(mode);
        let reference = db(analysis.band_energy[12].max(1e-24).sqrt());
        let correction: Vec<_> = analysis
            .band_energy
            .iter()
            .enumerate()
            .map(|(i, e)| {
                let observed = db(e.max(1e-24).sqrt()) - reference;
                // Neighboring third-octave peak filters overlap; half correction
                // avoids doubling the requested per-band adjustment.
                ((PODCAST_TARGET[i] - PODCAST_TARGET[12]) - observed).clamp(-15., 12.) * 0.5
            })
            .collect();
        Self {
            channels,
            rate,
            profile,
            mode,
            filters: (0..channels).map(|_| Biquad::highpass(rate, 70.)).collect(),
            eq: (0..channels)
                .map(|_| {
                    BANDS
                        .iter()
                        .zip(&correction)
                        .map(|(hz, gain)| Biquad::peak(rate, *hz, 4.318, *gain))
                        .collect()
                })
                .collect(),
            midband: (0..channels)
                .map(|_| Biquad::bandpass(rate, 360., 0.8))
                .collect(),
            highband: (0..channels)
                .map(|_| Biquad::bandpass(rate, 6600., 0.7))
                .collect(),
            presence: (0..channels)
                .map(|_| Biquad::peak(rate, 4000., 4.8, -3.))
                .collect(),
            noise_band: (0..channels)
                .map(|_| (Biquad::highpass(rate, 4000.), Biquad::lowpass(rate, 16000.)))
                .collect(),
            // The take was already scanned. Starting from its first active
            // frame can over-amplify a quiet lead-in and overdrive the first
            // word. Seed gain from the take's robust speech level instead.
            gain_db: (profile.target - Activity::new(analysis, profile).reference_db)
                .clamp(profile.gain_min, profile.gain_max),
            started: true,
            gate: 1.,
            compression: 1.,
            low_cut: 1.,
            high_cut: 1.,
            random: 0x6c61687a615f6175,
            min_gain: 0.,
            max_gain: 0.,
            sibilance_gain: 0.,
        }
    }
    fn process(&mut self, block: &mut Block, desired_gain: Option<f64>, keep: bool, restore: bool) {
        let desired = desired_gain
            .unwrap_or(self.gain_db)
            .clamp(self.profile.gain_min, self.profile.gain_max);
        if !self.started && desired_gain.is_some() {
            self.gain_db = desired;
            self.started = true;
        }
        let gain_alpha = coefficient(
            if desired < self.gain_db {
                self.profile.fall
            } else {
                self.profile.rise
            },
            self.rate,
        );
        let gate_target = if keep { 1. } else { self.profile.pause_gain };
        let gate_step = 1.0 / (self.profile.fade * 0.001 * self.rate);
        let low_target = if self.mode == AudioMode::Podcast {
            amp(-((db(block.f.low_mid) - db(block.f.upper_mid) - 7.5).max(0.)).min(4.))
        } else {
            1.
        };
        let high_target = if self.mode == AudioMode::Podcast {
            amp(-((db(block.f.high) + self.gain_db - (-23.)).max(0.) * 0.75).min(8.))
        } else {
            1.
        };
        let restoration = if restore {
            block.rf.high.min(block.rf.rms * amp(-12.)) * 1.3
        } else {
            0.
        };
        for frame in block.samples.chunks_exact_mut(self.channels) {
            self.gain_db += gain_alpha * (desired - self.gain_db);
            self.min_gain = self.min_gain.min(self.gain_db);
            self.max_gain = self.max_gain.max(self.gain_db);
            self.gate = fade_toward(self.gate, gate_target, gate_step);
            self.low_cut +=
                coefficient(if low_target < self.low_cut { 15. } else { 80. }, self.rate)
                    * (low_target - self.low_cut);
            self.high_cut += coefficient(
                if high_target < self.high_cut { 1. } else { 50. },
                self.rate,
            ) * (high_target - self.high_cut);
            self.sibilance_gain +=
                coefficient(30., self.rate) * (restoration - self.sibilance_gain);
            // One excitation per frame preserves dual-mono recordings. Filter
            // state remains independent for genuinely different stereo inputs.
            self.random ^= self.random << 13;
            self.random ^= self.random >> 7;
            self.random ^= self.random << 17;
            let noise = (self.random as i64 as f64 / i64::MAX as f64) * 1.732;
            let mut values = [0.; 2];
            for (ch, value) in frame.iter().enumerate() {
                let mut x = f64::from(*value);
                if self.mode == AudioMode::Podcast {
                    // Deterministic band-limited noise restores short original
                    // fricatives only, never continuous noise or an absent word.
                    let (hp, lp) = &mut self.noise_band[ch];
                    x += lp.tick(hp.tick(noise)) * self.sibilance_gain;
                    for eq in &mut self.eq[ch] {
                        x = eq.tick(x);
                    }
                    x += self.midband[ch].tick(x) * (self.low_cut - 1.);
                }
                x *= amp(self.gain_db);
                if self.mode == AudioMode::Podcast {
                    x += self.highband[ch].tick(x) * (self.high_cut - 1.);
                }
                x = self.filters[ch].tick(x);
                if self.mode == AudioMode::Podcast {
                    x = self.presence[ch].tick(x);
                }
                values[ch] = x;
            }
            // Portable soft-knee compressor. Apple's DynamicsProcessor
            // headroom/master-gain controls do not map numerically to this.
            if self.mode != AudioMode::Podcast {
                let level = db(values[..self.channels]
                    .iter()
                    .map(|x| x.abs())
                    .fold(0., f64::max));
                let threshold = -12.;
                let knee = 6.;
                let excess = level - threshold;
                let reduction = if excess <= -knee / 2. {
                    0.
                } else if excess >= knee / 2. {
                    excess * (1. - 1. / 3.)
                } else {
                    (1. - 1. / 3.) * (excess + knee / 2.).powi(2) / (2. * knee)
                };
                let target = amp(-reduction);
                self.compression +=
                    coefficient(if target < self.compression { 5. } else { 120. }, self.rate)
                        * (target - self.compression);
            }
            for (ch, value) in frame.iter_mut().enumerate() {
                *value = (values[ch] * self.compression * self.gate) as f32;
            }
        }
    }
}

struct Limiter {
    frames: VecDeque<[f32; 2]>,
    peaks: VecDeque<(u64, f64)>,
    pushed: u64,
    popped: u64,
    look: usize,
    channels: usize,
    gain: f64,
    release: f64,
    limited: u64,
}
impl Limiter {
    fn new(rate: u32, channels: usize, podcast: bool) -> Self {
        Self {
            frames: VecDeque::new(),
            peaks: VecDeque::new(),
            pushed: 0,
            popped: 0,
            look: (rate as usize / 500).max(1),
            channels,
            gain: 1.,
            release: coefficient(if podcast { 100. } else { 50. }, f64::from(rate)),
            limited: 0,
        }
    }
    fn push(&mut self, frame: &[f32]) -> Option<[f32; 2]> {
        let mut value = [0.; 2];
        value[..self.channels].copy_from_slice(frame);
        let peak = frame.iter().map(|x| f64::from(*x).abs()).fold(0., f64::max);
        while self.peaks.back().is_some_and(|(_, p)| *p <= peak) {
            self.peaks.pop_back();
        }
        self.peaks.push_back((self.pushed, peak));
        self.pushed += 1;
        self.frames.push_back(value);
        if self.frames.len() > self.look {
            self.pop()
        } else {
            None
        }
    }
    fn pop(&mut self) -> Option<[f32; 2]> {
        let mut frame = self.frames.pop_front()?;
        let peak = self.peaks.front().map_or(0., |(_, p)| *p);
        let target = (0.95 / peak.max(1e-9)).min(1.);
        if target < self.gain {
            self.gain = target;
        } else {
            self.gain += self.release * (target - self.gain);
        }
        if self.gain < 0.99999 {
            self.limited += 1;
        }
        for value in &mut frame[..self.channels] {
            *value = (f64::from(*value) * self.gain).clamp(-0.95, 0.95) as f32;
        }
        if self.peaks.front().is_some_and(|(i, _)| *i == self.popped) {
            self.peaks.pop_front();
        }
        self.popped += 1;
        Some(frame)
    }
}

/// Finish an already-normalized (and, for neural modes, enhanced) take.
/// The reference is the normalized ORIGINAL input, used only to recover short
/// sibilants lost by Podcast processing. Files must have identical frame counts.
pub fn finish_file(
    input: &Path,
    reference: Option<&Path>,
    output: &Path,
    sample_rate: u32,
    channels: usize,
    mode: AudioMode,
    check: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(f64),
) -> Result<DspReport, String> {
    validate(channels, sample_rate)?;
    separate_output(input, output)?;
    if let Some(reference) = reference {
        separate_output(reference, output)?;
    }
    let count = sample_count(input, channels)?;
    if let Some(reference) = reference {
        if sample_count(reference, channels)? != count {
            return Err("Neural audio changed the microphone duration".into());
        }
    }
    let mut input_file = BufReader::new(File::open(input).map_err(|e| e.to_string())?);
    let mut reference_file = reference
        .map(|path| File::open(path).map(BufReader::new))
        .transpose()
        .map_err(|e| e.to_string())?;
    let mut output_file = BufWriter::new(File::create(output).map_err(|e| e.to_string())?);
    finish_stream(
        &mut input_file,
        reference_file.as_mut().map(|r| r as &mut dyn Read),
        &mut output_file,
        count,
        sample_rate,
        channels,
        mode,
        check,
        progress,
    )
}

fn finish_stream(
    input: &mut (impl Read + Seek),
    mut reference: Option<&mut dyn Read>,
    output: &mut dyn Write,
    count: u64,
    sample_rate: u32,
    channels: usize,
    mode: AudioMode,
    check: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(f64),
) -> Result<DspReport, String> {
    if matches!(mode, AudioMode::Off | AudioMode::Standard) {
        let mut report = DspReport::default();
        loop {
            cancelled(check)?;
            let samples = read_samples(input, 8192 * channels)?;
            if samples.is_empty() {
                break;
            }
            report.input_peak = report
                .input_peak
                .max(samples.iter().map(|x| x.abs()).fold(0., f32::max));
            report.frames += (samples.len() / channels) as u64;
            write_samples(output, &samples)?;
            progress(report.frames as f64 * channels as f64 / count.max(1) as f64);
        }
        if report.frames * channels as u64 != count {
            return Err("Audio input changed during finishing".into());
        }
        report.output_peak = report.input_peak;
        output.flush().map_err(|e| e.to_string())?;
        progress(1.);
        return Ok(report);
    }
    let analysis = scan(input, sample_rate, channels, mode, count, check, progress)?;
    input.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    let profile = Profile::for_mode(mode);
    let activity = Activity::new(&analysis, profile);
    let reference_db = activity.reference_db;
    let noise_db = activity.noise_db;
    let mut state = VoiceState::new(sample_rate, channels, mode, &analysis);
    let mut limiter = Limiter::new(sample_rate, channels, mode == AudioMode::Podcast);
    let mut pending = VecDeque::<Block>::new();
    let mut history = VecDeque::<(f64, bool)>::new();
    let mut index = 0u64;
    let mut last_active: Option<u64> = None;
    let mut inactive_past = 0usize;
    let mut sibilant_past = 0usize;
    let mut eof = false;
    let mut active_frames = 0u64;
    let mut report = DspReport {
        input_peak: analysis.peak as f32,
        ..Default::default()
    };
    loop {
        cancelled(check)?;
        while !eof && pending.len() < CONTEXT_BLOCKS {
            let samples = read_samples(input, sample_rate as usize * BLOCK_MS / 1000 * channels)?;
            if samples.is_empty() {
                eof = true;
                break;
            }
            let ref_values = reference
                .as_deref_mut()
                .map(|reader| read_samples(reader, samples.len()))
                .transpose()?;
            if ref_values
                .as_ref()
                .is_some_and(|r| r.len() != samples.len())
            {
                return Err("Neural reference changed duration during processing".into());
            }
            let f = features(&samples, channels, sample_rate);
            let rf = ref_values
                .as_ref()
                .map_or(f, |v| features(v, channels, sample_rate));
            pending.push_back(Block {
                samples,
                reference: ref_values,
                f,
                rf,
                active: activity.active(f),
                level_active: activity.level_active(f),
            });
        }
        let Some(mut block) = pending.pop_front() else {
            break;
        };
        if block.active {
            last_active = Some(index);
        }
        let next_active = pending.iter().position(|block| block.active).map(|n| n + 1);
        let near = last_active.is_some_and(|last| index - last <= 75)
            || next_active.is_some_and(|n| n <= 75);
        let human = block.active || activity.nearby_human(block.f, near);
        let future_silence = pending.iter().take_while(|block| !block.active).count();
        let short_pause = inactive_past + future_silence + 1 < profile.minimum_pause;
        let fade_blocks = (profile.fade / BLOCK_MS as f64).ceil() as usize;
        let keep = human || short_pause || next_active.is_some_and(|n| n <= fade_blocks);
        if human {
            inactive_past = 0;
        } else {
            inactive_past += 1;
        }
        let mut window_energy = 0.;
        let mut window_count = 0;
        for (energy, is_active) in history
            .iter()
            .rev()
            .take(profile.window.saturating_sub(profile.look + 1))
        {
            if *is_active {
                window_energy += *energy;
                window_count += 1;
            }
        }
        if block.level_active {
            window_energy += block.f.rms.powi(2);
            window_count += 1;
        }
        for future in pending.iter().take(profile.look) {
            if future.level_active {
                window_energy += future.f.rms.powi(2);
                window_count += 1;
            }
        }
        let desired = if block.level_active && window_count > 0 {
            Some(profile.target - db((window_energy / window_count as f64).sqrt()))
        } else {
            None
        };
        let candidate = |b: &Block| {
            b.reference.is_some()
                && db(b.rf.high) > db(b.rf.mid) + 3.
                && db(b.rf.high) > noise_db + 20.
                && db(b.rf.rms) > reference_db - 20.
        };
        let is_sibilant = candidate(&block);
        let sibilant_length =
            sibilant_past + 1 + pending.iter().take_while(|b| candidate(b)).count();
        // Sibilants have a much narrower speech-neighborhood than retained
        // breaths: 150 ms, rounded up to one 20 ms analysis block.
        let near_sibilant = last_active.is_some_and(|last| index - last <= 8)
            || next_active.is_some_and(|n| n <= 8);
        let restore = mode == AudioMode::Podcast
            && near_sibilant
            && is_sibilant
            && (2..=10).contains(&sibilant_length)
            && block.f.high < block.rf.high * 0.5;
        if is_sibilant {
            sibilant_past += 1;
        } else {
            sibilant_past = 0;
        }
        state.process(&mut block, desired, keep, restore);
        let frames = (block.samples.len() / channels) as u64;
        if block.active {
            active_frames += frames;
        }
        let mut rendered = Vec::with_capacity(block.samples.len());
        for frame in block.samples.chunks_exact(channels) {
            if let Some(out) = limiter.push(frame) {
                report.output_peak = report
                    .output_peak
                    .max(out[..channels].iter().map(|x| x.abs()).fold(0., f32::max));
                rendered.extend_from_slice(&out[..channels]);
                report.frames += 1;
            }
        }
        write_samples(output, &rendered)?;
        history.push_back((block.f.rms.powi(2), block.level_active));
        if history.len() > profile.window {
            history.pop_front();
        }
        index += 1;
        progress(0.2 + 0.8 * report.frames as f64 * channels as f64 / count.max(1) as f64);
    }
    while let Some(out) = limiter.pop() {
        cancelled(check)?;
        report.output_peak = report
            .output_peak
            .max(out[..channels].iter().map(|x| x.abs()).fold(0., f32::max));
        write_samples(output, &out[..channels])?;
        report.frames += 1;
    }
    if report.frames * channels as u64 != count {
        return Err("Audio input changed duration during processing".into());
    }
    report.active_fraction = active_frames as f32 / report.frames.max(1) as f32;
    report.minimum_gain_db = state.min_gain as f32;
    report.maximum_gain_db = state.max_gain as f32;
    report.limited_frames = limiter.limited;
    output.flush().map_err(|e| e.to_string())?;
    cancelled(check)?;
    progress(1.);
    Ok(report)
}

/// In-memory convenience for small callers/tests. Neural modes require the
/// separate normalization → actual inference → finishing sequence.
pub fn process_interleaved(
    samples: &mut [f32],
    sample_rate: u32,
    channels: usize,
    mode: AudioMode,
    check: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(f64),
) -> Result<DspReport, String> {
    validate(channels, sample_rate)?;
    if samples.len() % channels != 0 || samples.iter().any(|x| !x.is_finite()) {
        return Err("Invalid microphone samples".into());
    }
    if mode.requires_neural() {
        return Err("Neural modes require a successful model pass before finishing".into());
    }
    let input_peak = samples.iter().map(|x| x.abs()).fold(0., f32::max);
    {
        let gain = if mode != AudioMode::Off && input_peak > 1e-12 {
            amp(-6.) / f64::from(input_peak)
        } else {
            1.
        };
        let mut energy = [0.; 2];
        for (i, x) in samples.iter().enumerate() {
            energy[i % channels] += f64::from(*x).powi(2);
        }
        let quieter = usize::from(energy[1] < energy[0]);
        let repair = channels == 2
            && energy[1 - quieter] > 1e-16
            && energy[quieter] < energy[1 - quieter] * 0.001;
        for frame in samples.chunks_exact_mut(channels) {
            cancelled(check)?;
            if repair {
                frame[quieter] = frame[1 - quieter];
            }
            for x in frame {
                *x = (f64::from(*x) * gain) as f32;
            }
        }
    }
    let bytes: Vec<u8> = samples.iter().flat_map(|x| x.to_le_bytes()).collect();
    let mut reader = Cursor::new(bytes);
    let mut output = Vec::new();
    let mut report = finish_stream(
        &mut reader,
        None,
        &mut output,
        samples.len() as u64,
        sample_rate,
        channels,
        mode,
        check,
        progress,
    )?;
    for (value, bytes) in samples.iter_mut().zip(output.chunks_exact(4)) {
        *value = f32::from_le_bytes(bytes.try_into().unwrap());
    }
    report.input_peak = input_peak;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        path::PathBuf,
        sync::atomic::{AtomicUsize, Ordering},
    };
    const RATE: u32 = 48000;
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            static COUNTER: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "lahza-dsp-{}-{}",
                std::process::id(),
                COUNTER.fetch_add(1, Ordering::SeqCst)
            ));
            fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn path(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
        fn write(&self, name: &str, samples: &[f32]) -> PathBuf {
            let path = self.path(name);
            let mut file = File::create(&path).unwrap();
            write_samples(&mut file, samples).unwrap();
            path
        }
        fn read(&self, name: &str) -> Vec<f32> {
            let path = self.path(name);
            read_samples(
                &mut File::open(&path).unwrap(),
                fs::metadata(path).unwrap().len() as usize / 4,
            )
            .unwrap()
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    fn tone(seconds: f64, amplitude: f64, frequency: f64) -> Vec<f32> {
        (0..(seconds * f64::from(RATE)) as usize)
            .map(|i| {
                (amplitude
                    * (2. * std::f64::consts::PI * frequency * i as f64 / f64::from(RATE)).sin())
                    as f32
            })
            .collect()
    }
    fn voiced(seconds: f64, amplitude: f64) -> Vec<f32> {
        (0..(seconds * f64::from(RATE)) as usize)
            .map(|i| {
                let t = i as f64 / f64::from(RATE);
                (amplitude
                    * (0.7 * (2. * std::f64::consts::PI * 180. * t).sin()
                        + 0.2 * (2. * std::f64::consts::PI * 360. * t).sin()
                        + 0.1 * (2. * std::f64::consts::PI * 720. * t).sin())
                    * (0.8 + 0.2 * (2. * std::f64::consts::PI * 3. * t).sin()))
                    as f32
            })
            .collect()
    }
    fn rms(samples: &[f32]) -> f64 {
        (samples.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>() / samples.len().max(1) as f64)
            .sqrt()
    }
    fn region(samples: &[f32], start: f64, end: f64) -> &[f32] {
        &samples[(start * f64::from(RATE)) as usize..(end * f64::from(RATE)) as usize]
    }
    fn finish(
        fixture: &Fixture,
        samples: &[f32],
        reference: Option<&[f32]>,
        mode: AudioMode,
    ) -> (Vec<f32>, DspReport) {
        let source = fixture.write("input.raw", samples);
        let reference = reference.map(|samples| fixture.write("reference.raw", samples));
        let output = fixture.path("output.raw");
        let report = finish_file(
            &source,
            reference.as_deref(),
            &output,
            RATE,
            1,
            mode,
            &|| false,
            &mut |_| {},
        )
        .unwrap();
        (fixture.read("output.raw"), report)
    }

    #[test]
    fn standard_normalizes_once_and_off_only_repairs_quiet_channel() {
        let fixture = Fixture::new();
        let mono = voiced(0.3, 0.04);
        let stereo: Vec<_> = mono.iter().flat_map(|x| [*x, *x * 0.001]).collect();
        let source = fixture.write("stereo.raw", &stereo);
        let normalized = fixture.path("normalized.raw");
        let report = normalize_file(
            &source,
            &normalized,
            2,
            AudioMode::Standard,
            &|| false,
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(report.frames, mono.len() as u64);
        assert_eq!(report.repaired_channel, Some(1));
        assert!((report.output_peak - amp(-6.) as f32).abs() < 1e-6);
        let output = fixture.path("standard.raw");
        finish_file(
            &normalized,
            None,
            &output,
            RATE,
            2,
            AudioMode::Standard,
            &|| false,
            &mut |_| {},
        )
        .unwrap();
        let samples = fixture.read("standard.raw");
        assert_eq!(samples.len(), stereo.len());
        assert!(samples.chunks_exact(2).all(|x| x[0] == x[1]));
        assert_eq!(fs::read(normalized).unwrap(), fs::read(output).unwrap());
        let original = fixture.path("original.raw");
        let off = normalize_file(
            &source,
            &original,
            2,
            AudioMode::Off,
            &|| false,
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(off.gain_db, 0.);
        assert_eq!(off.repaired_channel, Some(1));
        assert_eq!(
            fixture.read("original.raw"),
            mono.iter().flat_map(|x| [*x, *x]).collect::<Vec<_>>()
        );
        let balanced = fixture.write(
            "balanced.raw",
            &mono.iter().flat_map(|x| [*x, *x * 0.5]).collect::<Vec<_>>(),
        );
        let off = normalize_file(
            &balanced,
            &original,
            2,
            AudioMode::Off,
            &|| false,
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(off.repaired_channel, None);
        assert_eq!(fs::read(balanced).unwrap(), fs::read(original).unwrap());
    }

    #[test]
    fn loud_levels_quiet_speech_without_ramping_up_long_pauses() {
        let fixture = Fixture::new();
        let mut samples = voiced(3., 0.035);
        samples.extend(voiced(3., 0.35));
        let mut random = 17u64;
        let gap: Vec<f32> = (0..(4 * RATE) as usize)
            .map(|_| {
                random ^= random << 13;
                random ^= random >> 7;
                random ^= random << 17;
                (random as i64 as f64 / i64::MAX as f64 * 0.0001) as f32
            })
            .collect();
        samples.extend(&gap);
        samples.extend(voiced(3., 0.035));
        let (processed, report) = finish(&fixture, &samples, None, AudioMode::Loud);
        let quiet = rms(region(&processed, 2., 2.8));
        let loud = rms(region(&processed, 5., 5.8));
        let resumed = rms(region(&processed, 12., 12.8));
        assert!(
            (db(loud) - db(quiet)).abs() < 3.0,
            "level spread {} dB",
            db(loud) - db(quiet)
        );
        assert!(
            (db(resumed) - db(quiet)).abs() < 3.0,
            "resumed speech level changed"
        );
        assert!(
            quiet > 0.07 && quiet < 0.24,
            "unexpected speech level {quiet}"
        );
        let pause_start = rms(region(&processed, 6.5, 7.));
        let pause_end = rms(region(&processed, 9., 9.5));
        assert!(
            pause_end <= pause_start * 1.1,
            "pause gain pumped up: {pause_start} -> {pause_end}"
        );
        assert!(pause_end < 0.0001, "long pause was amplified: {pause_end}");
        assert_eq!(samples.len(), processed.len());
        assert!(report.output_peak <= 0.950001);
        assert!(report.active_fraction > 0.5 && report.active_fraction < 0.85);
    }

    #[test]
    fn leveler_does_not_raise_residual_fan_or_overdrive_first_word() {
        let fixture = Fixture::new();
        let mut samples = tone(5., 0.02, 120.);
        for start in [1., 3.] {
            for (sample, voice) in samples[(start * f64::from(RATE)) as usize..]
                .iter_mut()
                .zip(voiced(0.8, 0.3))
            {
                *sample += voice;
            }
        }
        let (processed, report) = finish(&fixture, &samples, None, AudioMode::NoiseReduction);
        let leading_noise = rms(region(&processed, 0.3, 0.9));
        let between_noise = rms(region(&processed, 2.2, 2.8));
        let speech = rms(region(&processed, 1.05, 1.65));
        assert!(
            leading_noise < speech * 0.35,
            "fan was leveled like speech: noise={leading_noise}, speech={speech}"
        );
        assert!(
            (db(leading_noise) - db(between_noise)).abs() < 3.,
            "noise gain pumped before the first word"
        );
        assert_eq!(report.limited_frames, 0, "first word drove the limiter");
        assert!(
            report.maximum_gain_db < 12.,
            "quiet lead-in selected excessive initial gain"
        );
    }

    #[test]
    fn lookahead_limiter_keeps_stereo_linked_and_does_not_shift_impulses() {
        let fixture = Fixture::new();
        let mut stereo: Vec<f32> = voiced(1., 0.1)
            .iter()
            .flat_map(|x| [*x, *x * 0.5])
            .collect();
        stereo[24000 * 2] = 12.;
        stereo[24000 * 2 + 1] = 6.;
        let source = fixture.write("source.raw", &stereo);
        let output = fixture.path("limited.raw");
        let report = finish_file(
            &source,
            None,
            &output,
            RATE,
            2,
            AudioMode::Loud,
            &|| false,
            &mut |_| {},
        )
        .unwrap();
        let processed = fixture.read("limited.raw");
        assert_eq!(processed.len(), stereo.len());
        assert!(report.limited_frames > 0);
        assert!(processed
            .iter()
            .all(|x| x.is_finite() && x.abs() <= 0.950001));
        for frame in processed.chunks_exact(2) {
            assert!(
                (frame[1] - frame[0] * 0.5).abs() < 1e-6,
                "stereo image changed"
            );
        }
        let peak = processed
            .chunks_exact(2)
            .enumerate()
            .max_by(|(_, a), (_, b)| a[0].abs().total_cmp(&b[0].abs()))
            .unwrap()
            .0;
        assert!(
            peak.abs_diff(24000) <= 1,
            "limiter shifted the impulse to {peak}"
        );
    }

    #[test]
    fn podcast_deessing_reduces_harsh_sibilance_without_inventing_continuous_noise() {
        let mut samples = voiced(2., 0.2);
        let hiss = tone(0.12, 0.5, 7000.);
        for (x, h) in samples[(0.8 * f64::from(RATE)) as usize..]
            .iter_mut()
            .zip(hiss)
        {
            *x += h;
        }
        let fixture = Fixture::new();
        let (processed, report) = finish(&fixture, &samples, Some(&samples), AudioMode::Podcast);
        assert_eq!(samples.len(), processed.len());
        assert!(report.output_peak <= 0.950001);
        assert!(processed.iter().all(|x| x.is_finite()));
        let mut filter = Biquad::bandpass(f64::from(RATE), 7000., 3.);
        let input_high: Vec<_> = samples
            .iter()
            .map(|x| filter.tick(f64::from(*x)) as f32)
            .collect();
        let mut filter = Biquad::bandpass(f64::from(RATE), 7000., 3.);
        let output_high: Vec<_> = processed
            .iter()
            .map(|x| filter.tick(f64::from(*x)) as f32)
            .collect();
        assert!(
            rms(region(&output_high, 0.82, 0.90)) < rms(region(&input_high, 0.82, 0.90)) * 0.7,
            "harsh S energy was not reduced"
        );
        let silence = vec![0.; (RATE * 5) as usize];
        let (processed, report) = finish(&fixture, &silence, Some(&silence), AudioMode::Podcast);
        assert!(processed.iter().all(|x| *x == 0.));
        assert_eq!(report.output_peak, 0.);
    }

    #[test]
    fn podcast_preparation_keeps_short_pauses_and_full_speech_onsets() {
        let fixture = Fixture::new();
        let mut samples = voiced(1., 0.2);
        let mut random = 91u64;
        let noise = |length: usize, random: &mut u64| -> Vec<f32> {
            (0..length)
                .map(|_| {
                    *random ^= *random << 13;
                    *random ^= *random >> 7;
                    *random ^= *random << 17;
                    (*random as i64 as f64 / i64::MAX as f64 * 0.00001) as f32
                })
                .collect()
        };
        samples.extend(noise(RATE as usize, &mut random));
        samples.extend(voiced(1., 0.2));
        samples.extend(noise((RATE * 5) as usize, &mut random));
        samples.extend(voiced(1., 0.2));
        let source = fixture.write("preparation.raw", &samples);
        let target = fixture.path("prepared.raw");
        prepare_restoration_file(&source, &target, 1, &|| false, &mut |_| {}).unwrap();
        let result = fixture.read("prepared.raw");
        assert_eq!(result.len(), samples.len());
        assert_eq!(
            region(&result, 1., 2.),
            region(&samples, 1., 2.),
            "short pause changed"
        );
        assert!(
            region(&result, 4., 7.).iter().all(|x| *x == 0.),
            "long pause remained before inference"
        );
        assert_eq!(
            region(&result, 8., 8.2),
            region(&samples, 8., 8.2),
            "speech onset faded in late"
        );
        let checks = AtomicUsize::new(0);
        assert!(prepare_restoration_file(
            &source,
            &target,
            1,
            &|| checks.fetch_add(1, Ordering::Relaxed) > 8,
            &mut |_| {}
        )
        .unwrap_err()
        .contains("cancelled"));
    }

    #[test]
    fn podcast_spectrum_analysis_is_symmetric_and_sibilance_preserves_dual_mono() {
        let fixture = Fixture::new();
        let left = voiced(1., 0.15);
        let right = tone(1., 0.1, 1500.);
        let stereo: Vec<f32> = left
            .iter()
            .zip(&right)
            .flat_map(|(l, r)| [*l, *r])
            .collect();
        let reversed: Vec<f32> = left
            .iter()
            .zip(&right)
            .flat_map(|(l, r)| [*r, *l])
            .collect();
        let source = fixture.write("stereo.raw", &stereo);
        let reverse = fixture.write("reverse.raw", &reversed);
        let a = fixture.path("a.raw");
        let b = fixture.path("b.raw");
        finish_file(
            &source,
            None,
            &a,
            RATE,
            2,
            AudioMode::Podcast,
            &|| false,
            &mut |_| {},
        )
        .unwrap();
        finish_file(
            &reverse,
            None,
            &b,
            RATE,
            2,
            AudioMode::Podcast,
            &|| false,
            &mut |_| {},
        )
        .unwrap();
        for (a, b) in fixture
            .read("a.raw")
            .chunks_exact(2)
            .zip(fixture.read("b.raw").chunks_exact(2))
        {
            assert!(
                (a[0] - b[1]).abs() < 1e-6 && (a[1] - b[0]).abs() < 1e-6,
                "Podcast preferred the left channel"
            );
        }
        let voice = voiced(1., 0.15);
        let mut reference = voice.clone();
        for (x, h) in reference[(RATE / 2) as usize..]
            .iter_mut()
            .zip(tone(0.1, 0.4, 7000.))
        {
            *x += h;
        }
        let source = fixture.write(
            "dual.raw",
            &voice.iter().flat_map(|x| [*x, *x]).collect::<Vec<_>>(),
        );
        let reference = fixture.write(
            "dual-ref.raw",
            &reference.iter().flat_map(|x| [*x, *x]).collect::<Vec<_>>(),
        );
        finish_file(
            &source,
            Some(&reference),
            &a,
            RATE,
            2,
            AudioMode::Podcast,
            &|| false,
            &mut |_| {},
        )
        .unwrap();
        assert!(
            fixture
                .read("a.raw")
                .chunks_exact(2)
                .all(|frame| frame[0] == frame[1]),
            "restoring a mono voice widened its stereo image"
        );
    }

    #[test]
    fn output_aliases_are_rejected_before_truncating_input() {
        let fixture = Fixture::new();
        let source = fixture.write("input.raw", &voiced(0.1, 0.2));
        let original = fs::read(&source).unwrap();
        let alias = fixture.path("alias.raw");
        fs::hard_link(&source, &alias).unwrap();
        assert!(normalize_file(
            &source,
            &alias,
            1,
            AudioMode::Standard,
            &|| false,
            &mut |_| {}
        )
        .is_err());
        assert!(finish_file(
            &source,
            None,
            &alias,
            RATE,
            1,
            AudioMode::Loud,
            &|| false,
            &mut |_| {}
        )
        .is_err());
        assert!(prepare_restoration_file(&source, &alias, 1, &|| false, &mut |_| {}).is_err());
        assert_eq!(fs::read(&source).unwrap(), original);
    }

    #[test]
    fn file_processing_is_continuous_across_context_boundary_and_cancellable() {
        let fixture = Fixture::new();
        let samples = voiced(9., 0.1);
        let (output, _) = finish(&fixture, &samples, None, AudioMode::Loud);
        for time in [3.99, 4.01, 7.99, 8.01] {
            let at = (time * f64::from(RATE)) as usize;
            assert!(
                (output[at] - output[at - 1]).abs() < 0.05,
                "chunk boundary discontinuity"
            );
        }
        let source = fixture.write("cancel.raw", &samples);
        let destination = fixture.path("cancel-output.raw");
        let checks = AtomicUsize::new(0);
        let result = finish_file(
            &source,
            None,
            &destination,
            RATE,
            1,
            AudioMode::Loud,
            &|| checks.fetch_add(1, Ordering::Relaxed) > 8,
            &mut |_| {},
        );
        assert!(result.unwrap_err().contains("cancelled"));
        let bad = fixture.write("nan.raw", &[f32::NAN]);
        assert!(normalize_file(
            &bad,
            &destination,
            1,
            AudioMode::Standard,
            &|| false,
            &mut |_| {}
        )
        .is_err());
        let mismatched = fixture.write("short.raw", &[0.; 10]);
        assert!(finish_file(
            &source,
            Some(&mismatched),
            &destination,
            RATE,
            1,
            AudioMode::Podcast,
            &|| false,
            &mut |_| {}
        )
        .unwrap_err()
        .contains("duration"));
        let mut input = vec![0.; 20];
        assert!(process_interleaved(
            &mut input,
            RATE,
            1,
            AudioMode::NoiseReduction,
            &|| false,
            &mut |_| {}
        )
        .unwrap_err()
        .contains("model"));
    }
}
