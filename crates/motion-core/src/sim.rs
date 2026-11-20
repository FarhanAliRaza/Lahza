//! Simulation: what the document says about it (bodies on layers, a
//! scene's settings, joints and pushes), and the bake that drawing reads.
//!
//! A frame is drawn from its time alone, and a simulation has state. So a
//! simulation is baked: `lahza-motion-sim` steps it at a fixed rate from
//! its start and writes every body's transform per frame to
//! `sim/<scene>.bake` in the project's folder. `compile` places a body's
//! layer from the bake; before the body's release, or without a bake, the
//! layer is where its keys put it. The bake carries a hash of everything
//! it was made from (`SimInput::hash`); `check` compares it with the
//! document's and says when the bake is stale or missing.
//!
//! Space is the scene's: px, x right, y down, z away. The simulator works
//! in metres (`unit` px per metre), so gravity is 9.81 down by default.

use crate::mesh::MeshParams;
use crate::property::Property;
use crate::space::Mat4;
use crate::timeref::TimeRef;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

// ------------------------------------------------------------ document

/// A scene's simulation settings. A scene with bodies and no `simulation`
/// uses the defaults.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Simulation {
    /// px per metre.
    #[serde(default = "unit", skip_serializing_if = "is_unit")]
    pub unit: f64,
    /// m/s² in the scene's axes (y down).
    #[serde(default = "gravity", skip_serializing_if = "is_gravity")]
    pub gravity: [f64; 3],
    /// When stepping starts (default: the scene's start) and stops
    /// (default: the scene's end; bodies hold their last pose after it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start: Option<TimeRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<TimeRef>,
    /// Steps per second, rounded up to a whole number per frame.
    #[serde(default = "rate", skip_serializing_if = "is_rate")]
    pub rate: f64,
    /// Another seed is another take (it turns `jitter` and behaviours).
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub seed: u64,
    /// px/s of random velocity (and as many degrees/s of spin) given to
    /// each body at its release.
    #[serde(default, skip_serializing_if = "is_zero_f")]
    pub jitter: f64,
    /// Bodies stay in the picture plane: no movement in z, turning about z
    /// only (flat layers as bodies).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub flat: bool,
    /// A floor that stands still.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ground: Option<Ground>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub joints: Vec<Joint>,
    /// Pushes away from a point, on every body near it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blasts: Vec<Blast>,
    /// Skeletons simulated as ragdolls.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ragdolls: Vec<Ragdoll>,
    /// Crowds: agents that steer, play a clip, and fall when hit.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub crowds: Vec<Crowd>,
}

fn unit() -> f64 {
    100.0
}
fn is_unit(v: &f64) -> bool {
    *v == 100.0
}
fn gravity() -> [f64; 3] {
    [0.0, 9.81, 0.0]
}
fn is_gravity(v: &[f64; 3]) -> bool {
    *v == gravity()
}
fn rate() -> f64 {
    240.0
}
fn is_rate(v: &f64) -> bool {
    *v == 240.0
}
fn is_zero_u64(v: &u64) -> bool {
    *v == 0
}
fn is_zero_u32(v: &u32) -> bool {
    *v == 0
}
fn is_zero_f(v: &f64) -> bool {
    *v == 0.0
}
fn is_zero3(v: &[f64; 3]) -> bool {
    *v == [0.0; 3]
}

impl Default for Simulation {
    fn default() -> Self {
        Self { unit: unit(), gravity: gravity(), start: None, end: None, rate: rate(), seed: 0, jitter: 0.0, flat: false, ground: None, joints: Vec::new(), blasts: Vec::new(), ragdolls: Vec::new(), crowds: Vec::new() }
    }
}

/// A floor: the plane y = `y` (px, the scene's space), solid below.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ground {
    pub y: f64,
    #[serde(default = "friction", skip_serializing_if = "is_friction")]
    pub friction: f64,
    #[serde(default = "bounce", skip_serializing_if = "is_bounce")]
    pub bounce: f64,
}

fn friction() -> f64 {
    0.6
}
fn is_friction(v: &f64) -> bool {
    *v == 0.6
}
fn bounce() -> f64 {
    0.1
}
fn is_bounce(v: &f64) -> bool {
    *v == 0.1
}
fn one() -> f64 {
    1.0
}
fn is_one(v: &f64) -> bool {
    *v == 1.0
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BodyKind {
    /// Moved by the simulation (from its `release`).
    #[default]
    Dynamic,
    /// Stands where its keys put it at the simulation's start.
    Fixed,
    /// Follows its keys and pushes what it meets.
    Kinematic,
}

/// A layer as a body (`"body": {...}` on a mesh, a comp of meshes, or any
/// layer with a `shape`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Body {
    #[serde(rename = "type", default, skip_serializing_if = "is_default")]
    pub kind: BodyKind,
    /// What collides (default: from the layer's mesh: a sphere, box,
    /// capsule or cylinder as such, anything else by its convex hull, or
    /// its triangles when it does not move).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shape: Option<BodyShape>,
    /// kg (default: from `density` and the shape's volume).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mass: Option<f64>,
    /// 1 is water (1000 kg/m³).
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub density: f64,
    #[serde(default = "friction", skip_serializing_if = "is_friction")]
    pub friction: f64,
    #[serde(default = "bounce", skip_serializing_if = "is_bounce")]
    pub bounce: f64,
    /// Drag on movement and on turning (per second).
    #[serde(default, skip_serializing_if = "is_zero_f")]
    pub damping: f64,
    #[serde(default, skip_serializing_if = "is_zero_f")]
    pub spin_damping: f64,
    /// px/s and degrees/s (about x, y, z) at release, added to what the
    /// layer's animation left it with.
    #[serde(default, skip_serializing_if = "is_zero3")]
    pub velocity: [f64; 3],
    #[serde(default, skip_serializing_if = "is_zero3")]
    pub spin: [f64; 3],
    /// A dynamic body follows its keys until this time (default: the
    /// simulation's start), then keeps the speed they gave it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release: Option<TimeRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pushes: Vec<Push>,
    /// Fast and small: test the path between steps, not only its ends.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ccd: bool,
}

fn is_default<T: Default + PartialEq>(v: &T) -> bool {
    *v == T::default()
}

/// A collider given outright (px, the layer's space, centred on its
/// origin unless `offset`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub enum BodyShape {
    Box {
        size: [f64; 3],
        #[serde(default, skip_serializing_if = "is_zero3")]
        offset: [f64; 3],
    },
    Sphere {
        radius: f64,
        #[serde(default, skip_serializing_if = "is_zero3")]
        offset: [f64; 3],
    },
    /// Along y, `height` end to end; `axis` "x" or "z" lays it down.
    Capsule {
        radius: f64,
        height: f64,
        #[serde(default, skip_serializing_if = "is_default")]
        axis: Axis,
        #[serde(default, skip_serializing_if = "is_zero3")]
        offset: [f64; 3],
    },
    Cylinder {
        radius: f64,
        height: f64,
        #[serde(default, skip_serializing_if = "is_default")]
        axis: Axis,
        #[serde(default, skip_serializing_if = "is_zero3")]
        offset: [f64; 3],
    },
    /// The convex hull of the layer's mesh (or meshes).
    Hull,
    /// The mesh cut into convex pieces: hollows and notches stay open (a
    /// letter's counter, a cup). Slower to bake than a hull.
    Pieces,
    /// The mesh's own triangles: only for bodies that are not dynamic.
    Mesh,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Axis {
    X,
    #[default]
    Y,
    Z,
}

/// A change of speed at a time: `velocity` px/s and `spin` degrees/s are
/// added to the body's (whatever its mass); over `dur` seconds when given.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Push {
    pub at: TimeRef,
    #[serde(default, skip_serializing_if = "is_zero3")]
    pub velocity: [f64; 3],
    #[serde(default, skip_serializing_if = "is_zero3")]
    pub spin: [f64; 3],
    /// Where it lands, px in the layer's space (default: the centre of
    /// mass, so the push alone does not turn the body).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub point: Option<[f64; 3]>,
    #[serde(default, skip_serializing_if = "is_zero_f")]
    pub dur: f64,
}

