//! Screen, webcam, and audio recording through the desktop portal and GStreamer.
#![allow(dead_code)]

use lahza_area as area;
use lahza_area_indicator as area_indicator;
use lahza_camera_preview as camera_preview;
use lahza_capture_exclusion as capture_exclusion;
use lahza_input as input;
use lahza_project as project;
use lahza_stream_geometry as stream_geometry;
#[cfg(test)]
use lahza_scene as scene;
#[cfg(test)]
use lahza_video::video;
#[cfg(test)]
use lahza_viewport as viewport;

mod native;
pub use native::*;
