//! Curves: the measured motion vocabulary (design section 5) and raw curves.
//!
//! An [`Ease`] is what a document says (`"glide"`, `{"type":"spring",…}`);
//! a [`Curve`] is the compiled, frame-rate-aware form. Every curve maps the
//! elapsed time of a move to progress, 0 at the start and exactly 1 at the
//! end of the move's duration. Physical curves (exponential lag, springs)
//! run in real seconds so their character does not change with the move's
//! length; a cubic correction that is invisible for adequate durations makes
//! them land exactly on the target.

use crate::time::{Duration, Fps, TimeBase};
use crate::{bail, Result};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Frame rate the studies were measured at (Motion Array and the showreel
/// are 25 fps). Per-frame preset numbers are converted to seconds with it.
pub const REFERENCE_FPS: f64 = 25.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PowerMode {
    In,
    Out,
    InOut,
}

/// A curve as written in a document.
#[derive(Clone, Debug, PartialEq)]
pub enum Ease {
    Preset(Preset),
    Linear,
    /// Jump to the new value at the end of the move.
    Hold,
    /// CSS-style cubic-bezier(x1, y1, x2, y2).
    Bezier([f64; 4]),
    Power { n: f64, mode: PowerMode },
    /// Exponential lag: keep `r` of the remaining distance per project frame,
    /// or time constant `tau`.
    Exp { r: Option<f64>, tau: Option<Duration> },
    /// Damped spring from rest: damping ratio and natural frequency in Hz.
    Spring { zeta: f64, hz: f64 },
    /// Hand-shaped bounces: arc exponent, duration ratio between bounces.
    Bounce { bounces: u32, ratio: f64, exponent: f64 },
    /// Accelerate into a dead stop, t^n.
    Slam { n: f64 },
}

impl Default for Ease {
    fn default() -> Self {
        Ease::Linear
    }
}

/// Named presets with their measured numbers (design section 5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Preset {
    Snap,
    Glide,
    Settle,
    Enter,
    Gesture,
    Pop,
    Land,
    Squash,
    Bounce,
    Slam,
    Zoom,
    Push,
}

pub struct PresetInfo {
    pub preset: Preset,
    pub name: &'static str,
    pub model: &'static str,
    pub measured: &'static str,
    pub use_for: &'static str,
    /// Natural duration in seconds, used when a key gives no `dur`.
    pub duration: f64,
}

