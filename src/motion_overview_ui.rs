//! Zoom creation and overview, shared with the selected-region inspector.
use crate::recording::viewport::{AutoZoomStyle, ZoomPresentation};
use crate::scene_ui::SceneSlider;
use crate::{blue, ink, muted, Studio};
use gpui::{div, prelude::*, px, rgb, svg, AnyElement, Context, FontWeight};

/// Small original illustrations explain whether the whole frame or a local
/// lens moves, without adding a second set of interaction controls.
fn zoom_illustration(presentation: ZoomPresentation) -> AnyElement {
    let glass = presentation == ZoomPresentation::GlassLoupe;
    div()
        .h(px(48.0))
        .w_full()
        .relative()
        .overflow_hidden()
        .rounded_md()
        .bg(rgb(0xe8edf6))
        .child(
            div()
                .absolute()
                .left(px(if glass { 15.0 } else { 4.0 }))
                .top(px(if glass { 8.0 } else { 2.0 }))
                .w(px(if glass { 91.0 } else { 112.0 }))
                .h(px(if glass { 35.0 } else { 47.0 }))
                .rounded_sm()
                .bg(rgb(0xffffff))
                .border_1()
                .border_color(rgb(0xc6d2e5))
                .child(div().h(px(7.0)).bg(rgb(0xd3deef)))
                .child(
                    div()
                        .mt(px(5.0))
                        .ml(px(7.0))
                        .w(px(if glass { 43.0 } else { 67.0 }))
                        .h(px(4.0))
                        .rounded_sm()
                        .bg(rgb(0xaabbd4)),
                )
                .child(
                    div()
                        .mt(px(4.0))
                        .ml(px(7.0))
                        .w(px(if glass { 57.0 } else { 87.0 }))
                        .h(px(4.0))
                        .rounded_sm()
                        .bg(rgb(0xd4dfed)),
                ),
        )
        .when(glass, |this| {
            this.child(
                div()
                    .absolute()
                    .left(px(61.0))
                    .top(px(7.0))
                    .size(px(35.0))
                    .rounded_full()
                    .bg(rgb(0xf5f9ff))
                    .border_2()
                    .border_color(rgb(0x5693df))
                    .shadow_sm()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(div().w(px(18.0)).h(px(7.0)).rounded_sm().bg(rgb(0x7ca8e3))),
            )
        })
        .child(
            svg()
                .path("icons/mouse-pointer.svg")
                .absolute()
                .left(px(if glass { 76.0 } else { 81.0 }))
                .top(px(if glass { 24.0 } else { 28.0 }))
                .size(px(15.0))
                .text_color(rgb(0x253752)),
        )
        .into_any_element()
}

