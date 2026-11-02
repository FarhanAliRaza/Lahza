//! Instancer layers (cloners): a template drawn once per layout point, with
//! per-copy delays, seeded variation, palettes and field effectors.
//!
//! Everything that does not change over time (delays, random values,
//! palette picks, stagger ranks) is worked out at compile time. A frame
//! evaluates the layout and the fields' parameters once; each copy is then a
//! few multiplies before its template is emitted like a comp's children, so
//! copies reach the display list as ordinary small primitives.

use super::*;
use crate::path::Poly;

/// Most copies one instancer may make.
pub const MAX_COPIES: usize = 50_000;

pub(super) struct CInstancer {
    pub(super) layers: Vec<CLayer>,
    layout: CLayout,
    copies: Vec<CopyBase>,
    fields: Vec<CField>,
}

/// What a copy is before the fields act on it.
struct CopyBase {
    delay: f64,
    scale: f64,
    rotation: f64,
    opacity: f64,
    jitter: (f64, f64),
    tint: Option<(Color, f64)>,
}

enum CLayout {
    Grid { columns: usize, rows: usize, spacing: Prop<Vec2>, hex: bool },
    Radial { radius: Prop<f64>, start: Prop<f64>, sweep: Prop<f64>, align: bool },
    Line { from: Prop<Vec2>, to: Prop<Vec2>, align: bool },
    /// Unit positions in [-0.5, 0.5]², scaled by `size`.
    Random { unit: Vec<(f64, f64)>, size: Prop<Vec2> },
    Path { target: Option<usize>, offset: Prop<f64>, align: bool },
}

struct CField {
    kind: FieldKind,
    center: Prop<Vec2>,
    radius: Prop<f64>,
    falloff: Option<Prop<f64>>,
    angle: Prop<f64>,
    range: Prop<f64>,
    size: Prop<f64>,
    speed: Prop<f64>,
    seed: u32,
    at: f64,
    wavelength: Prop<f64>,
    cycles: f64,
    decay: f64,
    curve: Option<Curve>,
    invert: bool,
    amount: Prop<f64>,
    position: Option<Prop<Vec2>>,
    scale: Option<Prop<f64>>,
    rotation: Option<Prop<f64>>,
    opacity: Option<Prop<f64>>,
    color: Option<Prop<Color>>,
}

impl CInstancer {
    pub(super) fn count(&self) -> usize {
        self.copies.len()
    }
    pub(super) fn path_target(&self) -> Option<usize> {
        match self.layout {
            CLayout::Path { target, .. } => target,
            _ => None,
        }
    }
    pub(super) fn clear_path_target(&mut self) {
        if let CLayout::Path { target, .. } = &mut self.layout {
            *target = None;
        }
    }
}

/// Parameters each field type reads (besides the shared ones).
fn field_params(kind: FieldKind) -> &'static [&'static str] {
    match kind {
        FieldKind::Radial => &["center", "radius", "falloff"],
        FieldKind::Linear => &["center", "angle", "range"],
        FieldKind::Noise => &["size", "speed", "seed"],
        FieldKind::Wave => &["center", "at", "speed", "wavelength", "cycles", "decay"],
    }
}

