//! Shared scene composition for screenshots and recordings.
//!
//! A scene is a canvas (background + effects) with one media surface (the
//! screenshot or a decoded video frame) placed on it through a 2D/3D
//! transform. Preview and export share geometry and asset preparation. Export
//! uses the offscreen Vulkan compositor when available, with this CPU renderer
//! as its reference and fallback.
#![allow(dead_code)]

use lahza_camera_matting as camera_matting;
use lahza_cursor_assets as cursor_assets;
use lahza_loupe as loupe;
use lahza_model as model;
use lahza_overlays as overlays;
use lahza_pointer_timeline as pointer_timeline;
use lahza_scene_model::{camera_layout, scene_model};
use lahza_screen_upscale as screen_upscale;
use lahza_viewport as viewport;
#[cfg(test)]
use lahza_motion_blur as motion_blur;
#[cfg(test)]
use lahza_scene_model::camera_timeline;
#[cfg(test)]
use lahza_video::video;

mod scene;
pub use scene::*;
