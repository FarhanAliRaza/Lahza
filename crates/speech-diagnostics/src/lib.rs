//! Durable text diagnostics for each voice-follow session, including desktop
//! launches whose stderr may not be connected to the development launcher.
#![allow(dead_code)]

mod speech_diagnostics;
pub use speech_diagnostics::*;
