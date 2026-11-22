//! A flat region followed as one plane.
//!
//! The region marked in one frame (four corners) is the template. Each
//! later frame is aligned to it directly, on the picture and not on
//! picked points: every pixel of the region with some gradient pulls the
//! eight numbers of the homography (inverse compositional Gauss–Newton,
//! Baker and Matthews), coarse to fine down a pyramid. So a surface with
//! little on it still holds, as long as the region as a whole has
//! something in it.
//!
//! - Always against the marked frame, so the track does not drift. The
//!   template is taken again from the current frame only when the region
//!   has changed size or look too much to compare (each time adds a
//!   little drift, so it is rare).
//! - Brightness and contrast are taken out per frame (both sides are
//!   normalised), and pixels that disagree strongly are left out (Tukey
//!   weights), so a hand over the table or a changing light does not
//!   pull the plane.
//! - A frame that does not align from where the last one was is tried
//!   from the previous frame's picture, then by a search of shifts at
//!   the coarsest level; a frame that still does not align is reported
//!   as not found, and the next one is tried from the last good place.

use crate::image::{to_base, to_level, Gray, Pyramid};
use crate::{convex, signed_area2, Quad, H3};

#[derive(Clone, Copy, Debug)]
pub struct Options {
    /// Pixels used per pyramid level.
    pub max_points: usize,
    /// Correlation (0–1) of the region with its template, over the
    /// pixels that are not covered, under which a frame is not found.
    pub min_score: f64,
    /// Share of the region's pixels that must match the template
    /// closely (the rest may be covered).
    pub min_agree: f64,
    /// Share of the region that must be inside the frame.
    pub min_visible: f64,
    /// Take the template again when the region changes too much.
    pub rekey: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options { max_points: 6000, min_score: 0.6, min_agree: 0.45, min_visible: 0.35, rekey: true }
    }
}

/// One frame of a track.
#[derive(Clone, Copy, Debug)]
pub struct Sample {
    pub found: bool,
    /// Marked frame → this frame.
    pub h: H3,
    /// Where the marked corners are in this frame.
    pub corners: Quad,
    /// Correlation of the region with its template, 0–1, over the pixels
    /// that agree.
    pub score: f64,
    /// Share of the region inside the frame.
    pub visible: f64,
    /// Share of the region's pixels that match the template closely
    /// (the rest is covered, or lit differently).
    pub agree: f64,
}

/// One pyramid level of a template: its pixels, in level-0 coordinates.
struct Level {
    x: Vec<f32>,
    y: Vec<f32>,
    /// Template values, zero mean and unit deviation.
    t: Vec<f32>,
    /// Steepest-descent rows, in normalised coordinates.
    sd: Vec<[f32; 8]>,
}

struct Template {
    levels: Vec<Level>,
    quad: Quad,
    centre: [f64; 2],
    scale: f64,
}

#[derive(Clone, Copy, Debug)]
struct Fit {
    h: H3,
    score: f64,
    visible: f64,
    agree: f64,
    /// The middle of the residuals' sizes: the footage's noise against
    /// the template's contrast, when the plane is found.
    spread: f64,
}

/// Inside the quad, at least `margin` from every side.
fn inside(q: &Quad, sign: f64, x: f64, y: f64, margin: f64) -> bool {
    (0..4).all(|i| {
        let (a, b) = (q[i], q[(i + 1) % 4]);
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        ((dx * (y - a[1]) - dy * (x - a[0])) * sign) >= margin * (dx * dx + dy * dy).sqrt()
    })
}

impl Template {
    /// `allow` says which pixels (level-0 coordinates) may be used: what
    /// covers the plane is kept out.
    fn new(pyr: &Pyramid, quad: &Quad, max_points: usize, allow: &dyn Fn(f64, f64) -> bool) -> Result<Template, String> {
        if !convex(quad) {
            return Err("the four corners must go round the region in order (no crossing, no three in a line)".into());
        }
        let sign = signed_area2(quad).signum();
        let centre = [quad.iter().map(|p| p[0]).sum::<f64>() / 4.0, quad.iter().map(|p| p[1]).sum::<f64>() / 4.0];
        let scale = quad.iter().map(|p| ((p[0] - centre[0]).powi(2) + (p[1] - centre[1]).powi(2)).sqrt()).sum::<f64>() / 4.0;
        if scale < 6.0 {
            return Err("the region is too small to follow at this size: mark a larger one, or track at a larger width".into());
        }
        let mut levels = Vec::new();
        for (l, img) in pyr.levels.iter().enumerate() {
            match Level::new(img, l, quad, sign, centre, scale, max_points, allow) {
                Some(level) => levels.push(level),
                None => break,
            }
        }
        if levels.is_empty() {
            return Err("nothing to hold on to in the region: it is flat (no edges, no texture) or outside the frame".into());
        }
        Ok(Template { levels, quad: *quad, centre, scale })
    }
}

