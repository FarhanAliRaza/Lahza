//! Music analysis into marker tracks (design section 4.2):
//! tempo and first downbeat, straight and triplet grids, per-band onsets,
//! `hit` (strongest onsets), `stop` (silence → hit) and `drop` (bass
//! dropout before a downbeat), plus `music.start` / `music.end`.
//!
//! Audio is decoded by ffmpeg to mono 22.05 kHz floats. Onsets come from
//! log-compressed spectral flux in three bands; the tempo from the
//! autocorrelation of the onset envelope with a log-normal prior around
//! 120 BPM, refined by a comb search over period and phase; bars from the
//! beat phase with the strongest low-band (kick) onsets.

use lahza_motion_core::markers::{Marker, MarkerTrack};
use rustfft::{num_complex::Complex32, FftPlanner};
use serde::Serialize;
use std::path::Path;
use std::process::Command;

pub const SAMPLE_RATE: u32 = 22050;
const WINDOW: usize = 1024;
const HOP: usize = 128;
const BANDS: [(f32, f32); 3] = [(20.0, 200.0), (200.0, 2000.0), (2000.0, 11025.0)];

#[derive(Clone, Debug, Serialize)]
pub struct Analysis {
    pub source: String,
    pub duration: f64,
    pub bpm: f64,
    /// Seconds per beat.
    pub beat: f64,
    pub first_beat: f64,
    pub first_downbeat: f64,
    pub beats_per_bar: u32,
    /// True when beats follow a changing tempo instead of a fixed grid.
    pub tempo_varies: bool,
    /// "straight" or "triplet".
    pub feel: String,
    /// Mean onset strength at beat fractions 0, 1/3, 1/2, 2/3.
    pub phase_strength: [f64; 4],
    /// How well onsets fit the grid (0..1).
    pub grid_confidence: f64,
    pub music_start: f64,
    pub music_end: f64,
    #[serde(skip)]
    pub tracks: Vec<MarkerTrack>,
}

impl Analysis {
    pub fn summary(&self) -> serde_json::Value {
        let mut v = serde_json::to_value(self).unwrap_or_default();
        v["tracks"] = self
            .tracks
            .iter()
            .map(|t| serde_json::json!({"name": t.name, "count": t.markers.len()}))
            .collect::<Vec<_>>()
            .into();
        v
    }
}

/// Decodes any audio or video file to mono f32 at `SAMPLE_RATE`.
pub fn decode(path: &Path) -> Result<Vec<f32>, String> {
    let output = Command::new("ffmpeg")
        .args(["-v", "error", "-nostdin", "-i"])
        .arg(path)
        .args(["-vn", "-ac", "1", "-ar", &SAMPLE_RATE.to_string(), "-f", "f32le", "-"])
        .output()
        .map_err(|e| format!("cannot run ffmpeg (install it to analyse audio): {e}"))?;
    if !output.status.success() {
        return Err(format!("ffmpeg could not decode {}: {}", path.display(), String::from_utf8_lossy(&output.stderr).trim()));
    }
    let samples: Vec<f32> = output.stdout.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
    if samples.is_empty() {
        return Err(format!("{} has no audio", path.display()));
    }
    Ok(samples)
}

pub fn analyze_file(path: &Path, offset: f64) -> Result<Analysis, String> {
    let samples = decode(path)?;
    let mut a = analyze(&samples, SAMPLE_RATE, offset);
    a.source = path.display().to_string();
    for t in &mut a.tracks {
        t.source = Some(format!("audio_analyze {}", path.display()));
    }
    Ok(a)
}

struct Envelopes {
    /// Seconds per envelope frame.
    dt: f64,
    /// Time of envelope frame 0.
    t0: f64,
    bands: [Vec<f32>; 3],
    onset: Vec<f32>,
    /// Mel-weighted flux, for tempo and beat phase.
    beat_env: Vec<f32>,
    /// RMS in dB per frame.
    rms_db: Vec<f32>,
    /// Low-band energy per frame (linear).
    low_energy: Vec<f32>,
}

