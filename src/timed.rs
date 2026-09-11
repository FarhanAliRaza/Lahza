//! Adapt annotation animation to the recording viewport and clip timeline.

use crate::recording::viewport::ViewportFrame;
use crate::AnnotationMark;
pub(crate) use lahza_annotations::timing::*;

/// A pinned mark expressed in media coordinates for `viewport`, so painting
/// it through the viewport crop leaves it fixed on the frame. Unpinned marks
/// pass through unchanged.
pub fn in_media_space(mark: AnnotationMark, viewport: ViewportFrame) -> AnnotationMark {
    in_cropped_media_space(mark, viewport, crate::CropRect::UNIT)
}

pub(crate) fn in_cropped_media_space(
    mark: AnnotationMark,
    viewport: ViewportFrame,
    crop: crate::CropRect,
) -> AnnotationMark {
    lahza_annotations::timing::in_visible_media_space(mark, crop.visible_rect(viewport))
}

/// Every mark visible at `time`, animated and in media coordinates for
/// `viewport`.
pub fn active_marks(
    marks: &[AnnotationMark],
    time: f64,
    viewport: ViewportFrame,
) -> Vec<AnnotationMark> {
    marks
        .iter()
        .filter(|mark| !mark.is_canvas())
        .filter_map(|mark| animated_mark(mark, time))
        .map(|mark| in_media_space(mark, viewport))
        .collect()
}

pub(crate) fn active_marks_in_crop(
    marks: &[AnnotationMark],
    time: f64,
    viewport: ViewportFrame,
    crop: crate::CropRect,
) -> Vec<AnnotationMark> {
    if crop == crate::CropRect::UNIT {
        return active_marks(marks, time, viewport);
    }
    marks
        .iter()
        .filter(|mark| !mark.is_canvas())
        .filter_map(|mark| animated_mark(mark, time))
        .map(|mark| in_cropped_media_space(mark, viewport, crop))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{NormPoint, Tool};
    fn mark(tool: Tool) -> AnnotationMark {
        AnnotationMark {
            tool,
            start: NormPoint { x: 0.2, y: 0.2 },
            end: NormPoint { x: 0.6, y: 0.4 },
            points: vec![
                NormPoint { x: 0.2, y: 0.2 },
                NormPoint { x: 0.4, y: 0.3 },
                NormPoint { x: 0.6, y: 0.4 },
            ],
            number: 1,
            color: 0xff0000,
            stroke_width: 4.0,
            density: 0.5,
            text: "Hello".into(),
            font_size: 24.0,
            font_family: 0,
            text_alignment: 0,
            bold: false,
            italic: false,
            underline: false,
            from_template: false,
            timing: Some(AnnotationTiming {
                start: 1.0,
                end: 3.0,
                entrance: EntranceEffect::Fade,
                exit: ExitEffect::Fade,
                transition: 0.5,
            }),
            opacity: 1.0,
            pinned: false,
            canvas: false,
            ..Default::default()
        }
    }

    #[test]
    fn pinned_marks_follow_the_viewport_crop() {
        let mut pinned = mark(Tool::Text);
        pinned.pinned = true;
        pinned.timing = None;
        let viewport = ViewportFrame {
            magnification: 2.0,
            anchor: crate::recording::model::NormalizedPoint { x: 0.75, y: 0.75 },
            ..ViewportFrame::default()
        };
        let mapped = in_media_space(pinned.clone(), viewport);
        assert!((mapped.start.x - 0.6).abs() < 1e-6 && (mapped.start.y - 0.6).abs() < 1e-6);
        assert!((mapped.end.x - 0.8).abs() < 1e-6);
        assert!((mapped.font_size - 12.0).abs() < 1e-6);
        pinned.pinned = false;
        assert_eq!(in_media_space(pinned.clone(), viewport), pinned);
    }
}

/// Media-attached overlays follow retained source ranges just like camera and
/// audio. Canvas overlays deliberately have an independent lifetime.
pub(crate) fn remap_clip_annotations(
    marks: &[crate::AnnotationMark],
    old: &crate::recording::clips::RecordingClipTimeline,
    new: &crate::recording::clips::RecordingClipTimeline,
) -> Vec<crate::AnnotationMark> {
    let mut result = Vec::new();
    let starts = old.clip_starts();
    for mark in marks {
        let Some(timing) = mark.timing.filter(|_| !mark.canvas) else {
            result.push(mark.clone());
            continue;
        };
        let mut intervals = Vec::new();
        for (clip, start) in old.segments.iter().zip(&starts) {
            let a = timing.start.max(*start);
            let b = timing.end.min(start + clip.editor_duration());
            if b <= a {
                continue;
            }
            let source_a = clip.source_start + (a - start) * clip.speed;
            let source_b = clip.source_start + (b - start) * clip.speed;
            intervals.extend(
                new.slices_overlapping(source_a, source_b)
                    .into_iter()
                    .map(|slice| (slice.editor_start, slice.editor_end)),
            );
        }
        intervals.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut merged: Vec<(f64, f64)> = Vec::new();
        for (start, end) in intervals {
            if let Some(last) = merged.last_mut() {
                if start <= last.1 + 1e-6 {
                    last.1 = last.1.max(end);
                    continue;
                }
            }
            merged.push((start, end));
        }
        for (start, end) in merged {
            let mut mark = mark.clone();
            mark.timing = Some(AnnotationTiming {
                start,
                end,
                ..timing
            });
            result.push(mark);
        }
    }
    result
}

#[cfg(test)]
mod clip_edit_tests {
    use super::*;
    use crate::recording::clips::RecordingClipTimeline;

    #[test]
    fn deleting_a_clip_removes_its_overlays_and_ripples_retained_timing() {
        let (old, _) = RecordingClipTimeline::full(9.0).split_at(3.0).unwrap();
        let (old, _) = old.split_at(6.0).unwrap();
        let new = old.deleting(old.segments[1].id).unwrap();
        let mark = |start, end| crate::AnnotationMark {
            timing: Some(AnnotationTiming {
                start,
                end,
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut canvas = mark(4.0, 5.0);
        canvas.canvas = true;
        let marks = vec![
            mark(4.0, 5.0),
            mark(7.0, 8.0),
            mark(2.0, 7.0),
            canvas.clone(),
        ];
        let remapped = remap_clip_annotations(&marks, &old, &new);
        assert_eq!(remapped.len(), 3);
        assert_eq!(
            (
                remapped[0].timing.unwrap().start,
                remapped[0].timing.unwrap().end
            ),
            (4.0, 5.0)
        );
        assert_eq!(
            (
                remapped[1].timing.unwrap().start,
                remapped[1].timing.unwrap().end
            ),
            (2.0, 4.0)
        );
        assert_eq!(remapped[2], canvas);
        assert_eq!(
            marks[0].timing.unwrap().start,
            4.0,
            "undo snapshot remains intact"
        );
    }
}
