//! The editor shell shared by the static, motion, and video modes: one top
//! bar (mode switcher, undo/redo, export), one tabbed inspector, and one
//! timeline bar. Mode-specific code only supplies the canvas and the lanes.

use gpui::{
    canvas, div, hsla, prelude::*, px, rgb, svg, AnyElement, Context, CursorStyle, FontWeight,
    Hsla, MouseButton, MouseDownEvent, Pixels, ScrollDelta, ScrollWheelEvent, Size,
    Window,
};

use crate::{
    blue,
    controls::SliderTarget,
    ink, line,
    motion_ui::{ANIMATION_DURATIONS, BORDER_COLORS},
    muted, panel,
    recording::{
        clips::ClipEdge, scene::WindowFrame, viewport::MotionPreset,
    },
    scene_ui::SceneSelection,
    timestamped_export_name, Studio, Tool, VideoMoveDrag,
};

pub(crate) const INSPECTOR_WIDTH: f32 = 404.0;
pub(crate) const TOP_BAR_HEIGHT: f32 = 60.0;
const CANVAS_PADDING: f32 = 40.0;
const TIMELINE_CONTROLS_HEIGHT: f32 = 78.0;
const SCENE_STRIP_HEIGHT: f32 = 42.0;
const TIMELINE_LANES_HEIGHT: f32 = 64.0;

pub(crate) const EDITOR_MIN_WIDTH: f32 = 980.0;
pub(crate) const EDITOR_MIN_HEIGHT: f32 = 680.0;

/// Keep the complete editor reachable when the compositor gives us less space
/// than the layout's minimum size (for example, a small or tiled window).
pub(crate) fn editor_viewport(content: AnyElement) -> AnyElement {
    div()
        .id("editor-viewport")
        .size_full()
        .overflow_scroll()
        .child(content)
        .into_any_element()
}

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
    Camera,
    Cursor,
    Audio,
    Record,
    Export,
}

impl InspectorTab {
    const ALL: [InspectorTab; 8] = [
        InspectorTab::Design,
        InspectorTab::Camera,
        InspectorTab::Cursor,
        InspectorTab::Motion,
        InspectorTab::Audio,
        InspectorTab::Record,
        InspectorTab::Annotate,
        InspectorTab::Export,
    ];

