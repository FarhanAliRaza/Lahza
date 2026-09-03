use futures_util::StreamExt;
use gpui::{
    anchored, canvas, div, font, hsla, img, linear_color_stop, linear_gradient, point, prelude::*,
    px, quad, rgb, size, svg, AnyElement, AnyWindowHandle, App, Application, AssetSource, AsyncApp,
    Background, Bounds, BoxShadow, ClickEvent, ContentMask, Context, CursorStyle, FocusHandle,
    FontWeight, Hsla, IntoElement, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, ObjectFit, PathBuilder, PathPromptOptions, Pixels, Point, Render, RenderImage,
    ScrollWheelEvent, SharedString, StyledImage, Task, TextRun, Timer, TitlebarOptions,
    UnderlineStyle, Window, WindowBounds, WindowDecorations, WindowOptions,
};
use std::fmt::Write as _;
use std::{
    borrow::Cow,
    collections::HashSet,
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc, Arc, Mutex,
    },
    time::{Duration, Instant},
};
use uuid::Uuid;

mod motion_ui;
mod preset_cards;
mod recording;
mod scene_ui;
mod shell_ui;
mod template_ui;
mod timed;

use scene_ui::{AnnotationDrag, MediaDrag, PreviewCache, SceneSelection};
use serde::{Deserialize, Serialize};
use shell_ui::InspectorTab;
use timed::AnnotationTiming;

use motion_ui::{MotionPick, MOTION_ZOOM_SLIDER};

use recording::{
    camera_preview::{CameraFrames, CameraPreview},
    clips::{ClipEdge, RecordingClipSegment, RecordingClipTimeline},
    export::{ExportFormat, ExportProgress, ExportResolution},
    model::{PointerCaptureFile, RecordingSession},
    native::{audio_sources, default_camera_device, AudioSource, NativeRecorder, RecordingOptions},
    pointer_timeline::PointerTimeline,
    presets::PresetLibrary,
    scene::{CameraOverlay, PointerStyle, SceneStyle, SceneTransform, Watermark, WindowFrame},
    session::{RecordingController, RecordingState},
    video::{
        decode_frame, load_or_rebuild_poster, probe_media, render_clip_preview,
        render_denoised_copy, DecodedFrame, SynchronizedPlaybackStream,
    },
    viewport::{synthesize_zoom_cues, visible_rect, MotionPreset, ViewportTimeline, ZoomCue},
};

struct Assets {
    base: PathBuf,
}

fn asset_directory() -> PathBuf {
    if let Some(path) = std::env::var_os("SCREENDROP_ASSETS") {
        return PathBuf::from(path);
    }

    if let Ok(executable) = std::env::current_exe() {
        if let Some(prefix) = executable.parent().and_then(|bin| bin.parent()) {
            let installed = prefix.join("share/screendrop/assets");
            if installed.is_dir() {
                return installed;
            }
        }
    }

    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets")
}

impl AssetSource for Assets {
    fn load(&self, path: &str) -> gpui::Result<Option<Cow<'static, [u8]>>> {
        fs::read(self.base.join(path))
            .map(|data| Some(Cow::Owned(data)))
            .map_err(Into::into)
    }

    fn list(&self, path: &str) -> gpui::Result<Vec<SharedString>> {
        fs::read_dir(self.base.join(path))
            .map(|entries| {
                entries
                    .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
                    .map(SharedString::from)
                    .collect()
            })
            .map_err(Into::into)
    }
}

fn ink() -> Hsla {
    hsla(220.0 / 360.0, 0.13, 0.12, 1.0)
}

fn muted() -> Hsla {
    hsla(220.0 / 360.0, 0.06, 0.43, 1.0)
}

fn line() -> Hsla {
    hsla(220.0 / 360.0, 0.08, 0.88, 1.0)
}

fn panel() -> Hsla {
    hsla(0.0, 0.0, 0.985, 1.0)
}

