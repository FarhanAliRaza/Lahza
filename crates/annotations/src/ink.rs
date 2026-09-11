//! Mouse ink: sample-distance pressure, streamlined points and a filled outline.
//! Keep original sample spacing: resampling before pressure loses fast/slow motion.
use crate::geometry::{Command, Geometry, Path, V};
use std::ops::{Add, Mul, Sub};

#[derive(Clone, Copy, Debug, Default)]
struct P {
    x: f64,
    y: f64,
}
impl Add for P {
    type Output = Self;
    fn add(self, b: Self) -> Self {
        Self {
            x: self.x + b.x,
            y: self.y + b.y,
        }
    }
}
impl Sub for P {
    type Output = Self;
    fn sub(self, b: Self) -> Self {
        Self {
            x: self.x - b.x,
            y: self.y - b.y,
        }
    }
}
impl Mul<f64> for P {
    type Output = Self;
    fn mul(self, n: f64) -> Self {
        Self {
            x: self.x * n,
            y: self.y * n,
        }
    }
}
impl P {
    fn dot(self, b: Self) -> f64 {
        self.x * b.x + self.y * b.y
    }
    fn len(self) -> f64 {
        self.dot(self).sqrt()
    }
    fn unit(self) -> Self {
        let n = self.len();
        if n == 0. {
            self
        } else {
            self * (1. / n)
        }
    }
    fn perp(self) -> Self {
        Self {
            x: -self.y,
            y: self.x,
        }
    }
    fn lerp(self, b: Self, t: f64) -> Self {
        self + (b - self) * t
    }
    fn rounded(self) -> Self {
        Self {
            x: (self.x * 100. + 0.5).floor() / 100.,
            y: (self.y * 100. + 0.5).floor() / 100.,
        }
    }
    fn v(self) -> V {
        V::new(self.x as f32, self.y as f32)
    }
    fn rotate(self, a: f64) -> Self {
        Self {
            x: self.x * a.cos() - self.y * a.sin(),
            y: self.x * a.sin() + self.y * a.cos(),
        }
    }
}
#[derive(Clone, Copy, Debug)]
struct Sample {
    p: P,
    input: P,
    distance: f64,
    running: f64,
    radius: f64,
    cap: bool,
}

fn streamline(size: f64) -> f64 {
    0.64 + ((size - 9.) / 7.).clamp(0., 1.) * 0.10
}

fn samples(raw: &[V], size: f64, smoothing: f64, last: bool) -> Vec<Sample> {
    let raw: Vec<P> = raw
        .iter()
        .filter(|p| p.x.is_finite() && p.y.is_finite())
        .map(|p| P {
            x: p.x as f64,
            y: p.y as f64,
        })
        .collect();
    let Some(&first) = raw.first() else {
        return vec![];
    };
    let min_dist2 = (size / 3.).powi(2);
    let mut staged = vec![first];
    staged.extend(
        raw.iter()
            .skip(1)
            .skip_while(|p| (**p - first).dot(**p - first) <= min_dist2)
            .copied(),
    );
    let mut removed = 0;
    if staged.len() > 1 {
        let end = *staged.last().unwrap();
        let mut remaining = staged.len() - 1;
        while remaining > 0
            && (staged[remaining - 1] - end).dot(staged[remaining - 1] - end) <= min_dist2
        {
            remaining -= 1;
            removed += 1;
        }
        staged.truncate(remaining);
        staged.push(end);
    }
    let complete = last
        || removed > 0
        || (staged.len() > 1 && (staged[staged.len() - 1] - staged[staged.len() - 2]).len() < size);
    if staged.len() == 2 {
        let (a, b) = (staged[0], staged[1]);
        staged = (0..=4).map(|i| a.lerp(b, i as f64 / 4.)).collect();
    }
    if complete && smoothing > 0. {
        staged.push(*staged.last().unwrap());
    }
    let t = 0.15 + (1. - smoothing) * 0.85;
    let mut prev = staged[0];
    let mut result = vec![Sample {
        p: prev,
        input: prev,
        distance: 0.,
        running: 0.,
        radius: 1.,
        cap: false,
    }];
    let mut total = 0.;
    for (i, &input) in staged.iter().enumerate().skip(1) {
        let p = if last && i == staged.len() - 1 {
            input
        } else {
            input + (prev - input) * (1. - t)
        };
        if (prev.x - p.x).abs() < 0.0001 && (prev.y - p.y).abs() < 0.0001 {
            continue;
        }
        let distance = (p - prev).len();
        total += distance;
        if i < 4 && total < size {
            continue;
        }
        result.push(Sample {
            p,
            input,
            distance,
            running: total,
            radius: 1.,
            cap: false,
        });
        prev = p;
    }
    // Warm up pressure over the first five stroke widths, avoiding a start blob.
    let advance = |pressure: f64, distance: f64| {
        let speed = (distance / size).min(1.);
        (pressure + ((1. - speed) - pressure) * (speed * 0.275)).min(1.)
    };
    let mut pressure = 0.5;
    for s in &result {
        if s.running > size * 5. {
            break;
        }
        pressure += (advance(pressure, s.distance) - pressure) * 0.5;
    }
    for s in &mut result {
        pressure = advance(pressure, s.distance);
        s.radius = size * ((0.5 - 0.5 * (0.5 - pressure)) * std::f64::consts::FRAC_PI_2).sin();
    }
    result
}

