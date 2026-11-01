//! Local person segmentation with the bundled, Apache-2.0 PPHumanSeg model.
//! Bundled ONNX Runtime CPU is preferred on Linux x86_64; portable Rust
//! inference remains available on other architectures or runtime load failure.
//! The renderer invokes this on its worker, before CPU/GPU composition. No
//! frame is sent over the network and no device is opened by this module.
#![allow(dead_code)]

use lahza_camera_matting_runtime as camera_matting_runtime;
use lahza_scene_model::scene_model;
#[cfg(test)]
use lahza_scene_model::{camera_layout, camera_timeline};

mod camera_matting;
pub use camera_matting::*;
