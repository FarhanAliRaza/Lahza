//! Curated scene templates: a background look, layout, 3D pose, motion, and
//! timed captions bundled into one click. Templates are the "make it look
//! like a launch video" starting points; everything they set stays editable
//! afterwards through the normal inspector, lanes, and tools.
#![allow(dead_code)]

use lahza_backgrounds as backgrounds;
use lahza_scene_model::scene_model;
use lahza_viewport as viewport;

mod templates;
pub use templates::*;
