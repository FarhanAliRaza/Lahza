//! Bind reusable text fields to scene values; the fields own keyboard focus.
use super::*;
use crate::text_field::{EventKind, FieldEvent, Target, TextField};

pub(crate) struct TextFields {
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
            &self.text_fields.annotation,
            &self.text_fields.watermark,
            &self.text_fields.start,
            &self.text_fields.end,
        ]
        .iter()
        .any(|field| field.read(cx).focus.is_focused(window))
    }
    pub(crate) fn sync_text_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let edit_request = self.editing_text.take();
        if edit_request.is_some() {
            self.inspector_visible = true;
        }
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
        if edit_request.is_some_and(|i| target == Target::Annotation(i)) {
            self.text_fields.annotation.read(cx).focus.focus(window);
        }
        if std::mem::take(&mut self.watermark_editing) && watermark_visible {
            self.text_fields.watermark.read(cx).focus.focus(window);
        }
    }
    fn on_text_field_event(&mut self, event: &FieldEvent, cx: &mut Context<Self>) {
        match event.target {
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