/// A push away from `origin` (px, the scene's space) at a time: bodies
/// within `radius` gain up to `speed` px/s, less the farther they are.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Blast {
    pub at: TimeRef,
    pub origin: [f64; 3],
    pub speed: f64,
    pub radius: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JointKind {
    /// Welded.
    Fixed,
    /// Turns about `axis` through `anchor`; `limits` in degrees.
    Hinge,
    /// Turns any way about `anchor`; `swing` and `twist` limit it.
    Ball,
    /// Slides along `axis`; `limits` in px.
    Slider,
    /// No farther apart than `length` (slack when nearer).
    Rope,
    /// Pulled toward `length` apart by `stiffness`, calmed by `damping`.
    Spring,
}

/// Holds body `a` to body `b` (or to the world).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Joint {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(rename = "type")]
    pub kind: JointKind,
    pub a: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub b: Option<String>,
    /// The point the two share, px in `a`'s layer space.
    #[serde(default, skip_serializing_if = "is_zero3")]
    pub anchor: [f64; 3],
    /// The same on `b`, in its space (default: where `anchor` is at the
    /// simulation's start; ropes and springs: `b`'s origin).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor_b: Option<[f64; 3]>,
    /// In `a`'s space: a hinge's axle, a slider's rail, a ball's twist.
    #[serde(default = "axis_z", skip_serializing_if = "is_axis_z")]
    pub axis: [f64; 3],
    /// Hinge: degrees [from, to] from the starting pose. Slider: px.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<[f64; 2]>,
    /// Ball: the most the bodies bend from the starting pose, degrees.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swing: Option<f64>,
    /// Ball: degrees [from, to] of turning about `axis`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub twist: Option<[f64; 2]>,
    /// Rope, spring: px (default: the distance at the start).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub length: Option<f64>,
    /// Spring: N/m and N·s/m.
    #[serde(default = "spring_stiffness", skip_serializing_if = "is_spring_stiffness")]
    pub stiffness: f64,
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub damping: f64,
    /// Hinge, slider: drives the joint toward a keyed target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub motor: Option<Motor>,
    /// The two bodies collide with each other (default: they do not).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub collide: bool,
    /// Rope, spring: a layer of the scene (a cylinder, capsule or box
    /// mesh, its length along y) that is drawn as it: from anchor to
    /// anchor, a rope in pieces that hang by the slack it has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draw: Option<String>,
    /// Rope: made of this many bodies (capsules jointed end to end, baked
    /// as `<joint>/<n>`) that swing, whip and lie on what is between its
    /// ends. 0: no bodies, a curve drawn between the ends (cheap).
    #[serde(default, skip_serializing_if = "is_zero_u32")]
    pub links: u32,
    /// Rope with `links`: its radius, px (default: the drawn layer's, else
    /// 4), and its mass in all, kg (default: as water).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radius: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mass: Option<f64>,
}

fn axis_z() -> [f64; 3] {
    [0.0, 0.0, 1.0]
}
fn is_axis_z(v: &[f64; 3]) -> bool {
    *v == axis_z()
}
fn spring_stiffness() -> f64 {
    100.0
}
fn is_spring_stiffness(v: &f64) -> bool {
    *v == 100.0
}

/// A motor on a hinge (degrees) or slider (px): a spring toward `target`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Motor {
    pub target: Property<f64>,
    #[serde(default = "motor_stiffness", skip_serializing_if = "is_motor_stiffness")]
    pub stiffness: f64,
    #[serde(default = "motor_damping", skip_serializing_if = "is_motor_damping")]
    pub damping: f64,
    /// The most force (N, N·m) it may use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_force: Option<f64>,
}

fn motor_stiffness() -> f64 {
    200.0
}
fn is_motor_stiffness(v: &f64) -> bool {
    *v == 200.0
}
fn motor_damping() -> f64 {
    20.0
}
fn is_motor_damping(v: &f64) -> bool {
    *v == 20.0
}

/// A skeleton as a ragdoll: every bone layer under `root` (the comps
/// `model_import` makes of a skin's joints) becomes a capsule body, held
/// to its parent bone by a joint whose limits come from `limits`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ragdoll {
    pub id: String,
    /// The layer the skeleton hangs from (a bone, or a comp of bones).
    pub root: String,
    /// The bones, as layer ids (default: every comp under `root` that a
    /// skinned mesh names as a joint).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bones: Vec<String>,
    /// Follows its keys until this time (default: the simulation's start),
    /// then keeps the pose and speed they gave it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release: Option<TimeRef>,
    /// The time whose pose is the joints' zero: `limits` are measured from
    /// it (default: the simulation's start; pick a moment it stands
    /// straight).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rest: Option<TimeRef>,
    /// kg, shared among the bones by their volume.
    #[serde(default = "ragdoll_mass", skip_serializing_if = "is_ragdoll_mass")]
    pub mass: f64,
    /// A capsule's radius as a fraction of its bone's length.
    #[serde(default = "ragdoll_thickness", skip_serializing_if = "is_ragdoll_thickness")]
    pub thickness: f64,
    #[serde(default = "friction", skip_serializing_if = "is_friction")]
    pub friction: f64,
    #[serde(default, skip_serializing_if = "is_zero_f")]
    pub bounce: f64,
    /// Joint limits by bone id (or a part of it: "arm", "leg"); the first
    /// that matches. Bones none matches take `swing` 40, `twist` ±20.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub limits: Vec<BoneLimit>,
    /// A bone's collider given outright (px, the bone layer's space),
    /// for where a capsule along the bone is wrong: a foot is a box that
    /// stands flat. By bone id or a part of it; the first that matches.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shapes: Vec<BoneShape>,
    /// How hard the motors hold the clip's pose: 0 is limp (it only
    /// falls), 1 holds the pose as a body would. Keyed; `strengths` scale
    /// it per bone.
    #[serde(default = "zero_prop", skip_serializing_if = "is_zero_prop")]
    pub strength: Property<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub strengths: Vec<BoneStrength>,
    /// Its limbs meet its body and each other (true by default; false:
    /// they pass through, as bones jointed together and bones whose
    /// shapes overlap at rest always do).
    #[serde(default = "yes", skip_serializing_if = "is_yes")]
    pub self_collide: bool,
    /// What sets the motors' targets on top of the clip.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub behaviours: Vec<Behaviour>,
    /// Pushes on a bone at a time.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hits: Vec<Hit>,
    /// What sets the motors (default: the clip and the behaviours).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub controller: Option<Controller>,
}

impl Ragdoll {
    /// A limp ragdoll of the skeleton under `root`.
    pub fn of(id: String, root: String, mass: f64, thickness: f64, friction: f64) -> Ragdoll {
        Ragdoll { id, root, bones: Vec::new(), release: None, rest: None, mass, thickness, friction, bounce: 0.0, limits: Vec::new(), shapes: Vec::new(), strength: zero_prop(), strengths: Vec::new(), behaviours: Vec::new(), hits: Vec::new(), self_collide: true, controller: None }
    }
}

fn yes() -> bool {
    true
}
fn is_yes(v: &bool) -> bool {
    *v
}

fn ragdoll_mass() -> f64 {
    70.0
}
fn is_ragdoll_mass(v: &f64) -> bool {
    *v == 70.0
}
fn ragdoll_thickness() -> f64 {
    0.22
}
fn is_ragdoll_thickness(v: &f64) -> bool {
    *v == 0.22
}
fn zero_prop() -> Property<f64> {
    Property::Constant(0.0)
}
fn is_zero_prop(p: &Property<f64>) -> bool {
    *p == zero_prop()
}
fn one_prop() -> Property<f64> {
    Property::Constant(1.0)
}
fn is_one_prop(p: &Property<f64>) -> bool {
    *p == one_prop()
}

/// A bone's collider, in place of its capsule.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoneShape {
    pub bone: String,
    pub shape: BodyShape,
}

/// Limits of the joint that holds a bone to its parent.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoneLimit {
    /// A bone's layer id, or text its id contains.
    pub bone: String,
    /// Degrees it may bend from the bind pose.
    #[serde(default = "default_swing")]
    pub swing: f64,
    /// Degrees it may turn about its length.
    #[serde(default = "default_twist")]
    pub twist: f64,
    /// A hinge (an elbow, a knee): bends about this axis of the bone's
    /// space only, `swing` degrees one way.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hinge: Option<[f64; 3]>,
}

pub fn default_swing() -> f64 {
    40.0
}
pub fn default_twist() -> f64 {
    20.0
}

