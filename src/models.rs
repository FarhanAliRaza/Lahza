//! Image-scene, crop, and video-editing data models.

pub(crate) use lahza_annotations::{AnnotationMark, AnnotationWorkspace, NormPoint, Tool, ANNOTATION_COLORS};

use crate::{
    recording::{
        self,
        clips::{ClipEdge, RecordingClipSegment, RecordingClipTimeline},
        model::PointerCaptureFile,
        pointer_timeline::PointerTimeline,
        viewport::{MotionPreset, ViewportTimeline, ZoomCue},
    },
};
use gpui::{Pixels, RenderImage};
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Arc};
use uuid::Uuid;

/// One image of an animated scene sequence: everything the editor needs to
/// bring it back, stored while another image is being edited.
#[derive(Clone)]
pub(crate) struct ImageScene {
    pub(crate) original_capture: Option<CropSnapshot>,
    pub(crate) source_crop: CropRect,
    pub(crate) path: PathBuf,
    pub(crate) processed_path: Option<PathBuf>,
    pub(crate) dimensions: (u32, u32),
    pub(crate) rgba: Arc<image::RgbaImage>,
    pub(crate) render: Arc<RenderImage>,
    pub(crate) annotations: Vec<AnnotationMark>,
    pub(crate) zoom_cues: Vec<ZoomCue>,
    pub(crate) duration: f64,
    pub(crate) image_start: f64,
    pub(crate) image_end: f64,
    pub(crate) preset: Option<MotionPreset>,
    pub(crate) pointer_capture: PointerCaptureFile,
    pub(crate) walkthrough_stops: Vec<recording::model::NormalizedPoint>,
    pub(crate) viewport: ViewportTimeline,
    pub(crate) pointer: Option<PointerTimeline>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct CropRect {
    pub(crate) x: f32,
    pub(crate) y: f32,
    pub(crate) width: f32,
    pub(crate) height: f32,
}

impl CropRect {
    pub(crate) const UNIT: Self = Self {
        x: 0.0,
        y: 0.0,
        width: 1.0,
        height: 1.0,
    };

    pub(crate) fn right(self) -> f32 {
        self.x + self.width
    }

    pub(crate) fn bottom(self) -> f32 {
        self.y + self.height
    }

    pub(crate) fn validated(self) -> Self {
        if ![self.x, self.y, self.width, self.height].iter().all(|v| v.is_finite())
            || self.width <= 0.0 || self.height <= 0.0
        {
            return Self::UNIT;
        }
        let x = self.x.clamp(0.0, 0.9999);
        let y = self.y.clamp(0.0, 0.9999);
        Self { x, y, width: self.width.clamp(0.0001, 1.0 - x), height: self.height.clamp(0.0001, 1.0 - y) }
    }

    pub(crate) fn sample_bounds(self, width: u32, height: u32) -> (f64, f64, f64, f64) {
        let crop = self.validated();
        let (w, h) = (width.max(1) as f64, height.max(1) as f64);
        let left = ((crop.x as f64 * w + 0.001).floor() + 0.5) / w;
        let top = ((crop.y as f64 * h + 0.001).floor() + 0.5) / h;
        let right = (((crop.x as f64 + crop.width as f64) * w - 0.001).ceil() - 0.5) / w;
        let bottom = (((crop.y as f64 + crop.height as f64) * h - 0.001).ceil() - 0.5) / h;
        (left, top, right.max(left), bottom.max(top))
    }

    pub(crate) fn snapped(self, width: u32, height: u32) -> Self {
        let crop = self.validated();
        let (w, h) = (width.max(1) as f32, height.max(1) as f32);
        let left = (crop.x * w).floor();
        let top = (crop.y * h).floor();
        let right = (crop.right() * w).ceil().min(w).max(left + 1.0);
        let bottom = (crop.bottom() * h).ceil().min(h).max(top + 1.0);
        Self { x: left / w, y: top / h, width: (right - left) / w, height: (bottom - top) / h }
    }

    /// Source coordinates shared by rendering, cursor placement, and editing.
    pub(crate) fn visible_rect(self, mut frame: recording::viewport::ViewportFrame) -> (f64, f64, f64, f64) {
        let crop = self.validated();
        let (x, y, width, height) = (crop.x as f64, crop.y as f64, crop.width as f64, crop.height as f64);
        frame.anchor.x = (frame.anchor.x - x) / width;
        frame.anchor.y = (frame.anchor.y - y) / height;
        let (left, top, visible) = recording::viewport::visible_rect(frame);
        (x + left * width, y + top * height, visible * width, visible * height)
    }
}

impl Default for CropRect {
    fn default() -> Self { Self::UNIT }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CropHandle {
    TopLeft,
    Top,
    TopRight,
    Left,
    Right,
    BottomLeft,
    Bottom,
    BottomRight,
}

pub(crate) const CROP_HANDLES: [CropHandle; 8] = [
    CropHandle::TopLeft,
    CropHandle::Top,
    CropHandle::TopRight,
    CropHandle::Left,
    CropHandle::Right,
    CropHandle::BottomLeft,
    CropHandle::Bottom,
    CropHandle::BottomRight,
];

#[derive(Clone, Copy, Debug)]
pub(crate) enum CropDrag {
    Move { start: NormPoint, rect: CropRect },
    Resize(CropHandle),
}

#[derive(Clone)]
pub(crate) struct CropSnapshot {
    pub(crate) source_crop: CropRect,
    pub(crate) path: PathBuf,
    pub(crate) dimensions: (u32, u32),
    pub(crate) annotations: Vec<AnnotationMark>,
}

/// Drag-to-reorder for a timeline clip. Armed on clip mouse-down, it only
/// becomes `active` (and suppresses playhead scrubbing) after the pointer
/// travels past a small threshold, so plain clicks keep seeking.
#[derive(Clone, Copy)]
pub(crate) struct VideoMoveDrag {
    pub(crate) clip_id: Uuid,
    pub(crate) start_x: Pixels,
    pub(crate) current_x: Pixels,
    pub(crate) active: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct VideoTrimDrag {
    pub(crate) start_x: Pixels,
    pub(crate) original_timeline: RecordingClipTimeline,
    pub(crate) original_clip: RecordingClipSegment,
    pub(crate) edge: ClipEdge,
    pub(crate) editor_seconds_per_pixel: f64,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum VideoZoomDragKind {
    Move,
    Leading,
    Trailing,
}

#[derive(Clone, Debug)]
pub(crate) struct VideoZoomDrag {
    pub(crate) start_x: Pixels,
    pub(crate) original_cues: Vec<ZoomCue>,
    pub(crate) original_cue: ZoomCue,
    pub(crate) kind: VideoZoomDragKind,
    pub(crate) editor_start: f64,
    pub(crate) editor_end: f64,
    pub(crate) editor_seconds_per_pixel: f64,
}

#[derive(Clone, Debug)]
pub(crate) enum VideoEditSnapshot {
    Crop(CropRect),
    Annotations(Vec<AnnotationMark>),
    ImageTiming { scene: f64, start: f64, end: f64 },
    Clips { timeline: RecordingClipTimeline, annotations: Vec<AnnotationMark> },
    Zoom(Vec<ZoomCue>),
}
