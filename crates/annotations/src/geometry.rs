//! Renderer-independent paths keep editing, hit testing and export in agreement.
use crate::{AnnotationMark, NormPoint, Tool};
use gpui::{point, px, Bounds, PathBuilder, Pixels, Point};
use std::fmt::Write;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct V {
    pub x: f32,
    pub y: f32,
}
impl V {
    pub fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
    pub fn add(self, b: Self) -> Self {
        Self::new(self.x + b.x, self.y + b.y)
    }
    pub fn sub(self, b: Self) -> Self {
        Self::new(self.x - b.x, self.y - b.y)
    }
    pub fn mul(self, n: f32) -> Self {
        Self::new(self.x * n, self.y * n)
    }
    pub fn len(self) -> f32 {
        self.x.hypot(self.y)
    }
    pub fn unit(self) -> Self {
        self.mul(1.0 / self.len().max(0.00001))
    }
    pub fn perp(self) -> Self {
        Self::new(-self.y, self.x)
    }
    pub fn dot(self, b: Self) -> f32 {
        self.x * b.x + self.y * b.y
    }
    pub fn lerp(self, b: Self, t: f32) -> Self {
        self.add(b.sub(self).mul(t))
    }
    pub fn screen(self) -> Point<Pixels> {
        point(px(self.x), px(self.y))
    }
}
impl From<Point<Pixels>> for V {
    fn from(p: Point<Pixels>) -> Self {
        Self::new(p.x / px(1.), p.y / px(1.))
    }
}
#[derive(Clone, Debug)]
pub enum Command {
    Move(V),
    Line(V),
    Quad(V, V),
    Cubic(V, V, V),
    Close,
}
#[derive(Clone, Debug, Default)]
pub struct Path {
    pub commands: Vec<Command>,
    pub filled: bool,
}
impl Path {
    pub fn paint(&self, width: f32, color: gpui::Hsla, window: &mut gpui::Window) {
        let mut b = if self.filled {
            PathBuilder::fill()
        } else {
            PathBuilder::stroke(px(width))
        };
        if self.filled {
            b.style = gpui::PathStyle::Fill(
                gpui::FillOptions::default().with_fill_rule(gpui::FillRule::NonZero),
            );
        } else {
            b.style = gpui::PathStyle::Stroke(
                gpui::StrokeOptions::default()
                    .with_line_width(width)
                    .with_line_cap(lyon::path::LineCap::Round)
                    .with_line_join(lyon::path::LineJoin::Round),
            );
        }
        for c in &self.commands {
            match *c {
                Command::Move(p) => b.move_to(p.screen()),
                Command::Line(p) => b.line_to(p.screen()),
                Command::Quad(c, p) => b.curve_to(p.screen(), c.screen()),
                Command::Cubic(a, bp, p) => b.cubic_bezier_to(p.screen(), a.screen(), bp.screen()),
                Command::Close => b.close(),
            }
        }
        if let Ok(path) = b.build() {
            window.paint_path(path, color);
        }
    }
    pub fn svg(&self) -> String {
        let mut s = String::new();
        for c in &self.commands {
            match *c {
                Command::Move(p) => {
                    let _ = write!(s, "M{:.4},{:.4}", p.x, p.y);
                }
                Command::Line(p) => {
                    let _ = write!(s, "L{:.4},{:.4}", p.x, p.y);
                }
                Command::Quad(c, p) => {
                    let _ = write!(s, "Q{:.4},{:.4} {:.4},{:.4}", c.x, c.y, p.x, p.y);
                }
                Command::Cubic(a, b, p) => {
                    let _ = write!(
                        s,
                        "C{:.4},{:.4} {:.4},{:.4} {:.4},{:.4}",
                        a.x, a.y, b.x, b.y, p.x, p.y
                    );
                }
                Command::Close => s.push('Z'),
            }
        }
        s
    }
}
#[derive(Clone, Debug)]
pub struct Geometry {
    pub paths: Vec<Path>,
    pub centerline: Vec<V>,
    pub handles: Vec<V>,
}
impl Geometry {
    pub fn bounds(&self, margin: f32) -> Bounds<Pixels> {
        let mut lo = V::new(f32::MAX, f32::MAX);
        let mut hi = V::new(f32::MIN, f32::MIN);
        for p in &self.centerline {
            lo.x = lo.x.min(p.x);
            lo.y = lo.y.min(p.y);
            hi.x = hi.x.max(p.x);
            hi.y = hi.y.max(p.y);
        }
        if self.centerline.is_empty() {
            lo = V::default();
            hi = lo;
        }
        Bounds::from_corners(
            lo.sub(V::new(margin, margin)).screen(),
            hi.add(V::new(margin, margin)).screen(),
        )
    }
    pub fn hit(&self, p: V, margin: f32) -> bool {
        if self
            .centerline
            .first()
            .is_some_and(|q| q.sub(p).len() <= margin)
            || self
                .centerline
                .windows(2)
                .any(|s| distance_to_segment(p, s[0], s[1]) <= margin)
        {
            return true;
        }
        for path in self.paths.iter().filter(|path| !path.filled) {
            let mut previous = None;
            for command in &path.commands {
                match *command {
                    Command::Move(q) => previous = Some(q),
                    Command::Line(q) => {
                        if previous.is_some_and(|a| distance_to_segment(p, a, q) <= margin) {
                            return true;
                        }
                        previous = Some(q);
                    }
                    _ => previous = None,
                }
            }
        }
        false
    }
}
pub fn distance_to_segment(p: V, a: V, b: V) -> f32 {
    let d = b.sub(a);
    let t = (p.sub(a).dot(d) / d.dot(d).max(0.000001)).clamp(0., 1.);
    p.sub(a.add(d.mul(t))).len()
}
pub fn arrow(a: V, b: V, bend: f32, width: f32, head: bool, progress: f32) -> Geometry {
    let d = b.sub(a);
    let length = d.len();
    let u = d.unit();
    let n = u.perp();
    let bend = if bend.is_finite() {
        bend.clamp(-4., 4.)
    } else {
        0.
    };
    let h = bend * length;
    let middle = a.lerp(b, 0.5).add(n.mul(h));
    let progress = progress.clamp(0., 1.);
    let mut body = Path::default();
    body.commands.push(Command::Move(a));
    let mut samples = vec![a];
    let mut end = a.lerp(b, progress);
    let mut tangent = u;
    let arc_length;
    if h.abs() < 0.01 || length < 0.01 {
        body.commands.push(Command::Line(end));
        samples.push(end);
        arc_length = length * progress;
    } else {
        // Circle through the endpoints and sagitta, expressed in an orthonormal chord basis.
        let cy = (h * h - length * length * 0.25) / (2. * h);
        let center = a.lerp(b, 0.5).add(n.mul(cy));
        let radius = a.sub(center).len();
        let theta0 = (a.y - center.y).atan2(a.x - center.x);
        let sweep = -4. * (2. * h / length).atan() * progress;
        let segments = (sweep.abs() / std::f32::consts::FRAC_PI_2).ceil().max(1.) as usize;
        let at = |theta: f32| center.add(V::new(theta.cos(), theta.sin()).mul(radius));
        for i in 0..segments {
            let t0 = theta0 + sweep * i as f32 / segments as f32;
            let t1 = theta0 + sweep * (i + 1) as f32 / segments as f32;
            let k = 4. / 3. * ((t1 - t0) / 4.).tan();
            let c1 = at(t0).add(V::new(-t0.sin(), t0.cos()).mul(radius * k));
            let c2 = at(t1).sub(V::new(-t1.sin(), t1.cos()).mul(radius * k));
            body.commands.push(Command::Cubic(c1, c2, at(t1)));
        }
        let count =
            ((sweep.abs() * radius / (width * 0.35).max(0.5)).ceil() as usize).clamp(8, 2048);
        for i in 1..=count {
            samples.push(at(theta0 + sweep * i as f32 / count as f32));
        }
        end = at(theta0 + sweep);
        tangent = V::new(-(theta0 + sweep).sin(), (theta0 + sweep).cos()).mul(sweep.signum());
        arc_length = radius * sweep.abs();
    }
    let mut paths = vec![body];
    if head && arc_length > 0.01 {
        let head_length = (width * 3.5).min(arc_length * 0.35).max(0.01);
        let wing = head_length * 0.5;
        let base = end.sub(tangent.mul(head_length));
        paths.push(Path {
            commands: vec![
                Command::Move(base.add(tangent.perp().mul(wing))),
                Command::Line(end),
                Command::Line(base.sub(tangent.perp().mul(wing))),
            ],
            filled: false,
        });
    }
    Geometry {
        paths,
        centerline: samples,
        handles: vec![a, b, middle],
    }
}
pub fn ink(raw: &[V], width: f32) -> Geometry {
    crate::ink::geometry(raw, width, width, true)
}

