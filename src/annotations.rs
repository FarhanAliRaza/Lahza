//! Connect annotation gestures to Studio selection, undo, media coordinates, and text input.
use super::{AnnotationMark, AnnotationTiming, NormPoint, Studio, Tool, ANNOTATION_COLORS};
use gpui::{point, px, quad, rgb, size, App, Bounds, KeyDownEvent, Pixels, Point, Window};
use lahza_annotations::canvas::{
    annotation_snap_bounds, constrained_delta, handles, hit_mark, hit_selection_box, mark_screen_bounds,
    paint_snap_guide, translate_mark, translation_limits,
};
pub(crate) use lahza_annotations::canvas::{
    norm_to_screen, paint_annotation, paint_highlights, paint_selection, paint_selection_box, screen_to_norm, Handle,
};
pub(crate) use lahza_annotations::svg::annotations_svg;
use std::fs;

#[derive(Clone, Debug)]
pub(crate) struct Gesture {
    origin: Point<Pixels>,
    bounds: Bounds<Pixels>,
    before: Vec<AnnotationMark>,
    undo_before: Vec<AnnotationMark>,
    selected: Vec<usize>,
    handle: Option<Handle>,
    moved: bool,
    history_recorded: bool,
    guides: Vec<crate::annotation_snapping::Guide>,
    snapping: Option<crate::annotation_snapping::Session>,
    pen_straight_start: Option<usize>,
    edit_on_click: bool,
    clicked: Option<usize>,
    additive: bool,
    was_selected: bool,
    marquee: bool,
    /// Frozen painted bounds, including visible canvas captions and media marks.
    brush_targets: Option<Vec<(usize, Bounds<Pixels>)>>,
    /// Mixed-layer groups move in screen space, then map into each mark's space.
    group_spaces: Option<(Bounds<Pixels>, Bounds<Pixels>, Vec<Bounds<Pixels>>)>,
}

impl Studio {
    /// Caption storage is resolution independent; editing controls use the
    /// same visible pixel size as image annotations.
    pub(crate) fn annotation_text_scale(&self, mark: &AnnotationMark) -> f32 {
        if !mark.is_canvas() {
            return 1.0;
        }
        self.scene_canvas_bounds
            .lock()
            .ok()
            .and_then(|bounds| *bounds)
            .or(self.annotation_view_bounds)
            .map(lahza_annotations::canvas::canvas_scale)
            .unwrap_or(1.0)
    }

    pub(crate) fn annotation_text_preview_size(&self, mark: &AnnotationMark) -> f32 {
        mark.font_size * self.annotation_text_scale(mark)
    }

    pub(crate) fn update_annotation_modifiers(&mut self, modifiers: gpui::Modifiers) {
        if self.annotation_modifiers == modifiers {
            return;
        }
        self.annotation_modifiers = modifiers;
        if let (Some(position), Some(gesture)) =
            (self.selection_last_point, &self.annotation_gesture)
        {
            let bounds = gesture.bounds;
            if self.pointer_is_down {
                self.pointer_move(position, bounds);
            }
        }
    }

    fn annotation_snap_session(
        &self,
        gesture: &Gesture,
    ) -> Option<crate::annotation_snapping::Session> {
        use crate::annotation_snapping::{Kind, Rect, Session, Target};
        // Projected media edits use local coordinates; canvas captions remain screen-space.
        if !self.canvas_annotation_drag && !self.annotations_paint_flat() {
            return None;
        }
        let canvas = self
            .scene_canvas_bounds
            .lock()
            .ok()
            .and_then(|b| *b)
            .unwrap_or(gesture.bounds);
        let frame = self
            .video_media_bounds
            .lock()
            .ok()
            .and_then(|b| *b)
            .unwrap_or(gesture.bounds);
        let projection = (!self.annotations_paint_flat()).then(|| {
            self.preview_projection(f32::from(canvas.size.width), f32::from(canvas.size.height))
                .1
        });
        let projected_bounds = |bounds: Rect| {
            let Some(projection) = projection else {
                return bounds;
            };
            let corners = [
                bounds.min,
                crate::annotation_geometry::V::new(bounds.max.x, bounds.min.y),
                bounds.max,
                crate::annotation_geometry::V::new(bounds.min.x, bounds.max.y),
            ];
            corners
                .into_iter()
                .map(|p| {
                    let (x, y) = projection.project(
                        (p.x - f32::from(frame.left())) as f64
                            / f32::from(frame.size.width).max(1.) as f64,
                        (p.y - f32::from(frame.top())) as f64
                            / f32::from(frame.size.height).max(1.) as f64,
                    );
                    Rect::new(
                        f32::from(canvas.left()) + x as f32,
                        f32::from(canvas.top()) + y as f32,
                        0.,
                        0.,
                    )
                })
                .reduce(Rect::union)
                .unwrap_or(bounds)
        };
        let media = if self.scene_is_timed() {
            let view = self.video_viewport_timeline.frame_at(self.video_position);
            let (left, top, visible_x, visible_y) =
                self.scene_style().source_crop.visible_rect(view);
            Bounds::new(
                point(
                    frame.left() - frame.size.width * (left / visible_x) as f32,
                    frame.top() - frame.size.height * (top / visible_y) as f32,
                ),
                size(
                    frame.size.width / visible_x as f32,
                    frame.size.height / visible_y as f32,
                ),
            )
        } else {
            frame
        };
        let moving = gesture
            .selected
            .iter()
            .filter_map(|i| gesture.before.get(*i))
            .map(|m| annotation_snap_bounds(m, gesture.bounds))
            .reduce(Rect::union)?;
        let moving = if gesture.handle.is_some() {
            Rect::new(
                f32::from(gesture.origin.x),
                f32::from(gesture.origin.y),
                0.,
                0.,
            )
        } else {
            moving
        };
        let mut targets = vec![Target {
            bounds: canvas.into(),
            kind: Kind::Canvas,
        }];
        if self.image_visible_at(self.video_position) {
            targets.push(Target {
                bounds: projected_bounds(frame.into()),
                kind: Kind::Image,
            });
        }
        for (i, mark) in gesture.before.iter().enumerate() {
            if gesture.selected.contains(&i)
                || mark.opacity <= 0.001
                || (mark.tool == Tool::Text && mark.text.trim().is_empty())
                || (!mark.is_canvas() && !self.image_visible_at(self.video_position))
                || (self.scene_is_timed()
                    && mark
                        .timing
                        .is_some_and(|t| !t.state_at(self.video_position).visible))
            {
                continue;
            }
            // All targets are converted to the same screen coordinates, including
            // captions, pinned marks and zoomed media annotations.
            let space = if mark.is_canvas() {
                canvas
            } else if mark.pinned {
                frame
            } else {
                media
            };
            let bounds = annotation_snap_bounds(mark, space);
            let clip: Rect = if mark.is_canvas() {
                canvas.into()
            } else {
                frame.into()
            };
            // Do not snap to invisible parts of a clipped annotation.
            if !bounds.intersects(clip)
                || bounds.min.x < clip.min.x
                || bounds.max.x > clip.max.x
                || bounds.min.y < clip.min.y
                || bounds.max.y > clip.max.y
            {
                continue;
            }
            targets.push(Target {
                bounds: if mark.is_canvas() {
                    bounds
                } else {
                    projected_bounds(bounds)
                },
                kind: Kind::Object,
            });
        }
        Some(Session::new(moving, targets, canvas.into()))
    }

    pub(crate) fn cancel_annotation_gesture(&mut self) {
        self.annotation_edit_preview_pending = false;
        self.annotation_editing_time = None;
        if let Some(g) = self.annotation_gesture.take() {
            if g.history_recorded {
                if self.scene_is_timed() {
                    self.video_undo_stack.pop();
                } else {
                    self.undo_stack.pop();
                }
            }
            self.annotations = g.undo_before;
            self.annotation_selection = g
                .selected
                .into_iter()
                .filter(|i| *i < self.annotations.len())
                .collect();
            self.selected_annotation = self.annotation_selection.last().copied();
            self.annotation_draft = None;
            self.pointer_is_down = false;
            self.selection_last_point = None;
        }
    }
    /// The shared screen-space outline also defines the group's drag surface.
    pub(crate) fn annotation_group_bounds(
        &self,
        canvas: Bounds<Pixels>,
        media: Bounds<Pixels>,
        rendered: &[Bounds<Pixels>],
        canvas_hits: &[(usize, Bounds<Pixels>)],
    ) -> Option<Bounds<Pixels>> {
        self.annotation_group_boxes(canvas, media, rendered, canvas_hits)
            .into_iter().reduce(|a, b| a.union(&b))
    }

    fn annotation_group_boxes(
        &self,
        canvas: Bounds<Pixels>,
        media: Bounds<Pixels>,
        rendered: &[Bounds<Pixels>],
        canvas_hits: &[(usize, Bounds<Pixels>)],
    ) -> Vec<Bounds<Pixels>> {
        let selected = self.annotation_selected_indices();
        if selected.len() < 2 || self.editing_text.is_some() || self.crop_active {
            return Vec::new();
        }
        let frame = self.pinned_bounds(media);
        let projection = (!self.annotations_paint_flat()).then(|| {
            self.preview_projection(f32::from(canvas.size.width), f32::from(canvas.size.height)).1
        });
        selected.into_iter().filter_map(|index| {
            let original = &self.annotations[index];
            if self.scene_is_timed() && !original.is_canvas() && !self.image_visible_at(self.video_position) {
                return None;
            }
            let mut mark = if self.scene_is_timed() {
                crate::timed::editor_mark(original, self.video_position, self.annotation_is_live(index))?
            } else { original.clone() };
            if mark.opacity <= 0.001 { return None; }
            let space = if mark.is_canvas() { canvas } else if mark.pinned { frame } else { media };
            let painted = if mark.is_canvas() {
                canvas_hits.iter().find(|(i, _)| *i == index).map(|(_, b)| *b)
            } else { rendered.get(index).copied() };
            if mark.is_canvas() {
                let scale = lahza_annotations::canvas::canvas_scale(canvas);
                mark.font_size *= scale;
                mark.scale_stroke_width(scale);
            }
            let mut bounds = painted.unwrap_or_else(|| {
                if matches!(mark.tool, Tool::Arrow | Tool::Line | Tool::Pen) {
                    crate::annotation_geometry::geometry(&mark, space).bounds(mark.stroke_width * 0.7)
                } else {
                    let b = annotation_snap_bounds(&mark, space);
                    Bounds::from_corners(b.min.screen(), b.max.screen())
                }
            });
            if bounds.size.width <= px(0.) || bounds.size.height <= px(0.) { return None; }
            if let Some(projection) = projection.as_ref().filter(|_| !mark.is_canvas()) {
                bounds = [bounds.origin, point(bounds.right(), bounds.top()),
                    point(bounds.right(), bounds.bottom()), point(bounds.left(), bounds.bottom())]
                    .into_iter().map(|p| {
                        let (x, y) = projection.project(
                            f32::from(p.x - frame.left()) as f64 / f32::from(frame.size.width).max(1.) as f64,
                            f32::from(p.y - frame.top()) as f64 / f32::from(frame.size.height).max(1.) as f64,
                        );
                        Bounds::new(point(canvas.left() + px(x as f32), canvas.top() + px(y as f32)), size(px(0.), px(0.)))
                    }).reduce(|a, b| a.union(&b)).unwrap();
            }
            let clip = if mark.is_canvas() || projection.is_some() { canvas } else { frame };
            let bounds = Bounds::from_corners(
                point(bounds.left().max(clip.left()), bounds.top().max(clip.top())),
                point(bounds.right().min(clip.right()), bounds.bottom().min(clip.bottom())),
            );
            (bounds.size.width > px(0.) && bounds.size.height > px(0.)).then_some(bounds)
        }).collect()
    }