pub const PRESETS: &[PresetInfo] = &[
    PresetInfo { preset: Preset::Snap, name: "snap", model: "exponential lag, r = 0.5 per 25 fps frame (tau ≈ 1.44 f)", measured: "90% in ~3 f (showreel pops)", use_for: "fast UI pops and snaps; starts at full speed", duration: 0.32 },
    PresetInfo { preset: Preset::Glide, name: "glide", model: "exponential lag, r = 0.75 per 25 fps frame (tau ≈ 3.5 f)", measured: "r 0.7–0.79, 90% in 6–9 f (showreel, IMEX, BeyondTheBlack)", use_for: "default entrance and camera moves; starts at full speed, long soft tail", duration: 0.68 },
    PresetInfo { preset: Preset::Settle, name: "settle", model: "exponential lag, r = 0.8 per 25 fps frame (tau ≈ 4.5 f)", measured: "r 0.80–0.82, 90% in ~11 f (showreel large drags, 3D settle)", use_for: "large or heavy moves settling into place", duration: 0.84 },
    PresetInfo { preset: Preset::Enter, name: "enter", model: "ease-out quart, 1-(1-u)^4", measured: "6–12 f, 90% in 4–6 f (Motion Array)", use_for: "type and object entrances over 6–12 frames", duration: 0.4 },
    PresetInfo { preset: Preset::Gesture, name: "gesture", model: "symmetric ease-in-out cubic", measured: "5–8 f, peak speed mid-move (press, riffle, wipe, zoom-through)", use_for: "transitions and moves between two rests; anchor its peak to the beat", duration: 0.28 },
    PresetInfo { preset: Preset::Pop, name: "pop", model: "spring, zeta 0.61, omega 40 rad/s (6.37 Hz)", measured: "~9% overshoot, settled ≤ 6 f (Motion Array end logo)", use_for: "scale only: pop-ins and logo landings", duration: 0.28 },
    PresetInfo { preset: Preset::Land, name: "land", model: "spring, zeta 0.55, 2.2 Hz", measured: "zeta 0.35–0.55, 1.6–2.8 Hz, 6–17% overshoot (IMEX)", use_for: "objects landing into a grid or onto a surface", duration: 0.9 },
    PresetInfo { preset: Preset::Squash, name: "squash", model: "spring, zeta 0.3, 5 Hz", measured: "~5 Hz, zeta ≈ 0.3 (BOUNC squash deformer)", use_for: "squash/stretch scale wobble after an impact", duration: 0.9 },
    PresetInfo { preset: Preset::Bounce, name: "bounce", model: "hand-shaped arcs, exponent 2.75, each bounce 0.43× the previous one's duration (heights ×0.18)", measured: "e ≈ 0.43, 35→15 f bounces (BOUNC)", use_for: "a drop that bounces to rest", duration: 1.2 },
    PresetInfo { preset: Preset::Slam, name: "slam", model: "t², accelerate into a dead stop", measured: "BeyondTheBlack opening words", use_for: "words or objects slamming into place on a hit; anchor its end to the hit", duration: 0.24 },
    PresetInfo { preset: Preset::Zoom, name: "zoom", model: "ease-in-out cubic in log space", measured: "6–10× zooms (Motion Array, IMEX tunnel)", use_for: "scale and camera zooms over large ratios", duration: 0.6 },
    PresetInfo { preset: Preset::Push, name: "push", model: "linear", measured: "~3–4% scale per beat (Vox); 'rarely snaps' (Harris)", use_for: "slow constant camera pushes; set dur to the shot length", duration: 2.0 },
];

impl Preset {
    pub fn info(self) -> &'static PresetInfo {
        PRESETS.iter().find(|info| info.preset == self).expect("every preset has info")
    }
    pub fn by_name(name: &str) -> Option<Self> {
        PRESETS.iter().find(|info| info.name == name).map(|info| info.preset)
    }
    /// The raw curve behind the preset, in seconds.
    pub fn raw(self) -> Ease {
        let tau = |r: f64| Some(Duration::Seconds(-1.0 / (REFERENCE_FPS * f64::ln(r))));
        match self {
            Preset::Snap => Ease::Exp { r: None, tau: tau(0.5) },
            Preset::Glide => Ease::Exp { r: None, tau: tau(0.75) },
            Preset::Settle => Ease::Exp { r: None, tau: tau(0.8) },
            Preset::Enter => Ease::Power { n: 4.0, mode: PowerMode::Out },
            Preset::Gesture | Preset::Zoom => Ease::Power { n: 3.0, mode: PowerMode::InOut },
            Preset::Pop => Ease::Spring { zeta: 0.61, hz: 40.0 / std::f64::consts::TAU },
            Preset::Land => Ease::Spring { zeta: 0.55, hz: 2.2 },
            Preset::Squash => Ease::Spring { zeta: 0.3, hz: 5.0 },
            Preset::Bounce => Ease::Bounce { bounces: 3, ratio: 0.43, exponent: 2.75 },
            Preset::Slam => Ease::Slam { n: 2.0 },
            Preset::Push => Ease::Linear,
        }
    }
}

impl Ease {
    pub fn parse(text: &str) -> Result<Self> {
        match text.trim() {
            "linear" => Ok(Ease::Linear),
            "hold" => Ok(Ease::Hold),
            "ease-in" | "in" => Ok(Ease::Power { n: 3.0, mode: PowerMode::In }),
            "ease-out" | "out" => Ok(Ease::Power { n: 3.0, mode: PowerMode::Out }),
            "ease-in-out" | "in-out" => Ok(Ease::Power { n: 3.0, mode: PowerMode::InOut }),
            name => match Preset::by_name(name) {
                Some(preset) => Ok(Ease::Preset(preset)),
                None => bail!(
                    "unknown ease '{name}': use a preset ({}), linear, hold, or an object like {{\"type\":\"bezier\",\"p\":[0.2,0,0,1]}}",
                    PRESETS.iter().map(|p| p.name).collect::<Vec<_>>().join(", ")
                ),
            },
        }
    }