fn simplify(track: &[P], tolerance: f64) -> Vec<P> {
    if track.len() < 3 {
        return track.to_vec();
    }
    let mut result = vec![track[0]];
    let mut anchor = 0;
    while anchor < track.len() - 1 {
        let mut best = anchor + 1;
        'candidate: for j in anchor + 2..=(anchor + 8).min(track.len() - 1) {
            let delta = track[j] - track[anchor];
            let length2 = delta.dot(delta);
            for k in anchor + 1..j {
                let t = if length2 == 0. {
                    0.
                } else {
                    ((track[k] - track[anchor]).dot(delta) / length2).clamp(0., 1.)
                };
                let error = track[k] - (track[anchor] + delta * t);
                if error.dot(error) > tolerance * tolerance {
                    break 'candidate;
                }
            }
            best = j;
        }
        result.push(track[best]);
        anchor = best;
    }
    result
}

fn tracks(src: &[Sample], size: f64, anchor: Option<P>) -> (Vec<P>, Vec<P>) {
    let n = src.len();
    let mut left = vec![];
    let mut right = vec![];
    let mut current = (src[0].p - src[1].p).unit();
    let mut previous = current;
    let (mut pl, mut pr) = (src[0].p, src[0].p);
    let mut previous_sharp = false;
    for (i, s) in src.iter().enumerate() {
        let vector = current;
        let next = if i < n - 1 {
            let from = if i == 0 && n > 2 {
                anchor.unwrap_or(s.p)
            } else {
                s.p
            };
            (from - src[i + 1].p).unit()
        } else {
            vector
        };
        current = next;
        let prev_dot = vector.dot(previous);
        let next_dot = if i < n - 1 { next.dot(vector) } else { 1. };
        let sharp = prev_dot < 0. && !previous_sharp;
        let next_sharp = next_dot < 0.2;
        if sharp || next_sharp {
            if next_dot > -0.62 && src[n - 1].running - s.running > s.radius {
                let offset = previous * s.radius;
                if previous.x * next.y - previous.y * next.x < 0. {
                    left.push(s.p + offset);
                    right.push(s.p - offset);
                } else {
                    left.push(s.p - offset);
                    right.push(s.p + offset);
                }
            } else {
                let arm = previous.perp() * s.radius;
                let pi = std::f64::consts::PI + 0.0001;
                let mut t = 0.;
                while t < 1. {
                    left.push(s.input + arm.rotate(pi * t));
                    right.push(s.input + arm.rotate(pi - pi * t));
                    t += 1. / 13.;
                }
            }
            pl = *left.last().unwrap();
            pr = *right.last().unwrap();
            if next_sharp {
                previous_sharp = true;
            }
            continue;
        }
        previous_sharp = false;
        if s.cap {
            let offset = vector.perp() * s.radius;
            left.push(s.p + offset);
            right.push(s.p - offset);
            continue;
        }
        let offset = next.lerp(vector, next_dot).perp() * s.radius;
        let l = s.p + offset;
        let r = s.p - offset;
        if i <= 1 || (pl - l).dot(pl - l) > (size * 0.62).powi(2) {
            left.push(l);
            pl = l;
        }
        if i <= 1 || (pr - r).dot(pr - r) > (size * 0.62).powi(2) {
            right.push(r);
            pr = r;
        }
        previous = vector;
    }
    (simplify(&left, size * 0.05), simplify(&right, size * 0.05))
}