impl Compiler<'_> {
    #[allow(clippy::too_many_arguments)]
    fn opt<T: Animatable>(&mut self, p: &Option<Property<T>>, default: T, id: &str, what: &str, start: f64, step: Option<Step>, shift: f64) -> Prop<T> {
        match p {
            Some(p) => self.prop(p, id, what, start, step, shift),
            None => Prop::constant(default),
        }
    }

    pub(super) fn instancer(&mut self, n: &InstancerLayer, id: &str, start: f64, step: Option<Step>, shift: f64, siblings: &HashMap<&str, usize>) -> CInstancer {
        let layers = self.layers(&n.layers, step, shift);
        if n.layers.is_empty() {
            self.errors.push(format!("{id}: an instancer needs \"layers\" to repeat"));
        }
        // The layout, how many copies it makes, and each copy's place in
        // the structure (cells for grids, index for rows), for stagger order.
        let (layout, mut count) = match &n.layout {
            Layout::Grid { columns, rows, spacing, hex } => (
                CLayout::Grid { columns: *columns as usize, rows: *rows as usize, spacing: self.prop(spacing, id, "layout.spacing", start, step, shift), hex: *hex },
                *columns as u64 * *rows as u64,
            ),
            Layout::Radial { count, radius, start: a0, sweep, align } => (
                CLayout::Radial {
                    radius: self.prop(radius, id, "layout.radius", start, step, shift),
                    start: self.prop(a0, id, "layout.start", start, step, shift),
                    sweep: self.prop(sweep, id, "layout.sweep", start, step, shift),
                    align: *align,
                },
                *count as u64,
            ),
            Layout::Line { count, from, to, align } => (
                CLayout::Line { from: self.prop(from, id, "layout.from", start, step, shift), to: self.prop(to, id, "layout.to", start, step, shift), align: *align },
                *count as u64,
            ),
            Layout::Random { count, size, area, seed } => {
                let unit = (0..(*count as usize).min(MAX_COPIES))
                    .map(|k| {
                        let (u, v) = (rand(*seed, k, 0), rand(*seed, k, 1));
                        match area {
                            Area::Box => (u - 0.5, v - 0.5),
                            // Uniform over the disc.
                            Area::Circle => {
                                let (r, a) = (u.sqrt() / 2.0, v * std::f64::consts::TAU);
                                (r * a.cos(), r * a.sin())
                            }
                        }
                    })
                    .collect();
                (CLayout::Random { unit, size: self.prop(size, id, "layout.size", start, step, shift) }, *count as u64)
            }
            Layout::Path { count, layer, offset, align } => {
                let target = match siblings.get(layer.as_str()) {
                    Some(&t) => Some(t),
                    None => {
                        self.errors.push(format!("{id}.layout.layer: no sibling layer '{layer}'"));
                        None
                    }
                };
                (CLayout::Path { target, offset: self.prop(offset, id, "layout.offset", start, step, shift), align: *align }, *count as u64)
            }
        };
        if count == 0 {
            self.errors.push(format!("{id}.layout: makes no copies (count, columns and rows must be at least 1)"));
        } else if count > MAX_COPIES as u64 {
            self.errors.push(format!("{id}.layout: {count} copies is more than the {MAX_COPIES} an instancer can draw"));
            count = 0;
        }
        let count = count as usize;
        let structure: Vec<(f64, f64)> = match &layout {
            CLayout::Grid { columns, rows, hex, .. } => (0..count).map(|k| grid_cell(k, *columns, *rows, *hex)).collect(),
            CLayout::Random { unit, .. } => unit.clone(),
            _ => (0..count).map(|k| (k as f64 - (count as f64 - 1.0) / 2.0, 0.0)).collect(),
        };

        // Delays from the stagger order.
        let mut delays = vec![0.0; count];
        if let Some(s) = &n.stagger {
            let ranks = ranks(&structure, s.order, s.seed);
            let max = ranks.iter().copied().fold(0.0, f64::max);
            let per = match (s.each, s.total) {
                (Some(each), None) => self.seconds(each, id, "stagger.each"),
                (None, Some(total)) if max > 0.0 => self.seconds(total, id, "stagger.total") / max,
                (None, Some(_)) => 0.0,
                _ => {
                    self.errors.push(format!("{id}.stagger: give \"each\" (delay per copy) or \"total\" (spread over), not both"));
                    0.0
                }
            };
            for (d, r) in delays.iter_mut().zip(&ranks) {
                *d = per * r;
            }
        }
        let spread = n.random.clone().unwrap_or_default();
        let range = |r: Option<[f64; 2]>, k: usize, channel: u32, default: f64| match r {
            Some([lo, hi]) => lo + (hi - lo) * rand(spread.seed, k, channel),
            None => default,
        };
        let copies = (0..count)
            .map(|k| {
                let jitter = spread.position.map(|j| (j.x() * (2.0 * rand(spread.seed, k, 3) - 1.0), j.y() * (2.0 * rand(spread.seed, k, 4) - 1.0)));
                let mut tint = (!n.palette.is_empty()).then(|| {
                    let pick = match n.palette_order {
                        PaletteOrder::Cycle => k,
                        PaletteOrder::Random => (rand(spread.seed, k, 7) * n.palette.len() as f64) as usize,
                    };
                    (n.palette[pick % n.palette.len()], 1.0)
                });
                if let Some([a, b]) = spread.color {
                    tint = Some((a.lerp(&b, rand(spread.seed, k, 6)), 1.0));
                }
                CopyBase {
                    delay: delays[k] + range(spread.time, k, 5, 0.0),
                    scale: range(spread.scale, k, 0, 1.0),
                    rotation: range(spread.rotation, k, 1, 0.0),
                    opacity: range(spread.opacity, k, 2, 1.0),
                    jitter: jitter.unwrap_or((0.0, 0.0)),
                    tint,
                }
            })
            .collect();

        let mut fields = Vec::new();
        for (i, f) in n.fields.iter().enumerate() {
            let what = |p: &str| format!("fields.{i}.{p}");
            let given = [
                ("center", f.center.is_some()),
                ("radius", f.radius.is_some()),
                ("falloff", f.falloff.is_some()),
                ("angle", f.angle.is_some()),
                ("range", f.range.is_some()),
                ("size", f.size.is_some()),
                ("speed", f.speed.is_some()),
                ("seed", f.seed.is_some()),
                ("at", f.at.is_some()),
                ("wavelength", f.wavelength.is_some()),
                ("cycles", f.cycles.is_some()),
                ("decay", f.decay.is_some()),
            ];
            let allowed = field_params(f.kind);
            for (name, _) in given.iter().filter(|(name, set)| *set && !allowed.contains(name)) {
                let kind = serde_json::to_value(f.kind).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default();
                self.errors.push(format!("{id}.{}: a {kind} field has no \"{name}\" (it takes {})", what(name), allowed.join(", ")));
            }
            if f.position.is_none() && f.scale.is_none() && f.rotation.is_none() && f.opacity.is_none() && f.color.is_none() {
                self.errors.push(format!("{id}.fields.{i}: does nothing; give it position, scale, rotation, opacity or color deltas"));
            }
            let at = match &f.at {
                Some(r) => {
                    let t = self.time(r, id, &what("at")).unwrap_or(start);
                    if r.is_marker() {
                        self.syncs.push(SyncRecord { element: id.into(), what: what("at"), reference: r.text(), anchor: Anchor::Start, requested: t - shift, kind: SyncKind::Edge });
                    }
                    t
                }
                None => start,
            };
            let curve = f.ease.as_ref().map(|e| match Curve::compile(e, &self.windows.base) {
                Ok(c) => c,
                Err(e) => {
                    self.errors.push(format!("{id}.{}: {e}", what("ease")));
                    Curve::Linear
                }
            });
            let wave = f.kind == FieldKind::Wave;
            let positive = |v: Option<f64>, name: &str, errors: &mut Vec<String>| {
                if v.is_some_and(|v| !(v >= 0.0)) {
                    errors.push(format!("{id}.{}: must be 0 or more", what(name)));
                }
            };
            positive(f.cycles, "cycles", &mut self.errors);
            positive(f.decay, "decay", &mut self.errors);
            fields.push(CField {
                kind: f.kind,
                center: self.opt(&f.center, Vec2::ZERO, id, &what("center"), start, step, shift),
                radius: self.opt(&f.radius, 200.0, id, &what("radius"), start, step, shift),
                falloff: f.falloff.as_ref().map(|p| self.prop(p, id, &what("falloff"), start, step, shift)),
                angle: self.opt(&f.angle, 0.0, id, &what("angle"), start, step, shift),
                range: self.opt(&f.range, 200.0, id, &what("range"), start, step, shift),
                size: self.opt(&f.size, 200.0, id, &what("size"), start, step, shift),
                speed: self.opt(&f.speed, if wave { 600.0 } else { 0.5 }, id, &what("speed"), start, step, shift),
                seed: f.seed.unwrap_or(0),
                at,
                wavelength: self.opt(&f.wavelength, 200.0, id, &what("wavelength"), start, step, shift),
                cycles: f.cycles.unwrap_or(1.0).max(0.0),
                decay: f.decay.unwrap_or(0.0).max(0.0),
                curve,
                invert: f.invert,
                amount: self.prop(&f.amount, id, &what("amount"), start, step, shift),
                position: f.position.as_ref().map(|p| self.prop(p, id, &what("position"), start, step, shift)),
                scale: f.scale.as_ref().map(|p| self.prop(p, id, &what("scale"), start, step, shift)),
                rotation: f.rotation.as_ref().map(|p| self.prop(p, id, &what("rotation"), start, step, shift)),
                opacity: f.opacity.as_ref().map(|p| self.prop(p, id, &what("opacity"), start, step, shift)),
                color: f.color.as_ref().map(|p| self.prop(p, id, &what("color"), start, step, shift)),
            });
        }
        CInstancer { layers, layout, copies, fields }
    }
}

