//! Motion editing: the orange motion lane, the selection-aware motion
//! inspector, animated screenshots, and scene export. Shared by the
//! recording editor and the screenshot editor so "static or motion" is one
//! workflow.

use gpui::{
    div, hsla, prelude::*, px, rgb, AnyElement, Context, CursorStyle, FontWeight, MouseButton,
    MouseDownEvent, MouseMoveEvent, Pixels, Timer,
};
use image::RgbaImage;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use crate::{
    line, muted,
    recording::{
        clips::{ClipEdge, RecordingClipTimeline},
        export::{
            export_scene, ExportFormat, ExportProgress, ImageSegment, SceneExportRequest,
            SceneSource,
        },
        model::NormalizedPoint,
        scene::{SceneBackground, SceneStyle},
        viewport::{
            synthesize_zoom_cues_with_style, AutoZoomStyle, CameraMotion, MotionPreset, MotionStyle,
            TransformMotion, ViewportTimeline, ZoomAnchorMode, ZoomCue, ZoomPresentation,
        },
    },
    xml_escape, ImageScene, SliderDrag, Studio, VideoEditSnapshot, VideoZoomDragKind,
    GRADIENT_BACKGROUNDS, SOLID_BACKGROUNDS,
};

/// Border swatches shared by the inspector, the preview, and export.
pub(crate) const BORDER_COLORS: [u32; 6] =
    [0xffc928, 0x22b45d, 0x22bfc2, 0x3678ef, 0x8c4ce8, 0xec3d87];

/// Slider id used by the motion inspector's magnification slider.
pub(crate) const MOTION_ZOOM_SLIDER: usize = 7;

pub(crate) const ANIMATION_DURATIONS: [f64; 4] = [3.0, 5.0, 8.0, 10.0];

/// What a click on the canvas sets while a motion region is selected.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum MotionPick {
    #[default]
    Focus,
    PanEnd,
}

pub(crate) const MINIMUM_IMAGE_DURATION: f64 = 0.1;

/// Clip out-points are exclusive. Preview End just inside the visible interval
/// so an endpoint shared with the image out-point does not show an empty canvas.
fn visible_motion_endpoint(start: f64, end: f64, image_start: f64, image_end: f64, endpoint: usize) -> Option<f64> {
    let start = start.max(image_start);
    let end = end.min(image_end);
    if end <= start { return None; }
    Some(if endpoint == 0 { start } else { (end - 1e-6).max(start) })
}


#[derive(Clone, Copy)]
pub(crate) struct ImageTrimDrag {
    start_x: Pixels,
    start_scroll: f64,
    original_start: f64,
    original_end: f64,
    edge: Option<ClipEdge>,
    seconds_per_pixel: f64,
    pub start: f64,
    pub end: f64,
}

impl ImageTrimDrag {
    fn range_at_scroll(self, x: Pixels, scroll: f64) -> (f64, f64) {
        self.range_at(x + px((scroll - self.start_scroll) as f32))
    }

    fn range_at(self, x: Pixels) -> (f64, f64) {
        if self.edge.is_none() {
            let pixels = f64::from((x - self.start_x) / px(1.0));
            // A click seeks and selects without nudging the clip.
            let start = if pixels.abs() < 3.0 {
                self.original_start
            } else {
                ((self.original_start + pixels * self.seconds_per_pixel) * 100.0)
                    .round().max(0.0) / 100.0
            };
            return (start, start + self.original_end - self.original_start);
        }
        let delta = f64::from((x - self.start_x) / px(1.0)) * self.seconds_per_pixel;
        let (start, end) = match self.edge.expect("trim edge") {
            ClipEdge::Leading => (
                ((self.original_start + delta) * 100.0).round().max(0.0) / 100.0,
                self.original_end,
            ),
            ClipEdge::Trailing => (
                self.original_start,
                ((self.original_end + delta) * 100.0).round() / 100.0,
            ),
        };
        match self.edge.expect("trim edge") {
            ClipEdge::Leading => (start.min((end - MINIMUM_IMAGE_DURATION).max(0.0)), end),
            ClipEdge::Trailing => (start, end.max(start + MINIMUM_IMAGE_DURATION)),
        }
    }
}

#[cfg(test)]
mod image_trim_tests {
    use super::*;

    #[test]
    fn endpoint_preview_stays_inside_the_visible_image_and_motion() {
        let end = visible_motion_endpoint(0.0, 2.5, 0.0, 2.5, 1).unwrap();
        assert!(end < 2.5 && end > 2.49999);
        assert_eq!(visible_motion_endpoint(0.0, 2.5, 1.0, 2.0, 0), Some(1.0));
        let trimmed_end = visible_motion_endpoint(0.0, 2.5, 1.0, 2.0, 1).unwrap();
        assert!(trimmed_end < 2.0 && trimmed_end > 1.99999);
        assert_eq!(visible_motion_endpoint(0.0, 1.0, 2.0, 3.0, 1), None);
    }

    #[test]
    fn leading_trim_changes_only_start_and_keeps_a_nonempty_clip() {
        let drag = ImageTrimDrag {
            start_x: px(100.0),
            start_scroll: 0.0,
            original_start: 1.0,
            original_end: 4.0,
            seconds_per_pixel: 0.01,
            edge: Some(ClipEdge::Leading),
            start: 1.0,
            end: 4.0,
        };
        assert_eq!(drag.range_at(px(200.0)), (2.0, 4.0));
        assert_eq!(drag.range_at(px(-200.0)), (0.0, 4.0));
        assert_eq!(drag.range_at(px(1000.0)), (3.9, 4.0));
        assert_eq!(drag.range_at(px(100.0)), (1.0, 4.0));
    }

    #[test]
    fn moving_image_preserves_duration_and_clamps_only_at_zero() {
        let drag = ImageTrimDrag {
            start_x: px(100.0),
            start_scroll: 0.0,
            original_start: 1.0,
            original_end: 4.0,
            seconds_per_pixel: 0.01,
            edge: None,
            start: 1.0,
            end: 4.0,
        };
        assert_eq!(drag.range_at(px(102.0)), (1.0, 4.0));
        assert_eq!(drag.range_at(px(200.0)), (2.0, 5.0));
        assert_eq!(drag.range_at(px(-500.0)), (0.0, 3.0));
        assert_eq!(drag.range_at(px(1000.0)), (10.0, 13.0));
        assert_eq!(drag.range_at(px(100.0)), (1.0, 4.0));
    }

    #[test]
    fn trailing_trim_respects_a_delayed_start() {
        let drag = ImageTrimDrag {
            start_x: px(100.0),
            start_scroll: 0.0,
            original_start: 1.0,
            original_end: 4.0,
            seconds_per_pixel: 0.01,
            edge: Some(ClipEdge::Trailing),
            start: 1.0,
            end: 4.0,
        };
        assert_eq!(drag.range_at(px(200.0)), (1.0, 5.0));
        assert_eq!(drag.range_at(px(-500.0)), (1.0, 1.1));
        assert_eq!(drag.range_at(px(100.0)), (1.0, 4.0));
    }

    #[test]
    fn image_trim_uses_the_scrolled_content_position() {
        let drag = ImageTrimDrag {
            start_x: px(100.0), start_scroll: 200.0,
            original_start: 1.0, original_end: 4.0,
            seconds_per_pixel: 0.01, edge: Some(ClipEdge::Trailing),
            start: 1.0, end: 4.0,
        };
        assert_eq!(drag.range_at_scroll(px(150.0), 250.0), (1.0, 5.0));
        assert_eq!(drag.range_at_scroll(px(100.0), 200.0), (1.0, 4.0));
    }
}

/// One scrub gesture records one undo entry and saves once on release.
pub(crate) struct MotionTransformDrag {
    cue_id: uuid::Uuid,
    axis: usize,
    endpoint: usize,
    start_x: Pixels,
    start_value: f64,
    original_cues: Vec<ZoomCue>,
}

fn transform_value(transform: &crate::recording::scene::SceneTransform, axis: usize) -> f64 {
    match axis {
        0 => transform.scale,
        1 => transform.position_x,
        2 => transform.position_y,
        3 => transform.rotation_x,
        4 => transform.rotation_y,
        _ => transform.rotation_z,
    }
}

fn set_transform_value(
    transform: &mut crate::recording::scene::SceneTransform,
    axis: usize,
    value: f64,
) {
    match axis {
        0 => transform.scale = value,
        1 => transform.position_x = value,
        2 => transform.position_y = value,
        3 => transform.rotation_x = value,
        4 => transform.rotation_y = value,
        _ => transform.rotation_z = value,
    }
    *transform = transform.clamped();
}

fn scrubbed_transform_value(axis: usize, start: f64, delta: f64, precise: bool) -> f64 {
    let sensitivity = match axis {
        0 => 0.01,
        1 | 2 => 0.01,
        _ => 1.0,
    };
    start + delta * sensitivity * if precise { 0.1 } else { 1.0 }
}

const MOTION_ROW_HEIGHT: f32 = 30.0;

#[derive(Clone, Copy, Debug)]
struct MotionLaneRegion {
    cue_index: usize,
    segment_index: usize,
    editor_start: f64,
    editor_end: f64,
    left: f64,
    width: f64,
    row: usize,
}

/// Pack the rendered intervals rather than source ranges: edits, speed,
/// gaps, and the minimum clickable width all affect visible overlap.
fn motion_lane_layout(
    cues: &[ZoomCue],
    clips: &RecordingClipTimeline,
    duration: f64,
    content_width: f64,
) -> Vec<MotionLaneRegion> {
    let mut regions = Vec::new();
    let mut slot_start = 0.0;
    for (segment_index, segment) in clips.segments.iter().enumerate() {
        let segment_start = slot_start + segment.gap_before;
        for (cue_index, cue) in cues.iter().enumerate() {
            let start = cue.start.max(segment.source_start);
            let end = cue.end.min(segment.source_end);
            if end - start <= f64::EPSILON {
                continue;
            }
            let editor_start = segment_start + (start - segment.source_start) / segment.speed;
            let editor_end = segment_start + (end - segment.source_start) / segment.speed;
            regions.push(MotionLaneRegion {
                cue_index,
                segment_index,
                editor_start,
                editor_end,
                left: editor_start / duration.max(f64::EPSILON) * content_width,
                width: ((editor_end - editor_start) / duration.max(f64::EPSILON) * content_width)
                    .max(24.0),
                row: 0,
            });
        }
        slot_start += segment.slot_duration();
    }
    regions.sort_by(|a, b| {
        a.editor_start
            .total_cmp(&b.editor_start)
            .then_with(|| a.cue_index.cmp(&b.cue_index))
            .then_with(|| a.segment_index.cmp(&b.segment_index))
    });
    let mut row_ends: Vec<f64> = Vec::new();
    for region in &mut regions {
        let row = row_ends
            .iter()
            .position(|end| *end <= region.left + 1e-9)
            .unwrap_or(row_ends.len());
        if row == row_ends.len() {
            row_ends.push(0.0);
        }
        row_ends[row] = region.left + region.width;
        region.row = row;
    }
    regions
}

#[cfg(test)]
mod motion_lane_tests {
    use super::*;
    use crate::recording::clips::RecordingClipSegment;

    fn cue(start: f64, end: f64) -> ZoomCue {
        ZoomCue::pinned(start, end, 2.0, NormalizedPoint { x: 0.5, y: 0.5 })
    }

    #[test]
    fn overlapping_regions_stack_and_adjacent_regions_reuse_rows() {
        let cues = vec![cue(2.0, 4.0), cue(0.0, 3.0), cue(1.0, 2.5), cue(3.0, 5.0)];
        let regions = motion_lane_layout(&cues, &RecordingClipTimeline::full(5.0), 5.0, 500.0);
        let row = |index| {
            regions
                .iter()
                .find(|region| region.cue_index == index)
                .unwrap()
                .row
        };
        assert_eq!(row(1), 0);
        assert_eq!(row(2), 1);
        assert_eq!(row(0), 2);
        assert_eq!(row(3), 0);
        for a in &regions {
            for b in &regions {
                if a.cue_index != b.cue_index && a.row == b.row {
                    assert!(a.left + a.width <= b.left || b.left + b.width <= a.left);
                }
            }
        }
    }

