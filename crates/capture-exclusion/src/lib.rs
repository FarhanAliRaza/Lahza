//! Recording-scoped requests for the GNOME helper to omit floating windows
//! from offscreen capture paints while preserving their desktop paint.
#![allow(dead_code)]

use lahza_input as input;

mod capture_exclusion;
pub use capture_exclusion::*;