    fn begin_annotation_group_move(
        &mut self,
        position: Point<Pixels>,
        canvas: Bounds<Pixels>,
        media: Bounds<Pixels>,
        screen_space: bool,
    ) {
        self.stop_editing_text();
        if self.scene_is_timed() && self.video_playing { self.pause_video_playback(); }
        let selected = self.annotation_selected_indices();
        let spaces: Vec<_> = self.annotations.iter().map(|mark| {
            if mark.is_canvas() { canvas } else if mark.pinned { self.pinned_bounds(media) } else { media }
        }).collect();
        let bounds = spaces[selected[0]];
        let mixed = selected.iter().any(|i| spaces[*i] != bounds)
            || (screen_space && !self.annotations_paint_flat());
        self.pointer_is_down = true;
        self.annotation_editing_time = self.scene_is_timed().then_some(self.video_position);
        self.annotation_view_bounds = Some(canvas);
        self.annotation_gesture = Some(Gesture {
            origin: position,
            bounds: if mixed { canvas } else { bounds },
            before: self.annotations.clone(),
            undo_before: self.annotations.clone(),
            selected,
            handle: None,
            moved: false,
            history_recorded: false,
            guides: Vec::new(),
            snapping: None,
            pen_straight_start: None,
            edit_on_click: false,
            clicked: self.selected_annotation,
            additive: false,
            was_selected: true,
            marquee: false,
            brush_targets: None,
            group_spaces: mixed.then_some((canvas, media, spaces)),
        });
        if screen_space { self.canvas_annotation_drag = true; }
    }

