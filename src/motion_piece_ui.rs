//! Studio UI for motion pieces: a "Motion" lane with one bar per piece,
//! dragging a bar moves it, and the inspector shows its placement. Their
//! layers are edited in the Motion tab (`motion_timeline`). Frames, sound
//! and export come from `motion_pieces`.

use crate::motion_pieces::MotionPieces;
use crate::Studio;
use gpui::{div, prelude::*, px, AnyElement, Context, CursorStyle, MouseButton, MouseDownEvent, Pixels, RenderImage, Timer};
use lahza_motion_core::recording::{self, Placement};
use crate::recording::export::ExportFormat;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// Slider id routed through `Studio::set_slider_value`.
pub(crate) const PIECE_OPACITY_SLIDER: usize = 8;
const ROW_HEIGHT: f32 = 28.0;

/// Row of each piece so overlapping pieces stack instead of covering
/// each other (first row that is free at the piece's start).
fn rows(pieces: &MotionPieces) -> (Vec<usize>, usize) {
    let mut order: Vec<usize> = (0..pieces.items.len()).collect();
    order.sort_by(|&a, &b| pieces.items[a].piece.start.total_cmp(&pieces.items[b].piece.start));
    let mut ends: Vec<f64> = Vec::new();
    let mut row = vec![0; pieces.items.len()];
    for i in order {
        let p = &pieces.items[i];
        let r = ends.iter().position(|&end| end <= p.piece.start + 1e-9).unwrap_or(ends.len());
        if r == ends.len() {
            ends.push(0.0);
        }
        ends[r] = p.end();
        row[i] = r;
    }
    (row, ends.len().max(1))
}

pub(crate) struct PieceDrag {
    id: String,
    start_x: Pixels,
    start_scroll: f64,
    seconds_per_pixel: f64,
    original_start: f64,
    active: bool,
}

