//! Built-in sound effects, synthesised: no files to source or license.
//!
//! Each is short, mono, 48 kHz and peaks at −1 dBFS, shaped for motion
//! design: air for movement (whoosh, swipe), transients for arrivals (pop,
//! click, thud, hit, boom), tails for reveals (ding, sparkle), build-ups
//! (riser) and texture (glitch, type). Generation is deterministic.

use std::f32::consts::TAU;
use std::path::Path;

pub const RATE: u32 = 48_000;

/// Name, length in seconds, and what it is for.
pub const SOUNDS: &[(&str, f32, &str)] = &[
    ("whoosh", 0.7, "air rushing past: a layer flying in or a camera move"),
    ("whoosh-out", 0.6, "air falling away: a layer leaving"),
    ("swipe", 0.28, "a quick flick: slides, wipes, cards"),
    ("pop", 0.14, "a bubbly pop: something appearing, a scale-up"),
    ("click", 0.06, "a UI click: buttons, toggles, cursor presses"),
    ("tick", 0.05, "a light tick: counters, small steps, staggered items"),
    ("type", 0.07, "a key press: typing text"),
    ("thud", 0.45, "a soft landing: a card settling, a drop"),
    ("hit", 0.9, "an impact: a title slam, a cut on a downbeat"),
    ("boom", 1.8, "a deep boom: the drop, a big reveal"),
    ("riser", 1.6, "a build-up that ends on its last frame: before a reveal"),
    ("ding", 1.4, "a bell: success, a notification, a reveal"),
    ("sparkle", 0.9, "glitter: magic, a highlight appearing"),
    ("glitch", 0.35, "digital breakup: glitch transitions, errors"),
];