    /// Whether this ease interpolates in log space (scale zooms).
    pub fn is_log(&self) -> bool {
        matches!(self, Ease::Preset(Preset::Zoom))
    }

    /// Natural duration in seconds when a key has none.
    pub fn natural_duration(&self, fps: Fps) -> f64 {
        match self {
            Ease::Preset(p) => p.info().duration,
            Ease::Exp { .. } | Ease::Spring { .. } => {
                // Time to 99.5% (exp) or to settle within 1% (spring).
                let curve = Curve::compile(self, &TimeBase::new(fps)).unwrap_or(Curve::Linear);
                curve.settle_time(0.005).min(4.0)
            }
            Ease::Bounce { .. } => 1.2,
            _ => 0.4,
        }
    }
}

impl Serialize for Ease {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        use serde_json::json;
        let value = match self {
            Ease::Preset(p) => json!(p.info().name),
            Ease::Linear => json!("linear"),
            Ease::Hold => json!("hold"),
            Ease::Bezier(p) => json!({"type": "bezier", "p": p}),
            Ease::Power { n, mode } => json!({"type": "power", "n": n, "mode": match mode {
                PowerMode::In => "in", PowerMode::Out => "out", PowerMode::InOut => "in-out" }}),
            Ease::Exp { r, tau } => {
                let mut v = json!({"type": "exp"});
                if let Some(r) = r {
                    v["r"] = json!(r);
                }
                if let Some(tau) = tau {
                    v["tau"] = serde_json::to_value(tau).unwrap_or_default();
                }
                v
            }
            Ease::Spring { zeta, hz } => json!({"type": "spring", "zeta": zeta, "hz": hz}),
            Ease::Bounce { bounces, ratio, exponent } => {
                json!({"type": "bounce", "bounces": bounces, "ratio": ratio, "exponent": exponent})
            }
            Ease::Slam { n } => json!({"type": "slam", "n": n}),
        };
        value.serialize(s)
    }
}

impl<'de> Deserialize<'de> for Ease {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(d)?;
        Ease::from_json(&value).map_err(serde::de::Error::custom)
    }
}

