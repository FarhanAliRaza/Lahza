//! Scene export: renders the full composition (background, media surface,
//! camera motion, pointer) frame by frame through the shared
//! [`SceneCompositor`] and encodes it with FFmpeg.
//!
//! Recordings and animated screenshots use the same pipeline; only the frame
//! source differs.
#![allow(dead_code)]

use lahza_audio_mode as audio_mode;
use lahza_audio_processing as audio_processing;
use lahza_camera_playback as camera_playback;
use lahza_clips as clips;
use lahza_export_encoder as export_encoder;
use lahza_media as media;
use lahza_motion_blur as motion_blur;
use lahza_pointer_timeline as pointer_timeline;
use lahza_scene as scene;
use lahza_studio_overlays as studio_overlays;
use lahza_video::{media_bank, video};
use lahza_viewport as viewport;
#[cfg(test)]
use lahza_model as model;
#[cfg(test)]
use lahza_playback as playback;
#[cfg(test)]
use lahza_scene_model::{camera_layout, camera_timeline};

mod export;
pub use export::*;