/// A bone's motor strength over time, times the ragdoll's.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoneStrength {
    /// A bone's layer id, or text its id contains; its children too.
    pub bone: String,
    #[serde(default = "one_prop", skip_serializing_if = "is_one_prop")]
    pub strength: Property<f64>,
}

/// A push on a ragdoll's bone: `velocity` px/s added to it at `at`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hit {
    pub at: TimeRef,
    pub bone: String,
    pub velocity: [f64; 3],
    #[serde(default, skip_serializing_if = "is_zero_f")]
    pub dur: f64,
}

/// A behaviour: from `from` to `to` it moves the motors' targets.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Behaviour {
    #[serde(rename = "type")]
    pub kind: BehaviourKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<TimeRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<TimeRef>,
    /// How much of it (keyed, 0..1).
    #[serde(default = "one_prop", skip_serializing_if = "is_one_prop")]
    pub amount: Property<f64>,
    /// By kind (see `BehaviourKind`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, serde_json::Value>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BehaviourKind {
    /// Keeps the centre of mass over the feet by leaning at the ankles
    /// and hips, and holds the trunk up. Params: `gain`, `upright`.
    Balance,
    /// Steps a foot to where the body is falling to. Params: `length`
    /// (px, the longest step), `time` (seconds a step takes), `trigger`
    /// (px the centre of mass may leave the feet by first).
    Step,
    /// A hand toward a body or a point that comes near. Params: `hand`
    /// ("left", "right", "near"), `target` (a layer id or [x, y, z]),
    /// `within` (px).
    Guard,
    /// A hand to where the last hit landed. Params: `hand`, `hold` (s).
    Reach,
    /// Both arms up about the head. No params.
    Protect,
    /// With `balance`: once it has fallen and lain still for `wait`
    /// seconds it rolls onto its front, pushes up, draws its legs under
    /// it and stands, in `time` seconds. An outside force on its hips of
    /// at most `help` × its weight (and the torque that would make)
    /// lifts and rights it. Params: `wait`, `time`, `help`, `way`
    /// ("front", "back": it sits up instead, or "near").
    GetUp,
}

/// What sets a ragdoll's motors instead of the clip and behaviours.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Controller {
    /// A network (ONNX, a project path): observation in, targets out
    /// (`lahza-motion-sim`'s `policy` module gives the layout).
    Policy { src: String },
    /// No training: each `every` seconds it tries `samples` plans (leans,
    /// arm swings, perhaps a step) `horizon` seconds ahead in the
    /// simulator and keeps the best (the one that stays up and nearest
    /// the clip).
    Search {
        #[serde(default = "search_samples")]
        samples: u32,
        #[serde(default = "search_horizon")]
        horizon: f64,
        #[serde(default = "search_every")]
        every: f64,
        /// How far a try may move a target, degrees.
        #[serde(default = "search_spread")]
        spread: f64,
    },
}

fn search_samples() -> u32 {
    16
}
fn search_horizon() -> f64 {
    0.5
}
fn search_every() -> f64 {
    0.1
}
fn search_spread() -> f64 {
    12.0
}

/// A crowd: agents (comps: a character each, with its clip keyed from
/// `clip` on) that walk from where they stand to their goals, round each
/// other and round bodies in their way. The bake places each agent's comp
/// and runs its clip with the distance it has walked, so feet do not
/// slide. An agent a body strikes at `knock` px/s or more goes down: a
/// limp ragdoll from then when the crowd has a `ragdoll`, else it stops.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Crowd {
    pub id: String,
    /// The agents' layers (comps), each walking on the level (x, z) from
    /// where its keys have it at the simulation's start.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub agents: Vec<String>,
    /// Instead of `agents`: one comp (a character) that every agent is an
    /// instance of. The layer itself is not drawn; the bake places its
    /// copies, at the level, turn and scale its keys give it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub character: Option<String>,
    /// Where the instances start, [x, z] px; or `count` of them spread
    /// over `area` ([x0, z0, x1, z1]) by the seed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub starts: Vec<[f64; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub area: Option<[f64; 4]>,
    /// Where they go, [x, z] px: one for all, or one per agent in turn.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub goals: Vec<[f64; 2]>,
    /// Instead of `goals`: each agent goes this far from where it starts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub march: Option<[f64; 2]>,
    /// Instances: how many are drawn as the character itself, nearest the
    /// camera first; the rest are pictures of it (impostors). And a
    /// distance from the camera (px) past which every one is a picture.
    #[serde(default = "crowd_full")]
    pub full: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub near: Option<f64>,
    /// px/s, and how much it differs between agents (0..1).
    #[serde(default = "crowd_speed")]
    pub speed: f64,
    #[serde(default = "crowd_vary")]
    pub vary: f64,
    /// An agent's radius and height, px (height: six radii by default).
    #[serde(default = "crowd_radius")]
    pub radius: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<f64>,
    /// px a cycle of the clip covers, and seconds the cycle lasts.
    #[serde(default = "crowd_stride")]
    pub stride: f64,
    #[serde(default = "one")]
    pub cycle: f64,
    /// When the agents' clip starts (default: the simulation's start).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<TimeRef>,
    /// The way an agent faces in its own space, [x, z] (default [0, −1]:
    /// toward the viewer, as an imported glTF character does).
    #[serde(default = "crowd_facing")]
    pub facing: [f64; 2],
    /// px/s of a body's speed into an agent that knocks it down.
    #[serde(default = "crowd_knock")]
    pub knock: f64,
    /// Agents find their way round what stands across it (false: they
    /// only steer off what is near, and a wall holds them).
    #[serde(default = "yes", skip_serializing_if = "is_yes")]
    pub plan: bool,
    /// Seconds ahead an agent sees a body coming that would knock it
    /// down, and steps out of its line (0: it does not look).
    #[serde(default = "crowd_dodge")]
    pub dodge: f64,
    /// Agents knocked down fall as ragdolls made like this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ragdoll: Option<CrowdRagdoll>,
}

/// How a crowd's agents are made ragdolls (`Ragdoll`'s fields of the same
/// names; the bones are the comps under each agent that a skin names).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CrowdRagdoll {
    #[serde(default = "ragdoll_mass")]
    pub mass: f64,
    #[serde(default = "ragdoll_thickness")]
    pub thickness: f64,
    #[serde(default = "friction")]
    pub friction: f64,
    #[serde(default)]
    pub limits: Vec<BoneLimit>,
    #[serde(default)]
    pub shapes: Vec<BoneShape>,
    /// How hard one knocked down holds the pose it fell in (0: limp).
    #[serde(default)]
    pub strength: f64,
    /// Instances: how many can be down as ragdolls at once in a take (the
    /// ones knocked after that stop where they stand).
    #[serde(default = "crowd_fallen")]
    pub max: u32,
}

fn crowd_dodge() -> f64 {
    0.6
}
fn crowd_full() -> u32 {
    150
}
fn crowd_fallen() -> u32 {
    24
}
fn crowd_facing() -> [f64; 2] {
    [0.0, -1.0]
}
fn crowd_speed() -> f64 {
    140.0
}
fn crowd_vary() -> f64 {
    0.2
}
fn crowd_radius() -> f64 {
    30.0
}
fn crowd_stride() -> f64 {
    140.0
}
fn crowd_knock() -> f64 {
    300.0
}

// ---------------------------------------------------------------- math

/// A rigid placement: where and how turned (a unit quaternion x, y, z, w).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub p: [f64; 3],
    pub q: [f64; 4],
}

impl Pose {
    pub const IDENTITY: Pose = Pose { p: [0.0; 3], q: [0.0, 0.0, 0.0, 1.0] };

    pub fn matrix(&self) -> Mat4 {
        let [x, y, z, w] = self.q;
        let [px, py, pz] = self.p;
        Mat4([
            1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y - z * w), 2.0 * (x * z + y * w), px,
            2.0 * (x * y + z * w), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z - x * w), py,
            2.0 * (x * z - y * w), 2.0 * (y * z + x * w), 1.0 - 2.0 * (x * x + y * y), pz,
            0.0, 0.0, 0.0, 1.0,
        ])
    }

    pub fn inverse(&self) -> Pose {
        let q = [-self.q[0], -self.q[1], -self.q[2], self.q[3]];
        let p = quat_rotate(q, self.p);
        Pose { p: [-p[0], -p[1], -p[2]], q }
    }

    /// self ∘ other (apply `other` first).
    pub fn then(&self, other: &Pose) -> Pose {
        let r = quat_rotate(self.q, other.p);
        Pose { p: [self.p[0] + r[0], self.p[1] + r[1], self.p[2] + r[2]], q: quat_mul(self.q, other.q) }
    }

    pub fn apply(&self, v: [f64; 3]) -> [f64; 3] {
        let r = quat_rotate(self.q, v);
        [self.p[0] + r[0], self.p[1] + r[1], self.p[2] + r[2]]
    }
}