/// Cell of copy `k` in a grid, centred on the origin (hex rows shifted by
/// ±¼ cell so the grid stays centred).
fn grid_cell(k: usize, columns: usize, rows: usize, hex: bool) -> (f64, f64) {
    let (c, r) = (k % columns.max(1), k / columns.max(1));
    let shift = if hex && rows > 1 { if r % 2 == 1 { 0.25 } else { -0.25 } } else { 0.0 };
    (c as f64 - (columns as f64 - 1.0) / 2.0 + shift, r as f64 - (rows as f64 - 1.0) / 2.0)
}

/// Each copy's place in the stagger order (copies with equal ranks start
/// together): index order, or rings of equal distance from the middle.
fn ranks(structure: &[(f64, f64)], order: Order, seed: u32) -> Vec<f64> {
    let n = structure.len();
    match order {
        Order::Forward => (0..n).map(|k| k as f64).collect(),
        Order::Reverse => (0..n).map(|k| (n - 1 - k) as f64).collect(),
        Order::Random => {
            let mut idx: Vec<usize> = (0..n).collect();
            idx.sort_by(|a, b| rand(seed, *a, 9).total_cmp(&rand(seed, *b, 9)));
            let mut ranks = vec![0.0; n];
            for (rank, k) in idx.into_iter().enumerate() {
                ranks[k] = rank as f64;
            }
            ranks
        }
        Order::Center | Order::Edges => {
            let d: Vec<f64> = structure.iter().map(|p| p.0.hypot(p.1)).collect();
            let mut rings = d.clone();
            rings.sort_by(f64::total_cmp);
            rings.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
            let last = rings.len().saturating_sub(1) as f64;
            d.iter()
                .map(|v| {
                    let r = rings.partition_point(|x| *x < v - 1e-6) as f64;
                    if order == Order::Center { r } else { last - r }
                })
                .collect()
        }
    }
}

/// Deterministic uniform value in [0, 1) for (seed, copy, channel).
fn rand(seed: u32, k: usize, channel: u32) -> f64 {
    let mut z = ((seed as u64) << 32 | channel as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (k as u64).wrapping_mul(0xD1B5_4A32_D192_ED03);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (z >> 11) as f64 / (1u64 << 53) as f64
}

/// Smooth value noise in [0, 1] over (x, y, z).
fn noise3(x: f64, y: f64, z: f64, seed: u32) -> f64 {
    let (ix, iy, iz) = (x.floor(), y.floor(), z.floor());
    let s = |f: f64| f * f * (3.0 - 2.0 * f);
    let (fx, fy, fz) = (s(x - ix), s(y - iy), s(z - iz));
    let h = |dx: i64, dy: i64, dz: i64| {
        let k = (ix as i64 + dx).wrapping_mul(73_856_093) ^ (iy as i64 + dy).wrapping_mul(19_349_663) ^ (iz as i64 + dz).wrapping_mul(83_492_791);
        rand(seed, k as usize, 11)
    };
    let lerp = |a: f64, b: f64, t: f64| a + (b - a) * t;
    let plane = |dz: i64| lerp(lerp(h(0, 0, dz), h(1, 0, dz), fx), lerp(h(0, 1, dz), h(1, 1, dz), fx), fy);
    lerp(plane(0), plane(1), fz)
}

/// A field's parameters at one moment.
struct FieldNow<'a> {
    f: &'a CField,
    center: (f64, f64),
    shape: Shape,
    amount: f64,
    position: (f64, f64),
    scale: f64,
    rotation: f64,
    opacity: f64,
    color: Option<Color>,
}

enum Shape {
    Radial { radius: f64, falloff: f64 },
    Linear { dir: (f64, f64), range: f64 },
    Noise { inv: f64, z: f64 },
    /// `front`: how far the ring has travelled (None before it starts).
    Wave { front: Option<f64>, wavelength: f64 },
}

impl CField {
    fn now(&self, t: f64) -> FieldNow<'_> {
        let c = self.center.at(t);
        let shape = match self.kind {
            FieldKind::Radial => {
                let radius = self.radius.at(t);
                Shape::Radial { radius, falloff: self.falloff.as_ref().map(|p| p.at(t)).unwrap_or(radius).max(0.0) }
            }
            FieldKind::Linear => {
                let (s, k) = self.angle.at(t).to_radians().sin_cos();
                Shape::Linear { dir: (k, s), range: self.range.at(t) }
            }
            FieldKind::Noise => Shape::Noise { inv: 1.0 / self.size.at(t).max(1e-6), z: t * self.speed.at(t) },
            FieldKind::Wave => Shape::Wave { front: (t >= self.at).then(|| self.speed.at(t) * (t - self.at)), wavelength: self.wavelength.at(t).max(1e-6) },
        };
        FieldNow {
            f: self,
            center: (c.x(), c.y()),
            shape,
            amount: self.amount.at(t),
            position: self.position.as_ref().map(|p| p.at(t)).map(|p| (p.x(), p.y())).unwrap_or((0.0, 0.0)),
            scale: self.scale.as_ref().map(|p| p.at(t)).unwrap_or(0.0),
            rotation: self.rotation.as_ref().map(|p| p.at(t)).unwrap_or(0.0),
            opacity: self.opacity.as_ref().map(|p| p.at(t)).unwrap_or(0.0),
            color: self.color.as_ref().map(|p| p.at(t)),
        }
    }
}

impl FieldNow<'_> {
    /// The field's weight at (x, y) in the instancer's space.
    fn weight(&self, x: f64, y: f64) -> f64 {
        let (dx, dy) = (x - self.center.0, y - self.center.1);
        let mut v = match self.shape {
            Shape::Radial { radius, falloff } => {
                let d = dx.hypot(dy);
                if falloff > 0.0 { ((radius - d) / falloff).clamp(0.0, 1.0) } else { (d <= radius) as u8 as f64 }
            }
            Shape::Linear { dir, range } => {
                let along = dx * dir.0 + dy * dir.1;
                if range > 0.0 { (along / range + 0.5).clamp(0.0, 1.0) } else { (along >= 0.0) as u8 as f64 }
            }
            Shape::Noise { inv, z } => noise3(x * inv, y * inv, z, self.f.seed),
            Shape::Wave { front, wavelength } => match front {
                None => 0.0,
                Some(front) => {
                    let d = dx.hypot(dy);
                    let u = (front - d) / wavelength;
                    if u < 0.0 || (self.f.cycles > 0.0 && u > self.f.cycles) {
                        0.0
                    } else {
                        let s = (std::f64::consts::PI * u).sin();
                        s * s * if self.f.decay > 0.0 { (-d / self.f.decay).exp() } else { 1.0 }
                    }
                }
            },
        };
        if self.f.invert {
            v = 1.0 - v;
        }
        if let Some(c) = &self.f.curve {
            v = c.progress(v, 1.0);
        }
        v * self.amount
    }
}

