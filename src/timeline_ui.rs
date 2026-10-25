//! Timeline navigation, controls and contextual actions shared by both editors.
use crate::timeline_edit::precise_timecode;
use crate::{blue, ink, line, muted, Studio};
use gpui::{
    div, point, prelude::*, px, rgb, svg, AnyElement, App, Context, CursorStyle, FontWeight,
    KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, Point, Render, SharedString,
    Timer, Window,
};
use std::time::Duration;
use uuid::Uuid;

#[derive(Clone, Copy)]
pub(crate) enum TimelineContextTarget {
    Clip(Uuid, f64),
    Motion(Uuid),
    EmptyMotion(f64),
    AddMedia,
}

pub(crate) struct TimelineUiState {
    pub snap_enabled: bool,
    pub snap_guide: Option<f64>,
    pub skim_enabled: bool,
    pub follow_playhead: bool,
    pub lane_height: f32,
    pub ruler_hover: bool,
    pub hovered_clip: Option<Uuid>,
    pub hover_time: Option<f64>,
    pub vertical_scroll: gpui::ScrollHandle,
    context: Option<(TimelineContextTarget, Point<Pixels>)>,
    scrollbar_drag: Option<(Pixels, f64)>,
    resize_drag: Option<(Pixels, f32)>,
    edge_pointer: Option<Pixels>,
    edge_task_running: bool,
    follow_suspended: bool,
}

impl Default for TimelineUiState {
    fn default() -> Self {
        Self {
            snap_enabled: true,
            snap_guide: None,
            skim_enabled: false,
            follow_playhead: true,
            lane_height: 176.0,
            ruler_hover: false,
            hovered_clip: None,
            hover_time: None,
            vertical_scroll: gpui::ScrollHandle::new(),
            context: None,
            scrollbar_drag: None,
            resize_drag: None,
            edge_pointer: None,
            edge_task_running: false,
            follow_suspended: false,
        }
    }
}

struct TimelineTooltip(SharedString);
impl Render for TimelineTooltip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .px_3()
            .py_2()
            .rounded_md()
            .bg(ink())
            .text_color(rgb(0xffffff))
            .text_xs()
            .shadow_md()
            .child(self.0.clone())
    }
}

pub(crate) fn tooltip(
    text: impl Into<SharedString>,
) -> impl Fn(&mut Window, &mut App) -> gpui::AnyView {
    let text = text.into();
    move |_, cx| cx.new(|_| TimelineTooltip(text.clone())).into()
}

fn scrollbar_geometry(viewport: f64, content: f64, scroll: f64) -> (f64, f64) {
    let width = (viewport * viewport / content.max(viewport).max(1.0))
        .clamp(32.0_f64.min(viewport), viewport);
    let travel = (viewport - width).max(0.0);
    let left = scroll / (content - viewport).max(1.0) * travel;
    (left.clamp(0.0, travel), width)
}

impl Studio {
    pub(crate) fn reset_timeline_interactions(&mut self) {
        self.timeline_ui.context = None;
        self.timeline_ui.snap_guide = None;
        self.timeline_ui.hover_time = None;
        self.timeline_ui.hovered_clip = None;
        self.timeline_ui.scrollbar_drag = None;
        self.timeline_ui.resize_drag = None;
        self.timeline_ui.edge_pointer = None;
        self.timeline_ui.follow_suspended = false;
        self.timeline_ui
            .vertical_scroll
            .set_offset(gpui::point(px(0.0), px(0.0)));
        self.timeline_motion = Default::default();
        self.video_seek_drag = None;
        self.video_trim_drag = None;
        self.video_move_drag = None;
        self.video_zoom_drag = None;
        self.video_audio_drag = None;
        self.annotation_drag = None;
    }

    pub(crate) fn fit_timeline(&mut self) {
        self.video_timeline_span = self.video_duration.max(0.001);
        self.video_timeline_extent = self.video_duration.max(self.video_position);
        self.video_timeline_zoom = 1.0;
        self.video_timeline_scroll = 0.0;
        self.timeline_ui.follow_suspended = false;
    }