pub fn quat_mul(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
    let ([ax, ay, az, aw], [bx, by, bz, bw]) = (a, b);
    [aw * bx + ax * bw + ay * bz - az * by, aw * by - ax * bz + ay * bw + az * bx, aw * bz + ax * by - ay * bx + az * bw, aw * bw - ax * bx - ay * by - az * bz]
}

pub fn quat_rotate(q: [f64; 4], v: [f64; 3]) -> [f64; 3] {
    let u = [q[0], q[1], q[2]];
    let w = q[3];
    let c1 = cross(u, v);
    let c2 = cross(u, c1);
    [v[0] + 2.0 * (w * c1[0] + c2[0]), v[1] + 2.0 * (w * c1[1] + c2[1]), v[2] + 2.0 * (w * c1[2] + c2[2])]
}

pub fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn norm(v: [f64; 3]) -> f64 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

/// The turn that takes y to `d` (unit).
pub fn quat_y_to(d: [f64; 3]) -> [f64; 4] {
    let c = d[1];
    if c < -1.0 + 1e-9 {
        return [1.0, 0.0, 0.0, 0.0];
    }
    let axis = cross([0.0, 1.0, 0.0], d);
    let q = [axis[0], axis[1], axis[2], 1.0 + c];
    let n = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    q.map(|v| v / n)
}

/// How far a rope `length` long sags between ends `apart` from each
/// other: the depth of the parabola as long as it (found by halving: its
/// length grows with its sag). 0 when it is taut.
pub fn rope_sag(length: f64, apart: f64) -> f64 {
    if length <= apart {
        return 0.0;
    }
    let long_with = |sag: f64| {
        let a = 4.0 * sag / apart.max(1e-9);
        if a < 1e-9 { apart } else { apart * (0.5 * (1.0 + a * a).sqrt() + crate::dm::ln(a + (1.0 + a * a).sqrt()) / (2.0 * a)) }
    };
    let (mut lo, mut hi) = (0.0, length / 2.0);
    for _ in 0..40 {
        let mid = (lo + hi) / 2.0;
        if long_with(mid) < length { lo = mid } else { hi = mid }
    }
    lo
}

/// A rotation matrix's quaternion (columns `c`: orthonormal, right-handed).
fn quat_from_columns(c: [[f64; 3]; 3]) -> [f64; 4] {
    // m[r][k] = c[k][r].
    let m = |r: usize, k: usize| c[k][r];
    let tr = m(0, 0) + m(1, 1) + m(2, 2);
    let q = if tr > 0.0 {
        let s = (tr + 1.0).sqrt() * 2.0;
        [(m(2, 1) - m(1, 2)) / s, (m(0, 2) - m(2, 0)) / s, (m(1, 0) - m(0, 1)) / s, 0.25 * s]
    } else if m(0, 0) > m(1, 1) && m(0, 0) > m(2, 2) {
        let s = (1.0 + m(0, 0) - m(1, 1) - m(2, 2)).sqrt() * 2.0;
        [0.25 * s, (m(0, 1) + m(1, 0)) / s, (m(0, 2) + m(2, 0)) / s, (m(2, 1) - m(1, 2)) / s]
    } else if m(1, 1) > m(2, 2) {
        let s = (1.0 + m(1, 1) - m(0, 0) - m(2, 2)).sqrt() * 2.0;
        [(m(0, 1) + m(1, 0)) / s, 0.25 * s, (m(1, 2) + m(2, 1)) / s, (m(0, 2) - m(2, 0)) / s]
    } else {
        let s = (1.0 + m(2, 2) - m(0, 0) - m(1, 1)).sqrt() * 2.0;
        [(m(0, 2) + m(2, 0)) / s, (m(1, 2) + m(2, 1)) / s, 0.25 * s, (m(1, 0) - m(0, 1)) / s]
    };
    let n = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt().max(1e-300);
    [q[0] / n, q[1] / n, q[2] / n, q[3] / n]
}

/// A layer's world matrix as a rigid pose and what is left (scale and
/// anchor): `m = pose.matrix() · rest`. The pose's axes are the matrix's,
/// made orthonormal; a mirrored matrix keeps its mirror in `rest`.
pub fn decompose(m: Mat4) -> (Pose, Mat4) {
    let a = m.0;
    let col = |k: usize| [a[k], a[4 + k], a[8 + k]];
    let (c0, c1) = (col(0), col(1));
    let unit = |v: [f64; 3], fallback: [f64; 3]| {
        let n = norm(v);
        if n > 1e-12 {
            [v[0] / n, v[1] / n, v[2] / n]
        } else {
            fallback
        }
    };
    let x = unit(c0, [1.0, 0.0, 0.0]);
    // y without its part along x.
    let d = c1[0] * x[0] + c1[1] * x[1] + c1[2] * x[2];
    let mut y = unit([c1[0] - d * x[0], c1[1] - d * x[1], c1[2] - d * x[2]], [0.0, 1.0, 0.0]);
    if norm(cross(x, y)) < 1e-9 {
        y = unit(cross([0.0, 0.0, 1.0], x), [0.0, 1.0, 0.0]);
    }
    let z = cross(x, y);
    let pose = Pose { p: [a[3], a[7], a[11]], q: quat_from_columns([x, y, z]) };
    (pose, pose.inverse().matrix().then(m))
}

/// The inverse of an affine matrix (last row 0 0 0 1), or the identity
/// when it is singular.
pub fn inverse(m: Mat4) -> Mat4 {
    let a = m.0;
    let Some(n) = m.normal_matrix() else { return Mat4::IDENTITY };
    // normal_matrix is (A⁻¹)ᵀ.
    let inv = [[n[0][0], n[1][0], n[2][0]], [n[0][1], n[1][1], n[2][1]], [n[0][2], n[1][2], n[2][2]]];
    let t = [a[3], a[7], a[11]];
    let it: [f64; 3] = std::array::from_fn(|r| -(inv[r][0] * t[0] + inv[r][1] * t[1] + inv[r][2] * t[2]));
    Mat4([inv[0][0], inv[0][1], inv[0][2], it[0], inv[1][0], inv[1][1], inv[1][2], it[1], inv[2][0], inv[2][1], inv[2][2], it[2], 0.0, 0.0, 0.0, 1.0])
}

// ---------------------------------------------------------------- bake

/// One body's baked motion: a pose per sample from `from` on.
#[derive(Clone, Debug, PartialEq)]
pub struct Track {
    /// The layer's id (a crowd's agent: `<crowd>#<n>`).
    pub id: String,
    /// The first sample the bake stands for (the body's release): before
    /// it the layer follows its keys.
    pub from: u32,
    /// Body frame → layer space at the release (scale and anchor): the
    /// layer's world matrix is `pose · rest`.
    pub rest: Mat4,
    /// Per sample: position px (x, y, z), then a quaternion (x, y, z, w).
    pub samples: Vec<[f32; 7]>,
    /// Per sample, for what plays a clip (a crowd's agent): the timeline
    /// seconds its children are shown at. Empty otherwise.
    pub clock: Vec<f32>,
}

/// A scene's baked simulation.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Bake {
    /// `SimInput::hash` of what it was made from.
    pub hash: u64,
    /// Timeline seconds of sample 0, and samples per second.
    pub start: f64,
    pub rate: f64,
    pub tracks: Vec<Track>,
    /// What happened, for directing: (seconds, kind, who, whom, how hard).
    pub events: Vec<SimEvent>,
    /// Crowds of instances.
    pub herds: Vec<HerdTrack>,
}