impl Level {
    #[allow(clippy::too_many_arguments)]
    fn new(img: &Gray, l: usize, quad: &Quad, sign: f64, centre: [f64; 2], scale: f64, max_points: usize, allow: &dyn Fn(f64, f64) -> bool) -> Option<Level> {
        let k = (1u32 << l) as f64;
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for p in quad {
            x0 = x0.min(to_level(p[0], l));
            y0 = y0.min(to_level(p[1], l));
            x1 = x1.max(to_level(p[0], l));
            y1 = y1.max(to_level(p[1], l));
        }
        if img.w < 3 || img.h < 3 {
            return None;
        }
        let (ix0, iy0) = (x0.ceil().max(1.0) as usize, y0.ceil().max(1.0) as usize);
        let (ix1, iy1) = ((x1.floor() as isize).min(img.w as isize - 2), (y1.floor() as isize).min(img.h as isize - 2));
        if ix1 < ix0 as isize || iy1 < iy0 as isize {
            return None;
        }
        // (x, y, value, gx, gy), gradients per level-0 pixel.
        let mut cand: Vec<(f32, f32, f32, f32, f32)> = Vec::new();
        for j in iy0..=iy1 as usize {
            for i in ix0..=ix1 as usize {
                let (bx, by) = (to_base(i as f64, l), to_base(j as f64, l));
                // Clear of the edge by what the blur and the gradient reach,
                // so nothing outside the plane is in the template.
                if !inside(quad, sign, bx, by, 2.0 * k) || !allow(bx, by) {
                    continue;
                }
                let gx = (img.at(i + 1, j) - img.at(i - 1, j)) * 0.5 / k as f32;
                let gy = (img.at(i, j + 1) - img.at(i, j - 1)) * 0.5 / k as f32;
                cand.push((bx as f32, by as f32, img.at(i, j), gx, gy));
            }
        }
        if cand.len() < 40 {
            return None;
        }
        // Photometric normalisation over the whole region.
        let n = cand.len() as f64;
        let mean = cand.iter().map(|c| c.2 as f64).sum::<f64>() / n;
        let dev = (cand.iter().map(|c| (c.2 as f64 - mean).powi(2)).sum::<f64>() / n).sqrt();
        if dev < 1.5 {
            return None;
        }
        // Keep the pixels that say something: where there are more than
        // twice what is wanted, the stronger gradients; then evenly.
        if cand.len() > max_points * 2 {
            let mut mags: Vec<f32> = cand.iter().map(|c| c.3 * c.3 + c.4 * c.4).collect();
            let cut = mags.len() - max_points * 2;
            let (_, &mut threshold, _) = mags.select_nth_unstable_by(cut, |a, b| a.total_cmp(b));
            cand.retain(|c| c.3 * c.3 + c.4 * c.4 >= threshold);
        }
        if cand.len() > max_points {
            let step = cand.len() as f64 / max_points as f64;
            cand = (0..max_points).map(|i| cand[(i as f64 * step) as usize]).collect();
        }
        let mut level = Level { x: Vec::with_capacity(cand.len()), y: Vec::with_capacity(cand.len()), t: Vec::with_capacity(cand.len()), sd: Vec::with_capacity(cand.len()) };
        for (x, y, v, gx, gy) in cand {
            let (u, w) = ((x as f64 - centre[0]) / scale, (y as f64 - centre[1]) / scale);
            let (gx, gy) = (gx as f64 * scale / dev, gy as f64 * scale / dev);
            level.x.push(x);
            level.y.push(y);
            level.t.push(((v as f64 - mean) / dev) as f32);
            level.sd.push([gx * u, gy * u, gx * w, gy * w, gx, gy, -gx * u * u - gy * u * w, -gx * u * w - gy * w * w].map(|v| v as f32));
        }
        Some(level)
    }
}

fn solve8sym(a: &[[f64; 8]; 8], b: &[f64; 8]) -> Option<[f64; 8]> {
    let mut m = [[0.0f64; 9]; 8];
    for r in 0..8 {
        m[r][..8].copy_from_slice(&a[r]);
        m[r][8] = b[r];
    }
    crate::solve8(m)
}

/// Samples the frame under every template pixel (NaN outside the frame);
/// returns how many landed inside.
fn sample_level(level: &Level, img: &Gray, l: usize, h: H3, vals: &mut Vec<f32>) -> usize {
    let m = h.0.map(|v| v as f32);
    let k = 1.0 / (1u32 << l) as f32;
    vals.clear();
    let mut n = 0usize;
    for i in 0..level.x.len() {
        let (x, y) = (level.x[i], level.y[i]);
        let w = m[6] * x + m[7] * y + m[8];
        let px = ((m[0] * x + m[1] * y + m[2]) / w + 0.5) * k - 0.5;
        let py = ((m[3] * x + m[4] * y + m[5]) / w + 0.5) * k - 0.5;
        // The finest level settles the answer: sampled sharply there.
        let v = if l == 0 { img.sample_cubic(px, py) } else { img.sample(px, py) };
        match v {
            Some(v) if w > 1e-9 => {
                vals.push(v);
                n += 1;
            }
            _ => vals.push(f32::NAN),
        }
    }
    n
}

/// Scratch for one alignment.
#[derive(Default)]
struct Work {
    /// The middle of the residuals' sizes, from the last weighing.
    median: f32,
    vals: Vec<f32>,
    res: Vec<f32>,
    abs: Vec<f32>,
    wt: Vec<f32>,
}