    pub(crate) fn timeline_button_zoom_anchor(&self) -> f64 {
        let x = self.video_position / self.video_timeline_seconds_per_pixel()
            - self.video_timeline_scroll;
        if x >= 0.0 && x <= self.video_timeline_viewport_width() {
            self.video_position
        } else {
            (self.video_timeline_scroll + self.video_timeline_viewport_width() / 2.0)
                * self.video_timeline_seconds_per_pixel()
        }
    }

    pub(crate) fn follow_timeline_playhead(&mut self, force: bool) {
        if !force && (!self.timeline_ui.follow_playhead || self.timeline_ui.follow_suspended) {
            return;
        }
        if force {
            self.timeline_ui.follow_suspended = false;
        }
        let width = self.video_timeline_viewport_width();
        let x = self.video_position / self.video_timeline_seconds_per_pixel();
        if x < self.video_timeline_scroll + 32.0 || x > self.video_timeline_scroll + width - 32.0 {
            let inset = if self.video_playing {
                width / 3.0
            } else {
                width / 2.0
            };
            self.pan_video_timeline(x - inset - self.video_timeline_scroll);
        }
    }

    pub(crate) fn manually_pan_timeline(&mut self, delta: f64) {
        self.timeline_ui.follow_suspended = self.video_playing;
        self.pan_video_timeline(delta);
    }

    pub(crate) fn timeline_pointer_move(
        &mut self,
        event: &MouseMoveEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        if let Some((start_y, height)) = self.timeline_ui.resize_drag {
            self.timeline_ui.lane_height =
                (height + f32::from(start_y - event.position.y)).clamp(110.0, 340.0);
            return true;
        }
        if let Some((start_x, scroll)) = self.timeline_ui.scrollbar_drag {
            let width = self.video_timeline_viewport_width();
            let content = self.video_timeline_content_width();
            let (_, thumb) = scrollbar_geometry(width, content, scroll);
            let target = scroll
                + f64::from(event.position.x - start_x) * (content - width)
                    / (width - thumb).max(1.0);
            self.manually_pan_timeline(target - self.video_timeline_scroll);
            return true;
        }
        if event.dragging() {
            self.update_timeline_skim(None, cx);
        }
        if event.dragging() && self.timeline_drag_active() {
            self.timeline_ui.edge_pointer = Some(event.position.x);
            if !self.timeline_ui.edge_task_running {
                self.timeline_ui.edge_task_running = true;
                cx.spawn(async move |weak, cx| loop {
                    Timer::after(Duration::from_millis(16)).await;
                    let keep = weak
                        .update(cx, |this, cx| {
                            if !this.timeline_drag_active()
                                || this.timeline_ui.edge_pointer.is_none()
                            {
                                this.timeline_ui.edge_task_running = false;
                                return false;
                            }
                            this.auto_pan_timeline(cx);
                            true
                        })
                        .unwrap_or(false);
                    if !keep {
                        break;
                    }
                })
                .detach();
            }
        }
        if event.dragging() && self.timeline_motion.creation.is_some() {
            return self.update_motion_creation(event.position.x);
        }
        false
    }

    fn timeline_drag_active(&self) -> bool {
        self.video_trim_drag.is_some()
            || self.video_move_drag.is_some_and(|d| d.active)
            || self.video_zoom_drag.is_some()
            || self.video_audio_drag.is_some()
            || self.video_seek_drag.is_some()
            || self.motion_creation_is_active()
            || self.annotation_drag.is_some()
            || self.image_trim_drag.is_some()
    }

