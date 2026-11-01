//! Live webcam preview frames for the Studio canvas.
//!
//! A V4L2 device can only be opened by one pipeline at a time, so the preview
//! has two producers that publish into one [`CameraFrames`] slot: a standalone
//! pipeline while the camera toggle is on and nothing is recording, and a
//! `tee` branch of the recording pipeline while a capture is running.
#![allow(dead_code)]

mod camera_preview;
pub use camera_preview::*;