fn blue() -> Hsla {
    hsla(211.0 / 360.0, 0.95, 0.55, 1.0)
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn timestamped_export_name() -> String {
    chrono::Local::now()
        .format("Lahza-%Y-%m-%d_%H-%M-%S-%3f.png")
        .to_string()
}

fn cached_render_image(mut pixels: image::RgbaImage) -> Arc<RenderImage> {
    // GPUI uploads raster images as BGRA. Keep this decoded image alive in
    // Studio so annotation-only redraws never restart asynchronous file IO.
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
    Arc::new(RenderImage::new(vec![image::Frame::new(pixels)]))
}

async fn capture_with_system_picker() -> Result<PathBuf, String> {
    let request = ashpd::desktop::screenshot::Screenshot::request()
        .interactive(true)
        .modal(false)
        .send()
        .await
        .map_err(|error| error.to_string())?;
    let response = request.response().map_err(|error| error.to_string())?;
    response
        .uri()
        .to_file_path()
        .map_err(|_| "The screenshot portal returned a non-file URI".to_string())
}

/// Runs the system screenshot picker with the Studio window out of the way.
/// GNOME freezes the screen when its picker opens, so a Studio window that is
/// still on top ends up inside the shot and covers the area being selected.
async fn capture_behind_window(
    window_handle: Option<AnyWindowHandle>,
    cx: &mut AsyncApp,
) -> Result<PathBuf, String> {
    if let Some(window_handle) = window_handle {
        let _ = window_handle.update(cx, |_, window, _| window.minimize_window());
        // Give the compositor time to finish the minimize animation before
        // the portal snapshots the screen.
        Timer::after(Duration::from_millis(400)).await;
    }
    let result = capture_with_system_picker().await;
    if let Some(window_handle) = window_handle {
        let _ = window_handle.update(cx, |_, window, cx| {
            window.activate_window();
            cx.activate(true);
        });
    }
    result
}

const SOLID_BACKGROUNDS: [(&str, u32); 16] = [
    ("Black", 0x050506),
    ("White", 0xf5f5f0),
    ("Graphite", 0x2b2e36),
    ("Red", 0xf03b47),
    ("Orange", 0xf78429),
    ("Yellow", 0xf5ba3b),
    ("Green", 0x3b9c5c),
    ("Blue", 0x2980e0),
    ("Purple", 0x7a42e8),
    ("Blush", 0xeda89e),
    ("Mint", 0xa8e6ba),
    ("Sky", 0xa1c9f0),
    ("Lavender", 0xccc2eb),
    ("Peach", 0xfaccb0),
    ("Sage", 0xbdd1b3),
    ("Sand", 0xe8dec2),
];

#[derive(Clone, Copy)]
struct GradientPreset {
    title: &'static str,
    colors: [u32; 3],
    angle: f32,
}

const GRADIENT_BACKGROUNDS: [GradientPreset; 16] = [
    GradientPreset {
        title: "Aurora",
        colors: [0xfa4f94, 0x6652f2, 0x4ad6cc],
        angle: 135.0,
    },
    GradientPreset {
        title: "Cobalt",
        colors: [0x0a0d80, 0x4230ed, 0x6babfa],
        angle: 160.0,
    },
    GradientPreset {
        title: "Peach",
        colors: [0xfa615c, 0xfcb55c, 0xe654a6],
        angle: 135.0,
    },
    GradientPreset {
        title: "Glass",
        colors: [0xdef2f0, 0x75c4db, 0x4087ed],
        angle: 135.0,
    },
    GradientPreset {
        title: "Plasma",
        colors: [0x140538, 0x591fd6, 0xf2426b],
        angle: 225.0,
    },
    GradientPreset {
        title: "Mango",
        colors: [0xfcbf33, 0xf55436, 0xab30e3],
        angle: 135.0,
    },
    GradientPreset {
        title: "Mist",
        colors: [0xf0f0eb, 0xcce0f0, 0xf2c2b3],
        angle: 135.0,
    },
    GradientPreset {
        title: "Lagoon",
        colors: [0x144d8a, 0x40a3b8, 0xb3ebc7],
        angle: 45.0,
    },
    GradientPreset {
        title: "Ember",
        colors: [0x2e0814, 0xdb2b2e, 0xffab40],
        angle: 135.0,
    },
    GradientPreset {
        title: "Violet",
        colors: [0x3d1482, 0x9638f0, 0xf56bbd],
        angle: 160.0,
    },
    GradientPreset {
        title: "Sea Glass",
        colors: [0x6edbbf, 0x409ecc, 0x3859bf],
        angle: 135.0,
    },
    GradientPreset {
        title: "Citrus",
        colors: [0xfce84d, 0x70c74a, 0x1f946b],
        angle: 225.0,
    },
    GradientPreset {
        title: "Amethyst",
        colors: [0x1a1447, 0x5926a6, 0xc263f2],
        angle: 45.0,
    },
    GradientPreset {
        title: "Sorbet",
        colors: [0xff7d82, 0xffbd7a, 0x8fc7fa],
        angle: 135.0,
    },
    GradientPreset {
        title: "Mineral",
        colors: [0xedf5f2, 0xa3b8d1, 0x546b8c],
        angle: 135.0,
    },
    GradientPreset {
        title: "Dawn",
        colors: [0xfa9ec4, 0xfad178, 0x6bb5f5],
        angle: 45.0,
    },
];

#[derive(Clone, Copy)]
struct BackgroundPreset {
    name: &'static str,
    wallpaper_tab: usize,
    library_tab: usize,
    wallpaper_asset: &'static str,
    color_index: usize,
    gradient_index: usize,
    padding: u8,
    shadow: u8,
    corners: u8,
    shadow_style: usize,
    aspect_ratio: usize,
    border: bool,
    border_color: usize,
    border_thickness: u8,
    border_opacity: u8,
}

const BACKGROUND_PRESETS: [BackgroundPreset; 5] = [
    BackgroundPreset {
        name: "Frosted Lake",
        wallpaper_tab: 2,
        library_tab: 1,
        wallpaper_asset: "wallpapers/uihssn/uihssn-2.jpeg",
        color_index: 7,
        gradient_index: 0,
        padding: 8,
        shadow: 14,
        corners: 2,
        shadow_style: 1,
        aspect_ratio: 0,
        border: false,
        border_color: 3,
        border_thickness: 12,
        border_opacity: 30,
    },
    BackgroundPreset {
        name: "Sunset Glass",
        wallpaper_tab: 2,
        library_tab: 1,
        wallpaper_asset: "wallpapers/uihssn/uihssn-4.jpeg",
        color_index: 4,
        gradient_index: 4,
        padding: 14,
        shadow: 32,
        corners: 10,
        shadow_style: 0,
        aspect_ratio: 0,
        border: false,
        border_color: 0,
        border_thickness: 0,
        border_opacity: 0,
    },
    BackgroundPreset {
        name: "Midnight",
        wallpaper_tab: 0,
        library_tab: 1,
        wallpaper_asset: "wallpapers/uihssn/uihssn-12.jpg",
        color_index: 0,
        gradient_index: 0,
        padding: 10,
        shadow: 38,
        corners: 7,
        shadow_style: 2,
        aspect_ratio: 0,
        border: true,
        border_color: 4,
        border_thickness: 5,
        border_opacity: 55,
    },
    BackgroundPreset {
        name: "Clean White",
        wallpaper_tab: 0,
        library_tab: 1,
        wallpaper_asset: "wallpapers/uihssn/uihssn-12.jpg",
        color_index: 1,
        gradient_index: 0,
        padding: 7,
        shadow: 18,
        corners: 4,
        shadow_style: 0,
        aspect_ratio: 0,
        border: false,
        border_color: 3,
        border_thickness: 0,
        border_opacity: 0,
    },
    BackgroundPreset {
        name: "Aurora",
        wallpaper_tab: 1,
        library_tab: 1,
        wallpaper_asset: "wallpapers/uihssn/uihssn-12.jpg",
        color_index: 7,
        gradient_index: 6,
        padding: 12,
        shadow: 26,
        corners: 12,
        shadow_style: 1,
        aspect_ratio: 0,
        border: true,
        border_color: 2,
        border_thickness: 4,
        border_opacity: 40,
    },
];

const UIHSSN_WALLPAPERS: [&str; 12] = [
    "wallpapers/uihssn/uihssn-2.jpeg",
    "wallpapers/uihssn/uihssn-3.jpeg",
    "wallpapers/uihssn/uihssn-4.jpeg",
    "wallpapers/uihssn/uihssn-5.jpeg",
    "wallpapers/uihssn/uihssn-6.jpg",
    "wallpapers/uihssn/uihssn-7.png",
    "wallpapers/uihssn/uihssn-9.jpg",
    "wallpapers/uihssn/uihssn-10.jpg",
    "wallpapers/uihssn/uihssn-11.jpg",
    "wallpapers/uihssn/uihssn-12.jpg",
    "wallpapers/uihssn/uihssn-13.jpg",
    "wallpapers/uihssn/uihssn-14.jpg",
];

const FAYAZ_WALLPAPERS: [&str; 5] = [
    "wallpapers/fayaz/blue-skies.jpg",
    "wallpapers/fayaz/canyon.jpg",
    "wallpapers/fayaz/golden-gate-bridge.jpg",
    "wallpapers/fayaz/India.jpg",
    "wallpapers/fayaz/nyc.jpg",
];

fn gradient_layers(preset: GradientPreset) -> (Background, Background) {
    let middle = Hsla::from(rgb(preset.colors[1]));
    (
        linear_gradient(
            preset.angle,
            linear_color_stop(rgb(preset.colors[0]), 0.0),
            linear_color_stop(middle, 1.0),
        ),
        linear_gradient(
            preset.angle,
            linear_color_stop(middle.opacity(0.0), 0.35),
            linear_color_stop(rgb(preset.colors[2]), 1.0),
        ),
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum Tool {
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
    const ALL: [(Tool, &'static str); 12] = [
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

    fn label(self) -> &'static str {
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

    fn help_text(self) -> &'static str {
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
struct AnnotationMark {
    tool: Tool,
    start: NormPoint,
    end: NormPoint,
    points: Vec<NormPoint>,
    number: usize,
    color: u32,
    stroke_width: f32,
    density: f32,
    text: String,
    font_size: f32,
    font_family: u8,
    text_alignment: u8,
    bold: bool,
    italic: bool,
    underline: bool,
    /// When the scene is animated: when and how the mark appears.
    timing: Option<AnnotationTiming>,
    /// Painted opacity (animation applies its fade here).
    opacity: f32,
    /// Placed by a template; replaced when another template is applied.
    from_template: bool,
    /// Anchored to the visible frame instead of the media, so camera motion
    /// pans beneath it (captions, step numbers).
    pinned: bool,
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
struct ImageScene {
    path: PathBuf,
    processed_path: Option<PathBuf>,
    dimensions: (u32, u32),
    rgba: Arc<image::RgbaImage>,
    render: Arc<RenderImage>,
    annotations: Vec<AnnotationMark>,
    zoom_cues: Vec<ZoomCue>,
    duration: f64,
    preset: Option<MotionPreset>,
    pointer_capture: PointerCaptureFile,
    walkthrough_stops: Vec<recording::model::NormalizedPoint>,
    viewport: ViewportTimeline,
    pointer: Option<PointerTimeline>,
}

/// Annotations plus their undo history, so the screenshot editor's marks
/// survive a detour through the recording editor (which has its own set).
#[derive(Clone, Debug, Default)]
struct AnnotationWorkspace {
    marks: Vec<AnnotationMark>,
    undo: Vec<Vec<AnnotationMark>>,
    redo: Vec<Vec<AnnotationMark>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
struct NormPoint {
    x: f32,
    y: f32,
}

#[derive(Clone, Copy, Debug)]
struct CropRect {
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}

impl CropRect {
    const UNIT: Self = Self {
        x: 0.0,
        y: 0.0,
        width: 1.0,
        height: 1.0,
    };

    fn right(self) -> f32 {
        self.x + self.width
    }

    fn bottom(self) -> f32 {
        self.y + self.height
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CropHandle {
    TopLeft,
    Top,
    TopRight,
    Left,
    Right,
    BottomLeft,
    Bottom,
    BottomRight,
}

const CROP_HANDLES: [CropHandle; 8] = [
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
enum CropDrag {
    Move { start: NormPoint, rect: CropRect },
    Resize(CropHandle),
}

#[derive(Clone)]
struct CropSnapshot {
    path: PathBuf,
    dimensions: (u32, u32),
    annotations: Vec<AnnotationMark>,
}

/// Drag-to-reorder for a timeline clip. Armed on clip mouse-down, it only
/// becomes `active` (and suppresses playhead scrubbing) after the pointer
/// travels past a small threshold, so plain clicks keep seeking.
#[derive(Clone, Copy)]
struct VideoMoveDrag {
    clip_id: Uuid,
    start_x: Pixels,
    current_x: Pixels,
    active: bool,
}

#[derive(Clone, Debug)]
struct VideoTrimDrag {
    start_x: Pixels,
    original_timeline: RecordingClipTimeline,
    original_clip: RecordingClipSegment,
    edge: ClipEdge,
    editor_seconds_per_pixel: f64,
}

#[derive(Clone, Copy, Debug)]
enum VideoZoomDragKind {
    Move,
    Leading,
    Trailing,
}

#[derive(Clone, Debug)]
struct VideoZoomDrag {
    start_x: Pixels,
    original_cues: Vec<ZoomCue>,
    original_cue: ZoomCue,
    kind: VideoZoomDragKind,
    editor_start: f64,
    editor_end: f64,
    editor_seconds_per_pixel: f64,
}

#[derive(Clone, Debug)]
enum VideoEditSnapshot {
    Clips(RecordingClipTimeline),
    Zoom(Vec<ZoomCue>),
}

const ANNOTATION_COLORS: [(&str, u32); 10] = [
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

fn norm_to_screen(point_: NormPoint, image: Bounds<Pixels>) -> Point<Pixels> {
    point(
        image.origin.x + image.size.width * point_.x,
        image.origin.y + image.size.height * point_.y,
    )
}

fn screen_to_norm(point_: Point<Pixels>, image: Bounds<Pixels>) -> NormPoint {
    NormPoint {
        x: ((point_.x - image.origin.x) / image.size.width).clamp(0.0, 1.0),
        y: ((point_.y - image.origin.y) / image.size.height).clamp(0.0, 1.0),
    }
}

fn crop_handle_point(handle: CropHandle, rect: CropRect) -> NormPoint {
    let (x, y) = match handle {
        CropHandle::TopLeft => (rect.x, rect.y),
        CropHandle::Top => (rect.x + rect.width * 0.5, rect.y),
        CropHandle::TopRight => (rect.right(), rect.y),
        CropHandle::Left => (rect.x, rect.y + rect.height * 0.5),
        CropHandle::Right => (rect.right(), rect.y + rect.height * 0.5),
        CropHandle::BottomLeft => (rect.x, rect.bottom()),
        CropHandle::Bottom => (rect.x + rect.width * 0.5, rect.bottom()),
        CropHandle::BottomRight => (rect.right(), rect.bottom()),
    };
    NormPoint { x, y }
}

fn crop_rect_with_aspect(rect: CropRect, aspect: f32) -> CropRect {
    let mut width = rect.width;
    let mut height = rect.height;
    if width / height > aspect {
        width = height * aspect;
    } else {
        height = width / aspect;
    }
    let x = (rect.x + (rect.width - width) * 0.5).clamp(0.0, 1.0 - width);
    let y = (rect.y + (rect.height - height) * 0.5).clamp(0.0, 1.0 - height);
    CropRect {
        x,
        y,
        width,
        height,
    }
}

fn move_crop_rect(rect: CropRect, delta: NormPoint) -> CropRect {
    CropRect {
        x: (rect.x + delta.x).clamp(0.0, 1.0 - rect.width),
        y: (rect.y + delta.y).clamp(0.0, 1.0 - rect.height),
        ..rect
    }
}

fn resize_crop_rect(
    rect: CropRect,
    handle: CropHandle,
    point: NormPoint,
    aspect: Option<f32>,
    min_width: f32,
    min_height: f32,
) -> CropRect {
    let is_left = matches!(
        handle,
        CropHandle::TopLeft | CropHandle::Left | CropHandle::BottomLeft
    );
    let is_right = matches!(
        handle,
        CropHandle::TopRight | CropHandle::Right | CropHandle::BottomRight
    );
    let is_top = matches!(
        handle,
        CropHandle::TopLeft | CropHandle::Top | CropHandle::TopRight
    );
    let is_bottom = matches!(
        handle,
        CropHandle::BottomLeft | CropHandle::Bottom | CropHandle::BottomRight
    );
    let is_corner = (is_left || is_right) && (is_top || is_bottom);
    let mut left = rect.x;
    let mut right = rect.right();
    let mut top = rect.y;
    let mut bottom = rect.bottom();

    if is_corner {
        let anchor_x = if is_left { right } else { left };
        let anchor_y = if is_top { bottom } else { top };
        let mut width = (point.x - anchor_x).abs().max(min_width);
        let mut height = (point.y - anchor_y).abs().max(min_height);
        if let Some(aspect) = aspect {
            if width / height > aspect {
                width = height * aspect;
            } else {
                height = width / aspect;
            }
        }
        width = width.min(if is_left { anchor_x } else { 1.0 - anchor_x });
        height = height.min(if is_top { anchor_y } else { 1.0 - anchor_y });
        if let Some(aspect) = aspect {
            if width / height > aspect {
                width = height * aspect;
            } else {
                height = width / aspect;
            }
        }
        left = if is_left { anchor_x - width } else { anchor_x };
        right = if is_left { anchor_x } else { anchor_x + width };
        top = if is_top { anchor_y - height } else { anchor_y };
        bottom = if is_top { anchor_y } else { anchor_y + height };
    } else {
        if is_left {
            left = point.x.min(right - min_width);
        }
        if is_right {
            right = point.x.max(left + min_width);
        }
        if is_top {
            top = point.y.min(bottom - min_height);
        }
        if is_bottom {
            bottom = point.y.max(top + min_height);
        }
    }
    CropRect {
        x: left.clamp(0.0, 1.0),
        y: top.clamp(0.0, 1.0),
        width: (right - left).clamp(min_width, 1.0),
        height: (bottom - top).clamp(min_height, 1.0),
    }
}

fn mark_screen_bounds(mark: &AnnotationMark, image: Bounds<Pixels>) -> Bounds<Pixels> {
    // A pen stroke's start/end are only its first and last samples; the
    // stroke itself can wander anywhere, so bound every recorded point.
    if mark.tool == Tool::Pen && mark.points.len() > 1 {
        let mut min_x = f32::MAX;
        let mut min_y = f32::MAX;
        let mut max_x = f32::MIN;
        let mut max_y = f32::MIN;
        for normalized in &mark.points {
            let screen = norm_to_screen(*normalized, image);
            min_x = min_x.min(screen.x / px(1.0));
            min_y = min_y.min(screen.y / px(1.0));
            max_x = max_x.max(screen.x / px(1.0));
            max_y = max_y.max(screen.y / px(1.0));
        }
        return Bounds::from_corners(point(px(min_x), px(min_y)), point(px(max_x), px(max_y)));
    }
    let start = norm_to_screen(mark.start, image);
    let end = norm_to_screen(mark.end, image);
    Bounds::from_corners(
        point(start.x.min(end.x), start.y.min(end.y)),
        point(start.x.max(end.x), start.y.max(end.y)),
    )
}

fn mark_hit_bounds(mark: &AnnotationMark, image: Bounds<Pixels>) -> Bounds<Pixels> {
    let bounds = mark_screen_bounds(mark, image);
    let minimum = px(14.0);
    let extra_x = ((minimum - bounds.size.width).max(px(0.0))) * 0.5 + px(5.0);
    let extra_y = ((minimum - bounds.size.height).max(px(0.0))) * 0.5 + px(5.0);
    Bounds::from_corners(
        point(bounds.origin.x - extra_x, bounds.origin.y - extra_y),
        point(
            bounds.origin.x + bounds.size.width + extra_x,
            bounds.origin.y + bounds.size.height + extra_y,
        ),
    )
}

pub(crate) fn paint_annotation(
    mark: &AnnotationMark,
    image: Bounds<Pixels>,
    is_draft: bool,
    show_text_caret: bool,
    window: &mut Window,
    cx: &mut App,
) -> Bounds<Pixels> {
    let bounds = mark_screen_bounds(mark, image);
    let mut rendered_bounds = bounds;
    let color = Hsla::from(rgb(mark.color)).opacity(mark.opacity.clamp(0.0, 1.0));
    let clear = hsla(0.0, 0.0, 0.0, 0.0);
    match mark.tool {
        Tool::Rectangle => window.paint_quad(quad(
            bounds,
            px(2.0),
            clear,
            px(mark.stroke_width),
            color,
            Default::default(),
        )),
        Tool::FilledRectangle => window.paint_quad(quad(
            bounds,
            px(2.0),
            color,
            px(0.0),
            clear,
            Default::default(),
        )),
        Tool::Ellipse | Tool::Number => {
            let radius = if bounds.size.width < bounds.size.height {
                bounds.size.width * 0.5
            } else {
                bounds.size.height * 0.5
            };
            window.paint_quad(quad(
                bounds,
                radius,
                if mark.tool == Tool::Number {
                    color
                } else {
                    clear
                },
                if mark.tool == Tool::Ellipse {
                    px(mark.stroke_width)
                } else {
                    px(0.0)
                },
                color,
                Default::default(),
            ));
        }
        Tool::Line | Tool::Arrow => {
            let start = norm_to_screen(mark.start, image);
            let end = norm_to_screen(mark.end, image);
            let mut builder = PathBuilder::stroke(px(mark.stroke_width));
            builder.move_to(start);
            builder.line_to(end);
            if mark.tool == Tool::Arrow {
                let dx = (end.x - start.x) / px(1.0);
                let dy = (end.y - start.y) / px(1.0);
                let length = (dx * dx + dy * dy).sqrt().max(1.0);
                let ux = dx / length;
                let uy = dy / length;
                let head = 10.0 + mark.stroke_width * 2.0;
                let wing = 5.0 + mark.stroke_width;
                builder.move_to(end);
                builder.line_to(point(
                    end.x + px(-ux * head + -uy * wing),
                    end.y + px(-uy * head + ux * wing),
                ));
                builder.move_to(end);
                builder.line_to(point(
                    end.x + px(-ux * head + uy * wing),
                    end.y + px(-uy * head + -ux * wing),
                ));
            }
            if let Ok(path) = builder.build() {
                window.paint_path(path, color);
            }
        }
        Tool::Pen => {
            if mark.points.len() > 1 {
                let mut builder = PathBuilder::stroke(px(mark.stroke_width));
                for (index, point_) in mark.points.iter().copied().enumerate() {
                    let point = norm_to_screen(point_, image);
                    if index == 0 {
                        builder.move_to(point)
                    } else {
                        builder.line_to(point)
                    }
                }
                if let Ok(path) = builder.build() {
                    window.paint_path(path, color);
                }
            }
        }
        Tool::Pixelate if is_draft => {
            let cell = px(10.0);
            let columns = (bounds.size.width / cell).ceil().max(1.0) as usize;
            let rows = (bounds.size.height / cell).ceil().max(1.0) as usize;
            for row in 0..rows {
                for column in 0..columns {
                    let color = if (row + column) % 2 == 0 {
                        0x363a40
                    } else {
                        0x747b84
                    };
                    let cell_x = cell * column;
                    let cell_y = cell * row;
                    window.paint_quad(quad(
                        Bounds {
                            origin: point(bounds.origin.x + cell_x, bounds.origin.y + cell_y),
                            size: size(
                                cell.min(bounds.size.width - cell_x),
                                cell.min(bounds.size.height - cell_y),
                            ),
                        },
                        px(0.0),
                        rgb(color),
                        px(0.0),
                        clear,
                        Default::default(),
                    ));
                }
            }
        }
        Tool::Blur if is_draft => {
            window.paint_quad(quad(
                bounds,
                px(8.0),
                hsla(210.0 / 360.0, 0.08, 0.72, 0.45),
                px(2.0),
                rgb(0xffffff),
                Default::default(),
            ));
        }
        Tool::Text => {
            let display = mark.text.as_str();
            let font_size = px(mark.font_size.max(8.0));
            let inset = px(3.0);
            let mut caret_x = match mark.text_alignment {
                1 => bounds.center().x,
                2 => bounds.origin.x + bounds.size.width - inset,
                _ => bounds.origin.x + inset,
            };
            if !display.is_empty() {
                let family = match mark.font_family {
                    1 => "DejaVu Sans Condensed",
                    2 => "Ubuntu",
                    _ => "Noto Sans",
                };
                let mut text_font = font(family);
                text_font.weight = if mark.bold {
                    FontWeight::BOLD
                } else {
                    FontWeight::NORMAL
                };
                if mark.italic {
                    text_font = text_font.italic();
                }
                let run = TextRun {
                    len: display.len(),
                    font: text_font,
                    color,
                    background_color: None,
                    underline: mark.underline.then_some(UnderlineStyle {
                        color: Some(color),
                        thickness: px((mark.font_size / 18.0).max(1.0)),
                        wavy: false,
                    }),
                    strikethrough: None,
                };
                let line = window.text_system().shape_line(
                    display.to_string().into(),
                    font_size,
                    &[run],
                    None,
                );
                let origin_x = match mark.text_alignment {
                    1 => bounds.center().x - line.width * 0.5,
                    2 => bounds.origin.x + bounds.size.width - line.width,
                    _ => bounds.origin.x,
                };
                caret_x = origin_x + line.width + px(2.0);
                rendered_bounds = Bounds {
                    origin: point(origin_x - inset, bounds.origin.y),
                    size: size((line.width + px(8.0)).max(px(16.0)), font_size * 1.25),
                };
                let _ = line.paint(
                    point(origin_x, bounds.origin.y),
                    font_size * 1.25,
                    window,
                    cx,
                );
            }
            if display.is_empty() {
                rendered_bounds = Bounds {
                    origin: bounds.origin,
                    size: size(px(16.0), font_size * 1.25),
                };
            }
            if show_text_caret {
                window.paint_quad(quad(
                    Bounds {
                        origin: point(caret_x, bounds.origin.y + font_size * 0.12),
                        size: size(px(1.0), font_size * 0.88),
                    },
                    px(0.0),
                    rgb(0x202124),
                    px(0.0),
                    clear,
                    Default::default(),
                ));
            }
        }
        Tool::Pixelate | Tool::Blur | Tool::Highlight | Tool::Select => {}
    }

    if mark.tool == Tool::Number {
        let label = mark.number.to_string();
        let run = TextRun {
            len: label.len(),
            font: font("Inter").bold(),
            color: rgb(0xffffff).into(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let font_size = px((bounds.size.height / px(1.0) * 0.48).clamp(11.0, 30.0));
        let line = window
            .text_system()
            .shape_line(label.clone().into(), font_size, &[run], None);
        let origin = point(
            bounds.center().x - line.width * 0.5,
            bounds.center().y - font_size * 0.62,
        );
        let _ = line.paint(origin, font_size * 1.25, window, cx);
    }
    rendered_bounds
}

pub(crate) fn paint_highlights(
    marks: &[AnnotationMark],
    image: Bounds<Pixels>,
    window: &mut Window,
) {
    let holes: Vec<_> = marks
        .iter()
        .filter(|mark| mark.tool == Tool::Highlight)
        .map(|mark| mark_screen_bounds(mark, image))
        .collect();
    if holes.is_empty() {
        return;
    }

    let mut xs = vec![image.origin.x, image.origin.x + image.size.width];
    let mut ys = vec![image.origin.y, image.origin.y + image.size.height];
    for hole in &holes {
        xs.extend([hole.origin.x, hole.origin.x + hole.size.width]);
        ys.extend([hole.origin.y, hole.origin.y + hole.size.height]);
    }
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    ys.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    xs.dedup();
    ys.dedup();
    let dim = hsla(0.0, 0.0, 0.0, 0.55);
    let clear = hsla(0.0, 0.0, 0.0, 0.0);
    for x in xs.windows(2) {
        for y in ys.windows(2) {
            let cell = Bounds::from_corners(point(x[0], y[0]), point(x[1], y[1]));
            let center = cell.center();
            if !holes.iter().any(|hole| hole.contains(&center)) {
                window.paint_quad(quad(cell, px(0.0), dim, px(0.0), clear, Default::default()));
            }
        }
    }
}

fn paint_crop_overlay(
    rect: CropRect,
    image: Bounds<Pixels>,
    aspect_locked: bool,
    window: &mut Window,
) {
    let top_left = norm_to_screen(
        NormPoint {
            x: rect.x,
            y: rect.y,
        },
        image,
    );
    let bottom_right = norm_to_screen(
        NormPoint {
            x: rect.right(),
            y: rect.bottom(),
        },
        image,
    );
    let crop = Bounds::from_corners(top_left, bottom_right);
    let dim = hsla(0.0, 0.0, 0.0, 0.55);
    let clear = hsla(0.0, 0.0, 0.0, 0.0);
    let image_right = image.origin.x + image.size.width;
    let image_bottom = image.origin.y + image.size.height;
    for bounds in [
        Bounds::from_corners(image.origin, point(image_right, crop.origin.y)),
        Bounds::from_corners(
            point(image.origin.x, crop.origin.y),
            point(crop.origin.x, crop.origin.y + crop.size.height),
        ),
        Bounds::from_corners(
            point(crop.origin.x + crop.size.width, crop.origin.y),
            point(image_right, crop.origin.y + crop.size.height),
        ),
        Bounds::from_corners(
            point(image.origin.x, crop.origin.y + crop.size.height),
            point(image_right, image_bottom),
        ),
    ] {
        if !bounds.is_empty() {
            window.paint_quad(quad(
                bounds,
                px(0.0),
                dim,
                px(0.0),
                clear,
                Default::default(),
            ));
        }
    }
    window.paint_quad(quad(
        crop,
        px(0.0),
        clear,
        px(1.5),
        rgb(0xffffff),
        Default::default(),
    ));
    for index in 1..=2 {
        let fraction = index as f32 / 3.0;
        let x = crop.origin.x + crop.size.width * fraction;
        let y = crop.origin.y + crop.size.height * fraction;
        let grid = hsla(0.0, 0.0, 1.0, 0.35);
        window.paint_quad(quad(
            Bounds::from_corners(
                point(x, crop.origin.y),
                point(x + px(1.0), crop.origin.y + crop.size.height),
            ),
            px(0.0),
            grid,
            px(0.0),
            clear,
            Default::default(),
        ));
        window.paint_quad(quad(
            Bounds::from_corners(
                point(crop.origin.x, y),
                point(crop.origin.x + crop.size.width, y + px(1.0)),
            ),
            px(0.0),
            grid,
            px(0.0),
            clear,
            Default::default(),
        ));
    }
    for handle in CROP_HANDLES {
        let corner = matches!(
            handle,
            CropHandle::TopLeft
                | CropHandle::TopRight
                | CropHandle::BottomLeft
                | CropHandle::BottomRight
        );
        if aspect_locked && !corner {
            continue;
        }
        let center = norm_to_screen(crop_handle_point(handle, rect), image);
        let size = if corner {
            size(px(13.0), px(13.0))
        } else if matches!(handle, CropHandle::Top | CropHandle::Bottom) {
            size(px(26.0), px(7.0))
        } else {
            size(px(7.0), px(26.0))
        };
        window.paint_quad(quad(
            Bounds {
                origin: point(center.x - size.width * 0.5, center.y - size.height * 0.5),
                size,
            },
            px(2.5),
            rgb(0xffffff),
            px(0.5),
            hsla(0.0, 0.0, 0.0, 0.25),
            Default::default(),
        ));
    }
}

fn fitted_image_bounds(
    canvas: Bounds<Pixels>,
    has_capture: bool,
    dimensions: Option<(u32, u32)>,
    padding: u8,
    border: bool,
    border_thickness: u8,
) -> Bounds<Pixels> {
    let border_width = if border {
        border_thickness as f32 * 0.48
    } else {
        0.0
    };
    // Zero padding means the capture reaches the canvas edge; only the
    // border adds space beyond the user's padding setting.
    let inset = padding as f32 * 2.0 + border_width;
    let x_inset = inset + if has_capture { 0.0 } else { 60.0 };
    let y_inset = inset + if has_capture { 0.0 } else { 30.0 };
    let available_width = ((canvas.size.width / px(1.0)) - x_inset * 2.0).max(1.0);
    let available_height = ((canvas.size.height / px(1.0)) - y_inset * 2.0).max(1.0);
    let (source_width, source_height) = dimensions.unwrap_or((1200, 720));
    let scale =
        (available_width / source_width as f32).min(available_height / source_height as f32);
    let width = source_width as f32 * scale;
    let height = source_height as f32 * scale;
    Bounds {
        origin: point(
            canvas.origin.x + px(x_inset + (available_width - width) * 0.5),
            canvas.origin.y + px(y_inset + (available_height - height) * 0.5),
        ),
        size: size(px(width), px(height)),
    }
}

struct Studio {
    tool: Tool,
    annotation_color_index: usize,
    annotation_stroke_width: f32,
    redaction_strength: u8,
    text_font_size: f32,
    text_font_family: u8,
    text_alignment: u8,
    text_bold: bool,
    text_italic: bool,
    text_underline: bool,
    editing_text: Option<usize>,
    caret_visible: bool,
    _caret_blink_task: Task<()>,
    _global_shortcut_task: Task<()>,
    _recording_clock_task: Task<()>,
    recording_controller: Option<RecordingController<NativeRecorder>>,
    recording_state: RecordingState,
    recording_busy: bool,
    recording_elapsed: Duration,
    recording_started_at: Option<Instant>,
    recording_session_path: Option<PathBuf>,
    record_system_audio: bool,
    record_microphone: bool,
    /// Node name of the chosen microphone; `None` follows the system default.
    record_microphone_device: Option<String>,
    /// Description of the chosen microphone for the toolbar button.
    microphone_label: Option<String>,
    /// The sources listed while the microphone picker is open.
    microphone_picker: Option<Vec<AudioSource>>,
    record_camera: bool,
    /// Latest webcam frame from the standalone preview or the recorder.
    camera_frames: Arc<CameraFrames>,
    /// Webcam pipeline while the camera is on but nothing is recording.
    camera_preview: Option<CameraPreview>,
    camera_frame: Option<Arc<RenderImage>>,
    camera_frame_generation: u64,
    camera_poll_running: bool,
    camera_preview_expanded: bool,
    video_project: Option<RecordingSession>,
    /// Directory of the recording closed by switching to Static or Motion,
    /// so the Video tab returns to it instead of asking again.
    last_video_project: Option<PathBuf>,
    video_frame: Option<Arc<RenderImage>>,
    video_pointer_timeline: PointerTimeline,
    video_viewport_timeline: ViewportTimeline,
    video_pointer_synthesized: bool,
    video_duration: f64,
    video_source_duration: f64,
    video_position: f64,
    video_playing: bool,
    video_edit_busy: bool,
    /// Pending clip speed while the speed dialog is open; applied only on OK.
    video_speed_draft: Option<f64>,
    /// Counts preview renders so a superseded render's result is dropped
    /// while playback and seeks stay independent of in-flight renders.
    video_preview_render_generation: u64,
    video_clip_timeline: RecordingClipTimeline,
    video_selected_clip: Option<Uuid>,
    video_undo_stack: Vec<VideoEditSnapshot>,
    video_redo_stack: Vec<VideoEditSnapshot>,
    video_preview_path: Option<PathBuf>,
    video_playback_generation: Arc<AtomicU64>,
    video_seek_drag: Option<(Pixels, f64)>,
    video_trim_drag: Option<VideoTrimDrag>,
    video_move_drag: Option<VideoMoveDrag>,
    video_zoom_cues: Vec<ZoomCue>,
    video_selected_zoom_cue: Option<Uuid>,
    video_zoom_drag: Option<VideoZoomDrag>,
    video_timeline_zoom: f64,
    video_timeline_scroll: f64,
    video_timeline_bounds: Arc<Mutex<Option<Bounds<Pixels>>>>,
    video_media_bounds: Arc<Mutex<Option<Bounds<Pixels>>>>,
    /// Pixel size of the open recording's master, for scene layout.
    video_source_size: (u32, u32),
    export_format: ExportFormat,
    export_progress: Option<Arc<ExportProgress>>,
    export_label: SharedString,
    /// Screenshot motion mode: the video motion state drives a still image.
    animation_active: bool,
    animation_duration: f64,
    animation_preset: Option<MotionPreset>,
    motion_pick: MotionPick,
    background_blur: u8,
    background_noise: u8,
    vignette: u8,
    scene_transform: SceneTransform,
    watermark: Watermark,
    watermark_enabled: bool,
    watermark_editing: bool,
    pointer_style: PointerStyle,
    scene_selection: SceneSelection,
    media_drag: Option<MediaDrag>,
    /// Which motion marker (focus or pan end) the pointer is dragging.
    focus_drag: Option<MotionPick>,
    scene_canvas_bounds: Arc<Mutex<Option<Bounds<Pixels>>>>,
    preview_cache: PreviewCache,
    /// RGBA copies of what the preview shows, for the compositor.
    video_frame_rgba: Option<Arc<image::RgbaImage>>,
    capture_rgba: Option<Arc<image::RgbaImage>>,
    persisted_scene_style: Option<SceneStyle>,
    persisted_extras: Option<RecordingExtras>,
    /// Annotations last written to the recording's edit draft.
    persisted_annotations: Option<Vec<AnnotationMark>>,
    /// The screenshot editor's annotations while a recording is open.
    screenshot_annotations: AnnotationWorkspace,
    export_resolution: ExportResolution,
    export_frame_rate: f64,
    export_loop: bool,
    preset_library: PresetLibrary,
    /// 0 quick, 1 customize, 2 advanced.
    inspector_level: usize,
    default_motion_zoom: f64,
    video_audio_levels: Vec<f32>,
    video_audio_muted: bool,
    video_noise_reduction: bool,
    video_thumbnails: Vec<Arc<RenderImage>>,
    video_extras_pending: bool,
    video_extras_token: u64,
    video_press_times: Vec<f64>,
    video_removed_presses: Vec<f64>,
    video_selected_press: Option<f64>,
    annotation_drag: Option<AnnotationDrag>,
    camera_overlay: CameraOverlay,
    video_camera_path: Option<PathBuf>,
    camera_frame_rgba: Option<Arc<image::RgbaImage>>,
    camera_decoded_time: f64,
    camera_decode_token: u64,
    camera_decode_in_flight: bool,
    /// Synthetic cursor for animated screenshots.
    animation_pointer_capture: PointerCaptureFile,
    walkthrough_stops: Vec<recording::model::NormalizedPoint>,
    walkthrough_mode: bool,
    /// Animated screenshot sequence; empty while a single image is edited.
    /// The scene at `image_scene_index` is the one live in the editor.
    image_scenes: Vec<ImageScene>,
    image_scene_index: usize,
    focus_handle: FocusHandle,
    wallpaper_tab: usize,
    library_tab: usize,
    color_index: usize,
    gradient_index: usize,
    wallpaper_asset: &'static str,
    custom_wallpaper: Option<PathBuf>,
    shadow_style: usize,
    aspect_ratio: usize,
    border_color: usize,
    padding: u8,
    shadow: u8,
    corners: u8,
    border_thickness: u8,
    border_opacity: u8,
    border: bool,
    window_frame: WindowFrame,
    crop_active: bool,
    crop_rect: CropRect,
    crop_aspect: usize,
    crop_drag: Option<CropDrag>,
    crop_undo_stack: Vec<CropSnapshot>,
    crop_redo_stack: Vec<CropSnapshot>,
    inspector_visible: bool,
    background_preset: Option<usize>,
    inspector_tab: InspectorTab,
    /// Collapsible inspector sections currently open.
    open_sections: HashSet<&'static str>,
    capturing: bool,
    captured_path: Option<PathBuf>,
    processed_capture_path: Option<PathBuf>,
    displayed_capture_image: Option<Arc<RenderImage>>,
    /// Replaced GPU images awaiting `Window::drop_image`; the sprite atlas
    /// never frees a `RenderImage` on its own.
    retired_images: Vec<Arc<RenderImage>>,
    captured_dimensions: Option<(u32, u32)>,
    effect_revision: u64,
    annotations: Vec<AnnotationMark>,
    undo_stack: Vec<Vec<AnnotationMark>>,
    redo_stack: Vec<Vec<AnnotationMark>>,
    annotation_draft: Option<AnnotationMark>,
    selected_annotation: Option<usize>,
    selection_last_point: Option<Point<Pixels>>,
    selection_resizing: bool,
    pointer_is_down: bool,
    toast: Option<SharedString>,
    slider_drag: Option<SliderDrag>,
}

/// Screendrop-specific recording settings stored beside the Swift edit
/// fields in the project's edit document.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct RecordingExtras {
    audio_muted: bool,
    /// Run the export audio through a gentle FFT denoiser.
    noise_reduction: bool,
    removed_press_times: Vec<f64>,
}

#[derive(Clone, Copy)]
struct SliderDrag {
    slider_id: usize,
    start_x: Pixels,
    start_value: u8,
}

#[derive(Clone, Copy)]
enum RecordingAction {
    Pause,
    Resume,
    Restart,
    Stop,
    Discard,
}

enum PlaybackMessage {
    Frame(DecodedFrame),
    Finished,
    Error(String),
}

impl Studio {
    fn new(
        window_handle: AnyWindowHandle,
        initial_recording: Option<PathBuf>,
        initial_image: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) -> Self {
        let caret_blink_task = cx.spawn(async move |weak, cx| loop {
            Timer::after(Duration::from_millis(530)).await;
            if weak
                .update(cx, |this, cx| {
                    if this.editing_text.is_some() {
                        this.caret_visible = !this.caret_visible;
                        cx.notify();
                    } else {
                        this.caret_visible = true;
                    }
                })
                .is_err()
            {
                break;
            }
        });
        let global_shortcut_task = cx.spawn(async move |weak, cx| {
            let result: Result<(), String> = async {
                use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut};

                let app_id = ashpd::AppID::try_from("com.screendrop.Screendrop")
                    .map_err(|error| error.to_string())?;
                ashpd::register_host_app(app_id)
                    .await
                    .map_err(|error| format!("could not register Screendrop's application ID: {error}"))?;
                let portal = GlobalShortcuts::new()
                    .await
                    .map_err(|error| error.to_string())?;
                let session = portal
                    .create_session()
                    .await
                    .map_err(|error| error.to_string())?;
                let shortcuts = [NewShortcut::new(
                    "capture-screenshot",
                    "Capture a screen, window, or area",
                )
                .preferred_trigger("CTRL+SHIFT+3")];
                let request = portal
                    .bind_shortcuts(&session, &shortcuts, None)
                    .await
                    .map_err(|error| error.to_string())?;
                let bindings = request.response().map_err(|error| error.to_string())?;
                let binding = bindings.shortcuts().first().ok_or_else(|| {
                    "GNOME did not bind the requested shortcut; enable it in Settings → Keyboard → View and Customize Shortcuts"
                        .to_string()
                })?;
                let shortcut_description = binding.trigger_description().to_string();
                eprintln!("Global screenshot shortcut active: {shortcut_description}");
                let _ = weak.update(cx, |this, cx| {
                    this.toast = Some(
                        format!("Global capture shortcut active: {shortcut_description}").into(),
                    );
                    cx.notify();
                });

                let mut activated = portal
                    .receive_activated()
                    .await
                    .map_err(|error| error.to_string())?;
                while let Some(event) = activated.next().await {
                    if event.shortcut_id() != "capture-screenshot" {
                        continue;
                    }
                    let should_capture = weak
                        .update(cx, |this, cx| {
                            if this.capturing {
                                return false;
                            }
                            this.capturing = true;
                            this.toast = Some(
                                "Choose a screen, window, or area in the system picker".into(),
                            );
                            cx.notify();
                            true
                        })
                        .unwrap_or(false);
                    if !should_capture {
                        continue;
                    }

                    let capture_result = capture_behind_window(Some(window_handle), cx).await;
                    if weak
                        .update(cx, |this, cx| {
                            this.finish_capture_request(capture_result);
                            cx.notify();
                        })
                        .is_err()
                    {
                        break;
                    }
                }
                Ok(())
            }
            .await;

            if let Err(error) = result {
                eprintln!("Global shortcut unavailable: {error}");
                let _ = weak.update(cx, |this, cx| {
                    this.toast = Some(
                        format!(
                            "Global shortcut unavailable: {error}. Capture still works from the toolbar."
                        )
                        .into(),
                    );
                    cx.notify();
                });
            }
        });
        let recording_clock_task = cx.spawn(async move |weak, cx| loop {
            Timer::after(Duration::from_millis(250)).await;
            if weak
                .update(cx, |this, cx| {
                    if this.recording_state == RecordingState::Recording {
                        cx.notify();
                    }
                })
                .is_err()
            {
                break;
            }
        });
        let mut studio = Self {
            tool: Tool::Select,
            annotation_color_index: 1,
            annotation_stroke_width: 4.0,
            redaction_strength: 55,
            text_font_size: 32.0,
            text_font_family: 0,
            text_alignment: 0,
            text_bold: true,
            text_italic: false,
            text_underline: false,
            editing_text: None,
            caret_visible: true,
            _caret_blink_task: caret_blink_task,
            _global_shortcut_task: global_shortcut_task,
            _recording_clock_task: recording_clock_task,
            recording_controller: None,
            recording_state: RecordingState::Idle,
            recording_busy: false,
            recording_elapsed: Duration::ZERO,
            recording_started_at: None,
            recording_session_path: None,
            record_system_audio: false,
            record_microphone: false,
            record_microphone_device: None,
            microphone_label: None,
            microphone_picker: None,
            record_camera: false,
            camera_frames: Arc::new(CameraFrames::default()),
            camera_preview: None,
            camera_frame: None,
            camera_frame_generation: 0,
            camera_poll_running: false,
            camera_preview_expanded: false,
            video_project: None,
            last_video_project: None,
            video_frame: None,
            video_pointer_timeline: PointerTimeline::default(),
            video_viewport_timeline: ViewportTimeline::default(),
            video_pointer_synthesized: false,
            video_duration: 0.0,
            video_source_duration: 0.0,
            video_position: 0.0,
            video_playing: false,
            video_edit_busy: false,
            video_speed_draft: None,
            video_preview_render_generation: 0,
            video_clip_timeline: RecordingClipTimeline::default(),
            video_selected_clip: None,
            video_undo_stack: Vec::new(),
            video_redo_stack: Vec::new(),
            video_preview_path: None,
            video_playback_generation: Arc::new(AtomicU64::new(0)),
            video_seek_drag: None,
            video_trim_drag: None,
            video_move_drag: None,
            video_zoom_cues: Vec::new(),
            video_selected_zoom_cue: None,
            video_zoom_drag: None,
            video_timeline_zoom: 1.0,
            video_timeline_scroll: 0.0,
            video_timeline_bounds: Arc::new(Mutex::new(None)),
            video_media_bounds: Arc::new(Mutex::new(None)),
            video_source_size: (1280, 720),
            export_format: ExportFormat::Mp4,
            export_progress: None,
            export_label: SharedString::default(),
            animation_active: false,
            animation_duration: 5.0,
            animation_preset: None,
            motion_pick: MotionPick::Focus,
            background_blur: 0,
            background_noise: 0,
            vignette: 0,
            scene_transform: SceneTransform::IDENTITY,
            watermark: Watermark::default(),
            watermark_enabled: false,
            watermark_editing: false,
            pointer_style: PointerStyle::default(),
            scene_selection: SceneSelection::Scene,
            media_drag: None,
            focus_drag: None,
            scene_canvas_bounds: scene_ui::scene_canvas_bounds_store(),
            preview_cache: PreviewCache::default(),
            video_frame_rgba: None,
            capture_rgba: None,
            persisted_scene_style: None,
            persisted_extras: None,
            persisted_annotations: None,
            screenshot_annotations: AnnotationWorkspace::default(),
            export_resolution: ExportResolution::Original,
            export_frame_rate: 30.0,
            export_loop: true,
            preset_library: PresetLibrary::load(),
            inspector_level: 0,
            default_motion_zoom: 2.0,
            video_audio_levels: Vec::new(),
            video_audio_muted: false,
            video_noise_reduction: false,
            video_thumbnails: Vec::new(),
            video_extras_pending: false,
            video_extras_token: 0,
            video_press_times: Vec::new(),
            video_removed_presses: Vec::new(),
            video_selected_press: None,
            annotation_drag: None,
            camera_overlay: CameraOverlay::default(),
            video_camera_path: None,
            camera_frame_rgba: None,
            camera_decoded_time: -1.0,
            camera_decode_token: 0,
            camera_decode_in_flight: false,
            animation_pointer_capture: PointerCaptureFile::default(),
            walkthrough_stops: Vec::new(),
            walkthrough_mode: false,
            image_scenes: Vec::new(),
            image_scene_index: 0,
            focus_handle: cx.focus_handle(),
            wallpaper_tab: 2,
            library_tab: 1,
            color_index: 7,
            gradient_index: 0,
            wallpaper_asset: UIHSSN_WALLPAPERS[0],
            custom_wallpaper: None,
            shadow_style: 1,
            aspect_ratio: 0,
            border_color: 3,
            padding: 8,
            shadow: 14,
            corners: 2,
            border_thickness: 12,
            border_opacity: 30,
            border: false,
            window_frame: WindowFrame::Off,
            crop_active: false,
            crop_rect: CropRect::UNIT,
            crop_aspect: 0,
            crop_drag: None,
            crop_undo_stack: Vec::new(),
            crop_redo_stack: Vec::new(),
            inspector_visible: true,
            background_preset: Some(0),
            inspector_tab: InspectorTab::Design,
            open_sections: HashSet::from(["pointer", "camera", "audio"]),
            capturing: false,
            captured_path: None,
            processed_capture_path: None,
            displayed_capture_image: None,
            retired_images: Vec::new(),
            captured_dimensions: None,
            effect_revision: 0,
            annotations: Vec::new(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            annotation_draft: None,
            selected_annotation: None,
            selection_last_point: None,
            selection_resizing: false,
            pointer_is_down: false,
            toast: None,
            slider_drag: None,
        };
        if let Some(path) = initial_image {
            if path.is_file() {
                studio.finish_capture_request(Ok(path));
            } else {
                studio.toast = Some(format!("Could not open {}", path.display()).into());
            }
        }
        if let Some(directory) = initial_recording {
            if let Err(error) = studio.open_video_project(directory) {
                studio.toast = Some(error.into());
            }
        }
        studio
    }

    /// Queues a no-longer-shown image for release from the GPU atlas on the
    /// next render.
    pub(crate) fn retire_image(&mut self, image: Option<Arc<RenderImage>>) {
        self.retired_images.extend(image);
    }

    /// Frees every retired image's atlas tile. Must run each render, since
    /// per-frame previews would otherwise fill VRAM within minutes.
    fn drop_retired_images(&mut self, window: &mut Window) {
        for image in self.retired_images.drain(..) {
            let _ = window.drop_image(image);
        }
    }

    /// Keeps an RGBA copy of the shown video frame for the compositor.
    fn set_video_frame(&mut self, pixels: image::RgbaImage) {
        self.video_frame_rgba = Some(Arc::new(pixels.clone()));
        let previous = self.video_frame.replace(cached_render_image(pixels));
        self.retire_image(previous);
    }

    /// Keeps an RGBA copy of the shown capture for the compositor.
    fn set_capture_image(&mut self, image: image::RgbaImage) {
        self.capture_rgba = Some(Arc::new(image.clone()));
        let previous = self.displayed_capture_image.replace(cached_render_image(image));
        self.retire_image(previous);
    }

    /// The pointer capture with individually removed clicks filtered out.
    fn filtered_pointer_capture(&self) -> PointerCaptureFile {
        let mut capture = self
            .video_project
            .as_ref()
            .and_then(|session| session.read_pointer_capture().ok())
            .unwrap_or_default();
        if !self.video_removed_presses.is_empty() {
            let removed = &self.video_removed_presses;
            capture
                .presses
                .retain(|press| !removed.iter().any(|time| (press.time - *time).abs() < 1e-6));
        }
        capture
    }

    /// Whether annotations carry timing: recordings always do, screenshots
    /// only once they are animated.
    fn scene_is_timed(&self) -> bool {
        self.animation_active || self.video_project.is_some()
    }

    /// Pixel size of the media annotations are drawn on.
    fn media_dimensions(&self) -> Option<(u32, u32)> {
        if self.video_project.is_some() {
            Some(self.video_source_size)
        } else {
            self.captured_dimensions
        }
    }

    /// Installs the recording's annotations (the screenshot's were stashed
    /// when the project opened).
    fn enter_video_annotations(&mut self, marks: Vec<AnnotationMark>) {
        self.stop_editing_text();
        self.annotations = marks;
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.persisted_annotations = Some(self.annotations.clone());
        self.reset_annotation_interaction();
    }

    fn reset_annotation_interaction(&mut self) {
        self.selected_annotation = None;
        self.editing_text = None;
        self.annotation_draft = None;
        self.annotation_drag = None;
        self.selection_last_point = None;
        self.selection_resizing = false;
        self.pointer_is_down = false;
        self.tool = Tool::Select;
    }

    /// Restores the screenshot editor's annotations after a recording closes.
    fn leave_video_annotations(&mut self) {
        self.stop_editing_text();
        let workspace = std::mem::take(&mut self.screenshot_annotations);
        self.annotations = workspace.marks;
        self.undo_stack = workspace.undo;
        self.redo_stack = workspace.redo;
        self.persisted_annotations = None;
        self.reset_annotation_interaction();
    }

    fn displayed_recording_elapsed(&self) -> Duration {
        self.recording_elapsed
            + self
                .recording_started_at
                .map(|started| started.elapsed())
                .unwrap_or_default()
    }

    fn recording_timecode(&self) -> String {
        let total_seconds = self.displayed_recording_elapsed().as_secs();
        let hours = total_seconds / 3600;
        let minutes = (total_seconds % 3600) / 60;
        let seconds = total_seconds % 60;
        if hours > 0 {
            format!("{hours:02}:{minutes:02}:{seconds:02}")
        } else {
            format!("{minutes:02}:{seconds:02}")
        }
    }

    fn freeze_recording_clock(&mut self) {
        if let Some(started) = self.recording_started_at.take() {
            self.recording_elapsed += started.elapsed();
        }
    }

    /// Whether a screenshot or recording is open for editing. The live webcam
    /// only belongs on the empty studio; the editors show the recorded camera.
    fn editing_media(&self) -> bool {
        self.captured_path.is_some() || self.video_project.is_some()
    }

    /// Whether webcam frames are currently being produced for the canvas.
    fn camera_preview_live(&self) -> bool {
        !self.editing_media()
            && (self.camera_preview.is_some()
                || (self.record_camera && self.recording_state != RecordingState::Idle))
    }

    /// Runs the standalone webcam pipeline exactly when the camera toggle is
    /// on and no recording owns the device, and polls frames while any
    /// producer is live.
    fn sync_camera_preview(&mut self, cx: &mut Context<Self>) {
        let wants_standalone = self.record_camera
            && self.recording_state == RecordingState::Idle
            && !self.editing_media();
        if !wants_standalone {
            self.camera_preview = None;
        } else if self.camera_preview.is_none() {
            match default_camera_device()
                .and_then(|device| CameraPreview::start(&device, self.camera_frames.clone()))
            {
                Ok(preview) => self.camera_preview = Some(preview),
                Err(error) => {
                    self.record_camera = false;
                    self.toast = Some(format!("Webcam preview unavailable: {error}").into());
                }
            }
        }
        if !self.camera_preview_live() {
            let frame = self.camera_frame.take();
            self.retire_image(frame);
            self.camera_preview_expanded = false;
            return;
        }
        if self.camera_poll_running {
            return;
        }
        self.camera_poll_running = true;
        let frames = self.camera_frames.clone();
        cx.spawn(async move |weak, cx| loop {
            Timer::after(Duration::from_millis(33)).await;
            let keep_polling = weak.update(cx, |this, cx| {
                if !this.camera_preview_live() {
                    this.camera_poll_running = false;
                    this.camera_preview = None;
                    let frame = this.camera_frame.take();
                    this.retire_image(frame);
                    cx.notify();
                    return false;
                }
                if let Some((generation, frame)) = frames.newer_than(this.camera_frame_generation) {
                    this.camera_frame_generation = generation;
                    if let Some(pixels) =
                        image::RgbaImage::from_raw(frame.width, frame.height, frame.rgba)
                    {
                        let previous = this.camera_frame.replace(cached_render_image(pixels));
                        this.retire_image(previous);
                        cx.notify();
                    }
                }
                true
            });
            if !keep_polling.unwrap_or(false) {
                break;
            }
        })
        .detach();
    }

    fn start_recording(&mut self, cx: &mut Context<Self>) {
        if self.recording_state != RecordingState::Idle || self.recording_busy {
            return;
        }
        // Starting a new capture from an open video project must stop its
        // synchronized playback before capture becomes the active media clock.
        self.pause_video_playback();
        self.recording_busy = true;
        self.recording_state = RecordingState::Starting;
        self.recording_elapsed = Duration::ZERO;
        self.recording_started_at = None;
        self.toast = Some("Choose a screen or window to record…".into());
        let options = RecordingOptions {
            system_audio: self.record_system_audio,
            microphone: self.record_microphone,
            microphone_device: self.record_microphone_device.clone(),
            camera: self.record_camera,
        };
        // The recorder opens the webcam itself and mirrors it into the preview.
        self.camera_preview = None;
        let camera_frames = self.camera_frames.clone();
        let task = cx.background_executor().spawn(async move {
            let mut controller = RecordingController::new(
                NativeRecorder::with_options(options).with_camera_preview(camera_frames),
            );
            let result = controller
                .start()
                .map(|session| session.directory.clone())
                .map_err(|error| error.to_string());
            Ok::<_, String>((controller, result))
        });
        cx.spawn(async move |weak, cx| {
            let outcome = task.await;
            let _ = weak.update(cx, |this, cx| {
                this.recording_busy = false;
                match outcome {
                    Ok((controller, Ok(path))) => {
                        this.recording_controller = Some(controller);
                        this.recording_state = RecordingState::Recording;
                        this.recording_started_at = Some(Instant::now());
                        this.recording_session_path = Some(path);
                        this.toast = Some(
                            format!("Recording with {}", NativeRecorder::description()).into(),
                        );
                    }
                    Ok((controller, Err(error))) => {
                        this.recording_state = controller.state();
                        this.recording_controller = None;
                        this.toast = Some(format!("Recording could not start: {error}").into());
                    }
                    Err(error) => {
                        this.recording_state = RecordingState::Idle;
                        this.toast = Some(error.into());
                    }
                }
                this.sync_camera_preview(cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn run_recording_action(&mut self, action: RecordingAction, cx: &mut Context<Self>) {
        if self.recording_busy {
            return;
        }
        let Some(mut controller) = self.recording_controller.take() else {
            self.toast = Some("There is no active recording".into());
            return;
        };
        self.recording_busy = true;
        self.freeze_recording_clock();
        if matches!(action, RecordingAction::Stop | RecordingAction::Discard) {
            self.recording_state = RecordingState::Finishing;
        }
        let task = cx.background_executor().spawn(async move {
            let result = match action {
                RecordingAction::Pause => controller.pause().map(|_| None),
                RecordingAction::Resume => controller.resume().map(|_| None),
                RecordingAction::Restart => controller
                    .restart()
                    .map(|session| Some(session.directory.clone())),
                RecordingAction::Stop => controller
                    .stop_and_save()
                    .map(|session| Some(session.directory)),
                RecordingAction::Discard => controller.discard().map(|_| None),
            }
            .map_err(|error| error.to_string());
            let warnings = controller.take_warnings();
            (controller, result, warnings)
        });
        cx.spawn(async move |weak, cx| {
            let (controller, result, warnings) = task.await;
            let _ = weak.update(cx, |this, cx| {
                this.recording_busy = false;
                this.recording_state = controller.state();
                match result {
                    Ok(path) => match action {
                        RecordingAction::Pause => {
                            this.toast = Some("Recording paused".into());
                        }
                        RecordingAction::Resume => {
                            this.recording_started_at = Some(Instant::now());
                            this.toast = Some("Recording resumed".into());
                        }
                        RecordingAction::Restart => {
                            this.recording_elapsed = Duration::ZERO;
                            this.recording_started_at = Some(Instant::now());
                            this.recording_session_path = path;
                            this.toast = Some("Recording restarted".into());
                        }
                        RecordingAction::Stop => {
                            this.recording_elapsed = Duration::ZERO;
                            this.recording_started_at = None;
                            this.recording_session_path = path.clone();
                            this.toast = path.and_then(|path| {
                                match this.open_video_project(path.clone()) {
                                    Ok(()) => {
                                        let mut message =
                                            format!("Recording saved to {}", path.display());
                                        if !warnings.is_empty() {
                                            message.push_str(&format!(" — {}", warnings.join(" ")));
                                        }
                                        Some(message.into())
                                    }
                                    Err(error) => Some(
                                        format!(
                                            "Recording saved to {}, but Studio could not open it: {error}",
                                            path.display()
                                        )
                                        .into(),
                                    ),
                                }
                            });
                        }
                        RecordingAction::Discard => {
                            this.recording_elapsed = Duration::ZERO;
                            this.recording_started_at = None;
                            this.recording_session_path = None;
                            this.toast = Some("Recording discarded".into());
                        }
                    },
                    Err(error) => {
                        if controller.state() == RecordingState::Recording {
                            this.recording_started_at = Some(Instant::now());
                        }
                        this.toast = Some(format!("Recording action failed: {error}").into());
                    }
                }
                if controller.state() == RecordingState::Idle {
                    this.recording_controller = None;
                } else {
                    this.recording_controller = Some(controller);
                }
                this.sync_camera_preview(cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn request_window_close(&mut self, window_handle: AnyWindowHandle, cx: &mut Context<Self>) {
        if self.recording_state == RecordingState::Idle {
            let _ = window_handle.update(cx, |_, window, _| window.remove_window());
            return;
        }
        if self.recording_busy {
            self.toast = Some("Wait for the current recording operation to finish".into());
            cx.notify();
            return;
        }
        let Some(mut controller) = self.recording_controller.take() else {
            self.toast =
                Some("Could not safely close: the recording controller is unavailable".into());
            cx.notify();
            return;
        };
        self.recording_busy = true;
        self.recording_state = RecordingState::Finishing;
        self.freeze_recording_clock();
        self.toast = Some("Saving the recording before closing…".into());
        let task = cx.background_executor().spawn(async move {
            let result = controller
                .stop_and_save()
                .map(|session| session.directory)
                .map_err(|error| error.to_string());
            let warnings = controller.take_warnings();
            (controller, result, warnings)
        });
        cx.spawn(async move |weak, cx| {
            let (controller, result, warnings) = task.await;
            let close = result.is_ok();
            let _ = weak.update(cx, |this, cx| {
                this.recording_busy = false;
                this.recording_state = controller.state();
                match result {
                    Ok(path) => {
                        this.recording_session_path = Some(path);
                        this.recording_controller = None;
                        for warning in warnings {
                            eprintln!("Recording finalized with warning: {warning}");
                        }
                    }
                    Err(error) => {
                        this.toast = Some(
                            format!("Could not safely close; recording was preserved: {error}")
                                .into(),
                        );
                        this.recording_controller = Some(controller);
                    }
                }
                cx.notify();
            });
            if close {
                let _ = window_handle.update(cx, |_, window, _| window.remove_window());
            }
        })
        .detach();
    }

    /// Returns to the screenshot studio. Unsaved timeline edits stay in the
    /// project's draft file, so reopening the recording restores them.
    fn close_video_editor(&mut self, cx: &mut Context<Self>) {
        self.pause_video_playback();
        self.autosave_scene_style();
        self.leave_video_annotations();
        self.video_preview_render_generation += 1;
        self.video_edit_busy = false;
        self.video_speed_draft = None;
        self.last_video_project = self.video_project.take().map(|session| session.directory);
        self.sync_camera_preview(cx);
        let frame = self.video_frame.take();
        self.retire_image(frame);
        self.video_preview_path = None;
        self.video_undo_stack.clear();
        self.video_redo_stack.clear();
        self.video_selected_clip = None;
        self.video_selected_zoom_cue = None;
        self.video_camera_path = None;
        self.camera_frame_rgba = None;
        // Recording motion must not leak into a later screenshot animation.
        self.video_zoom_cues.clear();
        self.video_viewport_timeline = ViewportTimeline::default();
        self.video_pointer_timeline = PointerTimeline::default();
        self.video_duration = 0.0;
        self.video_source_duration = 0.0;
        self.video_clip_timeline = RecordingClipTimeline::default();
        self.toast = None;
        cx.notify();
    }

    fn open_video_project_dialog(&mut self, cx: &mut Context<Self>) {
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Open recording".into()),
        });
        cx.spawn(async move |weak, cx| {
            let selected = match prompt.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                _ => None,
            };
            let Some(path) = selected else {
                return;
            };
            let _ = weak.update(cx, |this, cx| {
                this.pause_video_playback();
                match this.open_video_project(path.clone()) {
                    Ok(()) => this.toast = None,
                    Err(error) => {
                        this.toast =
                            Some(format!("Could not open {}: {error}", path.display()).into());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn open_video_project(&mut self, directory: PathBuf) -> Result<(), String> {
        let session = RecordingSession { directory };
        let mut manifest = session
            .read_manifest()
            .map_err(|error| format!("Could not open recording manifest: {error}"))?;
        let media = probe_media(&session.screen_path()).ok();
        if let Some(media) = media.as_ref() {
            if manifest.pixel_width != media.width
                || manifest.pixel_height != media.height
                || (manifest.duration - media.duration).abs() > 0.001
            {
                manifest.pixel_width = media.width;
                manifest.pixel_height = media.height;
                manifest.duration = media.duration;
                session
                    .write_manifest(&manifest)
                    .map_err(|error| format!("Could not repair recording manifest: {error}"))?;
            }
        }
        // A poster is a disposable cache, never a project validity
        // requirement. Repair it or decode directly from the master.
        let poster =
            load_or_rebuild_poster(&session.screen_path(), &session.poster_path(), 1280, 720)
                .map_err(|error| format!("Could not decode recording preview: {error}"))?;
        self.video_playback_generation
            .fetch_add(1, Ordering::SeqCst);
        // Drop any preview render still running for the previous project.
        self.video_preview_render_generation += 1;
        let source_duration = media
            .as_ref()
            .map(|media| media.duration)
            .unwrap_or(manifest.duration)
            .max(0.0);
        let clip_timeline = session
            .effective_clip_timeline(source_duration)
            .map_err(|error| format!("Could not load recording edits: {error}"))?;
        let pointer_capture = session.read_pointer_capture().unwrap_or_default();
        let saved_style = session
            .read_edit_field::<SceneStyle>("scene")
            .ok()
            .flatten();
        let pointer_timeline = PointerTimeline::build_with_clip_timeline(
            pointer_capture.clone(),
            source_duration,
            manifest.pixel_width as f64,
            manifest.pixel_height as f64,
            saved_style
                .as_ref()
                .map(|style| style.pointer)
                .unwrap_or_default()
                .timeline_options(),
            Some(&clip_timeline),
        );
        let generated_zoom_cues = synthesize_zoom_cues(&pointer_capture, source_duration);
        let zoom_cues = session
            .effective_zoom_cues()
            .map_err(|error| format!("Could not load zoom edits: {error}"))?
            .unwrap_or(generated_zoom_cues);
        let viewport_timeline = ViewportTimeline::build(
            &zoom_cues,
            &pointer_timeline,
            &clip_timeline,
            &pointer_capture,
        );
        let saved_extras = session
            .read_edit_field::<RecordingExtras>("screendropExtras")
            .ok()
            .flatten();
        let preview_path = session.directory.join(".edit-preview.mkv");
        let edited_preview = if clip_timeline.is_unedited(source_duration) {
            None
        } else {
            let noise_reduction = saved_extras
                .as_ref()
                .is_some_and(|extras| extras.noise_reduction);
            let source = Self::media_source_for(&session, noise_reduction);
            render_clip_preview(&source, &preview_path, &clip_timeline)
                .map_err(|error| format!("Could not build edited preview: {error}"))?;
            Some(preview_path)
        };
        if self.animation_active {
            self.exit_animation();
        }
        self.video_source_size = (manifest.pixel_width.max(1), manifest.pixel_height.max(1));
        self.motion_pick = MotionPick::Focus;
        if self.video_project.is_none() {
            self.screenshot_annotations = AnnotationWorkspace {
                marks: std::mem::take(&mut self.annotations),
                undo: std::mem::take(&mut self.undo_stack),
                redo: std::mem::take(&mut self.redo_stack),
            };
        }
        self.video_project = Some(session);
        self.set_video_frame(poster);
        self.video_pointer_timeline = pointer_timeline;
        self.video_viewport_timeline = viewport_timeline;
        self.video_pointer_synthesized = manifest.pointer_synthesized;
        self.video_source_duration = source_duration;
        self.video_duration = clip_timeline.duration();
        self.video_position = 0.0;
        self.video_playing = false;
        self.video_edit_busy = false;
        self.video_selected_clip = clip_timeline.segments.first().map(|clip| clip.id);
        self.video_clip_timeline = clip_timeline;
        self.video_undo_stack.clear();
        self.video_redo_stack.clear();
        self.video_preview_path = edited_preview;
        self.video_seek_drag = None;
        self.video_trim_drag = None;
        self.video_move_drag = None;
        self.video_zoom_cues = zoom_cues;
        self.video_selected_zoom_cue = None;
        self.video_zoom_drag = None;
        self.video_timeline_zoom = 1.0;
        self.video_timeline_scroll = 0.0;
        // Scene settings and Screendrop extras saved with this project.
        let session = self.video_project.clone().expect("project was just opened");
        let saved_annotations = session
            .read_edit_field::<Vec<AnnotationMark>>("annotations")
            .ok()
            .flatten()
            .unwrap_or_default();
        self.video_press_times = pointer_capture
            .presses
            .iter()
            .filter(|press| press.phase == recording::model::PressPhase::Down)
            .map(|press| press.time)
            .collect();
        if let Some(style) = saved_style.as_ref() {
            self.apply_scene_style(style);
        }
        self.persisted_scene_style = saved_style;
        let extras = saved_extras.clone().unwrap_or_default();
        self.video_audio_muted = extras.audio_muted;
        self.video_noise_reduction = extras.noise_reduction;
        self.video_removed_presses = extras.removed_press_times;
        self.persisted_extras = saved_extras;
        self.enter_video_annotations(saved_annotations);
        self.video_selected_press = None;
        self.video_audio_levels.clear();
        let thumbnails = self.video_thumbnails.drain(..).collect::<Vec<_>>();
        self.retired_images.extend(thumbnails);
        self.video_extras_pending = true;
        let camera_path = session.camera_path();
        self.video_camera_path = camera_path.is_file().then_some(camera_path);
        self.camera_frame_rgba = None;
        self.camera_decoded_time = -1.0;
        self.scene_selection = SceneSelection::Scene;
        self.media_drag = None;
        let previous = std::mem::take(&mut self.preview_cache).frame;
        self.retire_image(previous.map(|(_, image)| image));
        if !self.video_removed_presses.is_empty() {
            self.rebuild_video_motion_timelines();
        }
        Ok(())
    }

    /// Width of the visible timeline strip, measured on the last paint. The
    /// strip stretches with the window, so seek and drag math must use the
    /// same width the clips were laid out with.
    fn video_timeline_viewport_width(&self) -> f64 {
        self.video_timeline_bounds
            .lock()
            .ok()
            .and_then(|bounds| *bounds)
            .map(|bounds| (bounds.size.width / px(1.0)) as f64)
            .filter(|width| *width > 1.0)
            .unwrap_or(600.0)
    }

    fn zoom_video_timeline(&mut self, factor: f64, anchor_time: f64) {
        const MAX_POINTS_PER_SECOND: f64 = 240.0;
        const MAX_CONTENT_WIDTH: f64 = 100_000.0;
        if self.video_duration <= 0.0 || !factor.is_finite() || factor <= 0.0 {
            return;
        }
        let viewport_width = self.video_timeline_viewport_width();
        let maximum_width = MAX_CONTENT_WIDTH.min(MAX_POINTS_PER_SECOND * self.video_duration);
        let maximum_zoom = (maximum_width / viewport_width).max(1.0);
        let previous_zoom = self.video_timeline_zoom;
        let next_zoom = (previous_zoom * factor).clamp(1.0, maximum_zoom);
        if (next_zoom - previous_zoom).abs() < 0.000_1 {
            return;
        }
        let anchor_fraction = (anchor_time / self.video_duration).clamp(0.0, 1.0);
        let previous_anchor_x = anchor_fraction * viewport_width * previous_zoom;
        let anchor_viewport_x = previous_anchor_x - self.video_timeline_scroll;
        self.video_timeline_zoom = next_zoom;
        let next_anchor_x = anchor_fraction * viewport_width * next_zoom;
        let maximum_scroll = viewport_width * next_zoom - viewport_width;
        self.video_timeline_scroll = (next_anchor_x - anchor_viewport_x).clamp(0.0, maximum_scroll);
    }

    fn pan_video_timeline(&mut self, delta: f64) {
        let viewport_width = self.video_timeline_viewport_width();
        let maximum_scroll = (viewport_width * self.video_timeline_zoom - viewport_width).max(0.0);
        self.video_timeline_scroll =
            (self.video_timeline_scroll + delta).clamp(0.0, maximum_scroll);
    }

    fn begin_video_trim(&mut self, clip_id: Uuid, edge: ClipEdge, start_x: Pixels) {
        if self.video_edit_busy {
            return;
        }
        let Some(original_clip) = self
            .video_clip_timeline
            .segments
            .iter()
            .find(|clip| clip.id == clip_id)
            .cloned()
        else {
            return;
        };
        self.pause_video_playback();
        self.video_selected_clip = Some(clip_id);
        self.video_trim_drag = Some(VideoTrimDrag {
            start_x,
            original_timeline: self.video_clip_timeline.clone(),
            original_clip,
            edge,
            editor_seconds_per_pixel: self.video_duration
                / (self.video_timeline_viewport_width() * self.video_timeline_zoom).max(1.0),
        });
    }

    fn update_video_trim(&mut self, pointer_x: Pixels) {
        let Some(drag) = self.video_trim_drag.as_ref() else {
            return;
        };
        let editor_delta =
            ((pointer_x - drag.start_x) / px(1.0)) as f64 * drag.editor_seconds_per_pixel;
        let Some((timeline, _)) = drag.original_timeline.trimming(
            drag.original_clip.id,
            drag.edge,
            editor_delta,
            self.video_source_duration,
        ) else {
            return;
        };
        self.video_clip_timeline = timeline;
        self.video_duration = self.video_clip_timeline.duration();
        self.video_position = self.video_position.min(self.video_duration);
    }

    fn commit_video_trim(&mut self, cx: &mut Context<Self>) {
        let Some(drag) = self.video_trim_drag.take() else {
            return;
        };
        let requested = self.video_clip_timeline.clone();
        let selected = Some(drag.original_clip.id);
        self.video_clip_timeline = drag.original_timeline;
        self.video_duration = self.video_clip_timeline.duration();
        self.apply_video_clip_timeline(requested, selected, true, cx);
    }

    /// Where the dragged clip's content would start in editor time, given
    /// the drag's pixel displacement. The scale is frozen at the drag's
    /// current duration so the conversion is stable throughout the gesture.
    fn video_move_new_start(&self, drag: &VideoMoveDrag) -> Option<f64> {
        let range = self.video_clip_timeline.editor_range(drag.clip_id)?;
        let content_width = self.video_timeline_viewport_width() * self.video_timeline_zoom;
        if content_width <= 0.0 || self.video_duration <= 0.0 {
            return None;
        }
        let seconds_per_pixel = self.video_duration / content_width;
        let delta = ((drag.current_x - drag.start_x) / px(1.0)) as f64 * seconds_per_pixel;
        // Allow extending past the end by up to the current duration, and
        // snap to clip boundaries so gaps close seamlessly.
        let mut new_start = (range.start + delta).clamp(0.0, self.video_duration);
        let clip_length = range.end - range.start;
        let snap = 8.0 * seconds_per_pixel;
        let starts = self.video_clip_timeline.clip_starts();
        let mut candidates = vec![0.0];
        for (index, segment) in self.video_clip_timeline.segments.iter().enumerate() {
            if segment.id == drag.clip_id {
                continue;
            }
            // Snap this clip's head to a neighbor's tail, or its tail to a
            // neighbor's head.
            candidates.push(starts[index] + segment.editor_duration());
            candidates.push(starts[index] - clip_length);
        }
        if let Some(best) = candidates
            .into_iter()
            .filter(|candidate| *candidate >= 0.0 && (new_start - candidate).abs() < snap)
            .min_by(|left, right| {
                (new_start - left)
                    .abs()
                    .total_cmp(&(new_start - right).abs())
            })
        {
            new_start = best;
        }
        Some(new_start)
    }

    fn commit_video_move_drag(&mut self, drag: VideoMoveDrag, cx: &mut Context<Self>) {
        let Some(new_start) = self.video_move_new_start(&drag) else {
            return;
        };
        if let Some(timeline) = self
            .video_clip_timeline
            .repositioning(drag.clip_id, new_start)
        {
            self.apply_video_clip_timeline(timeline, Some(drag.clip_id), true, cx);
        }
    }

    fn begin_video_zoom_drag(
        &mut self,
        cue_id: Uuid,
        kind: VideoZoomDragKind,
        editor_start: f64,
        editor_end: f64,
        start_x: Pixels,
    ) {
        if self.video_edit_busy {
            return;
        }
        let Some(cue) = self
            .video_zoom_cues
            .iter()
            .find(|cue| cue.id == cue_id)
            .cloned()
        else {
            return;
        };
        self.pause_video_playback();
        self.video_selected_zoom_cue = Some(cue_id);
        self.video_selected_clip = None;
        self.video_seek_drag = None;
        self.video_zoom_drag = Some(VideoZoomDrag {
            start_x,
            original_cues: self.video_zoom_cues.clone(),
            original_cue: cue,
            kind,
            editor_start,
            editor_end,
            editor_seconds_per_pixel: self.video_duration
                / (self.video_timeline_viewport_width() * self.video_timeline_zoom).max(1.0),
        });
    }

    fn update_video_zoom_drag(&mut self, pointer_x: Pixels) {
        let Some(drag) = self.video_zoom_drag.as_ref().cloned() else {
            return;
        };
        let editor_delta =
            ((pointer_x - drag.start_x) / px(1.0)) as f64 * drag.editor_seconds_per_pixel;
        let mut cue = drag.original_cue.clone();
        match drag.kind {
            VideoZoomDragKind::Move => {
                let editor_duration = (drag.editor_end - drag.editor_start).max(0.0);
                let new_editor_start = (drag.editor_start + editor_delta)
                    .clamp(0.0, (self.video_duration - editor_duration).max(0.0));
                let source_start = self.video_clip_timeline.source_time_at(new_editor_start);
                let source_duration = drag.original_cue.end - drag.original_cue.start;
                cue.start = source_start.clamp(0.0, self.video_source_duration);
                cue.end = (cue.start + source_duration).min(self.video_source_duration);
                if cue.end - cue.start < ZoomCue::MINIMUM_DURATION {
                    cue.start = (cue.end - ZoomCue::MINIMUM_DURATION).max(0.0);
                }
            }
            VideoZoomDragKind::Leading => {
                let editor_time =
                    (drag.editor_start + editor_delta).clamp(0.0, drag.editor_end - f64::EPSILON);
                cue.start = self
                    .video_clip_timeline
                    .source_time_at(editor_time)
                    .clamp(0.0, cue.end - ZoomCue::MINIMUM_DURATION);
            }
            VideoZoomDragKind::Trailing => {
                let editor_time = (drag.editor_end + editor_delta)
                    .clamp(drag.editor_start + f64::EPSILON, self.video_duration);
                cue.end = self.video_clip_timeline.source_time_at(editor_time).clamp(
                    cue.start + ZoomCue::MINIMUM_DURATION,
                    self.video_source_duration,
                );
            }
        }
        self.video_zoom_cues = drag.original_cues;
        if let Some(current) = self
            .video_zoom_cues
            .iter_mut()
            .find(|current| current.id == cue.id)
        {
            *current = cue;
        }
        self.video_zoom_cues
            .sort_by(|left, right| left.start.total_cmp(&right.start));
        self.rebuild_video_motion_timelines();
    }

    fn commit_video_zoom_drag(&mut self, cx: &mut Context<Self>) {
        let Some(drag) = self.video_zoom_drag.take() else {
            return;
        };
        if self.video_zoom_cues == drag.original_cues {
            return;
        }
        self.video_undo_stack
            .push(VideoEditSnapshot::Zoom(drag.original_cues));
        self.video_redo_stack.clear();
        self.persist_video_zoom_cues(cx);
    }

    fn persist_video_zoom_cues(&mut self, cx: &mut Context<Self>) {
        self.persist_video_zoom_cues_quiet();
        cx.notify();
    }

    /// Autosaves the motion lane; screenshot animations have no project
    /// package yet and simply keep their regions in memory.
    fn persist_video_zoom_cues_quiet(&mut self) {
        let Some(session) = self.video_project.as_ref() else {
            return;
        };
        if let Err(error) = session.write_zoom_cues_draft(&self.video_zoom_cues) {
            self.toast = Some(format!("Could not autosave motion edit: {error}").into());
        }
    }

    fn delete_selected_video_zoom(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(selected) = self.video_selected_zoom_cue else {
            return false;
        };
        let original = self.video_zoom_cues.clone();
        self.video_zoom_cues.retain(|cue| cue.id != selected);
        if self.video_zoom_cues == original {
            return false;
        }
        self.video_undo_stack
            .push(VideoEditSnapshot::Zoom(original));
        self.video_redo_stack.clear();
        self.video_selected_zoom_cue = None;
        self.rebuild_video_motion_timelines();
        self.persist_video_zoom_cues(cx);
        true
    }

    fn mutate_selected_zoom_cue(
        &mut self,
        cx: &mut Context<Self>,
        mutate: impl FnOnce(&mut ZoomCue),
    ) {
        self.edit_selected_region(mutate);
        cx.notify();
    }

    fn add_video_zoom_at_playhead(&mut self, cx: &mut Context<Self>) {
        let position = self.video_position;
        self.add_motion_region_at(position, cx);
    }

    fn undo_video_edit(&mut self, cx: &mut Context<Self>) {
        let Some(previous) = self.video_undo_stack.pop() else {
            return;
        };
        match previous {
            VideoEditSnapshot::Clips(timeline) => {
                self.video_redo_stack
                    .push(VideoEditSnapshot::Clips(self.video_clip_timeline.clone()));
                let selected = timeline.segments.first().map(|clip| clip.id);
                self.apply_video_clip_timeline(timeline, selected, false, cx);
            }
            VideoEditSnapshot::Zoom(cues) => {
                self.video_redo_stack
                    .push(VideoEditSnapshot::Zoom(self.video_zoom_cues.clone()));
                self.video_zoom_cues = cues;
                self.video_selected_zoom_cue = None;
                self.rebuild_video_motion_timelines();
                self.persist_video_zoom_cues(cx);
            }
        }
    }

    fn redo_video_edit(&mut self, cx: &mut Context<Self>) {
        let Some(next) = self.video_redo_stack.pop() else {
            return;
        };
        match next {
            VideoEditSnapshot::Clips(timeline) => {
                self.video_undo_stack
                    .push(VideoEditSnapshot::Clips(self.video_clip_timeline.clone()));
                let selected = timeline.segments.first().map(|clip| clip.id);
                self.apply_video_clip_timeline(timeline, selected, false, cx);
            }
            VideoEditSnapshot::Zoom(cues) => {
                self.video_undo_stack
                    .push(VideoEditSnapshot::Zoom(self.video_zoom_cues.clone()));
                self.video_zoom_cues = cues;
                self.video_selected_zoom_cue = None;
                self.rebuild_video_motion_timelines();
                self.persist_video_zoom_cues(cx);
            }
        }
    }

    fn delete_selected_video_edit(&mut self, cx: &mut Context<Self>) {
        if !self.delete_selected_video_zoom(cx) {
            self.delete_selected_video_clip(cx);
        }
    }

    fn video_playback_path(&self) -> Option<PathBuf> {
        self.video_preview_path
            .clone()
            .or_else(|| self.video_media_source())
    }

    /// The recording the editor plays and cuts: the noise-reduced copy when
    /// that option is on and its render has finished, else the original.
    fn video_media_source(&self) -> Option<PathBuf> {
        self.video_project
            .as_ref()
            .map(|session| Self::media_source_for(session, self.video_noise_reduction))
    }

    fn media_source_for(session: &RecordingSession, noise_reduction: bool) -> PathBuf {
        let denoised = session.denoised_path();
        if noise_reduction && denoised.exists() {
            denoised
        } else {
            session.screen_path()
        }
    }

    /// Flips noise reduction, rendering the denoised copy on first use and
    /// refreshing whatever the editor is playing once it is ready.
    fn set_video_noise_reduction(&mut self, enabled: bool, cx: &mut Context<Self>) {
        self.video_noise_reduction = enabled;
        let Some(session) = self.video_project.clone() else {
            return;
        };
        self.pause_video_playback();
        if !enabled || session.denoised_path().exists() {
            self.refresh_video_media_source(cx);
            return;
        }
        self.video_edit_busy = true;
        self.toast = Some("Preparing noise-reduced audio…".into());
        let source = session.screen_path();
        let destination = session.denoised_path();
        let task = cx.background_executor().spawn(async move {
            render_denoised_copy(&source, &destination).map_err(|error| error.to_string())
        });
        cx.spawn(async move |weak, cx| {
            let result = task.await;
            let _ = weak.update(cx, |this, cx| {
                this.video_edit_busy = false;
                this.toast = None;
                match result {
                    Ok(()) => {
                        if this.video_project.as_ref() == Some(&session) {
                            this.refresh_video_media_source(cx);
                        }
                    }
                    Err(error) => {
                        this.video_noise_reduction = false;
                        this.toast = Some(format!("Could not reduce noise: {error}").into());
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Re-renders the edited preview from the current media source, or just
    /// re-seeks when the recording plays uncut.
    fn refresh_video_media_source(&mut self, cx: &mut Context<Self>) {
        if self.video_preview_path.is_some() {
            let timeline = self.video_clip_timeline.clone();
            self.apply_video_clip_timeline(timeline, self.video_selected_clip, false, cx);
        } else {
            self.seek_video(self.video_position, cx);
        }
    }

    fn rebuild_video_motion_timelines(&mut self) {
        let Some(session) = self.video_project.as_ref() else {
            if self.animation_active {
                if self.animation_pointer_capture.presses.is_empty() {
                    self.video_viewport_timeline =
                        ViewportTimeline::build_static(&self.video_zoom_cues, self.video_duration);
                } else {
                    let (width, height) = self.captured_dimensions.unwrap_or((1200, 720));
                    let clips = RecordingClipTimeline::full(self.video_duration);
                    let pointer = PointerTimeline::build_with_clip_timeline(
                        self.animation_pointer_capture.clone(),
                        self.video_duration,
                        width as f64,
                        height as f64,
                        self.pointer_style.timeline_options(),
                        Some(&clips),
                    );
                    self.video_viewport_timeline = ViewportTimeline::build(
                        &self.video_zoom_cues,
                        &pointer,
                        &clips,
                        &self.animation_pointer_capture,
                    );
                    self.video_pointer_timeline = pointer;
                }
            }
            return;
        };
        let manifest = session.read_manifest().unwrap_or_default();
        let capture = self.filtered_pointer_capture();
        let pointer = PointerTimeline::build_with_clip_timeline(
            capture.clone(),
            self.video_source_duration,
            manifest.pixel_width as f64,
            manifest.pixel_height as f64,
            self.pointer_style.timeline_options(),
            Some(&self.video_clip_timeline),
        );
        self.video_viewport_timeline = ViewportTimeline::build(
            &self.video_zoom_cues,
            &pointer,
            &self.video_clip_timeline,
            &capture,
        );
        self.video_pointer_timeline = pointer;
    }

    fn apply_video_clip_timeline(
        &mut self,
        timeline: RecordingClipTimeline,
        selected: Option<Uuid>,
        push_undo: bool,
        cx: &mut Context<Self>,
    ) {
        let timeline = timeline.normalized(self.video_source_duration);
        if timeline == self.video_clip_timeline || timeline.segments.is_empty() {
            return;
        }
        if push_undo {
            self.video_undo_stack
                .push(VideoEditSnapshot::Clips(self.video_clip_timeline.clone()));
            self.video_redo_stack.clear();
        }
        self.pause_video_playback();
        self.video_position = self.video_position.min(timeline.duration());
        self.video_duration = timeline.duration();
        self.video_selected_clip = selected
            .filter(|id| timeline.segments.iter().any(|clip| clip.id == *id))
            .or_else(|| timeline.segments.first().map(|clip| clip.id));
        self.video_clip_timeline = timeline.clone();
        self.rebuild_video_motion_timelines();

        let Some(session) = self.video_project.clone() else {
            return;
        };
        if let Err(error) = session.write_clip_timeline_draft(&timeline) {
            self.toast = Some(format!("Could not autosave clip edit: {error}").into());
            cx.notify();
            return;
        }
        if timeline.is_unedited(self.video_source_duration) {
            self.video_preview_path = None;
            self.video_edit_busy = false;
            self.seek_video(self.video_position, cx);
            return;
        }

        self.video_edit_busy = true;
        let previous_preview = self.video_preview_path.take();
        self.toast = Some("Updating video and audio preview…".into());
        let source = Self::media_source_for(&session, self.video_noise_reduction);
        self.video_preview_render_generation += 1;
        let token = self.video_preview_render_generation;
        let destination = session.directory.join(format!(".edit-preview-{token}.mkv"));
        let task = cx.background_executor().spawn(async move {
            render_clip_preview(&source, &destination, &timeline)
                .map(|_| destination)
                .map_err(|error| error.to_string())
        });
        cx.spawn(async move |weak, cx| {
            let result = task.await;
            let _ = weak.update(cx, |this, cx| {
                if this.video_preview_render_generation != token {
                    if let Ok(path) = result {
                        let _ = fs::remove_file(path);
                    }
                    return;
                }
                this.video_edit_busy = false;
                match result {
                    Ok(path) => {
                        this.video_preview_path = Some(path);
                        if let Some(previous) = previous_preview {
                            let _ = fs::remove_file(previous);
                        }
                        this.toast = None;
                        this.seek_video(this.video_position, cx);
                    }
                    Err(error) => {
                        this.toast =
                            Some(format!("Could not update edited preview: {error}").into());
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn split_video_clip(&mut self, cx: &mut Context<Self>) {
        if let Some((timeline, selected)) = self.video_clip_timeline.split_at(self.video_position) {
            self.apply_video_clip_timeline(timeline, Some(selected), true, cx);
        }
    }

    fn delete_selected_video_clip(&mut self, cx: &mut Context<Self>) {
        let Some(selected) = self.video_selected_clip else {
            return;
        };
        let Some(range) = self.video_clip_timeline.editor_range(selected) else {
            return;
        };
        let Some(timeline) = self.video_clip_timeline.deleting(selected) else {
            return;
        };
        self.video_position = range.start.min(timeline.duration());
        let next_selected = timeline
            .location_at(self.video_position)
            .map(|location| location.segment_id)
            .or_else(|| timeline.segments.last().map(|clip| clip.id));
        self.apply_video_clip_timeline(timeline, next_selected, true, cx);
    }

    /// Preset playback-rate steps between the slow-motion floor and the
    /// fast-forward ceiling, denser around 1× where fine control matters.
    const SPEED_LADDER: [f64; 16] = [
        0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0, 4.0, 5.0, 6.0, 8.0, 10.0, 12.0, 16.0,
    ];

    fn next_clip_speed(speed: f64, increase: bool) -> f64 {
        if increase {
            Self::SPEED_LADDER
                .iter()
                .copied()
                .find(|step| *step > speed + 0.001)
                .unwrap_or(RecordingClipSegment::MAXIMUM_SPEED)
        } else {
            Self::SPEED_LADDER
                .iter()
                .rev()
                .copied()
                .find(|step| *step < speed - 0.001)
                .unwrap_or(RecordingClipSegment::MINIMUM_SPEED)
        }
    }

    fn set_selected_video_clip_speed(&mut self, speed: f64, cx: &mut Context<Self>) {
        let Some(selected) = self.video_selected_clip else {
            return;
        };
        let Some(mut clip) = self
            .video_clip_timeline
            .segments
            .iter()
            .find(|clip| clip.id == selected)
            .cloned()
        else {
            return;
        };
        let speed = speed.clamp(
            RecordingClipSegment::MINIMUM_SPEED,
            RecordingClipSegment::MAXIMUM_SPEED,
        );
        if (clip.speed - speed).abs() < 0.001 {
            return;
        }
        clip.speed = speed;
        let timeline = self.video_clip_timeline.replacing(clip);
        self.apply_video_clip_timeline(timeline, Some(selected), true, cx);
    }

    fn start_video_playback(&mut self, cx: &mut Context<Self>) {
        if self.video_playing || self.video_duration <= 0.0 || self.video_edit_busy {
            return;
        }
        let Some(path) = self.video_playback_path() else {
            return;
        };
        if self.video_position >= self.video_duration - 0.01 {
            self.video_position = 0.0;
        }
        let start_time = self.video_position;
        let generation = self.video_playback_generation.clone();
        let token = generation.fetch_add(1, Ordering::SeqCst) + 1;
        let (sender, receiver) = mpsc::sync_channel(2);
        self.video_playing = true;
        self.toast = None;

        cx.background_executor()
            .spawn(async move {
                let mut stream =
                    match SynchronizedPlaybackStream::open(&path, start_time, 1920, 1080) {
                        Ok(stream) => stream,
                        Err(error) => {
                            let _ = sender.send(PlaybackMessage::Error(error.to_string()));
                            return;
                        }
                    };
                while generation.load(Ordering::SeqCst) == token {
                    match stream.next_frame() {
                        Ok(Some(frame)) => {
                            if sender.send(PlaybackMessage::Frame(frame)).is_err() {
                                break;
                            }
                        }
                        Ok(None) => {
                            let _ = sender.send(PlaybackMessage::Finished);
                            break;
                        }
                        Err(error) => {
                            let _ = sender.send(PlaybackMessage::Error(error.to_string()));
                            break;
                        }
                    }
                }
                stream.stop();
            })
            .detach();

        let active_generation = self.video_playback_generation.clone();
        cx.spawn(async move |weak, cx| loop {
            Timer::after(Duration::from_millis(8)).await;
            if active_generation.load(Ordering::SeqCst) != token {
                break;
            }
            let mut latest_frame = None;
            let mut terminal = None;
            while let Ok(message) = receiver.try_recv() {
                match message {
                    PlaybackMessage::Frame(frame) => latest_frame = Some(frame),
                    PlaybackMessage::Finished => terminal = Some(Ok(())),
                    PlaybackMessage::Error(error) => terminal = Some(Err(error)),
                }
            }
            let terminal_received = terminal.is_some();
            if weak
                .update(cx, |this, cx| {
                    if let Some(frame) = latest_frame {
                        this.video_position = frame.time.min(this.video_duration);
                        if let Some(pixels) =
                            image::RgbaImage::from_raw(frame.width, frame.height, frame.rgba)
                        {
                            this.set_video_frame(pixels);
                        }
                    }
                    if let Some(result) = terminal {
                        this.video_playing = false;
                        if let Err(error) = result {
                            this.toast = Some(format!("Playback failed: {error}").into());
                        } else {
                            this.video_position = this.video_duration;
                        }
                    }
                    cx.notify();
                })
                .is_err()
                || terminal_received
            {
                break;
            }
        })
        .detach();
    }

    fn pause_video_playback(&mut self) {
        self.video_playback_generation
            .fetch_add(1, Ordering::SeqCst);
        self.video_playing = false;
    }

    fn seek_video(&mut self, position: f64, cx: &mut Context<Self>) {
        let playback_path = self.video_playback_path();
        self.pause_video_playback();
        let position = position.clamp(0.0, self.video_duration);
        self.video_position = position;
        let Some(path) = playback_path else {
            cx.notify();
            return;
        };
        let generation = self.video_playback_generation.clone();
        let token = generation.fetch_add(1, Ordering::SeqCst) + 1;
        let task = cx.background_executor().spawn(async move {
            decode_frame(&path, position, 2560, 1440).map_err(|error| error.to_string())
        });
        cx.spawn(async move |weak, cx| {
            let result = task.await;
            if generation.load(Ordering::SeqCst) != token {
                return;
            }
            let _ = weak.update(cx, |this, cx| {
                match result {
                    Ok(frame) => {
                        if let Some(pixels) =
                            image::RgbaImage::from_raw(frame.width, frame.height, frame.rgba)
                        {
                            this.set_video_frame(pixels);
                        }
                    }
                    Err(error) => {
                        this.toast = Some(format!("Could not seek: {error}").into());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn video_timecode(value: f64) -> String {
        let seconds = value.max(0.0).floor() as u64;
        format!("{:02}:{:02}", seconds / 60, seconds % 60)
    }

    /// Modal that previews a clip speed change before rendering it once.
    /// The transparent layer that closes the microphone menu on an outside click.
    fn microphone_menu_backdrop(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        self.microphone_picker.as_ref()?;
        Some(
            div()
                .id("record-microphone-backdrop")
                .absolute()
                .inset_0()
                .occlude()
                .on_mouse_down(MouseButton::Left, |_, _, _| {})
                .on_click(cx.listener(|this, _, _, cx| {
                    this.microphone_picker = None;
                    cx.notify();
                }))
                .into_any_element(),
        )
    }

    /// The dropdown under the mic button: off, the system default, or a device.
    fn microphone_menu(&self, sources: &[AudioSource], cx: &mut Context<Self>) -> AnyElement {
        // `None` for the whole option is "no microphone".
        let mut choices: Vec<(Option<Option<String>>, String)> = vec![
            (None, "No microphone".to_string()),
            (
                Some(None),
                sources
                    .iter()
                    .find(|source| source.is_default)
                    .map(|source| format!("System default · {}", source.description))
                    .unwrap_or_else(|| "System default".to_string()),
            ),
        ];
        choices.extend(
            sources
                .iter()
                .map(|source| (Some(Some(source.name.clone())), source.description.clone())),
        );
        let current = self
            .record_microphone
            .then(|| self.record_microphone_device.clone());
        div()
            .id("record-microphone-menu")
            .occlude()
            .min_w(px(260.0))
            .p_1()
            .flex()
            .flex_col()
            .rounded_lg()
            .bg(rgb(0xffffff))
            .border_1()
            .border_color(line())
            .shadow_lg()
            .on_click(|_, _, cx| cx.stop_propagation())
            .children(
                choices
                    .into_iter()
                    .enumerate()
                    .map(|(index, (choice, label))| {
                        let selected = choice == current;
                        let chosen_label = label.clone();
                        div()
                            .id(("record-microphone-option", index))
                            .px_3()
                            .h(px(32.0))
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_3()
                            .rounded_md()
                            .text_sm()
                            .whitespace_nowrap()
                            .cursor_pointer()
                            .when(selected, |this| this.text_color(blue()))
                            .hover(|style| style.bg(rgb(0xeeeeef)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                match &choice {
                                    None => this.record_microphone = false,
                                    Some(device) => {
                                        this.record_microphone = true;
                                        this.record_microphone_device = device.clone();
                                        this.microphone_label =
                                            device.is_some().then(|| chosen_label.clone());
                                    }
                                }
                                this.microphone_picker = None;
                                cx.notify();
                            }))
                            .child(label)
                            .when(selected, |this| this.child("✓"))
                    }),
            )
            .into_any_element()
    }

    fn video_speed_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let draft = self.video_speed_draft?;
        let selected = self.video_selected_clip?;
        let clip = self
            .video_clip_timeline
            .segments
            .iter()
            .find(|clip| clip.id == selected)?
            .clone();
        let current_speed = clip.speed;
        let mut changed = clip.clone();
        changed.speed = draft;
        let new_timeline = self.video_clip_timeline.replacing(changed.clone());
        let old_end = self.video_clip_timeline.duration();
        let new_end = new_timeline.duration();
        let seconds = |value: f64| format!("{value:.1}s");
        let row = |label: &'static str, before: String, after: String| {
            div()
                .flex()
                .justify_between()
                .text_sm()
                .child(div().text_color(muted()).child(label))
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(div().text_color(muted()).child(before))
                        .child("→")
                        .child(div().font_weight(FontWeight::SEMIBOLD).child(after)),
                )
        };
        let step = |id: &'static str, glyph: &'static str, enabled: bool, increase: bool| {
            div()
                .id(id)
                .size(px(32.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded_md()
                .bg(rgb(0xf3f3f4))
                .opacity(if enabled { 1.0 } else { 0.35 })
                .when(enabled, |this| {
                    this.cursor_pointer()
                        .hover(|style| style.bg(rgb(0xe4e4e7)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(draft) = this.video_speed_draft {
                                this.video_speed_draft =
                                    Some(Self::next_clip_speed(draft, increase));
                                cx.notify();
                            }
                        }))
                })
                .child(glyph)
        };
        let button = |id: &'static str, label: &'static str, primary: bool| {
            div()
                .id(id)
                .px_4()
                .h(px(32.0))
                .flex()
                .items_center()
                .rounded_md()
                .text_sm()
                .cursor_pointer()
                .when(primary, |this| {
                    this.bg(rgb(0x2563eb))
                        .text_color(rgb(0xffffff))
                        .hover(|style| style.bg(rgb(0x1d4ed8)))
                })
                .when(!primary, |this| this.hover(|style| style.bg(rgb(0xeeeeef))))
                .child(label)
        };
        let unchanged = (draft - current_speed).abs() < 0.001;
        Some(
            div()
                .id("video-speed-dialog-backdrop")
                .absolute()
                .inset_0()
                .occlude()
                .flex()
                .items_center()
                .justify_center()
                .bg(hsla(0.0, 0.0, 0.0, 0.25))
                .on_mouse_down(MouseButton::Left, |_, _, _| {})
                .on_click(cx.listener(|this, _, _, cx| {
                    this.video_speed_draft = None;
                    cx.notify();
                }))
                .child(
                    div()
                        .id("video-speed-dialog")
                        .occlude()
                        .w(px(320.0))
                        .p_4()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .rounded_lg()
                        .bg(rgb(0xffffff))
                        .shadow_lg()
                        .on_click(|_, _, cx| cx.stop_propagation())
                        .child(div().font_weight(FontWeight::SEMIBOLD).child("Clip speed"))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_center()
                                .gap_3()
                                .child(step(
                                    "video-speed-dialog-down",
                                    "−",
                                    draft > RecordingClipSegment::MINIMUM_SPEED,
                                    false,
                                ))
                                .child(
                                    div()
                                        .w(px(64.0))
                                        .text_center()
                                        .text_lg()
                                        .font_weight(FontWeight::BOLD)
                                        .child(format!("{draft}×")),
                                )
                                .child(step(
                                    "video-speed-dialog-up",
                                    "+",
                                    draft < RecordingClipSegment::MAXIMUM_SPEED,
                                    true,
                                )),
                        )
                        .child(row(
                            "Clip length",
                            seconds(clip.editor_duration()),
                            seconds(changed.editor_duration()),
                        ))
                        .child(row(
                            "Video ends at",
                            Self::video_timecode(old_end),
                            Self::video_timecode(new_end),
                        ))
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap_2()
                                .pt_1()
                                .child(button("video-speed-cancel", "Cancel", false).on_click(
                                    cx.listener(|this, _, _, cx| {
                                        this.video_speed_draft = None;
                                        cx.notify();
                                    }),
                                ))
                                .child(
                                    button("video-speed-apply", "Apply", true)
                                        .opacity(if unchanged { 0.5 } else { 1.0 })
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.video_speed_draft = None;
                                            if !unchanged {
                                                this.set_selected_video_clip_speed(draft, cx);
                                            }
                                            cx.notify();
                                        })),
                                ),
                        ),
                ),
        )
        .map(IntoElement::into_any_element)
    }

    fn video_edit_controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let edit_busy = self.video_edit_busy;
        let can_delete = (self.video_selected_zoom_cue.is_some()
            || self.video_clip_timeline.segments.len() > 1)
            && !edit_busy;
        let selected_speed = self
            .video_selected_clip
            .and_then(|id| {
                self.video_clip_timeline
                    .segments
                    .iter()
                    .find(|clip| clip.id == id)
            })
            .map(|clip| clip.speed)
            .unwrap_or(1.0);
        div()
            .flex()
            .items_center()
            .gap_1()
            .child(
                div()
                    .id("video-split")
                    .px_3()
                    .h(px(32.0))
                    .flex()
                    .items_center()
                    .rounded_md()
                    .text_sm()
                    .when(!edit_busy, |this| {
                        this.cursor_pointer()
                            .hover(|style| style.bg(rgb(0xeeeeef)))
                            .on_click(cx.listener(|this, _, _, cx| this.split_video_clip(cx)))
                    })
                    .opacity(if edit_busy { 0.35 } else { 1.0 })
                    .child("Split"),
            )
            .child(
                div()
                    .id("video-add-zoom")
                    .px_3()
                    .h(px(32.0))
                    .flex()
                    .items_center()
                    .gap_1()
                    .rounded_md()
                    .text_sm()
                    .when(!edit_busy, |this| {
                        this.cursor_pointer()
                            .hover(|style| style.bg(rgb(0xe7f1ff)))
                            .on_click(
                                cx.listener(|this, _, _, cx| this.add_video_zoom_at_playhead(cx)),
                            )
                    })
                    .opacity(if edit_busy { 0.35 } else { 1.0 })
                    .child("+ Motion"),
            )
            .child(
                div()
                    .id("video-delete-clip")
                    .size(px(32.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_md()
                    .when(can_delete, |this| {
                        this.cursor_pointer()
                            .hover(|style| style.bg(rgb(0xfee2e2)))
                            .on_click(
                                cx.listener(|this, _, _, cx| this.delete_selected_video_edit(cx)),
                            )
                    })
                    .opacity(if can_delete { 1.0 } else { 0.35 })
                    .child(
                        svg()
                            .path("icons/trash.svg")
                            .size(px(16.0))
                            .text_color(ink()),
                    ),
            )
            .child(
                div()
                    .id("video-speed")
                    .ml_2()
                    .px_3()
                    .h(px(32.0))
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded_md()
                    .text_sm()
                    .when(self.video_selected_clip.is_some() && !edit_busy, |this| {
                        this.cursor_pointer()
                            .hover(|style| style.bg(rgb(0xeeeeef)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.video_speed_draft = Some(selected_speed);
                                cx.notify();
                            }))
                    })
                    .opacity(if edit_busy { 0.35 } else { 1.0 })
                    .child("Speed")
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(format!("{selected_speed}×")),
                    ),
            )
    }

    fn finish_capture_request(&mut self, result: Result<PathBuf, String>) {
        self.capturing = false;
        match result {
            Ok(path) => {
                self.captured_dimensions = image::image_dimensions(&path).ok();
                let image = self.displayed_capture_image.take();
                self.retired_images.extend(image);
                self.capture_rgba = None;
                if let Ok(image) = image::open(&path) {
                    self.set_capture_image(image.to_rgba8());
                }
                self.scene_selection = SceneSelection::Scene;
                self.media_drag = None;
                self.captured_path = Some(path);
                self.processed_capture_path = None;
                self.annotations.clear();
                self.undo_stack.clear();
                self.redo_stack.clear();
                self.crop_undo_stack.clear();
                self.crop_redo_stack.clear();
                self.crop_active = false;
                self.crop_rect = CropRect::UNIT;
                self.annotation_draft = None;
                self.selected_annotation = None;
                // A new capture starts static; its motion regions start fresh.
                if self.animation_active {
                    self.exit_animation();
                }
                self.video_zoom_cues.clear();
                self.animation_preset = None;
                let scenes = self.image_scenes.drain(..).map(|scene| scene.render);
                self.retired_images.extend(scenes);
                self.image_scene_index = 0;
                self.walkthrough_stops.clear();
                self.walkthrough_mode = false;
                self.animation_pointer_capture = PointerCaptureFile::default();
                self.video_pointer_timeline = PointerTimeline::default();
                self.toast = Some("Screenshot captured — editing controls are active".into());
            }
            Err(error) => {
                self.toast = Some(format!("Capture failed or was cancelled: {error}").into());
            }
        }
    }

    fn selected_canvas_ratio(&self) -> f32 {
        match self.aspect_ratio {
            1 => 1.0,
            2 => 4.0 / 3.0,
            3 => 3.0 / 2.0,
            4 => 16.0 / 9.0,
            _ => self
                .video_project
                .as_ref()
                .map(|_| self.video_source_size)
                .or(self.captured_dimensions)
                .filter(|(_, height)| *height > 0)
                .map(|(width, height)| width as f32 / height as f32)
                .unwrap_or(5.0 / 3.0),
        }
    }

    fn apply_background_preset(&mut self, index: usize) {
        let preset = BACKGROUND_PRESETS[index.min(BACKGROUND_PRESETS.len() - 1)];
        self.wallpaper_tab = preset.wallpaper_tab;
        self.library_tab = preset.library_tab;
        self.wallpaper_asset = preset.wallpaper_asset;
        self.custom_wallpaper = None;
        self.color_index = preset.color_index;
        self.gradient_index = preset.gradient_index;
        self.padding = preset.padding;
        self.shadow = preset.shadow;
        self.corners = preset.corners;
        self.shadow_style = preset.shadow_style;
        self.aspect_ratio = preset.aspect_ratio;
        self.border = preset.border;
        self.border_color = preset.border_color;
        self.border_thickness = preset.border_thickness;
        self.border_opacity = preset.border_opacity;
        self.background_preset = Some(index);
        self.toast = Some(format!("{} preset applied", preset.name).into());
    }

    fn preview_canvas_size(
        &self,
        available_width: Pixels,
        available_height: Pixels,
    ) -> (Pixels, Pixels) {
        let ratio = self.selected_canvas_ratio();
        if available_width / available_height > ratio {
            (available_height * ratio, available_height)
        } else {
            (available_width, available_width * (1.0 / ratio))
        }
    }

    fn crop_normalized_aspect(&self) -> Option<f32> {
        let (width, height) = self.captured_dimensions?;
        let pixel_ratio = match self.crop_aspect {
            1 => width as f32 / height.max(1) as f32,
            2 => 1.0,
            3 => 16.0 / 9.0,
            4 => 9.0 / 16.0,
            5 => 4.0 / 3.0,
            6 => 3.0 / 2.0,
            _ => return None,
        };
        Some(pixel_ratio * height as f32 / width.max(1) as f32)
    }

    fn begin_crop(&mut self) {
        if self.captured_path.is_none() {
            self.toast = Some("Capture an image first".into());
            return;
        }
        self.stop_editing_text();
        self.selected_annotation = None;
        self.annotation_draft = None;
        self.crop_rect = CropRect::UNIT;
        self.crop_aspect = 0;
        self.crop_drag = None;
        self.crop_active = true;
        self.toast = Some("Drag the crop handles, then choose Crop".into());
    }

    fn cancel_crop(&mut self) {
        self.crop_active = false;
        self.crop_rect = CropRect::UNIT;
        self.crop_aspect = 0;
        self.crop_drag = None;
        self.pointer_is_down = false;
        self.toast = Some("Crop cancelled".into());
    }

    fn set_crop_aspect(&mut self, aspect: usize) {
        self.crop_aspect = aspect;
        if let Some(ratio) = self.crop_normalized_aspect() {
            self.crop_rect = crop_rect_with_aspect(self.crop_rect, ratio);
        }
    }

    fn crop_pointer_down(&mut self, position: Point<Pixels>, image: Bounds<Pixels>) {
        if self.pointer_is_down {
            return;
        }
        self.pointer_is_down = true;
        let point = screen_to_norm(position, image);
        let handles: &[CropHandle] = if self.crop_aspect == 0 {
            &CROP_HANDLES
        } else {
            &[
                CropHandle::TopLeft,
                CropHandle::TopRight,
                CropHandle::BottomLeft,
                CropHandle::BottomRight,
            ]
        };
        let hit = handles.iter().copied().find(|handle| {
            let center = norm_to_screen(crop_handle_point(*handle, self.crop_rect), image);
            (center.x - position.x).abs() <= px(16.0) && (center.y - position.y).abs() <= px(16.0)
        });
        if let Some(handle) = hit {
            self.crop_drag = Some(CropDrag::Resize(handle));
        } else if point.x >= self.crop_rect.x
            && point.x <= self.crop_rect.right()
            && point.y >= self.crop_rect.y
            && point.y <= self.crop_rect.bottom()
        {
            self.crop_drag = Some(CropDrag::Move {
                start: point,
                rect: self.crop_rect,
            });
        }
    }

    fn crop_pointer_move(&mut self, position: Point<Pixels>, image: Bounds<Pixels>) {
        let Some(drag) = self.crop_drag else { return };
        let point = screen_to_norm(position, image);
        match drag {
            CropDrag::Move { start, rect } => {
                self.crop_rect = move_crop_rect(
                    rect,
                    NormPoint {
                        x: point.x - start.x,
                        y: point.y - start.y,
                    },
                );
            }
            CropDrag::Resize(handle) => {
                let (width, height) = self.captured_dimensions.unwrap_or((1200, 720));
                self.crop_rect = resize_crop_rect(
                    self.crop_rect,
                    handle,
                    point,
                    if matches!(
                        handle,
                        CropHandle::TopLeft
                            | CropHandle::TopRight
                            | CropHandle::BottomLeft
                            | CropHandle::BottomRight
                    ) {
                        self.crop_normalized_aspect()
                    } else {
                        None
                    },
                    (24.0 / width.max(1) as f32).clamp(0.01, 0.5),
                    (24.0 / height.max(1) as f32).clamp(0.01, 0.5),
                );
            }
        }
    }

    fn apply_crop(&mut self) -> Result<(), String> {
        let source = self
            .captured_path
            .as_ref()
            .ok_or_else(|| "Capture an image first".to_string())?;
        let image = image::open(source)
            .map_err(|error| format!("Could not read capture: {error}"))?
            .to_rgba8();
        let old_width = image.width();
        let old_height = image.height();
        let rect = self.crop_rect;
        let left = (rect.x * old_width as f32)
            .floor()
            .clamp(0.0, old_width as f32 - 1.0) as u32;
        let top = (rect.y * old_height as f32)
            .floor()
            .clamp(0.0, old_height as f32 - 1.0) as u32;
        let right = (rect.right() * old_width as f32)
            .ceil()
            .clamp((left + 1) as f32, old_width as f32) as u32;
        let bottom = (rect.bottom() * old_height as f32)
            .ceil()
            .clamp((top + 1) as f32, old_height as f32) as u32;
        if left == 0 && top == 0 && right == old_width && bottom == old_height {
            self.cancel_crop();
            return Ok(());
        }
        let cropped =
            image::imageops::crop_imm(&image, left, top, right - left, bottom - top).to_image();
        let destination = std::env::temp_dir().join(format!(
            "screendrop-crop-{}-{}.png",
            std::process::id(),
            self.effect_revision + 1
        ));
        cropped
            .save(&destination)
            .map_err(|error| format!("Could not save crop: {error}"))?;
        let used = CropRect {
            x: left as f32 / old_width as f32,
            y: top as f32 / old_height as f32,
            width: (right - left) as f32 / old_width as f32,
            height: (bottom - top) as f32 / old_height as f32,
        };
        self.crop_undo_stack.push(CropSnapshot {
            path: source.clone(),
            dimensions: (old_width, old_height),
            annotations: self.annotations.clone(),
        });
        self.crop_redo_stack.clear();
        for mark in &mut self.annotations {
            let remap = |point: &mut NormPoint| {
                point.x = (point.x - used.x) / used.width;
                point.y = (point.y - used.y) / used.height;
            };
            remap(&mut mark.start);
            remap(&mut mark.end);
            for point in &mut mark.points {
                remap(point);
            }
        }
        self.captured_path = Some(destination);
        self.captured_dimensions = Some((right - left, bottom - top));
        self.processed_capture_path = None;
        self.set_capture_image(cropped);
        self.crop_active = false;
        self.crop_rect = CropRect::UNIT;
        self.crop_aspect = 0;
        self.crop_drag = None;
        self.pointer_is_down = false;
        self.rebuild_redactions()?;
        self.toast = Some(format!("Cropped to {} × {}", right - left, bottom - top).into());
        Ok(())
    }

    fn current_crop_snapshot(&self) -> Option<CropSnapshot> {
        Some(CropSnapshot {
            path: self.captured_path.clone()?,
            dimensions: self.captured_dimensions?,
            annotations: self.annotations.clone(),
        })
    }

    fn restore_crop_snapshot(&mut self, snapshot: CropSnapshot) -> bool {
        let image = match image::open(&snapshot.path) {
            Ok(image) => image.to_rgba8(),
            Err(error) => {
                self.toast = Some(format!("Could not restore crop: {error}").into());
                return false;
            }
        };
        self.captured_path = Some(snapshot.path);
        self.captured_dimensions = Some(snapshot.dimensions);
        self.annotations = snapshot.annotations;
        self.processed_capture_path = None;
        self.set_capture_image(image);
        self.selected_annotation = None;
        self.editing_text = None;
        let _ = self.rebuild_redactions();
        true
    }

    fn undo_crop(&mut self) -> bool {
        let Some(previous) = self.crop_undo_stack.pop() else {
            return false;
        };
        let Some(current) = self.current_crop_snapshot() else {
            return false;
        };
        self.crop_redo_stack.push(current);
        self.restore_crop_snapshot(previous)
    }

    fn redo_crop(&mut self) -> bool {
        let Some(next) = self.crop_redo_stack.pop() else {
            return false;
        };
        let Some(current) = self.current_crop_snapshot() else {
            return false;
        };
        self.crop_undo_stack.push(current);
        self.restore_crop_snapshot(next)
    }

    fn set_slider_value(&mut self, slider_id: usize, value: u8) {
        match slider_id {
            0 => self.padding = value,
            1 => self.shadow = value,
            2 => self.corners = value,
            3 => self.border_thickness = value,
            4 => self.border_opacity = value,
            5 => {
                self.redaction_strength = value.clamp(15, 100);
                if let Some(mark) = self
                    .selected_annotation
                    .and_then(|index| self.annotations.get_mut(index))
                    .filter(|mark| mark.tool == Tool::Blur || mark.tool == Tool::Pixelate)
                {
                    mark.density = self.redaction_strength as f32 / 100.0;
                }
            }
            MOTION_ZOOM_SLIDER => self.set_motion_zoom_slider(value),
            id if id >= 100 => {
                self.set_scene_slider(id, value);
            }
            6 => {
                self.text_font_size = value.clamp(10, 96) as f32;
                let selected = self.selected_annotation;
                if let Some(mark) = selected
                    .and_then(|index| self.annotations.get_mut(index))
                    .filter(|mark| mark.tool == Tool::Text)
                {
                    mark.font_size = self.text_font_size;
                }
                if let Some(index) = selected {
                    self.fit_text_box_to_content(index);
                }
            }
            _ => {}
        }
    }

    fn record_annotation_undo(&mut self) {
        self.undo_stack.push(self.annotations.clone());
        if self.undo_stack.len() > 100 {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
    }

    fn undo_annotations(&mut self) -> bool {
        let Some(previous) = self.undo_stack.pop() else {
            return false;
        };
        self.redo_stack
            .push(std::mem::replace(&mut self.annotations, previous));
        self.selected_annotation = None;
        self.editing_text = None;
        self.annotation_draft = None;
        true
    }

    fn redo_annotations(&mut self) -> bool {
        let Some(next) = self.redo_stack.pop() else {
            return false;
        };
        self.undo_stack
            .push(std::mem::replace(&mut self.annotations, next));
        self.selected_annotation = None;
        self.editing_text = None;
        self.annotation_draft = None;
        true
    }

    fn stop_editing_text(&mut self) {
        let Some(index) = self.editing_text.take() else {
            return;
        };
        let is_empty = self
            .annotations
            .get(index)
            .is_none_or(|mark| mark.text.trim().is_empty());
        if is_empty && index < self.annotations.len() {
            self.annotations.remove(index);
            self.selected_annotation = None;
        }
        if self.tool == Tool::Text {
            self.tool = Tool::Select;
        }
    }

    fn fit_text_box_to_content(&mut self, index: usize) {
        let aspect = self
            .media_dimensions()
            .map(|(width, height)| width as f32 / height.max(1) as f32)
            .unwrap_or(16.0 / 9.0)
            .max(0.1);
        let Some(mark) = self.annotations.get_mut(index) else {
            return;
        };
        if mark.tool != Tool::Text {
            return;
        }
        let height_norm = (mark.end.y - mark.start.y).abs().max(0.001);
        let preview_height = (mark.font_size * 1.35).max(16.0);
        let preview_image_height = preview_height / height_norm;
        let preview_image_width = preview_image_height * aspect;
        let character_count = mark.text.chars().count() as f32;
        let desired_width = if character_count == 0.0 {
            16.0
        } else {
            character_count * mark.font_size * 0.59 + 8.0
        };
        mark.end.x = (mark.start.x + desired_width / preview_image_width.max(1.0)).min(1.0);
    }

    fn update_slider_drag(&mut self, event: &MouseMoveEvent) -> bool {
        let Some(drag) = self.slider_drag else {
            return false;
        };
        if !event.dragging() {
            self.slider_drag = None;
            return false;
        }

        let delta = (event.position.x - drag.start_x) / px(1.0);
        let value = (drag.start_value as f32 + delta).round().clamp(0.0, 100.0) as u8;
        self.set_slider_value(drag.slider_id, value);
        true
    }

    fn pointer_down(
        &mut self,
        position: Point<Pixels>,
        image: Bounds<Pixels>,
        rendered_bounds: &[Bounds<Pixels>],
    ) {
        // GPUI can retain more than one paint-scoped mouse listener across a
        // redraw. Treat a physical press as one editing transaction so a
        // single click cannot create stacked duplicate annotations.
        if self.pointer_is_down {
            return;
        }
        self.pointer_is_down = true;
        if !image.contains(&position) {
            return;
        }
        if let Some(index) = self.editing_text {
            let clicked_editing_text = rendered_bounds
                .get(index)
                .copied()
                .unwrap_or_else(|| mark_hit_bounds(&self.annotations[index], image))
                .contains(&position);
            if clicked_editing_text {
                self.selected_annotation = Some(index);
                self.caret_visible = true;
                if self.tool != Tool::Select {
                    self.tool = Tool::Select;
                }
            } else {
                self.stop_editing_text();
            }
        }
        let normalized = screen_to_norm(position, image);
        if self.tool == Tool::Select {
            self.selected_annotation =
                self.annotations
                    .iter()
                    .enumerate()
                    .rposition(|(index, mark)| {
                        rendered_bounds
                            .get(index)
                            .copied()
                            .unwrap_or_else(|| mark_hit_bounds(mark, image))
                            .contains(&position)
                    });
            self.selection_last_point = self.selected_annotation.map(|_| position);
            self.selection_resizing = self.selected_annotation.is_some_and(|index| {
                let bounds = rendered_bounds
                    .get(index)
                    .copied()
                    .unwrap_or_else(|| mark_screen_bounds(&self.annotations[index], image));
                (position.x - (bounds.origin.x + bounds.size.width)).abs() <= px(14.0)
                    && (position.y - (bounds.origin.y + bounds.size.height)).abs() <= px(14.0)
            });
            if self.selected_annotation.is_some() {
                self.record_annotation_undo();
            }
            self.editing_text = self.selected_annotation.filter(|index| {
                self.annotations
                    .get(*index)
                    .is_some_and(|mark| mark.tool == Tool::Text)
            });
            if self.editing_text.is_some() {
                self.caret_visible = true;
            }
            self.toast = Some(if self.selected_annotation.is_some() {
                "Annotation selected — drag to move; Delete removes it".into()
            } else {
                "No annotation at that point".into()
            });
            return;
        }

        let mut number = 1;
        while self
            .annotations
            .iter()
            .any(|mark| mark.tool == Tool::Number && mark.number == number)
        {
            number += 1;
        }
        self.record_annotation_undo();
        let color =
            ANNOTATION_COLORS[self.annotation_color_index.min(ANNOTATION_COLORS.len() - 1)].1;
        let mut mark = AnnotationMark {
            tool: self.tool,
            start: normalized,
            end: normalized,
            points: vec![normalized],
            number,
            color,
            stroke_width: self.annotation_stroke_width,
            density: self.redaction_strength as f32 / 100.0,
            text: String::new(),
            font_size: self.text_font_size,
            font_family: self.text_font_family,
            text_alignment: self.text_alignment,
            bold: self.text_bold,
            italic: self.text_italic,
            underline: self.text_underline,
            timing: self.scene_is_timed().then(|| {
                AnnotationTiming::for_tool(self.tool, self.video_position, self.video_duration)
            }),
            opacity: 1.0,
            from_template: false,
            pinned: false,
        };

        if self.tool == Tool::Number {
            let diameter_x = 42.0 / (image.size.width / px(1.0));
            let diameter_y = 42.0 / (image.size.height / px(1.0));
            mark.start = NormPoint {
                x: (normalized.x - diameter_x * 0.5).max(0.0),
                y: (normalized.y - diameter_y * 0.5).max(0.0),
            };
            mark.end = NormPoint {
                x: (mark.start.x + diameter_x).min(1.0),
                y: (mark.start.y + diameter_y).min(1.0),
            };
            self.annotations.push(mark);
            self.selected_annotation = Some(self.annotations.len() - 1);
        } else if self.tool == Tool::Text {
            let width = 16.0 / (image.size.width / px(1.0));
            let height = (self.text_font_size * 1.35).max(16.0) / (image.size.height / px(1.0));
            mark.end = NormPoint {
                x: (normalized.x + width).min(1.0),
                y: (normalized.y + height).min(1.0),
            };
            self.annotations.push(mark);
            let index = self.annotations.len() - 1;
            self.selected_annotation = Some(index);
            self.editing_text = Some(index);
            self.caret_visible = true;
            self.toast = Some("Type text; Enter commits, Escape cancels".into());
        } else {
            self.selected_annotation = None;
            self.editing_text = None;
            self.annotation_draft = Some(mark);
        }
    }

    /// The on-screen frame rect for pinned marks, recovered from the zoomed
    /// interaction rect `image` and the current viewport crop.
    fn pinned_bounds(&self, image: Bounds<Pixels>) -> Bounds<Pixels> {
        if !self.scene_is_timed() {
            return image;
        }
        let frame = self.video_viewport_timeline.frame_at(self.video_position);
        let (left, top, visible) = visible_rect(frame);
        let size = size(
            image.size.width * visible as f32,
            image.size.height * visible as f32,
        );
        Bounds {
            origin: point(
                image.origin.x + image.size.width * left as f32,
                image.origin.y + image.size.height * top as f32,
            ),
            size,
        }
    }

    fn pointer_move(&mut self, position: Point<Pixels>, image: Bounds<Pixels>) {
        if self.tool == Tool::Select {
            if let (Some(index), Some(last)) = (self.selected_annotation, self.selection_last_point)
            {
                let frame = self.pinned_bounds(image);
                if let Some(mark) = self.annotations.get_mut(index) {
                    let image = if mark.pinned { frame } else { image };
                    let dx = (position.x - last.x) / image.size.width;
                    let dy = (position.y - last.y) / image.size.height;
                    if self.selection_resizing && mark.tool != Tool::Pen {
                        mark.end = screen_to_norm(position, image);
                    } else {
                        mark.start.x = (mark.start.x + dx).clamp(0.0, 1.0);
                        mark.start.y = (mark.start.y + dy).clamp(0.0, 1.0);
                        mark.end.x = (mark.end.x + dx).clamp(0.0, 1.0);
                        mark.end.y = (mark.end.y + dy).clamp(0.0, 1.0);
                        for point in &mut mark.points {
                            point.x = (point.x + dx).clamp(0.0, 1.0);
                            point.y = (point.y + dy).clamp(0.0, 1.0);
                        }
                    }
                }
                self.selection_last_point = Some(position);
            }
        } else if let Some(mark) = &mut self.annotation_draft {
            let normalized = screen_to_norm(position, image);
            mark.end = normalized;
            if mark.tool == Tool::Pen {
                mark.points.push(normalized);
            }
        }
    }

    fn pointer_up(&mut self, position: Point<Pixels>, image: Bounds<Pixels>) -> bool {
        self.pointer_is_down = false;
        self.selection_last_point = None;
        self.selection_resizing = false;
        let Some(mut mark) = self.annotation_draft.take() else {
            return self.selected_annotation.is_some_and(|index| {
                self.annotations
                    .get(index)
                    .is_some_and(|mark| mark.tool == Tool::Blur || mark.tool == Tool::Pixelate)
            });
        };
        mark.end = screen_to_norm(position, image);
        let width = (mark.end.x - mark.start.x).abs();
        let height = (mark.end.y - mark.start.y).abs();
        if mark.tool == Tool::Pen {
            if mark.points.len() < 2 {
                return false;
            }
        } else if width < 0.003 || height < 0.003 {
            if matches!(
                mark.tool,
                Tool::Rectangle
                    | Tool::FilledRectangle
                    | Tool::Ellipse
                    | Tool::Highlight
                    | Tool::Blur
                    | Tool::Pixelate
            ) {
                let fallback = 80.0;
                mark.end = NormPoint {
                    x: (mark.start.x + fallback / (image.size.width / px(1.0))).min(1.0),
                    y: (mark.start.y + fallback / (image.size.height / px(1.0))).min(1.0),
                };
            } else {
                return false;
            }
        }
        let created_tool = mark.tool;
        let needs_redaction = created_tool == Tool::Blur || created_tool == Tool::Pixelate;
        self.annotations.push(mark);
        self.selected_annotation = Some(self.annotations.len() - 1);
        if matches!(
            created_tool,
            Tool::Rectangle
                | Tool::FilledRectangle
                | Tool::Ellipse
                | Tool::Line
                | Tool::Arrow
                | Tool::Highlight
                | Tool::Blur
                | Tool::Pixelate
        ) {
            self.tool = Tool::Select;
        }
        self.toast = Some("Annotation added".into());
        needs_redaction
    }

    fn handle_video_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
        if self.handle_watermark_key(event) {
            return true;
        }
        // The speed dialog owns the keyboard: Escape cancels, Enter applies.
        if let Some(draft) = self.video_speed_draft {
            match event.keystroke.key.as_str() {
                "escape" => self.video_speed_draft = None,
                "enter" => {
                    self.video_speed_draft = None;
                    self.set_selected_video_clip_speed(draft, cx);
                }
                _ => {}
            }
            return true;
        }
        // Typing into a text annotation owns the keyboard.
        if self.editing_text.is_some() {
            return self.handle_key(event);
        }
        let keystroke = &event.keystroke;
        if (keystroke.modifiers.control || keystroke.modifiers.platform) && keystroke.key == "z" {
            if keystroke.modifiers.shift {
                if !self.redo_annotations() {
                    self.redo_video_edit(cx);
                }
            } else if !self.undo_annotations() {
                self.undo_video_edit(cx);
            }
            return true;
        }
        if matches!(keystroke.key.as_str(), "delete" | "backspace") {
            if let Some(index) = self.selected_annotation.take() {
                if index < self.annotations.len() {
                    self.record_annotation_undo();
                    self.annotations.remove(index);
                    self.toast = Some("Annotation removed".into());
                }
                return true;
            }
        }
        if keystroke.key == "escape"
            && (self.selected_annotation.is_some() || self.tool != Tool::Select)
        {
            self.selected_annotation = None;
            self.annotation_draft = None;
            self.tool = Tool::Select;
            return true;
        }
        if keystroke.key == "v" && !keystroke.modifiers.control && !keystroke.modifiers.platform {
            self.tool = Tool::Select;
            return true;
        }
        if keystroke.key == "t" && !keystroke.modifiers.control && !keystroke.modifiers.platform {
            self.tool = Tool::Text;
            self.selected_annotation = None;
            self.toast = Some("Text tool: click the recording to place a caption".into());
            return true;
        }
        match keystroke.key.as_str() {
            "space" => {
                if self.video_playing {
                    self.pause_video_playback();
                } else {
                    self.start_video_playback(cx);
                }
                true
            }
            "left" => {
                self.seek_video(self.video_position - 5.0, cx);
                true
            }
            "right" => {
                self.seek_video(self.video_position + 5.0, cx);
                true
            }
            "s" => {
                self.split_video_clip(cx);
                true
            }
            "m" => {
                self.add_video_zoom_at_playhead(cx);
                true
            }
            "delete" | "backspace" => {
                if self.video_selected_press.is_some() {
                    self.remove_selected_press(cx);
                } else {
                    self.delete_selected_video_edit(cx);
                }
                true
            }
            "escape" => {
                self.video_selected_zoom_cue = None;
                self.video_selected_press = None;
                self.scene_selection = SceneSelection::Scene;
                true
            }
            _ => false,
        }
    }

    fn handle_key(&mut self, event: &KeyDownEvent) -> bool {
        if self.handle_watermark_key(event) {
            return true;
        }
        if self.crop_active {
            match event.keystroke.key.as_str() {
                "escape" => self.cancel_crop(),
                "enter" => {
                    if let Err(error) = self.apply_crop() {
                        self.toast = Some(error.into());
                    }
                }
                _ => return false,
            }
            return true;
        }
        if (event.keystroke.modifiers.control || event.keystroke.modifiers.platform)
            && event.keystroke.key == "z"
        {
            return if event.keystroke.modifiers.shift {
                self.redo_annotations() || self.redo_crop()
            } else {
                self.undo_annotations() || self.undo_crop()
            };
        }
        if let Some(index) = self.editing_text {
            self.caret_visible = true;
            match event.keystroke.key.as_str() {
                "enter" => {
                    self.stop_editing_text();
                }
                "escape" => {
                    self.stop_editing_text();
                }
                "backspace" => {
                    if let Some(mark) = self.annotations.get_mut(index) {
                        mark.text.pop();
                    }
                    self.fit_text_box_to_content(index);
                }
                _ => {
                    if !event.keystroke.modifiers.control
                        && !event.keystroke.modifiers.platform
                        && !event.keystroke.modifiers.alt
                    {
                        if let (Some(text), Some(mark)) = (
                            event.keystroke.key_char.as_ref(),
                            self.annotations.get_mut(index),
                        ) {
                            mark.text.push_str(text);
                        }
                        self.fit_text_box_to_content(index);
                    }
                }
            }
            return true;
        }

        if matches!(event.keystroke.key.as_str(), "delete" | "backspace") {
            if let Some(index) = self.selected_annotation.take() {
                self.record_annotation_undo();
                self.annotations.remove(index);
                return true;
            }
        }
        false
    }

    fn rebuild_redactions(&mut self) -> Result<(), String> {
        let Some(source_path) = self.captured_path.as_ref() else {
            return Ok(());
        };
        let mut output = image::open(source_path)
            .map_err(|error| error.to_string())?
            .to_rgba8();
        let width = output.width();
        let height = output.height();
        for mark in self
            .annotations
            .iter()
            .filter(|mark| mark.tool == Tool::Blur || mark.tool == Tool::Pixelate)
        {
            let left = mark.start.x.min(mark.end.x).clamp(0.0, 1.0);
            let top = mark.start.y.min(mark.end.y).clamp(0.0, 1.0);
            let right = mark.start.x.max(mark.end.x).clamp(0.0, 1.0);
            let bottom = mark.start.y.max(mark.end.y).clamp(0.0, 1.0);
            let x = (left * width as f32).floor() as u32;
            let y = (top * height as f32).floor() as u32;
            let region_width = ((right - left) * width as f32).ceil() as u32;
            let region_height = ((bottom - top) * height as f32).ceil() as u32;
            if region_width == 0 || region_height == 0 {
                continue;
            }
            let region_width = region_width.min(width.saturating_sub(x));
            let region_height = region_height.min(height.saturating_sub(y));
            let crop =
                image::imageops::crop_imm(&output, x, y, region_width, region_height).to_image();
            let processed = if mark.tool == Tool::Pixelate {
                let block = (4.0 + mark.density.clamp(0.0, 1.0) * 36.0).round() as u32;
                let small = image::imageops::resize(
                    &crop,
                    (region_width / block.max(1)).max(1),
                    (region_height / block.max(1)).max(1),
                    image::imageops::FilterType::Triangle,
                );
                image::imageops::resize(
                    &small,
                    region_width,
                    region_height,
                    image::imageops::FilterType::Nearest,
                )
            } else {
                image::imageops::blur(&crop, 2.0 + mark.density.clamp(0.0, 1.0) * 28.0)
            };
            image::imageops::replace(&mut output, &processed, i64::from(x), i64::from(y));
        }
        self.effect_revision += 1;
        let destination = std::env::temp_dir().join(format!(
            "screendrop-redacted-{}-{}.png",
            std::process::id(),
            self.effect_revision
        ));
        output
            .save(&destination)
            .map_err(|error| error.to_string())?;
        self.set_capture_image(output);
        if let Some(previous) = self.processed_capture_path.replace(destination) {
            if previous
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("screendrop-redacted-"))
            {
                let _ = fs::remove_file(previous);
            }
        }
        Ok(())
    }

    /// SVG fragment with every visible annotation, positioned relative to a
    /// capture drawn at (`x`, `y`) with the given pixel size. Shared by the
    /// static PNG export and the animated export's flattened frame.
    fn annotations_svg(
        &self,
        x: f32,
        y: f32,
        capture_width: u32,
        capture_height: u32,
        stroke_scale: f32,
    ) -> String {
        annotations_svg(
            &self.annotations,
            x,
            y,
            capture_width,
            capture_height,
            stroke_scale,
        )
    }
}

/// SVG fragment with every visible annotation in `marks`, positioned
/// relative to a capture drawn at (`x`, `y`) with the given pixel size. The
/// fragment closes the `<g>` opened by the caller.
fn annotations_svg(
    marks: &[AnnotationMark],
    x: f32,
    y: f32,
    capture_width: u32,
    capture_height: u32,
    stroke_scale: f32,
) -> String {
    {
        let mut svg = String::new();
        let highlights: Vec<_> = marks
            .iter()
            .filter(|mark| mark.tool == Tool::Highlight)
            .collect();
        if !highlights.is_empty() {
            let _ = write!(svg, "<path fill=\"black\" fill-opacity=\"0.55\" fill-rule=\"evenodd\" d=\"M{x},{y}h{capture_width}v{capture_height}h-{capture_width}z");
            for mark in highlights {
                let hx = x + mark.start.x.min(mark.end.x) * capture_width as f32;
                let hy = y + mark.start.y.min(mark.end.y) * capture_height as f32;
                let hw = (mark.end.x - mark.start.x).abs() * capture_width as f32;
                let hh = (mark.end.y - mark.start.y).abs() * capture_height as f32;
                let _ = write!(svg, " M{hx},{hy}v{hh}h{hw}v-{hh}z");
            }
            svg.push_str("\"/>");
        }

        for mark in marks.iter().filter(|mark| {
            !matches!(
                mark.tool,
                Tool::Select | Tool::Blur | Tool::Pixelate | Tool::Highlight
            )
        }) {
            if mark.opacity < 0.999 {
                let _ = write!(svg, "<g opacity=\"{:.3}\">", mark.opacity.clamp(0.0, 1.0));
            }
            let sx = x + mark.start.x * capture_width as f32;
            let sy = y + mark.start.y * capture_height as f32;
            let ex = x + mark.end.x * capture_width as f32;
            let ey = y + mark.end.y * capture_height as f32;
            let left = sx.min(ex);
            let top = sy.min(ey);
            let width = (ex - sx).abs();
            let height = (ey - sy).abs();
            let color = mark.color;
            let stroke = (mark.stroke_width * stroke_scale).max(1.0);
            match mark.tool {
                Tool::Rectangle => {
                    let _ = write!(svg, "<rect x=\"{left}\" y=\"{top}\" width=\"{width}\" height=\"{height}\" rx=\"2\" fill=\"none\" stroke=\"#{color:06x}\" stroke-width=\"{stroke}\"/>");
                }
                Tool::FilledRectangle => {
                    let _ = write!(svg, "<rect x=\"{left}\" y=\"{top}\" width=\"{width}\" height=\"{height}\" rx=\"2\" fill=\"#{color:06x}\"/>");
                }
                Tool::Ellipse => {
                    let _ = write!(svg, "<ellipse cx=\"{}\" cy=\"{}\" rx=\"{}\" ry=\"{}\" fill=\"none\" stroke=\"#{color:06x}\" stroke-width=\"{stroke}\"/>", left + width/2.0, top + height/2.0, width/2.0, height/2.0);
                }
                Tool::Line | Tool::Arrow => {
                    let _ = write!(svg, "<line x1=\"{sx}\" y1=\"{sy}\" x2=\"{ex}\" y2=\"{ey}\" stroke=\"#{color:06x}\" stroke-width=\"{stroke}\" stroke-linecap=\"round\"/>");
                    if mark.tool == Tool::Arrow {
                        let dx = ex - sx;
                        let dy = ey - sy;
                        let len = (dx * dx + dy * dy).sqrt().max(1.0);
                        let ux = dx / len;
                        let uy = dy / len;
                        let head = stroke * 4.0 + 12.0;
                        let wing = stroke * 2.0 + 6.0;
                        let ax = ex - ux * head - uy * wing;
                        let ay = ey - uy * head + ux * wing;
                        let bx = ex - ux * head + uy * wing;
                        let by = ey - uy * head - ux * wing;
                        let _ = write!(svg, "<path d=\"M{ax},{ay} L{ex},{ey} L{bx},{by}\" fill=\"none\" stroke=\"#{color:06x}\" stroke-width=\"{stroke}\" stroke-linecap=\"round\" stroke-linejoin=\"round\"/>");
                    }
                }
                Tool::Pen => {
                    let mut points = String::new();
                    for point in &mark.points {
                        let _ = write!(
                            points,
                            "{},{} ",
                            x + point.x * capture_width as f32,
                            y + point.y * capture_height as f32
                        );
                    }
                    let _ = write!(svg, "<polyline points=\"{points}\" fill=\"none\" stroke=\"#{color:06x}\" stroke-width=\"{stroke}\" stroke-linecap=\"round\" stroke-linejoin=\"round\"/>");
                }
                Tool::Number => {
                    let cx = left + width / 2.0;
                    let cy = top + height / 2.0;
                    let r = width.min(height) / 2.0;
                    let _ = write!(svg, "<circle cx=\"{cx}\" cy=\"{cy}\" r=\"{r}\" fill=\"#{color:06x}\"/><text x=\"{cx}\" y=\"{}\" text-anchor=\"middle\" font-family=\"sans-serif\" font-weight=\"700\" font-size=\"{}\" fill=\"white\">{}</text>", cy+r*0.36, r, mark.number);
                }
                Tool::Text if !mark.text.is_empty() => {
                    let weight = if mark.bold { "700" } else { "400" };
                    let style = if mark.italic { "italic" } else { "normal" };
                    let decoration = if mark.underline { "underline" } else { "none" };
                    // Fallbacks keep the export sans-serif on machines
                    // without the preferred face installed.
                    let family = match mark.font_family {
                        1 => {
                            "DejaVu Sans Condensed, DejaVu Sans, Liberation Sans Narrow, sans-serif"
                        }
                        2 => "Ubuntu, Cantarell, Noto Sans, DejaVu Sans, sans-serif",
                        _ => {
                            "Noto Sans, Inter, DejaVu Sans, Liberation Sans, Cantarell, sans-serif"
                        }
                    };
                    let (text_x, anchor) = match mark.text_alignment {
                        1 => (left + width / 2.0, "middle"),
                        2 => (left + width, "end"),
                        _ => (left, "start"),
                    };
                    let value = xml_escape(&mark.text);
                    let _ = write!(svg, "<text x=\"{text_x}\" y=\"{}\" text-anchor=\"{anchor}\" font-family=\"{family}\" font-weight=\"{weight}\" font-style=\"{style}\" text-decoration=\"{decoration}\" font-size=\"{}\" fill=\"#{color:06x}\">{value}</text>", top + mark.font_size*stroke_scale, mark.font_size*stroke_scale);
                }
                _ => {}
            }
            if mark.opacity < 0.999 {
                svg.push_str("</g>");
            }
        }
        svg.push_str("</g>");
        svg
    }
}

impl Studio {
    fn render_export(&mut self, destination: &std::path::Path) -> Result<(), String> {
        self.rebuild_redactions()?;
        if self.scene_style().needs_composited_preview() {
            return self.render_composited_export(destination);
        }
        let capture_path = self
            .processed_capture_path
            .as_ref()
            .or(self.captured_path.as_ref())
            .ok_or_else(|| "Capture an image first".to_string())?;
        let (capture_width, capture_height) = image::image_dimensions(capture_path)
            .map_err(|error| format!("Could not read capture: {error}"))?;
        let shortest = capture_width.min(capture_height) as f32;
        let padding = shortest * (self.padding as f32 * 0.0025);
        let content_width = capture_width as f32 + padding * 2.0;
        let content_height = capture_height as f32 + padding * 2.0;
        let (canvas_width_f, canvas_height_f) = if self.aspect_ratio == 0 {
            (content_width, content_height)
        } else {
            let ratio = self.selected_canvas_ratio();
            if content_width / content_height > ratio {
                (content_width, content_width / ratio)
            } else {
                (content_height * ratio, content_height)
            }
        };
        let canvas_width = canvas_width_f.ceil() as u32;
        let canvas_height = canvas_height_f.ceil() as u32;
        let x = (canvas_width as f32 - capture_width as f32) / 2.0;
        let y = (canvas_height as f32 - capture_height as f32) / 2.0;
        let radius = shortest * 0.12 * (self.corners as f32 / 100.0);
        let stroke_scale = shortest / 800.0;
        let border_width = if self.border {
            shortest * (0.002 + 0.078 * self.border_thickness as f32 / 100.0)
        } else {
            0.0
        };
        let border_colors = [0xffc928, 0x22b45d, 0x22bfc2, 0x3678ef, 0x8c4ce8, 0xec3d87];
        let border_color = border_colors[self.border_color.min(border_colors.len() - 1)];
        let capture_href = xml_escape(&capture_path.to_string_lossy());
        let mut svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="{canvas_width}" height="{canvas_height}" viewBox="0 0 {canvas_width} {canvas_height}"><defs><clipPath id="captureClip"><rect x="{x}" y="{y}" width="{capture_width}" height="{capture_height}" rx="{radius}"/></clipPath>"#
        );
        if self.wallpaper_tab == 1 {
            let gradient =
                GRADIENT_BACKGROUNDS[self.gradient_index.min(GRADIENT_BACKGROUNDS.len() - 1)];
            let _ = write!(
                svg,
                "<linearGradient id=\"pageFill\" x1=\"0\" y1=\"0\" x2=\"1\" y2=\"1\"><stop offset=\"0\" stop-color=\"#{:06x}\"/><stop offset=\".5\" stop-color=\"#{:06x}\"/><stop offset=\"1\" stop-color=\"#{:06x}\"/></linearGradient>",
                gradient.colors[0], gradient.colors[1], gradient.colors[2]
            );
        }
        let shadow_strength = self.shadow as f32 / 100.0;
        let (blur, dy, opacity) = match self.shadow_style {
            0 => (40.0, 8.0, 0.24),
            1 => (52.0, 34.0, 0.28),
            2 => (62.0, 0.0, 0.20),
            _ => (20.0, 10.0, 0.34),
        };
        let _ = write!(svg, "<filter id=\"dropShadow\" x=\"-50%\" y=\"-50%\" width=\"200%\" height=\"220%\"><feGaussianBlur stdDeviation=\"{}\"/><feOffset dy=\"{}\"/><feComponentTransfer><feFuncA type=\"linear\" slope=\"{}\"/></feComponentTransfer></filter></defs>", blur * shadow_strength, dy * shadow_strength, opacity * shadow_strength);

        match self.wallpaper_tab {
            0 => {
                let color = SOLID_BACKGROUNDS[self.color_index.min(SOLID_BACKGROUNDS.len() - 1)].1;
                let _ = write!(
                    svg,
                    "<rect width=\"100%\" height=\"100%\" fill=\"#{color:06x}\"/>"
                );
            }
            1 => svg.push_str("<rect width=\"100%\" height=\"100%\" fill=\"url(#pageFill)\"/>"),
            _ => {
                // Resolve bundled wallpapers against the same asset root the
                // live preview uses, never the process working directory.
                let wallpaper = self.custom_wallpaper.clone().unwrap_or_else(|| {
                    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                        .join("assets")
                        .join(self.wallpaper_asset)
                });
                let href = xml_escape(&wallpaper.to_string_lossy());
                let _ = write!(svg, "<image href=\"{href}\" width=\"{canvas_width}\" height=\"{canvas_height}\" preserveAspectRatio=\"xMidYMid slice\"/>");
            }
        }
        if self.shadow > 0 {
            let _ = write!(svg, "<rect x=\"{x}\" y=\"{y}\" width=\"{capture_width}\" height=\"{capture_height}\" rx=\"{radius}\" fill=\"black\" filter=\"url(#dropShadow)\"/>");
        }
        let _ = write!(svg, "<image href=\"{capture_href}\" x=\"{x}\" y=\"{y}\" width=\"{capture_width}\" height=\"{capture_height}\" preserveAspectRatio=\"none\" clip-path=\"url(#captureClip)\"/><g clip-path=\"url(#captureClip)\">");

        svg.push_str(&self.annotations_svg(x, y, capture_width, capture_height, stroke_scale));
        svg.push_str("</g>");
        if border_width > 0.0 && self.border_opacity > 0 {
            let opacity = self.border_opacity as f32 / 100.0;
            let _ = write!(svg, "<rect x=\"{x}\" y=\"{y}\" width=\"{capture_width}\" height=\"{capture_height}\" rx=\"{radius}\" fill=\"none\" stroke=\"#{border_color:06x}\" stroke-opacity=\"{opacity}\" stroke-width=\"{border_width}\"/>");
        }
        svg.push_str("</svg>");

        let mut options = resvg::usvg::Options::default();
        options.fontdb = crate::recording::scene::shared_fontdb();
        let tree = resvg::usvg::Tree::from_str(&svg, &options)
            .map_err(|error| format!("Could not build export: {error}"))?;
        let mut pixmap = resvg::tiny_skia::Pixmap::new(canvas_width, canvas_height)
            .ok_or_else(|| "Export dimensions are too large".to_string())?;
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::identity(),
            &mut pixmap.as_mut(),
        );
        pixmap
            .save_png(destination)
            .map_err(|error| format!("Could not save PNG: {error}"))
    }

    fn begin_screen_capture(&mut self, cx: &mut Context<Self>) {
        if self.capturing {
            return;
        }
        self.capturing = true;
        self.toast = Some("Choose a screen, window, or area in the system picker".into());
        cx.notify();
        let window_handle = cx.active_window();
        cx.spawn(async move |weak, cx| {
            let result = capture_behind_window(window_handle, cx).await;
            weak.update(cx, |this, cx| {
                this.finish_capture_request(result);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn toggle(&self, enabled: bool) -> impl IntoElement {
        div()
            .w(px(38.0))
            .h(px(22.0))
            .p(px(2.0))
            .rounded_full()
            .bg(if enabled {
                blue()
            } else {
                hsla(220.0 / 360.0, 0.03, 0.85, 1.0)
            })
            .flex()
            .justify_end()
            .when(!enabled, |this| this.justify_start())
            .child(
                div()
                    .size(px(18.0))
                    .rounded_full()
                    .bg(rgb(0xffffff))
                    .shadow_sm(),
            )
    }

    fn tool_grid(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_wrap()
            .flex_none()
            .h(px(90.0))
            .gap_1()
            .p_1()
            .rounded_lg()
            .bg(rgb(0xf4f4f5))
            .children(
                Tool::ALL
                    .into_iter()
                    .filter(|(tool, _)| {
                        // Redactions are baked into the still image; a recording
                        // draws its overlays live, so those two tools stay out.
                        self.video_project.is_none() || !matches!(tool, Tool::Blur | Tool::Pixelate)
                    })
                    .map(|(tool, icon)| {
                        let selected = self.tool == tool;
                        div()
                            .id(icon)
                            .w(px(42.0))
                            .h(px(42.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .text_color(if selected { ink() } else { muted() })
                            .bg(if selected {
                                rgb(0xe2e3e5)
                            } else {
                                rgb(0xf4f4f5)
                            })
                            .cursor_pointer()
                            .hover(|style| style.bg(rgb(0xe8e9eb)))
                            .child(svg().path(icon).size(px(20.0)).text_color(if selected {
                                blue()
                            } else {
                                ink()
                            }))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.stop_editing_text();
                                this.tool = tool;
                                if tool != Tool::Select {
                                    this.selected_annotation = None;
                                    this.editing_text = None;
                                }
                                this.toast = Some(format!("{:?} tool selected", tool).into());
                                cx.notify();
                            }))
                    }),
            )
    }

    /// Recording inspector: drawing tools with a one-line hint.
    pub(crate) fn video_annotate_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let hint = if self.annotations.is_empty() {
            "Pick a tool and draw on the recording. Marks appear at the playhead and animate in."
                .to_string()
        } else {
            format!("{} — {}", self.tool.label(), self.tool.help_text())
        };
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .child("Annotate"),
                    )
                    .when(!self.annotations.is_empty(), |this| {
                        this.child(div().text_xs().text_color(muted()).child(format!(
                            "{} mark{}",
                            self.annotations.len(),
                            if self.annotations.len() == 1 { "" } else { "s" }
                        )))
                    }),
            )
            .child(self.tool_grid(cx))
            .child(div().text_xs().text_color(muted()).child(hint))
            .into_any_element()
    }

    fn segmented<F>(
        &self,
        control_id: &'static str,
        labels: &'static [&'static str],
        selected: usize,
        on_select: F,
        cx: &mut Context<Self>,
    ) -> impl IntoElement
    where
        F: Fn(&mut Studio, usize) + Clone + 'static,
    {
        div()
            .flex()
            .flex_none()
            .w_full()
            .h(px(34.0))
            .p(px(3.0))
            .rounded_lg()
            .bg(rgb(0xf0f0f1))
            .children(labels.iter().enumerate().map(|(index, label)| {
                let on_select = on_select.clone();
                div()
                    .id((control_id, index))
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_md()
                    .text_xs()
                    .text_color(if selected == index { ink() } else { muted() })
                    .when(selected == index, |this| this.bg(rgb(0xffffff)).shadow_sm())
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        on_select(this, index);
                        cx.notify();
                    }))
                    .child((*label).to_string())
            }))
    }

    fn slider_row<F>(
        &self,
        title: &'static str,
        value: u8,
        suffix: &'static str,
        on_change: F,
        cx: &mut Context<Self>,
    ) -> impl IntoElement
    where
        F: Fn(&mut Studio, u8) + Clone + 'static,
    {
        let slider_id: usize = match title {
            "Padding" => 0,
            "Shadow" => 1,
            "Corners" => 2,
            "Thickness" => 3,
            "Opacity" => 4,
            "Strength" => 5,
            "Font size" => 6,
            _ => 99,
        };
        let decrease = on_change.clone();
        let increase = on_change;
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .child(
                div()
                    .id(("slider", slider_id))
                    .relative()
                    .flex_1()
                    .h(px(40.0))
                    .rounded_lg()
                    .bg(rgb(0xf1f1f2))
                    .overflow_hidden()
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            if matches!(slider_id, 5 | 6) && this.selected_annotation.is_some() {
                                this.record_annotation_undo();
                            }
                            this.slider_drag = Some(SliderDrag {
                                slider_id,
                                start_x: event.position.x,
                                start_value: value,
                            });
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .top_0()
                            .bottom_0()
                            .w(gpui::relative(value as f32 / 100.0))
                            .bg(hsla(211.0 / 360.0, 0.9, 0.88, 0.45)),
                    )
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .flex()
                            .items_center()
                            .px_3()
                            .text_sm()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(title),
                    ),
            )
            .child(
                div()
                    .id(("slider-minus", slider_id))
                    .w(px(28.0))
                    .h(px(40.0))
                    .rounded_lg()
                    .bg(rgb(0xf1f1f2))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .child("−")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        decrease(this, value.saturating_sub(2));
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .w(px(58.0))
                    .h(px(40.0))
                    .rounded_lg()
                    .bg(rgb(0xf1f1f2))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_sm()
                    .child(format!("{value}{suffix}")),
            )
            .child(
                div()
                    .id(("slider-plus", slider_id))
                    .w(px(28.0))
                    .h(px(40.0))
                    .rounded_lg()
                    .bg(rgb(0xf1f1f2))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .child("+")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        increase(this, value.saturating_add(2).min(100));
                        cx.notify();
                    })),
            )
    }

    fn annotation_text_field(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let editing = self.editing_text == Some(index);
        let text = self
            .annotations
            .get(index)
            .map(|mark| mark.text.clone())
            .unwrap_or_default();
        let empty = text.is_empty();
        div()
            .id("annotation-text-field")
            .w_full()
            .h(px(32.0))
            .px_2()
            .rounded_md()
            .border_1()
            .border_color(if editing {
                rgb(0x2997ff)
            } else {
                rgb(0xd9d9dc)
            })
            .bg(rgb(0xffffff))
            .flex()
            .items_center()
            .text_sm()
            .text_color(if empty && !editing { muted() } else { ink() })
            .cursor(CursorStyle::IBeam)
            .child(if editing {
                format!("{text}{}", if self.caret_visible { "|" } else { " " })
            } else if empty {
                "Click to type".to_string()
            } else {
                text
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                if index < this.annotations.len() {
                    this.record_annotation_undo();
                    this.selected_annotation = Some(index);
                    this.editing_text = Some(index);
                    this.caret_visible = true;
                    this.tool = Tool::Select;
                    this.toast = Some("Type text; Enter commits, Escape cancels".into());
                }
                cx.notify();
            }))
            .into_any_element()
    }

    fn annotation_style_controls(&self, cx: &mut Context<Self>) -> gpui::Div {
        let target_tool = self
            .selected_annotation
            .and_then(|index| self.annotations.get(index))
            .map(|mark| mark.tool)
            .unwrap_or(self.tool);
        let supports_color = matches!(
            target_tool,
            Tool::Rectangle
                | Tool::FilledRectangle
                | Tool::Ellipse
                | Tool::Line
                | Tool::Arrow
                | Tool::Pen
                | Tool::Number
                | Tool::Text
        );
        let supports_stroke = matches!(
            target_tool,
            Tool::Rectangle | Tool::Ellipse | Tool::Line | Tool::Arrow | Tool::Pen
        );
        let is_redaction = matches!(target_tool, Tool::Pixelate | Tool::Blur);
        let is_text = target_tool == Tool::Text;
        let selected_color = self
            .selected_annotation
            .and_then(|index| self.annotations.get(index))
            .map(|mark| mark.color)
            .unwrap_or(ANNOTATION_COLORS[self.annotation_color_index].1);
        let selected_stroke = self
            .selected_annotation
            .and_then(|index| self.annotations.get(index))
            .map(|mark| mark.stroke_width)
            .unwrap_or(self.annotation_stroke_width);
        let selected_text = self
            .selected_annotation
            .and_then(|index| self.annotations.get(index))
            .filter(|mark| mark.tool == Tool::Text);
        let selected_font_family = selected_text
            .map(|mark| mark.font_family)
            .unwrap_or(self.text_font_family);
        let selected_alignment = selected_text
            .map(|mark| mark.text_alignment)
            .unwrap_or(self.text_alignment);
        let selected_redaction_strength = self
            .selected_annotation
            .and_then(|index| self.annotations.get(index))
            .filter(|mark| mark.tool == Tool::Blur || mark.tool == Tool::Pixelate)
            .map(|mark| (mark.density * 100.0).round() as u8)
            .unwrap_or(self.redaction_strength);

        div()
            .flex()
            .flex_none()
            .flex_col()
            .gap_2()
            .when(supports_color, |this| {
                this.child(div().text_xs().text_color(muted()).child("Color"))
                    .child(
                        div().flex().flex_wrap().gap_2().children(
                            ANNOTATION_COLORS
                                .iter()
                                .enumerate()
                                .map(|(index, (_, color))| {
                                    let color = *color;
                                    div()
                                        .id(("annotation-color", index))
                                        .size(px(25.0))
                                        .rounded_md()
                                        .bg(rgb(color))
                                        .border_1()
                                        .border_color(if selected_color == color {
                                            blue()
                                        } else {
                                            line()
                                        })
                                        .cursor_pointer()
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if this.selected_annotation.is_some() {
                                                this.record_annotation_undo();
                                            }
                                            this.annotation_color_index = index;
                                            if let Some(mark) =
                                                this.selected_annotation.and_then(|selected| {
                                                    this.annotations.get_mut(selected)
                                                })
                                            {
                                                mark.color = color;
                                            }
                                            cx.notify();
                                        }))
                                }),
                        ),
                    )
            })
            .when(supports_stroke, |this| {
                this.child(div().text_xs().text_color(muted()).child("Stroke width"))
                    .child(div().flex().gap_1().children(
                        [2.0_f32, 4.0, 6.0, 8.0, 12.0].into_iter().enumerate().map(
                            |(index, width)| {
                                div()
                                    .id(("annotation-stroke", index))
                                    .flex_1()
                                    .h(px(32.0))
                                    .rounded_md()
                                    .bg(if (selected_stroke - width).abs() < 0.1 {
                                        rgb(0xffffff)
                                    } else {
                                        rgb(0xf0f0f1)
                                    })
                                    .when((selected_stroke - width).abs() < 0.1, |this| {
                                        this.shadow_sm().border_1().border_color(line())
                                    })
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_xs()
                                    .cursor_pointer()
                                    .child(format!("{}", width as u8))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if this.selected_annotation.is_some() {
                                            this.record_annotation_undo();
                                        }
                                        this.annotation_stroke_width = width;
                                        if let Some(mark) = this
                                            .selected_annotation
                                            .and_then(|selected| this.annotations.get_mut(selected))
                                        {
                                            mark.stroke_width = width;
                                        }
                                        cx.notify();
                                    }))
                            },
                        ),
                    ))
            })
            .when(is_redaction, |this| {
                this.child(self.slider_row(
                    "Strength",
                    selected_redaction_strength,
                    "%",
                    |studio, value| {
                        if studio.selected_annotation.is_some() {
                            studio.record_annotation_undo();
                        }
                        studio.redaction_strength = value.clamp(15, 100);
                        if let Some(mark) = studio
                            .selected_annotation
                            .and_then(|index| studio.annotations.get_mut(index))
                        {
                            mark.density = studio.redaction_strength as f32 / 100.0;
                        }
                        let _ = studio.rebuild_redactions();
                    },
                    cx,
                ))
            })
            .when(is_text, |this| {
                let size_value = self
                    .selected_annotation
                    .and_then(|index| self.annotations.get(index))
                    .map(|mark| mark.font_size)
                    .unwrap_or(self.text_font_size);
                this.child(div().text_xs().text_color(muted()).child("Text"))
                    .when_some(
                        self.selected_annotation.filter(|_| selected_text.is_some()),
                        |this, index| this.child(self.annotation_text_field(index, cx)),
                    )
                    .child(div().flex().gap_1().children(
                        ["Pro", "Compact", "Rounded"].into_iter().enumerate().map(
                            |(index, label)| {
                                div()
                                    .id(("text-family", index))
                                    .flex_1()
                                    .h(px(32.0))
                                    .rounded_md()
                                    .bg(if selected_font_family as usize == index {
                                        rgb(0xdcecff)
                                    } else {
                                        rgb(0xf0f0f1)
                                    })
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_xs()
                                    .cursor_pointer()
                                    .child(label)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if this.selected_annotation.is_some() {
                                            this.record_annotation_undo();
                                        }
                                        this.text_font_family = index as u8;
                                        if let Some(mark) = this
                                            .selected_annotation
                                            .and_then(|i| this.annotations.get_mut(i))
                                        {
                                            mark.font_family = index as u8;
                                        }
                                        cx.notify();
                                    }))
                            },
                        ),
                    ))
                    .child(self.slider_row(
                        "Font size",
                        size_value.round().clamp(10.0, 96.0) as u8,
                        " pt",
                        |studio, value| {
                            if studio.selected_annotation.is_some() {
                                studio.record_annotation_undo();
                            }
                            studio.set_slider_value(6, value);
                        },
                        cx,
                    ))
                    .child(
                        div().flex().gap_2().children(
                            [("B", 0_usize), ("I", 1), ("U", 2)].into_iter().map(
                                |(label, style)| {
                                    let enabled = match style {
                                        0 => selected_text
                                            .map(|mark| mark.bold)
                                            .unwrap_or(self.text_bold),
                                        1 => selected_text
                                            .map(|mark| mark.italic)
                                            .unwrap_or(self.text_italic),
                                        _ => selected_text
                                            .map(|mark| mark.underline)
                                            .unwrap_or(self.text_underline),
                                    };
                                    div()
                                        .id(("text-style", style))
                                        .w(px(42.0))
                                        .h(px(32.0))
                                        .rounded_md()
                                        .bg(if enabled {
                                            rgb(0xdcecff)
                                        } else {
                                            rgb(0xf0f0f1)
                                        })
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .cursor_pointer()
                                        .child(label)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if this.selected_annotation.is_some() {
                                                this.record_annotation_undo();
                                            }
                                            match style {
                                                0 => this.text_bold = !this.text_bold,
                                                1 => this.text_italic = !this.text_italic,
                                                _ => this.text_underline = !this.text_underline,
                                            }
                                            if let Some(mark) = this
                                                .selected_annotation
                                                .and_then(|index| this.annotations.get_mut(index))
                                            {
                                                match style {
                                                    0 => mark.bold = !mark.bold,
                                                    1 => mark.italic = !mark.italic,
                                                    _ => mark.underline = !mark.underline,
                                                }
                                            }
                                            cx.notify();
                                        }))
                                },
                            ),
                        ),
                    )
                    .child(
                        div().flex().gap_1().children(
                            ["Left", "Center", "Right", "Justify"]
                                .into_iter()
                                .enumerate()
                                .map(|(index, label)| {
                                    div()
                                        .id(("text-align", index))
                                        .flex_1()
                                        .h(px(30.0))
                                        .rounded_md()
                                        .bg(if selected_alignment as usize == index {
                                            rgb(0xdcecff)
                                        } else {
                                            rgb(0xf0f0f1)
                                        })
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .text_size(px(10.0))
                                        .cursor_pointer()
                                        .child(label)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if this.selected_annotation.is_some() {
                                                this.record_annotation_undo();
                                            }
                                            this.text_alignment = index as u8;
                                            if let Some(mark) = this
                                                .selected_annotation
                                                .and_then(|i| this.annotations.get_mut(i))
                                            {
                                                mark.text_alignment = index as u8;
                                            }
                                            cx.notify();
                                        }))
                                }),
                        ),
                    )
            })
    }

    /// A toolbar toggle for one recording source (system audio, mic, webcam).
    fn record_source_toggle(
        &self,
        id: &'static str,
        icon: &'static str,
        enabled: bool,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        div()
            .id(id)
            .h(px(34.0))
            .px_2()
            .flex()
            .items_center()
            .gap_1()
            .rounded_lg()
            .text_xs()
            .cursor_pointer()
            .bg(if enabled {
                rgb(0xe5f2ff)
            } else {
                rgb(0xf3f3f4)
            })
            .border_1()
            .border_color(if enabled { blue() } else { line() })
            .child(svg().path(icon).size(px(15.0)).text_color(if enabled {
                blue()
            } else {
                muted()
            }))
            .on_click(on_click)
    }

    /// The mic button: shows the selected input and opens the device menu.
    fn microphone_select(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let enabled = self.record_microphone;
        let label: SharedString = if !enabled {
            "No mic".into()
        } else {
            match &self.record_microphone_device {
                None => "Default mic".into(),
                Some(name) => self
                    .microphone_label
                    .clone()
                    .unwrap_or_else(|| name.clone())
                    .into(),
            }
        };
        div()
            .id("record-microphone")
            .h(px(34.0))
            .px_2()
            .flex()
            .items_center()
            .gap_1()
            .rounded_lg()
            .text_xs()
            .cursor_pointer()
            .bg(if enabled {
                rgb(0xe5f2ff)
            } else {
                rgb(0xf3f3f4)
            })
            .border_1()
            .border_color(if enabled { blue() } else { line() })
            .text_color(if enabled { blue() } else { muted() })
            .child(
                svg()
                    .path("icons/microphone.svg")
                    .size(px(15.0))
                    .text_color(if enabled { blue() } else { muted() }),
            )
            .child(
                div()
                    .max_w(px(160.0))
                    .overflow_hidden()
                    .text_ellipsis()
                    .whitespace_nowrap()
                    .child(label),
            )
            .child(div().text_color(muted()).child("▾"))
            .on_click(cx.listener(|this, _, _, cx| {
                this.microphone_picker = if this.microphone_picker.is_some() {
                    None
                } else {
                    Some(audio_sources())
                };
                cx.notify();
            }))
            .when_some(self.microphone_picker.as_ref(), |this, sources| {
                this.child(
                    gpui::deferred(
                        anchored()
                            .offset(point(px(0.0), px(38.0)))
                            .snap_to_window_with_margin(px(8.0))
                            .child(self.microphone_menu(sources, cx)),
                    )
                    .with_priority(2),
                )
            })
    }

    fn recording_controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
        if self.recording_state == RecordingState::Idle {
            return div()
                .id("recording-setup-controls")
                .flex()
                .items_center()
                .gap_1()
                .child(self.record_source_toggle(
                    "record-system-audio",
                    "icons/volume.svg",
                    self.record_system_audio,
                    cx.listener(|this, _, _, cx| {
                        this.record_system_audio = !this.record_system_audio;
                        cx.notify();
                    }),
                ))
                .child(self.microphone_select(cx))
                .child(self.record_source_toggle(
                    "record-camera",
                    "icons/video.svg",
                    self.record_camera,
                    cx.listener(|this, _, _, cx| {
                        this.record_camera = !this.record_camera;
                        this.sync_camera_preview(cx);
                        cx.notify();
                    }),
                ))
                .child(
                    div()
                        .id("toolbar-record")
                        .h(px(36.0))
                        .px_3()
                        .flex_none()
                        .flex()
                        .items_center()
                        .gap_2()
                        .rounded_lg()
                        .text_sm()
                        .cursor_pointer()
                        .hover(|style| style.bg(rgb(0xffe9eb)))
                        .child(
                            svg()
                                .path("icons/record.svg")
                                .size(px(17.0))
                                .text_color(rgb(0xe33442)),
                        )
                        .child("Record")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.start_recording(cx);
                            cx.notify();
                        })),
                );
        }

        let paused = self.recording_state == RecordingState::Paused;
        let settling = self.recording_busy
            || matches!(
                self.recording_state,
                RecordingState::Starting | RecordingState::Finishing
            );
        let status = match self.recording_state {
            RecordingState::Starting => "Starting…".to_string(),
            RecordingState::Finishing => "Saving…".to_string(),
            _ => self.recording_timecode(),
        };
        let icon_color = if settling {
            Hsla::from(rgb(0xa4a6aa))
        } else {
            ink()
        };

        div()
            .id("recording-controls")
            .h(px(40.0))
            .px_2()
            .flex_none()
            .flex()
            .items_center()
            .gap_1()
            .rounded_lg()
            .bg(rgb(0xf3f3f4))
            .border_1()
            .border_color(line())
            .child(
                div()
                    .w(px(10.0))
                    .h(px(10.0))
                    .rounded_full()
                    .bg(if paused || settling {
                        rgb(0x9c9fa4)
                    } else {
                        rgb(0xe33442)
                    }),
            )
            .child(
                div()
                    .w(px(62.0))
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(status),
            )
            .child(
                div()
                    .id("recording-pause-resume")
                    .size(px(30.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_md()
                    .when(!settling, |this| {
                        this.cursor_pointer().hover(|style| style.bg(rgb(0xe3e4e6)))
                    })
                    .child(
                        svg()
                            .path(if paused {
                                "icons/play.svg"
                            } else {
                                "icons/pause.svg"
                            })
                            .size(px(16.0))
                            .text_color(icon_color),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.run_recording_action(
                            if paused {
                                RecordingAction::Resume
                            } else {
                                RecordingAction::Pause
                            },
                            cx,
                        );
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .id("recording-restart")
                    .size(px(30.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_md()
                    .when(!settling, |this| {
                        this.cursor_pointer().hover(|style| style.bg(rgb(0xe3e4e6)))
                    })
                    .child(
                        svg()
                            .path("icons/restart.svg")
                            .size(px(16.0))
                            .text_color(icon_color),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.run_recording_action(RecordingAction::Restart, cx);
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .id("recording-stop")
                    .size(px(30.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_md()
                    .when(!settling, |this| {
                        this.cursor_pointer().hover(|style| style.bg(rgb(0xe3e4e6)))
                    })
                    .child(
                        svg()
                            .path("icons/stop.svg")
                            .size(px(16.0))
                            .text_color(icon_color),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.run_recording_action(RecordingAction::Stop, cx);
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .id("recording-discard")
                    .size(px(30.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_md()
                    .when(!settling, |this| {
                        this.cursor_pointer().hover(|style| style.bg(rgb(0xffe4e6)))
                    })
                    .child(
                        svg()
                            .path("icons/trash.svg")
                            .size(px(16.0))
                            .text_color(if settling {
                                icon_color
                            } else {
                                Hsla::from(rgb(0xd62f3d))
                            }),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.run_recording_action(RecordingAction::Discard, cx);
                        cx.notify();
                    })),
            )
    }

    fn fill_picker(&self, cx: &mut Context<Self>) -> gpui::Div {
        let grid = div().flex().flex_none().flex_wrap().gap_2().w_full();
        match self.wallpaper_tab {
            0 => grid.children(SOLID_BACKGROUNDS.iter().enumerate().map(
                |(index, (title, color))| {
                    let title = *title;
                    div()
                        .id(("background-color", index))
                        .size(px(27.0))
                        .rounded_md()
                        .bg(rgb(*color))
                        .cursor_pointer()
                        .when(self.color_index == index, |this| {
                            this.border_2().border_color(blue())
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.color_index = index;
                            this.custom_wallpaper = None;
                            this.toast = Some(format!("{title} background selected").into());
                            cx.notify();
                        }))
                },
            )),
            1 => grid.children(GRADIENT_BACKGROUNDS.iter().copied().enumerate().map(
                |(index, preset)| {
                    let (base, overlay) = gradient_layers(preset);
                    div()
                        .id(("background-gradient", index))
                        .relative()
                        .size(px(27.0))
                        .rounded_md()
                        .overflow_hidden()
                        .bg(base)
                        .cursor_pointer()
                        .child(div().absolute().inset_0().bg(overlay))
                        .when(self.gradient_index == index, |this| {
                            this.border_2().border_color(blue())
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.gradient_index = index;
                            this.custom_wallpaper = None;
                            this.toast = Some(format!("{} gradient selected", preset.title).into());
                            cx.notify();
                        }))
                },
            )),
            _ if self.library_tab == 0 => {
                let selected_path = self.custom_wallpaper.clone();
                grid.when_some(selected_path, |this, path| {
                    this.child(
                        div()
                            .id("recent-wallpaper")
                            .w(px(84.0))
                            .h(px(58.0))
                            .rounded_lg()
                            .overflow_hidden()
                            .border_2()
                            .border_color(blue())
                            .child(img(path).size_full().object_fit(ObjectFit::Cover)),
                    )
                })
                .child(
                    div()
                        .id("add-wallpaper")
                        .w(px(84.0))
                        .h(px(58.0))
                        .rounded_lg()
                        .border_1()
                        .border_color(line())
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_lg()
                        .cursor_pointer()
                        .child("+")
                        .on_click(cx.listener(|_, _, _, cx| {
                            let prompt = cx.prompt_for_paths(PathPromptOptions {
                                files: true,
                                directories: false,
                                multiple: false,
                                prompt: Some("Choose wallpaper".into()),
                            });
                            cx.spawn(async move |weak, cx| {
                                let selected = match prompt.await {
                                    Ok(Ok(Some(paths))) => paths.into_iter().next(),
                                    _ => None,
                                };
                                weak.update(cx, |this, cx| {
                                    if let Some(path) = selected {
                                        this.custom_wallpaper = Some(path);
                                        this.toast = Some("Custom wallpaper selected".into());
                                    }
                                    cx.notify();
                                })
                                .ok();
                            })
                            .detach();
                        })),
                )
            }
            _ => {
                let paths: &'static [&'static str] = if self.library_tab == 1 {
                    &UIHSSN_WALLPAPERS
                } else {
                    &FAYAZ_WALLPAPERS
                };
                grid.children(paths.iter().enumerate().map(|(index, path)| {
                    let path = *path;
                    div()
                        .id(("wallpaper-tile", self.library_tab * 100 + index))
                        .w(px(84.0))
                        .h(px(58.0))
                        .rounded_lg()
                        .overflow_hidden()
                        .cursor_pointer()
                        .when(
                            self.custom_wallpaper.is_none() && self.wallpaper_asset == path,
                            |this| this.border_2().border_color(blue()),
                        )
                        .child(img(path).size_full().object_fit(ObjectFit::Cover))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.wallpaper_asset = path;
                            this.custom_wallpaper = None;
                            this.toast = Some("Wallpaper selected".into());
                            cx.notify();
                        }))
                }))
            }
        }
    }

    fn mock_capture(
        &self,
        cx: &mut Context<Self>,
        canvas_width: Pixels,
        canvas_height: Pixels,
        composited: Option<Arc<RenderImage>>,
    ) -> impl IntoElement {
        let solid = SOLID_BACKGROUNDS[self.color_index.min(SOLID_BACKGROUNDS.len() - 1)].1;
        let gradient =
            GRADIENT_BACKGROUNDS[self.gradient_index.min(GRADIENT_BACKGROUNDS.len() - 1)];
        let (gradient_base, gradient_overlay) = gradient_layers(gradient);
        let background_base: Background = match self.wallpaper_tab {
            0 => rgb(solid).into(),
            1 => gradient_base,
            _ => rgb(0x111214).into(),
        };
        let custom_wallpaper = self.custom_wallpaper.clone();
        let border_colors = [0xffc928, 0x22b45d, 0x22bfc2, 0x3678ef, 0x8c4ce8, 0xec3d87];
        let border_color = border_colors[self.border_color.min(border_colors.len() - 1)];
        // Swift stores border thickness as 0.2%...8% of the screenshot's
        // shortest edge. At this preview size the full range is about 0...48px.
        let border_width = if self.border {
            px(self.border_thickness as f32 * 0.48)
        } else {
            px(0.0)
        };
        // The original app maps 0...100% to a radius of 0...12% of the
        // screenshot's shortest edge. At this preview size that is about 64px.
        let corner_radius = px(self.corners as f32 * 0.64);
        let border_tint = Hsla::from(rgb(border_color)).opacity(self.border_opacity as f32 / 100.0);
        let strength = self.shadow as f32 / 100.0;
        let (radius_scale, offset_scale, opacity_scale) = match self.shadow_style {
            0 => (1.0, 0.3, 1.0),  // Soft
            1 => (1.2, 0.9, 0.85), // Long
            2 => (1.6, 0.0, 0.7),  // Glow
            _ => (0.8, 0.2, 1.1),  // Crisp
        };
        let shadow_radius = 85.0 * strength * radius_scale;
        let shadow_alpha = (0.08 + strength * 1.35)
            .min(0.35)
            .mul_add(opacity_scale, 0.0)
            .min(0.5);
        let shadow_layers = if self.shadow == 0 {
            Vec::new()
        } else {
            vec![BoxShadow {
                color: hsla(0.0, 0.0, 0.0, shadow_alpha),
                offset: point(px(0.0), px(shadow_radius * offset_scale)),
                blur_radius: px(shadow_radius),
                spread_radius: px(0.0),
            }]
        };
        let has_capture = self.captured_path.is_some();
        // `object-fit: contain` can place the bitmap somewhere inside its box.
        // Size the box to the fitted bitmap instead so its rounded clipping,
        // border, shadow, annotations, and pointer hit testing share one rect.
        let image_bounds = fitted_image_bounds(
            Bounds {
                origin: point(px(0.0), px(0.0)),
                size: size(canvas_width, canvas_height),
            },
            has_capture,
            self.captured_dimensions,
            self.padding,
            self.border,
            self.border_thickness,
        );
        let image_x = image_bounds.origin.x;
        let image_y = image_bounds.origin.y;
        let image_width = image_bounds.size.width;
        let image_height = image_bounds.size.height;
        let card_x = image_x - border_width;
        let card_y = image_y - border_width;
        let card_width = image_width + border_width * 2.0;
        let card_height = image_height + border_width * 2.0;
        let card_radius = corner_radius + border_width;
        let shadow_x = if self.border { card_x } else { image_x };
        let shadow_y = if self.border { card_y } else { image_y };
        let shadow_width = if self.border { card_width } else { image_width };
        let shadow_height = if self.border {
            card_height
        } else {
            image_height
        };
        let shadow_radius_for_card = if self.border {
            card_radius
        } else {
            corner_radius
        };
        let mut annotations = self.annotations.clone();
        if let Some(draft) = self.annotation_draft.clone() {
            annotations.push(draft);
        }
        let committed_count = self.annotations.len();
        // Animated scenes paint each mark at its state for the playhead time.
        let selected_annotation = self.selected_annotation;
        let editing_text = self.editing_text;
        let (annotations, painted_indices): (Vec<AnnotationMark>, Vec<usize>) =
            if self.animation_active {
                let time = self.video_position;
                let mut marks = Vec::new();
                let mut indices = Vec::new();
                for (index, mark) in annotations.iter().enumerate() {
                    if let Some(animated) = timed::editor_mark(
                        mark,
                        time,
                        selected_annotation == Some(index) || editing_text == Some(index),
                    ) {
                        marks.push(animated);
                        indices.push(index);
                    }
                }
                (marks, indices)
            } else {
                let indices = (0..annotations.len()).collect();
                (annotations, indices)
            };
        let caret_visible = self.caret_visible;
        let crop_active = self.crop_active;
        let crop_rect = self.crop_rect;
        let crop_aspect_locked = self.crop_aspect != 0;
        let entity = cx.entity();
        let captured_dimensions = self.captured_dimensions;
        let padding = self.padding;
        let border = self.border;
        let border_thickness = self.border_thickness;
        let displayed_capture = self
            .processed_capture_path
            .clone()
            .or_else(|| self.captured_path.clone());
        let displayed_capture_image = self.displayed_capture_image.clone();
        let needs_path_fallback = displayed_capture_image.is_none();
        let animation_active = self.animation_active;
        // While animating, the still image is cropped by the same viewport
        // the exporter uses; annotations move with it.
        let (view_zoom, view_left, view_top) = if animation_active {
            let frame = self.video_viewport_timeline.frame_at(self.video_position);
            let (left, top, _) = visible_rect(frame);
            (frame.magnification.max(1.0) as f32, left as f32, top as f32)
        } else {
            (1.0, 0.0, 0.0)
        };
        let media_bounds_store = self.video_media_bounds.clone();
        let scene_bounds_store = self.scene_canvas_bounds.clone();
        // A composited preview lays the media out with the compositor's own
        // geometry, so annotations and hit testing must follow that rect.
        let composited_style = composited.is_some().then(|| self.scene_style());
        let composited_active = composited.is_some();
        // Under a 3D transform the compositor draws the annotations too.
        let paint_gpui_annotations = self.annotations_paint_flat();
        let select_tool = self.tool == Tool::Select;
        // Focus / pan-end markers of the selected motion region.
        let motion_markers = if animation_active {
            let (_, projection) =
                self.preview_projection(f32::from(canvas_width), f32::from(canvas_height));
            self.motion_marker_points(&projection)
        } else {
            Vec::new()
        };
        let zoomed = move |bounds: Bounds<Pixels>| Bounds {
            origin: point(
                bounds.origin.x - bounds.size.width * view_zoom * view_left,
                bounds.origin.y - bounds.size.height * view_zoom * view_top,
            ),
            size: size(
                bounds.size.width * view_zoom,
                bounds.size.height * view_zoom,
            ),
        };
        div()
            .id("editable-canvas")
            .w(canvas_width)
            .h(canvas_height)
            .flex_none()
            .shadow_lg()
            .bg(background_base)
            .relative()
            .overflow_hidden()
            .when(self.wallpaper_tab == 1, |this| {
                this.child(div().absolute().inset_0().bg(gradient_overlay))
            })
            .when(
                self.wallpaper_tab == 2 && custom_wallpaper.is_none(),
                |this| {
                    this.child(
                        img(self.wallpaper_asset)
                            .absolute()
                            .inset_0()
                            .size_full()
                            .object_fit(ObjectFit::Cover),
                    )
                },
            )
            .when_some(
                if self.wallpaper_tab == 2 {
                    custom_wallpaper
                } else {
                    None
                },
                |this, path| {
                    this.child(
                        img(path)
                            .absolute()
                            .inset_0()
                            .size_full()
                            .object_fit(ObjectFit::Cover),
                    )
                },
            )
            .child(
                div()
                    .absolute()
                    .left(shadow_x)
                    .top(shadow_y)
                    .w(shadow_width)
                    .h(shadow_height)
                    .rounded(shadow_radius_for_card)
                    .shadow(shadow_layers),
            )
            .when(
                self.border && self.border_thickness > 0 && self.border_opacity > 0,
                |this| {
                    this.child(
                        div()
                            .absolute()
                            .left(card_x)
                            .top(card_y)
                            .w(card_width)
                            .h(card_height)
                            .rounded(card_radius)
                            .bg(border_tint),
                    )
                },
            )
            .child(
                div()
                    .absolute()
                    .left(image_x)
                    .top(image_y)
                    .w(image_width)
                    .h(image_height)
                    .bg(rgb(0xfafafa))
                    .border_1()
                    .border_color(rgb(0xd6dde6))
                    .overflow_hidden()
                    .rounded(corner_radius)
                    .when(has_capture, |this| {
                        this.child(
                            img("mock-capture.svg")
                                .size_full()
                                .object_fit(ObjectFit::Contain)
                                .rounded(corner_radius),
                        )
                    })
                    .when(!has_capture, |this| {
                        this.flex()
                            .flex_col()
                            .items_center()
                            .justify_center()
                            .gap_3()
                            .child(
                                svg()
                                    .path("icons/capture.svg")
                                    .size(px(46.0))
                                    .text_color(hsla(220.0 / 360.0, 0.05, 0.78, 1.0)),
                            )
                            .child(
                                div()
                                    .text_lg()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(ink())
                                    .child("Nothing captured yet"),
                            )
                            .child(div().text_sm().text_color(muted()).child(
                                "Take a screenshot, record your screen, or open a saved recording",
                            ))
                            .child(
                                div()
                                    .mt_2()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .id("empty-take-screenshot")
                                            .px_4()
                                            .h(px(36.0))
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .rounded_lg()
                                            .bg(blue())
                                            .text_sm()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(rgb(0xffffff))
                                            .cursor_pointer()
                                            .hover(|style| {
                                                style.bg(hsla(211.0 / 360.0, 0.88, 0.45, 1.0))
                                            })
                                            .child(
                                                svg()
                                                    .path("icons/capture.svg")
                                                    .size(px(16.0))
                                                    .text_color(rgb(0xffffff)),
                                            )
                                            .child("Take screenshot")
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.begin_screen_capture(cx)
                                            })),
                                    )
                                    .child(
                                        div()
                                            .id("empty-record-video")
                                            .px_4()
                                            .h(px(36.0))
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .rounded_lg()
                                            .border_1()
                                            .border_color(line())
                                            .bg(rgb(0xffffff))
                                            .text_sm()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .cursor_pointer()
                                            .hover(|style| style.bg(rgb(0xf0f1f3)))
                                            .child(
                                                svg()
                                                    .path("icons/record.svg")
                                                    .size(px(16.0))
                                                    .text_color(rgb(0xd92d3a)),
                                            )
                                            .child("Record video")
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.start_recording(cx)
                                            })),
                                    )
                                    .child(
                                        div()
                                            .id("empty-open-recording")
                                            .px_4()
                                            .h(px(36.0))
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .rounded_lg()
                                            .border_1()
                                            .border_color(line())
                                            .bg(rgb(0xffffff))
                                            .text_sm()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .cursor_pointer()
                                            .hover(|style| style.bg(rgb(0xf0f1f3)))
                                            .child(
                                                svg()
                                                    .path("icons/play.svg")
                                                    .size(px(16.0))
                                                    .text_color(ink()),
                                            )
                                            .child("Open recording")
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.open_video_project_dialog(cx)
                                            })),
                                    ),
                            )
                    }),
            )
            .when_some(displayed_capture_image, |this, image| {
                this.child(
                    div()
                        .absolute()
                        .left(image_x)
                        .top(image_y)
                        .w(image_width)
                        .h(image_height)
                        .overflow_hidden()
                        .rounded(corner_radius)
                        .child(
                            img(image)
                                .absolute()
                                .left(-(image_width * view_zoom * view_left))
                                .top(-(image_height * view_zoom * view_top))
                                .w(image_width * view_zoom)
                                .h(image_height * view_zoom)
                                .object_fit(ObjectFit::Contain)
                                .rounded(corner_radius),
                        ),
                )
            })
            .when_some(
                if needs_path_fallback {
                    displayed_capture
                } else {
                    None
                },
                |this, path| {
                    this.child(
                        div()
                            .absolute()
                            .left(image_x)
                            .top(image_y)
                            .w(image_width)
                            .h(image_height)
                            .overflow_hidden()
                            .rounded(corner_radius)
                            .child(
                                img(path)
                                    .size_full()
                                    .object_fit(ObjectFit::Contain)
                                    .rounded(corner_radius),
                            ),
                    )
                },
            )
            .when_some(composited, |this, image| {
                this.child(img(image).absolute().inset_0().size_full())
            })
            .child(
                canvas(
                    // The hitbox lets occluding overlays (dialogs) shadow the
                    // raw mouse listeners registered below.
                    move |bounds, window, _| {
                        (
                            annotations,
                            window.insert_hitbox(bounds, gpui::HitboxBehavior::Normal),
                        )
                    },
                    move |bounds, (annotations, hitbox), window, cx| {
                        let image_bounds = match composited_style.as_ref() {
                            Some(style) => {
                                let (source_width, source_height) =
                                    captured_dimensions.unwrap_or((1200, 720));
                                let media = recording::scene::SceneGeometry::layout(
                                    f64::from(bounds.size.width),
                                    f64::from(bounds.size.height),
                                    source_width as f64,
                                    source_height as f64,
                                    style,
                                )
                                .media;
                                Bounds {
                                    origin: point(
                                        bounds.origin.x + px(media.x as f32),
                                        bounds.origin.y + px(media.y as f32),
                                    ),
                                    size: size(px(media.width as f32), px(media.height as f32)),
                                }
                            }
                            None => fitted_image_bounds(
                                bounds,
                                has_capture,
                                captured_dimensions,
                                padding,
                                border,
                                border_thickness,
                            ),
                        };
                        if let Ok(mut stored) = media_bounds_store.lock() {
                            *stored = Some(image_bounds);
                        }
                        if let Ok(mut stored) = scene_bounds_store.lock() {
                            *stored = Some(bounds);
                        }
                        let paint_bounds = zoomed(image_bounds);
                        // While animating, drawing happens in the zoomed view.
                        let interaction_bounds = if animation_active {
                            paint_bounds
                        } else {
                            image_bounds
                        };
                        let painted_indices = painted_indices.clone();
                        let annotation_bounds = window.with_content_mask(
                            Some(ContentMask {
                                bounds: image_bounds,
                            }),
                            |window| {
                                if !paint_gpui_annotations {
                                    return Vec::new();
                                }
                                paint_highlights(&annotations, paint_bounds, window);
                                let mut annotation_bounds = Vec::with_capacity(annotations.len());
                                for (painted, mark) in annotations.iter().enumerate() {
                                    let index = painted_indices[painted];
                                    let rendered_bounds = paint_annotation(
                                        mark,
                                        if mark.pinned {
                                            image_bounds
                                        } else {
                                            paint_bounds
                                        },
                                        index >= committed_count,
                                        editing_text == Some(index) && caret_visible,
                                        window,
                                        cx,
                                    );
                                    annotation_bounds.push(rendered_bounds);
                                    if selected_annotation == Some(index) {
                                        let selected_bounds = rendered_bounds;
                                        window.paint_quad(quad(
                                            selected_bounds,
                                            px(3.0),
                                            hsla(0.0, 0.0, 0.0, 0.0),
                                            px(2.0),
                                            rgb(0x2997ff),
                                            Default::default(),
                                        ));
                                        window.paint_quad(quad(
                                            Bounds {
                                                origin: point(
                                                    selected_bounds.origin.x
                                                        + selected_bounds.size.width
                                                        - px(5.0),
                                                    selected_bounds.origin.y
                                                        + selected_bounds.size.height
                                                        - px(5.0),
                                                ),
                                                size: size(px(10.0), px(10.0)),
                                            },
                                            px(5.0),
                                            rgb(0xffffff),
                                            px(2.0),
                                            rgb(0x2997ff),
                                            Default::default(),
                                        ));
                                    }
                                }
                                annotation_bounds
                            },
                        );
                        if !motion_markers.is_empty() {
                            window.with_content_mask(
                                Some(ContentMask {
                                    bounds: image_bounds,
                                }),
                                |window| {
                                    scene_ui::paint_motion_markers(&motion_markers, bounds, window)
                                },
                            );
                        }
                        if crop_active {
                            paint_crop_overlay(crop_rect, image_bounds, crop_aspect_locked, window);
                        }

                        window.on_mouse_event({
                            let entity = entity.clone();
                            let annotation_bounds = annotation_bounds.clone();
                            move |event: &MouseDownEvent, _, window, cx| {
                                let crop_hit_bounds = Bounds {
                                    origin: point(
                                        image_bounds.origin.x - px(18.0),
                                        image_bounds.origin.y - px(18.0),
                                    ),
                                    size: size(
                                        image_bounds.size.width + px(36.0),
                                        image_bounds.size.height + px(36.0),
                                    ),
                                };
                                if event.button != MouseButton::Left
                                    || !hitbox.is_hovered(window)
                                    || if crop_active {
                                        !crop_hit_bounds.contains(&event.position)
                                    } else {
                                        !bounds.contains(&event.position)
                                    }
                                {
                                    return;
                                }
                                entity.update(cx, |this, cx| {
                                    this.focus_handle.focus(window);
                                    // Drawing through a projected preview lands where
                                    // the pointer is on the card, not on the canvas.
                                    let flat = if composited_active {
                                        this.flat_pointer_position(
                                            event.position,
                                            bounds,
                                            interaction_bounds,
                                        )
                                    } else {
                                        event.position
                                    };
                                    if animation_active && this.walkthrough_mode {
                                        if let Some(point) =
                                            this.media_point_at(event.position, bounds)
                                        {
                                            this.add_walkthrough_stop(point);
                                        }
                                    } else if animation_active
                                        && (select_tool || this.video_selected_zoom_cue.is_some())
                                    {
                                        // Motion mode: clicks choose the focus of the
                                        // selected region; otherwise they pick an
                                        // annotation first and fall back to the media.
                                        if this.video_selected_zoom_cue.is_none()
                                            && interaction_bounds.contains(&flat)
                                            && !this.annotations.is_empty()
                                        {
                                            this.pointer_down(
                                                flat,
                                                interaction_bounds,
                                                &annotation_bounds,
                                            );
                                        }
                                        if this.selected_annotation.is_some() {
                                            this.video_selected_press = None;
                                            this.scene_selection = SceneSelection::Scene;
                                        } else {
                                            this.toast = None;
                                            this.scene_pointer_down(
                                                event.position,
                                                bounds,
                                                &event.modifiers,
                                                event.click_count,
                                                cx,
                                            );
                                        }
                                    } else if animation_active {
                                        // Drawing tools place timed marks at the playhead.
                                        this.pause_video_playback();
                                        if interaction_bounds.contains(&flat) {
                                            this.pointer_down(
                                                flat,
                                                interaction_bounds,
                                                &annotation_bounds,
                                            );
                                        }
                                    } else if this.crop_active {
                                        this.crop_pointer_down(event.position, image_bounds);
                                    } else if !paint_gpui_annotations && select_tool {
                                        // Transformed media: select moves the card.
                                        this.scene_pointer_down(
                                            event.position,
                                            bounds,
                                            &event.modifiers,
                                            event.click_count,
                                            cx,
                                        );
                                    } else if select_tool {
                                        if image_bounds.contains(&event.position) {
                                            this.pointer_down(
                                                event.position,
                                                image_bounds,
                                                &annotation_bounds,
                                            );
                                        }
                                        if this.selected_annotation.is_none() {
                                            this.scene_pointer_down(
                                                event.position,
                                                bounds,
                                                &event.modifiers,
                                                event.click_count,
                                                cx,
                                            );
                                        }
                                    } else if interaction_bounds.contains(&flat) {
                                        this.pointer_down(
                                            flat,
                                            interaction_bounds,
                                            &annotation_bounds,
                                        );
                                    }
                                    cx.notify();
                                });
                            }
                        });
                        window.on_mouse_event({
                            let entity = entity.clone();
                            move |event: &MouseMoveEvent, _, _, cx| {
                                if !event.dragging() {
                                    return;
                                }
                                entity.update(cx, |this, cx| {
                                    if this.focus_drag.is_some() {
                                        this.drag_motion_marker(event.position, bounds, cx);
                                        cx.notify();
                                        return;
                                    }
                                    if this.media_drag.is_some() {
                                        this.update_media_drag(event.position);
                                        cx.notify();
                                        return;
                                    }
                                    if this.crop_active {
                                        this.crop_pointer_move(event.position, image_bounds);
                                    } else {
                                        let flat = if composited_active {
                                            this.flat_pointer_position(
                                                event.position,
                                                bounds,
                                                interaction_bounds,
                                            )
                                        } else {
                                            event.position
                                        };
                                        this.pointer_move(flat, interaction_bounds);
                                    }
                                    cx.notify();
                                });
                            }
                        });
                        window.on_mouse_event(move |event: &MouseUpEvent, _, _, cx| {
                            if event.button != MouseButton::Left {
                                return;
                            }
                            entity.update(cx, |this, cx| {
                                this.focus_drag = None;
                                this.end_media_drag();
                                let flat = if composited_active {
                                    this.flat_pointer_position(
                                        event.position,
                                        bounds,
                                        interaction_bounds,
                                    )
                                } else {
                                    event.position
                                };
                                if this.crop_active {
                                    this.crop_drag = None;
                                    this.pointer_is_down = false;
                                } else if this.pointer_up(flat, interaction_bounds) {
                                    if let Err(error) = this.rebuild_redactions() {
                                        this.toast = Some(
                                            format!("Could not render redaction: {error}").into(),
                                        );
                                    }
                                }
                                cx.notify();
                            });
                        });
                    },
                )
                .absolute()
                .inset_0(),
            )
            .when(
                self.tool == Tool::Text || self.editing_text.is_some(),
                |this| this.cursor(CursorStyle::IBeam),
            )
            .when(
                self.tool != Tool::Text && self.editing_text.is_none(),
                |this| this.cursor_crosshair(),
            )
            .when(self.scene_selection == SceneSelection::Media, |this| {
                this.cursor(CursorStyle::OpenHand)
            })
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _, cx| {
                if this.scene_scroll(event) {
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
    }

    fn render_video(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        // Keyboard transport (space, arrows, split) dispatches through the
        // focused element, and nothing else in the video editor takes focus.
        if window.focused(cx).is_none() {
            self.focus_handle.focus(window);
        }
        self.autosave_scene_style();
        if self.video_extras_pending {
            self.video_extras_pending = false;
            self.spawn_video_extras(cx);
        }
        self.ensure_camera_frame(cx);
        let (canvas_width, canvas_height) = self.canvas_budget(window.viewport_size());
        let video_canvas = self.scene_canvas(canvas_width, canvas_height, cx);
        let top_bar = self.top_bar(cx);
        let canvas_area = self.canvas_area(video_canvas, cx);
        let timeline = self.timeline_bar(cx);
        let sidebar = self.inspector_visible.then(|| self.sidebar(cx));
        let speed_dialog = self.video_speed_dialog(cx);

        div()
            .size_full()
            .min_w(px(980.0))
            .min_h(px(680.0))
            .bg(rgb(0xf3f3f4))
            .text_color(ink())
            .font_family("Inter")
            .flex()
            .flex_col()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if this.handle_video_key(event, cx) {
                    cx.notify();
                }
            }))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                let mut changed = this.update_slider_drag(event);
                if event.dragging() && this.annotation_drag.is_some() {
                    if this.update_annotation_drag(event.position.x) {
                        changed = true;
                    }
                } else if event.dragging() && this.media_drag.is_some() {
                    if this.update_media_drag(event.position) {
                        changed = true;
                    }
                } else if event.dragging() {
                    if let Some(drag) = this.video_move_drag.as_mut() {
                        drag.current_x = event.position.x;
                        // Reordering while a preview render is in flight is
                        // safe: the next apply supersedes it via the
                        // generation token, so don't gate on video_edit_busy.
                        if !drag.active && (drag.current_x - drag.start_x).abs() > px(6.0) {
                            drag.active = true;
                            this.video_seek_drag = None;
                        }
                        if drag.active {
                            changed = true;
                        }
                    }
                    if this.video_zoom_drag.is_some() {
                        this.update_video_zoom_drag(event.position.x);
                        changed = true;
                    } else if this.video_trim_drag.is_some() {
                        this.update_video_trim(event.position.x);
                        changed = true;
                    } else if let Some((start_x, start_position)) = this.video_seek_drag {
                        let delta = (event.position.x - start_x) / px(1.0);
                        let content_width =
                            this.video_timeline_viewport_width() * this.video_timeline_zoom;
                        this.video_position = (start_position
                            + delta as f64 / content_width * this.video_duration)
                            .clamp(0.0, this.video_duration);
                        changed = true;
                    }
                }
                if changed {
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    if this.end_media_drag() {
                        cx.notify();
                    }
                    if this.end_annotation_drag() {
                        cx.notify();
                    }
                    if let Some(drag) = this.video_move_drag.take() {
                        if drag.active {
                            this.commit_video_move_drag(drag, cx);
                            this.slider_drag = None;
                            cx.notify();
                            return;
                        }
                    }
                    if this.video_zoom_drag.is_some() {
                        this.commit_video_zoom_drag(cx);
                    } else if this.video_trim_drag.is_some() {
                        this.commit_video_trim(cx);
                    } else if this.video_seek_drag.take().is_some() {
                        this.seek_video(this.video_position, cx);
                    }
                    if this
                        .slider_drag
                        .take()
                        .is_some_and(|drag| drag.slider_id == MOTION_ZOOM_SLIDER)
                    {
                        this.persist_video_zoom_cues(cx);
                    }
                }),
            )
            .child(top_bar)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .flex()
                            .flex_col()
                            .child(canvas_area)
                            .child(timeline),
                    )
                    .when_some(sidebar, |this, sidebar| this.child(sidebar)),
            )
            .when_some(speed_dialog, |this, dialog| {
                this.child(gpui::deferred(dialog).with_priority(1))
            })
            .when_some(self.microphone_menu_backdrop(cx), |this, backdrop| {
                this.child(gpui::deferred(backdrop).with_priority(1))
            })
            .into_any_element()
    }
}

