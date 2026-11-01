//! Recording projects on disk: the session layout, edit drafts, discovery of
//! saved recordings, and installation and recovery of captured segments.
#![allow(dead_code)]

use lahza_clips as clips;
use lahza_model as model;
use lahza_video::video;
use lahza_viewport as viewport;
#[cfg(test)]
use lahza_scene_model::scene_model;

mod project;
pub use project::*;