    fn auto_pan_timeline(&mut self, cx: &mut Context<Self>) {
        let Some(pointer) = self.timeline_ui.edge_pointer else {
            return;
        };
        let Some(bounds) = self.video_timeline_bounds.lock().ok().and_then(|b| *b) else {
            return;
        };
        let x = f64::from(pointer - bounds.origin.x);
        let width = f64::from(bounds.size.width);
        let delta = if x < 24.0 {
            -((24.0 - x) * 0.35).min(18.0)
        } else if x > width - 24.0 {
            ((x - width + 24.0) * 0.35).min(18.0)
        } else {
            0.0
        };
        if delta == 0.0 {
            return;
        }
        let before = self.video_timeline_scroll;
        self.pan_video_timeline(delta);
        if (self.video_timeline_scroll - before).abs() < 0.01 {
            return;
        }
        if self.timeline_motion.creation.is_some() {
            self.update_motion_creation(pointer);
        } else if self.image_trim_drag.is_some() {
            self.update_image_timeline_drag(pointer);
        } else if self.annotation_drag.is_some() {
            self.update_annotation_drag(pointer);
        } else if self.video_trim_drag.is_some() {
            self.update_video_trim(pointer);
        } else if self.video_zoom_drag.is_some() {
            self.update_video_zoom_drag(pointer);
        } else if self.video_audio_drag.is_some() {
            self.update_audio_drag(pointer);
        } else if self.video_move_drag.is_some() {
            self.update_video_move_drag(pointer);
        } else if self.video_seek_drag.is_some() {
            if let Some(time) = self.motion_timeline_time_at(pointer) {
                self.scrub_video_to(time, cx);
            }
        }
        cx.notify();
    }

    pub(crate) fn finish_timeline_pointer(
        &mut self,
        pointer: Pixels,
        cx: &mut Context<Self>,
    ) -> bool {
        self.timeline_ui.edge_pointer = None;
        let owned = self.timeline_ui.scrollbar_drag.take().is_some()
            | self.timeline_ui.resize_drag.take().is_some();
        let created = self.finish_motion_creation(pointer, cx);
        owned || created
    }

    pub(crate) fn handle_timeline_shortcut(
        &mut self,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let key = &event.keystroke;
        let command = key.modifiers.control || key.modifiers.platform;
        if key.key == "escape" {
            let menu = self.timeline_ui.context.take().is_some();
            let navigation = self.timeline_ui.scrollbar_drag.take().is_some()
                | self.timeline_ui.resize_drag.take().is_some();
            let creation = self.cancel_motion_creation();
            let edit = self.cancel_timeline_edit(cx);
            let scrub = self.video_seek_drag.take().is_some();
            if scrub {
                self.finish_timeline_scrub(cx);
            }
            self.timeline_ui.edge_pointer = None;
            return menu || navigation || creation || edit || scrub;
        }
        // These shortcuts are handled by the editor after this helper.
        // Dismiss a menu before they change its selection or timeline.
        if (command && matches!(key.key.as_str(), "z" | "y"))
            || (!command
                && matches!(
                    key.key.as_str(),
                    "s" | "m" | "delete" | "backspace" | "v" | "t" | "space"
                ))
        {
            self.timeline_ui.context = None;
        }
        if self.video_edit_busy {
            return false;
        }
        if command {
            match key.key.as_str() {
                "0" => self.fit_timeline(),
                "=" | "+" => self.zoom_video_timeline(1.4, self.timeline_button_zoom_anchor()),
                "-" => self.zoom_video_timeline(1.0 / 1.4, self.timeline_button_zoom_anchor()),
                _ => return false,
            }
            return true;
        }
        match key.key.as_str() {
            "left" | "right" if self.selected_annotation.is_none() => {
                let source_rate = self
                    .video_project
                    .as_ref()
                    .and(self.timeline_preview.frame_rate);
                let fps = source_rate
                    .filter(|v| v.is_finite() && *v >= 1.0 && *v <= 120.0)
                    .unwrap_or(self.export_frame_rate.max(1.0));
                let step = if key.modifiers.shift { 1.0 } else { 1.0 / fps };
                let target = self.video_position + if key.key == "left" { -step } else { step };
                self.seek_video(target.clamp(0.0, self.video_duration), cx);
                self.follow_timeline_playhead(true);
            }
            "home" => {
                self.seek_video(0.0, cx);
                self.follow_timeline_playhead(true);
            }
            "end" => {
                self.seek_video(self.video_duration, cx);
                self.follow_timeline_playhead(true);
            }
            "n" => {
                self.timeline_ui.snap_enabled = !self.timeline_ui.snap_enabled;
                self.timeline_ui.snap_guide = None;
            }
            _ => return false,
        }
        true
    }