fn envelopes(samples: &[f32], sr: u32) -> Envelopes {
    let frames = samples.len().saturating_sub(WINDOW) / HOP + 1;
    let window: Vec<f32> = (0..WINDOW).map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / WINDOW as f32).cos()).collect();
    let fft = FftPlanner::new().plan_fft_forward(WINDOW);
    let bin_hz = sr as f32 / WINDOW as f32;
    let ranges: Vec<(usize, usize)> = BANDS
        .iter()
        .map(|(lo, hi)| (((lo / bin_hz).floor() as usize).max(1), ((hi / bin_hz).ceil() as usize).min(WINDOW / 2)))
        .collect();
    let mut prev = vec![0.0f32; WINDOW / 2 + 1];
    // Mel-like weights (∝ 1/(700 + f)) so kicks and snares count more
    // than the many high bins of a hi-hat, as a mel spectrogram would.
    let mel: Vec<f32> = (0..=WINDOW / 2).map(|k| 1.0 / (700.0 + k as f32 * bin_hz)).collect();
    let mel_sum: f32 = mel[1..].iter().sum();
    let mut beat_env = Vec::with_capacity(frames);
    let mut bands: [Vec<f32>; 3] = [Vec::with_capacity(frames), Vec::with_capacity(frames), Vec::with_capacity(frames)];
    let mut rms_db = Vec::with_capacity(frames);
    let mut low_energy = Vec::with_capacity(frames);
    let mut buf = vec![Complex32::new(0.0, 0.0); WINDOW];
    for f in 0..frames {
        let start = f * HOP;
        let mut energy = 0.0f32;
        for i in 0..WINDOW {
            let s = samples.get(start + i).copied().unwrap_or(0.0);
            energy += s * s;
            buf[i] = Complex32::new(s * window[i], 0.0);
        }
        rms_db.push(10.0 * (energy / WINDOW as f32 + 1e-12).log10());
        fft.process(&mut buf);
        let mut low = 0.0;
        let mut weighted = 0.0f32;
        let mut logs = vec![0.0f32; WINDOW / 2 + 1];
        for k in 1..=WINDOW / 2 {
            let mag = buf[k].norm();
            let l = (1.0 + 100.0 * mag).ln();
            logs[k] = l;
            weighted += mel[k] * (l - prev[k]).max(0.0);
        }
        beat_env.push(weighted / mel_sum);
        for (b, (lo, hi)) in ranges.iter().enumerate() {
            let mut flux = 0.0f32;
            for k in *lo..=*hi {
                if b == 0 {
                    let mag = buf[k].norm();
                    low += mag * mag;
                }
                flux += (logs[k] - prev[k]).max(0.0);
            }
            bands[b].push(flux / (*hi - *lo + 1) as f32);
        }
        prev.copy_from_slice(&logs);
        low_energy.push(low);
    }
    // Normalise each band by its median so no band dominates.
    for band in bands.iter_mut() {
        let m = median(band).max(1e-6);
        for v in band.iter_mut() {
            *v /= m;
        }
    }
    let onset: Vec<f32> = (0..frames).map(|i| bands[0][i] + bands[1][i] + bands[2][i]).collect();
    let m = median(&beat_env).max(1e-9);
    let beat_env: Vec<f32> = beat_env.iter().map(|v| v / m).collect();
    // Flux peaks when a transient sits about 0.8 of a window after the
    // frame start (the steepest part of the Hann ramp, earlier still from
    // silence under log compression); measured on synthetic kicks and hats.
    let t0 = (WINDOW as f64 * 0.8) / sr as f64;
    Envelopes { dt: HOP as f64 / sr as f64, t0, bands, onset, beat_env, rms_db, low_energy }
}

fn median(v: &[f32]) -> f32 {
    if v.is_empty() {
        return 0.0;
    }
    let mut s = v.to_vec();
    s.sort_by(f32::total_cmp);
    s[s.len() / 2]
}

