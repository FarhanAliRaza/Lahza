//! Timeline navigation, controls and contextual actions shared by both editors.
use crate::timeline_edit::precise_timecode;
use crate::controls::{button, icon_button, ButtonKind, ButtonSize};
use crate::models::VideoZoomDragKind;
use crate::recording::clips::ClipEdge;
use crate::theme::{accent_hover, CONTROL_SM, RADIUS_SM};
use crate::{accent, control_bg, control_hover, ink, line, muted, on_accent, surface, Studio, TextRoles};
use gpui::{
    div, point, prelude::*, px, svg, AnyElement, App, Context, CursorStyle, Div, ElementId,
    FontWeight, KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, Point, Render,
    SharedString, Stateful, Timer, Window,
};
use std::{rc::Rc, time::Duration};
use uuid::Uuid;

#[derive(Clone, Copy)]
pub(crate) enum TimelineContextTarget {
    Clip(Uuid, f64),
    Motion(Uuid),
    Camera(Uuid),
    Audio(Uuid),
    EmptyMotion(f64),
    AddMedia,
}

type ContextAction = Rc<dyn Fn(&mut Studio, &mut Context<Studio>)>;

/// One row of the timeline context menu. Rows are data so the pointer and
/// the keyboard (arrows + Enter) activate exactly the same actions.
struct ContextItem {
    label: SharedString,
    shortcut: Option<&'static str>,
    enabled: bool,
    active: bool,
    action: ContextAction,
}

fn context_item(
    label: impl Into<SharedString>,
    enabled: bool,
    action: impl Fn(&mut Studio, &mut Context<Studio>) + 'static,
) -> ContextItem {
    ContextItem { label: label.into(), shortcut: None, enabled, active: false, action: Rc::new(action) }
}

/// Tooltip on the selected clip or region, naming the keys that edit it.
pub(crate) const REGION_KEYS_HELP: &str =
    "Drag to move · Alt+←/→ nudges a frame (Alt+Shift: 1 s) · [ / ] trim to playhead";

const TRIM_GROUP: &str = "timeline-trim-handle";

/// The one trim-handle pattern every lane uses: a 16 px occluding hit box
/// straddling the edge, a thin grip, a hover tint and a resize cursor. The
/// grip shows on the selected region and whenever the handle is hovered,
/// so edges respond on hover without a selection first. A region narrower
/// than two handles splits them into top and bottom halves so neither
/// edge steals the other's target. `origin` is the x of time zero, where
/// the hit box stops so a region starting at zero keeps its full target.
#[allow(clippy::too_many_arguments)]
pub(crate) fn trim_handle(
    id: impl Into<ElementId>,
    origin: f32,
    edge_x: f32,
    leading: bool,
    region_width: f32,
    top: f32,
    height: f32,
    selected: bool,
    keyboard: bool,
) -> Stateful<Div> {
    let hit_left = (edge_x - if leading { 5.0 } else { 11.0 }).max(origin);
    let grip_left = (edge_x - hit_left - 1.5).clamp(0.0, 13.0);
    let (top, height) = if region_width < 32.0 {
        (top + if leading { 0.0 } else { height / 2.0 }, height / 2.0)
    } else {
        (top, height)
    };
    let grip_height = (height - 6.0).clamp(6.0, 18.0);
    let help = match (leading, keyboard) {
        (true, true) => "Drag to trim start · [ trims to playhead · Escape cancels",
        (false, true) => "Drag to trim end · ] trims to playhead · Escape cancels",
        (true, false) => "Drag to trim start · Escape cancels",
        (false, false) => "Drag to trim end · Escape cancels",
    };
    div()
        .id(id)
        .group(TRIM_GROUP)
        .absolute()
        .left(px(hit_left))
        .top(px(top))
        .w(px(16.0))
        .h(px(height))
        .rounded(px(RADIUS_SM))
        .occlude()
        .cursor(CursorStyle::ResizeLeftRight)
        .tooltip(tooltip(help))
        .hover(|s| s.bg(gpui::white().opacity(0.25)))
        .child(
            div()
                .absolute()
                .left(px(grip_left))
                .top(px((height - grip_height) / 2.0))
                .w(px(3.0))
                .h(px(grip_height))
                .rounded_sm()
                .bg(gpui::white())
                .shadow_sm()
                .when(!selected, |grip| {
                    grip.opacity(0.0).group_hover(TRIM_GROUP, |s| s.opacity(1.0))
                }),
        )
}