    pub(crate) fn annotation_cursor(
        &self,
        position: Point<Pixels>,
        image: Bounds<Pixels>,
        rendered: &[Bounds<Pixels>],
    ) -> gpui::CursorStyle {
        use gpui::CursorStyle as C;
        if self.tool == Tool::Text
            || self.editing_text.is_some_and(|i| {
                self.annotations
                    .get(i)
                    .is_some_and(|m| hit_mark(m, position, image, rendered.get(i).copied()))
            })
        {
            return C::IBeam;
        }
        if self.tool != Tool::Select {
            return C::Crosshair;
        }
        if let Some(i) = self.selected_annotation.filter(|_| self.annotation_selected_indices().len() == 1) {
            if let Some(m) = self.annotations.get(i) {
                let b = rendered
                    .get(i)
                    .copied()
                    .unwrap_or_else(|| mark_screen_bounds(m, image));
                if let Some((h, _)) = handles(m, image, b).into_iter().find(|(_, p)| {
                    crate::annotation_geometry::V::from(*p)
                        .sub(position.into())
                        .len()
                        < 9.
                }) {
                    return match h {
                        Handle::Left | Handle::Right => C::ResizeLeftRight,
                        Handle::Corner(0 | 2) => C::ResizeUpLeftDownRight,
                        Handle::Corner(_) => C::ResizeUpRightDownLeft,
                        _ => C::Crosshair,
                    };
                }
            }
        }
        let selection_hit = self.annotation_selected_indices().into_iter().any(|i| {
            let original = &self.annotations[i];
            let mark = if self.scene_is_timed() {
                crate::timed::editor_mark(original, self.video_position, self.annotation_is_live(i))
            } else { Some(original.clone()) };
            mark.is_some_and(|mark| hit_selection_box(&mark, position, image, rendered.get(i).copied()))
        });
        if selection_hit || self
            .annotations
            .iter()
            .enumerate()
            .any(|(i, m)| hit_mark(m, position, image, rendered.get(i).copied()))
        {
            if self.pointer_is_down {
                C::ClosedHand
            } else {
                C::OpenHand
            }
        } else {
            C::Arrow
        }
    }
    pub(crate) fn annotation_selected_indices(&self) -> Vec<usize> {
        if self
            .selected_annotation
            .is_some_and(|i| self.annotation_selection.contains(&i))
        {
            self.annotation_selection
                .iter()
                .copied()
                .filter(|i| *i < self.annotations.len())
                .collect()
        } else {
            self.selected_annotation
                .filter(|i| *i < self.annotations.len())
                .into_iter()
                .collect()
        }
    }
    /// Delete the entire canvas selection as one history operation.
    pub(crate) fn delete_selected_annotations(&mut self) -> bool {
        let mut indices = self.annotation_selected_indices();
        if indices.is_empty() {
            return false;
        }
        indices.sort_unstable();
        indices.dedup();
        self.record_annotation_undo();
        // Removing from the end preserves the indices of remaining selections.
        for index in indices.into_iter().rev() {
            self.annotations.remove(index);
        }
        self.selected_annotation = None;
        self.annotation_selection.clear();
        self.editing_text = None;
        self.annotation_time_edit = None;
        self.canvas_caret_point = None;
        self.annotation_gesture = None;
        self.annotation_draft = None;
        self.selection_last_point = None;
        self.pointer_is_down = false;
        true
    }
    pub(crate) fn annotation_brushing(&self) -> bool {
        self.annotation_gesture.as_ref().is_some_and(|g| g.marquee)
    }
    pub(crate) fn paint_annotation_brush(&self, window: &mut Window, cx: &mut App) {
        if let Some(g) = &self.annotation_gesture {
            if let Some(session) = &g.snapping {
                let clip =
                    Bounds::from_corners(session.clip.min.screen(), session.clip.max.screen());
                window.with_content_mask(Some(gpui::ContentMask { bounds: clip }), |window| {
                    for guide in &g.guides {
                        paint_snap_guide(guide, clip, window, cx);
                    }
                });
            }
        }
        if let Some(g) = self
            .annotation_gesture
            .as_ref()
            .filter(|g| g.marquee && g.moved)
        {
            if let Some(end) = self.selection_last_point {
                let bounds = Bounds::from_corners(
                    point(g.origin.x.min(end.x), g.origin.y.min(end.y)),
                    point(g.origin.x.max(end.x), g.origin.y.max(end.y)),
                );
                window.paint_quad(quad(
                    bounds,
                    px(0.),
                    gpui::rgba(0x2997ff15),
                    px(1.),
                    rgb(0x2997ff),
                    Default::default(),
                ));
            }
        }
    }
    pub(super) fn record_annotation_undo(&mut self) {
        if self.scene_is_timed() {
            self.video_undo_stack
                .push(crate::VideoEditSnapshot::Annotations(
                    self.annotations.clone(),
                ));
            self.video_redo_stack.clear();
            return;
        }
        self.undo_stack.push(self.annotations.clone());
        if self.undo_stack.len() > 100 {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
    }

    pub(super) fn undo_annotations(&mut self) -> bool {
        let Some(previous) = self.undo_stack.pop() else {
            return false;
        };
        self.redo_stack
            .push(std::mem::replace(&mut self.annotations, previous));
        self.selected_annotation = None;
        self.editing_text = None;
        self.annotation_draft = None;
        self.annotation_gesture = None;
        self.annotation_selection.clear();
        self.pointer_is_down = false;
        self.annotations
            .retain(|m| m.tool != Tool::Text || !m.text.trim().is_empty());
        true
    }

    pub(super) fn redo_annotations(&mut self) -> bool {
        let Some(next) = self.redo_stack.pop() else {
            return false;
        };
        self.undo_stack
            .push(std::mem::replace(&mut self.annotations, next));
        self.selected_annotation = None;
        self.editing_text = None;
        self.annotation_draft = None;
        self.annotation_gesture = None;
        self.annotation_selection.clear();
        self.pointer_is_down = false;
        self.annotations
            .retain(|m| m.tool != Tool::Text || !m.text.trim().is_empty());
        true
    }

    /// Return keyboard control to the timeline when leaving canvas editing.
    pub(super) fn finish_annotation_interaction(&mut self) {
        self.annotation_edit_preview_pending = false;
        self.annotation_editing_time = None;
        self.stop_editing_text();
        self.selected_annotation = None;
        self.selection_last_point = None;
        self.annotation_gesture = None;
        self.annotation_selection.clear();
        self.pointer_is_down = false;
        self.toast = None;
    }

    pub(super) fn stop_editing_text(&mut self) {
        self.watermark_editing = false;
        self.annotation_time_edit = None;
        let Some(index) = self.editing_text.take() else {
            return;
        };
        let is_empty = self
            .annotations
            .get(index)
            .is_none_or(|mark| mark.text.trim().is_empty());
        if is_empty && index < self.annotations.len() {
            self.annotations.remove(index);
            self.selected_annotation = None;
            self.annotation_selection.clear();
            if self.scene_is_timed() {
                if self.video_undo_stack.last().is_some_and(|s|matches!(s,crate::VideoEditSnapshot::Annotations(m) if m==&self.annotations)){self.video_undo_stack.pop();}
            } else if self.undo_stack.last() == Some(&self.annotations) {
                self.undo_stack.pop();
            }
        }
        if self.tool == Tool::Text {
            self.tool = Tool::Select;
        }
    }

    pub(super) fn fit_text_box_to_content(&mut self, index: usize) {
        let canvas_text = self
            .annotations
            .get(index)
            .is_some_and(|mark| mark.is_canvas());
        let aspect = self
            .media_dimensions()
            .map(|(width, height)| width as f32 / height.max(1) as f32)
            .unwrap_or(16.0 / 9.0)
            .max(0.1);
        let aspect = if canvas_text {
            self.selected_canvas_ratio()
        } else {
            aspect
        };
        let view = if canvas_text {
            self.scene_canvas_bounds
                .lock()
                .ok()
                .and_then(|bounds| *bounds)
                .or(self.annotation_view_bounds)
        } else {
            self.annotation_view_bounds
        };
        let Some(mark) = self.annotations.get_mut(index) else {
            return;
        };
        if mark.tool != Tool::Text {
            return;
        }
        let preview_image_height = view.map(|b| f32::from(b.size.height)).unwrap_or(800.);
        let preview_image_width = view
            .map(|b| f32::from(b.size.width))
            .unwrap_or(preview_image_height * aspect);
        let display_scale = if canvas_text {
            preview_image_width.min(preview_image_height) / 800.
        } else {
            1.
        };
        let old_width = (mark.end.x - mark.start.x).abs() * preview_image_width;
        let mut display = mark.clone();
        display.font_size *= display_scale;
        let layout = crate::annotation_text::layout(&display, old_width);
        if mark.text_auto_width {
            let dx = (layout.width - old_width) / preview_image_width.max(1.);
            let anchor = match mark.text_alignment {
                1 => 0.5,
                2 => 1.,
                _ => 0.,
            };
            mark.start.x -= dx * anchor;
            mark.end.x = mark.start.x + layout.width / preview_image_width.max(1.);
        }
        mark.end.y = mark.start.y + layout.height / preview_image_height.max(1.);
    }

    /// Pick the same visible, animated marks that the two preview layers paint.
    /// A background press starts one screen-space marquee across both layers.
    pub(crate) fn scene_annotation_select(
        &mut self,
        position: Point<Pixels>,
        canvas: Bounds<Pixels>,
        media: Bounds<Pixels>,
        rendered: &[Bounds<Pixels>],
        canvas_hits: &[(usize, Bounds<Pixels>)],
        click_count: usize,
    ) -> bool {
        if self.tool != Tool::Select
            || self.crop_active
            || self.walkthrough_mode
            || self.annotations.is_empty()
            || !canvas.contains(&position)
        {
            return false;
        }
        let group_boxes = self.annotation_group_boxes(canvas, media, rendered, canvas_hits);
        // Double-clicking a member can enter editing; a gap remains a drag surface.
        if !self.annotation_modifiers.shift
            && (click_count < 2 || !group_boxes.iter().any(|b| b.contains(&position)))
            && group_boxes.into_iter().reduce(|a, b| a.union(&b))
                .is_some_and(|bounds| bounds.contains(&position))
        {
            self.begin_annotation_group_move(position, canvas, media, true);
            self.video_selected_zoom_cue = None;
            self.video_selected_clip = None;
            self.scene_selection = crate::SceneSelection::Scene;
            return true;
        }
        if !self.scene_is_timed() { return false; }
        let frame = self
            .video_media_bounds
            .lock()
            .ok()
            .and_then(|b| *b)
            .unwrap_or(media);
        let projected = !self.annotations_paint_flat();
        let projection = projected.then(|| {
            self.preview_projection(f32::from(canvas.size.width), f32::from(canvas.size.height))
                .1
        });
        let flat = if projected {
            self.flat_pointer_position(position, canvas, media)
        } else {
            position
        };
        let hidden = Bounds::new(point(px(-100000.), px(-100000.)), size(px(0.), px(0.)));
        let mut bounds = vec![hidden; self.annotations.len()];
        let mut brush = Vec::new();
        let mut media_hit = None;
        let mut canvas_hit = None;
        let mut handle_hit = None;
        let mut selection_hit = None;
        let selected_indices = self.annotation_selected_indices();
        let mut spaces: Vec<_> = self
            .annotations
            .iter()
            .map(|mark| {
                if mark.is_canvas() {
                    canvas
                } else if mark.pinned {
                    frame
                } else {
                    media
                }
            })
            .collect();
        for (index, original) in self.annotations.iter().enumerate() {
            if !original.is_canvas() && !self.image_visible_at(self.video_position) {
                continue;
            }
            let Some(mark) = (if self.scene_is_timed() {
                crate::timed::editor_mark(
                    original,
                    self.video_position,
                    self.annotation_is_live(index),
                )
            } else {
                Some(original.clone())
            }) else {
                continue;
            };
            if mark.opacity <= 0.001 {
                continue;
            }
            let space = spaces[index];
            let painted = if mark.is_canvas() {
                canvas_hits
                    .iter()
                    .find(|(i, _)| *i == index)
                    .map(|(_, b)| *b)
            } else {
                rendered.get(index).copied()
            };
            let b = painted.unwrap_or_else(|| {
                if matches!(mark.tool, Tool::Arrow | Tool::Line | Tool::Pen) {
                    return crate::annotation_geometry::geometry(&mark, space)
                        .bounds(mark.stroke_width * 0.7);
                }
                let b = annotation_snap_bounds(&mark, space);
                Bounds::from_corners(b.min.screen(), b.max.screen())
            });
            if b.size.width <= px(0.) || b.size.height <= px(0.) {
                continue;
            }
            bounds[index] = b;
            let screen = if mark.is_canvas() || !projected {
                b
            } else {
                let projection = projection.as_ref().unwrap();
                let corners = [
                    b.origin,
                    point(b.right(), b.top()),
                    point(b.right(), b.bottom()),
                    point(b.left(), b.bottom()),
                ];
                let mut min = point(px(f32::MAX), px(f32::MAX));
                let mut max = point(px(f32::MIN), px(f32::MIN));
                for p in corners {
                    let (x, y) = projection.project(
                        f32::from(p.x - frame.left()) as f64
                            / f32::from(frame.size.width).max(1.) as f64,
                        f32::from(p.y - frame.top()) as f64
                            / f32::from(frame.size.height).max(1.) as f64,
                    );
                    let p = point(canvas.left() + px(x as f32), canvas.top() + px(y as f32));
                    min.x = min.x.min(p.x);
                    min.y = min.y.min(p.y);
                    max.x = max.x.max(p.x);
                    max.y = max.y.max(p.y);
                }
                Bounds::from_corners(min, max)
            };
            let clip = if mark.is_canvas() || projected {
                canvas
            } else {
                frame
            };
            let clipped = Bounds::from_corners(
                point(screen.left().max(clip.left()), screen.top().max(clip.top())),
                point(
                    screen.right().min(clip.right()),
                    screen.bottom().min(clip.bottom()),
                ),
            );
            if clipped.size.width <= px(0.) || clipped.size.height <= px(0.) {
                continue;
            }
            brush.push((index, clipped));
            let p = if mark.is_canvas() { position } else { flat };
            let inside_media = mark.is_canvas()
                || (if projected {
                    frame.contains(&flat)
                } else {
                    frame.contains(&position)
                });
            if !inside_media {
                continue;
            }
            if selected_indices.len() == 1 && self.selected_annotation == Some(index)
                && click_count < 2
                && handles(&mark, space, b)
                    .iter()
                    .any(|(_, h)| crate::annotation_geometry::V::from(*h).sub(p.into()).len() <= 9.)
            {
                handle_hit = Some(index);
            }
            if selected_indices.contains(&index) && hit_selection_box(&mark, p, space, Some(b)) {
                selection_hit = Some(index);
            }
            if hit_mark(&mark, p, space, Some(b)) {
                if mark.is_canvas() {
                    canvas_hit = Some(index);
                } else {
                    media_hit = Some(index);
                }
            }
        }
        let hit = handle_hit.or(selection_hit).or(canvas_hit).or(media_hit);
        // Preserve focus picking for an explicitly selected motion region.
        if self.video_selected_zoom_cue.is_some()
            && hit.is_none_or(|i| !self.annotations[i].is_canvas())
        {
            return false;
        }
        let discarded = self.editing_text.filter(|i| {
            self.annotations
                .get(*i)
                .is_some_and(|m| m.text.trim().is_empty())
        });
        if let Some(index) = hit {
            // Only offer the visible hit to the lower-level gesture code; it must
            // not rediscover a hidden or covered mark from unanimated geometry.
            let b = bounds[index];
            bounds.fill(hidden);
            bounds[index] = b;
            self.canvas_annotation_drag = self.annotations[index].is_canvas();
            self.pointer_down(
                if self.canvas_annotation_drag {
                    position
                } else {
                    flat
                },
                if self.canvas_annotation_drag { canvas } else { media },
                &bounds,
                click_count,
            );
            if let Some(removed) = discarded {
                spaces.remove(removed);
            }
            if let Some(g) = self.annotation_gesture.as_mut() {
                let distinct_spaces = g
                    .selected
                    .iter()
                    .filter_map(|i| spaces.get(*i))
                    .any(|space| *space != g.bounds);
                if g.handle.is_none() && g.selected.len() > 1 && distinct_spaces {
                    g.group_spaces = Some((canvas, media, spaces.clone()));
                    g.origin = position;
                    g.bounds = canvas;
                    self.canvas_annotation_drag = true;
                }
            }
        } else {
            self.canvas_annotation_drag = true;
            bounds.fill(hidden);
            self.pointer_down(position, canvas, &bounds, click_count);
            if let Some(g) = self.annotation_gesture.as_mut().filter(|g| g.marquee) {
                if let Some(removed) = discarded {
                    brush.retain(|(i, _)| *i != removed);
                    for (i, _) in &mut brush {
                        *i -= usize::from(*i > removed);
                    }
                }
                g.brush_targets = Some(brush);
            }
        }
        self.video_selected_zoom_cue = None;
        self.video_selected_clip = None;
        self.scene_selection = crate::SceneSelection::Scene;
        true
    }

    pub(super) fn pointer_down(
        &mut self,
        position: Point<Pixels>,
        image: Bounds<Pixels>,
        rendered_bounds: &[Bounds<Pixels>],
        click_count: usize,
    ) {
        // GPUI can retain more than one paint-scoped mouse listener across a
        // redraw. Treat a physical press as one editing transaction so a
        // single click cannot create stacked duplicate annotations.
        if self.pointer_is_down {
            return;
        }
        let mut visible_marks: Vec<_> = self.annotations.iter().enumerate().map(|(i, mark)| {
            if self.scene_is_timed() {
                crate::timed::editor_mark(mark, self.video_position, self.annotation_is_live(i))
            } else { Some(mark.clone()) }
        }).collect();
        self.pointer_is_down = true;
        if !image.contains(&position) {
            return;
        }
        if self.scene_is_timed() && self.video_playing {
            self.pause_video_playback();
        }
        self.annotation_editing_time = self.scene_is_timed().then_some(self.video_position);
        self.annotation_view_bounds = Some(image);
        let was_editing = self.editing_text.is_some();
        let removed = self.editing_text.filter(|i| {
            self.annotations
                .get(*i)
                .is_some_and(|m| m.text.trim().is_empty())
        });
        self.stop_editing_text();
        let mut adjusted = rendered_bounds.to_vec();
        if let Some(i) = removed {
            if i < visible_marks.len() { visible_marks.remove(i); }
            if i < adjusted.len() {
                adjusted.remove(i);
            }
        }
        let rendered_bounds = adjusted.as_slice();
        let normalized = screen_to_norm(position, image);
        if self.tool == Tool::Select {
            let selected = self.annotation_selected_indices();
            if selected.len() > 1 && !self.annotation_modifiers.shift {
                let boxes: Vec<_> = selected.iter().filter_map(|&index| {
                    let mark = visible_marks.get(index)?.as_ref()?;
                    if mark.is_canvas() != self.canvas_annotation_drag || mark.opacity <= 0.001 { return None; }
                    let bounds = rendered_bounds.get(index).copied().unwrap_or_else(|| {
                        mark_screen_bounds(mark, if mark.pinned { self.pinned_bounds(image) } else { image })
                    });
                    (bounds.size.width > px(0.) && bounds.size.height > px(0.)).then_some(bounds)
                }).collect();
                let group = boxes.iter().copied().reduce(|a, b| a.union(&b));
                if (click_count < 2 || !boxes.iter().any(|b| b.contains(&position)))
                    && group.is_some_and(|bounds| bounds.contains(&position)) {
                    self.begin_annotation_group_move(position, image, image, false);
                    return;
                }
            }
            let previous = self.selected_annotation;
            let handle = previous.filter(|_| selected.len() == 1 && click_count < 2).and_then(|index| {
                let mark = visible_marks.get(index)?.as_ref()?;
                if mark.is_canvas() != self.canvas_annotation_drag {
                    return None;
                }
                let bounds = rendered_bounds
                    .get(index)
                    .copied()
                    .unwrap_or_else(|| mark_screen_bounds(mark, image));
                if bounds.size.width == px(0.) && bounds.size.height == px(0.) {
                    return None;
                }
                let space = if mark.pinned && !mark.is_canvas() { self.pinned_bounds(image) } else { image };
                handles(mark, space, bounds)
                    .into_iter()
                    .find(|(_, p)| {
                        crate::annotation_geometry::V::from(*p)
                            .sub(position.into())
                            .len()
                            <= 9.
                    })
                    .map(|(h, _)| (index, h))
            });
            let selection_hit = self.annotation_selected_indices().into_iter().rev().find(|&index| {
                let Some(mark) = visible_marks.get(index).and_then(|mark| mark.as_ref()) else { return false; };
                if mark.is_canvas() != self.canvas_annotation_drag { return false; }
                let space = if mark.pinned && !mark.is_canvas() { self.pinned_bounds(image) } else { image };
                hit_selection_box(mark, position, space, rendered_bounds.get(index).copied())
            });
            let hit = handle.map(|(i, _)| i).or(selection_hit).or_else(|| {
                self.annotations
                    .iter()
                    .enumerate()
                    .rfind(|(index, mark)| {
                        let Some(visible) = visible_marks.get(*index).and_then(|m| m.as_ref()) else { return false; };
                        mark.is_canvas() == self.canvas_annotation_drag
                            && hit_mark(
                                visible,
                                position,
                                if mark.pinned && !mark.is_canvas() {
                                    self.pinned_bounds(image)
                                } else {
                                    image
                                },
                                rendered_bounds.get(*index).copied(),
                            )
                    })
                    .map(|(i, _)| i)
            });
            if previous.is_none_or(|i| !self.annotation_selection.contains(&i)) {
                self.annotation_selection = previous.into_iter().collect();
            }
            let was_selected = hit.is_some_and(|i| self.annotation_selection.contains(&i));
            if click_count >= 2 && !self.annotation_modifiers.shift && hit.is_some() {
                self.annotation_selection = hit.into_iter().collect();
            } else if self.annotation_modifiers.shift && handle.is_none() {
                if let Some(i) = hit {
                    if !was_selected {
                        self.annotation_selection.push(i);
                    }
                }
            } else if let Some(i) = hit {
                if !was_selected {
                    self.annotation_selection = vec![i];
                }
            } else {
                self.annotation_selection.clear();
            }
            self.selected_annotation = hit
                .filter(|i| self.annotation_selection.contains(i))
                .or_else(|| self.annotation_selection.last().copied());
            if hit.is_none() && self.annotations.is_empty() {
                self.pointer_is_down = false;
                self.annotation_gesture = None;
                return;
            }
            let edit_on_click = self.annotation_selection.len() == 1
                && hit.is_some_and(|i| {
                    self.annotations[i].tool == Tool::Text
                        && (click_count >= 2 || previous == Some(i) || was_editing)
                })
                && handle.is_none()
                && !self.annotation_modifiers.shift;
            self.annotation_gesture = Some(Gesture {
                origin: position,
                bounds: if hit
                    .is_some_and(|i| self.annotations[i].pinned && !self.annotations[i].is_canvas())
                {
                    self.pinned_bounds(image)
                } else {
                    image
                },
                before: self.annotations.clone(),
                undo_before: self.annotations.clone(),
                selected: self.annotation_selection.clone(),
                handle: handle.map(|(_, h)| h),
                moved: false,
                history_recorded: false,
                guides: Vec::new(),
                snapping: None,
                pen_straight_start: None,
                edit_on_click,
                clicked: hit,
                additive: self.annotation_modifiers.shift,
                was_selected,
                marquee: hit.is_none(),
                brush_targets: None,
                group_spaces: None,
            });
            if click_count >= 2 && hit.is_none() {
                self.annotation_gesture = None;
                self.pointer_is_down = false;
                self.tool = Tool::Text;
                self.pointer_down(position, image, rendered_bounds, 1);
            }
            return;
        }

        let mut number = 1;
        while self
            .annotations
            .iter()
            .any(|mark| mark.tool == Tool::Number && mark.number == number)
        {
            number += 1;
        }
        self.annotation_gesture = Some(Gesture {
            origin: position,
            bounds: image,
            before: self.annotations.clone(),
            undo_before: self.annotations.clone(),
            selected: Vec::new(),
            handle: None,
            moved: false,
            history_recorded: false,
            guides: Vec::new(),
            snapping: None,
            pen_straight_start: None,
            edit_on_click: false,
            clicked: None,
            additive: false,
            was_selected: false,
            marquee: false,
            brush_targets: None,
            group_spaces: None,
        });
        let color =
            ANNOTATION_COLORS[self.annotation_color_index.min(ANNOTATION_COLORS.len() - 1)].1;
        let mut mark = AnnotationMark {
            tool: self.tool,
            start: normalized,
            end: normalized,
            points: vec![normalized],
            ink_in_progress: self.tool == Tool::Pen,
            number,
            color,
            stroke_width: self.annotation_stroke_width,
            hand_drawn: self.annotation_hand_drawn,
            draw_seed: crate::annotation_geometry::new_draw_seed(),
            density: self.redaction_strength as f32 / 100.0,
            text: String::new(),
            font_size: self.text_font_size,
            font_family: self.text_font_family,
            text_alignment: self.text_alignment,
            bold: self.text_bold,
            italic: self.text_italic,
            underline: self.text_underline,
            timing: self
                .scene_is_timed()
                .then(|| AnnotationTiming::for_tool(self.tool, self.video_position, self.video_duration)),
            opacity: 1.0,
            from_template: false,
            pinned: false,
            canvas: self.canvas_annotation_drag,
            ..Default::default()
        };

        if mark.tool == Tool::Text && mark.is_canvas() {
            mark.font_size /= lahza_annotations::canvas::canvas_scale(image);
        }
        if self.tool == Tool::Number {
            let diameter_x = 42.0 / (image.size.width / px(1.0));
            let diameter_y = 42.0 / (image.size.height / px(1.0));
            mark.start = NormPoint {
                x: (normalized.x - diameter_x * 0.5).max(0.0),
                y: (normalized.y - diameter_y * 0.5).max(0.0),
            };
            mark.end = NormPoint {
                x: (mark.start.x + diameter_x).min(1.0),
                y: (mark.start.y + diameter_y).min(1.0),
            };
            self.record_annotation_undo();
            self.annotations.push(mark);
            self.annotation_gesture = None;
            self.selected_annotation = Some(self.annotations.len() - 1);
        } else if self.tool == Tool::Text {
            let width = 16.0 / (image.size.width / px(1.0));
            let text_scale = if self.canvas_annotation_drag {
                lahza_annotations::canvas::canvas_scale(image)
            } else {
                1.
            };
            let height =
                (mark.font_size * text_scale * 1.35).max(16.0) / (image.size.height / px(1.0));
            mark.end = NormPoint {
                x: (normalized.x + width).min(1.0),
                y: (normalized.y + height).min(1.0),
            };
            self.record_annotation_undo();
            self.annotations.push(mark);
            if let Some(g) = self.annotation_gesture.as_mut() {
                g.history_recorded = true;
            }
            let index = self.annotations.len() - 1;
            self.selected_annotation = Some(index);
            self.editing_text = None;
            self.caret_visible = true;
        } else {
            self.selected_annotation = None;
            self.editing_text = None;
            self.annotation_draft = Some(mark);
        }
    }

    /// The on-screen frame rect for pinned marks, recovered from the zoomed
    /// interaction rect `image` and the current viewport crop.
    fn pinned_bounds(&self, image: Bounds<Pixels>) -> Bounds<Pixels> {
        if !self.scene_is_timed() { return image; }
        if let Some(frame) = self.video_media_bounds.lock().ok().and_then(|bounds| *bounds) {
            return frame;
        }
        let viewport = self.video_viewport_timeline.frame_at(self.video_position);
        let (left, top, visible_x, visible_y) = self.scene_style().source_crop.visible_rect(viewport);
        Bounds {
            origin: point(image.origin.x + image.size.width * left as f32,
                          image.origin.y + image.size.height * top as f32),
            size: size(image.size.width * visible_x as f32, image.size.height * visible_y as f32),
        }
    }

    pub(super) fn pointer_move(&mut self, position: Point<Pixels>, _image: Bounds<Pixels>) {
        // Snapping is temporary, so precise placement stays under pointer control.
        let snapping_enabled =
            self.annotation_modifiers.control || self.annotation_modifiers.platform;
        let Some(mut gesture) = self.annotation_gesture.take() else {
            return;
        };
        let image = gesture.bounds;
        let mut delta = crate::annotation_geometry::V::from(position).sub(gesture.origin.into());
        if !gesture.moved && delta.len() < 4. && self.tool != Tool::Pen {
            self.annotation_gesture = Some(gesture);
            return;
        }
        delta = constrained_delta(
            self.tool,
            delta,
            self.annotation_modifiers.shift && self.tool != Tool::Pen,
        );
        if self.tool == Tool::Select {
            if gesture.marquee {
                let brush = Bounds::from_corners(
                    point(
                        gesture.origin.x.min(position.x),
                        gesture.origin.y.min(position.y),
                    ),
                    point(
                        gesture.origin.x.max(position.x),
                        gesture.origin.y.max(position.y),
                    ),
                );
                self.annotation_selection = if self.annotation_modifiers.shift {
                    gesture.selected.clone()
                } else {
                    Vec::new()
                };
                let targets = gesture.brush_targets.clone().unwrap_or_else(|| {
                    self.annotations.iter().enumerate().filter_map(|(i, m)| {
                        if m.is_canvas() != self.canvas_annotation_drag { return None; }
                        let mark = if self.scene_is_timed() {
                            crate::timed::animated_mark(m, self.video_position)?
                        } else { m.clone() };
                        let bounds = annotation_snap_bounds(&mark, image);
                        Some((i, Bounds::from_corners(bounds.min.screen(), bounds.max.screen())))
                    }).collect()
                });
                for (i, b) in targets {
                    if brush.intersects(&b) && !self.annotation_selection.contains(&i) {
                        self.annotation_selection.push(i);
                    }
                }
                self.selected_annotation = self.annotation_selection.last().copied();
            } else {
                if !gesture.moved {
                    self.record_annotation_undo();
                    gesture.history_recorded = true;
                    if self.annotation_modifiers.alt && gesture.handle.is_none() {
                        let mut ids = Vec::new();
                        for i in &gesture.selected {
                            let m = gesture.before[*i].clone();
                            ids.push(self.annotations.len());
                            self.annotations.push(m);
                            if let Some((_, _, spaces)) = &mut gesture.group_spaces {
                                spaces.push(spaces[*i]);
                            }
                        }
                        gesture.before = self.annotations.clone();
                        gesture.selected = ids.clone();
                        self.annotation_selection = ids;
                        self.selected_annotation = self.annotation_selection.last().copied();
                    }
                }
                if let Some(index) = self.selected_annotation {
                    if let Some((canvas, media, spaces)) = &gesture.group_spaces {
                        // A pixel of motion must move every selected mark by the
                        // same visible amount, regardless of its storage space.
                        let end = point(gesture.origin.x + px(delta.x), gesture.origin.y + px(delta.y));
                        let projected = !self.annotations_paint_flat();
                        let mut moves = Vec::new();
                        let mut fraction = 1.0_f32;
                        for &i in &gesture.selected {
                            let mark = &gesture.before[i];
                            let space = spaces[i];
                            let (start, end) = if projected && !mark.is_canvas() {
                                (self.flat_pointer_position(gesture.origin, *canvas, *media),
                                 self.flat_pointer_position(end, *canvas, *media))
                            } else { (gesture.origin, end) };
                            let dx = f32::from(end.x-start.x) / f32::from(space.size.width).max(1.);
                            let dy = f32::from(end.y-start.y) / f32::from(space.size.height).max(1.);
                            let (lx, hx, ly, hy) = translation_limits(&gesture.before, &[i]);
                            for (d, lo, hi) in [(dx,lx,hx),(dy,ly,hy)] {
                                if d > 0. { fraction = fraction.min((hi/d).max(0.)); }
                                if d < 0. { fraction = fraction.min((lo/d).max(0.)); }
                            }
                            moves.push((i, dx, dy));
                        }
                        for (i, dx, dy) in moves {
                            let mut mark = gesture.before[i].clone();
                            translate_mark(&mut mark, dx*fraction, dy*fraction);
                            self.annotations[i] = mark;
                        }
                    } else if let Some(handle) = gesture.handle {
                        let mut mark = gesture.before[index].clone();
                        let mut adjusted_position = position;
                        if handle != Handle::Bend {
                            use crate::annotation_geometry::V;
                            if snapping_enabled && gesture.snapping.is_none() {
                                gesture.snapping = self.annotation_snap_session(&gesture);
                            }
                            let mut min: V = image.origin.into();
                            let mut max =
                                V::new(f32::from(image.right()), f32::from(image.bottom()));
                            if handle == Handle::Left {
                                max.x = f32::from(norm_to_screen(mark.end, image).x) - 1.;
                            }
                            if handle == Handle::Right {
                                min.x = f32::from(norm_to_screen(mark.start, image).x) + 1.;
                            }
                            max.x = max.x.max(min.x);
                            max.y = max.y.max(min.y);
                            if let Some(snapping) = &mut gesture.snapping {
                                let origin: V = gesture.origin.into();
                                let locked = (matches!(handle, Handle::Left | Handle::Right)
                                    || (mark.tool == Tool::Text
                                        && matches!(handle, Handle::Corner(_))))
                                .then_some(1);
                                let (offset, guides) = snapping.translate(
                                    V::from(position).sub(origin),
                                    min.sub(origin),
                                    max.sub(origin),
                                    locked,
                                    snapping_enabled,
                                );
                                gesture.guides = guides;
                                adjusted_position = origin.add(offset).screen();
                            }
                        }
                        let position = adjusted_position;
                        let p = screen_to_norm(position, image);
                        match handle {
                            Handle::Start => mark.start = p,
                            Handle::End => mark.end = p,
                            Handle::Bend => {
                                let a = crate::annotation_geometry::V::from(norm_to_screen(
                                    mark.start, image,
                                ));
                                let b = crate::annotation_geometry::V::from(norm_to_screen(
                                    mark.end, image,
                                ));
                                mark.bend = crate::annotation_geometry::V::from(position)
                                    .sub(a.lerp(b, 0.5))
                                    .dot(b.sub(a).unit().perp())
                                    / b.sub(a).len().max(1.);
                                if mark.bend.abs() * b.sub(a).len() < 5. {
                                    mark.bend = 0.;
                                }
                            }
                            Handle::Left | Handle::Right => {
                                if handle == Handle::Left {
                                    mark.start.x = p.x.min(mark.end.x - 0.001);
                                } else {
                                    mark.end.x = p.x.max(mark.start.x + 0.001);
                                }
                                mark.text_auto_width = false;
                            }
                            Handle::Corner(c) => {
                                let old = mark_screen_bounds(&mark, image);
                                let anchor = match c {
                                    0 => point(old.right(), old.bottom()),
                                    1 => point(old.left(), old.bottom()),
                                    2 => old.origin,
                                    _ => point(old.right(), old.top()),
                                };
                                let sx = f32::from(position.x - anchor.x)
                                    / ((if c == 0 || c == 3 { -1. } else { 1. })
                                        * f32::from(old.size.width).max(1.));
                                let sy = f32::from(position.y - anchor.y)
                                    / ((if c == 0 || c == 1 { -1. } else { 1. })
                                        * f32::from(old.size.height).max(1.));
                                let font_scale = if mark.is_canvas() {
                                    lahza_annotations::canvas::canvas_scale(image)
                                } else {
                                    1.0
                                };
                                let sx = if mark.tool == Tool::Text {
                                    sx.abs().max(8. / (mark.font_size * font_scale))
                                } else {
                                    sx
                                };
                                let sy = if mark.tool == Tool::Text { sx } else { sy };
                                let a = screen_to_norm(anchor, image);
                                let transform = |p: &mut NormPoint| {
                                    p.x = a.x + (p.x - a.x) * sx;
                                    p.y = a.y + (p.y - a.y) * sy;
                                };
                                transform(&mut mark.start);
                                transform(&mut mark.end);
                                for p in &mut mark.points {
                                    transform(p);
                                }
                                if mark.tool == Tool::Text {
                                    mark.font_size = (mark.font_size * sx.abs())
                                        .clamp(8. / font_scale, 240. / font_scale);
                                }
                            }
                        }
                        self.annotations[index] = mark;
                    } else {
                        let (lo_x, hi_x, lo_y, hi_y) =
                            translation_limits(&gesture.before, &gesture.selected);
                        let width = f32::from(image.size.width);
                        let height = f32::from(image.size.height);
                        if snapping_enabled && gesture.snapping.is_none() {
                            gesture.snapping = self.annotation_snap_session(&gesture);
                        }
                        gesture.guides.clear();
                        if let Some(snapping) = &mut gesture.snapping {
                            let locked_axis = self
                                .annotation_modifiers
                                .shift
                                .then_some(if delta.x.abs() > delta.y.abs() { 1 } else { 0 });
                            (delta, gesture.guides) = snapping.translate(
                                delta,
                                crate::annotation_geometry::V::new(lo_x * width, lo_y * height),
                                crate::annotation_geometry::V::new(hi_x * width, hi_y * height),
                                locked_axis,
                                snapping_enabled,
                            );
                        }
                        let dx = (delta.x / width).clamp(lo_x, hi_x);
                        let dy = (delta.y / height).clamp(lo_y, hi_y);
                        for i in &gesture.selected {
                            let mut m = gesture.before[*i].clone();
                            translate_mark(&mut m, dx, dy);
                            self.annotations[*i] = m;
                        }
                    }
                }
            }
        } else if self.tool == Tool::Text {
            if delta.x.abs() < 24. {
                self.annotation_gesture = Some(gesture);
                return;
            }
            if let Some(i) = self.selected_annotation {
                let mark = &mut self.annotations[i];
                mark.text_auto_width = false;
                mark.start.x = screen_to_norm(gesture.origin, image)
                    .x
                    .min(screen_to_norm(position, image).x);
                mark.end.x = screen_to_norm(gesture.origin, image)
                    .x
                    .max(screen_to_norm(position, image).x);
            }
        } else if let Some(mark) = &mut self.annotation_draft {
            let p = point(
                gesture.origin.x + px(delta.x),
                gesture.origin.y + px(delta.y),
            );
            let normalized = screen_to_norm(p, image);
            mark.end = normalized;
            if mark.tool == Tool::Pen {
                // Straight segments stay attached to the pointer instead of trailing it.
                mark.ink_in_progress = !self.annotation_modifiers.shift;
                if self.annotation_modifiers.shift {
                    let index = *gesture
                        .pen_straight_start
                        .get_or_insert(mark.points.len().saturating_sub(1));
                    let start = mark.points.get(index).copied().unwrap_or(mark.start);
                    let screen = norm_to_screen(start, image);
                    let delta = constrained_delta(
                        Tool::Pen,
                        crate::annotation_geometry::V::from(position).sub(screen.into()),
                        true,
                    );
                    let end = screen_to_norm(
                        point(screen.x + px(delta.x), screen.y + px(delta.y)),
                        image,
                    );
                    mark.points.truncate(index + 1);
                    mark.points.push(end);
                    mark.end = end;
                } else {
                    gesture.pen_straight_start = None;
                    let last = mark.points.last().copied().unwrap_or(mark.start);
                    // Preserve mouse event spacing for pressure simulation,
                    // including the close samples produced by slow movement.
                    let next = screen_to_norm(position, image);
                    if next != last {
                        mark.points.push(next);
                    }
                }
            }
        }
        self.selection_last_point = Some(position);
        gesture.moved = true;
        self.annotation_gesture = Some(gesture);
    }

    pub(super) fn pointer_up(&mut self, position: Point<Pixels>, image: Bounds<Pixels>) -> bool {
        if self.pointer_is_down && self.scene_is_timed() {
            self.annotation_edit_preview_pending = true;
        }
        if let Some(gesture) = self.annotation_gesture.take() {
            if gesture.history_recorded && self.annotations == gesture.undo_before {
                if self.scene_is_timed() {
                    self.video_undo_stack.pop();
                } else {
                    self.undo_stack.pop();
                }
            }
            if self.tool == Tool::Select && !gesture.moved && gesture.handle.is_none() {
                if let Some(i) = gesture.clicked {
                    if gesture.additive && gesture.was_selected {
                        self.annotation_selection.retain(|id| *id != i);
                        self.selected_annotation = self.annotation_selection.last().copied();
                    } else if !gesture.additive && !gesture.was_selected {
                        self.annotation_selection = vec![i];
                        self.selected_annotation = Some(i);
                    }
                }
            }
            if self.tool == Tool::Select && !gesture.moved && gesture.edit_on_click {
                self.editing_text = self.selected_annotation;
                self.canvas_caret_point = Some(position);
            }
            if self.tool == Tool::Text {
                self.editing_text = self.selected_annotation;
                self.tool = Tool::Select;
            }
        }
        self.pointer_is_down = false;
        self.selection_last_point = None;
        let Some(mut mark) = self.annotation_draft.take() else {
            return self.selected_annotation.is_some_and(|index| {
                self.annotations
                    .get(index)
                    .is_some_and(|mark| mark.tool == Tool::Blur || mark.tool == Tool::Pixelate)
            });
        };
        let start = norm_to_screen(mark.start, image);
        let delta = constrained_delta(
            mark.tool,
            crate::annotation_geometry::V::from(position).sub(start.into()),
            self.annotation_modifiers.shift && mark.tool != Tool::Pen,
        );
        if mark.tool != Tool::Pen || !self.annotation_modifiers.shift {
            mark.end = screen_to_norm(point(start.x + px(delta.x), start.y + px(delta.y)), image);
        }
        let width = (mark.end.x - mark.start.x).abs();
        let height = (mark.end.y - mark.start.y).abs();
        if mark.tool == Tool::Pen {
            mark.ink_in_progress = false;
            if mark.points.is_empty() {
                mark.points.push(mark.start);
            }
            if mark.points.last() != Some(&mark.end) {
                mark.points.push(mark.end);
            }
        } else if matches!(mark.tool, Tool::Line | Tool::Arrow) {
            if crate::annotation_geometry::V::from(norm_to_screen(mark.end, image))
                .sub(norm_to_screen(mark.start, image).into())
                .len()
                < 2.
            {
                return false;
            }
        } else if width < 0.003 || height < 0.003 {
            if matches!(
                mark.tool,
                Tool::Rectangle
                    | Tool::FilledRectangle
                    | Tool::Ellipse
                    | Tool::Highlight
                    | Tool::Blur
                    | Tool::Pixelate
            ) {
                let fallback = 80.0;
                mark.end = NormPoint {
                    x: (mark.start.x + fallback / (image.size.width / px(1.0))).min(1.0),
                    y: (mark.start.y + fallback / (image.size.height / px(1.0))).min(1.0),
                };
            } else {
                return false;
            }
        }
        let created_tool = mark.tool;
        let needs_redaction = created_tool == Tool::Blur || created_tool == Tool::Pixelate;
        self.record_annotation_undo();
        self.annotations.push(mark);
        self.selected_annotation = Some(self.annotations.len() - 1);
        self.annotation_selection = self.selected_annotation.into_iter().collect();
        if matches!(
            created_tool,
            Tool::Rectangle
                | Tool::FilledRectangle
                | Tool::Ellipse
                | Tool::Line
                | Tool::Arrow
                | Tool::Highlight
                | Tool::Blur
                | Tool::Pixelate
        ) {
            self.tool = Tool::Select;
        }
        needs_redaction
    }

    pub(super) fn handle_key(&mut self, event: &KeyDownEvent) -> bool {
        let key = event.keystroke.key.as_str();
        let command = event.keystroke.modifiers.control || event.keystroke.modifiers.platform;
        if key == "escape" && !self.crop_active {
            if self.annotation_gesture.is_some() {
                self.cancel_annotation_gesture();
            } else {
                self.finish_annotation_interaction();
            }
            self.tool = Tool::Select;
            return true;
        }
        if !self.crop_active && self.editing_text.is_none() {
            if key == "enter"
                && self
                    .selected_annotation
                    .is_some_and(|i| self.annotations[i].tool == Tool::Text)
            {
                self.editing_text = self.selected_annotation;
                self.tool = Tool::Select;
                return true;
            }
            if command && key == "a" {
                self.annotation_selection = (0..self.annotations.len()).collect();
                self.selected_annotation = self.annotation_selection.last().copied();
                return true;
            }
            if command && key == "d" && self.selected_annotation.is_some() {
                self.record_annotation_undo();
                let selected = self.annotation_selected_indices();
                let mut ids = Vec::new();
                for i in selected {
                    let mut m = self.annotations[i].clone();
                    translate_mark(&mut m, 0.02, 0.02);
                    ids.push(self.annotations.len());
                    self.annotations.push(m);
                }
                self.annotation_selection = ids;
                self.selected_annotation = self.annotation_selection.last().copied();
                return true;
            }
            if matches!(key, "left" | "right" | "up" | "down") && self.selected_annotation.is_some()
            {
                let selected = self.annotation_selected_indices();
                let b = self.annotation_view_bounds.unwrap_or_else(|| {
                    Bounds::new(point(px(0.), px(0.)), size(px(1000.), px(1000.)))
                });
                let step = if event.keystroke.modifiers.shift {
                    10.
                } else {
                    1.
                };
                let dx = match key {
                    "left" => -step,
                    "right" => step,
                    _ => 0.,
                } / f32::from(b.size.width);
                let dy = match key {
                    "up" => -step,
                    "down" => step,
                    _ => 0.,
                } / f32::from(b.size.height);
                let (lx, hx, ly, hy) = translation_limits(&self.annotations, &selected);
                self.record_annotation_undo();
                for i in selected {
                    translate_mark(&mut self.annotations[i], dx.clamp(lx, hx), dy.clamp(ly, hy));
                }
                return true;
            }
        }
        if self.crop_active {
            match event.keystroke.key.as_str() {
                "escape" => self.cancel_crop(),
                "enter" => {
                    if let Err(error) = self.apply_crop() {
                        self.toast = Some(error.into());
                    }
                }
                _ => return false,
            }
            return true;
        }
        if (event.keystroke.modifiers.control || event.keystroke.modifiers.platform)
            && matches!(event.keystroke.key.as_str(), "z" | "y")
        {
            return if event.keystroke.modifiers.shift || event.keystroke.key == "y" {
                self.redo_annotations() || self.redo_crop()
            } else {
                self.undo_annotations() || self.undo_crop()
            };
        }

        if matches!(event.keystroke.key.as_str(), "delete" | "backspace") {
            return self.delete_selected_annotations();
        }
        false
    }

    pub(super) fn rebuild_redactions(&mut self) -> Result<(), String> {
        let Some(source_path) = self.captured_path.as_ref() else {
            return Ok(());
        };
        let mut output = image::open(source_path)
            .map_err(|error| error.to_string())?
            .to_rgba8();
        lahza_annotations::redaction::apply_redactions(&mut output, &self.annotations);
        self.effect_revision += 1;
        let destination = std::env::temp_dir().join(format!(
            "lahza-redacted-{}-{}.png",
            std::process::id(),
            self.effect_revision
        ));
        output
            .save(&destination)
            .map_err(|error| error.to_string())?;
        self.set_capture_image(output);
        if let Some(previous) = self.processed_capture_path.replace(destination) {
            if previous
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("lahza-redacted-"))
            {
                let _ = fs::remove_file(previous);
            }
        }
        Ok(())
    }