impl Studio {
    /// Keeps the open recording's motion pieces loaded and polled; called
    /// every render of the video editor.
    pub(crate) fn ensure_motion_pieces(&mut self, cx: &mut Context<Self>) {
        let dir = self.video_project.as_ref().map(|s| s.directory.clone());
        if self.motion_pieces.as_ref().map(|m| &m.recording) == dir.as_ref() {
            return;
        }
        self.selected_motion_piece = None;
        self.motion_piece_drag = None;
        let old: Vec<Arc<RenderImage>> = self.motion_pieces.take().map(|m| m.frames.into_iter().map(|f| f.image).collect()).unwrap_or_default();
        self.retired_images.extend(old);
        self.motion_pieces_loop += 1;
        let Some(dir) = dir else { return };
        self.motion_pieces = Some(MotionPieces::load(&dir));
        let token = self.motion_pieces_loop;
        // Wake the editor when frames finish; reload on disk changes.
        cx.spawn(async move |weak, cx| {
            let mut tick = 0u32;
            loop {
                Timer::after(Duration::from_millis(16)).await;
                tick += 1;
                let alive = weak.update(cx, |this, cx| {
                    if this.motion_pieces_loop != token {
                        return false;
                    }
                    let dragging = this.motion_piece_drag.is_some();
                    let Some(pieces) = this.motion_pieces.as_mut() else { return false };
                    let mut changed = pieces.frames_ready();
                    if tick % 30 == 0 && !dragging && pieces.poll() {
                        changed = true;
                        let agent_edit = std::mem::take(&mut pieces.agent_edit);
                        let missing = this.selected_motion_piece.as_ref().is_some_and(|id| pieces.get(id).is_none());
                        if missing {
                            this.selected_motion_piece = None;
                        }
                        // The edit document changed on disk: an agent may
                        // have added sounds to the video too.
                        this.reload_external_audio_clips();
                        if agent_edit {
                            this.reload_after_agent_edit(cx);
                        }
                    }
                    if changed {
                        cx.notify();
                    }
                    true
                });
                if !matches!(alive, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }

    /// An agent edited the recording through MCP (`recording_edit`,
    /// `recording_export`): reload it in the background, keeping the
    /// playhead and the undo history, then run any export it asked for.
    fn reload_after_agent_edit(&mut self, cx: &mut Context<Self>) {
        let Some(dir) = self.video_project.as_ref().map(|s| s.directory.clone()) else { return };
        let load = dir.clone();
        let task = cx.background_executor().spawn(async move { crate::capture::PreparedVideoProject::load(load) });
        cx.spawn(async move |weak, cx| {
            let result = task.await;
            let _ = weak.update(cx, |this, cx| {
                if this.video_project.as_ref().map(|s| &s.directory) != Some(&dir) {
                    return;
                }
                match result {
                    Ok(prepared) => {
                        let position = this.video_position;
                        let undo = std::mem::take(&mut this.video_undo_stack);
                        let redo = std::mem::take(&mut this.video_redo_stack);
                        this.parked_video_history = Some((dir.clone(), undo, redo));
                        this.apply_video_project(prepared);
                        this.seek_video_frame(position, cx);
                        this.run_agent_export(cx);
                    }
                    Err(error) => {
                        this.toast = Some(crate::Notification::error(format!("Could not reload the agent's edit: {error}")));
                        Self::report_agent_export(&dir, recording::take_export_request(&dir), Err(error));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Runs the export an agent left for this recording, if any, through
    /// the same path as the Export button, without the file picker.
    fn run_agent_export(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.video_project.clone() else { return };
        let dir = session.directory.clone();
        let Some(job) = recording::take_export_request(&dir) else { return };
        if self.video_edit_busy || self.export_progress.is_some() {
            return Self::report_agent_export(&dir, Some(job), Err("an export or edit is already running".into()));
        }
        if let Err(error) = session.commit_draft() {
            return Self::report_agent_export(&dir, Some(job), Err(format!("could not save the recording's edits: {error}")));
        }
        let format = ExportFormat::from_path(&job.path).unwrap_or(ExportFormat::Mp4);
        let (source, mut request) = self.video_export_request(&session, format);
        if format != self.export_format {
            request.frame_rate = format.default_frame_rate();
        }
        request.destination = format.apply_to_path(&job.path);
        let running = recording::ExportStatus { id: job.id, path: request.destination.clone(), state: recording::ExportState::Running, error: None };
        let _ = recording::write_export_status(&dir, &running);
        self.run_scene_export(source, request, move |result| {
            let status = match result {
                Ok(()) => recording::ExportStatus { state: recording::ExportState::Done, ..running },
                Err(error) => recording::ExportStatus { state: recording::ExportState::Failed, error: Some(error.clone()), ..running },
            };
            let _ = recording::write_export_status(&dir, &status);
        }, cx);
    }

    fn report_agent_export(dir: &std::path::Path, job: Option<recording::ExportJob>, result: Result<(), String>) {
        let Some(job) = job else { return };
        let (state, error) = match result {
            Ok(()) => (recording::ExportState::Done, None),
            Err(error) => (recording::ExportState::Failed, Some(error)),
        };
        let _ = recording::write_export_status(dir, &recording::ExportStatus { id: job.id, path: job.path, state, error });
    }

    /// Swaps in finished piece frames and asks for the ones at the playhead:
    /// full-screen pieces for the `canvas` (preview pixels), over-the-video
    /// pieces at the recording's shape for the compositor's media layer.
    /// Returns the full-screen frames to draw over the canvas.
    pub(crate) fn motion_piece_frames(&mut self, canvas: (f64, f64)) -> Vec<(Arc<RenderImage>, Placement)> {
        let time = self.timeline_display_time();
        let proxy = self.video_playing || self.motion_piece_drag.is_some();
        let media = self.media_dimensions().map(|(w, h)| {
            let s = (canvas.0 / w as f64).min(canvas.1 / h as f64).min(1.0) * if proxy { 0.5 } else { 1.0 };
            (((w as f64 * s).round() as u32).max(2), ((h as f64 * s).round() as u32).max(2))
        });
        let Some(pieces) = self.motion_pieces.as_mut() else { return Vec::new() };
        let old = pieces.take_frames();
        self.retired_images.extend(old);
        let pieces = self.motion_pieces.as_mut().unwrap();
        pieces.request(time, canvas.0, media, proxy);
        if !pieces.any_active(time, Placement::Full) {
            return Vec::new();
        }
        pieces.frames.iter().map(|f| (f.image.clone(), f.placement)).collect()
    }

    /// Motion pieces' sound, placed on the video timeline.
    pub(crate) fn motion_piece_audio(&self) -> Vec<(PathBuf, crate::recording::media::AudioClip)> {
        self.motion_pieces.as_ref().map(MotionPieces::timeline_audio).unwrap_or_default()
    }

    /// Height of the "Motion" lane, or None when there are no pieces.
    pub(crate) fn motion_piece_lane_height(&self) -> Option<f32> {
        let pieces = self.motion_pieces.as_ref().filter(|m| !m.items.is_empty())?;
        Some(rows(pieces).1 as f32 * ROW_HEIGHT)
    }

    /// The "Motion" lane: one bar per piece.
    pub(crate) fn motion_piece_lane(&self, scroll: f64, cx: &mut Context<Self>) -> Option<AnyElement> {
        let pieces = self.motion_pieces.as_ref().filter(|m| !m.items.is_empty())?;
        let seconds_per_pixel = self.video_timeline_seconds_per_pixel();
        let (row_of, count) = rows(pieces);
        let mut lane = div().relative().w_full().flex_none().h(px(count as f32 * ROW_HEIGHT)).overflow_hidden().rounded_lg().bg(crate::theme::control_bg());
        for (index, p) in pieces.items.iter().enumerate() {
            let id = p.piece.id.clone();
            let selected = self.selected_motion_piece.as_deref() == Some(id.as_str());
            let left = (p.piece.start / seconds_per_pixel - scroll) as f32;
            let width = ((p.duration() / seconds_per_pixel) as f32).max(4.0);
            let label = format!("{} · {}", p.piece.name, p.piece.placement.label());
            lane = lane.child(
                div()
                    .id(gpui::ElementId::Name(format!("motion-piece-{id}").into()))
                    .absolute()
                    .top(px(row_of[index] as f32 * ROW_HEIGHT + 2.0))
                    .left(px(left))
                    .w(px(width))
                    .h(px(ROW_HEIGHT - 4.0))
                    .rounded_md()
                    .border_2()
                    .border_color(if selected { crate::theme::ink() } else { gpui::transparent_black() })
                    .hover(|s| s.border_color(crate::ink().opacity(0.4)))
                    .bg(crate::theme::accent())
                    .when(p.piece.hidden, |d| d.opacity(0.4))
                    .text_color(gpui::white())
                    .text_xs()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .flex()
                    .items_center()
                    .overflow_hidden()
                    .cursor(CursorStyle::OpenHand)
                    .when(width >= crate::timeline_ui::REGION_LABEL_MIN_WIDTH, |d| d.child(crate::timeline_ui::region_label(label)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            this.focus_handle.focus(window);
                            this.begin_piece_drag(&id, event.position.x);
                            cx.notify();
                        }),
                    ),
            );
        }
        Some(lane.into_any_element())
    }

    fn begin_piece_drag(&mut self, id: &str, x: Pixels) {
        let Some(original_start) = self.motion_pieces.as_ref().and_then(|m| m.get(id)).map(|p| p.piece.start) else { return };
        self.pause_video_playback();
        self.select_motion_piece(Some(id.to_string()));
        self.motion_piece_drag = Some(PieceDrag {
            id: id.to_string(),
            start_x: x,
            start_scroll: self.video_timeline_scroll,
            seconds_per_pixel: self.video_timeline_seconds_per_pixel(),
            original_start,
            active: false,
        });
    }

    /// Moves the dragged piece along the timeline (in memory).
    pub(crate) fn update_piece_drag(&mut self, x: Pixels) -> bool {
        let Some(drag) = self.motion_piece_drag.as_mut() else { return false };
        drag.active |= (x - drag.start_x).abs() > px(4.0);
        if !drag.active {
            return false;
        }
        let delta = (f64::from((x - drag.start_x) / px(1.0)) + self.video_timeline_scroll - drag.start_scroll) * drag.seconds_per_pixel;
        let start = (drag.original_start + delta).max(0.0);
        let id = drag.id.clone();
        if let Some(p) = self.motion_pieces.as_mut().and_then(|m| m.items.iter_mut().find(|p| p.piece.id == id)) {
            p.piece.start = start;
        }
        true
    }

    /// Ends a drag: saves a move, or seeks on a plain click.
    pub(crate) fn finish_piece_drag(&mut self, x: Pixels, cx: &mut Context<Self>) -> bool {
        self.update_piece_drag(x);
        let Some(drag) = self.motion_piece_drag.take() else { return false };
        if drag.active {
            self.save_motion_pieces();
        } else if let Some(t) = self.motion_timeline_time_at(x) {
            self.seek_video(t, cx);
        }
        cx.notify();
        true
    }

    pub(crate) fn select_motion_piece(&mut self, id: Option<String>) {
        if id.is_some() {
            self.select_inspector_tab(crate::shell_ui::InspectorTab::Motion);
        }
        self.selected_motion_piece = id;
    }

    fn save_motion_pieces(&mut self) {
        let Some(m) = self.motion_pieces.as_mut() else { return };
        let pieces = m.pieces();
        if let Err(e) = m.save(pieces) {
            self.toast = Some(crate::Notification::error(format!("Could not save the motion design: {e}")));
        }
    }

    fn edit_selected_piece(&mut self, edit: impl FnOnce(&mut lahza_motion_core::recording::MotionPiece)) {
        let Some(id) = self.selected_motion_piece.clone() else { return };
        let Some(m) = self.motion_pieces.as_mut() else { return };
        let mut pieces = m.pieces();
        if let Some(p) = pieces.iter_mut().find(|p| p.id == id) {
            edit(p);
        }
        if let Err(e) = m.save(pieces) {
            self.toast = Some(crate::Notification::error(format!("Could not save the motion design: {e}")));
        }
    }

    pub(crate) fn set_motion_piece_opacity(&mut self, percent: u8) {
        self.edit_selected_piece(|p| p.opacity = (percent as f64 / 100.0).clamp(0.0, 1.0));
    }

    fn remove_selected_piece(&mut self) {
        let Some(id) = self.selected_motion_piece.take() else { return };
        let Some(m) = self.motion_pieces.as_mut() else { return };
        let pieces = m.pieces().into_iter().filter(|p| p.id != id).collect();
        if let Err(e) = m.save(pieces) {
            self.toast = Some(crate::Notification::error(format!("Could not remove the motion design: {e}")));
        }
    }

    /// Inspector for the selected piece; None when none is selected.
    pub(crate) fn motion_piece_inspector(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let id = self.selected_motion_piece.as_ref()?;
        let p = self.motion_pieces.as_ref()?.get(id)?;
        let opacity = (p.piece.opacity * 100.0).round() as u8;
        let summary = format!(
            "{} – {} · {} × {} · {} scenes",
            crate::units::timecode(p.piece.start),
            crate::units::timecode(p.end()),
            p.project.settings.width,
            p.project.settings.height,
            p.project.scenes.len()
        );
        let row = crate::controls::SliderRow {
            slider_id: PIECE_OPACITY_SLIDER,
            label: "Opacity".into(),
            fraction: opacity as f32 / 100.0,
            value: format!("{opacity}%").into(),
            can_decrease: opacity > 0,
            can_increase: opacity < 100,
            show_reset: opacity != 100,
        };
        Some(
            div()
                .flex()
                .flex_col()
                .gap_3()
                .child(div().text_sm().font_weight(gpui::FontWeight::SEMIBOLD).child(p.piece.name.clone()))
                .child(div().text_xs().text_color(crate::theme::muted()).child(summary))
                .child(crate::controls::field(
                    "Placement",
                    self.segmented(
                        "motion-piece-placement",
                        &["Over the video", "Full screen"],
                        if p.piece.placement == Placement::Overlay { 0 } else { 1 },
                        |this, i| this.edit_selected_piece(|p| p.placement = if i == 0 { Placement::Overlay } else { Placement::Full }),
                        cx,
                    ),
                ))
                .child(self.slider_control(
                    row,
                    |_| true,
                    move |this, steps| this.set_motion_piece_opacity((opacity as i32 + steps * 5).clamp(0, 100) as u8),
                    |this| this.set_motion_piece_opacity(100),
                    cx,
                ))
                .child(div().text_xs().text_color(crate::theme::muted()).child(
                    "Edit its layers and sounds on the timeline, or click one in the preview. Isolate shows the design alone.",
                ))
                .child(div().flex().justify_end().child(self.selection_action_button("motion-piece-delete", true, true, cx, |this, _| this.remove_selected_piece())))
                .into_any_element(),
        )
    }

}