pub fn new_draw_seed() -> u32 {
    use std::sync::atomic::{AtomicU32, Ordering};
    static NEXT: AtomicU32 = AtomicU32::new(1);
    let time = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    time ^ NEXT
        .fetch_add(1, Ordering::Relaxed)
        .wrapping_mul(0x9e3779b9)
}

pub fn supports_drawn(tool: Tool) -> bool {
    matches!(
        tool,
        Tool::Rectangle | Tool::FilledRectangle | Tool::Ellipse | Tool::Line | Tool::Arrow
    )
}

// Drawn geometric shapes are constant-width, two-pass strokes. Their control
// points follow the reference's offset and corner-radius rules; pencil ink is
// a separate filled-outline renderer.
struct SketchRandom([u32; 4]);
impl SketchRandom {
    fn new(seed: u32, pass: usize) -> Self {
        let mut rng = Self([0; 4]);
        let key = format!("{seed}{pass}");
        for code in key
            .encode_utf16()
            .map(u32::from)
            .chain(std::iter::repeat_n(0, 64))
        {
            rng.0[0] ^= code;
            rng.next();
        }
        rng
    }
    fn next(&mut self) -> f64 {
        let [x, y, z, w] = self.0;
        let t = x ^ (x << 11);
        let next = w ^ (w >> 19) ^ t ^ (t >> 8);
        self.0 = [y, z, w, next];
        (next as i32 as f64) / 2_147_483_648.
    }
    fn offset(&mut self, limit: f32) -> V {
        let x = self.next();
        let y = self.next();
        let distance = self.next().abs().sqrt() * limit.max(0.) as f64;
        let length = x.hypot(y);
        if length == 0. {
            V::default()
        } else {
            V::new(
                (x / length * distance) as f32,
                (y / length * distance) as f32,
            )
        }
    }
}

