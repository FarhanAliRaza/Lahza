//! Select a rectangle from the actual portal stream, before encoding starts.
#![allow(dead_code)]

use lahza_camera_preview as camera_preview;
use lahza_capture_exclusion as capture_exclusion;
#[cfg(test)]
use lahza_video::video;

mod area;
pub use area::*;
