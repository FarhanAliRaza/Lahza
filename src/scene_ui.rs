//! Scene-level editing shared by both editors: exact paused previews and
//! responsive playback proxies, 3D transform editing (inspector and
//! direct manipulation), background effects, watermark, pointer styling,
//! export options, the preset library, and timed annotations.

use gpui::{
    canvas, div, hsla, img, point, prelude::*, px, quad, rgb, size, AnyElement, Bounds,
    ContentMask, Context, CursorStyle, FontWeight, Hsla, Modifiers, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ObjectFit, PathBuilder, Pixels, Point, RenderImage,
    ScrollDelta, ScrollWheelEvent, Window,
};
use image::RgbaImage;
use crate::scene_preview::{playback_canvas_size, PreviewJob, PreviewKey, PreviewMedia, PreviewWorker};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use crate::{
    accent, accent_tint, brand, cached_render_image, control_bg, control_hover, danger,
    danger_tint, ink, lane_clip, line, muted, on_accent, paint_annotation, surface, TextRoles,
    paint_highlights,
    recording::{
        cursor_assets::CursorFamily,
        export::{estimate_size_bytes, format_size, ExportFormat, ExportResolution},
        model::NormalizedPoint,
        presets::ScenePreset,
        scene::{
            CameraLayout, CameraOverlay, MediaProjection, PointerOverlay, SceneCompositor,
            SceneGeometry, SceneStyle, SceneTransform, WatermarkPosition,
        },
        viewport::{MotionEasing, ViewportFrame, ZoomCue},
    },
    timed::{self, AnnotationTiming, EntranceEffect, ExitEffect},
    AnnotationMark, Studio, Tool, VideoEditSnapshot, BACKGROUND_PRESETS,
};
use crate::controls::{field, hint, inline_error};
use crate::theme::{SPACE_1, SPACE_3};

/// Pack non-overlapping intervals together without ever stacking busy rows.
fn annotation_lane_rows(marks: &[AnnotationMark], duration: f64) -> Vec<usize> {
    let span = |mark: &AnnotationMark| mark.timing
        .map(|timing| (timing.start, timing.end))
        .unwrap_or((0.0, duration.max(f64::EPSILON)));
    let mut order: Vec<_> = (0..marks.len()).collect();
    order.sort_by(|a, b| span(&marks[*a]).0.total_cmp(&span(&marks[*b]).0));
    let mut row_ends = Vec::new();
    let mut rows = vec![0; marks.len()];
    for index in order {
        let (start, end) = span(&marks[index]);
        let row = row_ends.iter().position(|row_end| *row_end <= start)
            .unwrap_or_else(|| {
                row_ends.push(f64::NEG_INFINITY);
                row_ends.len() - 1
            });
        row_ends[row] = end;
        rows[index] = row;
    }
    rows
}

/// Which part of the scene the inspector is editing.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum SceneSelection {
    #[default]
    Scene,
    Media,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MediaDragKind {
    Move,
    /// Drag horizontally for Y rotation, vertically for X rotation.
    Rotate,
    /// Drag horizontally for Z rotation.
    Spin,
    Scale,
}

/// Dragging the camera bubble; positions are canvas pixels of the preview.
#[derive(Clone, Copy, Debug)]
pub(crate) struct CameraDrag {
    start: Point<Pixels>,
    start_center: (f64, f64),
    canvas_size: (f64, f64),
    source_aspect: f64,
    zoom: f64,
    /// Snap guide per axis, while snapped.
    guides: (Option<f64>, Option<f64>),
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct MediaDrag {
    pub kind: MediaDragKind,
    pub start: Point<Pixels>,
    pub original: SceneTransform,
    pub canvas_size: (f32, f32),
    /// Canvas-local pivot for scale drags.
    pub pivot: (f32, f32),
}

/// Keep the preview parked while its marker moves in source coordinates.
/// Rebuild the camera and save one undo entry when the gesture ends.
#[derive(Clone, Debug)]
pub(crate) struct FocusDrag {
    pick: crate::motion_ui::MotionPick,
    projection: MediaProjection,
    viewport: ViewportFrame,
    crop: crate::CropRect,
    original_cues: Vec<ZoomCue>,
}

impl FocusDrag {
    fn target_at(&self, x: f64, y: f64) -> NormalizedPoint {
        let (u, v) = self.projection.unproject(x, y);
        let (left, top, visible_x, visible_y) = self.crop.visible_rect(self.viewport);
        NormalizedPoint {
            x: left + u.clamp(0.0, 1.0) * visible_x,
            y: top + v.clamp(0.0, 1.0) * visible_y,
        }
        .clamped()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AnnotationDragKind {
    Move,
    Leading,
    Trailing,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct AnnotationDrag {
    pub index: usize,
    pub kind: AnnotationDragKind,
    pub start_x: Pixels,
    pub original: AnnotationTiming,
    pub original_timing: Option<AnnotationTiming>,
    pub start_scroll: f64,
    pub seconds_per_pixel: f64,
}

impl AnnotationDrag {
    fn preview(self, pointer_x: Pixels, scroll: f64, duration: f64, targets: &[f64], snapping: bool)
        -> (Option<AnnotationTiming>, Option<f64>)
    {
        let delta = (f64::from((pointer_x - self.start_x) / px(1.0))
            + scroll - self.start_scroll) * self.seconds_per_pixel;
        if delta.abs() < 1e-9 { return (self.original_timing, None); }
        let mut timing = self.original.clamped(duration);
        let (edge, length) = match self.kind {
            AnnotationDragKind::Move => (timing.start, timing.duration()),
            AnnotationDragKind::Leading => (timing.start, 0.0),
            AnnotationDragKind::Trailing => (timing.end, 0.0),
        };
        let (snapped, guide) = crate::timeline_edit::snap_timeline_edges(
            edge + delta, length, targets, self.seconds_per_pixel, snapping,
        );
        match self.kind {
            AnnotationDragKind::Move => {
                timing.start = snapped.clamp(0.0, (duration - length).max(0.0));
                timing.end = timing.start + length;
            }
            AnnotationDragKind::Leading => {
                timing.start = snapped.clamp(0.0, (timing.end - AnnotationTiming::MINIMUM_DURATION).max(0.0));
            }
            AnnotationDragKind::Trailing => {
                let minimum_end = timing.start + AnnotationTiming::MINIMUM_DURATION;
                timing.end = snapped.clamp(minimum_end, duration.max(minimum_end));
            }
        }
        let timing = timing.clamped(duration);
        let guide = crate::timeline_edit::snap_guide_at_edges(guide, timing.start, timing.end);
        (Some(timing), guide)
    }
}

#[cfg(test)]
mod annotation_timeline_drag_tests {
    use super::*;

    fn drag(kind: AnnotationDragKind) -> AnnotationDrag {
        let timing = AnnotationTiming { start: 2.0, end: 4.0, ..Default::default() };
        AnnotationDrag {
            index: 0, kind, start_x: px(100.0), start_scroll: 200.0,
            original: timing, original_timing: Some(timing), seconds_per_pixel: 0.01,
        }
    }

    #[test]
    fn annotation_drag_keeps_its_range_under_the_pointer_when_scrolled() {
        let drag = drag(AnnotationDragKind::Move);
        let (timing, guide) = drag.preview(px(150.0), 250.0, 10.0, &[5.05], true);
        let timing = timing.unwrap();
        assert!((timing.start - 3.05).abs() < 1e-9);
        assert!((timing.end - 5.05).abs() < 1e-9);
        assert_eq!(guide, Some(5.05));
        assert_eq!(drag.preview(px(100.0), 200.0, 10.0, &[2.05], true), (drag.original_timing, None));
    }

    #[test]
    fn annotation_limits_remove_unreachable_snap_guides() {
        let drag = drag(AnnotationDragKind::Leading);
        let (timing, guide) = drag.preview(px(305.0), 200.0, 10.0, &[4.05], true);
        let timing = timing.unwrap();
        assert!((timing.start - 3.8).abs() < 1e-9);
        assert_eq!(guide, None);
    }

    #[test]
    fn untouched_untimed_annotation_stays_untimed() {
        let mut drag = drag(AnnotationDragKind::Move);
        drag.original_timing = None;
        assert_eq!(drag.preview(px(100.0), 200.0, 10.0, &[], false), (None, None));
    }
}

/// Sliders driven by `Studio::set_slider_value` with ids from `BASE` up.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SceneSlider {
    Scale,
    PositionX,
    PositionY,
    RotationX,
    RotationY,
    RotationZ,
    Perspective,
    AnchorX,
    AnchorY,
    Blur,
    Noise,
    Vignette,
    PointerScale,
    WatermarkSize,
    WatermarkOpacity,
    DefaultZoom,
    AnnotationTransition,
    CameraSize,
    CameraMargin,
    SubtitlePosition,
    SubtitleSize,
    CursorIdleDelay,
    CursorRotation,
    CursorLoopDuration,
    CameraZoomedSize,
    CameraCornerRadius,
    CameraCropZoom,
    CameraCropX,
    CameraCropY,
    CameraSplitRatio,
    CameraCutoutSoftness,
    CameraCutoutInset,
    MotionBlurAmount,
}

impl SceneSlider {
    fn is_camera(self) -> bool {
        matches!(self, Self::CameraSize | Self::CameraMargin | Self::CameraZoomedSize | Self::CameraCornerRadius
            | Self::CameraCropZoom | Self::CameraCropX | Self::CameraCropY | Self::CameraSplitRatio
            | Self::CameraCutoutSoftness | Self::CameraCutoutInset)
    }
    const BASE: usize = 100;
    const ALL: [SceneSlider; 33] = [
        SceneSlider::Scale,
        SceneSlider::PositionX,
        SceneSlider::PositionY,
        SceneSlider::RotationX,
        SceneSlider::RotationY,
        SceneSlider::RotationZ,
        SceneSlider::Perspective,
        SceneSlider::AnchorX,
        SceneSlider::AnchorY,
        SceneSlider::Blur,
        SceneSlider::Noise,
        SceneSlider::Vignette,
        SceneSlider::PointerScale,
        SceneSlider::WatermarkSize,
        SceneSlider::WatermarkOpacity,
        SceneSlider::DefaultZoom,
        SceneSlider::AnnotationTransition,
        SceneSlider::CameraSize,
        SceneSlider::CameraMargin,
        SceneSlider::SubtitlePosition,
        SceneSlider::SubtitleSize,
        SceneSlider::CursorIdleDelay,
        SceneSlider::CursorRotation,
        SceneSlider::CursorLoopDuration,
        SceneSlider::CameraZoomedSize,
        SceneSlider::CameraCornerRadius,
        SceneSlider::CameraCropZoom,
        SceneSlider::CameraCropX,
        SceneSlider::CameraCropY,
        SceneSlider::CameraSplitRatio,
        SceneSlider::CameraCutoutSoftness,
        SceneSlider::CameraCutoutInset,
        SceneSlider::MotionBlurAmount,
    ];

    pub(crate) fn id(self) -> usize {
        Self::BASE + Self::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }

    pub(crate) fn from_id(id: usize) -> Option<Self> {
        id.checked_sub(Self::BASE)
            .and_then(|index| Self::ALL.get(index).copied())
    }

    fn label(self) -> &'static str {
        match self {
            SceneSlider::MotionBlurAmount => "Motion blur",
            SceneSlider::Scale => "Scale",
            SceneSlider::PositionX => "Position X",
            SceneSlider::PositionY => "Position Y",
            SceneSlider::RotationX => "Rotate X",
            SceneSlider::RotationY => "Rotate Y",
            SceneSlider::RotationZ => "Rotation",
            SceneSlider::Perspective => "Perspective",
            SceneSlider::AnchorX => "Anchor X",
            SceneSlider::AnchorY => "Anchor Y",
            SceneSlider::Blur => "Blur",
            SceneSlider::Noise => "Noise",
            SceneSlider::Vignette => "Vignette",
            SceneSlider::PointerScale => "Cursor size",
            SceneSlider::WatermarkSize => "Size",
            SceneSlider::WatermarkOpacity => "Opacity",
            SceneSlider::DefaultZoom => "Default zoom",
            SceneSlider::AnnotationTransition => "Transition",
            SceneSlider::CameraSize => "Size",
            SceneSlider::CameraMargin => "Margin",
            SceneSlider::SubtitlePosition => "Height",
            SceneSlider::SubtitleSize => "Text size",
            SceneSlider::CursorIdleDelay => "Hide after",
            SceneSlider::CursorRotation => "Rotation strength",
            SceneSlider::CursorLoopDuration => "Return duration",
            SceneSlider::CameraZoomedSize => "Size when zoomed",
            SceneSlider::CameraCornerRadius => "Roundness",
            SceneSlider::CameraCropZoom => "Crop zoom",
            SceneSlider::CameraCropX => "Crop horizontal",
            SceneSlider::CameraCropY => "Crop vertical",
            SceneSlider::CameraSplitRatio => "Screen share",
            SceneSlider::CameraCutoutSoftness => "Edge softness",
            SceneSlider::CameraCutoutInset => "Edge inset",

        }
    }

    fn range(self) -> (f64, f64) {
        match self {
            SceneSlider::MotionBlurAmount => (0.0, 100.0),
            SceneSlider::CursorIdleDelay => (0.5, 5.0),
            SceneSlider::CursorRotation => (0.0, 100.0),
            SceneSlider::CursorLoopDuration => (0.2, 3.0),
            SceneSlider::CameraZoomedSize => (10.0, 60.0),
            SceneSlider::CameraCornerRadius => (0.0, 50.0),
            SceneSlider::CameraCropZoom => (100.0, 300.0),
            SceneSlider::CameraCropX => (0.0, 100.0),
            SceneSlider::CameraCropY => (0.0, 100.0),
            SceneSlider::CameraSplitRatio => (25.0, 75.0),
            SceneSlider::CameraCutoutSoftness => (0.0, 100.0),
            SceneSlider::CameraCutoutInset => (-30.0, 30.0),
            SceneSlider::Scale => (SceneTransform::MIN_SCALE, SceneTransform::MAX_SCALE),
            SceneSlider::PositionX | SceneSlider::PositionY =>
                (-SceneTransform::MAX_POSITION, SceneTransform::MAX_POSITION),
            SceneSlider::RotationX | SceneSlider::RotationY => (-60.0, 60.0),
            SceneSlider::RotationZ => (-180.0, 180.0),
            SceneSlider::Perspective | SceneSlider::AnchorX | SceneSlider::AnchorY => (0.0, 1.0),
            SceneSlider::Blur | SceneSlider::Noise | SceneSlider::Vignette => (0.0, 100.0),
            SceneSlider::PointerScale => (50.0, 250.0),
            SceneSlider::WatermarkSize | SceneSlider::WatermarkOpacity => (0.0, 100.0),
            SceneSlider::DefaultZoom => (1.0, 4.0),
            SceneSlider::AnnotationTransition => (0.0, 2.0),
            SceneSlider::CameraSize => (10.0, 60.0),
            SceneSlider::CameraMargin => (0.0, 20.0),
            SceneSlider::SubtitlePosition => {
                let (low, high) = crate::recording::studio_overlays::SubtitleStyle::VERTICAL_RANGE;
                (low * 100.0, high * 100.0)
            }
            SceneSlider::SubtitleSize => {
                let (low, high) = crate::recording::studio_overlays::SubtitleStyle::FONT_SCALE_RANGE;
                (low * 100.0, high * 100.0)
            }
        }
    }

    fn default_value(self) -> f64 {
        match self {
            SceneSlider::CursorIdleDelay => 1.5,
            SceneSlider::CursorRotation => 50.0,
            SceneSlider::CursorLoopDuration => 1.0,
            SceneSlider::CameraZoomedSize => 18.0,
            SceneSlider::CameraCornerRadius => 18.0,
            SceneSlider::CameraCropZoom => 100.0,
            SceneSlider::CameraCropX => 50.0,
            SceneSlider::CameraCropY => 50.0,
            SceneSlider::CameraSplitRatio => 60.0,
            SceneSlider::CameraCutoutSoftness => 20.0,
            SceneSlider::Scale => 1.0,
            SceneSlider::Perspective => SceneTransform::IDENTITY.perspective,
            SceneSlider::AnchorX | SceneSlider::AnchorY => 0.5,
            SceneSlider::PointerScale => 100.0,
            SceneSlider::WatermarkSize => 30.0,
            SceneSlider::WatermarkOpacity => 70.0,
            SceneSlider::DefaultZoom => 2.0,
            SceneSlider::AnnotationTransition => 0.35,
            SceneSlider::CameraSize => 24.0,
            SceneSlider::CameraMargin => 4.0,
            SceneSlider::SubtitlePosition => 92.0,
            SceneSlider::SubtitleSize => 100.0,
            _ => 0.0,
        }
    }

    fn format(self, value: f64) -> String {
        match self {
            SceneSlider::Scale | SceneSlider::DefaultZoom => format!("{value:.2}×"),
            SceneSlider::PositionX | SceneSlider::PositionY => format!("{:.0}%", value * 100.0),
            SceneSlider::RotationX | SceneSlider::RotationY | SceneSlider::RotationZ => {
                format!("{value:.0}°")
            }
            SceneSlider::Perspective | SceneSlider::AnchorX | SceneSlider::AnchorY => {
                format!("{:.0}%", value * 100.0)
            }
            SceneSlider::CursorIdleDelay | SceneSlider::CursorLoopDuration | SceneSlider::AnnotationTransition => format!("{value:.2} s"),
            _ => format!("{value:.0}%"),
        }
    }

    fn step(self) -> f64 {
        match self {
            SceneSlider::Scale | SceneSlider::DefaultZoom => 0.05,
            SceneSlider::PositionX | SceneSlider::PositionY => 0.02,
            SceneSlider::RotationX | SceneSlider::RotationY | SceneSlider::RotationZ => 1.0,
            SceneSlider::Perspective | SceneSlider::AnchorX | SceneSlider::AnchorY => 0.05,
            SceneSlider::CursorIdleDelay | SceneSlider::CursorLoopDuration | SceneSlider::AnnotationTransition => 0.05,
            SceneSlider::CameraMargin => 1.0,
            _ => 2.0,
        }
    }

    fn get(self, studio: &Studio) -> f64 {
        let transform = studio.scene_transform;
        match self {
            SceneSlider::MotionBlurAmount => studio.animation_settings.motion_blur as f64,
            SceneSlider::CursorIdleDelay => studio.pointer_style.idle_hide_delay,
            SceneSlider::CursorRotation => studio.pointer_style.rotation_strength * 100.0,
            SceneSlider::CursorLoopDuration => studio.pointer_style.loop_return_duration,
            SceneSlider::CameraZoomedSize => studio.camera_edit_overlay().zoomed_size.unwrap_or(studio.camera_edit_overlay().size) as f64,
            SceneSlider::CameraCornerRadius => studio.camera_edit_overlay().corner_radius.unwrap_or(18) as f64,
            SceneSlider::CameraCropZoom => studio.camera_edit_overlay().crop_zoom as f64,
            SceneSlider::CameraCropX => studio.camera_edit_overlay().crop_center.0 * 100.0,
            SceneSlider::CameraCropY => studio.camera_edit_overlay().crop_center.1 * 100.0,
            SceneSlider::CameraSplitRatio => studio.camera_edit_overlay().split_ratio as f64,
            SceneSlider::CameraCutoutSoftness => studio.camera_edit_overlay().cutout_softness as f64,
            SceneSlider::CameraCutoutInset => studio.camera_edit_overlay().cutout_inset as f64,
            SceneSlider::Scale => transform.scale,
            SceneSlider::PositionX => transform.position_x,
            SceneSlider::PositionY => transform.position_y,
            SceneSlider::RotationX => transform.rotation_x,
            SceneSlider::RotationY => transform.rotation_y,
            SceneSlider::RotationZ => transform.rotation_z,
            SceneSlider::Perspective => transform.perspective,
            SceneSlider::AnchorX => transform.anchor_x,
            SceneSlider::AnchorY => transform.anchor_y,
            SceneSlider::Blur => studio.background_blur as f64,
            SceneSlider::Noise => studio.background_noise as f64,
            SceneSlider::Vignette => studio.vignette as f64,
            SceneSlider::PointerScale => studio.pointer_style.scale as f64,
            SceneSlider::WatermarkSize => studio.watermark.size as f64,
            SceneSlider::WatermarkOpacity => studio.watermark.opacity as f64,
            SceneSlider::DefaultZoom => studio.default_motion_zoom,
            SceneSlider::AnnotationTransition => studio
                .selected_annotation
                .and_then(|index| studio.annotations.get(index))
                .and_then(|mark| mark.timing)
                .map(|timing| timing.transition)
                .unwrap_or(0.35),
            SceneSlider::CameraSize => studio.camera_edit_overlay().size as f64,
            SceneSlider::CameraMargin => studio.camera_edit_overlay().margin as f64,
            SceneSlider::SubtitlePosition => studio.subtitle_style.vertical_position * 100.0,
            SceneSlider::SubtitleSize => studio.subtitle_style.font_scale * 100.0,
        }
    }

    fn set(self, studio: &mut Studio, value: f64) {
        let (min, max) = self.range();
        let value = value.clamp(min, max);
        match self {
            SceneSlider::MotionBlurAmount => {
                studio.animation_settings.motion_blur = value.round() as u8;
                studio.export_motion_blur = false;
            }
            SceneSlider::CursorIdleDelay => studio.pointer_style.idle_hide_delay = value,
            SceneSlider::CursorRotation => studio.pointer_style.rotation_strength = value / 100.0,
            SceneSlider::CursorLoopDuration => studio.pointer_style.loop_return_duration = value,
            SceneSlider::CameraZoomedSize => studio.edit_camera_overlay(|camera| camera.zoomed_size = Some(value.round() as u8)),
            SceneSlider::CameraCornerRadius => studio.edit_camera_overlay(|camera| camera.corner_radius = Some(value.round() as u8)),
            SceneSlider::CameraCropZoom => studio.edit_camera_overlay(|camera| camera.crop_zoom = value.round() as u16),
            SceneSlider::CameraCropX => studio.edit_camera_overlay(|camera| camera.crop_center.0 = value / 100.0),
            SceneSlider::CameraCropY => studio.edit_camera_overlay(|camera| camera.crop_center.1 = value / 100.0),
            SceneSlider::CameraSplitRatio => studio.edit_camera_overlay(|camera| camera.split_ratio = value.round() as u8),
            SceneSlider::CameraCutoutSoftness => studio.edit_camera_overlay(|camera| camera.cutout_softness = value.round() as u8),
            SceneSlider::CameraCutoutInset => studio.edit_camera_overlay(|camera| camera.cutout_inset = value.round() as i8),
            SceneSlider::Scale => studio.scene_transform.scale = value,
            SceneSlider::PositionX => studio.scene_transform.position_x = value,
            SceneSlider::PositionY => studio.scene_transform.position_y = value,
            SceneSlider::RotationX => studio.scene_transform.rotation_x = value,
            SceneSlider::RotationY => studio.scene_transform.rotation_y = value,
            SceneSlider::RotationZ => studio.scene_transform.rotation_z = value,
            SceneSlider::Perspective => studio.scene_transform.perspective = value,
            SceneSlider::AnchorX => studio.scene_transform.anchor_x = value,
            SceneSlider::AnchorY => studio.scene_transform.anchor_y = value,
            SceneSlider::Blur => studio.background_blur = value.round() as u8,
            SceneSlider::Noise => studio.background_noise = value.round() as u8,
            SceneSlider::Vignette => studio.vignette = value.round() as u8,
            SceneSlider::PointerScale => studio.pointer_style.scale = value.round() as u8,
            SceneSlider::WatermarkSize => studio.watermark.size = value.round() as u8,
            SceneSlider::WatermarkOpacity => studio.watermark.opacity = value.round() as u8,
            SceneSlider::DefaultZoom => studio.default_motion_zoom = value,
            SceneSlider::CameraSize => studio.edit_camera_overlay(|camera| camera.size = value.round() as u8),
            SceneSlider::CameraMargin => studio.edit_camera_overlay(|camera| camera.margin = value.round() as u8),
            SceneSlider::SubtitlePosition => studio.subtitle_style.vertical_position = value / 100.0,
            SceneSlider::SubtitleSize => studio.subtitle_style.font_scale = value / 100.0,
            SceneSlider::AnnotationTransition => {
                let duration = studio.video_duration;
                if let Some(mark) = studio
                    .selected_annotation
                    .and_then(|index| studio.annotations.get_mut(index))
                {
                    if let Some(timing) = mark.timing.as_mut() {
                        timing.transition = value;
                        *timing = timing.clamped(duration);
                        studio.annotation_edit_preview_pending = true;
                    }
                }
            }
        }
        if matches!(self, Self::CursorIdleDelay | Self::CursorRotation | Self::CursorLoopDuration) {
            // Rebuilding plans the whole recording; mid-drag it waits for release.
            if studio.slider_drag.is_some() {
                studio.motion_timelines_stale = true;
            } else {
                studio.rebuild_video_motion_timelines();
            }
        }
    }
}

/// Composited preview state cached between renders.
pub(crate) struct PreviewCache {
    pub frame: Option<(PreviewKey, Arc<RenderImage>)>,
    pub overlay: Option<(u64, Arc<RgbaImage>)>,
    worker: Option<PreviewWorker>,
    pub(crate) display_scale: f32,
    polling: bool,
    /// Counts continuous edits; see `PreviewKey::gesture`.
    gesture: u64,
    editing: bool,
}

impl Default for PreviewCache {
    fn default() -> Self {
        Self {
            frame: None,
            overlay: None,
            worker: None,
            display_scale: 1.0,
            polling: false,
            gesture: 0,
            editing: false,
        }
    }
}

pub(crate) const CLICK_COLORS: [u32; 6] =
    [0x007aff, 0xff3b30, 0xff9500, 0x34c759, 0xaf52de, 0xffffff];

#[cfg(test)]
mod motion_picking_tests {
    use super::*;

