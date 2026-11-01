//! Local speech post-processing, independent of the neural model runtime.
//!
//! The stage ordering and voice-level targets follow the recovered microphone
//! presets. These portable filters are not Apple's AudioUnits or SoundAnalysis:
//! speech activity uses energy, modulation, and periodicity, with nearby breaths
//! retained. In particular target dB here means linked-channel RMS, not LUFS.
//! Biquad equations follow https://www.w3.org/TR/audio-eq-cookbook/.
//!
//! Files contain little-endian f32 samples. Global scans use fixed histograms;
//! rendering retains at most four seconds of audio plus fixed filter state.
//! The neural model runs BETWEEN normalize_file and finish_file. This module
//! does not implement, emulate, or silently replace neural enhancement.
#![allow(dead_code)]

use lahza_audio_mode as audio_mode;

mod audio_dsp;
pub use audio_dsp::*;
