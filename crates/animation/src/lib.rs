//! Project-wide animation preferences. Missing overrides retain legacy regions.
#![allow(dead_code)]

use lahza_viewport as viewport;
#[cfg(test)]
use lahza_model as model;

mod animation;
pub use animation::*;
