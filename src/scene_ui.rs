//! Scene-level editing shared by both editors: the composited preview that
//! matches export pixel-for-pixel, 3D transform editing (inspector and
//! direct manipulation), background effects, watermark, pointer styling,
//! export options, the preset library, and timed annotations.

use gpui::{
    canvas, div, hsla, img, point, prelude::*, px, quad, rgb, size, AnyElement, Bounds, Context,
    CursorStyle, FontWeight, Hsla, KeyDownEvent, Modifiers, MouseButton, MouseDownEvent,
    PathBuilder, Pixels, Point, RenderImage, ScrollDelta, ScrollWheelEvent, Window,
};
use image::RgbaImage;
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use crate::{
    annotations_svg, blue, cached_render_image, ink, line, muted,
    recording::{
        export::{estimate_size_bytes, format_size, ExportFormat, ExportResolution},
        model::NormalizedPoint,
        presets::ScenePreset,
        scene::{
            render_svg_layer, MediaProjection, PointerOverlay, SceneCompositor, SceneGeometry,
            SceneStyle, SceneTransform, WatermarkPosition,
        },
        viewport::{MotionEasing, ViewportFrame},
    },
    timed::{self, AnnotationTiming, EntranceEffect, ExitEffect},
    AnnotationMark, SliderDrag, Studio, BACKGROUND_PRESETS,
};

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

#[derive(Clone, Copy, Debug)]
pub(crate) struct MediaDrag {
    pub kind: MediaDragKind,
    pub start: Point<Pixels>,
    pub original: SceneTransform,
    pub canvas_size: (f32, f32),
    /// Canvas-local pivot for scale drags.
    pub pivot: (f32, f32),
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
    pub seconds_per_pixel: f64,
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
}

impl SceneSlider {
    const BASE: usize = 100;
    const ALL: [SceneSlider; 19] = [
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
            SceneSlider::Scale => "Scale",
            SceneSlider::PositionX => "Position X",
            SceneSlider::PositionY => "Position Y",
            SceneSlider::RotationX => "Rotate X",
            SceneSlider::RotationY => "Rotate Y",
            SceneSlider::RotationZ => "Rotate Z",
            SceneSlider::Perspective => "Perspective",
            SceneSlider::AnchorX => "Anchor X",
            SceneSlider::AnchorY => "Anchor Y",
            SceneSlider::Blur => "Blur",
            SceneSlider::Noise => "Noise",
            SceneSlider::Vignette => "Vignette",
            SceneSlider::PointerScale => "Cursor size",
            SceneSlider::WatermarkSize => "Size",
            SceneSlider::WatermarkOpacity => "Opacity",
            SceneSlider::DefaultZoom => "Auto zoom",
            SceneSlider::AnnotationTransition => "Transition",
            SceneSlider::CameraSize => "Size",
            SceneSlider::CameraMargin => "Margin",
        }
    }

    fn range(self) -> (f64, f64) {
        match self {
            SceneSlider::Scale => (SceneTransform::MIN_SCALE, SceneTransform::MAX_SCALE),
            SceneSlider::PositionX | SceneSlider::PositionY => (-1.0, 1.0),
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
        }
    }

    fn default_value(self) -> f64 {
        match self {
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
            _ => 0.0,
        }
    }

    fn format(self, value: f64) -> String {
        match self {
            SceneSlider::Scale | SceneSlider::DefaultZoom => format!("{value:.2}×"),
            SceneSlider::PositionX | SceneSlider::PositionY => format!("{:.0}", value * 100.0),
            SceneSlider::RotationX | SceneSlider::RotationY | SceneSlider::RotationZ => {
                format!("{value:.0}°")
            }
            SceneSlider::Perspective | SceneSlider::AnchorX | SceneSlider::AnchorY => {
                format!("{:.0}%", value * 100.0)
            }
            SceneSlider::AnnotationTransition => format!("{value:.2}s"),
            _ => format!("{value:.0}%"),
        }
    }

    fn step(self) -> f64 {
        match self {
            SceneSlider::Scale | SceneSlider::DefaultZoom => 0.05,
            SceneSlider::PositionX | SceneSlider::PositionY => 0.02,
            SceneSlider::RotationX | SceneSlider::RotationY | SceneSlider::RotationZ => 1.0,
            SceneSlider::Perspective | SceneSlider::AnchorX | SceneSlider::AnchorY => 0.05,
            SceneSlider::AnnotationTransition => 0.05,
            SceneSlider::CameraMargin => 1.0,
            _ => 2.0,
        }
    }

    fn get(self, studio: &Studio) -> f64 {
        let transform = studio.scene_transform;
        match self {
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
            SceneSlider::CameraSize => studio.camera_overlay.size as f64,
            SceneSlider::CameraMargin => studio.camera_overlay.margin as f64,
        }
    }

    fn set(self, studio: &mut Studio, value: f64) {
        let (min, max) = self.range();
        let value = value.clamp(min, max);
        match self {
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
            SceneSlider::CameraSize => studio.camera_overlay.size = value.round() as u8,
            SceneSlider::CameraMargin => studio.camera_overlay.margin = value.round() as u8,
            SceneSlider::AnnotationTransition => {
                let duration = studio.video_duration;
                if let Some(mark) = studio
                    .selected_annotation
                    .and_then(|index| studio.annotations.get_mut(index))
                {
                    if let Some(timing) = mark.timing.as_mut() {
                        timing.transition = value;
                        *timing = timing.clamped(duration);
                    }
                }
            }
        }
    }
}

/// Composited preview state cached between renders.
pub(crate) struct PreviewCache {
    pub compositor: Option<(SceneStyle, (u32, u32), (u32, u32), SceneCompositor)>,
    pub frame: Option<(PreviewKey, Arc<RenderImage>)>,
    pub overlay: Option<(u64, Arc<RgbaImage>)>,
}

impl Default for PreviewCache {
    fn default() -> Self {
        Self {
            compositor: None,
            frame: None,
            overlay: None,
        }
    }
}

#[derive(Clone, PartialEq)]
pub(crate) struct PreviewKey {
    style: SceneStyle,
    canvas: (u32, u32),
    source: usize,
    time_bits: u64,
    overlay: u64,
    pointer: bool,
}

pub(crate) const CLICK_COLORS: [u32; 6] =
    [0x007aff, 0xff3b30, 0xff9500, 0x34c759, 0xaf52de, 0xffffff];