/// Residuals of the sampled frame against the template, the frame's
/// brightness and contrast matched to the template's over the pixels
/// that agree (`wt`, refined `rounds` times: covered pixels fall out of
/// the match, so they do not skew it). Returns the weighted correlation
/// and the share of pixels that agree; None when the frame is flat there.
fn residuals(level: &Level, work: &mut Work, fresh: bool, close: f32) -> Option<(f64, f64)> {
    let count = level.t.len();
    if fresh {
        // No weights yet: start from the middle of each side and its
        // spread about it (medians), which a covered quarter of the
        // region does not move, as means and deviations would be.
        let mut vi: Vec<f32> = Vec::with_capacity(count);
        let mut vt: Vec<f32> = Vec::with_capacity(count);
        for i in 0..count {
            if !work.vals[i].is_nan() {
                vi.push(work.vals[i]);
                vt.push(level.t[i]);
            }
        }
        if vi.is_empty() {
            return None;
        }
        let mid = vi.len() / 2;
        let middle = |v: &mut Vec<f32>| -> (f32, f32) {
            let m = *v.select_nth_unstable_by(mid, |a, b| a.total_cmp(b)).1;
            for x in v.iter_mut() {
                *x = (*x - m).abs();
            }
            (m, *v.select_nth_unstable_by(mid, |a, b| a.total_cmp(b)).1)
        };
        let ((mi, si), (mt, st)) = (middle(&mut vi), middle(&mut vt));
        if si < 0.5 || st < 1e-3 {
            return None;
        }
        weigh(level, work, mi, st / si, mt);
    }
    let mut out = None;
    let mut last_agree = usize::MAX;
    for round in 0..6 {
        let (mut sw, mut si, mut sii, mut st, mut stt, mut sit) = (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64);
        let (mut valid, mut agree) = (0usize, 0usize);
        for i in 0..count {
            let v = work.vals[i];
            if v.is_nan() {
                continue;
            }
            valid += 1;
            let w = work.wt[i] as f64;
            if w <= 0.0 {
                continue;
            }
            agree += 1;
            let (v, t) = (v as f64, level.t[i] as f64);
            sw += w;
            si += w * v;
            sii += w * v * v;
            st += w * t;
            stt += w * t * t;
            sit += w * v * t;
        }
        if sw <= 1e-9 || valid == 0 {
            return None;
        }
        let (mi, mt) = (si / sw, st / sw);
        let (vi, vt) = ((sii / sw - mi * mi).max(0.0), (stt / sw - mt * mt).max(0.0));
        if vi.sqrt() < 0.75 || vt.sqrt() < 1e-3 {
            return None;
        }
        out = Some(((sit / sw - mi * mt) / (vi * vt).sqrt(), agree as f64 / valid as f64));
        // Settled: the same pixels agree as the round before.
        if round > 0 && agree == last_agree || round == 5 {
            break;
        }
        last_agree = agree;
        weigh(level, work, mi as f32, (vt / vi).sqrt() as f32, mt as f32);
        if !fresh && round == 0 {
            // Within an alignment the weights carry over from the step
            // before: one refinement a step is enough.
            break;
        }
    }
    // How much of the region matches closely: a fixed measure, since the
    // weights above widen to whatever spread they find, and so say
    // little of whether this is the same picture at all.
    let (mut valid, mut near) = (0usize, 0usize);
    for e in &work.res {
        if !e.is_nan() {
            valid += 1;
            if e.abs() < close {
                near += 1;
            }
        }
    }
    out.map(|(score, _)| (score, near as f64 / valid.max(1) as f64))
}

/// A residual under this (in deviations of the template) is a match,
/// until the footage's own noise is known. Unrelated pictures, however
/// well fitted, leave about a third of their pixels this close; a plane
/// found leaves all but what covers it.
const CLOSE: f32 = 0.4;

/// Residuals with the frame mapped onto the template's scale
/// ((v − mi)·gain + mt), and Tukey's weights from their spread.
fn weigh(level: &Level, work: &mut Work, mi: f32, gain: f32, mt: f32) {
    let count = level.t.len();
    work.res.clear();
    work.abs.clear();
    for i in 0..count {
        let v = work.vals[i];
        if v.is_nan() {
            work.res.push(f32::NAN);
        } else {
            let e = (v - mi) * gain + mt - level.t[i];
            work.res.push(e);
            work.abs.push(e.abs());
        }
    }
    if work.abs.is_empty() {
        return;
    }
    // The residuals' scale from their lower quartile, which what covers
    // up to two thirds of the region does not reach (a median gives way
    // at a half). Written as the median a clean region would have.
    let quarter = work.abs.len() / 4;
    let median = *work.abs.select_nth_unstable_by(quarter, |a, b| a.total_cmp(b)).1 * (0.6745 / 0.3186);
    work.median = median;
    // Never tighter than a quarter of the template's deviation, so noise
    // alone drops nothing; never wider than about one deviation, so what
    // is plainly something else is out from the first step, before the
    // alignment has settled and the spread says so.
    let cut = (4.685 * 1.4826 * median).clamp(0.25, 1.1);
    let inv_cut2 = 1.0 / (cut * cut);
    for i in 0..count {
        let e = work.res[i];
        work.wt[i] = if e.is_nan() {
            0.0
        } else {
            let r2 = e * e * inv_cut2;
            if r2 >= 1.0 { 0.0 } else { (1.0 - r2) * (1.0 - r2) }
        };
    }
}