/// A crowd of instances, baked: every agent walks on the level, so a
/// sample of one is four numbers.
#[derive(Clone, Debug, PartialEq)]
pub struct HerdTrack {
    /// The crowd's id. A fallen agent's bones are tracks `<id>#<slot>/<bone layer>`.
    pub id: String,
    pub count: u32,
    /// As `Track::rest`: the character's scale and anchor.
    pub rest: Mat4,
    /// The level the agents walk on, and how the character is turned
    /// before an agent's own turn.
    pub y: f64,
    pub q: [f64; 4],
    /// Per sample, per agent (`sample · count + agent`): x, z, its turn
    /// about y (radians), and the timeline seconds its clip is shown at.
    pub samples: Vec<[f32; 4]>,
    /// Per agent: the sample from which it is down (`u32::MAX`: never),
    /// and the ragdoll of the pool it fell as (`u32::MAX`: none).
    pub down: Vec<u32>,
    pub slot: Vec<u32>,
}

impl HerdTrack {
    pub fn len(&self) -> usize {
        if self.count == 0 { 0 } else { self.samples.len() / self.count as usize }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Agent `k` at sample position `x`: where it stands, its clip's
    /// time, and its turn about y (radians).
    pub fn agent(&self, k: usize, x: f64) -> Option<(Pose, f64, f64)> {
        let last = self.len().checked_sub(1)?;
        let n = self.count as usize;
        let x = x.clamp(0.0, last as f64);
        let i = (x.floor() as usize).min(last);
        let j = (i + 1).min(last);
        let f = x - i as f64;
        let (a, b) = (self.samples[i * n + k], self.samples[j * n + k]);
        let l = |c: usize| a[c] as f64 + (b[c] as f64 - a[c] as f64) * f;
        // The turn the short way round.
        let mut d = (b[2] - a[2]) as f64;
        d -= (d / std::f64::consts::TAU).round() * std::f64::consts::TAU;
        let turn = a[2] as f64 + d * f;
        let q = quat_mul([0.0, crate::dm::sin(turn / 2.0), 0.0, crate::dm::cos(turn / 2.0)], self.q);
        // A clip that wrapped between the samples is not run backwards.
        let clock = if b[3] < a[3] { a[3] as f64 } else { l(3) };
        Some((Pose { p: [l(0), self.y, l(1)], q }, clock, turn))
    }
}

/// Something that happened in a take: a first touch between two bodies
/// (`speed` px/s of their approach), a fall, a recovery.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SimEvent {
    pub t: f64,
    pub kind: String,
    pub a: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub b: String,
    #[serde(default)]
    pub speed: f64,
}

const MAGIC: &[u8; 8] = b"LZSIM\x02\0\0";

impl Track {
    /// The pose at sample position `x` (fractional): positions lerp,
    /// rotations take the short way.
    pub fn pose(&self, x: f64) -> Option<Pose> {
        let last = self.samples.len().checked_sub(1)?;
        let x = x.clamp(0.0, last as f64);
        let i = (x.floor() as usize).min(last);
        let j = (i + 1).min(last);
        let f = x - i as f64;
        let (a, b) = (&self.samples[i], &self.samples[j]);
        let l = |k: usize| a[k] as f64 + (b[k] as f64 - a[k] as f64) * f;
        let dot: f64 = (3..7).map(|k| a[k] as f64 * b[k] as f64).sum();
        let s = if dot < 0.0 { -1.0 } else { 1.0 };
        let mut q: [f64; 4] = std::array::from_fn(|k| a[3 + k] as f64 + (s * b[3 + k] as f64 - a[3 + k] as f64) * f);
        let n = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
        if n < 1e-12 {
            q = [0.0, 0.0, 0.0, 1.0];
        } else {
            q = q.map(|v| v / n);
        }
        Some(Pose { p: [l(0), l(1), l(2)], q })
    }

    /// The time its children are shown at, at sample position `x`.
    pub fn clock_at(&self, x: f64) -> Option<f64> {
        let last = self.clock.len().checked_sub(1)?;
        let x = x.clamp(0.0, last as f64);
        let i = (x.floor() as usize).min(last);
        let j = (i + 1).min(last);
        Some(self.clock[i] as f64 + (self.clock[j] as f64 - self.clock[i] as f64) * (x - i as f64))
    }
}

impl Bake {
    pub fn track(&self, id: &str) -> Option<usize> {
        self.tracks.iter().position(|t| t.id == id)
    }

    /// Sample position of timeline time `t`.
    pub fn position(&self, t: f64) -> f64 {
        (t - self.start) * self.rate
    }

    /// The layer's world matrix at timeline time `t`, when the bake stands
    /// for it then.
    pub fn world(&self, track: usize, t: f64) -> Option<Mat4> {
        let tr = self.tracks.get(track)?;
        let x = self.position(t);
        if x < tr.from as f64 - 1e-9 {
            return None;
        }
        Some(tr.pose(x)?.matrix().then(tr.rest))
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut o = Vec::new();
        o.extend_from_slice(MAGIC);
        o.extend_from_slice(&self.hash.to_le_bytes());
        o.extend_from_slice(&self.start.to_le_bytes());
        o.extend_from_slice(&self.rate.to_le_bytes());
        o.extend_from_slice(&(self.tracks.len() as u32).to_le_bytes());
        for t in &self.tracks {
            o.extend_from_slice(&(t.id.len() as u32).to_le_bytes());
            o.extend_from_slice(t.id.as_bytes());
            o.extend_from_slice(&t.from.to_le_bytes());
            for v in t.rest.0 {
                o.extend_from_slice(&v.to_le_bytes());
            }
            o.extend_from_slice(&(t.samples.len() as u32).to_le_bytes());
            for s in &t.samples {
                for v in s {
                    o.extend_from_slice(&v.to_le_bytes());
                }
            }
            o.extend_from_slice(&(t.clock.len() as u32).to_le_bytes());
            for v in &t.clock {
                o.extend_from_slice(&v.to_le_bytes());
            }
        }
        let events = serde_json::to_vec(&self.events).unwrap_or_default();
        o.extend_from_slice(&(events.len() as u32).to_le_bytes());
        o.extend_from_slice(&events);
        // (A bake without crowds of instances ends here.)
        if !self.herds.is_empty() {
            o.extend_from_slice(&(self.herds.len() as u32).to_le_bytes());
        }
        for h in &self.herds {
            o.extend_from_slice(&(h.id.len() as u32).to_le_bytes());
            o.extend_from_slice(h.id.as_bytes());
            o.extend_from_slice(&h.count.to_le_bytes());
            for v in h.rest.0.iter().chain([h.y].iter()).chain(h.q.iter()) {
                o.extend_from_slice(&v.to_le_bytes());
            }
            o.extend_from_slice(&(h.samples.len() as u32).to_le_bytes());
            for s in &h.samples {
                for v in s {
                    o.extend_from_slice(&v.to_le_bytes());
                }
            }
            for v in h.down.iter().chain(&h.slot) {
                o.extend_from_slice(&v.to_le_bytes());
            }
        }
        o
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Bake, String> {
        struct R<'a>(&'a [u8]);
        impl<'a> R<'a> {
            fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
                if self.0.len() < n {
                    return Err("the bake is cut short".into());
                }
                let (a, b) = self.0.split_at(n);
                self.0 = b;
                Ok(a)
            }
            fn u32(&mut self) -> Result<u32, String> {
                Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
            }
            fn u64(&mut self) -> Result<u64, String> {
                Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
            }
            fn f64(&mut self) -> Result<f64, String> {
                Ok(f64::from_le_bytes(self.take(8)?.try_into().unwrap()))
            }
            fn f32(&mut self) -> Result<f32, String> {
                Ok(f32::from_le_bytes(self.take(4)?.try_into().unwrap()))
            }
        }
        let mut r = R(bytes);
        if r.take(8)? != MAGIC {
            return Err("not a simulation bake of this version (bake again)".into());
        }
        let (hash, start, rate) = (r.u64()?, r.f64()?, r.f64()?);
        let n = r.u32()? as usize;
        let mut tracks = Vec::with_capacity(n.min(1 << 16));
        for _ in 0..n {
            let len = r.u32()? as usize;
            let id = String::from_utf8(r.take(len)?.to_vec()).map_err(|_| "the bake has a bad id".to_string())?;
            let from = r.u32()?;
            let mut rest = [0.0; 16];
            for v in &mut rest {
                *v = r.f64()?;
            }
            let count = r.u32()? as usize;
            let raw = r.take(count.checked_mul(28).ok_or("the bake is cut short")?)?;
            let samples = raw.chunks_exact(28).map(|c| std::array::from_fn(|k| f32::from_le_bytes(c[k * 4..k * 4 + 4].try_into().unwrap()))).collect();
            let count = r.u32()? as usize;
            let mut clock = Vec::with_capacity(count.min(1 << 20));
            for _ in 0..count {
                clock.push(r.f32()?);
            }
            tracks.push(Track { id, from, rest: Mat4(rest), samples, clock });
        }
        let len = r.u32()? as usize;
        let events = serde_json::from_slice(r.take(len)?).map_err(|e| format!("the bake's events: {e}"))?;
        let mut herds = Vec::new();
        if !r.0.is_empty() {
            for _ in 0..r.u32()? {
                let len = r.u32()? as usize;
                let id = String::from_utf8(r.take(len)?.to_vec()).map_err(|_| "the bake has a bad id".to_string())?;
                let count = r.u32()?;
                let mut f = [0.0; 21];
                for v in &mut f {
                    *v = r.f64()?;
                }
                let n = r.u32()? as usize;
                let raw = r.take(n.checked_mul(16).ok_or("the bake is cut short")?)?;
                let samples = raw.chunks_exact(16).map(|c| std::array::from_fn(|k| f32::from_le_bytes(c[k * 4..k * 4 + 4].try_into().unwrap()))).collect();
                let mut marks = [Vec::new(), Vec::new()];
                for m in &mut marks {
                    let raw = r.take(count as usize * 4)?;
                    *m = raw.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect();
                }
                let [down, slot] = marks;
                herds.push(HerdTrack { id, count, rest: Mat4(f[..16].try_into().unwrap()), y: f[16], q: f[17..21].try_into().unwrap(), samples, down, slot });
            }
        }
        Ok(Bake { hash, start, rate, tracks, events, herds })
    }
}