impl Ease {
    pub fn from_json(value: &serde_json::Value) -> Result<Self> {
        use serde_json::Value;
        let obj = match value {
            Value::String(s) => return Ease::parse(s),
            Value::Object(o) => o,
            other => bail!("ease must be a name or an object, got {other}"),
        };
        let num = |key: &str, default: f64| -> Result<f64> {
            match obj.get(key) {
                None => Ok(default),
                Some(v) => v.as_f64().ok_or_else(|| format!("ease.{key} must be a number").into()),
            }
        };
        let kind = obj.get("type").and_then(Value::as_str).unwrap_or("");
        Ok(match kind {
            "bezier" | "cubic-bezier" => {
                let p = obj
                    .get("p")
                    .and_then(Value::as_array)
                    .filter(|a| a.len() == 4)
                    .ok_or("bezier ease needs \"p\": [x1, y1, x2, y2]")?;
                let mut out = [0.0; 4];
                for (slot, v) in out.iter_mut().zip(p) {
                    *slot = v.as_f64().ok_or("bezier control points must be numbers")?;
                }
                if !(0.0..=1.0).contains(&out[0]) || !(0.0..=1.0).contains(&out[2]) {
                    bail!("bezier x1 and x2 must be within 0..1");
                }
                Ease::Bezier(out)
            }
            "power" => {
                let mode = match obj.get("mode").and_then(Value::as_str).unwrap_or("out") {
                    "in" => PowerMode::In,
                    "out" => PowerMode::Out,
                    "in-out" | "inout" => PowerMode::InOut,
                    other => bail!("power mode '{other}': use in, out or in-out"),
                };
                let n = num("n", 3.0)?;
                if !(n > 0.0) {
                    bail!("power n must be positive");
                }
                Ease::Power { n, mode }
            }
            "exp" => {
                let r = obj.get("r").map(|v| v.as_f64().ok_or("exp.r must be a number")).transpose()?;
                let tau = match obj.get("tau") {
                    None => None,
                    Some(v) => Some(Duration::deserialize(v).map_err(|e| e.to_string())?),
                };
                if r.is_none() && tau.is_none() {
                    bail!("exp ease needs r (kept fraction per frame) or tau (time constant)");
                }
                if let Some(r) = r {
                    if !(r > 0.0 && r < 1.0) {
                        bail!("exp.r must be between 0 and 1");
                    }
                }
                Ease::Exp { r, tau }
            }
            "spring" => {
                let zeta = num("zeta", 0.6)?;
                let hz = num("hz", 3.0)?;
                if !(zeta > 0.0 && hz > 0.0) {
                    bail!("spring zeta and hz must be positive");
                }
                Ease::Spring { zeta, hz }
            }
            "bounce" => Ease::Bounce {
                bounces: num("bounces", 3.0)?.clamp(0.0, 8.0) as u32,
                ratio: num("ratio", 0.43)?.clamp(0.05, 0.95),
                exponent: num("exponent", 2.75)?.clamp(1.0, 6.0),
            },
            "slam" => Ease::Slam { n: num("n", 2.0)?.max(1.0) },
            "linear" => Ease::Linear,
            "hold" => Ease::Hold,
            "" if obj.contains_key("preset") => {
                Ease::parse(obj.get("preset").and_then(Value::as_str).unwrap_or(""))?
            }
            other => bail!("unknown ease type '{other}': use bezier, power, exp, spring, bounce, slam, linear or hold"),
        })
    }
}

/// A compiled curve, all times in seconds.
#[derive(Clone, Debug, PartialEq)]
pub enum Curve {
    Linear,
    Hold,
    Bezier([f64; 4]),
    Power { n: f64, mode: PowerMode },
    /// Progress 1 - exp(-t / tau).
    Exp { tau: f64 },
    /// Damped spring from rest, omega in rad/s.
    Spring { zeta: f64, omega: f64 },
    Bounce { bounces: u32, ratio: f64, exponent: f64 },
    Slam { n: f64 },
}

impl Curve {
    pub fn compile(ease: &Ease, base: &TimeBase) -> Result<Self> {
        Ok(match ease {
            Ease::Preset(p) => Curve::compile(&p.raw(), base)?,
            Ease::Linear => Curve::Linear,
            Ease::Hold => Curve::Hold,
            Ease::Bezier(p) => Curve::Bezier(*p),
            Ease::Power { n, mode } => Curve::Power { n: *n, mode: *mode },
            Ease::Exp { r, tau } => {
                let tau = match (tau, r) {
                    (Some(tau), _) => tau.seconds(base)?,
                    (None, Some(r)) => -base.fps.frame() / r.ln(),
                    (None, None) => bail!("exp ease needs r or tau"),
                };
                if !(tau > 0.0) {
                    bail!("exp tau must be positive");
                }
                Curve::Exp { tau }
            }
            Ease::Spring { zeta, hz } => Curve::Spring { zeta: *zeta, omega: hz * std::f64::consts::TAU },
            Ease::Bounce { bounces, ratio, exponent } => {
                Curve::Bounce { bounces: *bounces, ratio: *ratio, exponent: *exponent }
            }
            Ease::Slam { n } => Curve::Slam { n: *n },
        })
    }

    fn physical(&self) -> bool {
        matches!(self, Curve::Exp { .. } | Curve::Spring { .. })
    }

    /// Uncorrected progress of a physical curve at `t` seconds from its start.
    pub fn raw(&self, t: f64) -> f64 {
        let t = t.max(0.0);
        match *self {
            Curve::Exp { tau } => 1.0 - (-t / tau).exp(),
            Curve::Spring { zeta, omega } => spring(zeta, omega, t),
            _ => self.shape(t.min(1.0)),
        }
    }