fn rectangle_pass(a: V, b: V, width: f32, seed: u32, pass: usize, filled: bool) -> Path {
    let w = (b.x - a.x).max(0.);
    let h = (b.y - a.y).max(0.);
    let rx = (width * 2.).min(w / 4.);
    let ry = (width * 2.).min(h / 4.);
    let limit = if filled { 0. } else { width / 3. };
    let mut random = SketchRandom::new(seed, pass);
    // The initial move has no incoming edge; closing the loop reuses its offset.
    let first = a.add(random.offset(limit.min(w / 4.)));
    let corner_limit = limit.min(((w.min(h) - width * 4.) / 4.).max(0.));
    let tr = V::new(b.x, a.y).add(random.offset(corner_limit));
    let br = b.add(random.offset(corner_limit));
    let bl = V::new(a.x, b.y).add(random.offset(corner_limit));
    let mut commands = vec![Command::Move(first.add(V::new(rx, 0.)))];
    for (corner, before, after) in [
        (tr, V::new(-rx, 0.), V::new(0., ry)),
        (br, V::new(0., -ry), V::new(-rx, 0.)),
        (bl, V::new(rx, 0.), V::new(0., -ry)),
        (first, V::new(0., ry), V::new(rx, 0.)),
    ] {
        if rx > 0. && ry > 0. {
            commands.push(Command::Line(corner.add(before)));
            commands.push(Command::Quad(corner, corner.add(after)));
        } else {
            commands.push(Command::Line(corner.add(after)));
        }
    }
    Path { commands, filled }
}

fn cubic_length(a: V, b: V, c: V, d: V) -> f32 {
    // Twelve-point Gauss-Legendre quadrature (same precision as the reference).
    let nodes = [0.1252, 0.3678, 0.5873, 0.7699, 0.9041, 0.9816];
    let weights = [0.2491, 0.2335, 0.2032, 0.1601, 0.1069, 0.0472];
    let mut length = 0.;
    for (node, weight) in nodes.into_iter().zip(weights) {
        for sign in [-1., 1.] {
            let t = (1. + sign * node) * 0.5;
            let derivative = b
                .sub(a)
                .mul(3. * (1. - t) * (1. - t))
                .add(c.sub(b).mul(6. * (1. - t) * t))
                .add(d.sub(c).mul(3. * t * t));
            length += weight * derivative.len() * 0.5;
        }
    }
    length
}