    #[test]
    fn annotation_preview_texture_is_bounded_without_upscaling() {
        assert_eq!(annotation_preview_size((3840, 2160), (960, 540)), (1440, 810));
        assert_eq!(annotation_preview_size((3840, 2160), (480, 270)), (720, 405));
        assert_eq!(annotation_preview_size((800, 600), (960, 540)), (800, 600));
        assert_eq!(annotation_preview_size((2160, 3840), (540, 960)), (810, 1440));
    }

    #[test]
    #[ignore = "requires an isolated X11 process, FFmpeg and graphics driver; Xvfb supported"]
    fn focus_drag_reuses_preview_and_corner_scaling_skips_expensive_effects() {
        use crate::*;
        let directory = std::env::temp_dir().join(format!("lahza-focus-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        std::env::set_var("LAHZA_CONFIG_DIR", directory.join("settings"));
        std::env::set_var("LAHZA_RECORDINGS_DIR", directory.join("recordings"));
        std::env::set_var("XDG_CONFIG_HOME", directory.join("xdg-config"));
        let session = RecordingSession { directory: directory.clone() };
        assert!(std::process::Command::new("ffmpeg").args([
            "-v", "error", "-f", "lavfi", "-i", "testsrc2=size=320x180:rate=10",
            "-t", "5", "-c:v", "ffv1", "-y",
        ]).arg(session.screen_path()).status().unwrap().success());
        session.write_manifest(&crate::recording::model::CaptureManifest {
            duration: 5.0, pixel_width: 320, pixel_height: 180, ..Default::default()
        }).unwrap();
        let project = directory.clone();
        Application::new().with_assets(Assets { base: asset_directory() }).run(move |cx| {
            let handle = open_studio_window(cx, true, |handle, cx| {
                cx.new(|cx| Studio::new(handle, None, None, cx))
            }).unwrap();
            handle.update(cx, |studio, _, cx| {
                studio.open_video_project(project).unwrap();
                studio.video_position = 2.0;
                let mut cue = ZoomCue::pinned(0.0, 5.0, 2.0, NormalizedPoint { x: 0.5, y: 0.5 });
                cue.skips_easing = true;
                studio.video_selected_zoom_cue = Some(cue.id);
                studio.video_zoom_cues = vec![cue];
                studio.rebuild_video_motion_timelines();
                studio.persist_video_zoom_cues_quiet();
                let bounds = Bounds::new(point(px(0.), px(0.)), size(px(480.), px(270.)));
                for pick in [crate::motion_ui::MotionPick::Focus, crate::motion_ui::MotionPick::PanEnd] {
                    studio.video_selected_zoom_cue = Some(studio.video_zoom_cues[0].id);
                    studio.motion_pick = pick;
                    let original = studio.video_zoom_cues.clone();
                    let undo_count = studio.video_undo_stack.len();
                    let frame = studio.scene_preview_image_ready(px(480.), px(270.)).unwrap();
                    let viewport = studio.video_viewport_timeline.frame_at(2.0);
                    let (_, projection) = studio.preview_projection(480., 270.);
                    let (x, y) = projection.project(0.5, 0.8);
                    assert!(studio.scene_pointer_down(point(px(x as f32), px(y as f32)), bounds,
                        &Modifiers::default(), 1, cx));
                    assert_eq!(studio.focus_drag.as_ref().unwrap().pick, pick);
                    let started = std::time::Instant::now();
                    for sample in 0..60 {
                        studio.drag_motion_marker(point(px(x as f32 + sample as f32), px(y as f32 + 10.)), bounds, cx);
                        let next = studio.scene_preview_image_ready(px(480.), px(270.)).unwrap();
                        assert!(Arc::ptr_eq(&frame, &next), "focus samples must reuse the parked composition");
                        assert_eq!(studio.video_viewport_timeline.frame_at(2.0), viewport);
                        assert_eq!(studio.video_undo_stack.len(), undo_count);
                    }
                    eprintln!("60 {pick:?} samples with cached preview: {:?}", started.elapsed());
                    assert_eq!(studio.video_project.as_ref().unwrap().effective_zoom_cues().unwrap(), Some(original.clone()),
                        "dragging must not autosave each pointer sample");
                    assert!(studio.end_focus_drag());
                    assert!(!studio.end_focus_drag(), "release handlers must commit only once");
                    let edited = studio.video_zoom_cues.clone();
                    assert_ne!(edited, original);
                    assert_eq!(studio.video_undo_stack.len(), undo_count + 1);
                    assert_eq!(studio.video_project.as_ref().unwrap().effective_zoom_cues().unwrap(), Some(edited.clone()));
                    studio.undo_video_edit(cx);
                    assert_eq!(studio.video_zoom_cues, original);
                    studio.redo_video_edit(cx);
                    assert_eq!(studio.video_zoom_cues, edited);
                }

                studio.video_selected_zoom_cue = None;
                studio.scene_selection = SceneSelection::Media;
                for native_window in [false, true] {
                    studio.video_window_capture = native_window;
                    for corner in 0..4 {
                        let (_, projection) = studio.preview_projection(480., 270.);
                        let (x, y) = projection.quad[corner];
                        assert!(studio.scene_pointer_down(point(px(x as f32), px(y as f32)), bounds,
                            &Modifiers::default(), 1, cx));
                        assert_eq!(studio.media_drag.unwrap().kind, MediaDragKind::Scale);
                        studio.update_media_drag(point(px(x as f32 + 5.), px(y as f32 + 5.)));
                        studio.scene_preview_image_ready(px(480.), px(270.)).unwrap();
                        let key = &studio.preview_cache.frame.as_ref().unwrap().0;
                        assert!(!key.high_quality);
                        let motion = key.motion.as_ref().unwrap();
                        assert_eq!(motion.samples.len(), 1, "corner scaling must render only the current frame");
                        assert_eq!(motion.samples[0].time, studio.timeline_display_time());
                        assert!(studio.end_media_drag());
                        studio.scene_preview_image_ready(px(480.), px(270.)).unwrap();
                        let key = &studio.preview_cache.frame.as_ref().unwrap().0;
                        assert!(key.high_quality, "release must restore full quality");
                        assert!(key.motion.is_some());
                    }
                }
            }).unwrap();
            cx.quit();
        });
        crate::scene_preview::shutdown_workers();
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    #[ignore = "requires a display (Xvfb) and FFmpeg"]
    fn video_annotations_edit_at_the_chosen_frame_and_animate_when_scrubbing() {
        use crate::*;
        let directory = std::env::temp_dir().join(format!("lahza-annotation-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let session = RecordingSession { directory: directory.clone() };
        assert!(std::process::Command::new("ffmpeg").args([
            "-v", "error", "-f", "lavfi", "-i", "testsrc2=size=800x450:rate=10",
            "-t", "5", "-c:v", "libx264", "-preset", "ultrafast", "-y",
        ]).arg(session.screen_path()).status().unwrap().success());
        session.write_manifest(&crate::recording::model::CaptureManifest::default()).unwrap();
        let project = directory.clone();
        Application::new().with_assets(Assets { base: asset_directory() }).run(move |cx| {
            let handle = open_studio_window(cx, true, |handle, cx| {
                cx.new(|cx| Studio::new(handle, None, None, cx))
            }).unwrap();
            handle.update(cx, |studio, _, cx| {
                studio.open_video_project(project).unwrap();
                let bounds = Bounds::new(point(px(0.), px(0.)), size(px(800.), px(450.)));
                studio.video_position = 1.0;
                studio.video_playing = true;
                studio.tool = Tool::Pen;
                studio.pointer_down(point(px(100.), px(100.)), bounds, &[], 1);
                studio.pointer_move(point(px(200.), px(150.)), bounds);
                assert!(!studio.video_playing, "drawing pauses the transport");
                let draft = studio.annotation_draft.clone().unwrap();
                assert_eq!(timed::editor_mark(&draft, 1.0, studio.annotation_is_live(0)), Some(draft));
                let frame = studio.scene_preview_image_ready(px(480.), px(270.)).unwrap();
                let tick = std::time::Instant::now();
                for n in 0..30 {
                    studio.pointer_move(point(px(210. + n as f32), px(160.)), bounds);
                    let next = studio.scene_preview_image_ready(px(480.), px(270.)).unwrap();
                    assert!(Arc::ptr_eq(&frame, &next), "flat pen samples must not recompose the video");
                }
                eprintln!("30 video pen samples with cached preview: {:?}", tick.elapsed());
                let (_, layer) = studio.preview_overlay(1.0, (480, 270)).unwrap();
                assert!(layer.pixels().any(|p| p[3] > 0), "the projected path must include live drafts too");
                studio.pointer_up(point(px(240.), px(160.)), bounds);
                assert_eq!(studio.video_undo_stack.len(), 1);
                studio.update_annotation_edit_preview();
                assert_eq!(studio.video_position, 1.0, "finishing a stroke must not seek");
                assert!(studio.annotation_is_live(0), "the completed stroke stays visible for editing");
                let generation = studio.video_playback_generation.load(std::sync::atomic::Ordering::SeqCst);
                studio.update_annotation_edit_preview();
                assert_eq!(studio.video_playback_generation.load(std::sync::atomic::Ordering::SeqCst), generation);
                let first_timing = studio.annotations[0].timing;
                studio.pointer_down(point(px(300.), px(100.)), bounds, &[], 1);
                studio.pointer_move(point(px(400.), px(160.)), bounds);
                studio.pointer_up(point(px(400.), px(160.)), bounds);
                studio.update_annotation_edit_preview();
                assert_eq!(studio.annotations[1].timing, first_timing, "successive strokes stay together in time");
                assert_eq!(studio.video_position, 1.0);
                assert_eq!(studio.video_playback_generation.load(std::sync::atomic::Ordering::SeqCst), generation,
                    "the next stroke must not cancel the pending frame decode");
                assert!(studio.annotation_is_live(0) && studio.annotation_is_live(1),
                    "all annotations under the playhead stay complete while authoring");
                let mut future = studio.annotations[0].clone();
                future.timing = Some(AnnotationTiming { start: 3., end: 4., ..Default::default() });
                studio.annotations.push(future);
                assert!(!studio.annotation_is_live(2), "editing must not reveal future annotations");
                studio.annotations.pop();
                studio.video_playing = true;
                assert!(!studio.annotation_is_live(0), "playback always uses real animation");
                studio.video_playing = false;
                studio.seek_video(1.1, cx);
                assert!(!studio.annotation_is_live(0) && !studio.annotation_is_live(1));
                assert_eq!(timed::editor_mark(&studio.annotations[0], 1.1, studio.annotation_is_live(0)),
                    timed::animated_mark(&studio.annotations[0], 1.1));
                let generation = studio.video_playback_generation.load(std::sync::atomic::Ordering::SeqCst);
                studio.select_annotation_for_timing(0);
                studio.update_annotation_edit_preview();
                assert_eq!(studio.video_position, 1.1, "selecting for editing must not seek");
                assert!(studio.annotation_is_live(0) && studio.annotation_is_live(1));
                assert_eq!(studio.video_playback_generation.load(std::sync::atomic::Ordering::SeqCst), generation);

                studio.seek_video(studio.video_duration, cx);
                studio.tool = Tool::Text;
                studio.canvas_annotation_drag = true;
                studio.pointer_down(point(px(300.), px(220.)), bounds, &[], 1);
                studio.pointer_up(point(px(300.), px(220.)), bounds);
                studio.canvas_annotation_drag = false;
                studio.update_annotation_edit_preview();
                assert_eq!(studio.editing_text, Some(2), "editing preview preserves the empty text editor");
                let timing = studio.annotations[2].timing.unwrap();
                assert!(timing.end <= studio.video_duration);
                assert_eq!(studio.video_position, studio.video_duration);
                assert!(studio.annotation_is_live(2), "last-frame text stays editable without rewinding");
                studio.annotations[2].text = "Video caption".into();
                studio.seek_video(timing.start, cx);
                studio.selected_annotation = Some(2);
                studio.annotation_selection = vec![2];
                assert!(!studio.annotation_is_live(2));
                assert!(studio.canvas_annotation_marks().iter().all(|(_, mark)| mark.text.is_empty()),
                    "selected text must still type in at the actual scrubbed playhead");
                studio.select_annotation_for_timing(2);
                studio.update_annotation_edit_preview();
                assert_eq!(studio.canvas_annotation_marks()[0].1.text, "Video caption");

                studio.finish_annotation_interaction();
                studio.video_position = 2.0;
                studio.scene_transform.rotation_y = 10.0;
                studio.video_source_size = (3840, 2160);
                studio.tool = Tool::Arrow;
                studio.pointer_down(point(px(100.), px(100.)), bounds, &[], 1);
                studio.pointer_move(point(px(300.), px(180.)), bounds);
                assert_eq!(studio.video_position, 2.0, "drawing an arrow holds the target frame");
                studio.scene_preview_image_ready(px(480.), px(270.)).unwrap();
                assert_eq!(studio.preview_cache.frame.as_ref().unwrap().0.canvas, (240, 135));
                assert_eq!(studio.preview_cache.overlay.as_ref().unwrap().1.dimensions(), (360, 203));
                studio.pointer_up(point(px(300.), px(180.)), bounds);
                studio.update_annotation_edit_preview();
                assert_eq!(studio.video_position, 2.0, "finishing an arrow holds the target frame");
                studio.scene_preview_image_ready(px(480.), px(270.)).unwrap();
                assert_eq!(studio.preview_cache.frame.as_ref().unwrap().0.canvas, (480, 270));
            }).unwrap();
            cx.spawn(async |cx| {
                Timer::after(Duration::from_millis(500)).await;
                cx.update(|cx| cx.quit()).unwrap();
            }).detach();
        });
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn paused_preview_cache_tracks_motion_edits() {
        let key = PreviewKey {
            style: SceneStyle::default(),
            canvas: (960, 540), presentation_canvas:(960,540),
            source_size: (1920, 1080),
            source: 1,
            time_bits: 5.0_f64.to_bits(),
            viewport: ViewportFrame::default(),
            overlay: 0,
            pointer: false,
            media_visible: true,
            generation: 0,
            media: PreviewMedia::Image(1),
            high_quality: true,
            motion: None,
            gesture: 0,
        };
        let mut edited = key.clone();
        edited.viewport.magnification = 2.0;
        assert!(edited != key, "zoom edits must repaint at a paused playhead");
        let zoomed = edited.clone();
        edited.viewport.anchor = NormalizedPoint { x: 0.3, y: 0.7 };
        assert!(edited != zoomed, "focus and pan edits must repaint");
        let pinned = edited.clone();
        edited.viewport.tilt.x = 12.0;
        assert!(edited != pinned, "tilt edits must repaint");
        let tilted = edited.clone();
        edited.viewport.transform.scale = 1.3;
        assert!(edited != tilted, "card transform edits must repaint");
    }

    #[test]
    fn picks_resolve_visible_source_points_through_zoom_and_projection() {
        let style = SceneStyle::default();
        let geometry = SceneGeometry::layout(960.0, 540.0, 1920.0, 1080.0, &style);
        let mut transform = SceneTransform::IDENTITY;
        transform.scale = 0.65;
        transform.position_x = 0.5;
        transform.rotation_y = 15.0;
        transform.rotation_z = 8.0;
        let projection = geometry.projection(transform);
        let source = NormalizedPoint { x: 0.4, y: 0.6 };
        for (magnification, crop) in [1.0, 1.4, 2.0, 3.0].into_iter().flat_map(|zoom| {
            [crate::CropRect::UNIT, crate::CropRect { x: 0.25, y: 0.25, width: 0.5, height: 0.5 }]
                .map(move |crop| (zoom, crop))
        }) {
            let viewport = ViewportFrame {
                magnification,
                anchor: NormalizedPoint { x: 0.45, y: 0.55 },
                ..ViewportFrame::default()
            };
            let (left, top, visible_x, visible_y) = crop.visible_rect(viewport);
            let (x, y) = projection.project(
                (source.x - left) / visible_x,
                (source.y - top) / visible_y,
            );
            for pick in [crate::motion_ui::MotionPick::Focus, crate::motion_ui::MotionPick::PanEnd] {
                let drag = FocusDrag { pick, projection, viewport, crop, original_cues: Vec::new() };
                let target = drag.target_at(x, y);
                assert!((target.x - source.x).abs() < 1e-9);
                assert!((target.y - source.y).abs() < 1e-9);
                // A repeated mouse move must keep the same source target,
                // even after the edited cue has moved the live viewport.
                assert_eq!(drag.target_at(x, y), target);
            }
        }
    }
}

fn handle_hit(quad: &[(f64, f64); 4], x: f64, y: f64) -> Option<usize> {
    quad.iter()
        .position(|(qx, qy)| (qx - x).abs() <= 9.0 && (qy - y).abs() <= 9.0)
}


/// Seconds a camera edit eases over: a resize/move of an on-screen
/// element, so ease-in-out (the quintic in `blend`), kept under 300 ms.
const CAMERA_HANDOFF_SECONDS: f64 = 0.28;
impl Studio {
    // ------------------------------------------------------------------
    // Scene state helpers
    // ------------------------------------------------------------------

    /// Applies a saved or loaded style to the inspector state.
    pub(crate) fn apply_scene_style(&mut self, style: &SceneStyle) {
        use crate::recording::scene::SceneBackground;
        match &style.background {
            SceneBackground::Solid(color) => {
                self.wallpaper_tab = 0;
                if let Some(index) = crate::SOLID_BACKGROUNDS
                    .iter()
                    .position(|(_, value)| value == color)
                {
                    self.color_index = index;
                }
            }
            SceneBackground::Gradient { colors, .. } => {
                self.wallpaper_tab = 1;
                if let Some(index) = crate::GRADIENT_BACKGROUNDS
                    .iter()
                    .position(|preset| preset.colors == *colors)
                {
                    self.gradient_index = index;
                }
            }
            SceneBackground::Wallpaper(path) => {
                self.wallpaper_tab = 2;
                let bundled = crate::asset_directory();
                match path.strip_prefix(&bundled) {
                    Ok(relative) => {
                        let relative = relative.to_string_lossy().to_string();
                        if let Some(asset) = crate::CURATED_WALLPAPERS
                            .iter()
                            .find(|asset| **asset == relative)
                        {
                            self.wallpaper_asset = asset;
                            self.custom_wallpaper = None;
                        } else {
                            self.custom_wallpaper = Some(path.clone());
                        }
                    }
                    Err(_) => self.custom_wallpaper = Some(path.clone()),
                }
            }
        }
        self.padding = style.padding;
        self.corners = style.corners;
        self.shadow = style.shadow;
        self.shadow_style = style.shadow_style;
        self.shadow_color = style.shadow_color & 0xffffff;
        self.border = style.border;
        self.border_thickness = style.border_thickness;
        if let Some(index) = crate::motion_ui::BORDER_COLORS
            .iter()
            .position(|color| *color == style.border_color)
        {
            self.border_color = index;
        }
        self.border_opacity = style.border_opacity;
        self.window_frame = style.window_frame;
        self.background_blur = style.background_blur;
        self.background_noise = style.background_noise;
        self.vignette = style.vignette;
        self.scene_transform = style.transform.clamped();
        self.watermark_enabled = style.watermark.is_some();
        if let Some(watermark) = style.watermark.clone() {
            self.watermark = watermark;
        }
        self.pointer_style = style.pointer;
        self.animation_settings = style.animation;
        self.camera_overlay = style.camera.without_hidden_layout();
        self.camera_regions = style.camera_regions.clone();
        for region in &mut self.camera_regions {
            region.overlay = region.overlay.without_hidden_layout();
        }
        self.input_overlays = style.input_overlays;
        self.content_fill = style.fill;
        self.enhance_screen = style.enhance_screen;
        self.restore_canvas_ratio(style.aspect);
        if let Some((width, height)) = style.aspect_dimensions.filter(|(w, h)|
            (1..=10000).contains(w) && (1..=10000).contains(h)
            && style.aspect.is_some_and(|ratio| (ratio - *w as f64 / *h as f64).abs() < 0.0001)) {
            self.aspect_ratio = 10;
            self.custom_canvas_dimensions = (width, height);
        }
        self.custom_canvas_draft = self.custom_canvas_dimensions;
        self.subtitle_style = style.subtitles;
    }

    pub(crate) fn restore_canvas_ratio(&mut self, aspect: Option<f64>) {
        const RATIOS: [f64; 9] = [1.0, 4.0 / 3.0, 3.0 / 2.0, 16.0 / 9.0,
            9.0 / 16.0, 4.0 / 5.0, 3.0 / 4.0, 2.0 / 3.0, 21.0 / 9.0];
        let Some(ratio) = aspect.filter(|r| r.is_finite() && *r > 0.0) else {
            self.aspect_ratio = 0;
            return;
        };
        if let Some(index) = RATIOS.iter().position(|r| (r - ratio).abs() < 0.00001) {
            self.aspect_ratio = index + 1;
        } else {
            self.aspect_ratio = 10;
            self.custom_canvas_dimensions = if ratio >= 1.0 {
                (10000, (10000.0 / ratio.clamp(0.05, 20.0)).round() as u32)
            } else {
                ((10000.0 * ratio.clamp(0.05, 20.0)).round() as u32, 10000)
            };
        }
    }

    /// Autosaves scene settings into the recording's edit draft when they
    /// changed since the last save (called from render, skipped mid-drag).
    pub(crate) fn autosave_scene_style(&mut self) {
        if self.slider_drag.is_some() || self.media_drag.is_some() || self.camera_drag.is_some() || self.camera_region_drag.is_some() {
            return;
        }
        let Some(session) = self.video_project.as_ref() else {
            return;
        };
        let style = self.scene_style();
        if self.persisted_scene_style.as_ref() != Some(&style) {
            if let Err(error) = session.write_edit_field("scene", &style) {
                self.toast = Some(crate::Notification::error(format!("Could not autosave scene settings: {error}")));
            }
            self.persisted_scene_style = Some(style);
        }
        let Some(session) = self.video_project.as_ref() else {
            return;
        };
        let extras = crate::RecordingExtras {
            audio_muted: self.video_audio_muted,
            noise_reduction: self.video_noise_reduction,
            audio_mode: Some(self.video_audio_mode),
            removed_press_times: self.video_removed_presses.clone(),
        };
        if self.persisted_extras.as_ref() != Some(&extras) {
            if let Err(error) = session.write_edit_field("lahzaExtras", &extras) {
                self.toast = Some(crate::Notification::error(format!("Could not autosave settings: {error}")));
            }
            self.persisted_extras = Some(extras);
        }
        // Annotations save once an interaction settles, not per drag frame.
        let interacting = self.pointer_is_down
            || self.annotation_draft.is_some()
            || self.annotation_drag.is_some()
            || self.editing_text.is_some();
        if !interacting && self.persisted_annotations.as_ref() != Some(&self.annotations) {
            if let Err(error) = session.write_edit_field("annotations", &self.annotations) {
                self.toast = Some(crate::Notification::error(format!("Could not autosave annotations: {error}")));
            }
            self.persisted_annotations = Some(self.annotations.clone());
        }
    }

    pub(crate) fn set_scene_slider(&mut self, id: usize, value: u8) -> bool {
        let Some(slider) = SceneSlider::from_id(id) else {
            return false;
        };
        let (min, max) = slider.range();
        slider.set(self, min + (max - min) * value as f64 / 100.0);
        true
    }

    pub(crate) fn scene_slider_row(
        &self,
        slider: SceneSlider,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let value = slider.get(self);
        let (min, max) = slider.range();
        let fraction = ((value - min) / (max - min)).clamp(0.0, 1.0);
        let row = crate::controls::SliderRow {
            slider_id: slider.id(),
            label: slider.label().into(),
            fraction: fraction as f32,
            value: slider.format(value).into(),
            can_decrease: value > min + 1e-9,
            can_increase: value < max - 1e-9,
            show_reset: (value - slider.default_value()).abs() >= 1e-6,
        };
        self.slider_control(
            row,
            move |this| {
                if slider.is_camera() { this.record_camera_edit(); }
                if slider == SceneSlider::MotionBlurAmount { this.record_animation_edit(); }
                true
            },
            move |this, steps| {
                if slider == SceneSlider::MotionBlurAmount { this.record_animation_edit(); }
                slider.set(this, slider.get(this) + steps as f64 * slider.step());
            },
            move |this| {
                if slider == SceneSlider::MotionBlurAmount { this.record_animation_edit(); }
                slider.set(this, slider.default_value());
            },
            cx,
        )
    }

    pub(crate) fn scene_toggle_row(
        &self,
        id: &'static str,
        label: &'static str,
        enabled: bool,
        cx: &mut Context<Self>,
        on_toggle: impl Fn(&mut Studio) + 'static,
    ) -> AnyElement {
        crate::controls::toggle_row(id, label, enabled)
            .on_click(cx.listener(move |this, _, _, cx| {
                on_toggle(this);
                cx.notify();
            }))
            .into_any_element()
    }

    // ------------------------------------------------------------------
    // Composited preview
    // ------------------------------------------------------------------

    /// The style the editor preview draws: the authored style plus any
    /// camera handoff in flight. Export and autosave keep `scene_style`.
    pub(crate) fn preview_scene_style(&self) -> SceneStyle {
        let mut style = self.scene_style();
        style.camera_handoff = self.camera_handoff.as_ref().map(|(from, start)| {
            crate::recording::camera_timeline::CameraHandoff {
                from: from.clone(),
                progress: (start.elapsed().as_secs_f64() / CAMERA_HANDOFF_SECONDS).min(1.0),
            }
        });
        style
    }

    /// Camera edits (layout, position, shape, undo) ease from the pose on
    /// screen instead of jumping. Direct manipulation follows the pointer;
    /// releasing a bubble drag still eases into the snapped spot.
    pub(crate) fn update_camera_handoff(&mut self, window: &mut Window) {
        // Hold the finished handoff until its final pose is on screen. Ending
        // earlier cancels the in-flight frame, so the preview would stall a
        // step short and then jump once the full-quality frame arrives.
        let final_shown = self.preview_cache.frame.as_ref()
            .and_then(|(key, _)| key.style.camera_handoff.as_ref())
            .is_some_and(|handoff| handoff.progress >= 1.0);
        if self.camera_handoff.as_ref().is_some_and(|(_, start)| {
            let elapsed = start.elapsed().as_secs_f64();
            elapsed >= CAMERA_HANDOFF_SECONDS && (final_shown || elapsed >= CAMERA_HANDOFF_SECONDS + 0.5)
        }) {
            self.camera_handoff = None;
        }
        if self.video_project.is_none() {
            self.camera_shown = None;
            return;
        }
        let mut shown = self.preview_scene_style();
        if let Some(previous) = self.camera_shown.take() {
            let edited = previous.camera != shown.camera || previous.camera_regions != shown.camera_regions;
            let direct = self.camera_drag.is_some() || self.slider_drag.is_some()
                || self.camera_region_drag.is_some();
            if edited && !direct && crate::theme::animations_enabled() {
                self.camera_handoff = Some((Box::new(previous), std::time::Instant::now()));
                shown = self.preview_scene_style();
            }
        }
        if self.camera_handoff.is_some() {
            window.request_animation_frame();
        }
        self.camera_shown = Some(shown);
    }

    /// The RGBA media the preview composites: the current video frame or
    /// the processed screenshot.
    fn preview_source(&self) -> Option<Arc<RgbaImage>> {
        if self.video_project.is_some() {
            self.video_frame_rgba.clone()
        } else {
            self.capture_rgba.clone()
        }
    }

    /// Flattened annotation layer for the current time when the media is
    /// transformed (GPUI cannot paint annotations through a 3D projection).
    fn preview_overlay(&mut self, time: f64, canvas: (u32, u32)) -> Option<(u64, Arc<RgbaImage>)> {
        if self.annotations.is_empty() && self.annotation_draft.is_none() {
            return None;
        }
        let source_size = self.media_dimensions()?;
        let (width, height) = source_size;
        // This is a preview texture, not an export. Rasterizing a 4K layer
        // for every pointer sample needlessly stalls the UI thread.
        let (width, height) = annotation_preview_size((width, height), canvas);
        let viewport = self.video_viewport_timeline.frame_at(time);
        let mut style = self.preview_scene_style();
        if self.video_project.is_some() {
            let source_time = self.video_clip_timeline.content_source_time_at(time).unwrap_or(time);
            style.camera = style.camera_at(source_time, canvas, self.camera_source_aspect(), viewport.magnification);
        }
        let geometry = SceneGeometry::layout(canvas.0 as f64, canvas.1 as f64,
            source_size.0 as f64, source_size.1 as f64, &style);
        let viewport = geometry.camera_viewport(viewport, &style, source_size);
        let crop = style.source_crop;
        let marks: Vec<_> = self.annotations.iter().chain(self.annotation_draft.iter())
            .enumerate().filter(|(_, mark)| !mark.is_canvas())
            .filter_map(|(index, mark)| {
                let mark = if self.scene_is_timed() {
                    timed::editor_mark(mark, time, self.annotation_is_live(index))?
                } else { mark.clone() };
                Some(timed::in_cropped_media_space(mark, viewport, crop))
            }).collect();
        let signature = timed::marks_signature(&marks) ^ ((width as u64) << 32 | height as u64);
        if let Some((cached, layer)) = self.preview_cache.overlay.as_ref() {
            if *cached == signature {
                return Some((signature, layer.clone()));
            }
        }
        let layer = Arc::new(render_annotation_layer(&marks, width, height)?);
        self.preview_cache.overlay = Some((signature, layer.clone()));
        Some((signature, layer))
    }

    /// Source-resolution composition runs off-thread, including paused enhancement.
    pub(crate) fn scene_preview_image(
        &mut self,
        canvas_width: Pixels,
        canvas_height: Pixels,
    ) -> Option<Arc<RenderImage>> {
        let source = self.preview_source()?;
        // While a slider or the media is being dragged, compose a half-size
        // proxy that the canvas scales up; the full-size frame is rendered
        // once the drag ends. Native window recordings retain a full-size
        // preview during scaling so text detail does not appear to disappear.
        let annotation_proxy = self.pointer_is_down && !self.annotations_paint_flat()
            && !self.canvas_annotation_drag;
        let dragging = self.slider_drag.is_some() || self.media_drag.is_some()
            || self.camera_drag.is_some() || self.motion_transform_drag.is_some();
        // A camera handoff composes at full size so its last frame matches
        // the settled one; a half-size proxy would pop sharper at the end.
        let editing = dragging || self.camera_handoff.is_some();
        if editing && !self.preview_cache.editing {
            self.preview_cache.gesture += 1;
        }
        self.preview_cache.editing = editing;
        let proxy = if annotation_proxy || (!(self.video_project.is_some() && self.video_window_capture)
            && dragging)
        {
            0.5
        } else {
            1.0
        };
        let mut canvas = playback_canvas_size((
            (f32::from(canvas_width) * proxy * self.preview_cache.display_scale).round().max(2.0) as u32,
            (f32::from(canvas_height) * proxy * self.preview_cache.display_scale).round().max(2.0) as u32,
        ), self.video_playing);
        let style = self.preview_scene_style();
        let source_size = (source.width(), source.height());
        let time = if self.video_project.is_some() || self.animation_active {
            self.timeline_display_time()
        } else {
            0.0
        };
        let viewport = if self.video_project.is_some() || self.animation_active {
            self.video_viewport_timeline.frame_at(time)
        } else {
            ViewportFrame::default()
        };
        let has_pointer = (self.video_project.is_some() && self.video_pointer_synthesized)
            || (self.animation_active && self.has_walkthrough());
        let camera = if self.video_project.is_some() && style.has_camera_content() {
            self.camera_frame_rgba.clone()
        } else {
            None
        };
        let high_quality = !self.video_playing && !editing && proxy == 1.0
            && self.timeline_skim_time().is_none() && self.video_seek_drag.is_none()
            && (self.video_project.is_none() || self.video_window_capture || source_size == self.video_source_size);
        let presentation_canvas=canvas;
        let make_motion=|canvas| {
            Arc::new(crate::recording::motion_blur::plan(&style,&self.video_viewport_timeline,
                has_pointer.then_some(&self.video_pointer_timeline),
                self.video_project.as_ref().map(|_|&self.video_motion_clip_timeline),time,self.video_duration,
                self.export_frame_rate,canvas,source_size,self.camera_source_aspect(),
                if editing {crate::recording::motion_blur::Quality::Interactive}
                else if high_quality {crate::recording::motion_blur::Quality::High}
                else {crate::recording::motion_blur::Quality::Playing}))
        };
        // Direct manipulation needs the current frame, not a multi-frame
        // shutter or neural enhancement on every pointer sample.
        let mut motion=(self.video_project.is_some() || self.animation_active).then(||make_motion(canvas));
        if self.video_playing {
            if let Some(plan)=&motion {
                let bounded=crate::scene_preview::temporal_playback_canvas_size(canvas,plan.samples.len());
                if bounded!=canvas {canvas=bounded;motion=Some(make_motion(canvas));}
            }
        }
        let overlay = if !self.annotations_paint_flat() || self.has_media_image_overlays() {
            self.preview_overlay(time, canvas)
        } else {
            None
        };
        let temporal_overlay = motion.as_ref().filter(|plan| plan.samples.len() > 1)
            .and_then(|_| {
                let marks:Vec<_> = self.annotations.iter().chain(self.annotation_draft.iter())
                    .enumerate().filter(|(_,mark)| !mark.is_canvas())
                    .map(|(index,mark)|(mark.clone(),self.annotation_is_live(index))).collect();
                if marks.is_empty() { return None; }
                let dimensions=self.media_dimensions()?;
                Some(crate::scene_preview::TemporalOverlay {marks,size:annotation_preview_size(dimensions,canvas)})
            });
        let authored_overlay_signature = temporal_overlay.as_ref().map_or(0, |overlay| {
            let marks:Vec<_>=overlay.marks.iter().map(|(mark,_)|mark.clone()).collect();
            timed::marks_signature(&marks)
        });
        let key = PreviewKey {
            style: style.clone(),
            canvas, presentation_canvas,
            source_size,
            source: Arc::as_ptr(&source) as usize
                ^ camera
                    .as_ref()
                    .map(|frame| (Arc::as_ptr(frame) as usize).rotate_left(17))
                    .unwrap_or(0),
            time_bits: time.to_bits(),
            viewport,
            overlay: overlay
                .as_ref()
                .map(|(signature, _)| *signature)
                .unwrap_or(0) ^ authored_overlay_signature,
            pointer: has_pointer,
            media_visible: self.image_visible_at(time),
            generation: self.video_playback_generation.load(std::sync::atomic::Ordering::SeqCst),
            media: self.video_project.as_ref().map(|session| PreviewMedia::Video(session.directory.clone()))
                .unwrap_or_else(|| PreviewMedia::Image(Arc::as_ptr(&source) as usize)),
            // Hover/scrub decodes are deliberately small. Never spend neural
            // work enlarging those proxies; release supplies original pixels.
            high_quality,
            motion,
            gesture: if editing { self.preview_cache.gesture } else { 0 },
        };
        if let Some((cached, image)) = self.preview_cache.frame.as_ref() {
            if *cached == key {
                return Some(image.clone());
            }
        }
        let pointer = has_pointer
            .then(|| self.video_pointer_timeline.frame_at(time))
            .flatten()
            .map(|frame| PointerOverlay { frame });
        if self.preview_cache.worker.is_none() {
            match PreviewWorker::new() {
                Ok(worker) => self.preview_cache.worker = Some(worker),
                Err(error) => {
                    self.toast = Some(crate::Notification::error(format!("Preview worker could not start: {error}")));
                    return self.preview_cache.frame.as_ref().map(|(_, image)| image.clone());
                }
            }
        }
        let worker = self.preview_cache.worker.as_ref().unwrap();
        worker.request(PreviewJob {
            playing: self.video_playing,
            key: key.clone(), source, temporal_overlay,
            overlay: overlay.map(|(_, layer)| layer), pointer, camera,
            screen_video:self.video_playback_path().map(|path|crate::recording::motion_blur::VideoSource {
                path,size:self.video_source_size,revision:self.video_extras_token}),
            camera_video:self.video_camera_path.clone().map(|path|crate::recording::motion_blur::VideoSource {
                path,size:if high_quality {(u32::MAX,u32::MAX)}else{(640,640)},revision:self.video_extras_token}),
            recording_duration:if self.video_project.is_some(){self.video_recording_duration}else{self.video_duration},
        });
        if let Some(completed) = worker.take_completed(&key) {
            let previous = self.preview_cache.frame.replace((completed.key, completed.image));
            self.retire_image(previous.map(|(_, image)| image));
        }
        if let Some(message) = self.preview_cache.worker.as_ref().unwrap().take_failure(&key) {
            self.toast = Some(crate::Notification::error(message));
        }
        self.preview_cache.frame.as_ref()
            .filter(|(shown, _)| shown.same_media(&key))
            .map(|(_, image)| image.clone())
    }

    /// Only poll while a bounded preview job is active; a paused frame gets
    /// its own completion notification without rendering on the UI thread.
    pub(crate) fn watch_scene_preview(&mut self, cx: &mut Context<Self>) {
        if self.preview_cache.polling || !self.preview_cache.worker.as_ref().is_some_and(PreviewWorker::has_work) {
            return;
        }
        self.preview_cache.polling = true;
        cx.spawn(async move |weak, cx| loop {
            gpui::Timer::after(std::time::Duration::from_millis(16)).await;
            let keep = weak.update(cx, |this, cx| {
                let ready = this.preview_cache.worker.as_ref().is_some_and(PreviewWorker::has_completed);
                if ready { cx.notify(); }
                let pending = this.preview_cache.worker.as_ref().is_some_and(PreviewWorker::has_work);
                if !pending || ready {
                    this.preview_cache.polling = false;
                    false
                } else { true }
            }).unwrap_or(false);
            if !keep { break; }
        }).detach();
    }

    #[cfg(test)]
    pub(crate) fn scene_preview_image_ready(&mut self, width: Pixels, height: Pixels) -> Option<Arc<RenderImage>> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        loop {
            let frame = self.scene_preview_image(width, height);
            if self.preview_cache.frame.as_ref().is_some_and(|(key, _)| {
                self.preview_cache.worker.as_ref().is_some_and(|worker| worker.is_current(key))
            }) { return frame; }
            assert!(std::time::Instant::now() < deadline, "preview worker did not finish");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    /// Whether the screenshot preview must show the compositor's frame: the
    /// style needs it, an animated tilt is bending the media, or a cursor
    /// walkthrough has to be drawn.
    pub(crate) fn preview_needs_compositor(&self) -> bool {
        // GPUI cannot clip the zoomed still to a rounded rect, so animated
        // scenes always show the composited frame export produces.
        self.scene_style().needs_composited_preview() || self.animation_active
    }

    pub(crate) fn has_walkthrough(&self) -> bool {
        !self.animation_pointer_capture.presses.is_empty()
    }

    /// The synthetic cursor for an animated screenshot, if one was authored.
    pub(crate) fn animation_pointer_timeline(
        &self,
    ) -> Option<crate::recording::pointer_timeline::PointerTimeline> {
        if !self.animation_active || !self.has_walkthrough() {
            return None;
        }
        Some(self.video_pointer_timeline.clone())
    }

    /// Normalized media coordinates under a canvas position (through the
    /// projection and the current viewport crop).
    pub(crate) fn media_point_at(
        &self,
        position: Point<Pixels>,
        canvas: Bounds<Pixels>,
    ) -> Option<NormalizedPoint> {
        let (style, geometry, viewport) = self.preview_geometry_frame(f32::from(canvas.size.width), f32::from(canvas.size.height));
        let projection=geometry.projection(style.transform.with_motion(viewport));
        let local_x = f32::from(position.x - canvas.origin.x) as f64;
        let local_y = f32::from(position.y - canvas.origin.y) as f64;
        let (u, v) = projection.unproject(local_x, local_y);
        if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
            return None;
        }
        let (left, top, visible_x, visible_y) = style.source_crop.visible_rect(viewport);
        Some(
            NormalizedPoint {
                x: left + u * visible_x,
                y: top + v * visible_y,
            }
            .clamped(),
        )
    }

    /// Adds a cursor-walkthrough stop and regenerates the synthetic cursor
    /// and its click zooms.
    pub(crate) fn add_walkthrough_stop(&mut self, point: NormalizedPoint) {
        self.walkthrough_stops.push(point);
        self.rebuild_walkthrough();
    }

    pub(crate) fn rebuild_walkthrough(&mut self) {
        let duration = self.video_duration;
        self.animation_pointer_capture =
            crate::recording::viewport::walkthrough_capture(&self.walkthrough_stops, duration);
        let default_zoom = self.default_motion_zoom;
        let mut cues = crate::recording::viewport::synthesize_zoom_cues(
            &self.animation_pointer_capture,
            duration,
        );
        for cue in &mut cues {
            cue.zoom = default_zoom.clamp(1.0, 4.0);
        }
        if !cues.is_empty() || self.walkthrough_stops.is_empty() {
            self.video_zoom_cues = cues;
        }
        self.video_selected_zoom_cue = None;
        self.animation_preset = None;
        self.rebuild_video_motion_timelines();
    }

    pub(crate) fn clear_walkthrough(&mut self) {
        self.walkthrough_stops.clear();
        self.walkthrough_mode = false;
        self.animation_pointer_capture = Default::default();
        self.video_pointer_timeline = Default::default();
        self.rebuild_video_motion_timelines();
    }

    // ------------------------------------------------------------------
    // Camera overlay
    // ------------------------------------------------------------------

    /// Keeps the camera preview frame close to the playhead (decoded on a
    /// background thread; at most one decode in flight).
    pub(crate) fn ensure_camera_frame(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.video_camera_path.clone() else {
            return;
        };
        if self.video_playing || !self.scene_style().has_camera_content() || self.camera_decode_in_flight {
            return;
        }
        let Some(source_time) = self.video_clip_timeline.content_source_time_at(self.timeline_display_time()) else {
            self.camera_frame_rgba = None;
            return;
        };
        if source_time >= self.video_recording_duration {
            self.camera_frame_rgba = None;
            return;
        }
        if self.camera_frame_rgba.is_some() && (self.camera_decoded_time - source_time).abs() < 1.0 / 30.0
        {
            return;
        }
        self.camera_decode_in_flight = true;
        self.camera_decode_token = self.camera_decode_token.wrapping_add(1);
        let token = self.camera_decode_token;
        let decode_path = path.clone();
        let task = cx.background_executor().spawn(async move {
            crate::recording::video::decode_frame(&decode_path, source_time, u32::MAX, u32::MAX)
                .ok()
                .and_then(|frame| RgbaImage::from_raw(frame.width, frame.height, frame.rgba))
        });
        cx.spawn(async move |weak, cx| {
            let frame = task.await;
            let _ = weak.update(cx, |this, cx| {
                if this.camera_decode_token != token {
                    return;
                }
                this.camera_decode_in_flight = false;
                // A seek may have moved again while FFmpeg was decoding.
                let current = this.video_clip_timeline.content_source_time_at(this.timeline_display_time());
                if this.video_playing || this.video_camera_path.as_ref() != Some(&path)
                    || current.is_none_or(|time| (time - source_time).abs() >= 1.0 / 30.0)
                {
                    cx.notify();
                    return;
                }
                if let Some(frame) = frame {
                    this.camera_frame_rgba = Some(Arc::new(frame));
                    this.camera_decoded_time = source_time;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    pub(crate) fn import_camera_clip(&mut self, cx: &mut Context<Self>) {
        self.pause_video_playback();
        let Some(session) = self.video_project.clone() else {
            return;
        };
        let prompt = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        cx.spawn(async move |weak, cx| {
            let selected = match prompt.await {
                Ok(Ok(Some(paths))) => paths.into_iter().next(),
                _ => None,
            };
            let Some(source) = selected else {
                return;
            };
            let destination = session.camera_path();
            let copy = cx
                .background_executor()
                .spawn(async move {
                    crate::recording::video::probe_media(&source)
                        .map_err(|error| error.to_string())?;
                    std::fs::copy(&source, &destination).map_err(|error| error.to_string())?;
                    Ok::<PathBuf, String>(destination)
                })
                .await;
            let _ = weak.update(cx, |this, cx| {
                match copy {
                    Ok(path) => {
                        this.video_camera_path = Some(path);
                        this.camera_frame_rgba = None;
                        this.camera_overlay.enabled = true;
                        if let Ok(mut manifest) = session.read_manifest() {
                            manifest.includes_camera = true;
                            let _ = session.write_manifest(&manifest);
                        }
                    }
                    Err(error) => {
                        this.toast = Some(crate::Notification::error(format!("Could not add camera clip: {error}")));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn remove_camera_clip(&mut self) {
        self.pause_video_playback();
        if let Some(path) = self.video_camera_path.take() {
            let _ = std::fs::remove_file(path);
        }
        self.camera_frame_rgba = None;
        self.camera_overlay.enabled = false;
        self.camera_decode_token = self.camera_decode_token.wrapping_add(1);
        self.camera_decode_in_flight = false;
        if let Some(session) = self.video_project.as_ref() {
            if let Ok(mut manifest) = session.read_manifest() {
                manifest.includes_camera = false;
                let _ = session.write_manifest(&manifest);
            }
        }
    }

    /// Whether fitting the recording whole leaves background showing beside
    /// it, as a landscape recording does on a 9:16 canvas. Screen Studio
    /// offers "Avoid empty area" (our Fill) only in that case.
    fn fit_leaves_empty_space(&self) -> bool {
        let mut style = self.scene_style();
        let Some(aspect) = style.aspect.filter(|aspect| aspect.is_finite() && *aspect > 0.0) else {
            return false;
        };
        if self.video_project.is_none() {
            return false;
        }
        let (width, height) = (1000.0 * aspect, 1000.0);
        let source = (self.video_source_size.0 as f64, self.video_source_size.1 as f64);
        style.fill = false;
        let fit = SceneGeometry::layout(width, height, source.0, source.1, &style).media;
        style.fill = true;
        let fill = SceneGeometry::layout(width, height, source.0, source.1, &style).media;
        fit.width < fill.width - 1.0 || fit.height < fill.height - 1.0
    }

    pub(crate) fn camera_style_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        use crate::recording::camera_layout::{CameraAspect, CameraLayout, CameraSplitDirection};
        const LAYOUTS: [CameraLayout; 4] = [CameraLayout::Overlay, CameraLayout::Split, CameraLayout::Cutout, CameraLayout::Fullscreen];
        let overlay = self.camera_edit_overlay();
        div().flex().flex_col().gap(px(SPACE_3))
            .child(field("Layout", self.segmented("camera-layout", &["Overlay", "Split screen", "Cutout", "Camera only"], LAYOUTS.iter().position(|v| *v == overlay.layout).unwrap_or(usize::MAX),
                |this, i| this.edit_camera_overlay(|camera| { camera.layout = LAYOUTS[i]; camera.enabled = true; }), cx)))
            .when(overlay.layout == CameraLayout::Split, |panel| panel
                .child(self.segmented("camera-split-direction", &["Auto", "Horizontal", "Vertical"], CameraSplitDirection::ALL.iter().position(|v| *v == overlay.split_direction).unwrap_or(0),
                    |this, i| this.edit_camera_overlay(|camera| camera.split_direction = CameraSplitDirection::ALL[i]), cx))
                .child(hint("Auto places the camera beside landscape video and below portrait video."))
                .child(self.scene_slider_row(SceneSlider::CameraSplitRatio, cx))
                .child(self.scene_toggle_row("camera-split-full", "Fill split cells", overlay.split_full, cx, |this| this.edit_camera_overlay(|camera| camera.split_full = !camera.split_full)))
                .child(self.scene_toggle_row("camera-split-flip", "Swap sides", overlay.split_flip, cx, |this| this.edit_camera_overlay(|camera| camera.split_flip = !camera.split_flip))))
            .when(matches!(overlay.layout, CameraLayout::Overlay | CameraLayout::Cutout), |panel| panel
                .child(div().flex().flex_col().gap(px(SPACE_1))
                    .child(field("Position", self.position_grid("camera-position", &["Top left", "Top right", "Bottom left", "Bottom right"], 2,
                        if overlay.center.is_some() { usize::MAX } else { WatermarkPosition::ALL.iter().position(|v| *v == overlay.position).unwrap_or(usize::MAX) },
                        |this, i| this.edit_camera_overlay(|camera| { camera.position = WatermarkPosition::ALL[i]; camera.center = None; }), cx)))
                    .child(hint("Drag on the canvas to snap to an edge or the center.")))
                .child(self.scene_slider_row(SceneSlider::CameraSize, cx))
                .child(self.scene_toggle_row("camera-zoom-resize", "Resize during zoom", overlay.zoomed_size.is_some(), cx, |this| {
                    this.edit_camera_overlay(|camera| camera.zoomed_size = if camera.zoomed_size.is_some() { None } else { Some((camera.size * 3 / 4).max(10)) });
                }))
                .when(overlay.zoomed_size.is_some(), |panel| panel.child(self.scene_slider_row(SceneSlider::CameraZoomedSize, cx)))
                .when(self.fit_leaves_empty_space(), |panel| panel
                    .child(self.scene_toggle_row("camera-avoid-empty", "Avoid empty area", self.content_fill, cx,
                        |this| this.content_fill = !this.content_fill))
                    .child(hint("Crops the recording to fill the canvas; the camera follows your cursor."))))
            .when(overlay.layout == CameraLayout::Overlay || (overlay.layout == CameraLayout::Split && !overlay.split_full), |panel| panel
                .child(Self::section_divider())
                .child(div().section_title().child("Shape & frame"))
                .child(self.segmented("camera-shape", &["Circle", "Rounded", "Rectangle"], crate::recording::scene::CameraShape::ALL.iter().position(|v| *v == overlay.shape).unwrap_or(0),
                    |this, i| this.edit_camera_overlay(|camera| camera.shape = crate::recording::scene::CameraShape::ALL[i]), cx))
                .when(overlay.shape != crate::recording::scene::CameraShape::Circle, |panel| panel
                    .child(self.option_grid("camera-aspect", &["Square", "Original", "16:9", "9:16", "4:3", "3:4"], 3, CameraAspect::ALL.iter().position(|v| *v == overlay.aspect).unwrap_or(0),
                        |this, i| this.edit_camera_overlay(|camera| camera.aspect = CameraAspect::ALL[i]), cx))
                    .when(overlay.shape == crate::recording::scene::CameraShape::Squircle, |panel| panel.child(self.scene_slider_row(SceneSlider::CameraCornerRadius, cx))))
                .child(self.scene_slider_row(SceneSlider::CameraMargin, cx))
                .child(self.scene_toggle_row("camera-shadow", "Shadow", overlay.shadow, cx, |this| this.edit_camera_overlay(|camera| camera.shadow = !camera.shadow))))
            .when(overlay.layout == CameraLayout::Cutout, |panel| panel
                .child(hint("Remove the webcam background. Edge controls refine hair and clothing."))
                .child(self.scene_slider_row(SceneSlider::CameraCutoutSoftness, cx))
                .child(self.scene_slider_row(SceneSlider::CameraCutoutInset, cx))
                .child(self.scene_slider_row(SceneSlider::CameraMargin, cx)))
            .when(overlay.layout != CameraLayout::Hidden, |panel| panel
            .child(Self::section_divider())
            .child(div().section_title().child("Framing"))
            .child(self.scene_slider_row(SceneSlider::CameraCropZoom, cx))
            .child(self.scene_slider_row(SceneSlider::CameraCropX, cx))
            .child(self.scene_slider_row(SceneSlider::CameraCropY, cx))
            .child(self.scene_toggle_row("camera-mirror", "Mirror", overlay.mirror, cx, |this| this.edit_camera_overlay(|camera| camera.mirror = !camera.mirror)))
            .child(self.small_button("camera-reset-crop", "Reset framing", true, cx, |this, _| {
                this.edit_camera_overlay(|camera| { camera.crop_center = (0.5, 0.5); camera.crop_zoom = 100; });
            })))
            .into_any_element()
    }

    pub(crate) fn camera_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let has_camera = self.video_camera_path.is_some();
        div().flex().flex_col().gap_3()
            .when(!has_camera, |panel| panel
                .child(div().text_sm().text_color(muted()).child("Add a webcam clip to frame your camera alongside the recording."))
                .child(self.small_button("camera-add", "Add camera clip…", self.export_progress.is_none(), cx, |this, cx| this.import_camera_clip(cx))))
            .when(has_camera, |panel| panel
                .child(self.camera_regions_section(cx))
                .child(self.scene_toggle_row("camera-enabled", "Show camera", self.camera_edit_overlay().enabled, cx, |this| this.edit_camera_overlay(|camera| camera.enabled = !camera.enabled)))
                .child(self.camera_style_controls(cx))
                .child(self.small_button("camera-remove", "Remove camera clip", self.export_progress.is_none(), cx, |this, _| this.remove_camera_clip())))
            .into_any_element()
    }

    /// Author at a fixed frame with complete marks. Timing and animation are
    /// restored immediately when the user scrubs or plays, even with a selection.
    pub(crate) fn annotation_is_live(&self, index: usize) -> bool {
        if self.video_playing || self.video_seek_drag.is_some() || self.timeline_skim_time().is_some()
            || (self.timeline_display_time() - self.video_position).abs() > 1e-6 { return false; }
        let editing = self.pointer_is_down || self.editing_text == Some(index)
            || self.annotation_editing_time.is_some_and(|time| (time - self.video_position).abs() < 1e-6);
        if !editing { return false; }
        self.annotations.get(index).map_or(self.pointer_is_down, |mark| {
            mark.timing.is_none_or(|timing| (timing.start..=timing.end).contains(&self.video_position))
        })
    }

    pub(crate) fn update_annotation_edit_preview(&mut self) {
        if self.pointer_is_down || self.annotation_drag.is_some() || self.slider_drag.is_some() {
            return;
        }
        if !std::mem::take(&mut self.annotation_edit_preview_pending) || !self.scene_is_timed() {
            return;
        }
        // Editing changes only the annotation preview, never the video frame.
        self.annotation_editing_time = (!self.video_playing && self.video_seek_drag.is_none()
            && self.selected_annotation.is_some()).then_some(self.video_position);
    }

    pub(crate) fn canvas_annotation_marks(&self) -> Vec<(usize, AnnotationMark)> {
        self.annotations
            .iter()
            .enumerate()
            .filter(|(_, mark)| mark.is_canvas())
            .filter_map(|(index, mark)| {
                let mark = if self.scene_is_timed() {
                    timed::editor_mark(
                        mark,
                        self.timeline_display_time(),
                        self.annotation_is_live(index),
                    )?
                } else {
                    mark.clone()
                };
                Some((index, mark))
            })
            .collect()
    }

    pub(crate) fn canvas_annotation_pointer_down(
        &mut self,
        position: Point<Pixels>,
        canvas: Bounds<Pixels>,
        hits: &[(usize, Bounds<Pixels>)],
        media: Bounds<Pixels>,
        rendered: &[Bounds<Pixels>],
        click_count: usize,
    ) -> bool {
        // Retained paint listeners must not reclassify an active gesture.
        if self.pointer_is_down {
            return self.canvas_annotation_drag;
        }
        if self.scene_annotation_select(position, canvas, media, rendered, hits, click_count) {
            return true;
        }
        let create = self.scene_is_timed() && self.tool == Tool::Text;
        let hit =
            self.tool == Tool::Select && hits.iter().any(|(_, bounds)| Bounds::from_corners(point(bounds.left()-px(10.),bounds.top()-px(10.)),point(bounds.right()+px(10.),bounds.bottom()+px(10.))).contains(&position));
        if !canvas.contains(&position) || (!create && !hit) {
            self.canvas_annotation_drag = false;
            return false;
        }
        if create
            && self.animation_active
            && self.video_position + AnnotationTiming::DEFAULT_DURATION > self.video_duration
        {
            self.set_animation_duration(self.video_position + AnnotationTiming::DEFAULT_DURATION);
        }
        self.canvas_annotation_drag = true;
        let mut bounds = vec![
            Bounds {
                origin: point(px(-100000.0), px(-100000.0)),
                size: size(px(0.0), px(0.0))
            };
            self.annotations.len()
        ];
        for (index, hit) in hits {
            if let Some(value) = bounds.get_mut(*index) {
                *value = *hit;
            }
        }
        self.pointer_down(position, canvas, &bounds, click_count);
        self.video_selected_zoom_cue = None;
        self.video_selected_clip = None;
        self.scene_selection = SceneSelection::Scene;
        true
    }

    /// GPUI owns flat media marks only when no temporal composition is needed.
    /// A blur-enabled scene keeps one owner even on stationary single-sample
    /// frames, so marks cannot be duplicated or disappear as movement starts.
    pub(crate) fn annotations_paint_flat(&self) -> bool {
        let viewport = if self.scene_is_timed() {
            self.video_viewport_timeline.frame_at(self.timeline_display_time())
        } else {
            ViewportFrame::default()
        };
        !(self.scene_is_timed() && self.animation_settings.motion_blur>0)
            && viewport.loupe.is_none() && self.scene_transform.with_motion(viewport).is_identity()
    }

    fn has_media_image_overlays(&self) -> bool {
        self.annotations.iter().any(|mark| mark.tool == Tool::Image && !mark.is_canvas())
    }

    /// Maps a pointer position on a composited preview back to the position
    /// it would have on the flat, zoomed media rect `flat`, so annotation
    /// tools work through a 3D projection.
    pub(crate) fn flat_pointer_position(
        &self,
        position: Point<Pixels>,
        canvas: Bounds<Pixels>,
        flat: Bounds<Pixels>,
    ) -> Point<Pixels> {
        let (style, geometry, viewport) = self.preview_geometry_frame(f32::from(canvas.size.width), f32::from(canvas.size.height));
        let projection=geometry.projection(style.transform.with_motion(viewport));
        let local_x = f32::from(position.x - canvas.origin.x) as f64;
        let local_y = f32::from(position.y - canvas.origin.y) as f64;
        let (u, v) = projection.unproject(local_x, local_y);
        let (left, top, visible_x, visible_y) = style.source_crop.visible_rect(viewport);
        let media_x = left + u * visible_x;
        let media_y = top + v * visible_y;
        point(
            flat.origin.x + flat.size.width * media_x as f32,
            flat.origin.y + flat.size.height * media_y as f32,
        )
    }

    /// Time-resolved geometry and crop shared by drawing, picking and handles.
    pub(crate) fn preview_geometry_frame(&self, canvas_width:f32, canvas_height:f32) -> (SceneStyle,SceneGeometry,ViewportFrame) {
        let mut style=self.preview_scene_style();
        let source=if self.video_project.is_some(){self.video_source_size}else{self.captured_dimensions.unwrap_or((1200,720))};
        let time=self.timeline_display_time();
        let viewport=if self.video_project.is_some()||self.animation_active {self.video_viewport_timeline.frame_at(time)}else{ViewportFrame::default()};
        if self.video_project.is_some() {
            let source_time=self.video_clip_timeline.content_source_time_at(time).unwrap_or(time);
            style.camera=style.camera_at(source_time,(canvas_width.round() as u32,canvas_height.round() as u32),self.camera_source_aspect(),viewport.magnification);
        }
        let geometry=SceneGeometry::layout(canvas_width as f64,canvas_height as f64,source.0 as f64,source.1 as f64,&style);
        let viewport=geometry.camera_viewport(viewport,&style,source);
        (style,geometry,viewport)
    }

    /// Geometry and projection of the media on a preview canvas of this size.
    pub(crate) fn preview_projection(&self, canvas_width:f32, canvas_height:f32) -> (SceneGeometry,MediaProjection) {
        let (style,geometry,viewport)=self.preview_geometry_frame(canvas_width,canvas_height);
        (geometry,geometry.projection(style.transform.with_motion(viewport)))
    }

    // ------------------------------------------------------------------
    // Direct manipulation
    // ------------------------------------------------------------------

    /// Mouse-down on the scene canvas. Returns true when handled.
    pub(crate) fn scene_pointer_down(
        &mut self,
        position: Point<Pixels>,
        canvas: Bounds<Pixels>,
        modifiers: &Modifiers,
        click_count: usize,
        cx: &mut Context<Self>,
    ) -> bool {
        let local_x = f32::from(position.x - canvas.origin.x);
        let local_y = f32::from(position.y - canvas.origin.y);
        let canvas_size = (f32::from(canvas.size.width), f32::from(canvas.size.height));
        let (style,geometry,viewport)=self.preview_geometry_frame(canvas_size.0,canvas_size.1);
        let projection=geometry.projection(style.transform.with_motion(viewport));
        let inside = projection.contains(local_x as f64, local_y as f64);
        // A selected motion region turns clicks into focus picks; pressing
        // on a marker drags that marker instead of the default pick.
        if self.video_selected_zoom_cue.is_some() && inside {
            if self.selected_region().is_some_and(|cue| !cue.has_camera_motion()) {
                return true;
            }
            if self.video_playing || self.video_audio_play_when_ready {
                self.pause_video_playback();
            }
            let pick = self
                .motion_markers(viewport)
                .into_iter()
                .find(|(_, mu, mv)| {
                    let (mx, my) = projection.project(*mu, *mv);
                    (mx - local_x as f64).hypot(my - local_y as f64) <= MOTION_MARKER_HIT
                })
                .map(|(pick, _, _)| pick)
                .unwrap_or(self.motion_pick);
            let drag = FocusDrag {
                pick,
                projection,
                viewport,
                crop: style.source_crop,
                original_cues: self.video_zoom_cues.clone(),
            };
            let target = drag.target_at(local_x as f64, local_y as f64);
            self.focus_drag = Some(drag);
            self.pin_focus_at_media(target, pick, cx);
            return true;
        }
        if self.scene_selection == SceneSelection::Media {
            if let Some(corner) = handle_hit(&projection.quad, local_x as f64, local_y as f64) {
                let opposite = projection.quad[(corner + 2) % 4];
                self.media_drag = Some(MediaDrag {
                    kind: MediaDragKind::Scale,
                    start: position,
                    original: self.scene_transform,
                    canvas_size,
                    pivot: (opposite.0 as f32, opposite.1 as f32),
                });
                return true;
            }
        }
        if inside {
            if click_count >= 2 {
                self.scene_transform = SceneTransform::IDENTITY;
                self.media_drag = None;
                cx.notify();
                return true;
            }
            self.scene_selection = SceneSelection::Media;
            self.video_selected_clip = None;
            let kind = if modifiers.shift {
                MediaDragKind::Rotate
            } else if modifiers.control || modifiers.alt || modifiers.platform {
                MediaDragKind::Spin
            } else {
                MediaDragKind::Move
            };
            self.media_drag = Some(MediaDrag {
                kind,
                start: position,
                original: self.scene_transform,
                canvas_size,
                pivot: (0.0, 0.0),
            });
            cx.notify();
            return true;
        }
        if self.scene_selection == SceneSelection::Media {
            self.scene_selection = SceneSelection::Scene;
            cx.notify();
        }
        false
    }

    pub(crate) fn update_media_drag(&mut self, position: Point<Pixels>) -> bool {
        let Some(drag) = self.media_drag else {
            return false;
        };
        let dx = f32::from(position.x - drag.start.x);
        let dy = f32::from(position.y - drag.start.y);
        let mut transform = drag.original;
        match drag.kind {
            MediaDragKind::Move => {
                transform.position_x += (dx / (drag.canvas_size.0 * 0.5).max(1.0)) as f64;
                transform.position_y += (dy / (drag.canvas_size.1 * 0.5).max(1.0)) as f64;
            }
            MediaDragKind::Rotate => {
                transform.rotation_y += (dx * 0.4) as f64;
                transform.rotation_x -= (dy * 0.4) as f64;
            }
            MediaDragKind::Spin => {
                transform.rotation_z += (dx * 0.4) as f64;
            }
            MediaDragKind::Scale => {
                let start = (
                    f32::from(drag.start.x) - drag.pivot.0,
                    f32::from(drag.start.y) - drag.pivot.1,
                );
                let current = (start.0 + dx, start.1 + dy);
                let start_distance = (start.0 * start.0 + start.1 * start.1).sqrt().max(1.0);
                let current_distance = (current.0 * current.0 + current.1 * current.1).sqrt();
                transform.scale = drag.original.scale * (current_distance / start_distance) as f64;
            }
        }
        self.scene_transform = transform.clamped();
        true
    }

    pub(crate) fn end_media_drag(&mut self) -> bool {
        self.media_drag.take().is_some()
    }

    /// Whether the camera bubble is drawn at the playhead.
    pub(crate) fn camera_bubble_visible(&self) -> bool {
        self.video_project.is_some()
            && self.video_camera_path.is_some()
            && self.camera_at_playhead((1920, 1080)).opacity() > 0.0
            && self.video_clip_timeline
                .content_source_time_at(self.video_position)
                .is_some_and(|time| time < self.video_recording_duration)
    }

    pub(crate) fn camera_source_aspect(&self) -> f64 {
        self.camera_frame_rgba.as_ref()
            .filter(|frame| frame.height() > 0)
            .map(|frame| frame.width() as f64 / frame.height() as f64)
            .unwrap_or(1.0)
    }

    pub(crate) fn camera_at_playhead(&self, canvas: (u32, u32)) -> CameraOverlay {
        let time = self.timeline_display_time();
        let source_time = self.video_clip_timeline.content_source_time_at(time).unwrap_or(time);
        self.preview_scene_style().camera_at(source_time, canvas, self.camera_source_aspect(),
            self.video_viewport_timeline.frame_at(time).magnification)
    }

    /// Starts dragging the camera bubble when the press lands on it.
    pub(crate) fn camera_pointer_down(&mut self, position: Point<Pixels>, canvas: Bounds<Pixels>) -> bool {
        if !self.camera_bubble_visible() { return false; }
        let (width, height) = (f32::from(canvas.size.width) as f64, f32::from(canvas.size.height) as f64);
        let source_aspect = self.camera_source_aspect();
        let zoom = self.video_viewport_timeline.frame_at(self.timeline_display_time()).magnification;
        let effective = self.camera_at_playhead((width.round() as u32, height.round() as u32));
        let rect = effective.rect_for_frame(width, height, source_aspect, zoom);
        let local = (f32::from(position.x - canvas.origin.x) as f64, f32::from(position.y - canvas.origin.y) as f64);
        if !rect.contains(local.0, local.1) { return false; }
        self.pause_video_playback();
        let source_time = self.video_clip_timeline.content_source_time_at(self.timeline_display_time());
        self.selected_camera_region = source_time.and_then(|time| self.camera_regions.iter()
            .filter(|region| region.valid() && time >= region.start && time < region.end)
            .max_by(|a,b| a.start.total_cmp(&b.start)).map(|region| region.id));
        self.video_selected_zoom_cue = None;
        self.video_selected_clip = None;
        self.select_inspector_tab(crate::shell_ui::InspectorTab::Camera);
        if !matches!(self.camera_edit_overlay().layout, CameraLayout::Overlay | CameraLayout::Cutout) { return true; }
        self.record_camera_edit();
        self.camera_drag = Some(CameraDrag { start: position,
            start_center: (rect.x + rect.width * 0.5, rect.y + rect.height * 0.5),
            canvas_size: (width, height), source_aspect, zoom, guides: (None, None) });
        true
    }

    pub(crate) fn update_camera_drag(&mut self, position: Point<Pixels>) -> bool {
        let Some(drag) = self.camera_drag else { return false; };
        let proposed = (drag.start_center.0 + f32::from(position.x - drag.start.x) as f64,
            drag.start_center.1 + f32::from(position.y - drag.start.y) as f64);
        let (center, guides) = self.camera_edit_overlay().snapped_center_for_frame(proposed,
            drag.canvas_size.0, drag.canvas_size.1, drag.source_aspect, drag.zoom);
        self.camera_drag.as_mut().unwrap().guides = guides;
        self.edit_camera_overlay(|camera| camera.center = Some(center));
        true
    }

    pub(crate) fn end_camera_drag(&mut self) -> bool { self.camera_drag.take().is_some() }

    pub(crate) fn scene_scroll(&mut self, event: &ScrollWheelEvent) -> bool {
        if self.scene_selection != SceneSelection::Media {
            return false;
        }
        let delta = match event.delta {
            ScrollDelta::Pixels(delta) => f32::from(delta.y),
            ScrollDelta::Lines(delta) => delta.y * 16.0,
        };
        if delta.abs() < 0.01 {
            return false;
        }
        self.scene_transform.scale = (self.scene_transform.scale * 2f64.powf(delta as f64 / 400.0))
            .clamp(SceneTransform::MIN_SCALE, SceneTransform::MAX_SCALE);
        true
    }

    /// The selected region's focus and pan-end markers as (pick, u, v) in
    /// the visible crop's coordinates, for painting and hit-testing.
    fn motion_markers(&self, frame: ViewportFrame) -> Vec<(crate::motion_ui::MotionPick, f64, f64)> {
        let Some(cue) = self
            .video_selected_zoom_cue
            .and_then(|id| self.video_zoom_cues.iter().find(|cue| cue.id == id))
        else {
            return Vec::new();
        };
        if !cue.has_camera_motion() { return Vec::new(); }
        let (left, top, visible_x, visible_y) = self.scene_style().source_crop.visible_rect(frame);
        let to_visible = |p: NormalizedPoint| ((p.x - left) / visible_x, (p.y - top) / visible_y);
        let focus = match cue.anchor_mode {
            crate::recording::viewport::ZoomAnchorMode::PinnedAnchor => cue.pinned_point,
            _ => if cue.presentation == crate::recording::viewport::ZoomPresentation::GlassLoupe {
                frame.loupe.map_or(frame.anchor, |lens| lens.center)
            } else { frame.anchor },
        };
        let (u, v) = to_visible(focus);
        let mut markers = vec![(crate::motion_ui::MotionPick::Focus, u, v)];
        if let Some(pan) = cue.pan_to {
            let (u, v) = to_visible(pan);
            markers.push((crate::motion_ui::MotionPick::PanEnd, u, v));
        }
        markers
    }

    /// Canvas-local marker positions (is_focus, point) to paint while a
    /// region is selected and playback is paused; empty otherwise.
    pub(crate) fn motion_marker_points(
        &self,
        projection: &MediaProjection,
        viewport: ViewportFrame,
    ) -> Vec<(bool, Point<Pixels>)> {
        if self.video_selected_zoom_cue.is_none() || self.video_playing {
            return Vec::new();
        }
        self.motion_markers(viewport)
            .into_iter()
            .filter(|(_, u, v)| (0.0..=1.0).contains(u) && (0.0..=1.0).contains(v))
            .map(|(pick, u, v)| {
                let (x, y) = projection.project(u, v);
                (
                    pick == crate::motion_ui::MotionPick::Focus,
                    point(px(x as f32), px(y as f32)),
                )
            })
            .collect()
    }

    /// Moves the marker being dragged to the pointer, clamped to the media.
    pub(crate) fn drag_motion_marker(
        &mut self,
        position: Point<Pixels>,
        canvas: Bounds<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let Some(drag) = self.focus_drag.as_ref() else {
            return;
        };
        let local_x = f32::from(position.x - canvas.origin.x);
        let local_y = f32::from(position.y - canvas.origin.y);
        self.pin_focus_at_media(
            drag.target_at(local_x as f64, local_y as f64),
            drag.pick,
            cx,
        );
    }

    /// Sets the selected region's focus (or pan end) from media coordinates.
    pub(crate) fn pin_focus_at_media(
        &mut self,
        target: NormalizedPoint,
        pick: crate::motion_ui::MotionPick,
        cx: &mut Context<Self>,
    ) {
        let mutate = |cue: &mut ZoomCue| match pick {
            crate::motion_ui::MotionPick::Focus => {
                cue.anchor_mode = crate::recording::viewport::ZoomAnchorMode::PinnedAnchor;
                cue.pinned_point = target;
            }
            crate::motion_ui::MotionPick::PanEnd => {
                cue.anchor_mode = crate::recording::viewport::ZoomAnchorMode::PinnedAnchor;
                cue.pan_to = Some(target);
            }
        };
        if self.focus_drag.is_some() {
            if self.video_edit_busy { return; }
            if let Some(cue) = self.video_zoom_cues.iter_mut()
                .find(|cue| Some(cue.id) == self.video_selected_zoom_cue)
            {
                // Paint the marker over the parked preview. Rebuilding the
                // camera here also rereads and resamples the entire recording.
                mutate(cue);
            }
        } else {
            self.mutate_selected_zoom_cue(cx, mutate);
        }
        cx.notify();
    }

    pub(crate) fn end_focus_drag(&mut self) -> bool {
        let Some(drag) = self.focus_drag.take() else { return false; };
        if drag.original_cues != self.video_zoom_cues {
            self.video_undo_stack.push(VideoEditSnapshot::Zoom(drag.original_cues));
            self.video_redo_stack.clear();
            self.rebuild_video_viewport_timeline();
            self.persist_video_zoom_cues_quiet();
        }
        true
    }

    /// The composited scene canvas with selection handles, used by the
    /// recording editor and by transformed screenshots.
    pub(crate) fn scene_canvas(
        &mut self,
        canvas_width: Pixels,
        canvas_height: Pixels,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let canvas_annotations = self.canvas_annotation_marks();
        let media_visible = self.image_visible_at(self.timeline_display_time());
        let image = self.scene_preview_image(canvas_width, canvas_height);
        self.watch_scene_preview(cx);
        let bounds_store = self.scene_canvas_bounds.clone();
        let media_bounds_store = self.video_media_bounds.clone();
        let (style_at_time,geometry,frame)=self.preview_geometry_frame(f32::from(canvas_width),f32::from(canvas_height));
        let projection=geometry.projection(style_at_time.transform.with_motion(frame));
        let show_handles = media_visible && self.scene_selection == SceneSelection::Media;
        let picking = self.video_selected_zoom_cue.is_some();
        let markers = if media_visible { self.motion_marker_points(&projection,frame) } else { Vec::new() };
        let entity = cx.entity();
        let time = self.timeline_display_time();
        // Annotations paint flat over the media rect; a transformed card
        // hands them to the compositor instead (see `scene_preview_image`).
        let paint_flat = self.annotations_paint_flat();
        let composite_flat = paint_flat && self.has_media_image_overlays();
        let select_tool = self.tool == Tool::Select;
        let camera_guides = self.camera_drag.map(|drag| drag.guides);
        let camera_at_time = self.camera_at_playhead((f32::from(canvas_width).round() as u32, f32::from(canvas_height).round() as u32));
        let camera_grab = (select_tool && self.camera_bubble_visible() && matches!(camera_at_time.layout, CameraLayout::Overlay | CameraLayout::Cutout))
            .then(|| (camera_at_time, self.camera_source_aspect(), self.video_viewport_timeline.frame_at(self.timeline_display_time()).magnification));
        let camera_dragging = self.camera_drag.is_some();
        let text_cursor = self.tool == Tool::Text || self.editing_text.is_some();
        let committed_count = self.annotations.len();
        let mut marks = self.annotations.clone();
        if let Some(draft) = self.annotation_draft.clone() {
            marks.push(draft);
        }
        let selected_annotations = self.annotation_selected_indices();
        let canvas_focus = self.editing_text.map(|_| self.text_fields.canvas.read(cx).focus.clone());
        let editing_text = self.editing_text;
        let mut painted = Vec::with_capacity(marks.len());
        let mut painted_indices = Vec::with_capacity(marks.len());
        for (index, mark) in marks.iter().enumerate() {
            if mark.is_canvas() || !media_visible { continue; }
            if let Some(animated) = timed::editor_mark(
                mark,
                time,
                self.annotation_is_live(index),
            ) {
                painted.push(animated);
                painted_indices.push(index);
            }
        }
        let caret_visible = self.caret_visible;
        let (view_left, view_top, visible_x, visible_y) = style_at_time.source_crop.visible_rect(frame);
        let view_zoom_x = (1.0 / visible_x) as f32;
        let view_zoom_y = (1.0 / visible_y) as f32;
        let media_rect = geometry.media;
        let overlay_shapes = self.preview_overlay_shapes(
            (f32::from(canvas_width) as f64, f32::from(canvas_height) as f64),
            geometry.card(),
        );
        div()
            .id("scene-canvas")
            .w(canvas_width)
            .h(canvas_height)
            .flex_none()
            .relative()
            .overflow_hidden()
            .rounded(px(10.0))
            .shadow_lg()
            .bg(rgb(0x111214))
            .when_some(image, |this, image| {
                // The compositor already fits the scene. Fill the fractional
                // layout bounds so integer raster rounding cannot add bars.
                this.child(
                    img(image)
                        .absolute()
                        .inset_0()
                        .size_full()
                        .object_fit(ObjectFit::Fill),
                )
            })
            .child(
                canvas(
                    // The hitbox lets occluding overlays (dialogs) shadow the
                    // raw mouse listeners registered in paint.
                    move |bounds, window, cx| {
                        if let Some(focus) = &canvas_focus {
                            window.set_focus_handle(focus, cx);
                        }
                        if let Ok(mut stored) = bounds_store.lock() {
                            *stored = Some(bounds);
                        }
                        let media = Bounds {
                            origin: point(
                                bounds.origin.x + px(media_rect.x as f32),
                                bounds.origin.y + px(media_rect.y as f32),
                            ),
                            size: size(px(media_rect.width as f32), px(media_rect.height as f32)),
                        };
                        if let Ok(mut stored) = media_bounds_store.lock() {
                            *stored = Some(media);
                        }
                        let hitbox = window.insert_hitbox(bounds, gpui::HitboxBehavior::Normal);
                        (bounds, media, hitbox)
                    },
                    move |_, (bounds, media, hitbox), window, cx| {
                        // The full media through the current viewport crop.
                        let interaction_bounds = Bounds {
                            origin: point(
                                media.origin.x - media.size.width * view_zoom_x * view_left as f32,
                                media.origin.y - media.size.height * view_zoom_y * view_top as f32,
                            ),
                            size: size(media.size.width * view_zoom_x, media.size.height * view_zoom_y),
                        };
                        let mut selection_outline = None;
                        let annotation_bounds = window.with_content_mask(
                            Some(ContentMask { bounds: media }),
                            |window| {
                                if !paint_flat {
                                    return Vec::new();
                                }
                                if !composite_flat { paint_highlights(&painted, interaction_bounds, window); }
                                let mut rendered = vec![Bounds::new(point(px(-100000.),px(-100000.)),size(px(0.),px(0.))); committed_count+1];
                                for (slot, mark) in painted.iter().enumerate() {
                                    let index = painted_indices[slot];
                                    let space = if mark.pinned { media } else { interaction_bounds };
                                    let rendered_bounds = if composite_flat {
                                        let bounds = lahza_annotations::canvas::annotation_snap_bounds(mark, space);
                                        Bounds::from_corners(bounds.min.screen(), bounds.max.screen())
                                    } else { paint_annotation(
                                        mark,
                                        if mark.pinned {
                                            media
                                        } else {
                                            interaction_bounds
                                        },
                                        index >= committed_count,
                                        editing_text == Some(index) && caret_visible,
                                        window,
                                        cx,
                                    ) };
                                    rendered[index] = rendered_bounds;
                                    if selected_annotations.len() == 1 && selected_annotations.contains(&index) && editing_text != Some(index) {
                                        selection_outline = Some((slot, rendered_bounds));
                                    }
                                }
                                rendered
                            },
                        );
                        let canvas_hits = paint_canvas_annotations(&canvas_annotations, &selected_annotations, editing_text, bounds, window, cx);
                        if !overlay_shapes.is_empty() {
                            window.with_content_mask(Some(ContentMask { bounds }), |window| {
                                crate::overlays_ui::paint_overlay_shapes(&overlay_shapes, bounds.origin, window, cx);
                            });
                        }
                        crate::text_fields_ui::paint_canvas_text(&entity, bounds, interaction_bounds, media, window, cx);
                        if let Some((slot, selected_bounds)) = selection_outline {
                            let mark = &painted[slot];
                            window.with_content_mask(Some(ContentMask { bounds }), |window| {
                                crate::annotations::paint_selection(mark, if mark.pinned { media } else { interaction_bounds }, selected_bounds, window);
                            });
                        }
                        let group_bounds = entity.read(cx).annotation_group_bounds(bounds, interaction_bounds, &annotation_bounds, &canvas_hits);
                        if let Some(group) = group_bounds {
                            window.with_content_mask(Some(ContentMask { bounds }), |window| {
                                crate::annotations::paint_selection_box(group, window);
                            });
                        }
                        entity.update(cx, |this, cx| this.paint_annotation_brush(window, cx));
                        let mut cursor_bounds = annotation_bounds.clone();
                        cursor_bounds.resize(committed_count + 1, Bounds::new(point(px(-100000.), px(-100000.)), size(px(0.), px(0.))));
                        for (index, hit) in &canvas_hits { cursor_bounds[*index] = *hit; }
                        let studio = entity.read(cx);
                        let mouse = window.mouse_position();
                        let over_camera = camera_grab.is_some_and(|(camera, source_aspect, zoom)| {
                            camera
                                .rect_for_frame(f32::from(bounds.size.width) as f64, f32::from(bounds.size.height) as f64, source_aspect, zoom)
                                .contains(f32::from(mouse.x - bounds.origin.x) as f64, f32::from(mouse.y - bounds.origin.y) as f64)
                        });
                        let cursor = if camera_dragging {
                            gpui::CursorStyle::ClosedHand
                        } else if studio.tool == Tool::Select && group_bounds.is_some_and(|group| group.contains(&mouse)) {
                            if studio.pointer_is_down { gpui::CursorStyle::ClosedHand } else { gpui::CursorStyle::OpenHand }
                        } else if over_camera {
                            gpui::CursorStyle::OpenHand
                        } else { studio.annotation_cursor(mouse, interaction_bounds, &cursor_bounds) };
                        if let Some((guide_x, guide_y)) = camera_guides {
                            let guide = crate::selection();
                            window.with_content_mask(Some(ContentMask { bounds }), |window| {
                                if let Some(x) = guide_x {
                                    window.paint_quad(gpui::fill(
                                        Bounds::new(point(bounds.origin.x + px(x as f32 - 0.5), bounds.origin.y), size(px(1.0), bounds.size.height)),
                                        guide,
                                    ));
                                }
                                if let Some(y) = guide_y {
                                    window.paint_quad(gpui::fill(
                                        Bounds::new(point(bounds.origin.x, bounds.origin.y + px(y as f32 - 0.5)), size(bounds.size.width, px(1.0))),
                                        guide,
                                    ));
                                }
                            });
                        }
                        window.set_cursor_style(cursor, &hitbox);
                        if show_handles {
                            paint_selection_handles(&projection, bounds, window);
                        }
                        if !markers.is_empty() {
                            window.with_content_mask(
                                Some(ContentMask { bounds }),
                                |window| {
                                    paint_motion_markers(&markers, bounds, window);
                                },
                            );
                        }
                        window.on_mouse_event({
                            let entity = entity.clone();
                            move |event: &MouseDownEvent, _, window, cx| {
                                if event.button != MouseButton::Left
                                    || !bounds.contains(&event.position)
                                    || !hitbox.is_hovered(window)
                                {
                                    return;
                                }
                                entity.update(cx, |this, cx| {
                                    if this.canvas_text_mouse_down(event, window, cx) {
                                        cx.notify();
                                        return;
                                    }
                                    this.focus_handle.focus(window);
                                    if this.canvas_annotation_pointer_down(event.position, bounds, &canvas_hits, interaction_bounds, &annotation_bounds, event.click_count) {
                                        cx.notify();
                                        return;
                                    }
                                    if select_tool && this.camera_pointer_down(event.position, bounds) {
                                        cx.notify();
                                        return;
                                    }
                                    if !media_visible { return; }
                                    let flat = this.flat_pointer_position(
                                        event.position,
                                        bounds,
                                        interaction_bounds,
                                    );
                                    if this.video_selected_zoom_cue.is_some() {
                                        // A selected motion region takes focus picks.
                                        this.scene_pointer_down(
                                            event.position,
                                            bounds,
                                            &event.modifiers,
                                            event.click_count,
                                            cx,
                                        );
                                    } else if !select_tool {
                                        if interaction_bounds.contains(&flat) {
                                            this.pointer_down(
                                                flat,
                                                interaction_bounds,
                                                &annotation_bounds,
                                                event.click_count,
                                            );
                                        }
                                    } else {
                                        if interaction_bounds.contains(&flat)
                                            && !this.annotations.is_empty()
                                        {
                                            this.pointer_down(
                                                flat,
                                                interaction_bounds,
                                                &annotation_bounds,
                                                event.click_count,
                                            );
                                        }
                                        if this.selected_annotation.is_some() {
                                            this.video_selected_zoom_cue = None;
                                            this.video_selected_press = None;
                                            this.scene_selection = SceneSelection::Scene;
                                        } else if !this.annotation_brushing() {
                                            this.toast = None;
                                            this.scene_pointer_down(
                                                event.position,
                                                bounds,
                                                &event.modifiers,
                                                event.click_count,
                                                cx,
                                            );
                                        }
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
                                    if this.canvas_text_mouse_move(event, cx) {
                                        return;
                                    }
                                    if this.canvas_annotation_drag {
                                        this.pointer_move(event.position, bounds);
                                        cx.notify();
                                        return;
                                    }
                                    if this.focus_drag.is_some() {
                                        this.drag_motion_marker(event.position, bounds, cx);
                                        return;
                                    }
                                    if this.media_drag.is_some() || !this.pointer_is_down {
                                        return;
                                    }
                                    let flat = this.flat_pointer_position(
                                        event.position,
                                        bounds,
                                        interaction_bounds,
                                    );
                                    this.pointer_move(flat, interaction_bounds);
                                    cx.notify();
                                });
                            }
                        });
                        window.on_mouse_event(move |event: &MouseUpEvent, _, _, cx| {
                            if event.button != MouseButton::Left {
                                return;
                            }
                            entity.update(cx, |this, cx| {
                                this.text_fields.canvas.update(cx, |field, _| field.canvas_release());
                                if this.canvas_annotation_drag {
                                    this.pointer_up(event.position, bounds);
                                    this.canvas_annotation_drag = false;
                                    cx.notify();
                                    return;
                                }
                                if this.end_focus_drag() { cx.notify(); }
                                if !this.pointer_is_down {
                                    return;
                                }
                                let flat = this.flat_pointer_position(
                                    event.position,
                                    bounds,
                                    interaction_bounds,
                                );
                                this.pointer_up(flat, interaction_bounds);
                                cx.notify();
                            });
                        });
                    },
                )
                .absolute()
                .size_full(),
            )
            .when(text_cursor, |this| this.cursor(CursorStyle::IBeam))
            .when(!select_tool && !text_cursor, |this| this.cursor_crosshair())
            .when(select_tool && (show_handles || picking), |this| {
                this.cursor(if picking {
                    CursorStyle::Crosshair
                } else {
                    CursorStyle::OpenHand
                })
            })
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _, cx| {
                if this.scene_scroll(event) {
                    cx.stop_propagation();
                    cx.notify();
                }
            }))
            .into_any_element()
    }

    // ------------------------------------------------------------------
    // Inspector panels
    // ------------------------------------------------------------------

    /// Transform panel for the selected media surface.
    pub(crate) fn transform_inspector(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.scene_selection != SceneSelection::Media {
            return None;
        }
        let transform = self.scene_transform;
        let is_identity = transform.is_identity();
        Some(
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
                            div().section_title().child("Image properties"),
                        )
                        .child(self.small_button(
                            "transform-done",
                            "Deselect",
                            true,
                            cx,
                            |this, _| this.scene_selection = SceneSelection::Scene,
                        )),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(self.small_button(
                            "transform-fit",
                            "Fit",
                            true,
                            cx,
                            |this, _| {
                                this.scene_transform.scale = 1.0;
                                this.scene_transform.position_x = 0.0;
                                this.scene_transform.position_y = 0.0;
                            },
                        ))
                        .child(self.small_button(
                            "transform-fill",
                            "Fill",
                            true,
                            cx,
                            |this, _| {
                                let (geometry, _) = this.preview_projection(1600.0, 900.0);
                                this.scene_transform.scale = geometry
                                    .fill_scale()
                                    .clamp(SceneTransform::MIN_SCALE, SceneTransform::MAX_SCALE);
                                this.scene_transform.position_x = 0.0;
                                this.scene_transform.position_y = 0.0;
                            },
                        ))
                        .child(self.small_button(
                            "transform-actual",
                            "Actual size",
                            true,
                            cx,
                            |this, _| {
                                let (geometry, _) = this.preview_projection(1600.0, 900.0);
                                let (source_width, source_height) = if this.video_project.is_some()
                                {
                                    this.video_source_size
                                } else {
                                    this.captured_dimensions.unwrap_or((1200, 720))
                                };
                                let export_height = this
                                    .export_resolution
                                    .canvas_height(source_height);
                                let (export_width, _) = this.scene_style().export_canvas_size(
                                    source_width,
                                    source_height,
                                    export_height,
                                );
                                this.scene_transform.scale = geometry
                                    .actual_size_scale(source_width as f64, export_width as f64);
                            },
                        ))
                        .child(self.small_button(
                            "transform-reset",
                            "Reset all",
                            !is_identity,
                            cx,
                            |this, _| this.scene_transform = SceneTransform::IDENTITY,
                        )),
                )
                .child(self.scene_slider_row(SceneSlider::Scale, cx))
                .child(self.scene_slider_row(SceneSlider::PositionX, cx))
                .child(self.scene_slider_row(SceneSlider::PositionY, cx))
                .child(self.scene_slider_row(SceneSlider::RotationZ, cx))
                .child(self.section_header("advanced-transform", "Advanced transform", None, cx))
                .when(self.section_open("advanced-transform"), |this| {
                    this.child(self.scene_slider_row(SceneSlider::RotationX, cx))
                        .child(self.scene_slider_row(SceneSlider::RotationY, cx))
                        .child(self.scene_slider_row(SceneSlider::Perspective, cx))
                        .child(self.scene_slider_row(SceneSlider::AnchorX, cx))
                        .child(self.scene_slider_row(SceneSlider::AnchorY, cx))
                })
                .child(
                    div()
                        .text_xs()
                        .text_color(muted())
                        .child("Drag the media to move it, Shift-drag to tilt, Ctrl-drag to spin, scroll to scale, double-click to reset."),
                )
                .child(div().h(px(1.0)).bg(line()))
                .into_any_element(),
        )
    }

    pub(crate) fn background_effects_section(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(self.scene_slider_row(SceneSlider::Blur, cx))
            .child(self.scene_slider_row(SceneSlider::Noise, cx))
            .child(self.scene_slider_row(SceneSlider::Vignette, cx))
            .into_any_element()
    }

    pub(crate) fn watermark_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let enabled = self.watermark_enabled;
        let position_index = WatermarkPosition::ALL
            .iter()
            .position(|position| *position == self.watermark.position)
            .unwrap_or(3);
        div()
            .flex()
            .flex_col()
            .gap_2()
            .when(enabled, |this| {
                this.child(
                    self.text_fields.watermark.clone(),
                )
                .child(field("Position", self.position_grid(
                    "watermark-position",
                    &["Top left", "Top right", "Bottom left", "Bottom right"],
                    2,
                    position_index,
                    |this, index| this.watermark.position = WatermarkPosition::ALL[index],
                    cx,
                )))
                .child(self.scene_slider_row(SceneSlider::WatermarkSize, cx))
                .child(self.scene_slider_row(SceneSlider::WatermarkOpacity, cx))
            })
            .into_any_element()
    }

    pub(crate) fn pointer_section(&self, cx: &mut Context<Self>) -> AnyElement {
        use crate::recording::pointer_timeline::PointerRotation;
        let style = self.pointer_style;
        let selected_press = self.video_selected_press;
        let title = |text: &'static str| div().section_title().child(text);
        div()
            .flex()
            .flex_col()
            .gap(px(SPACE_3))
            .child(title("Cursor"))
            .child(self.scene_toggle_row(
                "pointer-visible",
                "Show cursor",
                style.visible,
                cx,
                |this| this.pointer_style.visible = !this.pointer_style.visible,
            ))
            .when(style.visible, |this| {
                this.child(field(
                    "Style",
                    self.segmented(
                        "pointer-family",
                        &["Recorded", "macOS", "macOS (new)", "Windows"],
                        CursorFamily::ALL
                            .iter()
                            .position(|family| *family == style.family)
                            .unwrap_or(0),
                        |this, value| {
                            this.pointer_style.family = CursorFamily::ALL[value.min(3)];
                        },
                        cx,
                    ),
                ))
                .child(self.scene_slider_row(SceneSlider::PointerScale, cx))
                // Styled cursor artwork carries its own shadow.
                .when(style.family == CursorFamily::Recorded, |this| {
                    this.child(self.scene_toggle_row(
                        "pointer-shadow",
                        "Cursor shadow",
                        style.shadow,
                        cx,
                        |this| this.pointer_style.shadow = !this.pointer_style.shadow,
                    ))
                })
                .child(Self::section_divider())
                .child(title("Hide & motion"))
                .child(self.scene_toggle_row(
                    "pointer-hide-idle",
                    "Hide when idle",
                    style.hide_when_idle,
                    cx,
                    |this| {
                        this.pointer_style.hide_when_idle = !this.pointer_style.hide_when_idle;
                        this.rebuild_video_motion_timelines();
                    },
                ))
                .when(style.hide_when_idle, |panel| panel.child(self.scene_slider_row(SceneSlider::CursorIdleDelay, cx)))
                .child(self.scene_toggle_row("pointer-hide-typing", "Hide while typing", style.hide_when_typing, cx, |this| {
                    this.pointer_style.hide_when_typing = !this.pointer_style.hide_when_typing;
                    this.rebuild_video_motion_timelines();
                }))
                .child(field("Rotation", self.segmented("pointer-rotation", &["Off", "Tilt", "Direction"],
                    PointerRotation::ALL.iter().position(|v| *v == style.rotation).unwrap_or(1),
                    |this, i| { this.pointer_style.rotation = PointerRotation::ALL[i]; this.rebuild_video_motion_timelines(); }, cx)))
                .when(style.rotation != PointerRotation::Off, |panel| panel.child(self.scene_slider_row(SceneSlider::CursorRotation, cx)))
                .child(self.cursor_animation_controls(cx))
                .child(self.scene_toggle_row(
                    "pointer-loop",
                    "Return to start at end",
                    style.loop_to_start,
                    cx,
                    |this| {
                        this.pointer_style.loop_to_start = !this.pointer_style.loop_to_start;
                        this.rebuild_video_motion_timelines();
                    },
                ))
                .when(style.loop_to_start, |panel| panel.child(self.scene_slider_row(SceneSlider::CursorLoopDuration, cx)))
            })
            .child(Self::section_divider())
            .child(title("Clicks"))
            .child(self.scene_toggle_row(
                "pointer-clicks",
                "Click effects",
                style.click_effects,
                cx,
                |this| this.pointer_style.click_effects = !this.pointer_style.click_effects,
            ))
            .when(style.click_effects, |this| {
                this.child(div().flex().items_center().gap(px(2.0)).children(
                    CLICK_COLORS.into_iter().enumerate().map(|(index, color)| {
                        let name = ["Blue", "Red", "Orange", "Green", "Purple", "White"][index];
                        crate::controls::swatch(("click-color", index), name, style.click_color == color, rgb(color), None)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.pointer_style.click_color = color;
                                cx.notify();
                            }))
                    }),
                ))
            })
            .when_some(selected_press, |this, time| {
                this.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(hint(format!("Click at {} selected", crate::units::fmt_seconds(time))))
                        .child(self.small_button(
                            "pointer-remove-click",
                            "Delete click",
                            true,
                            cx,
                            |this, cx| this.remove_selected_press(cx),
                        )),
                )
            })
            .into_any_element()
    }

    pub(crate) fn audio_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let has_audio = !self.video_audio_levels.is_empty();
        div()
            .flex()
            .flex_col()
            .gap(px(SPACE_3))
            .when(!has_audio, |this| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(muted())
                        .child("This recording has no audio track."),
                )
            })
            .when(has_audio, |this| {
                this.child(self.scene_toggle_row(
                    "audio-mute",
                    "Include audio",
                    !self.video_audio_muted,
                    cx,
                    |this| {
                        this.pause_video_playback();
                        this.video_audio_muted = !this.video_audio_muted;
                        this.cancel_audio_preparation();
                        this.video_audio_prepare_pending = !this.video_audio_muted;
                    },
                ))
            })
            .when(has_audio && !self.video_audio_muted, |this| {
                use crate::recording::audio_mode::AudioMode;
                this.child(div().field_label().child("Microphone sound"))
                    .children(AudioMode::ALL.into_iter().map(|mode| {
                        let selected = self.video_audio_mode == mode;
                        div().id(("microphone-audio-mode", mode as usize))
                            .px_3().py_2().rounded_md().border_1()
                            .border_color(if selected { accent() } else { line() })
                            .bg(if selected { accent_tint() } else { surface() })
                            .cursor_pointer().flex().items_center().gap_2()
                            .child(div().flex_1().min_w_0().flex().flex_col().gap_1()
                                .child(div().row_label().child(mode.label()))
                                .child(hint(mode.description())))
                            .on_click(cx.listener(move |this, _, _, cx| this.set_video_audio_mode(mode, cx)))
                    }))
            })
            .when_some(self.video_audio_job.as_ref(), |panel, job| {
                panel.child(div().flex().items_center().justify_between().gap_2()
                    .child(div().flex_1().min_w_0().flex().flex_col().gap_1()
                        .child(div().hint().child(format!(
                            "{} · {}%", job.control.status(), job.control.progress())))
                        .when(self.video_audio_play_when_ready, |row| row.child(
                            div().hint().child("Playback will start when audio is ready."))))
                    .child(self.small_button("pause-audio-preparation", "Cancel", true, cx,
                        |this, cx| this.pause_audio_preparation(cx))))
            })
            .when_some(self.video_audio_error.as_ref(), |panel, error| {
                panel.child(inline_error(error.clone())
                    .child(self.small_button("retry-audio-preparation", "Retry", true, cx,
                        |this, cx| this.prepare_video_audio(cx))))
            })
            .child(self.imported_audio_controls(cx))
            .into_any_element()
    }

    /// "3840 × 2160 · 30 fps · 20.5 s · about 55 MB" for the current settings.
    pub(crate) fn export_summary(&self) -> String {
        let (source_width, source_height) = if self.video_project.is_some() {
            self.video_source_size
        } else {
            self.captured_dimensions.unwrap_or((1200, 720))
        };
        let style = self.scene_style();
        let height = self.export_resolution.canvas_height(source_height);
        let (width, height) = style.export_canvas_size(source_width, source_height, height);
        let frame_rate = self.export_frame_rate;
        let duration = self.video_duration.max(0.0);
        let estimate = estimate_size_bytes(self.export_format, self.export_codec, width, height, frame_rate, duration);
        format!(
            "{width} × {height} · {frame_rate:.0} fps · {} · about {}",
            crate::units::fmt_seconds(duration),
            format_size(estimate)
        )
    }

    pub(crate) fn export_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let format = self.export_format;
        let frame_rate = self.export_frame_rate;
        let resolution_index = ExportResolution::ALL
            .iter()
            .position(|resolution| *resolution == self.export_resolution)
            .unwrap_or(0);
        let format_index = ExportFormat::ALL
            .iter()
            .position(|candidate| *candidate == format)
            .unwrap_or(0);
        let fps_index = if (frame_rate - 60.0).abs() < 1.0 {
            1
        } else {
            0
        };
        div()
            .flex()
            .flex_col()
            .gap(px(SPACE_3))
            .child(field("Format", self.segmented(
                "export-section-format",
                &["MP4", "WebM", "GIF"],
                format_index,
                |this, index| {
                    this.export_format = ExportFormat::ALL[index];
                    this.export_frame_rate = this.export_format.default_frame_rate();
                },
                cx,
            )))
            .child(field("Resolution", self.segmented(
                "export-resolution",
                &["Original", "720p", "1080p", "1440p", "4K"],
                resolution_index,
                |this, index| this.export_resolution = ExportResolution::ALL[index],
                cx,
            )))
            .child(field("Frame rate", self.segmented(
                "export-frame-rate",
                &["30 fps", "60 fps"],
                fps_index,
                |this, index| {
                    this.export_frame_rate = if index == 1 { 60.0 } else { 30.0 }
                },
                cx,
            )))
            .when(format == ExportFormat::Gif, |this| {
                this.child(self.scene_toggle_row(
                    "export-loop",
                    "Loop forever",
                    self.export_loop,
                    cx,
                    |this| this.export_loop = !this.export_loop,
                ))
            })
            .when(format == ExportFormat::Mp4, |this| {
                let codec_index = crate::recording::export::VideoCodec::ALL
                    .iter()
                    .position(|codec| *codec == self.export_codec)
                    .unwrap_or(0);
                this.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(SPACE_1))
                        .child(field("Codec", self.segmented(
                            "export-codec",
                            &["H.264", "HEVC"],
                            codec_index,
                            |this, index| this.export_codec = crate::recording::export::VideoCodec::ALL[index],
                            cx,
                        )))
                        .when(self.export_codec == crate::recording::export::VideoCodec::Hevc, |this| {
                            this.child(hint(
                                "Smaller files; some older players and browsers cannot play HEVC.",
                            ))
                        }),
                )
            })
            .into_any_element()
    }

    /// Export → Audio only: the soundtrack without video.
    pub(crate) fn audio_only_section(&self, cx: &mut Context<Self>) -> AnyElement {
        field(
            "Audio only",
            div()
                .flex()
                .gap_2()
                .child(self.small_button(
                    "export-audio-m4a",
                    "Export M4A…",
                    self.export_progress.is_none(),
                    cx,
                    |this, cx| this.export_audio_only(crate::recording::export::AudioFormat::M4a, cx),
                ))
                .child(self.small_button(
                    "export-audio-wav",
                    "Export WAV…",
                    self.export_progress.is_none(),
                    cx,
                    |this, cx| this.export_audio_only(crate::recording::export::AudioFormat::Wav, cx),
                )),
        )
        .into_any_element()
    }

    pub(crate) fn preset_library_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let presets: Vec<(usize, String)> = self
            .preset_library
            .presets
            .iter()
            .enumerate()
            .map(|(index, preset)| (index, preset.name.clone()))
            .collect();
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .child(self.small_button(
                        "preset-save",
                        "Save current",
                        true,
                        cx,
                        |this, _| this.save_current_preset(),
                    )),
            )
            .when(presets.is_empty(), |this| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(muted())
                        .child("Save the current background, layout, border, shadow, pointer, and zoom strength to reuse them on any screenshot or recording."),
                )
            })
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_1()
                    .children(presets.into_iter().map(|(index, name)| {
                        div()
                            .flex()
                            .items_center()
                            .rounded_md()
                            .bg(control_bg())
                            .overflow_hidden()
                            .child(
                                div()
                                    .id(("preset-apply", index))
                                    .px_3()
                                    .h(px(30.0))
                                    .flex()
                                    .items_center()
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .cursor_pointer()
                                    .hover(|style| style.bg(control_hover()))
                                    .child(name)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.apply_saved_preset(index);
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .id(("preset-delete", index))
                                    .tooltip(crate::timeline_ui::tooltip("Delete preset"))
                                    .w(px(24.0))
                                    .h(px(30.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_xs()
                                    .text_color(muted())
                                    .cursor_pointer()
                                    .hover(|style| style.bg(danger_tint()).text_color(danger()))
                                    .child("×")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.delete_saved_preset(index);
                                        cx.notify();
                                    })),
                            )
                    })),
            )
            .into_any_element()
    }

    /// Built-in background looks as one-click chips (used by the Quick level).
    pub(crate) fn quick_presets_row(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_wrap()
            .gap_1()
            .children(
                BACKGROUND_PRESETS
                    .iter()
                    .enumerate()
                    .map(|(index, preset)| {
                        let selected = self.background_preset == Some(index);
                        div()
                            .id(("quick-preset", index))
                            .px_3()
                            .h(px(30.0))
                            .flex()
                            .items_center()
                            .rounded_md()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .bg(if selected { accent() } else { control_bg() })
                            .text_color(if selected { on_accent() } else { ink() })
                            .cursor_pointer()
                            .when(!selected, |this| this.hover(|style| style.bg(control_hover())))
                            .child(preset.name)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.apply_background_preset(index);
                                cx.notify();
                            }))
                    }),
            )
            .into_any_element()
    }

    pub(crate) fn save_current_preset(&mut self) {
        let index = self.preset_library.add(ScenePreset {
            name: "My preset".into(),
            style: self.scene_style(),
            default_zoom: self.default_motion_zoom,
            aspect_index: self.aspect_ratio,
        });
        match self.preset_library.save() {
            Ok(()) => {
                self.toast =
                    Some(format!("Saved {}", self.preset_library.presets[index].name).into())
            }
            Err(error) => self.toast = Some(crate::Notification::error(format!("Could not save preset: {error}"))),
        }
    }

    pub(crate) fn apply_saved_preset(&mut self, index: usize) {
        let Some(preset) = self.preset_library.presets.get(index).cloned() else {
            return;
        };
        self.apply_scene_style(&preset.style);
        self.default_motion_zoom = preset.default_zoom;
        self.aspect_ratio = preset.aspect_index;
        self.background_preset = None;
    }

    pub(crate) fn delete_saved_preset(&mut self, index: usize) {
        if let Some(preset) = self.preset_library.remove(index) {
            if let Err(error) = self.preset_library.save() {
                self.toast = Some(crate::Notification::error(format!("Could not update presets: {error}")));
            } else {
                let message = format!("Deleted {}", preset.name);
                self.toast = Some(crate::notifications::Notification::with_action(message, "Undo", move |this, _| {
                    let index = index.min(this.preset_library.presets.len());
                    this.preset_library.presets.insert(index, preset.clone());
                    if let Err(error) = this.preset_library.save() {
                        this.toast = Some(crate::Notification::error(format!("Could not update presets: {error}")));
                    }
                }));
            }
        }
    }

    // ------------------------------------------------------------------
    // Pointer events (click markers)
    // ------------------------------------------------------------------

    pub(crate) fn remove_selected_press(&mut self, cx: &mut Context<Self>) {
        let Some(time) = self.video_selected_press.take() else {
            return;
        };
        if !self
            .video_removed_presses
            .iter()
            .any(|t| (t - time).abs() < 1e-6)
        {
            self.video_removed_presses.push(time);
        }
        self.rebuild_video_motion_timelines();
        cx.notify();
    }

    /// Ruler markers for pointer presses, positioned in editor time.
    pub(crate) fn press_markers(
        &self,
        timeline_duration: f64,
        timeline_content_width: f64,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let mut markers = Vec::new();
        for (index, press) in self.video_press_times.iter().enumerate() {
            let time = *press;
            if self
                .video_removed_presses
                .iter()
                .any(|removed| (removed - time).abs() < 1e-6)
            {
                continue;
            }
            let Some(editor_time) = self.video_clip_timeline.editor_time_for_event(time) else {
                continue;
            };
            let x = editor_time / timeline_duration * timeline_content_width;
            let selected = self.video_selected_press == Some(time);
            // 16 px hit area around the 8 px dot, in the ruler's marker
            // band between the labels and the ticks.
            markers.push(
                div()
                    .id(("press-marker", index))
                    .absolute()
                    .left(px(x as f32 - 8.0))
                    .top(px(17.0))
                    .size(px(16.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .tooltip(crate::timeline_ui::tooltip(format!(
                        "Click · {}",
                        crate::units::timecode(editor_time)
                    )))
                    .child(
                        div()
                            .size(px(8.0))
                            .rounded_full()
                            .bg(if selected {
                                ink()
                            } else {
                                Hsla::from(rgb(self.pointer_style.click_color))
                            })
                            .border_1()
                            .border_color(hsla(0.0, 0.0, 1.0, 0.9)),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.video_selected_press = Some(time);
                            this.video_selected_zoom_cue = None;
                            cx.notify();
                        }),
                    )
                    .into_any_element(),
            );
        }
        markers
    }

    // ------------------------------------------------------------------
    // Timeline extras
    // ------------------------------------------------------------------

    /// Waveform bars mapped through the clip timeline.
    pub(crate) fn audio_lane(
        &self,
        timeline_scroll: f64,
        timeline_content_width: f64,
        progress: f64,
    ) -> Option<AnyElement> {
        if self.video_audio_levels.is_empty() || self.video_source_duration <= 0.0 {
            return None;
        }
        let duration = self.video_timeline_duration();
        let muted_lane = self.video_audio_muted;
        let bucket_count = self.video_audio_levels.len();
        let mut bars: Vec<AnyElement> = Vec::new();
        let bar_width = (timeline_content_width / 300.0).clamp(1.5, 6.0);
        let mut x = 0.0;
        while x < timeline_content_width {
            let editor_time = x / timeline_content_width * duration;
            let Some(source_time) = self.video_clip_timeline.content_source_time_at(editor_time) else {
                x += bar_width;
                continue;
            };
            let bucket = ((source_time / self.video_source_duration) * bucket_count as f64)
                .floor()
                .clamp(0.0, bucket_count as f64 - 1.0) as usize;
            let level = self.video_audio_levels[bucket];
            let height = (2.0 + level * 18.0) as f32;
            bars.push(
                div()
                    .absolute()
                    .left(px(x as f32))
                    .top(px(11.0 - height * 0.5))
                    .w(px((bar_width - 0.5) as f32))
                    .h(px(height))
                    .rounded_sm()
                    .bg(if muted_lane {
                        hsla(0.0, 0.0, 0.6, 0.6)
                    } else {
                        hsla(152.0 / 360.0, 0.6, 0.45, 0.9)
                    })
                    .into_any_element(),
            );
            x += bar_width;
        }
        Some(
            div()
                .relative()
                .w_full()
                .h(px(22.0))
                .flex_none()
                .overflow_hidden()
                .rounded_lg()
                .bg(control_bg())
                .child(
                    div()
                        .absolute()
                        .left(px(-(timeline_scroll as f32)))
                        .top_0()
                        .w(px(timeline_content_width as f32))
                        .h_full()
                        .children(bars)
                        .children(self.video_clip_timeline.clip_starts().into_iter().skip(1).map(|start| {
                            div().absolute().left(px((start / duration * timeline_content_width) as f32))
                                .top_0().w(px(1.0)).h_full().bg(control_bg())
                        }))
                        .child(
                            div()
                                .absolute()
                                .left(px((timeline_content_width * progress) as f32 - 1.0))
                                .top_0()
                                .w(px(2.0))
                                .h_full()
                                .bg(brand()),
                        ),
                )
                .when(muted_lane, |this| {
                    this.child(
                        div()
                            .absolute()
                            .left(px(10.0))
                            .top(px(4.0))
                            .text_xs()
                            .text_color(muted())
                            .child("Muted"),
                    )
                })
                .into_any_element(),
        )
    }

    /// Stable source-time tiles, with coarse images while detailed samples load.
    pub(crate) fn clip_thumbnails(
        &self,
        editor_start: f64,
        source_start: f64,
        source_end: f64,
        speed: f64,
        clip_width: f32,
        lane_height: f32,
    ) -> Vec<AnyElement> {
        self.timeline_thumbnail_tiles(editor_start, source_start, source_end, speed, clip_width)
            .into_iter().map(|(left, width, thumbnail)| {
                div().absolute().left(px(left)).top(px(2.0))
                    .w(px(width)).h(px((lane_height - 4.0).max(8.0))).overflow_hidden()
                    .child(img(thumbnail).size_full().object_fit(gpui::ObjectFit::Cover).opacity(0.9))
                    .into_any_element()
            }).collect()
    }

    // ------------------------------------------------------------------
    // Timed annotations
    // ------------------------------------------------------------------

    /// Lane of timed annotations under the motion lane.
    pub(crate) fn annotation_track(
        &self,
        timeline_scroll: f64,
        timeline_content_width: f64,
        progress: f64,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.annotations.is_empty() {
            self.annotation_timeline_selection.set(None);
            self.annotation_timeline_scroll.set_offset(point(px(0.), px(0.)));
            return None;
        }
        let duration = self.video_timeline_duration();
        let selected = self.selected_annotation;
        let rows = self.annotation_lane_rows();
        let lane_height = self.annotation_lane_height();
        let content_height = 6.0 + (rows.iter().max().copied().unwrap_or(0) + 1) as f32 * Self::ANNOTATION_ROW_HEIGHT;
        let selected_row = selected.and_then(|index| rows.get(index).map(|row| (index, *row)));
        // Reveal a newly selected/created row once; manual scrolling must remain free.
        if self.annotation_timeline_selection.replace(selected_row) != selected_row {
            if let Some((_, row)) = selected_row {
                let top = 3.0 + row as f32 * Self::ANNOTATION_ROW_HEIGHT;
                let current = -f32::from(self.annotation_timeline_scroll.offset().y);
                let scroll = if top < current { top - 3.0 }
                    else if top + Self::ANNOTATION_ROW_HEIGHT > current + lane_height {
                        top + Self::ANNOTATION_ROW_HEIGHT - lane_height + 3.0
                    } else { current };
                self.annotation_timeline_scroll.set_offset(point(px(0.), px(-scroll.clamp(0.0, content_height - lane_height))));
            }
        }
        let mut regions: Vec<AnyElement> = Vec::new();
        let mut handles: Vec<AnyElement> = Vec::new();
        for (index, mark) in self.annotations.iter().enumerate() {
            let timing = mark.timing.unwrap_or(AnnotationTiming {
                start: 0.0,
                end: self.video_duration,
                ..AnnotationTiming::default()
            });
            let left = timing.start / duration * timeline_content_width;
            let width = ((timing.end - timing.start) / duration * timeline_content_width).max(20.0);
            let top =
                3.0 + rows.get(index).copied().unwrap_or(0) as f32 * Self::ANNOTATION_ROW_HEIGHT;
            let is_selected = selected == Some(index);
            let whole = mark.timing.is_none();
            let moving = self.annotation_drag.as_ref().is_some_and(|drag| {
                drag.index == index && matches!(drag.kind, AnnotationDragKind::Move)
            });
            regions.push(
                div()
                    .id(("annotation-region", index))
                    .absolute()
                    .left(px(left as f32))
                    .top(px(top))
                    .w(px(width as f32))
                    .h(px(22.0))
                    .rounded_md()
                    .border_2()
                    .border_color(if is_selected { ink() } else { gpui::transparent_black() })
                    .bg(if is_selected {
                        Hsla { l: lane_clip().l - 0.06, ..lane_clip() }
                    } else {
                        lane_clip()
                    })
                    .when(!is_selected, |this| this.hover(|s| s.border_color(ink().opacity(0.4))))
                    .when(whole, |this| this.opacity(0.7))
                    .text_color(gpui::white())
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .flex()
                    .items_center()
                    .overflow_hidden()
                    .cursor(if moving { CursorStyle::ClosedHand } else { CursorStyle::OpenHand })
                    .when(width as f32 >= crate::timeline_ui::REGION_LABEL_MIN_WIDTH, |this| {
                        let name = if mark.tool == Tool::Text && !mark.text.trim().is_empty() {
                            mark.text.trim()
                        } else {
                            mark.tool.label()
                        };
                        this.child(crate::timeline_ui::region_label(if whole {
                            format!("{name} · whole scene")
                        } else {
                            name.to_string()
                        }))
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.select_annotation_for_timing(index);
                            // Resolve a visible frame once the click/trim
                            // settles, not a new FFmpeg seek per drag sample.
                            this.begin_annotation_drag(
                                index,
                                AnnotationDragKind::Move,
                                event.position.x,
                            );
                            cx.notify();
                        }),
                    )
                    .into_any_element(),
            );
            if whole {
                continue;
            }
            for (leading, kind) in [
                (true, AnnotationDragKind::Leading),
                (false, AnnotationDragKind::Trailing),
            ] {
                handles.push(
                    crate::timeline_ui::trim_handle(
                        (if leading { "annotation-leading" } else { "annotation-trailing" }, index),
                        0.0,
                        (if leading { left } else { left + width }) as f32,
                        leading,
                        width as f32,
                        top,
                        22.0,
                        is_selected,
                        false,
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.select_annotation_for_timing(index);
                            this.begin_annotation_drag(index, kind, event.position.x);
                            cx.notify();
                        }),
                    )
                    .into_any_element(),
                );
            }
        }
        regions.extend(handles);
        Some(
            div()
                .id("annotation-track")
                .relative()
                .w_full()
                .h(px(lane_height))
                .flex_none()
                .overflow_hidden()
                .overflow_y_scroll()
                .track_scroll(&self.annotation_timeline_scroll)
                .rounded_lg()
                .bg(control_bg())
                .child(
                    div()
                        .relative()
                        .w_full()
                        .h(px(content_height))
                        .child(div()
                            .absolute()
                            .left(px(-(timeline_scroll as f32)))
                            .top_0()
                            .w(px(timeline_content_width as f32))
                            .h_full()
                            .children(regions))
                        .child(
                            div()
                                .absolute()
                                .left(px((timeline_content_width * progress - timeline_scroll) as f32 - 1.0))
                                .top_0()
                                .w(px(2.0))
                                .h_full()
                                .bg(brand()),
                        ),
                )
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, event: &MouseDownEvent, _, cx| {
                        this.pause_video_playback();
                        if let Some(target) = this.motion_timeline_time_at(event.position.x) {
                            this.finish_annotation_interaction();
                            this.video_position = target;
                            this.video_seek_drag = Some((event.position.x, target));
                        }
                        cx.notify();
                    }),
                )
                .into_any_element(),
        )
    }

    const ANNOTATION_ROW_HEIGHT: f32 = 24.0;
    const ANNOTATION_VISIBLE_ROWS: usize = 3;

    /// Reuse a row only after its previous annotation ends.
    pub(crate) fn annotation_lane_rows(&self) -> Vec<usize> {
        annotation_lane_rows(&self.annotations, self.video_duration)
    }

    /// Height of the annotation lane for the current stacking.
    pub(crate) fn annotation_lane_height(&self) -> f32 {
        let rows = self
            .annotation_lane_rows()
            .into_iter()
            .max()
            .map(|row| row + 1)
            .unwrap_or(1);
        6.0 + rows.min(Self::ANNOTATION_VISIBLE_ROWS) as f32 * Self::ANNOTATION_ROW_HEIGHT
    }

    pub(crate) fn select_annotation_for_timing(&mut self, index: usize) {
        if self.video_playing || self.video_audio_play_when_ready { self.pause_video_playback(); }
        self.annotation_editing_time = None;
        // Finishing an empty caption removes it, shifting subsequent row indices.
        let removed = self.editing_text.filter(|editing| {
            self.annotations.get(*editing).is_some_and(|mark| mark.text.trim().is_empty())
        });
        self.stop_editing_text();
        if removed == Some(index) {
            return;
        }
        let index = index - usize::from(removed.is_some_and(|removed| removed < index));
        if index >= self.annotations.len() {
            return;
        }
        self.selected_annotation = Some(index);
        self.annotation_selection = vec![index];
        self.annotation_edit_preview_pending = true;
        self.video_selected_zoom_cue = None;
        self.scene_selection = SceneSelection::Scene;
    }

    fn begin_annotation_drag(&mut self, index: usize, kind: AnnotationDragKind, start_x: Pixels) {
        let Some(mark) = self.annotations.get(index) else {
            return;
        };
        let duration = self.video_duration;
        let original = mark.timing.unwrap_or(AnnotationTiming {
            start: 0.0,
            end: duration,
            ..AnnotationTiming::default()
        });
        if mark.timing.is_none() && kind != AnnotationDragKind::Move {
            return;
        }
        let original_timing = mark.timing;
        if self.video_playing {
            self.pause_video_playback();
        }
        self.timeline_ui.snap_guide = None;
        self.annotation_drag = Some(AnnotationDrag {
            index,
            kind,
            start_x,
            original,
            original_timing,
            start_scroll: self.video_timeline_scroll,
            seconds_per_pixel: self.video_timeline_seconds_per_pixel(),
        });
    }

    pub(crate) fn update_annotation_drag(&mut self, pointer_x: Pixels) -> bool {
        let Some(drag) = self.annotation_drag else {
            return false;
        };
        let targets = self.timeline_snap_targets(None, None, None);
        let (timing, guide) = drag.preview(
            pointer_x, self.video_timeline_scroll, self.video_duration, &targets,
            self.timeline_ui.snap_enabled,
        );
        self.timeline_ui.snap_guide = guide;
        let Some(mark) = self.annotations.get_mut(drag.index) else { return false; };
        mark.timing = timing;
        true
    }

    pub(crate) fn end_annotation_drag(&mut self) -> bool {
        let Some(drag) = self.annotation_drag.take() else { return false; };
        self.timeline_ui.snap_guide = None;
        if let Some(mark) = self.annotations.get_mut(drag.index) {
            let current = mark.timing;
            if current != drag.original_timing {
                mark.timing = drag.original_timing;
                self.record_annotation_undo();
                self.annotations[drag.index].timing = current;
            }
        }
        self.annotation_edit_preview_pending = true;
        true
    }

    pub(crate) fn cancel_annotation_timeline_drag(&mut self) -> bool {
        let Some(drag) = self.annotation_drag.take() else { return false; };
        if let Some(mark) = self.annotations.get_mut(drag.index) {
            mark.timing = drag.original_timing;
        }
        self.timeline_ui.snap_guide = None;
        self.annotation_edit_preview_pending = true;
        true
    }

    fn edit_selected_timing(&mut self, edit: impl FnOnce(&mut AnnotationTiming, f64)) {
        let duration = self.video_duration;
        let Some(mark) = self
            .selected_annotation
            .and_then(|index| self.annotations.get_mut(index))
        else {
            return;
        };
        let mut timing = mark
            .timing
            .unwrap_or_else(|| AnnotationTiming::for_tool(mark.tool, 0.0, duration));
        edit(&mut timing, duration);
        mark.timing = Some(timing.clamped(duration));
        self.annotation_edit_preview_pending = true;
    }

    pub(crate) fn commit_annotation_time(&mut self) {
        let Some((index, start, text)) = self.annotation_time_edit.take() else {
            return;
        };
        if self.annotations.get(index).is_none() {
            return;
        }
        let Ok(value) = text.parse::<f64>() else {
            return;
        };
        if !value.is_finite() || value < 0.0 {
            return;
        }
        self.record_annotation_undo();
        let duration = self.video_duration;
        let mark = &mut self.annotations[index];
        let timing = mark.timing.unwrap_or_else(|| AnnotationTiming::for_tool(mark.tool, 0.0, duration));
        if let Some(updated) = timing.with_boundary(start, value, duration) {
            mark.timing = Some(updated.clamped(duration));
            self.annotation_edit_preview_pending = true;
        }
    }

    fn annotation_time_field(
        &self,
        _index: usize,
        start: bool,
        _value: f64,
        _cx: &mut Context<Self>,
    ) -> AnyElement {
        div().w(px(88.0)).flex().items_center().gap_1()
            .child(div().flex_1().min_w_0().child(if start {
                self.text_fields.start.clone()
            } else {
                self.text_fields.end.clone()
            }))
            .child(div().text_xs().text_color(muted()).child("s"))
            .into_any_element()
    }

    /// Timing panel for the selected annotation in an animated screenshot.
    pub(crate) fn annotation_timing_inspector(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.scene_is_timed() {
            return None;
        }
        let index = self.selected_annotation?;
        let mark = self.annotations.get(index)?;
        let whole = mark.timing.is_none();
        let timing = mark.timing.unwrap_or(AnnotationTiming {
            start: 0.0,
            end: self.video_duration,
            ..AnnotationTiming::default()
        });
        let entrance_index = EntranceEffect::ALL
            .iter()
            .position(|effect| *effect == timing.entrance)
            .unwrap_or(1);
        let exit_index = ExitEffect::ALL
            .iter()
            .position(|effect| *effect == timing.exit)
            .unwrap_or(1);
        let title = "Timing";
        Some(
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
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(div().size(px(10.0)).rounded_full().bg(lane_clip()))
                                .child(div().section_title().child(title)),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted())
                                .tabular()
                                .child(crate::units::fmt_seconds_range(timing.start, timing.end)),
                        ),
                )
                .child(self.scene_toggle_row(
                    "annotation-whole-scene",
                    "Show for the whole scene",
                    whole,
                    cx,
                    |this| {
                        let duration = this.video_duration;
                        let position = this.video_position;
                        if let Some(mark) = this
                            .selected_annotation
                            .and_then(|index| this.annotations.get_mut(index))
                        {
                            mark.timing = if mark.timing.is_some() {
                                None
                            } else {
                                Some(AnnotationTiming::for_tool(mark.tool, position, duration))
                            };
                        }
                    },
                ))
                .when(!mark.is_canvas(), |panel| panel.child(self.scene_toggle_row(
                    "annotation-pinned",
                    "Ignore camera zoom",
                    mark.pinned,
                    cx,
                    |this| {
                        if let Some(mark) = this
                            .selected_annotation
                            .and_then(|index| this.annotations.get_mut(index))
                        {
                            mark.pinned = !mark.pinned;
                        }
                    },
                )))
                .when(!whole, |this| {
                    this.child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted())
                                    .w(px(40.0))
                                    .child("Start"),
                            )
                            .child(self.annotation_time_field(index, true, timing.start, cx))
                            .child(self.small_button(
                                "timing-start-playhead",
                                "At playhead",
                                true,
                                cx,
                                |this, _| {
                                    let position = this.video_position;
                                    this.edit_selected_timing(|timing, _| {
                                        let length = timing.duration();
                                        timing.start = position;
                                        timing.end = position + length;
                                    })
                                },
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().text_xs().text_color(muted()).w(px(40.0)).child("End"))
                            .child(self.annotation_time_field(index, false, timing.end, cx))
                            .child(self.small_button(
                                "timing-end-playhead",
                                "At playhead",
                                true,
                                cx,
                                |this, _| {
                                    let position = this.video_position;
                                    this.edit_selected_timing(|timing, _| {
                                        timing.end = position
                                            .max(timing.start + AnnotationTiming::MINIMUM_DURATION)
                                    });
                                },
                            ))
                            .child(self.small_button(
                                "timing-end-scene",
                                "To end",
                                true,
                                cx,
                                |this, _| {
                                    this.edit_selected_timing(|timing, duration| {
                                        timing.end = duration
                                    })
                                },
                            )),
                    )
                    .child(
                        div()
                            .mt_3()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Animation"),
                    )
                    .child(div().field_label().child("Entrance"))
                    .child(self.segmented(
                        "annotation-entrance-a",
                        &["Cut", "Fade", "Pop", "Slide up"],
                        if entrance_index < 4 {
                            entrance_index
                        } else {
                            usize::MAX
                        },
                        |this, index| {
                            this.edit_selected_timing(|timing, _| {
                                timing.entrance = EntranceEffect::ALL[index]
                            })
                        },
                        cx,
                    ))
                    .child(self.segmented(
                        "annotation-entrance-b",
                        if mark.tool == Tool::Text {
                            &["Slide in", "Draw", "Type"]
                        } else {
                            &["Slide in", "Draw"]
                        },
                        if entrance_index >= 4 {
                            entrance_index - 4
                        } else {
                            usize::MAX
                        },
                        |this, index| {
                            this.edit_selected_timing(|timing, _| {
                                timing.entrance = EntranceEffect::ALL[index + 4]
                            })
                        },
                        cx,
                    ))
                    .child(div().field_label().child("Exit"))
                    .child(self.segmented(
                        "annotation-exit",
                        &["Cut", "Fade", "Shrink", "Slide out"],
                        exit_index,
                        |this, index| {
                            this.edit_selected_timing(|timing, _| {
                                timing.exit = ExitEffect::ALL[index]
                            })
                        },
                        cx,
                    ))
                    .child(self.scene_slider_row(SceneSlider::AnnotationTransition, cx))
                })
                .child(div().h(px(1.0)).bg(line()))
                .into_any_element(),
        )
    }

    /// Overlay renderer for animated-screenshot export: timed annotations
    /// at the capture's resolution, re-rendered only when they change.
    pub(crate) fn annotation_overlay_source(
        &self,
    ) -> Option<crate::recording::export::MediaOverlaySource> {
        let (width, height) = self.media_dimensions()?;
        cropped_overlay_source_for(
            self.annotations.clone(),
            width,
            height,
            self.scene_style().source_crop,
        )
    }
}

