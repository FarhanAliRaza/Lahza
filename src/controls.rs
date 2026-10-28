//! Reusable editor controls and annotation appearance settings.

use super::{
    accent, accent_hover, accent_tint, control_bg, control_hover, control_selected, danger,
    danger_tint, gradient_layers, ink, line, muted, on_accent, ring, SliderDrag, Studio,
    TextRoles, Tool, ANNOTATION_COLORS, CURATED_WALLPAPERS, GRADIENT_BACKGROUNDS,
    SOLID_BACKGROUNDS,
};
use crate::theme::{self, inner_radius, CONTROL_LG, CONTROL_MD, CONTROL_SM, RADIUS_LG, RADIUS_MD};
use gpui::{
    div, img, prelude::*, px, rgb, svg, AnyElement, App, Bounds, ClickEvent, Context,
    CursorStyle, Div, ElementId, Fill, FontWeight, IntoElement, MouseButton, MouseDownEvent,
    ObjectFit, Pixels, RenderOnce, SharedString, Stateful, Window,
};
use std::{cell::Cell, rc::Rc};

// ---------------------------------------------------------------------------
// Keyboard focus
// ---------------------------------------------------------------------------

/// Makes a shared control reachable with Tab and draws the 2 px focus ring
/// while it is focused. A pointer press leaves keyboard focus with the
/// editor, so the ring only ever shows for keyboard focus and Space/arrows
/// keep driving playback and nudges after a click. Enter and Space are
/// claimed here; GPUI turns their release into the control's click.
pub(crate) fn keyboard_focusable(element: Stateful<Div>) -> Stateful<Div> {
    element
        .tab_index(0)
        .focus(|style| style.focus_ring())
        .on_mouse_down(MouseButton::Left, |_, window, _| {
            window.prevent_default();
            window.blur();
        })
        .on_key_down(|event, _, cx| {
            if matches!(event.keystroke.key.as_str(), "enter" | "space")
                && !event.keystroke.modifiers.modified()
            {
                cx.stop_propagation();
            }
        })
}

/// Tab / Shift+Tab move between focusable controls. Returns true when handled.
pub(crate) fn tab_navigation(event: &gpui::KeyDownEvent, window: &mut Window) -> bool {
    let keystroke = &event.keystroke;
    let modifiers = keystroke.modifiers;
    if keystroke.key != "tab" || modifiers.control || modifiers.alt || modifiers.platform {
        return false;
    }
    if modifiers.shift {
        window.focus_prev();
    } else {
        window.focus_next();
    }
    true
}

// ---------------------------------------------------------------------------
// Buttons
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ButtonKind {
    /// Ink fill: the one main action in a group.
    Primary,
    /// Visible resting fill and border: everyday actions.
    Secondary,
    /// No fill until hovered: toolbars and low-emphasis actions.
    Ghost,
    /// Destructive actions.
    Danger,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ButtonSize {
    /// 28 px tall.
    Sm,
    /// 32 px tall.
    Md,
}

type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

/// Shared button. Build with `button` or `icon_button`, then chain options.
#[derive(IntoElement)]
pub(crate) struct Button {
    id: ElementId,
    label: Option<SharedString>,
    icon: Option<SharedString>,
    kind: ButtonKind,
    size: ButtonSize,
    disabled: bool,
    selected: bool,
    full_width: bool,
    tooltip: Option<SharedString>,
    on_click: Option<ClickHandler>,
}

pub(crate) fn button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    kind: ButtonKind,
    size: ButtonSize,
) -> Button {
    Button {
        id: id.into(),
        label: Some(label.into()),
        icon: None,
        kind,
        size,
        disabled: false,
        selected: false,
        full_width: false,
        tooltip: None,
        on_click: None,
    }
}

/// Square icon-only button; give it a tooltip.
pub(crate) fn icon_button(
    id: impl Into<ElementId>,
    icon: impl Into<SharedString>,
    kind: ButtonKind,
    size: ButtonSize,
) -> Button {
    Button { label: None, icon: Some(icon.into()), ..button(id, "", kind, size) }
}

impl Button {
    pub(crate) fn icon(mut self, icon: impl Into<SharedString>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    /// Disabled buttons drop their handler, dim, and keep the default cursor.
    pub(crate) fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Pressed state for toggle buttons.
    pub(crate) fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    pub(crate) fn full_width(mut self) -> Self {
        self.full_width = true;
        self
    }

    pub(crate) fn tooltip(mut self, text: impl Into<SharedString>) -> Self {
        self.tooltip = Some(text.into());
        self
    }

    pub(crate) fn tooltip_opt(mut self, text: Option<impl Into<SharedString>>) -> Self {
        self.tooltip = text.map(Into::into);
        self
    }

    pub(crate) fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }

    /// The styled element, for callers that attach their own mouse handlers.
    pub(crate) fn into_div(self) -> Stateful<Div> {
        let (height, icon_size, text_size) = match self.size {
            ButtonSize::Sm => (CONTROL_SM, 14.0, 12.0),
            ButtonSize::Md => (CONTROL_MD, 16.0, 13.0),
        };
        let transparent = gpui::transparent_black();
        let (bg, fg, border, hover_bg, hover_border) = match (self.kind, self.selected) {
            (ButtonKind::Primary, _) => {
                (accent(), on_accent(), accent(), accent_hover(), accent_hover())
            }
            (ButtonKind::Danger, _) => {
                (danger_tint(), danger(), danger_tint(), danger_tint(), danger().opacity(0.45))
            }
            // Raised fill plus a full-contrast ring, so "on" reads in both themes.
            (_, true) => (control_selected(), ink(), accent(), control_selected(), accent()),
            (ButtonKind::Secondary, false) => {
                (control_bg(), ink(), line(), control_hover(), line())
            }
            (ButtonKind::Ghost, false) => (transparent, ink(), transparent, control_hover(), transparent),
        };
        let enabled = !self.disabled;
        let icon_only = self.label.is_none();
        let padding = if self.size == ButtonSize::Sm { 8.0 } else { 12.0 };
        let element = div()
            .id(self.id)
            .h(px(height))
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .gap(px(6.0))
            .when(icon_only, |this| this.w(px(height)))
            .when(!icon_only, |this| this.px(px(padding)))
            .when(self.full_width, |this| this.w_full())
            .rounded(px(RADIUS_MD))
            .border_1()
            .border_color(border)
            .bg(bg)
            .text_color(fg)
            .text_size(px(text_size))
            .font_weight(FontWeight::MEDIUM)
            .whitespace_nowrap()
            .when(!enabled, |this| this.opacity(0.45))
            .when_some(self.tooltip, |this, text| this.tooltip(crate::timeline_ui::tooltip(text)))
            .when_some(self.on_click.filter(|_| enabled), |this, handler| {
                this.cursor_pointer()
                    .hover(move |style| style.bg(hover_bg).border_color(hover_border))
                    .active(|style| style.opacity(0.85))
                    .on_click(handler)
            })
            .when_some(self.icon, |this, icon| {
                this.child(svg().path(icon).size(px(icon_size)).flex_none().text_color(fg))
            })
            .when_some(self.label, |this, label| this.child(label));
        if !enabled {
            return element;
        }
        // The 2 px ring replaces the 1 px border; trim the padding so a
        // focused label button keeps its width.
        keyboard_focusable(element)
            .when(!icon_only, |this| this.focus(move |style| style.focus_ring().px(px(padding - 1.0))))
    }
}

impl RenderOnce for Button {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        self.into_div()
    }
}

// ---------------------------------------------------------------------------
// Segmented options, swatches
// ---------------------------------------------------------------------------

const TRACK_PADDING: f32 = 3.0;

/// Track holding `option_button`s (8 px radius, 3 px padding).
pub(crate) fn segmented_track() -> Div {
    div()
        .flex()
        .flex_none()
        .w_full()
        .p(px(TRACK_PADDING))
        .gap(px(2.0))
        .rounded(px(RADIUS_LG))
        .bg(control_bg())
}

/// One choice inside a `segmented_track`; selected is a raised surface.
/// Callers add `on_click`, sizing and the label.
pub(crate) fn option_button(id: impl Into<ElementId>, selected: bool) -> Stateful<Div> {
    keyboard_focusable(div().id(id))
        .h(px(CONTROL_SM))
        .min_w_0()
        .px_2()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(inner_radius(RADIUS_LG, TRACK_PADDING)))
        .overflow_hidden()
        .whitespace_nowrap()
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .text_color(if selected { ink() } else { muted() })
        .when(selected, |this| this.bg(control_selected()).shadow_sm())
        .when(!selected, |this| this.hover(|style| style.text_color(ink())))
        .cursor_pointer()
}

const SWATCH: f32 = 24.0;
const SWATCH_GAP: f32 = 2.0;
const SWATCH_RING: f32 = 2.0;

/// 24 px color chip. The selection ring sits outside the color with a 2 px
/// gap so it reads on any color; the name shows as a tooltip.
pub(crate) fn swatch(
    id: impl Into<ElementId>,
    name: impl Into<SharedString>,
    selected: bool,
    fill: impl Into<Fill>,
    overlay: Option<gpui::Background>,
) -> Stateful<Div> {
    let name: SharedString = name.into();
    let outer = SWATCH + 2.0 * (SWATCH_GAP + SWATCH_RING);
    keyboard_focusable(div().id(id))
        .flex_none()
        .size(px(outer))
        .p(px(SWATCH_GAP))
        .rounded(px(RADIUS_MD + SWATCH_GAP + SWATCH_RING))
        .border(px(SWATCH_RING))
        .border_color(if selected { accent() } else { gpui::transparent_black() })
        .when(!selected, |this| this.hover(|style| style.border_color(line())))
        .focus(|style| style.border_color(crate::theme::focus_ring()))
        .cursor_pointer()
        .tooltip(crate::timeline_ui::tooltip(name))
        .child(
            div()
                .relative()
                .size(px(SWATCH))
                .rounded(px(RADIUS_MD))
                .overflow_hidden()
                .bg(fill)
                .border_1()
                .border_color(ring())
                .when_some(overlay, |this, overlay| {
                    this.child(div().absolute().inset_0().bg(overlay))
                }),
        )
}

