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
    io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write},
    path::Path,
};

pub const DSP_VERSION: &str = "speech-dsp-v3-conservative-leveler-48k";
const BLOCK_MS: usize = 20;
const CONTEXT_BLOCKS: usize = 200;
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
    samples: u64,
}
impl Default for Analysis {
    fn default() -> Self {
        Self {
            histogram: [0; 121],
            voiced: [0; 121],
            peak: 0.,
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
    count: u64,
    check: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(f64),
) -> Result<Analysis, String> {
    let mut result = Analysis::default();
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
    fn voice() -> Self {
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

struct Block {
    samples: Vec<f32>,
    f: Features,
    active: bool,
    level_active: bool,
}
struct VoiceState {
    channels: usize,
    rate: f64,
    profile: Profile,
    filters: Vec<Biquad>,
    gain_db: f64,
    started: bool,
    gate: f64,
    compression: f64,
    min_gain: f64,
    max_gain: f64,
}
impl VoiceState {
    fn new(rate: u32, channels: usize, analysis: &Analysis) -> Self {
        let rate = f64::from(rate);
        let profile = Profile::voice();
        Self {
            channels,
            rate,
            profile,
            filters: (0..channels).map(|_| Biquad::highpass(rate, 70.)).collect(),
            // The take was already scanned. Starting from its first active
            // frame can over-amplify a quiet lead-in and overdrive the first
            // word. Seed gain from the take's robust speech level instead.
            gain_db: (profile.target - Activity::new(analysis, profile).reference_db)
                .clamp(profile.gain_min, profile.gain_max),
            started: true,
            gate: 1.,
            compression: 1.,
            min_gain: 0.,
            max_gain: 0.,
        }
    }
    fn process(&mut self, block: &mut Block, desired_gain: Option<f64>, keep: bool) {
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
        for frame in block.samples.chunks_exact_mut(self.channels) {
            self.gain_db += gain_alpha * (desired - self.gain_db);
            self.min_gain = self.min_gain.min(self.gain_db);
            self.max_gain = self.max_gain.max(self.gain_db);
            self.gate = fade_toward(self.gate, gate_target, gate_step);
            let mut values = [0.; 2];
            for (ch, value) in frame.iter().enumerate() {
                values[ch] = self.filters[ch].tick(f64::from(*value) * amp(self.gain_db));
            }
            // Portable soft-knee compressor. Apple's DynamicsProcessor
            // headroom/master-gain controls do not map numerically to this.
            {
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
    fn new(rate: u32, channels: usize) -> Self {
        Self {
            frames: VecDeque::new(),
            peaks: VecDeque::new(),
            pushed: 0,
            popped: 0,
            look: (rate as usize / 500).max(1),
            channels,
            gain: 1.,
            release: coefficient(50., f64::from(rate)),
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
pub fn finish_file(
    input: &Path,
    output: &Path,
    sample_rate: u32,
    channels: usize,
    mode: AudioMode,
    check: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(f64),
) -> Result<DspReport, String> {
    validate(channels, sample_rate)?;
    separate_output(input, output)?;
    let count = sample_count(input, channels)?;
    let mut input_file = BufReader::new(File::open(input).map_err(|e| e.to_string())?);
    let mut output_file = BufWriter::new(File::create(output).map_err(|e| e.to_string())?);
    finish_stream(&mut input_file, &mut output_file, count, sample_rate, channels, mode, check, progress)
}

fn finish_stream(
    input: &mut (impl Read + Seek),
    output: &mut dyn Write,
    count: u64,
    sample_rate: u32,
    channels: usize,
    mode: AudioMode,
    check: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(f64),
) -> Result<DspReport, String> {
    if mode == AudioMode::Off {
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
    let analysis = scan(input, sample_rate, channels, count, check, progress)?;
    input.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    let profile = Profile::voice();
    let activity = Activity::new(&analysis, profile);
    let mut state = VoiceState::new(sample_rate, channels, &analysis);
    let mut limiter = Limiter::new(sample_rate, channels);
    let mut pending = VecDeque::<Block>::new();
    let mut history = VecDeque::<(f64, bool)>::new();
    let mut index = 0u64;
    let mut last_active: Option<u64> = None;
    let mut inactive_past = 0usize;
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
            let f = features(&samples, channels, sample_rate);
            pending.push_back(Block {
                samples,
                f,
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
        state.process(&mut block, desired, keep);
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
        mode: AudioMode,
    ) -> (Vec<f32>, DspReport) {
        let source = fixture.write("input.raw", samples);
        let output = fixture.path("output.raw");
        let report = finish_file(
            &source,
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
            AudioMode::NoiseReduction,
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
            &output,
            RATE,
            2,
            AudioMode::Off,
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
        let (processed, report) = finish(&fixture, &samples, AudioMode::NoiseReduction);
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
        let (processed, report) = finish(&fixture, &samples, AudioMode::NoiseReduction);
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
            &output,
            RATE,
            2,
            AudioMode::NoiseReduction,
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
            AudioMode::NoiseReduction,
            &|| false,
            &mut |_| {}
        )
        .is_err());
        assert!(finish_file(
            &source,
            &alias,
            RATE,
            1,
            AudioMode::NoiseReduction,
            &|| false,
            &mut |_| {}
        )
        .is_err());
        assert_eq!(fs::read(&source).unwrap(), original);
    }

    #[test]
    fn file_processing_is_continuous_across_context_boundary_and_cancellable() {
        let fixture = Fixture::new();
        let samples = voiced(9., 0.1);
        let (output, _) = finish(&fixture, &samples, AudioMode::NoiseReduction);
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
            &destination,
            RATE,
            1,
            AudioMode::NoiseReduction,
            &|| checks.fetch_add(1, Ordering::Relaxed) > 8,
            &mut |_| {},
        );
        assert!(result.unwrap_err().contains("cancelled"));
        let bad = fixture.write("nan.raw", &[f32::NAN]);
        assert!(normalize_file(
            &bad,
            &destination,
            1,
            AudioMode::NoiseReduction,
            &|| false,
            &mut |_| {}
        )
        .is_err());
    }
}