/// Aligns `pyr` to the template from `init` (template frame → this
/// frame), coarse to fine. None when the region leaves the frame or the
/// frame is flat under it.
fn align(tpl: &Template, pyr: &Pyramid, init: H3, opts: &Options, close: f32) -> Option<Fit> {
    align_noting(tpl, pyr, init, opts, close, None)
}

/// `align`, also noting for each pixel of the finest level used where it
/// is (template coordinates) and whether it disagreed with the template.
fn align_noting(tpl: &Template, pyr: &Pyramid, init: H3, opts: &Options, close: f32, noted: Option<&mut Vec<(f32, f32, bool)>>) -> Option<Fit> {
    let (c, s) = (tpl.centre, tpl.scale);
    // Normalised coordinates → level-0 pixels, and back.
    let n_inv = H3([s, 0.0, c[0], 0.0, s, c[1], 0.0, 0.0, 1.0]);
    let n = H3([1.0 / s, 0.0, -c[0] / s, 0.0, 1.0 / s, -c[1] / s, 0.0, 0.0, 1.0]);
    let mut h = init;
    let mut work = Work::default();
    // A region now twice the size is compared one level down the
    // frame's pyramid (and one half the size, one level up the
    // template's): like against like, so a push in or a pull out does
    // not set a sharp picture against a soft one.
    let grown = (signed_area2(&init.quad(&tpl.quad)).abs() / signed_area2(&tpl.quad).abs().max(1e-9)).sqrt();
    if !grown.is_finite() || grown <= 0.0 {
        return None;
    }
    let off = grown.log2().round().clamp(-4.0, 4.0) as isize;
    // Template level `tl` against frame level `tl + off`.
    let lowest = (-off).max(0) as usize;
    let top = (tpl.levels.len() as isize).min(pyr.levels.len() as isize - off);
    if top <= lowest as isize {
        return None;
    }
    let top = top as usize;
    for tl in (lowest..top).rev() {
        let level = &tpl.levels[tl];
        let l = (tl as isize + off) as usize;
        let img = &pyr.levels[l];
        let count = level.x.len();
        let need = ((count as f64 * opts.min_visible) as usize).max(32);
        // The coarsest levels have few pixels: they place the region
        // (a shift, then an affine map) and leave perspective to the
        // finer ones.
        let free: usize = if top - lowest >= 3 && tl == top - 1 { 2 } else if tl >= lowest + 2 { 6 } else { 8 };
        let order = [4usize, 5, 0, 1, 2, 3, 6, 7];
        let iterations = if tl == lowest { 20 } else { 25 };
        work.wt.clear();
        work.wt.resize(count, 1.0);
        for it in 0..iterations {
            if sample_level(level, img, l, h, &mut work.vals) < need {
                return None;
            }
            residuals(level, &mut work, it == 0, close)?;
            let mut a = [[0.0f64; 8]; 8];
            let mut b = [0.0f64; 8];
            for i in 0..count {
                let w = work.wt[i] as f64;
                if w <= 0.0 {
                    continue;
                }
                let sd = &level.sd[i];
                let we = w * work.res[i] as f64;
                for r in 0..8 {
                    let sr = sd[r] as f64;
                    b[r] += sr * we;
                    let wr = w * sr;
                    for q in r..8 {
                        a[r][q] += wr * sd[q] as f64;
                    }
                }
            }
            for r in 0..8 {
                for q in 0..r {
                    a[r][q] = a[q][r];
                }
            }
            // Parameters this level leaves alone.
            for &k in &order[free..] {
                for q in 0..8 {
                    a[k][q] = 0.0;
                    a[q][k] = 0.0;
                }
                a[k][k] = 1.0;
                b[k] = 0.0;
            }
            for (r, row) in a.iter_mut().enumerate() {
                row[r] = row[r] * (1.0 + 1e-6) + 1e-9;
            }
            let Some(p) = solve8sym(&a, &b) else { break };
            let dw = H3([1.0 + p[0], p[2], p[4], p[1], 1.0 + p[3], p[5], p[6], p[7], 1.0]);
            let Some(dw_inv) = dw.inverse() else { break };
            let next = h.then(n_inv).then(dw_inv).then(n).normalised();
            if next.0.iter().any(|v| !v.is_finite()) {
                break;
            }
            // How far the corners moved, in level-0 pixels.
            let moved = tpl.quad.iter().map(|&q| {
                let (p0, p1) = (h.apply(q), next.apply(q));
                ((p0[0] - p1[0]).powi(2) + (p0[1] - p1[1]).powi(2)).sqrt()
            }).fold(0.0f64, f64::max);
            if !moved.is_finite() {
                break;
            }
            h = next;
            if moved < 0.002 * (1u32 << l) as f64 {
                break;
            }
        }
    }
    // Where it settled, at the finest pair of levels.
    let level = &tpl.levels[lowest];
    let l = (lowest as isize + off) as usize;
    let count = level.x.len();
    let valid = sample_level(level, &pyr.levels[l], l, h, &mut work.vals);
    if valid < ((count as f64 * opts.min_visible) as usize).max(32) {
        return None;
    }
    work.wt.clear();
    work.wt.resize(count, 1.0);
    let (score, agree) = residuals(level, &mut work, true, close)?;
    if let Some(noted) = noted {
        noted.clear();
        noted.extend((0..count).filter(|&i| !work.vals[i].is_nan()).map(|i| (level.x[i], level.y[i], work.wt[i] <= 0.0)));
    }
    Some(Fit { h, score, visible: valid as f64 / count as f64, agree, spread: work.median as f64 })
}

