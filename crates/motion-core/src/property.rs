//! Animated properties: keys as *moves* with sync anchors, additive tracks
//! and an optional stepped clock.
//!
//! A key says "arrive at `v`". Without `dur` it is a classic keyframe: the
//! move runs from the previous key's end to `at`. With `dur` (or a
//! non-`end` anchor) it is a move of that length placed so that its
//! `anchor` moment (start, peak speed, landing or end) falls on `at + lead`.
//! A move that starts before the previous one finished takes over from the
//! value at that moment, so the result is always continuous.

use crate::curve::{Curve, Ease};
use crate::markers::Markers;
use crate::time::{Duration, Lead, Step, TimeBase};
use crate::timeref::{Anchor, ElementTimes, TimeRef};
use crate::value::Animatable;
use crate::{bail, Result};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, bound = "T: Animatable")]
pub struct Key<T> {
    #[serde(alias = "t")]
    pub at: TimeRef,
    #[serde(alias = "value", alias = "to")]
    pub v: T,
    /// Start value of the move; on the first key it makes a one-key entrance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<T>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ease: Option<Ease>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<Anchor>,
    /// Offset of the anchor from `at`; bare numbers are frames.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lead: Option<Lead>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dur: Option<Duration>,
    /// "log" interpolates in log space (zooms); the `zoom` preset implies it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space: Option<Space>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Space {
    Linear,
    Log,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Per {
    #[default]
    Second,
    Frame,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Add,
    Mul,
}

/// Additive tracks layered on top of the keyed value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields, bound = "T: Animatable")]
pub enum Additive<T> {
    /// Constant drift: `add` grows by rate per unit time; `mul` compounds
    /// (`0.01` per frame = 1% scale growth per frame).
    Drift {
        rate: T,
        #[serde(default)]
        per: Per,
        #[serde(default)]
        mode: Mode,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from: Option<TimeRef>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        until: Option<TimeRef>,
    },
    /// Smooth seeded noise.
    Wiggle {
        hz: f64,
        amp: T,
        #[serde(default)]
        seed: u32,
        #[serde(default = "one")]
        octaves: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from: Option<TimeRef>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        until: Option<TimeRef>,
    },
    Sine {
        hz: f64,
        amp: T,
        /// Phase in turns (0..1).
        #[serde(default)]
        phase: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        from: Option<TimeRef>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        until: Option<TimeRef>,
    },
}

fn one() -> u32 {
    1
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, bound = "T: Animatable")]
pub struct Animated<T> {
    #[serde(default)]
    pub keys: Vec<Key<T>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub add: Vec<Additive<T>>,
    /// Stepped clock for this property only (layers have their own).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_fps: Option<f64>,
    /// Value before any key when the property has no keys (only additive).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<T>,
}

/// A property: a bare value, or `{"keys": [...], "add": [...]}`.
#[derive(Clone, Debug, PartialEq)]
pub enum Property<T> {
    Constant(T),
    Animated(Animated<T>),
}

impl<T: Animatable> Property<T> {
    pub fn constant(v: T) -> Self {
        Property::Constant(v)
    }
    pub fn is_animated(&self) -> bool {
        matches!(self, Property::Animated(_))
    }
    /// The value before any animation.
    pub fn initial(&self) -> T {
        match self {
            Property::Constant(v) => v.clone(),
            Property::Animated(a) => a
                .keys
                .first()
                .map(|k| k.from.clone().unwrap_or_else(|| k.v.clone()))
                .or_else(|| a.base.clone())
                .unwrap_or_else(|| T::splat(0.0)),
        }
    }
}

impl<T: Animatable> Serialize for Property<T> {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        match self {
            Property::Constant(v) => v.serialize(s),
            Property::Animated(a) => a.serialize(s),
        }
    }
}