/// A lane region's name: left-aligned and ellipsized, hidden when the
/// region is too narrow to read.
pub(crate) const REGION_LABEL_MIN_WIDTH: f32 = 36.0;

pub(crate) fn region_label(label: impl IntoElement) -> Div {
    div().flex_1().min_w_0().px_2().truncate().child(label)
}

pub(crate) struct TimelineUiState {
    pub snap_enabled: bool,
    pub snap_guide: Option<f64>,
    pub skim_enabled: bool,
    pub lane_height: f32,
    /// The user dragged the timeline edge; stop fitting it to the window.
    pub lane_height_resized: bool,
    pub ruler_hover: bool,
    pub hovered_clip: Option<Uuid>,
    pub hover_time: Option<f64>,
    pub vertical_scroll: gpui::ScrollHandle,
    context: Option<(TimelineContextTarget, Point<Pixels>)>,
    context_highlight: Option<usize>,
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
            lane_height: crate::shell_ui::DEFAULT_LANE_HEIGHT,
            lane_height_resized: false,
            ruler_hover: false,
            hovered_clip: None,
            hover_time: None,
            vertical_scroll: gpui::ScrollHandle::new(),
            context: None,
            context_highlight: None,
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
            .text_color(surface())
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
        self.camera_region_drag = None;
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
        if !force && self.timeline_ui.follow_suspended {
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
            self.timeline_ui.lane_height_resized = true;
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
        self.camera_region_drag.is_some() || self.video_trim_drag.is_some()
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
        } else if self.camera_region_drag.is_some() {
            self.update_camera_region_drag(pointer);
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
        if self.timeline_ui.context.is_some() && !command {
            match key.key.as_str() {
                "up" | "down" => {
                    self.move_context_highlight(key.key == "down");
                    return true;
                }
                "enter" => {
                    self.activate_context_highlight(cx);
                    return true;
                }
                _ => {}
            }
        }
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
                    "s" | "m" | "delete" | "backspace" | "v" | "t" | "space" | "[" | "]"
                ))
            || (key.modifiers.alt && matches!(key.key.as_str(), "left" | "right"))
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
            "left" | "right" if key.modifiers.alt => {
                let step = if key.modifiers.shift { 1.0 } else { self.timeline_frame_step() };
                let step = if key.key == "left" { -step } else { step };
                return self.keyboard_region_edit(VideoZoomDragKind::Move, |_, _| step, cx);
            }
            "[" => {
                let playhead = self.video_position;
                return self.keyboard_region_edit(
                    VideoZoomDragKind::Leading,
                    move |start, _| playhead - start,
                    cx,
                );
            }
            "]" => {
                let playhead = self.video_position;
                return self.keyboard_region_edit(
                    VideoZoomDragKind::Trailing,
                    move |_, end| playhead - end,
                    cx,
                );
            }
            "left" | "right" if self.selected_annotation.is_none() => {
                let step = if key.modifiers.shift { 1.0 } else { self.timeline_frame_step() };
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

    fn timeline_frame_step(&self) -> f64 {
        let source_rate = self
            .video_project
            .as_ref()
            .and(self.timeline_preview.frame_rate);
        let fps = source_rate
            .filter(|v| v.is_finite() && *v >= 1.0 && *v <= 120.0)
            .unwrap_or(self.export_frame_rate.max(1.0));
        1.0 / fps
    }

    /// Moves (Alt+arrows) or trims (`[` / `]`) the selected motion region,
    /// camera layout or clip by replaying the pointer gesture with an exact
    /// offset, so clamping, overlap rules and undo match a drag. `amount`
    /// maps the region's editor (start, end) to the seconds to shift the
    /// edited edge. Snapping is off: keys move by exactly what they say.
    fn keyboard_region_edit(
        &mut self,
        kind: VideoZoomDragKind,
        amount: impl Fn(f64, f64) -> f64,
        cx: &mut Context<Self>,
    ) -> bool {
        let seconds_per_pixel = self.video_timeline_seconds_per_pixel();
        let at = |seconds: f64| px((seconds / seconds_per_pixel) as f32);
        let origin = px(0.0);
        let snap = std::mem::replace(&mut self.timeline_ui.snap_enabled, false);
        let span = |this: &Self, start: f64, end: f64| {
            let slices = this.video_clip_timeline.slices_overlapping(start, end);
            Some((slices.first()?.editor_start, slices.last()?.editor_end))
        };
        let camera = self
            .selected_camera_region
            .and_then(|id| self.camera_regions.iter().find(|r| r.id == id))
            .and_then(|r| Some((r.id, span(self, r.start, r.end)?)));
        let handled = if let Some((id, (start, end))) = self
            .selected_region()
            .and_then(|cue| Some((cue.id, span(self, cue.start, cue.end)?)))
        {
            self.begin_video_zoom_drag(id, kind, start, end, origin);
            self.update_video_zoom_drag(at(amount(start, end)));
            self.commit_video_zoom_drag(cx);
            true
        } else if let Some((id, (start, end))) = camera {
            self.begin_camera_region_drag(id, kind, origin, start, end, cx);
            self.update_camera_region_drag(at(amount(start, end)));
            self.finish_camera_region_drag(cx);
            true
        } else if let Some((id, range)) = self
            .video_project
            .as_ref()
            .and(self.video_selected_clip)
            .and_then(|id| Some((id, self.video_clip_timeline.editor_range(id)?)))
        {
            let offset = at(amount(range.start, range.end));
            match kind {
                VideoZoomDragKind::Move => {
                    self.begin_video_move_drag(id, origin);
                    if let Some(mut drag) = self.video_move_drag.take() {
                        drag.current_x = offset;
                        drag.active = true;
                        self.commit_video_move_drag(drag, cx);
                    }
                }
                VideoZoomDragKind::Leading | VideoZoomDragKind::Trailing => {
                    let edge = if matches!(kind, VideoZoomDragKind::Leading) {
                        ClipEdge::Leading
                    } else {
                        ClipEdge::Trailing
                    };
                    self.begin_video_trim(id, edge, origin);
                    self.update_video_trim(offset);
                    self.commit_video_trim(cx);
                }
            }
            true
        } else {
            false
        };
        self.timeline_ui.snap_enabled = snap;
        if handled {
            self.follow_timeline_playhead(false);
        }
        handled
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
        if let TimelineContextTarget::Camera(id) = target {
            self.select_camera_region(id, cx);
        }
        self.timeline_ui.context = Some((target, position));
        self.timeline_ui.context_highlight = None;
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
            TimelineContextTarget::Camera(id) => {
                if !self.camera_regions.iter().any(|region| region.id == id) {
                    return false;
                }
                self.finish_annotation_interaction();
                self.selected_camera_region = Some(id);
                self.video_selected_zoom_cue = None;
                self.video_selected_clip = None;
            }
            TimelineContextTarget::Audio(id) => {
                if !self.video_audio_clips.iter().any(|clip| clip.id == id) {
                    return false;
                }
            }
            TimelineContextTarget::AddMedia | TimelineContextTarget::EmptyMotion(_) => {}
        }
        true
    }

    fn move_context_highlight(&mut self, forward: bool) {
        let Some((target, _)) = self.timeline_ui.context else {
            return;
        };
        let items = self.timeline_context_items(target);
        let count = items.len();
        let mut index = self.timeline_ui.context_highlight;
        for _ in 0..count {
            let next = match index {
                None if forward => 0,
                None => count - 1,
                Some(i) if forward => (i + 1) % count,
                Some(i) => (i + count - 1) % count,
            };
            index = Some(next);
            if items[next].enabled {
                break;
            }
        }
        self.timeline_ui.context_highlight = index.filter(|&i| items[i].enabled);
    }

    fn activate_context_highlight(&mut self, cx: &mut Context<Self>) {
        let Some((target, _)) = self.timeline_ui.context else {
            return;
        };
        let Some(item) = self
            .timeline_ui
            .context_highlight
            .and_then(|index| self.timeline_context_items(target).into_iter().nth(index))
            .filter(|item| item.enabled)
        else {
            return;
        };
        self.timeline_ui.context = None;
        (item.action)(self, cx);
        cx.notify();
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
        button(id, label, ButtonKind::Secondary, ButtonSize::Sm)
            .selected(active)
            .disabled(!enabled)
            .tooltip_opt(self.timeline_ui.context.is_none().then_some(help))
            .on_click(cx.listener(move |this, _, _, cx| {
                action(this, cx);
                cx.notify();
            }))
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn timeline_icon_button(
        &self,
        id: &'static str,
        icon: &'static str,
        kind: ButtonKind,
        help: &'static str,
        enabled: bool,
        cx: &mut Context<Self>,
        action: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> AnyElement {
        icon_button(id, icon, kind, ButtonSize::Sm)
            .disabled(!enabled)
            .tooltip_opt(self.timeline_ui.context.is_none().then_some(help))
            .on_click(cx.listener(move |this, _, _, cx| {
                action(this, cx);
                cx.notify();
            }))
            .into_any_element()
    }

    fn timeline_media_button(&self, cx: &mut Context<Self>) -> AnyElement {
        let open = matches!(
            self.timeline_ui.context,
            Some((TimelineContextTarget::AddMedia, _))
        );
        let enabled = !self.video_edit_busy;
        let button = button("timeline-add-media", "+ Media", ButtonKind::Secondary, ButtonSize::Sm)
            .selected(open)
            .disabled(!enabled)
            .tooltip_opt(
                self.timeline_ui.context.is_none().then_some("Import video, image overlay, or audio"),
            )
            .into_div()
            .when(enabled, |d| {
                d.cursor_pointer().on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        // Toggle on press so outside-dismiss cannot reopen it
                        // on the subsequent mouse-up over the same button.
                        this.timeline_ui.context = (!open)
                            .then_some((TimelineContextTarget::AddMedia, Point::default()));
                        this.timeline_ui.context_highlight = None;
                        cx.stop_propagation();
                        cx.notify();
                    }),
                )
                .hover(|s| s.bg(control_hover()))
            });
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
            .flex_1()
            .min_w_0()
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
                "+ Zoom",
                "Add zoom at playhead · M",
                !busy,
                false,
                cx,
                |this, cx| this.add_motion_region_at(this.video_position, cx),
            ))
            .when(!video, |d| {
                d.child(self.timeline_button(
                    "timeline-extend-scene",
                    "+ 5 s",
                    "Extend this scene by 5 s",
                    !busy,
                    false,
                    cx,
                    |this, _| this.set_animation_duration(this.animation_duration + 5.0),
                ))
            });
        let transport = div()
            .flex()
            .gap_1()
            .items_center()
            .flex_none()
            .child(self.timeline_icon_button(
                "timeline-start",
                "icons/skip-back.svg",
                ButtonKind::Ghost,
                "Go to start · Home",
                true,
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
                    .bg(accent())
                    .hover(|s| s.bg(accent_hover()))
                    .active(|s| s.opacity(0.85))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .tooltip(tooltip("Play / pause · Space"))
                    .child(
                        svg()
                            .path(if self.video_playing || self.video_audio_play_when_ready {
                                "icons/pause.svg"
                            } else {
                                "icons/play.svg"
                            })
                            .size(px(14.0))
                            .text_color(on_accent()),
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.timeline_ui.follow_suspended = false;
                        this.toggle_timeline_playback(cx);
                        cx.notify();
                    })),
            )
            .child(self.timeline_icon_button(
                "timeline-end",
                "icons/skip-forward.svg",
                ButtonKind::Ghost,
                "Go to end · End",
                true,
                cx,
                |this, cx| {
                    this.seek_video(this.video_duration, cx);
                    this.follow_timeline_playhead(true);
                },
            ));
        let zoom = div()
            .flex_1()
            .min_w_0()
            .flex()
            .gap_1()
            .items_center()
            .justify_end()
            .child(self.timeline_icon_button(
                "timeline-zoom-out",
                "icons/zoom-out.svg",
                ButtonKind::Secondary,
                "Zoom out · Ctrl+−",
                self.video_timeline_zoom > crate::video::MIN_TIMELINE_ZOOM + 0.0001,
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
            .child(self.timeline_icon_button(
                "timeline-zoom-in",
                "icons/zoom-in.svg",
                ButtonKind::Secondary,
                "Zoom in · Ctrl++ · Ctrl+wheel at pointer",
                true,
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
                            .flex_none()
                            .w(px(150.0))
                            .text_right()
                            .whitespace_nowrap()
                            .text_xs()
                            .tabular()
                            .tooltip(tooltip(if !video && self.image_scenes.len() > 1 {
                                format!(
                                    "Scene {} of {} · {} sequence",
                                    self.image_scene_index + 1,
                                    self.image_scenes.len(),
                                    crate::units::fmt_seconds(self.sequence_duration())
                                )
                            } else {
                                "Current time / duration".into()
                            }))
                            .text_color(ink())
                            .child(format!(
                                "{} / {}",
                                crate::units::timecode(self.video_position),
                                crate::units::timecode(self.video_duration)
                            )),
                    ),
            )
            .into_any_element()
    }

    pub(crate) fn timeline_resize_handle(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("timeline-resize")
            .h(px(12.0))
            .w_full()
            .flex_none()
            .cursor(CursorStyle::ResizeUpDown)
            .flex()
            .items_center()
            .justify_center()
            .hover(|s| s.bg(control_bg()))
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
        const GROUP: &str = "timeline-scrollbar";
        let viewport = self.video_timeline_viewport_width();
        let content = self.video_timeline_content_width();
        let (left, width) = scrollbar_geometry(viewport, content, self.video_timeline_scroll);
        let scrollable = content > viewport + 1.0;
        let dragging = self.timeline_ui.scrollbar_drag.is_some();
        div()
            .h(px(18.0))
            .flex_none()
            .pl(px(104.0))
            .flex()
            .items_center()
            .child(
                div()
                    .id("timeline-scrollbar")
                    .group(GROUP)
                    .h(px(12.0))
                    .flex_1()
                    .relative()
                    .rounded_full()
                    .bg(control_bg())
                    .when(scrollable, |d| {
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
                            .top(px(2.0))
                            .w(px(width as f32))
                            .h(px(8.0))
                            .rounded_full()
                            .bg(muted().opacity(if dragging { 0.85 } else { 0.45 }))
                            .when(scrollable && !dragging, |thumb| {
                                thumb.group_hover(GROUP, |s| s.bg(muted().opacity(0.7)))
                            }),
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

    fn context_header(&self, target: TimelineContextTarget) -> Option<SharedString> {
        Some(match target {
            TimelineContextTarget::Clip(_, time) => format!("Clip · {}", precise_timecode(time)).into(),
            TimelineContextTarget::Motion(_) => "Zoom".into(),
            TimelineContextTarget::Camera(_) => "Camera layout".into(),
            TimelineContextTarget::Audio(_) => "Music / SFX".into(),
            TimelineContextTarget::AddMedia | TimelineContextTarget::EmptyMotion(_) => return None,
        })
    }

    /// The rows for a right-clicked item. Each action re-selects the item
    /// that was right-clicked, even if another interaction changed the
    /// editor's current selection meanwhile.
    fn timeline_context_items(&self, target: TimelineContextTarget) -> Vec<ContextItem> {
        let ready = !self.video_edit_busy;
        match target {
            TimelineContextTarget::AddMedia => vec![
                context_item("Image / video…", true, |this, cx| {
                    this.import_timeline_media(crate::media_ui::ImportKind::Clip, cx)
                }),
                context_item("Image overlay…", true, |this, cx| {
                    this.import_timeline_media(crate::media_ui::ImportKind::Image, cx)
                }),
                context_item("Audio…", true, |this, cx| {
                    this.import_timeline_media(crate::media_ui::ImportKind::Audio, cx)
                }),
            ],
            TimelineContextTarget::Clip(id, time) => {
                let speed = self
                    .video_clip_timeline
                    .segments
                    .iter()
                    .find(|c| c.id == id)
                    .map(|c| c.speed);
                let mut items = vec![context_item("Split here", ready, move |this, cx| {
                    if this.select_timeline_context_target(target)
                        && this
                            .video_clip_timeline
                            .editor_range(id)
                            .is_some_and(|range| range.contains(&time))
                    {
                        this.split_video_clip_at(time, cx);
                    }
                })];
                for (label, value) in [("0.5×", 0.5), ("1×", 1.0), ("2×", 2.0), ("4×", 4.0)] {
                    let mut item = context_item(label, ready, move |this, cx| {
                        if this.select_timeline_context_target(target) {
                            this.set_selected_video_clip_speed(value, cx);
                        }
                    });
                    item.active = speed.is_some_and(|s| (s - value).abs() < 0.001);
                    items.push(item);
                }
                items.push(context_item("Custom speed…", ready, move |this, cx| {
                    if this.select_timeline_context_target(target) {
                        this.video_speed_draft = this
                            .video_clip_timeline
                            .segments
                            .iter()
                            .find(|clip| clip.id == id)
                            .map(|clip| clip.speed);
                        cx.notify();
                    }
                }));
                let mut delete = context_item(
                    "Delete clip",
                    ready && self.video_clip_timeline.segments.len() > 1,
                    move |this, cx| {
                        if this.select_timeline_context_target(target) {
                            this.delete_selected_video_clip(cx);
                        }
                    },
                );
                delete.shortcut = Some("Del");
                items.push(delete);
                items
            }
            TimelineContextTarget::Motion(id) => {
                let mut items: Vec<ContextItem> = [
                    ("1.25× zoom", 1.25),
                    ("1.5× zoom", 1.5),
                    ("2× zoom", 2.0),
                    ("3× zoom", 3.0),
                ]
                .into_iter()
                .map(|(label, zoom)| {
                    context_item(label, ready, move |this, _| {
                        if this.select_timeline_context_target(target) {
                            this.edit_selected_region(|cue| cue.zoom = zoom);
                        }
                    })
                })
                .collect();
                let enabled = self.video_zoom_cues.iter().find(|cue| cue.id == id).is_none_or(|cue| cue.is_enabled);
                items.push(context_item(if enabled { "Turn off zoom" } else { "Turn on zoom" }, ready, move |this, _| {
                    if this.select_timeline_context_target(target) {
                        this.edit_selected_region(|cue| cue.is_enabled = !cue.is_enabled);
                    }
                }));
                let mut delete = context_item("Delete zoom", ready, move |this, cx| {
                    if this.select_timeline_context_target(target) {
                        this.delete_selected_video_zoom(cx);
                    }
                });
                delete.shortcut = Some("Del");
                items.push(delete);
                items
            }
            TimelineContextTarget::Camera(_) => {
                let mut delete = context_item("Delete layout", ready, move |this, cx| {
                    if this.select_timeline_context_target(target) {
                        this.remove_selected_camera_region(cx);
                    }
                });
                delete.shortcut = Some("Del");
                vec![
                    context_item("Duplicate", ready, move |this, cx| {
                        if this.select_timeline_context_target(target) {
                            this.duplicate_camera_region(cx);
                        }
                    }),
                    delete,
                ]
            }
            TimelineContextTarget::Audio(id) => vec![
                context_item("Duplicate", ready, move |this, cx| {
                    if this.select_timeline_context_target(target) {
                        this.duplicate_audio_clip(id, cx);
                    }
                }),
                context_item("Delete", ready, move |this, cx| {
                    if this.select_timeline_context_target(target) {
                        this.delete_audio_clip(id, cx);
                    }
                }),
            ],
            TimelineContextTarget::EmptyMotion(time) => {
                vec![context_item("Add zoom here", ready, move |this, cx| {
                    this.add_motion_region_at(time, cx);
                })]
            }
        }
    }

    fn render_timeline_context_menu(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (target, position) = self.timeline_ui.context?;
        let highlight = self.timeline_ui.context_highlight;
        let rows = self
            .timeline_context_items(target)
            .into_iter()
            .enumerate()
            .map(|(index, item)| {
                let action = item.action.clone();
                div()
                    .id(("timeline-context-item", index))
                    .h(px(CONTROL_SM))
                    .px_3()
                    .flex()
                    .flex_none()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .rounded(px(RADIUS_SM))
                    .text_size(px(12.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(ink())
                    .when(highlight == Some(index), |row| row.bg(control_hover()))
                    .when(!item.enabled, |row| row.opacity(0.45))
                    .when(item.enabled, |row| {
                        row.cursor_pointer()
                            .hover(|s| s.bg(control_hover()))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.timeline_ui.context = None;
                                action(this, cx);
                                cx.notify();
                            }))
                    })
                    .child(div().min_w_0().truncate().child(item.label))
                    .when_some(item.shortcut, |row, keys| {
                        row.child(div().flex_none().text_color(muted()).child(keys))
                    })
                    .when(item.active, |row| {
                        row.child(svg().path("icons/check.svg").size(px(14.0)).flex_none().text_color(ink()))
                    })
            });
        let menu = div()
            .id("timeline-context-menu")
            .w(px(224.0))
            .p_1()
            .rounded_lg()
            .float()
            .flex()
            .flex_col()
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.timeline_ui.context = None;
                cx.notify();
            }))
            .when_some(self.context_header(target), |menu, header| {
                menu.child(div().px_3().py_1().micro().text_color(muted()).child(header))
            })
            .children(rows);
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