/// Samples of a built-in sound, or None for an unknown name.
pub fn generate(name: &str) -> Option<Vec<f32>> {
    let &(_, seconds, _) = SOUNDS.iter().find(|s| s.0 == name)?;
    let n = (seconds * RATE as f32) as usize;
    let mut rng = Rng(0x9e37_79b9 ^ name.bytes().fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32)));
    let t = |i: usize| i as f32 / RATE as f32;
    let mut out = vec![0.0f32; n];
    match name {
        "whoosh" | "whoosh-out" | "swipe" => {
            let (peak, f0, f1, f2): (f32, f32, f32, f32) = match name {
                "whoosh" => (0.6, 300.0, 2600.0, 700.0),
                "whoosh-out" => (0.2, 2400.0, 900.0, 250.0),
                _ => (0.45, 900.0, 5200.0, 2000.0),
            };
            let mut bp = Svf::default();
            for (i, o) in out.iter_mut().enumerate() {
                let x = i as f32 / n as f32;
                // Cutoff glides f0 → f1 at the peak → f2.
                let fc = if x < peak { f0 * (f1 / f0).powf(x / peak) } else { f1 * (f2 / f1).powf((x - peak) / (1.0 - peak)) };
                let env = if x < peak { (x / peak).powi(2) } else { ((1.0 - x) / (1.0 - peak)).powf(1.6) };
                *o = bp.band(rng.noise(), fc, 1.6) * env;
            }
        }
        "pop" => {
            let mut phase = 0.0f32;
            for (i, o) in out.iter_mut().enumerate() {
                let s = t(i);
                let f = 250.0 + 850.0 * (-s / 0.018).exp();
                phase += TAU * f / RATE as f32;
                let click = if s < 0.002 { rng.noise() * (1.0 - s / 0.002) * 0.4 } else { 0.0 };
                *o = phase.sin() * (-s / 0.03).exp() * (s / 0.002).min(1.0) + click;
            }
        }
        "click" | "tick" | "type" => {
            let (fa, fb, ta): (f32, f32, f32) = match name {
                "click" => (3000.0, 1200.0, 0.006),
                "tick" => (4600.0, 2300.0, 0.004),
                _ => (1800.0 + rng.noise() * 120.0, 700.0, 0.008),
            };
            let mut hp = OnePole::default();
            for (i, o) in out.iter_mut().enumerate() {
                let s = t(i);
                let burst = if s < 0.0025 { hp.high(rng.noise(), 2000.0) * (1.0 - s / 0.0025) } else { 0.0 };
                *o = 0.6 * burst + 0.7 * (TAU * fa * s).sin() * (-s / ta).exp() + 0.4 * (TAU * fb * s).sin() * (-s / (ta * 1.8)).exp();
            }
        }
        "thud" | "hit" | "boom" => {
            let (f_hi, f_lo, sweep, tau, noise_tau, noise_cut, drive): (f32, f32, f32, f32, f32, f32, f32) = match name {
                "thud" => (95.0, 50.0, 0.05, 0.09, 0.015, 600.0, 1.4),
                "hit" => (130.0, 45.0, 0.08, 0.25, 0.03, 900.0, 2.0),
                _ => (75.0, 31.0, 0.25, 0.7, 0.4, 300.0, 2.4),
            };
            let mut lp = OnePole::default();
            let mut phase = 0.0f32;
            for (i, o) in out.iter_mut().enumerate() {
                let s = t(i);
                let f = f_lo + (f_hi - f_lo) * (-s / sweep).exp();
                phase += TAU * f / RATE as f32;
                let body = phase.sin() * (-s / tau).exp();
                let air = lp.low(rng.noise(), noise_cut) * (-s / noise_tau).exp() * 1.5;
                let attack = (s / 0.002).min(1.0);
                *o = ((body + air) * drive).tanh() * attack;
            }
        }
        "riser" => {
            let mut bp = Svf::default();
            let mut phase = 0.0f32;
            for (i, o) in out.iter_mut().enumerate() {
                let x = i as f32 / n as f32;
                let fc = 400.0 * (6500.0f32 / 400.0).powf(x);
                let f = 180.0 * (1200.0f32 / 180.0).powf(x * x);
                phase += TAU * f / RATE as f32;
                let tone = (phase.sin() + 0.35 * (2.0 * phase).sin()) * 0.35;
                let end = ((1.0 - x) * n as f32 / (0.003 * RATE as f32)).min(1.0);
                *o = (bp.band(rng.noise(), fc, 2.5) + tone) * x.powf(2.2) * end;
            }
        }
        "ding" => {
            let partials = [(1.0, 1.0, 0.9), (2.76, 0.5, 0.5), (5.4, 0.25, 0.3), (8.93, 0.12, 0.2)];
            for (i, o) in out.iter_mut().enumerate() {
                let s = t(i);
                let attack = (s / 0.003).min(1.0);
                *o = attack * partials.iter().map(|&(r, a, d)| a * (TAU * 1320.0 * r * s).sin() * (-s / d).exp()).sum::<f32>();
            }
        }
        "sparkle" => {
            let notes = [2093.0, 2637.0, 3136.0, 4186.0, 5274.0, 6272.0];
            for (k, f) in notes.iter().enumerate() {
                let start = k as f32 * 0.07;
                for (i, o) in out.iter_mut().enumerate() {
                    let s = t(i) - start;
                    if s >= 0.0 {
                        let vib = 1.0 + 0.004 * (TAU * 7.0 * s).sin();
                        *o += 0.5 * (TAU * f * vib * s).sin() * (-s / 0.12).exp() * (s / 0.002).min(1.0);
                    }
                }
            }
        }
        "glitch" => {
            let mut i = 0;
            while i < n {
                let len = ((0.015 + 0.025 * rng.unit()) * RATE as f32) as usize;
                let f = 200.0 + 2200.0 * rng.unit();
                let noisy = rng.unit() < 0.4;
                let crush = 2.0 + (rng.unit() * 6.0).floor();
                for j in i..(i + len).min(n) {
                    let s = (j - i) as f32 / RATE as f32;
                    let v = if noisy { rng.noise() } else { (TAU * f * s).sin().signum() * 0.7 };
                    out[j] = (v * crush).round() / crush * 0.8;
                }
                i += len;
            }
            let fade = (0.01 * RATE as f32) as usize;
            for k in 0..fade.min(n) {
                out[n - 1 - k] *= k as f32 / fade as f32;
            }
        }
        _ => return None,
    }
    normalize(&mut out, 0.89);
    Some(out)
}

/// Writes mono 16-bit PCM.
pub fn write_wav(path: &Path, samples: &[f32], rate: u32) -> std::io::Result<()> {
    let data = samples.len() as u32 * 2;
    let mut b = Vec::with_capacity(44 + data as usize);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&rate.to_le_bytes());
    b.extend_from_slice(&(rate * 2).to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data.to_le_bytes());
    for s in samples {
        b.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, b)
}