    fn label(self) -> &'static str {
        match self {
            InspectorTab::Design => "General",
            InspectorTab::Annotate => "Annotate",
            InspectorTab::Motion => "Motion",
            InspectorTab::Camera => "Camera",
            InspectorTab::Cursor => "Cursor",
            InspectorTab::Audio => "Audio",
            InspectorTab::Record => "Captions",
            InspectorTab::Export => "Export",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            InspectorTab::Design => "icons/palette.svg",
            InspectorTab::Annotate => "icons/pen.svg",
            InspectorTab::Motion => "icons/film.svg",
            InspectorTab::Camera => "icons/video.svg",
            InspectorTab::Cursor => "icons/mouse-pointer.svg",
            InspectorTab::Audio => "icons/volume.svg",
            InspectorTab::Record => "icons/text.svg",
            InspectorTab::Export => "icons/upload.svg",
        }
    }

    fn available(self, mode: EditorMode) -> bool {
        match self {
            InspectorTab::Design | InspectorTab::Annotate => true,
            InspectorTab::Motion | InspectorTab::Export => mode != EditorMode::Static,
            InspectorTab::Cursor => mode != EditorMode::Static,
            InspectorTab::Record | InspectorTab::Camera | InspectorTab::Audio => mode == EditorMode::Video,
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
                    self.start_loading(crate::loading::LoadRequest::Recording(directory), false, cx);
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

    pub(crate) fn undo_current(&mut self, cx: &mut Context<Self>) {
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
                }
            }
        }
    }

    pub(crate) fn redo_current(&mut self, cx: &mut Context<Self>) {
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
                        Ok(()) => crate::notifications::Notification::exported(
                            format!("Exported to {}", path.display()), path,
                        ),
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

    fn timeline_bar_height(&self) -> f32 {
        if self.editor_mode() == EditorMode::Static {
            0.0
        } else {
            TIMELINE_CONTROLS_HEIGHT
                + TIMELINE_LANES_HEIGHT
                + self.timeline_ui.lane_height
                + 1.0
                + if self.editor_mode() == EditorMode::Motion {
                    SCENE_STRIP_HEIGHT
                } else {
                    0.0
                }
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
        let width = (viewport.width.max(px(EDITOR_MIN_WIDTH))
            - px(CANVAS_PADDING * 2.0 + inspector)).max(px(1.0));
        let height = (viewport.height.max(px(EDITOR_MIN_HEIGHT))
            - px(TOP_BAR_HEIGHT + CANVAS_PADDING * 2.0 + self.timeline_bar_height()))
        .max(px(1.0));
        self.preview_canvas_size(width, height)
    }

    /// The canvas workspace with its export status overlay.
    pub(crate) fn canvas_area(&self, canvas: AnyElement, cx: &mut Context<Self>) -> AnyElement {
        let export_overlay = self.export_status_overlay(cx);
        div()
            .relative()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .p(px(CANVAS_PADDING))
            .flex()
            .items_center()
            .justify_center()
            .bg(rgb(0xe9ebef))
            .child(div().flex_none().shadow_lg().child(canvas))
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
        let crop_pixel_size = self.media_dimensions().map(|(width, height)| {
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
                    this.reset_crop();
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
                    .child(crate::brand_wordmark_latin(110.0, 22.0))
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
                this.child(
                    div()
                        .id("bar-record-new")
                        .ml_2()
                        .px_3()
                        .h(px(32.0))
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap_2()
                        .rounded_md()
                        .bg(rgb(0xf0f1f3))
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(ink())
                        .cursor_pointer()
                        .hover(|style| style.bg(rgb(0xe5e7eb)))
                        .child(
                            svg()
                                .path("icons/record.svg")
                                .size(px(17.0))
                                .text_color(ink()),
                        )
                        .child("New capture")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.open_recorder_window(cx);
                            cx.notify();
                        })),
                )
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
                .when(mode == EditorMode::Static || mode == EditorMode::Video, |this| {
                    this.child(self.bar_button(
                        "bar-crop",
                        "icons/crop.svg",
                        Some("Crop"),
                        if mode == EditorMode::Video { self.video_frame.is_some() && !self.video_edit_busy } else { self.captured_path.is_some() },
                        cx,
                        |this, _, cx| {
                            this.stop_editing_text();
                            this.begin_crop();
                            if this.video_project.is_some() && this.crop_active {
                                this.seek_video(this.video_position, cx);
                            }
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
                                    if this.editor_mode() == EditorMode::Static {
                                        this.export_current(cx);
                                    } else {
                                        this.select_inspector_tab(InspectorTab::Export);
                                        this.inspector_visible = true;
                                    }
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
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .when(self.crop_active, |this| this.flex_none())
                    .child(left)
                    .child(drag_region("top-bar-drag-left")),
            )
            .child(if self.crop_active {
                self.crop_controls(cx)
            } else {
                self.mode_switcher(cx)
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .justify_end()
                    .child(drag_region("top-bar-drag-right"))
                    .child(right),
            )
            .into_any_element()
    }

    // ------------------------------------------------------------------
    // Inspector
    // ------------------------------------------------------------------

    /// The tab the inspector shows: a selection on the canvas or timeline
    /// pulls its own tab forward until it is done.
    pub(crate) fn effective_tab(&self) -> InspectorTab {
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

    pub(crate) fn select_inspector_tab(&mut self, tab: InspectorTab) {
        self.annotation_editing_time = None;
        self.annotation_edit_preview_pending = false;
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
        self.inspector_scroll.set_offset(gpui::point(px(0.0), px(0.0)));
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
    pub(crate) fn section_header(
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
            .mt_2()
            .text_sm()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(ink())
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
        if let Some(transform) = transform {
            return transform;
        }
        let wallpaper_tab = self.wallpaper_tab;
        let border = self.border;
        let watermark = self.watermark_enabled;
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(Self::tab_label("Canvas"))
            .child(div().text_xs().text_color(muted()).child("Aspect ratio"))
            .child(self.canvas_aspect_picker(cx))
            .when(self.video_project.is_some() && self.aspect_ratio != 0, |this| {
                this.child(self.segmented(
                    "content-fill",
                    &["Fit", "Fill"],
                    usize::from(self.content_fill),
                    |this, value| this.content_fill = value == 1,
                    cx,
                ))
                .child(div().text_xs().text_color(muted()).child(if self.content_fill {
                    "Crops the recording to the canvas; the camera follows your cursor."
                } else {
                    "Shows the whole recording on the background."
                }))
            })
            .child(self.slider_row(
                SliderTarget::Padding,
                "Padding",
                self.padding,
                "%",
                |this, value| this.padding = value,
                cx,
            ))
            .child(Self::tab_label("Background"))
            .child(self.segmented(
                "fill-type",
                &["Color", "Gradient", "Wallpaper"],
                wallpaper_tab,
                |this, value| this.wallpaper_tab = value,
                cx,
            ))
            .child(self.fill_picker(cx))
            .when(wallpaper_tab == 2, |this| {
                this.child(self.section_header(
                    "wallpaper-browser",
                    "Browse all wallpapers",
                    None,
                    cx,
                ))
            })
            .child(self.section_header("effects", "Background effects", None, cx))
            .when(self.section_open("effects"), |this| {
                this.child(self.background_effects_section(cx))
            })
            .child(div().h(px(4.0)))
            .child(Self::tab_label("Image appearance"))
            .child(self.slider_row(
                SliderTarget::Corners,
                "Corners",
                self.corners,
                "%",
                |this, value| this.corners = value,
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
                    SliderTarget::BorderThickness,
                    "Thickness",
                    self.border_thickness,
                    "",
                    |this, value| this.border_thickness = value,
                    cx,
                ))
                .child(self.slider_row(
                    SliderTarget::BorderOpacity,
                    "Opacity",
                    self.border_opacity,
                    "%",
                    |this, value| this.border_opacity = value,
                    cx,
                ))
            })
            .child(Self::tab_label("Shadow"))
            .child(self.slider_row(
                SliderTarget::Shadow,
                "Amount",
                self.shadow,
                "%",
                |this, value| this.shadow = value,
                cx,
            ))
            .child(self.shadow_color_picker(cx))
            .child(div().text_xs().text_color(muted()).child("Style"))
            .child(self.segmented(
                "shadow-style",
                &["Soft", "Long", "Glow", "Crisp"],
                self.shadow_style,
                |this, value| this.shadow_style = value,
                cx,
            ))
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

    pub(crate) fn canvas_aspect_picker(&self, cx: &mut Context<Self>) -> AnyElement {
        let presets = [
            ("Auto", 0, 1.6), ("16:9", 4, 16.0 / 9.0), ("9:16", 5, 9.0 / 16.0),
            ("1:1", 1, 1.0), ("4:5", 6, 4.0 / 5.0), ("4:3", 2, 4.0 / 3.0),
            ("3:2", 3, 1.5), ("3:4", 7, 0.75), ("2:3", 8, 2.0 / 3.0),
            ("21:9", 9, 21.0 / 9.0), ("Custom", 10, 1.4),
        ];
        div().flex().flex_col().gap_2()
            .child(div().grid().grid_cols(3).gap_2().children(presets.into_iter().map(|(label, index, ratio)| {
                let active = self.aspect_ratio == index;
                let width = 32.0_f32.min(22.0 * ratio);
                let height = width / ratio;
                div().id(("canvas-aspect", index)).h(px(64.0)).rounded_lg()
                    .border_1().border_color(if active { blue() } else { line() })
                    .bg(if active { rgb(0xe7f1ff) } else { rgb(0xffffff) })
                    .flex().flex_col().items_center().justify_center().gap_1()
                    .cursor_pointer().hover(|style| style.bg(rgb(0xeef4fb)))
                    .child(div().w(px(width)).h(px(height)).rounded(px(3.0))
                        .border_1().border_color(if active { blue() } else { muted() }))
                    .child(div().text_xs().text_color(if active { blue() } else { ink() }).child(label))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.aspect_ratio = index;
                        if index == 10 { this.custom_canvas_draft = this.custom_canvas_dimensions; }
                        cx.notify();
                    }))
            })))
            .when(self.aspect_ratio == 10, |this| {
                this.child(div().flex().items_center().gap_2()
                    .child(div().flex_1().min_w_0().child(self.text_fields.aspect_width.clone()))
                    .child(div().text_sm().text_color(muted()).child(":"))
                    .child(div().flex_1().min_w_0().child(self.text_fields.aspect_height.clone())))
                    .child(div().text_xs().text_color(muted()).child("Enter width and height proportions, then press Enter."))
            })
            .into_any_element()
    }

    fn annotate_tab(&self, cx: &mut Context<Self>) -> AnyElement {
        let selected = self
            .selected_annotation
            .filter(|index| *index < self.annotations.len());
        let selected_indices = self.annotation_selected_indices();
        let selection_count = selected_indices.len();
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(self.video_annotate_section(cx))
            .when(selection_count > 1, |this| {
                this.child(
                    div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .child(div().text_xs().text_color(muted()).child(format!("{selection_count} selected")))
                        .child(self.small_button(
                            "annotation-delete-selection",
                            if selection_count == self.annotations.len() { "Delete all" } else { "Delete selected" },
                            true,
                            cx,
                            |this, _| {
                                if this.delete_selected_annotations() && this.processed_capture_path.is_some() {
                                    if let Err(error) = this.rebuild_redactions() {
                                        this.toast = Some(error.into());
                                    }
                                }
                            },
                        )),
                )
            })
            .when(selected.is_none() && self.tool != Tool::Select, |this| {
                this.child(
                    div()
                        .flex()
                        .flex_none()
                        .flex_col()
                        .gap_2()
                        .child(Self::tab_label(self.tool.label()))
                        .child(self.annotation_style_controls(cx)),
                )
            })
            .children(self.annotations.iter().enumerate().map(|(index, mark)| {
                let open = selected == Some(index);
                let title = if mark.tool == Tool::Text && !mark.text.trim().is_empty() {
                    mark.text.split_whitespace().collect::<Vec<_>>().join(" ")
                } else {
                    format!("{} {}", mark.tool.label(), index + 1)
                };
                div()
                    .id(("annotation-card", index))
                    .flex()
                    .flex_none()
                    .flex_col()
                    .w_full()
                    .min_w_0()
                    .child(
                        div()
                            .flex()
                            .flex_none()
                            .items_center()
                            .gap_1()
                            .rounded_md()
                            .bg(if selected_indices.contains(&index) { rgb(0xe7f1ff) } else { rgb(0xf0f1f3) })
                            .child(
                                div()
                                    .id(("annotation-list", index))
                                    .anchor_scroll(
                                        open.then(|| self.annotation_inspector_anchor.clone()),
                                    )
                                    .flex()
                                    .flex_1()
                                    .min_w_0()
                                    .items_center()
                                    .gap_2()
                                    .h(px(40.0))
                                    .px_2()
                                    .cursor_pointer()
                                    .child(
                                        svg()
                                            .path(if open {
                                                "icons/chevron-down.svg"
                                            } else {
                                                "icons/chevron-right.svg"
                                            })
                                            .size(px(14.0))
                                            .flex_none()
                                            .text_color(muted()),
                                    )
                                    .child(
                                        svg()
                                            .path(
                                                Tool::ALL
                                                    .iter()
                                                    .find(|(tool, _)| *tool == mark.tool)
                                                    .map_or("icons/image.svg", |(_, icon)| *icon),
                                            )
                                            .size(px(16.0))
                                            .flex_none()
                                            .text_color(ink()),
                                    )
                                    .child(
                                        div().flex_1().min_w_0().text_sm().truncate().child(title),
                                    )
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        if this.selected_annotation == Some(index) {
                                            this.finish_annotation_interaction();
                                        } else {
                                            this.select_annotation_for_timing(index);
                                            this.annotation_inspector_anchor.scroll_to(window, cx);
                                        }
                                        this.tool = Tool::Select;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                div()
                                    .id(("annotation-row-delete", index))
                                    .flex()
                                    .flex_none()
                                    .items_center()
                                    .justify_center()
                                    .size(px(32.0))
                                    .mr_1()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .hover(|style| style.bg(rgb(0xfee2e2)))
                                    .child(
                                        svg()
                                            .path("icons/trash.svg")
                                            .size(px(14.0))
                                            .text_color(muted()),
                                    )
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.select_annotation_for_timing(index);
                                        if this.delete_selected_annotations()
                                            && this.processed_capture_path.is_some()
                                        {
                                            if let Err(error) = this.rebuild_redactions() {
                                                this.toast = Some(error.into());
                                            }
                                        }
                                        this.tool = Tool::Select;
                                        cx.notify();
                                    })),
                            ),
                    )
                    .when(open, |this| {
                        this.child(
                            div()
                                .flex()
                                .flex_none()
                                .flex_col()
                                .gap_3()
                                .py_3()
                                .child(self.annotation_style_controls(cx))
                                .when_some(self.annotation_timing_inspector(cx), |this, panel| {
                                    this.child(panel)
                                }),
                        )
                    })
            }))
            .into_any_element()
    }

    /// Animated-screenshot settings: duration, motion preset, scenes, and
    /// the cursor walkthrough.
    fn animation_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let duration_index = ANIMATION_DURATIONS
            .iter()
            .position(|value| (*value - self.animation_duration).abs() < 1e-9)
            .unwrap_or(usize::MAX);
        let selected_preset = self.animation_preset.filter(|_| !self.video_zoom_cues.is_empty());
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(Self::tab_label("Scene duration"))
            .child(self.segmented(
                "animation-duration",
                &["3 s", "5 s", "8 s", "10 s"],
                duration_index,
                |this, index| this.set_animation_duration(ANIMATION_DURATIONS[index]),
                cx,
            ))
            .child(div().text_xs().text_color(muted()).child(format!(
                "Image: {:.2}s – {:.2}s · Drag either edge to trim.",
                self.image_trim_drag.map(|drag| drag.start).unwrap_or(self.animation_image_start),
                self.image_trim_drag.map(|drag| drag.end).unwrap_or(self.animation_image_end))))
            .child(div().flex().items_center().justify_between()
                .child(div().text_xs().child(format!("Scene length: {:.2}s", self.animation_duration)))
                .child(self.small_button("extend-scene", "+ 5 seconds", true, cx, |this, _| {
                    this.set_animation_duration(this.animation_duration + 5.0);
                })))
            .child(Self::tab_label("Motion preset"))
            .child(
                div().text_xs().text_color(muted()).child(
                    selected_preset
                        .map(|preset| preset.label())
                        .unwrap_or(if self.video_zoom_cues.is_empty() {
                            "No motion applied"
                        } else {
                            "Custom motion"
                        }),
                ),
            )
            .child(
                div().flex().flex_wrap().gap(px(6.0)).children(
                    MotionPreset::ALL
                        .into_iter()
                        .enumerate()
                        .filter(|(index, preset)| {
                            self.section_open("motion-presets")
                                || *index < 3
                                || selected_preset == Some(*preset)
                        })
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
                                    blue()
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
                                        .text_color(if selected { blue() } else { ink() })
                                        .child(preset.label()),
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.apply_motion_preset(preset);
                                    cx.notify();
                                }))
                        }),
                ),
            )
            .child(self.section_header("motion-presets", "Browse all motion presets", None, cx))
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
                                blue()
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

    fn scene_strip(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .items_center()
            .gap_3()
            .child(div().text_xs().text_color(muted()).child("Scenes"))
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
            .into_any_element()
    }

    fn motion_tab(&self, cx: &mut Context<Self>) -> AnyElement {
        if let Some(region) = self.motion_inspector(cx) {
            return region;
        }
        if let Some(transform) = self.transform_inspector(cx) {
            return transform;
        }
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(self.motion_overview_section(cx))
            .when(self.editor_mode() == EditorMode::Motion, |this| this
                .child(self.section_header("scene-animation", "Scene animation", None, cx))
                .when(self.section_open("scene-animation"), |this| this.child(self.animation_settings(cx))))
            .child(self.section_header("templates", "Browse templates", None, cx))
            .when(self.section_open("templates"), |this| {
                this.child(self.template_gallery_section(cx))
            })
            .into_any_element()
    }

    fn record_tab(&self, cx: &mut Context<Self>) -> AnyElement {
        if self.transcript_open && self.video_transcript.is_some() {
            return self.transcript_panel(cx);
        }
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(Self::tab_label("Subtitles & transcript"))
            .child(self.subtitle_section(cx))
            .child(self.section_header("keystrokes", "Keystrokes", None, cx))
            .when(self.section_open("keystrokes"), |this| {
                this.child(self.keystroke_section(cx))
            })
            .child(self.section_header("speed-badge", "Speed badge", None, cx))
            .when(self.section_open("speed-badge"), |this| {
                this.child(self.speed_badge_section(cx))
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
            InspectorTab::Camera => self.camera_section(cx),
            InspectorTab::Cursor => self.pointer_section(cx),
            InspectorTab::Audio => self.audio_section(cx),
            InspectorTab::Record => self.record_tab(cx),
            InspectorTab::Export => div()
                .flex()
                .flex_col()
                .gap_3()
                .child(Self::tab_label("Export settings"))
                .child(self.export_section(cx))
                .child(
                    div()
                        .id("export-start")
                        .h(px(36.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_md()
                        .bg(blue())
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(rgb(0xffffff))
                        .opacity(if self.export_progress.is_some() {
                            0.5
                        } else {
                            1.0
                        })
                        .child(if self.export_progress.is_some() {
                            "Exporting…"
                        } else {
                            "Export file…"
                        })
                        .when(self.export_progress.is_none(), |this| {
                            this.cursor_pointer()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.export_current(cx);
                                    cx.notify();
                                }))
                        }),
                )
                .when(self.video_project.is_some(), |this| {
                    this.child(Self::tab_label("Share"))
                        .child(self.share_section(cx))
                })
                .into_any_element(),
        };
        let tabs = div()
            .id("inspector-navigation")
            .w(px(64.0))
            .h_full()
            .flex_none()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .px_1()
            .py_2()
            .gap_1()
            .border_r_1()
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
                            .flex_none()
                            .h(px(58.0))
                            .flex()
                            .flex_col()
                            .items_center()
                            .justify_center()
                            .gap_1()
                            .rounded_md()
                            .cursor_pointer()
                            .when(selected, |this| this.bg(rgb(0xe7f1ff)))
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
            .bg(panel())
            .border_l_1()
            .border_color(line())
            .child(tabs)
            .child(
                div()
                    .flex_1().min_w_0().h_full().flex().flex_col()
                    .child(div().h(px(52.0)).flex_none().px_4().flex().items_center()
                        .border_b_1().border_color(line()).text_sm().font_weight(FontWeight::SEMIBOLD)
                        .child(tab.label()))
                    .child(div()
                    .id("inspector-scroll")
                    .track_scroll(&self.inspector_scroll)
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_4()
                    .flex()
                    .flex_col()
                    .child(div().flex_none().w_full().child(body))
                    .child(div().flex_none().mt_5().child(crate::theme::credit_footer()))),
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

    pub(crate) fn toggle_timeline_playback(&mut self, cx: &mut Context<Self>) {
        if self.video_project.is_some() {
            if self.video_playing || self.video_audio_play_when_ready {
                self.pause_video_playback();
            } else {
                self.start_video_playback(cx);
            }
        } else {
            self.toggle_animation_playback(cx);
        }
    }

    /// Seeks to the pointer's position on the ruler or the clip lane and
    /// arms a scrub drag.
    pub(crate) fn timeline_seek_down(&mut self, event: &MouseDownEvent, cx: &mut Context<Self>) {
        self.finish_annotation_interaction();
        self.pause_video_playback();
        self.video_trim_drag = None;
        self.video_zoom_drag = None;
        let target = self.motion_timeline_time_at(event.position.x)
            .unwrap_or(self.video_position);
        self.scrub_video_to(target, cx);
        self.video_seek_drag = Some((event.position.x, target));
    }

    /// The timeline bar shared by the motion and video modes: transport and
    /// edit controls, the ruler, the clip lane, then the motion, annotation,
    /// camera, and audio lanes.
    fn track_label(label: &'static str, height: f32) -> AnyElement {
        div()
            .h(px(height))
            .flex_none()
            .flex()
            .items_center()
            .text_xs()
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(muted())
            .child(label)
            .into_any_element()
    }

    pub(crate) fn timeline_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let is_video = self.video_project.is_some();
        let progress = if self.video_duration > 0.0 {
            (self.video_position / self.video_timeline_duration()).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let timeline_duration = self.video_timeline_duration();
        let timeline_scroll = self.video_timeline_scroll;
        let timeline_content_width = self.video_timeline_content_width();
        let timeline_bounds = self.video_timeline_bounds.clone();
        let selected_clip = self.video_selected_clip;
        let move_drag = self.video_move_drag.filter(|drag| drag.active);
        // While dragging, the clip's ghost follows the pointer (snapped the
        // same way the drop will land) so the destination — including a gap
        // past the end of the timeline — is always visible.
        let move_ghosts: Vec<AnyElement> = move_drag
            .into_iter()
            .flat_map(|drag| {
                let Some((resolved, _)) = self.resolved_video_move_preview(&drag) else {
                    return Vec::new();
                };
                let scale = timeline_content_width / timeline_duration;
                resolved
                    .segments
                    .iter()
                    .filter_map(|clip| {
                        let range = resolved.editor_range(clip.id)?;
                        let old = self.video_clip_timeline.editor_range(clip.id)?;
                        if clip.id != drag.clip_id && (range.start - old.start).abs() < 1e-6 {
                            return None;
                        }
                        Some(
                            div()
                                .absolute()
                                .left(px((range.start * scale) as f32))
                                .top_0()
                                .h_full()
                                .w(px(((range.end - range.start) * scale).max(3.0) as f32))
                                .rounded_md()
                                .border_2()
                                .border_color(blue())
                                .bg(hsla(
                                    217.0 / 360.0,
                                    0.9,
                                    0.6,
                                    if clip.id == drag.clip_id { 0.35 } else { 0.16 },
                                ))
                                .into_any_element(),
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .collect();
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
        let visible_start = timeline_scroll * self.video_timeline_seconds_per_pixel();
        let visible_end = visible_start
            + self.video_timeline_viewport_width() * self.video_timeline_seconds_per_pixel();
        let mut ruler_tick = (visible_start / ruler_step).floor() * ruler_step;
        while ruler_tick <= timeline_duration.min(visible_end + ruler_step) + 1e-6 {
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
                crate::timeline_edit::precise_timecode(ruler_tick)
            } else {
                Self::video_timecode(ruler_tick)
            };
            ruler_marks.push(
                div()
                    .absolute()
                    .left(px(x + 5.0))
                    .top(px(19.0))
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
        let camera_lane = self.camera_region_lane(timeline_scroll, timeline_content_width, progress, cx);
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
            let image_start = self
                .image_trim_drag
                .map(|drag| drag.start)
                .unwrap_or(self.animation_image_start);
            let image_end = self
                .image_trim_drag
                .map(|drag| drag.end)
                .unwrap_or(self.animation_image_end)
                .min(self.video_duration);
            let image_duration = (image_end - image_start).max(0.0);
            let label = if self.image_scenes.len() > 1 {
                format!(
                    "Scene {} · {:.2}s",
                    self.image_scene_index + 1,
                    image_duration
                )
            } else {
                format!("Image · {:.2}s", image_duration)
            };
            if image_duration <= 0.0 {
                Vec::new()
            } else {
                vec![div()
                    .id("image-timeline-clip")
                    .cursor(CursorStyle::ClosedHand)
                    .absolute()
                    .left(px(
                        (timeline_content_width * image_start / timeline_duration) as f32,
                    ))
                    .overflow_hidden()
                    .h_full()
                    .w(px(
                        (timeline_content_width * image_duration / timeline_duration).max(32.0)
                            as f32,
                    ))
                    .flex_none()
                    .rounded_md()
                    .border_2()
                    .border_color(if self.video_selected_clip.is_some() {
                        ink()
                    } else {
                        hsla(0.0, 0.0, 0.0, 0.0)
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, event: &MouseDownEvent, window, cx| {
                            this.focus_handle.focus(window);
                            this.timeline_seek_down(event, cx);
                            this.begin_image_move(event.position.x);
                            this.video_selected_zoom_cue = None;
                            this.selected_annotation = None;
                            this.scene_selection = SceneSelection::Scene;
                            this.video_selected_clip = this
                                .video_clip_timeline
                                .segments
                                .first()
                                .map(|clip| clip.id);
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    )
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
                    .child(
                        div()
                            .id("image-trim-start")
                            .absolute()
                            .left_0()
                            .top_0()
                            .h_full()
                            .w(px(14.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor(CursorStyle::ResizeLeftRight)
                            .bg(hsla(0.0, 0.0, 0.0, 0.12))
                            .hover(|style| style.bg(hsla(0.0, 0.0, 0.0, 0.3)))
                            .child(div().w(px(3.0)).h(px(16.0)).rounded_sm().bg(rgb(0xffffff)))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                                    this.begin_image_trim(event.position.x, ClipEdge::Leading);
                                    cx.stop_propagation();
                                    cx.notify();
                                }),
                            ),
                    )
                    .child(
                        div()
                            .id("image-trim-end")
                            .absolute()
                            .right_0()
                            .top_0()
                            .h_full()
                            .w(px(14.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .cursor(CursorStyle::ResizeLeftRight)
                            .bg(hsla(0.0, 0.0, 0.0, 0.12))
                            .hover(|style| style.bg(hsla(0.0, 0.0, 0.0, 0.3)))
                            .child(div().w(px(3.0)).h(px(16.0)).rounded_sm().bg(rgb(0xffffff)))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                                    this.begin_image_trim(event.position.x, ClipEdge::Trailing);
                                    cx.stop_propagation();
                                    cx.notify();
                                }),
                            ),
                    )
                    .into_any_element()]
            }
        };
        let motion_track = self.motion_track(timeline_scroll, timeline_content_width, progress, cx);
        let annotation_track =
            self.annotation_track(timeline_scroll, timeline_content_width, progress, cx);
        let audio_height = if audio_lane.is_some() { 26.0 } else { 0.0 };
        let camera_height = if camera_lane.is_some() { 26.0 } else { 0.0 };
        let annotation_height = if annotation_track.is_some() {
            self.annotation_lane_height() + 4.0
        } else {
            0.0
        };
        let tracks_height = 48.0
            + 4.0
            + self.motion_lane_height()
            + audio_height
            + camera_height
            + annotation_height
            + if self.video_audio_clips.is_empty() {
                0.0
            } else {
                self.video_audio_clips.len() as f32 * 24.0 + 4.0
            };
        let vertical_overflow = tracks_height > self.timeline_ui.lane_height;
        let playhead_x = (timeline_content_width * progress - timeline_scroll) as f32;
        let expanded = self.timeline_ui.ruler_hover || self.video_seek_drag.is_some();
        let head_width = if expanded { 76.0 } else { 18.0 };
        let head_left = (playhead_x - head_width / 2.0).clamp(
            0.0,
            (self.video_timeline_viewport_width() as f32 - head_width).max(0.0),
        );
        let playhead_visible =
            playhead_x >= 0.0 && playhead_x <= self.video_timeline_viewport_width() as f32;
        let skim_x = self
            .timeline_skim_time()
            .map(|time| (time / self.video_timeline_seconds_per_pixel() - timeline_scroll) as f32);
        let snap_x = self
            .timeline_ui
            .snap_guide
            .map(|time| (time / self.video_timeline_seconds_per_pixel() - timeline_scroll) as f32);
        let context = self.timeline_context_menu(cx);
        let detail = self
            .timeline_drag_readout()
            .or_else(|| {
                self.motion_creation_times().map(|(a, b)| {
                    format!(
                        "{} – {} · {}",
                        crate::timeline_edit::precise_timecode(a),
                        crate::timeline_edit::precise_timecode(b),
                        crate::timeline_edit::precise_timecode(b - a)
                    )
                })
            })
            .or_else(|| {
                self.timeline_skim_time()
                    .map(|t| format!("Preview {}", crate::timeline_edit::precise_timecode(t)))
            });

        div()
            .relative()
            .capture_any_mouse_down(cx.listener(|this, _, window, _| {
                this.stop_editing_text();
                this.focus_handle.focus(window);
            }))
            .flex_none()
            .px_4()
            .flex()
            .flex_col()
            .bg(rgb(0xffffff))
            .border_t_1()
            .border_color(line())
            .child(self.timeline_resize_handle(cx))
            .when(!is_video, |this| {
                this.child(
                    div()
                        .id("timeline-scenes")
                        .h(px(SCENE_STRIP_HEIGHT))
                        .flex_none()
                        .overflow_x_scroll()
                        .flex()
                        .items_center()
                        .border_b_1()
                        .border_color(line())
                        .child(self.scene_strip(cx)),
                )
            })
            .child(self.timeline_controls(cx))
            .child(
                div()
                    .flex()
                    .flex_none()
                    .h(px(38.0))
                    .items_center()
                    .child(
                        div()
                            .w(px(104.0))
                            .flex_none()
                            .text_xs()
                            .text_color(muted())
                            .child("Time"),
                    )
                    .child(
                        div()
                            .id("timeline-ruler")
                            .relative()
                            .flex_1()
                            .h_full()
                            .overflow_hidden()
                            .cursor_pointer()
                            .on_hover(cx.listener(|this, hover: &bool, _, cx| {
                                this.timeline_ui.ruler_hover = *hover;
                                if !hover {
                                    this.update_timeline_skim(None, cx);
                                }
                                cx.notify();
                            }))
                            .on_mouse_move(cx.listener(
                                |this, event: &gpui::MouseMoveEvent, _, cx| {
                                    if !event.dragging() && this.timeline_ui.skim_enabled {
                                        this.update_timeline_skim(
                                            this.motion_timeline_time_at(event.position.x),
                                            cx,
                                        );
                                    }
                                },
                            ))
                            .on_scroll_wheel(cx.listener(
                                |this, event: &ScrollWheelEvent, _, cx| {
                                    let (dx, dy) = match event.delta {
                                        ScrollDelta::Pixels(d) => (f64::from(d.x), f64::from(d.y)),
                                        ScrollDelta::Lines(d) => {
                                            (d.x as f64 * 16.0, d.y as f64 * 16.0)
                                        }
                                    };
                                    if event.modifiers.control || event.modifiers.platform {
                                        let anchor = this
                                            .motion_timeline_time_at(event.position.x)
                                            .unwrap_or(this.video_position);
                                        this.zoom_video_timeline(2_f64.powf(dy / 220.0), anchor);
                                    } else {
                                        this.manually_pan_timeline(if dx.abs() > dy.abs() {
                                            -dx
                                        } else {
                                            -dy
                                        });
                                    }
                                    cx.stop_propagation();
                                    cx.notify();
                                },
                            ))
                            .child(
                                div()
                                    .absolute()
                                    .left(px(-(timeline_scroll as f32)))
                                    .w(px(timeline_content_width as f32))
                                    .h_full()
                                    .children(ruler_marks),
                            )
                            .when(playhead_visible, |this| {
                                this.child(
                                    div()
                                        .absolute()
                                        .left(px(head_left))
                                        .top(px(1.0))
                                        .w(px(head_width))
                                        .h(px(18.0))
                                        .rounded_sm()
                                        .bg(blue())
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .text_xs()
                                        .text_color(rgb(0xffffff))
                                        .child(if expanded {
                                            crate::timeline_edit::precise_timecode(
                                                self.video_position,
                                            )
                                        } else {
                                            "▾".to_owned()
                                        }),
                                )
                            })
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                                    this.timeline_seek_down(event, cx);
                                    cx.notify();
                                }),
                            ),
                    ),
            )
            .child(
                div()
                    .id("timeline-tracks-scroll")
                    .h(px(self.timeline_ui.lane_height))
                    .flex_none()
                    .overflow_y_scroll()
                    .track_scroll(&self.timeline_ui.vertical_scroll)
                    .on_scroll_wheel(cx.listener(move |this, event: &ScrollWheelEvent, _, cx| {
                        let (dx, dy) = match event.delta {
                            ScrollDelta::Pixels(d) => (f64::from(d.x), f64::from(d.y)),
                            ScrollDelta::Lines(d) => (d.x as f64 * 16.0, d.y as f64 * 16.0),
                        };
                        if event.modifiers.control || event.modifiers.platform {
                            let anchor = this
                                .motion_timeline_time_at(event.position.x)
                                .unwrap_or(this.video_position);
                            this.zoom_video_timeline(2_f64.powf(dy / 220.0), anchor);
                        } else if event.modifiers.shift || dx.abs() > dy.abs() || !vertical_overflow
                        {
                            this.manually_pan_timeline(if dx.abs() > dy.abs() { -dx } else { -dy });
                        } else {
                            return;
                        }
                        cx.stop_propagation();
                        cx.notify();
                    }))
                    .child(
                        div()
                            .flex()
                            .h(px(tracks_height))
                            .flex_none()
                            .child(
                                div()
                                    .w(px(104.0))
                                    .pr_3()
                                    .flex_none()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(Self::track_label(
                                        if is_video { "Video" } else { "Image" },
                                        48.0,
                                    ))
                                    .child(Self::track_label("Motion", self.motion_lane_height()))
                                    .when(annotation_track.is_some(), |d| {
                                        d.child(Self::track_label(
                                            "Annotations",
                                            self.annotation_lane_height(),
                                        ))
                                    })
                                    .when(camera_lane.is_some(), |d| {
                                        d.child(Self::track_label("Camera", 22.0))
                                    })
                                    .when(audio_lane.is_some(), |d| {
                                        d.child(Self::track_label("Audio", 22.0))
                                    })
                                    .when(!self.video_audio_clips.is_empty(), |d| {
                                        d.child(Self::track_label(
                                            "Music / SFX",
                                            self.video_audio_clips.len() as f32 * 24.0,
                                        ))
                                    }),
                            )
                            .child(
                                div()
                                    .id("timeline-track-content")
                                    .relative()
                                    .flex_1()
                                    .min_w(px(280.0))
                                    .h_full()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .overflow_hidden()
                                    .on_mouse_move(cx.listener(
                                        |this, event: &gpui::MouseMoveEvent, _, cx| {
                                            if !event.dragging() {
                                                let time =
                                                    this.motion_timeline_time_at(event.position.x);
                                                this.timeline_ui.hover_time = time;
                                                if this.timeline_ui.skim_enabled {
                                                    this.update_timeline_skim(time, cx);
                                                }
                                            }
                                        },
                                    ))
                                    .on_hover(cx.listener(|this, hover: &bool, _, cx| {
                                        if !hover {
                                            this.timeline_ui.hover_time = None;
                                            this.update_timeline_skim(None, cx);
                                        }
                                    }))
                                    .child(
                                        div()
                                            .id("timeline-seek-bar")
                                            .relative()
                                            .w_full()
                                            .h(px(48.0))
                                            .flex_none()
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
                                                    .children(move_ghosts),
                                            )
                                            .child(
                                                canvas(
                                                    move |bounds, window, _| {
                                                        if let Ok(mut stored) =
                                                            timeline_bounds.lock()
                                                        {
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
                                                cx.listener(
                                                    |this, event: &MouseDownEvent, _, cx| {
                                                        this.timeline_seek_down(event, cx);
                                                        cx.notify();
                                                    },
                                                ),
                                            ),
                                    )
                                    .child(motion_track)
                                    .when_some(annotation_track, |d, lane| d.child(lane))
                                    .when_some(camera_lane, |d, lane| d.child(lane))
                                    .when_some(audio_lane, |d, lane| d.child(lane))
                                    .when(!self.video_audio_clips.is_empty(), |d| {
                                        d.child(self.imported_audio_lane(
                                            timeline_content_width,
                                            timeline_scroll,
                                            cx,
                                        ))
                                    })
                                    .when_some(skim_x, |d, x| {
                                        d.child(
                                            div()
                                                .absolute()
                                                .left(px(x))
                                                .top_0()
                                                .bottom_0()
                                                .w(px(1.0))
                                                .bg(rgb(0x8b95a5)),
                                        )
                                    })
                                    .when_some(snap_x, |d, x| {
                                        d.child(
                                            div()
                                                .absolute()
                                                .left(px(x))
                                                .top_0()
                                                .bottom_0()
                                                .w(px(1.0))
                                                .bg(rgb(0xe08a26)),
                                        )
                                    })
                                    .when(playhead_visible, |d| {
                                        d.child(
                                            div()
                                                .absolute()
                                                .left(px(playhead_x - 1.0))
                                                .top_0()
                                                .bottom_0()
                                                .w(px(2.0))
                                                .bg(blue()),
                                        )
                                    }),
                            ),
                    ),
            )
            .child(self.timeline_scrollbar(cx))
            .when_some(detail, |d, text| {
                d.child(
                    div()
                        .absolute()
                        .right(px(20.0))
                        .bottom(px(22.0))
                        .px_3()
                        .py_1()
                        .rounded_md()
                        .bg(ink())
                        .text_color(rgb(0xffffff))
                        .text_xs()
                        .child(text),
                )
            })
            .when_some(context, |d, menu| d.child(menu))
            .into_any_element()
    }

    /// The recording's clips on the track: draggable to reposition, with
    /// trim handles on the selected clip.
    fn video_clip_lane(
        &self,
        timeline_duration: f64,
        timeline_content_width: f64,
        selected_clip: Option<uuid::Uuid>,
        move_drag: Option<VideoMoveDrag>,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let scale = timeline_content_width / timeline_duration.max(f64::EPSILON);
        let mut elements = Vec::new();
        let mut selected_handles = Vec::new();
        for clip in &self.video_clip_timeline.segments {
            let clip_id = clip.id;
            let Some(range) = self.video_clip_timeline.editor_range(clip_id) else { continue; };
            let left = (range.start * scale) as f32;
            let true_width = ((range.end - range.start) * scale) as f32;
            let width = true_width.max(1.0);
            let selected = selected_clip == Some(clip_id);
            let hovered = self.timeline_ui.hovered_clip == Some(clip_id);
            let dragging = move_drag.is_some_and(|drag| drag.clip_id == clip_id);
            // Absolute positions preserve real clip boundaries even when a
            // subpixel clip needs a minimum visible mark. Gaps stay empty.
            let body = div()
                .id(("video-clip", clip_id.as_u128() as u64))
                .absolute().left(px(left)).top_0().h_full().w(px(width))
                .rounded_md().border_2()
                .border_color(if selected { blue() } else if hovered { hsla(217.0 / 360.0, 0.8, 0.68, 0.75) } else { hsla(0.0, 0.0, 0.0, 0.0) })
                .bg(hsla(217.0 / 360.0, 0.86, 0.58, 1.0))
                .flex().items_center().justify_center().overflow_hidden()
                .text_xs().font_weight(FontWeight::SEMIBOLD).text_color(rgb(0xffffff))
                .when(dragging, |d| d.opacity(0.4))
                .children(self.clip_thumbnails(range.start, clip.source_start, clip.source_end, clip.speed, width, 48.0))
                .when(!selected, |d| d.child(div().absolute().size_full().bg(hsla(0.0, 0.0, 0.0, if hovered { 0.06 } else { 0.16 }))))
                .when(width >= 52.0, |d| {
                    let label = if (clip.speed - 1.0).abs() > f64::EPSILON {
                        format!("{:.1}s · {}×", clip.editor_duration(), clip.speed)
                    } else { format!("{:.1}s", clip.editor_duration()) };
                    d.child(div().relative().px_2().rounded_md().bg(hsla(0.0, 0.0, 0.0, 0.45)).child(label))
                })
                .cursor(if dragging { CursorStyle::ClosedHand } else { CursorStyle::OpenHand })
                .on_hover(cx.listener(move |this, hover: &bool, _, cx| {
                    if *hover { this.timeline_ui.hovered_clip = Some(clip_id); }
                    else if this.timeline_ui.hovered_clip == Some(clip_id) { this.timeline_ui.hovered_clip = None; }
                    cx.notify();
                }))
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    this.focus_handle.focus(window);
                    this.begin_video_move_drag(clip_id, event.position.x);
                    cx.notify();
                }))
                .on_mouse_down(MouseButton::Right, cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    let time = this.motion_timeline_time_at(event.position.x).unwrap_or(this.video_position);
                    this.open_timeline_context(crate::timeline_ui::TimelineContextTarget::Clip(clip_id, time), event.position, cx);
                }));
            elements.push(body.into_any_element());

            if !selected || dragging { continue; }
            for edge in [ClipEdge::Leading, ClipEdge::Trailing] {
                let leading = edge == ClipEdge::Leading;
                let edge_x = if leading { left } else { left + true_width };
                let narrow = true_width < 32.0;
                // Keep 16px hit targets outside the clipped thumbnail body.
                // Tiny clips use separate vertical halves, so neither edge
                // steals the other's target (including a clip at time zero).
                let hit_left = (edge_x - if leading { 5.0 } else { 11.0 }).max(0.0);
                let grip_left = (edge_x - hit_left - 1.5).clamp(0.0, 13.0);
                let height = if narrow { 24.0 } else { 48.0 };
                let top = if narrow && !leading { 24.0 } else { 0.0 };
                let handle = div()
                    .id((if leading { "video-trim-leading" } else { "video-trim-trailing" }, clip_id.as_u128() as u64))
                    .absolute().left(px(hit_left)).top(px(top)).w(px(16.0)).h(px(height))
                    .occlude().cursor(CursorStyle::ResizeLeftRight)
                    .tooltip(crate::timeline_ui::tooltip(if leading { "Drag to trim start · Escape cancels" } else { "Drag to trim end · Escape cancels" }))
                    .child(div().absolute().left(px(grip_left)).top(px((height - 18.0) / 2.0)).w(px(3.0)).h(px(18.0))
                        .rounded_sm().bg(rgb(0xffffff)).shadow_sm())
                    .hover(|s| s.bg(hsla(217.0 / 360.0, 0.8, 0.55, 0.35)))
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        this.focus_handle.focus(window);
                        this.begin_video_trim(clip_id, edge, event.position.x);
                        cx.notify();
                    }));
                selected_handles.push(handle.into_any_element());
            }
        }
        // Hitboxes are above all clip bodies; the selected clip wins a
        // shared boundary over a neighboring clip's move target.
        elements.extend(selected_handles);
        elements
    }
}