    /// Progress for normalised time u in [0, 1] of a shape curve.
    fn shape(&self, u: f64) -> f64 {
        let u = u.clamp(0.0, 1.0);
        match *self {
            Curve::Linear => u,
            Curve::Hold => {
                if u >= 1.0 {
                    1.0
                } else {
                    0.0
                }
            }
            Curve::Bezier(p) => bezier(p, u),
            Curve::Power { n, mode } => match mode {
                PowerMode::In => u.powf(n),
                PowerMode::Out => 1.0 - (1.0 - u).powf(n),
                PowerMode::InOut => {
                    if u < 0.5 {
                        0.5 * (2.0 * u).powf(n)
                    } else {
                        1.0 - 0.5 * (2.0 - 2.0 * u).powf(n)
                    }
                }
            },
            Curve::Slam { n } => u.powf(n),
            Curve::Bounce { bounces, ratio, exponent } => bounce(u, bounces, ratio, exponent),
            Curve::Exp { .. } | Curve::Spring { .. } => u,
        }
    }

    /// Progress at `t` seconds into a move lasting `dur` seconds: 0 at the
    /// start, exactly 1 at `dur` and after.
    pub fn progress(&self, t: f64, dur: f64) -> f64 {
        if t <= 0.0 {
            return 0.0;
        }
        if dur <= 0.0 || t >= dur {
            return 1.0;
        }
        if self.physical() {
            let u = t / dur;
            // The residual at the end is removed with u^3: zero slope at the
            // start, so entrances keep their full initial speed.
            self.raw(t) + (1.0 - self.raw(dur)) * u * u * u
        } else {
            self.shape(t / dur)
        }
    }

    /// Time in seconds after which a physical curve stays within `tolerance`
    /// of 1 (uncorrected). Shape curves report 1.0 (their unit duration).
    pub fn settle_time(&self, tolerance: f64) -> f64 {
        match *self {
            Curve::Exp { tau } => -tau * tolerance.ln(),
            Curve::Spring { .. } => {
                let mut t = 6.0;
                let dt = 1.0 / 2000.0;
                while t > 0.0 && (self.raw(t) - 1.0).abs() <= tolerance {
                    t -= dt;
                }
                t + dt
            }
            _ => 1.0,
        }
    }

    /// Time (seconds into a move of `dur`) of the named sync anchor.
    /// `peak` is the maximum speed, found numerically; `land` is the first
    /// moment progress reaches 98%.
    pub fn anchor_time(&self, anchor: crate::timeref::Anchor, dur: f64) -> f64 {
        use crate::timeref::Anchor;
        match anchor {
            Anchor::Start => 0.0,
            Anchor::End => dur,
            Anchor::Peak => self.peak_time(dur),
            Anchor::Land => self.land_time(dur),
        }
    }

    pub fn peak_time(&self, dur: f64) -> f64 {
        if dur <= 0.0 {
            return 0.0;
        }
        if matches!(self, Curve::Hold) {
            return dur;
        }
        const N: usize = 2048;
        let step = dur / N as f64;
        let mut best = (0.0, f64::MIN);
        for i in 0..N {
            let t0 = i as f64 * step;
            let speed = (self.progress(t0 + step, dur) - self.progress(t0, dur)).abs();
            // Strictly greater keeps the earliest of equal speeds (linear: start).
            if speed > best.1 + 1e-12 {
                best = (t0 + step * 0.5, speed);
            }
        }
        // For curves at full speed at t=0 the first interval wins; report 0.
        if best.0 <= step {
            0.0
        } else {
            best.0
        }
    }

    pub fn land_time(&self, dur: f64) -> f64 {
        if dur <= 0.0 {
            return 0.0;
        }
        const N: usize = 2048;
        for i in 0..=N {
            let t = dur * i as f64 / N as f64;
            if self.progress(t, dur) >= 0.98 {
                return t;
            }
        }
        dur
    }
}