    #[test]
    fn minimum_hit_width_does_not_cover_the_next_region() {
        let cues = vec![cue(0.0, 0.5), cue(0.5, 1.0)];
        let clips = RecordingClipTimeline::full(10.0);
        let narrow = motion_lane_layout(&cues, &clips, 10.0, 100.0);
        assert_ne!(narrow[0].row, narrow[1].row);
        let zoomed = motion_lane_layout(&cues, &clips, 10.0, 1000.0);
        assert_eq!(zoomed[0].row, zoomed[1].row);
    }

    #[test]
    fn rows_follow_edited_clip_order_speed_and_gaps() {
        let mut first = RecordingClipSegment::new(5.0, 9.0);
        first.speed = 2.0;
        first.gap_before = 1.0;
        let clips = RecordingClipTimeline::new(vec![first, RecordingClipSegment::new(0.0, 2.0)]);
        let cues = vec![cue(5.0, 9.0), cue(6.0, 8.0), cue(0.0, 1.0), cue(3.0, 4.0)];
        let regions = motion_lane_layout(&cues, &clips, clips.duration(), 500.0);
        assert_eq!(
            regions.len(),
            3,
            "removed source ranges have no visible row"
        );
        assert_eq!(
            (
                regions[0].editor_start,
                regions[0].editor_end,
                regions[0].row
            ),
            (1.0, 3.0, 0)
        );
        assert_eq!(
            (
                regions[1].editor_start,
                regions[1].editor_end,
                regions[1].row
            ),
            (1.5, 2.5, 1)
        );
        assert_eq!(
            (
                regions[2].editor_start,
                regions[2].editor_end,
                regions[2].row
            ),
            (3.0, 4.0, 0)
        );
    }
}

pub(crate) fn orange(selected: bool) -> gpui::Hsla {
    if selected {
        hsla(24.0 / 360.0, 0.95, 0.47, 1.0)
    } else {
        hsla(24.0 / 360.0, 0.95, 0.56, 1.0)
    }
}

fn zoom_from_slider(value: u8) -> f64 {
    ZoomCue::MINIMUM_ZOOM
        + (ZoomCue::MAXIMUM_ZOOM - ZoomCue::MINIMUM_ZOOM) * (value.min(100) as f64 / 100.0)
}

fn slider_from_zoom(zoom: f64) -> u8 {
    (((zoom - ZoomCue::MINIMUM_ZOOM) / (ZoomCue::MAXIMUM_ZOOM - ZoomCue::MINIMUM_ZOOM)) * 100.0)
        .round()
        .clamp(0.0, 100.0) as u8
}

impl Studio {
    // ------------------------------------------------------------------
    // Scene model
    // ------------------------------------------------------------------

    /// The style the preview shows and the exporter renders.
    pub(crate) fn scene_style(&self) -> SceneStyle {
        let background = match self.wallpaper_tab {
            0 => SceneBackground::Solid(
                SOLID_BACKGROUNDS[self.color_index.min(SOLID_BACKGROUNDS.len() - 1)].1,
            ),
            1 => {
                let gradient =
                    GRADIENT_BACKGROUNDS[self.gradient_index.min(GRADIENT_BACKGROUNDS.len() - 1)];
                SceneBackground::Gradient {
                    colors: gradient.colors,
                    angle_degrees: gradient.angle as f64,
                }
            }
            _ => SceneBackground::Wallpaper(
                self.custom_wallpaper
                    .clone()
                    .unwrap_or_else(|| crate::asset_directory().join(self.wallpaper_asset)),
            ),
        };
        SceneStyle {
            background,
            source_crop: if self.video_project.is_some() { self.video_crop } else { crate::CropRect::UNIT },
            window_frame: self.window_frame,
            padding: self.padding,
            corners: self.corners,
            shadow: self.shadow,
            shadow_style: self.shadow_style,
            shadow_color: self.shadow_color,
            border: self.border,
            border_thickness: self.border_thickness,
            border_color: BORDER_COLORS[self.border_color.min(BORDER_COLORS.len() - 1)],
            border_opacity: self.border_opacity,
            aspect: (self.aspect_ratio != 0).then(|| if self.aspect_ratio == 10 {
                self.custom_canvas_dimensions.0 as f64 / self.custom_canvas_dimensions.1.max(1) as f64
            } else { self.selected_canvas_ratio() as f64 }),
            aspect_dimensions: (self.aspect_ratio == 10).then_some(self.custom_canvas_dimensions),
            // Fill needs the follow camera, which only recordings build.
            fill: self.content_fill && self.aspect_ratio != 0 && self.video_project.is_some(),
            background_blur: self.background_blur,
            background_noise: self.background_noise,
            vignette: self.vignette,
            transform: self.scene_transform.clamped(),
            watermark: (self.watermark_enabled && !self.watermark.text.trim().is_empty())
                .then(|| self.watermark.clone()),
            pointer: self.pointer_style,
            camera: self.camera_overlay,
            input_overlays: self.input_overlays,
            subtitles: self.subtitle_style,
        }
    }

    /// Export height from the resolution picker (Original keeps the media's
    /// own height within 720p–4K).
    fn export_canvas_height(&self) -> u32 {
        let height = if self.video_project.is_some() {
            self.video_source_size.1
        } else {
            self.captured_dimensions
                .map(|(_, height)| height)
                .unwrap_or(1080)
        };
        self.export_resolution.canvas_height(height)
    }

    /// Whether the open scene has a reconstructed cursor to follow.
    pub(crate) fn scene_has_pointer(&self) -> bool {
        (self.video_project.is_some() && !self.video_pointer_timeline.is_empty())
            || (self.animation_active && self.has_walkthrough())
    }

    // ------------------------------------------------------------------
    // Motion regions
    // ------------------------------------------------------------------

    /// Editor time under a pointer x position on any timeline lane.
    pub(crate) fn motion_timeline_time_at(&self, x: Pixels) -> Option<f64> {
        let bounds = (*self.video_timeline_bounds.lock().ok()?)?;
        let local = f64::from((x - bounds.origin.x) / px(1.0));
        Some(((self.video_timeline_scroll + local) * self.video_timeline_seconds_per_pixel())
            .clamp(0.0, self.video_timeline_duration()))
    }

    /// Adds a default motion region around `editor_time` and selects it.
    pub(crate) fn add_motion_region_at(&mut self, editor_time: f64, cx: &mut Context<Self>) {
        self.add_motion_region_with_presentation_at(editor_time, ZoomPresentation::Classic, cx);
    }

    pub(crate) fn add_motion_region_with_presentation_at(
        &mut self,
        editor_time: f64,
        presentation: ZoomPresentation,
        cx: &mut Context<Self>,
    ) {
        if self.video_edit_busy || self.video_source_duration < ZoomCue::MINIMUM_DURATION {
            return;
        }
        let editor_time = editor_time.clamp(0.0, self.video_duration);
        let source_time = self
            .video_clip_timeline
            .source_time_at(editor_time)
            .clamp(0.0, self.video_source_duration);
        let mut start = (source_time - 0.3).max(0.0);
        let end = (source_time + 2.5).min(self.video_source_duration);
        if end - start < ZoomCue::MINIMUM_DURATION {
            start = (end - ZoomCue::MINIMUM_DURATION).max(0.0);
        }
        let point = self
            .video_pointer_timeline
            .location_at(editor_time)
            .unwrap_or(NormalizedPoint { x: 0.5, y: 0.5 });
        let mut cue = ZoomCue::pinned(start, end, self.default_motion_zoom, point);
        cue.presentation = presentation;
        cue.camera_motion = CameraMotion::Focused;
        if self.scene_has_pointer() {
            cue.anchor_mode = ZoomAnchorMode::PointerAnchor;
            cue.bounds_bias = 0.25;
        }
        let original = self.video_zoom_cues.clone();
        self.select_motion_region(cue.id);
        self.video_zoom_cues.push(cue);
        self.video_zoom_cues
            .sort_by(|left, right| left.start.total_cmp(&right.start));
        self.video_undo_stack
            .push(VideoEditSnapshot::Zoom(original));
        self.video_redo_stack.clear();
        self.rebuild_video_motion_timelines();
        self.persist_video_zoom_cues(cx);
    }

    /// Applies `mutate` to the selected region with undo support. Returns
    /// whether anything changed. Persistence happens here too; callers
    /// only need to notify.
    pub(crate) fn edit_selected_region(&mut self, mutate: impl FnOnce(&mut ZoomCue)) -> bool {
        if self.video_edit_busy {
            return false;
        }
        let Some(selected) = self.video_selected_zoom_cue else {
            return false;
        };
        let original = self.video_zoom_cues.clone();
        let Some(cue) = self
            .video_zoom_cues
            .iter_mut()
            .find(|cue| cue.id == selected)
        else {
            return false;
        };
        mutate(cue);
        if self.video_zoom_cues == original {
            return false;
        }
        self.video_undo_stack
            .push(VideoEditSnapshot::Zoom(original));
        self.video_redo_stack.clear();
        self.rebuild_video_motion_timelines();
        self.persist_video_zoom_cues_quiet();
        true
    }

    /// Live slider updates: no undo entry per step (the drag start records
    /// one) and no autosave until the drag ends.
    pub(crate) fn set_selected_region_zoom_live(&mut self, zoom: f64) {
        let Some(selected) = self.video_selected_zoom_cue else {
            return;
        };
        let Some(cue) = self
            .video_zoom_cues
            .iter_mut()
            .find(|cue| cue.id == selected)
        else {
            return;
        };
        let zoom = zoom.clamp(ZoomCue::MINIMUM_ZOOM, ZoomCue::MAXIMUM_ZOOM);
        if (cue.zoom - zoom).abs() < 1e-9 {
            return;
        }
        cue.zoom = zoom;
        self.rebuild_video_motion_timelines();
    }

    pub(crate) fn begin_motion_zoom_slider(&mut self, start_x: Pixels, start_value: u8) {
        if self.video_edit_busy || self.video_selected_zoom_cue.is_none() {
            return;
        }
        self.video_undo_stack
            .push(VideoEditSnapshot::Zoom(self.video_zoom_cues.clone()));
        self.video_redo_stack.clear();
        self.slider_drag = Some(SliderDrag {
            slider_id: MOTION_ZOOM_SLIDER,
            start_x,
            start_value,
        });
    }

    /// Slider callback from `set_slider_value`.
    pub(crate) fn set_motion_zoom_slider(&mut self, value: u8) {
        self.set_selected_region_zoom_live(zoom_from_slider(value));
    }

    /// Replaces every region with the automatic click-based suggestion.
    pub(crate) fn regenerate_motion_from_clicks(
        &mut self,
        style: AutoZoomStyle,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.video_project.as_ref() else {
            return;
        };
        if self.video_edit_busy {
            return;
        }
        let _ = session;
        let capture = self.filtered_pointer_capture();
        if !capture.presses.iter().any(|press| press.phase == crate::recording::model::PressPhase::Down) {
            self.toast = Some("This recording has no clicks to generate zooms from".into());
            cx.notify();
            return;
        }
        let mut generated =
            synthesize_zoom_cues_with_style(&capture, self.video_source_duration, style);
        if style == AutoZoomStyle::Compact {
            for cue in &mut generated {
                cue.zoom = self
                    .default_motion_zoom
                    .clamp(ZoomCue::MINIMUM_ZOOM, ZoomCue::MAXIMUM_ZOOM);
            }
        }
        if generated == self.video_zoom_cues {
            self.toast = Some("Motion already matches the automatic suggestion".into());
            cx.notify();
            return;
        }
        self.video_undo_stack
            .push(VideoEditSnapshot::Zoom(self.video_zoom_cues.clone()));
        self.video_redo_stack.clear();
        self.video_zoom_cues = generated;
        self.video_selected_zoom_cue = None;
        self.rebuild_video_motion_timelines();
        self.persist_video_zoom_cues(cx);
        cx.notify();
    }

