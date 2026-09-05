//! The editor shell shared by the static, motion, and video modes: one top
//! bar (mode switcher, undo/redo, export), one tabbed inspector, and one
//! timeline bar. Mode-specific code only supplies the canvas and the lanes.

use gpui::{
    canvas, div, hsla, prelude::*, px, rgb, svg, AnyElement, Context, CursorStyle, FontWeight,
    Hsla, MouseButton, MouseDownEvent, Pixels, ScrollDelta, ScrollWheelEvent, Size,
    Window,
};

use crate::{
    blue, crop_rect_with_aspect, ink, line,
    motion_ui::{ANIMATION_DURATIONS, BORDER_COLORS},
    muted, panel,
    recording::{
        clips::ClipEdge, scene::WindowFrame, viewport::MotionPreset,
    },
    scene_ui::{SceneSelection, SceneSlider},
    timestamped_export_name, CropRect, Studio, Tool, VideoMoveDrag,
};

pub(crate) const INSPECTOR_WIDTH: f32 = 316.0;
pub(crate) const TOP_BAR_HEIGHT: f32 = 52.0;
const CANVAS_PADDING: f32 = 24.0;
const TIMELINE_CONTROLS_HEIGHT: f32 = 46.0;
const TIMELINE_LANES_HEIGHT: f32 = 148.0;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EditorMode {
    Static,
    Motion,
    Video,
}

impl EditorMode {
    const ALL: [EditorMode; 3] = [EditorMode::Static, EditorMode::Motion, EditorMode::Video];

    fn label(self) -> &'static str {
        match self {
            EditorMode::Static => "Static",
            EditorMode::Motion => "Motion",
            EditorMode::Video => "Video",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum InspectorTab {
    #[default]
    Design,
    Annotate,
    Motion,
    Record,
    Export,
}

impl InspectorTab {
    const ALL: [InspectorTab; 5] = [
        InspectorTab::Design,
        InspectorTab::Annotate,
        InspectorTab::Motion,
        InspectorTab::Record,
        InspectorTab::Export,
    ];

    fn label(self) -> &'static str {
        match self {
            InspectorTab::Design => "Design",
            InspectorTab::Annotate => "Annotate",
            InspectorTab::Motion => "Motion",
            InspectorTab::Record => "Record",
            InspectorTab::Export => "Export",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            InspectorTab::Design => "icons/palette.svg",
            InspectorTab::Annotate => "icons/pen.svg",
            InspectorTab::Motion => "icons/film.svg",
            InspectorTab::Record => "icons/video.svg",
            InspectorTab::Export => "icons/upload.svg",
        }
    }

    fn available(self, mode: EditorMode) -> bool {
        match self {
            InspectorTab::Design | InspectorTab::Annotate => true,
            InspectorTab::Motion | InspectorTab::Export => mode != EditorMode::Static,
            InspectorTab::Record => mode == EditorMode::Video,
        }
    }
}

fn divider() -> AnyElement {
    div()
        .w(px(1.0))
        .h(px(22.0))
        .mx_1()
        .bg(line())
        .into_any_element()
}

/// Empty stretch of the top bar that moves the window.
fn drag_region(id: &'static str) -> AnyElement {
    div()
        .id(id)
        .flex_1()
        .h_full()
        .on_mouse_down(MouseButton::Left, |event, window, _| {
            if event.click_count >= 2 {
                window.zoom_window();
            } else {
                window.start_window_move();
            }
        })
        .into_any_element()
}

impl Studio {
    // ------------------------------------------------------------------
    // Mode
    // ------------------------------------------------------------------

    pub(crate) fn editor_mode(&self) -> EditorMode {
        if self.video_project.is_some() {
            EditorMode::Video
        } else if self.animation_active {
            EditorMode::Motion
        } else {
            EditorMode::Static
        }
    }

    pub(crate) fn switch_mode(&mut self, mode: EditorMode, cx: &mut Context<Self>) {
        if mode == self.editor_mode() {
            return;
        }
        if self.crop_active {
            self.cancel_crop();
        }
        self.stop_editing_text();
        match mode {
            EditorMode::Static => {
                if self.video_project.is_some() {
                    self.close_video_editor(cx);
                }
                if self.animation_active {
                    self.exit_animation();
                }
            }
            EditorMode::Motion => {
                // Videos already have a motion timeline. Open its controls
                // without switching to screenshot animation or closing it.
                if self.video_project.is_some() {
                    self.select_inspector_tab(InspectorTab::Motion);
                    self.inspector_visible = true;
                    cx.notify();
                    return;
                }
                if self.captured_path.is_none() {
                    self.toast = Some("Capture an image first".into());
                    cx.notify();
                    return;
                }
                if !self.animation_active {
                    self.toggle_animation(cx);
                }
                self.select_inspector_tab(InspectorTab::Motion);
                self.inspector_visible = true;
            }
            EditorMode::Video => match self.last_video_project.clone() {
                Some(directory) => {
                    if let Err(error) = self.open_video_project(directory.clone()) {
                        self.last_video_project = None;
                        self.toast =
                            Some(format!("Could not open {}: {error}", directory.display()).into());
                    }
                }
                None => self.open_video_project_dialog(cx),
            },
        }
        cx.notify();
    }

    fn can_undo(&self) -> bool {
        match self.editor_mode() {
            EditorMode::Video => !self.video_undo_stack.is_empty() && !self.video_edit_busy,
            EditorMode::Motion => !self.video_undo_stack.is_empty() || !self.undo_stack.is_empty(),
            EditorMode::Static => {
                !self.crop_active
                    && (!self.undo_stack.is_empty() || !self.crop_undo_stack.is_empty())
            }
        }
    }

    fn can_redo(&self) -> bool {
        match self.editor_mode() {
            EditorMode::Video => !self.video_redo_stack.is_empty() && !self.video_edit_busy,
            EditorMode::Motion => !self.video_redo_stack.is_empty() || !self.redo_stack.is_empty(),
            EditorMode::Static => {
                !self.crop_active
                    && (!self.redo_stack.is_empty() || !self.crop_redo_stack.is_empty())
            }
        }
    }

    fn undo_current(&mut self, cx: &mut Context<Self>) {
        match self.editor_mode() {
            EditorMode::Video => self.undo_video_edit(cx),
            EditorMode::Motion => {
                if !self.video_undo_stack.is_empty() {
                    self.undo_video_edit(cx);
                } else if self.undo_annotations() {
                    let _ = self.rebuild_redactions();
                }
            }
            EditorMode::Static => {
                if self.undo_annotations() || self.undo_crop() {
                    if self.captured_path.is_some() {
                        let _ = self.rebuild_redactions();
                    }
                    self.toast = Some("Undo".into());
                }
            }
        }
    }

    fn redo_current(&mut self, cx: &mut Context<Self>) {
        match self.editor_mode() {
            EditorMode::Video => self.redo_video_edit(cx),
            EditorMode::Motion => {
                if !self.video_redo_stack.is_empty() {
                    self.redo_video_edit(cx);
                } else if self.redo_annotations() {
                    let _ = self.rebuild_redactions();
                }
            }
            EditorMode::Static => {
                if self.redo_annotations() || self.redo_crop() {
                    if self.captured_path.is_some() {
                        let _ = self.rebuild_redactions();
                    }
                    self.toast = Some("Redo".into());
                }
            }
        }
    }

