//! Shared, cut-aware shutter sampling for preview and export.
//! Planning only evaluates immutable timelines; decoding and drawing stay off UI.
#![allow(dead_code)]

use lahza_clips as clips;
use lahza_pointer_timeline as pointer_timeline;
use lahza_primary_frames as primary_frames;
use lahza_scene_model::{camera_layout, scene_model};
use lahza_temporal_decode as temporal_decode;
use lahza_video::{media_bank, video};
use lahza_viewport as viewport;
#[cfg(test)]
use lahza_model as model;
#[cfg(test)]
use lahza_scene_model::camera_timeline;

mod motion_blur;
pub use motion_blur::*;
