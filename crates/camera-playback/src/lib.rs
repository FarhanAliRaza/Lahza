//! Persistent source-time webcam decoding. The screen/audio transport owns
//! the clock; this decoder follows timestamps rather than running a second clock.
#![allow(dead_code)]

use lahza_video::video;
#[cfg(test)]
use lahza_motion_blur as motion_blur;

mod camera_playback;
pub use camera_playback::*;
