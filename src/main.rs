use gpui::{
    div, point, prelude::*, px, rgb, size, svg, AnyElement, AnyWindowHandle, App, Application,
    AssetSource, AsyncApp, Bounds, Context, FocusHandle, IntoElement, KeyDownEvent, MouseButton,
    MouseMoveEvent, Pixels, Point, Render, RenderImage, SharedString, Task, Timer, TitlebarOptions,
    Window, WindowBounds, WindowDecorations, WindowHandle, WindowOptions,
};
use std::fmt::Write as _;
use std::{
    borrow::Cow,
    collections::HashSet,
    fs,
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use uuid::Uuid;

mod annotations;
mod capture;
mod capture_access;
mod controls;
mod crop;
mod launcher;
mod launcher_library;
mod launcher_recording;
mod library;
mod models;
mod motion_ui;
mod notifications;
mod text_field;
mod text_fields_ui;
mod preset_cards;
mod preview;
mod recording;
mod scene_ui;
mod shell_ui;
mod template_ui;
mod theme;
mod timed;
mod video;

use annotations::{annotations_svg, paint_annotation, paint_highlights};
use crop::paint_crop_overlay;
use models::{
    AnnotationMark, AnnotationWorkspace, CropDrag, CropHandle, CropRect, CropSnapshot, ImageScene,
    NormPoint, Tool, VideoEditSnapshot, VideoMoveDrag, VideoTrimDrag, VideoZoomDrag,
    VideoZoomDragKind, ANNOTATION_COLORS, CROP_HANDLES,
};
use theme::{
    blue, brand_wordmark, brand_wordmark_latin, gradient_layers, ink, line, muted, panel,
    BACKGROUND_PRESETS, CURATED_WALLPAPERS, GRADIENT_BACKGROUNDS, SOLID_BACKGROUNDS,
};

use scene_ui::{AnnotationDrag, MediaDrag, PreviewCache, SceneSelection};
use serde::{Deserialize, Serialize};
use shell_ui::InspectorTab;
use timed::AnnotationTiming;

use motion_ui::{MotionPick, MOTION_ZOOM_SLIDER};

use recording::{
    camera_preview::{CameraFrames, CameraPreview},
    clips::RecordingClipTimeline,
    export::{ExportFormat, ExportProgress, ExportResolution},
    model::{PointerCaptureFile, RecordingSession},
    native::{camera_devices, microphone_devices, NativeRecorder, RecordingOptions},
    pointer_timeline::PointerTimeline,
    presets::PresetLibrary,
    scene::{CameraOverlay, PointerStyle, SceneStyle, SceneTransform, Watermark, WindowFrame},
    session::{RecordingController, RecordingState},
    video::DecodedFrame,
    viewport::{visible_rect, MotionPreset, ViewportTimeline, ZoomCue},
};

struct Assets {
    base: PathBuf,
}

fn asset_directory() -> PathBuf {
    if let Some(path) = std::env::var_os("LAHZA_ASSETS") {
        return PathBuf::from(path);
    }

    if let Ok(executable) = std::env::current_exe() {
        if let Some(prefix) = executable.parent().and_then(|bin| bin.parent()) {
            let installed = prefix.join("share/lahza/assets");
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

const EDITOR_WINDOW_SIZE: gpui::Size<Pixels> = gpui::Size {
    width: px(1440.0),
    height: px(900.0),
};

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
    /// The lightweight capture home shown before there is anything to edit.
    /// The window currently showing this studio; replaced when the compact
    /// launcher hands off to the full-size editor window.
    window_handle: AnyWindowHandle,
    launcher_active: bool,
    /// The studio is still in the launcher's compact window and must move to
    /// an editor-sized one when the editor first renders.
    launcher_window: bool,
    recorder_window: Option<WindowHandle<Studio>>,
    launcher_tab: usize,
    library_state: launcher_library::LibraryState,
    recent_projects: Vec<PathBuf>,
    recent_screenshots: Vec<PathBuf>,
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
    text_fields: text_fields_ui::TextFields,
    annotation_time_edit: Option<(usize, bool, String)>,
    caret_visible: bool,
    _caret_blink_task: Task<()>,
    _recording_clock_task: Task<()>,
    recording_controller: Option<RecordingController<NativeRecorder>>,
    recording_state: RecordingState,
    recording_busy: bool,
    recording_elapsed: Duration,
    recording_started_at: Option<Instant>,
    recording_session_path: Option<PathBuf>,
    record_system_audio: bool,
    capture_access_prompt: Option<capture_access::AccessPrompt>,
    capture_access_busy: bool,
    camera_access_checked: bool,
    record_microphone: bool,
    /// Selected microphone source name; `None` follows the system default.
    microphone_device: Option<String>,
    /// `(source name, display name)` of every microphone, refreshed with the launcher.
    microphone_devices: Vec<(String, String)>,
    launcher_mic_menu_open: bool,
    record_camera: bool,
    /// Selected webcam device node; `None` uses the first webcam found.
    camera_device: Option<String>,
    /// `(device node, display name)` of every webcam, refreshed with the launcher.
    camera_devices: Vec<(String, String)>,
    launcher_camera_menu_open: bool,
    /// Latest webcam frame from the launcher preview pipeline.
    camera_frames: Arc<CameraFrames>,
    /// Webcam pipeline while the launcher shows the camera and nothing records.
    camera_preview: Option<CameraPreview>,
    camera_frame: Option<Arc<RenderImage>>,
    camera_frame_generation: u64,
    camera_poll_running: bool,
    recording_camera_enabled: bool,
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
    animation_image_start: f64,
    animation_image_end: f64,
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
    focus_drag: Option<scene_ui::FocusDrag>,
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
    color_index: usize,
    gradient_index: usize,
    wallpaper_asset: &'static str,
    custom_wallpaper: Option<PathBuf>,
    shadow_style: usize,
    shadow_color: u32,
    aspect_ratio: usize,
    border_color: usize,
    padding: u8,
    shadow: u8,
    corners: u8,
    border_thickness: u8,
    border_opacity: u8,
    border: bool,
    window_frame: WindowFrame,
    original_capture: Option<CropSnapshot>,
    source_crop: CropRect,
    crop_session: Option<CropSnapshot>,
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
    toast: Option<notifications::Notification>,
    toast_timer: Option<Task<()>>,
    toast_timer_id: Option<u64>,
    slider_drag: Option<SliderDrag>,
    motion_transform_drag: Option<motion_ui::MotionTransformDrag>,
    image_trim_drag: Option<motion_ui::ImageTrimDrag>,
    canvas_annotation_drag: bool,
}

/// Lahza-specific recording settings stored beside the Swift edit
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

/// The decoder must keep draining GStreamer's video pipe even when scene
/// compositing is slow, otherwise the shared pipeline can starve its audio.
#[derive(Default)]
struct PlaybackUpdates {
    frame: Option<DecodedFrame>,
    terminal: Option<Result<(), String>>,
}

#[derive(Clone, Default)]
struct PlaybackMailbox(Arc<Mutex<PlaybackUpdates>>);

impl PlaybackMailbox {
    fn publish(&self, frame: DecodedFrame) {
        let previous = self
            .0
            .lock()
            .expect("playback mailbox poisoned")
            .frame
            .replace(frame);
        // Release the potentially large previous image outside the lock.
        drop(previous);
    }

    fn finish(&self, result: Result<(), String>) {
        self.0.lock().expect("playback mailbox poisoned").terminal = Some(result);
    }

    fn take(&self) -> PlaybackUpdates {
        std::mem::take(&mut *self.0.lock().expect("playback mailbox poisoned"))
    }
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
        let focus_handle = cx.focus_handle();
        let text_fields = text_fields_ui::TextFields::new(focus_handle.clone(), cx);
        let mut studio = Self {
            window_handle,
            launcher_active: initial_recording.is_none() && initial_image.is_none(),
            launcher_window: initial_recording.is_none() && initial_image.is_none(),
            recorder_window: None,
            launcher_tab: 0,
            library_state: launcher_library::LibraryState::default(),
            recent_projects: Vec::new(),
            recent_screenshots: Vec::new(),
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
            text_fields,
            annotation_time_edit: None,
            caret_visible: true,
            _caret_blink_task: caret_blink_task,
            _recording_clock_task: recording_clock_task,
            recording_controller: None,
            recording_state: RecordingState::Idle,
            recording_busy: false,
            recording_elapsed: Duration::ZERO,
            recording_started_at: None,
            recording_session_path: None,
            record_system_audio: false,
            capture_access_prompt: None,
            capture_access_busy: false,
            camera_access_checked: false,
            record_microphone: false,
            microphone_device: None,
            microphone_devices: microphone_devices(),
            launcher_mic_menu_open: false,
            record_camera: false,
            camera_device: None,
            camera_devices: camera_devices(),
            launcher_camera_menu_open: false,
            camera_frames: Arc::new(CameraFrames::default()),
            camera_preview: None,
            camera_frame: None,
            camera_frame_generation: 0,
            camera_poll_running: false,
            recording_camera_enabled: false,
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
            animation_image_start: 0.0,
            animation_image_end: 5.0,
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
            focus_handle,
            wallpaper_tab: 2,
            color_index: 7,
            gradient_index: 0,
            wallpaper_asset: CURATED_WALLPAPERS[0],
            custom_wallpaper: None,
            shadow_style: 1,
            shadow_color: 0x000000,
            aspect_ratio: 0,
            border_color: 3,
            padding: 8,
            shadow: 14,
            corners: 2,
            border_thickness: 12,
            border_opacity: 30,
            border: false,
            window_frame: WindowFrame::Off,
            original_capture: None,
            source_crop: CropRect::UNIT,
            crop_session: None,
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
            toast_timer: None,
            toast_timer_id: None,
            slider_drag: None,
            motion_transform_drag: None,
            image_trim_drag: None,
            canvas_annotation_drag: false,
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

    fn wants_camera_preview(&self) -> bool {
        self.launcher_active
            && match self.recording_state {
                RecordingState::Idle => self.record_camera,
                RecordingState::Starting | RecordingState::Recording | RecordingState::Paused => {
                    self.recording_camera_enabled
                }
                _ => false,
            }
    }

    /// Poll the latest frame from the standalone camera or recording pipeline.
    fn sync_camera_preview(&mut self, cx: &mut Context<Self>) {
        let wants_preview = self.wants_camera_preview();
        if !wants_preview {
            self.camera_preview = None;
            let frame = self.camera_frame.take();
            self.retire_image(frame);
            return;
        }
        if self.recording_state != RecordingState::Idle {
            self.camera_preview = None;
        } else if self.camera_preview.is_none() {
            // A new recorder window can inherit an enabled camera. Check
            // access before trying the device, just as an explicit toggle does.
            if !self.camera_access_checked {
                self.record_camera = false;
                self.request_capture_access(capture_access::CaptureAccess::Camera, cx);
                return;
            }
            let device = match self.camera_device.clone() {
                Some(device) => Ok(device),
                None => recording::native::default_camera_device(),
            };
            match device
                .and_then(|device| CameraPreview::start(&device, self.camera_frames.clone()))
            {
                Ok(preview) => self.camera_preview = Some(preview),
                Err(error) => {
                    self.record_camera = false;
                    self.toast = Some(format!("Webcam preview unavailable: {error}").into());
                    return;
                }
            }
        }
        if self.camera_poll_running {
            return;
        }
        self.camera_poll_running = true;
        let frames = self.camera_frames.clone();
        cx.spawn(async move |weak, cx| loop {
            Timer::after(Duration::from_millis(66)).await;
            let state = weak.update(cx, |this, cx| {
                if !this.wants_camera_preview() {
                    this.camera_poll_running = false;
                    let frame = this.camera_frame.take();
                    this.retire_image(frame);
                    cx.notify();
                    return None;
                }
                Some((this.camera_frame_generation, this.camera_overlay))
            });
            let Ok(Some((seen, overlay))) = state else {
                break;
            };
            let Some((generation, frame)) = frames.newer_than(seen) else {
                continue;
            };
            // One frame at a time: discard intermediate camera frames while
            // compositing so a slow preview never queues work or delays input.
            let preview = cx
                .background_executor()
                .spawn(async move {
                    let pixels = image::RgbaImage::from_raw(frame.width, frame.height, frame.rgba)?;
                    Some(recording::scene::camera_framing_preview(&pixels, overlay))
                })
                .await;
            if weak
                .update(cx, |this, cx| {
                    if this.wants_camera_preview() {
                        this.camera_frame_generation = generation;
                        if let Some(pixels) = preview {
                            let previous = this.camera_frame.replace(cached_render_image(pixels));
                            this.retire_image(previous);
                            cx.notify();
                        }
                    }
                })
                .is_err()
            {
                break;
            }
        })
        .detach();
    }

    /// Keeps an RGBA copy of the shown capture for the compositor.
    fn set_capture_image(&mut self, image: image::RgbaImage) {
        self.capture_rgba = Some(Arc::new(image.clone()));
        let previous = self
            .displayed_capture_image
            .replace(cached_render_image(image));
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

    fn selected_canvas_ratio(&self) -> f32 {
        match self.aspect_ratio {
            1 => 1.0,
            2 => 4.0 / 3.0,
            3 => 3.0 / 2.0,
            4 => 16.0 / 9.0,
            5 => 9.0 / 16.0,
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
        self.wallpaper_asset = preset.wallpaper_asset;
        self.custom_wallpaper = None;
        self.color_index = preset.color_index;
        self.gradient_index = preset.gradient_index;
        self.padding = preset.padding;
        self.shadow = preset.shadow;
        self.corners = preset.corners;
        self.shadow_style = preset.shadow_style;
        self.shadow_color = 0x000000;
        self.aspect_ratio = preset.aspect_ratio;
        self.border = preset.border;
        self.border_color = preset.border_color;
        self.border_thickness = preset.border_thickness;
        self.border_opacity = preset.border_opacity;
        self.background_preset = Some(index);
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

    fn update_slider_drag(&mut self, event: &MouseMoveEvent) -> bool {
        if self.image_trim_drag.is_some() {
            return self.update_image_trim(event);
        }
        if self.motion_transform_drag.is_some() {
            return self.update_motion_transform_drag(event);
        }
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

    fn handle_video_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) -> bool {
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
}

impl Studio {
    fn render_export(&mut self, destination: &std::path::Path) -> Result<(), String> {
        self.rebuild_redactions()?;
        if self.scene_style().needs_composited_preview() || self.annotations.iter().any(|mark| mark.canvas) {
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
                let wallpaper = self
                    .custom_wallpaper
                    .clone()
                    .unwrap_or_else(|| asset_directory().join(self.wallpaper_asset));
                let href = xml_escape(&wallpaper.to_string_lossy());
                let _ = write!(svg, "<image href=\"{href}\" width=\"{canvas_width}\" height=\"{canvas_height}\" preserveAspectRatio=\"xMidYMid slice\"/>");
            }
        }
        if self.shadow > 0 {
            let shadow_color = self.shadow_color;
            let _ = write!(svg, "<rect x=\"{x}\" y=\"{y}\" width=\"{capture_width}\" height=\"{capture_height}\" rx=\"{radius}\" fill=\"#{shadow_color:06x}\" filter=\"url(#dropShadow)\"/>");
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
}

impl Studio {
    fn render_content(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        self.drop_retired_images(window);
        self.sync_camera_preview(cx);
        if self.launcher_active {
            return self.render_launcher(cx);
        }
        if std::mem::take(&mut self.launcher_window) {
            // GPUI cannot reliably resize a Wayland window in place, so the
            // editor gets its own full-size window and the launcher closes.
            let studio = cx.entity();
            let launcher_window = self.window_handle;
            cx.defer(move |cx| {
                let Ok(editor_window) = open_studio_window(cx, true, move |_, _| studio.clone())
                else {
                    return;
                };
                let _ = editor_window.update(cx, |studio, _, _| {
                    studio.window_handle = editor_window.into();
                });
                let _ = launcher_window.update(cx, |_, window, _| window.remove_window());
                cx.activate(true);
            });
        }
        self.sync_text_fields(window, cx);
        // Both video and animated-still transport need an initial keyboard
        // target, even before the user clicks the canvas.
        if window.focused(cx).is_none() {
            self.focus_handle.focus(window);
        }
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
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if this.capture_access_prompt.is_some() {
                    cx.stop_propagation();
                    return;
                }
                if this.native_text_focused(window, cx) { return; }
                if this.handle_animation_key(event, cx) {
                    cx.stop_propagation();
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
                    this.end_image_trim();
                    this.end_motion_transform_drag();
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
                    this.end_image_trim();
                    this.end_motion_transform_drag();
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
            .into_any_element()
    }
}

/// Opens a Lahza window at the editor or compact launcher size and installs
/// the close guard that keeps a live recording from being lost.
fn open_studio_window(
    cx: &mut App,
    editor: bool,
    build: impl FnOnce(AnyWindowHandle, &mut App) -> gpui::Entity<Studio>,
) -> gpui::Result<WindowHandle<Studio>> {
    let window_size = if editor {
        EDITOR_WINDOW_SIZE
    } else {
        size(px(430.0), px(610.0))
    };
    let bounds = Bounds::centered(None, window_size, cx);
    cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(if editor {
                size(px(980.0), px(680.0))
            } else {
                size(px(400.0), px(560.0))
            }),
            app_id: Some("com.lahza.Lahza".into()),
            titlebar: Some(TitlebarOptions {
                title: Some("Lahza".into()),
                appears_transparent: false,
                traffic_light_position: None,
            }),
            window_decorations: Some(WindowDecorations::Server),
            ..Default::default()
        },
        move |window, cx| {
            let studio = build(window.window_handle(), cx);
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
}

fn main() {
    let arguments: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    if arguments.iter().any(|argument| argument == "--version") {
        println!("Lahza {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    let initial_recording = arguments
        .iter()
        .find(|path| path.extension().and_then(|value| value.to_str()) == Some("lahzarec"))
        .cloned();
    // `lahza shot.png` opens an existing image in the screenshot editor.
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
    // GNOME's Wayland compositor has no xdg-decoration protocol, so GPUI
    // windows there get no frame at all. Run through XWayland instead, where
    // the compositor draws its standard title bar and controls. Recording
    // still detects a Wayland session via XDG_SESSION_TYPE.
    if std::env::var_os("DISPLAY").is_some_and(|display| !display.is_empty()) {
        std::env::remove_var("WAYLAND_DISPLAY");
    }
    Application::new()
        .with_assets(Assets {
            base: asset_directory(),
        })
        .run(move |cx: &mut App| {
            let starts_in_editor = initial_recording.is_some() || initial_image.is_some();
            open_studio_window(cx, starts_in_editor, move |window_handle, cx| {
                cx.new(|cx| {
                    Studio::new(
                        window_handle,
                        initial_recording.clone(),
                        initial_image.clone(),
                        cx,
                    )
                })
            })
            .expect("failed to open Lahza window");
            cx.activate(true);
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playback_keeps_draining_while_the_preview_is_stalled() {
        let mailbox = PlaybackMailbox::default();
        let producer = mailbox.clone();
        let (done, completed) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            // Ten seconds of 60 fps playback while the UI consumes nothing.
            for index in 0..600 {
                producer.publish(DecodedFrame {
                    time: index as f64 / 60.0,
                    width: 1,
                    height: 1,
                    rgba: vec![0, 0, 0, 255],
                });
            }
            producer.finish(Ok(()));
            done.send(()).unwrap();
        });
        completed
            .recv_timeout(Duration::from_secs(2))
            .expect("preview stalled the decoder");
        worker.join().unwrap();
        let update = mailbox.take();
        assert_eq!(update.frame.unwrap().time, 599.0 / 60.0);
        assert_eq!(update.terminal, Some(Ok(())));
        let empty = mailbox.take();
        assert!(empty.frame.is_none() && empty.terminal.is_none());
    }
}
