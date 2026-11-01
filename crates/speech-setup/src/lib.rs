//! Locate a matched Whisper runtime and prepare on-device speech recognition.
//! Release packages contain the runtime; source installs can fetch the exact
//! application release's runtime, checked against its published SHA256SUMS.
#![allow(dead_code)]

use lahza_speech_runtime as speech_runtime;
use lahza_teleprompter as teleprompter;
use lahza_transcription as transcription;
use lahza_video::video;

mod speech_setup;
pub use speech_setup::*;
