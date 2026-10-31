//! The editor shell shared by the static, motion, and video modes: one top
//! bar (mode switcher, undo/redo, export), one tabbed inspector, and one
//! timeline bar. Mode-specific code only supplies the canvas and the lanes.

use gpui::{
    canvas, div, hsla, prelude::*, px, rgb, svg, AnyElement, Context, CursorStyle, FontWeight, MouseButton, MouseDownEvent, Pixels, ScrollDelta, ScrollWheelEvent, Size,
    Window,
};

use crate::{
    accent, accent_tint, brand, canvas_well, lane_video, control_bg, control_hover, control_selected,
    controls::{button, field, hint, icon_button, ButtonKind, ButtonSize, SliderTarget},
    theme::{RADIUS_MD, SPACE_1, SPACE_3},
    danger, danger_tint, ink, line, on_accent, surface, TextRoles,
    motion_ui::{ANIMATION_DURATIONS, BORDER_COLORS},
    muted,
    recording::{
        clips::ClipEdge, scene::WindowFrame, viewport::MotionPreset,
    },
    scene_ui::SceneSelection,
    timestamped_export_name, Studio, Tool, VideoMoveDrag,
};

const INSPECTOR_WIDTH: f32 = 404.0;
/// Inspector width at `WIDE_WINDOW` and wider.
const INSPECTOR_WIDTH_WIDE: f32 = 440.0;
const WIDE_WINDOW: f32 = 1800.0;
pub(crate) const TOP_BAR_HEIGHT: f32 = 60.0;
/// Canvas well padding; less vertical so the canvas keeps its height.
const CANVAS_PADDING_X: f32 = 40.0;
const CANVAS_PADDING_Y: f32 = 24.0;
/// Default lane area height; it grows on tall windows until resized by hand.
pub(crate) const DEFAULT_LANE_HEIGHT: f32 = 176.0;
const MAX_LANE_HEIGHT: f32 = 340.0;
const TIMELINE_CONTROLS_HEIGHT: f32 = 78.0;
const SCENE_STRIP_HEIGHT: f32 = 42.0;
/// Label column shared by the scene strip, the ruler and every lane.
const TRACK_LABEL_WIDTH: f32 = 104.0;
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
            EditorMode::Static => "Image",
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
            InspectorTab::Design => "Style",
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
        .mx_2()
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

    /// The segment the mode switcher highlights. A recording keeps its
    /// motion controls in the Motion tab, so that tab reads as Motion.
    fn displayed_mode(&self) -> EditorMode {
        let mode = self.editor_mode();
        if mode == EditorMode::Video
            && self.inspector_visible
            && self.inspector_tab == InspectorTab::Motion
        {
            EditorMode::Motion
        } else {
            mode
        }
    }

    pub(crate) fn switch_mode(&mut self, mode: EditorMode, cx: &mut Context<Self>) {
        if mode == self.editor_mode() {
            // Video highlighted as Motion: Video returns to the general tab.
            if mode == EditorMode::Video && self.displayed_mode() == EditorMode::Motion {
                self.select_inspector_tab(InspectorTab::Design);
                cx.notify();
            }
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
            self.toast = Some(crate::Notification::error(format!("Could not create screenshot folder: {error}")));
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
                        Err(error) => crate::notifications::failure("Export failed", &error, path.parent()),
                    },
                    Ok(None) => "Export cancelled".into(),
                    Err(error) => crate::Notification::error(format!("Export failed: {error}")),
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

    pub(crate) fn inspector_width(viewport_width: Pixels) -> f32 {
        if viewport_width >= px(WIDE_WINDOW) { INSPECTOR_WIDTH_WIDE } else { INSPECTOR_WIDTH }
    }

    /// Until the user drags the timeline edge, its lanes take a share of
    /// tall windows instead of a fixed height.
    pub(crate) fn fit_timeline_to_window(&mut self, viewport: Size<Pixels>) {
        if self.timeline_ui.lane_height_resized {
            return;
        }
        let extra = (f32::from(viewport.height) - 900.0).max(0.0) * 0.25;
        self.timeline_ui.lane_height = (DEFAULT_LANE_HEIGHT + extra).min(MAX_LANE_HEIGHT);
    }

    /// The preview size that fits between the top bar, the timeline, and the
    /// inspector.
    pub(crate) fn canvas_budget(&self, viewport: Size<Pixels>) -> (Pixels, Pixels) {
        let inspector = if self.inspector_visible {
            Self::inspector_width(viewport.width)
        } else {
            0.0
        };
        let width = (viewport.width.max(px(EDITOR_MIN_WIDTH))
            - px(CANVAS_PADDING_X * 2.0 + inspector)).max(px(1.0));
        let height = (viewport.height.max(px(EDITOR_MIN_HEIGHT))
            - px(TOP_BAR_HEIGHT + CANVAS_PADDING_Y * 2.0 + self.timeline_bar_height()))
        .max(px(1.0));
        self.preview_canvas_size(width, height)
    }

    /// The canvas workspace. The export panel stacks with the toasts.
    pub(crate) fn canvas_area(&self, canvas: AnyElement) -> AnyElement {
        div()
            .relative()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .overflow_hidden()
            .px(px(CANVAS_PADDING_X))
            .py(px(CANVAS_PADDING_Y))
            .flex()
            .items_center()
            .justify_center()
            .bg(canvas_well())
            .child(div().flex_none().card().shadow(crate::theme::canvas_shadow()).child(canvas))
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
        tooltip: Option<&'static str>,
        enabled: bool,
        cx: &mut Context<Self>,
        on_click: impl Fn(&mut Studio, &mut Window, &mut Context<Studio>) + 'static,
    ) -> AnyElement {
        let base = match label {
            Some(label) => button(id, label, ButtonKind::Ghost, ButtonSize::Md).icon(icon),
            None => icon_button(id, icon, ButtonKind::Ghost, ButtonSize::Md),
        };
        base.disabled(!enabled)
            .tooltip_opt(tooltip)
            .on_click(cx.listener(move |this, _, window, cx| {
                on_click(this, window, cx);
                cx.notify();
            }))
            .into_any_element()
    }

    fn mode_switcher(&self, cx: &mut Context<Self>) -> AnyElement {
        let current = self.displayed_mode();
        div()
            .flex()
            .flex_none()
            .h(px(34.0))
            .p(px(3.0))
            .rounded_lg()
            .bg(control_bg())
            .children(
                EditorMode::ALL
                    .into_iter()
                    .enumerate()
                    .map(|(index, mode)| {
                        let selected = mode == current;
                        crate::controls::keyboard_focusable(div().id(("editor-mode", index)))
                            .px_4()
                            .h_full()
                            .flex()
                            .items_center()
                            .rounded(px(crate::theme::inner_radius(crate::theme::RADIUS_LG, 3.0)))
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(if selected { ink() } else { muted() })
                            .when(selected, |this| this.bg(control_selected()).shadow_sm())
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
            button(id, label, ButtonKind::Ghost, ButtonSize::Md)
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
                this.child(div().px_2().text_xs().tabular().text_color(muted()).child(value))
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
                button("crop-apply", "Crop", ButtonKind::Primary, ButtonSize::Md)
                    .on_click(cx.listener(|this, _, _, cx| {
                        if let Err(error) = this.apply_crop() {
                            this.toast = Some(crate::Notification::error(error));
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
                    button("bar-record-new", "New capture", ButtonKind::Secondary, ButtonSize::Md)
                        .icon("icons/record.svg")
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
                    Some("Undo · Ctrl+Z"),
                    can_undo,
                    cx,
                    |this, _, cx| this.undo_current(cx),
                ))
                .child(self.bar_button(
                    "bar-redo",
                    "icons/redo.svg",
                    None,
                    Some("Redo · Ctrl+Shift+Z"),
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
                        None,
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
                    div().ml_1().child(
                        button(
                            "bar-export",
                            // Outside image mode the button only opens the tab.
                            if mode == EditorMode::Static { "Export" } else { "Export…" },
                            // The Export tab has its own primary action.
                            if self.effective_tab() == InspectorTab::Export && self.inspector_visible {
                                ButtonKind::Secondary
                            } else {
                                ButtonKind::Primary
                            },
                            ButtonSize::Md,
                        )
                            .icon("icons/upload.svg")
                            .disabled(export_busy)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.stop_editing_text();
                                if this.editor_mode() == EditorMode::Static {
                                    this.export_current(cx);
                                } else {
                                    this.select_inspector_tab(InspectorTab::Export);
                                    this.inspector_visible = true;
                                }
                                cx.notify();
                            })),
                    ),
                )
                .child({
                    let preference = crate::theme::theme_preference();
                    icon_button("bar-theme", preference.icon(), ButtonKind::Ghost, ButtonSize::Md)
                        .tooltip(format!("Appearance: {} · click for {}", preference.label(), preference.next().label()))
                        .on_click(cx.listener(move |_, _, _, cx| {
                            crate::theme::set_theme_preference(preference.next(), cx);
                        }))
                })
                .child(
                    div().ml_1().child(
                        icon_button("bar-sidebar", "icons/sidebar.svg", ButtonKind::Ghost, ButtonSize::Md)
                            .selected(!self.inspector_visible)
                            .tooltip(if self.inspector_visible { "Hide inspector" } else { "Show inspector" })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.inspector_visible = !this.inspector_visible;
                                cx.notify();
                            })),
                    ),
                )
            })
            .pr_4();

        div()
            .h(px(TOP_BAR_HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .bg(surface())
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
            .border_t_1()
            .border_color(line())
            .cursor_pointer()
            .child(div().section_title().child(title))
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
        div().section_title().child(text).into_any_element()
    }

    /// Hairline between sections; with the tab's 12 px gap on either side
    /// it makes the 24 px section break.
    pub(crate) fn section_divider() -> AnyElement {
        div().flex_none().h(px(1.0)).bg(line()).into_any_element()
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
            .gap(px(SPACE_3))
            .child(Self::tab_label("Canvas"))
            .child(field("Aspect ratio", self.canvas_aspect_picker(cx)))
            .when(self.video_project.is_some() && self.aspect_ratio != 0, |this| {
                this.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(SPACE_1))
                        .child(self.segmented(
                            "content-fill",
                            &["Fit", "Fill"],
                            usize::from(self.content_fill),
                            |this, value| this.content_fill = value == 1,
                            cx,
                        ))
                        .child(hint(if self.content_fill {
                            "Crops the recording to the canvas; the camera follows your cursor."
                        } else {
                            "Shows the whole recording on the background."
                        })),
                )
            })
            .when(self.video_project.is_some(), |this| {
                this.child(self.scene_toggle_row(
                    "enhance-screen",
                    "Upscale zoomed footage",
                    self.enhance_screen,
                    cx,
                    |this| this.enhance_screen = !this.enhance_screen,
                ))
            })
            .child(self.slider_row(
                SliderTarget::Padding,
                "Padding",
                self.padding,
                "%",
                |this, value| this.padding = value,
                cx,
            ))
            .child(Self::section_divider())
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
            .child(Self::section_divider())
            .child(Self::tab_label("Image appearance"))
            .child(self.slider_row(
                SliderTarget::Corners,
                "Corners",
                self.corners,
                "%",
                |this, value| this.corners = value,
                cx,
            ))
            .child(field(
                "Window frame",
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
                this.child(div().flex().items_center().gap(px(2.0)).children(
                    BORDER_COLORS.iter().enumerate().map(|(index, color)| {
                        let name = ["Yellow", "Green", "Teal", "Blue", "Purple", "Pink"][index];
                        crate::controls::swatch(("border-color", index), name, self.border_color == index, rgb(*color), None)
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
            .child(Self::section_divider())
            .child(Self::tab_label("Shadow"))
            .child(self.slider_row(
                SliderTarget::Shadow,
                "Amount",
                self.shadow,
                "%",
                |this, value| this.shadow = value,
                cx,
            ))
            .when(self.shadow > 0, |this| this
                .child(self.shadow_color_picker(cx))
                .child(field("Style", self.segmented(
                    "shadow-style",
                    &["Soft", "Long", "Glow", "Crisp"],
                    self.shadow_style,
                    |this, value| this.shadow_style = value,
                    cx,
                ))))
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
            .child(Self::section_divider())
            .child(Self::tab_label("Presets"))
            .child(self.quick_presets_row(cx))
            .child(self.section_header("saved-presets", "Saved presets", None, cx))
            .when(self.section_open("saved-presets"), |this| {
                this.child(self.preset_library_section(cx))
            })
            .into_any_element()
    }

    /// Compact aspect picker: four columns of tiles with the ratio's shape
    /// beside its name.
    pub(crate) fn canvas_aspect_picker(&self, cx: &mut Context<Self>) -> AnyElement {
        let presets = [
            ("Auto", 0, 1.6), ("16:9", 4, 16.0 / 9.0), ("9:16", 5, 9.0 / 16.0),
            ("1:1", 1, 1.0), ("4:5", 6, 4.0 / 5.0), ("4:3", 2, 4.0 / 3.0),
            ("3:2", 3, 1.5), ("3:4", 7, 0.75), ("2:3", 8, 2.0 / 3.0),
            ("21:9", 9, 21.0 / 9.0), ("Custom", 10, 1.4),
        ];
        div().flex().flex_col().gap_2()
            .child(div().grid().grid_cols(4).gap_2().children(presets.into_iter().map(|(label, index, ratio)| {
                let active = self.aspect_ratio == index;
                let width = 16.0_f32.min(16.0 * ratio);
                let height = width / ratio;
                crate::controls::keyboard_focusable(div().id(("canvas-aspect", index)))
                    .h(px(40.0)).px_2().rounded(px(RADIUS_MD))
                    .border_1().border_color(if active { accent() } else { line() })
                    .bg(if active { accent_tint() } else { surface() })
                    .flex().items_center().justify_center().gap(px(6.0))
                    .cursor_pointer().when(!active, |this| this.hover(|style| style.bg(control_bg())))
                    .child(div().flex_none().size(px(16.0)).flex().items_center().justify_center()
                        .child(div().w(px(width)).h(px(height)).rounded(px(2.0))
                            .border_1().border_color(if active { accent() } else { muted() })))
                    .child(div().text_xs().font_weight(FontWeight::MEDIUM).text_color(ink()).truncate().child(label))
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
                        .child(div().hint().child(format!("{selection_count} selected")))
                        .child(self.small_button(
                            "annotation-delete-selection",
                            if selection_count == self.annotations.len() { "Delete all" } else { "Delete selected" },
                            true,
                            cx,
                            |this, _| {
                                if this.delete_selected_annotations() && this.processed_capture_path.is_some() {
                                    if let Err(error) = this.rebuild_redactions() {
                                        this.toast = Some(crate::Notification::error(error));
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
                            .bg(if selected_indices.contains(&index) { accent_tint() } else { control_bg() })
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
                                    .tooltip(crate::timeline_ui::tooltip("Delete annotation"))
                                    .flex()
                                    .flex_none()
                                    .items_center()
                                    .justify_center()
                                    .size(px(32.0))
                                    .mr_1()
                                    .rounded_md()
                                    .cursor_pointer()
                                    .hover(|style| style.bg(danger_tint()).text_color(danger()))
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
                                                this.toast = Some(crate::Notification::error(error));
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
        let selected_preset = self.animation_preset.filter(|_| !self.video_zoom_cues.is_empty());
        let duration = self.animation_duration;
        let step = |id: &'static str, label: &'static str, tip: &'static str, enabled: bool, delta: f64| {
            button(id, label, ButtonKind::Secondary, ButtonSize::Sm)
                .tooltip(tip)
                .disabled(!enabled)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.set_animation_duration((this.animation_duration + delta).max(1.0));
                    cx.notify();
                }))
        };
        div()
            .flex()
            .flex_col()
            .gap(px(SPACE_3))
            .child(Self::tab_label("Scene duration"))
            .child(div().flex().items_center().gap_1()
                .child(step("animation-duration-shorter", "−", "Shorter", duration > 1.0, -1.0))
                .child(div().w(px(56.0)).h(px(28.0)).flex().items_center().justify_center()
                    .rounded_md().bg(control_bg()).text_xs().tabular()
                    .child(crate::units::fmt_seconds(duration)))
                .child(step("animation-duration-longer", "+", "Longer", true, 1.0))
                .child(div().flex_1())
                .children(ANIMATION_DURATIONS.into_iter().enumerate().map(|(index, preset)| {
                    button(("animation-duration", index), format!("{preset:.0} s"), ButtonKind::Ghost, ButtonSize::Sm)
                        .selected((preset - duration).abs() < 1e-9)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.set_animation_duration(preset);
                            cx.notify();
                        }))
                })))
            .child(Self::tab_label("Motion preset"))
            .child(
                div().grid().grid_cols(3).gap_2().children(
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
                                .flex()
                                .flex_col()
                                .rounded_md()
                                .overflow_hidden()
                                .border_2()
                                .border_color(if selected { accent() } else { line() })
                                .bg(control_bg())
                                .cursor_pointer()
                                .hover(move |style| {
                                    if selected {
                                        style
                                    } else {
                                        style.border_color(muted())
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
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(ink())
                                        .truncate()
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
                        button(
                            "walkthrough-toggle",
                            if self.walkthrough_mode {
                                "Placing stops… (Enter to finish)"
                            } else if self.has_walkthrough() {
                                "Add more stops"
                            } else {
                                "Place cursor stops"
                            },
                            if self.walkthrough_mode { ButtonKind::Primary } else { ButtonKind::Secondary },
                            ButtonSize::Md,
                        )
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
            .into_any_element()
    }

    fn scene_strip(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .items_center()
            .child(Self::track_label("Scenes", 30.0))
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
                            .bg(if selected { accent() } else { control_bg() })
                            .text_color(if selected { on_accent() } else { ink() })
                            .cursor_pointer()
                            .when(!selected, |this| this.hover(|style| style.bg(control_hover())))
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
                            "Delete scene",
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
        let overlays = self.input_overlays;
        let keystrokes_captured = !self.video_keystrokes.is_empty();
        div()
            .flex()
            .flex_col()
            .gap(px(SPACE_3))
            .child(self.subtitle_section(cx))
            .child(self.section_header(
                "keystrokes",
                "Keystrokes",
                Some(if keystrokes_captured {
                    self.header_switch("keystrokes-switch", "keystrokes", overlays.keystrokes, cx, |this| {
                        this.input_overlays.keystrokes = !this.input_overlays.keystrokes;
                    })
                } else {
                    div()
                        .id("keystrokes-switch")
                        .opacity(0.4)
                        .tooltip(crate::timeline_ui::tooltip(
                            "No keystrokes in this recording. Record with the Lahza GNOME Shell extension enabled.",
                        ))
                        .child(self.toggle(false))
                        .into_any_element()
                }),
                cx,
            ))
            .when(keystrokes_captured && overlays.keystrokes && self.section_open("keystrokes"), |this| {
                this.child(self.keystroke_section(cx))
            })
            .child(self.section_header(
                "speed-badge",
                "Speed badge",
                Some(self.header_switch("speed-badge-switch", "speed-badge", overlays.speed_badge, cx, |this| {
                    this.input_overlays.speed_badge = !this.input_overlays.speed_badge;
                })),
                cx,
            ))
            .when(self.section_open("speed-badge"), |this| {
                this.child(self.speed_badge_section(cx))
            })
            .into_any_element()
    }

    /// Header naming the selected object, with a way back to the tab. The
    /// rail already names the tab, so without a selection there is none.
    fn inspector_context(&self, tab: InspectorTab, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (title, trailing) = match tab {
            InspectorTab::Motion => {
                let cue = self.selected_region()?;
                let slices = self.video_clip_timeline.slices_overlapping(cue.start, cue.end);
                let range = slices.first().zip(slices.last())
                    .map(|(first, last)| crate::units::fmt_seconds_range(first.editor_start, last.editor_end))
                    .unwrap_or_else(|| "Outside current edit".to_string());
                let kind = if cue.transform.is_none() || cue.has_camera_motion() { "Zoom" } else { "Transform" };
                let enabled = cue.is_enabled;
                let switch = div()
                    .id("motion-enabled-toggle")
                    .cursor_pointer()
                    .tooltip(crate::timeline_ui::tooltip(if enabled { "Turn off zoom" } else { "Turn on zoom" }))
                    .child(self.toggle(enabled))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.edit_selected_region(|cue| cue.is_enabled = !cue.is_enabled);
                        cx.notify();
                    }))
                    .into_any_element();
                (format!("{kind} · {range}"), Some(switch))
            }
            InspectorTab::Annotate => {
                let mark = self.selected_annotation.and_then(|index| self.annotations.get(index))?;
                let text = mark.text.split_whitespace().collect::<Vec<_>>().join(" ");
                let title = if mark.tool == Tool::Text && !text.is_empty() {
                    format!("{} · {text}", mark.tool.label())
                } else {
                    mark.tool.label().to_string()
                };
                (title, None)
            }
            _ => return None,
        };
        Some(div().h(px(52.0)).flex_none().pl_2().pr_4().flex().items_center().gap_2()
            .border_b_1().border_color(line())
            .child(icon_button("inspector-back", "icons/chevron-left.svg", ButtonKind::Ghost, ButtonSize::Sm)
                .tooltip("Back")
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.select_inspector_tab(tab);
                    cx.notify();
                })))
            .child(div().flex_1().min_w_0().truncate().panel_title().child(title))
            .when_some(trailing, |this, trailing| this.child(trailing))
            .into_any_element())
    }

    pub(crate) fn sidebar(&self, viewport_width: Pixels, cx: &mut Context<Self>) -> AnyElement {
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
                .gap(px(SPACE_3))
                .child(self.export_section(cx))
                .child(
                    div()
                        .text_size(px(13.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(ink())
                        .tabular()
                        .child(self.export_summary()),
                )
                .child(
                    button(
                        "export-start",
                        if self.export_progress.is_some() { "Exporting…" } else { "Export file…" },
                        ButtonKind::Primary,
                        ButtonSize::Md,
                    )
                    .full_width()
                    .disabled(self.export_progress.is_some())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.export_current(cx);
                        cx.notify();
                    })),
                )
                .when(self.video_project.is_some(), |this| {
                    this.child(Self::section_divider())
                        .child(self.audio_only_section(cx))
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
                            .when(selected, |this| this.bg(accent_tint()))
                            .hover(move |style| {
                                if selected {
                                    style
                                } else {
                                    style.bg(control_bg())
                                }
                            })
                            .child(
                                svg()
                                    .path(candidate.icon())
                                    .size(px(17.0))
                                    .text_color(if selected { ink() } else { muted() }),
                            )
                            .child(
                                div()
                                    .micro()
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
            .w(px(Self::inspector_width(viewport_width)))
            .flex_none()
            .h_full()
            .relative()
            .flex()
            .bg(surface())
            .border_l_1()
            .border_color(line())
            .child(tabs)
            .child(
                div()
                    .flex_1().min_w_0().h_full().flex().flex_col()
                    .when_some(self.inspector_context(tab, cx), |this, header| this.child(header))
                    .child(div()
                    .id("inspector-scroll")
                    .track_scroll(&self.inspector_scroll)
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_4()
                    .flex()
                    .flex_col()
                    .child(div().flex_none().w_full().child(body))),
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
            .w(px(TRACK_LABEL_WIDTH))
            .pr_3()
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
                                .border_color(lane_video())
                                .bg(lane_video().opacity(if clip.id == drag.clip_id { 0.35 } else { 0.16 }))
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
            // Bands, top to bottom: labels, click markers, ticks.
            ruler_marks.push(
                div()
                    .absolute()
                    .left(px(x))
                    .bottom_0()
                    .w(px(1.0))
                    .h(px(5.0))
                    .bg(ink().opacity(0.30))
                    .into_any_element(),
            );
            ruler_marks.push(
                div()
                    .absolute()
                    .left(px(x + 4.0))
                    .top(px(4.0))
                    .micro()
                    .line_height(px(14.0))
                    .tabular()
                    .whitespace_nowrap()
                    .text_color(muted())
                    .child(crate::units::ruler_time(ruler_tick, ruler_step < 1.0))
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
                        .bg(ink().opacity(0.15))
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
                    "Scene {} · {}",
                    self.image_scene_index + 1,
                    crate::units::fmt_seconds(image_duration)
                )
            } else {
                format!("Image · {}", crate::units::fmt_seconds(image_duration))
            };
            if image_duration <= 0.0 {
                Vec::new()
            } else {
                let left = (timeline_content_width * image_start / timeline_duration) as f32;
                let width =
                    ((timeline_content_width * image_duration / timeline_duration) as f32).max(32.0);
                let selected = self.video_selected_clip.is_some();
                let mut elements = vec![div()
                    .id("image-timeline-clip")
                    .cursor(if self.image_trim_drag.is_some() {
                        CursorStyle::ClosedHand
                    } else {
                        CursorStyle::OpenHand
                    })
                    .absolute()
                    .left(px(left))
                    .overflow_hidden()
                    .h_full()
                    .w(px(width))
                    .flex_none()
                    .rounded_md()
                    .border_2()
                    .border_color(if selected { ink() } else { gpui::transparent_black() })
                    .when(!selected, |d| d.hover(|s| s.border_color(ink().opacity(0.4))))
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
                    .bg(lane_video())
                    .when(selected || width >= 96.0, |d| d.child(Self::clip_duration_pill(label)))
                    .into_any_element()];
                for edge in [ClipEdge::Leading, ClipEdge::Trailing] {
                    let leading = edge == ClipEdge::Leading;
                    elements.push(
                        crate::timeline_ui::trim_handle(
                            if leading { "image-trim-start" } else { "image-trim-end" },
                            0.0,
                            if leading { left } else { left + width },
                            leading,
                            width,
                            0.0,
                            48.0,
                            selected,
                            false,
                        )
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                this.begin_image_trim(event.position.x, edge);
                                cx.stop_propagation();
                                cx.notify();
                            }),
                        )
                        .into_any_element(),
                    );
                }
                elements
            }
        };
        let motion_track = self.motion_track(timeline_scroll, timeline_content_width, progress, cx);
        let annotation_track =
            self.annotation_track(timeline_scroll, timeline_content_width, progress, cx);
        let audio_height = if audio_lane.is_some() { 26.0 } else { 0.0 };
        let camera_height = if camera_lane.is_some() { 32.0 } else { 0.0 };
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
            .bg(surface())
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
                    .child(Self::track_label("Time", 38.0))
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
                                        .bg(brand())
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .text_xs()
                                        .tabular()
                                        .text_color(gpui::white())
                                        .child(if expanded {
                                            crate::units::timecode(self.video_position)
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
                                    .w(px(TRACK_LABEL_WIDTH))
                                    .flex_none()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .child(Self::track_label(
                                        if is_video { "Video" } else { "Image" },
                                        48.0,
                                    ))
                                    .child(Self::track_label("Zooms", self.motion_lane_height()))
                                    .when(annotation_track.is_some(), |d| {
                                        d.child(Self::track_label(
                                            "Annotations",
                                            self.annotation_lane_height(),
                                        ))
                                    })
                                    .when(camera_lane.is_some(), |d| {
                                        d.child(Self::track_label("Camera", 28.0))
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
                                            .bg(control_bg())
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
                                                .bg(muted()),
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
                                                .bg(crate::theme::selection()),
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
                                                .bg(brand()),
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
                        .text_color(surface())
                        .text_xs()
                        .child(text),
                )
            })
            .when_some(context, |d, menu| d.child(menu))
            .into_any_element()
    }

    /// Duration pill pinned to a clip's top-left corner.
    fn clip_duration_pill(label: String) -> gpui::Div {
        div()
            .absolute()
            .top(px(4.0))
            .left(px(4.0))
            .px(px(6.0))
            .rounded_sm()
            .bg(hsla(0.0, 0.0, 0.0, 0.45))
            .micro()
            .line_height(px(14.0))
            .tabular()
            .whitespace_nowrap()
            .text_color(gpui::white())
            .child(label)
    }

    /// The recording's clips on the track: draggable to reposition, with
    /// trim handles on every clip edge.
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
        let mut handles = Vec::new();
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
                .border_color(if selected { ink() } else if hovered { ink().opacity(0.4) } else { gpui::transparent_black() })
                .bg(lane_video())
                .overflow_hidden()
                .when(dragging, |d| d.opacity(0.4))
                .children(self.clip_thumbnails(range.start, clip.source_start, clip.source_end, clip.speed, width, 48.0))
                .when(!selected, |d| d.child(div().absolute().size_full().bg(hsla(0.0, 0.0, 0.0, if hovered { 0.06 } else { 0.16 }))))
                .when(width >= 52.0 && (selected || hovered || width >= 96.0), |d| {
                    let label = if (clip.speed - 1.0).abs() > f64::EPSILON {
                        format!("{} · {}", crate::units::fmt_seconds(clip.editor_duration()), crate::units::fmt_zoom(clip.speed))
                    } else { crate::units::fmt_seconds(clip.editor_duration()) };
                    d.child(Self::clip_duration_pill(label))
                })
                .cursor(if dragging { CursorStyle::ClosedHand } else { CursorStyle::OpenHand })
                .when(selected && !dragging, |d| d.tooltip(crate::timeline_ui::tooltip(crate::timeline_ui::REGION_KEYS_HELP)))
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

            if dragging { continue; }
            for edge in [ClipEdge::Leading, ClipEdge::Trailing] {
                let leading = edge == ClipEdge::Leading;
                let handle = crate::timeline_ui::trim_handle(
                    (if leading { "video-trim-leading" } else { "video-trim-trailing" }, clip_id.as_u128() as u64),
                    0.0,
                    if leading { left } else { left + true_width },
                    leading,
                    true_width,
                    0.0,
                    48.0,
                    selected,
                    true,
                )
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    cx.stop_propagation();
                    this.focus_handle.focus(window);
                    this.begin_video_trim(clip_id, edge, event.position.x);
                    cx.notify();
                }));
                if selected { selected_handles.push(handle.into_any_element()); } else { handles.push(handle.into_any_element()); }
            }
        }
        // Hitboxes are above all clip bodies; the selected clip wins a
        // shared boundary over a neighboring clip's handle or move target.
        elements.extend(handles);
        elements.extend(selected_handles);
        elements
    }
}