    pub(crate) fn open_timeline_context(
        &mut self,
        target: TimelineContextTarget,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        if self.video_edit_busy {
            return;
        }
        self.finish_annotation_interaction();
        self.selected_annotation = None;
        self.annotation_selection.clear();
        self.pause_video_playback();
        self.update_timeline_skim(None, cx);
        if let TimelineContextTarget::Motion(id) = target {
            self.video_selected_zoom_cue = Some(id);
            self.video_selected_clip = None;
        }
        if let TimelineContextTarget::Clip(id, _) = target {
            self.video_selected_clip = Some(id);
            self.video_selected_zoom_cue = None;
        }
        self.timeline_ui.context = Some((target, position));
        cx.notify();
    }

    /// Menu actions keep the item that was right-clicked, even if some
    /// other interaction changed the editor's current selection.
    fn select_timeline_context_target(&mut self, target: TimelineContextTarget) -> bool {
        if self.video_edit_busy {
            return false;
        }
        match target {
            TimelineContextTarget::Clip(id, _) => {
                if !self
                    .video_clip_timeline
                    .segments
                    .iter()
                    .any(|clip| clip.id == id)
                {
                    return false;
                }
                self.finish_annotation_interaction();
                self.video_selected_clip = Some(id);
                self.video_selected_zoom_cue = None;
            }
            TimelineContextTarget::Motion(id) => {
                if !self.video_zoom_cues.iter().any(|cue| cue.id == id) {
                    return false;
                }
                self.finish_annotation_interaction();
                self.video_selected_zoom_cue = Some(id);
                self.video_selected_clip = None;
            }
            TimelineContextTarget::AddMedia | TimelineContextTarget::EmptyMotion(_) => {}
        }
        true
    }

    fn timeline_button(
        &self,
        id: &'static str,
        label: &'static str,
        help: &'static str,
        enabled: bool,
        active: bool,
        cx: &mut Context<Self>,
        action: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> AnyElement {
        div()
            .id(id)
            .h(px(28.0))
            .px_2()
            .flex_none()
            .rounded_md()
            .flex()
            .items_center()
            .justify_center()
            .text_xs()
            .font_weight(FontWeight::MEDIUM)
            .text_color(if active { blue() } else { ink() })
            .bg(if active { rgb(0xe7f1ff) } else { rgb(0xf5f5f7) })
            .opacity(if enabled { 1.0 } else { 0.35 })
            .when(self.timeline_ui.context.is_none(), |d| {
                d.tooltip(tooltip(help))
            })
            .when(enabled, |d| {
                d.cursor_pointer()
                    .hover(|s| s.bg(rgb(0xe8edf4)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        action(this, cx);
                        cx.notify();
                    }))
            })
            .child(label)
            .into_any_element()
    }

    fn timeline_media_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let open = matches!(
            self.timeline_ui.context,
            Some((TimelineContextTarget::AddMedia, _))
        );
        let enabled = !self.video_edit_busy;
        let button = div()
            .id("timeline-add-media")
            .h(px(28.0))
            .px_2()
            .rounded_md()
            .flex()
            .items_center()
            .justify_center()
            .text_xs()
            .font_weight(FontWeight::MEDIUM)
            .text_color(if open { blue() } else { ink() })
            .bg(if open { rgb(0xe7f1ff) } else { rgb(0xf5f5f7) })
            .opacity(if enabled { 1.0 } else { 0.35 })
            .when(self.timeline_ui.context.is_none(), |d| {
                d.tooltip(tooltip("Import video, image overlay, or audio"))
            })
            .when(enabled, |d| {
                d.cursor_pointer()
                    .hover(|s| s.bg(rgb(0xe8edf4)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            // Toggle on press so outside-dismiss cannot reopen it
                            // on the subsequent mouse-up over the same button.
                            this.timeline_ui.context = (!open)
                                .then_some((TimelineContextTarget::AddMedia, Point::default()));
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    )
            })
            .child("+ Media");
        div()
            .relative()
            .flex_none()
            .flex()
            .child(button)
            .when(open, |d| d.children(self.render_timeline_context_menu(cx)))
            .into_any_element()
    }