/// Switch-backed row; the whole row toggles, with a hover fill.
pub(crate) fn toggle_row(id: impl Into<ElementId>, label: impl Into<SharedString>, enabled: bool) -> Stateful<Div> {
    keyboard_focusable(div().id(id))
        .flex()
        .items_center()
        .justify_between()
        .gap_2()
        .min_h(px(CONTROL_MD))
        .mx(px(-6.0))
        .px(px(6.0))
        .focus(|style| style.focus_ring().px(px(4.0)))
        .rounded(px(RADIUS_MD))
        .cursor_pointer()
        .hover(|style| style.bg(control_bg()))
        .child(div().row_label().child(label.into()))
        .child(switch(enabled))
}

/// 38×22 switch; the on state is accent with an `on_accent` knob.
pub(crate) fn switch(enabled: bool) -> Div {
    let track = if enabled { accent() } else { muted().opacity(0.35) };
    let knob = if enabled {
        on_accent()
    } else if theme::is_dark() {
        ink()
    } else {
        gpui::white()
    };
    div()
        .flex_none()
        .w(px(38.0))
        .h(px(22.0))
        .p(px(2.0))
        .rounded_full()
        .bg(track)
        .flex()
        .when(enabled, |this| this.justify_end())
        .when(!enabled, |this| this.justify_start())
        .child(div().size(px(18.0)).rounded_full().bg(knob).shadow_sm())
}

// ---------------------------------------------------------------------------
// Inspector layout: fields, hints, inline errors
// ---------------------------------------------------------------------------

/// A field label stacked 6 px above its control.
pub(crate) fn field(label: impl Into<SharedString>, control: impl IntoElement) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(6.0))
        .child(div().field_label().child(label.into()))
        .child(control)
}

/// Hint text under a control.
pub(crate) fn hint(text: impl Into<SharedString>) -> Div {
    div().hint().child(text.into())
}

/// Inline error: danger text on `danger_tint` behind an alert icon. Callers
/// append a recovery button as a child when there is one.
pub(crate) fn inline_error(message: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .items_start()
        .gap_2()
        .p_2()
        .rounded(px(RADIUS_MD))
        .bg(danger_tint())
        .text_color(danger())
        .text_size(px(12.0))
        .line_height(px(16.0))
        .child(svg().path("icons/alert.svg").size(px(14.0)).mt(px(1.0)).flex_none().text_color(danger()))
        .child(div().flex_1().min_w_0().child(message.into()))
}

// ---------------------------------------------------------------------------
// Slider row
// ---------------------------------------------------------------------------

/// What a slider row shows; behavior comes from the callbacks.
pub(crate) struct SliderRow {
    /// `SliderDrag` id routed through `Studio::set_slider_value`.
    pub(crate) slider_id: usize,
    pub(crate) label: SharedString,
    /// Thumb position, 0–1.
    pub(crate) fraction: f32,
    pub(crate) value: SharedString,
    pub(crate) can_decrease: bool,
    pub(crate) can_increase: bool,
    /// Show the reset × (only when the value differs from its default).
    pub(crate) show_reset: bool,
}

