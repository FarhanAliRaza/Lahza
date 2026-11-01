//! Teleprompter: the script model, the fuzzy matcher that tracks how far the
//! speaker has read, the fixed line layout the prompter scrolls through, and
//! live speech following through whisper.cpp's `whisper-stream`.
//!
//! Following is best effort. Without whisper.cpp, a speech model or a
//! microphone the script still shows and can be scrolled by hand; it never
//! interferes with the recording.
#![allow(dead_code)]

use lahza_model as model;
use lahza_speech_diagnostics as speech_diagnostics;
use lahza_speech_runtime as speech_runtime;
use lahza_transcription as transcription;

mod teleprompter;
pub use teleprompter::*;
