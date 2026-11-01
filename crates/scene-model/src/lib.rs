//! The scene model with the camera layout and camera timeline it embeds:
//! styles, geometry, and timed camera regions shared by preview and export.
#![allow(dead_code)]

use lahza_animation as animation;
use lahza_cursor_assets as cursor_assets;
use lahza_pointer_timeline as pointer_timeline;
use lahza_rect as rect;
use lahza_studio_overlays as studio_overlays;
use lahza_viewport as viewport;
#[cfg(test)]
use lahza_clips as clips;

pub mod camera_layout;
pub mod camera_timeline;
pub mod scene_model;