impl Studio {
    /// Shared slider: 36 px track with the label inside, −/value/+ to the
    /// right. Pressing anywhere on the track jumps the value there, and
    /// dragging follows the pointer's absolute position. With keyboard
    /// focus, the arrow keys step it (Shift steps ten at a time).
    /// `on_step` receives a signed number of steps.
    pub(crate) fn slider_control(
        &self,
        row: SliderRow,
        on_press: impl Fn(&mut Studio) -> bool + 'static,
        on_step: impl Fn(&mut Studio, i32) + 'static,
        on_reset: impl Fn(&mut Studio) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let SliderRow { slider_id, label, fraction, value, can_decrease, can_increase, show_reset } = row;
        let fraction = fraction.clamp(0.0, 1.0);
        let dragging = self.slider_drag.is_some_and(|drag| drag.slider_id == slider_id);
        let measured = Rc::new(Cell::new(None::<Bounds<Pixels>>));
        let track = measured.clone();
        let on_step = Rc::new(on_step);
        let stepper = |increase: bool, enabled: bool| {
            let on_step = on_step.clone();
            div()
                .id((if increase { "slider-plus" } else { "slider-minus" }, slider_id))
                .flex_none()
                .w(px(CONTROL_SM))
                .h(px(CONTROL_LG))
                .rounded(px(RADIUS_MD))
                .bg(control_bg())
                .flex()
                .items_center()
                .justify_center()
                .text_color(ink())
                .child(if increase { "+" } else { "−" })
                .when(!enabled, |this| this.opacity(0.4))
                .when(enabled, |this| {
                    this.cursor_pointer()
                        .hover(|style| style.bg(control_hover()))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            on_step(this, if increase { 1 } else { -1 });
                            cx.notify();
                        }))
                })
        };
        let key_step = on_step.clone();
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap_1()
            .child(
                keyboard_focusable(div().id(("slider", slider_id)))
                    .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, _, cx| {
                        let keystroke = &event.keystroke;
                        let modifiers = keystroke.modifiers;
                        if modifiers.control || modifiers.alt || modifiers.platform {
                            return;
                        }
                        let direction = match keystroke.key.as_str() {
                            "left" | "down" => -1,
                            "right" | "up" => 1,
                            _ => return,
                        };
                        cx.stop_propagation();
                        if (direction < 0 && !can_decrease) || (direction > 0 && !can_increase) {
                            return;
                        }
                        key_step(this, direction * if modifiers.shift { 10 } else { 1 });
                        cx.notify();
                    }))
                    .relative()
                    .flex_1()
                    .min_w(px(72.0))
                    .h(px(CONTROL_LG))
                    .rounded(px(RADIUS_MD))
                    .bg(control_bg())
                    .overflow_hidden()
                    .cursor(CursorStyle::ResizeLeftRight)
                    .child(
                        gpui::canvas(
                            move |bounds, _, _| {
                                measured.set(Some(bounds));
                            },
                            move |_, _, window, _| {
                                if dragging {
                                    window.set_window_cursor_style(CursorStyle::ResizeLeftRight);
                                }
                            },
                        )
                        .absolute()
                        .inset_0()
                        .size_full(),
                    )
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .top_0()
                            .bottom_0()
                            .w(gpui::relative(fraction))
                            .bg(accent_tint()),
                    )
                    .child(
                        div()
                            .absolute()
                            .top(px(7.0))
                            .bottom(px(7.0))
                            .left(gpui::relative(fraction))
                            .ml(px(-1.5))
                            .w(px(3.0))
                            .rounded_full()
                            .bg(accent()),
                    )
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .flex()
                            .items_center()
                            .justify_between()
                            .pl_3()
                            .pr_1()
                            .child(div().row_label().truncate().child(label))
                            .when(show_reset, |this| {
                                this.child(
                                    div()
                                        .id(("slider-reset", slider_id))
                                        .size(px(24.0))
                                        .flex()
                                        .flex_none()
                                        .items_center()
                                        .justify_center()
                                        .rounded(px(RADIUS_MD - 2.0))
                                        .text_color(muted())
                                        .cursor_pointer()
                                        .hover(|style| style.text_color(ink()).bg(control_hover()))
                                        .tooltip(crate::timeline_ui::tooltip("Reset"))
                                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            on_reset(this);
                                            cx.notify();
                                        }))
                                        .child("×"),
                                )
                            }),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            let Some(bounds) = track.get() else { return };
                            let mut drag = SliderDrag {
                                slider_id,
                                track_left: bounds.origin.x,
                                track_width: bounds.size.width,
                                value: 0,
                            };
                            if !on_press(this) {
                                return;
                            }
                            drag.value = drag.value_at(event.position.x);
                            this.slider_drag = Some(drag);
                            this.set_slider_value(slider_id, drag.value);
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    ),
            )
            .child(stepper(false, can_decrease))
            .child(
                div()
                    .flex_none()
                    .w(px(56.0))
                    .h(px(CONTROL_LG))
                    .rounded(px(RADIUS_MD))
                    .bg(control_bg())
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_xs()
                    .text_color(ink())
                    .tabular()
                    .child(value),
            )
            .child(stepper(true, can_increase))
            .into_any_element()
    }
}

/// Stable targets shared with `Studio::set_slider_value`, independent of UI labels.
#[derive(Clone, Copy)]
#[repr(usize)]
pub(crate) enum SliderTarget {
    Padding = 0,
    Shadow = 1,
    Corners = 2,
    BorderThickness = 3,
    BorderOpacity = 4,
    RedactionStrength = 5,
    FontSize = 6,
}

const SHADOW_COLORS: [(&str, u32); 8] = [
    ("Black", 0x000000),
    ("White", 0xffffff),
    ("Slate", 0x475569),
    ("Blue", 0x3678ef),
    ("Purple", 0x8c4ce8),
    ("Pink", 0xec3d87),
    ("Orange", 0xff8a24),
    ("Teal", 0x22bfc2),
];

