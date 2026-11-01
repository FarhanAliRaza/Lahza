//! A local magnifying lens over the composed screen and pointer.
//!
//! The optical model is independently authored: a smooth radial bevel bends
//! the image, disperses RGB at the edge, reflects the surrounding screen, and
//! catches a light above the canvas. CPU and WGSL use the same equations.
#![allow(dead_code)]

use lahza_viewport as viewport;
#[cfg(test)]
use lahza_model as model;

mod loupe;
pub use loupe::*;