// A semicircle expressed as two cubic Beziers, shared by native paint and SVG.
fn cap(commands: &mut Vec<Command>, start: P, end: P) {
    let center = (start + end) * 0.5;
    let arm = start - center;
    let middle = center + arm.perp();
    let k = 0.5522847498307936;
    commands.push(Command::Cubic(
        (start + arm.perp() * k).v(),
        (middle + arm * k).v(),
        middle.v(),
    ));
    commands.push(Command::Cubic(
        (middle - arm * k).v(),
        (end + arm.perp() * k).v(),
        end.v(),
    ));
}
fn smooth_to(commands: &mut Vec<Command>, current: &mut P, control: &mut P, end: P) {
    let c = *current * 2. - *control;
    commands.push(Command::Quad(c.v(), end.v()));
    *control = c;
    *current = end;
}
fn render_partition(src: &[Sample], size: f64, anchor: Option<P>, commands: &mut Vec<Command>) {
    if src.is_empty() {
        return;
    }
    if src.len() == 1 || (src[0].p - src[src.len() - 1].p).len() < 0.000001 && src.len() == 2 {
        let center = src[0].p.rounded();
        let radius = (src[0].radius * 100. + 0.5).floor() / 100.;
        let a = center - P { x: radius, y: 0. };
        let b = center + P { x: radius, y: 0. };
        commands.push(Command::Move(a.v()));
        cap(commands, a, b);
        cap(commands, b, a);
        commands.push(Command::Close);
        return;
    }
    let (left, right) = tracks(src, size, anchor);
    if left.is_empty() || right.is_empty() {
        return;
    }
    let mut current = left[0].rounded();
    let mut control = current;
    commands.push(Command::Move(current.v()));
    for pair in left.windows(2) {
        smooth_to(
            commands,
            &mut current,
            &mut control,
            ((pair[0] + pair[1]) * 0.5).rounded(),
        );
    }
    let end = src.last().unwrap();
    let offset = (src[src.len() - 2].p - end.p).unit().perp() * end.radius;
    let a = (end.p + offset).rounded();
    let b = (end.p - offset).rounded();
    smooth_to(commands, &mut current, &mut control, a);
    cap(commands, a, b);
    current = b;
    control = b;
    for i in (1..right.len()).rev() {
        smooth_to(
            commands,
            &mut current,
            &mut control,
            ((right[i] + right[i - 1]) * 0.5).rounded(),
        );
    }
    let start = src[0];
    let offset = (start.p - src[1].p).unit().perp() * (-start.radius);
    let a = (start.p + offset).rounded();
    let b = (start.p - offset).rounded();
    smooth_to(commands, &mut current, &mut control, a);
    cap(commands, a, b);
    commands.push(Command::Close);
}

