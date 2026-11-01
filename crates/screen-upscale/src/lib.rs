//! Tiled GPU UI reconstruction, independently trained on natively rendered
//! screen content. Separate 2× and 4× models predict an RGB residual over
//! bilinear sampling.
//! The compositor owns the context and submission lifetime; this module only
//! records commands. See assets/upscaler/README.md for provenance and training.
#![allow(dead_code)]

mod screen_upscale;
pub use screen_upscale::*;