/// `a` (a colour mixed in by an amount) followed by mixing in `c` by `w`,
/// as one mix.
fn mix_tint(a: Option<(Color, f64)>, c: Color, w: f64) -> Option<(Color, f64)> {
    let w = w.clamp(0.0, 1.0);
    if w <= 0.0 {
        return a;
    }
    match a {
        None => Some((c, w)),
        Some((ac, aw)) => {
            let k = 1.0 - (1.0 - aw) * (1.0 - w);
            Some((Color::from_fn(|i| (ac.0[i] * aw * (1.0 - w) + c.0[i] * w) / k), k))
        }
    }
}

/// Mixes a state's solid colours toward `c` by `a`, keeping their alpha.
fn tint(state: &mut LayerState, c: Color, a: f64) {
    let mix = |x: &mut Color| *x = Color([x.0[0] + (c.0[0] - x.0[0]) * a, x.0[1] + (c.0[1] - x.0[1]) * a, x.0[2] + (c.0[2] - x.0[2]) * a, x.0[3]]);
    match &mut state.content {
        Content::Shape(s) => {
            if let Some(PaintState::Solid(f)) = &mut s.fill {
                mix(f);
            }
            if let Some((sc, _)) = &mut s.stroke {
                mix(sc);
            }
        }
        Content::Text(t) => mix(&mut t.color),
        Content::Group(children) => children.iter_mut().for_each(|s| tint(s, c, a)),
        Content::Mesh(m) => {
            m.materials.iter_mut().for_each(|mat| mix(&mut mat.color));
            m.colors.iter_mut().for_each(|x| mix(x));
            m.part_colors.iter_mut().flatten().for_each(|x| mix(x));
        }
        _ => {}
    }
}

/// Layout points (x, y, angle) at clock `t` in the instancer's space.
#[allow(clippy::too_many_arguments)]
fn points(n: &CInstancer, layers: &[CLayer], i: usize, tc: f64, t: f64, inherited: Option<Step>, space: &Space, cache: &mut [Option<Mat4>]) -> Vec<(f64, f64, f64)> {
    let count = n.count();
    let along = |k: usize| if count > 1 { k as f64 / (count - 1) as f64 } else { 0.5 };
    match &n.layout {
        CLayout::Grid { columns, rows, spacing, hex } => {
            let s = spacing.at(t);
            (0..count).map(|k| grid_cell(k, *columns, *rows, *hex)).map(|(x, y)| (x * s.x(), y * s.y(), 0.0)).collect()
        }
        CLayout::Radial { radius, start, sweep, align } => {
            let (r, a0, sw) = (radius.at(t), start.at(t), sweep.at(t));
            let step = if sw.abs() >= 360.0 - 1e-9 { sw / count as f64 } else if count > 1 { sw / (count - 1) as f64 } else { 0.0 };
            (0..count)
                .map(|k| {
                    let a = a0 + step * k as f64;
                    let (s, c) = a.to_radians().sin_cos();
                    (r * s, -r * c, if *align { a } else { 0.0 })
                })
                .collect()
        }
        CLayout::Line { from, to, align } => {
            let (a, b) = (from.at(t), to.at(t));
            let angle = if *align { (b.y() - a.y()).atan2(b.x() - a.x()).to_degrees() } else { 0.0 };
            (0..count).map(|k| (a.x() + (b.x() - a.x()) * along(k), a.y() + (b.y() - a.y()) * along(k), angle)).collect()
        }
        CLayout::Random { unit, size } => {
            let s = size.at(t);
            unit.iter().map(|(u, v)| (u * s.x(), v * s.y(), 0.0)).collect()
        }
        CLayout::Path { target: Some(target), offset, align } => {
            let tl = &layers[*target];
            let CKind::Shape(CShape { path: Some((paths, index)), .. }) = &tl.kind else { return Vec::new() };
            let tm = layer_matrix(layers, *target, tc, inherited, space, cache);
            let own = layer_matrix(layers, i, tc, inherited, space, cache);
            // Path layer space → the instancer's space (2D).
            let Some(inv) = own.to_affine().inverse() else { return Vec::new() };
            let to_local = inv.then(tm.to_affine());
            let screen = space.screen(space.world.then(tm), space.three_d || tl.three_d);
            let tt = clock(tl.step.or(inherited), tc);
            let Ok(subpaths) = crate::path::morph(&path_morph(paths, index.at(tt)), crate::path::tolerance(screen.max_scale())) else { return Vec::new() };
            let closed = !subpaths.is_empty() && subpaths.iter().all(|p: &Poly| p.1);
            let shift = offset.at(t);
            (0..count)
                .filter_map(|k| {
                    let f = if closed { (k as f64 / count as f64 + shift).rem_euclid(1.0) } else { (along(k) + shift).clamp(0.0, 1.0) };
                    let ((x, y), (dx, dy)) = crate::path::point_at(&subpaths, f)?;
                    let p = to_local.apply(x, y);
                    let q = to_local.apply(x + dx, y + dy);
                    Some((p.0, p.1, if *align { (q.1 - p.1).atan2(q.0 - p.0).to_degrees() } else { 0.0 }))
                })
                .collect()
        }
        CLayout::Path { target: None, .. } => Vec::new(),
    }
}

