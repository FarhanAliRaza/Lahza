//! Source-time audio preparation shared by playback and export.
//!
//! New captures retain microphone and desktop streams. Only the microphone is
//! cleaned; their lossless prepared mix is reused before any cuts or speed
//! changes. Legacy recordings retain their original first-audio-stream behavior.
#![allow(dead_code)]

use lahza_audio_dsp as audio_dsp;
use lahza_audio_mode as audio_mode;
use lahza_model as model;
use lahza_reuse_audio as reuse_audio;
use lahza_video::{media_bank, video};

mod audio_processing;
pub use audio_processing::*;