impl Render for Studio {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.drop_retired_images(window);
        if self.video_project.is_some() {
            return self.render_video(window, cx);
        }
        let (canvas_width, canvas_height) = self.canvas_budget(window.viewport_size());
        let composited = if self.preview_needs_compositor() {
            self.scene_preview_image(canvas_width, canvas_height)
        } else {
            None
        };
        let canvas = self
            .mock_capture(cx, canvas_width, canvas_height, composited)
            .into_any_element();
        let top_bar = self.top_bar(cx);
        let canvas_area = self.canvas_area(canvas, cx);
        let timeline = self.animation_active.then(|| self.timeline_bar(cx));
        let sidebar = self.inspector_visible.then(|| self.sidebar(cx));

        div()
            .size_full()
            .min_w(px(980.0))
            .min_h(px(680.0))
            .bg(rgb(0xf3f3f4))
            .text_color(ink())
            .font_family("Inter")
            .flex()
            .flex_col()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if this.handle_animation_key(event, cx) {
                    cx.notify();
                    return;
                }
                if this.handle_key(event) {
                    if this.processed_capture_path.is_some() {
                        let _ = this.rebuild_redactions();
                    }
                    cx.notify();
                }
            }))
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                let mut changed = this.update_slider_drag(event);
                if event.dragging() && this.media_drag.is_some() {
                    changed |= this.update_media_drag(event.position);
                } else if this.animation_active && event.dragging() {
                    if this.video_zoom_drag.is_some() {
                        this.update_video_zoom_drag(event.position.x);
                        changed = true;
                    } else if this.annotation_drag.is_some() {
                        changed |= this.update_annotation_drag(event.position.x);
                    } else if let Some((start_x, start_position)) = this.video_seek_drag {
                        let delta = (event.position.x - start_x) / px(1.0);
                        let content_width =
                            this.video_timeline_viewport_width() * this.video_timeline_zoom;
                        this.video_position = (start_position
                            + delta as f64 / content_width * this.video_duration)
                            .clamp(0.0, this.video_duration);
                        changed = true;
                    }
                }
                if changed {
                    cx.notify();
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.end_media_drag();
                    this.end_annotation_drag();
                    if this.video_zoom_drag.is_some() {
                        this.commit_video_zoom_drag(cx);
                    }
                    this.video_seek_drag = None;
                    let rebuild = this.slider_drag.is_some_and(|drag| drag.slider_id == 5);
                    let motion_slider = this
                        .slider_drag
                        .is_some_and(|drag| drag.slider_id == MOTION_ZOOM_SLIDER);
                    this.slider_drag = None;
                    if rebuild {
                        let _ = this.rebuild_redactions();
                    }
                    if motion_slider {
                        this.persist_video_zoom_cues(cx);
                    }
                    cx.notify();
                }),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    let rebuild = this.slider_drag.is_some_and(|drag| drag.slider_id == 5);
                    this.slider_drag = None;
                    if rebuild {
                        let _ = this.rebuild_redactions();
                    }
                    cx.notify();
                }),
            )
            .child(top_bar)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .flex()
                            .flex_col()
                            .child(canvas_area)
                            .when_some(timeline, |this, timeline| this.child(timeline)),
                    )
                    .when_some(sidebar, |this, sidebar| this.child(sidebar)),
            )
            .when_some(self.microphone_menu_backdrop(cx), |this, backdrop| {
                this.child(gpui::deferred(backdrop).with_priority(1))
            })
            .into_any_element()
    }
}