/// Peaks of an envelope: local maxima above an adaptive threshold, at
/// least `spacing` seconds apart. Returns (time, strength).
fn peaks(env: &[f32], dt: f64, t0: f64, spacing: f64, sensitivity: f32) -> Vec<(f64, f32)> {
    let n = env.len();
    let half = ((0.15 / dt) as usize).max(1);
    let near = ((0.03 / dt) as usize).max(1);
    let floor = median(env);
    let loud = {
        let mut s = env.to_vec();
        s.sort_by(f32::total_cmp);
        s.get(s.len() * 99 / 100).copied().unwrap_or(0.0)
    };
    let mut prefix = vec![0.0f64; n + 1];
    for i in 0..n {
        prefix[i + 1] = prefix[i] + env[i] as f64;
    }
    let mut out: Vec<(f64, f32)> = Vec::new();
    for i in 1..n.saturating_sub(1) {
        let v = env[i];
        let lo = i.saturating_sub(half);
        let hi = (i + half + 1).min(n);
        let mean = ((prefix[hi] - prefix[lo]) / (hi - lo) as f64) as f32;
        if v < mean * sensitivity + floor * 0.5 || v < 1.5 * floor.max(1e-6) || v < 0.06 * loud {
            continue;
        }
        let (a, b) = (i.saturating_sub(near), (i + near + 1).min(n));
        if env[a..b].iter().any(|&x| x > v) || env[a..i].iter().any(|&x| x == v) {
            continue;
        }
        // Parabolic refinement.
        let (l, r) = (env[i - 1], env[i + 1]);
        let denom = l - 2.0 * v + r;
        let shift = if denom.abs() > 1e-9 { (0.5 * (l - r) / denom).clamp(-0.5, 0.5) } else { 0.0 };
        let t = t0 + (i as f64 + shift as f64) * dt;
        if let Some(last) = out.last_mut() {
            if t - last.0 < spacing {
                if v > last.1 {
                    *last = (t, v);
                }
                continue;
            }
        }
        out.push((t, v));
    }
    out
}

fn sample(env: &[f32], x: f64) -> f32 {
    if x < 0.0 {
        return 0.0;
    }
    let i = x.floor() as usize;
    if i + 1 >= env.len() {
        return env.last().copied().unwrap_or(0.0) * (i < env.len()) as u8 as f32;
    }
    let f = (x - i as f64) as f32;
    env[i] * (1.0 - f) + env[i + 1] * f
}

/// Sum of the envelope at period/phase grid points (in frames).
fn comb(env: &[f32], period: f64, phase: f64) -> f64 {
    let mut s = 0.0;
    let mut x = phase;
    let mut n = 0;
    while x < env.len() as f64 - 1.0 {
        // Max over ±1 frame tolerates sub-frame jitter.
        s += sample(env, x).max(sample(env, x - 1.0)).max(sample(env, x + 1.0)) as f64;
        x += period;
        n += 1;
    }
    if n == 0 {
        0.0
    } else {
        s / n as f64
    }
}

