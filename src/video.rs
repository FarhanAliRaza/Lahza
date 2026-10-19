//! Video timeline editing, preview rendering, playback, and clip controls.

use super::{
    ink, muted, PlaybackMailbox, PlaybackUpdates, Studio, VideoEditSnapshot, VideoMoveDrag,
    VideoTrimDrag, VideoZoomDrag, VideoZoomDragKind,
};
use crate::recording::{
    camera_playback::CameraPlaybackDecoder,
    clips::{ClipEdge, RecordingClipSegment, RecordingClipTimeline},
    model::RecordingSession,
    playback::TimelinePlaybackStream,
    pointer_timeline::PointerTimeline,
    video::{decode_frame, render_denoised_copy},
    viewport::{ViewportTimeline, ZoomCue},
};
use gpui::{
    div, hsla, prelude::*, px, rgb, svg, AnyElement, Context, FontWeight, IntoElement, MouseButton,
    Pixels, Timer,
};
use std::{
    path::PathBuf,
    sync::{atomic::Ordering, Arc},
    time::{Duration, Instant},
};
use uuid::Uuid;

/// Geometry for a recording's independent, scrollable time ruler.
struct RecordingTimelineGeometry {
    duration: f64,
    seconds_per_pixel: f64,
}

impl RecordingTimelineGeometry {
    fn new(span: f64, extent: f64, media_duration: f64, viewport_width: f64, zoom: f64) -> Self {
        let span = span.max(0.001);
        Self {
            duration: extent.max(media_duration).max(span) + span,
            seconds_per_pixel: span / (viewport_width * zoom).max(1.0),
        }
    }
}

impl VideoTrimDrag {
    fn preview(&self, pointer_x: Pixels, scroll: f64, source_duration: f64) -> Option<RecordingClipTimeline> {
        let range = self.original_timeline.editor_range(self.original_clip.id)?;
        let edge_time = match self.edge {
            ClipEdge::Leading => range.start,
            ClipEdge::Trailing => range.end,
        };
        let pixels = f64::from((pointer_x - self.start_x) / px(1.0)) + scroll - self.start_scroll;
        let mut delta = pixels * self.editor_seconds_per_pixel;
        // The parked playhead is the trim target. Snap the edge to it without
        // ever seeking or changing the source frame under retained content.
        if pixels.abs() > f64::EPSILON
            && (edge_time + delta - self.original_position).abs() <= 8.0 * self.editor_seconds_per_pixel
        {
            delta = self.original_position - edge_time;
        }
        self.original_timeline.trimming(self.original_clip.id, self.edge, delta, source_duration)
            .map(|(timeline, _)| timeline)
    }
}

impl Studio {
    /// Width of the visible timeline strip, measured on the last paint. The
    /// strip stretches with the window, so seek and drag math must use the
    /// same width the clips were laid out with.
    pub(super) fn video_timeline_viewport_width(&self) -> f64 {
        self.video_timeline_bounds
            .lock()
            .ok()
            .and_then(|bounds| *bounds)
            .map(|bounds| (bounds.size.width / px(1.0)) as f64)
            .filter(|width| *width > 1.0)
            .unwrap_or(600.0)
    }

    /// The ruler has its own extent and scale. Editing clips must never fit
    /// the remaining media back into the viewport. Leave room to place clips
    /// beyond the current end; this empty workspace is not exported.
    pub(crate) fn video_timeline_duration(&self) -> f64 {
        if self.video_project.is_some() {
            self.recording_timeline_geometry().duration
        } else {
            self.video_duration.max(f64::EPSILON)
        }
    }

    pub(crate) fn video_timeline_seconds_per_pixel(&self) -> f64 {
        if self.video_project.is_some() {
            self.recording_timeline_geometry().seconds_per_pixel
        } else {
            self.video_duration.max(f64::EPSILON)
                / (self.video_timeline_viewport_width() * self.video_timeline_zoom).max(1.0)
        }
    }

    fn recording_timeline_geometry(&self) -> RecordingTimelineGeometry {
        RecordingTimelineGeometry::new(
            self.video_timeline_span, self.video_timeline_extent, self.video_duration,
            self.video_timeline_viewport_width(), self.video_timeline_zoom,
        )
    }

    pub(crate) fn video_timeline_content_width(&self) -> f64 {
        self.video_timeline_duration() / self.video_timeline_seconds_per_pixel()
    }

    pub(super) fn zoom_video_timeline(&mut self, factor: f64, anchor_time: f64) {
        const MAX_POINTS_PER_SECOND: f64 = 240.0;
        const MAX_CONTENT_WIDTH: f64 = 100_000.0;
        if self.video_duration <= 0.0 || !factor.is_finite() || factor <= 0.0 {
            return;
        }
        let viewport_width = self.video_timeline_viewport_width();
        let duration = self.video_timeline_duration();
        let maximum_width = MAX_CONTENT_WIDTH.min(MAX_POINTS_PER_SECOND * duration);
        let previous_zoom = self.video_timeline_zoom;
        let maximum_zoom = (maximum_width / self.video_timeline_content_width() * previous_zoom).max(1.0);
        let next_zoom = (previous_zoom * factor).clamp(1.0, maximum_zoom);
        if (next_zoom - previous_zoom).abs() < 0.000_1 {
            return;
        }
        let anchor_time = anchor_time.clamp(0.0, duration);
        let anchor_viewport_x = anchor_time / self.video_timeline_seconds_per_pixel()
            - self.video_timeline_scroll;
        self.video_timeline_zoom = next_zoom;
        let next_anchor_x = anchor_time / self.video_timeline_seconds_per_pixel();
        let maximum_scroll = (self.video_timeline_content_width() - viewport_width).max(0.0);
        self.video_timeline_scroll = (next_anchor_x - anchor_viewport_x).clamp(0.0, maximum_scroll);
    }

