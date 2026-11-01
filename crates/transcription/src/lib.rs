//! On-device narration transcription through whisper.cpp's `whisper-cli`
//! (Ubuntu: `sudo apt install whisper.cpp`), the same way FFmpeg is used:
//! as a runtime tool, never linked. Speech models are downloaded once into
//! the user's data directory.
#![allow(dead_code)]

use lahza_clips as clips;
use lahza_speech_runtime as speech_runtime;
use lahza_transcript as transcript;
use lahza_video::{media_bank, video};

mod transcription;
pub use transcription::*;