fn spring(zeta: f64, omega: f64, t: f64) -> f64 {
    if zeta < 1.0 {
        let wd = omega * (1.0 - zeta * zeta).sqrt();
        let decay = (-zeta * omega * t).exp();
        1.0 - decay * ((wd * t).cos() + zeta * omega / wd * (wd * t).sin())
    } else if (zeta - 1.0).abs() < 1e-9 {
        1.0 - (-omega * t).exp() * (1.0 + omega * t)
    } else {
        let s = (zeta * zeta - 1.0).sqrt();
        let r1 = -omega * (zeta - s);
        let r2 = -omega * (zeta + s);
        // x(0)=0, x'(0)=0 for the displacement from target.
        1.0 - (r2 * (r1 * t).exp() - r1 * (r2 * t).exp()) / (r2 - r1)
    }
}

fn bezier(p: [f64; 4], x: f64) -> f64 {
    let [x1, y1, x2, y2] = p;
    let coord = |a: f64, b: f64, s: f64| {
        let m = 1.0 - s;
        3.0 * m * m * s * a + 3.0 * m * s * s * b + s * s * s
    };
    let slope = |a: f64, b: f64, s: f64| {
        let m = 1.0 - s;
        3.0 * m * m * a + 6.0 * m * s * (b - a) + 3.0 * s * s * (1.0 - b)
    };
    let mut s = x;
    for _ in 0..8 {
        let err = coord(x1, x2, s) - x;
        let d = slope(x1, x2, s);
        if err.abs() < 1e-9 || d.abs() < 1e-9 {
            break;
        }
        s = (s - err / d).clamp(0.0, 1.0);
    }
    if (coord(x1, x2, s) - x).abs() > 1e-6 {
        let (mut lo, mut hi) = (0.0, 1.0);
        for _ in 0..60 {
            s = 0.5 * (lo + hi);
            if coord(x1, x2, s) < x {
                lo = s;
            } else {
                hi = s;
            }
        }
    }
    coord(y1, y2, s)
}