fn main() {
    let arguments: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let initial_recording = arguments
        .iter()
        .find(|path| path.extension().and_then(|value| value.to_str()) == Some("screendroprec"))
        .cloned();
    // `screendrop shot.png` opens an existing image in the screenshot editor.
    let initial_image = arguments
        .iter()
        .find(|path| {
            path.extension()
                .and_then(|value| value.to_str())
                .map(|extension| extension.to_ascii_lowercase())
                .is_some_and(|extension| {
                    matches!(extension.as_str(), "png" | "jpg" | "jpeg" | "webp")
                })
        })
        .cloned();
    Application::new()
        .with_assets(Assets {
            base: asset_directory(),
        })
        .run(move |cx: &mut App| {
            let bounds = Bounds::centered(None, size(px(1440.0), px(900.0)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(980.0), px(680.0))),
                    app_id: Some("com.screendrop.Screendrop".into()),
                    titlebar: Some(TitlebarOptions {
                        title: Some("Lahza".into()),
                        appears_transparent: false,
                        traffic_light_position: None,
                    }),
                    window_decorations: Some(WindowDecorations::Client),
                    ..Default::default()
                },
                move |window, cx| {
                    let window_handle = window.window_handle();
                    let studio = cx.new(|cx| {
                        Studio::new(
                            window_handle,
                            initial_recording.clone(),
                            initial_image.clone(),
                            cx,
                        )
                    });
                    let weak = studio.downgrade();
                    window.on_window_should_close(cx, move |window, cx| {
                        weak.update(cx, |studio, cx| {
                            if studio.recording_state == RecordingState::Idle {
                                true
                            } else {
                                studio.request_window_close(window.window_handle(), cx);
                                false
                            }
                        })
                        .unwrap_or(true)
                    });
                    studio
                },
            )
            .expect("failed to open Lahza window");
            cx.activate(true);
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crop_aspect_and_handles_stay_inside_image() {
        let square = crop_rect_with_aspect(CropRect::UNIT, 0.5);
        assert!((square.width - 0.5).abs() < 0.0001);
        assert!((square.height - 1.0).abs() < 0.0001);
        assert!((square.x - 0.25).abs() < 0.0001);

        let resized = resize_crop_rect(
            square,
            CropHandle::BottomRight,
            NormPoint { x: 2.0, y: 2.0 },
            Some(0.5),
            0.01,
            0.01,
        );
        assert!(resized.x >= 0.0 && resized.y >= 0.0);
        assert!(resized.right() <= 1.0 && resized.bottom() <= 1.0);
        assert!((resized.width / resized.height - 0.5).abs() < 0.0001);
    }

    #[test]
    fn moving_crop_never_leaves_image() {
        let rect = CropRect {
            x: 0.2,
            y: 0.2,
            width: 0.4,
            height: 0.3,
        };
        let moved = move_crop_rect(rect, NormPoint { x: 5.0, y: -5.0 });
        assert!((moved.x - 0.6).abs() < 0.0001);
        assert_eq!(moved.y, 0.0);
    }

    #[test]
    fn export_renderer_includes_raster_images() {
        let source = std::env::temp_dir().join(format!(
            "screendrop-export-raster-test-{}.png",
            std::process::id()
        ));
        image::RgbaImage::from_pixel(2, 2, image::Rgba([231, 37, 53, 255]))
            .save(&source)
            .expect("write raster fixture");
        let svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="2" height="2"><image href="{}" width="2" height="2"/></svg>"#,
            xml_escape(&source.to_string_lossy())
        );
        let tree = resvg::usvg::Tree::from_str(&svg, &resvg::usvg::Options::default())
            .expect("parse SVG containing a raster image");
        let mut output = resvg::tiny_skia::Pixmap::new(2, 2).expect("allocate output");
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::identity(),
            &mut output.as_mut(),
        );
        let pixel = output.pixel(0, 0).expect("rendered pixel");
        assert_eq!(
            (pixel.red(), pixel.green(), pixel.blue(), pixel.alpha()),
            (231, 37, 53, 255)
        );
        let _ = fs::remove_file(source);
    }
}