fn ellipse_pass(a: V, b: V, width: f32, seed: u32, pass: usize) -> Path {
    let center = a.lerp(b, 0.5);
    let rx = (b.x - a.x) * 0.5;
    let ry = (b.y - a.y) * 0.5;
    let k = 0.5522847498307936_f32;
    let anchors = [
        V::new(a.x, center.y),
        V::new(center.x, a.y),
        V::new(b.x, center.y),
        V::new(center.x, b.y),
        V::new(a.x, center.y),
    ];
    let tangents = [
        V::new(0., -ry * k),
        V::new(rx * k, 0.),
        V::new(0., ry * k),
        V::new(-rx * k, 0.),
        V::new(0., -ry * k),
    ];
    let length = cubic_length(
        anchors[0],
        anchors[0].add(tangents[0]),
        anchors[1].sub(tangents[1]),
        anchors[1],
    );
    let limit = (width / 3.).min(length / 4.);
    let mut random = SketchRandom::new(seed, pass);
    let initial_offset = random.offset(limit);
    let mut commands = vec![Command::Move(anchors[0].add(initial_offset))];
    for i in 1..5 {
        let offset = if i == 4 {
            initial_offset
        } else {
            random.offset(limit)
        };
        commands.push(Command::Cubic(
            anchors[i - 1].add(tangents[i - 1]).add(offset),
            anchors[i].sub(tangents[i]).add(offset),
            anchors[i].add(offset),
        ));
    }
    Path {
        commands,
        filled: false,
    }
}

/// Sample the actual painted curves for hit testing, including both draw passes.
fn path_samples(paths: &[Path]) -> Vec<V> {
    let mut points = Vec::new();
    let mut current = V::default();
    for path in paths {
        let mut start = current;
        for command in &path.commands {
            match *command {
                Command::Move(p) => {
                    points.push(p);
                    current = p;
                    start = p;
                }
                Command::Line(p) => {
                    points.push(p);
                    current = p;
                }
                Command::Quad(c, p) => {
                    let a = current;
                    for i in 1..=16 {
                        let t = i as f32 / 16.;
                        points.push(a.lerp(c, t).lerp(c.lerp(p, t), t));
                    }
                    current = p;
                }
                Command::Cubic(b, c, p) => {
                    let a = current;
                    for i in 1..=64 {
                        let t = i as f32 / 64.;
                        points.push(
                            a.lerp(b, t)
                                .lerp(b.lerp(c, t), t)
                                .lerp(b.lerp(c, t).lerp(c.lerp(p, t), t), t),
                        );
                    }
                    current = p;
                }
                Command::Close => {
                    points.push(start);
                    current = start;
                }
            }
        }
    }
    points
}

fn drawn_shape(mark: &AnnotationMark, a: V, b: V) -> Geometry {
    let width = mark.stroke_width.max(0.1);
    let lo = V::new(a.x.min(b.x), a.y.min(b.y));
    let hi = V::new(a.x.max(b.x), a.y.max(b.y));
    let mut paths = Vec::new();
    if mark.tool == Tool::FilledRectangle {
        paths.push(rectangle_pass(lo, hi, width, mark.draw_seed, 0, true));
    }
    // Both passes share a single stroked path, so translucent overlaps do not
    // accumulate opacity and the preview agrees with SVG export.
    let mut outline = Path::default();
    for pass in 0..2 {
        let path = if mark.tool == Tool::Ellipse {
            ellipse_pass(lo, hi, width, mark.draw_seed, pass)
        } else {
            rectangle_pass(lo, hi, width, mark.draw_seed, pass, false)
        };
        outline.commands.extend(path.commands);
    }
    paths.push(outline);
    Geometry {
        centerline: path_samples(&paths),
        paths,
        handles: vec![],
    }
}

