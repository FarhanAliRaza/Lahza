//! Reusable editor controls and annotation appearance settings.

use super::{
    blue, gradient_layers, ink, line, muted, SliderDrag, Studio, Tool, ANNOTATION_COLORS,
    CURATED_WALLPAPERS, GRADIENT_BACKGROUNDS, SOLID_BACKGROUNDS,
};
use gpui::{
    div, hsla, img, prelude::*, px, rgb, svg, AnyElement, Context, CursorStyle, FontWeight,
    IntoElement, MouseButton, MouseDownEvent, ObjectFit,
};

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
        let selected_name = SHADOW_COLORS
            .iter()
            .find(|(_, color)| *color == self.shadow_color)
            .map(|(name, _)| (*name).to_string())
            .unwrap_or_else(|| format!("#{:06X}", self.shadow_color));
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .justify_between()
                    .text_xs()
                    .text_color(muted())
                    .child("Color")
                    .child(selected_name),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .children(SHADOW_COLORS.iter().enumerate().map(|(index, (_, color))| {
                        let color = *color;
                        let selected = self.shadow_color == color;
                        div()
                            .id(("shadow-color", index))
                            .size(px(30.0))
                            .p(px(3.0))
                            .rounded_md()
                            .border_2()
                            .border_color(if selected { blue() } else { line() })
                            .cursor_pointer()
                            .hover(|style| style.border_color(blue()))
                            .child(div().size_full().rounded_sm().bg(rgb(color)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.shadow_color = color;
                                cx.notify();
                            }))
                    })),
            )
            .when(self.shadow == 0, |this| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(muted())
                        .child("Increase Amount to show the shadow."),
                )
            })
            .into_any_element()
    }

    pub(super) fn toggle(&self, enabled: bool) -> impl IntoElement {
        div()
            .w(px(38.0))
            .h(px(22.0))
            .p(px(2.0))
            .rounded_full()
            .bg(if enabled {
                blue()
            } else {
                hsla(220.0 / 360.0, 0.03, 0.85, 1.0)
            })
            .flex()
            .justify_end()
            .when(!enabled, |this| this.justify_start())
            .child(
                div()
                    .size(px(18.0))
                    .rounded_full()
                    .bg(rgb(0xffffff))
                    .shadow_sm(),
            )
    }

    pub(super) fn tool_grid(&self, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .flex_wrap()
            .flex_none()
            .h(px(90.0))
            .gap_1()
            .p_1()
            .rounded_lg()
            .bg(rgb(0xf4f4f5))
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
                        div()
                            .id(icon)
                            .w(px(42.0))
                            .h(px(42.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_md()
                            .text_color(if selected { ink() } else { muted() })
                            .bg(if selected {
                                rgb(0xe2e3e5)
                            } else {
                                rgb(0xf4f4f5)
                            })
                            .cursor_pointer()
                            .hover(|style| style.bg(rgb(0xe8e9eb)))
                            .child(svg().path(icon).size(px(20.0)).text_color(if selected {
                                blue()
                            } else {
                                ink()
                            }))
                            .on_click(cx.listener(move |this, _, _, cx| {
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
        let hint = if self.annotations.is_empty() {
            "Pick a tool and draw on the canvas.".to_string()
        } else {
            format!("{} — {}", self.tool.label(), self.tool.help_text())
        };
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::BOLD)
                            .child("Annotate"),
                    )
                    .when(!self.annotations.is_empty(), |this| {
                        this.child(div().text_xs().text_color(muted()).child(format!(
                            "{} mark{}",
                            self.annotations.len(),
                            if self.annotations.len() == 1 { "" } else { "s" }
                        )))
                    }),
            )
            .child(self.tool_grid(cx))
            .child(div().text_xs().text_color(muted()).child(hint))
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
        div()
            .flex()
            .flex_none()
            .w_full()
            .h(px(34.0))
            .p(px(3.0))
            .rounded_lg()
            .bg(rgb(0xf0f0f1))
            .children(labels.iter().enumerate().map(|(index, label)| {
                let on_select = on_select.clone();
                div()
                    .id((control_id, index))
                    .flex_1()
                    .min_w_0()
                    .px_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_md()
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_xs()
                    .text_color(if selected == index { ink() } else { muted() })
                    .when(selected == index, |this| this.bg(rgb(0xffffff)).shadow_sm())
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        on_select(this, index);
                        cx.notify();
                    }))
                    .child(div().truncate().child(*label))
            }))
    }

    pub(super) fn slider_row<F>(
        &self,
        target: SliderTarget,
        title: &'static str,
        value: u8,
        suffix: &'static str,
        on_change: F,
        cx: &mut Context<Self>,
    ) -> impl IntoElement
    where
        F: Fn(&mut Studio, u8) + Clone + 'static,
    {
        let slider_id = target as usize;
        let decrease = on_change.clone();
        let increase = on_change;
        div()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .child(
                div()
                    .id(("slider", slider_id))
                    .relative()
                    .flex_1()
                    .h(px(40.0))
                    .rounded_lg()
                    .bg(rgb(0xf1f1f2))
                    .overflow_hidden()
                    .cursor_pointer()
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            if matches!(slider_id, 5 | 6) && this.selected_annotation.is_some() {
                                this.record_annotation_undo();
                            }
                            this.slider_drag = Some(SliderDrag {
                                slider_id,
                                start_x: event.position.x,
                                start_value: value,
                            });
                            cx.notify();
                        }),
                    )
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .top_0()
                            .bottom_0()
                            .w(gpui::relative(value as f32 / 100.0))
                            .bg(hsla(211.0 / 360.0, 0.9, 0.88, 0.45)),
                    )
                    .child(
                        div()
                            .absolute()
                            .inset_0()
                            .flex()
                            .items_center()
                            .px_3()
                            .text_sm()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(title),
                    ),
            )
            .child(
                div()
                    .id(("slider-minus", slider_id))
                    .w(px(28.0))
                    .h(px(40.0))
                    .rounded_lg()
                    .bg(rgb(0xf1f1f2))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .child("−")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        decrease(this, value.saturating_sub(2));
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .w(px(58.0))
                    .h(px(40.0))
                    .rounded_lg()
                    .bg(rgb(0xf1f1f2))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_sm()
                    .child(format!("{value}{suffix}")),
            )
            .child(
                div()
                    .id(("slider-plus", slider_id))
                    .w(px(28.0))
                    .h(px(40.0))
                    .rounded_lg()
                    .bg(rgb(0xf1f1f2))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .child("+")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        increase(this, value.saturating_add(2).min(100));
                        cx.notify();
                    })),
            )
    }

    pub(super) fn annotation_text_field(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let editing = self.editing_text == Some(index);
        let text = self
            .annotations
            .get(index)
            .map(|mark| mark.text.clone())
            .unwrap_or_default();
        let empty = text.is_empty();
        div()
            .id("annotation-text-field")
            .w_full()
            .h(px(32.0))
            .px_2()
            .rounded_md()
            .border_1()
            .border_color(if editing {
                rgb(0x2997ff)
            } else {
                rgb(0xd9d9dc)
            })
            .bg(rgb(0xffffff))
            .flex()
            .items_center()
            .text_sm()
            .text_color(if empty && !editing { muted() } else { ink() })
            .cursor(CursorStyle::IBeam)
            .child(if editing {
                format!("{text}{}", if self.caret_visible { "|" } else { " " })
            } else if empty {
                "Click to type".to_string()
            } else {
                text
            })
            .on_click(cx.listener(move |this, _, _, cx| {
                if index < this.annotations.len() {
                    this.record_annotation_undo();
                    this.selected_annotation = Some(index);
                    this.editing_text = Some(index);
                    this.caret_visible = true;
                    this.tool = Tool::Select;
                }
                cx.notify();
            }))
            .into_any_element()
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
                this.child(div().text_xs().text_color(muted()).child("Color"))
                    .child(
                        div().flex().flex_wrap().gap_2().children(
                            ANNOTATION_COLORS
                                .iter()
                                .enumerate()
                                .map(|(index, (_, color))| {
                                    let color = *color;
                                    div()
                                        .id(("annotation-color", index))
                                        .size(px(25.0))
                                        .rounded_md()
                                        .bg(rgb(color))
                                        .border_1()
                                        .border_color(if selected_color == color {
                                            blue()
                                        } else {
                                            line()
                                        })
                                        .cursor_pointer()
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
            .when(supports_stroke, |this| {
                this.child(div().text_xs().text_color(muted()).child("Stroke width"))
                    .child(div().flex().gap_1().children(
                        [2.0_f32, 4.0, 6.0, 8.0, 12.0].into_iter().enumerate().map(
                            |(index, width)| {
                                div()
                                    .id(("annotation-stroke", index))
                                    .flex_1()
                                    .h(px(32.0))
                                    .rounded_md()
                                    .bg(if (selected_stroke - width).abs() < 0.1 {
                                        rgb(0xffffff)
                                    } else {
                                        rgb(0xf0f0f1)
                                    })
                                    .when((selected_stroke - width).abs() < 0.1, |this| {
                                        this.shadow_sm().border_1().border_color(line())
                                    })
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_xs()
                                    .cursor_pointer()
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
                    .map(|mark| mark.font_size)
                    .unwrap_or(self.text_font_size);
                this.child(div().text_xs().text_color(muted()).child("Text"))
                    .when_some(
                        self.selected_annotation.filter(|_| selected_text.is_some()),
                        |this, index| this.child(self.annotation_text_field(index, cx)),
                    )
                    .child(div().flex().gap_1().children(
                        ["Pro", "Compact", "Rounded"].into_iter().enumerate().map(
                            |(index, label)| {
                                div()
                                    .id(("text-family", index))
                                    .flex_1()
                                    .h(px(32.0))
                                    .rounded_md()
                                    .bg(if selected_font_family as usize == index {
                                        rgb(0xdcecff)
                                    } else {
                                        rgb(0xf0f0f1)
                                    })
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .text_xs()
                                    .cursor_pointer()
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
                        " pt",
                        |studio, value| {
                            if studio.selected_annotation.is_some() {
                                studio.record_annotation_undo();
                            }
                            studio.set_slider_value(6, value);
                        },
                        cx,
                    ))
                    .child(
                        div().flex().gap_2().children(
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
                                    div()
                                        .id(("text-style", style))
                                        .w(px(42.0))
                                        .h(px(32.0))
                                        .rounded_md()
                                        .bg(if enabled {
                                            rgb(0xdcecff)
                                        } else {
                                            rgb(0xf0f0f1)
                                        })
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .cursor_pointer()
                                        .child(label)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            if this.selected_annotation.is_some() {
                                                this.record_annotation_undo();
                                            }
                                            match style {
                                                0 => this.text_bold = !this.text_bold,
                                                1 => this.text_italic = !this.text_italic,
                                                _ => this.text_underline = !this.text_underline,
                                            }
                                            if let Some(mark) = this
                                                .selected_annotation
                                                .and_then(|index| this.annotations.get_mut(index))
                                            {
                                                match style {
                                                    0 => mark.bold = !mark.bold,
                                                    1 => mark.italic = !mark.italic,
                                                    _ => mark.underline = !mark.underline,
                                                }
                                            }
                                            cx.notify();
                                        }))
                                },
                            ),
                        ),
                    )
                    .child(
                        div().flex().gap_1().children(
                            ["Left", "Center", "Right", "Justify"]
                                .into_iter()
                                .enumerate()
                                .map(|(index, label)| {
                                    div()
                                        .id(("text-align", index))
                                        .flex_1()
                                        .h(px(30.0))
                                        .rounded_md()
                                        .bg(if selected_alignment as usize == index {
                                            rgb(0xdcecff)
                                        } else {
                                            rgb(0xf0f0f1)
                                        })
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .text_size(px(10.0))
                                        .cursor_pointer()
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
        let grid = div().flex().flex_none().flex_wrap().gap_2().w_full();
        match self.wallpaper_tab {
            0 => grid.children(SOLID_BACKGROUNDS.iter().enumerate().map(
                |(index, (_, color))| {
                    div()
                        .id(("background-color", index))
                        .size(px(27.0))
                        .rounded_md()
                        .bg(rgb(*color))
                        .cursor_pointer()
                        .when(self.color_index == index, |this| {
                            this.border_2().border_color(blue())
                        })
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
                    div()
                        .id(("background-gradient", index))
                        .relative()
                        .size(px(27.0))
                        .rounded_md()
                        .overflow_hidden()
                        .bg(base)
                        .cursor_pointer()
                        .child(div().absolute().inset_0().bg(overlay))
                        .when(self.gradient_index == index, |this| {
                            this.border_2().border_color(blue())
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.gradient_index = index;
                            this.custom_wallpaper = None;
                            cx.notify();
                        }))
                },
            )),
            _ => {
                let paths = &CURATED_WALLPAPERS;
                grid.children(
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
                            div()
                                .id(("wallpaper-tile", index))
                                .w(px(100.0))
                                .h(px(64.0))
                                .rounded_lg()
                                .overflow_hidden()
                                .cursor_pointer()
                                .when(
                                    self.custom_wallpaper.is_none() && self.wallpaper_asset == path,
                                    |this| this.border_2().border_color(blue()),
                                )
                                .child(img(path).size_full().object_fit(ObjectFit::Cover))
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
