//! Shared interaction rules for the timeline's editable lanes.

use crate::{recording::clips::RecordingClipTimeline, Studio, VideoEditSnapshot, VideoMoveDrag};
use gpui::Context;
use uuid::Uuid;

/// Snap either edge of a moving range, or one trim edge when `duration` is
/// zero. The tolerance is measured in screen pixels at the gesture's scale.
pub(crate) fn snap_timeline_edges(
    start: f64,
    duration: f64,
    targets: &[f64],
    seconds_per_pixel: f64,
    enabled: bool,
) -> (f64, Option<f64>) {
    if !enabled || !start.is_finite() || !seconds_per_pixel.is_finite() {
        return (start, None);
    }
    let tolerance = 8.0 * seconds_per_pixel.max(0.0);
    let mut closest: Option<(f64, f64)> = None;
    for &target in targets {
        if !target.is_finite() || target < 0.0 {
            continue;
        }
        for edge in [start, start + duration.max(0.0)] {
            let delta = target - edge;
            if delta.abs() <= tolerance
                && start + delta >= 0.0
                && closest.is_none_or(|(best, _)| delta.abs() < best.abs())
            {
                closest = Some((delta, target));
            }
        }
    }
    closest.map_or((start, None), |(delta, target)| {
        (start + delta, Some(target))
    })
}

/// Clamping against source limits may prevent a requested snap. Only show
/// the guide when an actual edge reaches the target.
pub(crate) fn snap_guide_at_edges(guide: Option<f64>, start: f64, end: f64) -> Option<f64> {
    guide.filter(|target| (start - target).abs() < 0.000_001 || (end - target).abs() < 0.000_001)
}

pub(crate) fn precise_timecode(seconds: f64) -> String {
    let milliseconds = if seconds.is_finite() {
        (seconds.max(0.0) * 1_000.0).round() as u64
    } else {
        0
    };
    let seconds = milliseconds / 1_000;
    if seconds >= 3_600 {
        format!(
            "{}:{:02}:{:02}.{:03}",
            seconds / 3_600,
            seconds / 60 % 60,
            seconds % 60,
            milliseconds % 1_000
        )
    } else {
        format!(
            "{}:{:02}.{:03}",
            seconds / 60,
            seconds % 60,
            milliseconds % 1_000
        )
    }
}

/// Resolve overlaps exactly as the committed edit will, so the preview
/// never promises a destination that will change on mouse release.
fn resolved_reposition(
    timeline: &RecordingClipTimeline,
    clip_id: Uuid,
    requested_start: f64,
) -> Option<(RecordingClipTimeline, f64)> {
    let resolved = timeline
        .repositioning(clip_id, requested_start)
        .unwrap_or_else(|| timeline.clone());
    let start = resolved.editor_range(clip_id)?.start;
    Some((resolved, start))
}

impl Studio {
    pub(crate) fn timeline_snap_targets(
        &self,
        exclude_video: Option<Uuid>,
        exclude_motion: Option<Uuid>,
        exclude_audio: Option<Uuid>,
    ) -> Vec<f64> {
        let mut targets = vec![0.0, self.video_position];
        let starts = self.video_clip_timeline.clip_starts();
        for (index, clip) in self.video_clip_timeline.segments.iter().enumerate() {
            if Some(clip.id) != exclude_video {
                targets.extend([starts[index], starts[index] + clip.editor_duration()]);
            }
        }
        for cue in &self.video_zoom_cues {
            if Some(cue.id) != exclude_motion {
                for slice in self
                    .video_clip_timeline
                    .slices_overlapping(cue.start, cue.end)
                {
                    targets.extend([slice.editor_start, slice.editor_end]);
                }
            }
        }
        for clip in &self.video_audio_clips {
            if Some(clip.id) != exclude_audio {
                targets.extend([clip.start, clip.start + clip.duration]);
            }
        }
        targets.retain(|time| time.is_finite() && *time >= 0.0);
        targets.sort_by(f64::total_cmp);
        targets.dedup_by(|a, b| (*a - *b).abs() < 0.000_001);
        targets
    }

    pub(crate) fn resolved_video_move_preview(
        &self,
        drag: &VideoMoveDrag,
    ) -> Option<(RecordingClipTimeline, f64)> {
        resolved_reposition(
            &self.video_clip_timeline,
            drag.clip_id,
            self.video_move_new_start(drag)?,
        )
    }

    pub(crate) fn clip_edit_snapshot(&self) -> VideoEditSnapshot {
        VideoEditSnapshot::Clips {
            timeline: self.video_clip_timeline.clone(),
            annotations: self.annotations.clone(),
            selected: self.video_selected_clip,
            position: self.video_position,
        }
    }