/// The shift (level-0 pixels) of the template's coarsest level that
/// matches the frame best, searched over a quarter of the frame each way.
fn search_shift(tpl: &Template, pyr: &Pyramid, h: H3) -> Option<[f64; 2]> {
    let l = tpl.levels.len().min(pyr.levels.len()).checked_sub(1)?;
    let level = &tpl.levels[l];
    let img = &pyr.levels[l];
    let k = (1u32 << l) as f64;
    // Where each template pixel lands at this level, to the pixel.
    let at: Vec<(i32, i32, f32)> = (0..level.x.len()).map(|i| {
        let p = h.apply([level.x[i] as f64, level.y[i] as f64]);
        (to_level(p[0], l).round() as i32, to_level(p[1], l).round() as i32, level.t[i])
    }).collect();
    let (rx, ry) = ((img.w / 4).max(2) as i32, (img.h / 4).max(2) as i32);
    let need = (at.len() / 2).max(24);
    let mut best: Option<(f64, i32, i32)> = None;
    for dy in -ry..=ry {
        for dx in -rx..=rx {
            let (mut n, mut si, mut sii, mut st, mut stt, mut sit) = (0usize, 0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64);
            for &(x, y, t) in &at {
                let (x, y) = (x + dx, y + dy);
                if x < 0 || y < 0 || x >= img.w as i32 || y >= img.h as i32 {
                    continue;
                }
                let v = img.at(x as usize, y as usize) as f64;
                let t = t as f64;
                n += 1;
                si += v;
                sii += v * v;
                st += t;
                stt += t * t;
                sit += v * t;
            }
            if n < need {
                continue;
            }
            let nf = n as f64;
            let (vi, vt) = (sii - si * si / nf, stt - st * st / nf);
            if vi <= 1e-6 || vt <= 1e-6 {
                continue;
            }
            let zncc = (sit - si * st / nf) / (vi * vt).sqrt();
            if best.is_none_or(|b| zncc > b.0) {
                best = Some((zncc, dx, dy));
            }
        }
    }
    best.filter(|b| b.0 > 0.5).map(|b| [b.1 as f64 * k, b.2 as f64 * k])
}

/// Cells across the region on which what covers it is kept.
const GRID: usize = 24;
/// Frames a covered cell stays so without word of it.
const COVER_FRAMES: u8 = 12;

/// Follows one marked region through frames given in order.
pub struct PlaneTracker {
    opts: Options,
    levels: usize,
    reference: Quad,
    sign: f64,
    area: f64,
    key: Template,
    /// Marked frame → the frame the template was taken from.
    key_h: H3,
    key_area: f64,
    /// Template frame → the last frame found, and the one before it.
    rel: H3,
    rel_before: Option<H3>,
    /// The last frame found, for a frame that does not align directly.
    last: Option<Pyramid>,
    /// Frames not found since the last one found.
    lost: usize,
    /// The usual size of a residual on the frames found so far.
    spread: Option<f64>,
    /// Frames running on which the template did not hold.
    weak: usize,
    /// The usual residual between one frame and the next.
    step_spread: Option<f64>,
    /// Marked frame → the unit square of the region.
    to_unit: H3,
    texture: f64,
    /// What covers the region, on a grid over it (marked frame's
    /// coordinates): frames a cell is still held to be covered.
    cover: Vec<u8>,
    /// Times the template was taken again.
    pub rekeys: usize,
}

