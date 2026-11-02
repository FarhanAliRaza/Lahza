//! Path geometry shared by evaluation and rendering: SVG path data
//! flattened to polylines, morphs between outlines, and arc-length
//! measures. `trim` (the drawn part of a stroke) and `point_at` (where a
//! `follow` layer sits) measure length the same way, so a layer riding a
//! stroke's head sits exactly where the stroke ends.

use crate::compile::PathMorph;

type Pt = (f64, f64);

/// A flattened subpath and whether it is closed.
pub type Poly = (Vec<Pt>, bool);

/// Flattening tolerance in layer units for a layer drawn at `scale`
/// canvas pixels per unit: fine enough for the on-screen size (¼ px).
pub fn tolerance(scale: f64) -> f64 {
    0.25 / scale.max(1e-6)
}

/// Flattened subpaths of SVG path data, in layer units (each a polyline,
/// with whether it was closed).
pub fn flatten(d: &str, tolerance: f64) -> Result<Vec<Poly>, String> {
    let path = kurbo::BezPath::from_svg(d).map_err(|e| format!("bad path data: {e}"))?;
    let mut out: Vec<Poly> = Vec::new();
    kurbo::flatten(&path, tolerance.max(1e-4), |el| match el {
        kurbo::PathEl::MoveTo(p) => out.push((vec![(p.x, p.y)], false)),
        kurbo::PathEl::LineTo(p) => {
            if let Some(last) = out.last_mut() {
                last.0.push((p.x, p.y));
            }
        }
        kurbo::PathEl::ClosePath => {
            if let Some(last) = out.last_mut() {
                last.1 = true;
            }
        }
        _ => {}
    });
    Ok(out.into_iter().filter(|(pts, _)| pts.len() >= 2).collect())
}

/// Subpaths as open polylines, closed ones ending where they start.
fn polylines(subpaths: &[Poly]) -> Vec<Vec<Pt>> {
    subpaths
        .iter()
        .map(|(pts, closed)| {
            let mut p = pts.clone();
            if *closed && p.first() != p.last() {
                p.push(p[0]);
            }
            p
        })
        .collect()
}

fn seg_len(a: Pt, b: Pt) -> f64 {
    ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt()
}

fn total_length(lines: &[Vec<Pt>]) -> f64 {
    lines.iter().flat_map(|l| l.windows(2).map(|w| seg_len(w[0], w[1]))).sum()
}

/// The part of the polylines between fractions [t0, t1] of total length.
pub fn trim(subpaths: &[Poly], t0: f64, t1: f64) -> Vec<Vec<Pt>> {
    let lines = polylines(subpaths);
    let total = total_length(&lines);
    let (from, to) = (t0 * total, t1 * total);
    let mut out = Vec::new();
    let mut acc = 0.0;
    for line in &lines {
        let mut cur: Vec<Pt> = Vec::new();
        for w in line.windows(2) {
            let l = seg_len(w[0], w[1]);
            let (a, b) = (acc, acc + l);
            acc = b;
            if b < from || a > to || l <= 0.0 {
                if !cur.is_empty() && a > to {
                    break;
                }
                continue;
            }
            let lerp = |x: f64| {
                let k = ((x - a) / l).clamp(0.0, 1.0);
                (w[0].0 + (w[1].0 - w[0].0) * k, w[0].1 + (w[1].1 - w[0].1) * k)
            };
            if cur.is_empty() {
                cur.push(lerp(from.max(a)));
            }
            cur.push(lerp(to.min(b)));
        }
        if cur.len() >= 2 {
            out.push(cur);
        }
    }
    out
}

/// The point at fraction `f` (0…1) of the total length, measured as
/// `trim` measures it (so it is the end of `trim(…, t0, f)`), and the unit
/// direction of travel there. The direction turns smoothly through each
/// polyline vertex (along its bisector) instead of jumping between chords.
pub fn point_at(subpaths: &[Poly], f: f64) -> Option<(Pt, Pt)> {
    let lines = polylines(subpaths);
    let x = f.clamp(0.0, 1.0) * total_length(&lines);
    let mut acc = 0.0;
    let mut hit = None;
    'walk: for (li, line) in lines.iter().enumerate() {
        for si in 0..line.len().saturating_sub(1) {
            let l = seg_len(line[si], line[si + 1]);
            let a = acc;
            acc += l;
            if l <= 0.0 {
                continue;
            }
            if a > x {
                break 'walk;
            }
            hit = Some((li, si, a, l));
        }
    }
    let (li, si, a, l) = hit?;
    let line = &lines[li];
    let k = ((x - a) / l).clamp(0.0, 1.0);
    let (p, q) = (line[si], line[si + 1]);
    let point = (p.0 + (q.0 - p.0) * k, p.1 + (q.1 - p.1) * k);
    let unit = |v: Pt| {
        let n = v.0.hypot(v.1);
        if n > 0.0 { (v.0 / n, v.1 / n) } else { v }
    };
    let dir = unit((q.0 - p.0, q.1 - p.1));
    // Neighbouring chords (around the loop when closed).
    let n = line.len() - 1;
    let closed = subpaths[li].1;
    let neighbour = |step: isize| {
        let mut j = si as isize;
        for _ in 0..n {
            j += step;
            if !closed && (j < 0 || j >= n as isize) {
                break;
            }
            let j = j.rem_euclid(n as isize) as usize;
            let v = (line[j + 1].0 - line[j].0, line[j + 1].1 - line[j].1);
            if v.0 != 0.0 || v.1 != 0.0 {
                return unit(v);
            }
        }
        dir
    };
    let (before, after) = (neighbour(-1), neighbour(1));
    let start = unit((before.0 + dir.0, before.1 + dir.1));
    let end = unit((dir.0 + after.0, dir.1 + after.1));
    let tangent = unit((start.0 * (1.0 - k) + end.0 * k, start.1 * (1.0 - k) + end.1 * k));
    Some((point, if tangent.0 == 0.0 && tangent.1 == 0.0 { dir } else { tangent }))
}