/// Where a scene's bake is kept.
pub fn bake_path(dir: &Path, scene: &str) -> PathBuf {
    dir.join("sim").join(format!("{scene}.bake"))
}

/// The bakes in a project's folder, by scene id (unreadable ones are left
/// out: `check` then reports the scene as not baked).
pub fn load_bakes(dir: &Path) -> BTreeMap<String, Arc<Bake>> {
    let mut out = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(dir.join("sim")) else { return out };
    let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    paths.sort();
    for path in paths {
        if path.extension().and_then(|e| e.to_str()) != Some("bake") {
            continue;
        }
        let Some(scene) = path.file_stem().and_then(|s| s.to_str()) else { continue };
        if let Ok(bake) = std::fs::read(&path).map_err(|e| e.to_string()).and_then(|b| Bake::from_bytes(&b)) {
            out.insert(scene.to_string(), Arc::new(bake));
        }
    }
    out
}

pub fn save_bake(dir: &Path, scene: &str, bake: &Bake) -> crate::Result<()> {
    crate::io::write_atomic(&bake_path(dir, scene), &bake.to_bytes())
}

// --------------------------------------------------------------- input

/// What collides, in the body's frame.
#[derive(Clone, Debug, PartialEq)]
pub enum ShapeInput {
    /// Half extents, px.
    Box { half: [f64; 3] },
    Ball { radius: f64 },
    /// Along y: `half` is half the distance between the end spheres' centres.
    Capsule { radius: f64, half: f64 },
    Cylinder { radius: f64, half: f64 },
    /// The convex hull of a mesh's points.
    Hull(Arc<MeshParams>),
    /// A mesh cut into convex pieces.
    Pieces(Arc<MeshParams>),
    /// A mesh's triangles.
    Trimesh(Arc<MeshParams>),
}

/// One collider of a body: `shape` placed by `local` (shape space → the
/// body's frame, px; rigid for the round and box shapes, any affine for
/// meshes).
#[derive(Clone, Debug, PartialEq)]
pub struct ColliderInput {
    pub shape: ShapeInput,
    pub local: Mat4,
}

