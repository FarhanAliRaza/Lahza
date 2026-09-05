//! Shared annotation, image-scene, crop, and video-editing data models.

use crate::{
    recording::{
        self,
        clips::{ClipEdge, RecordingClipSegment, RecordingClipTimeline},
        model::PointerCaptureFile,
        pointer_timeline::PointerTimeline,
        viewport::{MotionPreset, ViewportTimeline, ZoomCue},
    },
    timed::AnnotationTiming,
};
use gpui::{Pixels, RenderImage};
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Arc};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) enum Tool {
    Select,
    Rectangle,
    FilledRectangle,
    Ellipse,
    Line,
    Arrow,
    Pen,
    Number,
    Text,
    Pixelate,
    Blur,
    Highlight,
}

impl Tool {
    pub(crate) const ALL: [(Tool, &'static str); 12] = [
        (Tool::Select, "icons/select.svg"),
        (Tool::Rectangle, "icons/rectangle.svg"),
        (Tool::FilledRectangle, "icons/filled-rectangle.svg"),
        (Tool::Ellipse, "icons/ellipse.svg"),
        (Tool::Line, "icons/line.svg"),
        (Tool::Arrow, "icons/arrow.svg"),
        (Tool::Pen, "icons/pen.svg"),
        (Tool::Number, "icons/number.svg"),
        (Tool::Text, "icons/text.svg"),
        (Tool::Pixelate, "icons/pixelate.svg"),
        (Tool::Blur, "icons/blur.svg"),
        (Tool::Highlight, "icons/highlight.svg"),
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Tool::Select => "Select",
            Tool::Rectangle => "Rectangle",
            Tool::FilledRectangle => "Filled rectangle",
            Tool::Ellipse => "Ellipse",
            Tool::Line => "Line",
            Tool::Arrow => "Arrow",
            Tool::Pen => "Pen",
            Tool::Number => "Number",
            Tool::Text => "Text",
            Tool::Pixelate => "Pixelate",
            Tool::Blur => "Blur",
            Tool::Highlight => "Highlight",
        }
    }

    pub(crate) fn help_text(self) -> &'static str {
        match self {
            Tool::Select => "Select and move an existing annotation",
            Tool::Rectangle => "Drag to draw an outlined rectangle",
            Tool::FilledRectangle => "Drag to draw a solid rectangle",
            Tool::Ellipse => "Drag to draw a circle or ellipse",
            Tool::Line => "Drag between two endpoints for a straight line",
            Tool::Arrow => "Drag from the tail toward the arrow point",
            Tool::Pen => "Hold and drag to draw a freehand stroke",
            Tool::Number => "Click to place the next numbered circle",
            Tool::Text => "Click to place a text annotation",
            Tool::Pixelate => "Drag over an area to hide it with pixels",
            Tool::Blur => "Drag over an area to obscure it with blur",
            Tool::Highlight => "Drag an area to keep visible; everything outside is dimmed",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub(crate) struct AnnotationMark {
    pub(crate) tool: Tool,
    pub(crate) start: NormPoint,
    pub(crate) end: NormPoint,
    pub(crate) points: Vec<NormPoint>,
    pub(crate) number: usize,
    pub(crate) color: u32,
    pub(crate) stroke_width: f32,
    pub(crate) density: f32,
    pub(crate) text: String,
    pub(crate) font_size: f32,
    pub(crate) font_family: u8,
    pub(crate) text_alignment: u8,
    pub(crate) bold: bool,
    pub(crate) italic: bool,
    pub(crate) underline: bool,
    /// When the scene is animated: when and how the mark appears.
    pub(crate) timing: Option<AnnotationTiming>,
    /// Painted opacity (animation applies its fade here).
    pub(crate) opacity: f32,
    /// Placed by a template; replaced when another template is applied.
    pub(crate) from_template: bool,
    /// Anchored to the visible frame instead of the media, so camera motion
    /// pans beneath it (captions, step numbers).
    pub(crate) pinned: bool,
}

impl Default for AnnotationMark {
    fn default() -> Self {
        Self {
            tool: Tool::Rectangle,
            start: NormPoint::default(),
            end: NormPoint::default(),
            points: Vec::new(),
            number: 1,
            color: ANNOTATION_COLORS[1].1,
            stroke_width: 4.0,
            density: 0.5,
            text: String::new(),
            font_size: 24.0,
            font_family: 0,
            text_alignment: 0,
            bold: false,
            italic: false,
            underline: false,
            timing: None,
            opacity: 1.0,
            from_template: false,
            pinned: false,
        }
    }
}

/// One image of an animated scene sequence: everything the editor needs to
/// bring it back, stored while another image is being edited.
#[derive(Clone)]
pub(crate) struct ImageScene {
    pub(crate) path: PathBuf,
    pub(crate) processed_path: Option<PathBuf>,
    pub(crate) dimensions: (u32, u32),
    pub(crate) rgba: Arc<image::RgbaImage>,
    pub(crate) render: Arc<RenderImage>,
    pub(crate) annotations: Vec<AnnotationMark>,
    pub(crate) zoom_cues: Vec<ZoomCue>,
    pub(crate) duration: f64,
    pub(crate) preset: Option<MotionPreset>,
    pub(crate) pointer_capture: PointerCaptureFile,
    pub(crate) walkthrough_stops: Vec<recording::model::NormalizedPoint>,
    pub(crate) viewport: ViewportTimeline,
    pub(crate) pointer: Option<PointerTimeline>,
}

/// Annotations plus their undo history, so the screenshot editor's marks
/// survive a detour through the recording editor (which has its own set).
#[derive(Clone, Debug, Default)]
pub(crate) struct AnnotationWorkspace {
    pub(crate) marks: Vec<AnnotationMark>,
    pub(crate) undo: Vec<Vec<AnnotationMark>>,
    pub(crate) redo: Vec<Vec<AnnotationMark>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct NormPoint {
    pub(crate) x: f32,
    pub(crate) y: f32,
}

#[derive(Clone, Copy, Debug)]
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
    Clips(RecordingClipTimeline),
    Zoom(Vec<ZoomCue>),
}

pub(crate) const ANNOTATION_COLORS: [(&str, u32); 10] = [
    ("Black", 0x050506),
    ("Red", 0xf73833),
    ("Orange", 0xff8714),
    ("Yellow", 0xffd12e),
    ("Green", 0x2eb85c),
    ("Turquoise", 0x33c4b8),
    ("Blue", 0x2e7aff),
    ("Purple", 0x8c4cf2),
    ("Pink", 0xff2e6e),
    ("White", 0xf5f5f5),
];