/// Overlay renderer for `marks` on media of the given size. The compositor
/// supplies the effective viewport for each shutter sample.
pub(crate) fn overlay_source_for(
    marks: Vec<AnnotationMark>,
    width: u32,
    height: u32,
) -> Option<crate::recording::export::MediaOverlaySource> {
    cropped_overlay_source_for(marks, width, height, crate::CropRect::UNIT)
}

fn cropped_overlay_source_for(
    marks: Vec<AnnotationMark>,
    width: u32, height: u32, crop: crate::CropRect,
) -> Option<crate::recording::export::MediaOverlaySource> {
    {
        if marks.is_empty() {
            return None;
        }
        let mut cache: Option<(u64, Arc<RgbaImage>)> = None;
        Some(Box::new(move |time: f64, viewport: ViewportFrame| {
            let active = timed::active_marks_in_crop(&marks, time, viewport, crop);
            if active.is_empty() {
                return None;
            }
            let signature = timed::marks_signature(&active);
            if let Some((cached, layer)) = cache.as_ref() {
                if *cached == signature {
                    return Some(layer.clone());
                }
            }
            let layer = Arc::new(render_annotation_layer(&active, width, height)?);
            cache = Some((signature, layer.clone()));
            Some(layer)
        }))
    }
}

/// Canvas-space overlays use a fixed reference resolution and are scaled
/// to the final canvas by the shared compositor.
pub(crate) fn canvas_overlay_source_for(
    marks: Vec<AnnotationMark>,
    aspect: f64,
) -> Option<crate::recording::export::OverlaySource> {
    let marks: Vec<_> = marks.into_iter().filter(|mark| mark.is_canvas()).collect();
    if marks.is_empty() {
        return None;
    }
    let (width, height) = if aspect >= 1.0 {
        ((800.0 * aspect).round() as u32, 800)
    } else {
        (800, (800.0 / aspect.max(0.1)).round() as u32)
    };
    let mut cache: Option<(u64, Arc<RgbaImage>)> = None;
    Some(Box::new(move |time| {
        let active: Vec<_> = marks
            .iter()
            .filter_map(|mark| timed::animated_mark(mark, time))
            .collect();
        // An inactive caption must not send a full transparent canvas through
        // the compositor (including its export-resolution resampling).
        if active.is_empty() {
            return None;
        }
        let signature = timed::marks_signature(&active);
        if let Some((cached, layer)) = &cache {
            if *cached == signature {
                return Some(layer.clone());
            }
        }
        let layer = Arc::new(render_annotation_layer(&active, width, height)?);
        cache = Some((signature, layer.clone()));
        Some(layer)
    }))
}