fn tempo(env: &[f32], dt: f64, start: usize, end: usize) -> (f64, f64, f64) {
    let seg: Vec<f32> = env[start..end.max(start + 1)].to_vec();
    let mean = seg.iter().sum::<f32>() / seg.len().max(1) as f32;
    let raw: Vec<f32> = seg.iter().map(|v| (v - mean).max(0.0)).collect();
    // Gaussian smoothing (σ ≈ 12 ms) so lags between frames still match.
    let sigma = (0.012 / dt) as f32;
    let radius = (sigma * 3.0).ceil() as isize;
    let kernel: Vec<f32> = (-radius..=radius).map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp()).collect();
    let x: Vec<f32> = (0..raw.len() as isize)
        .map(|i| {
            kernel
                .iter()
                .enumerate()
                .map(|(k, w)| raw.get((i + k as isize - radius).max(0) as usize).copied().unwrap_or(0.0) * w)
                .sum()
        })
        .collect();
    let min_lag = (60.0 / 220.0 / dt) as usize;
    let max_lag = ((60.0 / 50.0 / dt) as usize).min(x.len().saturating_sub(1));
    let ac_at = |lag: usize| -> f64 {
        if lag == 0 || lag >= x.len() {
            return 0.0;
        }
        x.iter().zip(&x[lag..]).map(|(a, b)| (*a * *b) as f64).sum::<f64>() / (x.len() - lag) as f64
    };
    // Fractional lag: the best of the neighbouring integer lags.
    let ac = |lag: f64| -> f64 { (lag.floor() as usize..=lag.ceil() as usize).map(ac_at).fold(0.0, f64::max) };
    let mut best = (0.0, 0usize);
    for lag in min_lag.max(1)..=max_lag.max(min_lag + 1) {
        if lag >= x.len() {
            break;
        }
        let bpm = 60.0 / (lag as f64 * dt);
        let prior = (-0.5 * ((bpm / 120.0).log2() / 1.0).powi(2)).exp();
        // Reward periods whose double and quadruple (a 4/4 bar) also
        // correlate: accents repeat per bar, which settles triplet and
        // half/double ambiguities.
        let l = lag as f64;
        let score = (ac(l) + 0.5 * ac(2.0 * l) + 0.5 * ac(4.0 * l)) * prior;
        if score > best.0 {
            best = (score, lag);
        }
    }
    let lag = best.1.max(1) as f64;
    // Fine search of period and phase with the comb.
    let mut top = (f64::MIN, lag, 0.0);
    let steps = 120;
    for k in 0..=steps {
        let period = lag * (0.97 + 0.06 * k as f64 / steps as f64);
        let phases = (period * 4.0).ceil() as usize;
        for p in 0..phases {
            let phase = start as f64 + p as f64 * 0.25;
            let s = comb(env, period, phase);
            if s > top.0 {
                top = (s, period, phase);
            }
        }
    }
    let (score, period, phase) = top;
    let baseline = env[start..end.max(start + 1)].iter().map(|v| *v as f64).sum::<f64>() / (end - start).max(1) as f64;
    let confidence = (1.0 - baseline / score.max(1e-9)).clamp(0.0, 1.0);
    (period * dt, phase, confidence)
}

