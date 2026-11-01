//! Narration transcript of a recording: timed words on the source timeline
//! and the readable subtitle cues derived from them. Words drive karaoke
//! highlighting and transcript-based cutting; cues drive the subtitle bar
//! and are the editable source of truth for its text.
#![allow(dead_code)]

use lahza_clips as clips;

mod transcript;
pub use transcript::*;
