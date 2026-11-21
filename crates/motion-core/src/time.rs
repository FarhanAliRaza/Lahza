//! Clocks: rational frame rates, durations with units, and stepped time.

use crate::{bail, Result};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// A rational frame rate. Serialises as a number (`30`, `25`) or a ratio
/// string (`"30000/1001"`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Fps {
    pub num: u32,
    pub den: u32,
}

impl Fps {
    pub const fn new(num: u32, den: u32) -> Self {
        Self { num, den }
    }
    pub const fn whole(fps: u32) -> Self {
        Self { num: fps, den: 1 }
    }
    pub fn as_f64(self) -> f64 {
        self.num as f64 / self.den.max(1) as f64
    }
    /// Seconds per frame.
    pub fn frame(self) -> f64 {
        self.den.max(1) as f64 / self.num.max(1) as f64
    }
    /// Time of frame `n`.
    pub fn time_of(self, n: i64) -> f64 {
        n as f64 * self.den as f64 / self.num as f64
    }
    /// Frame displayed at time `t` (frames show [n/fps, (n+1)/fps)).
    pub fn frame_at(self, t: f64) -> i64 {
        // A tiny epsilon keeps exact frame times from flooring one frame early.
        (t * self.as_f64() + 1e-9).floor() as i64
    }
    /// Nearest frame to `t`.
    pub fn nearest_frame(self, t: f64) -> i64 {
        (t * self.as_f64()).round() as i64
    }
    pub fn frame_count(self, duration: f64) -> u64 {
        (duration * self.as_f64() - 1e-9).ceil().max(0.0) as u64
    }
    pub fn parse(text: &str) -> Result<Self> {
        let text = text.trim();
        if let Some((n, d)) = text.split_once('/') {
            let num = n.trim().parse::<u32>().map_err(|_| format!("bad fps '{text}'"))?;
            let den = d.trim().parse::<u32>().map_err(|_| format!("bad fps '{text}'"))?;
            if num == 0 || den == 0 {
                bail!("fps must be positive: '{text}'");
            }
            return Ok(Self { num, den });
        }
        let value: f64 = text.parse().map_err(|_| format!("bad fps '{text}'"))?;
        Self::from_f64(value)
    }
    pub fn from_f64(value: f64) -> Result<Self> {
        if !(value.is_finite() && value > 0.0 && value <= 1000.0) {
            bail!("fps must be between 0 and 1000, got {value}");
        }
        if (value - value.round()).abs() < 1e-9 {
            return Ok(Self::whole(value.round() as u32));
        }
        // NTSC rates.
        for base in [24u32, 30, 60, 120] {
            let ntsc = base as f64 * 1000.0 / 1001.0;
            if (value - ntsc).abs() < 0.01 {
                return Ok(Self::new(base * 1000, 1001));
            }
        }
        Ok(Self::new((value * 1000.0).round() as u32, 1000))
    }
}

impl Default for Fps {
    fn default() -> Self {
        Self::whole(30)
    }
}

impl Serialize for Fps {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        if self.den == 1 {
            s.serialize_u32(self.num)
        } else {
            s.serialize_str(&format!("{}/{}", self.num, self.den))
        }
    }
}

impl<'de> Deserialize<'de> for Fps {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(d)?;
        match value {
            serde_json::Value::Number(n) => {
                Fps::from_f64(n.as_f64().unwrap_or(0.0)).map_err(serde::de::Error::custom)
            }
            serde_json::Value::String(s) => Fps::parse(&s).map_err(serde::de::Error::custom),
            other => Err(serde::de::Error::custom(format!("fps must be a number, got {other}"))),
        }
    }
}

/// A span of time. A bare number is seconds; strings carry a unit:
/// `"8f"` frames, `"0.25s"` seconds, `"120ms"`, `"1b"` beats (uses the
/// project's beat track), `"0.5bar"` bars.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Duration {
    Seconds(f64),
    Frames(f64),
    Beats(f64),
    Bars(f64),
}

impl Default for Duration {
    fn default() -> Self {
        Duration::Seconds(0.0)
    }
}

/// What a duration needs to become seconds.
#[derive(Clone, Copy, Debug)]
pub struct TimeBase {
    pub fps: Fps,
    /// Seconds per beat, from the beat track (median interval), if any.
    pub beat: Option<f64>,
    /// Seconds per bar, from the bar track, if any.
    pub bar: Option<f64>,
}

impl TimeBase {
    pub fn new(fps: Fps) -> Self {
        Self { fps, beat: None, bar: None }
    }
}

impl Duration {
    pub fn parse(text: &str) -> Result<Self> {
        let text = text.trim();
        let split = text
            .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+'))
            .unwrap_or(text.len());
        let (number, unit) = text.split_at(split);
        let value: f64 = number
            .trim()
            .parse()
            .map_err(|_| format!("bad duration '{text}': expected a number with an optional unit (f, s, ms, b, bar)"))?;
        if !value.is_finite() {
            bail!("bad duration '{text}'");
        }
        Ok(match unit.trim() {
            "" | "s" => Duration::Seconds(value),
            "ms" => Duration::Seconds(value / 1000.0),
            "f" => Duration::Frames(value),
            "b" | "beat" | "beats" => Duration::Beats(value),
            "bar" | "bars" => Duration::Bars(value),
            other => bail!("bad duration unit '{other}' in '{text}': use f, s, ms, b or bar"),
        })
    }

