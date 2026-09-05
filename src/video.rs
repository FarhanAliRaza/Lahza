//! Video timeline editing, preview rendering, playback, and clip controls.

use super::{
    ink, muted, PlaybackMailbox, PlaybackUpdates, Studio, VideoEditSnapshot, VideoMoveDrag,
    VideoTrimDrag, VideoZoomDrag, VideoZoomDragKind,
};
use crate::recording::{
    clips::{ClipEdge, RecordingClipSegment, RecordingClipTimeline},
    model::RecordingSession,
    pointer_timeline::PointerTimeline,
    video::{decode_frame, render_clip_preview, render_denoised_copy, SynchronizedPlaybackStream},
    viewport::{ViewportTimeline, ZoomCue},
};
use gpui::{
    div, hsla, prelude::*, px, rgb, svg, AnyElement, Context, FontWeight, IntoElement, MouseButton,
    Pixels, Timer,
};
use std::{
    fs,
    path::PathBuf,
    sync::{atomic::Ordering, Arc},
    time::Duration,
};
use uuid::Uuid;

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

    pub(super) fn zoom_video_timeline(&mut self, factor: f64, anchor_time: f64) {
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

    pub(super) fn pan_video_timeline(&mut self, delta: f64) {
        let viewport_width = self.video_timeline_viewport_width();
        let maximum_scroll = (viewport_width * self.video_timeline_zoom - viewport_width).max(0.0);
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
        self.video_trim_drag = Some(VideoTrimDrag {
            start_x,
            original_timeline: self.video_clip_timeline.clone(),
            original_clip,
            edge,
            editor_seconds_per_pixel: self.video_duration
                / (self.video_timeline_viewport_width() * self.video_timeline_zoom).max(1.0),
        });
    }

    pub(super) fn update_video_trim(&mut self, pointer_x: Pixels) {
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

    pub(super) fn commit_video_trim(&mut self, cx: &mut Context<Self>) {
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
    pub(super) fn video_move_new_start(&self, drag: &VideoMoveDrag) -> Option<f64> {
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
            editor_seconds_per_pixel: self.video_duration
                / (self.video_timeline_viewport_width() * self.video_timeline_zoom).max(1.0),
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

    pub(super) fn redo_video_edit(&mut self, cx: &mut Context<Self>) {
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

    pub(super) fn delete_selected_video_edit(&mut self, cx: &mut Context<Self>) {
        if !self.delete_selected_video_zoom(cx) {
            self.delete_selected_video_clip(cx);
        }
    }

    pub(super) fn video_playback_path(&self) -> Option<PathBuf> {
        self.video_preview_path
            .clone()
            .or_else(|| self.video_media_source())
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
            session.screen_path()
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
    pub(super) fn refresh_video_media_source(&mut self, cx: &mut Context<Self>) {
        if self.video_preview_path.is_some() {
            let timeline = self.video_clip_timeline.clone();
            self.apply_video_clip_timeline(timeline, self.video_selected_clip, false, cx);
        } else {
            self.seek_video(self.video_position, cx);
        }
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
        if self.video_playing || self.video_duration <= 0.0 || self.video_edit_busy {
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
        let generation = self.video_playback_generation.clone();
        let token = generation.fetch_add(1, Ordering::SeqCst) + 1;
        let receiver = PlaybackMailbox::default();
        let sender = receiver.clone();
        self.video_playing = true;
        self.toast = None;

        cx.background_executor()
            .spawn(async move {
                let mut stream =
                    match SynchronizedPlaybackStream::open(&path, start_time, 1920, 1080) {
                        Ok(stream) => stream,
                        Err(error) => {
                            sender.finish(Err(error.to_string()));
                            return;
                        }
                    };
                while generation.load(Ordering::SeqCst) == token && Arc::strong_count(&sender.0) > 1
                {
                    match stream.next_frame() {
                        Ok(Some(frame)) => {
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
        cx.spawn(async move |weak, cx| loop {
            Timer::after(Duration::from_millis(8)).await;
            if active_generation.load(Ordering::SeqCst) != token {
                break;
            }
            let PlaybackUpdates {
                frame: latest_frame,
                terminal,
            } = receiver.take();
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

    pub(super) fn pause_video_playback(&mut self) {
        self.video_playback_generation
            .fetch_add(1, Ordering::SeqCst);
        self.video_playing = false;
    }

    pub(super) fn seek_video(&mut self, position: f64, cx: &mut Context<Self>) {
        self.stop_editing_text();
        let playback_path = self.video_playback_path();
        self.pause_video_playback();
        let position = position.clamp(0.0, self.video_duration);
        self.video_position = position;
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

    pub(super) fn video_timecode(value: f64) -> String {
        let seconds = value.max(0.0).floor() as u64;
        format!("{:02}:{:02}", seconds / 60, seconds % 60)
    }

    /// Modal that previews a clip speed change before rendering it once.
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
