//! GPUI painting, hit testing, selection handles, and gesture geometry.
use crate::{AnnotationMark, NormPoint, Tool};
use gpui::{
    font, hsla, point, px, quad, rgb, size, App, Bounds, Hsla, Pixels, Point, TextRun, Window,
};

/// Scale from the document's 800-unit canvas to the current preview.
pub fn canvas_scale(bounds: Bounds<Pixels>) -> f32 {
    f32::from(bounds.size.width.min(bounds.size.height)).max(1.0) / 800.0
}

pub fn norm_to_screen(point_: NormPoint, image: Bounds<Pixels>) -> Point<Pixels> {
    point(
        image.origin.x + image.size.width * point_.x,
        image.origin.y + image.size.height * point_.y,
    )
}

pub fn screen_to_norm(point_: Point<Pixels>, image: Bounds<Pixels>) -> NormPoint {
    NormPoint {
        x: ((point_.x - image.origin.x) / image.size.width).clamp(0.0, 1.0),
        y: ((point_.y - image.origin.y) / image.size.height).clamp(0.0, 1.0),
    }
}

pub fn mark_screen_bounds(mark: &AnnotationMark, image: Bounds<Pixels>) -> Bounds<Pixels> {
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

pub fn mark_hit_bounds(mark: &AnnotationMark, image: Bounds<Pixels>) -> Bounds<Pixels> {
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

pub fn annotation_snap_bounds(
    mark: &AnnotationMark,
    space: Bounds<Pixels>,
) -> crate::snapping::Rect {
    let mut bounds = mark_screen_bounds(mark, space);
    if mark.tool == Tool::Text {
        let mut displayed = mark.clone();
        if mark.is_canvas() {
            displayed.font_size *= canvas_scale(space);
        }
        let layout = crate::text::layout(&displayed, f32::from(bounds.size.width));
        bounds.size = size(px(layout.width), px(layout.height));
    } else if matches!(mark.tool, Tool::Arrow | Tool::Line | Tool::Pen) {
        bounds = crate::geometry::geometry(mark, space).bounds(0.);
    }
    bounds.into()
}

pub fn paint_snap_guide(
    guide: &crate::snapping::Guide,
    clip: Bounds<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    let color = rgb(0xd63686);
    for (width, color) in [(3., gpui::rgba(0xffffffb0).into()), (1., Hsla::from(color))] {
        let mut path = gpui::PathBuilder::stroke(px(width));
        path.move_to(guide.start.screen());
        path.line_to(guide.end.screen());
        for p in &guide.points {
            path.move_to(point(px(p.x - 2.5), px(p.y - 2.5)));
            path.line_to(point(px(p.x + 2.5), px(p.y + 2.5)));
            path.move_to(point(px(p.x - 2.5), px(p.y + 2.5)));
            path.line_to(point(px(p.x + 2.5), px(p.y - 2.5)));
        }
        if guide.gap {
            let normal = guide.end.sub(guide.start).unit().perp().mul(4.);
            for p in [guide.start, guide.end] {
                path.move_to(p.sub(normal).screen());
                path.line_to(p.add(normal).screen());
            }
        }
        if let Ok(path) = path.build() {
            window.paint_path(path, color);
        }
    }
    if let Some(label) = &guide.label {
        let run = TextRun {
            len: label.len(),
            font: font("Inter"),
            color: rgb(0xffffff).into(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let line = window
            .text_system()
            .shape_line(label.clone().into(), px(11.), &[run], None);
        let width = line.width + px(10.);
        let x = (px(guide.start.x) + px(6.)).clamp(
            clip.left() + px(4.),
            (clip.right() - width - px(4.)).max(clip.left() + px(4.)),
        );
        let y = (px(guide.start.y) + px(6.)).clamp(
            clip.top() + px(4.),
            (clip.bottom() - px(22.)).max(clip.top() + px(4.)),
        );
        let bounds = Bounds::new(point(x, y), size(width, px(18.)));
        window.paint_quad(quad(
            bounds,
            px(3.),
            color,
            px(0.),
            color,
            Default::default(),
        ));
        let _ = line.paint(point(x + px(5.), y + px(2.)), px(14.), window, cx);
    }
}

pub fn paint_annotation(
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
    if mark.hand_drawn && crate::geometry::supports_drawn(mark.tool) {
        let geometry = crate::geometry::geometry(mark, image);
        for path in &geometry.paths {
            path.paint(mark.stroke_width, color, window);
        }
        return if matches!(mark.tool, Tool::Arrow | Tool::Line) {
            geometry.bounds(mark.stroke_width * 0.7)
        } else {
            bounds
        };
    }
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
        Tool::Line | Tool::Arrow | Tool::Pen => {
            let geometry = crate::geometry::geometry(mark, image);
            rendered_bounds = geometry.bounds(mark.stroke_width * 0.75);
            for path in &geometry.paths {
                path.paint(mark.stroke_width, color, window);
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
            let layout = crate::text::layout(mark, f32::from(bounds.size.width));
            rendered_bounds = Bounds::new(bounds.origin, size(px(layout.width), px(layout.height)));
            for (row, line) in layout.lines.iter().enumerate() {
                let text = &mark.text[line.range.clone()];
                let run = TextRun {
                    len: text.len(),
                    font: crate::text::font(mark),
                    color,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                let shaped = window.text_system().shape_line(
                    text.to_owned().into(),
                    px(mark.font_size),
                    &[run],
                    None,
                );
                let origin = point(
                    bounds.left() + px(layout.x(line, mark.text_alignment)),
                    bounds.top() + px(row as f32 * layout.line_height),
                );
                let _ = shaped.paint(origin, px(layout.line_height), window, cx);
                if mark.underline {
                    window.paint_quad(gpui::fill(
                        Bounds::new(
                            point(
                                origin.x,
                                origin.y + px(layout.baseline + mark.font_size * 0.1),
                            ),
                            size(px(line.width), px((mark.font_size / 18.).max(1.))),
                        ),
                        color,
                    ));
                }
            }
            let _ = show_text_caret; // The canvas input paints its actual insertion caret.
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

pub fn paint_highlights(marks: &[AnnotationMark], image: Bounds<Pixels>, window: &mut Window) {
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

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Handle {
    Start,
    End,
    Bend,
    Corner(u8),
    Left,
    Right,
}
pub fn hit_mark(
    mark: &AnnotationMark,
    position: Point<Pixels>,
    image: Bounds<Pixels>,
    rendered: Option<Bounds<Pixels>>,
) -> bool {
    if rendered.is_some_and(|bounds| bounds.size.width == px(0.) && bounds.size.height == px(0.)) {
        return false;
    }
    let b = rendered.unwrap_or_else(|| mark_screen_bounds(mark, image));
    // Closed shapes own their interior even when their appearance has no fill.
    // Keep the outline hit test below for forgiving clicks around their edges.
    if mark.tool == Tool::Rectangle && b.contains(&position) {
        return true;
    }
    if mark.tool == Tool::Ellipse && b.size.width > px(0.) && b.size.height > px(0.) {
        let dx = f32::from(position.x - b.center().x) / (f32::from(b.size.width) * 0.5);
        let dy = f32::from(position.y - b.center().y) / (f32::from(b.size.height) * 0.5);
        if dx * dx + dy * dy <= 1. {
            return true;
        }
    }
    if matches!(mark.tool, Tool::Arrow | Tool::Line | Tool::Pen)
        || (mark.hand_drawn && matches!(mark.tool, Tool::Rectangle | Tool::Ellipse))
    {
        return crate::geometry::geometry(mark, image)
            .hit(position.into(), mark.stroke_width * 0.75 + 5.);
    }
    if mark.tool == Tool::Ellipse {
        let rx = f32::from(b.size.width) * 0.5;
        let ry = f32::from(b.size.height) * 0.5;
        if rx < 1. || ry < 1. {
            return mark_hit_bounds(mark, image).contains(&position);
        }
        let dx = f32::from(position.x - b.center().x) / rx;
        let dy = f32::from(position.y - b.center().y) / ry;
        return ((dx * dx + dy * dy).sqrt() - 1.).abs() * rx.min(ry)
            <= mark.stroke_width * 0.5 + 5.;
    }
    if mark.tool == Tool::Rectangle {
        let p = crate::geometry::V::from(position);
        let corners = [
            b.origin,
            point(b.right(), b.top()),
            point(b.right(), b.bottom()),
            point(b.left(), b.bottom()),
        ];
        return (0..4).any(|i| {
            crate::geometry::distance_to_segment(p, corners[i].into(), corners[(i + 1) % 4].into())
                <= mark.stroke_width * 0.5 + 5.
        });
    }
    b.contains(&position)
}

/// A visible selection box is a drag surface, including the gaps around ink.
/// Unselected marks continue to use their actual shape for hit testing.
pub fn hit_selection_box(
    mark: &AnnotationMark,
    position: Point<Pixels>,
    image: Bounds<Pixels>,
    rendered: Option<Bounds<Pixels>>,
) -> bool {
    if matches!(mark.tool, Tool::Arrow | Tool::Line)
        || (mark.tool == Tool::Text && mark.text_auto_width && mark.text.trim().is_empty())
    {
        return false;
    }
    let bounds = rendered.unwrap_or_else(|| {
        if mark.tool == Tool::Pen {
            crate::geometry::geometry(mark, image).bounds(mark.stroke_width * 0.75)
        } else {
            let bounds = annotation_snap_bounds(mark, image);
            Bounds::from_corners(bounds.min.screen(), bounds.max.screen())
        }
    });
    bounds.size.width > px(0.) && bounds.size.height > px(0.) && bounds.contains(&position)
}

pub fn handles(
    mark: &AnnotationMark,
    image: Bounds<Pixels>,
    bounds: Bounds<Pixels>,
) -> Vec<(Handle, Point<Pixels>)> {
    if matches!(mark.tool, Tool::Arrow | Tool::Line) {
        let g = crate::geometry::geometry(mark, image);
        let mut h = vec![
            (Handle::Start, g.handles[0].screen()),
            (Handle::End, g.handles[1].screen()),
        ];
        if mark.tool == Tool::Arrow {
            h.push((Handle::Bend, g.handles[2].screen()));
        }
        return h;
    }
    let mut h = vec![
        (Handle::Corner(0), bounds.origin),
        (Handle::Corner(1), point(bounds.right(), bounds.top())),
        (Handle::Corner(2), point(bounds.right(), bounds.bottom())),
        (Handle::Corner(3), point(bounds.left(), bounds.bottom())),
    ];
    if mark.tool == Tool::Text {
        h.push((Handle::Left, point(bounds.left(), bounds.center().y)));
        h.push((Handle::Right, point(bounds.right(), bounds.center().y)));
    }
    h
}
const SELECTION_STROKE: f32 = 1.5;

fn selection_point(p: Point<Pixels>, scale: f32) -> Point<Pixels> {
    point(px((f32::from(p.x) * scale).round() / scale), px((f32::from(p.y) * scale).round() / scale))
}

fn selection_color() -> Hsla {
    hsla(214.0 / 360.0, 0.84, 0.56, 1.0)
}

/// A centered, screen-sized outline shared by single and group selections.
/// Align centerlines in physical pixels so opposite edges have equal coverage.
pub fn paint_selection_box(bounds: Bounds<Pixels>, window: &mut Window) {
    if bounds.size.width <= px(0.) || bounds.size.height <= px(0.) { return; }
    let scale = window.scale_factor();
    let min = selection_point(bounds.origin, scale);
    let max = selection_point(point(bounds.right(), bounds.bottom()), scale);
    // A stroked path avoids the dark inner fringe of a transparent quad border.
    let mut outline = gpui::PathBuilder::stroke(px(SELECTION_STROKE));
    outline.move_to(min);
    outline.line_to(point(max.x, min.y));
    outline.line_to(max);
    outline.line_to(point(min.x, max.y));
    outline.close();
    if let Ok(path) = outline.build() {
        window.paint_path(path, selection_color());
    }
}

pub fn paint_selection(
    mark: &AnnotationMark,
    image: Bounds<Pixels>,
    bounds: Bounds<Pixels>,
    window: &mut Window,
) {
    // Click-created text waits for pointer-up to focus its editor. Do not
    // flash resize handles around that empty, auto-width placeholder.
    // Fixed-width text still shows its bounds during drag-to-create.
    if mark.tool == Tool::Text && mark.text_auto_width && mark.text.trim().is_empty() {
        return;
    }
    if !matches!(mark.tool, Tool::Arrow | Tool::Line) {
        paint_selection_box(bounds, window);
    }
    if mark.tool == Tool::Pen
        && bounds.size.width < px(mark.stroke_width * 2.)
        && bounds.size.height < px(mark.stroke_width * 2.)
    {
        return;
    }
    for (handle, p) in handles(mark, image, bounds) {
        let p = selection_point(p, window.scale_factor());
        // Eight-pixel square corner handles; path controls remain round.
        let half = 4.0 + SELECTION_STROKE / 2.0;
        window.paint_quad(quad(
            Bounds::new(point(p.x - px(half), p.y - px(half)), size(px(half * 2.), px(half * 2.))),
            if matches!(handle, Handle::Corner(_)) { px(0.) } else { px(half) },
            rgb(0xffffff),
            px(SELECTION_STROKE),
            selection_color(),
            Default::default(),
        ));
    }
}
pub fn constrained_delta(tool: Tool, delta: crate::geometry::V, shift: bool) -> crate::geometry::V {
    use crate::geometry::V;
    if !shift {
        return delta;
    }
    match tool {
        Tool::Rectangle
        | Tool::FilledRectangle
        | Tool::Ellipse
        | Tool::Highlight
        | Tool::Blur
        | Tool::Pixelate => {
            let side = delta.x.abs().max(delta.y.abs());
            V::new(
                side * if delta.x < 0. { -1. } else { 1. },
                side * if delta.y < 0. { -1. } else { 1. },
            )
        }
        Tool::Arrow | Tool::Line | Tool::Pen => {
            let step = std::f32::consts::FRAC_PI_4;
            let angle = (delta.y.atan2(delta.x) / step).round() * step;
            V::new(angle.cos(), angle.sin()).mul(delta.len())
        }
        Tool::Select => {
            if delta.x.abs() > delta.y.abs() {
                V::new(delta.x, 0.)
            } else {
                V::new(0., delta.y)
            }
        }
        _ => delta,
    }
}

pub fn translate_mark(mark: &mut AnnotationMark, dx: f32, dy: f32) {
    mark.start.x += dx;
    mark.start.y += dy;
    mark.end.x += dx;
    mark.end.y += dy;
    for p in &mut mark.points {
        p.x += dx;
        p.y += dy;
    }
}
pub fn translation_limits(marks: &[AnnotationMark], selected: &[usize]) -> (f32, f32, f32, f32) {
    let mut min_x = f32::MAX;
    let mut min_y = f32::MAX;
    let mut max_x = f32::MIN;
    let mut max_y = f32::MIN;
    for i in selected {
        if let Some(m) = marks.get(*i) {
            for p in std::iter::once(&m.start)
                .chain(std::iter::once(&m.end))
                .chain(m.points.iter().filter(|_| m.tool == Tool::Pen))
            {
                min_x = min_x.min(p.x);
                min_y = min_y.min(p.y);
                max_x = max_x.max(p.x);
                max_y = max_y.max(p.y);
            }
        }
    }
    (
        -min_x,
        (1. - max_x).max(-min_x),
        -min_y,
        (1. - max_y).max(-min_y),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::svg::{annotations_svg, xml_escape};
    use std::fs;
    #[test]
    fn drawn_style_preview_and_export_use_the_same_contours() {
        let mut marks = Vec::new();
        for (row, hand_drawn) in [false, true].into_iter().enumerate() {
            for (column, tool) in [
                Tool::Rectangle,
                Tool::Ellipse,
                Tool::Arrow,
                Tool::Line,
                Tool::FilledRectangle,
            ]
            .into_iter()
            .enumerate()
            {
                marks.push(AnnotationMark {
                    tool,
                    hand_drawn,
                    draw_seed: 42 + column as u32,
                    start: NormPoint {
                        x: (40. + column as f32 * 230.) / 1200.,
                        y: (90. + row as f32 * 240.) / 800.,
                    },
                    end: NormPoint {
                        x: (210. + column as f32 * 230.) / 1200.,
                        y: (210. + row as f32 * 240.) / 800.,
                    },
                    stroke_width: 5.,
                    color: 0x24252b,
                    bend: if tool == Tool::Arrow { -0.3 } else { 0. },
                    ..Default::default()
                });
            }
        }
        let mut points = Vec::new();
        let mut x = 40.;
        while x < 1140. {
            points.push(NormPoint {
                x: x / 1200.,
                y: (645. + (x / 55_f32).sin() * 45.) / 800.,
            });
            // Slow at the left, fast in the middle, slow again at the right.
            x += if x > 400. && x < 800. { 22. } else { 2. };
        }
        marks.push(AnnotationMark {
            tool: Tool::Pen,
            points,
            stroke_width: 8.,
            color: 0x346cf0,
            ..Default::default()
        });
        let fragment = annotations_svg(&marks, 0., 0., 1200, 800, 1.);
        let bounds = Bounds::new(point(px(0.), px(0.)), size(px(1200.), px(800.)));
        for mark in marks.iter().filter(|m| m.hand_drawn || m.tool == Tool::Pen) {
            for path in crate::geometry::geometry(mark, bounds).paths {
                assert!(fragment.contains(&path.svg()));
            }
        }
        let svg = format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="1200" height="800">
            <rect width="1200" height="800" fill="white"/>
            <g font-family="sans-serif" font-size="22" fill="#555555">
                <text x="40" y="50">Clean</text><text x="40" y="290">Drawn</text>
                <text x="40" y="560">Pencil: slow → fast → slow</text>
            </g>{fragment}</svg>"##
        );
        let options = resvg::usvg::Options {
            fontdb: crate::fonts::shared_fontdb(),
            ..Default::default()
        };
        let tree = resvg::usvg::Tree::from_str(&svg, &options).unwrap();
        let mut pixmap = resvg::tiny_skia::Pixmap::new(1200, 800).unwrap();
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::identity(),
            &mut pixmap.as_mut(),
        );
        assert!(pixmap.pixels().iter().filter(|p| p.red() < 100).count() > 15000);
        if let Some(path) = std::env::var_os("LAHZA_ANNOTATION_PREVIEW") {
            pixmap.save_png(path).unwrap();
        }
    }

    #[test]
    fn shift_constrains_boxes_to_squares_and_arrows_to_angles() {
        use crate::geometry::V;
        assert_eq!(
            constrained_delta(Tool::Rectangle, V::new(80., -30.), true),
            V::new(80., -80.)
        );
        let line = constrained_delta(Tool::Arrow, V::new(80., 3.), true);
        assert!(line.y.abs() < 0.001);
        assert_eq!(
            constrained_delta(Tool::Select, V::new(80., 30.), true),
            V::new(80., 0.)
        );
    }
    #[test]
    fn selection_hits_curves_without_selecting_their_empty_bounds() {
        let image = Bounds::new(point(px(0.), px(0.)), size(px(1000.), px(600.)));
        let mark = AnnotationMark {
            tool: Tool::Arrow,
            start: NormPoint { x: 0.1, y: 0.2 },
            end: NormPoint { x: 0.5, y: 0.2 },
            bend: 0.5,
            ..Default::default()
        };
        assert!(hit_mark(&mark, point(px(300.), px(320.)), image, None));
        assert!(!hit_mark(&mark, point(px(300.), px(120.)), image, None));
    }

    #[test]
    fn outlined_shapes_select_from_their_interior_in_both_styles() {
        let image = Bounds::new(point(px(50.), px(30.)), size(px(1000.), px(600.)));
        for tool in [Tool::Rectangle, Tool::Ellipse] {
            for hand_drawn in [false, true] {
                for reversed in [false, true] {
                    let mut mark = AnnotationMark {
                        tool,
                        hand_drawn,
                        start: NormPoint { x: 0.1, y: 0.2 },
                        end: NormPoint { x: 0.5, y: 0.7 },
                        ..Default::default()
                    };
                    if reversed {
                        std::mem::swap(&mut mark.start, &mut mark.end);
                    }
                    assert!(hit_mark(&mark, point(px(350.), px(300.)), image, None));
                    assert!(hit_mark(&mark, point(px(220.), px(280.)), image, None));
                    assert!(hit_mark(&mark, point(px(150.), px(300.)), image, None));
                    assert!(!hit_mark(&mark, point(px(80.), px(300.)), image, None));
                    // An ellipse's bounding-box corners are still outside the shape.
                    assert_eq!(
                        hit_mark(&mark, point(px(160.), px(160.)), image, None),
                        tool == Tool::Rectangle,
                    );
                    let displayed = Bounds::new(point(px(600.), px(500.)), size(px(200.), px(100.)));
                    assert!(hit_mark(&mark, displayed.center(), image, Some(displayed)));
                    // Timed annotations hidden at the playhead cannot be selected.
                    let hidden = Bounds::new(point(px(0.), px(0.)), size(px(0.), px(0.)));
                    assert!(!hit_mark(&mark, point(px(350.), px(300.)), image, Some(hidden)));
                }
            }
        }
    }
    #[test]
    fn dragging_to_an_edge_preserves_all_point_distances() {
        let mut mark = AnnotationMark {
            tool: Tool::Pen,
            start: NormPoint { x: 0.2, y: 0.2 },
            end: NormPoint { x: 0.8, y: 0.8 },
            points: vec![
                NormPoint { x: 0.2, y: 0.2 },
                NormPoint { x: 0.5, y: 0.6 },
                NormPoint { x: 0.8, y: 0.8 },
            ],
            ..Default::default()
        };
        let original = mark.clone();
        let (lx, hx, ly, hy) = translation_limits(&[mark.clone()], &[0]);
        translate_mark(&mut mark, 0.8_f32.clamp(lx, hx), 0.8_f32.clamp(ly, hy));
        assert!((mark.end.x - 1.).abs() < 0.0001);
        for (a, b) in mark.points.iter().zip(original.points.iter()) {
            assert!(((a.x - mark.start.x) - (b.x - original.start.x)).abs() < 0.0001);
            assert!(((a.y - mark.start.y) - (b.y - original.start.y)).abs() < 0.0001);
        }
    }
    #[test]
    fn old_documents_default_to_straight_arrows_and_auto_width_text() {
        let mark: AnnotationMark =
            serde_json::from_str(r#"{"tool":"arrow","text":"legacy"}"#).unwrap();
        assert_eq!(mark.bend, 0.);
        assert!(mark.text_auto_width);
        assert_eq!(mark.draw_progress, None);
    }
    #[test]
    fn shared_paths_export_curved_arrows_ink_and_wrapped_text() {
        let marks = [
            AnnotationMark {
                tool: Tool::Arrow,
                start: NormPoint { x: 0.1, y: 0.1 },
                end: NormPoint { x: 0.8, y: 0.1 },
                bend: 0.3,
                ..Default::default()
            },
            AnnotationMark {
                tool: Tool::Pen,
                start: NormPoint { x: 0.1, y: 0.5 },
                end: NormPoint { x: 0.6, y: 0.6 },
                points: vec![
                    NormPoint { x: 0.1, y: 0.5 },
                    NormPoint { x: 0.3, y: 0.4 },
                    NormPoint { x: 0.6, y: 0.6 },
                ],
                ..Default::default()
            },
            AnnotationMark {
                tool: Tool::Text,
                text: "First line\nSecond line wraps".into(),
                text_auto_width: false,
                start: NormPoint { x: 0.1, y: 0.7 },
                end: NormPoint { x: 0.3, y: 0.9 },
                ..Default::default()
            },
        ];
        let fragment = annotations_svg(&marks, 0., 0., 800, 600, 1.);
        assert!(fragment.contains('C'));
        assert!(fragment.contains('Q'));
        assert!(!fragment.contains("polyline"));
        assert!(fragment.matches("<text ").count() >= 3);
        let svg = format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="800" height="600">{fragment}</svg>"#
        );
        let options = resvg::usvg::Options {
            fontdb: crate::fonts::shared_fontdb(),
            ..Default::default()
        };
        let tree = resvg::usvg::Tree::from_str(&svg, &options).unwrap();
        let mut pixmap = resvg::tiny_skia::Pixmap::new(800, 600).unwrap();
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::identity(),
            &mut pixmap.as_mut(),
        );
        assert!(pixmap.pixels().iter().filter(|p| p.alpha() > 0).count() > 1000);
    }

    #[test]
    fn handwritten_styles_render_distinctly_without_system_fonts() {
        use resvg::usvg::fontdb::{Database, Family, Query, Style, Weight};

        let mut fonts = Database::new();
        for bytes in crate::fonts::HANDWRITTEN_FONTS {
            fonts.load_font_data(bytes.to_vec());
        }
        let shared = crate::fonts::shared_fontdb();
        let mut options = resvg::usvg::Options::default();
        options.fontdb = std::sync::Arc::new(fonts);
        let mut rendered = Vec::new();
        for (bold, italic, face_name) in [
            (false, false, "ShantellSansInformal-Regular"),
            (true, false, "ShantellSansInformal-Bold"),
            (false, true, "ShantellSansInformal-Italic"),
            (true, true, "ShantellSansInformal-BoldItalic"),
        ] {
            let query = Query {
                families: &[Family::Name(crate::fonts::HANDWRITTEN_FAMILY)],
                weight: if bold { Weight::BOLD } else { Weight::NORMAL },
                style: if italic { Style::Italic } else { Style::Normal },
                ..Query::default()
            };
            for database in [shared.as_ref(), options.fontdb.as_ref()] {
                let face = database
                    .query(&query)
                    .expect("bundled annotation font style");
                assert_eq!(database.face(face).unwrap().post_script_name, face_name);
            }
            for underline in [false, true] {
                let mark = AnnotationMark {
                    tool: Tool::Text,
                    text: "Handwritten notes".into(),
                    font_family: 3,
                    font_size: 32.0,
                    bold,
                    italic,
                    underline,
                    start: NormPoint { x: 0.05, y: 0.1 },
                    end: NormPoint { x: 0.95, y: 0.9 },
                    ..AnnotationMark::default()
                };
                let fragment = annotations_svg(&[mark], 0.0, 0.0, 400, 80, 1.0);
                assert!(fragment.contains("font-family=\"Shantell Sans Informal\""));
                let svg = format!(
                    r#"<svg xmlns="http://www.w3.org/2000/svg" width="400" height="80">{fragment}</svg>"#
                );
                let tree = resvg::usvg::Tree::from_str(&svg, &options).unwrap();
                let mut output = resvg::tiny_skia::Pixmap::new(400, 80).unwrap();
                resvg::render(
                    &tree,
                    resvg::tiny_skia::Transform::identity(),
                    &mut output.as_mut(),
                );
                assert!(output.pixels().iter().any(|pixel| pixel.alpha() > 0));
                for previous in &rendered {
                    assert_ne!(
                        output.data(),
                        previous,
                        "styles must change rendered pixels"
                    );
                }
                rendered.push(output.data().to_vec());
            }
        }
    }

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
                crate::svg::render_annotations(&marks, 20, 20).expect("render annotation layer");
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
