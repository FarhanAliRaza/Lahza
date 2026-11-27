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

/// Full length of the subpaths (closed ones include their closing edge).
pub fn length(subpaths: &[Poly]) -> f64 {
    total_length(&polylines(subpaths))
}

/// The part of the polylines between fractions [t0, t1] of total length.
pub fn trim(subpaths: &[Poly], t0: f64, t1: f64) -> Vec<Vec<Pt>> {
    trim_measured(subpaths, t0, t1).into_iter().map(|line| line.into_iter().map(|(p, _)| p).collect()).collect()
}

/// `trim`, with each vertex's fraction of the full (untrimmed) length.
pub fn trim_measured(subpaths: &[Poly], t0: f64, t1: f64) -> Vec<Vec<(Pt, f64)>> {
    let lines = polylines(subpaths);
    let total = total_length(&lines);
    let (from, to) = (t0 * total, t1 * total);
    let u = |x: f64| if total > 0.0 { x / total } else { 0.0 };
    let mut out = Vec::new();
    let mut acc = 0.0;
    for line in &lines {
        let mut cur: Vec<(Pt, f64)> = Vec::new();
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
                ((w[0].0 + (w[1].0 - w[0].0) * k, w[0].1 + (w[1].1 - w[0].1) * k), u(a + l * k))
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
        let n = crate::dm::hypot(v.0, v.1);
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

/// `flatten`, remembered per path text and tolerance (rounded down to a
/// power of two, so a zoom keeps hitting the cache): parsing a map's
/// countries every frame cost more than drawing them. The cache holds the
/// text itself, so an entry's address cannot be reused by another path.
pub fn flatten_cached(d: &std::sync::Arc<str>, tolerance: f64) -> Result<std::sync::Arc<Vec<Poly>>, String> {
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::sync::Arc;
    type Cache = HashMap<(usize, usize, i32), (Arc<str>, Arc<Vec<Poly>>)>;
    thread_local! {
        static CACHE: RefCell<Cache> = RefCell::new(HashMap::new());
    }
    let bucket = crate::dm::log2(tolerance.max(1e-9)).floor() as i32;
    let key = (Arc::as_ptr(d) as *const u8 as usize, d.len(), bucket);
    if let Some(hit) = CACHE.with(|c| c.borrow().get(&key).map(|e| e.1.clone())) {
        return Ok(hit);
    }
    let polys = Arc::new(flatten(d, 2f64.powi(bucket))?);
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if c.len() > 8192 {
            c.clear();
        }
        c.insert(key, (d.clone(), polys.clone()));
    });
    Ok(polys)
}