    pub fn seconds(self, base: &TimeBase) -> Result<f64> {
        Ok(match self {
            Duration::Seconds(s) => s,
            Duration::Frames(f) => f * base.fps.frame(),
            Duration::Beats(b) => match base.beat {
                Some(beat) => b * beat,
                None => bail!("duration in beats needs a beat track: run audio_analyze first"),
            },
            Duration::Bars(b) => match base.bar.or(base.beat.map(|beat| beat * 4.0)) {
                Some(bar) => b * bar,
                None => bail!("duration in bars needs a beat track: run audio_analyze first"),
            },
        })
    }

    /// Frames, for fields whose bare numbers mean frames (`lead`).
    pub fn parse_frames(value: &serde_json::Value) -> Result<Self> {
        match value {
            serde_json::Value::Number(n) => Ok(Duration::Frames(n.as_f64().unwrap_or(0.0))),
            serde_json::Value::String(s) => Self::parse(s),
            other => bail!("expected frames or a duration string, got {other}"),
        }
    }
}

impl Serialize for Duration {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        match *self {
            Duration::Seconds(v) => s.serialize_f64(v),
            Duration::Frames(v) => s.serialize_str(&format!("{}f", trim(v))),
            Duration::Beats(v) => s.serialize_str(&format!("{}b", trim(v))),
            Duration::Bars(v) => s.serialize_str(&format!("{}bar", trim(v))),
        }
    }
}

fn trim(v: f64) -> String {
    let s = format!("{v:.6}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

impl<'de> Deserialize<'de> for Duration {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        match serde_json::Value::deserialize(d)? {
            serde_json::Value::Number(n) => Ok(Duration::Seconds(n.as_f64().unwrap_or(0.0))),
            serde_json::Value::String(s) => Duration::parse(&s).map_err(serde::de::Error::custom),
            other => Err(serde::de::Error::custom(format!("expected a duration, got {other}"))),
        }
    }
}

/// A lead offset: bare numbers are frames (design section 4.3).
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Lead(pub Duration);

impl Serialize for Lead {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        match self.0 {
            Duration::Frames(f) => s.serialize_f64(f),
            other => other.serialize(s),
        }
    }
}

impl<'de> Deserialize<'de> for Lead {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(d)?;
        Duration::parse_frames(&value).map(Lead).map_err(serde::de::Error::custom)
    }
}

/// A stepped clock (stop-motion): animation is sampled at `fps` and held.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Step {
    pub fps: f64,
    /// Phase offset in seconds (where the first held frame starts).
    #[serde(default)]
    pub phase: f64,
}

impl Step {
    /// `t_q = floor((t - t0) * step_fps) / step_fps + t0`, with t0 = phase.
    /// Every time inside one held step maps to the identical value, so
    /// held frames are bit-identical and the renderer can reuse them.
    pub fn quantize(self, t: f64) -> f64 {
        if !(self.fps > 0.0) {
            return t;
        }
        ((t - self.phase) * self.fps + 1e-9).floor() / self.fps + self.phase
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stepped_holds_match_the_collage_study() {
        // At 30 fps, 8 fps steps hold 4,4,4,3 frames; 12 fps holds 3,2,3,2.
        let fps = Fps::whole(30);
        let holds = |step: f64| {
            let step = Step { fps: step, phase: 0.0 };
            let mut runs = Vec::new();
            let mut last = step.quantize(0.0);
            let mut run = 0;
            for n in 0..30 {
                let q = step.quantize(fps.time_of(n));
                if q != last {
                    runs.push(run);
                    run = 0;
                    last = q;
                }
                run += 1;
            }
            runs
        };
        assert_eq!(&holds(8.0)[..4], &[4, 4, 4, 3]);
        assert_eq!(&holds(12.0)[..4], &[3, 2, 3, 2]);
    }

    #[test]
    fn durations_parse_units() {
        let base = TimeBase { fps: Fps::whole(25), beat: Some(0.5), bar: None };
        assert_eq!(Duration::parse("8f").unwrap().seconds(&base).unwrap(), 0.32);
        assert_eq!(Duration::parse("250ms").unwrap().seconds(&base).unwrap(), 0.25);
        assert_eq!(Duration::parse("2b").unwrap().seconds(&base).unwrap(), 1.0);
        assert_eq!(Duration::parse("1bar").unwrap().seconds(&base).unwrap(), 2.0);
        assert!(Duration::parse("3 parsecs").is_err());
        assert_eq!(Fps::parse("30000/1001").unwrap(), Fps::new(30000, 1001));
        assert_eq!(Fps::from_f64(29.97).unwrap(), Fps::new(30000, 1001));
    }
}