impl<'de, T: Animatable> Deserialize<'de> for Property<T> {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        use serde::de::Error;
        let value = serde_json::Value::deserialize(d)?;
        if let serde_json::Value::Object(o) = &value {
            if o.contains_key("keys") || o.contains_key("add") || o.contains_key("base") {
                return serde_json::from_value(value).map(Property::Animated).map_err(D::Error::custom);
            }
        }
        serde_json::from_value(value).map(Property::Constant).map_err(D::Error::custom)
    }
}

/// A requested sync point kept for `check`.
#[derive(Clone, Debug, PartialEq)]
pub struct SyncPoint {
    pub reference: String,
    pub anchor: Anchor,
    /// `at + lead`, seconds.
    pub requested: f64,
    /// Where the move starts and how long it lasts.
    pub start: f64,
    pub dur: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Segment<T> {
    pub start: f64,
    pub dur: f64,
    pub from: T,
    pub to: T,
    pub curve: Curve,
    pub log: bool,
    pub sync: Option<SyncPoint>,
}

impl<T: Animatable> Segment<T> {
    fn value(&self, t: f64) -> T {
        let p = self.curve.progress(t - self.start, self.dur);
        if self.log {
            self.from.lerp_log(&self.to, p)
        } else {
            self.from.lerp(&self.to, p)
        }
    }
    pub fn end(&self) -> f64 {
        self.start + self.dur
    }
}

#[derive(Clone, Debug, PartialEq)]
enum CAdd<T> {
    Drift { rate: T, mul: bool, from: f64, until: f64 },
    Wiggle { hz: f64, amp: T, seed: u32, octaves: u32, from: f64, until: f64 },
    Sine { hz: f64, amp: T, phase: f64, from: f64, until: f64 },
}

/// A compiled property: every time resolved, evaluation is pure arithmetic.
#[derive(Clone, Debug, PartialEq)]
pub struct Prop<T> {
    pub base: T,
    pub segments: Vec<Segment<T>>,
    add: Vec<CAdd<T>>,
    pub step: Option<Step>,
}

impl<T: Animatable> Prop<T> {
    pub fn constant(v: T) -> Self {
        Self { base: v, segments: Vec::new(), add: Vec::new(), step: None }
    }

    pub fn is_constant(&self) -> bool {
        self.segments.is_empty() && self.add.is_empty()
    }

    /// Value at timeline time `t` (already quantised by the layer clock).
    pub fn at(&self, t: f64) -> T {
        let t = match self.step {
            Some(step) => step.quantize(t),
            None => t,
        };
        let mut v = self.keyed(t);
        for add in &self.add {
            v = apply_add(add, v, t);
        }
        v
    }

    fn keyed(&self, t: f64) -> T {
        let idx = self.segments.partition_point(|s| s.start <= t);
        if idx == 0 {
            return self.base.clone();
        }
        self.segments[idx - 1].value(t)
    }

    /// Time span with keyed motion.
    pub fn motion_span(&self) -> Option<(f64, f64)> {
        let start = self.segments.iter().map(|s| s.start).fold(f64::INFINITY, f64::min);
        let end = self.segments.iter().map(|s| s.end()).fold(f64::NEG_INFINITY, f64::max);
        (start.is_finite() && end.is_finite()).then_some((start, end))
    }