impl Studio {
    /// In the overview cards add a region; in a selected region they edit
    /// its presentation while retaining zoom, focus, timing, and transforms.
    pub(crate) fn motion_style_choices(
        &self,
        selected: Option<ZoomPresentation>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let busy = self.video_edit_busy;
        div()
            .flex()
            .gap_2()
            .children(
                [
                    (
                        ZoomPresentation::Classic,
                        "Classic zoom",
                        "motion-add-at-playhead",
                    ),
                    (
                        ZoomPresentation::GlassLoupe,
                        "Glass loupe",
                        "motion-add-glass-at-playhead",
                    ),
                ]
                .into_iter()
                .enumerate()
                .map(|(index, (presentation, title, create_id))| {
                    let active = selected == Some(presentation);
                    let card = if selected.is_some() {
                        div().id(("motion-presentation", index))
                    } else {
                        div().id(create_id)
                    };
                    card.flex_1()
                        .min_w_0()
                        .h(px(84.0))
                        .p(px(5.0))
                        .rounded_lg()
                        .border_2()
                        .border_color(if active { blue() } else { rgb(0xe0e4eb).into() })
                        .bg(if active { rgb(0xf0f6ff) } else { rgb(0xffffff) })
                        .flex()
                        .flex_col()
                        .gap_1()
                        .opacity(if busy { 0.45 } else { 1.0 })
                        .child(zoom_illustration(presentation))
                        .child(
                            div()
                                .flex_1()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(if active { blue() } else { ink() })
                                .child(title),
                        )
                        .when(!busy, |this| {
                            this.cursor_pointer()
                                .hover(|style| style.border_color(rgb(0x6c9ddd)))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    if selected.is_some() {
                                        this.edit_selected_region(|cue| {
                                            cue.presentation = presentation
                                        });
                                    } else {
                                        this.add_motion_region_with_presentation_at(
                                            this.video_position,
                                            presentation,
                                            cx,
                                        );
                                    }
                                    cx.notify();
                                }))
                        })
                }),
            )
            .into_any_element()
    }

    pub(crate) fn motion_overview_section(&self, cx: &mut Context<Self>) -> AnyElement {
        let busy = self.video_edit_busy;
        let has_clicks = self.video_press_times.iter().any(|time| {
            time.is_finite()
                && *time >= 0.0
                && *time <= self.video_source_duration
                && !self
                    .video_removed_presses
                    .iter()
                    .any(|removed| (removed - time).abs() < 1e-6)
        });
        let mut regions: Vec<_> = self
            .video_zoom_cues
            .iter()
            .enumerate()
            .map(|(index, cue)| {
                let slices = self
                    .video_clip_timeline
                    .slices_overlapping(cue.start, cue.end);
                let range = slices.first().zip(slices.last()).map(|(first, last)| {
                    (
                        first.editor_start,
                        last.editor_end,
                        (first.editor_start + first.editor_end) * 0.5,
                    )
                });
                (index, cue, range)
            })
            .collect();
        regions.sort_by(|left, right| {
            left.2
                .map_or(f64::INFINITY, |range| range.0)
                .total_cmp(&right.2.map_or(f64::INFINITY, |range| range.0))
        });
        div().flex().flex_col().gap_3()
            .child(div().flex().items_center().justify_between()
                .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child("Add zoom"))
                .child(div().text_xs().text_color(muted()).child(format!("At {:.1}s", self.timeline_display_time()))))
            .child(self.motion_style_choices(None, cx))
            .child(div().text_xs().text_color(muted()).child("Enlarge the frame, or magnify one area."))
            .when(regions.is_empty(), |this| this.child(div().py_3().text_xs().text_color(muted())
                .child("Add a zoom at the playhead. Drag its edges on the timeline to set its duration.")))
            .when(!regions.is_empty(), |this| this
                .child(div().flex().items_center().justify_between()
                    .child(div().text_xs().font_weight(FontWeight::SEMIBOLD).child("Your zooms"))
                    .child(div().text_xs().text_color(muted()).child(regions.len().to_string())))
                .child(div().id("motion-region-list").flex().flex_col().gap_1().max_h(px(200.0)).overflow_y_scroll()
                    .children(regions.into_iter().map(|(index, cue, range)| {
                    let title = if cue.transform.is_some() && !cue.has_camera_motion() { "Transform" }
                        else if cue.presentation == ZoomPresentation::GlassLoupe { "Glass loupe" } else { "Classic zoom" };
                    let detail = range.map(|(start, end, _)| format!("{:.1}× · {start:.1}s – {end:.1}s", cue.zoom))
                        .unwrap_or_else(|| format!("{:.1}× · Outside current edit", cue.zoom));
                    let cue_id = cue.id;
                    div().id(("motion-region-select", index)).flex_none().px_3().py_2().rounded_md().bg(rgb(0xf5f6f8))
                        .flex().items_center().justify_between().gap_2()
                        .opacity(if cue.is_enabled { 1.0 } else { 0.6 })
                        .child(div().flex().flex_col().gap_1().min_w_0()
                            .child(div().text_xs().font_weight(FontWeight::SEMIBOLD).child(title))
                            .child(div().text_xs().text_color(muted()).child(detail)))
                        .child(div().flex().items_center().gap_1().flex_none()
                            .when(!cue.is_enabled, |this| this.child(div().text_xs().text_color(muted()).child("Off")))
                            .child(svg().path("icons/chevron-right.svg").size(px(14.0)).text_color(muted())))
                        .when(!busy, |this| this.cursor_pointer().hover(|style| style.bg(rgb(0xeaf1fc)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.select_motion_region(cue_id);
                                if let Some((_, _, midpoint)) = range { this.seek_video(midpoint, cx); }
                                cx.notify();
                            })))
                }))))
            .when(self.video_project.is_some(), |this| this
                .child(self.section_header("motion-automatic", "Automatic zoom", None, cx))
                .when(self.section_open("motion-automatic"), |this| this
                    .child(div().flex().flex_wrap().gap_2()
                        .child(self.small_button("motion-regenerate", "Compact clicks", !busy && has_clicks, cx,
                            |this, cx| this.regenerate_motion_from_clicks(AutoZoomStyle::Compact, cx)))
                        .child(self.small_button("motion-regenerate-cinematic", "Cinematic clicks", !busy && has_clicks, cx,
                            |this, cx| this.regenerate_motion_from_clicks(AutoZoomStyle::Cinematic, cx))))
                    .child(div().text_xs().text_color(muted()).child(if has_clicks {
                        "Build zooms from recorded clicks. Replaces existing regions; Undo restores them."
                    } else { "This recording has no captured clicks. Add a zoom above to choose its focus manually." }))))
            .child(self.section_header("motion-defaults", "New zoom defaults", None, cx))
            .when(self.section_open("motion-defaults"), |this| this
                .child(self.scene_slider_row(SceneSlider::DefaultZoom, cx))
                .child(div().text_xs().text_color(muted()).child("Starting magnification for new zooms.")))
            .into_any_element()
    }
}