fn drawn_arrow(mark: &AnnotationMark, a: V, b: V) -> Geometry {
    let width = mark.stroke_width.max(0.1);
    // Keep offset=0 and roundness=0 for the arrow body,
    // and zero endpoint offsets for two-point lines.
    let mut result = arrow(
        a,
        b,
        mark.bend,
        width,
        false,
        mark.draw_progress.unwrap_or(1.),
    );
    if mark.tool == Tool::Arrow && result.centerline.len() > 1 {
        let end = *result.centerline.last().unwrap();
        let length: f32 = result
            .centerline
            .windows(2)
            .map(|s| s[1].sub(s[0]).len())
            .sum();
        if length > 0.01 {
            let head_length = (length / 5.).clamp(width, width * 3.);
            let tangent = end
                .sub(result.centerline[result.centerline.len() - 2])
                .unit();
            let chord = b.sub(a).len();
            let sagitta = mark.bend * chord;
            let back = if sagitta.abs() > 0.01 && chord > 0.01 {
                let cy = (sagitta * sagitta - chord * chord * 0.25) / (2. * sagitta);
                let center = a.lerp(b, 0.5).add(b.sub(a).unit().perp().mul(cy));
                let radius = a.sub(center).len();
                let theta = 2. * (head_length / (2. * radius)).min(1.).asin();
                // Intersection with the arc a head-length chord behind its tip.
                let angle = (end.y - center.y).atan2(end.x - center.x) + theta * sagitta.signum();
                center
                    .add(V::new(angle.cos(), angle.sin()).mul(radius))
                    .sub(end)
            } else {
                tangent.mul(-head_length)
            };
            let rotated = |angle: f32| {
                V::new(
                    back.x * angle.cos() - back.y * angle.sin(),
                    back.x * angle.sin() + back.y * angle.cos(),
                )
                .add(end)
            };
            result.paths.push(Path {
                commands: vec![
                    Command::Move(rotated(std::f32::consts::PI / 6.)),
                    Command::Line(end),
                    Command::Line(rotated(-std::f32::consts::PI / 6.)),
                ],
                filled: false,
            });
        }
    }
    result
}