    /// SVG fragment with every visible annotation, positioned relative to a
    /// capture drawn at (`x`, `y`) with the given pixel size. Shared by the
    /// static PNG export and the animated export's flattened frame.
    pub(super) fn annotations_svg(
        &self,
        x: f32,
        y: f32,
        capture_width: u32,
        capture_height: u32,
        stroke_scale: f32,
    ) -> String {
        annotations_svg(
            &self.annotations,
            x,
            y,
            capture_width,
            capture_height,
            stroke_scale,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires a display (can run under Xvfb)"]
    fn video_caption_size_matches_image_text_and_scales_on_export() {
        use crate::*;
        gpui::Application::new()
            .with_assets(Assets {
                base: asset_directory(),
            })
            .run(|cx| {
                let handle = open_studio_window(cx, true, |handle, cx| {
                    cx.new(|cx| Studio::new(handle, None, None, cx))
                })
                .unwrap();
                handle
                    .update(cx, |studio, _, _| {
                        studio.video_duration = 5.0;
                        for (width, height) in [(960., 320.), (1000., 600.), (360., 640.)] {
                            let bounds =
                                Bounds::new(point(px(0.), px(0.)), size(px(width), px(height)));
                            *studio.scene_canvas_bounds.lock().unwrap() = Some(bounds);
                            *studio.video_media_bounds.lock().unwrap() = Some(bounds);
                            studio.captured_dimensions = Some((width as u32, height as u32));
                            for canvas_text in [false, true] {
                                studio.finish_annotation_interaction();
                                studio.annotations.clear();
                                studio.animation_active = canvas_text;
                                studio.canvas_annotation_drag = canvas_text;
                                studio.tool = Tool::Text;
                                studio.text_font_size = 24.;
                                studio.pointer_down(point(px(30.), px(50.)), bounds, &[], 1);
                                studio.pointer_up(point(px(30.), px(50.)), bounds);
                                studio.canvas_annotation_drag = false;
                                studio.annotations[0].text = "Readable text".into();
                                studio.fit_text_box_to_content(0);
                                assert!(
                                    (studio.annotation_text_preview_size(&studio.annotations[0])
                                        - 24.)
                                        .abs()
                                        < 0.001
                                );
                                let expected = crate::annotation_text::layout(
                                    &AnnotationMark {
                                        tool: Tool::Text,
                                        text: "Readable text".into(),
                                        font_size: 24.,
                                        ..Default::default()
                                    },
                                    width,
                                );
                                let box_ = mark_screen_bounds(&studio.annotations[0], bounds);
                                assert!((f32::from(box_.size.width) - expected.width).abs() < 0.01);
                                assert!(
                                    (f32::from(box_.size.height) - expected.height).abs() < 0.01
                                );
                                studio.set_slider_value(6, 48);
                                assert!(
                                    (studio.annotation_text_preview_size(&studio.annotations[0])
                                        - 48.)
                                        .abs()
                                        < 0.001
                                );
                                if canvas_text {
                                    let fragment = annotations_svg(
                                        &studio.annotations,
                                        0.,
                                        0.,
                                        (width * 2.) as u32,
                                        (height * 2.) as u32,
                                        width.min(height) * 2. / 800.,
                                    );
                                    let size: f32 = fragment
                                        .split("font-size=\"")
                                        .nth(1)
                                        .unwrap()
                                        .split('"')
                                        .next()
                                        .unwrap()
                                        .parse()
                                        .unwrap();
                                    assert!(
                                        (size - 96.).abs() < 0.001,
                                        "2x export retains the same relative caption size"
                                    );
                                    let saved = studio.annotations[0].clone();
                                    *studio.scene_canvas_bounds.lock().unwrap() =
                                        Some(Bounds::new(bounds.origin, gpui::size(bounds.size.width * 0.5, bounds.size.height * 0.5)));
                                    studio.fit_text_box_to_content(0);
                                    assert_eq!(studio.annotations[0].font_size, saved.font_size);
                                    assert!(
                                        (studio.annotations[0].end.x - saved.end.x).abs() < 0.001
                                    );
                                    *studio.scene_canvas_bounds.lock().unwrap() = Some(bounds);
                                }
                            }
                        }
                    })
                    .unwrap();
                cx.spawn(async |cx| {
                    Timer::after(Duration::from_millis(200)).await;
                    cx.update(|cx| cx.quit()).unwrap();
                })
                .detach();
            });
    }

    #[test]
    #[ignore = "requires a display (can run under Xvfb)"]
    fn selected_ink_boxes_keep_the_group_and_drag_from_empty_space() {
        use crate::*;
        gpui::Application::new().with_assets(Assets { base: asset_directory() }).run(|cx| {
            let handle=open_studio_window(cx,true,|handle,cx|cx.new(|cx|Studio::new(handle,None,None,cx))).unwrap();
            handle.update(cx,|studio,_,cx| {
                let space=Bounds::new(point(px(0.),px(0.)),size(px(1000.),px(800.)));
                *studio.scene_canvas_bounds.lock().unwrap()=Some(space);
                *studio.video_media_bounds.lock().unwrap()=Some(space);
                studio.captured_dimensions=Some((1000,800));
                studio.animation_image_end=8.;studio.video_duration=8.;studio.video_position=2.;
                for timed in [false,true] {
                    studio.finish_annotation_interaction();studio.canvas_annotation_drag=false;
                    studio.animation_active=timed;studio.tool=Tool::Select;
                    studio.annotation_modifiers=Default::default();
                    studio.annotations=vec![
                        AnnotationMark { tool:Tool::Pen,
                            start:NormPoint{x:0.1,y:0.125},end:NormPoint{x:0.3,y:0.375},
                            points:vec![NormPoint{x:0.1,y:0.125},NormPoint{x:0.1,y:0.375},NormPoint{x:0.3,y:0.375}],
                            ..Default::default() },
                        AnnotationMark {tool:Tool::Rectangle,start:NormPoint{x:0.7,y:0.6},end:NormPoint{x:0.85,y:0.75},..Default::default()},
                    ];
                    let rendered=vec![crate::annotation_geometry::geometry(&studio.annotations[0],space).bounds(3.),mark_screen_bounds(&studio.annotations[1],space)];
                    let inside=point(px(230.),px(170.));
                    assert!(rendered[0].contains(&inside));
                    assert!(!hit_mark(&studio.annotations[0],inside,space,Some(rendered[0])));
                    let press=|studio:&mut Studio,p| {
                        if timed {assert!(studio.canvas_annotation_pointer_down(p,space,&[],space,&rendered,1));}
                        else {studio.pointer_down(p,space,&rendered,1);}
                    };
                    // The empty bounds of an unselected stroke do not capture a click.
                    press(studio,inside);
                    assert!(studio.annotation_brushing());
                    studio.pointer_up(inside,space);studio.canvas_annotation_drag=false;
                    assert!(studio.annotation_selected_indices().is_empty());
                    // Once selected, the same space belongs to the visible selection box.
                    let gap = point(px(500.), px(400.));
                    assert!(rendered.iter().all(|bounds| !bounds.contains(&gap)));
                    for (selection, press_at) in [(vec![0], inside), (vec![0,1], inside), (vec![0,1], gap)] {
                        studio.selected_annotation=selection.last().copied();
                        studio.annotation_selection=selection.clone();
                        let original=studio.annotations.clone();
                        studio.undo_stack.clear();studio.video_undo_stack.clear();
                        assert_eq!(studio.annotation_cursor(inside,space,&rendered),gpui::CursorStyle::OpenHand);
                        if selection.len() > 1 {
                            assert_eq!(studio.annotation_group_bounds(space,space,&rendered,&[]),Some(rendered[0].union(&rendered[1])));
                            assert_eq!(studio.annotation_cursor(rendered[1].origin,space,&rendered),gpui::CursorStyle::OpenHand);
                        } else {
                            assert!(studio.annotation_group_bounds(space,space,&rendered,&[]).is_none());
                        }
                        press(studio,press_at);
                        assert!(!studio.annotation_brushing());
                        studio.pointer_up(press_at,space);studio.canvas_annotation_drag=false;
                        assert_eq!(studio.annotation_selection,selection);
                        assert_eq!(studio.annotations,original);
                        assert!(studio.undo_stack.is_empty()&&studio.video_undo_stack.is_empty());
                        if selection.len() > 1 {
                            assert!(studio.canvas_annotation_pointer_down(gap,space,&[],space,&rendered,2));
                            studio.pointer_up(gap,space);studio.canvas_annotation_drag=false;
                            assert_eq!(studio.tool,Tool::Select);
                            assert_eq!(studio.annotation_selection,selection);
                            assert_eq!(studio.annotations,original);
                        }
                        // Dragging that empty area moves every selected item once.
                        // Exercise the shared canvas entry point in static mode too.
                        if selection.len() > 1 {
                            assert!(studio.canvas_annotation_pointer_down(press_at,space,&[],space,&rendered,1));
                        } else { press(studio,press_at); }
                        let end=point(press_at.x+px(25.),press_at.y+px(15.));
                        studio.pointer_move(end,space);studio.pointer_up(end,space);studio.canvas_annotation_drag=false;
                        for &i in &selection {
                            let delta=norm_to_screen(studio.annotations[i].start,space)-norm_to_screen(original[i].start,space);
                            assert!((f32::from(delta.x)-25.).abs()<0.001);
                            assert!((f32::from(delta.y)-15.).abs()<0.001);
                        }
                        if timed {assert_eq!(studio.video_undo_stack.len(),1);studio.undo_video_edit(cx);}
                        else {assert_eq!(studio.undo_stack.len(),1);assert!(studio.undo_annotations());}
                        assert_eq!(studio.annotations,original);
                    }
                    // Shift still removes just the clicked selected item.
                    studio.selected_annotation=Some(1);studio.annotation_selection=vec![0,1];
                    studio.annotation_modifiers.shift=true;
                    press(studio,inside);studio.pointer_up(inside,space);studio.canvas_annotation_drag=false;
                    assert_eq!(studio.annotation_selection,vec![1]);
                    studio.annotation_modifiers=Default::default();
                    let outside=point(px(950.),px(60.));
                    press(studio,outside);studio.pointer_up(outside,space);studio.canvas_annotation_drag=false;
                    assert!(studio.annotation_selected_indices().is_empty());
                    if timed {
                        studio.annotations[0].timing=Some(AnnotationTiming{start:4.,end:7.,..Default::default()});
                        studio.selected_annotation=Some(0);studio.annotation_selection=vec![0];
                        press(studio,inside);studio.pointer_up(inside,space);studio.canvas_annotation_drag=false;
                        assert!(studio.annotation_selected_indices().is_empty(),"hidden selection boxes cannot capture clicks");
                    }
                }
            }).unwrap();
            cx.spawn(async|cx|{Timer::after(Duration::from_millis(200)).await;cx.update(|cx|cx.quit()).unwrap();}).detach();
        });
    }

    #[test]
    #[ignore = "requires a display (can run under Xvfb)"]
    fn scene_selection_includes_visible_captions_and_moves_mixed_layers_together() {
        use crate::*;
        gpui::Application::new().with_assets(Assets { base: asset_directory() }).run(|cx| {
            let handle = open_studio_window(cx, true, |handle, cx| {
                cx.new(|cx| Studio::new(handle, None, None, cx))
            }).unwrap();
            handle.update(cx, |studio, _, _| {
                let canvas=Bounds::new(point(px(40.),px(30.)),size(px(1000.),px(800.)));
                let media=Bounds::new(point(px(140.),px(130.)),size(px(800.),px(600.)));
                *studio.scene_canvas_bounds.lock().unwrap()=Some(canvas);
                *studio.video_media_bounds.lock().unwrap()=Some(media);
                studio.captured_dimensions=Some((800,600));
                studio.animation_active=true;
                studio.animation_image_end=8.;
                studio.video_duration=8.;studio.video_position=2.;
                studio.tool=Tool::Select;
                studio.annotations=vec![
                    AnnotationMark { tool:Tool::Rectangle, start:NormPoint{x:0.2,y:0.2},end:NormPoint{x:0.4,y:0.5}, ..Default::default() },
                    AnnotationMark { tool:Tool::Text, canvas:true,text:"Canvas caption".into(),font_size:32.,start:NormPoint{x:0.55,y:0.7},end:NormPoint{x:0.8,y:0.8},..Default::default() },
                    AnnotationMark { tool:Tool::Text,text:"Moving text".into(),start:NormPoint{x:0.5,y:0.15},end:NormPoint{x:0.8,y:0.3},timing:Some(AnnotationTiming{start:1.9,end:6.,entrance:timed::EntranceEffect::SlideUp,transition:1.,..Default::default()}),..Default::default() },
                    AnnotationMark { tool:Tool::Text,canvas:true,text:"Hidden caption".into(),start:NormPoint{x:0.6,y:0.3},end:NormPoint{x:0.8,y:0.4},timing:Some(AnnotationTiming{start:4.,end:7.,..Default::default()}),..Default::default() },
                    AnnotationMark { tool:Tool::Ellipse,start:NormPoint{x:0.6,y:0.4},end:NormPoint{x:0.8,y:0.6},timing:Some(AnnotationTiming{start:4.,end:7.,..Default::default()}),..Default::default() },
                ];
                let hidden=Bounds::new(point(px(-100000.),px(-100000.)),size(px(0.),px(0.)));
                let mut rendered=vec![hidden;studio.annotations.len()];
                let mut canvas_hits=vec![];
                for (i,m) in studio.annotations.iter().enumerate() {
                    if let Some(mark)=timed::animated_mark(m,2.) {
                        let b=annotation_snap_bounds(&mark,if m.is_canvas(){canvas}else{media});
                        let b=Bounds::from_corners(b.min.screen(),b.max.screen());
                        if m.is_canvas(){canvas_hits.push((i,b));}else{rendered[i]=b;}
                    }
                }
                // Both canvas captions and text during its slide animation are clickable.
                for (index,b) in [(1,canvas_hits[0].1),(2,rendered[2])] {
                    studio.finish_annotation_interaction();studio.canvas_annotation_drag=false;
                    assert!(studio.canvas_annotation_pointer_down(b.center(),canvas,&canvas_hits,media,&rendered,1));
                    assert_eq!(studio.selected_annotation,Some(index));
                    studio.pointer_up(b.center(),if index==1{canvas}else{media});
                }
                studio.finish_annotation_interaction();studio.canvas_annotation_drag=false;
                // Start in canvas padding, then enclose both paint layers.
                let from=point(px(50.),px(40.));let to=point(px(1030.),px(820.));
                assert!(studio.canvas_annotation_pointer_down(from,canvas,&canvas_hits,media,&rendered,1));
                studio.pointer_move(to,canvas);
                assert_eq!(studio.annotation_selection,vec![0,1,2]);
                studio.pointer_up(to,canvas);studio.canvas_annotation_drag=false;
                // Shift click can toggle canvas text out of the mixed selection.
                studio.annotation_modifiers.shift=true;
                let p=canvas_hits[0].1.center();
                assert!(studio.canvas_annotation_pointer_down(p,canvas,&canvas_hits,media,&rendered,1));
                studio.pointer_up(p,canvas);studio.canvas_annotation_drag=false;
                assert_eq!(studio.annotation_selection,vec![0,2]);
                studio.annotation_modifiers=Default::default();
                studio.annotation_selection=vec![0,1,2];studio.selected_annotation=Some(1);
                let before=studio.annotations.clone();
                // The gap between media marks and the caption is part of one box.
                let p=point(px(550.),px(510.));
                assert!(rendered.iter().chain(canvas_hits.iter().map(|(_,b)|b)).all(|b|!b.contains(&p)));
                assert!(studio.annotation_group_bounds(canvas,media,&rendered,&canvas_hits).unwrap().contains(&p));
                assert!(studio.canvas_annotation_pointer_down(p,canvas,&canvas_hits,media,&rendered,1));
                let moved=point(p.x+px(40.),p.y+px(20.));
                studio.pointer_move(moved,canvas);studio.pointer_up(moved,canvas);studio.canvas_annotation_drag=false;
                for i in 0..3 {
                    let space=if before[i].is_canvas(){canvas}else{media};
                    let delta=norm_to_screen(studio.annotations[i].start,space)-norm_to_screen(before[i].start,space);
                    assert!((f32::from(delta.x)-40.).abs()<0.001,"mark {i}");
                    assert!((f32::from(delta.y)-20.).abs()<0.001,"mark {i}");
                }
                assert_eq!(studio.annotations[3..],before[3..]);
                assert!(studio.delete_selected_annotations());
                assert_eq!(studio.annotations.len(),2);
                assert_eq!(studio.annotations[0].text,"Hidden caption");
            }).unwrap();
            cx.spawn(async |cx| {Timer::after(Duration::from_millis(200)).await;cx.update(|cx|cx.quit()).unwrap();}).detach();
        });
    }

    #[test]
    #[ignore = "requires a display (can run under Xvfb)"]
    fn pencil_retains_slow_mouse_samples_and_settles_on_release() {
        use crate::*;
        gpui::Application::new().with_assets(Assets { base: asset_directory() }).run(|cx| {
            let handle = open_studio_window(cx, true, |handle, cx| {
                cx.new(|cx| Studio::new(handle, None, None, cx))
            }).unwrap();
            handle.update(cx, |studio, _, _| {
                let bounds = Bounds::new(point(px(0.), px(0.)), size(px(1000.), px(800.)));
                *studio.scene_canvas_bounds.lock().unwrap() = Some(bounds);
                *studio.video_media_bounds.lock().unwrap() = Some(bounds);
                studio.captured_dimensions = Some((1000, 800));
                studio.video_duration = 5.;
                for timed in [false, true] {
                    studio.animation_active = timed;
                    studio.annotations.clear();
                    studio.finish_annotation_interaction();
                    studio.tool = Tool::Pen;
                    studio.pointer_down(point(px(100.), px(100.)), bounds, &[], 1);
                    for i in 1..=40 {
                        studio.pointer_move(point(px(100. + i as f32 * 0.25), px(100.)), bounds);
                    }
                    let draft = studio.annotation_draft.as_ref().unwrap();
                    assert!(draft.ink_in_progress);
                    assert_eq!(draft.points.len(), 41, "slow samples must not be discarded");
                    for i in 1..=10 {
                        studio.pointer_move(point(px(110. + i as f32 * 20.), px(100.)), bounds);
                    }
                    studio.pointer_up(point(px(310.), px(100.)), bounds);
                    assert!(studio.annotation_draft.is_none());
                    assert_eq!(studio.annotations.len(), 1);
                    let mark = &studio.annotations[0];
                    assert!(!mark.ink_in_progress);
                    assert_eq!(mark.points.len(), 51);
                    let g = crate::annotation_geometry::geometry(mark, bounds);
                    assert!(g.centerline.last().unwrap().sub(crate::annotation_geometry::V::new(310., 100.)).len() < 0.001);
                }
            }).unwrap();
            cx.spawn(async |cx| {
                Timer::after(Duration::from_millis(200)).await;
                cx.update(|cx| cx.quit()).unwrap();
            }).detach();
        });
    }

    #[test]
    #[ignore = "requires a display (can run under Xvfb)"]
    fn highlight_moves_and_resizes_freely_until_ctrl_is_held() {
        use crate::*;
        gpui::Application::new()
            .with_assets(Assets {
                base: asset_directory(),
            })
            .run(|cx| {
                let handle = open_studio_window(cx, true, |handle, cx| {
                    cx.new(|cx| Studio::new(handle, None, None, cx))
                })
                .unwrap();
                handle
                    .update(cx, |studio, _, _| {
                        let bounds = Bounds::new(point(px(0.), px(0.)), size(px(1000.), px(800.)));
                        *studio.scene_canvas_bounds.lock().unwrap() = Some(bounds);
                        *studio.video_media_bounds.lock().unwrap() = Some(bounds);
                        studio.captured_dimensions = Some((1000, 800));
                        studio.video_duration = 5.0;
                        for timed in [false, true] {
                            for resize in [false, true] {
                                studio.animation_active = timed;
                                studio.annotations = vec![AnnotationMark {
                                    tool: Tool::Highlight,
                                    start: NormPoint { x: 0.2, y: 0.2 },
                                    end: NormPoint { x: 0.4, y: 0.4 },
                                    ..Default::default()
                                }];
                                studio.undo_stack.clear();
                                studio.video_undo_stack.clear();
                                studio.tool = Tool::Select;
                                studio.selected_annotation = Some(0);
                                studio.annotation_selection = vec![0];
                                studio.annotation_modifiers = Default::default();
                                let origin = if resize {
                                    point(px(400.), px(320.))
                                } else {
                                    point(px(200.), px(240.))
                                };
                                let target = if resize {
                                    point(px(497.), px(397.))
                                } else {
                                    point(px(396.), px(397.))
                                };
                                studio.pointer_down(origin, bounds, &[], 1);
                                studio.pointer_move(target, bounds);
                                let free = studio.annotations[0].clone();
                                let rect = annotation_snap_bounds(&free, bounds);
                                let anchor = if resize {
                                    rect.max
                                } else {
                                    rect.min.lerp(rect.max, 0.5)
                                };
                                assert!((anchor.x - if resize { 497. } else { 496. }).abs() < 0.01);
                                assert!((anchor.y - 397.).abs() < 0.01);
                                assert!(studio
                                    .annotation_gesture
                                    .as_ref()
                                    .unwrap()
                                    .snapping
                                    .is_none());

                                // Modifier changes must work immediately, without another pointer event.
                                studio.update_annotation_modifiers(gpui::Modifiers {
                                    control: true,
                                    ..Default::default()
                                });
                                let rect = annotation_snap_bounds(&studio.annotations[0], bounds);
                                let snapped = if resize {
                                    rect.max
                                } else {
                                    rect.min.lerp(rect.max, 0.5)
                                };
                                assert!(
                                    (snapped.x - 500.).abs() < 0.01
                                        && (snapped.y - 400.).abs() < 0.01
                                );
                                assert!(!studio
                                    .annotation_gesture
                                    .as_ref()
                                    .unwrap()
                                    .guides
                                    .is_empty());
                                studio.update_annotation_modifiers(Default::default());
                                assert_eq!(studio.annotations[0], free);
                                assert!(studio
                                    .annotation_gesture
                                    .as_ref()
                                    .unwrap()
                                    .guides
                                    .is_empty());
                                studio.pointer_up(target, bounds);
                                assert_eq!(studio.annotations[0], free);
                                assert_eq!(
                                    if timed {
                                        studio.video_undo_stack.len()
                                    } else {
                                        studio.undo_stack.len()
                                    },
                                    1
                                );
                            }
                        }
                    })
                    .unwrap();
                cx.spawn(async |cx| {
                    Timer::after(Duration::from_millis(200)).await;
                    cx.update(|cx| cx.quit()).unwrap();
                })
                .detach();
            });
    }

    #[test]
    #[ignore = "requires a display (can run under Xvfb)"]
    fn text_drag_snaps_to_real_canvas_and_image_bounds_and_undoes_once() {
        use crate::*;
        gpui::Application::new()
            .with_assets(Assets {
                base: asset_directory(),
            })
            .run(|cx| {
                let handle = open_studio_window(cx, true, |handle, cx| {
                    cx.new(|cx| Studio::new(handle, None, None, cx))
                })
                .unwrap();
                handle
                    .update(cx, |studio, _, _| {
                        let canvas = Bounds::new(point(px(0.), px(0.)), size(px(1000.), px(800.)));
                        let image = Bounds::new(point(px(100.), px(80.)), size(px(800.), px(500.)));
                        *studio.scene_canvas_bounds.lock().unwrap() = Some(canvas);
                        *studio.video_media_bounds.lock().unwrap() = Some(image);
                        studio.captured_dimensions = Some((800, 500));
                        for on_canvas in [false, true] {
                            let space = if on_canvas { canvas } else { image };
                            studio.annotations = vec![AnnotationMark {
                                tool: Tool::Text,
                                text: "Align this text".into(),
                                canvas: on_canvas,
                                start: NormPoint { x: 0.1, y: 0.2 },
                                end: NormPoint { x: 0.3, y: 0.3 },
                                ..Default::default()
                            }];
                            studio.annotation_view_bounds = Some(space);
                            studio.fit_text_box_to_content(0);
                            studio.undo_stack.clear();
                            studio.redo_stack.clear();
                            let before = studio.annotations.clone();
                            studio.tool = Tool::Select;
                            studio.selected_annotation = Some(0);
                            studio.annotation_selection = vec![0];
                            studio.canvas_annotation_drag = on_canvas;
                            studio.annotation_modifiers = gpui::Modifiers { control: true, ..Default::default() };
                            let bounds = annotation_snap_bounds(&studio.annotations[0], space);
                            let center = bounds.min.lerp(bounds.max, 0.5);
                            studio.pointer_down(center.screen(), space, &[], 1);
                            studio.pointer_move(point(px(497.), px(397.)), space);
                            let moved = annotation_snap_bounds(&studio.annotations[0], space);
                            assert!(
                                moved
                                    .min
                                    .lerp(moved.max, 0.5)
                                    .sub(crate::annotation_geometry::V::new(500., 400.))
                                    .len()
                                    < 0.05
                            );
                            assert!(studio
                                .annotation_gesture
                                .as_ref()
                                .unwrap()
                                .guides
                                .iter()
                                .any(|g| g.label.as_deref() == Some("Canvas · center")));
                            studio.pointer_move(point(px(508.), px(408.)), space);
                            let held = annotation_snap_bounds(&studio.annotations[0], space);
                            assert!(
                                held.min
                                    .lerp(held.max, 0.5)
                                    .sub(crate::annotation_geometry::V::new(500., 400.))
                                    .len()
                                    < 0.05
                            );
                            studio.update_annotation_modifiers(Default::default());
                            assert!(studio
                                .annotation_gesture
                                .as_ref()
                                .unwrap()
                                .guides
                                .is_empty());
                            let free = annotation_snap_bounds(&studio.annotations[0], space);
                            assert!(
                                free.min
                                    .lerp(free.max, 0.5)
                                    .sub(crate::annotation_geometry::V::new(508., 408.))
                                    .len()
                                    < 0.05
                            );
                            studio.update_annotation_modifiers(gpui::Modifiers { control: true, ..Default::default() });
                            studio.pointer_move(point(px(498.), px(328.)), space);
                            let aligned = annotation_snap_bounds(&studio.annotations[0], space);
                            assert!(
                                aligned
                                    .min
                                    .lerp(aligned.max, 0.5)
                                    .sub(crate::annotation_geometry::V::new(500., 330.))
                                    .len()
                                    < 0.05
                            );
                            assert!(studio
                                .annotation_gesture
                                .as_ref()
                                .unwrap()
                                .guides
                                .iter()
                                .any(|g| g.label.as_deref() == Some("Image · center")));
                            studio.pointer_up(point(px(498.), px(328.)), space);
                            assert!(studio.annotation_gesture.is_none());
                            assert_eq!(studio.undo_stack.len(), 1);
                            assert!(studio.undo_annotations());
                            assert_eq!(studio.annotations, before);
                        }
                    })
                    .unwrap();
                cx.spawn(async |cx| {
                    Timer::after(Duration::from_millis(200)).await;
                    cx.update(|cx| cx.quit()).unwrap();
                })
                .detach();
            });
    }
}
