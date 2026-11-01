//! Saved scene presets: a coordinated background, layout, border, shadow,
//! pointer look, and default zoom strength that applies to screenshots and
//! recordings alike. Stored as JSON in the user's config directory.
#![allow(dead_code)]

use lahza_model as model;
use lahza_scene_model::scene_model;

mod presets;
pub use presets::*;
