//! Deterministic cursor reconstruction in recording pixels.
//!
//! A spring alone misses a click while it catches up with the pointer. This
//! planner searches bounded look-ahead and response speeds against the recorded
//! interaction positions, then stores an immutable path shared by preview and
//! export. Edit boundaries are independent trajectories. The implementation is
//! intentionally independent of any external renderer or application code.
use super::motion::{DampedSpring, SpringConstant};

const STEP: f64 = 0.001;
const INTERPOLATION_GAP: f64 = 0.04;
const MAX_LEAD: f64 = 0.6;
const APPROACH: f64 = 1.5;
const GROUP_GAP: f64 = 0.1;
const MAX_GROUP_DURATION: f64 = 2.0;
const MERGE_GAP: f64 = 0.4;
const MAX_MERGED_DURATION: f64 = 4.0;
const MAX_TIME_COMPRESSION: f64 = 1.4;
const SPEEDS: [f64; 16] = [1.0, 1.25, 1.6, 2.0, 2.5, 3.2, 4.0, 5.0, 6.4, 8.0, 10.0, 13.0, 16.0, 20.0, 25.0, 32.0];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CursorEventKind {
    Travel,
    Press,
    Release,
    Drag,
    Hover,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct CursorSample {
    pub time: f64,
    pub x: f64,
    pub y: f64,
    pub kind: CursorEventKind,
    pub artwork_change: bool,
}

impl CursorSample {
    fn tolerance(self) -> Option<f64> {
        match self.kind {
            CursorEventKind::Press | CursorEventKind::Release | CursorEventKind::Drag => Some(2.0),
            CursorEventKind::Travel if self.artwork_change => Some(6.0),
            CursorEventKind::Hover => Some(4.0),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct CursorState {
    pub x: f64,
    pub y: f64,
    pub velocity_x: f64,
    pub velocity_y: f64,
}

#[derive(Clone, Copy, Debug)]
struct Knot {
    time: f64,
    value: [f32; 4],
    cut: bool,
}

impl Knot {
    fn new(time: f64, state: CursorState, cut: bool) -> Self {
        Self {
            time,
            value: [
                state.x as f32,
                state.y as f32,
                state.velocity_x as f32,
                state.velocity_y as f32,
            ],
            cut,
        }
    }
    fn state(self) -> CursorState {
        CursorState {
            x: self.value[0] as f64,
            y: self.value[1] as f64,
            velocity_x: self.value[2] as f64,
            velocity_y: self.value[3] as f64,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(super) struct CursorPath {
    knots: Vec<Knot>,
    #[cfg(test)]
    evaluations: usize,
    #[cfg(test)]
    merged_groups: usize,
}

impl CursorPath {
    pub fn build(
        samples: &[CursorSample],
        duration: f64,
        spring: Option<SpringConstant>,
        cuts: &[f64],
    ) -> Self {
        if samples.is_empty() || !duration.is_finite() || duration <= 0.0 {
            return Self::default();
        }
        let mut boundaries = vec![0.0];
        boundaries.extend(
            cuts.iter()
                .copied()
                .filter(|v| v.is_finite() && *v > 0.0 && *v < duration),
        );
        boundaries.push(duration);
        boundaries.sort_by(f64::total_cmp);
        boundaries.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
        let mut result = Self::default();
        for window in boundaries.windows(2) {
            let (start, end) = (window[0], window[1]);
            let first_index = samples.partition_point(|p| p.time < start);
            let last_index =
                samples.partition_point(|p| p.time < end || (end == duration && p.time == end));
            let mut segment = Vec::with_capacity(last_index.saturating_sub(first_index) + 1);
            let seed = samples
                .get(first_index)
                .filter(|p| p.time == start)
                .copied()
                .or_else(|| first_index.checked_sub(1).map(|i| samples[i]))
                .unwrap_or(samples[0]);
            segment.push(CursorSample {
                time: start,
                ..seed
            });
            segment.extend(samples[first_index..last_index].iter().copied());
            let raw = RawPath::new(&segment, start, end);
            let mut initial = CursorState {
                x: seed.x,
                y: seed.y,
                ..CursorState::default()
            };
            if spring.is_none() {
                let seed = raw.at(start);
                initial.x = seed.x;
                initial.y = seed.y;
                result.push(start, initial, start > 0.0);
                let mut previous_time = start;
                for point in segment.iter().filter(|p| p.time > start) {
                    let state = CursorState {
                        x: point.x,
                        y: point.y,
                        ..CursorState::default()
                    };
                    result.push(
                        point.time,
                        state,
                        point.time - previous_time > INTERPOLATION_GAP,
                    );
                    previous_time = point.time;
                    initial = state;
                }
                result.push(end, initial, false);
                continue;
            }
            let (plans, evaluations, merged_groups) = if let Some(spring) = spring {
                plan(&raw, spring, initial)
            } else {
                (Vec::new(), 0, 0)
            };
            #[cfg(test)]
            {
                result.evaluations += evaluations;
                result.merged_groups += merged_groups;
            }
            #[cfg(not(test))]
            let _ = (evaluations, merged_groups);
            let segment_start = result.knots.len();
            result.push(start, initial, start > 0.0);
            let mut time = start;
            while time < end - 1e-10 {
                let next = (time + STEP).min(end);
                initial = simulate(&raw, spring, &plans, time, next, initial);
                result.push(next, initial, false);
                time = next;
            }
            if spring.is_some() {
                result.constrain(segment_start, &raw);
            }
        }
        result
    }

    /// The bounded spring search may not satisfy a cold-start drag or a
    /// recording that ends during movement: neither has room for anticipation.
    /// In that case a local C1 correction enforces recorded interactions. Its
    /// support ends 80ms outside the interactions and never crosses an edit.
    fn constrain(&mut self, first_knot: usize, raw: &RawPath<'_>) {
        let points: Vec<_> = raw
            .samples
            .iter()
            .copied()
            .filter(|p| p.tolerance().is_some())
            .collect();
        if !points.iter().any(|p| {
            let state = self.at(p.time);
            (state.x - p.x).hypot(state.y - p.y) > p.tolerance().unwrap() * 0.9
        }) {
            return;
        }
        let mut keys: Vec<(f64, [f64; 2])> = vec![(raw.start, [0.0; 2])];
        for point in points {
            let previous = keys.last().unwrap().0;
            if point.time - previous > 0.16 {
                keys.push(((previous + 0.08).min(raw.end), [0.0; 2]));
                keys.push(((point.time - 0.08).max(raw.start), [0.0; 2]));
            }
            let state = self.at(point.time);
            let value = [point.x - state.x, point.y - state.y];
            if (point.time - keys.last().unwrap().0).abs() < 1e-9 {
                keys.last_mut().unwrap().1 = value;
            } else {
                keys.push((point.time, value));
            }
        }
        let last = keys.last().unwrap().0;
        if last < raw.end {
            keys.push(((last + 0.08).min(raw.end), [0.0; 2]));
        }
        let mut slopes = vec![[0.0; 2]; keys.len()];
        for i in 1..keys.len().saturating_sub(1) {
            for axis in 0..2 {
                let a = (keys[i].1[axis] - keys[i - 1].1[axis]) / (keys[i].0 - keys[i - 1].0);
                let b = (keys[i + 1].1[axis] - keys[i].1[axis]) / (keys[i + 1].0 - keys[i].0);
                // Monotone Hermite slopes cannot introduce correction overshoot.
                if a * b > 0.0 {
                    slopes[i][axis] = 2.0 * a * b / (a + b);
                }
            }
        }
        let mut index = 0;
        for knot in &mut self.knots[first_knot..] {
            while index + 1 < keys.len() && keys[index + 1].0 < knot.time {
                index += 1;
            }
            let Some(right) = keys.get(index + 1) else {
                continue;
            };
            let left = keys[index];
            let dt = right.0 - left.0;
            if dt <= 0.0 {
                continue;
            }
            let t = ((knot.time - left.0) / dt).clamp(0.0, 1.0);
            for axis in 0..2 {
                let (a, b) = (left.1[axis], right.1[axis]);
                let (u, v) = (slopes[index][axis] * dt, slopes[index + 1][axis] * dt);
                let correction = (2.0 * t * t * t - 3.0 * t * t + 1.0) * a
                    + (t * t * t - 2.0 * t * t + t) * u
                    + (-2.0 * t * t * t + 3.0 * t * t) * b
                    + (t * t * t - t * t) * v;
                let velocity = ((6.0 * t * t - 6.0 * t) * a
                    + (3.0 * t * t - 4.0 * t + 1.0) * u
                    + (-6.0 * t * t + 6.0 * t) * b
                    + (3.0 * t * t - 2.0 * t) * v)
                    / dt;
                knot.value[axis] += correction as f32;
                knot.value[axis + 2] += velocity as f32;
            }
        }
    }

    fn push(&mut self, time: f64, state: CursorState, cut: bool) {
        let knot = Knot::new(time, state, cut);
        // Preserve both ends of a rest, so interpolation cannot start the next
        // movement early. A long idle recording costs two knots, not 1,000/s.
        let n = self.knots.len();
        if n >= 2
            && !cut
            && !self.knots[n - 1].cut
            && self.knots[n - 1].value == knot.value
            && self.knots[n - 2].value == knot.value
        {
            self.knots[n - 1] = knot;
        } else {
            self.knots.push(knot);
        }
    }

    pub fn at(&self, time: f64) -> CursorState {
        let Some(first) = self.knots.first() else {
            return CursorState::default();
        };
        if !time.is_finite() || time < first.time {
            return first.state();
        }
        let upper = self.knots.partition_point(|k| k.time <= time);
        let left = &self.knots[upper.saturating_sub(1)];
        let Some(right) = self.knots.get(upper) else {
            return left.state();
        };
        if right.cut || right.time <= left.time {
            return left.state();
        }
        let t = ((time - left.time) / (right.time - left.time)).clamp(0.0, 1.0);
        let mut v = [0.0; 4];
        for (index, value) in v.iter_mut().enumerate() {
            *value = left.value[index] as f64
                + (right.value[index] as f64 - left.value[index] as f64) * t;
        }
        CursorState {
            x: v[0],
            y: v[1],
            velocity_x: v[2],
            velocity_y: v[3],
        }
    }
}

#[derive(Clone, Copy)]
struct RestSpan { start: f64, end: f64, x: f64, y: f64 }

struct RawPath<'a> {
    samples: &'a [CursorSample],
    start: f64,
    end: f64,
    release_times: Vec<f64>,
    press_indices: Vec<usize>,
    rests: Vec<RestSpan>,
    held: Vec<bool>,
}
impl<'a> RawPath<'a> {
    fn new(samples: &'a [CursorSample], start: f64, end: f64) -> Self {
        let mut rests: Vec<RestSpan> = Vec::new();
        for (index, sample) in samples.iter().enumerate() {
            let next = samples.get(index + 1);
            let rest_end = next.map_or(end, |next| next.time).min(end);
            let stationary = next.is_none_or(|next| {
                next.time - sample.time > INTERPOLATION_GAP
                    || (next.x - sample.x).hypot(next.y - sample.y) < 0.01
            });
            let rest_start = sample.time.max(start);
            if stationary && rest_end > rest_start {
                if let Some(previous) = rests.last_mut().filter(|previous| {
                    (previous.end - rest_start).abs() < 1e-9
                        && (previous.x - sample.x).hypot(previous.y - sample.y) < 0.01
                }) { previous.end = rest_end; }
                else { rests.push(RestSpan { start: rest_start, end: rest_end, x: sample.x, y: sample.y }); }
            }
        }
        let mut pressed = false;
        let held = samples.iter().map(|sample| {
            match sample.kind {
                CursorEventKind::Press | CursorEventKind::Drag => pressed = true,
                CursorEventKind::Release | CursorEventKind::Travel => pressed = false,
                CursorEventKind::Hover => {}
            }
            pressed
        }).collect();
        Self {
            samples,
            start,
            end,
            release_times: samples
                .iter()
                .filter(|p| p.kind == CursorEventKind::Release)
                .map(|p| p.time)
                .collect(),
            press_indices: samples
                .iter()
                .enumerate()
                .filter_map(|(index, p)| (p.kind == CursorEventKind::Press).then_some(index))
                .collect(),
            rests,
            held,
        }
    }
    fn at(&self, time: f64) -> CursorSample {
        let upper = self.samples.partition_point(|p| p.time <= time);
        let left = self.samples[upper.saturating_sub(1)];
        let Some(right) = self.samples.get(upper) else {
            return left;
        };
        let gap = right.time - left.time;
        if time < left.time || gap <= 0.0 || gap > INTERPOLATION_GAP {
            return left;
        }
        let fraction = ((time - left.time) / gap).clamp(0.0, 1.0);
        CursorSample {
            x: left.x + (right.x - left.x) * fraction,
            y: left.y + (right.y - left.y) * fraction,
            ..left
        }
    }
    fn target(&self, time: f64, lead: f64) -> CursorSample {
        let mut target_time = (time + lead).min(self.end);
        // Look-ahead can track a drag, but must never chase a different click
        // beyond its release. This also protects stationary held objects.
        let index = self.samples.partition_point(|sample| sample.time <= time).saturating_sub(1);
        if self.held[index] {
            if let Some(release) = self
                .release_times
                .get(self.release_times.partition_point(|v| *v < time))
            {
                target_time = target_time.min(*release);
            } else if let Some(next_press) = self.press_indices.get(
                self.press_indices
                    .partition_point(|index| self.samples[*index].time <= time),
            ) {
                // Missing mouse-up metadata must not send a drag toward a
                // later click. Remaining drag samples are still safe to lead.
                let previous = self.samples[next_press.saturating_sub(1)].time;
                target_time = target_time.min(previous.max(time));
            }
        }
        self.at(target_time.max(self.start))
    }
}

#[derive(Clone, Copy, Debug)]
struct ResponsePlan {
    approach: f64,
    first: f64,
    last: f64,
    lead: f64,
    speed: f64,
    lead_start: f64,
    rest_jump: f64,
    release_at: f64,
}
impl ResponsePlan {
    fn new(raw: &RawPath<'_>, approach: f64, first: f64, last: f64, lead: f64, speed: f64) -> Self {
        let compression = MAX_TIME_COMPRESSION - 1.0;
        // Spend available stillness first. Changing the time offset while
        // both endpoints lie in the same rest leaves the target unchanged.
        // Otherwise the clock advances at no more than 1.4x until the event.
        let rest = raw.rests.iter().rev().filter_map(|rest| {
            let begin = rest.start.max(approach);
            let end = rest.end.min(first);
            let jump = lead.min((end - begin - STEP).max(0.0));
            (jump > 0.0 && first - begin >= (lead - jump) / compression)
                .then_some((begin, jump))
        }).next();
        let (lead_start, rest_jump, lead) = match rest {
            Some((begin, jump)) => (begin, jump, lead),
            None => (approach, 0.0, lead.min((first - approach) * compression)),
        };
        let release_at = raw.rests.iter().filter(|rest| rest.end > last).find_map(|rest| {
            let begin = rest.start.max(last);
            let remaining = (lead - (begin - last) * 0.35).max(0.0);
            (begin + remaining < rest.end).then_some(begin)
        }).unwrap_or(f64::INFINITY);
        Self { approach, first, last, lead, speed, lead_start, rest_jump, release_at }
    }

    fn controls(self, time: f64) -> (f64, f64) {
        if time < self.approach || time > self.last + self.lead / 0.35 + 0.3 {
            return (0.0, 1.0);
        }
        let lead = if time >= self.release_at {
            0.0
        } else if time < self.lead_start {
            0.0
        } else if time < self.first {
            let t = ((time - self.lead_start) / (self.first - self.lead_start).max(1e-9)).clamp(0.0, 1.0);
            self.rest_jump + (self.lead - self.rest_jump) * t
        } else {
            (self.lead - (time - self.last).max(0.0) * 0.35).max(0.0)
        };
        let intensity = if time < self.first {
            smooth(
                ((time - (self.first - 0.25).max(self.approach))
                    / (self.first - (self.first - 0.25).max(self.approach)).max(1e-9))
                .clamp(0.0, 1.0),
            )
        } else {
            1.0 - smooth(((time - self.last) / 0.3).clamp(0.0, 1.0))
        };
        (lead, (self.speed.ln() * intensity).exp())
    }
}
fn controls(plans: &[ResponsePlan], time: f64) -> (f64, f64) {
    let upper = plans.partition_point(|p| p.approach <= time);
    let mut lead: f64 = 0.0;
    let mut speed: f64 = 1.0;
    for p in plans[..upper].iter().rev() {
        if time - p.last > MAX_LEAD / 0.35 + 0.3 {
            break;
        }
        let value = p.controls(time);
        lead = lead.max(value.0);
        speed = speed.max(value.1);
    }
    (lead, speed)
}
fn smooth(t: f64) -> f64 {
    t * t * (3.0 - 2.0 * t)
}

/// Stable, bounded substeps preserve momentum while increasing damping near a
/// moving target that would otherwise be overshot. Also used by cursor effects.
pub(super) fn advance_scalar(
    spring: &mut DampedSpring,
    target: f64,
    constant: SpringConstant,
    dt: f64,
) {
    if constant.inertia <= 0.0 {
        spring.snap(target);
        return;
    }
    let omega = (constant.tension / constant.inertia).sqrt();
    let substeps = (dt / (0.25 / omega.max(1.0)).min(STEP)).ceil().max(1.0) as usize;
    let step = dt / substeps as f64;
    for _ in 0..substeps {
        let distance = target - spring.position;
        let friction =
            if spring.velocity * distance > 0.0 && spring.velocity.abs() > distance.abs() * omega {
                constant
                    .friction
                    .max(2.0 * (constant.inertia * constant.tension).sqrt())
            } else {
                constant.friction
            };
        spring.step(
            target,
            SpringConstant {
                friction,
                ..constant
            },
            step,
        );
    }
    if spring.velocity.abs() < 0.000_01 && (spring.position - target).abs() < 0.000_01 {
        spring.snap(target);
    }
}
fn simulate(
    raw: &RawPath<'_>,
    spring: Option<SpringConstant>,
    plans: &[ResponsePlan],
    from: f64,
    to: f64,
    state: CursorState,
) -> CursorState {
    let mut x = DampedSpring {
        position: state.x,
        velocity: state.velocity_x,
    };
    let mut y = DampedSpring {
        position: state.y,
        velocity: state.velocity_y,
    };
    let mut time = from;
    while time < to - 1e-10 {
        let next = (time + STEP).min(to);
        let (lead, speed) = controls(plans, next);
        let target = raw.target(next, lead);
        if let Some(base) = spring {
            let scaled = SpringConstant {
                tension: base.tension * speed * speed,
                friction: base.friction * speed,
                inertia: base.inertia,
            };
            advance_scalar(&mut x, target.x, scaled, next - time);
            advance_scalar(&mut y, target.y, scaled, next - time);
        } else {
            x.snap(target.x);
            y.snap(target.y);
        }
        time = next;
    }
    CursorState {
        x: x.position,
        y: y.position,
        velocity_x: x.velocity,
        velocity_y: y.velocity,
    }
}

fn plan(
    raw: &RawPath<'_>,
    spring: SpringConstant,
    initial: CursorState,
) -> (Vec<ResponsePlan>, usize, usize) {
    let constraints: Vec<_> = raw
        .samples
        .iter()
        .copied()
        .filter(|p| p.tolerance().is_some())
        .collect();
    let mut groups: Vec<Vec<CursorSample>> = Vec::new();
    for point in constraints {
        if let Some(group) = groups.last_mut() {
            if point.time - group.last().unwrap().time <= GROUP_GAP
                && point.time - group[0].time <= MAX_GROUP_DURATION
            {
                group.push(point);
                continue;
            }
        }
        groups.push(vec![point]);
    }
    struct SolvedGroup {
        points: Vec<CursorSample>, before_time: f64, before: CursorState,
        prefix: usize, error: f64, end: CursorState, merges: usize,
    }
    let mut solved: Vec<SolvedGroup> = Vec::new();
    let mut plans = Vec::new();
    let mut evaluations = 0;
    let mut merged_groups = 0;
    for mut group in groups {
        let mut before_time = solved.last().map_or(raw.start, |g| g.points.last().unwrap().time);
        let mut before = solved.last().map_or(initial, |g| g.end);
        let mut prefix = plans.len();
        let (mut chosen, mut error, mut end, work) = solve_group(raw, spring, &mut plans, before_time, before, &group);
        evaluations += work;
        let mut merges = 0;
        // Revisit difficult neighboring groups jointly. Bound the repair
        // horizon so a dense recording cannot become a quadratic global solve.
        while error > 1.0 || solved.last().is_some_and(|previous| previous.error > 1.0) {
            let Some(previous) = solved.last().filter(|previous| {
                group[0].time - previous.points.last().unwrap().time <= MERGE_GAP
                    && group.last().unwrap().time - previous.points[0].time <= MAX_MERGED_DURATION
                    && previous.merges + merges < 3
            }) else { break; };
            let mut together = previous.points.clone();
            together.extend_from_slice(&group);
            let mut earlier_plans = plans[..previous.prefix].to_vec();
            let (joint, joint_error, joint_end, work) = solve_group(
                raw, spring, &mut earlier_plans, previous.before_time, previous.before, &together);
            evaluations += work;
            if joint_error > error.max(previous.error) + 1e-9 { break; }
            before_time = previous.before_time;
            before = previous.before;
            prefix = previous.prefix;
            merges += previous.merges + 1;
            merged_groups += 1;
            group = together;
            chosen = joint;
            error = joint_error;
            end = joint_end;
            plans.truncate(prefix);
            solved.pop();
        }
        if let Some(plan) = chosen { plans.push(plan); }
        solved.push(SolvedGroup { points: group, before_time, before, prefix, error, end, merges });
    }
    (plans, evaluations, merged_groups)
}

fn solve_group(
    raw: &RawPath<'_>, spring: SpringConstant, plans: &mut Vec<ResponsePlan>,
    before_time: f64, before: CursorState, group: &[CursorSample],
) -> (Option<ResponsePlan>, f64, CursorState, usize) {
    let first = group[0].time;
    let last = group.last().unwrap().time;
    let approach = (first - APPROACH).max(before_time);
    let state = simulate(raw, Some(spring), plans, before_time, approach, before);
    let (mut best_error, mut best_state) = group_error(raw, spring, plans, approach, state, group);
    let mut evaluations = 1;
    let mut chosen = None;
    if best_error > 0.75 {
        'search: for speed in SPEEDS {
            let lag = (spring.friction / spring.tension / speed).min(MAX_LEAD);
            let mut leads = vec![0.0, lag * 0.5, lag - STEP, lag - STEP * 0.5,
                lag, lag * 1.5, lag * 2.0, 0.3, MAX_LEAD];
            leads.iter_mut().for_each(|v| *v = v.clamp(0.0, MAX_LEAD));
            leads.sort_by(f64::total_cmp);
            leads.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
            let mut previous_lead = None;
            for lead in leads {
                let candidate = ResponsePlan::new(raw, approach, first, last, lead, speed);
                if previous_lead == Some((candidate.lead, candidate.lead_start, candidate.rest_jump)) { continue; }
                previous_lead = Some((candidate.lead, candidate.lead_start, candidate.rest_jump));
                plans.push(candidate);
                let (error, end) = group_error(raw, spring, plans, approach, state, group);
                plans.pop();
                evaluations += 1;
                if error < best_error {
                    best_error = error;
                    chosen = Some(candidate);
                    best_state = end;
                }
                if best_error <= 0.75 { break 'search; }
            }
        }
    }
    (chosen, best_error, best_state, evaluations)
}

fn group_error(
    raw: &RawPath<'_>,
    spring: SpringConstant,
    plans: &[ResponsePlan],
    mut time: f64,
    mut state: CursorState,
    group: &[CursorSample],
) -> (f64, CursorState) {
    let mut error: f64 = 0.0;
    for point in group {
        state = simulate(raw, Some(spring), plans, time, point.time, state);
        time = point.time;
        error = error
            .max((state.x - point.x).hypot(state.y - point.y) / point.tolerance().unwrap_or(2.0));
    }
    (error, state)
}

#[cfg(test)]
mod tests {
    use super::*;
    const SPRING: SpringConstant = SpringConstant {
        tension: 470.0,
        friction: 70.0,
        inertia: 3.0,
    };
    fn sample(time: f64, x: f64, kind: CursorEventKind) -> CursorSample {
        CursorSample {
            time,
            x,
            y: 300.0,
            kind,
            artwork_change: false,
        }
    }
    #[test]
    fn grouped_clicks_and_drag_are_accurate() {
        let mut samples = vec![sample(0.0, 100.0, CursorEventKind::Travel)];
        for i in 0..=100 {
            samples.push(sample(
                0.3 + i as f64 * 0.01,
                100.0 + i as f64 * 4.0,
                if i < 50 {
                    CursorEventKind::Travel
                } else {
                    CursorEventKind::Drag
                },
            ));
        }
        samples.push(sample(1.3, 500.0, CursorEventKind::Release));
        let path = CursorPath::build(&samples, 2.0, Some(SPRING), &[]);
        for point in samples.iter().filter(|p| p.tolerance().is_some()) {
            let actual = path.at(point.time);
            assert!(
                (actual.x - point.x).abs() < 2.0,
                "{}: {} vs {}",
                point.time,
                actual.x,
                point.x
            );
        }
    }
    #[test]
    fn hover_has_its_own_accuracy_constraint_and_does_not_release_a_drag() {
        let samples = [
            sample(0.0, 100.0, CursorEventKind::Travel),
            sample(0.2, 200.0, CursorEventKind::Press),
            sample(0.3, 300.0, CursorEventKind::Hover),
            sample(0.4, 350.0, CursorEventKind::Release),
            sample(0.5, 900.0, CursorEventKind::Press),
        ];
        let raw = RawPath::new(&samples, 0.0, 1.0);
        assert_eq!(samples[2].tolerance(), Some(4.0));
        assert_eq!(raw.target(0.3, 0.6).x, 350.0, "hover must not let anticipation cross release");
        let path = CursorPath::build(&samples, 1.0, Some(SPRING), &[]);
        assert!((path.at(0.3).x - 300.0).abs() < 4.0);
    }

    #[test]
    fn anticipation_uses_rest_and_limits_moving_time_compression() {
        let mut samples = vec![sample(0.0, 100.0, CursorEventKind::Travel)];
        samples.extend((0..=50).map(|i| sample(0.5 + i as f64 * 0.01,
            100.0 + i as f64 * 8.0, CursorEventKind::Travel)));
        let raw = RawPath::new(&samples, 0.0, 2.0);
        let plan = ResponsePlan::new(&raw, 0.0, 1.0, 1.0, 0.2, 2.0);
        assert!(plan.rest_jump > 0.19, "available rest must absorb anticipation");
        let before = raw.target(0.0, 0.0);
        let after = raw.target(0.0, plan.controls(0.0).0);
        assert_eq!((before.x, before.y), (after.x, after.y), "rest lead changes must not jump the target");
        assert_eq!(plan.controls(1.0).0, 0.0, "trailing rest can remove lead immediately");

        let moving: Vec<_> = (0..=100).map(|i|
            sample(i as f64 * 0.01, i as f64 * 10.0, CursorEventKind::Travel)).collect();
        let raw = RawPath::new(&moving, 0.0, 1.0);
        let plan = ResponsePlan::new(&raw, 0.0, 1.0, 1.0, 0.6, 2.0);
        assert_eq!(plan.rest_jump, 0.0);
        for index in 0..999 {
            let t = index as f64 * STEP;
            let delta = (t + STEP + plan.controls(t + STEP).0) - (t + plan.controls(t).0);
            assert!(delta <= STEP * MAX_TIME_COMPRESSION + 1e-10, "moving time was compressed too far: {delta}");
        }
    }

    #[test]
    fn difficult_nearby_groups_remerge_without_crossing_a_cut() {
        let samples = [
            sample(0.0, 100.0, CursorEventKind::Travel),
            sample(0.01, 1900.0, CursorEventKind::Press),
            sample(0.02, 1900.0, CursorEventKind::Release),
            sample(0.22, 100.0, CursorEventKind::Press),
            sample(0.23, 100.0, CursorEventKind::Release),
        ];
        let path = CursorPath::build(&samples, 1.0, Some(SPRING), &[]);
        assert!(path.merged_groups > 0, "cold-start constraint groups should get a joint retry");
        for point in &samples[1..] {
            assert!((path.at(point.time).x - point.x).abs() <= 2.0);
        }
        let cut = CursorPath::build(&samples, 1.0, Some(SPRING), &[0.1]);
        assert_eq!(cut.merged_groups, 0, "constraint repair cannot cross an edit boundary");
    }
    #[test]
    fn cut_prevents_anticipation_and_clears_velocity() {
        let samples = [
            sample(0.0, 100.0, CursorEventKind::Travel),
            sample(1.0, 900.0, CursorEventKind::Travel),
            sample(1.1, 900.0, CursorEventKind::Press),
        ];
        let path = CursorPath::build(&samples, 2.0, Some(SPRING), &[1.0]);
        assert_eq!(path.at(0.99).x, 100.0);
        assert_eq!(path.at(1.0).x, 900.0);
        assert_eq!(path.at(1.0).velocity_x, 0.0);
    }
    #[test]
    fn none_tracks_exactly_and_seeks_are_deterministic() {
        let samples = [
            sample(0.0, 100.0, CursorEventKind::Travel),
            sample(0.02, 200.0, CursorEventKind::Travel),
            sample(0.04, 300.0, CursorEventKind::Travel),
        ];
        let path = CursorPath::build(&samples, 1.0, None, &[]);
        assert!((path.at(0.01).x - 150.0).abs() < 0.001);
        let expected = path.at(0.015).x;
        path.at(0.9);
        path.at(0.0);
        assert_eq!(path.at(0.015).x, expected);
    }
    #[test]
    fn none_preserves_off_grid_event_times_and_discontinuities() {
        let samples = [
            sample(0.0, 100.0, CursorEventKind::Travel),
            sample(0.5037, 500.0, CursorEventKind::Press),
            sample(0.5237, 600.0, CursorEventKind::Release),
        ];
        let path = CursorPath::build(&samples, 1.0, None, &[]);
        assert_eq!(path.at(0.5036).x, 100.0);
        assert_eq!(path.at(0.5037).x, 500.0);
        assert!((path.at(0.5137).x - 550.0).abs() < 1e-6);
        assert_eq!(path.at(0.5237).x, 600.0);
    }
    #[test]
    fn long_idle_is_compact_and_search_is_bounded() {
        let samples = [
            sample(0.0, 100.0, CursorEventKind::Travel),
            sample(60.0, 100.0, CursorEventKind::Press),
        ];
        let path = CursorPath::build(&samples, 120.0, Some(SPRING), &[]);
        assert!(path.knots.len() < 10);
        assert!(path.evaluations <= 1 + SPEEDS.len() * 9);
    }
    #[test]
    fn fast_response_remains_finite_and_bounded() {
        let samples = [
            sample(0.0, 100.0, CursorEventKind::Travel),
            sample(0.5, 1900.0, CursorEventKind::Press),
            sample(0.52, 1900.0, CursorEventKind::Release),
        ];
        let path = CursorPath::build(&samples, 1.5, Some(SPRING), &[]);
        for i in 0..1500 {
            let s = path.at(i as f64 / 1000.0);
            assert!(s.x.is_finite() && s.velocity_x.is_finite());
            assert!((90.0..=1910.0).contains(&s.x));
        }
        assert!((path.at(0.5).x - 1900.0).abs() < 2.0);
    }
    #[test]
    fn dense_long_drag_has_bounded_work_and_tracks_without_release_metadata() {
        let samples: Vec<_> = (0..=6000)
            .map(|i| {
                let time = i as f64 / 100.0;
                sample(
                    time,
                    1000.0 + 600.0 * (time * 2.0).sin(),
                    if i == 0 {
                        CursorEventKind::Travel
                    } else {
                        CursorEventKind::Drag
                    },
                )
            })
            .collect();
        let path = CursorPath::build(&samples, 60.0, Some(SPRING), &[]);
        assert!(
            path.evaluations < 3500,
            "unbounded candidate count: {}",
            path.evaluations
        );
        for point in samples.iter().step_by(17).skip(1) {
            assert!(
                (path.at(point.time).x - point.x).abs() < 2.0,
                "drag miss at {}: {}",
                point.time,
                path.at(point.time).x - point.x
            );
        }
        let a = path.at(21.321).x;
        path.at(59.0);
        path.at(0.1);
        assert_eq!(path.at(21.321).x, a);
    }
}