fn handle_hit(quad: &[(f64, f64); 4], x: f64, y: f64) -> Option<usize> {
    quad.iter()
        .position(|(qx, qy)| (qx - x).abs() <= 9.0 && (qy - y).abs() <= 9.0)
}

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
                        if let Some(asset) = crate::UIHSSN_WALLPAPERS
                            .iter()
                            .chain(crate::FAYAZ_WALLPAPERS.iter())
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
        self.border = style.border;
        self.border_thickness = style.border_thickness;
        if let Some(index) = crate::motion_ui::BORDER_COLORS
            .iter()
            .position(|color| *color == style.border_color)
        {
            self.border_color = index;
        }
        self.border_opacity = style.border_opacity;
        self.background_blur = style.background_blur;
        self.background_noise = style.background_noise;
        self.vignette = style.vignette;
        self.scene_transform = style.transform.clamped();
        self.watermark_enabled = style.watermark.is_some();
        if let Some(watermark) = style.watermark.clone() {
            self.watermark = watermark;
        }
        self.pointer_style = style.pointer;
        self.camera_overlay = style.camera;
    }

    /// Autosaves scene settings into the recording's edit draft when they
    /// changed since the last save (called from render, skipped mid-drag).
    pub(crate) fn autosave_scene_style(&mut self) {
        if self.slider_drag.is_some() || self.media_drag.is_some() {
            return;
        }
        let Some(session) = self.video_project.as_ref() else {
            return;
        };
        let style = self.scene_style();
        if self.persisted_scene_style.as_ref() != Some(&style) {
            if let Err(error) = session.write_edit_field("scene", &style) {
                self.toast = Some(format!("Could not autosave scene settings: {error}").into());
            }
            self.persisted_scene_style = Some(style);
        }
        let Some(session) = self.video_project.as_ref() else {
            return;
        };
        let extras = crate::RecordingExtras {
            audio_muted: self.video_audio_muted,
            removed_press_times: self.video_removed_presses.clone(),
        };
        if self.persisted_extras.as_ref() != Some(&extras) {
            if let Err(error) = session.write_edit_field("screendropExtras", &extras) {
                self.toast = Some(format!("Could not autosave settings: {error}").into());
            }
            self.persisted_extras = Some(extras);
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

    fn scene_slider_row(&self, slider: SceneSlider, cx: &mut Context<Self>) -> AnyElement {
        let value = slider.get(self);
        let (min, max) = slider.range();
        let fraction = ((value - min) / (max - min)).clamp(0.0, 1.0);
        let slider_value = (fraction * 100.0).round() as u8;
        let id = slider.id();
        let is_default = (value - slider.default_value()).abs() < 1e-6;
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .child(
                div()
                    .id(("scene-slider", id))
                    .relative()
                    .flex_1()
                    .h(px(36.0))
                    .rounded_lg()
                    .bg(rgb(0xf1f1f2))
                    .overflow_hidden()
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            this.slider_drag = Some(SliderDrag {
                                slider_id: id,
                                start_x: event.position.x,
                                start_value: slider_value,
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
                            .w(gpui::relative(fraction as f32))
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
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(slider.label()),
                    ),
            )
            .child(
                div()
                    .id(("scene-slider-minus", id))
                    .w(px(26.0))
                    .h(px(36.0))
                    .rounded_lg()
                    .bg(rgb(0xf1f1f2))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .child("−")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        slider.set(this, slider.get(this) - slider.step());
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .w(px(60.0))
                    .h(px(36.0))
                    .rounded_lg()
                    .bg(rgb(0xf1f1f2))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_xs()
                    .child(slider.format(value)),
            )
            .child(
                div()
                    .id(("scene-slider-plus", id))
                    .w(px(26.0))
                    .h(px(36.0))
                    .rounded_lg()
                    .bg(rgb(0xf1f1f2))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .child("+")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        slider.set(this, slider.get(this) + slider.step());
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .id(("scene-slider-reset", id))
                    .w(px(22.0))
                    .h(px(36.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(muted())
                    .opacity(if is_default { 0.0 } else { 1.0 })
                    .when(!is_default, |this| {
                        this.cursor_pointer()
                            .hover(|style| style.text_color(ink()))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                slider.set(this, slider.default_value());
                                cx.notify();
                            }))
                    })
                    .child("×"),
            )
            .into_any_element()
    }

    pub(crate) fn scene_section_title(title: &'static str) -> AnyElement {
        div()
            .mt_2()
            .pt_3()
            .border_t_1()
            .border_color(line())
            .text_sm()
            .font_weight(FontWeight::BOLD)
            .child(title)
            .into_any_element()
    }

    pub(crate) fn scene_toggle_row(
        &self,
        id: &'static str,
        label: &'static str,
        enabled: bool,
        cx: &mut Context<Self>,
        on_toggle: impl Fn(&mut Studio) + 'static,
    ) -> AnyElement {
        div()
            .flex()
            .items_center()
            .justify_between()
            .child(div().text_sm().child(label))
            .child(
                div()
                    .id(id)
                    .cursor_pointer()
                    .child(self.toggle(enabled))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        on_toggle(this);
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    // ------------------------------------------------------------------
    // Composited preview
    // ------------------------------------------------------------------

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
    fn preview_overlay(&mut self, time: f64) -> Option<(u64, Arc<RgbaImage>)> {
        if self.video_project.is_some() || self.annotations.is_empty() {
            return None;
        }
        let (width, height) = self.captured_dimensions?;
        let marks = if self.animation_active {
            timed::active_marks(&self.annotations, time)
        } else {
            self.annotations.clone()
        };
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

    /// Composited frame for the preview canvas, cached until an input changes.
    pub(crate) fn scene_preview_image(
        &mut self,
        canvas_width: Pixels,
        canvas_height: Pixels,
    ) -> Option<Arc<RenderImage>> {
        let source = self.preview_source()?;
        let canvas = (
            (f32::from(canvas_width).round() as u32).max(2),
            (f32::from(canvas_height).round() as u32).max(2),
        );
        let style = self.scene_style();
        let source_size = (source.width(), source.height());
        let time = if self.video_project.is_some() || self.animation_active {
            self.video_position
        } else {
            0.0
        };
        let viewport = if self.video_project.is_some() || self.animation_active {
            self.video_viewport_timeline.frame_at(time)
        } else {
            ViewportFrame::default()
        };
        let overlay = if !style.transform.is_identity() || !viewport.tilt.is_zero() {
            self.preview_overlay(time)
        } else {
            None
        };
        let has_pointer = (self.video_project.is_some() && self.video_pointer_synthesized)
            || (self.animation_active && self.has_walkthrough());
        let camera = if self.video_project.is_some() && self.camera_overlay.enabled {
            self.camera_frame_rgba.clone()
        } else {
            None
        };
        let key = PreviewKey {
            style: style.clone(),
            canvas,
            source: Arc::as_ptr(&source) as usize
                ^ camera
                    .as_ref()
                    .map(|frame| (Arc::as_ptr(frame) as usize).rotate_left(17))
                    .unwrap_or(0),
            time_bits: time.to_bits(),
            overlay: overlay
                .as_ref()
                .map(|(signature, _)| *signature)
                .unwrap_or(0),
            pointer: has_pointer,
        };
        if let Some((cached, image)) = self.preview_cache.frame.as_ref() {
            if *cached == key {
                return Some(image.clone());
            }
        }
        let rebuild = match self.preview_cache.compositor.as_ref() {
            Some((cached_style, cached_canvas, cached_source, _)) => {
                *cached_style != style || *cached_canvas != canvas || *cached_source != source_size
            }
            None => true,
        };
        if rebuild {
            let compositor =
                SceneCompositor::new(&style, canvas.0, canvas.1, source_size.0, source_size.1)
                    .ok()?;
            self.preview_cache.compositor = Some((style.clone(), canvas, source_size, compositor));
        }
        let pointer = has_pointer
            .then(|| self.video_pointer_timeline.frame_at(time))
            .flatten()
            .map(|frame| PointerOverlay { frame });
        let compositor = &self.preview_cache.compositor.as_ref()?.3;
        let frame = compositor.compose(crate::recording::scene::FrameInput {
            source: &source,
            overlay: overlay.as_ref().map(|(_, layer)| layer.as_ref()),
            viewport,
            pointer: pointer.as_ref(),
            camera: camera.as_deref(),
        });
        let image = cached_render_image(frame);
        self.preview_cache.frame = Some((key, image.clone()));
        Some(image)
    }

    /// Whether the screenshot preview must show the compositor's frame: the
    /// style needs it, an animated tilt is bending the media, or a cursor
    /// walkthrough has to be drawn.
    pub(crate) fn preview_needs_compositor(&self) -> bool {
        self.scene_style().needs_composited_preview()
            || (self.animation_active
                && (self.has_walkthrough()
                    || !self
                        .video_viewport_timeline
                        .frame_at(self.video_position)
                        .tilt
                        .is_zero()))
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
        let (_, projection) =
            self.preview_projection(f32::from(canvas.size.width), f32::from(canvas.size.height));
        let local_x = f32::from(position.x - canvas.origin.x) as f64;
        let local_y = f32::from(position.y - canvas.origin.y) as f64;
        let (u, v) = projection.unproject(local_x, local_y);
        if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
            return None;
        }
        let viewport = if self.video_project.is_some() || self.animation_active {
            self.video_viewport_timeline.frame_at(self.video_position)
        } else {
            ViewportFrame::default()
        };
        let (left, top, visible) = crate::recording::viewport::visible_rect(viewport);
        Some(
            NormalizedPoint {
                x: left + u * visible,
                y: top + v * visible,
            }
            .clamped(),
        )
    }

    /// Adds a cursor-walkthrough stop and regenerates the synthetic cursor
    /// and its click zooms.
    pub(crate) fn add_walkthrough_stop(&mut self, point: NormalizedPoint) {
        self.walkthrough_stops.push(point);
        self.rebuild_walkthrough();
        self.toast = Some(
            format!(
                "Cursor stop {} added · Enter to finish",
                self.walkthrough_stops.len()
            )
            .into(),
        );
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
        if !self.camera_overlay.enabled || self.camera_decode_in_flight {
            return;
        }
        let source_time = self.video_clip_timeline.source_time_at(self.video_position);
        if self.camera_frame_rgba.is_some() && (self.camera_decoded_time - source_time).abs() < 0.12
        {
            return;
        }
        self.camera_decode_in_flight = true;
        self.camera_decode_token = self.camera_decode_token.wrapping_add(1);
        let token = self.camera_decode_token;
        let task = cx.background_executor().spawn(async move {
            crate::recording::video::decode_frame(&path, source_time, 640, 640)
                .ok()
                .and_then(|frame| RgbaImage::from_raw(frame.width, frame.height, frame.rgba))
        });
        cx.spawn(async move |weak, cx| {
            let frame = task.await;
            let _ = weak.update(cx, |this, cx| {
                this.camera_decode_in_flight = false;
                if this.camera_decode_token != token {
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
                        this.toast = Some("Camera clip added to the project".into());
                    }
                    Err(error) => {
                        this.toast = Some(format!("Could not add camera clip: {error}").into());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn remove_camera_clip(&mut self) {
        if let Some(path) = self.video_camera_path.take() {
            let _ = std::fs::remove_file(path);
        }
        self.camera_frame_rgba = None;
        if let Some(session) = self.video_project.as_ref() {
            if let Ok(mut manifest) = session.read_manifest() {
                manifest.includes_camera = false;
                let _ = session.write_manifest(&manifest);
            }
        }
        self.toast = Some("Camera clip removed".into());
    }

    pub(crate) fn camera_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let has_camera = self.video_camera_path.is_some();
        let overlay = self.camera_overlay;
        let position_index = WatermarkPosition::ALL
            .iter()
            .position(|position| *position == overlay.position)
            .unwrap_or(3);
        let shape_index = crate::recording::scene::CameraShape::ALL
            .iter()
            .position(|shape| *shape == overlay.shape)
            .unwrap_or(0);
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(Self::scene_section_title("Camera"))
            .when(!has_camera, |this| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(muted())
                        .child("Add a webcam clip recorded alongside this screen capture to show it as a picture-in-picture bubble."),
                )
                .child(self.small_button(
                    "camera-add",
                    "Add camera clip…",
                    self.export_progress.is_none(),
                    cx,
                    |this, cx| this.import_camera_clip(cx),
                ))
            })
            .when(has_camera, |this| {
                this.child(self.scene_toggle_row(
                    "camera-enabled",
                    "Show camera",
                    overlay.enabled,
                    cx,
                    |this| this.camera_overlay.enabled = !this.camera_overlay.enabled,
                ))
                .child(self.segmented(
                    "camera-position",
                    &["Top left", "Top right", "Bottom left", "Bottom right"],
                    position_index,
                    |this, index| this.camera_overlay.position = WatermarkPosition::ALL[index],
                    cx,
                ))
                .child(self.segmented(
                    "camera-shape",
                    &["Circle", "Rounded", "Square"],
                    shape_index,
                    |this, index| {
                        this.camera_overlay.shape = crate::recording::scene::CameraShape::ALL[index]
                    },
                    cx,
                ))
                .child(self.scene_slider_row(SceneSlider::CameraSize, cx))
                .child(self.scene_slider_row(SceneSlider::CameraMargin, cx))
                .child(self.scene_toggle_row(
                    "camera-mirror",
                    "Mirror",
                    overlay.mirror,
                    cx,
                    |this| this.camera_overlay.mirror = !this.camera_overlay.mirror,
                ))
                .child(self.scene_toggle_row(
                    "camera-shadow",
                    "Shadow",
                    overlay.shadow,
                    cx,
                    |this| this.camera_overlay.shadow = !this.camera_overlay.shadow,
                ))
                .child(self.small_button(
                    "camera-remove",
                    "Remove camera clip",
                    self.export_progress.is_none(),
                    cx,
                    |this, _| this.remove_camera_clip(),
                ))
            })
            .into_any_element()
    }

    /// Timeline lane showing the camera clip's presence.
    pub(crate) fn camera_lane(
        &self,
        timeline_scroll: f64,
        timeline_content_width: f64,
        progress: f64,
    ) -> Option<AnyElement> {
        let path = self.video_camera_path.as_ref()?;
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "camera".into());
        let enabled = self.camera_overlay.enabled;
        Some(
            div()
                .relative()
                .w_full()
                .h(px(22.0))
                .flex_none()
                .overflow_hidden()
                .rounded_lg()
                .bg(rgb(0xECEDF1))
                .child(
                    div()
                        .absolute()
                        .left(px(-(timeline_scroll as f32)))
                        .top(px(3.0))
                        .w(px(timeline_content_width as f32))
                        .h(px(16.0))
                        .rounded_md()
                        .bg(if enabled {
                            hsla(271.0 / 360.0, 0.6, 0.55, 1.0)
                        } else {
                            hsla(271.0 / 360.0, 0.2, 0.7, 1.0)
                        })
                        .flex()
                        .items_center()
                        .px_2()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(0xffffff))
                        .child(format!("Camera · {name}")),
                )
                .child(
                    div()
                        .absolute()
                        .left(px((timeline_content_width * progress - timeline_scroll)
                            as f32
                            - 1.0))
                        .top_0()
                        .w(px(2.0))
                        .h_full()
                        .bg(hsla(222.0 / 360.0, 0.2, 0.15, 0.7)),
                )
                .into_any_element(),
        )
    }

    /// True when GPUI can paint annotations directly over the preview (no
    /// authored transform and no animated tilt).
    pub(crate) fn annotations_paint_flat(&self) -> bool {
        self.scene_transform.is_identity()
            && (!self.animation_active
                || self
                    .video_viewport_timeline
                    .frame_at(self.video_position)
                    .tilt
                    .is_zero())
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
        let (_, projection) =
            self.preview_projection(f32::from(canvas.size.width), f32::from(canvas.size.height));
        let local_x = f32::from(position.x - canvas.origin.x) as f64;
        let local_y = f32::from(position.y - canvas.origin.y) as f64;
        let (u, v) = projection.unproject(local_x, local_y);
        let viewport = if self.video_project.is_some() || self.animation_active {
            self.video_viewport_timeline.frame_at(self.video_position)
        } else {
            ViewportFrame::default()
        };
        let (left, top, visible) = crate::recording::viewport::visible_rect(viewport);
        let media_x = left + u * visible;
        let media_y = top + v * visible;
        point(
            flat.origin.x + flat.size.width * media_x as f32,
            flat.origin.y + flat.size.height * media_y as f32,
        )
    }

    /// Geometry and projection of the media on a preview canvas of this size.
    pub(crate) fn preview_projection(
        &self,
        canvas_width: f32,
        canvas_height: f32,
    ) -> (SceneGeometry, MediaProjection) {
        let style = self.scene_style();
        let (source_width, source_height) = if self.video_project.is_some() {
            self.video_source_size
        } else {
            self.captured_dimensions.unwrap_or((1200, 720))
        };
        let geometry = SceneGeometry::layout(
            canvas_width as f64,
            canvas_height as f64,
            source_width as f64,
            source_height as f64,
            &style,
        );
        let viewport = if self.video_project.is_some() || self.animation_active {
            self.video_viewport_timeline.frame_at(self.video_position)
        } else {
            ViewportFrame::default()
        };
        let projection = geometry.projection(style.transform.with_tilt(viewport.tilt));
        (geometry, projection)
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
        let (_, projection) = self.preview_projection(canvas_size.0, canvas_size.1);
        let inside = projection.contains(local_x as f64, local_y as f64);
        // A selected motion region turns clicks into focus picks.
        if self.video_selected_zoom_cue.is_some() && inside {
            let (u, v) = projection.unproject(local_x as f64, local_y as f64);
            self.pin_focus_at_media(u, v, cx);
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
                self.toast = Some("Transform reset".into());
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

    /// Sets the selected region's focus (or pan end) from media coordinates.
    pub(crate) fn pin_focus_at_media(&mut self, u: f64, v: f64, cx: &mut Context<Self>) {
        if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
            return;
        }
        let frame = self.video_viewport_timeline.frame_at(self.video_position);
        let (left, top, visible) = crate::recording::viewport::visible_rect(frame);
        let target = NormalizedPoint {
            x: left + u * visible,
            y: top + v * visible,
        }
        .clamped();
        match self.motion_pick {
            crate::motion_ui::MotionPick::Focus => {
                self.mutate_selected_zoom_cue(cx, |cue| {
                    cue.anchor_mode = crate::recording::viewport::ZoomAnchorMode::PinnedAnchor;
                    cue.pinned_point = target;
                });
                self.toast = Some("Focus point set".into());
            }
            crate::motion_ui::MotionPick::PanEnd => {
                self.mutate_selected_zoom_cue(cx, |cue| {
                    cue.anchor_mode = crate::recording::viewport::ZoomAnchorMode::PinnedAnchor;
                    cue.pan_to = Some(target);
                });
                self.toast = Some("Pan destination set".into());
            }
        }
        cx.notify();
    }

    /// The composited scene canvas with selection handles, used by the
    /// recording editor and by transformed screenshots.
    pub(crate) fn scene_canvas(
        &mut self,
        canvas_width: Pixels,
        canvas_height: Pixels,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let image = self.scene_preview_image(canvas_width, canvas_height);
        let bounds_store = self.scene_canvas_bounds.clone();
        let media_bounds_store = self.video_media_bounds.clone();
        let (_, projection) =
            self.preview_projection(f32::from(canvas_width), f32::from(canvas_height));
        let show_handles = self.scene_selection == SceneSelection::Media;
        let picking = self.video_selected_zoom_cue.is_some();
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
                this.child(img(image).absolute().inset_0().size_full())
            })
            .child(
                canvas(
                    move |bounds, _, _| {
                        if let Ok(mut stored) = bounds_store.lock() {
                            *stored = Some(bounds);
                        }
                        if let Ok(mut stored) = media_bounds_store.lock() {
                            *stored = Some(Bounds {
                                origin: point(
                                    bounds.origin.x + px(projection.bounds.x as f32),
                                    bounds.origin.y + px(projection.bounds.y as f32),
                                ),
                                size: size(
                                    px(projection.bounds.width as f32),
                                    px(projection.bounds.height as f32),
                                ),
                            });
                        }
                        bounds
                    },
                    move |_, bounds, window, _| {
                        if !show_handles {
                            return;
                        }
                        paint_selection_handles(&projection, bounds, window);
                    },
                )
                .absolute()
                .size_full(),
            )
            .when(show_handles || picking, |this| {
                this.cursor(if picking {
                    CursorStyle::Crosshair
                } else {
                    CursorStyle::OpenHand
                })
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    let Some(bounds) = this.scene_canvas_bounds.lock().ok().and_then(|b| *b) else {
                        return;
                    };
                    this.scene_pointer_down(
                        event.position,
                        bounds,
                        &event.modifiers,
                        event.click_count,
                        cx,
                    );
                    cx.notify();
                }),
            )
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

    pub(crate) fn inspector_level_picker(&self, cx: &mut Context<Self>) -> AnyElement {
        self.segmented(
            "inspector-level",
            &["Quick", "Customize", "Advanced"],
            self.inspector_level,
            |this, value| this.inspector_level = value,
            cx,
        )
        .into_any_element()
    }

    /// Transform panel for the selected media surface.
    pub(crate) fn transform_inspector(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.scene_selection != SceneSelection::Media {
            return None;
        }
        let advanced = self.inspector_level >= 2;
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
                            div()
                                .text_sm()
                                .font_weight(FontWeight::BOLD)
                                .child("Transform"),
                        )
                        .child(self.small_button(
                            "transform-done",
                            "Done",
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
                .child(self.scene_slider_row(SceneSlider::RotationX, cx))
                .child(self.scene_slider_row(SceneSlider::RotationY, cx))
                .child(self.scene_slider_row(SceneSlider::RotationZ, cx))
                .child(self.scene_slider_row(SceneSlider::Perspective, cx))
                .when(advanced, |this| {
                    this.child(self.scene_slider_row(SceneSlider::AnchorX, cx))
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

    pub(crate) fn effects_section(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(Self::scene_section_title("Background effects"))
            .child(self.scene_slider_row(SceneSlider::Blur, cx))
            .child(self.scene_slider_row(SceneSlider::Noise, cx))
            .child(self.scene_slider_row(SceneSlider::Vignette, cx))
            .into_any_element()
    }

    pub(crate) fn watermark_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let enabled = self.watermark_enabled;
        let editing = self.watermark_editing;
        let text = if self.watermark.text.is_empty() {
            if editing {
                String::new()
            } else {
                "Click to type a watermark".to_string()
            }
        } else {
            self.watermark.text.clone()
        };
        let position_index = WatermarkPosition::ALL
            .iter()
            .position(|position| *position == self.watermark.position)
            .unwrap_or(3);
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .mt_2()
                    .pt_3()
                    .border_t_1()
                    .border_color(line())
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .child("Watermark"),
                    )
                    .child(
                        div()
                            .id("watermark-toggle")
                            .cursor_pointer()
                            .child(self.toggle(enabled))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.watermark_enabled = !this.watermark_enabled;
                                if this.watermark_enabled && this.watermark.text.is_empty() {
                                    this.watermark_editing = true;
                                }
                                cx.notify();
                            })),
                    ),
            )
            .when(enabled, |this| {
                this.child(
                    div()
                        .id("watermark-text")
                        .h(px(36.0))
                        .px_3()
                        .rounded_lg()
                        .border_1()
                        .border_color(if editing { blue() } else { line() })
                        .bg(rgb(0xffffff))
                        .flex()
                        .items_center()
                        .text_sm()
                        .text_color(if self.watermark.text.is_empty() && !editing {
                            muted()
                        } else {
                            ink()
                        })
                        .cursor(CursorStyle::IBeam)
                        .child(format!("{text}{}", if editing { "|" } else { "" }))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.watermark_editing = true;
                            cx.notify();
                        })),
                )
                .child(self.segmented(
                    "watermark-position",
                    &["Top left", "Top right", "Bottom left", "Bottom right"],
                    position_index,
                    |this, index| this.watermark.position = WatermarkPosition::ALL[index],
                    cx,
                ))
                .child(self.scene_slider_row(SceneSlider::WatermarkSize, cx))
                .child(self.scene_slider_row(SceneSlider::WatermarkOpacity, cx))
            })
            .into_any_element()
    }

    /// Keyboard input for the watermark text field. Returns true when the
    /// key was consumed.
    pub(crate) fn handle_watermark_key(&mut self, event: &KeyDownEvent) -> bool {
        if !self.watermark_editing {
            return false;
        }
        match event.keystroke.key.as_str() {
            "enter" | "escape" => self.watermark_editing = false,
            "backspace" => {
                self.watermark.text.pop();
            }
            _ => {
                if !event.keystroke.modifiers.control
                    && !event.keystroke.modifiers.platform
                    && !event.keystroke.modifiers.alt
                {
                    if let Some(text) = event.keystroke.key_char.as_ref() {
                        if self.watermark.text.chars().count() < 60 {
                            self.watermark.text.push_str(text);
                        }
                    }
                }
            }
        }
        true
    }

    pub(crate) fn pointer_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let style = self.pointer_style;
        let selected_press = self.video_selected_press;
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(Self::scene_section_title("Pointer"))
            .child(self.scene_toggle_row(
                "pointer-visible",
                "Show cursor",
                style.visible,
                cx,
                |this| this.pointer_style.visible = !this.pointer_style.visible,
            ))
            .when(style.visible, |this| {
                this.child(self.scene_slider_row(SceneSlider::PointerScale, cx))
                    .child(self.scene_toggle_row(
                        "pointer-shadow",
                        "Cursor shadow",
                        style.shadow,
                        cx,
                        |this| this.pointer_style.shadow = !this.pointer_style.shadow,
                    ))
                    .child(self.scene_toggle_row(
                        "pointer-hide-idle",
                        "Hide when idle",
                        style.hide_when_idle,
                        cx,
                        |this| {
                            this.pointer_style.hide_when_idle = !this.pointer_style.hide_when_idle
                        },
                    ))
            })
            .child(self.scene_toggle_row(
                "pointer-clicks",
                "Click effects",
                style.click_effects,
                cx,
                |this| this.pointer_style.click_effects = !this.pointer_style.click_effects,
            ))
            .when(style.click_effects, |this| {
                this.child(div().flex().items_center().gap_2().children(
                    CLICK_COLORS.into_iter().enumerate().map(|(index, color)| {
                        let selected = style.click_color == color;
                        div()
                            .id(("click-color", index))
                            .size(px(24.0))
                            .rounded_full()
                            .bg(rgb(color))
                            .border_2()
                            .border_color(if selected {
                                ink()
                            } else {
                                Hsla::from(rgb(0xd4d5d8))
                            })
                            .cursor_pointer()
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
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted())
                                .child(format!("Click at {time:.2}s selected")),
                        )
                        .child(self.small_button(
                            "pointer-remove-click",
                            "Remove click",
                            true,
                            cx,
                            |this, cx| this.remove_selected_press(cx),
                        )),
                )
            })
            .child(
                div()
                    .text_xs()
                    .text_color(muted())
                    .child("Click markers on the ruler select individual clicks."),
            )
            .into_any_element()
    }

    pub(crate) fn audio_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let has_audio = !self.video_audio_levels.is_empty();
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(Self::scene_section_title("Audio"))
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
                    "Include audio in export",
                    !self.video_audio_muted,
                    cx,
                    |this| this.video_audio_muted = !this.video_audio_muted,
                ))
            })
            .into_any_element()
    }

    pub(crate) fn export_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let (source_width, source_height) = if self.video_project.is_some() {
            self.video_source_size
        } else {
            self.captured_dimensions.unwrap_or((1200, 720))
        };
        let style = self.scene_style();
        let height = self.export_resolution.canvas_height(source_height);
        let (width, height) = style.export_canvas_size(source_width, source_height, height);
        let format = self.export_format;
        let frame_rate = self.export_frame_rate;
        let duration = self.video_duration.max(0.0);
        let estimate = estimate_size_bytes(format, width, height, frame_rate, duration);
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
        let advanced = self.inspector_level >= 2;
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(Self::scene_section_title("Export"))
            .child(self.segmented(
                "export-section-format",
                &["MP4", "WebM", "GIF"],
                format_index,
                |this, index| {
                    this.export_format = ExportFormat::ALL[index];
                    this.export_frame_rate = this.export_format.default_frame_rate();
                },
                cx,
            ))
            .child(self.segmented(
                "export-resolution",
                &["Original", "720p", "1080p", "1440p", "4K"],
                resolution_index,
                |this, index| this.export_resolution = ExportResolution::ALL[index],
                cx,
            ))
            .when(advanced, |this| {
                this.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted())
                                .w(px(72.0))
                                .child("Frame rate"),
                        )
                        .child(div().flex_1().child(self.segmented(
                            "export-frame-rate",
                            &["30 fps", "60 fps"],
                            fps_index,
                            |this, index| {
                                this.export_frame_rate = if index == 1 { 60.0 } else { 30.0 }
                            },
                            cx,
                        ))),
                )
            })
            .when(format == ExportFormat::Gif, |this| {
                this.child(self.scene_toggle_row(
                    "export-loop",
                    "Loop forever",
                    self.export_loop,
                    cx,
                    |this| this.export_loop = !this.export_loop,
                ))
            })
            .child(div().text_xs().text_color(muted()).child(format!(
                "{width} × {height} · {frame_rate:.0} fps · {:.1}s · about {}",
                duration,
                format_size(estimate)
            )))
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
                    .justify_between()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .child("My presets"),
                    )
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
                            .bg(rgb(0xf0f0f1))
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
                                    .hover(|style| style.bg(rgb(0xe4e4e7)))
                                    .child(name)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.apply_saved_preset(index);
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .id(("preset-delete", index))
                                    .w(px(24.0))
                                    .h(px(30.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_xs()
                                    .text_color(muted())
                                    .cursor_pointer()
                                    .hover(|style| style.bg(rgb(0xfee2e2)).text_color(rgb(0xb91c1c)))
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
                            .bg(if selected {
                                blue()
                            } else {
                                Hsla::from(rgb(0xf0f0f1))
                            })
                            .text_color(if selected {
                                Hsla::from(rgb(0xffffff))
                            } else {
                                ink()
                            })
                            .cursor_pointer()
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
            Err(error) => self.toast = Some(format!("Could not save preset: {error}").into()),
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
        self.toast = Some(format!("{} applied", preset.name).into());
    }

    pub(crate) fn delete_saved_preset(&mut self, index: usize) {
        if let Some(preset) = self.preset_library.remove(index) {
            if let Err(error) = self.preset_library.save() {
                self.toast = Some(format!("Could not update presets: {error}").into());
            } else {
                self.toast = Some(format!("Deleted {}", preset.name).into());
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
        self.toast = Some("Click removed from the pointer track".into());
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
            markers.push(
                div()
                    .id(("press-marker", index))
                    .absolute()
                    .left(px(x as f32 - 4.0))
                    .bottom(px(1.0))
                    .size(px(8.0))
                    .rounded_full()
                    .bg(if selected {
                        ink()
                    } else {
                        Hsla::from(rgb(self.pointer_style.click_color))
                    })
                    .border_1()
                    .border_color(hsla(0.0, 0.0, 1.0, 0.9))
                    .cursor_pointer()
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
        let duration = self.video_duration.max(f64::EPSILON);
        let muted_lane = self.video_audio_muted;
        let bucket_count = self.video_audio_levels.len();
        let mut bars: Vec<AnyElement> = Vec::new();
        let bar_width = (timeline_content_width / 300.0).clamp(1.5, 6.0);
        let mut x = 0.0;
        while x < timeline_content_width {
            let editor_time = x / timeline_content_width * duration;
            let source_time = self.video_clip_timeline.source_time_at(editor_time);
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
                .bg(rgb(0xECEDF1))
                .child(
                    div()
                        .absolute()
                        .left(px(-(timeline_scroll as f32)))
                        .top_0()
                        .w(px(timeline_content_width as f32))
                        .h_full()
                        .children(bars)
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

    /// Thumbnails for one clip, positioned by source time.
    pub(crate) fn clip_thumbnails(
        &self,
        source_start: f64,
        source_end: f64,
        speed: f64,
        clip_width: f32,
        lane_height: f32,
    ) -> Vec<AnyElement> {
        if self.video_thumbnails.is_empty() || self.video_source_duration <= 0.0 {
            return Vec::new();
        }
        let count = self.video_thumbnails.len();
        let interval = self.video_source_duration / count as f64;
        let clip_duration = ((source_end - source_start) / speed.max(0.01)).max(f64::EPSILON);
        let mut elements = Vec::new();
        for (index, thumbnail) in self.video_thumbnails.iter().enumerate() {
            let time = index as f64 * interval;
            if time < source_start || time >= source_end {
                continue;
            }
            let x = ((time - source_start) / speed.max(0.01)) / clip_duration * clip_width as f64;
            let thumb_height = (lane_height - 6.0).max(8.0);
            let thumb_width = thumb_height * thumbnail.size(0).width.0 as f32
                / thumbnail.size(0).height.0.max(1) as f32;
            if x as f32 + thumb_width > clip_width {
                continue;
            }
            elements.push(
                img(thumbnail.clone())
                    .absolute()
                    .left(px(x as f32))
                    .top(px(3.0))
                    .w(px(thumb_width))
                    .h(px(thumb_height))
                    .rounded_sm()
                    .opacity(0.85)
                    .into_any_element(),
            );
        }
        elements
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
            return None;
        }
        let duration = self.video_duration.max(f64::EPSILON);
        let selected = self.selected_annotation;
        let mut regions: Vec<AnyElement> = Vec::new();
        for (index, mark) in self.annotations.iter().enumerate() {
            let timing = mark.timing.unwrap_or(AnnotationTiming {
                start: 0.0,
                end: duration,
                ..AnnotationTiming::default()
            });
            let left = timing.start / duration * timeline_content_width;
            let width = ((timing.end - timing.start) / duration * timeline_content_width).max(20.0);
            let is_selected = selected == Some(index);
            let whole = mark.timing.is_none();
            let label = format!(
                "{}{}",
                mark.tool.label(),
                if whole { " · whole scene" } else { "" }
            );
            regions.push(
                div()
                    .id(("annotation-region", index))
                    .absolute()
                    .left(px(left as f32))
                    .top(px(3.0))
                    .w(px(width as f32))
                    .h(px(22.0))
                    .rounded_md()
                    .border_2()
                    .border_color(if is_selected {
                        hsla(222.0 / 360.0, 0.2, 0.15, 1.0)
                    } else {
                        hsla(0.0, 0.0, 0.0, 0.0)
                    })
                    .bg(if is_selected {
                        hsla(188.0 / 360.0, 0.7, 0.36, 1.0)
                    } else {
                        hsla(188.0 / 360.0, 0.6, 0.45, 1.0)
                    })
                    .when(whole, |this| this.opacity(0.7))
                    .text_color(rgb(0xffffff))
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .flex()
                    .items_center()
                    .justify_center()
                    .overflow_hidden()
                    .cursor(CursorStyle::PointingHand)
                    .when(width >= 60.0, |this| this.child(label))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.select_annotation_for_timing(index);
                            this.begin_annotation_drag(
                                index,
                                AnnotationDragKind::Move,
                                event.position.x,
                            );
                            cx.notify();
                        }),
                    )
                    .when(is_selected && !whole, |this| {
                        this.child(
                            div()
                                .id(("annotation-leading", index))
                                .absolute()
                                .left_0()
                                .top_0()
                                .w(px(8.0))
                                .h_full()
                                .rounded_l_md()
                                .bg(hsla(0.0, 0.0, 1.0, 0.38))
                                .cursor(CursorStyle::ResizeLeftRight)
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                        cx.stop_propagation();
                                        this.begin_annotation_drag(
                                            index,
                                            AnnotationDragKind::Leading,
                                            event.position.x,
                                        );
                                        cx.notify();
                                    }),
                                ),
                        )
                        .child(
                            div()
                                .id(("annotation-trailing", index))
                                .absolute()
                                .right_0()
                                .top_0()
                                .w(px(8.0))
                                .h_full()
                                .rounded_r_md()
                                .bg(hsla(0.0, 0.0, 1.0, 0.38))
                                .cursor(CursorStyle::ResizeLeftRight)
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                        cx.stop_propagation();
                                        this.begin_annotation_drag(
                                            index,
                                            AnnotationDragKind::Trailing,
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
        Some(
            div()
                .id("annotation-track")
                .relative()
                .w_full()
                .h(px(28.0))
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
                        .children(regions)
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
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, event: &MouseDownEvent, _, cx| {
                        this.pause_video_playback();
                        if let Some(target) = this.motion_timeline_time_at(event.position.x) {
                            this.video_position = target;
                            this.video_seek_drag = Some((event.position.x, target));
                        }
                        cx.notify();
                    }),
                )
                .into_any_element(),
        )
    }

    fn select_annotation_for_timing(&mut self, index: usize) {
        self.stop_editing_text();
        self.selected_annotation = Some(index);
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
        self.pause_video_playback();
        self.record_annotation_undo();
        self.annotation_drag = Some(AnnotationDrag {
            index,
            kind,
            start_x,
            original,
            seconds_per_pixel: duration
                / (self.video_timeline_viewport_width() * self.video_timeline_zoom).max(1.0),
        });
    }

    pub(crate) fn update_annotation_drag(&mut self, pointer_x: Pixels) -> bool {
        let Some(drag) = self.annotation_drag else {
            return false;
        };
        let duration = self.video_duration;
        let delta = f64::from((pointer_x - drag.start_x) / px(1.0)) * drag.seconds_per_pixel;
        if delta.abs() < 1e-9 {
            return false;
        }
        let Some(mark) = self.annotations.get_mut(drag.index) else {
            return false;
        };
        let mut timing = drag.original;
        match drag.kind {
            AnnotationDragKind::Move => {
                let length = timing.duration();
                timing.start = (timing.start + delta).clamp(0.0, (duration - length).max(0.0));
                timing.end = timing.start + length;
            }
            AnnotationDragKind::Leading => {
                timing.start = (timing.start + delta)
                    .clamp(0.0, timing.end - AnnotationTiming::MINIMUM_DURATION);
            }
            AnnotationDragKind::Trailing => {
                timing.end = (timing.end + delta)
                    .clamp(timing.start + AnnotationTiming::MINIMUM_DURATION, duration);
            }
        }
        mark.timing = Some(timing.clamped(duration));
        true
    }

    pub(crate) fn end_annotation_drag(&mut self) -> bool {
        self.annotation_drag.take().is_some()
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
    }

    /// Timing panel for the selected annotation in an animated screenshot.
    pub(crate) fn annotation_timing_inspector(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.animation_active {
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
        let title = format!("{} timing", mark.tool.label());
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
                                .child(div().size(px(10.0)).rounded_full().bg(hsla(
                                    188.0 / 360.0,
                                    0.6,
                                    0.45,
                                    1.0,
                                )))
                                .child(div().text_sm().font_weight(FontWeight::BOLD).child(title)),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted())
                                .child(format!("{:.1}s – {:.1}s", timing.start, timing.end)),
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
                            .child(self.small_button(
                                "timing-start-earlier",
                                "−0.25s",
                                true,
                                cx,
                                |this, _| {
                                    this.edit_selected_timing(|timing, _| timing.start -= 0.25)
                                },
                            ))
                            .child(self.small_button(
                                "timing-start-later",
                                "+0.25s",
                                true,
                                cx,
                                |this, _| {
                                    this.edit_selected_timing(|timing, _| {
                                        timing.start = (timing.start + 0.25)
                                            .min(timing.end - AnnotationTiming::MINIMUM_DURATION)
                                    })
                                },
                            ))
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
                            .child(self.small_button(
                                "timing-end-earlier",
                                "−0.25s",
                                true,
                                cx,
                                |this, _| {
                                    this.edit_selected_timing(|timing, _| {
                                        timing.end = (timing.end - 0.25)
                                            .max(timing.start + AnnotationTiming::MINIMUM_DURATION)
                                    })
                                },
                            ))
                            .child(self.small_button(
                                "timing-end-later",
                                "+0.25s",
                                true,
                                cx,
                                |this, _| {
                                    this.edit_selected_timing(|timing, duration| {
                                        timing.end = (timing.end + 0.25).min(duration)
                                    })
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
                    .child(div().text_xs().text_color(muted()).child("Entrance"))
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
                        &["Slide in", "Draw", "Type"],
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
                    .child(div().text_xs().text_color(muted()).child("Exit"))
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
    ) -> Option<crate::recording::export::OverlaySource> {
        if self.annotations.is_empty() {
            return None;
        }
        let (width, height) = self.captured_dimensions?;
        let marks = self.annotations.clone();
        let mut cache: Option<(u64, Arc<RgbaImage>)> = None;
        Some(Box::new(move |time: f64| {
            let active = timed::active_marks(&marks, time);
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

/// Renders annotation marks (no capture) to a transparent layer.
pub(crate) fn render_annotation_layer(
    marks: &[AnnotationMark],
    width: u32,
    height: u32,
) -> Option<RgbaImage> {
    let stroke_scale = width.min(height) as f32 / 800.0;
    let mut svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}"><g>"#
    );
    svg.push_str(&annotations_svg(
        marks,
        0.0,
        0.0,
        width,
        height,
        stroke_scale,
    ));
    svg.push_str("</svg>");
    render_svg_layer(&svg, width, height).ok()
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
        window.paint_path(path, hsla(211.0 / 360.0, 1.0, 0.55, 0.9));
    }
    for corner in corners {
        window.paint_quad(quad(
            Bounds {
                origin: point(corner.x - px(6.0), corner.y - px(6.0)),
                size: size(px(12.0), px(12.0)),
            },
            px(3.0),
            rgb(0xffffff),
            px(2.0),
            rgb(0x2997ff),
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
        let Some(session) = self.video_project.as_ref() else {
            return;
        };
        let path = session.screen_path();
        self.video_extras_token = self.video_extras_token.wrapping_add(1);
        let token = self.video_extras_token;
        let task = cx.background_executor().spawn(async move {
            (
                crate::recording::video::audio_levels(&path, 240).unwrap_or_default(),
                crate::recording::video::decode_thumbnails(&path, 24, 28).unwrap_or_default(),
            )
        });
        cx.spawn(async move |weak, cx| {
            let (levels, thumbnails) = task.await;
            let _ = weak.update(cx, |this, cx| {
                if this.video_extras_token != token || this.video_project.is_none() {
                    return;
                }
                this.video_audio_levels = levels;
                this.video_thumbnails = thumbnails.into_iter().map(cached_render_image).collect();
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
        let frame = compositor.compose(crate::recording::scene::FrameInput {
            source: &image,
            overlay: None,
            viewport: ViewportFrame::default(),
            pointer: None,
            camera: None,
        });
        frame
            .save(destination)
            .map_err(|error| format!("Could not save PNG: {error}"))
    }

    /// The scene panel of the recording editor at the current inspector level.
    pub(crate) fn video_scene_panel(&self, cx: &mut Context<Self>) -> AnyElement {
        let level = self.inspector_level;
        let mut panel = div()
            .flex()
            .flex_col()
            .gap_3()
            .child(self.inspector_level_picker(cx));
        if level == 0 {
            panel = panel
                .child(self.preset_library_section(cx))
                .child(div().text_sm().font_weight(FontWeight::BOLD).child("Looks"))
                .child(self.quick_presets_row(cx))
                .child(div().text_xs().text_color(muted()).child("Aspect ratio"))
                .child(self.segmented(
                    "video-aspect-ratio",
                    &["Auto", "1:1", "4:3", "3:2", "16:9"],
                    self.aspect_ratio,
                    |this, value| this.aspect_ratio = value,
                    cx,
                ))
                .child(self.scene_slider_row(SceneSlider::DefaultZoom, cx))
                .child(self.motion_overview_section(cx))
                .child(self.export_section(cx));
            return panel.into_any_element();
        }
        panel = panel
            .child(self.motion_overview_section(cx))
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .child("Background"),
            )
            .child(self.segmented(
                "video-fill-type",
                &["Color", "Gradient", "Wallpaper"],
                self.wallpaper_tab,
                |this, value| this.wallpaper_tab = value,
                cx,
            ))
            .when(self.wallpaper_tab == 2, |this| {
                this.child(self.segmented(
                    "video-fill-library",
                    &["Recent", "UIHSSN", "Fayazara"],
                    self.library_tab,
                    |this, value| this.library_tab = value,
                    cx,
                ))
            })
            .child(self.fill_picker(cx))
            .child(self.effects_section(cx))
            .child(Self::scene_section_title("Layout"))
            .child(self.slider_row(
                "Padding",
                self.padding,
                "%",
                |this, value| this.padding = value,
                cx,
            ))
            .child(self.slider_row(
                "Corners",
                self.corners,
                "%",
                |this, value| this.corners = value,
                cx,
            ))
            .child(self.slider_row(
                "Shadow",
                self.shadow,
                "%",
                |this, value| this.shadow = value,
                cx,
            ))
            .child(div().text_xs().text_color(muted()).child("Shadow style"))
            .child(self.segmented(
                "video-shadow-style",
                &["Soft", "Long", "Glow", "Crisp"],
                self.shadow_style,
                |this, value| this.shadow_style = value,
                cx,
            ))
            .child(div().text_xs().text_color(muted()).child("Aspect ratio"))
            .child(self.segmented(
                "video-aspect-ratio",
                &["Auto", "1:1", "4:3", "3:2", "16:9"],
                self.aspect_ratio,
                |this, value| this.aspect_ratio = value,
                cx,
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .mt_2()
                    .pt_3()
                    .border_t_1()
                    .border_color(line())
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .child("Border"),
                    )
                    .child(
                        div()
                            .id("video-border-toggle")
                            .cursor_pointer()
                            .child(self.toggle(self.border))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.border = !this.border;
                                cx.notify();
                            })),
                    ),
            )
            .when(self.border, |this| {
                this.child(
                    div().flex().items_center().gap_2().children(
                        crate::motion_ui::BORDER_COLORS
                            .iter()
                            .enumerate()
                            .map(|(index, color)| {
                                let selected = self.border_color == index;
                                div()
                                    .id(("video-border-color", index))
                                    .size(px(24.0))
                                    .rounded_full()
                                    .bg(rgb(*color))
                                    .border_2()
                                    .border_color(if selected {
                                        ink()
                                    } else {
                                        Hsla::from(rgb(0xd4d5d8))
                                    })
                                    .cursor_pointer()
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.border_color = index;
                                        cx.notify();
                                    }))
                            }),
                    ),
                )
                .child(self.slider_row(
                    "Thickness",
                    self.border_thickness,
                    "%",
                    |this, value| this.border_thickness = value,
                    cx,
                ))
                .child(self.slider_row(
                    "Opacity",
                    self.border_opacity,
                    "%",
                    |this, value| this.border_opacity = value,
                    cx,
                ))
            })
            .child(self.watermark_section(cx))
            .child(self.pointer_section(cx))
            .child(self.camera_section(cx))
            .child(self.audio_section(cx))
            .child(self.export_section(cx))
            .child(div().mt_2().pt_3().border_t_1().border_color(line()))
            .child(self.preset_library_section(cx));
        panel.into_any_element()
    }
}