fn partition(points: &[Sample], size: f64, commands: &mut Vec<Command>) {
    if points.len() <= 2 {
        let mut src = points.to_vec();
        for p in &mut src {
            p.cap = true;
        }
        render_partition(&src, size, None, commands);
        return;
    }
    // Each partition keeps the incoming direction at a cut, including points
    // stripped near its ends. Acute reversals use the original pointer position.
    let finish = |a: usize,
                  a_elbow: bool,
                  b: usize,
                  b_elbow: bool,
                  duplicate: bool,
                  mut anchor: Option<P>,
                  commands: &mut Vec<Command>| {
        let mut start = points[a];
        if a_elbow {
            start.p = start.input;
        }
        start.cap = true;
        let mut end = points[b];
        if b_elbow {
            end.p = end.input;
        }
        end.cap = true;
        let mut indices: Vec<usize> = (a + 1..b).collect();
        if duplicate {
            indices.push(b);
        }
        let mut lo = 0;
        let mut hi = indices.len();
        while lo < hi {
            let next = points[indices[lo]];
            if (start.p - next.p).len() >= (start.radius + next.radius) * 0.25 {
                break;
            }
            anchor = Some(next.p);
            lo += 1;
        }
        while hi > lo {
            let prev = points[indices[hi - 1]];
            if (end.p - prev.p).len() >= (end.radius + prev.radius) * 0.25 {
                break;
            }
            hi -= 1;
        }
        let mut src = vec![start];
        for &i in &indices[lo..hi] {
            let mut s = points[i];
            s.cap = duplicate && i == b;
            src.push(s);
        }
        src.push(end);
        render_partition(&src, size, anchor, commands);
    };
    let mut a = 0;
    let mut a_elbow = false;
    let mut anchor = None;
    let mut previous = (points[1].p - points[0].p).unit();
    for i in 1..points.len() - 1 {
        let next = (points[i + 1].p - points[i].p).unit();
        let dot = previous.dot(next);
        previous = next;
        if dot < -0.8 {
            finish(a, a_elbow, i, true, false, anchor, commands);
            a = i;
            a_elbow = true;
            anchor = Some(points[i].p);
            continue;
        }
        if dot > 0.7 {
            continue;
        }
        let prev = points[i].p - points[i - 1].p;
        let next = points[i + 1].p - points[i].p;
        let radius = (points[i - 1].radius + points[i].radius + points[i + 1].radius) / 3.;
        if (prev.dot(prev) + next.dot(next)) / (radius * radius) < 1.5 {
            finish(a, a_elbow, i, false, true, anchor, commands);
            a = i;
            a_elbow = false;
            anchor = None;
        }
    }
    finish(a, a_elbow, points.len() - 1, false, false, anchor, commands);
}

