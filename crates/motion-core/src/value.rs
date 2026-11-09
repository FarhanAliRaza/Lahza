//! Animatable value types.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt::Debug;

/// A value a property can hold and animate.
pub trait Animatable: Clone + Debug + PartialEq + Serialize + for<'de> Deserialize<'de> + Send + Sync + 'static {
    const COMPONENTS: usize;
    fn get(&self, i: usize) -> f64;
    fn from_fn(f: impl FnMut(usize) -> f64) -> Self;

    fn map2(&self, other: &Self, mut f: impl FnMut(f64, f64) -> f64) -> Self {
        Self::from_fn(|i| f(self.get(i), other.get(i)))
    }
    fn lerp(&self, other: &Self, p: f64) -> Self {
        self.map2(other, |a, b| a + (b - a) * p)
    }
    /// Log-space interpolation for zooms; components that are not both
    /// positive fall back to linear.
    fn lerp_log(&self, other: &Self, p: f64) -> Self {
        self.map2(other, |a, b| {
            if a > 0.0 && b > 0.0 {
                (a.ln() + (b.ln() - a.ln()) * p).exp()
            } else {
                a + (b - a) * p
            }
        })
    }
    fn add(&self, other: &Self) -> Self {
        self.map2(other, |a, b| a + b)
    }
    fn scale(&self, k: f64) -> Self {
        Self::from_fn(|i| self.get(i) * k)
    }
    fn splat(v: f64) -> Self {
        Self::from_fn(|_| v)
    }
    fn is_finite(&self) -> bool {
        (0..Self::COMPONENTS).all(|i| self.get(i).is_finite())
    }
}

impl Animatable for f64 {
    const COMPONENTS: usize = 1;
    fn get(&self, _: usize) -> f64 {
        *self
    }
    fn from_fn(mut f: impl FnMut(usize) -> f64) -> Self {
        f(0)
    }
}

/// A 2D point or size. Deserialises from `[x, y]` or a single number
/// (uniform, for scale).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec2(pub [f64; 2]);

impl Vec2 {
    pub const ZERO: Vec2 = Vec2([0.0, 0.0]);
    pub const ONE: Vec2 = Vec2([1.0, 1.0]);
    pub fn new(x: f64, y: f64) -> Self {
        Vec2([x, y])
    }
    pub fn x(self) -> f64 {
        self.0[0]
    }
    pub fn y(self) -> f64 {
        self.0[1]
    }
}

impl Animatable for Vec2 {
    const COMPONENTS: usize = 2;
    fn get(&self, i: usize) -> f64 {
        self.0[i]
    }
    fn from_fn(mut f: impl FnMut(usize) -> f64) -> Self {
        Vec2([f(0), f(1)])
    }
}

impl Serialize for Vec2 {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(s)
    }
}

impl<'de> Deserialize<'de> for Vec2 {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        match serde_json::Value::deserialize(d)? {
            serde_json::Value::Number(n) => {
                let v = n.as_f64().unwrap_or(0.0);
                Ok(Vec2([v, v]))
            }
            serde_json::Value::Array(a) if a.len() == 2 => {
                let x = a[0].as_f64().ok_or_else(|| D::Error::custom("vector components must be numbers"))?;
                let y = a[1].as_f64().ok_or_else(|| D::Error::custom("vector components must be numbers"))?;
                Ok(Vec2([x, y]))
            }
            other => Err(D::Error::custom(format!("expected [x, y] or a number, got {other}"))),
        }
    }
}

/// A 3D point or direction. Deserialises from `[x, y, z]`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec3(pub [f64; 3]);

impl Vec3 {
    pub fn new(x: f64, y: f64, z: f64) -> Self {
        Vec3([x, y, z])
    }
}

impl Animatable for Vec3 {
    const COMPONENTS: usize = 3;
    fn get(&self, i: usize) -> f64 {
        self.0[i]
    }
    fn from_fn(mut f: impl FnMut(usize) -> f64) -> Self {
        Vec3([f(0), f(1), f(2)])
    }
}

impl Serialize for Vec3 {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(s)
    }
}

impl<'de> Deserialize<'de> for Vec3 {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        match serde_json::Value::deserialize(d)? {
            serde_json::Value::Array(a) if a.len() == 3 => {
                let mut v = [0.0; 3];
                for (slot, x) in v.iter_mut().zip(&a) {
                    *slot = x.as_f64().ok_or_else(|| D::Error::custom("vector components must be numbers"))?;
                }
                Ok(Vec3(v))
            }
            other => Err(D::Error::custom(format!("expected [x, y, z], got {other}"))),
        }
    }
}

/// Straight (not premultiplied) sRGB colour with alpha, components 0..1.
/// Deserialises from "#rgb", "#rrggbb", "#rrggbbaa", or [r, g, b(, a)] in 0..1.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Color(pub [f64; 4]);