/// Cap preview texture work to the display size; exports retain source resolution.
fn annotation_preview_size(source: (u32, u32), canvas: (u32, u32)) -> (u32, u32) {
    let scale = ((canvas.0.max(canvas.1) as f64 * 1.5)
        / source.0.max(source.1).max(1) as f64).min(1.0);
    ((source.0 as f64 * scale).round().max(1.0) as u32,
     (source.1 as f64 * scale).round().max(1.0) as u32)
}

pub(crate) use lahza_annotations::svg::render_annotations as render_annotation_layer;

/// Paint independent text above the entire scene, including its background.
pub(crate) fn paint_canvas_annotations(
    marks: &[(usize, AnnotationMark)],
    selected: &[usize],
    editing: Option<usize>,
    bounds: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut gpui::App,
) -> Vec<(usize, Bounds<Pixels>)> {
    let mut hits = Vec::new();
    let scale = lahza_annotations::canvas::canvas_scale(bounds);
    window.with_content_mask(Some(ContentMask { bounds }), |window| {
        for (index, mark) in marks {
            let mut mark = mark.clone();
            mark.font_size *= scale;
            mark.scale_stroke_width(scale);
            let hit = paint_annotation(&mark, bounds, false, false, window, cx);
            hits.push((*index, hit));
            if selected.len() == 1 && selected.contains(index) && editing!=Some(*index) {
                crate::annotations::paint_selection(&mark,bounds,hit,window);
            }
        }
    });
    hits
}