    pub fn has_additive(&self) -> bool {
        !self.add.is_empty()
    }
}

fn apply_add<T: Animatable>(add: &CAdd<T>, v: T, t: f64) -> T {
    match add {
        CAdd::Drift { rate, mul, from, until } => {
            let elapsed = (t.min(*until) - from).max(0.0);
            if *mul {
                v.map2(rate, |x, r| x * (1.0 + r).max(1e-6).powf(elapsed))
            } else {
                v.add(&rate.scale(elapsed))
            }
        }
        CAdd::Wiggle { hz, amp, seed, octaves, from, until } => {
            if t < *from || t > *until {
                return v;
            }
            let x = (t - from) * hz;
            v.add(&T::from_fn(|i| {
                let mut sum = 0.0;
                let mut norm = 0.0;
                for o in 0..*octaves {
                    let scale = 2f64.powi(o as i32);
                    sum += noise(x * scale, seed.wrapping_add(i as u32 * 7919).wrapping_add(o * 104729)) / scale;
                    norm += 1.0 / scale;
                }
                amp.get(i) * sum / norm
            }))
        }
        CAdd::Sine { hz, amp, phase, from, until } => {
            if t < *from || t > *until {
                return v;
            }
            let s = (std::f64::consts::TAU * ((t - from) * hz + phase)).sin();
            v.add(&amp.scale(s))
        }
    }
}

/// Smooth seeded value noise in [-1, 1], zero-mean, C1-continuous.
fn noise(x: f64, seed: u32) -> f64 {
    let i = x.floor();
    let f = x - i;
    let h = |n: i64| {
        let mut z = (n as u64).wrapping_add((seed as u64) << 32).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        (z >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
    };
    let a = h(i as i64);
    let b = h(i as i64 + 1);
    let s = f * f * (3.0 - 2.0 * f);
    a + (b - a) * s
}

/// What compiling a property needs.
pub struct Ctx<'a> {
    pub markers: &'a Markers,
    pub base: TimeBase,
    pub elements: &'a mut dyn ElementTimes,
    /// Default time for additive tracks with no `from` (the layer start).
    pub origin: f64,
}

impl Ctx<'_> {
    pub fn time(&mut self, r: &TimeRef) -> Result<f64> {
        r.resolve(self.markers, &self.base, self.elements)
    }
}

