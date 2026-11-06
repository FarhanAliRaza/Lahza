//! Motion engine core: the project document, the time model, curves and
//! presets, marker tracks, time references with sync anchors, property
//! evaluation and atomic edit operations.
//!
//! Pure and deterministic: no GPU, no clock, and no I/O beyond loading and
//! saving a project folder (`io`). Everything a renderer needs comes from
//! [`compile::Compiled::frame`], which turns a time into a flat list of
//! resolved layer states.

pub mod catalog;
pub mod check;
pub mod compile;
pub mod components;
pub mod curve;
pub mod document;
pub mod edit;
pub mod io;
pub mod map;
pub mod markers;
pub mod mesh;
pub mod narration;
pub mod noise;
pub mod path;
pub mod property;
pub mod recording;
pub mod space;
pub mod time;
pub mod timeref;
pub mod value;

pub use compile::{Compiled, FrameState, LayerState};
pub use curve::{Curve, Ease};
pub use document::*;
pub use markers::{Marker, MarkerTrack, Markers};
pub use property::{Key, Property};
pub use time::{Duration, Fps};
pub use timeref::{Anchor, TimeRef};
pub use value::{Color, Vec2, Vec3};

/// Errors are plain messages: they are shown to people and agents as-is,
/// so each one names the field or reference at fault.
#[derive(Clone, Debug, PartialEq)]
pub struct Error(pub String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl From<String> for Error {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for Error {
    fn from(value: &str) -> Self {
        Self(value.into())
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[macro_export]
macro_rules! bail {
    ($($arg:tt)*) => { return Err($crate::Error(format!($($arg)*))) };
}
