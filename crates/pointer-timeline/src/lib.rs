//! Smoothed pointer motion, cursor artwork, and click effects over time.
#![allow(dead_code)]

use lahza_clips as clips;
use lahza_cursor_assets as cursor_assets;
use lahza_cursor_planner as cursor_planner;
use lahza_model as model;
use lahza_motion as motion;
use lahza_pointer as pointer;

mod pointer_timeline;
pub use pointer_timeline::*;