pub fn geometry(mark: &AnnotationMark, bounds: Bounds<Pixels>) -> Geometry {
    let p = |p: NormPoint| {
        V::new(
            (bounds.origin.x + bounds.size.width * p.x) / px(1.),
            (bounds.origin.y + bounds.size.height * p.y) / px(1.),
        )
    };
    if mark.hand_drawn && supports_drawn(mark.tool) {
        if matches!(mark.tool, Tool::Arrow | Tool::Line) {
            drawn_arrow(mark, p(mark.start), p(mark.end))
        } else {
            drawn_shape(mark, p(mark.start), p(mark.end))
        }
    } else if mark.tool == Tool::Pen {
        let style_width = mark.ink_style_width.unwrap_or(mark.stroke_width).max(0.1);
        let scale = mark.stroke_width / style_width;
        crate::ink::geometry(
            &mark.points.iter().copied().map(p).collect::<Vec<_>>(),
            (style_width + 1.) * scale,
            style_width + 1.,
            !mark.ink_in_progress,
        )
    } else {
        arrow(
            p(mark.start),
            p(mark.end),
            mark.bend,
            mark.stroke_width,
            mark.tool == Tool::Arrow,
            mark.draw_progress.unwrap_or(1.),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn drawn_shapes_match_reference_paths() {
        let data: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/drawn-paths.json")).unwrap();
        fn tokens(path: &str) -> Vec<String> {
            let mut result = Vec::new();
            let mut number = String::new();
            for c in path.chars() {
                if c.is_ascii_alphabetic() || c.is_whitespace() || c == ',' {
                    if !number.is_empty() {
                        result.push(std::mem::take(&mut number));
                    }
                    if c.is_ascii_alphabetic() {
                        result.push(c.to_string());
                    }
                } else {
                    number.push(c);
                }
            }
            if !number.is_empty() {
                result.push(number);
            }
            result
        }
        for fixture in data["fixtures"].as_array().unwrap() {
            let tool = serde_json::from_value(fixture["tool"].clone()).unwrap();
            let width = fixture["w"].as_f64().unwrap() as f32;
            let height = fixture["h"].as_f64().unwrap() as f32;
            let mark = AnnotationMark {
                tool,
                hand_drawn: true,
                draw_seed: fixture["seed"].as_u64().unwrap() as u32,
                stroke_width: fixture["stroke"].as_f64().unwrap() as f32,
                bend: fixture["bend"].as_f64().unwrap_or(0.) as f32,
                start: NormPoint { x: 0., y: 0. },
                end: NormPoint { x: 1., y: 1. },
                ..Default::default()
            };
            let actual = geometry(
                &mark,
                Bounds::new(point(px(0.), px(0.)), gpui::size(px(width), px(height))),
            );
            let expected = fixture["paths"].as_array().unwrap();
            assert_eq!(actual.paths.len(), expected.len());
            for (path, expected) in actual.paths.iter().zip(expected) {
                assert_eq!(path.filled, expected["filled"].as_bool().unwrap());
                let actual = tokens(&path.svg());
                let expected = tokens(expected["d"].as_str().unwrap());
                assert_eq!(actual.len(), expected.len(), "{fixture}");
                for (actual, expected) in actual.iter().zip(&expected) {
                    if let Ok(expected) = expected.parse::<f32>() {
                        let value = actual.parse::<f32>().unwrap();
                        assert!(
                            (value - expected).abs() <= 0.00021,
                            "{tool:?} seed {}: {value} != {expected}",
                            mark.draw_seed
                        );
                    } else {
                        assert_eq!(actual, expected);
                    }
                }
            }
            // Compare coverage as well as coordinates: both sides go through
            // the real SVG export rasterizer, including caps, joins and fills.
            let render = |paths: Vec<(bool, String)>| {
                let mut svg = format!(
                    r#"<svg xmlns="http://www.w3.org/2000/svg" width="300" height="300"><g transform="translate(50,60)" stroke-width="{}" stroke-linecap="round" stroke-linejoin="round">"#,
                    mark.stroke_width
                );
                for (filled, d) in paths {
                    let (fill, stroke) = if filled {
                        ("black", "none")
                    } else {
                        ("none", "black")
                    };
                    svg.push_str(&format!(
                        r#"<path d="{d}" fill="{fill}" stroke="{stroke}"/>"#
                    ));
                }
                svg.push_str("</g></svg>");
                let tree =
                    resvg::usvg::Tree::from_str(&svg, &resvg::usvg::Options::default()).unwrap();
                let mut pixels = resvg::tiny_skia::Pixmap::new(300, 300).unwrap();
                resvg::render(
                    &tree,
                    resvg::tiny_skia::Transform::identity(),
                    &mut pixels.as_mut(),
                );
                pixels
            };
            let ours = render(actual.paths.iter().map(|p| (p.filled, p.svg())).collect());
            let reference = render(
                expected
                    .iter()
                    .map(|p| {
                        (
                            p["filled"].as_bool().unwrap(),
                            p["d"].as_str().unwrap().to_string(),
                        )
                    })
                    .collect(),
            );
            let coverage: u64 = reference.pixels().iter().map(|p| p.alpha() as u64).sum();
            let error: u64 = ours
                .pixels()
                .iter()
                .zip(reference.pixels())
                .map(|(a, b)| a.alpha().abs_diff(b.alpha()) as u64)
                .sum();
            assert!(coverage > 0);
            assert!(
                error as f64 / (coverage as f64) < 0.001,
                "rendered coverage differs: {fixture}"
            );
        }
    }

    #[test]
    fn slow_mouse_strokes_are_wider_than_fast_sweeps_and_scale_with_export() {
        let slow: Vec<_> = (0..=150).map(|i| V::new(i as f32 * 2., 0.)).collect();
        let fast: Vec<_> = (0..=15).map(|i| V::new(i as f32 * 20., 0.)).collect();
        let radius = |g: &Geometry| {
            g.paths
                .iter()
                .flat_map(|p| &p.commands)
                .filter_map(|c| match c {
                    Command::Move(p) | Command::Line(p) | Command::Quad(_, p)
                        if p.x > 100. && p.x < 200. =>
                    {
                        Some(p.y.abs())
                    }
                    _ => None,
                })
                .fold(0_f32, f32::max)
        };
        let slow_ink = ink(&slow, 4.);
        let fast_ink = ink(&fast, 4.);
        assert!(radius(&slow_ink) > radius(&fast_ink) * 1.4);
        let scaled = ink(&slow.iter().map(|p| p.mul(2.)).collect::<Vec<_>>(), 8.);
        assert_eq!(scaled.centerline.len(), slow_ink.centerline.len());
        for (big, small) in scaled.centerline.iter().zip(&slow_ink.centerline) {
            assert!(big.sub(small.mul(2.)).len() < 0.001);
        }
    }

    #[test]
    fn drawn_shapes_keep_their_character_when_moved_scaled_and_saved() {
        let bounds = Bounds::new(point(px(0.), px(0.)), gpui::size(px(800.), px(600.)));
        for tool in [
            Tool::Rectangle,
            Tool::FilledRectangle,
            Tool::Ellipse,
            Tool::Line,
            Tool::Arrow,
        ] {
            let mark = AnnotationMark {
                tool,
                hand_drawn: true,
                draw_seed: 42,
                start: NormPoint { x: 0.1, y: 0.2 },
                end: NormPoint { x: 0.6, y: 0.6 },
                ..Default::default()
            };
            let original = geometry(&mark, bounds);
            let restored: AnnotationMark =
                serde_json::from_str(&serde_json::to_string(&mark).unwrap()).unwrap();
            assert_eq!(
                original.paths[0].svg(),
                geometry(&restored, bounds).paths[0].svg()
            );
            let moved_bounds = Bounds::new(point(px(37.), px(-21.)), bounds.size);
            let moved = geometry(&mark, moved_bounds);
            let scaled = geometry(
                &AnnotationMark {
                    stroke_width: 8.,
                    ..mark.clone()
                },
                Bounds::new(
                    bounds.origin,
                    gpui::size(bounds.size.width * 2., bounds.size.height * 2.),
                ),
            );
            assert_eq!(original.centerline.len(), scaled.centerline.len());
            for ((a, b), c) in original
                .centerline
                .iter()
                .zip(&moved.centerline)
                .zip(&scaled.centerline)
            {
                assert!(b.sub(a.add(V::new(37., -21.))).len() < 0.002, "{tool:?}");
                assert!(c.sub(a.mul(2.)).len() < 0.002, "{tool:?}");
            }
            let other = geometry(
                &AnnotationMark {
                    draw_seed: 123,
                    ..mark
                },
                bounds,
            );
            if matches!(tool, Tool::Rectangle | Tool::Ellipse) {
                assert_ne!(original.paths[0].svg(), other.paths[0].svg());
            }
            assert!(original.paths.iter().all(|p| !p.svg().contains("NaN")));
        }
        let old: AnnotationMark = serde_json::from_str(r#"{"tool":"rectangle"}"#).unwrap();
        assert!(!old.hand_drawn, "old projects keep their original style");
    }

    #[test]
    fn drawn_outlines_hit_the_stroke_and_leave_the_interior_unselected() {
        let bounds = Bounds::new(point(px(0.), px(0.)), gpui::size(px(800.), px(600.)));
        for tool in [Tool::Rectangle, Tool::Ellipse] {
            let mark = AnnotationMark {
                tool,
                hand_drawn: true,
                draw_seed: 4,
                start: NormPoint { x: 0.1, y: 0.1 },
                end: NormPoint { x: 0.5, y: 0.5 },
                ..Default::default()
            };
            let g = geometry(&mark, bounds);
            assert!(g.hit(g.centerline[g.centerline.len() / 2], 6.));
            assert!(!g.hit(V::new(240., 180.), 6.));
        }
    }

    #[test]
    fn curved_arrow_goes_through_bend_and_has_finite_degenerate_paths() {
        let g = arrow(V::new(0., 0.), V::new(100., 0.), 0.5, 4., true, 1.);
        assert!(g
            .centerline
            .iter()
            .any(|p| p.sub(V::new(50., 50.)).len() < 2.));
        assert!(g.centerline.last().unwrap().sub(V::new(100., 0.)).len() < 0.001);
        assert!(!g.hit(V::new(50., 0.), 5.));
        for bend in [0., 0.00001, -0.8, 4., f32::NAN] {
            let g = arrow(V::default(), V::default(), bend, 4., true, 1.);
            assert!(!g.paths[0].svg().contains("NaN"));
        }
    }
    #[test]
    fn arc_scales_uniformly_and_progress_follows_original_curve() {
        let g = arrow(V::default(), V::new(100., 0.), 0.5, 4., true, 0.5);
        assert!(g.centerline.last().unwrap().sub(V::new(50., 50.)).len() < 0.001);
        let big = arrow(V::default(), V::new(200., 0.), 0.5, 8., true, 0.5);
        assert!(
            big.centerline
                .last()
                .unwrap()
                .sub(g.centerline.last().unwrap().mul(2.))
                .len()
                < 0.001
        );
    }
    #[test]
    fn ink_handles_dots_duplicates_and_reversals() {
        for pts in [
            vec![V::default()],
            vec![V::default(); 4],
            vec![V::default(), V::new(40., 20.), V::new(0., 1.)],
        ] {
            let g = ink(&pts, 4.);
            assert!(!g.paths.is_empty());
            assert!(g.paths.iter().all(|p| p.filled && !p.svg().contains("NaN")));
        }
    }
}