pub fn analyze(samples: &[f32], sr: u32, offset: f64) -> Analysis {
    let env = envelopes(samples, sr);
    let duration = samples.len() as f64 / sr as f64;
    let n = env.onset.len();
    // Music start/end: frames within 40 dB of the loudest.
    let peak_db = env.rms_db.iter().copied().fold(f32::MIN, f32::max);
    let gate = peak_db - 40.0;
    let first = env.rms_db.iter().position(|v| *v > gate).unwrap_or(0);
    let last = env.rms_db.iter().rposition(|v| *v > gate).unwrap_or(n.saturating_sub(1));
    let music_start = (first as f64 * env.dt).max(0.0);
    let music_end = ((last * HOP + WINDOW) as f64 / sr as f64).min(duration);

    let (beat, mut phase_frames, grid_confidence) = tempo(&env.beat_env, env.dt, first, last.max(first + 1));
    let period_frames = beat / env.dt;
    // On-beat or off-beat? Kicks and snares sit on the beat.
    let (low_on, low_off) = (comb(&env.bands[0], period_frames, phase_frames), comb(&env.bands[0], period_frames, phase_frames + period_frames / 2.0));
    let off_beat = if (low_on - low_off).abs() > 0.2 * low_on.max(low_off) {
        low_off > low_on
    } else {
        comb(&env.beat_env, period_frames, phase_frames + period_frames / 2.0) > comb(&env.beat_env, period_frames, phase_frames)
    };
    if off_beat {
        phase_frames += period_frames / 2.0;
    }
    // Beat grid covering the music.
    let mut phase = env.t0 + phase_frames * env.dt;
    while phase - beat >= music_start - beat * 0.25 {
        phase -= beat;
    }
    while phase < music_start - beat * 0.25 {
        phase += beat;
    }
    // Sharpen the phase on refined onsets near grid points.
    let refined: Vec<f64> = peaks(&env.beat_env, env.dt, env.t0, 0.05, 1.4).iter().map(|(t, _)| refine(samples, sr, *t)).collect();
    let mut offsets: Vec<f64> = refined
        .iter()
        .map(|t| {
            let k = ((t - phase) / beat).round();
            t - (phase + k * beat)
        })
        .filter(|d| d.abs() < 0.06)
        .collect();
    if offsets.len() >= 4 {
        offsets.sort_by(f64::total_cmp);
        phase += offsets[offsets.len() / 2];
    }
    while phase - beat >= music_start - beat * 0.25 {
        phase -= beat;
    }
    let grid: Vec<f64> = (0..).map(|k| phase + k as f64 * beat).take_while(|t| *t <= music_end + 1e-9).collect();
    // Window max of an envelope around time t (−20…+40 ms).
    let window_max = |band: &[f32], t: f64| -> f64 {
        let a = ((t - 0.02 - env.t0) / env.dt).floor().max(0.0) as usize;
        let b = (((t + 0.04 - env.t0) / env.dt).ceil().max(0.0) as usize).min(band.len());
        band[a.min(b)..b].iter().copied().fold(0.0f32, f32::max) as f64
    };
    let low_max = env.bands[0].iter().copied().fold(0.0f32, f32::max).max(1e-9) as f64;
    let energy_max = env.low_energy.iter().copied().fold(0.0f32, f32::max).max(1e-12) as f64;
    let beat_max = env.beat_env.iter().copied().fold(0.0f32, f32::max).max(1e-9) as f64;
    // Tempo changes: a dynamic-programming beat tracker (Ellis 2007) seeded
    // with the period. It replaces the constant grid only when it fits the
    // onsets clearly better, so steady music keeps an exact grid.
    let fit = |b: &[f64]| b.iter().map(|t| window_max(&env.beat_env, *t)).sum::<f64>() / b.len().max(1) as f64 / beat_max;
    // Kicks and snares mark beats, so the tracker hears the low band too.
    let accent_env: Vec<f32> = env
        .beat_env
        .iter()
        .zip(&env.bands[0])
        .map(|(b, l)| (*b as f64 / beat_max + *l as f64 / low_max) as f32)
        .collect();
    let tracked = track_beats(&accent_env, period_frames, first, last.max(first + 1))
        .into_iter()
        .map(|i| env.t0 + i as f64 * env.dt)
        .collect::<Vec<f64>>();
    let mut tracked = align_to_onsets(tracked, &refined);
    // On the beat or between beats? Compare kick energy.
    if tracked.len() >= 3 {
        let mids: Vec<f64> = tracked.windows(2).map(|w| (w[0] + w[1]) / 2.0).collect();
        let kick = |ts: &[f64]| ts.iter().map(|t| window_max(&env.low_energy, *t + 0.02)).sum::<f64>() / ts.len() as f64;
        if kick(&mids) > 1.2 * kick(&tracked[..tracked.len() - 1]) {
            tracked = mids;
        }
    }
    let tempo_map = tracked.len() >= 8 && fit(&tracked) > 1.15 * fit(&grid);
    let beats: Vec<f64> = if tempo_map { tracked } else { grid };
    let beat = if tempo_map {
        let mut d: Vec<f64> = beats.windows(2).map(|w| w[1] - w[0]).collect();
        d.sort_by(f64::total_cmp);
        d[d.len() / 2]
    } else {
        beat
    };
    // Meter and bar phase: accents (kick energy) repeat every 3 or 4 beats.
    let accent: Vec<f64> = beats
        .iter()
        .map(|t| {
            window_max(&env.low_energy, *t + 0.02) / energy_max + 0.5 * window_max(&env.bands[0], *t) / low_max + 0.25 * window_max(&env.beat_env, *t) / beat_max
        })
        .collect();
    let mean_accent = accent.iter().sum::<f64>() / accent.len().max(1) as f64;
    let best_phase = |m: usize| -> (usize, f64) {
        (0..m)
            .map(|o| {
                let picked: Vec<f64> = accent.iter().skip(o).step_by(m).copied().collect();
                (o, picked.iter().sum::<f64>() / picked.len().max(1) as f64)
            })
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .unwrap_or((0, 0.0))
    };
    let (o4, s4) = best_phase(4);
    let (o3, s3) = best_phase(3);
    // 4/4 unless 3/4 is clearly stronger (contrast of the downbeat accent).
    let (beats_per_bar, bar_offset) = if s3 / mean_accent.max(1e-9) > 1.12 * s4 / mean_accent.max(1e-9) { (3u32, o3) } else { (4u32, o4) };
    let downbeats: Vec<f64> = beats.iter().skip(bar_offset).step_by(beats_per_bar as usize).copied().collect();
    // Feel: onset strength at beat fractions.
    let frac = |f: f64| -> f64 {
        let vals: Vec<f64> = beats
            .iter()
            .map(|t| {
                let x = (t + f * beat - env.t0) / env.dt;
                sample(&env.onset, x).max(sample(&env.onset, x - 1.0)).max(sample(&env.onset, x + 1.0)) as f64
            })
            .collect();
        vals.iter().sum::<f64>() / vals.len().max(1) as f64
    };
    let phase_strength = [frac(0.0), frac(1.0 / 3.0), frac(0.5), frac(2.0 / 3.0)];
    let triplet = (phase_strength[1] + phase_strength[3]) / 2.0;
    let feel = if triplet > 1.6 * phase_strength[2] && triplet > 0.35 * phase_strength[0] { "triplet" } else { "straight" };

    let shift = |t: f64| t + offset;
    let track = |name: &str, times: Vec<f64>| MarkerTrack::from_times(name, times.into_iter().map(shift));
    let sub = |div: usize| -> Vec<f64> {
        let mut v = Vec::new();
        for (i, t) in beats.iter().enumerate() {
            // Subdivide each beat by its own length (tempo maps).
            let len = beats.get(i + 1).map(|n| n - t).unwrap_or(beat);
            for k in 0..div {
                let s = t + len * k as f64 / div as f64;
                if s <= music_end {
                    v.push(s);
                }
            }
        }
        v
    };
    let mut tracks = vec![
        track("beat", beats.clone()),
        track("bar", downbeats.clone()),
        track("half", sub(2)),
        track("sixteenth", sub(4)),
        track("triplet", sub(3)),
    ];
    // Onsets per band and combined.
    let spacing = 0.05;
    let refine_all = |p: Vec<(f64, f32)>| -> Vec<(f64, f32)> { p.into_iter().map(|(t, v)| (refine(samples, sr, t), v)).collect() };
    let mut all = refine_all(peaks(&env.onset, env.dt, env.t0, spacing, 1.4));
    for (name, band) in ["onset.low", "onset.mid", "onset.high"].iter().zip(env.bands.iter()) {
        let p = refine_all(peaks(band, env.dt, env.t0, spacing, 1.5));
        tracks.push(MarkerTrack {
            name: name.to_string(),
            source: None,
            markers: p.iter().map(|(t, v)| Marker { t: shift(*t), value: Some(round3(*v as f64)), ..Default::default() }).collect(),
        });
    }
    all.retain(|(t, _)| *t >= music_start - 0.05 && *t <= music_end + 0.05);
    let med = {
        let mut s: Vec<f32> = all.iter().map(|p| p.1).collect();
        s.sort_by(f32::total_cmp);
        s.get(s.len() / 2).copied().unwrap_or(1.0)
    };
    tracks.push(MarkerTrack {
        name: "onset".into(),
        source: None,
        markers: all.iter().map(|(t, v)| Marker { t: shift(*t), value: Some(round3((*v / med.max(1e-6)) as f64)), ..Default::default() }).collect(),
    });
    // Hits: onsets at least twice the median and in the top fifth.
    let mut strengths: Vec<f32> = all.iter().map(|p| p.1).collect();
    strengths.sort_by(f32::total_cmp);
    let top_fifth = strengths.get(strengths.len() * 4 / 5).copied().unwrap_or(f32::MAX);
    let mut hits: Vec<&(f64, f32)> = all.iter().filter(|(_, v)| *v >= (2.0 * med).max(top_fifth)).collect();
    if hits.is_empty() {
        hits.extend(all.iter().max_by(|a, b| a.1.total_cmp(&b.1)));
    }
    let max_hit = hits.iter().map(|h| h.1).fold(0.0f32, f32::max).max(1e-6);
    tracks.push(MarkerTrack {
        name: "hit".into(),
        source: None,
        markers: hits
            .iter()
            .map(|(t, v)| Marker {
                t: shift(*t),
                value: Some(round3((*v / max_hit) as f64)),
                label: (*v == max_hit).then(|| "largest".to_string()),
                ..Default::default()
            })
            .collect(),
    });
    // Stops: ≥ 120 ms well below the typical level, then an onset.
    let level = median(&env.rms_db[first..=last.max(first)]);
    let quiet = level - 18.0;
    let mut stops = Vec::new();
    let mut i = first;
    while i <= last {
        if env.rms_db[i] < quiet {
            let s = i;
            while i <= last && env.rms_db[i] < quiet {
                i += 1;
            }
            let gap = (i - s) as f64 * env.dt;
            let gap_end = env.t0 + i as f64 * env.dt;
            if gap >= 0.12 {
                if let Some((t, _)) = all.iter().find(|(t, _)| (*t - gap_end).abs() <= 0.06) {
                    stops.push(Marker {
                        t: shift(*t),
                        value: Some(round3(gap)),
                        label: Some(format!("{:.0} ms silence before the hit", gap * 1000.0)),
                        ..Default::default()
                    });
                }
            }
        }
        i += 1;
    }
    tracks.push(MarkerTrack { name: "stop".into(), source: None, markers: stops });
    // Drops: the bass falls under a quarter of its running level for at
    // least half a beat and comes back on (or near) a downbeat.
    let smooth = ((0.05 / env.dt) as usize).max(1);
    let low: Vec<f32> = (0..n)
        .map(|i| {
            let (a, b) = (i.saturating_sub(smooth), (i + smooth + 1).min(n));
            env.low_energy[a..b].iter().sum::<f32>() / (b - a) as f32
        })
        .collect();
    let running = ((4.0 / env.dt) as usize).max(1);
    let mut drops = Vec::new();
    let mut i = first;
    while i <= last {
        let (a, b) = (i.saturating_sub(running), (i + running).min(n));
        let reference = median(&low[a..b]);
        if reference > 0.0 && low[i] < 0.25 * reference {
            let s = i;
            while i <= last && low[i] < 0.25 * reference {
                i += 1;
            }
            let len = (i - s) as f64 * env.dt;
            let back = env.t0 + i as f64 * env.dt;
            if len >= 0.5 * beat {
                if let Some(db) = downbeats.iter().find(|d| (**d - back).abs() <= 0.5 * beat) {
                    drops.push(Marker {
                        t: shift(*db),
                        value: Some(round3(len)),
                        label: Some(format!("bass out {:.2} s", len)),
                        ..Default::default()
                    });
                }
            }
        }
        i += 1;
    }
    tracks.push(MarkerTrack { name: "drop".into(), source: None, markers: drops });
    tracks.push(MarkerTrack::from_times("music.start", [shift(music_start)]));
    tracks.push(MarkerTrack::from_times("music.end", [shift(music_end)]));
    Analysis {
        source: String::new(),
        duration,
        bpm: round3(60.0 / beat),
        beat,
        first_beat: shift(beats.first().copied().unwrap_or(0.0)),
        first_downbeat: shift(downbeats.first().copied().unwrap_or(0.0)),
        beats_per_bar,
        tempo_varies: tempo_map,
        feel: feel.into(),
        phase_strength: phase_strength.map(round3),
        grid_confidence: round3(grid_confidence),
        music_start: shift(music_start),
        music_end: shift(music_end),
        tracks,
    }
}