    pub(crate) fn timeline_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        let video = self.video_project.is_some();
        let busy = self.video_edit_busy;
        let edit = div()
            .flex()
            .gap_1()
            .items_center()
            .when(video, |d| d.child(self.timeline_media_button(cx)))
            .when(video, |d| {
                d.child(self.timeline_button(
                    "video-split",
                    "Split",
                    "Split at playhead · S",
                    !busy,
                    false,
                    cx,
                    |this, cx| this.split_video_clip(cx),
                ))
            })
            .child(self.timeline_button(
                "timeline-add-motion",
                "+ Motion",
                "Add motion at playhead · M",
                !busy,
                false,
                cx,
                |this, cx| this.add_motion_region_at(this.video_position, cx),
            ))
            .when(!video, |d| {
                d.child(self.timeline_button(
                    "timeline-extend-scene",
                    "+ 5s",
                    "Extend this scene by five seconds",
                    !busy,
                    false,
                    cx,
                    |this, _| this.set_animation_duration(this.animation_duration + 5.0),
                ))
            })
            .child(self.timeline_button(
                "timeline-undo",
                "↶",
                "Undo · Ctrl+Z",
                !busy && !self.video_undo_stack.is_empty(),
                false,
                cx,
                |this, cx| this.undo_current(cx),
            ))
            .child(self.timeline_button(
                "timeline-redo",
                "↷",
                "Redo · Ctrl+Shift+Z",
                !busy && !self.video_redo_stack.is_empty(),
                false,
                cx,
                |this, cx| this.redo_current(cx),
            ));
        let transport = div()
            .flex()
            .gap_1()
            .items_center()
            .flex_none()
            .child(self.timeline_button(
                "timeline-start",
                "|‹",
                "Go to start · Home",
                true,
                false,
                cx,
                |this, cx| {
                    this.seek_video(0.0, cx);
                    this.follow_timeline_playhead(true);
                },
            ))
            .child(
                div()
                    .id("timeline-play-pause")
                    .size(px(32.0))
                    .rounded_full()
                    .bg(ink())
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .tooltip(tooltip("Play / pause · Space"))
                    .child(
                        svg()
                            .path(if self.video_playing {
                                "icons/pause.svg"
                            } else {
                                "icons/play.svg"
                            })
                            .size(px(14.0))
                            .text_color(rgb(0xffffff)),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.timeline_ui.follow_suspended = false;
                        this.toggle_timeline_playback(cx);
                        cx.notify();
                    })),
            )
            .child(self.timeline_button(
                "timeline-end",
                "›|",
                "Go to end · End",
                true,
                false,
                cx,
                |this, cx| {
                    this.seek_video(this.video_duration, cx);
                    this.follow_timeline_playhead(true);
                },
            ));
        let zoom = div()
            .flex()
            .gap_1()
            .items_center()
            .flex_none()
            .child(self.timeline_button(
                "timeline-zoom-out",
                "−",
                "Zoom out · Ctrl+−",
                self.video_timeline_zoom > crate::video::MIN_TIMELINE_ZOOM + 0.0001,
                false,
                cx,
                |this, _| this.zoom_video_timeline(1.0 / 1.4, this.timeline_button_zoom_anchor()),
            ))
            .child(self.timeline_button(
                "timeline-fit",
                "Fit",
                "Fit recording · Ctrl+0",
                true,
                false,
                cx,
                |this, _| this.fit_timeline(),
            ))
            .child(self.timeline_button(
                "timeline-zoom-in",
                "+",
                "Zoom in · Ctrl++ · Ctrl+wheel at pointer",
                true,
                false,
                cx,
                |this, _| this.zoom_video_timeline(1.4, this.timeline_button_zoom_anchor()),
            ));
        div()
            .flex_none()
            .flex()
            .flex_col()
            .child(
                div()
                    .h(px(44.0))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(edit)
                    .child(transport)
                    .child(zoom),
            )
            .child(
                div()
                    .h(px(34.0))
                    .flex()
                    .items_center()
                    .gap_1()
                    .border_b_1()
                    .border_color(line())
                    .child(self.timeline_button(
                        "timeline-snap",
                        "Snap",
                        "Snap to playhead and clip edges · N",
                        true,
                        self.timeline_ui.snap_enabled,
                        cx,
                        |this, _| {
                            this.timeline_ui.snap_enabled = !this.timeline_ui.snap_enabled;
                            this.timeline_ui.snap_guide = None;
                        },
                    ))
                    .when(video, |d| {
                        d.child(self.timeline_button(
                            "timeline-skim",
                            "Skim",
                            "Preview while hovering; keep the playhead parked",
                            true,
                            self.timeline_ui.skim_enabled,
                            cx,
                            |this, cx| {
                                this.timeline_ui.skim_enabled = !this.timeline_ui.skim_enabled;
                                this.update_timeline_skim(None, cx);
                            },
                        ))
                    })
                    .child(self.timeline_button(
                        "timeline-follow",
                        "Follow",
                        "Keep playback visible; scrolling temporarily suspends follow",
                        true,
                        self.timeline_ui.follow_playhead,
                        cx,
                        |this, _| {
                            this.timeline_ui.follow_playhead = !this.timeline_ui.follow_playhead;
                            this.timeline_ui.follow_suspended = false;
                            if this.timeline_ui.follow_playhead {
                                this.follow_timeline_playhead(true);
                            }
                        },
                    ))
                    .when(self.video_selected_clip.is_some() && video, |d| {
                        d.child(self.timeline_button(
                            "video-speed",
                            "Speed…",
                            "Change selected clip speed",
                            !busy,
                            false,
                            cx,
                            |this, cx| {
                                this.video_speed_draft = this.video_selected_clip.and_then(|id| {
                                    this.video_clip_timeline
                                        .segments
                                        .iter()
                                        .find(|c| c.id == id)
                                        .map(|c| c.speed)
                                });
                                cx.notify();
                            },
                        ))
                    })
                    .child(div().flex_1())
                    .child(
                        div()
                            .id("timeline-clock")
                            .text_xs()
                            .font_family("monospace")
                            .tooltip(tooltip(if !video && self.image_scenes.len() > 1 {
                                format!(
                                    "Scene {} of {} · {:.1}s sequence",
                                    self.image_scene_index + 1,
                                    self.image_scenes.len(),
                                    self.sequence_duration()
                                )
                            } else {
                                "Current time / duration".into()
                            }))
                            .text_color(ink())
                            .child(format!(
                                "{} / {}",
                                precise_timecode(self.video_position),
                                precise_timecode(self.video_duration)
                            )),
                    )
                    .child(
                        div()
                            .w(px(48.0))
                            .text_right()
                            .text_xs()
                            .text_color(muted())
                            .child(if self.video_timeline_zoom < 1.0 {
                                format!("{:.2}×", self.video_timeline_zoom)
                            } else {
                                format!("{:.1}×", self.video_timeline_zoom)
                            }),
                    ),
            )
            .into_any_element()
    }

    pub(crate) fn timeline_resize_handle(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("timeline-resize")
            .h(px(8.0))
            .w_full()
            .flex_none()
            .cursor(CursorStyle::ResizeUpDown)
            .flex()
            .items_center()
            .justify_center()
            .hover(|s| s.bg(rgb(0xf0f3f7)))
            .tooltip(tooltip("Drag to resize timeline"))
            .child(div().w(px(32.0)).h(px(2.0)).rounded_full().bg(line()))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    this.timeline_ui.resize_drag =
                        Some((event.position.y, this.timeline_ui.lane_height));
                    cx.stop_propagation();
                }),
            )
            .into_any_element()
    }

    pub(crate) fn timeline_scrollbar(&self, cx: &mut Context<Self>) -> AnyElement {
        let viewport = self.video_timeline_viewport_width();
        let content = self.video_timeline_content_width();
        let (left, width) = scrollbar_geometry(viewport, content, self.video_timeline_scroll);
        div()
            .h(px(18.0))
            .flex_none()
            .pl(px(104.0))
            .flex()
            .items_center()
            .child(
                div()
                    .id("timeline-scrollbar")
                    .h(px(10.0))
                    .flex_1()
                    .relative()
                    .rounded_full()
                    .bg(rgb(0xf0f1f4))
                    .when(content > viewport + 1.0, |d| {
                        d.cursor_pointer().on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, event: &MouseDownEvent, _, cx| {
                                let Some(bounds) =
                                    this.video_timeline_bounds.lock().ok().and_then(|b| *b)
                                else {
                                    return;
                                };
                                let viewport = this.video_timeline_viewport_width();
                                let content = this.video_timeline_content_width();
                                let (left, width) = scrollbar_geometry(
                                    viewport,
                                    content,
                                    this.video_timeline_scroll,
                                );
                                let x = f64::from(event.position.x - bounds.origin.x);
                                if x < left || x > left + width {
                                    let target = ((x - width / 2.0) / (viewport - width).max(1.0))
                                        * (content - viewport);
                                    this.manually_pan_timeline(target - this.video_timeline_scroll);
                                }
                                this.timeline_ui.scrollbar_drag =
                                    Some((event.position.x, this.video_timeline_scroll));
                                cx.stop_propagation();
                                cx.notify();
                            }),
                        )
                    })
                    .child(
                        div()
                            .absolute()
                            .left(px(left as f32))
                            .top(px(1.0))
                            .w(px(width as f32))
                            .h(px(8.0))
                            .rounded_full()
                            .bg(rgb(0xb7bfcc)),
                    ),
            )
            .into_any_element()
    }

    pub(crate) fn timeline_context_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if matches!(
            self.timeline_ui.context,
            Some((TimelineContextTarget::AddMedia, _))
        ) {
            return None; // Rendered beside its toolbar trigger instead.
        }
        self.render_timeline_context_menu(cx)
    }

    fn render_timeline_context_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (target, position) = self.timeline_ui.context?;
        let mut menu = div()
            .id("timeline-context-menu")
            .w(px(224.0))
            .p_2()
            .rounded_lg()
            .bg(rgb(0xffffff))
            .border_1()
            .border_color(line())
            .shadow_lg()
            .flex()
            .flex_col()
            .gap_1()
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.timeline_ui.context = None;
                cx.notify();
            }));
        let option = |id,
                      label,
                      enabled,
                      cx: &mut Context<Self>,
                      action: fn(&mut Self, &mut Context<Self>)| {
            self.timeline_button(id, label, label, enabled, false, cx, move |this, cx| {
                this.timeline_ui.context = None;
                action(this, cx);
            })
        };
        match target {
            TimelineContextTarget::AddMedia => {
                menu = menu
                    .child(option(
                        "add-video",
                        "Image / video…",
                        true,
                        cx,
                        |this, cx| {
                            this.import_timeline_media(crate::media_ui::ImportKind::Clip, cx)
                        },
                    ))
                    .child(option(
                        "add-overlay",
                        "Image overlay…",
                        true,
                        cx,
                        |this, cx| {
                            this.import_timeline_media(crate::media_ui::ImportKind::Image, cx)
                        },
                    ))
                    .child(option("add-audio", "Audio…", true, cx, |this, cx| {
                        this.import_timeline_media(crate::media_ui::ImportKind::Audio, cx)
                    }));
            }
            TimelineContextTarget::Clip(id, time) => {
                menu = menu
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted())
                            .px_2()
                            .py_1()
                            .child(format!("Clip · {}", precise_timecode(time))),
                    )
                    .child(self.timeline_button(
                        "context-split",
                        "Split here",
                        "Split at the clicked time",
                        !self.video_edit_busy,
                        false,
                        cx,
                        move |this, cx| {
                            this.timeline_ui.context = None;
                            if this.select_timeline_context_target(target)
                                && this
                                    .video_clip_timeline
                                    .editor_range(id)
                                    .is_some_and(|range| range.contains(&time))
                            {
                                this.split_video_clip_at(time, cx);
                            }
                        },
                    ));
                for (label, speed) in [("0.5×", 0.5), ("1×", 1.0), ("2×", 2.0), ("4×", 4.0)] {
                    let active = self
                        .video_clip_timeline
                        .segments
                        .iter()
                        .find(|c| c.id == id)
                        .is_some_and(|c| (c.speed - speed).abs() < 0.001);
                    menu = menu.child(self.timeline_button(
                        label,
                        label,
                        "Set clip speed",
                        !self.video_edit_busy,
                        active,
                        cx,
                        move |this, cx| {
                            this.timeline_ui.context = None;
                            if this.select_timeline_context_target(target) {
                                this.set_selected_video_clip_speed(speed, cx);
                            }
                        },
                    ));
                }
                menu = menu
                    .child(self.timeline_button(
                        "context-speed",
                        "Custom speed…",
                        "Change this clip's speed",
                        !self.video_edit_busy,
                        false,
                        cx,
                        move |this, cx| {
                            this.timeline_ui.context = None;
                            if !this.select_timeline_context_target(target) {
                                return;
                            }
                            this.video_speed_draft = this
                                .video_clip_timeline
                                .segments
                                .iter()
                                .find(|clip| clip.id == id)
                                .map(|clip| clip.speed);
                            cx.notify();
                        },
                    ))
                    .child(self.timeline_button(
                        "context-delete",
                        "Delete clip",
                        "Delete this clip",
                        !self.video_edit_busy && self.video_clip_timeline.segments.len() > 1,
                        false,
                        cx,
                        move |this, cx| {
                            this.timeline_ui.context = None;
                            if this.select_timeline_context_target(target) {
                                this.delete_selected_video_clip(cx);
                            }
                        },
                    ));
            }
            TimelineContextTarget::Motion(_) => {
                menu = menu.child(
                    div()
                        .text_xs()
                        .text_color(muted())
                        .px_2()
                        .py_1()
                        .child("Motion region"),
                );
                for (label, zoom) in [
                    ("1.25× zoom", 1.25),
                    ("1.5× zoom", 1.5),
                    ("2× zoom", 2.0),
                    ("3× zoom", 3.0),
                ] {
                    menu = menu.child(self.timeline_button(
                        label,
                        label,
                        "Set motion magnification",
                        !self.video_edit_busy,
                        false,
                        cx,
                        move |this, cx| {
                            this.timeline_ui.context = None;
                            if this.select_timeline_context_target(target) {
                                this.edit_selected_region(|cue| cue.zoom = zoom);
                            }
                            cx.notify();
                        },
                    ));
                }
                menu = menu
                    .child(self.timeline_button(
                        "context-toggle-motion",
                        "Enable / disable",
                        "Enable or disable this motion region",
                        !self.video_edit_busy,
                        false,
                        cx,
                        move |this, _| {
                            this.timeline_ui.context = None;
                            if this.select_timeline_context_target(target) {
                                this.edit_selected_region(|cue| cue.is_enabled = !cue.is_enabled);
                            }
                        },
                    ))
                    .child(self.timeline_button(
                        "context-remove-motion",
                        "Remove motion",
                        "Remove this motion region",
                        !self.video_edit_busy,
                        false,
                        cx,
                        move |this, cx| {
                            this.timeline_ui.context = None;
                            if this.select_timeline_context_target(target) {
                                this.delete_selected_video_zoom(cx);
                            }
                        },
                    ));
            }
            TimelineContextTarget::EmptyMotion(time) => {
                menu = menu.child(self.timeline_button(
                    "context-add-motion",
                    "Add motion here",
                    "Add motion at the clicked time",
                    !self.video_edit_busy,
                    false,
                    cx,
                    move |this, cx| {
                        this.timeline_ui.context = None;
                        this.add_motion_region_at(time, cx);
                    },
                ));
            }
        }
        let anchored = if matches!(target, TimelineContextTarget::AddMedia) {
            gpui::anchored()
                .position_mode(gpui::AnchoredPositionMode::Local)
                .position(point(px(0.0), px(32.0)))
        } else {
            gpui::anchored().position(position)
        };
        Some(
            gpui::deferred(
                anchored
                    .snap_to_window_with_margin(gpui::Edges {
                        top: px(8.0),
                        right: px(8.0),
                        bottom: px(8.0),
                        left: px(8.0),
                    })
                    .child(menu),
            )
            .with_priority(2)
            .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scrollbar_endpoints_and_minimum_thumb_match_scroll_range() {
        assert_eq!(scrollbar_geometry(600.0, 1200.0, 0.0), (0.0, 300.0));
        assert_eq!(scrollbar_geometry(600.0, 1200.0, 600.0), (300.0, 300.0));
        assert_eq!(scrollbar_geometry(600.0, 100000.0, 99400.0), (568.0, 32.0));
        assert_eq!(scrollbar_geometry(600.0, 600.0, 0.0), (0.0, 600.0));
    }
}