/// Canvas-pixel radius within which a press grabs a motion marker.
const MOTION_MARKER_HIT: f64 = 14.0;

/// Paints the focus (filled) and pan-end (hollow) markers of the selected
/// motion region, joined by a line when both exist.
pub(crate) fn paint_motion_markers(
    markers: &[(bool, Point<Pixels>)],
    bounds: Bounds<Pixels>,
    window: &mut Window,
) {
    let orange = hsla(24.0 / 360.0, 0.95, 0.55, 1.0);
    let at = |p: Point<Pixels>| point(bounds.origin.x + p.x, bounds.origin.y + p.y);
    if markers.len() == 2 {
        let mut builder = PathBuilder::stroke(px(1.5));
        builder.move_to(at(markers[0].1));
        builder.line_to(at(markers[1].1));
        if let Ok(path) = builder.build() {
            window.paint_path(path, orange);
        }
    }
    for (is_focus, position) in markers {
        let center = at(*position);
        let radius = if *is_focus { 10.0 } else { 8.0 };
        window.paint_quad(quad(
            Bounds {
                origin: point(center.x - px(radius), center.y - px(radius)),
                size: size(px(radius * 2.0), px(radius * 2.0)),
            },
            px(radius),
            if *is_focus {
                hsla(24.0 / 360.0, 0.95, 0.55, 0.35)
            } else {
                hsla(0.0, 0.0, 0.0, 0.0)
            },
            px(2.0),
            if *is_focus {
                hsla(0.0, 0.0, 1.0, 1.0)
            } else {
                orange
            },
            Default::default(),
        ));
        window.paint_quad(quad(
            Bounds {
                origin: point(center.x - px(3.0), center.y - px(3.0)),
                size: size(px(6.0), px(6.0)),
            },
            px(3.0),
            orange,
            px(0.0),
            hsla(0.0, 0.0, 0.0, 0.0),
            Default::default(),
        ));
    }
}

