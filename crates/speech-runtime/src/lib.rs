//! Shared speech processing preferences and evidence from Whisper itself.
//!
//! Discovering a GPU, loading its plugin, or requesting `use gpu = 1` does
//! not mean the model is running there. Only backend initialization confirms it.
#![allow(dead_code)]

use lahza_model as model;

mod speech_runtime;
pub use speech_runtime::*;
