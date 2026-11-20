//! Setting a ragdoll's motors with no training: every `every` seconds,
//! try `samples` plans `horizon` seconds ahead in copies of the world,
//! and keep the one that leaves the body best off.
//!
//! A plan is three leans, each a turn about a level axis: one made at
//! the ankles, one at the hips, one at the spine; a swing of each upper
//! arm, the same way (ten numbers: a search over every joint's every axis
//! would need far more tries than a shot can pay for); and, when its
//! weight is headed past its feet and no step is under way, perhaps a
//! step: the foot behind, to where it is falling to or past it. The
//! first try is always the clip as it is, the second the last choice; the
//! rest are the last choice moved at random within `spread` degrees
//! (arms twice that), a third of them with a step. A try is scored at its
//! end: fallen is worst; then how far its weight is headed past its feet,
//! how far its trunk is over, how much height it lost, and a little for
//! how far the targets are from the clip and for a step.
//!
//! The tries are independent: they run on every core, and the choice is
//! the same however many there are (the plans are drawn first, the best
//! is the first of the least cost).

use crate::behaviour::Stance;
use crate::world::Rng;
use crate::Take;
use lahza_motion_core::sim::{quat_mul, RagdollInput, SimInput};

pub struct Search {
    pub samples: u32,
    pub horizon: f64,
    pub every: f64,
    /// Radians.
    pub spread: f64,
    next: f64,
    /// The plan chosen last: [ankles, hips, spine, left arm, right arm] ×
    /// [about x, about z].
    best: [f64; 10],
    stand: Option<Stance>,
    decisions: u64,
    /// Steps taken in tries, and the seconds they took.
    pub tried_steps: u64,
    pub seconds: f64,
    /// How many tries run at once: the cores there are, or
    /// `LAHZA_SIM_THREADS`. The take is the same whatever it is.
    pub threads: usize,
}

fn turn(axis: [f64; 3], angle: f64) -> [f64; 4] {
    let (s, c) = (libm::sin(angle / 2.0), libm::cos(angle / 2.0));
    [axis[0] * s, axis[1] * s, axis[2] * s, c]
}

