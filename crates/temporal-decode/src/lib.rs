//! Native temporal frames: one sequential decode for forward shutters, with
//! accurate seeks for discontinuities. Cache intervals come from encoded PTS.
#![allow(dead_code)]

use lahza_video::video;

mod temporal_decode;
pub use temporal_decode::*;
