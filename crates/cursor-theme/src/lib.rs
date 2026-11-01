//! Swaps captured cursor bitmaps for the high-resolution versions shipped by
//! the user's Xcursor theme, replacing known system cursors with sharp
//! artwork when the cursor is enlarged, and records which shape each
//! captured cursor is so it can be redrawn in another cursor style.
#![allow(dead_code)]

use lahza_cursor_assets as cursor_assets;
use lahza_model as model;

mod cursor_theme;
pub use cursor_theme::*;
