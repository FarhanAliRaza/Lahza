//! Input feedback and narration drawn above the finished scene: keystroke
//! captions, the speed badge, and the subtitle bar. Layout lives here and is
//! shared verbatim by the GPUI preview (which paints the shapes directly) and
//! the exporter (which rasterizes the same shapes through SVG), so both agree
//! at any canvas size. Every metric derives from the card or canvas size.
#![allow(dead_code)]

use lahza_clips as clips;
use lahza_model as model;
use lahza_overlays as overlays;
use lahza_rect as rect;
use lahza_transcript as transcript;

mod studio_overlays;
pub use studio_overlays::*;
