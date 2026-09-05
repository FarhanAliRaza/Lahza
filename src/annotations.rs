//! Annotation interaction, canvas painting, redaction, and SVG serialization.

use super::{
    xml_escape, AnnotationMark, AnnotationTiming, NormPoint, Studio, Tool, ANNOTATION_COLORS,
};
use crate::recording::viewport::visible_rect;
use gpui::{
    font, hsla, point, px, quad, rgb, size, App, Bounds, FontWeight, Hsla, KeyDownEvent,
    PathBuilder, Pixels, Point, TextRun, UnderlineStyle, Window,
};
use std::{fmt::Write as _, fs};

pub(super) fn norm_to_screen(point_: NormPoint, image: Bounds<Pixels>) -> Point<Pixels> {
    point(
        image.origin.x + image.size.width * point_.x,
        image.origin.y + image.size.height * point_.y,
    )
}

pub(super) fn screen_to_norm(point_: Point<Pixels>, image: Bounds<Pixels>) -> NormPoint {
    NormPoint {
        x: ((point_.x - image.origin.x) / image.size.width).clamp(0.0, 1.0),
        y: ((point_.y - image.origin.y) / image.size.height).clamp(0.0, 1.0),
    }
}

fn mark_screen_bounds(mark: &AnnotationMark, image: Bounds<Pixels>) -> Bounds<Pixels> {
    // A pen stroke's start/end are only its first and last samples; the
    // stroke itself can wander anywhere, so bound every recorded point.
    if mark.tool == Tool::Pen && mark.points.len() > 1 {
        let mut min_x = f32::MAX;
        let mut min_y = f32::MAX;
        let mut max_x = f32::MIN;
        let mut max_y = f32::MIN;
        for normalized in &mark.points {
            let screen = norm_to_screen(*normalized, image);
            min_x = min_x.min(screen.x / px(1.0));
            min_y = min_y.min(screen.y / px(1.0));
            max_x = max_x.max(screen.x / px(1.0));
            max_y = max_y.max(screen.y / px(1.0));
        }
        return Bounds::from_corners(point(px(min_x), px(min_y)), point(px(max_x), px(max_y)));
    }
    let start = norm_to_screen(mark.start, image);
    let end = norm_to_screen(mark.end, image);
    Bounds::from_corners(
        point(start.x.min(end.x), start.y.min(end.y)),
        point(start.x.max(end.x), start.y.max(end.y)),
    )
}

fn mark_hit_bounds(mark: &AnnotationMark, image: Bounds<Pixels>) -> Bounds<Pixels> {
    let bounds = mark_screen_bounds(mark, image);
    let minimum = px(14.0);
    let extra_x = ((minimum - bounds.size.width).max(px(0.0))) * 0.5 + px(5.0);
    let extra_y = ((minimum - bounds.size.height).max(px(0.0))) * 0.5 + px(5.0);
    Bounds::from_corners(
        point(bounds.origin.x - extra_x, bounds.origin.y - extra_y),
        point(
            bounds.origin.x + bounds.size.width + extra_x,
            bounds.origin.y + bounds.size.height + extra_y,
        ),
    )
}