/// The fall from rest is the descending half of the first arc; each bounce
/// lasts `ratio` × the previous one and rises ratio² as high, the
/// measured restitution in BOUNC. Arc shape: height × (1 − |s|^exponent).
fn bounce(u: f64, bounces: u32, ratio: f64, exponent: f64) -> f64 {
    // Durations: fall = 1 unit (half arc), bounce k (full arc) = 2·ratio^k.
    let mut total = 1.0;
    for k in 1..=bounces {
        total += 2.0 * ratio.powi(k as i32);
    }
    let mut t = u * total;
    if t <= 1.0 {
        return t.powf(exponent);
    }
    t -= 1.0;
    for k in 1..=bounces {
        let half = ratio.powi(k as i32);
        let height = ratio.powi(2 * k as i32);
        if t <= 2.0 * half {
            let s = (t - half) / half;
            return 1.0 - height * (1.0 - s.abs().powf(exponent));
        }
        t -= 2.0 * half;
    }
    1.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn compile(name: &str) -> Curve {
        Curve::compile(&Ease::parse(name).unwrap(), &TimeBase::new(Fps::whole(25))).unwrap()
    }

    /// First frame (at 25 fps) where uncorrected progress reaches `level`.
    fn frame_reaching(curve: &Curve, level: f64, dur: f64) -> u32 {
        (0..200)
            .find(|&n| curve.progress(n as f64 / 25.0, dur) >= level)
            .unwrap()
    }

    fn overshoot(curve: &Curve) -> f64 {
        (0..4000).map(|i| curve.raw(i as f64 / 1000.0)).fold(0.0f64, f64::max) - 1.0
    }

    #[test]
    fn exp_lag_presets_reach_ninety_percent_on_the_measured_frame() {
        // snap: r 0.5 → 90% at 3.3 f; glide r 0.75 → 8.0 f; settle r 0.8 → 10.3 f.
        let snap = compile("snap");
        let glide = compile("glide");
        let settle = compile("settle");
        let n = |c: &Curve| frame_reaching(c, 0.9, 10.0) as i32;
        assert!((n(&snap) - 3).abs() <= 1, "snap {}", n(&snap));
        assert!((6..=9).contains(&n(&glide)), "glide {}", n(&glide));
        assert!((n(&settle) - 11).abs() <= 1, "settle {}", n(&settle));
        // Keep r of the remaining distance per frame.
        let r = 1.0 - glide.raw(1.0 / 25.0);
        assert!((r - 0.75).abs() < 1e-9);
        // Exp lag starts at full speed: the peak anchor is the start.
        assert_eq!(glide.peak_time(0.68), 0.0);
    }

    #[test]
    fn enter_and_gesture_match_their_measurements() {
        let enter = compile("enter");
        // 10 f move: 90% after 4.4 f → frame 5; measured 4–6 f.
        assert!((4..=6).contains(&frame_reaching(&enter, 0.9, 0.4)));
        let gesture = compile("gesture");
        let peak = gesture.peak_time(0.28) / 0.28;
        assert!((peak - 0.5).abs() < 0.01, "gesture peaks mid-move, got {peak}");
    }

    #[test]
    fn springs_reproduce_overshoot_and_settle() {
        let pop = compile("pop");
        let o = overshoot(&pop) * 100.0;
        assert!((o - 9.0).abs() <= 1.0, "pop overshoot {o}%");
        let settle_frames = pop.settle_time(0.02) * 25.0;
        assert!(settle_frames <= 6.0 + 1.0, "pop settles in {settle_frames} f");
        // Peak of scale at ~2.5 f (t_peak = π / ωd).
        let peak_frame = (0..4000)
            .max_by(|a, b| pop.raw(*a as f64 / 1000.0).total_cmp(&pop.raw(*b as f64 / 1000.0)))
            .unwrap() as f64
            / 1000.0
            * 25.0;
        assert!((peak_frame - 2.5).abs() <= 1.0, "pop overshoot peak at {peak_frame} f");
        let land = overshoot(&compile("land")) * 100.0;
        assert!((6.0..=17.0).contains(&land), "land overshoot {land}%");
    }

    #[test]
    fn bounce_restitution_and_slam() {
        let curve = compile("bounce");
        // Apexes (local minima of progress after the fall) are spaced by
        // half of one arc plus half of the next, so their spacing shrinks
        // by the duration ratio 0.43.
        let n = 20000;
        let p: Vec<f64> = (0..=n).map(|i| curve.progress(i as f64 / n as f64, 1.0)).collect();
        let apexes: Vec<f64> = (1..n)
            .filter(|&i| p[i] < p[i - 1] && p[i] <= p[i + 1] && p[i] < 1.0)
            .map(|i| i as f64 / n as f64)
            .collect();
        assert_eq!(apexes.len(), 3, "{apexes:?}");
        let ratio = (apexes[2] - apexes[1]) / (apexes[1] - apexes[0]);
        assert!((ratio - 0.43).abs() < 0.01, "ratio {ratio}");
        // Heights drop to ~18% per bounce (0.43²).
        let h1 = 1.0 - p[(apexes[0] * n as f64) as usize];
        let h2 = 1.0 - p[(apexes[1] * n as f64) as usize];
        assert!((h2 / h1 - 0.185).abs() < 0.01, "height ratio {}", h2 / h1);
        let slam = compile("slam");
        assert!((slam.progress(0.5, 1.0) - 0.25).abs() < 1e-12);
        assert_eq!(slam.peak_time(1.0), slam.peak_time(1.0).max(0.99));
    }

    #[test]
    fn physical_curves_land_exactly_and_bezier_solves() {
        for name in ["snap", "glide", "settle", "pop", "land", "squash"] {
            let c = compile(name);
            assert_eq!(c.progress(0.3, 0.3), 1.0);
            assert_eq!(c.progress(0.0, 0.3), 0.0);
        }
        let ease = Ease::from_json(&serde_json::json!({"type":"bezier","p":[0.42,0,0.58,1]})).unwrap();
        let c = Curve::compile(&ease, &TimeBase::new(Fps::whole(30))).unwrap();
        assert!((c.progress(0.5, 1.0) - 0.5).abs() < 1e-6);
        assert!(Ease::parse("wobble").is_err());
    }
}
