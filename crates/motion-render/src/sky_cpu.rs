//! Sky visibility on the CPU (`RenderOptions::sky`): what pt.wgsl's `sky`
//! and `sky_filter` find with the GPU's ray queries, found here with a BVH
//! over the list's triangles in camera space, so the CPU's frame has the
//! lighting the GPU's has. The same rays from the same points toward the
//! same fixed set of directions, the same light back from what they hit
//! (`sky_lit`), the same two filter passes, in the same f32 arithmetic;
//! `at` is motion.wgsl's `sky_at`. The two differ where a ray grazes an
//! edge (two ways of meeting a triangle).

use crate::mesh3d::{LROWS, MAT_ROWS};
use crate::pt::{Trace, TraceMesh};

type V3 = [f32; 3];

fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn mul(a: V3, k: f32) -> V3 {
    [a[0] * k, a[1] * k, a[2] * k]
}
fn dot(a: V3, b: V3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: V3, b: V3) -> V3 {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn norm(a: V3) -> V3 {
    let l = dot(a, a).sqrt();
    if l > 0.0 {
        mul(a, 1.0 / l)
    } else {
        a
    }
}

/// pt.wgsl `basis`: columns (tangent, bitangent, n).
fn basis(n: V3) -> [V3; 3] {
    let t = if n[0].abs() > 0.9 { [0.0, 1.0, 0.0] } else { [1.0, 0.0, 0.0] };
    let b = norm(cross(n, t));
    [cross(b, n), b, n]
}

pub(crate) fn oct_encode(n: V3) -> [f32; 2] {
    let s = n[0].abs() + n[1].abs() + n[2].abs();
    let o = [n[0] / s, n[1] / s];
    if n[2] >= 0.0 {
        return o;
    }
    let sign = |v: f32| if v >= 0.0 { 1.0 } else { -1.0 };
    [(1.0 - o[1].abs()) * sign(o[0]), (1.0 - o[0].abs()) * sign(o[1])]
}

pub(crate) fn oct_decode(e: [f32; 2]) -> V3 {
    let z = 1.0 - e[0].abs() - e[1].abs();
    if z >= 0.0 {
        return norm([e[0], e[1], z]);
    }
    let sign = |v: f32| if v >= 0.0 { 1.0 } else { -1.0 };
    norm([(1.0 - e[1].abs()) * sign(e[0]), (1.0 - e[0].abs()) * sign(e[1]), z])
}

const FLAG_FACETED: u32 = 2;
/// Shadow catchers, wires and volumes: not in the sky's rays.
const NOT_SOLID: u32 = 8 | 32 | 64;

/// What the rays need of a material.
#[derive(Clone, Copy)]
struct Mat {
    flags: u32,
    /// Blocks a shadow ray (at least half opaque, and none of `NOT_SOLID`).
    blocks: bool,
    /// Base colour less what is metal, and the emission.
    kd: V3,
    emit: V3,
}

/// A triangle in camera space with its corners' normals.
struct Tri {
    p: [V3; 3],
    n: [V3; 3],
    mat: u32,
}

struct Node {
    lo: V3,
    hi: V3,
    /// A leaf: its first triangle and how many; else its second child
    /// (the first follows it) and 0.
    at: u32,
    count: u32,
}

struct Scene {
    tris: Vec<Tri>,
    mats: Vec<Mat>,
    nodes: Vec<Node>,
}

struct Hit {
    t: f32,
    p: V3,
    n: V3,
    g: V3,
    mat: Mat,
}

impl Scene {
    fn new(t: &Trace) -> Scene {
        let (mut tris, mut mats) = (Vec::new(), Vec::new());
        for inst in &t.instances {
            // (A mesh the GPU makes in its pool is not here to cast at.)
            let TraceMesh::Cpu(mesh) = &inst.mesh else { continue };
            let mat_base = mats.len() as u32;
            for m in inst.materials.chunks_exact(MAT_ROWS) {
                let flags = m[2][3] as u32;
                mats.push(Mat {
                    flags,
                    blocks: flags & NOT_SOLID == 0 && m[0][3] * (1.0 - m[3][0]) >= 0.5,
                    kd: mul([m[0][0], m[0][1], m[0][2]], 1.0 - m[1][0]),
                    emit: [m[2][0], m[2][1], m[2][2]],
                });
            }
            let r = inst.transform;
            let rows = [[r[0][0], r[0][1], r[0][2]], [r[1][0], r[1][1], r[1][2]], [r[2][0], r[2][1], r[2][2]]];
            let place = |p: [f32; 3]| [dot(rows[0], p) + r[0][3], dot(rows[1], p) + r[1][3], dot(rows[2], p) + r[2][3]];
            // Normals go through the inverse transpose (its cofactors).
            let co = [cross(rows[1], rows[2]), cross(rows[2], rows[0]), cross(rows[0], rows[1])];
            let turn = |n: [f32; 3]| [dot(co[0], n), dot(co[1], n), dot(co[2], n)];
            let p: Vec<V3> = mesh.positions.iter().map(|p| place(*p)).collect();
            let n: Vec<V3> = mesh.normals.iter().map(|n| turn(*n)).collect();
            for (k, tri) in mesh.tris.iter().enumerate() {
                let tag = mesh.tags.get(k).copied().unwrap_or(0);
                let (part, seg) = ((tag & 0xff) as usize, (tag >> 8) & 0xffff);
                let Some((first, count)) = inst.parts.get(part).copied() else { continue };
                let mat = mat_base + first + seg % count.max(1);
                let [a, b, c] = tri.map(|i| i as usize);
                if mat as usize >= mats.len() || a.max(b).max(c) >= p.len().min(n.len()) {
                    continue;
                }
                tris.push(Tri { p: [p[a], p[b], p[c]], n: [n[a], n[b], n[c]], mat });
            }
        }
        let mut scene = Scene { tris, mats, nodes: Vec::new() };
        let n = scene.tris.len();
        if n > 0 {
            let mut centres: Vec<V3> = scene.tris.iter().map(|t| mul(add(add(t.p[0], t.p[1]), t.p[2]), 1.0 / 3.0)).collect();
            scene.nodes.reserve(n / 2 + 1);
            scene.split(&mut centres, 0, n);
        }
        scene
    }

    /// The node over triangles `from..to` (sorted in place, with their
    /// centres): halved at the median along its centres' longest side
    /// until four or fewer are left.
    fn split(&mut self, centres: &mut [V3], from: usize, to: usize) -> u32 {
        let (mut lo, mut hi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
        let (mut clo, mut chi) = (lo, hi);
        for k in from..to {
            for p in self.tris[k].p {
                for a in 0..3 {
                    lo[a] = lo[a].min(p[a]);
                    hi[a] = hi[a].max(p[a]);
                }
            }
            for a in 0..3 {
                clo[a] = clo[a].min(centres[k][a]);
                chi[a] = chi[a].max(centres[k][a]);
            }
        }
        let me = self.nodes.len();
        self.nodes.push(Node { lo, hi, at: from as u32, count: (to - from) as u32 });
        if to - from <= 4 {
            return me as u32;
        }
        let side = sub(chi, clo);
        let axis = if side[0] >= side[1] && side[0] >= side[2] { 0 } else if side[1] >= side[2] { 1 } else { 2 };
        let mid = (from + to) / 2;
        // (Triangles and centres together: sort an order, then apply it.)
        let mut order: Vec<usize> = (from..to).collect();
        order.select_nth_unstable_by(mid - from, |a, b| centres[*a][axis].total_cmp(&centres[*b][axis]));
        let mut seen = vec![false; to - from];
        for start in 0..order.len() {
            if seen[start] {
                continue;
            }
            // Follow the cycle that brings each to its place.
            let mut k = start;
            while !seen[k] {
                seen[k] = true;
                let next = order[k] - from;
                if !seen[next] {
                    self.tris.swap(from + k, from + next);
                    centres.swap(from + k, from + next);
                }
                k = next;
            }
        }
        self.split(centres, from, mid);
        let second = self.split(centres, mid, to);
        self.nodes[me].at = second;
        self.nodes[me].count = 0;
        me as u32
    }

    /// The nearest triangle along `o + t d`, 0 < t < `tmax` (`any`: the
    /// first found that blocks a shadow ray): t, the triangle and where
    /// in it (the weights of its second and third corners).
    fn cast(&self, o: V3, d: V3, tmax: f32, any: bool) -> Option<(f32, usize, [f32; 2])> {
        if self.nodes.is_empty() {
            return None;
        }
        let inv = [1.0 / d[0], 1.0 / d[1], 1.0 / d[2]];
        let mut best: Option<(f32, usize, [f32; 2])> = None;
        let mut far = tmax;
        let mut stack = [0u32; 64];
        let mut top = 1;
        while top > 0 {
            top -= 1;
            let index = stack[top];
            let node = &self.nodes[index as usize];
            let (mut t0, mut t1) = (0.0f32, far);
            for a in 0..3 {
                let (x, y) = ((node.lo[a] - o[a]) * inv[a], (node.hi[a] - o[a]) * inv[a]);
                t0 = t0.max(x.min(y));
                t1 = t1.min(x.max(y));
            }
            // (A NaN, from a ray along a box's face, passes: looked into.)
            if t0 > t1 {
                continue;
            }
            if node.count == 0 {
                // (Its first child follows it. Halving keeps the tree
                // far shallower than the stack.)
                if top + 2 <= stack.len() {
                    stack[top] = node.at;
                    stack[top + 1] = index + 1;
                    top += 2;
                }
                continue;
            }
            for k in node.at as usize..(node.at + node.count) as usize {
                let tri = &self.tris[k];
                if any && !self.mats[tri.mat as usize].blocks {
                    continue;
                }
                let (e1, e2) = (sub(tri.p[1], tri.p[0]), sub(tri.p[2], tri.p[0]));
                let pv = cross(d, e2);
                let det = dot(e1, pv);
                if det.abs() < 1e-20 {
                    continue;
                }
                let tv = sub(o, tri.p[0]);
                let u = dot(tv, pv) / det;
                if !(0.0..=1.0).contains(&u) {
                    continue;
                }
                let qv = cross(tv, e1);
                let v = dot(d, qv) / det;
                if v < 0.0 || u + v > 1.0 {
                    continue;
                }
                let t = dot(e2, qv) / det;
                if t > 0.0 && t < far {
                    far = t;
                    best = Some((t, k, [u, v]));
                    if any {
                        return best;
                    }
                }
            }
        }
        best
    }

    /// pt.wgsl `cast_ray`, for meshes: the nearest surface and its normals
    /// facing the ray.
    fn hit(&self, o: V3, d: V3) -> Option<Hit> {
        let (t, k, uv) = self.cast(o, d, 1e30, false)?;
        let tri = &self.tris[k];
        let mat = self.mats[tri.mat as usize];
        let w = [1.0 - uv[0] - uv[1], uv[0], uv[1]];
        let mut g = norm(cross(sub(tri.p[1], tri.p[0]), sub(tri.p[2], tri.p[0])));
        let mut n = norm(add(add(mul(tri.n[0], w[0]), mul(tri.n[1], w[1])), mul(tri.n[2], w[2])));
        if mat.flags & FLAG_FACETED != 0 {
            n = g;
        }
        if dot(g, d) > 0.0 {
            g = mul(g, -1.0);
        }
        if dot(n, g) < 0.0 {
            n = mul(n, -1.0);
        }
        Some(Hit { t, p: add(o, mul(d, t)), n, g, mat })
    }
}

/// pt.wgsl `offset`.
fn offset(h: &Hit, toward: V3) -> V3 {
    let eps = 0.02 + h.t * 1e-5;
    if dot(toward, h.g) >= 0.0 {
        add(h.p, mul(h.g, eps))
    } else {
        sub(h.p, mul(h.g, eps))
    }
}

/// pt.wgsl `sky_lit`: the light a surface the sky's ray ends on sends
/// back.
fn sky_lit(scene: &Scene, t: &Trace, h: &Hit) -> V3 {
    let lights = &t.lights[..];
    let row = |i: usize| lights.get(i).copied().unwrap_or([0.0; 4]);
    let xyz = |r: [f32; 4]| [r[0], r[1], r[2]];
    let world = [dot(xyz(row(1)), h.n), dot(xyz(row(2)), h.n), dot(xyz(row(3)), h.n)];
    // The sky, horizon, ground and lobes as the raster's own shading
    // takes them, or an environment image's diffuse level.
    let mut l = if lights.len() <= 6 {
        [0.0; 3]
    } else if lights[0][2] > 0.5 {
        match t.env.as_ref().map(|e| (&e.raster, e.raster.first().copied().unwrap_or([0.0; 4]))) {
            Some((rows, d)) if d[2] >= 1.0 && d[3] >= 1.0 && d[1] as usize + d[2] as usize * d[3] as usize <= rows.len() => mul(crate::mesh3d::env_tex(rows, d[1] as usize, d[2] as usize, d[3] as usize, crate::mesh3d::env_uv(lights, 0, world)), lights[0][3]),
            _ => [0.0; 3],
        }
    } else {
        crate::mesh3d::env_diffuse(lights, 0, world)
    };
    for i in 0..row(0)[0] as usize {
        let (r, col, extra) = (row(7 + i * LROWS), row(7 + i * LROWS + 1), row(7 + i * LROWS + 2));
        let kind = r[3] as u32;
        if kind == 2 {
            l = add(l, xyz(col));
            continue;
        }
        let (mut to, mut dist, mut window) = (norm(xyz(r)), 1e30f32, 1.0f32);
        if kind != 0 {
            let away = sub(xyz(r), h.p);
            dist = dot(away, away).sqrt();
            to = mul(away, 1.0 / dist.max(1e-6));
            if extra[0] > 0.0 {
                let x = dist / extra[0];
                let w = (1.0 - x * x * x * x).clamp(0.0, 1.0);
                window = w * w;
            }
        }
        let facing = dot(h.n, to);
        if facing <= 0.0 {
            continue;
        }
        if col[3] > 0.5 && scene.cast(offset(h, to), to, dist * 0.999, true).is_some() {
            continue;
        }
        l = add(l, mul(xyz(col), facing * window));
    }
    add([h.mat.kd[0] * l[0], h.mat.kd[1] * l[1], h.mat.kd[2] * l[2]], h.mat.emit)
}

const SKY_ORDER: [u32; 16] = [0, 8, 2, 10, 12, 4, 14, 6, 3, 11, 1, 9, 15, 7, 13, 5];

/// What `sky` leaves of a texel before the filters.
#[derive(Clone, Copy, Default)]
struct Raw {
    /// The open rays' directions summed, over the rays; the camera depth.
    open: [f32; 4],
    /// The true normal (two numbers), its plane, the open part.
    plane: [f32; 4],
    back: V3,
    /// For the light that comes back a second time (pt.wgsl
    /// `sky_second`): the mean base colour of what the rays ended on, and
    /// for each ray the texel its end is seen in (and one; 0: none) with
    /// the end's depth.
    kd: V3,
    ends: [(u32, f32); 4],
}

/// Fills traced list `t`'s sky group, `w` × `h` texels of visibility, as
/// many after them of the light that comes back and as many of the
/// surfaces' planes, at `out`.
pub(crate) fn fill(t: &Trace, w: usize, h: usize, out: &mut [[f32; 4]], threads: usize) {
    if out.len() < w * h * 3 || w == 0 || h == 0 {
        return;
    }
    // LAHZA_SKY_TIMES: where the time goes (the structure, the rays, the
    // two filter passes), on stderr.
    let times = std::env::var_os("LAHZA_SKY_TIMES").is_some();
    let mut clock = std::time::Instant::now();
    let mut lap = |what: &str| {
        if times {
            eprintln!("sky visibility, {w} x {h}: {what} {:.1} ms", clock.elapsed().as_secs_f64() * 1000.0);
        }
        clock = std::time::Instant::now();
    };
    let scene = Scene::new(t);
    lap(&format!("a BVH over {} triangles", scene.tris.len()));
    let inv = t.inv_root;
    let dir = |x: f32, y: f32| {
        let tp = [t.origin[0] + t.factor[0] * (x + 0.5), t.origin[1] + t.factor[1] * (y + 0.5), 1.0];
        let q = [dot([inv[0], inv[1], inv[2]], tp), dot([inv[3], inv[4], inv[5]], tp), dot([inv[6], inv[7], inv[8]], tp)];
        norm([(q[0] / q[2] - t.centre[0]) / t.focal, (q[1] / q[2] - t.centre[1]) / t.focal, 1.0])
    };
    let rays = t.options.spp.max(1);
    // pt.wgsl `sky_texel`.
    let root = t.root;
    let seen_in = |p: V3| -> u32 {
        if p[2] <= 1e-3 {
            return 0;
        }
        let c = [t.focal * p[0] / p[2] + t.centre[0], t.focal * p[1] / p[2] + t.centre[1], 1.0];
        let tq = [dot([root[0], root[1], root[2]], c), dot([root[3], root[4], root[5]], c), dot([root[6], root[7], root[8]], c)];
        let (x, y) = (((tq[0] / tq[2] - t.origin[0]) / t.factor[0]).floor(), ((tq[1] / tq[2] - t.origin[1]) / t.factor[1]).floor());
        if x < 0.0 || y < 0.0 || x >= w as f32 || y >= h as f32 {
            return 0;
        }
        y as u32 * w as u32 + x as u32 + 1
    };
    let texel = |x: usize, y: usize| -> Raw {
        let d = dir(x as f32, y as f32);
        let mut raw = Raw::default();
        let Some(hit) = scene.hit([0.0; 3], d) else { return raw };
        if hit.mat.flags & NOT_SOLID != 0 {
            return raw;
        }
        let total = (rays * 16) as f32;
        let place = SKY_ORDER[(y % 4) * 4 + x % 4];
        let frame = basis(hit.n);
        let o = offset(&hit, hit.g);
        let (mut sum, mut n, mut back) = ([0.0f32; 3], 0.0f32, [0.0f32; 3]);
        let (mut kd, mut hits) = ([0.0f32; 3], 0.0f32);
        for k in 0..rays {
            let j = (k * 16 + place) as f32;
            let u1 = (j + 0.5) / total;
            let phi = 2.0 * std::f32::consts::PI * (j * 0.618_034 + 0.25).fract();
            let r = u1.sqrt();
            let local = [r * phi.cos(), r * phi.sin(), (1.0 - u1).sqrt()];
            let ray = norm(add(add(mul(frame[0], local[0]), mul(frame[1], local[1])), mul(frame[2], local[2])));
            if dot(ray, hit.g) <= 0.02 {
                continue;
            }
            match scene.hit(o, ray).filter(|far| far.mat.flags & NOT_SOLID == 0) {
                None => {
                    sum = add(sum, ray);
                    n += 1.0;
                }
                Some(far) => {
                    back = add(back, sky_lit(&scene, t, &far));
                    kd = add(kd, far.mat.kd);
                    hits += 1.0;
                    if k < 4 {
                        raw.ends[k as usize] = (seen_in(far.p), far.p[2]);
                    }
                }
            }
        }
        let e = oct_encode(hit.g);
        raw.open = [sum[0] / rays as f32, sum[1] / rays as f32, sum[2] / rays as f32, d[2] * hit.t];
        raw.plane = [e[0], e[1], dot(hit.g, hit.p), n / rays as f32];
        raw.back = mul(back, 1.0 / rays as f32);
        if hits > 0.0 {
            raw.kd = mul(kd, 1.0 / hits);
        }
        raw
    };
    let rows = |f: &(dyn Fn(usize, &mut [Raw]) + Sync), into: &mut [Raw]| {
        let per = h.div_ceil(threads.max(1)).max(1);
        std::thread::scope(|s| {
            for (k, chunk) in into.chunks_mut(per * w).enumerate() {
                s.spawn(move || {
                    for (r, row) in chunk.chunks_mut(w).enumerate() {
                        f(k * per + r, row);
                    }
                });
            }
        });
    };
    let mut raw = vec![Raw::default(); w * h];
    rows(
        &|y, row| {
            for (x, v) in row.iter_mut().enumerate() {
                *v = texel(x, y);
            }
        },
        &mut raw,
    );
    lap("the rays");
    // (Each texel's ray and true normal once, not once a tap: the same
    // numbers the filter would work out.)
    let dirs: Vec<V3> = (0..w * h).map(|i| dir((i % w) as f32, (i / w) as f32)).collect();
    let normals: Vec<V3> = raw.iter().map(|r| if r.open[3] > 0.0 { oct_decode([r.plane[0], r.plane[1]]) } else { [0.0; 3] }).collect();
    // pt.wgsl `sky_second`, once a texel.
    let second: Vec<V3> = raw
        .iter()
        .map(|r| {
            let mut sum = [0.0f32; 3];
            for (at, deep) in r.ends {
                if at == 0 {
                    continue;
                }
                let there = &raw[at as usize - 1];
                if (there.open[3] - deep).abs() > 0.02 * deep + 0.5 {
                    continue;
                }
                sum = add(sum, there.back);
            }
            mul([r.kd[0] * sum[0], r.kd[1] * sum[1], r.kd[2] * sum[2]], 1.0 / rays as f32)
        })
        .collect();
    // pt.wgsl `sky_filter`: the tile's sixteen places as one value (step
    // 0), then smoothed two texels apart.
    let filter = |from: &[Raw], values: Option<&[Raw]>, step: i32, x: usize, y: usize| -> Raw {
        let c = from[y * w + x];
        if c.open[3] <= 0.0 {
            return Raw::default();
        }
        let n0 = normals[y * w + x];
        let (mut sums, mut backs, mut weights) = ([[0.0f32; 4]; 16], [[0.0f32; 3]; 16], [0.0f32; 16]);
        for dy in -2i32..=2 {
            for dx in -2i32..=2 {
                let (px, py) = (x as i32 + dx * step.max(1), y as i32 + dy * step.max(1));
                if px < 0 || py < 0 || px >= w as i32 || py >= h as i32 {
                    continue;
                }
                let j = py as usize * w + px as usize;
                let cj = from[j];
                if cj.open[3] <= 0.0 {
                    continue;
                }
                let dj = dirs[j];
                let pj = mul(dj, cj.open[3] / dj[2]);
                if (dot(n0, pj) - c.plane[2]).abs() > 0.01 * c.open[3] + 0.5 || dot(n0, normals[j]) < 0.9 {
                    continue;
                }
                let mut wt = 1.0;
                if dx.abs() == 2 {
                    wt *= 0.5;
                }
                if dy.abs() == 2 {
                    wt *= 0.5;
                }
                let (place, value, back) = match values {
                    Some(v) => (0, v[j].open, v[j].back),
                    None => ((py as usize % 4) * 4 + px as usize % 4, [cj.open[0], cj.open[1], cj.open[2], cj.plane[3]], add(cj.back, second[j])),
                };
                for a in 0..4 {
                    sums[place][a] += value[a] * wt;
                }
                backs[place] = add(backs[place], mul(back, wt));
                weights[place] += wt;
            }
        }
        let (mut sum, mut back, mut weight) = ([0.0f32; 4], [0.0f32; 3], 0.0f32);
        for k in 0..16 {
            if weights[k] > 0.0 {
                for a in 0..4 {
                    sum[a] += sums[k][a] / weights[k];
                }
                back = add(back, mul(backs[k], 1.0 / weights[k]));
                weight += 1.0;
            }
        }
        let weight = weight.max(1e-6);
        // (As the filter's passes hand on: the summed direction with the
        // open part, and the light back.)
        Raw { open: [sum[0] / weight, sum[1] / weight, sum[2] / weight, sum[3] / weight], plane: c.plane, back: mul(back, 1.0 / weight), ..Raw::default() }
    };
    let mut once = vec![Raw::default(); w * h];
    rows(
        &|y, row| {
            for (x, v) in row.iter_mut().enumerate() {
                *v = filter(&raw, None, 0, x, y);
            }
        },
        &mut once,
    );
    let mut twice = vec![Raw::default(); w * h];
    rows(
        &|y, row| {
            for (x, v) in row.iter_mut().enumerate() {
                *v = filter(&raw, Some(&once), 2, x, y);
            }
        },
        &mut twice,
    );
    lap("the filter's two passes");
    for i in 0..w * h {
        let (c, v) = (raw[i], twice[i]);
        if c.open[3] <= 0.0 {
            out[i] = [0.0; 4];
            out[w * h + i] = [0.0; 4];
            out[2 * w * h + i] = [0.0; 4];
            continue;
        }
        let m = [v.open[0], v.open[1], v.open[2]];
        let toward = if dot(m, m) > 1e-8 { norm(m) } else { oct_decode([c.plane[0], c.plane[1]]) };
        let e = oct_encode(toward);
        out[i] = [e[0], e[1], v.open[3], c.open[3]];
        out[w * h + i] = [v.back[0], v.back[1], v.back[2], 0.0];
        out[2 * w * h + i] = [c.plane[0], c.plane[1], c.plane[2], 0.0];
    }
}

/// motion.wgsl `sky_at`: of the sky group at `off` (`w` × `h` texels, a
/// texel two target px each way), what the surface at canvas point `c`,
/// camera point `p` on a triangle whose true normal toward the camera is
/// `gn`, sees of the sky: the open direction (camera space), the open
/// part, and the light that comes back. None: no surface there.
pub(crate) fn at(groups: &[[f32; 4]], off: usize, w: i32, h: i32, c: [f32; 2], p: V3, gn: V3) -> Option<(V3, f32, V3)> {
    let g = [c[0] * 0.5 - 0.5, c[1] * 0.5 - 0.5];
    let g0 = [(g[0] + 0.5).floor(), (g[1] + 0.5).floor()];
    let texel = |base: usize, x: i32, y: i32| groups.get(base + (y.clamp(0, h - 1) * w + x.clamp(0, w - 1)) as usize).copied().unwrap_or([0.0; 4]);
    let (mut sum, mut back, mut weight) = ([0.0f32; 4], [0.0f32; 3], 0.0f32);
    let (mut near, mut near_back, mut nearest) = ([0.0f32; 4], [0.0f32; 3], 1e30f32);
    for k in 0..9 {
        let a = [g0[0] + (k % 3 - 1) as f32, g0[1] + (k / 3 - 1) as f32];
        let t = texel(off, a[0] as i32, a[1] as i32);
        if t[3] <= 0.0 {
            continue;
        }
        let b = texel(off + (w * h) as usize, a[0] as i32, a[1] as i32);
        let plane = texel(off + 2 * (w * h) as usize, a[0] as i32, a[1] as i32);
        let nt = oct_decode([plane[0], plane[1]]);
        let away = (dot(nt, p) - plane[2]).abs() / (0.01 * p[2] + 0.5) + (0.9 - dot(nt, gn)).max(0.0) * 10.0;
        if away < nearest {
            nearest = away;
            near = t;
            near_back = [b[0], b[1], b[2]];
        }
        let d = [(a[0] - g[0]).abs(), (a[1] - g[1]).abs()];
        let wt = (1.5 - d[0]).max(0.0) * (1.5 - d[1]).max(0.0) * (1.0 - away).max(0.0);
        let dir = oct_decode([t[0], t[1]]);
        sum = [sum[0] + dir[0] * wt, sum[1] + dir[1] * wt, sum[2] + dir[2] * wt, sum[3] + t[2] * wt];
        back = add(back, mul([b[0], b[1], b[2]], wt));
        weight += wt;
    }
    if nearest > 1e29 {
        return None;
    }
    if weight <= 1e-3 {
        return Some((oct_decode([near[0], near[1]]), near[2].clamp(0.0, 1.0), near_back));
    }
    Some((norm([sum[0], sum[1], sum[2]]), (sum[3] / weight).clamp(0.0, 1.0), mul(back, 1.0 / weight)))
}