fn paint_selection_handles(
    projection: &MediaProjection,
    bounds: Bounds<Pixels>,
    window: &mut Window,
) {
    let corners: Vec<Point<Pixels>> = projection
        .quad
        .iter()
        .map(|(x, y)| {
            point(
                bounds.origin.x + px(*x as f32),
                bounds.origin.y + px(*y as f32),
            )
        })
        .collect();
    let mut builder = PathBuilder::stroke(px(1.5));
    builder.move_to(corners[0]);
    for corner in corners.iter().skip(1) {
        builder.line_to(*corner);
    }
    builder.line_to(corners[0]);
    if let Ok(path) = builder.build() {
        window.paint_path(path, crate::selection().opacity(0.9));
    }
    for corner in corners {
        window.paint_quad(quad(
            Bounds {
                origin: point(corner.x - px(6.0), corner.y - px(6.0)),
                size: size(px(12.0), px(12.0)),
            },
            px(3.0),
            gpui::white(),
            px(2.0),
            crate::selection(),
            Default::default(),
        ));
    }
}

pub(crate) fn scene_canvas_bounds_store() -> Arc<Mutex<Option<Bounds<Pixels>>>> {
    Arc::new(Mutex::new(None))
}

pub(crate) fn easing_index(easing: MotionEasing) -> usize {
    MotionEasing::ALL
        .iter()
        .position(|candidate| *candidate == easing)
        .unwrap_or(0)
}