pub fn compile<T: Animatable>(prop: &Property<T>, ctx: &mut Ctx) -> Result<Prop<T>> {
    let animated = match prop {
        Property::Constant(v) => {
            if !v.is_finite() {
                bail!("value is not finite");
            }
            return Ok(Prop::constant(v.clone()));
        }
        Property::Animated(a) => a,
    };
    let mut segments = Vec::new();
    let mut base = animated.base.clone();
    let mut prev_end: Option<f64> = None;
    let mut explicit_from = Vec::new();
    for (i, key) in animated.keys.iter().enumerate() {
        let ease = key.ease.clone().unwrap_or_default();
        let curve = Curve::compile(&ease, &ctx.base).map_err(|e| format!("key {}: {e}", i + 1))?;
        let lead = match key.lead {
            Some(Lead(d)) => d.seconds(&ctx.base)?,
            None => 0.0,
        };
        let at = ctx.time(&key.at).map_err(|e| format!("key {}: {e}", i + 1))? + lead;
        let anchored = key.dur.is_some() || key.anchor.is_some_and(|a| a != Anchor::End);
        let first = prev_end.is_none();
        if first && key.from.is_none() && !anchored {
            // A plain first key sets the starting value.
            base = Some(key.v.clone());
            prev_end = Some(at);
            continue;
        }
        let anchor = key.anchor.unwrap_or_default();
        let (start, dur) = if anchored || first {
            let dur = match key.dur {
                Some(d) => d.seconds(&ctx.base)?,
                None => ease.natural_duration(ctx.base.fps),
            };
            if !(dur >= 0.0) {
                bail!("key {}: duration must not be negative", i + 1);
            }
            (at - curve.anchor_time(anchor, dur), dur)
        } else {
            let start = prev_end.unwrap_or(at);
            (start, (at - start).max(0.0))
        };
        let sync = (key.at.is_marker() || matches!(key.at, TimeRef::Expr(_)) || key.anchor.is_some()).then(|| SyncPoint {
            reference: key.at.text(),
            anchor,
            requested: at,
            start,
            dur,
        });
        if !key.v.is_finite() {
            bail!("key {}: value is not finite", i + 1);
        }
        if base.is_none() {
            base = key.from.clone().or_else(|| Some(key.v.clone()));
        }
        explicit_from.push(key.from.clone());
        segments.push(Segment {
            start,
            dur,
            from: key.v.clone(), // replaced below
            to: key.v.clone(),
            curve,
            log: key.space == Some(Space::Log) || key.space.is_none() && ease.is_log(),
            sync,
        });
        prev_end = Some(start + dur);
    }
    // Order by start; each move starts from the value at its start.
    let mut order: Vec<usize> = (0..segments.len()).collect();
    order.sort_by(|a, b| segments[*a].start.total_cmp(&segments[*b].start));
    let segments: Vec<_> = order.iter().map(|&i| segments[i].clone()).collect();
    let explicit_from: Vec<_> = order.iter().map(|&i| explicit_from[i].clone()).collect();
    let base = base.unwrap_or_else(|| T::splat(0.0));
    let mut prop = Prop { base: base.clone(), segments: Vec::new(), add: Vec::new(), step: None };
    for (mut seg, from) in segments.into_iter().zip(explicit_from) {
        seg.from = match from {
            Some(f) => f,
            None => prop.keyed(seg.start),
        };
        prop.segments.push(seg);
    }
    if prop.segments.first().is_some() {
        // Before the first move the value is that move's start value.
        prop.base = prop.segments[0].from.clone();
    }
    let origin = prop.motion_span().map(|s| s.0).unwrap_or(ctx.origin);
    for add in &animated.add {
        let window = |ctx: &mut Ctx, from: &Option<TimeRef>, until: &Option<TimeRef>| -> Result<(f64, f64)> {
            let f = match from {
                Some(r) => ctx.time(r)?,
                None => origin,
            };
            let u = match until {
                Some(r) => ctx.time(r)?,
                None => f64::INFINITY,
            };
            Ok((f, u))
        };
        prop.add.push(match add {
            Additive::Drift { rate, per, mode, from, until } => {
                let (from, until) = window(ctx, from, until)?;
                let k = match per {
                    Per::Second => 1.0,
                    Per::Frame => ctx.base.fps.as_f64(),
                };
                // Rates are stored per second.
                let rate = match mode {
                    Mode::Add => rate.scale(k),
                    Mode::Mul => T::from_fn(|i| (1.0 + rate.get(i)).max(1e-6).powf(k) - 1.0),
                };
                CAdd::Drift { rate, mul: *mode == Mode::Mul, from, until }
            }
            Additive::Wiggle { hz, amp, seed, octaves, from, until } => {
                let (from, until) = window(ctx, from, until)?;
                CAdd::Wiggle { hz: *hz, amp: amp.clone(), seed: *seed, octaves: (*octaves).clamp(1, 6), from, until }
            }
            Additive::Sine { hz, amp, phase, from, until } => {
                let (from, until) = window(ctx, from, until)?;
                CAdd::Sine { hz: *hz, amp: amp.clone(), phase: *phase, from, until }
            }
        });
    }
    prop.step = animated.step_fps.filter(|f| *f > 0.0).map(|fps| Step { fps, phase: 0.0 });
    Ok(prop)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markers::MarkerTrack;
    use crate::time::Fps;
    use crate::timeref::NoElements;

    fn compiled<T: Animatable>(json: &str, markers: &Markers) -> Prop<T> {
        let prop: Property<T> = serde_json::from_str(json).unwrap();
        let mut elements = NoElements;
        let mut ctx = Ctx { markers, base: TimeBase::new(Fps::whole(25)), elements: &mut elements, origin: 0.0 };
        compile(&prop, &mut ctx).unwrap()
    }

    fn beats() -> Markers {
        let mut m = Markers::default();
        m.insert(MarkerTrack::from_times("beat", (0..64).map(|i| 1.093 + i as f64 * 60.0 / 90.0)));
        m
    }

    #[test]
    fn classic_keys_interpolate() {
        let p: Prop<f64> = compiled(r#"{"keys":[{"at":1,"v":0},{"at":2,"v":10}]}"#, &Markers::default());
        assert_eq!(p.at(0.0), 0.0);
        assert_eq!(p.at(1.5), 5.0);
        assert_eq!(p.at(3.0), 10.0);
    }

    /// Time of the largest per-sample change, sampled finely.
    fn measured_peak(p: &Prop<f64>, from: f64, to: f64) -> f64 {
        let n = 20000;
        let dt = (to - from) / n as f64;
        (0..n)
            .map(|i| from + i as f64 * dt)
            .max_by(|a, b| (p.at(a + dt) - p.at(*a)).abs().total_cmp(&(p.at(b + dt) - p.at(*b)).abs()))
            .unwrap()
            + dt / 2.0
    }

    #[test]
    fn anchored_peak_lands_on_its_marker_after_lead() {
        // gesture move whose peak speed lands one frame before beat 5.
        let p: Prop<f64> = compiled(
            r#"{"keys":[{"at":0,"v":0},{"at":"beat:5","v":100,"ease":"gesture","dur":"8f","anchor":"peak","lead":-1}]}"#,
            &beats(),
        );
        let target = 1.093 + 4.0 * 60.0 / 90.0 - 1.0 / 25.0;
        let peak = measured_peak(&p, target - 0.5, target + 0.5);
        assert!(((peak - target) * 25.0).abs() <= 0.5, "peak {peak} target {target}");
        let sync = p.segments[0].sync.as_ref().unwrap();
        assert!((sync.requested - target).abs() < 1e-9);
    }

    #[test]
    fn one_key_entrance_with_from() {
        let p: Prop<f64> = compiled(r#"{"keys":[{"at":"beat:1","from":0,"v":1,"ease":"pop","dur":"6f","anchor":"start"}]}"#, &beats());
        assert_eq!(p.at(0.0), 0.0);
        assert_eq!(p.at(1.093), 0.0);
        assert!(p.at(1.093 + 0.1) > 1.0, "pop overshoots");
        assert_eq!(p.at(5.0), 1.0);
    }

    #[test]
    fn overlapping_moves_stay_continuous() {
        let p: Prop<f64> = compiled(
            r#"{"keys":[{"at":0,"v":0},{"at":1,"v":100,"ease":"glide","dur":1,"anchor":"start"},{"at":1.3,"v":-50,"ease":"snap","dur":0.5,"anchor":"start"}]}"#,
            &Markers::default(),
        );
        let before = p.at(1.3 - 1e-9);
        let after = p.at(1.3 + 1e-9);
        assert!((before - after).abs() < 1e-3, "{before} {after}");
        assert_eq!(p.at(2.0), -50.0);
    }

    #[test]
    fn additive_drift_and_wiggle_are_deterministic() {
        let p: Prop<f64> = compiled(
            r#"{"keys":[{"at":0,"from":0.5,"v":1,"ease":"enter","dur":"10f","anchor":"start"}],
               "add":[{"type":"drift","rate":0.01,"per":"frame","mode":"mul"},{"type":"wiggle","hz":2,"amp":0.0,"seed":3}]}"#,
            &Markers::default(),
        );
        // 1% per frame compounding over 25 frames after t=0.
        let expected = 1.0 * 1.01f64.powi(25);
        assert!((p.at(1.0) - expected).abs() < 1e-9, "{} {expected}", p.at(1.0));
        let w: Prop<f64> = compiled(r#"{"base":0,"add":[{"type":"wiggle","hz":3,"amp":10,"seed":7}]}"#, &Markers::default());
        assert_eq!(w.at(0.37), w.at(0.37));
        assert!((0..100).any(|i| w.at(i as f64 * 0.05).abs() > 1.0));
        assert!((0..100).all(|i| w.at(i as f64 * 0.05).abs() <= 10.0));
    }

    #[test]
    fn stepped_property_holds() {
        let p: Prop<f64> = compiled(r#"{"keys":[{"at":0,"v":0},{"at":1,"v":12}],"step_fps":12}"#, &Markers::default());
        assert_eq!(p.at(0.01), p.at(0.08));
        assert_ne!(p.at(0.08), p.at(0.09));
    }

    #[test]
    fn log_space_zoom() {
        let p: Prop<f64> = compiled(r#"{"keys":[{"at":0,"v":1},{"at":1,"v":100,"ease":"zoom"}]}"#, &Markers::default());
        assert!((p.at(0.5) - 10.0).abs() < 1e-9);
    }
}
