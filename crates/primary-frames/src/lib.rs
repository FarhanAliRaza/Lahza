//! Native buffers shared by the main transport and temporal preview. The tap
//! precedes videorate, so intervals describe encoded pictures, not UI clocks.
#![allow(dead_code)]

use lahza_temporal_decode as temporal_decode;
use lahza_video::video;

mod primary_frames;
pub use primary_frames::*;