impl Color {
    pub const WHITE: Color = Color([1.0, 1.0, 1.0, 1.0]);
    pub const BLACK: Color = Color([0.0, 0.0, 0.0, 1.0]);
    pub const TRANSPARENT: Color = Color([0.0, 0.0, 0.0, 0.0]);

    pub fn parse(text: &str) -> Result<Self, String> {
        let hex = text.trim().strip_prefix('#').ok_or_else(|| format!("colour '{text}' must start with #"))?;
        let digit = |i: usize, len: usize| -> Result<f64, String> {
            let s = &hex[i..i + len];
            let v = u8::from_str_radix(&if len == 1 { s.repeat(2) } else { s.to_string() }, 16)
                .map_err(|_| format!("bad colour '{text}'"))?;
            Ok(v as f64 / 255.0)
        };
        match hex.len() {
            3 => Ok(Color([digit(0, 1)?, digit(1, 1)?, digit(2, 1)?, 1.0])),
            6 => Ok(Color([digit(0, 2)?, digit(2, 2)?, digit(4, 2)?, 1.0])),
            8 => Ok(Color([digit(0, 2)?, digit(2, 2)?, digit(4, 2)?, digit(6, 2)?])),
            _ => Err(format!("colour '{text}' must be #rgb, #rrggbb or #rrggbbaa")),
        }
    }

    pub fn to_hex(self) -> String {
        let c = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        let [r, g, b, a] = self.0;
        if c(a) == 255 {
            format!("#{:02x}{:02x}{:02x}", c(r), c(g), c(b))
        } else {
            format!("#{:02x}{:02x}{:02x}{:02x}", c(r), c(g), c(b), c(a))
        }
    }

    /// Relative luminance (WCAG).
    pub fn luminance(self) -> f64 {
        let lin = |c: f64| if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) };
        0.2126 * lin(self.0[0]) + 0.7152 * lin(self.0[1]) + 0.0722 * lin(self.0[2])
    }
}

impl Animatable for Color {
    const COMPONENTS: usize = 4;
    fn get(&self, i: usize) -> f64 {
        self.0[i]
    }
    fn from_fn(mut f: impl FnMut(usize) -> f64) -> Self {
        Color([f(0), f(1), f(2), f(3)])
    }
}

impl Serialize for Color {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Color {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        match serde_json::Value::deserialize(d)? {
            serde_json::Value::String(s) => Color::parse(&s).map_err(D::Error::custom),
            serde_json::Value::Array(a) if a.len() == 3 || a.len() == 4 => {
                let mut c = [1.0; 4];
                for (slot, v) in c.iter_mut().zip(&a) {
                    *slot = v.as_f64().ok_or_else(|| D::Error::custom("colour components must be numbers"))?;
                }
                Ok(Color(c))
            }
            other => Err(D::Error::custom(format!("expected a colour like \"#ff3355\", got {other}"))),
        }
    }
}

/// Four numbers (a shader layer's `float4` parameter). Deserialises from
/// `[x, y, z, w]` or a colour ("#rrggbb", "#rrggbbaa"); serialises as the
/// array, so values outside 0..1 keep.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec4(pub [f64; 4]);

impl Animatable for Vec4 {
    const COMPONENTS: usize = 4;
    fn get(&self, i: usize) -> f64 {
        self.0[i]
    }
    fn from_fn(mut f: impl FnMut(usize) -> f64) -> Self {
        Vec4([f(0), f(1), f(2), f(3)])
    }
}

impl Serialize for Vec4 {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(s)
    }
}

impl<'de> Deserialize<'de> for Vec4 {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        match serde_json::Value::deserialize(d)? {
            serde_json::Value::String(s) => Color::parse(&s).map(|c| Vec4(c.0)).map_err(D::Error::custom),
            serde_json::Value::Array(a) if a.len() == 4 => {
                let mut v = [0.0; 4];
                for (slot, x) in v.iter_mut().zip(&a) {
                    *slot = x.as_f64().ok_or_else(|| D::Error::custom("vector components must be numbers"))?;
                }
                Ok(Vec4(v))
            }
            other => Err(D::Error::custom(format!("expected [x, y, z, w] or a colour, got {other}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colours_and_vectors_parse() {
        let c: Color = serde_json::from_str("\"#ff000080\"").unwrap();
        assert_eq!(c.0[0], 1.0);
        assert!((c.0[3] - 128.0 / 255.0).abs() < 1e-12);
        assert_eq!(c.to_hex(), "#ff000080");
        let v: Vec2 = serde_json::from_str("1.5").unwrap();
        assert_eq!(v, Vec2([1.5, 1.5]));
        assert_eq!(Vec2::new(1.0, 10.0).lerp_log(&Vec2::new(10.0, 1.0), 0.5).0[0], 10f64.sqrt());
    }
}