    pub(super) fn pan_video_timeline(&mut self, delta: f64) {
        let maximum_scroll =
            (self.video_timeline_content_width() - self.video_timeline_viewport_width()).max(0.0);
        self.video_timeline_scroll =
            (self.video_timeline_scroll + delta).clamp(0.0, maximum_scroll);
    }

    pub(super) fn begin_video_trim(&mut self, clip_id: Uuid, edge: ClipEdge, start_x: Pixels) {
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
        self.video_seek_drag = None;
        self.video_move_drag = None;
        self.video_trim_drag = Some(VideoTrimDrag {
            start_x,
            start_scroll: self.video_timeline_scroll,
            original_position: self.video_position,
            original_timeline: self.video_clip_timeline.clone(),
            original_clip,
            edge,
            editor_seconds_per_pixel: self.video_timeline_seconds_per_pixel(),
        });
    }

    pub(super) fn update_video_trim(&mut self, pointer_x: Pixels) {
        let Some(drag) = self.video_trim_drag.as_ref() else {
            return;
        };
        let Some(timeline) = drag.preview(pointer_x, self.video_timeline_scroll, self.video_source_duration) else {
            return;
        };
        self.video_clip_timeline = timeline;
        self.video_duration = self.video_clip_timeline.duration();
    }

    pub(super) fn commit_video_trim(&mut self, cx: &mut Context<Self>) {
        let Some(drag) = self.video_trim_drag.take() else {
            return;
        };
        let requested = self.video_clip_timeline.clone();
        let selected = Some(drag.original_clip.id);
        self.video_position = drag.original_position;
        self.video_clip_timeline = drag.original_timeline;
        self.video_duration = self.video_clip_timeline.duration();
        self.apply_video_clip_timeline(requested, selected, true, cx);
    }

    pub(super) fn begin_video_move_drag(&mut self, clip_id: Uuid, start_x: Pixels) {
        self.finish_annotation_interaction();
        self.pause_video_playback();
        self.video_selected_clip = Some(clip_id);
        self.video_selected_zoom_cue = None;
        self.video_seek_drag = None;
        self.video_trim_drag = None;
        self.video_zoom_drag = None;
        self.video_move_drag = Some(VideoMoveDrag {
            clip_id,
            start_x,
            current_x: start_x,
            start_scroll: self.video_timeline_scroll,
            editor_seconds_per_pixel: self.video_timeline_seconds_per_pixel(),
            active: false,
        });
    }

    /// A press selects immediately; a small motion threshold prevents a
    /// normal click from accidentally repositioning the clip.
    pub(super) fn update_video_move_drag(&mut self, pointer_x: Pixels) -> bool {
        let Some(drag) = self.video_move_drag.as_mut() else { return false; };
        drag.current_x = pointer_x;
        drag.active |= (pointer_x - drag.start_x).abs() > px(6.0);
        drag.active
    }

    pub(super) fn finish_video_move_drag(&mut self, pointer_x: Pixels, cx: &mut Context<Self>) -> bool {
        self.update_video_move_drag(pointer_x);
        let Some(drag) = self.video_move_drag.take() else { return false; };
        if drag.active {
            self.commit_video_move_drag(drag, cx);
        } else if let Some(position) = self.motion_timeline_time_at(pointer_x) {
            // Like Cap, a clip click selects and seeks on release. Waiting
            // until release lets a drag move the clip without scrubbing.
            self.seek_video(position, cx);
        }
        cx.notify();
        true
    }

    /// Where the dragged clip's content would start in editor time, given
    /// the drag's pixel displacement, including any scrolling during the
    /// gesture. The time scale is captured when the drag begins.
    pub(super) fn video_move_new_start(&self, drag: &VideoMoveDrag) -> Option<f64> {
        let range = self.video_clip_timeline.editor_range(drag.clip_id)?;
        let content_width = self.video_timeline_content_width();
        if content_width <= 0.0 || self.video_duration <= 0.0 {
            return None;
        }
        let seconds_per_pixel = drag.editor_seconds_per_pixel;
        let delta = (f64::from((drag.current_x - drag.start_x) / px(1.0))
            + self.video_timeline_scroll - drag.start_scroll) * seconds_per_pixel;
        // Place clips anywhere in the scrollable workspace.
        let mut new_start = (range.start + delta).clamp(0.0, self.video_timeline_duration());
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

    pub(super) fn commit_video_move_drag(&mut self, drag: VideoMoveDrag, cx: &mut Context<Self>) {
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

    pub(super) fn begin_video_zoom_drag(
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
            editor_seconds_per_pixel: self.video_timeline_seconds_per_pixel(),
        });
    }