    pub(crate) fn timeline_drag_readout(&self) -> Option<String> {
        let range = if let Some(drag) = self.video_trim_drag.as_ref() {
            let range = self
                .video_clip_timeline
                .editor_range(drag.original_clip.id)?;
            Some((range.start, range.end))
        } else if let Some(drag) = self.video_move_drag.as_ref().filter(|drag| drag.active) {
            let (timeline, _) = self.resolved_video_move_preview(drag)?;
            let range = timeline.editor_range(drag.clip_id)?;
            Some((range.start, range.end))
        } else if let Some(drag) = self.video_zoom_drag.as_ref() {
            let cue = self
                .video_zoom_cues
                .iter()
                .find(|cue| cue.id == drag.original_cue.id)?;
            let slices = self
                .video_clip_timeline
                .slices_overlapping(cue.start, cue.end);
            Some((slices.first()?.editor_start, slices.last()?.editor_end))
        } else {
            self.audio_drag_range()
        }?;
        Some(format!(
            "{} – {} · {}",
            precise_timecode(range.0),
            precise_timecode(range.1),
            precise_timecode(range.1 - range.0)
        ))
    }

    /// Cancel transient edits without creating undo entries or saving a
    /// draft. Committed edits remain available through ordinary undo.
    pub(crate) fn cancel_timeline_edit(&mut self, cx: &mut Context<Self>) -> bool {
        let mut cancelled = self.cancel_motion_creation();
        cancelled |= self.cancel_camera_region_drag();
        if let Some(drag) = self.video_move_drag.take() {
            self.video_selected_clip = drag.original_selection;
            self.video_selected_zoom_cue = drag.original_zoom_selection;
            cancelled = true;
        }
        if let Some(drag) = self.video_trim_drag.take() {
            self.video_clip_timeline = drag.original_timeline;
            self.video_duration = self.video_clip_timeline.duration();
            self.video_position = drag.original_position;
            self.video_selected_clip = drag.original_selection;
            self.video_selected_zoom_cue = drag.original_zoom_selection;
            cancelled = true;
        }
        if let Some(drag) = self.video_zoom_drag.take() {
            self.video_zoom_cues = drag.original_cues;
            self.video_selected_clip = drag.original_selection;
            self.video_selected_zoom_cue = drag.original_zoom_selection;
            self.rebuild_video_motion_timelines();
            cancelled = true;
        }
        cancelled |= self.cancel_audio_drag();
        cancelled |= self.cancel_annotation_timeline_drag();
        cancelled |= self.cancel_image_trim();
        if cancelled {
            self.timeline_ui.snap_guide = None;
            cx.notify();
        }
        cancelled
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recording::clips::RecordingClipSegment;

    #[test]
    fn snapping_handles_both_edges_and_can_be_disabled() {
        let (start, guide) = snap_timeline_edges(2.94, 2.0, &[5.0], 0.01, true);
        assert!((start - 3.0).abs() < 1e-9);
        assert_eq!(guide, Some(5.0));
        assert_eq!(
            snap_timeline_edges(2.94, 2.0, &[5.0], 0.01, false),
            (2.94, None)
        );
        assert_eq!(
            snap_timeline_edges(2.94, 0.0, &[3.0], 0.01, true),
            (3.0, Some(3.0))
        );
        assert_eq!(
            snap_timeline_edges(2.8, 2.0, &[5.0], 0.01, true),
            (2.8, None)
        );
    }

    #[test]
    fn snapping_does_not_place_a_range_before_zero_or_show_unreachable_guides() {
        assert_eq!(
            snap_timeline_edges(0.02, 2.0, &[1.96], 0.01, true),
            (0.02, None)
        );
        assert_eq!(snap_guide_at_edges(Some(4.0), 1.0, 3.8), None);
        assert_eq!(snap_guide_at_edges(Some(4.0), 1.0, 4.0), Some(4.0));
    }

    #[test]
    fn snapping_tolerance_tracks_the_visible_scale() {
        assert_eq!(
            snap_timeline_edges(2.5, 0.0, &[3.0], 0.1, true),
            (3.0, Some(3.0))
        );
        assert_eq!(
            snap_timeline_edges(2.5, 0.0, &[3.0], 0.01, true),
            (2.5, None)
        );
    }

    #[test]
    fn overlapping_move_preview_matches_resolved_drop_and_neighbors() {
        let first = RecordingClipSegment::new(0.0, 4.0);
        let second = RecordingClipSegment::new(4.0, 8.0);
        let third = RecordingClipSegment::new(8.0, 10.0);
        let timeline =
            RecordingClipTimeline::new(vec![first.clone(), second.clone(), third.clone()]);
        let (preview, start) = resolved_reposition(&timeline, third.id, 2.0).unwrap();
        assert_eq!(
            start, 4.0,
            "overlap resolves after the first clip, not at the raw pointer time"
        );
        assert_eq!(preview.editor_range(second.id).unwrap().start, 6.0);
        assert_eq!(preview, timeline.repositioning(third.id, 2.0).unwrap());
    }

    #[test]
    fn precise_times_round_without_invalid_sixty_second_fields() {
        assert_eq!(precise_timecode(59.9998), "1:00.000");
        assert_eq!(precise_timecode(3_661.125), "1:01:01.125");
        assert_eq!(precise_timecode(-5.0), "0:00.000");
        assert_eq!(precise_timecode(f64::NAN), "0:00.000");
    }
}