/// Outlines of a path, interpolated when it is mid-morph.
pub fn morph(m: &PathMorph, tolerance: f64) -> Result<Vec<Poly>, String> {
    let a = flatten_cached(&m.from, tolerance)?;
    let Some(to) = &m.to else { return Ok((*a).clone()) };
    let a = (*a).clone();
    let b = (*flatten_cached(to, tolerance)?).clone();
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

/// Smooth seeded value noise in −1..1 along a line; with `period` cells
/// (> 0) it wraps, so a closed outline has no seam.
pub fn noise1(x: f64, seed: u32, period: i64) -> f64 {
    let i = x.floor();
    let f = x - i;
    let h = |k: i64| {
        let k = if period > 0 { k.rem_euclid(period) } else { k };
        crate::noise::hash3(k as i32, (k >> 31) as i32, 0, seed) as f64 * 2.0 - 1.0
    };
    let (a, b) = (h(i as i64), h(i as i64 + 1));
    a + (b - a) * (f * f * (3.0 - 2.0 * f))
}

/// `line` with extra points so no segment is longer than `step`, and each
/// point's arc length.
fn densify(line: &[Pt], step: f64) -> (Vec<Pt>, Vec<f64>) {
    let mut pts = Vec::with_capacity(line.len());
    let mut s = Vec::with_capacity(line.len());
    let mut acc = 0.0;
    pts.push(line[0]);
    s.push(0.0);
    for w in line.windows(2) {
        let l = seg_len(w[0], w[1]);
        let n = ((l / step).ceil() as usize).clamp(1, 4096);
        for k in 1..=n {
            let t = k as f64 / n as f64;
            pts.push((w[0].0 + (w[1].0 - w[0].0) * t, w[0].1 + (w[1].1 - w[0].1) * t));
            s.push(acc + l * t);
        }
        acc += l;
    }
    (pts, s)
}

/// Unit normals (left of the direction of travel) of a polyline; a closed
/// one (`pts` without its repeated end) wraps around.
fn normals(pts: &[Pt], closed: bool) -> Vec<Pt> {
    let n = pts.len();
    (0..n)
        .map(|i| {
            let (a, b) = if closed {
                (pts[(i + n - 1) % n], pts[(i + 1) % n])
            } else {
                (pts[i.saturating_sub(1)], pts[(i + 1).min(n - 1)])
            };
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            let l = crate::dm::hypot(dx, dy);
            if l < 1e-12 {
                (0.0, 0.0)
            } else {
                (-dy / l, dx / l)
            }
        })
        .collect()
}

/// A polyline that ends where it starts.
fn is_loop(line: &[Pt]) -> bool {
    line.len() > 3 && seg_len(line[0], line[line.len() - 1]) < 1e-6
}

/// Filled outlines of dip-pen lines along `lines` (layer units): the width
/// swells with seeded pressure, open ends taper to points, the centre
/// wobbles. Open lines give one polygon each; a closed loop gives an outer
/// and a reversed inner ring (non-zero fill leaves the inside empty).
pub fn ink_outline(lines: &[Vec<Pt>], width: f64, ink: &crate::document::Ink, tolerance: f64) -> Vec<Vec<Pt>> {
    let mut out = Vec::new();
    let step = (width * 0.6).min(ink.wobble_scale.max(1.0) / 6.0).min(ink.pressure_scale.max(1.0) / 6.0).max(tolerance * 2.0).max(1e-3);
    for (index, line) in lines.iter().enumerate() {
        if line.len() < 2 {
            continue;
        }
        let seed = ink.seed.wrapping_add((index as u32).wrapping_mul(7919));
        let closed = is_loop(line);
        let (mut pts, mut s) = densify(line, step);
        let total = *s.last().unwrap_or(&0.0);
        if total <= 1e-9 {
            continue;
        }
        if closed {
            pts.pop();
            s.pop();
        }
        let nrm = normals(&pts, closed);
        // Noise coordinates: periodic around a loop (whole cells), plain
        // along an open line.
        let coord = |s: f64, scale: f64| -> (f64, i64) {
            let scale = scale.max(1e-3);
            if closed {
                let cells = (total / scale).round().max(3.0);
                (s / total * cells, cells as i64)
            } else {
                (s / scale, 0)
            }
        };
        let (t0, t1) = (ink.taper[0].max(0.0).min(total * 0.45), ink.taper[1].max(0.0).min(total * 0.45));
        let ease = |u: f64| {
            let u = u.clamp(0.0, 1.0);
            1.0 - (1.0 - u) * (1.0 - u)
        };
        let mut left = Vec::with_capacity(pts.len());
        let mut right = Vec::with_capacity(pts.len());
        for i in 0..pts.len() {
            let (x, period) = coord(s[i], ink.pressure_scale);
            let mut w = width * (1.0 + ink.pressure * noise1(x, seed, period)).max(0.0);
            if !closed {
                if t0 > 0.0 {
                    w *= ease(s[i] / t0);
                }
                if t1 > 0.0 {
                    w *= ease((total - s[i]) / t1);
                }
            }
            let (x, period) = coord(s[i], ink.wobble_scale);
            let off = ink.wobble * noise1(x, seed ^ 0x5bd1_e995, period);
            let (p, n) = (pts[i], nrm[i]);
            let c = (p.0 + n.0 * off, p.1 + n.1 * off);
            left.push((c.0 + n.0 * w * 0.5, c.1 + n.1 * w * 0.5));
            right.push((c.0 - n.0 * w * 0.5, c.1 - n.1 * w * 0.5));
        }
        right.reverse();
        if closed {
            out.push(left);
            out.push(right);
        } else {
            left.extend(right);
            out.push(left);
        }
    }
    out.retain(|p| p.len() >= 3);
    out
}

/// Hand-cut outlines: each subpath pushed in and out along its normals by
/// seeded noise (`amount` px, a bump every `scale` px).
pub fn roughen(subpaths: &[Poly], rough: &crate::document::Rough, tolerance: f64) -> Vec<Poly> {
    if rough.amount == 0.0 {
        return subpaths.to_vec();
    }
    let scale = rough.scale.max(1e-3);
    let step = (scale / 5.0).max(tolerance * 2.0).max(1e-3);
    subpaths
        .iter()
        .enumerate()
        .map(|(index, (pts, closed))| {
            let seed = rough.seed.wrapping_add((index as u32).wrapping_mul(104_729));
            let mut line = pts.clone();
            if *closed && line.first() != line.last() {
                line.push(line[0]);
            }
            let (mut dense, mut s) = densify(&line, step);
            let total = *s.last().unwrap_or(&0.0);
            if *closed {
                dense.pop();
                s.pop();
            }
            let nrm = normals(&dense, *closed);
            let cells = (total / scale).round().max(3.0);
            let moved = dense
                .iter()
                .zip(&nrm)
                .zip(&s)
                .map(|((p, n), s)| {
                    let k = if *closed { rough.amount * noise1(s / total.max(1e-9) * cells, seed, cells as i64) } else { rough.amount * noise1(s / scale, seed, 0) };
                    (p.0 + n.0 * k, p.1 + n.1 * k)
                })
                .collect();
            (moved, *closed)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ink_tapers_to_points_and_is_stable() {
        let ink = crate::document::Ink { taper: [10.0, 10.0], pressure: 0.4, pressure_scale: 40.0, wobble: 0.6, wobble_scale: 30.0, seed: 3, fixed: false };
        let line = vec![(0.0, 0.0), (100.0, 0.0), (200.0, 40.0)];
        let a = ink_outline(&[line.clone()], 4.0, &ink, 0.25);
        let b = ink_outline(&[line], 4.0, &ink, 0.25);
        assert_eq!(a, b, "same seed, same line");
        assert_eq!(a.len(), 1);
        let poly = &a[0];
        let n = poly.len();
        // The first and last left points meet the matching right points.
        assert!(seg_len(poly[0], poly[n - 1]) < 1e-6, "start is a point");
        assert!(seg_len(poly[n / 2 - 1], poly[n / 2]) < 1e-6, "end is a point");
        // Mid-line the width is near 4 (pressure keeps it within 0.6..1.4×).
        let mid = n / 4;
        let w = seg_len(poly[mid], poly[n - 1 - mid]);
        assert!(w > 4.0 * 0.5 && w < 4.0 * 1.5, "width {w}");
        // A closed loop gives two rings and no taper.
        let ring: Vec<Pt> = (0..=64).map(|i| {
            let a = std::f64::consts::TAU * (i % 64) as f64 / 64.0;
            (50.0 * crate::dm::cos(a), 50.0 * crate::dm::sin(a))
        }).collect();
        assert_eq!(ink_outline(&[ring], 4.0, &ink, 0.25).len(), 2);
    }

    #[test]
    fn rough_closed_outline_has_no_seam() {
        let sq = flatten("M -50 -50 L 50 -50 L 50 50 L -50 50 Z", 0.25).unwrap();
        let r = roughen(&sq, &crate::document::Rough { amount: 3.0, scale: 20.0, seed: 1 }, 0.25);
        let pts = &r[0].0;
        let (first, last) = (pts[0], pts[pts.len() - 1]);
        assert!(seg_len(first, last) < 6.0, "seam {first:?} {last:?}");
        assert!(pts.iter().any(|p| (p.1 + 50.0).abs() > 0.5 && p.0.abs() < 40.0), "edges move");
    }

    #[test]
    fn point_at_is_the_end_of_trim() {
        let subpaths = flatten("M 0 0 C 100 -150 300 150 400 0 M 0 50 L 200 50 Z", tolerance(1.0)).unwrap();
        for f in [0.05, 0.3, 0.5, 0.71, 0.999, 1.0] {
            let end = *trim(&subpaths, 0.0, f).last().unwrap().last().unwrap();
            let (point, dir) = point_at(&subpaths, f).unwrap();
            assert_eq!(point, end, "at {f}");
            assert!((crate::dm::hypot(dir.0, dir.1) - 1.0).abs() < 1e-9);
        }
        assert_eq!(point_at(&subpaths, 0.0).unwrap().0, (0.0, 0.0));
        // The direction turns smoothly: nearby points have nearby directions.
        let angle = |f: f64| {
            let (_, d) = point_at(&subpaths, f).unwrap();
            crate::dm::atan2(d.1, d.0).to_degrees()
        };
        for i in 0..400 {
            let f = 0.1 + i as f64 * 0.0005;
            assert!((angle(f + 0.0005) - angle(f)).abs() < 0.5, "jump at {f}");
        }
    }
}