/// Emits every visible copy of instancer `i`'s template. `inner` is the
/// instancer's own space; `t` its clock and `tc` the content time its
/// copies are delayed from.
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_copies(n: &CInstancer, layers: &[CLayer], i: usize, tc: f64, t: f64, step: Option<Step>, inherited: Option<Step>, space: &Space, inner: &Space, opacity: f64, cache: &mut [Option<Mat4>], out: &mut Vec<LayerState>) {
    let points = points(n, layers, i, tc, t, inherited, space, cache);
    let fields: Vec<FieldNow> = n.fields.iter().map(|f| f.now(t)).collect();
    for (base, &(px, py, angle)) in n.copies.iter().zip(&points) {
        let (x, y) = (px + base.jitter.0, py + base.jitter.1);
        let (mut dx, mut dy) = (0.0, 0.0);
        let mut scale = base.scale;
        let mut rotation = angle + base.rotation;
        let mut alpha = base.opacity;
        let mut colour = base.tint;
        for f in &fields {
            let w = f.weight(x, y);
            if w == 0.0 {
                continue;
            }
            dx += f.position.0 * w;
            dy += f.position.1 * w;
            scale += f.scale * w;
            rotation += f.rotation * w;
            alpha += f.opacity * w;
            if let Some(c) = f.color {
                colour = mix_tint(colour, c, w);
            }
        }
        let alpha = alpha.clamp(0.0, 1.0);
        if alpha <= 0.0 || scale <= 0.0 {
            continue;
        }
        let m = Mat4::translate(x + dx, y + dy, 0.0).then(Mat4::rotate_z(rotation)).then(Mat4::scale(scale, scale, scale.abs()));
        let copy = Space { world: inner.world.then(m), ..*inner };
        let first = out.len();
        emit(&n.layers, tc - base.delay, step, &copy, opacity * alpha, out);
        if let Some((c, a)) = colour {
            out[first..].iter_mut().for_each(|s| tint(s, c, a));
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::check::{check, Options};
    use crate::compile::{Compiled, Content, FrameState, LayerState, PaintState};
    use crate::document::Project;
    use crate::time::Fps;
    use serde_json::json;

    fn project(layers: serde_json::Value) -> Project {
        let mut p = Project::new(1000, 1000, Fps::whole(30), 6.0);
        p.scenes.push(serde_json::from_value(json!({"id": "s", "start": 0, "layers": layers})).unwrap());
        p
    }

    fn compiled(layers: serde_json::Value) -> Compiled {
        let c = Compiled::new(&project(layers));
        assert!(c.errors.is_empty(), "{:?}", c.errors);
        c
    }

    /// An instancer at the canvas centre repeating a 10 px white dot.
    fn dots(layout: serde_json::Value, extra: serde_json::Value) -> serde_json::Value {
        let mut l = json!({"id": "dots", "kind": "instancer", "transform": {"position": [500, 500]}, "layout": layout,
            "layers": [{"id": "dot", "kind": "shape", "shape": "circle", "size": 10, "fill": "#ffffff"}]});
        for (k, v) in extra.as_object().unwrap() {
            l[k] = v.clone();
        }
        l
    }

    /// Canvas origin, scale and angle (degrees) of every copy of `id`.
    fn copies(f: &FrameState, id: &str) -> Vec<(f64, f64, f64, f64)> {
        f.layers
            .iter()
            .filter(|l| &*l.id == id)
            .map(|l| {
                let (o, x) = (l.matrix.apply(0.0, 0.0), l.matrix.apply(1.0, 0.0));
                let (dx, dy) = (x.0 - o.0, x.1 - o.1);
                (o.0, o.1, dx.hypot(dy), dy.atan2(dx).to_degrees())
            })
            .collect()
    }

    fn fill(l: &LayerState) -> [f64; 4] {
        match &l.content {
            Content::Shape(s) => match &s.fill {
                Some(PaintState::Solid(c)) => c.0,
                _ => panic!("no solid fill"),
            },
            _ => panic!("not a shape"),
        }
    }

    fn near(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn layouts_place_copies() {
        let c = compiled(json!([dots(json!({"type": "grid", "columns": 3, "rows": 2, "spacing": [10, 20]}), json!({}))]));
        let p = copies(&c.frame(0.0), "dot");
        let want = [(490.0, 490.0), (500.0, 490.0), (510.0, 490.0), (490.0, 510.0), (500.0, 510.0), (510.0, 510.0)];
        assert_eq!(p.len(), 6);
        for (got, want) in p.iter().zip(want) {
            assert!(near(got.0, want.0) && near(got.1, want.1), "{p:?}");
        }
        // Hex rows sit a quarter cell either side of the centre.
        let c = compiled(json!([dots(json!({"type": "grid", "columns": 2, "rows": 2, "spacing": 40, "hex": true}), json!({}))]));
        let p = copies(&c.frame(0.0), "dot");
        assert!(near(p[0].0, 470.0) && near(p[2].0, 490.0), "{p:?}");

        // Radial: 0° is up, clockwise; align turns each copy to its angle.
        let c = compiled(json!([dots(json!({"type": "radial", "count": 4, "radius": 100, "align": true}), json!({}))]));
        let p = copies(&c.frame(0.0), "dot");
        let want = [(500.0, 400.0, 0.0), (600.0, 500.0, 90.0), (500.0, 600.0, 180.0), (400.0, 500.0, -90.0)];
        for (got, want) in p.iter().zip(want) {
            let turn = (got.3 - want.2).rem_euclid(360.0);
            assert!(near(got.0, want.0) && near(got.1, want.1) && (turn < 1e-6 || turn > 360.0 - 1e-6), "{p:?}");
        }
        // A partial sweep includes both ends; the radius animates.
        let c = compiled(json!([dots(json!({"type": "radial", "count": 3, "sweep": 180, "radius": {"keys": [{"at": 0, "v": 0}, {"at": 1, "v": 100}]}}), json!({}))]));
        let p = copies(&c.frame(1.0), "dot");
        assert!(near(p[2].0, 500.0) && near(p[2].1, 600.0), "{p:?}");
        assert!(copies(&c.frame(0.0), "dot").iter().all(|q| near(q.0, 500.0) && near(q.1, 500.0)));

        // Line: ends included, aligned along it.
        let c = compiled(json!([dots(json!({"type": "line", "count": 3, "from": [0, 0], "to": [0, 200], "align": true}), json!({}))]));
        let p = copies(&c.frame(0.0), "dot");
        assert!(near(p[1].1, 600.0) && near(p[2].1, 700.0) && near(p[1].3, 90.0), "{p:?}");

        // Random: inside the box or the disc, and spread over it.
        type Inside = fn(f64, f64) -> bool;
        let cases: [(&str, Inside); 2] = [("box", |x, y| x.abs() <= 100.0 && y.abs() <= 50.0), ("circle", |x, y| (x / 100.0).powi(2) + (y / 50.0).powi(2) <= 1.0 + 1e-9)];
        for (area, inside) in cases {
            let c = compiled(json!([dots(json!({"type": "random", "count": 200, "size": [200, 100], "area": area, "seed": 3}), json!({}))]));
            let p = copies(&c.frame(0.0), "dot");
            assert_eq!(p.len(), 200);
            assert!(p.iter().all(|q| inside(q.0 - 500.0, q.1 - 500.0)), "{area}");
            let spread = p.iter().map(|q| q.0).fold(f64::NEG_INFINITY, f64::max) - p.iter().map(|q| q.0).fold(f64::INFINITY, f64::min);
            assert!(spread > 150.0, "{area}: {spread}");
        }

        // Path: a closed square, one copy per corner, turned along it; the
        // path's own transform applies.
        let c = compiled(json!([
            {"id": "sq", "kind": "shape", "shape": "path", "d": "M -100 -100 L 100 -100 L 100 100 L -100 100 Z", "stroke": {"color": "#fff", "width": 2},
             "transform": {"position": [500, 500]}},
            {"id": "dots", "kind": "instancer", "layout": {"type": "path", "layer": "sq", "count": 4, "align": true},
             "layers": [{"id": "dot", "kind": "shape", "shape": "circle", "size": 10, "fill": "#fff"}]}
        ]));
        let p = copies(&c.frame(0.0), "dot");
        let want = [(400.0, 400.0), (600.0, 400.0), (600.0, 600.0), (400.0, 600.0)];
        assert_eq!(p.len(), 4);
        for (got, want) in p.iter().zip(want) {
            assert!((got.0 - want.0).abs() < 1e-3 && (got.1 - want.1).abs() < 1e-3, "{p:?}");
        }
        assert!((p[1].3 - 45.0).abs() < 1.0, "corners take the bisector: {p:?}");
        // An open path includes both ends; `offset` slides the copies.
        let c = compiled(json!([
            {"id": "ln", "kind": "shape", "shape": "path", "d": "M 0 0 L 200 0", "stroke": {"color": "#fff", "width": 2}},
            {"id": "dots", "kind": "instancer", "transform": {"position": [100, 100]}, "layout": {"type": "path", "layer": "ln", "count": 3, "offset": 0.25},
             "layers": [{"id": "dot", "kind": "shape", "shape": "circle", "size": 10, "fill": "#fff"}]}
        ]));
        let p = copies(&c.frame(0.0), "dot");
        assert!(near(p[0].0, 50.0) && near(p[1].0, 150.0) && near(p[2].0, 200.0) && near(p[0].1, 0.0), "{p:?}");
    }

    #[test]
    fn stagger_delays_each_copy() {
        // Each dot fades in over 1 s from its start; copies start 0.5 s apart.
        let fade = json!([{"id": "dot", "kind": "shape", "shape": "circle", "size": 10, "fill": "#fff",
            "transform": {"opacity": {"keys": [{"at": 0, "v": 0}, {"at": 1, "v": 1}]}}}]);
        let mut l = dots(json!({"type": "line", "count": 3}), json!({"stagger": {"each": 0.5}}));
        l["layers"] = fade.clone();
        let c = compiled(json!([l]));
        let alphas = |t: f64| c.frame(t).layers.iter().map(|l| l.opacity).collect::<Vec<_>>();
        // Copies at zero opacity are not drawn.
        assert_eq!(alphas(0.75).len(), 2);
        assert!(near(alphas(0.75)[0], 0.75) && near(alphas(0.75)[1], 0.25), "{:?}", alphas(0.75));
        assert!(near(alphas(1.25)[2], 0.25));
        // Reverse order spread over a total.
        let mut l = dots(json!({"type": "line", "count": 3}), json!({"stagger": {"total": 1.0, "order": "reverse"}}));
        l["layers"] = fade.clone();
        let c = compiled(json!([l]));
        let f = c.frame(0.75);
        assert_eq!(f.layers.len(), 2);
        assert!(near(f.layers[0].opacity, 0.25) && near(f.layers[1].opacity, 0.75));
        // Centre order starts in the middle of a grid and moves out in rings.
        let mut l = dots(json!({"type": "grid", "columns": 3, "rows": 3}), json!({"stagger": {"each": 0.5, "order": "center"}}));
        l["layers"] = fade;
        let c = compiled(json!([l]));
        let f = c.frame(0.25);
        assert_eq!(f.layers.len(), 1);
        assert!(near(f.layers[0].matrix.apply(0.0, 0.0).0, 500.0));
        assert_eq!(c.frame(0.75).layers.len(), 5, "centre and edge midpoints");
        assert_eq!(c.frame(1.25).layers.len(), 9);
    }

    #[test]
    fn random_spread_is_seeded_and_in_range() {
        let l = dots(json!({"type": "grid", "columns": 10, "rows": 10}), json!({"random": {"seed": 5, "scale": [0.5, 1.5], "rotation": [-30, 30], "opacity": [0.4, 0.8], "position": [5, 0]}}));
        let c = compiled(json!([l.clone()]));
        let f = c.frame(0.0);
        let p = copies(&f, "dot");
        assert!(p.iter().all(|q| q.2 >= 0.5 - 1e-9 && q.2 <= 1.5 + 1e-9 && q.3.abs() <= 30.0 + 1e-9));
        assert!(f.layers.iter().all(|l| l.opacity >= 0.4 - 1e-9 && l.opacity <= 0.8 + 1e-9));
        assert!(p.iter().map(|q| q.2).fold(0.0, f64::max) > 1.3 && p.iter().map(|q| q.2).fold(9.0, f64::min) < 0.7, "values spread over the range");
        // Same seed, same picture; another seed, another one.
        assert_eq!(Compiled::new(&project(json!([l.clone()]))).frame(0.0), f);
        let mut other = l;
        other["random"]["seed"] = json!(6);
        assert_ne!(Compiled::new(&project(json!([other]))).frame(0.0), f);
    }

    #[test]
    fn palettes_colour_mesh_copies() {
        let c = compiled(json!([{"id": "n", "kind": "instancer", "layout": {"type": "line", "count": 2}, "palette": ["#ff0000", "#0000ff"],
            "layers": [{"id": "m", "kind": "mesh", "mesh": {"type": "sphere", "radius": 10}}]}]));
        let colours: Vec<crate::value::Color> = c.frame(0.5).layers.iter().filter_map(|l| match &l.content {
            Content::Mesh(m) => Some(m.materials[crate::mesh::PART_SIDE as usize].color),
            _ => None,
        }).collect();
        assert_eq!(colours, vec![crate::value::Color::parse("#ff0000").unwrap(), crate::value::Color::parse("#0000ff").unwrap()]);
    }

    #[test]
    fn palettes_cycle_and_colour_fields_mix() {
        let c = compiled(json!([dots(json!({"type": "line", "count": 4}), json!({"palette": ["#ff0000", "#0000ff"]}))]));
        let fills: Vec<[f64; 4]> = c.frame(0.0).layers.iter().map(fill).collect();
        let (red, blue) = ([1.0, 0.0, 0.0, 1.0], [0.0, 0.0, 1.0, 1.0]);
        assert_eq!(fills, [red, blue, red, blue]);
        // A radial field turning copies green near the centre (half way at
        // 50 px: the falloff defaults to the whole radius).
        let c = compiled(json!([dots(json!({"type": "line", "count": 3, "from": [0, 0], "to": [100, 0]}),
            json!({"fields": [{"type": "radial", "radius": 100, "color": "#00ff00"}]}))]));
        let f = c.frame(0.0);
        assert_eq!(fill(&f.layers[0]), [0.0, 1.0, 0.0, 1.0]);
        assert!(near(fill(&f.layers[1])[0], 0.5) && near(fill(&f.layers[1])[1], 1.0));
        assert_eq!(fill(&f.layers[2]), [1.0, 1.0, 1.0, 1.0]);
    }

    #[test]
    fn radial_and_linear_fields_shape_copies() {
        // Copies at x = 0, 50, 100, 150 in the instancer's space.
        let line = json!({"type": "line", "count": 4, "from": [0, 0], "to": [150, 0]});
        let c = compiled(json!([dots(line.clone(), json!({"fields": [
            {"type": "radial", "radius": 100, "falloff": 50, "scale": 1, "position": [0, -10], "rotation": 90, "opacity": -0.5}
        ]}))]));
        let f = c.frame(0.0);
        let p = copies(&f, "dot");
        // Full within 50 px, none from 100 px.
        assert!(near(p[0].2, 2.0) && near(p[1].2, 2.0) && near(p[2].2, 1.0) && near(p[3].2, 1.0), "{p:?}");
        assert!(near(p[0].1, 490.0) && near(p[0].3, 90.0) && near(f.layers[0].opacity, 0.5));
        assert!(near(p[3].1, 500.0) && near(f.layers[3].opacity, 1.0));
        // Linear along +x over 100 px centred on x = 75.
        let c = compiled(json!([dots(line.clone(), json!({"fields": [{"type": "linear", "center": [75, 0], "range": 100, "scale": 1}]}))]));
        let s: Vec<f64> = copies(&c.frame(0.0), "dot").iter().map(|q| q.2).collect();
        assert!(near(s[0], 1.0) && near(s[1], 1.25) && near(s[2], 1.75) && near(s[3], 2.0), "{s:?}");
        // Inverted, eased (hold: only full weight counts) and half strength.
        let c = compiled(json!([dots(line, json!({"fields": [{"type": "linear", "center": [75, 0], "range": 100, "scale": 1, "invert": true, "ease": {"type": "hold"}, "amount": 0.5}]}))]));
        let s: Vec<f64> = copies(&c.frame(0.0), "dot").iter().map(|q| q.2).collect();
        assert!(near(s[0], 1.5) && near(s[1], 1.0) && near(s[3], 1.0), "{s:?}");
    }

    #[test]
    fn noise_field_is_smooth_bounded_and_evolves() {
        let l = dots(json!({"type": "grid", "columns": 30, "rows": 30, "spacing": 10}), json!({"fields": [{"type": "noise", "size": 80, "speed": 1, "seed": 2, "scale": 1}]}));
        let c = compiled(json!([l]));
        let s = |t: f64| copies(&c.frame(t), "dot").iter().map(|q| q.2 - 1.0).collect::<Vec<_>>();
        let a = s(0.0);
        assert!(a.iter().all(|v| (-1e-9..=1.0 + 1e-9).contains(v)));
        let (lo, hi) = (a.iter().copied().fold(1.0, f64::min), a.iter().copied().fold(0.0, f64::max));
        assert!(hi - lo > 0.4, "varied: {lo}..{hi}");
        // Neighbours 10 px apart differ little next to 80 px features.
        assert!(a.windows(2).enumerate().filter(|(k, _)| k % 30 != 29).all(|(_, w)| (w[0] - w[1]).abs() < 0.2));
        assert_ne!(a, s(0.5), "evolves over time");
        assert_eq!(a, s(0.0), "deterministic");
    }

    #[test]
    fn wave_field_ripples_out_from_its_start() {
        // Dots that grow as a ring leaves the centre at 3.82 s (500 px/s, a
        // 100 px crest), plus a subtle noise twinkle.
        let l = dots(json!({"type": "line", "count": 5, "from": [0, 0], "to": [400, 0]}), json!({"fields": [
            {"type": "wave", "at": 3.82, "speed": 500, "wavelength": 100, "scale": 2},
            {"type": "noise", "size": 50, "scale": 0.0001}
        ]}));
        let c = compiled(json!([l]));
        let s = |t: f64| copies(&c.frame(t), "dot").iter().map(|q| q.2).collect::<Vec<_>>();
        assert!(s(3.8).iter().all(|v| (v - 1.0).abs() < 1e-3), "nothing before it starts");
        // At 3.92 s the front is 50 px out: its crest (half a wavelength
        // behind) is on the centre, at full size.
        assert!((s(3.92)[0] - 3.0).abs() < 1e-3, "{:?}", s(3.92));
        // At 4.32 s the front is at 250 px, the crest at 200 px (copy 2);
        // copy 1 (100 px) is past the single crest, copy 3 (300 px) ahead.
        let v = s(4.32);
        assert!((v[2] - 3.0).abs() < 1e-3 && (v[1] - 1.0).abs() < 1e-3 && (v[3] - 1.0).abs() < 1e-3, "{v:?}");
        // Endless ripples fading with distance: crests 100 px apart.
        let l = dots(json!({"type": "line", "count": 5, "from": [0, 0], "to": [200, 0]}), json!({"fields": [
            {"type": "wave", "at": 0, "speed": 100, "wavelength": 100, "cycles": 0, "decay": 200, "scale": 1}
        ]}));
        let c = compiled(json!([l]));
        let v: Vec<f64> = copies(&c.frame(4.5), "dot").iter().map(|q| q.2 - 1.0).collect();
        let want = [1.0, 0.0, (-0.5f64).exp(), 0.0, (-1.0f64).exp()];
        assert!(v.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-6), "{v:?}");
    }

    #[test]
    fn comps_repeat_and_frames_are_deterministic() {
        let l = json!({"id": "petals", "kind": "instancer", "transform": {"position": [500, 500]},
            "layout": {"type": "radial", "count": 8, "radius": 0, "align": true},
            "stagger": {"each": "1f"}, "random": {"seed": 1, "rotation": [-5, 5]},
            "layers": [{"id": "petal", "kind": "comp", "transform": {"position": {"keys": [{"at": 0, "v": [0, 0]}, {"at": 1, "v": [0, -200], "ease": "glide"}]}},
                        "layers": [{"id": "a", "kind": "shape", "shape": "capsule", "size": [20, 60], "fill": "#ff3d7f"},
                                   {"id": "b", "kind": "shape", "shape": "circle", "size": 8, "fill": "#ffffff"}]}]});
        let c = compiled(json!([l.clone()]));
        let f = c.frame(2.0);
        assert_eq!(f.layers.len(), 16);
        // Every petal ends 200 px out along its own angle.
        let centres: Vec<(f64, f64)> = f.layers.iter().filter(|l| &*l.id == "a").map(|l| l.matrix.apply(0.0, 0.0)).collect();
        assert!(centres.iter().all(|p| ((p.0 - 500.0).hypot(p.1 - 500.0) - 200.0).abs() < 1e-6), "{centres:?}");
        assert_eq!(Compiled::new(&project(json!([l]))).frame(2.0), f);
        // With opacity below 1 the copies are drawn as one picture.
        let mut g = dots(json!({"type": "line", "count": 3}), json!({}));
        g["transform"]["opacity"] = json!(0.5);
        let f = compiled(json!([g])).frame(0.0);
        assert!(matches!(&f.layers[0].content, Content::Group(kids) if kids.len() == 3));
    }

    #[test]
    fn check_reports_instancer_mistakes() {
        let errors = |layers| Compiled::new(&project(layers)).errors;
        let e = errors(json!([dots(json!({"type": "grid", "columns": 0, "rows": 3}), json!({}))]));
        assert!(e.iter().any(|e| e.contains("dots.layout: makes no copies")), "{e:?}");
        let e = errors(json!([dots(json!({"type": "grid", "columns": 1000, "rows": 1000}), json!({}))]));
        assert!(e.iter().any(|e| e.contains("more than the 50000")), "{e:?}");
        let e = errors(json!([dots(json!({"type": "line", "count": 3}), json!({"fields": [{"type": "wave", "radius": 3, "scale": 1}, {"type": "noise"}]}))]));
        assert!(e.iter().any(|e| e.contains("dots.fields.0.radius: a wave field has no \"radius\"")), "{e:?}");
        assert!(e.iter().any(|e| e.contains("dots.fields.1: does nothing")), "{e:?}");
        let e = errors(json!([dots(json!({"type": "line", "count": 3}), json!({"stagger": {"each": 1, "total": 2}}))]));
        assert!(e.iter().any(|e| e.contains("dots.stagger")), "{e:?}");
        let e = errors(json!([{"id": "c", "kind": "shape", "shape": "circle"}, dots(json!({"type": "path", "layer": "c", "count": 3}), json!({}))]));
        assert!(e.iter().any(|e| e.contains("dots.layout.layer: 'c' is not a path layer")), "{e:?}");
        let e = errors(json!([dots(json!({"type": "path", "layer": "nope", "count": 3}), json!({}))]));
        assert!(e.iter().any(|e| e.contains("no sibling layer 'nope'")), "{e:?}");
        let e = errors(json!([{"id": "dots", "kind": "instancer", "layout": {"type": "line", "count": 2}}]));
        assert!(e.iter().any(|e| e.contains("needs \"layers\"")), "{e:?}");
        // Typos are caught when the document is read.
        let bad = crate::document::Layer::from_json(dots(json!({"type": "grid", "columns": 2, "rows": 2, "spacng": 3}), json!({})));
        assert!(bad.unwrap_err().contains("spacng"));
        let bad = crate::document::Layer::from_json(dots(json!({"type": "line", "count": 2}), json!({"fields": [{"type": "ripple"}]})));
        assert!(bad.is_err());
        // `check` reports them as structure errors on the instancer.
        let p = project(json!([dots(json!({"type": "line", "count": 3}), json!({"fields": [{"type": "radial", "wavelength": 3, "scale": 1}]}))]));
        let r = check(&p, &Compiled::new(&p), &Options::default());
        assert!(!r.ok && r.issues.iter().any(|i| i.kind == "structure" && i.element.as_deref() == Some("dots")), "{r:#?}");
    }

    #[test]
    fn a_wave_on_a_marker_is_a_sync_point() {
        let mut p = project(json!([dots(json!({"type": "line", "count": 3}), json!({"fields": [{"type": "wave", "at": "beat:2", "scale": 1}]}))]));
        p.markers.insert(crate::markers::MarkerTrack::from_times("beat", [1.0, 2.0]));
        let c = Compiled::new(&p);
        assert!(c.errors.is_empty(), "{:?}", c.errors);
        assert!(c.syncs.iter().any(|s| s.element == "dots" && s.what == "fields.0.at" && (s.requested - 2.0).abs() < 1e-9), "{:?}", c.syncs);
    }

    #[test]
    fn guide_recipes_compile_and_draw() {
        let section = crate::catalog::GUIDE.split("## Repeaters").nth(1).unwrap().split("\n## ").next().unwrap();
        let recipes: Vec<serde_json::Value> = section.split("```json").skip(1).map(|b| serde_json::from_str(b.split("```").next().unwrap()).unwrap()).collect();
        assert_eq!(recipes.len(), 2);
        for recipe in recipes {
            let template = recipe["layers"][0]["id"].as_str().unwrap().to_string();
            let c = compiled(json!([recipe]));
            assert!(copies(&c.frame(0.45), &template).len() >= 12, "{template}");
            if template == "halftone-dot" {
                // All 1372 dots are up, and the ripple swells some of them.
                let p = copies(&c.frame(4.1), &template);
                assert_eq!(p.len(), 49 * 28);
                assert!(p.iter().any(|q| q.2 > 2.5) && p.iter().any(|q| q.2 < 1.3));
            }
        }
    }

    #[test]
    fn documents_round_trip() {
        let l = dots(json!({"type": "radial", "count": 12, "radius": 300, "align": true}), json!({
            "stagger": {"each": "1f", "order": "center"}, "random": {"seed": 2, "scale": [0.5, 1]}, "palette": ["#ffffff", "#ff3d7f"], "palette_order": "random",
            "fields": [{"type": "wave", "at": 1.5, "speed": 800, "wavelength": 160, "scale": 0.6, "ease": "gesture"}]}));
        let layer = crate::document::Layer::from_json(l).unwrap();
        let back = serde_json::to_value(&layer).unwrap();
        assert_eq!(crate::document::Layer::from_json(back).unwrap(), layer);
    }
}