/// Generates `name` into `path` unless it is already there.
pub fn ensure(name: &str, path: &Path) -> Result<(), String> {
    if path.is_file() {
        return Ok(());
    }
    let samples = generate(name).ok_or_else(|| format!("no built-in sound '{name}' (try: {})", SOUNDS.iter().map(|s| s.0).collect::<Vec<_>>().join(", ")))?;
    write_wav(path, &samples, RATE).map_err(|e| format!("{}: {e}", path.display()))
}

fn normalize(s: &mut [f32], peak: f32) {
    let m = s.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    if m > 0.0 {
        for v in s {
            *v *= peak / m;
        }
    }
}

struct Rng(u32);

impl Rng {
    fn unit(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 >> 8) as f32 / (1u32 << 24) as f32
    }
    fn noise(&mut self) -> f32 {
        self.unit() * 2.0 - 1.0
    }
}

/// Chamberlin state-variable filter.
#[derive(Default)]
struct Svf {
    low: f32,
    band: f32,
}

impl Svf {
    fn band(&mut self, x: f32, fc: f32, q: f32) -> f32 {
        let f = 2.0 * (std::f32::consts::PI * fc.min(RATE as f32 / 6.0) / RATE as f32).sin();
        let damp = 1.0 / q;
        let high = x - self.low - damp * self.band;
        self.band += f * high;
        self.low += f * self.band;
        self.band
    }
}

#[derive(Default)]
struct OnePole {
    y: f32,
    x: f32,
}

impl OnePole {
    fn low(&mut self, x: f32, fc: f32) -> f32 {
        let a = 1.0 - (-TAU * fc / RATE as f32).exp();
        self.y += a * (x - self.y);
        self.y
    }
    fn high(&mut self, x: f32, fc: f32) -> f32 {
        let a = (-TAU * fc / RATE as f32).exp();
        self.y = a * (self.y + x - self.x);
        self.x = x;
        self.y
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_sound_is_audible_finite_deterministic_and_shaped() {
        for &(name, seconds, _) in SOUNDS {
            let s = generate(name).unwrap();
            assert_eq!(s.len(), (seconds * RATE as f32) as usize, "{name}");
            assert!(s.iter().all(|v| v.is_finite()), "{name}");
            let peak = s.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            assert!((peak - 0.89).abs() < 1e-3, "{name} peak {peak}");
            assert_eq!(s, generate(name).unwrap(), "{name} is deterministic");
            // No click at the end: the last 2 ms are quiet.
            let tail = &s[s.len() - (0.002 * RATE as f32) as usize..];
            let tail_peak = tail.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            assert!(tail_peak < 0.35, "{name} ends at {tail_peak}");
        }
        assert!(generate("nope").is_none());
    }

    #[test]
    fn arrivals_peak_early_and_air_peaks_late() {
        let peak_at = |name: &str| {
            let s = generate(name).unwrap();
            let i = s.iter().enumerate().fold((0, 0.0f32), |b, (i, v)| if v.abs() > b.1 { (i, v.abs()) } else { b }).0;
            i as f32 / RATE as f32
        };
        assert!(peak_at("pop") < 0.02 && peak_at("click") < 0.01 && peak_at("hit") < 0.05);
        assert!(peak_at("whoosh") > 0.25, "a whoosh swells towards its middle");
        assert!(peak_at("riser") > 1.3, "a riser peaks at its end");
    }

    #[test]
    fn wav_files_decode() {
        let dir = std::env::temp_dir().join(format!("lahza-sfx-{}", std::process::id()));
        let path = dir.join("pop.wav");
        ensure("pop", &path).unwrap();
        let decoded = crate::decode(&path).unwrap();
        assert!((decoded.len() as f32 / crate::SAMPLE_RATE as f32 - 0.14).abs() < 0.01);
        std::fs::remove_dir_all(&dir).ok();
    }
}

/// `LAHZA_SFX_OUT=<dir> cargo test -p lahza-motion-audio -- --ignored dump_sounds`
/// writes every sound as a wav to listen to.
#[test]
#[ignore = "writes files for listening"]
fn dump_sounds() {
    let dir = std::path::PathBuf::from(std::env::var("LAHZA_SFX_OUT").unwrap_or_else(|_| "sfx-out".into()));
    for &(name, _, _) in SOUNDS {
        ensure(name, &dir.join(format!("{name}.wav"))).unwrap();
    }
}
