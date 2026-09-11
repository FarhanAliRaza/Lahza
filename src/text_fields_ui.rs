//! Bind reusable text fields to scene values; the fields own keyboard focus.
use super::*;
use crate::text_field::{EventKind, FieldEvent, Target, TextField};

pub(crate) struct TextFields {
    pub canvas: gpui::Entity<TextField>,
    pub annotation: gpui::Entity<TextField>,
    pub watermark: gpui::Entity<TextField>,
    pub start: gpui::Entity<TextField>,
    pub end: gpui::Entity<TextField>,
}
impl TextFields {
    pub fn new(parent: FocusHandle, cx: &mut Context<Studio>) -> Self {
        let mut create = |placeholder: &str| {
            let field = cx.new(|cx| TextField::new(parent.clone(), placeholder, cx));
            cx.subscribe(&field, |this, _, event, cx| {
                this.on_text_field_event(event, cx)
            })
            .detach();
            field
        };
        Self {
            canvas: create(""),
            annotation: create("Type text…"),
            watermark: create("Type a watermark…"),
            start: create("Seconds"),
            end: create("Seconds"),
        }
    }
}
impl Studio {
    pub(crate) fn native_text_focused(&self, window: &Window, cx: &App) -> bool {
        [
            &self.text_fields.canvas,
            &self.text_fields.annotation,
            &self.text_fields.watermark,
            &self.text_fields.start,
            &self.text_fields.end,
        ]
        .iter()
        .any(|field| field.read(cx).focus.is_focused(window))
    }
    pub(crate) fn sync_text_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // GPUI text input is axis aligned. Projected media keeps the inspector editor;
        // canvas captions can still edit in place above a projected scene.
        let projected_edit = self.editing_text.filter(|i| {
            !self.annotations_paint_flat()
                && self.annotations.get(*i).is_some_and(|m| !m.is_canvas())
        });
        if projected_edit.is_some() {
            self.inspector_visible = true;
            self.editing_text = None;
        }
        if let Some(i) = self.selected_annotation {
            self.fit_text_box_to_content(i);
        }
        if self.editing_text != self.selected_annotation || self.crop_active {
            self.stop_editing_text();
        }
        let canvas_target = self.editing_text.and_then(|i| {
            self.annotations
                .get(i)
                .filter(|m| m.tool == Tool::Text)
                .map(|m| (i, m.text.clone()))
        });
        let target = canvas_target
            .as_ref()
            .map(|(i, _)| Target::CanvasAnnotation(*i))
            .unwrap_or(Target::None);
        let changed = self.text_fields.canvas.read(cx).target != target;
        self.text_fields.canvas.update(cx, |field, cx| {
            field.sync(
                target,
                canvas_target
                    .as_ref()
                    .map(|(_, s)| s.as_str())
                    .unwrap_or(""),
                window,
                cx,
            );
            if changed && target != Target::None {
                field.begin_canvas(self.canvas_caret_point.take(), window, cx);
            }
        });
        let annotation_visible = self.inspector_visible
            && self.effective_tab() == InspectorTab::Annotate
            && !self.crop_active;
        let selected = self.selected_annotation;
        let mark = selected.and_then(|i| self.annotations.get(i));
        let annotation = selected
            .zip(mark)
            .filter(|(_, mark)| annotation_visible && mark.tool == Tool::Text);
        let (target, text) = annotation
            .map(|(i, m)| (Target::Annotation(i), m.text.clone()))
            .unwrap_or((Target::None, String::new()));
        self.text_fields
            .annotation
            .update(cx, |field, cx| field.sync(target, &text, window, cx));
        let timing = selected
            .zip(mark)
            .filter(|(_, _)| annotation_visible && self.scene_is_timed())
            .and_then(|(i, m)| m.timing.map(|t| (i, t)));
        for (field, leading) in [
            (&self.text_fields.start, true),
            (&self.text_fields.end, false),
        ] {
            let (target, value) = timing
                .map(|(i, t)| {
                    (
                        Target::Time(i, leading),
                        format!("{:.2}", if leading { t.start } else { t.end }),
                    )
                })
                .unwrap_or((Target::None, String::new()));
            field.update(cx, |field, cx| field.sync(target, &value, window, cx));
        }
        let watermark_visible = self.inspector_visible
            && self.watermark_enabled
            && self.effective_tab() == InspectorTab::Design
            && self.section_open("watermark");
        self.text_fields.watermark.update(cx, |field, cx| {
            field.sync(
                if watermark_visible {
                    Target::Watermark
                } else {
                    Target::None
                },
                &self.watermark.text,
                window,
                cx,
            )
        });
        if projected_edit.is_some_and(|i| target == Target::Annotation(i)) {
            self.text_fields.annotation.read(cx).focus.focus(window);
        }
        if std::mem::take(&mut self.watermark_editing) && watermark_visible {
            self.text_fields.watermark.read(cx).focus.focus(window);
        }
    }
    fn on_text_field_event(&mut self, event: &FieldEvent, cx: &mut Context<Self>) {
        match event.target {
            Target::CanvasAnnotation(index)
                if self.editing_text == Some(index)
                    && self
                        .annotations
                        .get(index)
                        .is_some_and(|m| m.tool == Tool::Text) =>
            {
                match event.kind {
                    EventKind::Focus => {
                        self.pause_video_playback();
                        if !self.annotations[index].text.is_empty() {
                            self.record_annotation_undo();
                        }
                    }
                    EventKind::Change => {
                        self.annotations[index].text = event.text.clone();
                        self.fit_text_box_to_content(index);
                    }
                    EventKind::Commit => {
                        self.stop_editing_text();
                    }
                    EventKind::Cancel => {}
                }
            }
            Target::Annotation(index)
                if self.selected_annotation == Some(index)
                    && self
                        .annotations
                        .get(index)
                        .is_some_and(|m| m.tool == Tool::Text) =>
            {
                match event.kind {
                    EventKind::Focus => {
                        self.pause_video_playback();
                        self.record_annotation_undo();
                    }
                    EventKind::Change | EventKind::Cancel => {
                        self.annotations[index].text = event.text.clone();
                        self.fit_text_box_to_content(index);
                    }
                    EventKind::Commit => {
                        if self.annotations[index].text.trim().is_empty() {
                            self.annotations.remove(index);
                            self.selected_annotation = None;
                        }
                    }
                }
            }
            Target::Watermark => {
                if matches!(event.kind, EventKind::Change | EventKind::Cancel) {
                    self.watermark.text = event.text.clone();
                }
            }
            Target::Time(index, start) => {
                if event.kind == EventKind::Focus {
                    self.stop_editing_text();
                }
                if event.kind == EventKind::Commit {
                    self.annotation_time_edit = Some((index, start, event.text.clone()));
                    self.commit_annotation_time();
                }
            }
            _ => {}
        }
        cx.notify();
    }
}