/// Dynamic-programming beat tracking: maximise onset strength at beats
/// minus a penalty on deviations from the period (log-ratio squared).
fn track_beats(env: &[f32], period: f64, start: usize, end: usize) -> Vec<usize> {
    let end = end.min(env.len());
    if end <= start + 4 || period < 2.0 {
        return Vec::new();
    }
    let seg = &env[start..end];
    let mean = seg.iter().map(|v| *v as f64).sum::<f64>() / seg.len() as f64;
    let sd = (seg.iter().map(|v| (*v as f64 - mean).powi(2)).sum::<f64>() / seg.len() as f64).sqrt().max(1e-9);
    let o: Vec<f64> = seg.iter().map(|v| *v as f64 / sd).collect();
    let n = o.len();
    let tightness = 100.0;
    let mut score = vec![0.0f64; n];
    let mut back = vec![usize::MAX; n];
    let (lo, hi) = ((period * 0.5).round() as usize, (period * 2.0).round() as usize);
    for t in 0..n {
        let mut best = (0.0f64, usize::MAX);
        if t >= lo {
            for tau in t.saturating_sub(hi)..=t - lo {
                let r = ((t - tau) as f64 / period).ln();
                let v = score[tau] - tightness * r * r;
                if back[t] == usize::MAX && best.1 == usize::MAX || v > best.0 {
                    best = (v, tau);
                }
            }
        }
        score[t] = o[t] + if best.1 == usize::MAX { 0.0 } else { best.0.max(0.0) };
        back[t] = if best.1 != usize::MAX && best.0 > 0.0 { best.1 } else { usize::MAX };
    }
    // Start from the best beat within the last period.
    let tail = n.saturating_sub(period.ceil() as usize);
    let mut t = (tail..n).max_by(|a, b| score[*a].total_cmp(&score[*b])).unwrap_or(n - 1);
    let mut beats = vec![t];
    while back[t] != usize::MAX {
        t = back[t];
        beats.push(t);
    }
    beats.reverse();
    beats.into_iter().map(|b| b + start).collect()
}