impl Studio {
    pub(crate) fn shadow_color_picker(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(div().field_label().child("Color"))
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(2.0))
                    .children(SHADOW_COLORS.iter().enumerate().map(|(index, (name, color))| {
                        let color = *color;
                        swatch(("shadow-color", index), *name, self.shadow_color == color, rgb(color), None)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.shadow_color = color;
                                cx.notify();
                            }))
                    })),
            )
            .into_any_element()
    }

    pub(super) fn toggle(&self, enabled: bool) -> impl IntoElement {
        switch(enabled)
    }

    pub(super) fn tool_grid(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_wrap()
            .flex_none()
            .gap_1()
            .p_1()
            .rounded(px(RADIUS_LG))
            .bg(control_bg())
            .children(
                Tool::ALL
                    .into_iter()
                    .filter(|(tool, _)| {
                        // Redactions are baked into the still image; a recording
                        // draws its overlays live, so those two tools stay out.
                        self.video_project.is_none() || !matches!(tool, Tool::Blur | Tool::Pixelate)
                    })
                    .map(|(tool, icon)| {
                        let selected = self.tool == tool;
                        keyboard_focusable(div().id(icon))
                            .tooltip(crate::timeline_ui::tooltip(format!(
                                "{} · {}",
                                tool.label(),
                                tool.help_text()
                            )))
                            .w(px(42.0))
                            .h(px(42.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(inner_radius(RADIUS_LG, 4.0)))
                            .text_color(if selected { ink() } else { muted() })
                            .when(selected, |this| this.bg(control_selected()).shadow_sm())
                            .cursor_pointer()
                            .when(!selected, |this| this.hover(|style| style.bg(control_hover())))
                            .child(svg().path(icon).size(px(20.0)).text_color(ink()))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.cancel_annotation_gesture();
                                this.stop_editing_text();
                                this.tool = tool;
                                if tool != Tool::Select {
                                    this.selected_annotation = None;
                                    this.editing_text = None;
                                }
                                cx.notify();
                            }))
                    }),
            )
    }

    /// Annotation tools shared by still and timed scenes.
    pub(crate) fn video_annotate_section(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(self.tool_grid(cx))
            .when(!self.annotations.is_empty(), |this| {
                this.child(div().hint().child(format!(
                    "{} mark{}",
                    self.annotations.len(),
                    if self.annotations.len() == 1 { "" } else { "s" }
                )))
            })
            .into_any_element()
    }

    pub(super) fn segmented<F>(
        &self,
        control_id: &'static str,
        labels: &'static [&'static str],
        selected: usize,
        on_select: F,
        cx: &mut Context<Self>,
    ) -> impl IntoElement
    where
        F: Fn(&mut Studio, usize) + Clone + 'static,
    {
        let last = labels.len().saturating_sub(1);
        segmented_track().children(labels.iter().enumerate().map(|(index, label)| {
            let on_select = on_select.clone();
            let on_key = on_select.clone();
            option_button((control_id, index), selected == index)
                .flex_1()
                .px_1()
                // Left/Right move the selection along with keyboard focus.
                .on_key_down(cx.listener(move |this, event: &gpui::KeyDownEvent, window, cx| {
                    if event.keystroke.modifiers.modified() {
                        return;
                    }
                    let target = match event.keystroke.key.as_str() {
                        "left" if index > 0 => {
                            window.focus_prev();
                            index - 1
                        }
                        "right" if index < last => {
                            window.focus_next();
                            index + 1
                        }
                        "left" | "right" => {
                            cx.stop_propagation();
                            return;
                        }
                        _ => return,
                    };
                    cx.stop_propagation();
                    on_key(this, target);
                    cx.notify();
                }))
                .on_click(cx.listener(move |this, _, _, cx| {
                    on_select(this, index);
                    cx.notify();
                }))
                .child(div().truncate().child(*label))
        }))
    }

    /// `segmented` wrapped onto rows of `columns` options.
    pub(crate) fn option_grid<F>(
        &self,
        control_id: &'static str,
        labels: &'static [&'static str],
        columns: u16,
        selected: usize,
        on_select: F,
        cx: &mut Context<Self>,
    ) -> AnyElement
    where
        F: Fn(&mut Studio, usize) + Clone + 'static,
    {
        segmented_track()
            .grid()
            .grid_cols(columns)
            .children(labels.iter().enumerate().map(|(index, label)| {
                let on_select = on_select.clone();
                option_button((control_id, index), selected == index)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        on_select(this, index);
                        cx.notify();
                    }))
                    .child(div().truncate().child(*label))
            }))
            .into_any_element()
    }

    /// Corner picker shared by camera, watermark and overlay placement: a
    /// small grid, `columns` wide, whose cells show a dot where the object
    /// sits. Names show as tooltips. `selected` may be out of range when the
    /// object was dragged off the presets.
    pub(crate) fn position_grid<F>(
        &self,
        control_id: &'static str,
        names: &'static [&'static str],
        columns: usize,
        selected: usize,
        on_select: F,
        cx: &mut Context<Self>,
    ) -> AnyElement
    where
        F: Fn(&mut Studio, usize) + Clone + 'static,
    {
        let rows = names.len().div_ceil(columns.max(1));
        segmented_track()
            .w(px(36.0 * columns as f32 + 2.0 * TRACK_PADDING))
            .grid()
            .grid_cols(columns as u16)
            .children(names.iter().enumerate().map(|(index, name)| {
                let on_select = on_select.clone();
                let (column, row) = (index % columns, index / columns);
                let active = selected == index;
                let cell = option_button((control_id, index), active)
                    .px(px(6.0))
                    .py(px(6.0))
                    .tooltip(crate::timeline_ui::tooltip(*name))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        on_select(this, index);
                        cx.notify();
                    }));
                let cell = match column {
                    0 => cell.justify_start(),
                    c if c + 1 == columns => cell.justify_end(),
                    _ => cell.justify_center(),
                };
                let cell = match row {
                    0 => cell.items_start(),
                    r if r + 1 == rows => cell.items_end(),
                    _ => cell.items_center(),
                };
                cell.child(
                    div()
                        .size(px(6.0))
                        .rounded(px(1.5))
                        .bg(if active { ink() } else { muted().opacity(0.6) }),
                )
            }))
            .into_any_element()
    }

    pub(super) fn slider_row<F>(
        &self,
        target: SliderTarget,
        title: &'static str,
        value: u8,
        suffix: &'static str,
        on_change: F,
        cx: &mut Context<Self>,
    ) -> AnyElement
    where
        F: Fn(&mut Studio, u8) + 'static,
    {
        let slider_id = target as usize;
        let row = SliderRow {
            slider_id,
            label: title.into(),
            fraction: value as f32 / 100.0,
            value: format!("{value}{suffix}").into(),
            can_decrease: value > 0,
            can_increase: value < 100,
            show_reset: false,
        };
        self.slider_control(
            row,
            move |this| {
                if matches!(slider_id, 5 | 6) && this.selected_annotation.is_some() {
                    this.record_annotation_undo();
                }
                true
            },
            move |this, steps| on_change(this, (value as i32 + steps).clamp(0, 100) as u8),
            |_| {},
            cx,
        )
    }

    pub(super) fn annotation_text_field(&self, _index: usize, _cx: &mut Context<Self>) -> AnyElement {
        self.text_fields.annotation.clone().into_any_element()
    }

    pub(super) fn annotation_style_controls(&self, cx: &mut Context<Self>) -> gpui::Div {
        let target_tool = self
            .selected_annotation
            .and_then(|index| self.annotations.get(index))
            .map(|mark| mark.tool)
            .unwrap_or(self.tool);
        let supports_color = matches!(
            target_tool,
            Tool::Rectangle
                | Tool::FilledRectangle
                | Tool::Ellipse
                | Tool::Line
                | Tool::Arrow
                | Tool::Pen
                | Tool::Number
                | Tool::Text
        );
        let supports_stroke = matches!(
            target_tool,
            Tool::Rectangle | Tool::Ellipse | Tool::Line | Tool::Arrow | Tool::Pen
        );
        let supports_drawn = crate::annotation_geometry::supports_drawn(target_tool);
        let hand_drawn = self
            .selected_annotation
            .and_then(|i| self.annotations.get(i))
            .map(|mark| mark.hand_drawn)
            .unwrap_or(self.annotation_hand_drawn);
        let is_redaction = matches!(target_tool, Tool::Pixelate | Tool::Blur);
        let is_text = target_tool == Tool::Text;
        let selected_color = self
            .selected_annotation
            .and_then(|index| self.annotations.get(index))
            .map(|mark| mark.color)
            .unwrap_or(ANNOTATION_COLORS[self.annotation_color_index].1);
        let selected_stroke = self
            .selected_annotation
            .and_then(|index| self.annotations.get(index))
            .map(|mark| mark.stroke_width)
            .unwrap_or(self.annotation_stroke_width);
        let selected_text = self
            .selected_annotation
            .and_then(|index| self.annotations.get(index))
            .filter(|mark| mark.tool == Tool::Text);
        let selected_font_family = selected_text
            .map(|mark| mark.font_family)
            .unwrap_or(self.text_font_family);
        let selected_alignment = selected_text
            .map(|mark| mark.text_alignment)
            .unwrap_or(self.text_alignment);
        let selected_redaction_strength = self
            .selected_annotation
            .and_then(|index| self.annotations.get(index))
            .filter(|mark| mark.tool == Tool::Blur || mark.tool == Tool::Pixelate)
            .map(|mark| (mark.density * 100.0).round() as u8)
            .unwrap_or(self.redaction_strength);

        div()
            .flex()
            .flex_none()
            .flex_col()
            .gap_2()
            .when(supports_color, |this| {
                this.child(div().field_label().child("Color"))
                    .child(
                        div().flex().flex_wrap().gap(px(2.0)).children(
                            ANNOTATION_COLORS
                                .iter()
                                .enumerate()
                                .map(|(index, (name, color))| {
                                    let color = *color;
                                    swatch(
                                        ("annotation-color", index),
                                        *name,
                                        selected_color == color,
                                        rgb(color),
                                        None,
                                    )
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if this.selected_annotation.is_some() {
                                                this.record_annotation_undo();
                                            }
                                            this.annotation_color_index = index;
                                            if let Some(mark) =
                                                this.selected_annotation.and_then(|selected| {
                                                    this.annotations.get_mut(selected)
                                                })
                                            {
                                                mark.color = color;
                                            }
                                            cx.notify();
                                        }))
                                }),
                        ),
                    )
            })
            .when(supports_drawn, |this| {
                this.child(div().field_label().child("Style"))
                    .child(
                        segmented_track().children(
                            [(true, "Drawn"), (false, "Clean")]
                                .into_iter()
                                .enumerate()
                                .map(|(index, (drawn, label))| {
                                    option_button(("annotation-drawn-style", index), hand_drawn == drawn)
                                        .flex_1()
                                        .child(label)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            let selected: Vec<_> = this
                                                .annotation_selected_indices()
                                                .into_iter()
                                                .filter(|i| {
                                                    crate::annotation_geometry::supports_drawn(
                                                        this.annotations[*i].tool,
                                                    ) && this.annotations[*i].hand_drawn != drawn
                                                })
                                                .collect();
                                            if !selected.is_empty() {
                                                this.record_annotation_undo();
                                            }
                                            for i in selected {
                                                let mark = &mut this.annotations[i];
                                                mark.hand_drawn = drawn;
                                                if mark.draw_seed == 0 {
                                                    mark.draw_seed =
                                                        crate::annotation_geometry::new_draw_seed();
                                                }
                                            }
                                            this.annotation_hand_drawn = drawn;
                                            cx.notify();
                                        }))
                                }),
                        ),
                    )
            })
            .when(supports_stroke, |this| {
                this.child(div().field_label().child("Stroke width"))
                    .child(segmented_track().children(
                        [2.0_f32, 4.0, 6.0, 8.0, 12.0].into_iter().enumerate().map(
                            |(index, width)| {
                                option_button(
                                    ("annotation-stroke", index),
                                    (selected_stroke - width).abs() < 0.1,
                                )
                                    .flex_1()
                                    .tabular()
                                    .child(format!("{}", width as u8))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if this.selected_annotation.is_some() {
                                            this.record_annotation_undo();
                                        }
                                        this.annotation_stroke_width = width;
                                        if let Some(mark) = this
                                            .selected_annotation
                                            .and_then(|selected| this.annotations.get_mut(selected))
                                        {
                                            mark.stroke_width = width;
                                        }
                                        cx.notify();
                                    }))
                            },
                        ),
                    ))
            })
            .when(is_redaction, |this| {
                this.child(self.slider_row(
                    SliderTarget::RedactionStrength,
                    "Strength",
                    selected_redaction_strength,
                    "%",
                    |studio, value| {
                        if studio.selected_annotation.is_some() {
                            studio.record_annotation_undo();
                        }
                        studio.redaction_strength = value.clamp(15, 100);
                        if let Some(mark) = studio
                            .selected_annotation
                            .and_then(|index| studio.annotations.get_mut(index))
                        {
                            mark.density = studio.redaction_strength as f32 / 100.0;
                        }
                        let _ = studio.rebuild_redactions();
                    },
                    cx,
                ))
            })
            .when(is_text, |this| {
                let size_value = self
                    .selected_annotation
                    .and_then(|index| self.annotations.get(index))
                    .map(|mark| self.annotation_text_preview_size(mark))
                    .unwrap_or(self.text_font_size);
                this.child(div().field_label().child("Text"))
                    .when_some(
                        self.selected_annotation.filter(|_| selected_text.is_some()),
                        |this, index| this.child(self.annotation_text_field(index, cx)),
                    )
                    .child(segmented_track().children(
                        ["Pro", "Compact", "Rounded", "Hand"].into_iter().enumerate().map(
                            |(index, label)| {
                                option_button(("text-family", index), selected_font_family as usize == index)
                                    .flex_1()
                                    .when(index == 3, |this| {
                                        this.font_family(crate::fonts::HANDWRITTEN_FAMILY)
                                            .text_base()
                                    })
                                    .child(label)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if this.selected_annotation.is_some() {
                                            this.record_annotation_undo();
                                        }
                                        this.text_font_family = index as u8;
                                        if let Some(mark) = this
                                            .selected_annotation
                                            .and_then(|i| this.annotations.get_mut(i))
                                        {
                                            mark.font_family = index as u8;
                                        }
                                        cx.notify();
                                    }))
                            },
                        ),
                    ))
                    .child(self.slider_row(
                        SliderTarget::FontSize,
                        "Font size",
                        size_value.round().clamp(10.0, 96.0) as u8,
                        " px",
                        |studio, value| {
                            if studio.selected_annotation.is_some() {
                                studio.record_annotation_undo();
                            }
                            studio.set_slider_value(6, value);
                        },
                        cx,
                    ))
                    .child(
                        segmented_track().w_auto().children(
                            [("B", 0_usize), ("I", 1), ("U", 2)].into_iter().map(
                                |(label, style)| {
                                    let enabled = match style {
                                        0 => selected_text
                                            .map(|mark| mark.bold)
                                            .unwrap_or(self.text_bold),
                                        1 => selected_text
                                            .map(|mark| mark.italic)
                                            .unwrap_or(self.text_italic),
                                        _ => selected_text
                                            .map(|mark| mark.underline)
                                            .unwrap_or(self.text_underline),
                                    };
                                    option_button(("text-style", style), enabled)
                                        .w(px(42.0))
                                        .text_sm()
                                        .when(style == 0, |this| this.font_weight(FontWeight::SEMIBOLD))
                                        .when(style == 1, |this| this.italic())
                                        .when(style == 2, |this| this.underline())
                                        .child(label)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if this.selected_annotation.is_some() {
                                                this.record_annotation_undo();
                                            }
                                            match style {
                                                0 => this.text_bold = !enabled,
                                                1 => this.text_italic = !enabled,
                                                _ => this.text_underline = !enabled,
                                            }
                                            if let Some(mark) = this
                                                .selected_annotation
                                                .and_then(|index| this.annotations.get_mut(index))
                                            {
                                                match style {
                                                    0 => mark.bold = !enabled,
                                                    1 => mark.italic = !enabled,
                                                    _ => mark.underline = !enabled,
                                                }
                                            }
                                            cx.notify();
                                        }))
                                },
                            ),
                        ),
                    )
                    .child(
                        segmented_track().children(
                            ["Left", "Center", "Right", "Justify"]
                                .into_iter()
                                .enumerate()
                                .map(|(index, label)| {
                                    option_button(("text-align", index), selected_alignment as usize == index)
                                        .flex_1()
                                        .child(label)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if this.selected_annotation.is_some() {
                                                this.record_annotation_undo();
                                            }
                                            this.text_alignment = index as u8;
                                            if let Some(mark) = this
                                                .selected_annotation
                                                .and_then(|i| this.annotations.get_mut(i))
                                            {
                                                mark.text_alignment = index as u8;
                                            }
                                            cx.notify();
                                        }))
                                }),
                        ),
                    )
            })
    }

    /// Background swatches for solid colors, gradients, and wallpapers.
    pub(super) fn fill_picker(&self, cx: &mut Context<Self>) -> gpui::Div {
        let grid = div().flex().flex_none().flex_wrap().gap(px(2.0)).w_full();
        match self.wallpaper_tab {
            0 => grid.children(SOLID_BACKGROUNDS.iter().enumerate().map(
                |(index, (name, color))| {
                    swatch(("background-color", index), *name, self.color_index == index, rgb(*color), None)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.color_index = index;
                            this.custom_wallpaper = None;
                            cx.notify();
                        }))
                },
            )),
            1 => grid.children(GRADIENT_BACKGROUNDS.iter().copied().enumerate().map(
                |(index, preset)| {
                    let (base, overlay) = gradient_layers(preset);
                    swatch(
                        ("background-gradient", index),
                        preset.title,
                        self.gradient_index == index,
                        base,
                        Some(overlay),
                    )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.gradient_index = index;
                            this.custom_wallpaper = None;
                            cx.notify();
                        }))
                },
            )),
            _ => {
                let paths = &CURATED_WALLPAPERS;
                div().grid().grid_cols(3).gap_2().w_full().children(
                    paths
                        .iter()
                        .enumerate()
                        .filter(|(index, path)| {
                            self.section_open("wallpaper-browser")
                                || *index < 6
                                || (self.custom_wallpaper.is_none()
                                    && self.wallpaper_asset == **path)
                        })
                        .map(|(index, path)| {
                            let path = *path;
                            let selected = self.custom_wallpaper.is_none() && self.wallpaper_asset == path;
                            // The selection ring sits 2 px outside the image,
                            // like `swatch`; the image carries its own radius
                            // and a 1 px inner outline.
                            keyboard_focusable(div().id(("wallpaper-tile", index)))
                                .relative()
                                .h(px(64.0))
                                .p(px(2.0))
                                .rounded(px(RADIUS_MD + 4.0))
                                .border_2()
                                .border_color(if selected { accent() } else { gpui::transparent_black() })
                                .when(!selected, |this| this.hover(|style| style.border_color(line())))
                                .cursor_pointer()
                                .child(
                                    img(path)
                                        .size_full()
                                        .rounded(px(RADIUS_MD))
                                        .object_fit(ObjectFit::Cover),
                                )
                                .child(
                                    div()
                                        .absolute()
                                        .inset(px(2.0))
                                        .rounded(px(RADIUS_MD))
                                        .border_1()
                                        .border_color(ring()),
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.wallpaper_asset = path;
                                    this.custom_wallpaper = None;
                                    cx.notify();
                                }))
                        }),
                )
            }
        }
    }
}