    pub(crate) fn selected_region(&self) -> Option<ZoomCue> {
        self.video_selected_zoom_cue
            .and_then(|id| self.video_zoom_cues.iter().find(|cue| cue.id == id))
            .cloned()
    }

    // ------------------------------------------------------------------
    // Timeline lane
    // ------------------------------------------------------------------

    /// Orange, draggable motion regions laid out in editor time.
    pub(crate) fn motion_lane_elements(
        &self,
        timeline_duration: f64,
        timeline_content_width: f64,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let selected_zoom_cue = self.video_selected_zoom_cue;
        let mut lane: Vec<AnyElement> = Vec::new();
        for region in motion_lane_layout(
            &self.video_zoom_cues,
            &self.video_clip_timeline,
            timeline_duration,
            timeline_content_width,
        ) {
            let cue = &self.video_zoom_cues[region.cue_index];
            let cue_id = cue.id;
            let segment_index = region.segment_index;
            let editor_start = region.editor_start;
            let editor_end = region.editor_end;
            let left = region.left;
            let width = region.width;
            let selected = selected_zoom_cue == Some(cue_id);
            let hovered = self.timeline_motion.hovered_region == Some(cue_id);
            let label = cue.summary();
            let enabled = cue.is_enabled;
            lane.push(
                div()
                    .id((
                        "motion-region",
                        (cue_id.as_u128() as u64).wrapping_add(segment_index as u64),
                    ))
                    .absolute()
                    .left(px(left as f32))
                    .top(px(3.0 + region.row as f32 * MOTION_ROW_HEIGHT))
                    .w(px(width as f32))
                    .h(px(24.0))
                    .rounded_md()
                    .border_2()
                    .border_color(if selected {
                        crate::blue()
                    } else {
                        hsla(0.0, 0.0, 0.0, 0.0)
                    })
                    .bg(orange(selected))
                    .when(!enabled, |this| this.opacity(0.45))
                    .text_color(rgb(0xffffff))
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .flex()
                    .items_center()
                    .justify_center()
                    .overflow_hidden()
                    .cursor(CursorStyle::PointingHand)
                    .hover(|style| style.bg(orange(true)))
                    .on_hover(cx.listener(move |this, hovering: &bool, _, cx| {
                        let next = if *hovering { Some(cue_id) }
                            else if this.timeline_motion.hovered_region == Some(cue_id) { None }
                            else { this.timeline_motion.hovered_region };
                        if next != this.timeline_motion.hovered_region {
                            this.timeline_motion.hovered_region = next;
                            cx.notify();
                        }
                    }))
                    .when(width >= 44.0, |this| this.child(label))
                    .on_mouse_down(MouseButton::Right, cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        this.open_timeline_context(crate::timeline_ui::TimelineContextTarget::Motion(cue_id), event.position, cx);
                    }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.finish_annotation_interaction();
                        this.select_motion_region(cue_id);
                        cx.notify();
                    }))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            // Clicking a region also moves the playhead there.
                            if let Some(target) = this.motion_timeline_time_at(event.position.x) {
                                this.seek_video(target, cx);
                            }
                            this.begin_video_zoom_drag(
                                cue_id,
                                VideoZoomDragKind::Move,
                                editor_start,
                                editor_end,
                                event.position.x,
                            );
                            cx.notify();
                        }),
                    )
                    .when(selected || hovered, |this| {
                        this.child(
                            div()
                                .id((
                                    "motion-region-leading",
                                    cue_id.as_u128() as u64 ^ segment_index as u64,
                                ))
                                .absolute()
                                .left_0()
                                .top_0()
                                .w(px(10.0))
                                .h_full()
                                .rounded_l_md()
                                .bg(hsla(0.0, 0.0, 1.0, 0.38))
                                .cursor(CursorStyle::ResizeLeftRight)
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                        cx.stop_propagation();
                                        this.begin_video_zoom_drag(
                                            cue_id,
                                            VideoZoomDragKind::Leading,
                                            editor_start,
                                            editor_end,
                                            event.position.x,
                                        );
                                        cx.notify();
                                    }),
                                ),
                        )
                        .child(
                            div()
                                .id((
                                    "motion-region-trailing",
                                    cue_id.as_u128() as u64 ^ !(segment_index as u64),
                                ))
                                .absolute()
                                .right_0()
                                .top_0()
                                .w(px(10.0))
                                .h_full()
                                .rounded_r_md()
                                .bg(hsla(0.0, 0.0, 1.0, 0.38))
                                .cursor(CursorStyle::ResizeLeftRight)
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                        cx.stop_propagation();
                                        this.begin_video_zoom_drag(
                                            cue_id,
                                            VideoZoomDragKind::Trailing,
                                            editor_start,
                                            editor_end,
                                            event.position.x,
                                        );
                                        cx.notify();
                                    }),
                                ),
                        )
                    })
                    .into_any_element(),
            );
        }
        lane
    }

    pub(crate) fn motion_lane_height(&self) -> f32 {
        let width = self.video_timeline_content_width();
        let rows = motion_lane_layout(
            &self.video_zoom_cues,
            &self.video_clip_timeline,
            self.video_timeline_duration(),
            width,
        )
        .iter()
        .map(|region| region.row + 1)
        .max()
        .unwrap_or(1);
        rows as f32 * MOTION_ROW_HEIGHT
    }

    /// Click empty space to seek, draw a range to create motion, or
    /// double-click for a default region.
    pub(crate) fn motion_track(
        &self,
        timeline_scroll: f64,
        timeline_content_width: f64,
        progress: f64,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let timeline_duration = self.video_timeline_duration();
        let lane = self.motion_lane_elements(timeline_duration, timeline_content_width, cx);
        let empty = self.video_zoom_cues.is_empty();
        div()
            .id("motion-track")
            .relative()
            .w_full()
            .h(px(self.motion_lane_height()))
            .flex_none()
            .overflow_hidden()
            .rounded_lg()
            .bg(rgb(0xECEDF1))
            .child(
                div()
                    .absolute()
                    .left(px(-(timeline_scroll as f32)))
                    .top_0()
                    .w(px(timeline_content_width as f32))
                    .h_full()
                    .children(lane)
                    .child(
                        div()
                            .absolute()
                            .left(px((timeline_content_width * progress) as f32 - 1.0))
                            .top_0()
                            .w(px(2.0))
                            .h_full()
                            .bg(hsla(222.0 / 360.0, 0.2, 0.15, 0.7)),
                    ),
            )
            .when(empty && self.timeline_motion.creation.is_none(), |this| {
                this.child(
                    div()
                        .absolute()
                        .left(px(10.0))
                        .top(px(7.0))
                        .text_xs()
                        .text_color(muted())
                        .child("Drag to add motion · double-click for a quick region"),
                )
            })
            .when_some(self.motion_creation_preview(), |this, preview| this.child(preview))
            .on_mouse_down(MouseButton::Right, cx.listener(|this, event: &MouseDownEvent, _, cx| {
                cx.stop_propagation();
                if let Some(time) = this.motion_timeline_time_at(event.position.x) {
                    this.open_timeline_context(crate::timeline_ui::TimelineContextTarget::EmptyMotion(time), event.position, cx);
                }
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    let Some(target) = this.motion_timeline_time_at(event.position.x) else {
                        return;
                    };
                    if event.click_count >= 2 {
                        this.cancel_motion_creation();
                        this.video_seek_drag = None;
                        this.seek_video(target, cx);
                        this.add_motion_region_at(target, cx);
                    } else {
                        this.begin_motion_creation(event.position.x, cx);
                    }
                    cx.notify();
                }),
            )
            .into_any_element()
    }

    // ------------------------------------------------------------------
    // Inspector
    // ------------------------------------------------------------------

    pub(crate) fn inspector_label(text: &'static str) -> AnyElement {
        div()
            .text_xs()
            .text_color(muted())
            .child(text)
            .into_any_element()
    }

    pub(crate) fn selection_action_button(
        &self,
        id: &'static str,
        delete: bool,
        enabled: bool,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut Studio, &mut Context<Studio>) + 'static,
    ) -> AnyElement {
        div()
            .id(id)
            .px_3()
            .h(px(30.0))
            .flex()
            .items_center()
            .justify_center()
            .gap_2()
            .rounded_md()
            .bg(rgb(if delete { 0xfee2e2 } else { 0xf0f0f1 }))
            .text_color(rgb(if delete { 0xb91c1c } else { 0x27272a }))
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD)
            .opacity(if enabled { 1.0 } else { 0.4 })
            .when(enabled, |button| {
                button.cursor_pointer()
                    .hover(move |style| style.bg(rgb(if delete { 0xfecaca } else { 0xe4e4e7 })))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        on_click(this, cx);
                        cx.notify();
                    }))
            })
            .child(gpui::svg()
                .path(if delete { "icons/trash.svg" } else { "icons/close.svg" })
                .size(px(14.0))
                .flex_none()
                .text_color(rgb(if delete { 0xb91c1c } else { 0x27272a })))
            .child(if delete { "Delete" } else { "Deselect" })
            .into_any_element()
    }

    pub(crate) fn small_button(
        &self,
        id: &'static str,
        label: &'static str,
        enabled: bool,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut Studio, &mut Context<Studio>) + 'static,
    ) -> AnyElement {
        div()
            .id(id)
            .px_3()
            .h(px(30.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded_md()
            .bg(rgb(0xf0f0f1))
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD)
            .opacity(if enabled { 1.0 } else { 0.4 })
            .when(enabled, |this| {
                this.cursor_pointer()
                    .hover(|style| style.bg(rgb(0xe4e4e7)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        on_click(this, cx);
                        cx.notify();
                    }))
            })
            .child(label)
            .into_any_element()
    }

    fn motion_zoom_slider(&self, zoom: f64, cx: &mut Context<Self>) -> AnyElement {
        let value = slider_from_zoom(zoom);
        let measured = std::rc::Rc::new(std::cell::Cell::new(None::<gpui::Bounds<Pixels>>));
        let track = measured.clone();
        div().flex().flex_none().items_center().gap_2()
            .child(div().id("motion-zoom-slider").relative().flex_1().min_w(px(72.0)).h(px(40.0))
                .cursor_pointer()
                .child(gpui::canvas(
                    move |bounds, _, _| { measured.set(Some(bounds)); bounds },
                    move |_, bounds, window, _| {
                        let left = bounds.origin.x + px(8.0);
                        let width = (bounds.size.width - px(16.0)).max(px(1.0));
                        let top = bounds.origin.y + px(17.0);
                        let progress = width * (value as f32 / 100.0);
                        for (length, color) in [(width, rgb(0xe2e5ea).into()), (progress, crate::blue())] {
                            window.paint_quad(gpui::fill(gpui::Bounds::new(gpui::point(left, top), gpui::size(length, px(6.0))), color));
                        }
                        window.paint_quad(gpui::quad(
                            gpui::Bounds::new(gpui::point(left + progress - px(7.0), top - px(4.0)), gpui::size(px(14.0), px(14.0))),
                            px(7.0), rgb(0xffffff), px(2.0), crate::blue(), gpui::BorderStyle::Solid,
                        ));
                    },
                ).absolute().inset_0().size_full())
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    let Some(bounds) = track.get() else { return; };
                    if this.video_edit_busy { return; }
                    let left = bounds.origin.x + px(8.0);
                    let width = (bounds.size.width - px(16.0)).max(px(1.0));
                    let value = ((event.position.x - left) / width * 100.0).round().clamp(0.0, 100.0) as u8;
                    this.pause_video_playback();
                    this.begin_motion_zoom_slider(event.position.x, value);
                    this.motion_zoom_track = Some((left, width));
                    this.set_motion_zoom_slider(value);
                    cx.stop_propagation();
                    cx.notify();
                })))
            .children((0..3).map(|slot| {
                if slot == 1 {
                    return div().w(px(58.0)).h(px(40.0)).rounded_md().bg(rgb(0xf0f0f1))
                        .flex().items_center().justify_center().text_sm().child(format!("{zoom:.1}×")).into_any_element();
                }
                let sign = if slot == 0 { -1.0 } else { 1.0 };
                div().id(if slot == 0 { "motion-zoom-minus" } else { "motion-zoom-plus" })
                    .w(px(28.0)).h(px(40.0)).rounded_md().bg(rgb(0xf0f0f1))
                    .flex().items_center().justify_center().child(if slot == 0 { "−" } else { "+" })
                    .when(!self.video_edit_busy, |this| this.cursor_pointer().on_click(cx.listener(move |this, _, _, cx| {
                        this.edit_selected_region(|cue| cue.zoom = (cue.zoom + sign * 0.1).clamp(ZoomCue::MINIMUM_ZOOM, ZoomCue::MAXIMUM_ZOOM));
                        cx.notify();
                    })))
                    .into_any_element()
            }))
            .into_any_element()
    }

    fn preview_motion_endpoint(&mut self, endpoint: usize, cx: &mut Context<Self>) {
        let Some(cue) = self.selected_region() else { return; };
        let Some(start) = self.video_clip_timeline.editor_time_for_source(cue.start) else { return; };
        let Some(end) = self.video_clip_timeline.editor_time_for_source(cue.end) else { return; };
        let (visible_start, visible_end) = if self.animation_active && self.video_project.is_none() {
            (self.animation_image_start, self.animation_image_end.min(self.video_duration))
        } else {
            (0.0, self.video_duration)
        };
        if let Some(time) = visible_motion_endpoint(start, end, visible_start, visible_end, endpoint) {
            self.seek_video(time, cx);
        }
    }

    fn begin_motion_transform_drag(&mut self, axis: usize, endpoint: usize, start_x: Pixels, cx: &mut Context<Self>) {
        if self.video_edit_busy || self.motion_transform_drag.is_some() {
            return;
        }
        let Some(cue) = self.selected_region() else {
            return;
        };
        let Some(motion) = cue.transform else {
            return;
        };
        self.pause_video_playback();
        self.preview_motion_endpoint(endpoint, cx);
        self.motion_transform_drag = Some(MotionTransformDrag {
            cue_id: cue.id,
            axis,
            endpoint,
            start_x,
            start_value: transform_value(
                if endpoint == 0 {
                    &motion.start
                } else {
                    &motion.end
                },
                axis,
            ),
            original_cues: self.video_zoom_cues.clone(),
        });
    }

    pub(crate) fn update_motion_transform_drag(&mut self, event: &MouseMoveEvent) -> bool {
        let Some(drag) = self.motion_transform_drag.as_ref() else {
            return false;
        };
        if !event.dragging()
            || self.video_selected_zoom_cue != Some(drag.cue_id)
            || self.video_edit_busy
        {
            return self.end_motion_transform_drag();
        }
        let Some(cue) = self
            .video_zoom_cues
            .iter_mut()
            .find(|cue| cue.id == drag.cue_id)
        else {
            return false;
        };
        let Some(motion) = cue.transform.as_mut() else {
            return false;
        };
        let transform = if drag.endpoint == 0 {
            &mut motion.start
        } else {
            &mut motion.end
        };
        let previous = *transform;
        let delta = f64::from((event.position.x - drag.start_x) / px(1.0));
        set_transform_value(
            transform,
            drag.axis,
            scrubbed_transform_value(drag.axis, drag.start_value, delta, event.modifiers.shift),
        );
        if *transform == previous {
            return false;
        }
        self.rebuild_video_motion_timelines();
        true
    }

    pub(crate) fn end_motion_transform_drag(&mut self) -> bool {
        let Some(drag) = self.motion_transform_drag.take() else {
            return false;
        };
        if drag.original_cues != self.video_zoom_cues {
            self.video_undo_stack
                .push(VideoEditSnapshot::Zoom(drag.original_cues));
            self.video_redo_stack.clear();
            self.persist_video_zoom_cues_quiet();
        }
        true
    }

    fn motion_transform_controls(
        &self,
        motion: TransformMotion,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mut panel = div()
            .flex()
            .flex_col()
            .gap_2()
            .child(Self::inspector_label("Card transform"))
            .child(
                div()
                    .text_xs()
                    .text_color(muted())
                    .child("Position, scale, and rotation relative to scene layout."),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(muted())
                    .child("Drag values left/right. Editing Start or End seeks to that endpoint. Hold Shift for fine adjustments."),
            )
            .child(
                div()
                    .flex()
                    .gap_1()
                    .child(div().w(px(68.0)).flex_none())
                    .child(div().flex_1().min_w_0().text_xs().child("Start"))
                    .child(div().flex_1().min_w_0().text_xs().child("End")),
            );
        for (axis, label) in [
            "Scale", "Move X", "Move Y", "Rotate X", "Rotate Y", "Rotate Z",
        ]
        .into_iter()
        .enumerate()
        {
            let mut row = div()
                .flex()
                .items_center()
                .gap_1()
                .child(div().w(px(68.0)).flex_none().text_xs().child(label));
            for (endpoint, transform) in [motion.start, motion.end].into_iter().enumerate() {
                let value = transform_value(&transform, axis);
                let value_label = match axis {
                    0 => format!("{value:.2}×"),
                    1 | 2 => format!("{:.0}%", value * 50.0),
                    _ => format!("{value:.0}°"),
                };
                let mut cell = div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .items_center()
                    .justify_between()
                    .rounded_md()
                    .bg(rgb(0xf1f1f2));
                for (direction, label) in [(0, "−"), (1, "+")] {
                    if direction == 1 {
                        cell = cell.child(
                            div()
                                .id(("motion-transform-scrub", axis * 2 + endpoint))
                                .flex_1()
                                .min_w_0()
                                .h(px(30.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_xs()
                                .cursor(CursorStyle::ResizeLeftRight)
                                .hover(|style| style.bg(rgb(0xe4e4e7)))
                                .child(value_label.clone())
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                        this.begin_motion_transform_drag(
                                            axis,
                                            endpoint,
                                            event.position.x,
                                            cx,
                                        );
                                        cx.stop_propagation();
                                        cx.notify();
                                    }),
                                ),
                        );
                    }
                    cell = cell.child(
                        div()
                            .id(("motion-transform-step", axis * 4 + endpoint * 2 + direction))
                            .w(px(22.0))
                            .flex_none()
                            .h(px(30.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_sm()
                            .cursor_pointer()
                            .hover(|style| style.bg(rgb(0xe4e4e7)))
                            .child(label)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.preview_motion_endpoint(endpoint, cx);
                                this.edit_selected_region(|cue| {
                                    let Some(motion) = cue.transform.as_mut() else {
                                        return;
                                    };
                                    let transform = if endpoint == 0 {
                                        &mut motion.start
                                    } else {
                                        &mut motion.end
                                    };
                                    let step = if axis == 0 {
                                        0.05
                                    } else if axis <= 2 {
                                        0.02
                                    } else {
                                        5.0
                                    };
                                    let delta = if direction == 0 { -step } else { step };
                                    let value = transform_value(transform, axis) + delta;
                                    set_transform_value(transform, axis, value);
                                });
                                cx.notify();
                            })),
                    );
                }
                row = row.child(cell);
            }
            panel = panel.child(row);
        }
        panel
            .child(Self::inspector_label("After motion"))
            .child(self.segmented(
                "motion-transform-after",
                &["Keep end state", "Reset to layout"],
                if motion.keep_end_state { 0 } else { 1 },
                |this, index| {
                    this.edit_selected_region(|cue| {
                        if let Some(motion) = cue.transform.as_mut() {
                            motion.keep_end_state = index == 0;
                        }
                    });
                },
                cx,
            ))
            .child(self.small_button(
                "motion-transform-reset",
                "Reset transform",
                !self.video_edit_busy,
                cx,
                |this, _| {
                    this.edit_selected_region(|cue| {
                        let keep_end_state = cue.transform.is_some_and(|motion| motion.keep_end_state);
                        cue.transform = Some(TransformMotion { keep_end_state, ..Default::default() });
                    });
                },
            ))
            .into_any_element()
    }

    fn loupe_parameter(&self, index: usize, label: &'static str, value: f64, minimum: f64, maximum: f64, step: f64, cx: &mut Context<Self>) -> AnyElement {
        div().flex().items_center().justify_between().gap_2()
            .child(div().text_sm().child(label))
            .child(div().flex().items_center().gap_1()
                .children((0..3).map(|slot| {
                    if slot == 1 {
                        return div().w(px(48.0)).h(px(32.0)).rounded_md().bg(rgb(0xf0f0f1))
                            .flex().items_center().justify_center().text_xs()
                            .child(format!("{:.0}%", value * 100.0)).into_any_element();
                    }
                    let direction = usize::from(slot == 2);
                    let sign = if direction == 0 { -1.0 } else { 1.0 };
                    div().id(("loupe-parameter", index * 2 + direction))
                        .w(px(28.0)).h(px(32.0)).rounded_md().bg(rgb(0xf0f0f1))
                        .flex().items_center().justify_center().child(if direction == 0 { "−" } else { "+" })
                        .opacity(if self.video_edit_busy { 0.4 } else { 1.0 })
                        .when(!self.video_edit_busy, |this| this.cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.edit_selected_region(|cue| {
                                    let parameter = match index { 0 => &mut cue.loupe.radius, 1 => &mut cue.loupe.bevel, _ => &mut cue.loupe.chromatic_aberration };
                                    *parameter = (*parameter + sign * step).clamp(minimum, maximum);
                                });
                                cx.notify();
                            })))
                        .into_any_element()
                })))
            .into_any_element()
    }

    pub(crate) fn duplicate_selected_motion(&mut self) {
        let Some(mut cue) = self.selected_region() else { return; };
        if self.video_edit_busy { return; }
        let duration = if self.video_project.is_some() { self.video_source_duration } else { self.video_duration };
        let length = cue.end - cue.start;
        self.video_undo_stack.push(VideoEditSnapshot::Zoom(self.video_zoom_cues.clone()));
        self.video_redo_stack.clear();
        cue.id = uuid::Uuid::new_v4();
        cue.start = cue.end.min((duration - length).max(0.0));
        cue.end = (cue.start + length).min(duration);
        self.select_motion_region(cue.id);
        self.video_zoom_cues.push(cue);
        self.rebuild_video_motion_timelines(); self.persist_video_zoom_cues_quiet();
    }

    pub(crate) fn apply_selected_motion_style_to_all(&mut self) {
        let Some(selected) = self.selected_region() else { return; };
        if self.video_edit_busy { return; }
        let original = self.video_zoom_cues.clone();
        for cue in &mut self.video_zoom_cues {
            cue.presentation = selected.presentation; cue.loupe = selected.loupe;
            cue.camera_motion = selected.camera_motion; cue.easing = selected.easing;
            cue.skips_easing = selected.skips_easing; cue.start_animation_early = selected.start_animation_early;
        }
        if original == self.video_zoom_cues { return; }
        self.video_undo_stack.push(VideoEditSnapshot::Zoom(original)); self.video_redo_stack.clear();
        self.rebuild_video_motion_timelines(); self.persist_video_zoom_cues_quiet();
    }

    /// Select a region through one path so the inspector always starts at
    /// its primary controls, even after editing the canvas transform.
    pub(crate) fn select_motion_region(&mut self, id: uuid::Uuid) {
        if self.video_selected_zoom_cue != Some(id)
            || self.scene_selection != crate::scene_ui::SceneSelection::Scene
        {
            self.select_inspector_tab(crate::shell_ui::InspectorTab::Motion);
        }
        self.video_selected_zoom_cue = Some(id);
        self.video_selected_clip = None;
        self.motion_pick = MotionPick::Focus;
    }

    pub(crate) fn apply_selected_zoom_level_to_all(&mut self) {
        let Some(selected) = self.selected_region() else { return; };
        if self.video_edit_busy { return; }
        let original = self.video_zoom_cues.clone();
        for cue in &mut self.video_zoom_cues {
            if cue.transform.is_none() || cue.has_camera_motion() { cue.zoom = selected.zoom; }
        }
        if original == self.video_zoom_cues { return; }
        self.video_undo_stack.push(VideoEditSnapshot::Zoom(original));
        self.video_redo_stack.clear();
        self.rebuild_video_motion_timelines();
        self.persist_video_zoom_cues_quiet();
    }

    /// A source-frame position picker, independent of the current zoom. The
    /// bitmap and hit test share the same fitted rectangle, including letterbox.
    fn motion_position_preview(&self, cue: &ZoomCue, cx: &mut Context<Self>) -> AnyElement {
        let image = if self.video_project.is_some() {
            self.video_frame.clone()
        } else {
            self.displayed_capture_image.clone()
        };
        let dimensions = self.media_dimensions();
        let bounds = std::rc::Rc::new(std::cell::Cell::new(None::<gpui::Bounds<Pixels>>));
        let measured = bounds.clone();
        let pick = self.motion_pick;
        let target = if pick == MotionPick::PanEnd {
            cue.pan_to.unwrap_or(cue.pinned_point)
        } else { cue.pinned_point };
        div()
            .id("motion-position-preview")
            .relative().w_full().h(px(104.0)).flex_none()
            .rounded_md().overflow_hidden().bg(rgb(0xe9edf2))
            .cursor(CursorStyle::Crosshair)
            .when_some(image, |this, image| this.child(
                gpui::img(image).absolute().inset_0().size_full().object_fit(gpui::ObjectFit::Contain)
            ))
            .child(gpui::canvas(
                move |bounds, _, _| {
                    let fitted = crate::fitted_image_bounds(bounds, true, dimensions, 0, false, 0);
                    measured.set(Some(fitted));
                    fitted
                },
                move |_, fitted, window, _| {
                    let center = gpui::point(
                        fitted.origin.x + fitted.size.width * target.x.clamp(0.0, 1.0) as f32,
                        fitted.origin.y + fitted.size.height * target.y.clamp(0.0, 1.0) as f32,
                    );
                    window.paint_quad(gpui::quad(
                        gpui::Bounds::new(center - gpui::point(px(7.0), px(7.0)), gpui::size(px(14.0), px(14.0))),
                        px(7.0), crate::blue(), px(2.0), rgb(0xffffff), gpui::BorderStyle::Solid,
                    ));
                },
            ).absolute().inset_0().size_full())
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                let Some(bounds) = bounds.get() else { return; };
                if !bounds.contains(&event.position) || this.video_edit_busy { return; }
                this.pause_video_playback();
                this.pin_focus_at_media(NormalizedPoint {
                    x: f64::from((event.position.x - bounds.origin.x) / bounds.size.width).clamp(0.0, 1.0),
                    y: f64::from((event.position.y - bounds.origin.y) / bounds.size.height).clamp(0.0, 1.0),
                }, pick, cx);
                cx.stop_propagation();
            }))
            .into_any_element()
    }

    fn motion_position_controls(&self, cue: &ZoomCue, cx: &mut Context<Self>) -> AnyElement {
        let has_pointer = self.scene_has_pointer();
        let manual = !has_pointer || cue.anchor_mode == ZoomAnchorMode::PinnedAnchor;
        div().flex().flex_col().gap_2()
            .child(Self::inspector_label("Zoom position"))
            .when(has_pointer, |panel| panel.child(self.segmented(
                "motion-position", &["Auto", "Manual"], usize::from(manual),
                |this, index| {
                    let mode = if index == 0 { ZoomAnchorMode::SmartAnchor } else { ZoomAnchorMode::PinnedAnchor };
                    if this.selected_region().is_some_and(|cue| (cue.anchor_mode == ZoomAnchorMode::PinnedAnchor) == (index == 1)) { return; }
                    this.pause_video_playback();
                    this.motion_pick = MotionPick::Focus;
                    let frame = this.video_viewport_timeline.frame_at(this.video_position);
                    let point = frame.loupe.map_or(frame.anchor, |lens| lens.center);
                    this.edit_selected_region(|cue| {
                        cue.anchor_mode = mode;
                        if index == 0 { cue.pan_to = None; } else { cue.pinned_point = point; }
                    });
                }, cx,
            )))
            .when(manual, |panel| panel
                .child(self.motion_position_preview(cue, cx))
                .child(div().text_xs().text_color(muted()).child(
                    if self.motion_pick == MotionPick::PanEnd { "Click the preview to set the pan destination." }
                    else { "Click the preview to set the zoom position." }
                )))
            .when(!manual, |panel| panel.child(div().text_xs().text_color(muted()).child(
                "Keeps the cursor and its surrounding activity in view."
            )))
            .into_any_element()
    }

    fn motion_advanced_controls(&self, cue: &ZoomCue, cx: &mut Context<Self>) -> AnyElement {
        let glass = cue.presentation == ZoomPresentation::GlassLoupe;
        let style_index = MotionStyle::ALL.iter().position(|style| *style == cue.motion).unwrap_or(0);
        div().flex().flex_col().gap_3()
            .child(self.scene_toggle_row("motion-early", "Start 350 ms early", cue.start_animation_early, cx, |this| {
                this.edit_selected_region(|cue| cue.start_animation_early = !cue.start_animation_early);
            }))
            .child(div().text_xs().text_color(muted()).child("Applies to this region."))
            .when(glass, |panel| panel
                .child(self.scene_toggle_row("motion-instant", "Instant animation", cue.skips_easing, cx, |this| {
                    this.edit_selected_region(|cue| cue.skips_easing = !cue.skips_easing);
                })))
            .when(glass, |panel| panel
                .child(Self::inspector_label("Glass appearance"))
                .child(self.scene_toggle_row("loupe-glass", "Glass optics", cue.loupe.glass, cx, |this| {
                    this.edit_selected_region(|cue| cue.loupe.glass = !cue.loupe.glass);
                }))
                .when(cue.loupe.glass, |panel| panel
                    .child(self.loupe_parameter(1, "Bevel", cue.loupe.bevel, 0.0, 0.5, 0.05, cx))
                    .child(self.loupe_parameter(2, "Color dispersion", cue.loupe.chromatic_aberration, 0.0, 1.0, 0.05, cx))))
            .when(cue.transform.is_none() || cue.has_camera_motion(), |panel| panel
                .child(Self::inspector_label("Zoom animation"))
                .child(self.segmented("motion-style", &["Hold", "Zoom in", "Zoom out"], style_index,
                    |this, index| { this.edit_selected_region(|cue| cue.motion = MotionStyle::ALL[index]); }, cx))
                .child(Self::inspector_label("Follow response"))
                .child(self.segmented("motion-camera-response", &["Standard", "Smooth", "Focused"],
                    CameraMotion::ALL.iter().position(|motion| *motion == cue.camera_motion).unwrap_or(0),
                    |this, index| { this.edit_selected_region(|cue| cue.camera_motion = CameraMotion::ALL[index]); }, cx))
                .when(self.scene_has_pointer() && cue.anchor_mode != ZoomAnchorMode::PinnedAnchor, |panel| panel
                    .child(Self::inspector_label("Automatic position"))
                    .child(self.segmented("motion-auto-tracking", &["Follow cursor", "Smart framing"],
                        usize::from(cue.anchor_mode == ZoomAnchorMode::SmartAnchor),
                        |this, index| { this.edit_selected_region(|cue| cue.anchor_mode = if index == 0 {
                            ZoomAnchorMode::PointerAnchor
                        } else { ZoomAnchorMode::SmartAnchor }); }, cx))))
            .child(Self::inspector_label("Easing"))
            .child(self.segmented("motion-easing", &["Smooth", "Linear", "Snappy", "Cinematic"],
                crate::scene_ui::easing_index(cue.easing),
                |this, index| { this.edit_selected_region(|cue| cue.easing = crate::recording::viewport::MotionEasing::ALL[index]); }, cx))
            .child(Self::inspector_label("Source timing"))
            .child(div().text_xs().text_color(muted()).child("Fine-tune in the source clip, or drag the region edges in the timeline."))
            .children([false, true].into_iter().map(|end| {
                div().flex().items_center().justify_between().gap_2()
                    .child(div().text_sm().child(format!("{}  {:.2} s", if end { "End" } else { "Start" }, if end { cue.end } else { cue.start })))
                    .child(div().flex().gap_1()
                        .children([-0.1, 0.1].into_iter().enumerate().map(|(direction, delta)| {
                            let id = match (end, direction) {
                                (false, 0) => "motion-start-earlier", (false, _) => "motion-start-later",
                                (true, 0) => "motion-end-earlier", _ => "motion-end-later",
                            };
                            self.small_button(id, if direction == 0 { "−0.1 s" } else { "+0.1 s" }, !self.video_edit_busy, cx, move |this, _| {
                                let duration = this.video_source_duration;
                                this.edit_selected_region(|cue| {
                                    if end { cue.end = (cue.end + delta).clamp(cue.start + ZoomCue::MINIMUM_DURATION, duration); }
                                    else { cue.start = (cue.start + delta).clamp(0.0, cue.end - ZoomCue::MINIMUM_DURATION); }
                                });
                            })
                        })))
            }))
            .child(self.small_button("motion-apply-style", "Apply animation settings to all", !self.video_edit_busy, cx,
                |this, _| this.apply_selected_motion_style_to_all()))
            .into_any_element()
    }

    fn motion_region_transform_controls(&self, cue: &ZoomCue, cx: &mut Context<Self>) -> AnyElement {
        let has_camera = cue.transform.is_none() || cue.has_camera_motion();
        let kind_index = if cue.transform.is_none() { 0 } else if has_camera { 2 } else { 1 };
        div().flex().flex_col().gap_3()
                .child(Self::inspector_label("Animate"))
                .child(self.segmented(
                    "motion-kind",
                    &["Zoom / pan", "Transform", "Both"],
                    kind_index,
                    |this, index| {
                        let default_zoom = this.default_motion_zoom;
                        let initial_pose = this.selected_region().map(|selected| {
                            let previous: Vec<_> = this.video_zoom_cues.iter()
                                .filter(|cue| cue.id != selected.id).cloned().collect();
                            crate::recording::viewport::transform_at(selected.start, &previous)
                        }).unwrap_or(crate::recording::scene::SceneTransform::IDENTITY);
                        this.edit_selected_region(|cue| {
                            if index == 0 {
                                cue.transform = None;
                                if cue.zoom <= 1.0 {
                                    cue.zoom = default_zoom;
                                }
                            } else {
                                if cue.transform.is_none() {
                                    let motion = TransformMotion {
                                        start: initial_pose,
                                        end: initial_pose,
                                        keep_end_state: true,
                                    };
                                    cue.transform = Some(motion);
                                }
                                if index == 1 {
                                    cue.presentation = ZoomPresentation::Classic;
                                    cue.zoom = 1.0;
                                    cue.pan_to = None;
                                    cue.tilt = None;
                                    cue.motion = MotionStyle::Hold;
                                } else if cue.zoom <= 1.0 {
                                    cue.zoom = default_zoom;
                                }
                            }
                        });
                    },
                    cx,
                ))
                .when_some(cue.transform, |panel, motion| {
                    panel.child(self.motion_transform_controls(motion, cx))
                })

            .when(has_camera, |panel| panel
                .child(self.scene_toggle_row("motion-tilt-toggle", "3D tilt", cue.tilt.is_some(), cx, |this| {
                    this.edit_selected_region(|cue| cue.tilt = if cue.tilt.is_some() { None }
                        else { Some(crate::recording::viewport::Tilt { x: 8.0, y: -18.0, z: 0.0 }) });
                }))
                .child(Self::inspector_label("Pan position"))
                .child(self.segmented("motion-pick", &["Start position", "End position"], usize::from(self.motion_pick == MotionPick::PanEnd),
                    |this, index| { this.motion_pick = if index == 0 { MotionPick::Focus } else { MotionPick::PanEnd }; }, cx))
                .child(div().text_xs().text_color(muted()).child("Click the canvas to set this position. Adding an end position creates a pan."))
                .when(cue.pan_to.is_some(), |panel| panel.child(self.small_button("motion-clear-pan", "Remove pan", !self.video_edit_busy, cx,
                    |this, _| { this.edit_selected_region(|cue| cue.pan_to = None); this.motion_pick = MotionPick::Focus; }))))
            .into_any_element()
    }

    /// Selected-region hierarchy follows the recovered ZoomEditor: style,
    /// style-specific control, position, zoom level, then advanced settings.
    pub(crate) fn motion_inspector(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let cue = self.selected_region()?;
        let has_camera = cue.transform.is_none() || cue.has_camera_motion();
        let glass = cue.presentation == ZoomPresentation::GlassLoupe;
        let slices = self.video_clip_timeline.slices_overlapping(cue.start, cue.end);
        let time_label = slices.first().zip(slices.last())
            .map(|(first, last)| format!("{:.1} s – {:.1} s", first.editor_start, last.editor_end))
            .unwrap_or_else(|| "Outside current edit".to_string());
        Some(div().flex().flex_col().gap_3()
            .child(div().flex().items_center().justify_between()
                .child(self.small_button("motion-deselect", "‹ All motion", true, cx, |this, _| {
                    this.select_inspector_tab(crate::shell_ui::InspectorTab::Motion);
                }))
                .child(div().text_xs().text_color(muted()).child(time_label)))
            .child(div().flex().items_center().justify_between()
                .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(if has_camera { "Zoom" } else { "Transform" }))
                .child(div().flex().items_center().gap_2()
                    .child(div().text_xs().text_color(muted()).child(if cue.is_enabled { "On" } else { "Off" }))
                    .child(div().id("motion-enabled-toggle").cursor_pointer().child(self.toggle(cue.is_enabled))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.edit_selected_region(|cue| cue.is_enabled = !cue.is_enabled); cx.notify();
                        })))))
            .when(has_camera, |panel| panel
                .child(Self::inspector_label("Zoom style"))
                .child(self.motion_style_choices(Some(cue.presentation), cx))
                .when(glass, |panel| panel.child(self.loupe_parameter(0, "Glass Loupe size", cue.loupe.radius, 0.25, 0.75, 0.01, cx)))
                .when(!glass, |panel| panel.child(self.scene_toggle_row("motion-instant", "Instant animation", cue.skips_easing, cx, |this| {
                    this.edit_selected_region(|cue| cue.skips_easing = !cue.skips_easing);
                })))
                .child(self.motion_position_controls(&cue, cx))
                .child(div().flex().items_center().justify_between()
                    .child(Self::inspector_label("Zoom level"))
                    .child(self.small_button("motion-zoom-reset", "Reset", !self.video_edit_busy, cx, |this, _| {
                        let zoom = this.default_motion_zoom;
                        this.edit_selected_region(|cue| cue.zoom = zoom);
                    })))
                .child(self.motion_zoom_slider(cue.zoom, cx)))
            .when(!has_camera, |panel| panel.child(self.motion_region_transform_controls(&cue, cx)))
            .child(self.section_header("motion-advanced", "Advanced", None, cx))
            .when(self.section_open("motion-advanced"), |panel| panel.child(self.motion_advanced_controls(&cue, cx)))
            .when(has_camera, |panel| panel
                .child(self.section_header("motion-transform", "Pan & transform", None, cx))
                .when(self.section_open("motion-transform"), |panel| panel.child(self.motion_region_transform_controls(&cue, cx))))
            .child(div().flex().flex_wrap().gap_2()
                .child(self.small_button("motion-duplicate", "Duplicate", !self.video_edit_busy, cx, |this, _| this.duplicate_selected_motion()))
                .when(has_camera && self.video_zoom_cues.len() > 1, |panel| panel.child(self.small_button(
                    "motion-apply-zoom", "Apply zoom level to all", !self.video_edit_busy, cx, |this, _| this.apply_selected_zoom_level_to_all(),
                )))
                .child(self.selection_action_button("motion-delete", true, !self.video_edit_busy, cx, |this, cx| { this.delete_selected_video_zoom(cx); })))
            .into_any_element())
    }

    // ------------------------------------------------------------------
    // Animated screenshots
    // ------------------------------------------------------------------

    pub(crate) fn toggle_animation(&mut self, cx: &mut Context<Self>) {
        if self.animation_active {
            self.exit_animation();
            cx.notify();
            return;
        }
        if self.captured_path.is_none() {
            self.toast = Some("Capture an image first".into());
            cx.notify();
            return;
        }
        if self.crop_active {
            self.cancel_crop();
        }
        self.stop_editing_text();
        self.selected_annotation = None;
        self.annotation_draft = None;
        self.pause_video_playback();
        self.animation_active = true;
        self.inspector_visible = true;
        self.video_duration = self.animation_duration;
        self.video_source_duration = self.animation_duration;
        self.video_clip_timeline = RecordingClipTimeline::full(self.animation_duration);
        self.video_position = 0.0;
        self.video_timeline_zoom = 1.0;
        self.video_timeline_scroll = 0.0;
        self.video_selected_clip = None;
        self.video_selected_zoom_cue = None;
        self.video_zoom_drag = None;
        self.video_seek_drag = None;
        self.video_undo_stack.clear();
        self.video_redo_stack.clear();
        self.motion_pick = MotionPick::Focus;
        // Opening the editor must not apply a preset or recreate deleted regions.
        if self.video_zoom_cues.is_empty() {
            self.animation_preset = None;
        }
        self.rebuild_video_motion_timelines();
        cx.notify();
    }

    pub(crate) fn exit_animation(&mut self) {
        self.pause_video_playback();
        self.walkthrough_mode = false;
        self.animation_active = false;
        self.video_playing = false;
        self.video_position = 0.0;
        self.video_selected_zoom_cue = None;
        self.video_zoom_drag = None;
        self.video_seek_drag = None;
        self.video_viewport_timeline = ViewportTimeline::default();
    }

    pub(crate) fn begin_image_move(&mut self, x: Pixels) {
        self.begin_image_timing_drag(x, None);
    }

    pub(crate) fn begin_image_trim(&mut self, x: Pixels, edge: ClipEdge) {
        self.begin_image_timing_drag(x, Some(edge));
    }

    fn begin_image_timing_drag(&mut self, x: Pixels, edge: Option<ClipEdge>) {
        if !self.animation_active || self.video_project.is_some()
        {
            return;
        }
        self.pause_video_playback();
        self.video_seek_drag = None;
        self.video_selected_zoom_cue = None;
        self.selected_annotation = None;
        self.scene_selection = crate::scene_ui::SceneSelection::Scene;
        self.video_selected_clip = self
            .video_clip_timeline
            .segments
            .first()
            .map(|clip| clip.id);
        self.image_trim_drag = Some(ImageTrimDrag {
            start_x: x,
            start_scroll: self.video_timeline_scroll,
            original_start: self.animation_image_start,
            original_end: self.animation_image_end,
            edge,
            seconds_per_pixel: self.video_duration
                / (self.video_timeline_viewport_width() * self.video_timeline_zoom).max(1.0),
            start: self.animation_image_start,
            end: self.animation_image_end,
        });
    }

    pub(crate) fn update_image_trim(&mut self, event: &MouseMoveEvent) -> bool {
        if self.image_trim_drag.is_none() { return false; }
        if !event.dragging() {
            return self.end_image_trim();
        }
        self.update_image_timeline_drag(event.position.x)
    }

    pub(crate) fn update_image_timeline_drag(&mut self, pointer_x: Pixels) -> bool {
        let Some(mut drag) = self.image_trim_drag else { return false; };
        let (start, end) = drag.range_at_scroll(pointer_x, self.video_timeline_scroll);
        let moved = (f64::from(pointer_x - drag.start_x) + self.video_timeline_scroll - drag.start_scroll).abs() >= 3.0;
        let (edge, length) = match drag.edge {
            None => (start, end - start),
            Some(ClipEdge::Leading) => (start, 0.0),
            Some(ClipEdge::Trailing) => (end, 0.0),
        };
        let (snapped, guide) = crate::timeline_edit::snap_timeline_edges(
            edge, length, &self.timeline_snap_targets(self.video_selected_clip, None, None),
            drag.seconds_per_pixel, self.timeline_ui.snap_enabled && moved,
        );
        (drag.start, drag.end) = match drag.edge {
            None => (snapped.max(0.0), snapped.max(0.0) + length),
            Some(ClipEdge::Leading) => (snapped.clamp(0.0, (end - MINIMUM_IMAGE_DURATION).max(0.0)), end),
            Some(ClipEdge::Trailing) => (start, snapped.max(start + MINIMUM_IMAGE_DURATION)),
        };
        self.timeline_ui.snap_guide = crate::timeline_edit::snap_guide_at_edges(guide, drag.start, drag.end);
        self.image_trim_drag = Some(drag);
        true
    }

    pub(crate) fn end_image_trim(&mut self) -> bool {
        let Some(drag) = self.image_trim_drag.take() else {
            return false;
        };
        self.timeline_ui.snap_guide = None;
        if (drag.start - drag.original_start).abs() > 1e-9
            || (drag.end - drag.original_end).abs() > 1e-9
        {
            self.video_undo_stack.push(VideoEditSnapshot::ImageTiming {
                scene: self.animation_duration,
                start: drag.original_start,
                end: drag.original_end,
            });
            self.video_redo_stack.clear();
            self.restore_image_timing(self.animation_duration.max(drag.end), drag.start, drag.end);
        }
        true
    }

    /// Image trim values remain a draft until release, so cancelling only
    /// drops the draft and keeps both edit history stacks intact.
    pub(crate) fn cancel_image_trim(&mut self) -> bool {
        let cancelled = self.image_trim_drag.take().is_some();
        if cancelled { self.timeline_ui.snap_guide = None; }
        cancelled
    }

    /// Trim boundaries affect only the image layer, not other scene content.
    pub(crate) fn restore_image_timing(&mut self, scene: f64, start: f64, end: f64) {
        self.pause_video_playback();
        self.animation_duration = scene;
        self.animation_image_start = start;
        self.animation_image_end = end;
        self.video_duration = scene;
        self.video_source_duration = scene;
        self.video_clip_timeline = RecordingClipTimeline::full(scene);
        if self.video_selected_clip.is_some() {
            self.video_selected_clip = self
                .video_clip_timeline
                .segments
                .first()
                .map(|clip| clip.id);
        }
        self.video_position = self.video_position.min(scene);
        self.video_timeline_scroll = 0.0;
        self.rebuild_video_motion_timelines();
    }

    pub(crate) fn set_animation_duration(&mut self, duration: f64) {
        if !duration.is_finite() || duration < MINIMUM_IMAGE_DURATION {
            return;
        }
        if (duration - self.animation_duration).abs() < 1e-9 {
            return;
        }
        if !self.animation_active {
            self.animation_duration = duration;
            return;
        }
        self.video_undo_stack.push(VideoEditSnapshot::ImageTiming {
            scene: self.animation_duration,
            start: self.animation_image_start,
            end: self.animation_image_end,
        });
        self.video_redo_stack.clear();
        self.restore_image_timing(
            duration,
            self.animation_image_start,
            self.animation_image_end,
        );
    }

    pub(crate) fn image_visible_at(&self, time: f64) -> bool {
        !self.animation_active
            || self.video_project.is_some()
            || (time >= self.animation_image_start && time < self.animation_image_end)
    }

    pub(crate) fn apply_motion_preset(&mut self, preset: MotionPreset) {
        if !self.animation_active {
            return;
        }
        self.pause_video_playback();
        let cues = preset.cues(self.video_duration);
        if cues != self.video_zoom_cues {
            self.video_undo_stack
                .push(VideoEditSnapshot::Zoom(self.video_zoom_cues.clone()));
            self.video_redo_stack.clear();
            self.video_zoom_cues = cues;
        }
        self.animation_preset = Some(preset);
        self.video_selected_zoom_cue = None;
        self.video_position = 0.0;
        self.rebuild_video_motion_timelines();
    }

    /// Plays the animation in the preview by advancing the playhead on a
    /// wall-clock timer; the viewport timeline provides the camera state.
    pub(crate) fn start_animation_playback(&mut self, cx: &mut Context<Self>) {
        if !self.animation_active || self.video_playing || self.video_duration <= 0.0 {
            return;
        }
        self.cancel_timeline_edit(cx);
        self.video_seek_drag = None;
        self.follow_timeline_playhead(true);
        self.finish_annotation_interaction();
        if self.video_position >= self.video_duration - 0.01 {
            self.video_position = 0.0;
        }
        let generation = self.video_playback_generation.clone();
        let token = generation.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        self.video_playing = true;
        let mut last_tick = Instant::now();
        cx.spawn(async move |weak, cx| loop {
            Timer::after(Duration::from_millis(16)).await;
            if generation.load(std::sync::atomic::Ordering::SeqCst) != token {
                break;
            }
            let now = Instant::now();
            let elapsed = now.duration_since(last_tick).as_secs_f64();
            last_tick = now;
            if weak
                .update(cx, |this, cx| {
                    let next = this.video_position + elapsed;
                    this.video_position = if next >= this.video_duration {
                        // A sequence continues into the next image; a single
                        // scene loops, like the exported GIF.
                        if this.image_scenes.len() > 1 {
                            let next_index = (this.image_scene_index + 1) % this.image_scenes.len();
                            this.store_current_scene();
                            this.load_image_scene(next_index);
                        }
                        0.0
                    } else {
                        next
                    };
                    this.follow_timeline_playhead(false);
                    cx.notify();
                })
                .is_err()
            {
                break;
            }
        })
        .detach();
    }

    pub(crate) fn toggle_animation_playback(&mut self, cx: &mut Context<Self>) {
        if self.video_playing {
            self.pause_video_playback();
        } else {
            self.start_animation_playback(cx);
        }
    }

    /// Keyboard transport for the animated screenshot editor.
    pub(crate) fn handle_animation_key(
        &mut self,
        event: &gpui::KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.animation_active || self.editing_text.is_some() || self.crop_active {
            return false;
        }
        if self.handle_timeline_shortcut(event, cx) {
            return true;
        }
        let keystroke = &event.keystroke;
        if keystroke.modifiers.control || keystroke.modifiers.platform {
            match keystroke.key.as_str() {
                "z" if !keystroke.modifiers.shift => self.undo_current(cx),
                "z" | "y" => self.redo_current(cx),
                _ => return false,
            }
            return true;
        }
        if matches!(keystroke.key.as_str(), "delete" | "backspace")
            && self.selected_annotation.is_some()
        {
            return self.handle_key(event);
        }
        if self.walkthrough_mode && matches!(keystroke.key.as_str(), "enter" | "escape") {
            self.walkthrough_mode = false;
            return true;
        }
        match keystroke.key.as_str() {
            "space" => {
                self.toggle_animation_playback(cx);
                true
            }
            "m" => {
                let position = self.video_position;
                self.add_motion_region_at(position, cx);
                true
            }
            "left" => {
                self.pause_video_playback();
                self.finish_annotation_interaction();
                self.video_position = (self.video_position - 0.5).max(0.0);
                true
            }
            "right" => {
                self.pause_video_playback();
                self.finish_annotation_interaction();
                self.video_position = (self.video_position + 0.5).min(self.video_duration);
                true
            }
            "delete" | "backspace" => {
                self.delete_selected_video_edit(cx);
                true
            }
            "escape" => {
                self.finish_annotation_interaction();
                self.video_selected_zoom_cue = None;
                self.video_selected_clip = None;
                self.tool = crate::Tool::Select;
                true
            }
            _ => false,
        }
    }

    // ------------------------------------------------------------------
    // Export
    // ------------------------------------------------------------------

    /// Flattens the capture with its annotations (no scene styling) so the
    /// scene compositor can treat it exactly like a video frame.
    pub(crate) fn render_annotated_capture(&mut self) -> Result<RgbaImage, String> {
        self.rebuild_redactions()?;
        let capture_path = self
            .processed_capture_path
            .as_ref()
            .or(self.captured_path.as_ref())
            .ok_or_else(|| "Capture an image first".to_string())?;
        let (width, height) = image::image_dimensions(capture_path)
            .map_err(|error| format!("Could not read capture: {error}"))?;
        let stroke_scale = width.min(height) as f32 / 800.0;
        let href = xml_escape(&capture_path.to_string_lossy());
        let mut svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}"><image href="{href}" x="0" y="0" width="{width}" height="{height}" preserveAspectRatio="none"/><g>"#
        );
        let marks: Vec<_> = self.annotations.iter().filter(|mark| !mark.is_canvas()).cloned().collect();
        svg.push_str(&crate::annotations_svg(&marks, 0.0, 0.0, width, height, stroke_scale));
        svg.push_str("</g></svg>");
        let mut options = resvg::usvg::Options::default();
        options.fontdb = crate::recording::scene::shared_fontdb();
        let tree = resvg::usvg::Tree::from_str(&svg, &options)
            .map_err(|error| format!("Could not flatten annotations: {error}"))?;
        let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height)
            .ok_or_else(|| "Capture dimensions are too large".to_string())?;
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::identity(),
            &mut pixmap.as_mut(),
        );
        let mut data = pixmap.take();
        // tiny-skia stores premultiplied alpha; the compositor expects straight.
        for pixel in data.chunks_exact_mut(4) {
            let alpha = pixel[3] as u32;
            if alpha > 0 && alpha < 255 {
                for channel in pixel.iter_mut().take(3) {
                    *channel = ((*channel as u32 * 255 + alpha / 2) / alpha).min(255) as u8;
                }
            }
        }
        RgbaImage::from_raw(width, height, data)
            .ok_or_else(|| "Flattened capture had an invalid byte count".to_string())
    }

    /// Recording export: the whole scene, not only the source clip.
    pub(crate) fn export_video_recording(&mut self, cx: &mut Context<Self>) {
        if self.video_edit_busy || self.export_progress.is_some() {
            self.toast = Some("Wait for the current edit to finish".into());
            cx.notify();
            return;
        }
        let Some(session) = self.video_project.clone() else {
            return;
        };
        // Export is the single "keep my work" action: promote the autosaved
        // edit draft to the project before rendering the file.
        if let Err(error) = session.commit_draft() {
            self.toast = Some(format!("Could not save recording edits: {error}").into());
            cx.notify();
            return;
        }
        let format = self.export_format;
        let suggested_name = chrono::Local::now()
            .format(&format!("Lahza-%Y-%m-%d_%H-%M-%S.{}", format.extension()))
            .to_string();
        let (source, request) = self.video_export_request(&session, format);
        self.prompt_and_run_scene_export(source, request, suggested_name, cx);
    }

    /// The current recording edit as an export job in `format`, with the
    /// export settings from the inspector. The destination is left empty.
    pub(crate) fn video_export_request(
        &mut self,
        session: &crate::RecordingSession,
        format: ExportFormat,
    ) -> (SceneSource, SceneExportRequest) {
        self.sync_reframe();
        let mut request = SceneExportRequest::new(
            Default::default(),
            format,
            self.export_canvas_height(),
            self.scene_style(),
            self.video_viewport_timeline.clone(),
            self.video_duration,
        );
        request.frame_rate = self.export_frame_rate;
        request.loop_forever = self.export_loop;
        request.video_codec = self.export_codec;
        request.motion_blur = self.export_motion_blur;
        request.include_audio = !self.video_audio_muted;
        request.noise_reduction = self.video_noise_reduction;
        request.audio_clips = self.video_audio_clips.iter().cloned().map(|clip| (session.directory.join(&clip.file), clip)).collect();
        request.recording_duration = Some(self.video_recording_duration);
        request.overlay = self.annotation_overlay_source();
        request.canvas_overlay = crate::scene_ui::canvas_overlay_source_for(
            self.annotations.clone(), self.selected_canvas_ratio() as f64);
        request.studio_overlays = Some((*self.studio_overlays()).clone());
        let source = SceneSource::Video {
            media: Self::media_source_for(&session, false),
            clips: self.video_clip_timeline.clone(),
            pointer: self
                .video_pointer_synthesized
                .then(|| self.video_pointer_timeline.clone()),
            camera: self.video_camera_path.clone(),
        };
        (source, request)
    }

    /// Animated screenshot export through the same scene pipeline.
    pub(crate) fn export_animated_screenshot(&mut self, cx: &mut Context<Self>) {
        if self.export_progress.is_some() {
            self.toast = Some("An export is already running".into());
            cx.notify();
            return;
        }
        if !self.animation_active || self.video_duration <= 0.0 {
            return;
        }
        // Annotations travel as a timed overlay so entrance and exit effects
        // render frame by frame; the media itself is the processed capture.
        if let Err(error) = self.rebuild_redactions() {
            self.toast = Some(format!("Export failed: {error}").into());
            cx.notify();
            return;
        }
        let Some(image) = self.capture_rgba.as_ref().map(|image| (**image).clone()) else {
            self.toast = Some("Capture an image first".into());
            cx.notify();
            return;
        };
        let format = self.export_format;
        let suggested_name = chrono::Local::now()
            .format(&format!(
                "Lahza-%Y-%m-%d_%H-%M-%S-%3f.{}",
                format.extension()
            ))
            .to_string();
        let mut request = SceneExportRequest::new(
            Default::default(),
            format,
            self.export_canvas_height(),
            self.scene_style(),
            self.video_viewport_timeline.clone(),
            self.video_duration,
        );
        request.media_start = self.animation_image_start;
        request.media_end = Some(self.animation_image_end);
        request.frame_rate = self.export_frame_rate;
        request.loop_forever = self.export_loop;
        request.video_codec = self.export_codec;
        request.motion_blur = self.export_motion_blur;
        request.overlay = self.annotation_overlay_source();
        request.canvas_overlay = crate::scene_ui::canvas_overlay_source_for(
            self.annotations.clone(), self.selected_canvas_ratio() as f64);
        let mut source = SceneSource::Image {
            image,
            pointer: self.animation_pointer_timeline(),
        };
        // A sequence always exports from its first image, whichever one is
        // being edited; the rest follow back to back.
        if self.image_scenes.len() > 1 {
            self.store_current_scene();
            let first_scene = &self.image_scenes[0];
            let (width, height) = request.style.export_canvas_size(first_scene.dimensions.0, first_scene.dimensions.1, request.canvas_height);
            let aspect = width as f64 / height as f64;
            let mut segments = self.image_scenes.iter().map(|scene| self.image_segment(scene, aspect));
            let first = segments.next().expect("sequence has scenes");
            request.viewport = first.viewport;
            request.duration = first.duration;
            request.overlay = first.overlay;
            request.canvas_overlay = first.canvas_overlay;
            request.media_start = first.media_start;
            request.media_end = first.media_end;
            source = SceneSource::Image {
                image: first.image,
                pointer: first.pointer,
            };
            request.followers = segments.collect();
        }
        self.prompt_and_run_scene_export(source, request, suggested_name, cx);
    }

    fn image_segment(&self, scene: &ImageScene, aspect: f64) -> ImageSegment {
        ImageSegment {
            image: (*scene.rgba).clone(),
            pointer: scene.pointer.clone(),
            viewport: scene.viewport.clone(),
            duration: scene.duration,
            media_start: scene.image_start,
            media_end: Some(scene.image_end),
            canvas_overlay: crate::scene_ui::canvas_overlay_source_for(scene.annotations.clone(), aspect),
            overlay: crate::scene_ui::overlay_source_for(
                scene.annotations.clone(),
                scene.viewport.clone(),
                scene.dimensions.0,
                scene.dimensions.1,
            ),
        }
    }

    // ------------------------------------------------------------------
    // Image scene sequence
    // ------------------------------------------------------------------

    /// The live image and its scene as a storable entry.
    fn current_image_scene(&self) -> Option<ImageScene> {
        Some(ImageScene {
            original_capture: self.original_capture.clone(),
            source_crop: self.source_crop,
            path: self.captured_path.clone()?,
            processed_path: self.processed_capture_path.clone(),
            dimensions: self.captured_dimensions?,
            rgba: self.capture_rgba.clone()?,
            render: self.displayed_capture_image.clone()?,
            annotations: self.annotations.clone(),
            zoom_cues: self.video_zoom_cues.clone(),
            duration: self.animation_duration,
            image_start: self.animation_image_start,
            image_end: self.animation_image_end,
            preset: self.animation_preset,
            pointer_capture: self.animation_pointer_capture.clone(),
            walkthrough_stops: self.walkthrough_stops.clone(),
            viewport: self.video_viewport_timeline.clone(),
            pointer: self.animation_pointer_timeline(),
        })
    }

    /// Writes the live scene back into the sequence.
    pub(crate) fn store_current_scene(&mut self) {
        let Some(scene) = self.current_image_scene() else {
            return;
        };
        if let Some(slot) = self.image_scenes.get_mut(self.image_scene_index) {
            *slot = scene;
        } else {
            self.image_scenes.push(scene);
            self.image_scene_index = self.image_scenes.len() - 1;
        }
    }

    /// Makes the sequence entry at `index` the live scene. Playback state
    /// is left alone so a running sequence flows into the next image.
    pub(crate) fn load_image_scene(&mut self, index: usize) {
        let Some(scene) = self.image_scenes.get(index).cloned() else {
            return;
        };
        self.stop_editing_text();
        self.image_scene_index = index;
        self.original_capture = scene.original_capture;
        self.source_crop = scene.source_crop;
        self.crop_session = None;
        self.captured_path = Some(scene.path);
        self.processed_capture_path = scene.processed_path;
        self.captured_dimensions = Some(scene.dimensions);
        self.capture_rgba = Some(scene.rgba);
        self.displayed_capture_image = Some(scene.render);
        self.annotations = scene.annotations;
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.crop_undo_stack.clear();
        self.crop_redo_stack.clear();
        self.crop_active = false;
        self.video_zoom_cues = scene.zoom_cues;
        self.animation_duration = scene.duration;
        self.animation_image_start = scene.image_start;
        self.animation_image_end = scene.image_end;
        self.animation_preset = scene.preset;
        self.animation_pointer_capture = scene.pointer_capture;
        self.walkthrough_stops = scene.walkthrough_stops;
        self.walkthrough_mode = false;
        self.video_duration = scene.duration;
        self.video_source_duration = scene.duration;
        self.video_clip_timeline = RecordingClipTimeline::full(scene.duration);
        self.video_position = 0.0;
        self.video_selected_zoom_cue = None;
        self.video_zoom_drag = None;
        self.video_seek_drag = None;
        self.video_undo_stack.clear();
        self.video_redo_stack.clear();
        self.reset_annotation_interaction();
        self.rebuild_video_motion_timelines();
    }

    pub(crate) fn switch_image_scene(&mut self, index: usize, cx: &mut Context<Self>) {
        if index == self.image_scene_index || index >= self.image_scenes.len() {
            return;
        }
        self.pause_video_playback();
        self.store_current_scene();
        self.load_image_scene(index);
        cx.notify();
    }

    /// Picks an image file and appends it to the sequence as a new scene.
    pub(crate) fn add_image_scene_dialog(&mut self, cx: &mut Context<Self>) {
        let prompt = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Add image scene".into()),
        });
        cx.spawn(async move |weak, cx| {
            let selected = match prompt.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                _ => None,
            };
            let Some(path) = selected else {
                return;
            };
            let loaded = cx
                .background_executor()
                .spawn(async move {
                    image::open(&path)
                        .map(|image| (path, image.to_rgba8()))
                        .map_err(|error| error.to_string())
                })
                .await;
            let _ = weak.update(cx, |this, cx| {
                match loaded {
                    Ok((path, image)) => this.add_image_scene(path, image),
                    Err(error) => this.toast = Some(format!("Could not add image: {error}").into()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn add_image_scene(&mut self, path: std::path::PathBuf, image: RgbaImage) {
        if !self.animation_active || image.width() == 0 || image.height() == 0 {
            return;
        }
        self.pause_video_playback();
        self.store_current_scene();
        let duration = self.animation_duration;
        // Each new image starts without motion; presets are applied explicitly.
        let zoom_cues = Vec::new();
        let viewport = ViewportTimeline::build_static(&zoom_cues, duration);
        let dimensions = (image.width(), image.height());
        let rgba = Arc::new(image.clone());
        self.image_scenes.push(ImageScene {
            original_capture: None,
            source_crop: super::CropRect::UNIT,
            path,
            processed_path: None,
            dimensions,
            render: crate::cached_render_image(image),
            rgba,
            annotations: Vec::new(),
            zoom_cues,
            duration,
            image_start: 0.0,
            image_end: duration,
            preset: None,
            pointer_capture: Default::default(),
            walkthrough_stops: Vec::new(),
            viewport,
            pointer: None,
        });
        let index = self.image_scenes.len() - 1;
        self.load_image_scene(index);
    }

    /// Drops the live scene from the sequence and shows its neighbour.
    pub(crate) fn remove_image_scene(&mut self, cx: &mut Context<Self>) {
        if self.image_scenes.len() < 2 || self.image_scene_index >= self.image_scenes.len() {
            return;
        }
        self.pause_video_playback();
        let removed = self.image_scenes.remove(self.image_scene_index);
        self.retire_image(Some(removed.render));
        let index = self.image_scene_index.min(self.image_scenes.len() - 1);
        self.load_image_scene(index);
        cx.notify();
    }

    /// Total length of the sequence in seconds.
    pub(crate) fn sequence_duration(&self) -> f64 {
        if self.image_scenes.len() < 2 {
            return self.video_duration;
        }
        self.image_scenes
            .iter()
            .enumerate()
            .map(|(index, scene)| {
                if index == self.image_scene_index {
                    self.video_duration
                } else {
                    scene.duration
                }
            })
            .sum()
    }

    fn prompt_and_run_scene_export(
        &mut self,
        source: SceneSource,
        mut request: SceneExportRequest,
        suggested_name: String,
        cx: &mut Context<Self>,
    ) {
        let directory =
            std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("/tmp"));
        let prompt = cx.prompt_for_new_path(&directory, Some(&suggested_name));
        cx.spawn(async move |weak, cx| {
            let selected = match prompt.await {
                Ok(Ok(destination)) => Ok(destination),
                Ok(Err(error)) => Err(error.to_string()),
                Err(error) => Err(error.to_string()),
            };
            let destination = match selected {
                Ok(Some(path)) => path,
                Ok(None) => {
                    let _ = weak.update(cx, |this, cx| {
                        this.toast = Some("Export cancelled".into());
                        cx.notify();
                    });
                    return;
                }
                Err(error) => {
                    let _ = weak.update(cx, |this, cx| {
                        this.toast = Some(format!("Export failed: {error}").into());
                        cx.notify();
                    });
                    return;
                }
            };
            // A typed extension wins over the picker; otherwise the picker's
            // format decides the extension.
            match ExportFormat::from_path(&destination) {
                Some(format) => {
                    request.format = format;
                    request.frame_rate = format.default_frame_rate();
                    request.destination = destination;
                }
                None => request.destination = request.format.apply_to_path(&destination),
            }
            let _ = weak.update(cx, |this, cx| this.run_scene_export(source, request, cx));
        })
        .detach();
    }

    fn run_scene_export(
        &mut self,
        source: SceneSource,
        request: SceneExportRequest,
        cx: &mut Context<Self>,
    ) {
        // The file picker is asynchronous; another export may have started
        // since this request was captured. Keep ownership of the active job.
        if self.export_progress.is_some() {
            self.toast = Some("An export is already running".into());
            cx.notify();
            return;
        }
        // Source and request own a snapshot of the edit. Preview playback and
        // later edits can continue independently on the UI's project state.
        let progress = Arc::new(ExportProgress::default());
        self.export_progress = Some(progress.clone());
        self.export_label = format!("Exporting {}…", request.format.label()).into();
        self.export_panel_collapsed = false;
        self.export_filename = request.destination.file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Export".into()).into();
        self.toast = None;
        let format_label = request.format.label();
        let destination = request.destination.clone();
        let mut request = request;
        let job_progress = progress.clone();
        let task = cx.background_executor().spawn(async move {
            export_scene(source, &mut request, &progress).map_err(|error| error.to_string())
        });
        // Keep the progress bar fresh while the background render runs.
        cx.spawn(async move |weak, cx| loop {
            Timer::after(Duration::from_millis(120)).await;
            let active = weak
                .update(cx, |this, cx| {
                    cx.notify();
                    this.export_progress.as_ref()
                        .is_some_and(|active| Arc::ptr_eq(active, &job_progress))
                })
                .unwrap_or(false);
            if !active {
                break;
            }
        })
        .detach();
        cx.spawn(async move |weak, cx| {
            let result = task.await;
            let _ = weak.update(cx, |this, cx| {
                this.export_progress = None;
                this.toast = Some(match result {
                    Ok(()) => {
                        crate::notifications::Notification::exported(
                            format!("Exported {format_label} to {}", destination.display()),
                            destination,
                        )
                    }
                    Err(error) if error == "export cancelled" => "Export cancelled".into(),
                    Err(error) => format!("Export failed: {error}").into(),
                });
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn cancel_export(&mut self) {
        if let Some(progress) = self.export_progress.as_ref() {
            progress.cancel();
        }
    }

    /// A nonmodal job panel. Only the panel itself occupies pointer space,
    /// leaving the canvas and timeline available while the snapshot renders.
    pub(crate) fn export_status_overlay(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let progress = self.export_progress.as_ref()?;
        let fraction = progress.fraction();
        let cancelling = progress.is_cancelled();
        let collapsed = self.export_panel_collapsed;
        let label = if cancelling {
            "Cancelling…".to_string()
        } else {
            format!("{} {:.0}%", self.export_label, fraction * 100.0)
        };
        Some(
            div()
                .absolute()
                .bottom(px(16.0))
                .right(px(16.0))
                .w(px(320.0))
                .p_4()
                .rounded_xl()
                .border_1()
                .border_color(line())
                .bg(rgb(0xffffff))
                .text_color(crate::ink())
                .shadow_lg()
                .flex()
                .flex_col()
                .gap_3()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child(label))
                        .child(
                            div()
                                .id("export-collapse")
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .text_xs()
                                .text_color(muted())
                                .cursor_pointer()
                                .hover(|style| style.bg(rgb(0xf1f2f4)))
                                .child(if collapsed { "Expand" } else { "Minimize" })
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.export_panel_collapsed = !this.export_panel_collapsed;
                                    cx.notify();
                                })),
                        ),
                )
                .when(!collapsed, |panel| {
                    panel
                        .child(div().text_xs().text_color(muted()).text_ellipsis().child(self.export_filename.clone()))
                        .child(div().text_xs().text_color(muted())
                            .child("Keep editing. This export uses the version you started with."))
                        .children(progress.compositor_label().map(|label| div().text_xs().text_color(muted()).child(format!("Composition: {label}"))))
                        .children(progress.encoder_label().map(|label| div().text_xs().text_color(muted()).child(format!("Encoding: {label}"))))
                })
                .child(
                    div().w_full().h(px(5.0)).rounded_full().bg(rgb(0xe9ebef)).overflow_hidden()
                        .child(div().h_full().w(gpui::relative(fraction as f32)).rounded_full().bg(orange(false))),
                )
                .when(!collapsed, |panel| {
                    panel.child(
                        div().flex().items_center().justify_between().gap_2()
                            .child(div().text_xs().child(if cancelling {
                                "Stopping export…".into()
                            } else {
                                progress.remaining_label()
                            }))
                            .child(
                                div().id("export-cancel").px_2().py_1().rounded_md()
                                    .border_1().border_color(line()).text_xs()
                                    .opacity(if cancelling { 0.5 } else { 1.0 })
                                    .child("Cancel")
                                    .when(!cancelling, |button| {
                                        button.cursor_pointer()
                                            .hover(|style| style.bg(rgb(0xf1f2f4)))
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.cancel_export();
                                                cx.notify();
                                            }))
                                    }),
                            ),
                    )
                })
                .into_any_element(),
        )
    }
}