pub(crate) fn paint_annotation(
    mark: &AnnotationMark,
    image: Bounds<Pixels>,
    is_draft: bool,
    show_text_caret: bool,
    window: &mut Window,
    cx: &mut App,
) -> Bounds<Pixels> {
    let bounds = mark_screen_bounds(mark, image);
    let mut rendered_bounds = bounds;
    let color = Hsla::from(rgb(mark.color)).opacity(mark.opacity.clamp(0.0, 1.0));
    let clear = hsla(0.0, 0.0, 0.0, 0.0);
    match mark.tool {
        Tool::Rectangle => window.paint_quad(quad(
            bounds,
            px(2.0),
            clear,
            px(mark.stroke_width),
            color,
            Default::default(),
        )),
        Tool::FilledRectangle => window.paint_quad(quad(
            bounds,
            px(2.0),
            color,
            px(0.0),
            clear,
            Default::default(),
        )),
        Tool::Ellipse | Tool::Number => {
            let radius = if bounds.size.width < bounds.size.height {
                bounds.size.width * 0.5
            } else {
                bounds.size.height * 0.5
            };
            window.paint_quad(quad(
                bounds,
                radius,
                if mark.tool == Tool::Number {
                    color
                } else {
                    clear
                },
                if mark.tool == Tool::Ellipse {
                    px(mark.stroke_width)
                } else {
                    px(0.0)
                },
                color,
                Default::default(),
            ));
        }
        Tool::Line | Tool::Arrow => {
            let start = norm_to_screen(mark.start, image);
            let end = norm_to_screen(mark.end, image);
            let mut builder = PathBuilder::stroke(px(mark.stroke_width));
            builder.move_to(start);
            builder.line_to(end);
            if mark.tool == Tool::Arrow {
                let dx = (end.x - start.x) / px(1.0);
                let dy = (end.y - start.y) / px(1.0);
                let length = (dx * dx + dy * dy).sqrt().max(1.0);
                let ux = dx / length;
                let uy = dy / length;
                let head = 10.0 + mark.stroke_width * 2.0;
                let wing = 5.0 + mark.stroke_width;
                builder.move_to(end);
                builder.line_to(point(
                    end.x + px(-ux * head + -uy * wing),
                    end.y + px(-uy * head + ux * wing),
                ));
                builder.move_to(end);
                builder.line_to(point(
                    end.x + px(-ux * head + uy * wing),
                    end.y + px(-uy * head + -ux * wing),
                ));
            }
            if let Ok(path) = builder.build() {
                window.paint_path(path, color);
            }
        }
        Tool::Pen => {
            if mark.points.len() > 1 {
                let mut builder = PathBuilder::stroke(px(mark.stroke_width));
                for (index, point_) in mark.points.iter().copied().enumerate() {
                    let point = norm_to_screen(point_, image);
                    if index == 0 {
                        builder.move_to(point)
                    } else {
                        builder.line_to(point)
                    }
                }
                if let Ok(path) = builder.build() {
                    window.paint_path(path, color);
                }
            }
        }
        Tool::Pixelate if is_draft => {
            let cell = px(10.0);
            let columns = (bounds.size.width / cell).ceil().max(1.0) as usize;
            let rows = (bounds.size.height / cell).ceil().max(1.0) as usize;
            for row in 0..rows {
                for column in 0..columns {
                    let color = if (row + column) % 2 == 0 {
                        0x363a40
                    } else {
                        0x747b84
                    };
                    let cell_x = cell * column;
                    let cell_y = cell * row;
                    window.paint_quad(quad(
                        Bounds {
                            origin: point(bounds.origin.x + cell_x, bounds.origin.y + cell_y),
                            size: size(
                                cell.min(bounds.size.width - cell_x),
                                cell.min(bounds.size.height - cell_y),
                            ),
                        },
                        px(0.0),
                        rgb(color),
                        px(0.0),
                        clear,
                        Default::default(),
                    ));
                }
            }
        }
        Tool::Blur if is_draft => {
            window.paint_quad(quad(
                bounds,
                px(8.0),
                hsla(210.0 / 360.0, 0.08, 0.72, 0.45),
                px(2.0),
                rgb(0xffffff),
                Default::default(),
            ));
        }
        Tool::Text => {
            let display = mark.text.as_str();
            let font_size = px(mark.font_size.max(8.0));
            let inset = px(3.0);
            let mut caret_x = match mark.text_alignment {
                1 => bounds.center().x,
                2 => bounds.origin.x + bounds.size.width - inset,
                _ => bounds.origin.x + inset,
            };
            if !display.is_empty() {
                let family = match mark.font_family {
                    1 => "DejaVu Sans Condensed",
                    2 => "Ubuntu",
                    _ => "Noto Sans",
                };
                let mut text_font = font(family);
                text_font.weight = if mark.bold {
                    FontWeight::BOLD
                } else {
                    FontWeight::NORMAL
                };
                if mark.italic {
                    text_font = text_font.italic();
                }
                let run = TextRun {
                    len: display.len(),
                    font: text_font,
                    color,
                    background_color: None,
                    underline: mark.underline.then_some(UnderlineStyle {
                        color: Some(color),
                        thickness: px((mark.font_size / 18.0).max(1.0)),
                        wavy: false,
                    }),
                    strikethrough: None,
                };
                let line = window.text_system().shape_line(
                    display.to_string().into(),
                    font_size,
                    &[run],
                    None,
                );
                let origin_x = match mark.text_alignment {
                    1 => bounds.center().x - line.width * 0.5,
                    2 => bounds.origin.x + bounds.size.width - line.width,
                    _ => bounds.origin.x,
                };
                caret_x = origin_x + line.width + px(2.0);
                rendered_bounds = Bounds {
                    origin: point(origin_x - inset, bounds.origin.y),
                    size: size((line.width + px(8.0)).max(px(16.0)), font_size * 1.25),
                };
                let _ = line.paint(
                    point(origin_x, bounds.origin.y),
                    font_size * 1.25,
                    window,
                    cx,
                );
            }
            if display.is_empty() {
                rendered_bounds = Bounds {
                    origin: bounds.origin,
                    size: size(px(16.0), font_size * 1.25),
                };
            }
            if show_text_caret {
                window.paint_quad(quad(
                    Bounds {
                        origin: point(caret_x, bounds.origin.y + font_size * 0.12),
                        size: size(px(1.0), font_size * 0.88),
                    },
                    px(0.0),
                    rgb(0x202124),
                    px(0.0),
                    clear,
                    Default::default(),
                ));
            }
        }
        Tool::Pixelate | Tool::Blur | Tool::Highlight | Tool::Select => {}
    }

    if mark.tool == Tool::Number {
        let label = mark.number.to_string();
        let run = TextRun {
            len: label.len(),
            font: font("Inter").bold(),
            color: rgb(0xffffff).into(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let font_size = px((bounds.size.height / px(1.0) * 0.48).clamp(11.0, 30.0));
        let line = window
            .text_system()
            .shape_line(label.clone().into(), font_size, &[run], None);
        let origin = point(
            bounds.center().x - line.width * 0.5,
            bounds.center().y - font_size * 0.62,
        );
        let _ = line.paint(origin, font_size * 1.25, window, cx);
    }
    rendered_bounds
}

pub(crate) fn paint_highlights(
    marks: &[AnnotationMark],
    image: Bounds<Pixels>,
    window: &mut Window,
) {
    let holes: Vec<_> = marks
        .iter()
        .filter(|mark| mark.tool == Tool::Highlight)
        .map(|mark| mark_screen_bounds(mark, image))
        .collect();
    if holes.is_empty() {
        return;
    }

    let mut xs = vec![image.origin.x, image.origin.x + image.size.width];
    let mut ys = vec![image.origin.y, image.origin.y + image.size.height];
    for hole in &holes {
        xs.extend([hole.origin.x, hole.origin.x + hole.size.width]);
        ys.extend([hole.origin.y, hole.origin.y + hole.size.height]);
    }
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    ys.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    xs.dedup();
    ys.dedup();
    let dim = hsla(0.0, 0.0, 0.0, 0.55);
    let clear = hsla(0.0, 0.0, 0.0, 0.0);
    for x in xs.windows(2) {
        for y in ys.windows(2) {
            let cell = Bounds::from_corners(point(x[0], y[0]), point(x[1], y[1]));
            let center = cell.center();
            if !holes.iter().any(|hole| hole.contains(&center)) {
                window.paint_quad(quad(cell, px(0.0), dim, px(0.0), clear, Default::default()));
            }
        }
    }
}

impl Studio {
    pub(super) fn record_annotation_undo(&mut self) {
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
        true
    }

    /// Return keyboard control to the timeline when leaving canvas editing.
    pub(super) fn finish_annotation_interaction(&mut self) {
        self.stop_editing_text();
        self.selected_annotation = None;
        self.selection_last_point = None;
        self.selection_resizing = false;
        self.toast = None;
    }

    pub(super) fn stop_editing_text(&mut self) {
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
        }
        if self.tool == Tool::Text {
            self.tool = Tool::Select;
        }
    }

    pub(super) fn fit_text_box_to_content(&mut self, index: usize) {
        let aspect = self
            .media_dimensions()
            .map(|(width, height)| width as f32 / height.max(1) as f32)
            .unwrap_or(16.0 / 9.0)
            .max(0.1);
        let Some(mark) = self.annotations.get_mut(index) else {
            return;
        };
        if mark.tool != Tool::Text {
            return;
        }
        let height_norm = (mark.end.y - mark.start.y).abs().max(0.001);
        let preview_height = (mark.font_size * 1.35).max(16.0);
        let preview_image_height = preview_height / height_norm;
        let preview_image_width = preview_image_height * aspect;
        let character_count = mark.text.chars().count() as f32;
        let desired_width = if character_count == 0.0 {
            16.0
        } else {
            character_count * mark.font_size * 0.59 + 8.0
        };
        mark.end.x = (mark.start.x + desired_width / preview_image_width.max(1.0)).min(1.0);
    }

    pub(super) fn pointer_down(
        &mut self,
        position: Point<Pixels>,
        image: Bounds<Pixels>,
        rendered_bounds: &[Bounds<Pixels>],
    ) {
        // GPUI can retain more than one paint-scoped mouse listener across a
        // redraw. Treat a physical press as one editing transaction so a
        // single click cannot create stacked duplicate annotations.
        if self.pointer_is_down {
            return;
        }
        self.pointer_is_down = true;
        if !image.contains(&position) {
            return;
        }
        if let Some(index) = self.editing_text {
            let clicked_editing_text = rendered_bounds
                .get(index)
                .copied()
                .unwrap_or_else(|| mark_hit_bounds(&self.annotations[index], image))
                .contains(&position);
            if clicked_editing_text {
                self.selected_annotation = Some(index);
                self.caret_visible = true;
                if self.tool != Tool::Select {
                    self.tool = Tool::Select;
                }
            } else {
                self.stop_editing_text();
            }
        }
        let normalized = screen_to_norm(position, image);
        if self.tool == Tool::Select {
            self.selected_annotation =
                self.annotations
                    .iter()
                    .enumerate()
                    .rposition(|(index, mark)| {
                        rendered_bounds
                            .get(index)
                            .copied()
                            .unwrap_or_else(|| mark_hit_bounds(mark, image))
                            .contains(&position)
                    });
            self.selection_last_point = self.selected_annotation.map(|_| position);
            self.selection_resizing = self.selected_annotation.is_some_and(|index| {
                let bounds = rendered_bounds
                    .get(index)
                    .copied()
                    .unwrap_or_else(|| mark_screen_bounds(&self.annotations[index], image));
                (position.x - (bounds.origin.x + bounds.size.width)).abs() <= px(14.0)
                    && (position.y - (bounds.origin.y + bounds.size.height)).abs() <= px(14.0)
            });
            if self.selected_annotation.is_some() {
                self.record_annotation_undo();
            }
            self.editing_text = self.selected_annotation.filter(|index| {
                self.annotations
                    .get(*index)
                    .is_some_and(|mark| mark.tool == Tool::Text)
            });
            if self.editing_text.is_some() {
                self.caret_visible = true;
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
        self.record_annotation_undo();
        let color =
            ANNOTATION_COLORS[self.annotation_color_index.min(ANNOTATION_COLORS.len() - 1)].1;
        let mut mark = AnnotationMark {
            tool: self.tool,
            start: normalized,
            end: normalized,
            points: vec![normalized],
            number,
            color,
            stroke_width: self.annotation_stroke_width,
            density: self.redaction_strength as f32 / 100.0,
            text: String::new(),
            font_size: self.text_font_size,
            font_family: self.text_font_family,
            text_alignment: self.text_alignment,
            bold: self.text_bold,
            italic: self.text_italic,
            underline: self.text_underline,
            timing: self.scene_is_timed().then(|| {
                AnnotationTiming::for_tool(self.tool, self.video_position, self.video_duration)
            }),
            opacity: 1.0,
            from_template: false,
            pinned: false,
        };

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
            self.annotations.push(mark);
            self.selected_annotation = Some(self.annotations.len() - 1);
        } else if self.tool == Tool::Text {
            let width = 16.0 / (image.size.width / px(1.0));
            let height = (self.text_font_size * 1.35).max(16.0) / (image.size.height / px(1.0));
            mark.end = NormPoint {
                x: (normalized.x + width).min(1.0),
                y: (normalized.y + height).min(1.0),
            };
            self.annotations.push(mark);
            let index = self.annotations.len() - 1;
            self.selected_annotation = Some(index);
            self.editing_text = Some(index);
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
        if !self.scene_is_timed() {
            return image;
        }
        let frame = self.video_viewport_timeline.frame_at(self.video_position);
        let (left, top, visible) = visible_rect(frame);
        let size = size(
            image.size.width * visible as f32,
            image.size.height * visible as f32,
        );
        Bounds {
            origin: point(
                image.origin.x + image.size.width * left as f32,
                image.origin.y + image.size.height * top as f32,
            ),
            size,
        }
    }

    pub(super) fn pointer_move(&mut self, position: Point<Pixels>, image: Bounds<Pixels>) {
        if self.tool == Tool::Select {
            if let (Some(index), Some(last)) = (self.selected_annotation, self.selection_last_point)
            {
                let frame = self.pinned_bounds(image);
                if let Some(mark) = self.annotations.get_mut(index) {
                    let image = if mark.pinned { frame } else { image };
                    let dx = (position.x - last.x) / image.size.width;
                    let dy = (position.y - last.y) / image.size.height;
                    if self.selection_resizing && mark.tool != Tool::Pen {
                        mark.end = screen_to_norm(position, image);
                    } else {
                        mark.start.x = (mark.start.x + dx).clamp(0.0, 1.0);
                        mark.start.y = (mark.start.y + dy).clamp(0.0, 1.0);
                        mark.end.x = (mark.end.x + dx).clamp(0.0, 1.0);
                        mark.end.y = (mark.end.y + dy).clamp(0.0, 1.0);
                        for point in &mut mark.points {
                            point.x = (point.x + dx).clamp(0.0, 1.0);
                            point.y = (point.y + dy).clamp(0.0, 1.0);
                        }
                    }
                }
                self.selection_last_point = Some(position);
            }
        } else if let Some(mark) = &mut self.annotation_draft {
            let normalized = screen_to_norm(position, image);
            mark.end = normalized;
            if mark.tool == Tool::Pen {
                mark.points.push(normalized);
            }
        }
    }

    pub(super) fn pointer_up(&mut self, position: Point<Pixels>, image: Bounds<Pixels>) -> bool {
        self.pointer_is_down = false;
        self.selection_last_point = None;
        self.selection_resizing = false;
        let Some(mut mark) = self.annotation_draft.take() else {
            return self.selected_annotation.is_some_and(|index| {
                self.annotations
                    .get(index)
                    .is_some_and(|mark| mark.tool == Tool::Blur || mark.tool == Tool::Pixelate)
            });
        };
        mark.end = screen_to_norm(position, image);
        let width = (mark.end.x - mark.start.x).abs();
        let height = (mark.end.y - mark.start.y).abs();
        if mark.tool == Tool::Pen {
            if mark.points.len() < 2 {
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
        self.annotations.push(mark);
        self.selected_annotation = Some(self.annotations.len() - 1);
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
        if self.handle_watermark_key(event) {
            return true;
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
            && event.keystroke.key == "z"
        {
            return if event.keystroke.modifiers.shift {
                self.redo_annotations() || self.redo_crop()
            } else {
                self.undo_annotations() || self.undo_crop()
            };
        }
        if let Some(index) = self.editing_text {
            self.caret_visible = true;
            match event.keystroke.key.as_str() {
                "enter" => {
                    self.stop_editing_text();
                }
                "escape" => {
                    self.stop_editing_text();
                }
                "backspace" => {
                    if let Some(mark) = self.annotations.get_mut(index) {
                        mark.text.pop();
                    }
                    self.fit_text_box_to_content(index);
                }
                _ => {
                    if !event.keystroke.modifiers.control
                        && !event.keystroke.modifiers.platform
                        && !event.keystroke.modifiers.alt
                    {
                        if let (Some(text), Some(mark)) = (
                            event.keystroke.key_char.as_ref(),
                            self.annotations.get_mut(index),
                        ) {
                            mark.text.push_str(text);
                        }
                        self.fit_text_box_to_content(index);
                    }
                }
            }
            return true;
        }

        if matches!(event.keystroke.key.as_str(), "delete" | "backspace") {
            if let Some(index) = self.selected_annotation.take() {
                self.record_annotation_undo();
                self.annotations.remove(index);
                return true;
            }
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
        let width = output.width();
        let height = output.height();
        for mark in self
            .annotations
            .iter()
            .filter(|mark| mark.tool == Tool::Blur || mark.tool == Tool::Pixelate)
        {
            let left = mark.start.x.min(mark.end.x).clamp(0.0, 1.0);
            let top = mark.start.y.min(mark.end.y).clamp(0.0, 1.0);
            let right = mark.start.x.max(mark.end.x).clamp(0.0, 1.0);
            let bottom = mark.start.y.max(mark.end.y).clamp(0.0, 1.0);
            let x = (left * width as f32).floor() as u32;
            let y = (top * height as f32).floor() as u32;
            let region_width = ((right - left) * width as f32).ceil() as u32;
            let region_height = ((bottom - top) * height as f32).ceil() as u32;
            if region_width == 0 || region_height == 0 {
                continue;
            }
            let region_width = region_width.min(width.saturating_sub(x));
            let region_height = region_height.min(height.saturating_sub(y));
            let crop =
                image::imageops::crop_imm(&output, x, y, region_width, region_height).to_image();
            let processed = if mark.tool == Tool::Pixelate {
                let block = (4.0 + mark.density.clamp(0.0, 1.0) * 36.0).round() as u32;
                let small = image::imageops::resize(
                    &crop,
                    (region_width / block.max(1)).max(1),
                    (region_height / block.max(1)).max(1),
                    image::imageops::FilterType::Triangle,
                );
                image::imageops::resize(
                    &small,
                    region_width,
                    region_height,
                    image::imageops::FilterType::Nearest,
                )
            } else {
                image::imageops::blur(&crop, 2.0 + mark.density.clamp(0.0, 1.0) * 28.0)
            };
            image::imageops::replace(&mut output, &processed, i64::from(x), i64::from(y));
        }
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

/// SVG fragment with every visible annotation in `marks`, positioned
/// relative to a capture drawn at (`x`, `y`) with the given pixel size. The
/// fragment is self-contained; callers own any surrounding groups.
pub(super) fn annotations_svg(
    marks: &[AnnotationMark],
    x: f32,
    y: f32,
    capture_width: u32,
    capture_height: u32,
    stroke_scale: f32,
) -> String {
    {
        let mut svg = String::new();
        let highlights: Vec<_> = marks
            .iter()
            .filter(|mark| mark.tool == Tool::Highlight)
            .collect();
        if !highlights.is_empty() {
            let _ = write!(svg, "<path fill=\"black\" fill-opacity=\"0.55\" fill-rule=\"evenodd\" d=\"M{x},{y}h{capture_width}v{capture_height}h-{capture_width}z");
            for mark in highlights {
                let hx = x + mark.start.x.min(mark.end.x) * capture_width as f32;
                let hy = y + mark.start.y.min(mark.end.y) * capture_height as f32;
                let hw = (mark.end.x - mark.start.x).abs() * capture_width as f32;
                let hh = (mark.end.y - mark.start.y).abs() * capture_height as f32;
                let _ = write!(svg, " M{hx},{hy}v{hh}h{hw}v-{hh}z");
            }
            svg.push_str("\"/>");
        }

        for mark in marks.iter().filter(|mark| {
            !matches!(
                mark.tool,
                Tool::Select | Tool::Blur | Tool::Pixelate | Tool::Highlight
            )
        }) {
            if mark.opacity < 0.999 {
                let _ = write!(svg, "<g opacity=\"{:.3}\">", mark.opacity.clamp(0.0, 1.0));
            }
            let sx = x + mark.start.x * capture_width as f32;
            let sy = y + mark.start.y * capture_height as f32;
            let ex = x + mark.end.x * capture_width as f32;
            let ey = y + mark.end.y * capture_height as f32;
            let left = sx.min(ex);
            let top = sy.min(ey);
            let width = (ex - sx).abs();
            let height = (ey - sy).abs();
            let color = mark.color;
            let stroke = (mark.stroke_width * stroke_scale).max(1.0);
            match mark.tool {
                Tool::Rectangle => {
                    let _ = write!(svg, "<rect x=\"{left}\" y=\"{top}\" width=\"{width}\" height=\"{height}\" rx=\"2\" fill=\"none\" stroke=\"#{color:06x}\" stroke-width=\"{stroke}\"/>");
                }
                Tool::FilledRectangle => {
                    let _ = write!(svg, "<rect x=\"{left}\" y=\"{top}\" width=\"{width}\" height=\"{height}\" rx=\"2\" fill=\"#{color:06x}\"/>");
                }
                Tool::Ellipse => {
                    let _ = write!(svg, "<ellipse cx=\"{}\" cy=\"{}\" rx=\"{}\" ry=\"{}\" fill=\"none\" stroke=\"#{color:06x}\" stroke-width=\"{stroke}\"/>", left + width/2.0, top + height/2.0, width/2.0, height/2.0);
                }
                Tool::Line | Tool::Arrow => {
                    let _ = write!(svg, "<line x1=\"{sx}\" y1=\"{sy}\" x2=\"{ex}\" y2=\"{ey}\" stroke=\"#{color:06x}\" stroke-width=\"{stroke}\" stroke-linecap=\"round\"/>");
                    if mark.tool == Tool::Arrow {
                        let dx = ex - sx;
                        let dy = ey - sy;
                        let len = (dx * dx + dy * dy).sqrt().max(1.0);
                        let ux = dx / len;
                        let uy = dy / len;
                        let head = stroke * 4.0 + 12.0;
                        let wing = stroke * 2.0 + 6.0;
                        let ax = ex - ux * head - uy * wing;
                        let ay = ey - uy * head + ux * wing;
                        let bx = ex - ux * head + uy * wing;
                        let by = ey - uy * head - ux * wing;
                        let _ = write!(svg, "<path d=\"M{ax},{ay} L{ex},{ey} L{bx},{by}\" fill=\"none\" stroke=\"#{color:06x}\" stroke-width=\"{stroke}\" stroke-linecap=\"round\" stroke-linejoin=\"round\"/>");
                    }
                }
                Tool::Pen => {
                    let mut points = String::new();
                    for point in &mark.points {
                        let _ = write!(
                            points,
                            "{},{} ",
                            x + point.x * capture_width as f32,
                            y + point.y * capture_height as f32
                        );
                    }
                    let _ = write!(svg, "<polyline points=\"{points}\" fill=\"none\" stroke=\"#{color:06x}\" stroke-width=\"{stroke}\" stroke-linecap=\"round\" stroke-linejoin=\"round\"/>");
                }
                Tool::Number => {
                    let cx = left + width / 2.0;
                    let cy = top + height / 2.0;
                    let r = width.min(height) / 2.0;
                    let _ = write!(svg, "<circle cx=\"{cx}\" cy=\"{cy}\" r=\"{r}\" fill=\"#{color:06x}\"/><text x=\"{cx}\" y=\"{}\" text-anchor=\"middle\" font-family=\"sans-serif\" font-weight=\"700\" font-size=\"{}\" fill=\"white\">{}</text>", cy+r*0.36, r, mark.number);
                }
                Tool::Text if !mark.text.is_empty() => {
                    let weight = if mark.bold { "700" } else { "400" };
                    let style = if mark.italic { "italic" } else { "normal" };
                    let decoration = if mark.underline { "underline" } else { "none" };
                    // Fallbacks keep the export sans-serif on machines
                    // without the preferred face installed.
                    let family = match mark.font_family {
                        1 => {
                            "DejaVu Sans Condensed, DejaVu Sans, Liberation Sans Narrow, sans-serif"
                        }
                        2 => "Ubuntu, Cantarell, Noto Sans, DejaVu Sans, sans-serif",
                        _ => {
                            "Noto Sans, Inter, DejaVu Sans, Liberation Sans, Cantarell, sans-serif"
                        }
                    };
                    let (text_x, anchor) = match mark.text_alignment {
                        1 => (left + width / 2.0, "middle"),
                        2 => (left + width, "end"),
                        _ => (left, "start"),
                    };
                    let value = xml_escape(&mark.text);
                    let _ = write!(svg, "<text x=\"{text_x}\" y=\"{}\" text-anchor=\"{anchor}\" font-family=\"{family}\" font-weight=\"{weight}\" font-style=\"{style}\" text-decoration=\"{decoration}\" font-size=\"{}\" fill=\"#{color:06x}\">{value}</text>", top + mark.font_size*stroke_scale, mark.font_size*stroke_scale);
                }
                _ => {}
            }
            if mark.opacity < 0.999 {
                svg.push_str("</g>");
            }
        }
        svg
    }
}

#[cfg(test)]
mod tests {
    use super::{annotations_svg, xml_escape, AnnotationMark, NormPoint, Tool};
    use crate::scene_ui;
    use std::fs;

    #[test]
    fn export_annotations_render_in_a_caller_owned_group() {
        let mark = AnnotationMark {
            tool: Tool::FilledRectangle,
            start: NormPoint { x: 0.0, y: 0.0 },
            end: NormPoint { x: 1.0, y: 1.0 },
            color: 0xff0000,
            ..AnnotationMark::default()
        };
        for marks in [
            vec![],
            vec![mark.clone()],
            vec![AnnotationMark {
                opacity: 0.5,
                ..mark
            }],
        ] {
            let fragment = annotations_svg(&marks, 0.0, 0.0, 20, 20, 1.0);
            let svg = format!(
                r#"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20"><defs><clipPath id="captureClip"><rect width="20" height="20"/></clipPath></defs><g clip-path="url(#captureClip)">{fragment}</g></svg>"#
            );
            let tree = resvg::usvg::Tree::from_str(&svg, &resvg::usvg::Options::default())
                .expect("parse export with caller-owned annotation group");
            let mut output = resvg::tiny_skia::Pixmap::new(20, 20).unwrap();
            resvg::render(
                &tree,
                resvg::tiny_skia::Transform::identity(),
                &mut output.as_mut(),
            );
            let expected_alpha = marks
                .first()
                .map_or(0, |mark| (mark.opacity * 255.0).round() as u8);
            assert_eq!(output.pixel(10, 10).unwrap().alpha(), expected_alpha);
            let layer =
                scene_ui::render_annotation_layer(&marks, 20, 20).expect("render annotation layer");
            assert_eq!(layer.get_pixel(10, 10)[3], expected_alpha);
        }
    }

    #[test]
    fn export_renderer_includes_raster_images() {
        let source = std::env::temp_dir().join(format!(
            "lahza-export-raster-test-{}.png",
            std::process::id()
        ));
        image::RgbaImage::from_pixel(2, 2, image::Rgba([231, 37, 53, 255]))
            .save(&source)
            .expect("write raster fixture");
        let svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="2" height="2"><image href="{}" width="2" height="2"/></svg>"#,
            xml_escape(&source.to_string_lossy())
        );
        let tree = resvg::usvg::Tree::from_str(&svg, &resvg::usvg::Options::default())
            .expect("parse SVG containing a raster image");
        let mut output = resvg::tiny_skia::Pixmap::new(2, 2).expect("allocate output");
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::identity(),
            &mut output.as_mut(),
        );
        let pixel = output.pixel(0, 0).expect("rendered pixel");
        assert_eq!(
            (pixel.red(), pixel.green(), pixel.blue(), pixel.alpha()),
            (231, 37, 53, 255)
        );
        let _ = fs::remove_file(source);
    }
}
