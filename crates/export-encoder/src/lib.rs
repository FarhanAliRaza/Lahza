//! Hardware encoding is selected by a real, bounded encode at the requested
//! size. An FFmpeg build listing an encoder does not imply a usable GPU.
#![allow(dead_code)]

use lahza_audio_processing as audio_processing;

mod export_encoder;
pub use export_encoder::*;
