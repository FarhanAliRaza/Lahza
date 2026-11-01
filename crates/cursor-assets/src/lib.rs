//! Vector cursor artwork used when the editor re-renders the recorded
//! pointer in a chosen cursor style, the way Cap swaps the captured cursor
//! for its SVG reproductions of the system cursors.  Only the everyday
//! shapes are shipped: arrow, pointing hand, open/closed hand and I-beam.
#![allow(dead_code)]

use lahza_model as model;

mod cursor_assets;
pub use cursor_assets::*;