impl Studio {
    pub(crate) fn canvas_text_mouse_down(
        &mut self,
        event: &gpui::MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        self.annotation_modifiers = event.modifiers;
        if self.editing_text.is_some()
            && self.text_fields.canvas.read(cx).canvas_hit(event.position)
        {
            self.text_fields
                .canvas
                .update(cx, |input, cx| input.mouse_down(event, window, cx));
            return true;
        }
        false
    }
    pub(crate) fn canvas_text_mouse_move(
        &mut self,
        event: &gpui::MouseMoveEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        self.annotation_modifiers = event.modifiers;
        self.editing_text.is_some()
            && self
                .text_fields
                .canvas
                .update(cx, |input, cx| input.canvas_drag(event, cx))
    }
}
pub(crate) fn paint_canvas_text(
    entity: &gpui::Entity<Studio>,
    canvas: Bounds<Pixels>,
    media: Bounds<Pixels>,
    frame: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    let studio = entity.read(cx);
    let Some(index) = studio.editing_text else {
        return;
    };
    let Some(mut mark) = studio.annotations.get(index).cloned() else {
        return;
    };
    let field = studio.text_fields.canvas.clone();
    let image = if mark.is_canvas() {
        let scale = lahza_annotations::canvas::canvas_scale(canvas);
        mark.font_size *= scale;
        canvas
    } else if mark.pinned {
        frame
    } else {
        media
    };
    let origin = crate::annotations::norm_to_screen(mark.start, image);
    let end = crate::annotations::norm_to_screen(mark.end, image);
    let bounds = Bounds::new(
        origin,
        size((end.x - origin.x).abs(), (end.y - origin.y).abs()),
    );
    let clip = if mark.is_canvas() { canvas } else { frame };
    window.with_content_mask(Some(gpui::ContentMask { bounds: clip }), |window| {
        TextField::paint_canvas(&field, &mark, bounds, window, cx)
    });
}
