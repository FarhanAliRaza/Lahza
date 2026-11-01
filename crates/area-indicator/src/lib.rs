//! Desktop-only, click-through border for an active area recording.
//!
//! Uses X11 or XWayland override-redirect windows so GNOME Wayland can show
//! the border without installing a Shell extension. The windows are outside
//! the encoded crop and never take keyboard or pointer focus.
#![allow(dead_code)]

use lahza_area as area;
use lahza_input as input;

mod area_indicator;
pub use area_indicator::*;