pub(crate) fn geometry(raw: &[V], size: f32, style_size: f32, complete: bool) -> Geometry {
    let size = f64::from(size.max(0.1));
    let points = samples(raw, size, streamline(f64::from(style_size)), complete);
    let mut commands = vec![];
    partition(&points, size, &mut commands);
    Geometry {
        paths: if commands.is_empty() {
            vec![]
        } else {
            vec![Path {
                commands,
                filled: true,
            }]
        },
        centerline: points.iter().map(|s| s.p.v()).collect(),
        handles: vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn raster(d: &str) -> resvg::tiny_skia::Pixmap {
        let svg = format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="384" height="300"><path fill="#202124" d="{d}"/></svg>"##
        );
        let tree = resvg::usvg::Tree::from_str(&svg, &Default::default()).unwrap();
        let mut pixmap = resvg::tiny_skia::Pixmap::new(384, 300).unwrap();
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::identity(),
            &mut pixmap.as_mut(),
        );
        pixmap
    }
    #[test]
    fn mouse_ink_matches_reference_pressure_and_rendered_contours() {
        let data: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/ink-paths.json")).unwrap();
        for fixture in data["fixtures"].as_array().unwrap() {
            let name = fixture["name"].as_str().unwrap();
            let size = fixture["size"].as_f64().unwrap();
            let last = fixture["last"].as_bool().unwrap();
            let raw: Vec<_> = fixture["points"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| {
                    V::new(
                        p["x"].as_f64().unwrap() as f32,
                        p["y"].as_f64().unwrap() as f32,
                    )
                })
                .collect();
            let expected = fixture["samples"].as_array().unwrap();
            let actual = samples(&raw, size, streamline(size), last);
            assert_eq!(
                actual.len(),
                expected.len(),
                "{name}, {size}, complete={last}"
            );
            for (a, b) in actual.iter().zip(expected) {
                for (value, key) in [(a.p.x, "x"), (a.p.y, "y"), (a.radius, "radius")] {
                    assert!(
                        (value - b[key].as_f64().unwrap()).abs() < 0.00001,
                        "{name}, {size}, complete={last}, {key}: {value} vs {}",
                        b[key]
                    );
                }
            }
            let ours = geometry(&raw, size as f32, size as f32, last);
            let d = ours.paths.iter().map(Path::svg).collect::<String>();
            assert!(!d.contains("NaN") && !d.contains("inf"));
            let ours = raster(&d);
            let reference = raster(fixture["d"].as_str().unwrap());
            let coverage: u64 = reference.pixels().iter().map(|p| p.alpha() as u64).sum();
            let error: u64 = ours
                .pixels()
                .iter()
                .zip(reference.pixels())
                .map(|(a, b)| a.alpha().abs_diff(b.alpha()) as u64)
                .sum();
            let ratio = error as f64 / coverage.max(1) as f64;
            if let Some(dir) = std::env::var_os("LAHZA_INK_PREVIEW") {
                if last && size == 4. && ["letter_a", "speed_change", "hook"].contains(&name) {
                    let dir = std::path::PathBuf::from(dir);
                    ours.save_png(dir.join(format!("{name}-ours.png"))).unwrap();
                    reference
                        .save_png(dir.join(format!("{name}-reference.png")))
                        .unwrap();
                }
            }
            assert!(
                ratio < 0.01,
                "{name}, {size}, complete={last}: contour coverage difference {ratio:.5}"
            );
        }
    }

    #[test]
    fn scaled_ink_preserves_style_and_live_state_is_not_persisted() {
        use crate::{AnnotationMark, NormPoint, Tool};
        use gpui::{point, px, size, Bounds};
        let bounds = Bounds::new(point(px(0.), px(0.)), size(px(400.), px(300.)));
        let big_bounds = Bounds::new(point(px(0.), px(0.)), size(px(1200.), px(900.)));
        let mut mark = AnnotationMark {
            tool: Tool::Pen,
            stroke_width: 12.,
            ink_in_progress: true,
            points: (0..60)
                .map(|i| NormPoint {
                    x: 0.1 + i as f32 * 0.01,
                    y: 0.5 + (i as f32 * 0.1).sin() * 0.2,
                })
                .collect(),
            ..Default::default()
        };
        let before = crate::geometry::geometry(&mark, bounds);
        let end = before.centerline.last().unwrap();
        let pointer = mark.points.last().unwrap();
        assert!(end.sub(V::new(pointer.x * 400., pointer.y * 300.)).len() > 0.5);
        mark.ink_in_progress = false;
        let finished = crate::geometry::geometry(&mark, bounds);
        assert!(
            finished
                .centerline
                .last()
                .unwrap()
                .sub(V::new(pointer.x * 400., pointer.y * 300.))
                .len()
                < 0.001
        );
        let mut enlarged = mark.clone();
        enlarged.scale_stroke_width(3.);
        let big = crate::geometry::geometry(&enlarged, big_bounds);
        assert_eq!(big.centerline.len(), finished.centerline.len());
        for (a, b) in big.centerline.iter().zip(&finished.centerline) {
            assert!(a.sub(b.mul(3.)).len() < 0.001);
        }
        let mut reduced = big.paths[0].clone();
        for c in &mut reduced.commands {
            match c {
                Command::Move(p) | Command::Line(p) => *p = p.mul(1. / 3.),
                Command::Quad(a, b) => {
                    *a = a.mul(1. / 3.);
                    *b = b.mul(1. / 3.);
                }
                Command::Cubic(a, b, c) => {
                    *a = a.mul(1. / 3.);
                    *b = b.mul(1. / 3.);
                    *c = c.mul(1. / 3.);
                }
                Command::Close => {}
            }
        }
        let small = raster(&finished.paths[0].svg());
        let scaled = raster(&reduced.svg());
        let coverage: u64 = small.pixels().iter().map(|p| p.alpha() as u64).sum();
        let error: u64 = small
            .pixels()
            .iter()
            .zip(scaled.pixels())
            .map(|(a, b)| a.alpha().abs_diff(b.alpha()) as u64)
            .sum();
        assert!((error as f64) / (coverage as f64) < 0.01);
        let svg = crate::svg::annotations_svg(&[mark.clone()], 0., 0., 1200, 900, 3.);
        assert!(svg.contains(&big.paths[0].svg()));
        mark.ink_in_progress = true;
        let saved = serde_json::to_string(&mark).unwrap();
        assert!(!saved.contains("inkInProgress") && !saved.contains("inkStyleWidth"));
        let restored: AnnotationMark = serde_json::from_str(&saved).unwrap();
        assert!(!restored.ink_in_progress);
        assert!(restored.ink_style_width.is_none());
    }
}