/// `n` points evenly spaced along a polyline (closed: around the loop).
fn resample(pts: &[(f64, f64)], closed: bool, n: usize) -> Vec<(f64, f64)> {
    let mut p = pts.to_vec();
    if closed && p.first() != p.last() {
        p.push(p[0]);
    }
    let lens: Vec<f64> = p.windows(2).map(|w| ((w[1].0 - w[0].0).powi(2) + (w[1].1 - w[0].1).powi(2)).sqrt()).collect();
    let total: f64 = lens.iter().sum();
    if total <= 0.0 || p.len() < 2 {
        return vec![p[0]; n];
    }
    let steps = if closed { n } else { n - 1 };
    let mut out = Vec::with_capacity(n);
    let (mut seg, mut acc) = (0usize, 0.0f64);
    for i in 0..n {
        let target = total * i as f64 / steps.max(1) as f64;
        while seg + 1 < lens.len() && acc + lens[seg] < target {
            acc += lens[seg];
            seg += 1;
        }
        let k = if lens[seg] > 0.0 { ((target - acc) / lens[seg]).clamp(0.0, 1.0) } else { 0.0 };
        out.push((p[seg].0 + (p[seg + 1].0 - p[seg].0) * k, p[seg].1 + (p[seg + 1].1 - p[seg].1) * k));
    }
    out
}

fn centroid(pts: &[(f64, f64)]) -> (f64, f64) {
    let n = pts.len().max(1) as f64;
    (pts.iter().map(|p| p.0).sum::<f64>() / n, pts.iter().map(|p| p.1).sum::<f64>() / n)
}

/// Point correspondence for closed outlines: the rotation (and direction)
/// of `b` that sits closest to `a`.
fn align(a: &[(f64, f64)], b: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let n = a.len();
    let stride = (n / 64).max(1);
    let cost = |c: &[(f64, f64)], off: usize| -> f64 {
        (0..n).step_by(stride).map(|i| {
            let q = c[(i + off) % n];
            (q.0 - a[i].0).powi(2) + (q.1 - a[i].1).powi(2)
        }).sum()
    };
    let reversed: Vec<(f64, f64)> = b.iter().rev().copied().collect();
    let mut best = (f64::INFINITY, 0usize, false);
    for (rev, c) in [(false, b), (true, reversed.as_slice())] {
        for off in 0..n {
            let v = cost(c, off);
            if v < best.0 {
                best = (v, off, rev);
            }
        }
    }
    let c = if best.2 { &reversed } else { b };
    (0..n).map(|i| c[(i + best.1) % n]).collect()
}

/// Outlines of a path, interpolated when it is mid-morph.
pub fn morph(m: &PathMorph, tolerance: f64) -> Result<Vec<Poly>, String> {
    let a = flatten(&m.from, tolerance)?;
    let Some(to) = &m.to else { return Ok(a) };
    let b = flatten(to, tolerance)?;
    let t = m.t;
    let count = a.len().max(b.len());
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        // A subpath without a partner grows from (or shrinks to) a point.
        let (pa, ca) = a.get(i).cloned().unwrap_or_else(|| {
            let c = centroid(&b[i].0);
            (vec![c, c], b[i].1)
        });
        let (pb, cb) = b.get(i).cloned().unwrap_or_else(|| {
            let c = centroid(&a[i].0);
            (vec![c, c], a[i].1)
        });
        let closed = ca || cb;
        let n = pa.len().max(pb.len()).clamp(32, 4096);
        let ra = resample(&pa, closed, n);
        let mut rb = resample(&pb, closed, n);
        if closed {
            rb = align(&ra, &rb);
        }
        let pts = ra.iter().zip(&rb).map(|(p, q)| (p.0 + (q.0 - p.0) * t, p.1 + (q.1 - p.1) * t)).collect();
        out.push((pts, closed));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn point_at_is_the_end_of_trim() {
        let subpaths = flatten("M 0 0 C 100 -150 300 150 400 0 M 0 50 L 200 50 Z", tolerance(1.0)).unwrap();
        for f in [0.05, 0.3, 0.5, 0.71, 0.999, 1.0] {
            let end = *trim(&subpaths, 0.0, f).last().unwrap().last().unwrap();
            let (point, dir) = point_at(&subpaths, f).unwrap();
            assert_eq!(point, end, "at {f}");
            assert!((dir.0.hypot(dir.1) - 1.0).abs() < 1e-9);
        }
        assert_eq!(point_at(&subpaths, 0.0).unwrap().0, (0.0, 0.0));
        // The direction turns smoothly: nearby points have nearby directions.
        let angle = |f: f64| {
            let (_, d) = point_at(&subpaths, f).unwrap();
            d.1.atan2(d.0).to_degrees()
        };
        for i in 0..400 {
            let f = 0.1 + i as f64 * 0.0005;
            assert!((angle(f + 0.0005) - angle(f)).abs() < 0.5, "jump at {f}");
        }
    }
}