impl Studio {
    /// Loads waveform buckets and clip thumbnails for the open recording.
    pub(crate) fn spawn_video_extras(&mut self, cx: &mut Context<Self>) {
        self.reset_timeline_thumbnails();
        let Some(session) = self.video_project.as_ref() else {
            return;
        };
        let path = Self::media_source_for(session);
        self.video_extras_token = self.video_extras_token.wrapping_add(1);
        let token = self.video_extras_token;
        let task = cx.background_executor().spawn(async move {
            crate::recording::video::decode_thumbnails(&path, 32, 56).unwrap_or_default()
        });
        cx.spawn(async move |weak, cx| {
            let thumbnails = task.await;
            let _ = weak.update(cx, |this, cx| {
                if this.video_extras_token != token || this.video_project.is_none() {
                    return;
                }
                let previous = std::mem::replace(
                    &mut this.video_thumbnails,
                    thumbnails.into_iter().map(cached_render_image).collect(),
                );
                this.retired_images.extend(previous);
                cx.notify();
            });
        })
        .detach();
        self.refresh_audio_levels(cx);
    }

    /// Redraws the waveform for the current microphone preset: the cleaned
    /// audio once it is prepared, otherwise the unprocessed mix.
    pub(crate) fn refresh_audio_levels(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.video_project.as_ref() else {
            return;
        };
        let path = Self::media_source_for(session);
        let mode = self.video_audio_mode;
        self.video_levels_token = self.video_levels_token.wrapping_add(1);
        let token = self.video_levels_token;
        let task = cx.background_executor().spawn(async move {
            crate::recording::video::audio_levels(&path, 240, mode).unwrap_or_default()
        });
        cx.spawn(async move |weak, cx| {
            let levels = task.await;
            let _ = weak.update(cx, |this, cx| {
                if this.video_levels_token != token || this.video_project.is_none() {
                    return;
                }
                this.video_audio_levels = levels;
                cx.notify();
            });
        })
        .detach();
    }

