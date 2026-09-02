//! The template gallery: one-click starting points that set the scene look,
//! the camera motion, and timed captions for a screenshot or a recording.

use gpui::{
    div, hsla, linear_color_stop, linear_gradient, prelude::*, px, rgb, AnyElement, Context,
    FontWeight, Hsla,
};

use crate::{
    ink, line, muted,
    recording::{
        scene::SceneBackground,
        templates::{self, SceneTemplate},
    },
    Studio, VideoEditSnapshot,
};

impl Studio {
    /// Applies a template to the open scene. A screenshot becomes an animated
    /// scene of the template's length; a recording keeps its own length and
    /// gets the template's motion and captions as an intro.
    pub(crate) fn apply_template(&mut self, template: &SceneTemplate, cx: &mut Context<Self>) {
        let is_video = self.video_project.is_some();
        if !is_video && self.captured_path.is_none() {
            self.toast = Some("Capture an image first".into());
            cx.notify();
            return;
        }
        self.stop_editing_text();
        self.pause_video_playback();
        self.apply_scene_style(&template.style);
        self.aspect_ratio = template.aspect_index;
        self.background_preset = None;
        self.inspector_visible = true;
        if !is_video {
            if !self.animation_active {
                self.toggle_animation(cx);
            }
            if !self.animation_active {
                return;
            }
            self.set_animation_duration(template.duration);
        }
        let scene_duration = self.video_duration;
        let intro = template.duration.min(scene_duration);

        // Motion: replace whatever the template's window covers, keep a
        // recording's later regions (cursor zooms and hand-placed motion).
        let mut cues = template.cues(scene_duration);
        if is_video {
            cues.extend(
                self.video_zoom_cues
                    .iter()
                    .filter(|cue| cue.start >= intro - 1e-6)
                    .cloned(),
            );
        }
        if cues != self.video_zoom_cues {
            self.video_undo_stack
                .push(VideoEditSnapshot::Zoom(self.video_zoom_cues.clone()));
            self.video_redo_stack.clear();
            self.video_zoom_cues = cues;
        }
        self.animation_preset = template.motion.preset();

        // Captions: the previous template's marks make way, hand-drawn ones stay.
        let marks = template.marks(scene_duration);
        let had_template_marks = self.annotations.iter().any(|mark| mark.from_template);
        if had_template_marks || !marks.is_empty() {
            self.record_annotation_undo();
            self.annotations.retain(|mark| !mark.from_template);
            self.annotations.extend(marks);
        }
        self.selected_annotation = None;
        self.annotation_draft = None;
        self.video_selected_zoom_cue = None;
        self.video_position = 0.0;
        self.rebuild_video_motion_timelines();
        self.toast = Some(
            format!(
                "{} applied — select a caption to edit its text or timing",
                template.name
            )
            .into(),
        );
        cx.notify();
    }

    fn template_card(&self, template: &SceneTemplate, cx: &mut Context<Self>) -> AnyElement {
        let id = template.id;
        let swatch = template.swatch();
        let name = template.name;
        let tagline = template.tagline;
        let preview = div()
            .w_full()
            .h(px(44.0))
            .rounded_md()
            .overflow_hidden()
            .relative()
            .bg(match &template.style.background {
                SceneBackground::Solid(color) => gpui::Background::from(rgb(*color)),
                _ => linear_gradient(
                    135.0,
                    linear_color_stop(rgb(swatch[0]), 0.0),
                    linear_color_stop(rgb(swatch[2]), 1.0),
                ),
            })
            .child(
                // A miniature of the card the template frames.
                div()
                    .absolute()
                    .left(px(10.0 + template.style.padding as f32 * 0.16))
                    .right(px(10.0 + template.style.padding as f32 * 0.16))
                    .top(px(8.0 + template.style.padding as f32 * 0.12))
                    .bottom(px(4.0))
                    .rounded(px(3.0 + template.style.corners as f32 * 0.08))
                    .bg(hsla(0.0, 0.0, 1.0, 0.82))
                    .when(template.style.border, |this| {
                        this.border_1()
                            .border_color(Hsla::from(rgb(template.style.border_color)))
                    }),
            )
            .when(!template.marks.is_empty(), |this| {
                this.child(
                    div()
                        .absolute()
                        .right(px(4.0))
                        .top(px(4.0))
                        .px(px(4.0))
                        .rounded_sm()
                        .bg(hsla(0.0, 0.0, 0.0, 0.55))
                        .text_color(rgb(0xffffff))
                        .text_size(px(9.0))
                        .child("Aa"),
                )
            });
        div()
            .id(gpui::SharedString::from(format!("template-card-{id}")))
            .flex()
            .flex_col()
            .gap_1()
            .p_1()
            .rounded_lg()
            .border_1()
            .border_color(line())
            .bg(rgb(0xffffff))
            .cursor_pointer()
            .hover(|style| style.bg(rgb(0xf3f4f6)))
            .child(preview)
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(ink())
                    .child(name),
            )
            .child(div().text_size(px(10.0)).text_color(muted()).child(tagline))
            .on_click(cx.listener(move |this, _, _, cx| {
                if let Some(template) = templates::find(id) {
                    this.apply_template(&template, cx);
                }
            }))
            .into_any_element()
    }

    /// Gallery of built-in templates, two per row.
    pub(crate) fn template_gallery_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let templates = templates::all();
        let mut rows: Vec<AnyElement> = Vec::new();
        for pair in templates.chunks(2) {
            let mut row = div().flex().gap_2();
            for template in pair {
                row = row.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .child(self.template_card(template, cx)),
                );
            }
            if pair.len() == 1 {
                row = row.child(div().flex_1());
            }
            rows.push(row.into_any_element());
        }
        let hint = if self.video_project.is_some() {
            "Sets the look, adds intro motion and captions; your later motion regions stay."
        } else {
            "Turns the screenshot into a short animated scene with captions you can edit."
        };
        div()
            .flex()
            .flex_col()
            .gap_2()
            .children(rows)
            .child(div().text_xs().text_color(muted()).child(hint))
            .into_any_element()
    }
}
