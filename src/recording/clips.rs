use serde::{Deserialize, Serialize};
use std::ops::Range;
use uuid::Uuid;

const EPSILON: f64 = 0.000_001;

fn new_uuid() -> Uuid {
    Uuid::new_v4()
}

fn default_speed() -> f64 {
    1.0
}

/// A non-destructive range on the source recording.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingClipSegment {
    #[serde(default = "new_uuid")]
    pub id: Uuid,
    #[serde(alias = "start")]
    pub source_start: f64,
    #[serde(alias = "end")]
    pub source_end: f64,
    #[serde(default = "default_speed")]
    pub speed: f64,
}

impl RecordingClipSegment {
    pub const MINIMUM_DURATION: f64 = 0.12;
    // ffmpeg's atempo audio filter only supports tempo factors >= 0.5, which
    // bounds how far a clip can be slowed down.
    pub const MINIMUM_SPEED: f64 = 0.5;
    pub const MAXIMUM_SPEED: f64 = 16.0;

    pub fn new(source_start: f64, source_end: f64) -> Self {
        Self {
            id: Uuid::new_v4(),
            source_start,
            source_end,
            speed: 1.0,
        }
    }

    pub fn duration(&self) -> f64 {
        (self.source_end - self.source_start).max(0.0)
    }