    /// The top-bar Export action for the current mode.
    pub(crate) fn export_current(&mut self, cx: &mut Context<Self>) {
        match self.editor_mode() {
            EditorMode::Video => self.export_video_recording(cx),
            EditorMode::Motion => self.export_animated_screenshot(cx),
            EditorMode::Static => self.save_static_image(cx),
        }
    }

    fn save_static_image(&mut self, cx: &mut Context<Self>) {
        if self.captured_path.is_none() {
            self.toast = Some("Capture an image first".into());
            cx.notify();
            return;
        }
        let directory = crate::library::screenshots_root();
        if let Err(error) = std::fs::create_dir_all(&directory) {
            self.toast = Some(format!("Could not create screenshot folder: {error}").into());
            cx.notify();
            return;
        }
        let suggested_name = timestamped_export_name();
        let prompt = cx.prompt_for_new_path(&directory, Some(&suggested_name));
        cx.spawn(async move |weak, cx| {
            let selected = match prompt.await {
                Ok(Ok(destination)) => Ok(destination),
                Ok(Err(error)) => Err(error.to_string()),
                Err(error) => Err(error.to_string()),
            };
            weak.update(cx, |this, cx| {
                this.toast = Some(match selected {
                    Ok(Some(path)) => match this.render_export(&path) {
                        Ok(()) => format!("Exported to {}", path.display()).into(),
                        Err(error) => format!("Export failed: {error}").into(),
                    },
                    Ok(None) => "Export cancelled".into(),
                    Err(error) => format!("Export failed: {error}").into(),
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // ------------------------------------------------------------------
    // Layout budget
    // ------------------------------------------------------------------

    fn timeline_lane_extra(&self) -> f32 {
        let camera = if self.video_camera_path.is_some() {
            26.0
        } else {
            0.0
        };
        let annotations = if self.annotations.is_empty() {
            0.0
        } else {
            self.annotation_lane_height() + 4.0
        };
        camera + annotations
    }

    fn timeline_bar_height(&self) -> f32 {
        if self.editor_mode() == EditorMode::Static {
            0.0
        } else {
            TIMELINE_CONTROLS_HEIGHT + TIMELINE_LANES_HEIGHT + self.timeline_lane_extra() + 1.0
        }
    }

    /// The preview size that fits between the top bar, the timeline, and the
    /// inspector.
    pub(crate) fn canvas_budget(&self, viewport: Size<Pixels>) -> (Pixels, Pixels) {
        let inspector = if self.inspector_visible {
            INSPECTOR_WIDTH
        } else {
            0.0
        };
        let width = (viewport.width - px(CANVAS_PADDING * 2.0 + inspector)).max(px(320.0));
        let height = (viewport.height
            - px(TOP_BAR_HEIGHT + CANVAS_PADDING * 2.0 + self.timeline_bar_height()))
        .max(px(220.0));
        self.preview_canvas_size(width, height)
    }

    /// The canvas area with its overlays (zoom label, toast, export status).
    pub(crate) fn canvas_area(&self, canvas: AnyElement, cx: &mut Context<Self>) -> AnyElement {
        let export_overlay = self.export_status_overlay(cx);
        let toast = self.toast.clone();
        div()
            .relative()
            .flex_1()
            .min_h_0()
            .p(px(CANVAS_PADDING))
            .flex()
            .items_center()
            .justify_center()
            .bg(rgb(0xf3f3f4))
            .child(canvas)
            .when_some(toast, |this, toast| {
                this.child(
                    div()
                        .absolute()
                        .bottom(px(20.0))
                        .left_0()
                        .right_0()
                        .flex()
                        .justify_center()
                        .child(
                            div()
                                .px_4()
                                .py_2()
                                .rounded_lg()
                                .bg(hsla(220.0 / 360.0, 0.2, 0.12, 0.9))
                                .text_sm()
                                .text_color(rgb(0xffffff))
                                .child(toast),
                        ),
                )
            })
            .when_some(export_overlay, |this, overlay| this.child(overlay))
            .into_any_element()
    }

    // ------------------------------------------------------------------
    // Top bar
    // ------------------------------------------------------------------

    fn bar_button(
        &self,
        id: &'static str,
        icon: &'static str,
        label: Option<&'static str>,
        enabled: bool,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut Studio, &mut Window, &mut Context<Studio>) + 'static,
    ) -> AnyElement {
        let color = if enabled { ink() } else { muted() };
        div()
            .id(id)
            .h(px(32.0))
            .when(label.is_some(), |this| this.px_2())
            .when(label.is_none(), |this| this.w(px(32.0)).justify_center())
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .rounded_md()
            .text_sm()
            .text_color(color)
            .opacity(if enabled { 1.0 } else { 0.45 })
            .when(enabled, |this| {
                this.cursor_pointer()
                    .hover(|style| style.bg(rgb(0xeeeeef)))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        on_click(this, window, cx);
                        cx.notify();
                    }))
            })
            .child(svg().path(icon).size(px(17.0)).text_color(color))
            .when_some(label, |this, label| this.child(label))
            .into_any_element()
    }

    fn mode_switcher(&self, cx: &mut Context<Self>) -> AnyElement {
        let current = self.editor_mode();
        div()
            .flex()
            .flex_none()
            .h(px(34.0))
            .p(px(3.0))
            .rounded_lg()
            .bg(rgb(0xf0f0f1))
            .children(
                EditorMode::ALL
                    .into_iter()
                    .enumerate()
                    .map(|(index, mode)| {
                        let selected = mode == current;
                        div()
                            .id(("editor-mode", index))
                            .px_4()
                            .h_full()
                            .flex()
                            .items_center()
                            .rounded_md()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(if selected { ink() } else { muted() })
                            .when(selected, |this| this.bg(rgb(0xffffff)).shadow_sm())
                            .cursor_pointer()
                            .child(mode.label())
                            .on_click(cx.listener(move |this, _, _, cx| this.switch_mode(mode, cx)))
                    }),
            )
            .into_any_element()
    }

    fn crop_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        let crop_pixel_size = self.captured_dimensions.map(|(width, height)| {
            format!(
                "{} × {}",
                (self.crop_rect.width * width as f32).round() as u32,
                (self.crop_rect.height * height as f32).round() as u32
            )
        });
        let text_button = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .px_3()
                .h(px(32.0))
                .flex()
                .items_center()
                .rounded_md()
                .text_sm()
                .cursor_pointer()
                .hover(|style| style.bg(rgb(0xeeeeef)))
                .child(label)
        };
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .child(div().w(px(430.0)).child(self.segmented(
                "crop-aspect",
                &["Free", "Original", "1:1", "16:9", "9:16", "4:3", "3:2"],
                self.crop_aspect,
                |this, value| this.set_crop_aspect(value),
                cx,
            )))
            .when_some(crop_pixel_size, |this, value| {
                this.child(div().px_2().text_xs().text_color(muted()).child(value))
            })
            .child(
                text_button("crop-reset", "Reset").on_click(cx.listener(|this, _, _, cx| {
                    this.crop_rect = CropRect::UNIT;
                    if let Some(ratio) = this.crop_normalized_aspect() {
                        this.crop_rect = crop_rect_with_aspect(this.crop_rect, ratio);
                    }
                    cx.notify();
                })),
            )
            .child(
                text_button("crop-cancel", "Cancel").on_click(cx.listener(|this, _, _, cx| {
                    this.cancel_crop();
                    cx.notify();
                })),
            )
            .child(
                div()
                    .id("crop-apply")
                    .px_4()
                    .h(px(32.0))
                    .flex()
                    .items_center()
                    .rounded_md()
                    .bg(blue())
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(rgb(0xffffff))
                    .cursor_pointer()
                    .child("Crop")
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Err(error) = this.apply_crop() {
                            this.toast = Some(error.into());
                        }
                        cx.notify();
                    })),
            )
            .into_any_element()
    }

    pub(crate) fn top_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let mode = self.editor_mode();
        let export_busy = self.export_progress.is_some();
        let can_undo = self.can_undo();
        let can_redo = self.can_redo();

        let left = div()
            .flex()
            .flex_none()
            .items_center()
            .h_full()
            .gap_1()
            .child(
                div()
                    .id("wordmark-drag")
                    .h_full()
                    .pl_4()
                    .pr_3()
                    .flex()
                    .items_center()
                    .child(crate::brand_wordmark(87.5, 28.0))
                    .on_mouse_down(MouseButton::Left, |event, window, _| {
                        if event.click_count >= 2 {
                            window.zoom_window();
                        } else {
                            window.start_window_move();
                        }
                    }),
            )
            .child(divider())
            .when(!self.crop_active, |this| {
                this.child(self.bar_button(
                    "bar-record-new",
                    "icons/record.svg",
                    Some("Record new"),
                    true,
                    cx,
                    |this, _, cx| this.open_recorder_window(cx),
                ))
            });

        let right = div()
            .flex()
            .flex_none()
            .items_center()
            .h_full()
            .gap_1()
            .when(!self.crop_active, |this| {
                this.child(self.bar_button(
                    "bar-undo",
                    "icons/undo.svg",
                    None,
                    can_undo,
                    cx,
                    |this, _, cx| this.undo_current(cx),
                ))
                .child(self.bar_button(
                    "bar-redo",
                    "icons/redo.svg",
                    None,
                    can_redo,
                    cx,
                    |this, _, cx| this.redo_current(cx),
                ))
                .child(divider())
                .when(mode == EditorMode::Static, |this| {
                    this.child(self.bar_button(
                        "bar-crop",
                        "icons/crop.svg",
                        Some("Crop"),
                        self.captured_path.is_some(),
                        cx,
                        |this, _, _| {
                            this.stop_editing_text();
                            this.begin_crop();
                        },
                    ))
                })
                .child(
                    div()
                        .id("bar-export")
                        .h(px(32.0))
                        .px_3()
                        .ml_1()
                        .flex()
                        .items_center()
                        .gap_2()
                        .rounded_md()
                        .bg(blue())
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(0xffffff))
                        .opacity(if export_busy { 0.5 } else { 1.0 })
                        .when(!export_busy, |this| {
                            this.cursor_pointer()
                                .hover(|style| style.bg(hsla(211.0 / 360.0, 0.95, 0.48, 1.0)))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.stop_editing_text();
                                    this.export_current(cx);
                                    cx.notify();
                                }))
                        })
                        .child(
                            svg()
                                .path("icons/upload.svg")
                                .size(px(15.0))
                                .text_color(rgb(0xffffff)),
                        )
                        .child("Export"),
                )
                .child(
                    div()
                        .id("bar-sidebar")
                        .size(px(32.0))
                        .ml_1()
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_md()
                        .cursor_pointer()
                        .when(!self.inspector_visible, |this| this.bg(rgb(0xe7f1ff)))
                        .hover(|style| style.bg(rgb(0xeeeeef)))
                        .child(svg().path("icons/sidebar.svg").size(px(17.0)).text_color(
                            if self.inspector_visible {
                                muted()
                            } else {
                                blue()
                            },
                        ))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.inspector_visible = !this.inspector_visible;
                            cx.notify();
                        })),
                )
            })
            .pr_3();

        div()
            .h(px(TOP_BAR_HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .bg(rgb(0xffffff))
            .border_b_1()
            .border_color(line())
            .child(left)
            .child(drag_region("top-bar-drag-left"))
            .child(if self.crop_active {
                self.crop_controls(cx)
            } else {
                self.mode_switcher(cx)
            })
            .child(drag_region("top-bar-drag-right"))
            .child(right)
            .into_any_element()
    }

    // ------------------------------------------------------------------
    // Inspector
    // ------------------------------------------------------------------

    /// The tab the inspector shows: a selection on the canvas or timeline
    /// pulls its own tab forward until it is done.
    fn effective_tab(&self) -> InspectorTab {
        let mode = self.editor_mode();
        let tab = if mode != EditorMode::Static
            && (self.video_selected_zoom_cue.is_some() || self.walkthrough_mode)
        {
            InspectorTab::Motion
        } else if self.selected_annotation.is_some() || self.tool != Tool::Select {
            InspectorTab::Annotate
        } else if self.scene_selection == SceneSelection::Media {
            // Transform lives in Motion; static mode has no Motion tab and
            // falls back to Design below.
            InspectorTab::Motion
        } else {
            self.inspector_tab
        };
        if tab.available(mode) {
            tab
        } else {
            InspectorTab::Design
        }
    }

    fn select_inspector_tab(&mut self, tab: InspectorTab) {
        self.stop_editing_text();
        self.selected_annotation = None;
        self.annotation_draft = None;
        self.video_selected_zoom_cue = None;
        self.scene_selection = SceneSelection::Scene;
        self.walkthrough_mode = false;
        if tab != InspectorTab::Annotate {
            self.tool = Tool::Select;
        }
        self.inspector_tab = tab;
    }

    pub(crate) fn section_open(&self, id: &'static str) -> bool {
        self.open_sections.contains(id)
    }

    fn toggle_section(&mut self, id: &'static str) {
        if !self.open_sections.remove(id) {
            self.open_sections.insert(id);
        }
    }

    /// Collapsible section header; `trailing` (a switch, usually) sits before
    /// the chevron and handles its own clicks.
    fn section_header(
        &self,
        id: &'static str,
        title: &'static str,
        trailing: Option<AnyElement>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open = self.section_open(id);
        div()
            .id(gpui::SharedString::from(format!("section-header-{id}")))
            .flex()
            .flex_none()
            .items_center()
            .justify_between()
            .h(px(40.0))
            .mt_1()
            .border_t_1()
            .border_color(line())
            .cursor_pointer()
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .when_some(trailing, |this, element| this.child(element))
                    .child(
                        svg()
                            .path(if open {
                                "icons/chevron-down.svg"
                            } else {
                                "icons/chevron-right.svg"
                            })
                            .size(px(16.0))
                            .text_color(muted()),
                    ),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.toggle_section(id);
                cx.notify();
            }))
            .into_any_element()
    }

    /// A switch for a section header. Turning a feature on opens its section.
    fn header_switch(
        &self,
        id: &'static str,
        section: &'static str,
        enabled: bool,
        cx: &mut Context<Self>,
        on_toggle: impl Fn(&mut Studio) + 'static,
    ) -> AnyElement {
        div()
            .id(id)
            .cursor_pointer()
            .child(self.toggle(enabled))
            .on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                on_toggle(this);
                if !enabled {
                    this.open_sections.insert(section);
                }
                cx.notify();
            }))
            .into_any_element()
    }

    fn tab_label(text: &'static str) -> AnyElement {
        div()
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(muted())
            .child(text)
            .into_any_element()
    }

    fn design_tab(&self, cx: &mut Context<Self>) -> AnyElement {
        // Transform only surfaces here in static mode, where Motion is absent.
        let transform = if self.editor_mode() == EditorMode::Static {
            self.transform_inspector(cx)
        } else {
            None
        };
        let wallpaper_tab = self.wallpaper_tab;
        let border = self.border;
        let watermark = self.watermark_enabled;
        div()
            .flex()
            .flex_col()
            .gap_2()
            .when_some(transform, |this, panel| this.child(panel))
            .child(Self::tab_label("Background"))
            .child(self.segmented(
                "fill-type",
                &["Color", "Gradient", "Wallpaper"],
                wallpaper_tab,
                |this, value| this.wallpaper_tab = value,
                cx,
            ))
            .when(wallpaper_tab == 2, |this| {
                this.child(self.segmented(
                    "fill-library",
                    &["Recent", "UIHSSN", "Fayazara"],
                    self.library_tab,
                    |this, value| this.library_tab = value,
                    cx,
                ))
            })
            .child(self.fill_picker(cx))
            .child(self.section_header("effects", "Background effects", None, cx))
            .when(self.section_open("effects"), |this| {
                this.child(self.background_effects_section(cx))
            })
            .child(div().h(px(4.0)))
            .child(Self::tab_label("Layout"))
            .child(self.slider_row(
                "Padding",
                self.padding,
                "%",
                |this, value| this.padding = value,
                cx,
            ))
            .child(self.slider_row(
                "Corners",
                self.corners,
                "%",
                |this, value| this.corners = value,
                cx,
            ))
            .child(self.slider_row(
                "Shadow",
                self.shadow,
                "%",
                |this, value| this.shadow = value,
                cx,
            ))
            .child(self.segmented(
                "shadow-style",
                &["Soft", "Long", "Glow", "Crisp"],
                self.shadow_style,
                |this, value| this.shadow_style = value,
                cx,
            ))
            .child(div().text_xs().text_color(muted()).child("Window frame"))
            .child(
                self.segmented(
                    "window-frame",
                    &["None", "Light", "Dark"],
                    WindowFrame::ALL
                        .iter()
                        .position(|frame| *frame == self.window_frame)
                        .unwrap_or(0),
                    |this, value| this.window_frame = WindowFrame::ALL[value.min(2)],
                    cx,
                ),
            )
            .child(div().text_xs().text_color(muted()).child("Aspect ratio"))
            .child(self.segmented(
                "aspect-ratio",
                &["Auto", "1:1", "4:3", "3:2", "16:9"],
                self.aspect_ratio,
                |this, value| this.aspect_ratio = value,
                cx,
            ))
            .child(self.section_header(
                "border",
                "Border",
                Some(
                    self.header_switch("border-switch", "border", border, cx, |this| {
                        this.border = !this.border
                    }),
                ),
                cx,
            ))
            .when(self.section_open("border"), |this| {
                this.child(div().flex().items_center().gap_2().children(
                    BORDER_COLORS.iter().enumerate().map(|(index, color)| {
                        let selected = self.border_color == index;
                        div()
                            .id(("border-color", index))
                            .size(px(24.0))
                            .rounded_full()
                            .bg(rgb(*color))
                            .border_2()
                            .border_color(if selected {
                                ink()
                            } else {
                                Hsla::from(rgb(0xd4d5d8))
                            })
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.border_color = index;
                                this.border = true;
                                cx.notify();
                            }))
                    }),
                ))
                .child(self.slider_row(
                    "Thickness",
                    self.border_thickness,
                    "",
                    |this, value| this.border_thickness = value,
                    cx,
                ))
                .child(self.slider_row(
                    "Opacity",
                    self.border_opacity,
                    "%",
                    |this, value| this.border_opacity = value,
                    cx,
                ))
            })
            .child(self.section_header(
                "watermark",
                "Watermark",
                Some(
                    self.header_switch("watermark-switch", "watermark", watermark, cx, |this| {
                        this.watermark_enabled = !this.watermark_enabled;
                        if this.watermark_enabled && this.watermark.text.is_empty() {
                            this.watermark_editing = true;
                        }
                    }),
                ),
                cx,
            ))
            .when(self.section_open("watermark"), |this| {
                this.child(self.watermark_section(cx))
            })
            .child(div().h(px(4.0)))
            .child(Self::tab_label("Presets"))
            .child(self.quick_presets_row(cx))
            .child(self.section_header("saved-presets", "Saved presets", None, cx))
            .when(self.section_open("saved-presets"), |this| {
                this.child(self.preset_library_section(cx))
            })
            .into_any_element()
    }

    fn annotate_tab(&self, cx: &mut Context<Self>) -> AnyElement {
        let selected = self
            .selected_annotation
            .filter(|index| *index < self.annotations.len());
        let timing = self.annotation_timing_inspector(cx);
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(self.video_annotate_section(cx))
            .child(self.annotation_style_controls(cx))
            .when_some(timing, |this, panel| this.child(panel))
            .when(selected.is_some() || self.tool != Tool::Select, |this| {
                this.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .when(selected.is_some(), |this| {
                            this.child(self.small_button(
                                "annotation-delete",
                                "Delete mark",
                                true,
                                cx,
                                |this, _| {
                                    if let Some(index) = this.selected_annotation.take() {
                                        if index < this.annotations.len() {
                                            this.record_annotation_undo();
                                            this.annotations.remove(index);
                                        }
                                    }
                                },
                            ))
                        })
                        .child(self.small_button(
                            "annotation-done",
                            "Done",
                            true,
                            cx,
                            |this, _| {
                                this.stop_editing_text();
                                this.selected_annotation = None;
                                this.tool = Tool::Select;
                            },
                        )),
                )
            })
            .into_any_element()
    }

    /// Animated-screenshot settings: duration, motion preset, scenes, and
    /// the cursor walkthrough.
    fn animation_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let duration_index = ANIMATION_DURATIONS
            .iter()
            .position(|value| (*value - self.animation_duration).abs() < 1e-9)
            .unwrap_or(usize::MAX);
        let selected_preset = self.animation_preset;
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(Self::tab_label("Duration"))
            .child(self.segmented(
                "animation-duration",
                &["3 s", "5 s", "8 s", "10 s"],
                duration_index,
                |this, index| this.set_animation_duration(ANIMATION_DURATIONS[index]),
                cx,
            ))
            .child(Self::tab_label("Motion preset"))
            .child(
                div().flex().flex_wrap().gap(px(6.0)).children(
                    MotionPreset::ALL
                        .into_iter()
                        .enumerate()
                        .map(|(index, preset)| {
                            let selected = selected_preset == Some(preset);
                            div()
                                .id(("animation-preset", index))
                                .w(px(90.0))
                                .flex()
                                .flex_col()
                                .rounded_md()
                                .overflow_hidden()
                                .border_2()
                                .border_color(if selected {
                                    crate::motion_ui::orange(false)
                                } else {
                                    rgb(0xe4e4e7).into()
                                })
                                .bg(rgb(0xf0f0f1))
                                .cursor_pointer()
                                .hover(move |style| {
                                    if selected {
                                        style
                                    } else {
                                        style.border_color(rgb(0xc8c8cc))
                                    }
                                })
                                .child(
                                    div().w_full().h(px(52.0)).bg(rgb(0x2a2a30)).child(
                                        div()
                                            .size_full()
                                            .child(crate::preset_cards::preset_preview(preset)),
                                    ),
                                )
                                .child(
                                    div()
                                        .px_1()
                                        .h(px(20.0))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(if selected {
                                            crate::motion_ui::orange(false)
                                        } else {
                                            ink()
                                        })
                                        .child(preset.label()),
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.apply_motion_preset(preset);
                                    cx.notify();
                                }))
                        }),
                ),
            )
            .child(Self::tab_label("Scenes"))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_1()
                    .children((0..self.image_scenes.len().max(1)).map(|index| {
                        let selected = index == self.image_scene_index;
                        div()
                            .id(("image-scene", index))
                            .min_w(px(30.0))
                            .h(px(30.0))
                            .px_2()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .bg(if selected {
                                blue()
                            } else {
                                rgb(0xf0f0f1).into()
                            })
                            .text_color(if selected {
                                rgb(0xffffff).into()
                            } else {
                                ink()
                            })
                            .cursor_pointer()
                            .child(format!("{}", index + 1))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.switch_image_scene(index, cx);
                            }))
                    }))
                    .child(self.small_button(
                        "image-scene-add",
                        "+ Add image",
                        true,
                        cx,
                        |this, cx| this.add_image_scene_dialog(cx),
                    ))
                    .when(self.image_scenes.len() > 1, |this| {
                        this.child(self.small_button(
                            "image-scene-remove",
                            "Remove scene",
                            true,
                            cx,
                            |this, cx| this.remove_image_scene(cx),
                        ))
                    }),
            )
            .child(Self::tab_label("Cursor walkthrough"))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        div()
                            .id("walkthrough-toggle")
                            .px_3()
                            .h(px(30.0))
                            .flex()
                            .items_center()
                            .rounded_md()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .bg(if self.walkthrough_mode {
                                crate::motion_ui::orange(false)
                            } else {
                                rgb(0xf0f0f1).into()
                            })
                            .text_color(if self.walkthrough_mode {
                                rgb(0xffffff).into()
                            } else {
                                ink()
                            })
                            .cursor_pointer()
                            .child(if self.walkthrough_mode {
                                "Placing stops… (Enter to finish)"
                            } else if self.has_walkthrough() {
                                "Add more stops"
                            } else {
                                "Place cursor stops"
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.walkthrough_mode = !this.walkthrough_mode;
                                this.video_selected_zoom_cue = None;
                                this.scene_selection = SceneSelection::Scene;
                                if this.walkthrough_mode {
                                    this.pause_video_playback();
                                    this.toast = Some(
                                        "Click the spots the cursor should visit, in order".into(),
                                    );
                                }
                                cx.notify();
                            })),
                    )
                    .when(self.has_walkthrough(), |this| {
                        this.child(self.small_button(
                            "walkthrough-clear",
                            "Clear path",
                            true,
                            cx,
                            |this, _| this.clear_walkthrough(),
                        ))
                    }),
            )
            .child(div().h(px(1.0)).mt_1().bg(line()))
            .into_any_element()
    }

    fn motion_tab(&self, cx: &mut Context<Self>) -> AnyElement {
        let mode = self.editor_mode();
        let region = self.motion_inspector(cx);
        let transform = self.transform_inspector(cx);
        div()
            .flex()
            .flex_col()
            .gap_3()
            .when_some(transform, |this, panel| this.child(panel))
            .when_some(region, |this, panel| this.child(panel))
            .when(mode == EditorMode::Motion, |this| {
                this.child(self.animation_settings(cx))
            })
            .child(self.motion_overview_section(cx))
            .child(self.scene_slider_row(SceneSlider::DefaultZoom, cx))
            .child(self.section_header("templates", "Templates", None, cx))
            .when(self.section_open("templates"), |this| {
                this.child(self.template_gallery_section(cx))
            })
            .into_any_element()
    }

    fn record_tab(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(self.section_header("pointer", "Pointer", None, cx))
            .when(self.section_open("pointer"), |this| {
                this.child(self.pointer_section(cx))
            })
            .child(self.section_header("camera", "Camera", None, cx))
            .when(self.section_open("camera"), |this| {
                this.child(self.camera_section(cx))
            })
            .child(self.section_header("audio", "Audio", None, cx))
            .when(self.section_open("audio"), |this| {
                this.child(self.audio_section(cx))
            })
            .into_any_element()
    }

    pub(crate) fn sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let mode = self.editor_mode();
        let tab = self.effective_tab();
        let body = match tab {
            InspectorTab::Design => self.design_tab(cx),
            InspectorTab::Annotate => self.annotate_tab(cx),
            InspectorTab::Motion => self.motion_tab(cx),
            InspectorTab::Record => self.record_tab(cx),
            InspectorTab::Export => self.export_section(cx),
        };
        let tabs = div()
            .h(px(56.0))
            .flex_none()
            .flex()
            .px_2()
            .pt_1()
            .gap_1()
            .border_b_1()
            .border_color(line())
            .children(
                InspectorTab::ALL
                    .into_iter()
                    .filter(|candidate| candidate.available(mode))
                    .enumerate()
                    .map(|(index, candidate)| {
                        let selected = candidate == tab;
                        div()
                            .id(("inspector-tab", index))
                            .flex_1()
                            .flex()
                            .flex_col()
                            .items_center()
                            .justify_center()
                            .gap_1()
                            .rounded_md()
                            .cursor_pointer()
                            .when(selected, |this| this.bg(rgb(0xffffff)).shadow_sm())
                            .hover(move |style| {
                                if selected {
                                    style
                                } else {
                                    style.bg(rgb(0xf1f1f2))
                                }
                            })
                            .child(
                                svg()
                                    .path(candidate.icon())
                                    .size(px(17.0))
                                    .text_color(if selected { blue() } else { muted() }),
                            )
                            .child(
                                div()
                                    .text_size(px(10.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(if selected { ink() } else { muted() })
                                    .child(candidate.label()),
                            )
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.select_inspector_tab(candidate);
                                cx.notify();
                            }))
                    }),
            );
        div()
            .w(px(INSPECTOR_WIDTH))
            .flex_none()
            .h_full()
            .relative()
            .flex()
            .flex_col()
            .bg(panel())
            .border_l_1()
            .border_color(line())
            .child(tabs)
            .child(
                div()
                    .id("inspector-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_4()
                    .flex()
                    .flex_col()
                    .child(body),
            )
            .when(self.crop_active, |this| {
                this.opacity(0.52).child(
                    div()
                        .id("crop-inspector-blocker")
                        .absolute()
                        .inset_0()
                        .bg(hsla(0.0, 0.0, 1.0, 0.01))
                        .on_mouse_down(MouseButton::Left, |_, _, _| {}),
                )
            })
            .into_any_element()
    }

    // ------------------------------------------------------------------
    // Timeline
    // ------------------------------------------------------------------

    fn toggle_timeline_playback(&mut self, cx: &mut Context<Self>) {
        if self.video_project.is_some() {
            if self.video_playing {
                self.pause_video_playback();
            } else {
                self.start_video_playback(cx);
            }
        } else {
            self.toggle_animation_playback(cx);
        }
    }

    /// Editing controls for an animated screenshot's timeline.
    fn motion_edit_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        let busy = self.export_progress.is_some();
        let has_selection = self.video_selected_zoom_cue.is_some();
        div()
            .flex()
            .items_center()
            .gap_1()
            .child(
                div()
                    .id("timeline-add-motion")
                    .px_3()
                    .h(px(32.0))
                    .flex()
                    .items_center()
                    .rounded_md()
                    .text_sm()
                    .opacity(if busy { 0.35 } else { 1.0 })
                    .when(!busy, |this| {
                        this.cursor_pointer()
                            .hover(|style| style.bg(rgb(0xe7f1ff)))
                            .on_click(cx.listener(|this, _, _, cx| {
                                let position = this.video_position;
                                this.add_motion_region_at(position, cx);
                                cx.notify();
                            }))
                    })
                    .child("+ Motion"),
            )
            .child(
                div()
                    .id("timeline-delete-motion")
                    .size(px(32.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_md()
                    .opacity(if has_selection { 1.0 } else { 0.35 })
                    .when(has_selection, |this| {
                        this.cursor_pointer()
                            .hover(|style| style.bg(rgb(0xfee2e2)))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.delete_selected_video_zoom(cx);
                                cx.notify();
                            }))
                    })
                    .child(
                        svg()
                            .path("icons/trash.svg")
                            .size(px(16.0))
                            .text_color(ink()),
                    ),
            )
            .when(self.image_scenes.len() > 1, |this| {
                this.child(
                    div()
                        .ml_2()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(muted())
                        .child(format!(
                            "Scene {} of {} · {:.1}s total",
                            self.image_scene_index + 1,
                            self.image_scenes.len(),
                            self.sequence_duration()
                        )),
                )
            })
            .into_any_element()
    }

    /// Seeks to the pointer's position on the ruler or the clip lane and
    /// arms a scrub drag.
    fn timeline_seek_down(&mut self, event: &MouseDownEvent) {
        self.finish_annotation_interaction();
        self.pause_video_playback();
        self.video_trim_drag = None;
        self.video_zoom_drag = None;
        let target = self
            .video_timeline_bounds
            .lock()
            .ok()
            .and_then(|bounds| *bounds)
            .map(|bounds| {
                let local = ((event.position.x - bounds.origin.x) / px(1.0)) as f64;
                ((self.video_timeline_scroll + local)
                    / (self.video_timeline_viewport_width() * self.video_timeline_zoom))
                    .clamp(0.0, 1.0)
                    * self.video_duration
            })
            .unwrap_or(self.video_position);
        self.video_position = target;
        self.video_seek_drag = Some((event.position.x, target));
    }

    /// The timeline bar shared by the motion and video modes: transport and
    /// edit controls, the ruler, the clip lane, then the motion, annotation,
    /// camera, and audio lanes.
    pub(crate) fn timeline_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let is_video = self.video_project.is_some();
        let playing = self.video_playing;
        let progress = if self.video_duration > 0.0 {
            (self.video_position / self.video_duration).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let current_time = Self::video_timecode(self.video_position);
        let duration = Self::video_timecode(self.video_duration);
        let timeline_duration = self.video_duration.max(f64::EPSILON);
        let timeline_zoom = self.video_timeline_zoom;
        let timeline_scroll = self.video_timeline_scroll;
        let timeline_viewport_width = self.video_timeline_viewport_width();
        let timeline_content_width = timeline_viewport_width * timeline_zoom;
        let timeline_bounds = self.video_timeline_bounds.clone();
        let lane_extra = self.timeline_lane_extra();
        let selected_clip = self.video_selected_clip;
        let move_drag = self.video_move_drag.filter(|drag| drag.active);
        // While dragging, the clip's ghost follows the pointer (snapped the
        // same way the drop will land) so the destination — including a gap
        // past the end of the timeline — is always visible.
        let move_ghost = move_drag.and_then(|drag| {
            let range = self.video_clip_timeline.editor_range(drag.clip_id)?;
            let new_start = self.video_move_new_start(&drag)?;
            let scale = timeline_content_width / timeline_duration;
            Some((
                (new_start * scale) as f32,
                (((range.end - range.start) * scale).max(3.0)) as f32,
            ))
        });
        // Time ruler: adaptive tick step targeting ~80px between labels.
        let ruler_step = {
            let raw = 80.0 * timeline_duration / timeline_content_width.max(1.0);
            [
                0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0,
            ]
            .into_iter()
            .find(|step| *step >= raw)
            .unwrap_or(600.0)
        };
        let mut ruler_marks: Vec<AnyElement> = Vec::new();
        let mut ruler_tick = 0.0;
        while ruler_tick <= timeline_duration + 1e-6 {
            let x = (ruler_tick / timeline_duration * timeline_content_width) as f32;
            ruler_marks.push(
                div()
                    .absolute()
                    .left(px(x))
                    .bottom_0()
                    .w(px(1.0))
                    .h(px(5.0))
                    .bg(hsla(0.0, 0.0, 0.0, 0.30))
                    .into_any_element(),
            );
            let label = if ruler_step < 1.0 {
                format!("{ruler_tick:.1}")
            } else {
                Self::video_timecode(ruler_tick)
            };
            ruler_marks.push(
                div()
                    .absolute()
                    .left(px(x + 5.0))
                    .top(px(1.0))
                    .text_xs()
                    .text_color(muted())
                    .child(label)
                    .into_any_element(),
            );
            let half = ruler_tick + ruler_step / 2.0;
            if half <= timeline_duration {
                let half_x = (half / timeline_duration * timeline_content_width) as f32;
                ruler_marks.push(
                    div()
                        .absolute()
                        .left(px(half_x))
                        .bottom_0()
                        .w(px(1.0))
                        .h(px(3.0))
                        .bg(hsla(0.0, 0.0, 0.0, 0.15))
                        .into_any_element(),
                );
            }
            ruler_tick += ruler_step;
        }
        ruler_marks.extend(self.press_markers(timeline_duration, timeline_content_width, cx));
        let audio_lane = self.audio_lane(timeline_scroll, timeline_content_width, progress);
        let camera_lane = self.camera_lane(timeline_scroll, timeline_content_width, progress);
        let clip_lane: Vec<AnyElement> = if is_video {
            self.video_clip_lane(
                timeline_duration,
                timeline_content_width,
                selected_clip,
                move_drag,
                cx,
            )
        } else {
            // An animated screenshot is one still per scene; the lane shows
            // the scene that is open.
            let label = if self.image_scenes.len() > 1 {
                format!(
                    "Scene {} · {:.1}s",
                    self.image_scene_index + 1,
                    self.video_duration
                )
            } else {
                format!("Image · {:.1}s", self.video_duration)
            };
            vec![div()
                .h_full()
                .w(px(timeline_content_width as f32))
                .flex_none()
                .rounded_md()
                .bg(hsla(217.0 / 360.0, 0.86, 0.58, 1.0))
                .flex()
                .items_center()
                .justify_center()
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(hsla(0.0, 0.0, 1.0, 0.92))
                .child(
                    div()
                        .px_2()
                        .rounded_md()
                        .bg(hsla(0.0, 0.0, 0.0, 0.35))
                        .child(label),
                )
                .into_any_element()]
        };
        let motion_track = self.motion_track(timeline_scroll, timeline_content_width, progress, cx);
        let annotation_track =
            self.annotation_track(timeline_scroll, timeline_content_width, progress, cx);
        let edit_controls = if is_video {
            self.video_edit_controls(cx).into_any_element()
        } else {
            self.motion_edit_controls(cx)
        };

        div()
            .flex_none()
            .px_6()
            .flex()
            .flex_col()
            .bg(rgb(0xffffff))
            .border_t_1()
            .border_color(line())
            .child(
                div()
                    .h(px(TIMELINE_CONTROLS_HEIGHT))
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_3()
                    .border_b_1()
                    .border_color(line())
                    .child(div().flex_1().flex().items_center().child(edit_controls))
                    .child(
                        div()
                            .flex_none()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .id("timeline-play-pause")
                                    .size(px(34.0))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .rounded_full()
                                    .bg(ink())
                                    .cursor_pointer()
                                    .child(
                                        svg()
                                            .path(if playing {
                                                "icons/pause.svg"
                                            } else {
                                                "icons/play.svg"
                                            })
                                            .size(px(15.0))
                                            .text_color(rgb(0xffffff)),
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.toggle_timeline_playback(cx);
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(format!("{current_time} / {duration}")),
                            ),
                    )
                    .child(
                        div().flex_1().flex().items_center().justify_end().child(
                            div()
                                .flex()
                                .items_center()
                                .gap_1()
                                .text_xs()
                                .child(
                                    div()
                                        .id("timeline-zoom-out")
                                        .size(px(28.0))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .rounded_md()
                                        .cursor_pointer()
                                        .hover(|style| style.bg(rgb(0xeeeeef)))
                                        .child("−")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.zoom_video_timeline(
                                                1.0 / 1.25,
                                                this.video_position,
                                            );
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    div()
                                        .id("timeline-fit")
                                        .w(px(42.0))
                                        .text_center()
                                        .cursor_pointer()
                                        .child(format!("{timeline_zoom:.1}×"))
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.video_timeline_zoom = 1.0;
                                            this.video_timeline_scroll = 0.0;
                                            cx.notify();
                                        })),
                                )
                                .child(
                                    div()
                                        .id("timeline-zoom-in")
                                        .size(px(28.0))
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .rounded_md()
                                        .cursor_pointer()
                                        .hover(|style| style.bg(rgb(0xeeeeef)))
                                        .child("+")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.zoom_video_timeline(1.25, this.video_position);
                                            cx.notify();
                                        })),
                                ),
                        ),
                    ),
            )
            .child(
                div()
                    .h(px(TIMELINE_LANES_HEIGHT + lane_extra))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(320.0))
                            .h(px(126.0 + lane_extra))
                            .flex()
                            .flex_col()
                            .justify_center()
                            .child(
                                div()
                                    .relative()
                                    .w_full()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .overflow_hidden()
                                    .cursor(CursorStyle::ResizeLeftRight)
                            .child(
                                div()
                                    .id("timeline-ruler")
                                    .relative()
                                    .w_full()
                                    .h(px(16.0))
                                    .flex_none()
                                    .overflow_hidden()
                                    .child(
                                        div()
                                            .absolute()
                                            .left(px(-(timeline_scroll as f32)))
                                            .top_0()
                                            .w(px(timeline_content_width as f32))
                                            .h_full()
                                            .children(ruler_marks)
                                            .child(
                                                div()
                                                    .absolute()
                                                    .left(px((timeline_content_width * progress)
                                                        as f32
                                                        - 5.0))
                                                    .top(px(2.0))
                                                    .w(px(10.0))
                                                    .h(px(13.0))
                                                    .rounded_sm()
                                                    .bg(ink()),
                                            ),
                                    )
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, event: &MouseDownEvent, _, cx| {
                                            this.timeline_seek_down(event);
                                            cx.notify();
                                        }),
                                    ),
                            )
                            .child(
                                div()
                                    .id("timeline-seek-bar")
                                    .relative()
                                    .w_full()
                                    .h(px(34.0))
                                    .flex()
                                    .overflow_hidden()
                                    .rounded_lg()
                                    .bg(rgb(0xECEDF1))
                                    .child(
                                        div()
                                            .absolute()
                                            .left(px(-(timeline_scroll as f32)))
                                            .top_0()
                                            .w(px(timeline_content_width as f32))
                                            .h_full()
                                            .flex()
                                            .children(clip_lane)
                                            .child(
                                                div()
                                                    .absolute()
                                                    .left(px((timeline_content_width * progress)
                                                        as f32
                                                        - 1.0))
                                                    .top_0()
                                                    .w(px(2.0))
                                                    .h_full()
                                                    .bg(hsla(222.0 / 360.0, 0.2, 0.15, 0.85)),
                                            )
                                            .when_some(
                                                move_ghost,
                                                |this, (ghost_left, ghost_width)| {
                                                    this.child(
                                                        div()
                                                            .absolute()
                                                            .left(px(ghost_left))
                                                            .top_0()
                                                            .h_full()
                                                            .w(px(ghost_width))
                                                            .rounded_md()
                                                            .border_2()
                                                            .border_color(hsla(
                                                                222.0 / 360.0,
                                                                0.2,
                                                                0.15,
                                                                0.8,
                                                            ))
                                                            .bg(hsla(
                                                                217.0 / 360.0,
                                                                0.9,
                                                                0.6,
                                                                0.35,
                                                            )),
                                                    )
                                                },
                                            ),
                                    )
                                    .child(
                                        canvas(
                                            move |bounds, window, _| {
                                                if let Ok(mut stored) = timeline_bounds.lock() {
                                                    // Lanes are laid out with the
                                                    // previous width; redraw once
                                                    // the real width is known.
                                                    if *stored != Some(bounds) {
                                                        window.refresh();
                                                    }
                                                    *stored = Some(bounds);
                                                }
                                            },
                                            |_, _, _, _| {},
                                        )
                                        .absolute()
                                        .size_full(),
                                    )
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, event: &MouseDownEvent, _, cx| {
                                            this.timeline_seek_down(event);
                                            cx.notify();
                                        }),
                                    )
                                    .on_scroll_wheel(cx.listener(
                                        |this, event: &ScrollWheelEvent, _, cx| {
                                            let delta = match event.delta {
                                                ScrollDelta::Pixels(delta) => (
                                                    (delta.x / px(1.0)) as f64,
                                                    (delta.y / px(1.0)) as f64,
                                                ),
                                                ScrollDelta::Lines(delta) => {
                                                    (delta.x as f64 * 16.0, delta.y as f64 * 16.0)
                                                }
                                            };
                                            if event.modifiers.control || event.modifiers.platform {
                                                let factor = 2_f64.powf(delta.1 / 220.0);
                                                this.zoom_video_timeline(
                                                    factor,
                                                    this.video_position,
                                                );
                                            } else {
                                                let pan = if delta.0.abs() > delta.1.abs() {
                                                    -delta.0
                                                } else {
                                                    -delta.1
                                                };
                                                this.pan_video_timeline(pan);
                                            }
                                            cx.stop_propagation();
                                            cx.notify();
                                        },
                                    )),
                            )
                            .child(motion_track)
                            .when_some(annotation_track, |this, lane| this.child(lane))
                            .when_some(camera_lane, |this, lane| this.child(lane))
                            .when_some(audio_lane, |this, lane| this.child(lane))
                            // Paint one continuous playhead above every lane and gap.
                            .child(
                                div()
                                    .absolute()
                                    .left(px((timeline_content_width * progress - timeline_scroll) as f32 - 1.0))
                                    .top(px(15.0))
                                    .bottom_0()
                                    .w(px(2.0))
                                    .bg(ink()),
                            ),
                            ),
                    ),
            )
            .into_any_element()
    }

    /// The recording's clips on the seek bar: draggable to reorder, with
    /// trim handles on the selected clip.
    fn video_clip_lane(
        &self,
        timeline_duration: f64,
        timeline_content_width: f64,
        selected_clip: Option<uuid::Uuid>,
        move_drag: Option<VideoMoveDrag>,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        self.video_clip_timeline
            .segments
            .iter()
            .flat_map(|clip| {
                let clip_id = clip.id;
                let dragging = move_drag.is_some_and(|drag| drag.clip_id == clip_id);
                let width = (clip.editor_duration() / timeline_duration * timeline_content_width)
                    .max(3.0) as f32;
                // A clip's leading gap renders as an empty stretch of track.
                let spacer = (clip.gap_before > 0.0).then(|| {
                    div()
                        .h_full()
                        .w(px(
                            (clip.gap_before / timeline_duration * timeline_content_width) as f32,
                        ))
                        .flex_none()
                        .into_any_element()
                });
                let clip_element = div()
                    .id(("video-clip", clip_id.as_u128() as u64))
                    .h_full()
                    .w(px(width))
                    .flex_none()
                    .when(dragging, |this| this.opacity(0.4))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            // Arm a potential reorder drag; the seek-bar's own
                            // mouse-down still runs (no stop_propagation) so a
                            // plain click keeps moving the playhead.
                            this.video_move_drag = Some(VideoMoveDrag {
                                clip_id,
                                start_x: event.position.x,
                                current_x: event.position.x,
                                active: false,
                            });
                            this.video_selected_clip = Some(clip_id);
                            this.video_selected_zoom_cue = None;
                            cx.notify();
                        }),
                    )
                    // All clips share one bright fill (they come from the
                    // same source recording); a gap is just bare track.
                    // Selection is shown by a light ring alone.
                    .rounded_md()
                    .border_2()
                    .border_color(if selected_clip == Some(clip_id) {
                        hsla(222.0 / 360.0, 0.2, 0.15, 1.0)
                    } else {
                        hsla(0.0, 0.0, 0.0, 0.0)
                    })
                    .bg(hsla(217.0 / 360.0, 0.86, 0.58, 1.0))
                    .relative()
                    .flex()
                    .items_center()
                    .justify_center()
                    .overflow_hidden()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(hsla(0.0, 0.0, 1.0, 0.92))
                    .children(self.clip_thumbnails(
                        clip.source_start,
                        clip.source_end,
                        clip.speed,
                        width,
                        34.0,
                    ))
                    .when(width >= 52.0, |this| {
                        let label = if (clip.speed - 1.0).abs() > f64::EPSILON {
                            format!("{:.1}s · {}×", clip.editor_duration(), clip.speed)
                        } else {
                            format!("{:.1}s", clip.editor_duration())
                        };
                        this.child(
                            div()
                                .px_2()
                                .rounded_md()
                                .bg(hsla(0.0, 0.0, 0.0, 0.35))
                                .child(label),
                        )
                    })
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        // Selection only: the seek bar's mouse-down already
                        // moved the playhead to the clicked spot; seeking to
                        // the clip head here would yank the playhead back.
                        this.video_selected_clip = Some(clip_id);
                        this.video_selected_zoom_cue = None;
                        cx.notify();
                    }))
                    .when(selected_clip == Some(clip_id), |this| {
                        this.child(
                            div()
                                .id(("video-trim-leading", clip_id.as_u128() as u64))
                                .absolute()
                                .left_0()
                                .top_0()
                                .w(px(10.0))
                                .h_full()
                                .bg(hsla(0.0, 0.0, 1.0, 0.5))
                                .cursor(CursorStyle::ResizeLeftRight)
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                        cx.stop_propagation();
                                        this.begin_video_trim(
                                            clip_id,
                                            ClipEdge::Leading,
                                            event.position.x,
                                        );
                                        cx.notify();
                                    }),
                                ),
                        )
                        .child(
                            div()
                                .id(("video-trim-trailing", clip_id.as_u128() as u64))
                                .absolute()
                                .right_0()
                                .top_0()
                                .w(px(10.0))
                                .h_full()
                                .bg(hsla(0.0, 0.0, 1.0, 0.5))
                                .cursor(CursorStyle::ResizeLeftRight)
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                        cx.stop_propagation();
                                        this.begin_video_trim(
                                            clip_id,
                                            ClipEdge::Trailing,
                                            event.position.x,
                                        );
                                        cx.notify();
                                    }),
                                ),
                        )
                    })
                    .into_any_element();
                spacer.into_iter().chain(std::iter::once(clip_element))
            })
            .collect()
    }
}
