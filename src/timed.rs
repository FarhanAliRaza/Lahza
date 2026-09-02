//! Timing and entrance/exit animation for annotations inside motion scenes.
//!
//! A static screenshot ignores timing. Once a scene is animated, every
//! annotation may carry an [`AnnotationTiming`]; the preview painter and the
//! exporter both call [`animated_mark`] to get the exact geometry, text, and
//! opacity for a frame time, so both stay in sync.

use serde::{Deserialize, Serialize};
use std::hash::{Hash, Hasher};

use crate::recording::viewport::{visible_rect, ViewportFrame};
use crate::{AnnotationMark, NormPoint, Tool};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EntranceEffect {
    None,
    #[default]
    Fade,
    Pop,
    SlideUp,
    SlideLeft,
    Draw,
    Type,
}

impl EntranceEffect {
    pub const ALL: [EntranceEffect; 7] = [
        EntranceEffect::None,
        EntranceEffect::Fade,
        EntranceEffect::Pop,
        EntranceEffect::SlideUp,
        EntranceEffect::SlideLeft,
        EntranceEffect::Draw,
        EntranceEffect::Type,
    ];

    #[allow(dead_code)]
    pub fn label(self) -> &'static str {
        match self {
            EntranceEffect::None => "Cut",
            EntranceEffect::Fade => "Fade",
            EntranceEffect::Pop => "Pop",
            EntranceEffect::SlideUp => "Slide up",
            EntranceEffect::SlideLeft => "Slide in",
            EntranceEffect::Draw => "Draw",
            EntranceEffect::Type => "Type",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ExitEffect {
    None,
    #[default]
    Fade,
    Pop,
    SlideDown,
}

impl ExitEffect {
    pub const ALL: [ExitEffect; 4] = [
        ExitEffect::None,
        ExitEffect::Fade,
        ExitEffect::Pop,
        ExitEffect::SlideDown,
    ];

    #[allow(dead_code)]
    pub fn label(self) -> &'static str {
        match self {
            ExitEffect::None => "Cut",
            ExitEffect::Fade => "Fade",
            ExitEffect::Pop => "Shrink",
            ExitEffect::SlideDown => "Slide out",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AnnotationTiming {
    pub start: f64,
    pub end: f64,
    pub entrance: EntranceEffect,
    pub exit: ExitEffect,
    /// Seconds each transition takes.
    pub transition: f64,
}

impl Default for AnnotationTiming {
    fn default() -> Self {
        Self {
            start: 0.0,
            end: 2.5,
            entrance: EntranceEffect::Fade,
            exit: ExitEffect::Fade,
            transition: 0.35,
        }
    }
}

impl AnnotationTiming {
    pub const MINIMUM_DURATION: f64 = 0.2;
    pub const DEFAULT_DURATION: f64 = 2.5;

    /// A sensible default for a mark placed at the playhead: tools that
    /// naturally "draw" or "type" animate that way.
    pub fn for_tool(tool: Tool, start: f64, scene_duration: f64) -> Self {
        let end = (start + Self::DEFAULT_DURATION)
            .min(scene_duration.max(start + Self::MINIMUM_DURATION));
        let entrance = match tool {
            Tool::Pen | Tool::Line | Tool::Arrow => EntranceEffect::Draw,
            Tool::Text => EntranceEffect::Type,
            Tool::Number | Tool::Ellipse => EntranceEffect::Pop,
            _ => EntranceEffect::Fade,
        };
        Self {
            start,
            end,
            entrance,
            exit: ExitEffect::Fade,
            transition: 0.35,
        }
    }

    pub fn duration(&self) -> f64 {
        (self.end - self.start).max(0.0)
    }

    pub fn clamped(mut self, scene_duration: f64) -> Self {
        let scene_duration = scene_duration.max(Self::MINIMUM_DURATION);
        self.start = self
            .start
            .clamp(0.0, scene_duration - Self::MINIMUM_DURATION);
        self.end = self
            .end
            .clamp(self.start + Self::MINIMUM_DURATION, scene_duration);
        self.transition = self.transition.clamp(0.0, 3.0);
        self
    }

    pub fn state_at(&self, time: f64) -> AnimationState {
        if !(self.start..=self.end).contains(&time) || self.duration() <= 0.0 {
            return AnimationState::HIDDEN;
        }
        let transition = self.transition.clamp(0.0, self.duration() * 0.5).max(1e-6);
        let t_in = ((time - self.start) / transition).clamp(0.0, 1.0);
        let t_out = ((self.end - time) / transition).clamp(0.0, 1.0);
        let mut state = AnimationState::STATIC;
        let ease_in = smoothstep(t_in);
        match self.entrance {
            EntranceEffect::None => {}
            EntranceEffect::Fade => state.opacity = ease_in as f32,
            EntranceEffect::Pop => {
                state.opacity = ease_in as f32;
                state.scale = (0.6 + 0.4 * back_out(t_in)) as f32;
            }
            EntranceEffect::SlideUp => {
                state.opacity = ease_in as f32;
                state.offset.y = ((1.0 - ease_in) * 0.06) as f32;
            }
            EntranceEffect::SlideLeft => {
                state.opacity = ease_in as f32;
                state.offset.x = ((1.0 - ease_in) * 0.08) as f32;
            }
            EntranceEffect::Draw => state.progress = ease_in as f32,
            EntranceEffect::Type => state.progress = t_in as f32,
        }
        let ease_out = smoothstep(t_out);
        match self.exit {
            ExitEffect::None => {}
            ExitEffect::Fade => state.opacity *= ease_out as f32,
            ExitEffect::Pop => {
                state.opacity *= ease_out as f32;
                state.scale *= (0.6 + 0.4 * ease_out) as f32;
            }
            ExitEffect::SlideDown => {
                state.opacity *= ease_out as f32;
                state.offset.y += ((1.0 - ease_out) * 0.06) as f32;
            }
        }
        state
    }
}

/// Resolved animation for one frame time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AnimationState {
    pub visible: bool,
    pub opacity: f32,
    pub scale: f32,
    /// Normalized offset relative to the media size.
    pub offset: NormPoint,
    /// Draw/type completion, 0..1.
    pub progress: f32,
}

impl AnimationState {
    pub const STATIC: AnimationState = AnimationState {
        visible: true,
        opacity: 1.0,
        scale: 1.0,
        offset: NormPoint { x: 0.0, y: 0.0 },
        progress: 1.0,
    };
    pub const HIDDEN: AnimationState = AnimationState {
        visible: false,
        ..AnimationState::STATIC
    };
}

fn smoothstep(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn back_out(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    let c1 = 1.70158;
    let c3 = c1 + 1.0;
    1.0 + c3 * (t - 1.0).powi(3) + c1 * (t - 1.0).powi(2)
}

/// The mark as it should be painted at `time`, or `None` while hidden.
/// Marks without timing are always shown unchanged.
/// The mark as the editor paints it: a selected or edited mark shows its
/// full, static state so it can be seen and hit-tested while its entrance
/// (e.g. `Type` at the playhead) would otherwise hide it.
pub fn editor_mark(mark: &AnnotationMark, time: f64, focused: bool) -> Option<AnnotationMark> {
    if focused {
        return Some(mark.clone());
    }
    animated_mark(mark, time)
}

pub fn animated_mark(mark: &AnnotationMark, time: f64) -> Option<AnnotationMark> {
    let Some(timing) = mark.timing else {
        return Some(mark.clone());
    };
    let state = timing.state_at(time);
    if !state.visible || state.opacity <= 0.001 {
        return None;
    }
    let mut animated = mark.clone();
    animated.opacity = (mark.opacity * state.opacity).clamp(0.0, 1.0);
    let progress = state.progress.clamp(0.0, 1.0);
    if progress < 1.0 {
        match (timing.entrance, mark.tool) {
            (EntranceEffect::Draw, Tool::Pen) if mark.points.len() > 1 => {
                let last = mark.points.len() - 1;
                let position = progress * last as f32;
                let index = position.floor() as usize;
                let fraction = position - index as f32;
                let mut points: Vec<NormPoint> = mark.points[..=index.min(last)].to_vec();
                if index < last {
                    let from = mark.points[index];
                    let to = mark.points[index + 1];
                    points.push(NormPoint {
                        x: from.x + (to.x - from.x) * fraction,
                        y: from.y + (to.y - from.y) * fraction,
                    });
                }
                if points.len() < 2 {
                    points.push(points[0]);
                }
                animated.points = points;
            }
            (EntranceEffect::Draw, Tool::Line | Tool::Arrow) => {
                animated.end = NormPoint {
                    x: mark.start.x + (mark.end.x - mark.start.x) * progress,
                    y: mark.start.y + (mark.end.y - mark.start.y) * progress,
                };
            }
            (EntranceEffect::Type, Tool::Text) => {
                let count = mark.text.chars().count();
                let shown = ((count as f32) * progress).ceil() as usize;
                animated.text = mark.text.chars().take(shown).collect();
            }
            (EntranceEffect::Draw | EntranceEffect::Type, _) => {
                // Shapes without a natural stroke order grow into place.
                animated.opacity *= smoothstep(progress as f64) as f32;
                scale_mark(&mut animated, (0.7 + 0.3 * progress).max(0.05));
            }
            _ => {}
        }
    }
    if (state.scale - 1.0).abs() > 1e-4 {
        scale_mark(&mut animated, state.scale.max(0.05));
    }
    if state.offset.x.abs() > 1e-5 || state.offset.y.abs() > 1e-5 {
        offset_mark(&mut animated, state.offset);
    }
    Some(animated)
}

/// A pinned mark expressed in media coordinates for `viewport`, so painting
/// it through the viewport crop leaves it fixed on the frame. Unpinned marks
/// pass through unchanged.
pub fn in_media_space(mark: AnnotationMark, viewport: ViewportFrame) -> AnnotationMark {
    if !mark.pinned {
        return mark;
    }
    let (left, top, visible) = visible_rect(viewport);
    let mut mapped = mark;
    let map = |point: NormPoint| NormPoint {
        x: left as f32 + point.x * visible as f32,
        y: top as f32 + point.y * visible as f32,
    };
    mapped.start = map(mapped.start);
    mapped.end = map(mapped.end);
    for point in &mut mapped.points {
        *point = map(*point);
    }
    mapped.font_size *= visible as f32;
    mapped.stroke_width *= visible as f32;
    mapped
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
        .filter_map(|mark| animated_mark(mark, time))
        .map(|mark| in_media_space(mark, viewport))
        .collect()
}

/// Cheap fingerprint of a frame's annotation state so per-frame overlays
/// are only re-rendered when something changed.
pub fn marks_signature(marks: &[AnnotationMark]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    format!("{marks:?}").hash(&mut hasher);
    hasher.finish()
}

fn scale_mark(mark: &mut AnnotationMark, scale: f32) {
    let (cx, cy) = mark_center(mark);
    let scale_point = |point: NormPoint| NormPoint {
        x: cx + (point.x - cx) * scale,
        y: cy + (point.y - cy) * scale,
    };
    mark.start = scale_point(mark.start);
    mark.end = scale_point(mark.end);
    for point in &mut mark.points {
        *point = scale_point(*point);
    }
    if mark.tool == Tool::Text {
        mark.font_size *= scale;
    } else {
        mark.stroke_width *= scale.max(0.3);
    }
}

fn offset_mark(mark: &mut AnnotationMark, offset: NormPoint) {
    let shift = |point: NormPoint| NormPoint {
        x: point.x + offset.x,
        y: point.y + offset.y,
    };
    mark.start = shift(mark.start);
    mark.end = shift(mark.end);
    for point in &mut mark.points {
        *point = shift(*point);
    }
}

fn mark_center(mark: &AnnotationMark) -> (f32, f32) {
    if mark.tool == Tool::Pen && !mark.points.is_empty() {
        let (min_x, max_x, min_y, max_y) = mark.points.iter().fold(
            (f32::MAX, f32::MIN, f32::MAX, f32::MIN),
            |(min_x, max_x, min_y, max_y), point| {
                (
                    min_x.min(point.x),
                    max_x.max(point.x),
                    min_y.min(point.y),
                    max_y.max(point.y),
                )
            },
        );
        return ((min_x + max_x) * 0.5, (min_y + max_y) * 0.5);
    }
    (
        (mark.start.x + mark.end.x) * 0.5,
        (mark.start.y + mark.end.y) * 0.5,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn timing_hides_before_and_after_and_fades_between() {
        let mark = mark(Tool::Rectangle);
        assert!(animated_mark(&mark, 0.5).is_none());
        assert!(animated_mark(&mark, 3.5).is_none());
        let early = animated_mark(&mark, 1.1).unwrap();
        let middle = animated_mark(&mark, 2.0).unwrap();
        let late = animated_mark(&mark, 2.9).unwrap();
        assert!(early.opacity < 0.5 && early.opacity > 0.0);
        assert!((middle.opacity - 1.0).abs() < 1e-6);
        assert!(late.opacity < 0.5 && late.opacity > 0.0);
        assert_eq!(middle.start, mark.start);
    }

    #[test]
    fn draw_and_type_reveal_progressively() {
        let mut line = mark(Tool::Arrow);
        line.timing.as_mut().unwrap().entrance = EntranceEffect::Draw;
        let partial = animated_mark(&line, 1.25).unwrap();
        assert!(partial.end.x > line.start.x && partial.end.x < line.end.x);
        let complete = animated_mark(&line, 2.0).unwrap();
        assert_eq!(complete.end, line.end);

        let mut pen = mark(Tool::Pen);
        pen.timing.as_mut().unwrap().entrance = EntranceEffect::Draw;
        let partial = animated_mark(&pen, 1.2).unwrap();
        assert!(partial.points.len() >= 2 && partial.points.len() <= pen.points.len());
        assert!(partial.points.last().unwrap().x < 0.6);

        let mut text = mark(Tool::Text);
        text.timing.as_mut().unwrap().entrance = EntranceEffect::Type;
        let partial = animated_mark(&text, 1.2).unwrap();
        assert!(partial.text.len() < 5 && !partial.text.is_empty());
        assert_eq!(animated_mark(&text, 2.0).unwrap().text, "Hello");
    }

    #[test]
    fn pop_scales_around_the_center_and_untimed_marks_pass_through() {
        let mut pop = mark(Tool::Ellipse);
        pop.timing.as_mut().unwrap().entrance = EntranceEffect::Pop;
        let animated = animated_mark(&pop, 1.05).unwrap();
        let original_width = pop.end.x - pop.start.x;
        let animated_width = animated.end.x - animated.start.x;
        assert!(animated_width < original_width);
        let center = (animated.start.x + animated.end.x) * 0.5;
        assert!((center - 0.4).abs() < 1e-5);

        let mut plain = mark(Tool::Rectangle);
        plain.timing = None;
        assert_eq!(animated_mark(&plain, 99.0).unwrap(), plain);
    }

    #[test]
    fn defaults_follow_the_tool_and_clamp_to_the_scene() {
        let timing = AnnotationTiming::for_tool(Tool::Pen, 4.0, 5.0);
        assert_eq!(timing.entrance, EntranceEffect::Draw);
        assert!((timing.end - 5.0).abs() < 1e-9);
        let clamped = AnnotationTiming {
            start: 9.0,
            end: 20.0,
            ..AnnotationTiming::default()
        }
        .clamped(5.0);
        assert!(clamped.end <= 5.0 && clamped.start < clamped.end);
        let a = marks_signature(&[mark(Tool::Rectangle)]);
        let mut changed = mark(Tool::Rectangle);
        changed.opacity = 0.5;
        assert_ne!(a, marks_signature(&[changed]));
    }
}