/// Shifts tracked beats by the median offset to nearby refined onsets
/// (removes the envelope's frame-time bias).
fn align_to_onsets(beats: Vec<f64>, onsets: &[f64]) -> Vec<f64> {
    let mut offsets: Vec<f64> = beats
        .iter()
        .filter_map(|b| onsets.iter().map(|o| o - b).min_by(|x, y| x.abs().total_cmp(&y.abs())))
        .filter(|d| d.abs() < 0.05)
        .collect();
    if offsets.len() < 4 {
        return beats;
    }
    offsets.sort_by(f64::total_cmp);
    let shift = offsets[offsets.len() / 2];
    beats.into_iter().map(|b| b + shift).collect()
}

/// Moves an onset estimate onto the transient: the peak of a short-window
/// (256-sample, 1.5 ms hop) spectral flux within −50…+30 ms. Stationary
/// sounds (a bass note) produce no flux, so they cannot pull the estimate.
fn refine(samples: &[f32], sr: u32, t: f64) -> f64 {
    const N: usize = 256;
    const STEP: usize = 32;
    let from = ((t - 0.05) * sr as f64).max(0.0) as usize;
    let to = (((t + 0.03) * sr as f64).max(0.0) as usize).min(samples.len().saturating_sub(N));
    if to <= from + 2 * STEP {
        return t;
    }
    thread_local! {
        static FFT: std::sync::Arc<dyn rustfft::Fft<f32>> = FftPlanner::new().plan_fft_forward(N);
    }
    let window: Vec<f32> = (0..N).map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / N as f32).cos()).collect();
    let mut prev: Option<Vec<f32>> = None;
    let mut best = (0.0f32, t);
    let mut buf = vec![Complex32::new(0.0, 0.0); N];
    let mut pos = from;
    while pos <= to {
        for i in 0..N {
            buf[i] = Complex32::new(samples[pos + i] * window[i], 0.0);
        }
        FFT.with(|f| f.process(&mut buf));
        let logs: Vec<f32> = buf[1..N / 2].iter().map(|c| (1.0 + 100.0 * c.norm()).ln()).collect();
        if let Some(p) = &prev {
            let flux: f32 = logs.iter().zip(p).map(|(a, b)| (a - b).max(0.0)).sum();
            if flux > best.0 {
                best = (flux, (pos as f64 + 0.8 * N as f64) / sr as f64);
            }
        }
        prev = Some(logs);
        pos += STEP;
    }
    best.1
}

fn round3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

#[cfg(test)]
mod tests;