    pub fn editor_duration(&self) -> f64 {
        self.duration() / self.speed.max(Self::MINIMUM_SPEED)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ClipLocation {
    pub segment_index: usize,
    pub segment_id: Uuid,
    pub editor_start: f64,
    /// Offset within the segment in editor time.
    pub offset: f64,
    pub source_time: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ClipSlice {
    pub segment_id: Uuid,
    pub source_start: f64,
    pub source_end: f64,
    pub editor_start: f64,
    pub editor_end: f64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClipEdge {
    Leading,
    Trailing,
}

/// The output timeline is the concatenation of these source ranges.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingClipTimeline {
    pub segments: Vec<RecordingClipSegment>,
}

impl RecordingClipTimeline {
    pub fn new(segments: Vec<RecordingClipSegment>) -> Self {
        Self { segments }
    }

    pub fn full(source_duration: f64) -> Self {
        let safe_duration = finite_nonnegative(source_duration);
        if safe_duration <= 0.0 {
            Self::default()
        } else {
            Self::new(vec![RecordingClipSegment::new(0.0, safe_duration)])
        }
    }

    pub fn legacy_trim(start: Option<f64>, end: Option<f64>, source_duration: f64) -> Self {
        let safe_duration = finite_nonnegative(source_duration);
        let safe_start = start.unwrap_or(0.0).clamp(0.0, safe_duration);
        let safe_end = end
            .unwrap_or(safe_duration)
            .clamp(safe_start, safe_duration);
        if safe_end - safe_start < RecordingClipSegment::MINIMUM_DURATION {
            Self::full(safe_duration)
        } else {
            Self::new(vec![RecordingClipSegment::new(safe_start, safe_end)])
        }
    }

    pub fn duration(&self) -> f64 {
        self.segments
            .iter()
            .map(RecordingClipSegment::editor_duration)
            .sum()
    }

    pub fn normalized(&self, source_duration: f64) -> Self {
        let safe_duration = finite_nonnegative(source_duration);
        let mut seen_ids = std::collections::HashSet::new();
        let mut segments: Vec<_> = self
            .segments
            .iter()
            .filter_map(|segment| {
                let start = finite_or(segment.source_start, 0.0).clamp(0.0, safe_duration);
                let end = finite_or(segment.source_end, start).clamp(start, safe_duration);
                if end - start < RecordingClipSegment::MINIMUM_DURATION {
                    return None;
                }
                let id = if seen_ids.insert(segment.id) {
                    segment.id
                } else {
                    Uuid::new_v4()
                };
                Some(RecordingClipSegment {
                    id,
                    source_start: start,
                    source_end: end,
                    speed: finite_or(segment.speed, 1.0).clamp(
                        RecordingClipSegment::MINIMUM_SPEED,
                        RecordingClipSegment::MAXIMUM_SPEED,
                    ),
                })
            })
            .collect();
        segments.sort_by(|left, right| {
            left.source_start
                .total_cmp(&right.source_start)
                .then_with(|| left.source_end.total_cmp(&right.source_end))
        });

        let mut previous_end: f64 = 0.0;
        segments.retain_mut(|segment| {
            segment.source_start = segment.source_start.max(previous_end);
            if segment.duration() < RecordingClipSegment::MINIMUM_DURATION {
                false
            } else {
                previous_end = segment.source_end;
                true
            }
        });
        if segments.is_empty() && safe_duration > 0.0 {
            Self::full(safe_duration)
        } else {
            Self::new(segments)
        }
    }

    pub fn location_at(&self, editor_time: f64) -> Option<ClipLocation> {
        let duration = self.duration();
        if self.segments.is_empty() || duration <= 0.0 {
            return None;
        }
        let clamped = finite_or(editor_time, 0.0).clamp(0.0, duration);
        let mut editor_start = 0.0;
        for (index, segment) in self.segments.iter().enumerate() {
            let editor_end = editor_start + segment.editor_duration();
            if clamped < editor_end || index + 1 == self.segments.len() {
                let offset = (clamped - editor_start).clamp(0.0, segment.editor_duration());
                let source_offset = (offset * segment.speed).min(segment.duration());
                return Some(ClipLocation {
                    segment_index: index,
                    segment_id: segment.id,
                    editor_start,
                    offset,
                    source_time: segment.source_start + source_offset,
                });
            }
            editor_start = editor_end;
        }
        None
    }

    pub fn source_time_at(&self, editor_time: f64) -> f64 {
        self.location_at(editor_time)
            .map(|location| location.source_time)
            .unwrap_or(0.0)
    }

    /// Inclusive lookup used for playhead placement, matching the Swift app.
    pub fn editor_time_for_source(&self, source_time: f64) -> Option<f64> {
        let mut editor_start = 0.0;
        for segment in &self.segments {
            if source_time >= segment.source_start - EPSILON
                && source_time <= segment.source_end + EPSILON
            {
                let offset = (source_time - segment.source_start).clamp(0.0, segment.duration());
                return Some(editor_start + offset / segment.speed);
            }
            editor_start += segment.editor_duration();
        }
        None
    }

    /// Half-open lookup for discrete events, preventing an event on an
    /// outgoing clip's end boundary from leaking into the following clip.
    pub fn editor_time_for_event(&self, source_time: f64) -> Option<f64> {
        let mut editor_start = 0.0;
        for segment in &self.segments {
            if source_time >= segment.source_start && source_time < segment.source_end {
                return Some(editor_start + (source_time - segment.source_start) / segment.speed);
            }
            editor_start += segment.editor_duration();
        }
        None
    }

    pub fn editor_range(&self, segment_id: Uuid) -> Option<Range<f64>> {
        let mut editor_start = 0.0;
        for segment in &self.segments {
            let editor_end = editor_start + segment.editor_duration();
            if segment.id == segment_id {
                return Some(editor_start..editor_end);
            }
            editor_start = editor_end;
        }
        None
    }

    pub fn split_at(&self, editor_time: f64) -> Option<(Self, Uuid)> {
        let location = self.location_at(editor_time)?;
        let segment = &self.segments[location.segment_index];
        let source_time = location.source_time;
        if source_time - segment.source_start < RecordingClipSegment::MINIMUM_DURATION
            || segment.source_end - source_time < RecordingClipSegment::MINIMUM_DURATION
        {
            return None;
        }
        let trailing_id = Uuid::new_v4();
        let leading = RecordingClipSegment {
            id: segment.id,
            source_start: segment.source_start,
            source_end: source_time,
            speed: segment.speed,
        };
        let trailing = RecordingClipSegment {
            id: trailing_id,
            source_start: source_time,
            source_end: segment.source_end,
            speed: segment.speed,
        };
        let mut segments = self.segments.clone();
        segments.splice(
            location.segment_index..=location.segment_index,
            [leading, trailing],
        );
        Some((Self::new(segments), trailing_id))
    }

    pub fn deleting(&self, segment_id: Uuid) -> Option<Self> {
        if self.segments.len() <= 1 || !self.segments.iter().any(|item| item.id == segment_id) {
            return None;
        }
        let segments: Vec<_> = self
            .segments
            .iter()
            .filter(|item| item.id != segment_id)
            .cloned()
            .collect();
        (segments
            .iter()
            .map(RecordingClipSegment::duration)
            .sum::<f64>()
            >= RecordingClipSegment::MINIMUM_DURATION)
            .then(|| Self::new(segments))
    }

    pub fn replacing(&self, replacement: RecordingClipSegment) -> Self {
        let mut segments = self.segments.clone();
        if let Some(index) = segments.iter().position(|item| item.id == replacement.id) {
            segments[index] = replacement;
        }
        Self::new(segments)
    }

    pub fn trimming(
        &self,
        segment_id: Uuid,
        edge: ClipEdge,
        editor_delta: f64,
        source_duration: f64,
    ) -> Option<(Self, RecordingClipSegment)> {
        let index = self
            .segments
            .iter()
            .position(|clip| clip.id == segment_id)?;
        let original = &self.segments[index];
        let source_delta = editor_delta * original.speed;
        let previous_end = index
            .checked_sub(1)
            .map(|previous| self.segments[previous].source_end)
            .unwrap_or(0.0);
        let next_start = self
            .segments
            .get(index + 1)
            .map(|next| next.source_start)
            .unwrap_or_else(|| finite_nonnegative(source_duration));
        let mut replacement = original.clone();
        match edge {
            ClipEdge::Leading => {
                replacement.source_start = (original.source_start + source_delta)
                    .max(previous_end)
                    .min(original.source_end - RecordingClipSegment::MINIMUM_DURATION);
            }
            ClipEdge::Trailing => {
                replacement.source_end = (original.source_end + source_delta)
                    .min(next_start)
                    .max(original.source_start + RecordingClipSegment::MINIMUM_DURATION);
            }
        }
        Some((self.replacing(replacement.clone()), replacement))
    }

    pub fn removing_source_ranges(&self, ranges: &[(f64, f64)]) -> Option<Self> {
        let cuts = Self::merged_ranges(ranges);
        if cuts.is_empty() {
            return Some(self.clone());
        }
        let mut next = Vec::new();
        for segment in &self.segments {
            let mut pieces = Vec::new();
            let mut cursor = segment.source_start;
            for &(cut_start, cut_end) in cuts
                .iter()
                .filter(|(_, end)| *end > segment.source_start)
                .filter(|(start, _)| *start < segment.source_end)
            {
                if cut_start > cursor {
                    pieces.push((cursor, cut_start.min(segment.source_end)));
                }
                cursor = cursor.max(cut_end);
            }
            if cursor < segment.source_end {
                pieces.push((cursor, segment.source_end));
            }
            let mut kept_id = false;
            for (start, end) in pieces
                .into_iter()
                .filter(|(start, end)| end - start >= RecordingClipSegment::MINIMUM_DURATION)
            {
                next.push(RecordingClipSegment {
                    id: if kept_id { Uuid::new_v4() } else { segment.id },
                    source_start: start,
                    source_end: end,
                    speed: segment.speed,
                });
                kept_id = true;
            }
        }
        (!next.is_empty()).then(|| Self::new(next))
    }

    pub fn merged_ranges(ranges: &[(f64, f64)]) -> Vec<(f64, f64)> {
        let mut sorted: Vec<_> = ranges
            .iter()
            .copied()
            .filter(|(start, end)| start.is_finite() && end.is_finite() && end > start)
            .collect();
        sorted.sort_by(|left, right| left.0.total_cmp(&right.0));
        let mut merged: Vec<(f64, f64)> = Vec::new();
        for range in sorted {
            if let Some(last) = merged.last_mut() {
                if range.0 <= last.1 {
                    last.1 = last.1.max(range.1);
                    continue;
                }
            }
            merged.push(range);
        }
        merged
    }

    pub fn slices_overlapping(&self, source_start: f64, source_end: f64) -> Vec<ClipSlice> {
        if source_end <= source_start {
            return Vec::new();
        }
        let mut result = Vec::new();
        let mut editor_offset = 0.0;
        for segment in &self.segments {
            let overlap_start = source_start.max(segment.source_start);
            let overlap_end = source_end.min(segment.source_end);
            if overlap_end > overlap_start {
                result.push(ClipSlice {
                    segment_id: segment.id,
                    source_start: overlap_start,
                    source_end: overlap_end,
                    editor_start: editor_offset
                        + (overlap_start - segment.source_start) / segment.speed,
                    editor_end: editor_offset
                        + (overlap_end - segment.source_start) / segment.speed,
                });
            }
            editor_offset += segment.editor_duration();
        }
        result
    }

    pub fn is_unedited(&self, source_duration: f64) -> bool {
        self.segments.len() == 1
            && self.segments.first().is_some_and(|only| {
                only.source_start.abs() < EPSILON
                    && (only.source_end - source_duration).abs() < EPSILON
                    && (only.speed - 1.0).abs() < EPSILON
            })
    }
}

fn finite_nonnegative(value: f64) -> f64 {
    finite_or(value, 0.0).max(0.0)
}

fn finite_or(value: f64, fallback: f64) -> f64 {
    if value.is_finite() {
        value
    } else {
        fallback
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segment(start: f64, end: f64, speed: f64) -> RecordingClipSegment {
        RecordingClipSegment {
            speed,
            ..RecordingClipSegment::new(start, end)
        }
    }

    #[test]
    fn speed_changes_editor_mapping_without_changing_source_ranges() {
        let timeline =
            RecordingClipTimeline::new(vec![segment(1.0, 5.0, 2.0), segment(7.0, 11.0, 4.0)]);
        assert_eq!(timeline.duration(), 3.0);
        assert_eq!(timeline.source_time_at(1.5), 4.0);
        assert_eq!(timeline.source_time_at(2.5), 9.0);
        assert_eq!(timeline.editor_time_for_source(9.0), Some(2.5));
    }

    #[test]
    fn split_keeps_leading_identity_and_speed() {
        let original = segment(2.0, 10.0, 2.0);
        let original_id = original.id;
        let (timeline, selected) = RecordingClipTimeline::new(vec![original])
            .split_at(1.0)
            .expect("valid split");
        assert_eq!(timeline.segments.len(), 2);
        assert_eq!(timeline.segments[0].id, original_id);
        assert_eq!(timeline.segments[0].source_end, 4.0);
        assert_eq!(timeline.segments[1].id, selected);
        assert_eq!(timeline.segments[1].speed, 2.0);
    }

    #[test]
    fn normalization_repairs_overlap_duplicate_ids_and_invalid_speed() {
        let shared = Uuid::new_v4();
        let timeline = RecordingClipTimeline::new(vec![
            RecordingClipSegment {
                id: shared,
                source_start: 4.0,
                source_end: 8.0,
                speed: 99.0,
            },
            RecordingClipSegment {
                id: shared,
                source_start: 1.0,
                source_end: 5.0,
                speed: f64::NAN,
            },
        ])
        .normalized(10.0);
        assert_eq!(timeline.segments[0].source_start, 1.0);
        assert_eq!(timeline.segments[0].speed, 1.0);
        assert_eq!(timeline.segments[1].source_start, 5.0);
        assert_eq!(timeline.segments[1].speed, 16.0);
        assert_ne!(timeline.segments[0].id, timeline.segments[1].id);
    }

    #[test]
    fn event_lookup_is_half_open_but_playhead_lookup_is_inclusive() {
        let timeline =
            RecordingClipTimeline::new(vec![segment(0.0, 2.0, 1.0), segment(4.0, 6.0, 1.0)]);
        assert_eq!(timeline.editor_time_for_source(2.0), Some(2.0));
        assert_eq!(timeline.editor_time_for_event(2.0), None);
        assert_eq!(timeline.editor_time_for_event(4.0), Some(2.0));
    }

    #[test]
    fn source_range_removal_splits_and_preserves_first_identity() {
        let original = segment(0.0, 10.0, 2.0);
        let id = original.id;
        let timeline = RecordingClipTimeline::new(vec![original])
            .removing_source_ranges(&[(2.0, 4.0), (3.0, 6.0)])
            .expect("survivors");
        assert_eq!(timeline.segments.len(), 2);
        assert_eq!(timeline.segments[0].id, id);
        assert_eq!(
            (
                timeline.segments[0].source_start,
                timeline.segments[0].source_end
            ),
            (0.0, 2.0)
        );
        assert_eq!(
            (
                timeline.segments[1].source_start,
                timeline.segments[1].source_end
            ),
            (6.0, 10.0)
        );
        assert_eq!(timeline.duration(), 3.0);
    }

    #[test]
    fn swift_legacy_clip_keys_still_decode() {
        let value = serde_json::json!({"segments": [{"start": 1.0, "end": 3.0}]});
        let timeline: RecordingClipTimeline = serde_json::from_value(value).unwrap();
        assert_eq!(timeline.segments[0].speed, 1.0);
        assert_eq!(timeline.segments[0].source_start, 1.0);
    }

    #[test]
    fn trim_drag_uses_editor_delta_times_speed_and_neighbor_limits() {
        let leading = segment(0.0, 2.0, 1.0);
        let selected = segment(4.0, 8.0, 2.0);
        let selected_id = selected.id;
        let trailing = segment(10.0, 12.0, 1.0);
        let timeline = RecordingClipTimeline::new(vec![leading, selected, trailing]);

        let (_, replacement) = timeline
            .trimming(selected_id, ClipEdge::Leading, -2.0, 12.0)
            .unwrap();
        assert_eq!(replacement.source_start, 2.0);
        let (_, replacement) = timeline
            .trimming(selected_id, ClipEdge::Trailing, 2.0, 12.0)
            .unwrap();
        assert_eq!(replacement.source_end, 10.0);
        let (_, replacement) = timeline
            .trimming(selected_id, ClipEdge::Leading, 99.0, 12.0)
            .unwrap();
        assert!((replacement.duration() - RecordingClipSegment::MINIMUM_DURATION).abs() < 1e-12);
    }
}
