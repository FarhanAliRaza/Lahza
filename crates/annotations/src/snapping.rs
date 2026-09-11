//! Screen-space annotation snapping. Resolve the final pose first, then derive
//! indicators from exact bounds alignments.
use crate::geometry::V;
use gpui::{Bounds, Pixels};

const ACQUIRE: f32 = 8.;
const RELEASE: f32 = 12.;
const EXACT: f32 = 0.05;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub min: V,
    pub max: V,
}
impl Rect {
    pub fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self {
            min: V::new(x, y),
            max: V::new(x + w, y + h),
        }
    }
    pub fn translated(self, d: V) -> Self {
        Self {
            min: self.min.add(d),
            max: self.max.add(d),
        }
    }
    pub fn union(self, other: Self) -> Self {
        Self {
            min: V::new(self.min.x.min(other.min.x), self.min.y.min(other.min.y)),
            max: V::new(self.max.x.max(other.max.x), self.max.y.max(other.max.y)),
        }
    }
    pub fn intersects(self, other: Self) -> bool {
        self.max.x >= other.min.x
            && self.min.x <= other.max.x
            && self.max.y >= other.min.y
            && self.min.y <= other.max.y
    }
    fn anchors(self, axis: usize) -> [f32; 3] {
        let (lo, hi) = self.span(axis);
        [lo, (lo + hi) * 0.5, hi]
    }
    fn span(self, axis: usize) -> (f32, f32) {
        if axis == 0 {
            (self.min.x, self.max.x)
        } else {
            (self.min.y, self.max.y)
        }
    }
}
impl From<Bounds<Pixels>> for Rect {
    fn from(b: Bounds<Pixels>) -> Self {
        Self {
            min: b.origin.into(),
            max: V::new(f32::from(b.right()), f32::from(b.bottom())),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Canvas,
    Image,
    Object,
}
#[derive(Clone, Copy, Debug)]
pub struct Target {
    pub bounds: Rect,
    pub kind: Kind,
}
#[derive(Clone, Debug)]
pub struct Guide {
    pub start: V,
    pub end: V,
    pub points: Vec<V>,
    pub label: Option<String>,
    pub gap: bool,
}
#[derive(Clone, Debug)]
enum Match {
    Anchor {
        target: usize,
        source: usize,
        slot: usize,
    },
    // Equal spacing between two objects, or continuing their spacing on either side.
    Gap {
        first: usize,
        second: usize,
        placement: i8,
    },
}
#[derive(Clone, Debug)]
struct Candidate {
    axis: usize,
    delta: f32,
    priority: u8,
    kind: Match,
}
#[derive(Clone, Debug)]
pub struct Session {
    moving: Rect,
    targets: Vec<Target>,
    candidates: Vec<Candidate>,
    locked: [Option<usize>; 2],
    pub clip: Rect,
}
fn component(v: V, axis: usize) -> f32 {
    if axis == 0 {
        v.x
    } else {
        v.y
    }
}
fn at(axis: usize, along: f32, across: f32) -> V {
    if axis == 0 {
        V::new(along, across)
    } else {
        V::new(across, along)
    }
}
impl Session {
    pub fn new(moving: Rect, targets: Vec<Target>, clip: Rect) -> Self {
        let mut s = Self {
            moving,
            targets,
            candidates: Vec::new(),
            locked: [None; 2],
            clip,
        };
        for axis in 0..2 {
            let from = moving.anchors(axis);
            for (index, target) in s.targets.iter().enumerate() {
                let to = target.bounds.anchors(axis);
                if target.kind == Kind::Object {
                    for (source, a) in from.into_iter().enumerate() {
                        for (slot, b) in to.into_iter().enumerate() {
                            s.candidates.push(Candidate {
                                axis,
                                delta: b - a,
                                priority: 3,
                                kind: Match::Anchor {
                                    target: index,
                                    source,
                                    slot,
                                },
                            });
                        }
                    }
                } else {
                    // Frame centers/thirds attract the selection center, not an arbitrary edge.
                    let values = [
                        to[0],
                        to[0] + (to[2] - to[0]) / 3.,
                        to[1],
                        to[0] + (to[2] - to[0]) * 2. / 3.,
                        to[2],
                    ];
                    for (slot, value) in values.into_iter().enumerate() {
                        let source = if slot == 0 {
                            0
                        } else if slot == 4 {
                            2
                        } else {
                            1
                        };
                        s.candidates.push(Candidate {
                            axis,
                            delta: value - from[source],
                            priority: if slot == 2 {
                                0
                            } else if slot == 0 || slot == 4 {
                                1
                            } else {
                                2
                            },
                            kind: Match::Anchor {
                                target: index,
                                source,
                                slot,
                            },
                        });
                    }
                }
            }
            for a in 0..s.targets.len() {
                if moving.min == moving.max {
                    break;
                }
                if s.targets[a].kind != Kind::Object {
                    continue;
                }
                for b in a + 1..s.targets.len() {
                    if s.targets[b].kind != Kind::Object {
                        continue;
                    }
                    let (a, b) =
                        if s.targets[a].bounds.span(axis).0 <= s.targets[b].bounds.span(axis).0 {
                            (a, b)
                        } else {
                            (b, a)
                        };
                    let first = s.targets[a].bounds;
                    let second = s.targets[b].bounds;
                    let (l0, l1) = first.span(axis);
                    let (r0, r1) = second.span(axis);
                    let (u0, u1) = first.span(1 - axis);
                    let (v0, v1) = second.span(1 - axis);
                    let gap = r0 - l1;
                    if gap <= 0. || u1.min(v1) <= u0.max(v0) {
                        continue;
                    }
                    // Only adjacent objects define a gap; never snap through an intervening one.
                    if s.targets.iter().enumerate().any(|(i, t)| {
                        i != a
                            && i != b
                            && t.kind == Kind::Object
                            && t.bounds.span(axis).0 < r0
                            && t.bounds.span(axis).1 > l1
                            && t.bounds.span(1 - axis).0 < u1.min(v1)
                            && t.bounds.span(1 - axis).1 > u0.max(v0)
                    }) {
                        continue;
                    }
                    let width = from[2] - from[0];
                    for (placement, delta) in [
                        (-1, l0 - gap - from[2]),
                        (0, (l1 + r0 - width) * 0.5 - from[0]),
                        (1, r1 + gap - from[0]),
                    ] {
                        if placement == 0 && width > gap {
                            continue;
                        }
                        s.candidates.push(Candidate {
                            axis,
                            delta,
                            priority: 4,
                            kind: Match::Gap {
                                first: a,
                                second: b,
                                placement,
                            },
                        });
                    }
                }
            }
        }
        s
    }
    fn eligible(&self, c: &Candidate, delta: V) -> bool {
        match c.kind {
            Match::Anchor { .. } => true,
            Match::Gap { first, second, .. } => {
                let (a, b) = self.targets[first].bounds.span(1 - c.axis);
                let (c0, d) = self.targets[second].bounds.span(1 - c.axis);
                let (u, v) = self.moving.translated(delta).span(1 - c.axis);
                u < b.min(d) && v > a.max(c0)
            }
        }
    }
    /// Limits are also screen-space. A guide must describe the constrained final
    /// position, never a snap that was subsequently undone by edge clamping.
    pub fn translate(
        &mut self,
        raw: V,
        min: V,
        max: V,
        locked_axis: Option<usize>,
        enabled: bool,
    ) -> (V, Vec<Guide>) {
        let mut delta = V::new(raw.x.clamp(min.x, max.x), raw.y.clamp(min.y, max.y));
        if !enabled {
            self.locked = [None; 2];
            return (delta, Vec::new());
        }
        for axis in 0..2 {
            if locked_axis == Some(axis) {
                self.locked[axis] = None;
                continue;
            }
            let original = component(raw, axis);
            let valid = |i: usize, threshold: f32| {
                let c = &self.candidates[i];
                c.axis == axis
                    && (c.delta - original).abs() <= threshold
                    && c.delta >= component(min, axis) - EXACT
                    && c.delta <= component(max, axis) + EXACT
                    && self.eligible(c, delta)
            };
            let chosen = self.locked[axis]
                .filter(|i| valid(*i, RELEASE))
                .or_else(|| {
                    self.candidates
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| valid(*i, ACQUIRE))
                        .min_by(|(_, a), (_, b)| {
                            // Subpixel ties prefer frames, centers and then stable document order.
                            let da = ((a.delta - original).abs() * 2.).round();
                            let db = ((b.delta - original).abs() * 2.).round();
                            da.total_cmp(&db).then(a.priority.cmp(&b.priority))
                        })
                        .map(|(i, _)| i)
                });
            self.locked[axis] = chosen;
            if let Some(i) = chosen {
                let value = self.candidates[i]
                    .delta
                    .clamp(component(min, axis), component(max, axis));
                if axis == 0 {
                    delta.x = value
                } else {
                    delta.y = value
                }
            }
        }
        let mut guides = Vec::new();
        for c in &self.candidates {
            if locked_axis == Some(c.axis)
                || (component(delta, c.axis) - c.delta).abs() > EXACT
                || !self.eligible(c, delta)
            {
                continue;
            }
            guides.extend(self.indicators(c, delta));
        }
        // Combine coincident point guides; no stacked lines or repeated labels.
        let mut merged: Vec<Guide> = Vec::new();
        for g in guides {
            if let Some(existing) = merged.iter_mut().find(|a| {
                !a.gap
                    && !g.gap
                    && ((a.start.x - a.end.x).abs() < EXACT
                        && (g.start.x - g.end.x).abs() < EXACT
                        && (a.start.x - g.start.x).abs() < EXACT
                        || (a.start.y - a.end.y).abs() < EXACT
                            && (g.start.y - g.end.y).abs() < EXACT
                            && (a.start.y - g.start.y).abs() < EXACT)
            }) {
                existing.start = V::new(
                    existing.start.x.min(g.start.x),
                    existing.start.y.min(g.start.y),
                );
                existing.end = V::new(existing.end.x.max(g.end.x), existing.end.y.max(g.end.y));
                existing.points.extend(g.points);
                if existing.label.is_none() {
                    existing.label = g.label;
                }
            } else {
                merged.push(g);
            }
        }
        (delta, merged)
    }
    fn indicators(&self, c: &Candidate, delta: V) -> Vec<Guide> {
        let moved = self.moving.translated(delta);
        let axis = c.axis;
        match c.kind {
            Match::Anchor {
                target,
                source,
                slot,
            } => {
                let target = self.targets[target];
                let value = moved.anchors(axis)[source];
                let (a, b) = target.bounds.span(1 - axis);
                let (u, v) = moved.span(1 - axis);
                let label = if target.kind == Kind::Object {
                    None
                } else {
                    let name = if target.kind == Kind::Canvas {
                        "Canvas"
                    } else {
                        "Image"
                    };
                    let location = match slot {
                        0 => {
                            if axis == 0 {
                                "left"
                            } else {
                                "top"
                            }
                        }
                        1 => "1/3",
                        2 => "center",
                        3 => "2/3",
                        _ => {
                            if axis == 0 {
                                "right"
                            } else {
                                "bottom"
                            }
                        }
                    };
                    Some(format!("{name} · {location}"))
                };
                vec![Guide {
                    start: at(axis, value, a.min(u)),
                    end: at(axis, value, b.max(v)),
                    points: vec![
                        at(axis, value, a),
                        at(axis, value, b),
                        at(axis, value, u),
                        at(axis, value, v),
                    ],
                    label,
                    gap: false,
                }]
            }
            Match::Gap {
                first,
                second,
                placement,
            } => {
                let first = self.targets[first].bounds;
                let second = self.targets[second].bounds;
                let (a, b) = first.span(axis);
                let (c, d) = second.span(axis);
                let (u, v) = moved.span(axis);
                let (p, q) = first.span(1 - axis);
                let (r, s) = second.span(1 - axis);
                let (m, n) = moved.span(1 - axis);
                let cross = (p.max(r).max(m) + q.min(s).min(n)) * 0.5;
                let intervals = match placement {
                    -1 => [(v, a), (b, c)],
                    0 => [(b, u), (v, c)],
                    _ => [(b, c), (d, u)],
                };
                intervals
                    .into_iter()
                    .map(|(from, to)| Guide {
                        start: at(axis, from, cross),
                        end: at(axis, to, cross),
                        points: vec![],
                        label: Some("Equal spacing".into()),
                        gap: true,
                    })
                    .collect()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn limits() -> (V, V) {
        (V::new(-2000., -2000.), V::new(2000., 2000.))
    }
    fn frame(kind: Kind, x: f32, y: f32, w: f32, h: f32) -> Target {
        Target {
            bounds: Rect::new(x, y, w, h),
            kind,
        }
    }
    fn session(targets: Vec<Target>) -> Session {
        Session::new(
            Rect::new(20., 30., 100., 40.),
            targets,
            Rect::new(0., 0., 1000., 800.),
        )
    }
    #[test]
    fn canvas_center_and_thirds_work_without_other_annotations() {
        let (min, max) = limits();
        let target = frame(Kind::Canvas, 0., 0., 900., 600.);
        let mut s = session(vec![target]);
        let (d, g) = s.translate(V::new(376., 247.), min, max, None, true);
        assert_eq!(d, V::new(380., 250.));
        assert_eq!(
            g.iter()
                .filter(|g| g.label.as_deref() == Some("Canvas · center"))
                .count(),
            2
        );
        let mut s = session(vec![target]);
        let (d, g) = s.translate(V::new(233., 352.), min, max, None, true);
        assert_eq!(d, V::new(230., 350.));
        assert!(g.iter().any(|g| g.label.as_deref() == Some("Canvas · 1/3")));
        assert!(g.iter().any(|g| g.label.as_deref() == Some("Canvas · 2/3")));
    }
    #[test]
    fn image_and_canvas_have_independent_real_positions() {
        let (min, max) = limits();
        let mut s = session(vec![
            frame(Kind::Canvas, 0., 0., 1000., 800.),
            frame(Kind::Image, 130., 90., 600., 400.),
        ]);
        let (d, g) = s.translate(V::new(108., 58.), min, max, None, true);
        assert_eq!(d, V::new(110., 60.));
        assert!(g.iter().any(|g| g.label.as_deref() == Some("Image · left")));
        assert!(g.iter().any(|g| g.label.as_deref() == Some("Image · top")));
        let (d, g) = s.translate(V::new(428., 348.), min, max, None, true);
        assert_eq!(d, V::new(430., 350.));
        assert!(g
            .iter()
            .all(|g| g.label.as_deref() == Some("Canvas · center")));
    }
    #[test]
    fn snap_latches_while_pointer_jitters_and_ctrl_releases_immediately() {
        let (min, max) = limits();
        let mut s = session(vec![frame(Kind::Canvas, 0., 0., 900., 600.)]);
        for x in [374., 387.9, 388.1, 387.8, 390.] {
            let (d, g) = s.translate(V::new(x, 211.), min, max, None, true);
            assert_eq!(d.x, 380.);
            assert!(!g.is_empty());
        }
        let (d, g) = s.translate(V::new(390., 211.), min, max, None, false);
        assert_eq!(d.x, 390.);
        assert!(g.is_empty());
        let (d, g) = s.translate(V::new(390., 211.), min, max, None, true);
        assert_eq!(d.x, 390.);
        assert!(g.is_empty());
        s.translate(V::new(375., 211.), min, max, None, true);
        assert_eq!(
            s.translate(V::new(393., 211.), min, max, None, true).0.x,
            393.
        );
    }
    #[test]
    fn clamping_and_axis_locks_cannot_leave_false_guides() {
        let (min, mut max) = limits();
        max.x = 378.;
        let mut s = session(vec![frame(Kind::Canvas, 0., 0., 900., 600.)]);
        let (d, g) = s.translate(V::new(377., 211.), min, max, None, true);
        assert_eq!(d.x, 377.);
        assert!(g.is_empty());
        let (_, g) = s.translate(V::new(381., 211.), min, max, None, true);
        assert!(g.is_empty());
        let (min, max) = limits();
        let (d, g) = s.translate(V::new(378., 0.), min, max, Some(1), true);
        assert_eq!(d, V::new(380., 0.));
        assert!(g.iter().all(|g| (g.start.x - g.end.x).abs() < EXACT));
    }
    #[test]
    fn guides_collect_all_objects_aligned_at_the_final_position() {
        let (min, max) = limits();
        let mut s = session(vec![
            frame(Kind::Object, 200., 100., 100., 50.),
            frame(Kind::Object, 200., 300., 100., 50.),
        ]);
        let (d, g) = s.translate(V::new(178., 128.), min, max, None, true);
        assert_eq!(d.x, 180.);
        assert!(g
            .iter()
            .any(|g| g.points.iter().any(|p| p.y == 100.) && g.points.iter().any(|p| p.y == 350.)));
    }
    #[test]
    fn equal_spacing_requires_visible_adjacent_objects_and_matching_row() {
        let (min, max) = limits();
        let mut s = Session::new(
            Rect::new(0., 0., 50., 40.),
            vec![
                frame(Kind::Object, 100., 0., 50., 40.),
                frame(Kind::Object, 300., 0., 50., 40.),
            ],
            Rect::new(0., 0., 900., 600.),
        );
        let (d, g) = s.translate(V::new(197., 0.), min, max, None, true);
        assert_eq!(d.x, 200.);
        assert_eq!(g.iter().filter(|g| g.gap).count(), 2);
        let (d, g) = s.translate(V::new(497., 0.), min, max, None, true);
        assert_eq!(d.x, 500.);
        assert_eq!(g.iter().filter(|g| g.gap).count(), 2);
        let (_, g) = s.translate(V::new(197., 100.), min, max, None, true);
        assert!(g.iter().all(|g| !g.gap));
    }
    #[test]
    fn screen_space_threshold_does_not_grow_with_canvas_zoom() {
        let (min, max) = limits();
        for scale in [0.5, 1., 2.] {
            let moving = Rect::new(20. * scale, 30. * scale, 100. * scale, 40. * scale);
            let mut s = Session::new(
                moving,
                vec![frame(Kind::Canvas, 0., 0., 900. * scale, 600. * scale)],
                Rect::new(0., 0., 1800., 1200.),
            );
            let expected = 380. * scale;
            assert_eq!(
                s.translate(V::new(expected - 7., 123.), min, max, None, true)
                    .0
                    .x,
                expected
            );
            assert_eq!(
                s.translate(V::new(expected - 13., 123.), min, max, None, true)
                    .0
                    .x,
                expected - 13.
            );
        }
    }
    #[test]
    fn resize_handles_snap_to_frame_guides_without_spacing_candidates() {
        let (min, max) = limits();
        let mut s = Session::new(
            Rect::new(200., 100., 0., 0.),
            vec![frame(Kind::Canvas, 0., 0., 900., 600.)],
            Rect::new(0., 0., 900., 600.),
        );
        let (d, g) = s.translate(V::new(97., 198.), min, max, None, true);
        assert_eq!(d, V::new(100., 200.));
        assert!(g.iter().any(|g| g.label.as_deref() == Some("Canvas · 1/3")));
        assert!(g.iter().all(|g| !g.gap));
    }
}