    /// Static PNG export through the scene compositor, used whenever the
    /// style needs it (3D transform, blur, noise, vignette, watermark).
    pub(crate) fn render_composited_export(
        &mut self,
        destination: &std::path::Path,
    ) -> Result<(), String> {
        let image = self.render_annotated_capture()?;
        let style = self.scene_style();
        let height = self.export_resolution.canvas_height(image.height());
        let (width, height) = style.export_canvas_size(image.width(), image.height(), height);
        let compositor =
            SceneCompositor::new(&style, width, height, image.width(), image.height())?;
        let marks = self.annotations.iter().cloned().map(|mut mark| { mark.timing = None; mark }).collect();
        let overlay = canvas_overlay_source_for(marks, width as f64 / height as f64).and_then(|mut source| source(0.0));
        let frame = compositor.compose_layers(crate::recording::scene::FrameInput {
            source: &image,
            overlay: None,
            viewport: ViewportFrame::default(),
            pointer: None,
            camera: None,
        }, true, overlay.as_deref());
        frame
            .save(destination)
            .map_err(|error| format!("Could not save PNG: {error}"))
    }
}

#[cfg(test)]
mod canvas_caption_tests {
    use super::*;
    use crate::recording::scene::{FrameInput, SceneBackground, SceneCompositor, SceneTransform};

    #[test]
    fn annotation_rows_keep_every_overlapping_mark_accessible() {
        let marks = vec![AnnotationMark::default(); 12];
        assert_eq!(annotation_lane_rows(&marks, 8.0), (0..12).collect::<Vec<_>>());
        let mut marks = marks;
        marks.remove(4);
        assert_eq!(annotation_lane_rows(&marks, 8.0), (0..11).collect::<Vec<_>>());
        assert!(annotation_lane_rows(&[], 8.0).is_empty());
    }

    #[test]
    fn annotation_rows_reuse_only_finished_intervals() {
        let spans = [(4., 6.), (0., 2.), (1., 5.), (2., 4.), (0., 8.), (0.5, 7.)];
        let marks: Vec<_> = spans.iter().map(|&(start, end)| AnnotationMark {
            timing: Some(AnnotationTiming { start, end, ..Default::default() }),
            ..Default::default()
        }).collect();
        let rows = annotation_lane_rows(&marks, 8.0);
        assert_eq!(rows[0], rows[1]);
        assert_eq!(rows[1], rows[3]);
        assert_eq!(rows.iter().max(), Some(&3));
        for i in 0..spans.len() {
            for j in i + 1..spans.len() {
                if spans[i].0 < spans[j].1 && spans[j].0 < spans[i].1 {
                    assert_ne!(rows[i], rows[j], "overlapping marks {i} and {j}");
                }
            }
        }
    }

    #[test]
    fn canvas_caption_is_absent_outside_its_timing_and_survives_seeking() {
        let caption = AnnotationMark {
            tool: Tool::Text, canvas: true, text: "Caption".into(),
            timing: Some(AnnotationTiming { start: 12.5, end: 15.0, ..Default::default() }),
            ..Default::default()
        };
        let mut overlay = canvas_overlay_source_for(vec![caption], 16.0 / 9.0).unwrap();
        assert!(overlay(0.0).is_none());
        let visible = overlay(13.5).unwrap();
        assert!(visible.pixels().any(|pixel| pixel[3] > 0));
        assert!(Arc::ptr_eq(&visible, &overlay(13.6).unwrap()));
        assert!(overlay(16.0).is_none());
        assert!(overlay(12.0).is_none());
        assert_eq!(visible.as_ref(), overlay(13.5).unwrap().as_ref());
    }

    #[test]
    fn saved_template_caption_renders_beyond_transformed_screenshot() {
        let template = crate::recording::templates::find("product-launch").unwrap();
        let mut caption = template.marks(template.duration).into_iter()
            .find(|mark| mark.tool == Tool::Text).unwrap();
        // Simulate an older saved template caption dragged onto the background.
        caption.canvas = false;
        caption.start = crate::NormPoint { x: 0.05, y: 0.85 };
        caption.end = crate::NormPoint { x: 0.95, y: 0.99 };
        caption.color = 0xff0000;
        caption.font_size = 60.0;
        caption.timing = None;
        let caption: AnnotationMark = serde_json::from_str(
            &serde_json::to_string(&caption).unwrap()).unwrap();
        assert!(caption.is_canvas());
        assert!(timed::active_marks(&[caption.clone()], 2.0, Default::default()).is_empty());
        let mut overlay = canvas_overlay_source_for(vec![caption], 16.0 / 9.0).unwrap();
        let layer = overlay(2.0).unwrap();
        let style = SceneStyle {
            background: SceneBackground::Solid(0),
            padding: 0, shadow: 0, corners: 0, border: false,
            transform: SceneTransform {
                scale: 0.5, rotation_y: 15.0, ..Default::default()
            },
            ..Default::default()
        };
        let compositor = SceneCompositor::new(&style, 480, 270, 160, 90).unwrap();
        let source = RgbaImage::from_pixel(160, 90, image::Rgba([0, 255, 0, 255]));
        let output = compositor.compose_layers(FrameInput {
            source: &source, overlay: None, viewport: Default::default(),
            pointer: None, camera: None,
        }, true, Some(&layer));
        assert!(output.enumerate_pixels().any(|(_, y, p)|
            y > 220 && p[0] > 150 && p[1] < 80),
            "caption must remain visible below the screenshot");
    }

    #[test]
    fn template_callouts_and_manual_media_text_stay_on_media() {
        let mut mark = AnnotationMark {
            tool: Tool::Number, from_template: true, ..Default::default()
        };
        assert!(!mark.is_canvas());
        mark.tool = Tool::Text;
        mark.from_template = false;
        assert!(!mark.is_canvas());
        mark.canvas = true;
        assert!(mark.is_canvas());
    }
}
