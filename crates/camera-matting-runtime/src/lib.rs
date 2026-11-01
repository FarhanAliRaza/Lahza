//! Bundled CPU inference runtime. The standalone executable contains all
//! runtime bytes and materializes a verified, atomic cache entry locally.
#![allow(dead_code)]

mod camera_matting_runtime;
pub use camera_matting_runtime::*;
