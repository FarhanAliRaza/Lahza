//! Image-scene, crop, and video-editing data models.

pub(crate) use lahza_annotations::{AnnotationMark, AnnotationWorkspace, NormPoint, Tool, ANNOTATION_COLORS};
pub(crate) use lahza_recording::CropRect;

use crate::{
    recording::{
        self,
        clips::{ClipEdge, RecordingClipSegment, RecordingClipTimeline},
        model::PointerCaptureFile,
        viewport::{MotionPreset, ZoomCue},
    },
};
use gpui::{Pixels, RenderImage};
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

/// A clip press selects; release seeks unless movement crossed the drag
/// threshold. Moving a clip never doubles as playhead scrubbing.
#[derive(Clone, Copy)]
pub(crate) struct VideoMoveDrag {
    pub(crate) clip_id: Uuid,
    pub(crate) start_x: Pixels,
    pub(crate) current_x: Pixels,
    pub(crate) start_scroll: f64,
    pub(crate) editor_seconds_per_pixel: f64,
    pub(crate) active: bool,
    pub(crate) original_selection: Option<Uuid>,
    pub(crate) original_zoom_selection: Option<Uuid>,
}

#[derive(Clone, Debug)]
pub(crate) struct VideoTrimDrag {
    pub(crate) start_x: Pixels,
    pub(crate) start_scroll: f64,
    pub(crate) original_position: f64,
    pub(crate) original_timeline: RecordingClipTimeline,
    pub(crate) original_clip: RecordingClipSegment,
    pub(crate) edge: ClipEdge,
    pub(crate) editor_seconds_per_pixel: f64,
    pub(crate) snap_targets: Vec<f64>,
    pub(crate) original_selection: Option<Uuid>,
    pub(crate) original_zoom_selection: Option<Uuid>,
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
    pub(crate) start_scroll: f64,
    pub(crate) original_cues: Vec<ZoomCue>,
    pub(crate) original_cue: ZoomCue,
    pub(crate) kind: VideoZoomDragKind,
    pub(crate) editor_start: f64,
    pub(crate) editor_end: f64,
    pub(crate) editor_seconds_per_pixel: f64,
    pub(crate) snap_targets: Vec<f64>,
    pub(crate) original_selection: Option<Uuid>,
    pub(crate) original_zoom_selection: Option<Uuid>,
}

#[derive(Clone, Debug)]
pub(crate) enum VideoEditSnapshot {
    AnimationPreferences {
        settings: recording::animation::AnimationSettings,
        cursor_motion: recording::pointer_timeline::PointerMotion,
    },
    CameraLayouts {
        base: recording::scene::CameraOverlay,
        regions: Vec<recording::camera_timeline::CameraRegion>,
        selected: Option<Uuid>,
    },
    Audio(Vec<recording::media::AudioClip>),
    Crop(CropRect),
    Annotations(Vec<AnnotationMark>),
    ImageTiming { scene: f64, start: f64, end: f64 },
    Clips {
        timeline: RecordingClipTimeline,
        annotations: Vec<AnnotationMark>,
        selected: Option<Uuid>,
        position: f64,
    },
    Zoom(Vec<ZoomCue>),
    Transcript(Option<recording::transcript::Transcript>),
    /// A transcript-driven cut: footage, annotations and caption text together.
    TranscriptCut {
        timeline: RecordingClipTimeline,
        annotations: Vec<AnnotationMark>,
        transcript: Option<recording::transcript::Transcript>,
    },
}