impl Search {
    pub fn new(samples: u32, horizon: f64, every: f64, spread_degrees: f64) -> Search {
        Search { samples: samples.max(1), horizon: horizon.max(0.0), every: every.max(1e-3), spread: spread_degrees.to_radians(), next: f64::NEG_INFINITY, best: [0.0; 10], stand: None, decisions: 0, tried_steps: 0, seconds: 0.0, threads: std::env::var("LAHZA_SIM_THREADS").ok().and_then(|v| v.parse().ok()).filter(|n| *n > 0).unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| n.get())) }
    }

    /// The leans as turns on top of the clip, per bone, in each bone's
    /// parent's frame as it stands now.
    fn turns(take: &Take, rig: usize, r: &RagdollInput, feet: &[usize], leans: &[f64; 10]) -> Vec<Option<[f64; 4]>> {
        let mut out = vec![None; r.bones.len()];
        let conj = |q: [f64; 4]| [-q[0], -q[1], -q[2], q[3]];
        // A turn about the scene's x and z (level axes), seen from a bone's parent.
        let lean = |bone: usize, about_x: f64, about_z: f64| -> Option<[f64; 4]> {
            let pq = take.world.pose(r.bones[r.bones[bone].parent?].body).q;
            let world = quat_mul(turn([1.0, 0.0, 0.0], about_x), turn([0.0, 0.0, 1.0], about_z));
            Some(quat_mul(conj(pq), quat_mul(world, pq)))
        };
        for &f in feet {
            out[f] = lean(f, leans[0], leans[1]);
            // The hip above this foot: the bone under the root on its chain.
            let mut top = f;
            while let Some(p) = r.bones[top].parent {
                if r.bones[p].parent.is_none() {
                    break;
                }
                top = p;
            }
            if top != f {
                out[top] = lean(top, leans[2], leans[3]);
            }
        }
        // The spine: the root's child that no foot hangs from.
        let legs: Vec<usize> = out.iter().enumerate().filter(|(_, t)| t.is_some()).map(|(k, _)| k).collect();
        if let Some(spine) = (0..r.bones.len()).find(|&k| r.bones[k].parent == Some(0) && !legs.contains(&k)) {
            out[spine] = lean(spine, leans[4], leans[5]);
        }
        for (k, upper) in take.brains[rig].upper_arms().into_iter().take(2).enumerate() {
            out[upper] = lean(upper, leans[6 + 2 * k], leans[7 + 2 * k]);
        }
        out
    }

    /// The turns to make on top of the clip from time `t` (sample `k`) on,
    /// for ragdoll `rig`, and a step to begin now if the plan has one;
    /// chosen anew when `every` has passed.
    pub fn decide(&mut self, take: &Take, input: &SimInput, rig: usize, k: u32, t: f64) -> (Vec<Option<[f64; 4]>>, Option<Stride>) {
        let r = &input.ragdolls[rig];
        let feet = take.brains[rig].feet().to_vec();
        if t < self.next || !take.world.released[r.bones[0].body] {
            return (Self::turns(take, rig, r, &feet, &self.best), None);
        }
        let began = std::time::Instant::now();
        self.next = t + self.every;
        self.decisions += 1;
        let Some(now) = Stance::of(&take.world, input, r, &feet) else { return (vec![None; r.bones.len()], None) };
        let stand = *self.stand.get_or_insert(now);
        let n = input.substeps as u64;
        let steps = (self.horizon * input.rate * n as f64).round().max(1.0) as u64;
        let mut rng = Rng(input.seed ^ self.decisions.wrapping_mul(0xd6e8feb86659fd93) ^ (rig as u64) << 48);
        // A step is worth trying when its weight is headed off its feet
        // by a twentieth of its height, and it is free to take one.
        let l = now.heading;
        let may_step = l > 0.05 * stand.height && take.brains[rig].can_step(t);
        let foot_at = |f: usize| crate::behaviour::bone_origin(&take.world, r, f);
        // The plans, drawn before any is tried.
        let plans: Vec<([f64; 10], Option<Stride>)> = (0..self.samples)
            .map(|c| match c {
                0 => ([0.0; 10], None),
                1 => (self.best, None),
                _ => {
                    let leans = std::array::from_fn(|a| if a < 6 { (self.best[a] + self.spread * rng.signed()).clamp(-0.6, 0.6) } else { (self.best[a] + 2.0 * self.spread * rng.signed()).clamp(-1.4, 1.4) });
                    let (past, wanted) = (1.0 + 0.8 * (rng.signed() + 1.0) / 2.0, c % 3 == 2);
                    let stride = (may_step && wanted).then(|| {
                        // The foot behind, to where it is falling to and up to 1.8 times as far.
                        let way = now.toward.map(|v| v / l);
                        let behind = |f: &usize| (0..3).map(|a| (foot_at(*f)[a] - now.support[a]) * way[a]).sum::<f64>();
                        let foot = *feet.iter().min_by(|a, b| behind(a).total_cmp(&behind(b))).unwrap();
                        let from = foot_at(foot);
                        let mut by: [f64; 3] = std::array::from_fn(|a| now.support[a] + now.toward[a] * past - from[a]);
                        // (On the level, half its height at most.)
                        let up = input.gravity.map(|g| -g / (input.gravity.iter().map(|v| v * v).sum::<f64>().sqrt().max(1e-9)));
                        let rise: f64 = (0..3).map(|a| by[a] * up[a]).sum();
                        by = std::array::from_fn(|a| by[a] - up[a] * rise);
                        let far = by.iter().map(|v| v * v).sum::<f64>().sqrt();
                        if far > 0.5 * stand.height {
                            by = by.map(|v| v * 0.5 * stand.height / far);
                        }
                        Stride { foot, from, to: std::array::from_fn(|a| from[a] + by[a]), lift: 0.12 * stand.height }
                    });
                    (leans, stride)
                }
            })
            .collect();
        // Each tried in a copy of the world, on every core.
        let try_one = |(leans, stride): &([f64; 10], Option<Stride>)| -> (f64, u64) {
            let mut trial = take.fork();
            trial.turns[rig] = Some(Self::turns(take, rig, r, &feet, leans));
            if let Some(s) = stride {
                trial.brains[rig].order_step(input, r, s.foot, s.from, s.to, s.lift, t);
            }
            // From this step on, through the frames' ends as the bake goes.
            let (mut kk, mut s, mut tried) = (k, 0u64, 0u64);
            for _ in 0..steps {
                if kk >= input.samples {
                    break;
                }
                trial.step(input, kk, s);
                s += 1;
                if s == n {
                    (kk, s) = (kk + 1, 0);
                    trial.release_due(input, kk);
                }
                tried += 1;
            }
            let cost = match Stance::of(&trial.world, input, r, &feet) {
                Some(end) => {
                    let fallen = end.height < 0.55 * stand.height || end.tilt(&stand) > 1.15;
                    let effort: f64 = leans[..6].iter().map(|v| v * v).sum::<f64>().sqrt() + 0.25 * leans[6..].iter().map(|v| v * v).sum::<f64>().sqrt();
                    (if fallen { 100.0 } else { 0.0 }) + 4.0 * end.heading / stand.height + 2.0 * end.tilt(&stand) + 3.0 * ((stand.height - end.height) / stand.height).max(0.0) + 0.3 * effort + if stride.is_some() { 0.1 } else { 0.0 }
                }
                None => f64::INFINITY,
            };
            (cost, tried)
        };
        let threads = self.threads.min(plans.len()).max(1);
        let mut results: Vec<(f64, u64)> = vec![(f64::INFINITY, 0); plans.len()];
        if threads == 1 {
            for (slot, plan) in results.iter_mut().zip(&plans) {
                *slot = try_one(plan);
            }
        } else {
            let share = plans.len().div_ceil(threads);
            std::thread::scope(|scope| {
                for (slots, plans) in results.chunks_mut(share).zip(plans.chunks(share)) {
                    let try_one = &try_one;
                    scope.spawn(move || {
                        for (slot, plan) in slots.iter_mut().zip(plans) {
                            *slot = try_one(plan);
                        }
                    });
                }
            });
        }
        self.tried_steps += results.iter().map(|r| r.1).sum::<u64>();
        // The first of the least cost.
        let chosen = (0..plans.len()).fold(0, |best, c| if results[c].0 < results[best].0 { c } else { best });
        self.best = plans[chosen].0;
        self.seconds += began.elapsed().as_secs_f64();
        (Self::turns(take, rig, r, &feet, &self.best), plans[chosen].1)
    }
}

/// A step a plan takes: this foot (a bone) from here to there (px).
#[derive(Clone, Copy, Debug)]
pub struct Stride {
    pub foot: usize,
    pub from: [f64; 3],
    pub to: [f64; 3],
    pub lift: f64,
}
