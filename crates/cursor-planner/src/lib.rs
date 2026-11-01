//! Deterministic cursor reconstruction in recording pixels.
//!
//! A spring alone misses a click while it catches up with the pointer. This
//! planner searches bounded look-ahead and response speeds against the recorded
//! interaction positions, then stores an immutable path shared by preview and
//! export. Edit boundaries are independent trajectories. The implementation is
//! intentionally independent of any external renderer or application code.
#![allow(dead_code)]

use lahza_motion as motion;

mod cursor_planner;
pub use cursor_planner::*;
