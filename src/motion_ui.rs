//! Motion editing: the orange motion lane, the selection-aware motion
//! inspector, animated screenshots, and scene export. Shared by the
//! recording editor and the screenshot editor so "static or motion" is one
//! workflow.

use gpui::{
    canvas, div, hsla, prelude::*, px, rgb, svg, AnyElement, Context, CursorStyle, FontWeight,
    MouseButton, MouseDownEvent, Pixels, Timer,
};
use image::RgbaImage;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use crate::{
    blue, ink, line, muted,
    recording::{
        clips::RecordingClipTimeline,
        export::{export_scene, ExportFormat, ExportProgress, SceneExportRequest, SceneSource},
        model::NormalizedPoint,
        scene::{SceneBackground, SceneStyle},
        viewport::{
            synthesize_zoom_cues, MotionPreset, MotionStyle, ViewportTimeline, ZoomAnchorMode,
            ZoomCue,
        },
    },
    xml_escape, SliderDrag, Studio, VideoEditSnapshot, VideoZoomDragKind, GRADIENT_BACKGROUNDS,
    SOLID_BACKGROUNDS,
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

fn orange(selected: bool) -> gpui::Hsla {
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
            padding: self.padding,
            corners: self.corners,
            shadow: self.shadow,
            shadow_style: self.shadow_style,
            border: self.border,
            border_thickness: self.border_thickness,
            border_color: BORDER_COLORS[self.border_color.min(BORDER_COLORS.len() - 1)],
            border_opacity: self.border_opacity,
            aspect: (self.aspect_ratio != 0).then(|| self.selected_canvas_ratio() as f64),
            background_blur: self.background_blur,
            background_noise: self.background_noise,
            vignette: self.vignette,
            transform: self.scene_transform.clamped(),
            watermark: (self.watermark_enabled && !self.watermark.text.trim().is_empty())
                .then(|| self.watermark.clone()),
            pointer: self.pointer_style,
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
    fn scene_has_pointer(&self) -> bool {
        self.video_project.is_some() && !self.video_pointer_timeline.is_empty()
    }

    // ------------------------------------------------------------------
    // Motion regions
    // ------------------------------------------------------------------

    /// Editor time under a pointer x position on any timeline lane.
    pub(crate) fn motion_timeline_time_at(&self, x: Pixels) -> Option<f64> {
        let bounds = (*self.video_timeline_bounds.lock().ok()?)?;
        let local = f64::from((x - bounds.origin.x) / px(1.0));
        let content = (self.video_timeline_viewport_width() * self.video_timeline_zoom).max(1.0);
        Some(((self.video_timeline_scroll + local) / content).clamp(0.0, 1.0) * self.video_duration)
    }

    /// Adds a default motion region around `editor_time` and selects it.
    pub(crate) fn add_motion_region_at(&mut self, editor_time: f64, cx: &mut Context<Self>) {
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
        if self.scene_has_pointer() {
            cue.anchor_mode = ZoomAnchorMode::PointerAnchor;
            cue.bounds_bias = 0.25;
        }
        let original = self.video_zoom_cues.clone();
        self.video_selected_zoom_cue = Some(cue.id);
        self.video_selected_clip = None;
        self.motion_pick = MotionPick::Focus;
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
    pub(crate) fn regenerate_motion_from_clicks(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.video_project.as_ref() else {
            return;
        };
        if self.video_edit_busy {
            return;
        }
        let _ = session;
        let capture = self.filtered_pointer_capture();
        let mut generated = synthesize_zoom_cues(&capture, self.video_source_duration);
        for cue in &mut generated {
            cue.zoom = self
                .default_motion_zoom
                .clamp(ZoomCue::MINIMUM_ZOOM, ZoomCue::MAXIMUM_ZOOM);
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
        self.toast = Some("Motion regenerated from clicks".into());
        cx.notify();
    }

    fn selected_region(&self) -> Option<ZoomCue> {
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
        let mut segment_slot_start = 0.0;
        for (segment_index, segment) in self.video_clip_timeline.segments.iter().enumerate() {
            let segment_editor_start = segment_slot_start + segment.gap_before;
            for cue in self.video_zoom_cues.iter() {
                let overlap_start = cue.start.max(segment.source_start);
                let overlap_end = cue.end.min(segment.source_end);
                if overlap_end - overlap_start <= f64::EPSILON {
                    continue;
                }
                let cue_id = cue.id;
                let editor_start =
                    segment_editor_start + (overlap_start - segment.source_start) / segment.speed;
                let editor_end =
                    segment_editor_start + (overlap_end - segment.source_start) / segment.speed;
                let left = editor_start / timeline_duration * timeline_content_width;
                let width = ((editor_end - editor_start) / timeline_duration
                    * timeline_content_width)
                    .max(24.0);
                let selected = selected_zoom_cue == Some(cue_id);
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
                        .top(px(3.0))
                        .w(px(width as f32))
                        .h(px(24.0))
                        .rounded_md()
                        .border_2()
                        .border_color(if selected {
                            hsla(222.0 / 360.0, 0.2, 0.15, 1.0)
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
                        .when(width >= 44.0, |this| this.child(label))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.video_selected_zoom_cue = Some(cue_id);
                            this.video_selected_clip = None;
                            this.motion_pick = MotionPick::Focus;
                            this.seek_video(editor_start, cx);
                            cx.notify();
                        }))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                cx.stop_propagation();
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
                        .when(selected, |this| {
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
            segment_slot_start += segment.slot_duration();
        }
        lane
    }

    /// The motion lane container. A single click on empty lane moves the
    /// playhead; a double-click creates a region there.
    pub(crate) fn motion_track(
        &self,
        timeline_scroll: f64,
        timeline_content_width: f64,
        progress: f64,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let timeline_duration = self.video_duration.max(f64::EPSILON);
        let lane = self.motion_lane_elements(timeline_duration, timeline_content_width, cx);
        let empty = self.video_zoom_cues.is_empty();
        div()
            .id("motion-track")
            .relative()
            .w_full()
            .h(px(30.0))
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
            .when(empty, |this| {
                this.child(
                    div()
                        .absolute()
                        .left(px(10.0))
                        .top(px(7.0))
                        .text_xs()
                        .text_color(muted())
                        .child("Double-click to add a motion region"),
                )
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    this.pause_video_playback();
                    this.video_trim_drag = None;
                    this.video_zoom_drag = None;
                    let Some(target) = this.motion_timeline_time_at(event.position.x) else {
                        return;
                    };
                    if event.click_count >= 2 {
                        this.video_seek_drag = None;
                        this.seek_video(target, cx);
                        this.add_motion_region_at(target, cx);
                    } else {
                        this.video_position = target;
                        this.video_seek_drag = Some((event.position.x, target));
                    }
                    cx.notify();
                }),
            )
            .into_any_element()
    }

    // ------------------------------------------------------------------
    // Inspector
    // ------------------------------------------------------------------

    fn inspector_label(text: &'static str) -> AnyElement {
        div()
            .text_xs()
            .text_color(muted())
            .child(text)
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
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .child(
                div()
                    .id("motion-zoom-slider")
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
                            this.begin_motion_zoom_slider(event.position.x, value);
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
                            .bg(hsla(24.0 / 360.0, 0.9, 0.8, 0.5)),
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
                            .child("Zoom"),
                    ),
            )
            .child(
                div()
                    .id("motion-zoom-minus")
                    .w(px(28.0))
                    .h(px(40.0))
                    .rounded_lg()
                    .bg(rgb(0xf1f1f2))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .child("−")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.edit_selected_region(|cue| {
                            cue.zoom = (cue.zoom - 0.1).max(ZoomCue::MINIMUM_ZOOM)
                        });
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
                    .child(format!("{zoom:.1}×")),
            )
            .child(
                div()
                    .id("motion-zoom-plus")
                    .w(px(28.0))
                    .h(px(40.0))
                    .rounded_lg()
                    .bg(rgb(0xf1f1f2))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .child("+")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.edit_selected_region(|cue| {
                            cue.zoom = (cue.zoom + 0.1).min(ZoomCue::MAXIMUM_ZOOM)
                        });
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    /// Contextual panel for the selected motion region.
    pub(crate) fn motion_inspector(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let cue = self.selected_region()?;
        let edit_busy = self.video_edit_busy;
        let has_pointer = self.scene_has_pointer();
        let editor_start = self
            .video_clip_timeline
            .editor_time_for_source(cue.start)
            .unwrap_or(cue.start);
        let editor_end = self
            .video_clip_timeline
            .editor_time_for_source(cue.end)
            .unwrap_or(cue.end);
        let style_index = MotionStyle::ALL
            .iter()
            .position(|style| *style == cue.motion)
            .unwrap_or(0);
        let anchor_index = match cue.anchor_mode {
            ZoomAnchorMode::PointerAnchor => 0,
            ZoomAnchorMode::SmartAnchor => 1,
            ZoomAnchorMode::PinnedAnchor => 2,
        };
        let pick_index = match self.motion_pick {
            MotionPick::Focus => 0,
            MotionPick::PanEnd => 1,
        };
        let enabled = cue.is_enabled;
        let has_pan = cue.pan_to.is_some();
        let pick_hint = match (self.motion_pick, self.video_project.is_some()) {
            (MotionPick::Focus, true) => "Click the video to set the focus point",
            (MotionPick::Focus, false) => "Click the image to set the focus point",
            (MotionPick::PanEnd, true) => "Click the video where the pan should end",
            (MotionPick::PanEnd, false) => "Click the image where the pan should end",
        };
        Some(
            div()
                .flex()
                .flex_col()
                .gap_3()
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
                                .child(div().size(px(10.0)).rounded_full().bg(orange(true)))
                                .child(
                                    div()
                                        .text_sm()
                                        .font_weight(FontWeight::BOLD)
                                        .child("Motion region"),
                                ),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted())
                                .child(format!("{:.1}s – {:.1}s", editor_start, editor_end)),
                        ),
                )
                .child(Self::inspector_label("Style"))
                .child(self.segmented(
                    "motion-style",
                    &["Hold", "Zoom in", "Zoom out"],
                    style_index,
                    |this, index| {
                        this.edit_selected_region(|cue| cue.motion = MotionStyle::ALL[index]);
                    },
                    cx,
                ))
                .child(self.motion_zoom_slider(cue.zoom, cx))
                .when(self.inspector_level >= 2, |this| {
                    let easing_index = crate::scene_ui::easing_index(cue.easing);
                    let has_tilt = cue.tilt.is_some();
                    this.child(Self::inspector_label("Easing (ramps and pans)"))
                        .child(self.segmented(
                            "motion-easing",
                            &["Smooth", "Linear", "Snappy", "Cinematic"],
                            easing_index,
                            |this, index| {
                                this.edit_selected_region(|cue| {
                                    cue.easing =
                                        crate::recording::viewport::MotionEasing::ALL[index]
                                });
                            },
                            cx,
                        ))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(muted())
                                        .w(px(40.0))
                                        .child("Start"),
                                )
                                .child(self.small_button(
                                    "motion-start-earlier",
                                    "−0.1s",
                                    !edit_busy,
                                    cx,
                                    |this, _| {
                                        this.edit_selected_region(|cue| {
                                            cue.start = (cue.start - 0.1).max(0.0)
                                        });
                                    },
                                ))
                                .child(self.small_button(
                                    "motion-start-later",
                                    "+0.1s",
                                    !edit_busy,
                                    cx,
                                    |this, _| {
                                        this.edit_selected_region(|cue| {
                                            cue.start = (cue.start + 0.1)
                                                .min(cue.end - ZoomCue::MINIMUM_DURATION)
                                        });
                                    },
                                ))
                                .child(div().text_xs().text_color(muted()).w(px(30.0)).child("End"))
                                .child(self.small_button(
                                    "motion-end-earlier",
                                    "−0.1s",
                                    !edit_busy,
                                    cx,
                                    |this, _| {
                                        this.edit_selected_region(|cue| {
                                            cue.end = (cue.end - 0.1)
                                                .max(cue.start + ZoomCue::MINIMUM_DURATION)
                                        });
                                    },
                                ))
                                .child(self.small_button(
                                    "motion-end-later",
                                    "+0.1s",
                                    !edit_busy,
                                    cx,
                                    |this, _| {
                                        let limit = this.video_source_duration;
                                        this.edit_selected_region(|cue| {
                                            cue.end = (cue.end + 0.1).min(limit)
                                        });
                                    },
                                )),
                        )
                        .child(self.scene_toggle_row(
                            "motion-tilt-toggle",
                            "3D tilt while active",
                            has_tilt,
                            cx,
                            |this| {
                                this.edit_selected_region(|cue| {
                                    cue.tilt = if cue.tilt.is_some() {
                                        None
                                    } else {
                                        Some(crate::recording::viewport::Tilt {
                                            x: 8.0,
                                            y: -18.0,
                                            z: 0.0,
                                        })
                                    };
                                });
                            },
                        ))
                })
                .when(has_pointer, |this| {
                    this.child(Self::inspector_label("Target"))
                        .child(self.segmented(
                            "motion-target",
                            &["Cursor", "Auto", "Pinned"],
                            anchor_index,
                            |this, index| {
                                this.edit_selected_region(|cue| {
                                    cue.anchor_mode = match index {
                                        0 => ZoomAnchorMode::PointerAnchor,
                                        1 => ZoomAnchorMode::SmartAnchor,
                                        _ => ZoomAnchorMode::PinnedAnchor,
                                    };
                                });
                            },
                            cx,
                        ))
                })
                .child(Self::inspector_label("Canvas click sets"))
                .child(self.segmented(
                    "motion-pick",
                    &["Focus point", "Pan end"],
                    pick_index,
                    |this, index| {
                        this.motion_pick = if index == 0 {
                            MotionPick::Focus
                        } else {
                            MotionPick::PanEnd
                        };
                    },
                    cx,
                ))
                .child(div().text_xs().text_color(muted()).child(pick_hint))
                .when(has_pan, |this| {
                    this.child(self.small_button(
                        "motion-clear-pan",
                        "Remove pan",
                        !edit_busy,
                        cx,
                        |this, _| {
                            this.edit_selected_region(|cue| cue.pan_to = None);
                        },
                    ))
                })
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(div().text_sm().child("Enabled"))
                        .child(
                            div()
                                .id("motion-enabled-toggle")
                                .cursor_pointer()
                                .child(self.toggle(enabled))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.edit_selected_region(|cue| {
                                        cue.is_enabled = !cue.is_enabled
                                    });
                                    cx.notify();
                                })),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            self.small_button("motion-deselect", "Done", true, cx, |this, _| {
                                this.video_selected_zoom_cue = None;
                            }),
                        )
                        .child(
                            div()
                                .id("motion-delete")
                                .px_3()
                                .h(px(30.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_md()
                                .bg(rgb(0xfee2e2))
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(rgb(0xb91c1c))
                                .opacity(if edit_busy { 0.4 } else { 1.0 })
                                .when(!edit_busy, |this| {
                                    this.cursor_pointer()
                                        .hover(|style| style.bg(rgb(0xfecaca)))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.delete_selected_video_zoom(cx);
                                            cx.notify();
                                        }))
                                })
                                .child("Delete region"),
                        ),
                )
                .child(div().h(px(1.0)).bg(line()))
                .into_any_element(),
        )
    }

    /// Summary row shown in the scene panel of the recording editor.
    pub(crate) fn motion_overview_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let count = self.video_zoom_cues.len();
        let enabled = self
            .video_zoom_cues
            .iter()
            .filter(|cue| cue.is_enabled)
            .count();
        let busy = self.video_edit_busy;
        let summary = match count {
            0 => "No motion regions yet".to_string(),
            1 => "1 motion region".to_string(),
            _ if enabled == count => format!("{count} motion regions"),
            _ => format!("{count} motion regions · {enabled} enabled"),
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
                            .child("Motion"),
                    )
                    .child(div().text_xs().text_color(muted()).child(summary)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(self.small_button(
                        "motion-add-at-playhead",
                        "+ Motion at playhead",
                        !busy,
                        cx,
                        |this, cx| this.add_video_zoom_at_playhead(cx),
                    ))
                    .when(self.video_project.is_some(), |this| {
                        this.child(self.small_button(
                            "motion-regenerate",
                            "Auto from clicks",
                            !busy,
                            cx,
                            |this, cx| this.regenerate_motion_from_clicks(cx),
                        ))
                    }),
            )
            .child(
                div().text_xs().text_color(muted()).child(
                    "Select a region on the orange lane to adjust its zoom, focus, and timing.",
                ),
            )
            .child(div().h(px(1.0)).bg(line()))
            .into_any_element()
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
        if self.video_zoom_cues.is_empty() {
            let preset = self.animation_preset.unwrap_or(MotionPreset::SlowZoomIn);
            self.video_zoom_cues = preset.cues(self.animation_duration);
            self.animation_preset = Some(preset);
        }
        self.rebuild_video_motion_timelines();
        self.toast = Some("Animation on: pick a preset or edit the orange motion lane".into());
        cx.notify();
    }

    pub(crate) fn exit_animation(&mut self) {
        self.pause_video_playback();
        self.animation_active = false;
        self.video_playing = false;
        self.video_position = 0.0;
        self.video_selected_zoom_cue = None;
        self.video_zoom_drag = None;
        self.video_seek_drag = None;
        self.video_viewport_timeline = ViewportTimeline::default();
    }

    pub(crate) fn set_animation_duration(&mut self, duration: f64) {
        if !duration.is_finite() || duration < ZoomCue::MINIMUM_DURATION {
            return;
        }
        let previous = self.animation_duration.max(f64::EPSILON);
        self.animation_duration = duration;
        if !self.animation_active {
            return;
        }
        self.pause_video_playback();
        let factor = duration / previous;
        let original = self.video_zoom_cues.clone();
        for cue in &mut self.video_zoom_cues {
            cue.start = (cue.start * factor).clamp(0.0, duration);
            cue.end = (cue.end * factor).clamp(0.0, duration);
            if cue.end - cue.start < ZoomCue::MINIMUM_DURATION {
                cue.end = (cue.start + ZoomCue::MINIMUM_DURATION).min(duration);
                cue.start = (cue.end - ZoomCue::MINIMUM_DURATION).max(0.0);
            }
        }
        if self.video_zoom_cues != original {
            self.video_undo_stack
                .push(VideoEditSnapshot::Zoom(original));
            self.video_redo_stack.clear();
        }
        self.video_duration = duration;
        self.video_source_duration = duration;
        self.video_clip_timeline = RecordingClipTimeline::full(duration);
        self.video_position = self.video_position.min(duration);
        self.rebuild_video_motion_timelines();
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
                        // Animated screenshots loop, like the exported GIF.
                        0.0
                    } else {
                        next
                    };
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
        // A focused watermark field owns every key, including space.
        if self.handle_watermark_key(event) {
            return true;
        }
        let keystroke = &event.keystroke;
        if (keystroke.modifiers.control || keystroke.modifiers.platform) && keystroke.key == "z" {
            if keystroke.modifiers.shift {
                self.redo_video_edit(cx);
            } else {
                self.undo_video_edit(cx);
            }
            return true;
        }
        match keystroke.key.as_str() {
            "space" => {
                self.toggle_animation_playback(cx);
                true
            }
            "left" => {
                self.pause_video_playback();
                self.video_position = (self.video_position - 0.5).max(0.0);
                true
            }
            "right" => {
                self.pause_video_playback();
                self.video_position = (self.video_position + 0.5).min(self.video_duration);
                true
            }
            "delete" | "backspace" if self.video_selected_zoom_cue.is_some() => {
                self.delete_selected_video_zoom(cx);
                true
            }
            "escape" if self.video_selected_zoom_cue.is_some() => {
                self.video_selected_zoom_cue = None;
                true
            }
            _ => false,
        }
    }

    /// Timeline strip under the screenshot canvas while animation is on.
    pub(crate) fn animation_strip(&self, cx: &mut Context<Self>) -> AnyElement {
        let timeline_bounds = self.video_timeline_bounds.clone();
        let duration = self.video_duration.max(f64::EPSILON);
        let content_width = self.video_timeline_viewport_width() * self.video_timeline_zoom;
        let progress = (self.video_position / duration).clamp(0.0, 1.0);
        let playing = self.video_playing;
        let busy = self.export_progress.is_some();
        let can_undo = !self.video_undo_stack.is_empty();
        let can_redo = !self.video_redo_stack.is_empty();
        let has_selection = self.video_selected_zoom_cue.is_some();
        let ruler_step = {
            let raw = 80.0 * duration / content_width.max(1.0);
            [0.25, 0.5, 1.0, 2.0, 5.0]
                .into_iter()
                .find(|step| *step >= raw)
                .unwrap_or(10.0)
        };
        let mut ruler_marks: Vec<AnyElement> = Vec::new();
        let mut tick = 0.0;
        while tick <= duration + 1e-6 {
            let x = (tick / duration * content_width) as f32;
            ruler_marks.push(
                div()
                    .absolute()
                    .left(px(x))
                    .bottom_0()
                    .w(px(1.0))
                    .h(px(5.0))
                    .bg(hsla(0.0, 0.0, 0.0, 0.3))
                    .into_any_element(),
            );
            ruler_marks.push(
                div()
                    .absolute()
                    .left(px(x + 4.0))
                    .top(px(0.0))
                    .text_xs()
                    .text_color(muted())
                    .child(format!("{tick:.1}s"))
                    .into_any_element(),
            );
            tick += ruler_step;
        }
        let track = self.motion_track(self.video_timeline_scroll, content_width, progress, cx);
        let annotation_track =
            self.annotation_track(self.video_timeline_scroll, content_width, progress, cx);
        div()
            .w_full()
            .flex_none()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        div()
                            .id("animation-play-pause")
                            .size(px(32.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .bg(blue())
                            .cursor_pointer()
                            .child(
                                svg()
                                    .path(if playing {
                                        "icons/pause.svg"
                                    } else {
                                        "icons/play.svg"
                                    })
                                    .size(px(16.0))
                                    .text_color(rgb(0xffffff)),
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.toggle_animation_playback(cx);
                                cx.notify();
                            })),
                    )
                    .child(
                        div()
                            .w(px(92.0))
                            .text_xs()
                            .text_color(muted())
                            .child(format!(
                                "{:.1}s / {:.1}s",
                                self.video_position, self.video_duration
                            )),
                    )
                    .child(
                        div()
                            .id("animation-undo")
                            .size(px(30.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .opacity(if can_undo { 1.0 } else { 0.35 })
                            .when(can_undo, |this| {
                                this.cursor_pointer()
                                    .hover(|style| style.bg(rgb(0xeeeeef)))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.undo_video_edit(cx);
                                        cx.notify();
                                    }))
                            })
                            .child(
                                svg()
                                    .path("icons/undo.svg")
                                    .size(px(16.0))
                                    .text_color(ink()),
                            ),
                    )
                    .child(
                        div()
                            .id("animation-redo")
                            .size(px(30.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .opacity(if can_redo { 1.0 } else { 0.35 })
                            .when(can_redo, |this| {
                                this.cursor_pointer()
                                    .hover(|style| style.bg(rgb(0xeeeeef)))
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.redo_video_edit(cx);
                                        cx.notify();
                                    }))
                            })
                            .child(
                                svg()
                                    .path("icons/redo.svg")
                                    .size(px(16.0))
                                    .text_color(ink()),
                            ),
                    )
                    .child(self.small_button(
                        "animation-add-motion",
                        "+ Motion",
                        !busy,
                        cx,
                        |this, cx| {
                            let position = this.video_position;
                            this.add_motion_region_at(position, cx);
                        },
                    ))
                    .when(has_selection, |this| {
                        this.child(
                            div()
                                .id("animation-delete-motion")
                                .size(px(30.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_md()
                                .cursor_pointer()
                                .hover(|style| style.bg(rgb(0xfee2e2)))
                                .child(
                                    svg()
                                        .path("icons/trash.svg")
                                        .size(px(15.0))
                                        .text_color(ink()),
                                )
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.delete_selected_video_zoom(cx);
                                    cx.notify();
                                })),
                        )
                    })
                    .child(div().flex_1())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(div().size(px(10.0)).rounded_full().bg(orange(false)))
                            .child(div().text_xs().text_color(muted()).child("Motion")),
                    ),
            )
            .child(
                div()
                    .id("animation-ruler")
                    .relative()
                    .w_full()
                    .h(px(18.0))
                    .flex_none()
                    .overflow_hidden()
                    .child(
                        div()
                            .absolute()
                            .left(px(-(self.video_timeline_scroll as f32)))
                            .top_0()
                            .w(px(content_width as f32))
                            .h_full()
                            .children(ruler_marks)
                            .child(
                                div()
                                    .absolute()
                                    .left(px((content_width * progress) as f32 - 5.0))
                                    .top(px(2.0))
                                    .size(px(10.0))
                                    .rounded_full()
                                    .bg(hsla(222.0 / 360.0, 0.2, 0.15, 1.0)),
                            ),
                    )
                    .child(
                        canvas(
                            move |bounds, _, _| {
                                if let Ok(mut stored) = timeline_bounds.lock() {
                                    *stored = Some(bounds);
                                }
                            },
                            |_, _, _, _| {},
                        )
                        .size_full(),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, _, cx| {
                            this.pause_video_playback();
                            this.video_zoom_drag = None;
                            if let Some(target) = this.motion_timeline_time_at(event.position.x) {
                                this.video_position = target;
                                this.video_seek_drag = Some((event.position.x, target));
                            }
                            cx.notify();
                        }),
                    ),
            )
            .child(track)
            .when_some(annotation_track, |this, lane| this.child(lane))
            .into_any_element()
    }

    /// Inspector section for animated screenshots: duration and presets.
    pub(crate) fn animation_inspector_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let duration_index = ANIMATION_DURATIONS
            .iter()
            .position(|value| (*value - self.animation_duration).abs() < 1e-9)
            .unwrap_or(1);
        let selected_preset = self.animation_preset;
        let format_index = ExportFormat::ALL
            .iter()
            .position(|format| *format == self.export_format)
            .unwrap_or(0);
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
                            .child("Animation"),
                    )
                    .child(self.small_button(
                        "animation-static",
                        "Back to static",
                        true,
                        cx,
                        |this, _| this.exit_animation(),
                    )),
            )
            .child(Self::inspector_label("Duration"))
            .child(self.segmented(
                "animation-duration",
                &["3 s", "5 s", "8 s", "10 s"],
                duration_index,
                |this, index| this.set_animation_duration(ANIMATION_DURATIONS[index]),
                cx,
            ))
            .child(Self::inspector_label("Preset"))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_1()
                    .children(MotionPreset::ALL.into_iter().enumerate().map(|(index, preset)| {
                        let selected = selected_preset == Some(preset);
                        div()
                            .id(("animation-preset", index))
                            .px_3()
                            .h(px(30.0))
                            .flex()
                            .items_center()
                            .rounded_md()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .bg(if selected { orange(false) } else { rgb(0xf0f0f1).into() })
                            .text_color(if selected { rgb(0xffffff).into() } else { ink() })
                            .cursor_pointer()
                            .hover(move |style| {
                                if selected {
                                    style
                                } else {
                                    style.bg(rgb(0xe4e4e7))
                                }
                            })
                            .child(preset.label())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.apply_motion_preset(preset);
                                cx.notify();
                            }))
                    })),
            )
            .child(Self::inspector_label("Export as"))
            .child(self.segmented(
                "animation-export-format",
                &["MP4", "WebM", "GIF"],
                format_index,
                |this, index| this.export_format = ExportFormat::ALL[index],
                cx,
            ))
            .child(
                div()
                    .text_xs()
                    .text_color(muted())
                    .child("Finish exports the animation. Double-click the motion lane to add a region, drag its edges to retime it."),
            )
            .child(div().h(px(1.0)).bg(line()))
            .into_any_element()
    }

    /// The toolbar toggle that switches a screenshot between static and
    /// motion editing.
    pub(crate) fn animate_toolbar_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let active = self.animation_active;
        div()
            .id("toolbar-animate")
            .px_3()
            .h(px(34.0))
            .flex()
            .items_center()
            .gap_2()
            .rounded_md()
            .text_sm()
            .cursor_pointer()
            .when(active, |this| {
                this.bg(hsla(24.0 / 360.0, 0.95, 0.94, 1.0))
                    .text_color(hsla(24.0 / 360.0, 0.9, 0.35, 1.0))
            })
            .hover(move |style| {
                if active {
                    style.bg(hsla(24.0 / 360.0, 0.95, 0.9, 1.0))
                } else {
                    style.bg(rgb(0xeeeeef))
                }
            })
            .child(
                svg()
                    .path("icons/play.svg")
                    .size(px(15.0))
                    .text_color(if active {
                        hsla(24.0 / 360.0, 0.9, 0.4, 1.0)
                    } else {
                        ink()
                    }),
            )
            .child(if active { "Animating" } else { "Animate" })
            .on_click(cx.listener(|this, _, _, cx| this.toggle_animation(cx)))
            .into_any_element()
    }

    /// Format picker for the recording editor's top bar.
    pub(crate) fn export_format_picker(&self, cx: &mut Context<Self>) -> AnyElement {
        let busy = self.export_progress.is_some();
        div()
            .flex()
            .items_center()
            .h(px(32.0))
            .p(px(3.0))
            .rounded_md()
            .bg(rgb(0xf0f0f1))
            .opacity(if busy { 0.5 } else { 1.0 })
            .children(
                ExportFormat::ALL
                    .into_iter()
                    .enumerate()
                    .map(|(index, format)| {
                        let selected = self.export_format == format;
                        div()
                            .id(("export-format", index))
                            .px_2()
                            .h_full()
                            .flex()
                            .items_center()
                            .rounded_sm()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(if selected { ink() } else { muted() })
                            .when(selected, |this| this.bg(rgb(0xffffff)).shadow_sm())
                            .when(!busy, |this| {
                                this.cursor_pointer().on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.export_format = format;
                                        cx.notify();
                                    },
                                ))
                            })
                            .child(format.label())
                    }),
            )
            .into_any_element()
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
        svg.push_str(&self.annotations_svg(0.0, 0.0, width, height, stroke_scale));
        svg.push_str("</svg>");
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
            .format(&format!(
                "Screendrop-%Y-%m-%d_%H-%M-%S.{}",
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
        request.frame_rate = self.export_frame_rate;
        request.loop_forever = self.export_loop;
        request.include_audio = !self.video_audio_muted;
        let source = SceneSource::Video {
            media: session.screen_path(),
            clips: self.video_clip_timeline.clone(),
            pointer: self
                .video_pointer_synthesized
                .then(|| self.video_pointer_timeline.clone()),
        };
        self.prompt_and_run_scene_export(source, request, suggested_name, cx);
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
                "Screendrop-%Y-%m-%d_%H-%M-%S-%3f.{}",
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
        request.frame_rate = self.export_frame_rate;
        request.loop_forever = self.export_loop;
        request.overlay = self.annotation_overlay_source();
        self.prompt_and_run_scene_export(SceneSource::Image(image), request, suggested_name, cx);
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
        self.pause_video_playback();
        let progress = Arc::new(ExportProgress::default());
        self.export_progress = Some(progress.clone());
        self.export_label = format!("Exporting {}…", request.format.label()).into();
        self.video_edit_busy = true;
        self.toast = None;
        let format_label = request.format.label();
        let destination = request.destination.clone();
        let mut request = request;
        let task = cx.background_executor().spawn(async move {
            export_scene(source, &mut request, &progress).map_err(|error| error.to_string())
        });
        // Keep the progress bar fresh while the background render runs.
        cx.spawn(async move |weak, cx| loop {
            Timer::after(Duration::from_millis(120)).await;
            let active = weak
                .update(cx, |this, cx| {
                    cx.notify();
                    this.export_progress.is_some()
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
                this.video_edit_busy = false;
                this.toast = Some(match result {
                    Ok(()) => {
                        format!("Exported {format_label} to {}", destination.display()).into()
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

    /// Floating progress card with a cancel button while an export runs.
    pub(crate) fn export_status_overlay(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let progress = self.export_progress.as_ref()?;
        let fraction = progress.fraction();
        let cancelling = progress.is_cancelled();
        let label = if cancelling {
            "Cancelling…".to_string()
        } else {
            format!("{} {:.0}%", self.export_label, fraction * 100.0)
        };
        Some(
            div()
                .absolute()
                .bottom(px(72.0))
                .left(px(220.0))
                .w(px(320.0))
                .p_3()
                .rounded_lg()
                .bg(hsla(220.0 / 360.0, 0.2, 0.12, 0.94))
                .text_color(rgb(0xffffff))
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
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(label),
                        )
                        .child(
                            div()
                                .id("export-cancel")
                                .px_2()
                                .h(px(24.0))
                                .flex()
                                .items_center()
                                .rounded_md()
                                .bg(hsla(0.0, 0.0, 1.0, 0.12))
                                .text_xs()
                                .cursor_pointer()
                                .hover(|style| style.bg(hsla(0.0, 0.0, 1.0, 0.22)))
                                .child("Cancel")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.cancel_export();
                                    cx.notify();
                                })),
                        ),
                )
                .child(
                    div()
                        .w_full()
                        .h(px(6.0))
                        .rounded_full()
                        .bg(hsla(0.0, 0.0, 1.0, 0.15))
                        .overflow_hidden()
                        .child(
                            div()
                                .h_full()
                                .w(gpui::relative(fraction as f32))
                                .rounded_full()
                                .bg(orange(false)),
                        ),
                )
                .into_any_element(),
        )
    }
}