    pub(super) fn update_video_zoom_drag(&mut self, pointer_x: Pixels) {
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

    pub(super) fn commit_video_zoom_drag(&mut self, cx: &mut Context<Self>) {
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

    pub(super) fn persist_video_zoom_cues(&mut self, cx: &mut Context<Self>) {
        self.persist_video_zoom_cues_quiet();
        cx.notify();
    }

    /// Autosaves the motion lane; screenshot animations have no project
    /// package yet and simply keep their regions in memory.
    pub(super) fn persist_video_zoom_cues_quiet(&mut self) {
        let Some(session) = self.video_project.as_ref() else {
            return;
        };
        if let Err(error) = session.write_zoom_cues_draft(&self.video_zoom_cues) {
            self.toast = Some(format!("Could not autosave motion edit: {error}").into());
        }
    }

    pub(super) fn delete_selected_video_zoom(&mut self, cx: &mut Context<Self>) -> bool {
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

    pub(super) fn mutate_selected_zoom_cue(
        &mut self,
        cx: &mut Context<Self>,
        mutate: impl FnOnce(&mut ZoomCue),
    ) {
        self.edit_selected_region(mutate);
        cx.notify();
    }

    pub(super) fn add_video_zoom_at_playhead(&mut self, cx: &mut Context<Self>) {
        let position = self.video_position;
        self.add_motion_region_at(position, cx);
    }

    pub(super) fn undo_video_edit(&mut self, cx: &mut Context<Self>) {
        let Some(previous) = self.video_undo_stack.pop() else {
            return;
        };
        match previous {
            VideoEditSnapshot::Audio(clips) => {
                self.video_redo_stack.push(VideoEditSnapshot::Audio(std::mem::replace(&mut self.video_audio_clips, clips)));
                self.save_audio_clips(cx);
            }
            VideoEditSnapshot::Crop(crop) => {
                self.video_redo_stack.push(VideoEditSnapshot::Crop(std::mem::replace(&mut self.video_crop, crop)));
                self.autosave_scene_style();
                cx.notify();
            }
            VideoEditSnapshot::Annotations(marks) => {
                self.video_redo_stack.push(VideoEditSnapshot::Annotations(
                    std::mem::replace(&mut self.annotations, marks)));
                self.finish_annotation_interaction();
                let _ = self.rebuild_redactions();
                cx.notify();
            }
            VideoEditSnapshot::ImageTiming { scene, start, end } => {
                self.video_redo_stack.push(VideoEditSnapshot::ImageTiming { scene: self.animation_duration, start: self.animation_image_start, end: self.animation_image_end });
                self.restore_image_timing(scene, start, end);
                cx.notify();
            }
            VideoEditSnapshot::Clips { timeline, annotations } => {
                self.video_redo_stack
                    .push(VideoEditSnapshot::Clips { timeline: self.video_clip_timeline.clone(), annotations: self.annotations.clone() });
                let selected = timeline.segments.first().map(|clip| clip.id);
                self.apply_video_clip_timeline(timeline, selected, false, cx);
                self.annotations = annotations;
                self.finish_annotation_interaction();
                let _ = self.rebuild_redactions();
                cx.notify();
            }
            VideoEditSnapshot::Zoom(cues) => {
                self.video_redo_stack
                    .push(VideoEditSnapshot::Zoom(self.video_zoom_cues.clone()));
                self.video_zoom_cues = cues;
                self.video_selected_zoom_cue = None;
                self.rebuild_video_motion_timelines();
                self.persist_video_zoom_cues(cx);
            }
            snapshot @ (VideoEditSnapshot::Transcript(_) | VideoEditSnapshot::TranscriptCut { .. }) => {
                let inverse = self.restore_transcript_snapshot(snapshot, cx);
                self.video_redo_stack.push(inverse);
            }
        }
    }

    pub(super) fn redo_video_edit(&mut self, cx: &mut Context<Self>) {
        let Some(next) = self.video_redo_stack.pop() else {
            return;
        };
        match next {
            VideoEditSnapshot::Audio(clips) => {
                self.video_undo_stack.push(VideoEditSnapshot::Audio(std::mem::replace(&mut self.video_audio_clips, clips)));
                self.save_audio_clips(cx);
            }
            VideoEditSnapshot::Crop(crop) => {
                self.video_undo_stack.push(VideoEditSnapshot::Crop(std::mem::replace(&mut self.video_crop, crop)));
                self.autosave_scene_style();
                cx.notify();
            }
            VideoEditSnapshot::Annotations(marks) => {
                self.video_undo_stack.push(VideoEditSnapshot::Annotations(
                    std::mem::replace(&mut self.annotations, marks)));
                self.finish_annotation_interaction();
                let _ = self.rebuild_redactions();
                cx.notify();
            }
            VideoEditSnapshot::ImageTiming { scene, start, end } => {
                self.video_undo_stack.push(VideoEditSnapshot::ImageTiming { scene: self.animation_duration, start: self.animation_image_start, end: self.animation_image_end });
                self.restore_image_timing(scene, start, end);
                cx.notify();
            }
            VideoEditSnapshot::Clips { timeline, annotations } => {
                self.video_undo_stack
                    .push(VideoEditSnapshot::Clips { timeline: self.video_clip_timeline.clone(), annotations: self.annotations.clone() });
                let selected = timeline.segments.first().map(|clip| clip.id);
                self.apply_video_clip_timeline(timeline, selected, false, cx);
                self.annotations = annotations;
                self.finish_annotation_interaction();
                let _ = self.rebuild_redactions();
                cx.notify();
            }
            VideoEditSnapshot::Zoom(cues) => {
                self.video_undo_stack
                    .push(VideoEditSnapshot::Zoom(self.video_zoom_cues.clone()));
                self.video_zoom_cues = cues;
                self.video_selected_zoom_cue = None;
                self.rebuild_video_motion_timelines();
                self.persist_video_zoom_cues(cx);
            }
            snapshot @ (VideoEditSnapshot::Transcript(_) | VideoEditSnapshot::TranscriptCut { .. }) => {
                let inverse = self.restore_transcript_snapshot(snapshot, cx);
                self.video_undo_stack.push(inverse);
            }
        }
    }

    pub(super) fn delete_selected_video_edit(&mut self, cx: &mut Context<Self>) {
        if self.delete_selected_video_zoom(cx) { return; }
        if self.animation_active && self.video_project.is_none() {
            if self.video_selected_clip.is_some() && self.animation_image_end > self.animation_image_start {
                self.video_undo_stack.push(VideoEditSnapshot::ImageTiming {
                    scene: self.animation_duration,
                    start: self.animation_image_start,
                    end: self.animation_image_end,
                });
                self.video_redo_stack.clear();
                self.restore_image_timing(self.animation_duration, 0.0, 0.0);
                self.video_selected_clip = None;
                cx.notify();
            }
        } else {
            self.delete_selected_video_clip(cx);
        }
    }

    pub(super) fn video_playback_path(&self) -> Option<PathBuf> {
        self.video_media_source()
    }

    /// The recording the editor plays and cuts: the noise-reduced copy when
    /// that option is on and its render has finished, else the original.
    pub(super) fn video_media_source(&self) -> Option<PathBuf> {
        self.video_project
            .as_ref()
            .map(|session| Self::media_source_for(session, self.video_noise_reduction))
    }

    pub(super) fn media_source_for(session: &RecordingSession, noise_reduction: bool) -> PathBuf {
        let denoised = session.denoised_path();
        if noise_reduction && denoised.exists() {
            denoised
        } else {
            crate::recording::media::source_path(session).unwrap_or_else(|_| session.screen_path())
        }
    }

    /// Flips noise reduction, rendering the denoised copy on first use and
    /// refreshing whatever the editor is playing once it is ready.
    pub(super) fn set_video_noise_reduction(&mut self, enabled: bool, cx: &mut Context<Self>) {
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
        let source = Self::media_source_for(&session, false);
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

    /// Source changes only require a seek; cuts stay virtual until export.
    pub(super) fn refresh_video_media_source(&mut self, cx: &mut Context<Self>) {
        self.seek_video(self.video_position, cx);
    }

    /// Keep Fill's follow camera in step with the style and motion edits.
    /// A rebuilt motion timeline carries no reframe, so it is rebuilt here.
    pub(super) fn sync_reframe(&mut self) {
        if self.video_project.is_none() {
            return;
        }
        let style = self.scene_style();
        let Some(fill) = style.fill_fraction(self.video_source_size.0, self.video_source_size.1) else {
            self.video_viewport_timeline.clear_reframe();
            return;
        };
        let key = crate::recording::viewport::ReframeKey { fill, crop: style.source_crop.validated() };
        if self.video_viewport_timeline.reframe().map(|reframe| reframe.key) == Some(key) {
            return;
        }
        // Mid-drag, reshape the existing camera; rebuild it once the drag ends.
        if (self.slider_drag.is_some() || self.media_drag.is_some())
            && self.video_viewport_timeline.reshape_reframe(fill)
        {
            return;
        }
        let pointer = &self.video_pointer_timeline;
        self.video_viewport_timeline.set_reframe(fill, style.source_crop, |time| {
            pointer.frame_at(time).map(|frame| frame.location)
        });
    }

    pub(super) fn rebuild_video_motion_timelines(&mut self) {
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
        let mut pointer = PointerTimeline::build_with_clip_timeline(
            capture.clone(),
            self.video_source_duration,
            manifest.pixel_width as f64,
            manifest.pixel_height as f64,
            self.pointer_style.timeline_options(),
            Some(&self.video_clip_timeline),
        );
        pointer.restrict_to_recording(&self.video_clip_timeline, manifest.duration);
        self.video_viewport_timeline = ViewportTimeline::build(
            &self.video_zoom_cues,
            &pointer,
            &self.video_clip_timeline,
            &capture,
        );
        self.video_pointer_timeline = pointer;
    }

    pub(super) fn apply_video_clip_timeline(
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
                .push(VideoEditSnapshot::Clips { timeline: self.video_clip_timeline.clone(), annotations: self.annotations.clone() });
            self.video_redo_stack.clear();
        }
        let mapping_changed = !timeline.same_source_mapping(&self.video_clip_timeline);
        if mapping_changed {
            self.pause_video_playback();
            self.annotations = crate::timed::remap_clip_annotations(
                &self.annotations, &self.video_clip_timeline, &timeline);
            self.finish_annotation_interaction();
            let _ = self.rebuild_redactions();
        }
        // Keep the workspace (and the parked playhead) even after deleting or
        // trimming content at its end. Only explicit Fit changes the scale.
        self.video_timeline_extent = self.video_timeline_extent
            .max(self.video_duration).max(timeline.duration()).max(self.video_position);
        self.video_duration = timeline.duration();
        self.video_selected_clip = selected
            .filter(|id| timeline.segments.iter().any(|clip| clip.id == *id))
            .or_else(|| timeline.segments.first().map(|clip| clip.id));
        self.video_clip_timeline = timeline.clone();
        if mapping_changed {
            self.camera_decode_token = self.camera_decode_token.wrapping_add(1);
            self.camera_decode_in_flight = false;
            self.camera_frame_rgba = None;
            self.camera_decoded_time = -1.0;
            if self.video_selected_zoom_cue.is_some_and(|id| {
                self.video_zoom_cues.iter().find(|cue| cue.id == id).is_none_or(|cue|
                    timeline.slices_overlapping(cue.start, cue.end).is_empty())
            }) {
                self.video_selected_zoom_cue = None;
            }
            self.rebuild_video_motion_timelines();
        }

        let Some(session) = self.video_project.clone() else {
            return;
        };
        if let Err(error) = session.write_clip_timeline_draft(&timeline) {
            self.toast = Some(format!("Could not autosave clip edit: {error}").into());
            cx.notify();
            return;
        }
        if mapping_changed {
            self.seek_video(self.video_position, cx);
        }
        cx.notify();
    }

    pub(super) fn split_video_clip(&mut self, cx: &mut Context<Self>) {
        if let Some((timeline, selected)) = self.video_clip_timeline.split_at(self.video_position) {
            self.apply_video_clip_timeline(timeline, Some(selected), true, cx);
        }
    }

    pub(super) fn delete_selected_video_clip(&mut self, cx: &mut Context<Self>) {
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

    pub(super) fn next_clip_speed(speed: f64, increase: bool) -> f64 {
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

    pub(super) fn set_selected_video_clip_speed(&mut self, speed: f64, cx: &mut Context<Self>) {
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

    pub(super) fn start_video_playback(&mut self, cx: &mut Context<Self>) {
        if self.crop_active || self.video_playing || self.video_duration <= 0.0 || self.video_edit_busy {
            return;
        }
        let Some(path) = self.video_playback_path() else {
            return;
        };
        self.finish_annotation_interaction();
        if self.video_position >= self.video_duration - 0.01 {
            self.video_position = 0.0;
        }
        let start_time = self.video_position;
        let decode_size = if self.video_crop != crate::CropRect::UNIT { self.video_source_size } else { (1920, 1080) };
        let timeline = self.video_clip_timeline.clone();
        let muted = self.video_audio_muted;
        let audio_clips = self.video_project.as_ref().map(|session| self.video_audio_clips.iter().cloned()
            .map(|clip| (session.directory.join(&clip.file), clip)).collect()).unwrap_or_default();
        let generation = self.video_playback_generation.clone();
        let token = generation.fetch_add(1, Ordering::SeqCst) + 1;
        let receiver = PlaybackMailbox::default();
        let mut camera_ready = None;
        let recording_duration = self.video_recording_duration;
        let sender = if let Some(path) = self.video_camera_path.clone() {
            let pending = PlaybackMailbox::default();
            let raw_sender = pending.clone();
            let paired_sender = receiver.clone();
            let camera_generation = generation.clone();
            let camera_timeline = timeline.clone();
            let (ready_sender, ready_receiver) = std::sync::mpsc::sync_channel(1);
            camera_ready = Some(ready_receiver);
            // Blocking pipe reads use a dedicated worker so they cannot
            // occupy the executor thread needed by the screen/audio transport.
            std::thread::spawn(move || {
                let mut decoder = Some(CameraPlaybackDecoder::new(path));
                let cancelled = || camera_generation.load(Ordering::SeqCst) != token
                    || Arc::strong_count(&paired_sender.0) <= 1;
                // Open FFmpeg and decode its first frame before allowing the
                // master transport to start its screen/audio clock.
                // Starting in a gap warms the upcoming clip, without showing
                // its camera until actual content begins.
                let source_time = camera_timeline.location_at(start_time).map(|location| location.source_time).filter(|time| *time < recording_duration);
                if let Some(Err(error)) = decoder.as_mut().map(|decoder| decoder.frame_at(source_time, &cancelled)) {
                    eprintln!("Webcam playback failed: {error}");
                    decoder = None;
                }
                if cancelled() || ready_sender.send(()).is_err() { return; }
                while !cancelled() {
                    let update = pending.take();
                    let had_frame = update.frame.is_some();
                    if let Some(frame) = update.frame {
                        let source_time = camera_timeline.content_source_time_at(frame.time).filter(|time| *time < recording_duration);
                        let camera = match decoder.as_mut().filter(|_| source_time.is_some())
                            .map(|decoder| decoder.frame_at(source_time, &cancelled)) {
                            Some(Ok(camera)) => camera,
                            Some(Err(error)) => {
                                eprintln!("Webcam playback failed: {error}");
                                decoder = None;
                                None
                            }
                            None => None,
                        };
                        if cancelled() { break; }
                        paired_sender.publish_with_camera(frame, camera);
                    }
                    if let Some(terminal) = update.terminal {
                        paired_sender.finish(terminal);
                        break;
                    }
                    if !had_frame { std::thread::sleep(Duration::from_millis(2)); }
                }
            });
            raw_sender
        } else {
            receiver.clone()
        };
        // Invalidate any still-frame seek that predates playback.
        self.camera_decode_token = self.camera_decode_token.wrapping_add(1);
        self.camera_decode_in_flight = false;
        self.video_playing = true;
        self.toast = None;

        cx.background_executor()
            .spawn(async move {
                if let Some(ready) = camera_ready {
                    if !crate::recording::camera_playback::wait_until_ready(&ready, || {
                        generation.load(Ordering::SeqCst) != token || Arc::strong_count(&sender.0) <= 1
                    }) {
                        sender.finish(Err("Webcam playback initialization stopped".into()));
                        return;
                    }
                }
                let mut stream =
                    match TimelinePlaybackStream::open(&path, timeline, start_time, decode_size.0, decode_size.1, muted) {
                        Ok(stream) => stream,
                        Err(error) => {
                            sender.finish(Err(error.to_string()));
                            return;
                        }
                    };
                let mut audio = match crate::recording::media::AudioPlayback::open(audio_clips) {
                    Ok(audio) => audio,
                    Err(error) => { sender.finish(Err(error.to_string())); return; }
                };
                while generation.load(Ordering::SeqCst) == token && Arc::strong_count(&sender.0) > 1
                {
                    match stream.next_frame(|| generation.load(Ordering::SeqCst) != token || Arc::strong_count(&sender.0) <= 1) {
                        Ok(Some(frame)) => {
                            if let Err(error) = audio.sync(frame.time) {
                                sender.finish(Err(error.to_string()));
                                break;
                            }
                            sender.publish(frame);
                        }
                        Ok(None) => {
                            sender.finish(Ok(()));
                            break;
                        }
                        Err(error) => {
                            sender.finish(Err(error.to_string()));
                            break;
                        }
                    }
                }
                stream.stop();
            })
            .detach();

        let active_generation = self.video_playback_generation.clone();
        // Screen frames arrive at 30 fps, but the cursor and camera are
        // animated at 120 Hz. Between frames the playhead advances on the
        // wall clock, capped at the last observed frame interval so it never
        // runs ahead of the next frame, and the scene re-renders at 60 Hz.
        let mut clock: Option<(Instant, f64)> = None;
        let mut frame_interval = 1.0 / 30.0;
        let mut last_advance = Instant::now();
        cx.spawn(async move |weak, cx| loop {
            Timer::after(Duration::from_millis(8)).await;
            if active_generation.load(Ordering::SeqCst) != token {
                break;
            }
            let PlaybackUpdates {
                frame: latest_frame,
                camera,
                terminal,
            } = receiver.take();
            let terminal_received = terminal.is_some();
            let now = Instant::now();
            let predicted = match (&latest_frame, clock) {
                (Some(frame), _) => {
                    if let Some((_, previous)) = clock {
                        let interval = frame.time - previous;
                        if interval > 0.0 && interval < 0.5 {
                            frame_interval = interval;
                        }
                    }
                    clock = Some((now, frame.time));
                    Some(frame.time)
                }
                (None, Some((since, time)))
                    if now.duration_since(last_advance) >= Duration::from_millis(15) =>
                {
                    Some(time + since.elapsed().as_secs_f64().min(frame_interval))
                }
                _ => None,
            };
            if predicted.is_none() && !terminal_received {
                continue;
            }
            last_advance = now;
            if weak
                .update(cx, |this, cx| {
                    if let Some(frame) = latest_frame {
                        this.camera_decoded_time = camera.as_ref().map_or(-1.0, |(time, _)| *time);
                        this.camera_frame_rgba = camera.map(|(_, image)| image);
                        if let Some(pixels) =
                            image::RgbaImage::from_raw(frame.width, frame.height, frame.rgba)
                        {
                            this.set_video_frame(pixels);
                        }
                    }
                    if let Some(time) = predicted {
                        this.video_position = time.min(this.video_duration).max(this.video_position);
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

    pub(super) fn pause_video_playback(&mut self) {
        self.camera_decode_token = self.camera_decode_token.wrapping_add(1);
        self.camera_decode_in_flight = false;
        self.video_playback_generation
            .fetch_add(1, Ordering::SeqCst);
        self.video_playing = false;
    }

    pub(super) fn seek_video(&mut self, position: f64, cx: &mut Context<Self>) {
        self.annotation_edit_preview_pending = false;
        self.annotation_editing_time = None;
        self.stop_editing_text();
        self.seek_video_frame(position, cx);
    }

    /// Decode an explicitly requested video frame without entering annotation editing.
    pub(crate) fn seek_video_frame(&mut self, position: f64, cx: &mut Context<Self>) {
        self.annotation_edit_preview_pending = false;
        self.annotation_editing_time = None;
        let playback_path = self.video_playback_path();
        self.pause_video_playback();
        let position = position.clamp(0.0, self.video_timeline_duration());
        self.video_position = position;
        self.camera_frame_rgba = None;
        self.camera_decoded_time = -1.0;
        if self.selected_annotation.is_some_and(|index| {
            self.annotations.get(index).is_none_or(|mark| {
                mark.timing
                    .is_some_and(|timing| !timing.state_at(position).visible)
            })
        }) {
            self.finish_annotation_interaction();
        }
        let Some(path) = playback_path else {
            cx.notify();
            return;
        };
        let generation = self.video_playback_generation.clone();
        let token = generation.fetch_add(1, Ordering::SeqCst) + 1;
        if position > self.video_duration
            || self.video_clip_timeline.location_at(position).is_some_and(|location| position < location.editor_start)
        {
            self.set_video_frame(image::RgbaImage::from_pixel(16, 9, image::Rgba([0, 0, 0, 255])));
            cx.notify();
            return;
        }
        let source_time = self.video_clip_timeline.source_time_at(position);
        let decode_size = if self.crop_active || self.video_crop != crate::CropRect::UNIT { self.video_source_size } else { (2560, 1440) };
        let task = cx.background_executor().spawn(async move {
            decode_frame(&path, source_time, decode_size.0, decode_size.1).map_err(|error| error.to_string())
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

    pub(super) fn video_timecode(value: f64) -> String {
        let seconds = value.max(0.0).floor() as u64;
        format!("{:02}:{:02}", seconds / 60, seconds % 60)
    }

    /// Modal that previews a clip speed change before applying it.
    pub(super) fn video_speed_dialog(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
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

    pub(super) fn video_edit_controls(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
            .child(self.small_button("import-timeline-clip", "+ Image / video", !edit_busy, cx, |this, cx| this.import_timeline_media(crate::media_ui::ImportKind::Clip, cx)))
            .child(self.small_button("import-image-overlay", "+ Image overlay", !edit_busy, cx, |this, cx| this.import_timeline_media(crate::media_ui::ImportKind::Image, cx)))
            .child(self.small_button("import-audio", "+ Audio", !edit_busy, cx, |this, cx| this.import_timeline_media(crate::media_ui::ImportKind::Audio, cx)))
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
}


#[cfg(test)]
mod timeline_interaction_tests {
    use super::*;

    fn trim_drag(edge: ClipEdge, position: f64) -> VideoTrimDrag {
        let timeline = RecordingClipTimeline::full(10.0);
        VideoTrimDrag {
            start_x: px(100.0),
            start_scroll: 200.0,
            original_position: position,
            original_clip: timeline.segments[0].clone(),
            original_timeline: timeline,
            edge,
            editor_seconds_per_pixel: 0.01,
        }
    }

    #[test]
    fn leading_handle_snaps_to_parked_frame_at_a_scrolled_zoom() {
        let drag = trim_drag(ClipEdge::Leading, 4.0);
        let trimmed = drag.preview(px(497.0), 200.0, 10.0).unwrap();
        assert_eq!(trimmed.editor_range(drag.original_clip.id).unwrap().start, 4.0);
        assert_eq!(trimmed.source_time_at(drag.original_position), 4.0);
        assert_eq!(drag.original_position, 4.0);
        // Scrolling by 100 pixels during the drag has the same effect as
        // moving the pointer by that amount in the content.
        assert_eq!(drag.preview(px(397.0), 300.0, 10.0).unwrap(), trimmed);
    }

    #[test]
    fn trailing_handle_can_cross_target_and_return_without_losing_it() {
        let drag = trim_drag(ClipEdge::Trailing, 6.0);
        let shortened = drag.preview(px(-600.0), 200.0, 10.0).unwrap();
        assert_eq!(shortened.duration(), 3.0);
        let at_target = drag.preview(px(-297.0), 200.0, 10.0).unwrap();
        assert_eq!(at_target.duration(), 6.0);
        assert_eq!(at_target.source_time_at(drag.original_position), 6.0);
        assert_eq!(drag.preview(drag.start_x, drag.start_scroll, 10.0).unwrap(), drag.original_timeline);
    }

    #[test]
    fn trim_and_move_do_not_change_ruler_scale_or_playhead_coordinates() {
        let drag = trim_drag(ClipEdge::Trailing, 6.0);
        let before = RecordingTimelineGeometry::new(10.0, 10.0, 10.0, 600.0, 2.0);
        let trimmed = drag.preview(px(-600.0), 200.0, 10.0).unwrap();
        let after = RecordingTimelineGeometry::new(10.0, 10.0, trimmed.duration(), 600.0, 2.0);
        let playhead_x = |geometry: &RecordingTimelineGeometry| {
            drag.original_position / geometry.duration
                * (geometry.duration / geometry.seconds_per_pixel) - 200.0
        };
        assert_eq!(before.seconds_per_pixel, after.seconds_per_pixel);
        assert_eq!(before.duration, after.duration);
        assert_eq!(playhead_x(&before), playhead_x(&after));
        let moved = trimmed.repositioning(drag.original_clip.id, 15.0).unwrap();
        let extended = RecordingTimelineGeometry::new(10.0, moved.duration(), moved.duration(), 600.0, 2.0);
        assert!(extended.duration > before.duration);
        assert_eq!(playhead_x(&before), playhead_x(&extended));
        // Undoing a move preserves the scrollable extent as well as scale.
        let undone = RecordingTimelineGeometry::new(10.0, moved.duration(), 10.0, 600.0, 2.0);
        assert_eq!(undone.duration, extended.duration);
    }

    #[test]
    #[ignore = "requires X11, python3 and libXtst (run under Xvfb)"]
    fn rendered_clip_click_seeks_and_drag_moves_without_scrubbing() {
        use crate::{asset_directory, open_studio_window, Assets};
        use gpui::{point, Application};
        // XTest produces the native XI2 events GPUI uses on Linux.
        fn pointer(x: i32, y: i32, button: &str) {
            let status = std::process::Command::new("python3").arg("-c").arg(r#"
import ctypes, sys
x11 = ctypes.CDLL('libX11.so.6')
xtst = ctypes.CDLL('libXtst.so.6')
x11.XOpenDisplay.restype = ctypes.c_void_p
display = ctypes.c_void_p(x11.XOpenDisplay(None))
assert display.value
xtst.XTestFakeMotionEvent(display, -1, int(sys.argv[1]), int(sys.argv[2]), 0)
if sys.argv[3] != 'move':
    xtst.XTestFakeButtonEvent(display, 1, sys.argv[3] == 'down', 0)
x11.XSync(display, False)
x11.XCloseDisplay(display)
"#).arg(x.to_string()).arg(y.to_string()).arg(button).status().unwrap();
            assert!(status.success());
        }
        let root = std::env::temp_dir().join(format!("lahza-timeline-input-{}", Uuid::new_v4()));
        let session = RecordingSession::create_in(&root).unwrap();
        Application::new().with_assets(Assets { base: asset_directory() }).run(move |cx| {
            let handle = open_studio_window(cx, true, |handle, cx| {
                cx.new(|cx| Studio::new(handle, None, None, cx))
            }).unwrap();
            let clip_id = handle.update(cx, |studio, _, cx| {
                studio.load_generation = studio.load_generation.wrapping_add(1);
                studio.loading = None;
                studio.launcher_active = false;
                studio.launcher_window = false;
                studio.video_project = Some(session);
                studio.video_clip_timeline = RecordingClipTimeline::full(10.0);
                studio.video_duration = 10.0;
                studio.video_source_duration = 10.0;
                studio.video_timeline_span = 20.0;
                studio.video_timeline_extent = 20.0;
                studio.video_position = 4.0;
                studio.video_selected_clip = None;
                studio.set_video_frame(image::RgbaImage::from_pixel(160, 90, image::Rgba([50, 70, 100, 255])));
                cx.notify();
                studio.video_clip_timeline.segments[0].id
            }).unwrap();
            cx.spawn(async move |cx| {
                Timer::after(Duration::from_millis(200)).await;
                let window_handle: gpui::AnyWindowHandle = handle.into();
                window_handle.update(cx, |_, window, cx| {
                    window.draw(cx).clear();
                    window.draw(cx).clear();
                }).unwrap();
                let (start, drag_pixels, expected_start) = handle.update(cx, |studio, window, _| {
                    let bounds = studio.video_timeline_bounds.lock().unwrap()
                        .expect("rendered video timeline bounds");
                    let origin = window.bounds().origin;
                    // Release beyond the original clip's right edge.
                    let drag_pixels = (studio.video_timeline_viewport_width() * 0.55).round() as i32;
                    (point(origin.x + bounds.origin.x + px(40.0), origin.y + bounds.origin.y + px(17.0)),
                        drag_pixels, f64::from(drag_pixels) * studio.video_timeline_seconds_per_pixel())
                }).unwrap();
                let x = f32::from(start.x) as i32;
                let y = f32::from(start.y) as i32;
                pointer(x, y, "down");
                Timer::after(Duration::from_millis(100)).await;
                let expected_click_time = handle.update(cx, |studio, _, _| {
                    assert_eq!(studio.video_selected_clip, Some(clip_id), "clip press must select the video");
                    assert!(studio.video_move_drag.is_some(), "clip press must arm dragging");
                    assert!(studio.video_seek_drag.is_none(), "clip selection must not arm scrubbing");
                    assert_eq!(studio.video_position, 4.0, "mouse-down must wait to distinguish a click from a drag");
                    // Use window-local native coordinates, excluding any
                    // X11 window decoration offset from the global pointer.
                    let bounds = studio.video_timeline_bounds.lock().unwrap().unwrap();
                    let local_x = studio.video_move_drag.unwrap().start_x - bounds.origin.x;
                    f64::from(local_x / px(1.0)) * studio.video_timeline_seconds_per_pixel()
                }).unwrap();
                pointer(x, y, "up");
                Timer::after(Duration::from_millis(100)).await;
                let clicked_position = handle.update(cx, |studio, _, _| {
                    assert!(studio.video_move_drag.is_none());
                    assert_eq!(studio.video_clip_timeline.editor_range(clip_id).unwrap().start, 0.0,
                        "a plain click must not reposition the clip");
                    let expected = expected_click_time;
                    assert!((studio.video_position - expected).abs() < studio.video_timeline_seconds_per_pixel(),
                        "a clip click must seek: actual {}, expected {expected}, seconds/pixel {}",
                        studio.video_position, studio.video_timeline_seconds_per_pixel());
                    assert!(studio.video_seek_drag.is_none());
                    studio.video_position
                }).unwrap();
                pointer(x, y, "down");
                Timer::after(Duration::from_millis(100)).await;
                pointer(x + drag_pixels, y, "move");
                Timer::after(Duration::from_millis(100)).await;
                handle.update(cx, |studio, _, _| {
                    let drag = studio.video_move_drag.unwrap();
                    assert!(drag.active, "dragging the clip must activate the move");
                    assert_eq!(studio.video_position, clicked_position, "clip dragging must not scrub");
                }).unwrap();
                pointer(x + drag_pixels, y, "up");
                Timer::after(Duration::from_millis(100)).await;
                handle.update(cx, |studio, _, cx| {
                    assert!(studio.video_move_drag.is_none());
                    let actual_start = studio.video_clip_timeline.editor_range(clip_id).unwrap().start;
                    // XI2 fixed-point coordinates round slightly; require
                    // placement within one physical pixel of the drop.
                    assert!((actual_start - expected_start).abs() < studio.video_timeline_seconds_per_pixel(),
                        "drop placed clip at {actual_start}, expected {expected_start}");
                    assert_eq!(studio.video_position, clicked_position);
                    let moved = studio.video_clip_timeline.clone();
                    studio.undo_video_edit(cx);
                    assert_eq!(studio.video_clip_timeline.editor_range(clip_id).unwrap().start, 0.0);
                    studio.redo_video_edit(cx);
                    assert_eq!(studio.video_clip_timeline, moved);
                    assert_eq!(studio.video_project.as_ref().unwrap().effective_clip_timeline(10.0).unwrap(), moved);
                }).unwrap();
                cx.update(|cx| cx.quit()).unwrap();
                std::fs::remove_dir_all(&root).unwrap();
            }).detach();
        });
    }

}