impl PlaneTracker {
    /// `quad`: the region's four corners in `frame`, going round it.
    pub fn new(frame: &Gray, quad: Quad, opts: Options) -> Result<Self, String> {
        if quad.iter().flatten().any(|v| !v.is_finite()) {
            return Err("corners must be numbers".into());
        }
        let levels = {
            // Down to where the frame's short side is a few dozen pixels.
            let side = frame.w.min(frame.h).max(1);
            (((side as f64) / 40.0).log2().floor() as usize + 1).clamp(1, 6)
        };
        let pyr = Pyramid::new(frame, levels);
        let key = Template::new(&pyr, &quad, opts.max_points, &|_, _| true)?;
        let to_unit = H3::from_quads(&quad, &[[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]).ok_or("the four corners must go round the region in order")?;
        // The mean gradient of each cell of the region.
        let texture = {
            let img = &pyr.levels[0];
            let mut cells = vec![(0.0f64, 0u32); GRID * GRID];
            let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
            for p in &quad {
                x0 = x0.min(p[0]);
                y0 = y0.min(p[1]);
                x1 = x1.max(p[0]);
                y1 = y1.max(p[1]);
            }
            let (xa, ya) = (x0.ceil().max(1.0) as usize, y0.ceil().max(1.0) as usize);
            let (xb, yb) = ((x1.floor() as isize).min(img.w as isize - 2), (y1.floor() as isize).min(img.h as isize - 2));
            for y in ya as isize..=yb {
                for x in xa as isize..=xb {
                    let (x, y) = (x as usize, y as usize);
                    let u = to_unit.apply([x as f64, y as f64]);
                    let (cx, cy) = ((u[0] * GRID as f64).floor(), (u[1] * GRID as f64).floor());
                    if cx >= 0.0 && cy >= 0.0 && (cx as usize) < GRID && (cy as usize) < GRID {
                        let gx = (img.at(x + 1, y) - img.at(x - 1, y)) * 0.5;
                        let gy = (img.at(x, y + 1) - img.at(x, y - 1)) * 0.5;
                        let c = &mut cells[cy as usize * GRID + cx as usize];
                        c.0 += ((gx * gx + gy * gy) as f64).sqrt();
                        c.1 += 1;
                    }
                }
            }
            let seen = cells.iter().filter(|c| c.1 > 0).count().max(1);
            cells.iter().filter(|c| c.1 > 0 && c.0 / c.1 as f64 >= 1.5).count() as f64 / seen as f64
        };
        let area = signed_area2(&quad);
        Ok(PlaneTracker {
            opts,
            levels,
            reference: quad,
            sign: area.signum(),
            area: area.abs(),
            key,
            key_h: H3::IDENTITY,
            key_area: area.abs(),
            rel: H3::IDENTITY,
            rel_before: None,
            last: Some(pyr),
            lost: 0,
            spread: None,
            weak: 0,
            step_spread: None,
            to_unit,
            texture,
            cover: vec![0; GRID * GRID],
            rekeys: 0,
        })
    }

    /// This frame is known to show the region where it was marked (the
    /// camera has not moved since): the track goes on from there.
    pub fn hold(&mut self, frame: &Gray) -> Sample {
        if let Some(back) = self.key_h.inverse() {
            self.rel = back.normalised();
        }
        self.rel_before = None;
        self.lost = 0;
        self.weak = 0;
        self.last = Some(Pyramid::new(frame, self.levels));
        Sample { found: true, h: H3::IDENTITY, corners: self.reference, score: 1.0, visible: 1.0, agree: 1.0 }
    }

    /// How much of the region has something on it: the share of its
    /// cells (a 24 × 24 grid) with texture or edges in the marked frame.
    /// A blank table top with a few things standing on it is near 0.1; a
    /// stone worktop is 1.
    pub fn texture(&self) -> f64 {
        self.texture
    }

    fn sane(&self, h: H3) -> bool {
        let q = h.then(self.key_h).quad(&self.reference);
        if q.iter().flatten().any(|v| !v.is_finite()) || !convex(&q) {
            return false;
        }
        let a = signed_area2(&q);
        a.signum() == self.sign && a.abs() > self.area / 900.0 && a.abs() < self.area * 900.0
    }

    /// How close a pixel must be to count as matching: five times the
    /// usual residual of the frames found so far (clean footage is held
    /// to a tight match, which no other picture passes; noisy footage is
    /// given the room it needs).
    fn close(&self) -> f32 {
        self.spread.map_or(CLOSE, |s| (5.0 * s as f32).clamp(0.12, CLOSE))
    }

    fn good(&self, f: &Fit) -> bool {
        f.score >= self.opts.min_score && f.agree >= self.opts.min_agree && f.visible >= self.opts.min_visible && self.sane(f.h) && self.steady(f.h)
    }

    /// From one frame to the next a plane keeps its shape, near enough:
    /// with the shift taken out, no corner moves by more than a third of
    /// the region's size (more when frames were lost between). A fit to
    /// some other picture bends the region to suit it, and fails this.
    fn steady(&self, h: H3) -> bool {
        let (a, b) = (self.rel.quad(&self.key.quad), h.quad(&self.key.quad));
        let centre = |q: &Quad| [q.iter().map(|p| p[0]).sum::<f64>() / 4.0, q.iter().map(|p| p[1]).sum::<f64>() / 4.0];
        let (ca, cb) = (centre(&a), centre(&b));
        let size = a.iter().map(|p| ((p[0] - ca[0]).powi(2) + (p[1] - ca[1]).powi(2)).sqrt()).sum::<f64>() / 4.0;
        let bend = (0..4).map(|i| (((a[i][0] - ca[0]) - (b[i][0] - cb[0])).powi(2) + ((a[i][1] - ca[1]) - (b[i][1] - cb[1])).powi(2)).sqrt()).fold(0.0f64, f64::max);
        bend <= size * 0.35 * (1 + self.lost).min(3) as f64
    }

    /// The next frame (the one after, or before, the last one given).
    ///
    /// Two things say where the plane is. The frame before, which looks
    /// almost the same as this one whatever has changed since the frame
    /// was marked (the view, the focus, a sheen on the surface): it gives
    /// the step, and steps add up to a slow drift. And the template,
    /// which does not drift but stops comparing as the look changes, and
    /// then wanders. So the step is taken from the frame before, the
    /// template corrects it while it still matches closely, and when it
    /// no longer does the template is taken again from this frame, early,
    /// while what the steps have added is still small.
    pub fn step(&mut self, frame: &Gray) -> Sample {
        let pyr = Pyramid::new(frame, self.levels);
        let close = self.close();
        let debug = std::env::var_os("LAHZA_TRACK_DEBUG").is_some();
        // Marked frame → the frame before.
        let total_before = self.rel.then(self.key_h).normalised();
        let mut step_noted: Vec<(f32, f32, bool)> = Vec::new();
        let mut key_noted: Vec<(f32, f32, bool)> = Vec::new();
        // 1. From the frame before (not after frames were lost: that
        // picture is then no neighbour of this one).
        let mut chain: Option<Fit> = None;
        if self.lost == 0 {
            if let Some(last) = &self.last {
                let quad = self.rel.quad(&self.key.quad);
                // Left out of the step: what covered the region on the
                // frame before (it moves by itself, not with the plane).
                let to_cell = total_before.inverse().map(|inv| self.to_unit.then(inv));
                let allow = |x: f64, y: f64| to_cell.is_none_or(|m| !self.covered(m.apply([x, y])));
                if let Ok(tpl) = Template::new(last, &quad, self.opts.max_points, &allow) {
                    // Neighbours match closely or not at all.
                    let fine = |s: &Fit| s.score >= 0.9 && s.agree >= self.opts.min_agree;
                    let mut step = align_noting(&tpl, &pyr, H3::IDENTITY, &self.opts, close, Some(&mut step_noted));
                    if !step.is_some_and(|s| fine(&s)) {
                        if let Some(d) = search_shift(&tpl, &pyr, H3::IDENTITY) {
                            let mut noted = Vec::new();
                            let moved = align_noting(&tpl, &pyr, H3([1.0, 0.0, d[0], 0.0, 1.0, d[1], 0.0, 0.0, 1.0]), &self.opts, close, Some(&mut noted));
                            if moved.is_some_and(|m| step.is_none_or(|s| m.score > s.score)) {
                                step = moved;
                                step_noted = noted;
                            }
                        }
                    }
                    chain = step.filter(fine).map(|s| Fit { h: s.h.then(self.rel).normalised(), ..s }).filter(|c| c.visible >= self.opts.min_visible && self.sane(c.h) && self.steady(c.h));
                }
            }
        }
        // 2. Against the template, from where the step puts it; without
        // a step, from where the motion so far would put it, from where
        // it was, and by a search of shifts.
        let mut key: Option<Fit> = None;
        let better = |cand: Option<Fit>, fit: &mut Option<Fit>| {
            if let Some(c) = cand {
                if fit.is_none_or(|f| c.score > f.score) {
                    *fit = Some(c);
                }
            }
        };
        if let Some(c) = chain {
            key = align_noting(&self.key, &pyr, c.h, &self.opts, close, Some(&mut key_noted));
        } else {
            if let Some(before) = self.rel_before.and_then(|b| b.inverse()) {
                let predicted = self.rel.then(before).then(self.rel).normalised();
                if predicted.0.iter().all(|v| v.is_finite()) {
                    better(align(&self.key, &pyr, predicted, &self.opts, close), &mut key);
                }
            }
            if !key.is_some_and(|f| self.good(&f)) {
                better(align(&self.key, &pyr, self.rel, &self.opts, close), &mut key);
            }
            if !key.is_some_and(|f| self.good(&f)) {
                if let Some(d) = search_shift(&self.key, &pyr, self.rel) {
                    let init = H3([1.0, 0.0, d[0], 0.0, 1.0, d[1], 0.0, 0.0, 1.0]).then(self.rel);
                    better(align(&self.key, &pyr, init, &self.opts, close), &mut key);
                }
            }
        }
        // 3. Which to take. The template holds while its residuals are
        // about what they were when it was taken (a covered part of the
        // region does not raise them: they are measured at the middle).
        let holds = |k: &Fit| self.good(k) && k.spread <= self.anchor_spread();
        let key_good = key.filter(|k| self.good(k));
        let (taken, by_template) = match (key_good, chain) {
            (Some(k), _) if holds(&k) => (Some(k), true),
            (_, Some(c)) => (Some(c), false),
            (Some(k), None) => (Some(k), true),
            (None, None) => (None, false),
        };
        if debug {
            let show = |f: Option<Fit>| f.map_or("none".to_string(), |f| format!("score {:.3} agree {:.2} spread {:.3}", f.score, f.agree, f.spread));
            eprintln!("track: template [{}] step [{}] holds under {:.3}: {}", show(key), show(chain), self.anchor_spread(), if taken.is_none() { "lost" } else if by_template { "template" } else { "step" });
        }
        let Some(f) = taken else {
            self.lost += 1;
            self.weak = 0;
            let h = self.rel.then(self.key_h).normalised();
            let (score, visible, agree) = key.map(|f| (f.score, f.visible, f.agree)).unwrap_or((0.0, 0.0, 0.0));
            return Sample { found: false, h, corners: h.quad(&self.reference), score: score.max(0.0), visible, agree };
        };
        // What covers the region now: where the template's own pixels
        // disagree (when it holds, it knows the plane's look), else where
        // the frame before disagrees with this one.
        if by_template && !key_noted.is_empty() {
            let m = self.key_h.inverse().map(|inv| self.to_unit.then(inv));
            self.note_cover(&key_noted, m);
        } else if !step_noted.is_empty() {
            let m = total_before.inverse().map(|inv| self.to_unit.then(inv));
            self.note_cover(&step_noted, m);
        }
        if by_template {
            self.weak = 0;
            self.spread = Some(self.spread.map_or(f.spread, |s| 0.8 * s + 0.2 * f.spread));
        } else {
            self.weak += 1;
        }
        if let Some(c) = chain {
            self.step_spread = Some(self.step_spread.map_or(c.spread, |s| 0.8 * s + 0.2 * c.spread));
        }
        self.rel_before = (self.lost == 0).then_some(self.rel);
        self.lost = 0;
        self.rel = f.h;
        let h = f.h.then(self.key_h).normalised();
        let corners = h.quad(&self.reference);
        if self.opts.rekey {
            // Past about three times the size, or a third of it; or the
            // template has not held for two frames running.
            let ratio = signed_area2(&corners).abs() / self.key_area;
            let resized = by_template && !(0.1..=10.0).contains(&ratio) && f.agree >= 0.7;
            let stale = !by_template && self.weak >= 2 && f.agree >= 0.8;
            if resized || stale {
                // What covers the region stays out of the new template.
                let to_cell = h.inverse().map(|inv| self.to_unit.then(inv));
                let allow = |x: f64, y: f64| to_cell.is_none_or(|m| !self.covered(m.apply([x, y])));
                if let Ok(tpl) = Template::new(&pyr, &corners, self.opts.max_points, &allow) {
                    self.key = tpl;
                    self.key_h = h;
                    self.key_area = signed_area2(&corners).abs();
                    self.rel = H3::IDENTITY;
                    self.rel_before = None;
                    self.weak = 0;
                    self.rekeys += 1;
                }
            }
        }
        self.last = Some(pyr);
        Sample { found: true, h, corners, score: f.score.clamp(0.0, 1.0), visible: f.visible, agree: f.agree }
    }

    /// Whether a point of the region (unit square) is covered: its cell
    /// or one next to it.
    fn covered(&self, p: [f64; 2]) -> bool {
        if !(p[0].is_finite() && p[1].is_finite()) {
            return false;
        }
        let (cx, cy) = ((p[0] * GRID as f64).floor() as isize, (p[1] * GRID as f64).floor() as isize);
        for dy in -1..=1 {
            for dx in -1..=1 {
                let (x, y) = (cx + dx, cy + dy);
                if x >= 0 && y >= 0 && (x as usize) < GRID && (y as usize) < GRID && self.cover[y as usize * GRID + x as usize] > 0 {
                    return true;
                }
            }
        }
        false
    }

    /// Takes an alignment's word on which pixels disagreed (`to_cell`:
    /// their coordinates → the region's unit square). A cell most of
    /// whose pixels disagree is covered; one most of whose pixels agree
    /// is clear; one with no word keeps its state a while, then clears
    /// (if it is still covered, the next word says so).
    fn note_cover(&mut self, noted: &[(f32, f32, bool)], to_cell: Option<H3>) {
        let Some(m) = to_cell else { return };
        let mut counts = vec![(0u32, 0u32); GRID * GRID];
        for &(x, y, out) in noted {
            let p = m.apply([x as f64, y as f64]);
            let (cx, cy) = ((p[0] * GRID as f64).floor(), (p[1] * GRID as f64).floor());
            if cx >= 0.0 && cy >= 0.0 && (cx as usize) < GRID && (cy as usize) < GRID {
                let c = &mut counts[cy as usize * GRID + cx as usize];
                if out { c.1 += 1 } else { c.0 += 1 }
            }
        }
        for (cell, &(agree, out)) in self.cover.iter_mut().zip(&counts) {
            if out >= 3 && out * 2 > (agree + out) {
                *cell = COVER_FRAMES;
            } else if agree + out >= 3 {
                *cell = 0;
            } else {
                *cell = cell.saturating_sub(1);
            }
        }
    }

    /// Residuals (at their middle) up to which the template is believed:
    /// twice what neighbouring frames leave between them (the footage's
    /// own noise), and between 0.12 and 0.3 of the template's deviation.
    /// On a hand-held shot of a worktop the template's own answer was
    /// within 0.2 px while its residuals were under 0.15, and 0.8 px out
    /// at 0.3.
    fn anchor_spread(&self) -> f64 {
        (2.0 * self.step_spread.unwrap_or(0.1)).clamp(0.12, 0.3)
    }
}