/// A change of speed over steps.
#[derive(Clone, Debug, PartialEq)]
pub struct PushInput {
    /// Timeline seconds.
    pub at: f64,
    pub dur: f64,
    /// px/s and degrees/s, the scene's axes.
    pub velocity: [f64; 3],
    pub spin: [f64; 3],
    /// px, the body's frame.
    pub point: Option<[f64; 3]>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BodyInput {
    pub id: String,
    pub kind: BodyKind,
    pub colliders: Vec<ColliderInput>,
    pub mass: Option<f64>,
    pub density: f64,
    pub friction: f64,
    pub bounce: f64,
    pub damping: f64,
    pub spin_damping: f64,
    pub velocity: [f64; 3],
    pub spin: [f64; 3],
    pub ccd: bool,
    /// The sample at which a dynamic body is let go.
    pub release: u32,
    /// Where the layer's keys put the body, per sample (one entry when
    /// they never move it): what a kinematic body does, and a dynamic one
    /// up to its release.
    pub keyed: Vec<Pose>,
    /// Where they put it one sample before the release (its speed there).
    pub before: Pose,
    /// Body frame → layer space (scale and anchor) at the release.
    pub rest: Mat4,
    pub pushes: Vec<PushInput>,
    /// Bodies of one group (a ragdoll's bones) never collide with the
    /// bone they are jointed to; 0 is no group.
    pub group: u32,
    /// Bodies it never touches (a rope's link: the two it is tied to).
    pub apart: Vec<usize>,
    /// An emitter's particle: the body is out of the world but for these
    /// spans (`release` is `u32::MAX`), put at each one's start where the
    /// particle is born.
    pub lives: Vec<LifeInput>,
}

/// One particle of an emitter's slot as a body: in the world from sample
/// `from` until `to`.
#[derive(Clone, Debug, PartialEq)]
pub struct LifeInput {
    pub from: u32,
    pub to: u32,
    /// Where the particle is at `from`, and a sample later by its own
    /// flight (its speed as it becomes a body).
    pub pose: Pose,
    pub next: Pose,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MotorInput {
    /// Per sample: degrees or px.
    pub target: Vec<f64>,
    pub stiffness: f64,
    pub damping: f64,
    pub max_force: Option<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct JointInput {
    pub id: String,
    pub kind: JointKind,
    /// Indices into `SimInput::bodies`; `None` is the world.
    pub a: usize,
    pub b: Option<usize>,
    /// px, each body's frame (the world's for `b: None`).
    pub anchor_a: [f64; 3],
    pub anchor_b: [f64; 3],
    /// Unit, each body's frame.
    pub axis_a: [f64; 3],
    pub axis_b: [f64; 3],
    pub limits: Option<[f64; 2]>,
    pub swing: Option<f64>,
    pub twist: Option<[f64; 2]>,
    pub length: f64,
    pub stiffness: f64,
    pub damping: f64,
    pub motor: Option<MotorInput>,
    pub collide: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BlastInput {
    pub at: f64,
    pub origin: [f64; 3],
    pub speed: f64,
    pub radius: f64,
}

/// A ragdoll's bone: a body, and the joint to its parent bone.
#[derive(Clone, Debug, PartialEq)]
pub struct BoneInput {
    /// Index into `SimInput::bodies`.
    pub body: usize,
    /// Index into the ragdoll's `bones`; the root has none.
    pub parent: Option<usize>,
    /// The joint's place, px in the bone's frame (its origin) and in the
    /// parent's.
    pub anchor_parent: [f64; 3],
    /// The bone's direction in its own frame (unit): the twist axis.
    pub along: [f64; 3],
    pub length: f64,
    pub swing: f64,
    pub twist: f64,
    pub hinge: Option<[f64; 3]>,
    /// The bone's rotation relative to its parent in the bind pose.
    pub bind: [f64; 4],
    /// Per sample: its rotation relative to its parent as the clip has it
    /// (the motors' target), and its strength (0..1).
    pub clip: Vec<[f64; 4]>,
    pub strength: Vec<f64>,
    pub mass: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BehaviourInput {
    pub kind: BehaviourKind,
    pub from: f64,
    pub to: f64,
    /// Per sample.
    pub amount: Vec<f64>,
    pub params: BTreeMap<String, serde_json::Value>,
    /// A `guard`'s target when it is a body: its index.
    pub target_body: Option<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RagdollInput {
    pub id: String,
    pub bones: Vec<BoneInput>,
    /// Its bones collide with each other (but for the jointed and the
    /// ones that overlap at rest).
    pub self_collide: bool,
    pub release: u32,
    pub behaviours: Vec<BehaviourInput>,
    /// (seconds, bone index, px/s, seconds it lasts).
    pub hits: Vec<(f64, usize, [f64; 3], f64)>,
    pub controller: Option<Controller>,
}

/// A crowd's agent: where it starts and goes, and its skeleton.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentInput {
    pub id: String,
    /// Its keyed place at the simulation's start, and its scale and anchor.
    pub start: Pose,
    pub rest: Mat4,
    pub goal: [f64; 2],
    /// Its ragdoll, in `SimInput::ragdolls`.
    pub ragdoll: Option<usize>,
    /// Per bone of that ragdoll, per sample of one cycle of the clip: the
    /// bone's place in the agent's frame.
    pub local: Vec<Vec<Pose>>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CrowdInput {
    pub id: String,
    pub agents: Vec<AgentInput>,
    /// The agents are instances of one character: the bake keeps them as
    /// a `HerdTrack`, and those knocked down take a ragdoll of `pool`
    /// (indices into `SimInput::ragdolls`) placed by `local` (as
    /// `AgentInput::local`).
    pub instanced: bool,
    pub pool: Vec<usize>,
    pub local: Vec<Vec<Pose>>,
    pub speed: f64,
    pub vary: f64,
    pub radius: f64,
    pub height: f64,
    pub stride: f64,
    pub cycle: f64,
    /// Timeline seconds the agents' clip starts at.
    pub clip_at: f64,
    pub facing: [f64; 2],
    pub knock: f64,
    pub plan: bool,
    pub dodge: f64,
}

/// Everything a bake is made from: the document's simulation resolved to
/// numbers (times in timeline seconds, places in the scene's px).
#[derive(Clone, Debug, PartialEq)]
pub struct SimInput {
    pub scene: String,
    /// Timeline seconds of sample 0; samples per second (the project's
    /// frame rate); how many samples.
    pub start: f64,
    pub rate: f64,
    pub samples: u32,
    /// Steps between two samples.
    pub substeps: u32,
    pub unit: f64,
    pub gravity: [f64; 3],
    pub seed: u64,
    pub jitter: f64,
    pub flat: bool,
    pub ground: Option<Ground>,
    pub bodies: Vec<BodyInput>,
    pub joints: Vec<JointInput>,
    pub blasts: Vec<BlastInput>,
    pub ragdolls: Vec<RagdollInput>,
    pub crowds: Vec<CrowdInput>,
    /// Hashes of the files colliders are read from (glTF), by path.
    pub files: Vec<(String, u64)>,
}

/// FNV-1a, 64 bits: the same on every machine.
#[derive(Clone, Copy)]
pub struct Fnv(pub u64);

impl Default for Fnv {
    fn default() -> Self {
        Fnv(0xcbf29ce484222325)
    }
}

impl Fnv {
    pub fn bytes(&mut self, b: &[u8]) {
        for &x in b {
            self.0 ^= x as u64;
            self.0 = self.0.wrapping_mul(0x100000001b3);
        }
    }
    pub fn u64(&mut self, v: u64) {
        self.bytes(&v.to_le_bytes());
    }
    pub fn f(&mut self, v: f64) {
        // −0 and 0 are one number.
        self.u64(if v == 0.0 { 0 } else { v.to_bits() });
    }
    pub fn fs(&mut self, v: &[f64]) {
        self.u64(v.len() as u64);
        for x in v {
            self.f(*x);
        }
    }
    pub fn str(&mut self, s: &str) {
        self.u64(s.len() as u64);
        self.bytes(s.as_bytes());
    }
    fn opt(&mut self, v: Option<&[f64]>) {
        match v {
            Some(v) => {
                self.u64(1);
                self.fs(v);
            }
            None => self.u64(0),
        }
    }
    fn pose(&mut self, p: &Pose) {
        self.fs(&p.p);
        self.fs(&p.q);
    }
}

/// A mesh's parameters as they matter to a collider: the drawn size
/// (`screen`) is left out, so a camera move does not make a bake stale.
pub fn collider_params(p: &MeshParams) -> MeshParams {
    MeshParams { screen: 1.0, deform: None, ..p.clone() }
}

impl SimInput {
    /// Whether there is anything to bake.
    pub fn is_empty(&self) -> bool {
        self.crowds.is_empty() && !self.bodies.iter().any(|b| b.kind == BodyKind::Dynamic)
    }

    /// A hash of everything in it.
    pub fn hash(&self) -> u64 {
        let mut h = Fnv::default();
        h.str("lahza-sim-3");
        h.str(&self.scene);
        h.fs(&[self.start, self.rate, self.samples as f64, self.substeps as f64, self.unit, self.jitter]);
        h.fs(&self.gravity);
        h.u64(self.seed);
        h.u64(self.flat as u64);
        h.opt(self.ground.as_ref().map(|g| [g.y, g.friction, g.bounce]).as_ref().map(|v| &v[..]));
        h.u64(self.bodies.len() as u64);
        for b in &self.bodies {
            h.str(&b.id);
            h.u64(b.kind as u64);
            h.u64(b.colliders.len() as u64);
            for c in &b.colliders {
                match &c.shape {
                    ShapeInput::Box { half } => {
                        h.u64(1);
                        h.fs(half);
                    }
                    ShapeInput::Ball { radius } => h.fs(&[2.0, *radius]),
                    ShapeInput::Capsule { radius, half } => h.fs(&[3.0, *radius, *half]),
                    ShapeInput::Cylinder { radius, half } => h.fs(&[4.0, *radius, *half]),
                    ShapeInput::Hull(p) => {
                        h.u64(5);
                        h.str(&format!("{:?}", collider_params(p)));
                    }
                    ShapeInput::Trimesh(p) => {
                        h.u64(6);
                        h.str(&format!("{:?}", collider_params(p)));
                    }
                    ShapeInput::Pieces(p) => {
                        h.u64(7);
                        h.str(&format!("{:?}", collider_params(p)));
                    }
                }
                h.fs(&c.local.0);
            }
            h.opt(b.mass.as_ref().map(std::slice::from_ref));
            h.fs(&[b.density, b.friction, b.bounce, b.damping, b.spin_damping, b.release as f64, b.ccd as u8 as f64, b.group as f64]);
            h.fs(&b.velocity);
            h.fs(&b.spin);
            h.u64(b.keyed.len() as u64);
            for p in &b.keyed {
                h.pose(p);
            }
            h.pose(&b.before);
            h.fs(&b.rest.0);
            h.u64(b.pushes.len() as u64);
            for p in &b.pushes {
                h.fs(&[p.at, p.dur]);
                h.fs(&p.velocity);
                h.fs(&p.spin);
                h.opt(p.point.as_ref().map(|v| &v[..]));
            }
            // (Only where there are any: older bakes keep their hash.)
            if !b.apart.is_empty() {
                h.str("apart");
                h.fs(&b.apart.iter().map(|&i| i as f64).collect::<Vec<_>>());
            }
            if !b.lives.is_empty() {
                h.str("lives");
                for l in &b.lives {
                    h.fs(&[l.from as f64, l.to as f64]);
                    h.pose(&l.pose);
                    h.pose(&l.next);
                }
            }
        }
        h.u64(self.joints.len() as u64);
        for j in &self.joints {
            h.str(&j.id);
            h.u64(j.kind as u64);
            h.u64(j.a as u64);
            h.u64(j.b.map(|b| b as u64 + 1).unwrap_or(0));
            h.fs(&j.anchor_a);
            h.fs(&j.anchor_b);
            h.fs(&j.axis_a);
            h.fs(&j.axis_b);
            h.opt(j.limits.as_ref().map(|v| &v[..]));
            h.opt(j.swing.as_ref().map(std::slice::from_ref));
            h.opt(j.twist.as_ref().map(|v| &v[..]));
            h.fs(&[j.length, j.stiffness, j.damping, j.collide as u8 as f64]);
            match &j.motor {
                Some(m) => {
                    h.fs(&m.target);
                    h.fs(&[m.stiffness, m.damping, m.max_force.unwrap_or(-1.0)]);
                }
                None => h.u64(0),
            }
        }
        h.u64(self.blasts.len() as u64);
        for b in &self.blasts {
            h.fs(&[b.at, b.speed, b.radius]);
            h.fs(&b.origin);
        }
        h.u64(self.ragdolls.len() as u64);
        for r in &self.ragdolls {
            h.str(&r.id);
            h.u64(r.release as u64);
            h.u64(r.bones.len() as u64);
            for b in &r.bones {
                h.u64(b.body as u64);
                h.u64(b.parent.map(|p| p as u64 + 1).unwrap_or(0));
                h.fs(&b.anchor_parent);
                h.fs(&b.along);
                h.fs(&[b.length, b.swing, b.twist, b.mass]);
                h.opt(b.hinge.as_ref().map(|v| &v[..]));
                h.fs(&b.bind);
                h.u64(b.clip.len() as u64);
                for q in &b.clip {
                    h.fs(q);
                }
                h.fs(&b.strength);
            }
            h.u64(r.behaviours.len() as u64);
            for b in &r.behaviours {
                h.u64(b.kind as u64);
                h.fs(&[b.from, b.to]);
                h.fs(&b.amount);
                h.str(&serde_json::to_string(&b.params).unwrap_or_default());
                h.u64(b.target_body.map(|p| p as u64 + 1).unwrap_or(0));
            }
            h.u64(r.self_collide as u64);
            h.u64(r.hits.len() as u64);
            for (t, bone, v, dur) in &r.hits {
                h.fs(&[*t, *bone as f64, *dur]);
                h.fs(v);
            }
            h.str(&r.controller.as_ref().map(|c| serde_json::to_string(c).unwrap_or_default()).unwrap_or_default());
        }
        h.u64(self.crowds.len() as u64);
        for c in &self.crowds {
            h.str(&c.id);
            h.fs(&[c.speed, c.vary, c.radius, c.height, c.stride, c.cycle, c.clip_at, c.knock, c.dodge, c.plan as u8 as f64]);
            h.fs(&c.facing);
            h.u64(c.instanced as u64);
            h.u64(c.pool.len() as u64);
            for r in &c.pool {
                h.u64(*r as u64);
            }
            for bone in &c.local {
                h.u64(bone.len() as u64);
                for p in bone {
                    h.pose(p);
                }
            }
            h.u64(c.agents.len() as u64);
            for a in &c.agents {
                h.str(&a.id);
                h.pose(&a.start);
                h.fs(&a.rest.0);
                h.fs(&a.goal);
                h.u64(a.ragdoll.map(|r| r as u64 + 1).unwrap_or(0));
                for bone in &a.local {
                    h.u64(bone.len() as u64);
                    for p in bone {
                        h.pose(p);
                    }
                }
            }
        }
        h.u64(self.files.len() as u64);
        for (path, hash) in &self.files {
            h.str(path);
            h.u64(*hash);
        }
        h.0
    }
}

/// A file's hash (0 when it cannot be read).
pub fn file_hash(path: &Path) -> u64 {
    let mut h = Fnv::default();
    match std::fs::read(path) {
        Ok(bytes) => h.bytes(&bytes),
        Err(_) => return 0,
    }
    h.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decompose_gives_back_the_matrix() {
        let m = Mat4::translate(30.0, -20.0, 5.0).then(Mat4::rotate_y(25.0)).then(Mat4::rotate_x(-40.0)).then(Mat4::rotate_z(70.0)).then(Mat4::scale(2.0, 0.5, 3.0)).then(Mat4::translate(-4.0, 6.0, 0.0));
        let (pose, rest) = decompose(m);
        let back = pose.matrix().then(rest);
        for k in 0..16 {
            assert!((back.0[k] - m.0[k]).abs() < 1e-9, "{k}: {} vs {}", back.0[k], m.0[k]);
        }
        // What is left has no rotation: its axes are the scale's.
        assert!((rest.0[0] - 2.0).abs() < 1e-9 && (rest.0[5] - 0.5).abs() < 1e-9 && (rest.0[10] - 3.0).abs() < 1e-9);
        assert!(rest.0[1].abs() < 1e-9 && rest.0[4].abs() < 1e-9);
        let inv = inverse(m).then(m);
        for k in 0..16 {
            assert!((inv.0[k] - Mat4::IDENTITY.0[k]).abs() < 1e-9);
        }
    }

    #[test]
    fn poses_compose_and_invert() {
        let a = decompose(Mat4::translate(1.0, 2.0, 3.0).then(Mat4::rotate_x(30.0)).then(Mat4::rotate_z(50.0))).0;
        let b = decompose(Mat4::translate(-4.0, 0.5, 2.0).then(Mat4::rotate_y(-70.0))).0;
        let ab = a.then(&b);
        let m = a.matrix().then(b.matrix());
        for k in 0..16 {
            assert!((ab.matrix().0[k] - m.0[k]).abs() < 1e-9);
        }
        let id = a.then(&a.inverse()).matrix();
        for k in 0..16 {
            assert!((id.0[k] - Mat4::IDENTITY.0[k]).abs() < 1e-9);
        }
    }

    #[test]
    fn a_bake_survives_its_file_and_samples_between_frames() {
        let half = std::f32::consts::FRAC_1_SQRT_2;
        let bake = Bake {
            hash: 42,
            start: 1.0,
            rate: 30.0,
            tracks: vec![Track { id: "box".into(), from: 1, rest: Mat4::scale(2.0, 2.0, 2.0), samples: vec![[0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0], [10.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0], [20.0, 4.0, 0.0, 0.0, 0.0, half, half]], clock: vec![0.0, 0.5, 1.0] }],
            events: vec![SimEvent { t: 1.2, kind: "touch".into(), a: "box".into(), b: "floor".into(), speed: 120.0 }],
            herds: Vec::new(),
        };
        let back = Bake::from_bytes(&bake.to_bytes()).unwrap();
        assert_eq!(back, bake);
        // A crowd of instances comes after the rest, when there is one:
        // two agents, three samples; the second turns the short way
        // round through ±π, and its clip wraps without running back.
        let (pi, y) = (std::f32::consts::PI, 800.0);
        let herd = HerdTrack { id: "walkers".into(), count: 2, rest: Mat4::scale(3.0, 3.0, 3.0), y, q: [0.0, 0.0, 0.0, 1.0], samples: vec![[0.0, 0.0, 0.0, 0.2], [5.0, 5.0, pi - 0.1, 0.9], [10.0, 0.0, 0.0, 0.3], [5.0, 9.0, 0.1 - pi, 0.1], [20.0, 0.0, 0.0, 0.4], [5.0, 13.0, 0.1 - pi, 0.2]], down: vec![u32::MAX, 2], slot: vec![u32::MAX, 0] };
        let with = Bake { herds: vec![herd], ..bake.clone() };
        let bytes = with.to_bytes();
        assert_eq!(Bake::from_bytes(&bytes).unwrap(), with);
        assert!(bytes.len() > bake.to_bytes().len() && Bake::from_bytes(&bytes[..bytes.len() - 3]).is_err());
        let h = &with.herds[0];
        assert_eq!(h.len(), 3);
        let (pose, clock, turn) = h.agent(0, 0.5).unwrap();
        assert!(pose.p == [5.0, 800.0, 0.0] && (clock - 0.25).abs() < 1e-6 && turn == 0.0);
        let (pose, clock, turn) = h.agent(1, 0.5).unwrap();
        assert!((pose.p[2] - 7.0).abs() < 1e-9 && (clock - 0.9).abs() < 1e-6 && (turn.abs() - std::f64::consts::PI).abs() < 1e-6, "{pose:?} {clock} {turn}");
        assert!(h.agent(1, 9.0).unwrap().0.p[2] == 13.0);
        assert!(Bake::from_bytes(&bake.to_bytes()[..40]).is_err());
        // Before the release the keys stand; after the last sample it holds.
        assert!(bake.world(0, 1.0).is_none());
        let m = bake.world(0, 1.0 + 1.5 / 30.0).unwrap();
        assert!((m.0[3] - 15.0).abs() < 1e-6 && (m.0[7] - 2.0).abs() < 1e-6 && (crate::dm::hypot(m.0[0], m.0[4]) - 2.0).abs() < 1e-6);
        let (held, last) = (bake.world(0, 9.0).unwrap(), bake.world(0, 1.0 + 2.0 / 30.0).unwrap());
        assert!((0..16).all(|k| (held.0[k] - last.0[k]).abs() < 1e-9));
    }
}
