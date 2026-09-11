//! Shared annotation engine for the screenshot and video editors.
//!
//! This crate owns persisted marks, geometry, snapping, text metrics, GPUI
//! painting, SVG/raster export, redaction, and entrance/exit animation. The
//! host owns input dispatch, selection/undo transactions, files, and playhead
//! or viewport state. See the crate README for the integration boundary.

pub mod canvas;
pub mod fonts;
pub mod geometry;
mod ink;
mod model;
pub mod redaction;
pub mod snapping;
pub mod svg;
pub mod text;
pub mod timing;

pub use model::{AnnotationMark, AnnotationWorkspace, NormPoint, Tool, ANNOTATION_COLORS};
pub use timing::{AnnotationTiming, EntranceEffect, ExitEffect};
