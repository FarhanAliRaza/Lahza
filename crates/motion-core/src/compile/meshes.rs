//! Mesh layers, materials and the scene's lights: compiled once, evaluated
//! per frame into `MeshState` (geometry parameters, placement, camera and
//! resolved materials) and a shared `Lighting`.

use super::*;
use crate::mesh::{Caps, MeshParams, MeshShape, SweepProfile, PARTS};

/// A mesh layer at one moment.
#[derive(Clone, Debug, PartialEq)]
pub struct MeshState {
    pub params: Arc<MeshParams>,
    /// Layer space → world (parents and comps included).
    pub world: Mat4,
    pub camera: Camera3,
    /// One per part (`mesh::PART_*`).
    pub materials: [MaterialState; PARTS],
    /// Per-segment colours; a part whose own material sets a colour keeps it.
    pub colors: Vec<Color>,
    pub own_color: [bool; PARTS],
    /// Per-segment colours of each part (over `colors` and `own_color`).
    pub part_colors: [Vec<Color>; PARTS],
    /// The scene's lights (set for every mesh of a frame).
    pub lighting: Option<Arc<Lighting>>,
    /// Depth of field: [focus depth (camera z), blur px per px of depth].
    pub lens: Option<[f64; 2]>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MaterialState {
    pub color: Color,
    pub metallic: f64,
    pub roughness: f64,
    pub clearcoat: f64,
    pub clearcoat_roughness: f64,
    /// Emitted light (straight colour × strength).
    pub emissive: [f64; 3],
    pub opacity: f64,
    pub transmission: f64,
    pub ior: f64,
    pub thickness: f64,
    pub unlit: bool,
    pub faceted: bool,
    /// Shows only the shadows falling on it.
    pub shadow_catcher: bool,
    /// Edge width in px (0: a surface).
    pub wireframe: f64,
    /// Wireframe face colour (hidden-line); `None`: see-through wires.
    pub wire_fill: Option<Color>,
    /// Dielectric reflection (0.5: 4% head-on).
    pub specular: f64,
    pub specular_tint: Color,
    pub anisotropy: f64,
    /// Radians.
    pub anisotropy_rotation: f64,
    pub sheen: f64,
    pub sheen_color: Color,
    pub sheen_roughness: f64,
    /// Subsurface weight, per-channel radius (relative), its scale (px),
    /// anisotropy.
    pub subsurface: f64,
    pub subsurface_radius: [f64; 3],
    pub subsurface_scale: f64,
    pub subsurface_anisotropy: f64,
    pub clearcoat_color: Color,
    /// 1 / Abbe number.
    pub dispersion: f64,
    pub noise: Option<NoiseState>,
    /// Image maps (project paths).
    pub map: Option<Arc<str>>,
    pub normal_map: Option<Arc<str>>,
    pub bump_map: Option<Arc<str>>,
    /// Scale roughness (green channel) and metallic (blue channel).
    pub roughness_map: Option<Arc<str>>,
    pub metallic_map: Option<Arc<str>>,
    /// Multiplies `emissive` (sRGB colour image).
    pub emissive_map: Option<Arc<str>>,
    /// bump_map's height, px.
    pub bump: f64,
    /// UV scale x, y, offset x, y, rotation (radians).
    pub uv: [f64; 5],
    /// A volume instead of a surface.
    pub volume: Option<VolumeState>,
    /// The volume's density grid at this moment: its image and cells.
    pub volume_grid: Option<(Arc<str>, [u32; 3])>,
}

/// A medium at a moment: extinction per px, scattering colour (linear),
/// anisotropy, emission (linear radiance of a thick part).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VolumeState {
    pub sigma: f64,
    pub albedo: [f64; 3],
    pub g: f64,
    pub emission: [f64; 3],
    /// With the material's noise: the share of it that is medium, and the
    /// fade at its edges (noise units).
    pub coverage: f64,
    pub softness: f64,
    /// Noise and grid volumes: marched as the preview draws them (true) or
    /// walked exactly.
    pub artistic: bool,
    /// A cumulus form, and its up direction (mesh space, unit).
    pub cumulus: bool,
    pub up: [f64; 3],
}

/// Surface noise at a moment (see `document::SurfaceNoise`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NoiseState {
    pub field: crate::noise::Field,
    pub world: bool,
    /// The colour at the noise's peaks and how far toward it.
    pub color: Option<Color>,
    pub amount: f64,
    pub roughness: f64,
    pub bump: f64,
}

impl MaterialState {
    pub fn preset(p: MaterialPreset) -> Self {
        let base = MaterialState {
            color: Color::WHITE,
            metallic: 0.0,
            roughness: 0.35,
            clearcoat: 0.0,
            clearcoat_roughness: 0.06,
            emissive: [0.0; 3],
            opacity: 1.0,
            transmission: 0.0,
            ior: 1.45,
            thickness: 60.0,
            unlit: false,
            faceted: false,
            shadow_catcher: false,
            wireframe: 0.0,
            wire_fill: None,
            specular: 0.5,
            specular_tint: Color::WHITE,
            anisotropy: 0.0,
            anisotropy_rotation: 0.0,
            sheen: 0.0,
            sheen_color: Color::WHITE,
            sheen_roughness: 0.5,
            subsurface: 0.0,
            subsurface_radius: [1.0, 0.2, 0.1],
            subsurface_scale: 5.0,
            subsurface_anisotropy: 0.0,
            clearcoat_color: Color::WHITE,
            dispersion: 0.0,
            noise: None,
            map: None,
            normal_map: None,
            bump_map: None,
            roughness_map: None,
            metallic_map: None,
            emissive_map: None,
            bump: 2.0,
            uv: [1.0, 1.0, 0.0, 0.0, 0.0],
            volume: None,
            volume_grid: None,
        };
        match p {
            MaterialPreset::Matte => MaterialState { roughness: 0.85, ..base },
            MaterialPreset::Plastic => MaterialState { roughness: 0.45, ..base },
            MaterialPreset::Glossy => MaterialState { roughness: 0.22, clearcoat: 0.35, ..base },
            MaterialPreset::Coated => MaterialState { roughness: 0.45, clearcoat: 1.0, clearcoat_roughness: 0.04, ..base },
            MaterialPreset::Metal => MaterialState { metallic: 1.0, roughness: 0.32, ..base },
            MaterialPreset::Chrome => MaterialState { metallic: 1.0, roughness: 0.06, ..base },
            MaterialPreset::Brushed => MaterialState { metallic: 1.0, roughness: 0.3, anisotropy: 0.8, ..base },
            MaterialPreset::Velvet => MaterialState { roughness: 0.9, specular: 0.2, sheen: 1.0, sheen_roughness: 0.4, ..base },
            MaterialPreset::Liquid => MaterialState {
                roughness: 0.02,
                transmission: 1.0,
                ior: 1.33,
                thickness: 400.0,
                color: Color([0.92, 0.97, 1.0, 1.0]),
                noise: Some(NoiseState {
                    field: crate::noise::Field { kind: crate::noise::NoiseKind::Fbm, scale: 80.0, octaves: 3, seed: 0, offset: [0.0; 3] },
                    world: false,
                    color: None,
                    amount: 0.0,
                    roughness: 0.0,
                    bump: 2.0,
                }),
                ..base
            },
            MaterialPreset::Glass => MaterialState { roughness: 0.04, transmission: 0.92, ..base },
            MaterialPreset::Emissive => MaterialState { roughness: 0.5, ..base },
            MaterialPreset::Flat => MaterialState { unlit: true, ..base },
            MaterialPreset::Shadow => MaterialState { color: Color::BLACK, opacity: 0.55, shadow_catcher: true, ..base },
            MaterialPreset::Wireframe => MaterialState { color: Color([0.86, 0.89, 0.94, 1.0]), unlit: true, wireframe: 1.5, ..base },
        }
    }
}

/// Lights and environment in world space, colours linear.
#[derive(Clone, Debug, PartialEq)]
pub struct Lighting {
    pub lights: Vec<LightState>,
    pub env: EnvState,
    /// Tone mapping of lit content: 0 standard, 1 filmic, 2 AgX.
    pub tone: u32,
    /// Exposure as a multiplier (2^stops).
    pub exposure: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LightState {
    pub kind: LightKind,
    /// Directional: unit vector toward the light; point: its position.
    pub vector: [f64; 3],
    /// Linear colour × intensity.
    pub color: [f64; 3],
    pub shadow: bool,
    /// 0 sharp … 1 very soft.
    pub softness: f64,
    /// Point lights: px where the light has faded out (0: no falloff).
    pub range: f64,
    /// Area lights: half extents across the panel (world px); u × v
    /// points the way it shines.
    pub u: [f64; 3],
    pub v: [f64; 3],
    pub disc: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EnvState {
    /// Linear colours × intensity (up = −y).
    pub sky: [f64; 3],
    pub horizon: [f64; 3],
    pub ground: [f64; 3],
    pub lobes: Vec<Lobe>,
    /// An equirectangular image lighting the scene (project-relative
    /// path), with its intensity and turn about the vertical (radians).
    pub image: Option<std::sync::Arc<str>>,
    pub image_intensity: f64,
    pub image_rotation: f64,
}

/// A soft bright area of the environment (a softbox or the sun).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lobe {
    /// Unit direction toward it, world space.
    pub dir: [f64; 3],
    /// Angular radius, radians.
    pub size: f64,
    pub color: [f64; 3],
}

pub fn srgb_to_linear(v: f64) -> f64 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

fn linear(c: Color, k: f64) -> [f64; 3] {
    [srgb_to_linear(c.0[0]) * k, srgb_to_linear(c.0[1]) * k, srgb_to_linear(c.0[2]) * k]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn unit(v: [f64; 3]) -> [f64; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l > 1e-12 {
        [v[0] / l, v[1] / l, v[2] / l]
    } else {
        [0.0, -1.0, 0.0]
    }
}

pub(super) struct CMaterial {
    preset: MaterialPreset,
    color: Option<Prop<Color>>,
    metallic: Option<Prop<f64>>,
    roughness: Option<Prop<f64>>,
    clearcoat: Option<Prop<f64>>,
    clearcoat_roughness: Option<Prop<f64>>,
    emissive: Option<Prop<Color>>,
    emissive_strength: Option<Prop<f64>>,
    opacity: Option<Prop<f64>>,
    transmission: Option<Prop<f64>>,
    ior: Option<Prop<f64>>,
    thickness: Option<Prop<f64>>,
    unlit: Option<bool>,
    faceted: Option<bool>,
    wireframe: Option<Prop<f64>>,
    fill: Option<Prop<Color>>,
    specular: Option<Prop<f64>>,
    specular_tint: Option<Prop<Color>>,
    anisotropy: Option<Prop<f64>>,
    anisotropy_rotation: Option<Prop<f64>>,
    sheen: Option<Prop<f64>>,
    sheen_color: Option<Prop<Color>>,
    sheen_roughness: Option<Prop<f64>>,
    subsurface: Option<Prop<f64>>,
    subsurface_radius: Option<[f64; 3]>,
    subsurface_scale: Option<Prop<f64>>,
    subsurface_anisotropy: Option<Prop<f64>>,
    clearcoat_color: Option<Prop<Color>>,
    dispersion: Option<Prop<f64>>,
    noise: Option<CNoise>,
    map: Option<Arc<str>>,
    normal_map: Option<Arc<str>>,
    bump_map: Option<Arc<str>>,
    roughness_map: Option<Arc<str>>,
    metallic_map: Option<Arc<str>>,
    emissive_map: Option<Arc<str>>,
    bump: Option<Prop<f64>>,
    uv: Option<(Prop<crate::value::Vec2>, Prop<crate::value::Vec2>, Prop<f64>)>,
    volume: Option<CVolume>,
}

pub(super) struct CVolume {
    density: Prop<f64>,
    color: Option<Prop<Color>>,
    anisotropy: Prop<f64>,
    emission: Option<Prop<Color>>,
    emission_strength: Prop<f64>,
    coverage: Option<Prop<f64>>,
    softness: Option<Prop<f64>>,
    grid: Option<(String, [u32; 3], Prop<f64>)>,
    model: crate::document::VolumeModel,
    shape: crate::document::VolumeShape,
    up: [f64; 3],
}

pub(super) struct CNoise {
    drift: Option<[f64; 3]>,
    kind: crate::noise::NoiseKind,
    scale: Prop<f64>,
    octaves: u32,
    seed: u32,
    world: bool,
    color: Option<Prop<Color>>,
    amount: Prop<f64>,
    roughness: Option<Prop<f64>>,
    bump: Option<Prop<f64>>,
}

impl CMaterial {
    fn at(&self, t: f64) -> MaterialState {
        let mut m = MaterialState::preset(self.preset);
        let f = |p: &Option<Prop<f64>>, d: f64, lo: f64, hi: f64| p.as_ref().map(|p| p.at(t)).unwrap_or(d).clamp(lo, hi);
        if let Some(c) = &self.color {
            m.color = clamp_color(c.at(t));
        }
        m.metallic = f(&self.metallic, m.metallic, 0.0, 1.0);
        m.roughness = f(&self.roughness, m.roughness, 0.02, 1.0);
        m.clearcoat = f(&self.clearcoat, m.clearcoat, 0.0, 1.0);
        m.clearcoat_roughness = f(&self.clearcoat_roughness, m.clearcoat_roughness, 0.02, 1.0);
        m.opacity = f(&self.opacity, m.opacity, 0.0, 1.0);
        m.transmission = f(&self.transmission, m.transmission, 0.0, 1.0);
        m.ior = f(&self.ior, m.ior, 1.0, 3.0);
        m.thickness = f(&self.thickness, m.thickness, 0.0, 10_000.0);
        let strength = f(&self.emissive_strength, 1.0, 0.0, 100.0);
        let emissive = match (&self.emissive, self.preset) {
            (Some(c), _) => Some(clamp_color(c.at(t))),
            (None, MaterialPreset::Emissive) => Some(m.color),
            (None, _) if self.emissive_map.is_some() => Some(Color([1.0; 4])),
            _ => None,
        };
        if let Some(e) = emissive {
            m.emissive = [e.0[0] * e.0[3] * strength, e.0[1] * e.0[3] * strength, e.0[2] * e.0[3] * strength];
        }
        m.wireframe = f(&self.wireframe, m.wireframe, 0.0, 200.0);
        if let Some(fill) = &self.fill {
            m.wire_fill = Some(clamp_color(fill.at(t)));
        }
        m.unlit = self.unlit.unwrap_or(m.unlit);
        m.faceted = self.faceted.unwrap_or(m.faceted);
        m.specular = f(&self.specular, m.specular, 0.0, 1.0);
        m.anisotropy = f(&self.anisotropy, m.anisotropy, 0.0, 1.0);
        m.anisotropy_rotation = self.anisotropy_rotation.as_ref().map(|p| p.at(t)).unwrap_or(0.0).to_radians();
        m.sheen = f(&self.sheen, m.sheen, 0.0, 1.0);
        m.sheen_roughness = f(&self.sheen_roughness, m.sheen_roughness, 0.05, 1.0);
        m.subsurface = f(&self.subsurface, m.subsurface, 0.0, 1.0);
        m.subsurface_radius = self.subsurface_radius.map(|r| r.map(|v| v.max(0.0))).unwrap_or(m.subsurface_radius);
        m.subsurface_scale = f(&self.subsurface_scale, m.subsurface_scale, 0.0, 1e5);
        m.subsurface_anisotropy = f(&self.subsurface_anisotropy, m.subsurface_anisotropy, -0.95, 0.95);
        m.dispersion = f(&self.dispersion, m.dispersion, 0.0, 0.5);
        let colour = |p: &Option<Prop<Color>>, d: Color| p.as_ref().map(|p| clamp_color(p.at(t))).unwrap_or(d);
        m.specular_tint = colour(&self.specular_tint, m.specular_tint);
        m.sheen_color = colour(&self.sheen_color, m.sheen_color);
        m.clearcoat_color = colour(&self.clearcoat_color, m.clearcoat_color);
        // A material's own noise replaces its preset's (the liquid ripple).
        if let Some(n) = self.noise.as_ref().map(|n| NoiseState {
            field: crate::noise::Field { kind: n.kind, scale: n.scale.at(t).max(0.1), octaves: n.octaves.clamp(1, 8), seed: n.seed, offset: n.drift.map(|d| [d[0] * t, d[1] * t, d[2] * t]).unwrap_or([0.0; 3]) },
            world: n.world,
            color: n.color.as_ref().map(|c| clamp_color(c.at(t))),
            amount: n.amount.at(t).clamp(0.0, 1.0),
            roughness: n.roughness.as_ref().map(|r| r.at(t)).unwrap_or(0.0).clamp(-1.0, 1.0),
            bump: n.bump.as_ref().map(|b| b.at(t)).unwrap_or(0.0).clamp(-1000.0, 1000.0),
        }) {
            m.noise = Some(n);
        }
        m.volume_grid = self.volume.as_ref().and_then(|v| v.grid.as_ref()).map(|(src, size, frame)| (Arc::from(frame_path(src, frame.at(t))), *size));
        m.volume = self.volume.as_ref().map(|v| {
            let lin = |c: Color, k: f64| linear(c, k * c.0[3]);
            VolumeState {
                sigma: v.density.at(t).max(0.0) / 100.0,
                albedo: lin(v.color.as_ref().map(|c| clamp_color(c.at(t))).unwrap_or(Color::WHITE), 1.0),
                g: v.anisotropy.at(t).clamp(-0.95, 0.95),
                emission: v.emission.as_ref().map(|c| lin(clamp_color(c.at(t)), v.emission_strength.at(t).max(0.0))).unwrap_or([0.0; 3]),
                coverage: v.coverage.as_ref().map_or(0.5, |p| p.at(t).clamp(0.0, 1.0)),
                softness: v.softness.as_ref().map_or(0.1, |p| p.at(t).clamp(0.01, 1.0)),
                artistic: v.model == crate::document::VolumeModel::Artistic,
                cumulus: v.shape == crate::document::VolumeShape::Cumulus,
                up: v.up,
            }
        });
        m.map = self.map.clone();
        m.normal_map = self.normal_map.clone();
        m.bump_map = self.bump_map.clone();
        m.roughness_map = self.roughness_map.clone();
        m.metallic_map = self.metallic_map.clone();
        m.emissive_map = self.emissive_map.clone();
        m.bump = f(&self.bump, m.bump, -1000.0, 1000.0);
        if let Some((sc, off, rot)) = &self.uv {
            let (sc, off) = (sc.at(t), off.at(t));
            m.uv = [sc.x(), sc.y(), off.x(), off.y(), rot.at(t).to_radians()];
        }
        m
    }
}

pub(super) enum CMeshShape {
    Sphere(Prop<f64>),
    Plane(Prop<Vec2>),
    Box(Prop<Vec2>, Prop<f64>, Prop<f64>),
    Cylinder(Prop<f64>, Option<Prop<f64>>, Prop<f64>, Prop<f64>),
    Capsule(Prop<f64>, Prop<f64>),
    Torus(Prop<f64>, Prop<f64>, Prop<Vec2>, Caps),
    Extrude((Vec<Arc<str>>, Prop<f64>), Prop<f64>, Prop<f64>),
    Revolve((Vec<Arc<str>>, Prop<f64>), Prop<Vec2>, bool),
    Sweep { path: (Vec<Arc<str>>, Prop<f64>), radius: Prop<f64>, width: Prop<f64>, profile: SweepProfile, trim: Prop<Vec2>, caps: Caps, head: Prop<f64> },
    Gltf { src: Arc<str>, mesh: u32, primitive: u32, unit: f64, frame: Prop<f64>, bounds: Option<[[f64; 3]; 2]> },
}

pub(super) struct CMesh {
    shape: CMeshShape,
    materials: Vec<CMaterial>,
    own_color: [bool; PARTS],
    colors: Vec<Prop<Color>>,
    part_colors: Vec<Vec<Prop<Color>>>,
    spans: Vec<f64>,
    segments: u32,
    gap: Prop<f64>,
    detail: f64,
    displace: Option<(crate::noise::NoiseKind, Prop<f64>, u32, u32, Prop<f64>, [f64; 3], Option<String>)>,
    /// Last parameters, so unchanged frames share one `Arc` (and one
    /// cached mesh in the renderer).
    last: std::sync::Mutex<Option<Arc<MeshParams>>>,
}

fn caps(c: MeshCaps) -> Caps {
    match c {
        MeshCaps::Flat => Caps::Flat,
        MeshCaps::Round => Caps::Round,
    }
}

/// `over` with unset fields taken from `base`.
fn merge(base: &Material, over: Option<&Material>) -> Material {
    let Some(o) = over else { return base.clone() };
    Material {
        preset: o.preset.or(base.preset),
        color: o.color.clone().or_else(|| base.color.clone()),
        metallic: o.metallic.clone().or_else(|| base.metallic.clone()),
        roughness: o.roughness.clone().or_else(|| base.roughness.clone()),
        clearcoat: o.clearcoat.clone().or_else(|| base.clearcoat.clone()),
        clearcoat_roughness: o.clearcoat_roughness.clone().or_else(|| base.clearcoat_roughness.clone()),
        emissive: o.emissive.clone().or_else(|| base.emissive.clone()),
        emissive_strength: o.emissive_strength.clone().or_else(|| base.emissive_strength.clone()),
        colors: if o.colors.is_empty() { base.colors.clone() } else { o.colors.clone() },
        wireframe: o.wireframe.clone().or_else(|| base.wireframe.clone()),
        fill: o.fill.clone().or_else(|| base.fill.clone()),
        opacity: o.opacity.clone().or_else(|| base.opacity.clone()),
        transmission: o.transmission.clone().or_else(|| base.transmission.clone()),
        ior: o.ior.clone().or_else(|| base.ior.clone()),
        thickness: o.thickness.clone().or_else(|| base.thickness.clone()),
        unlit: o.unlit.or(base.unlit),
        faceted: o.faceted.or(base.faceted),
        specular: o.specular.clone().or_else(|| base.specular.clone()),
        specular_tint: o.specular_tint.clone().or_else(|| base.specular_tint.clone()),
        anisotropy: o.anisotropy.clone().or_else(|| base.anisotropy.clone()),
        anisotropy_rotation: o.anisotropy_rotation.clone().or_else(|| base.anisotropy_rotation.clone()),
        sheen: o.sheen.clone().or_else(|| base.sheen.clone()),
        sheen_color: o.sheen_color.clone().or_else(|| base.sheen_color.clone()),
        sheen_roughness: o.sheen_roughness.clone().or_else(|| base.sheen_roughness.clone()),
        subsurface: o.subsurface.clone().or_else(|| base.subsurface.clone()),
        subsurface_radius: o.subsurface_radius.or(base.subsurface_radius),
        subsurface_scale: o.subsurface_scale.clone().or_else(|| base.subsurface_scale.clone()),
        subsurface_anisotropy: o.subsurface_anisotropy.clone().or_else(|| base.subsurface_anisotropy.clone()),
        clearcoat_color: o.clearcoat_color.clone().or_else(|| base.clearcoat_color.clone()),
        dispersion: o.dispersion.clone().or_else(|| base.dispersion.clone()),
        noise: o.noise.clone().or_else(|| base.noise.clone()),
        map: o.map.clone().or_else(|| base.map.clone()),
        normal_map: o.normal_map.clone().or_else(|| base.normal_map.clone()),
        bump_map: o.bump_map.clone().or_else(|| base.bump_map.clone()),
        roughness_map: o.roughness_map.clone().or_else(|| base.roughness_map.clone()),
        metallic_map: o.metallic_map.clone().or_else(|| base.metallic_map.clone()),
        emissive_map: o.emissive_map.clone().or_else(|| base.emissive_map.clone()),
        bump: o.bump.clone().or_else(|| base.bump.clone()),
        uv: o.uv.clone().or_else(|| base.uv.clone()),
        volume: o.volume.clone().or_else(|| base.volume.clone()),
    }
}

impl Compiler<'_> {
    /// Path data as morph keys: the paths and the morph index property.
    pub(super) fn path_keys(&mut self, d: &PathData, id: &str, what: &str, start: f64, step: Option<Step>, shift: f64) -> (Vec<Arc<str>>, Prop<f64>) {
        match d {
            PathData::Static(v) => (vec![v.svg().into()], Prop::constant(0.0)),
            PathData::Keys(keys) => {
                let paths: Vec<Arc<str>> = keys.iter().map(|k| k.v.svg().into()).collect();
                let index = Property::Animated(crate::property::Animated {
                    keys: keys
                        .iter()
                        .enumerate()
                        .map(|(i, k)| crate::property::Key {
                            at: k.at.clone(),
                            v: i as f64,
                            from: None,
                            ease: k.ease.clone(),
                            anchor: k.anchor,
                            lead: k.lead,
                            dur: k.dur,
                            space: None,
                        })
                        .collect(),
                    add: Vec::new(),
                    step_fps: None,
                    base: None,
                });
                (paths, self.prop(&index, id, what, start, step, shift))
            }
        }
    }

    fn material(&mut self, m: &Material, id: &str, what: &str, start: f64, step: Option<Step>, shift: f64) -> CMaterial {
        let w = |f: &str| format!("{what}.{f}");
        CMaterial {
            preset: m.preset.unwrap_or_default(),
            color: m.color.as_ref().map(|p| self.prop(p, id, &w("color"), start, step, shift)),
            metallic: m.metallic.as_ref().map(|p| self.prop(p, id, &w("metallic"), start, step, shift)),
            roughness: m.roughness.as_ref().map(|p| self.prop(p, id, &w("roughness"), start, step, shift)),
            clearcoat: m.clearcoat.as_ref().map(|p| self.prop(p, id, &w("clearcoat"), start, step, shift)),
            clearcoat_roughness: m.clearcoat_roughness.as_ref().map(|p| self.prop(p, id, &w("clearcoat_roughness"), start, step, shift)),
            emissive: m.emissive.as_ref().map(|p| self.prop(p, id, &w("emissive"), start, step, shift)),
            emissive_strength: m.emissive_strength.as_ref().map(|p| self.prop(p, id, &w("emissive_strength"), start, step, shift)),
            opacity: m.opacity.as_ref().map(|p| self.prop(p, id, &w("opacity"), start, step, shift)),
            transmission: m.transmission.as_ref().map(|p| self.prop(p, id, &w("transmission"), start, step, shift)),
            ior: m.ior.as_ref().map(|p| self.prop(p, id, &w("ior"), start, step, shift)),
            thickness: m.thickness.as_ref().map(|p| self.prop(p, id, &w("thickness"), start, step, shift)),
            unlit: m.unlit,
            faceted: m.faceted,
            wireframe: m.wireframe.as_ref().map(|p| self.prop(p, id, &w("wireframe"), start, step, shift)),
            fill: m.fill.as_ref().map(|p| self.prop(p, id, &w("fill"), start, step, shift)),
            specular: m.specular.as_ref().map(|p| self.prop(p, id, &w("specular"), start, step, shift)),
            specular_tint: m.specular_tint.as_ref().map(|p| self.prop(p, id, &w("specular_tint"), start, step, shift)),
            anisotropy: m.anisotropy.as_ref().map(|p| self.prop(p, id, &w("anisotropy"), start, step, shift)),
            anisotropy_rotation: m.anisotropy_rotation.as_ref().map(|p| self.prop(p, id, &w("anisotropy_rotation"), start, step, shift)),
            sheen: m.sheen.as_ref().map(|p| self.prop(p, id, &w("sheen"), start, step, shift)),
            sheen_color: m.sheen_color.as_ref().map(|p| self.prop(p, id, &w("sheen_color"), start, step, shift)),
            sheen_roughness: m.sheen_roughness.as_ref().map(|p| self.prop(p, id, &w("sheen_roughness"), start, step, shift)),
            subsurface: m.subsurface.as_ref().map(|p| self.prop(p, id, &w("subsurface"), start, step, shift)),
            subsurface_radius: m.subsurface_radius,
            subsurface_scale: m.subsurface_scale.as_ref().map(|p| self.prop(p, id, &w("subsurface_scale"), start, step, shift)),
            subsurface_anisotropy: m.subsurface_anisotropy.as_ref().map(|p| self.prop(p, id, &w("subsurface_anisotropy"), start, step, shift)),
            clearcoat_color: m.clearcoat_color.as_ref().map(|p| self.prop(p, id, &w("clearcoat_color"), start, step, shift)),
            dispersion: m.dispersion.as_ref().map(|p| self.prop(p, id, &w("dispersion"), start, step, shift)),
            noise: m.noise.as_ref().map(|n| CNoise {
                drift: n.drift.map(|d| d.0),
                kind: n.kind,
                scale: self.prop(&n.scale, id, &w("noise.scale"), start, step, shift),
                octaves: n.octaves,
                seed: n.seed,
                world: n.space == crate::document::NoiseSpace::World,
                color: n.color.as_ref().map(|p| self.prop(p, id, &w("noise.color"), start, step, shift)),
                amount: self.prop(&n.amount, id, &w("noise.amount"), start, step, shift),
                roughness: n.roughness.as_ref().map(|p| self.prop(p, id, &w("noise.roughness"), start, step, shift)),
                bump: n.bump.as_ref().map(|p| self.prop(p, id, &w("noise.bump"), start, step, shift)),
            }),
            map: m.map.as_deref().map(Arc::from),
            normal_map: m.normal_map.as_deref().map(Arc::from),
            bump_map: m.bump_map.as_deref().map(Arc::from),
            roughness_map: m.roughness_map.as_deref().map(Arc::from),
            metallic_map: m.metallic_map.as_deref().map(Arc::from),
            emissive_map: m.emissive_map.as_deref().map(Arc::from),
            bump: m.bump.as_ref().map(|p| self.prop(p, id, &w("bump"), start, step, shift)),
            uv: m.uv.as_ref().map(|u| (self.prop(&u.scale, id, &w("uv.scale"), start, step, shift), self.prop(&u.offset, id, &w("uv.offset"), start, step, shift), self.prop(&u.rotation, id, &w("uv.rotation"), start, step, shift))),
            volume: m.volume.as_ref().map(|v| CVolume {
                density: self.prop(&v.density, id, &w("volume.density"), start, step, shift),
                color: v.color.as_ref().map(|p| self.prop(p, id, &w("volume.color"), start, step, shift)),
                anisotropy: self.prop(&v.anisotropy, id, &w("volume.anisotropy"), start, step, shift),
                emission: v.emission.as_ref().map(|p| self.prop(p, id, &w("volume.emission"), start, step, shift)),
                emission_strength: self.prop(&v.emission_strength, id, &w("volume.emission_strength"), start, step, shift),
                coverage: v.coverage.as_ref().map(|p| self.prop(p, id, &w("volume.coverage"), start, step, shift)),
                softness: v.softness.as_ref().map(|p| self.prop(p, id, &w("volume.softness"), start, step, shift)),
                grid: v.grid.as_ref().map(|g| (g.src.clone(), g.size.map(|n| n.clamp(1, 1024)), self.prop(&g.frame, id, &w("volume.grid.frame"), start, step, shift))),
                model: v.model.unwrap_or_default(),
                shape: v.shape.unwrap_or_default(),
                up: {
                    let u = v.up.map(|u| u.0).unwrap_or([0.0, -1.0, 0.0]);
                    let l = (u[0] * u[0] + u[1] * u[1] + u[2] * u[2]).sqrt();
                    if l > 1e-9 { u.map(|c| c / l) } else { [0.0, -1.0, 0.0] }
                },
            }),
        }
    }

    pub(super) fn mesh(&mut self, m: &MeshLayer, id: &str, start: f64, step: Option<Step>, shift: f64) -> CMesh {
        let p = |prop: &Property<f64>, what: &str, this: &mut Self| this.prop(prop, id, what, start, step, shift);
        let shape = match &m.mesh {
            MeshDef::Sphere { radius } => CMeshShape::Sphere(p(radius, "mesh.radius", self)),
            MeshDef::Plane { size } => CMeshShape::Plane(self.prop(size, id, "mesh.size", start, step, shift)),
            MeshDef::Box { size, depth, bevel } => {
                CMeshShape::Box(self.prop(size, id, "mesh.size", start, step, shift), p(depth, "mesh.depth", self), p(bevel, "mesh.bevel", self))
            }
            MeshDef::Cylinder { radius, top, height, bevel } => CMeshShape::Cylinder(
                p(radius, "mesh.radius", self),
                top.as_ref().map(|t| p(t, "mesh.top", self)),
                p(height, "mesh.height", self),
                p(bevel, "mesh.bevel", self),
            ),
            MeshDef::Capsule { radius, height } => CMeshShape::Capsule(p(radius, "mesh.radius", self), p(height, "mesh.height", self)),
            MeshDef::Torus { radius, tube, angles, caps: c } => {
                CMeshShape::Torus(p(radius, "mesh.radius", self), p(tube, "mesh.tube", self), self.prop(angles, id, "mesh.angles", start, step, shift), caps(*c))
            }
            MeshDef::Extrude { d, depth, bevel } => CMeshShape::Extrude(self.path_keys(d, id, "mesh.d", start, step, shift), p(depth, "mesh.depth", self), p(bevel, "mesh.bevel", self)),
            MeshDef::Revolve { profile, angles, axis } => CMeshShape::Revolve(
                self.path_keys(profile, id, "mesh.profile", start, step, shift),
                self.prop(angles, id, "mesh.angles", start, step, shift),
                *axis == RevolveAxis::Y,
            ),
            MeshDef::Sweep { d, radius, profile, width, trim, caps: c, head } => CMeshShape::Sweep {
                path: self.path_keys(d, id, "mesh.d", start, step, shift),
                radius: p(radius, "mesh.radius", self),
                width: p(width, "mesh.width", self),
                profile: match profile {
                    SweepShape::Circle => SweepProfile::Circle,
                    SweepShape::Square => SweepProfile::Square,
                    SweepShape::Ribbon => SweepProfile::Ribbon,
                },
                trim: self.prop(trim, id, "mesh.trim", start, step, shift),
                caps: caps(*c),
                head: p(head, "mesh.head", self),
            },
            MeshDef::Gltf { src, mesh, primitive, unit, frame, bounds } => CMeshShape::Gltf {
                src: src.as_str().into(),
                mesh: *mesh,
                primitive: *primitive,
                unit: if unit.is_finite() && *unit > 0.0 { *unit } else { 100.0 },
                frame: p(frame, "mesh.frame", self),
                bounds: *bounds,
            },
        };
        let parts = m.parts.list();
        let part_colors = (0..PARTS)
            .map(|i| {
                let merged = merge(&m.material, parts[i]);
                merged.colors.iter().enumerate().map(|(k, c)| self.prop(c, id, &format!("{}.colors.{k}", if parts[i].is_some() { format!("parts.{}", crate::mesh::PART_NAMES[i]) } else { "material".into() }), start, step, shift)).collect()
            })
            .collect();
        let materials = (0..PARTS)
            .map(|i| {
                let merged = merge(&m.material, parts[i]);
                let what = match parts[i] {
                    Some(_) => format!("parts.{}", crate::mesh::PART_NAMES[i]),
                    None => "material".into(),
                };
                self.material(&merged, id, &what, start, step, shift)
            })
            .collect();
        let own_color = std::array::from_fn(|i| parts[i].is_some_and(|m| m.color.is_some()));
        let colors = m.colors.iter().enumerate().map(|(i, c)| self.prop(c, id, &format!("colors.{i}"), start, step, shift)).collect();
        if m.segments == 0 {
            self.errors.push(format!("{id}.segments: must be at least 1"));
        }
        let displace = m.displace.as_ref().map(|d| {
            (d.kind, self.prop(&d.scale, id, "displace.scale", start, step, shift), d.octaves, d.seed, self.prop(&d.amount, id, "displace.amount", start, step, shift), d.drift.map(|v| v.0).unwrap_or([0.0; 3]), d.src.clone())
        });
        CMesh {
            displace,
            shape,
            materials,
            own_color,
            colors,
            part_colors,
            spans: m.spans.iter().map(|s| s.max(0.0)).collect(),
            segments: m.segments.clamp(1, 360),
            gap: self.prop(&m.gap, id, "gap", start, step, shift),
            detail: m.detail,
            last: std::sync::Mutex::new(None),
        }
    }

    pub(super) fn lighting(&mut self, scene: &Scene, pre_roll: f64, start: f64) -> CLighting {
        let id = scene.id.as_str();
        // Only the first directional or point light casts shadows unless
        // others ask (each shadow costs a depth map per frame).
        let key = scene.lights.iter().position(|l| l.kind != LightKind::Ambient);
        let lights = if scene.lights.is_empty() {
            default_rig()
        } else {
            scene
                .lights
                .iter()
                .enumerate()
                .map(|(i, l)| {
                    let (what, fallback) = match l.kind {
                        LightKind::Point | LightKind::Area => (("position", &l.position), [0.0, 0.0, -500.0]),
                        _ => (("from", &l.from), [-1.0, -1.0, -1.0]),
                    };
                    let vector = match what.1 {
                        Some(p) => self.prop(p, id, &format!("lights.{i}.{}", what.0), start, None, pre_roll),
                        None => Prop::constant(crate::value::Vec3(fallback)),
                    };
                    if matches!(l.kind, LightKind::Point | LightKind::Area) && l.position.is_none() {
                        let kind = if l.kind == LightKind::Area { "an area" } else { "a point" };
                        self.errors.push(format!("{id}.lights.{i}: {kind} light needs \"position\""));
                    }
                    let settings = &self.windows.project.settings;
                    let centre = crate::value::Vec3([settings.width as f64 / 2.0, settings.height as f64 / 2.0, 0.0]);
                    CLight {
                        kind: l.kind,
                        vector,
                        color: self.prop(&l.color, id, &format!("lights.{i}.color"), start, None, pre_roll),
                        intensity: self.prop(&l.intensity, id, &format!("lights.{i}.intensity"), start, None, pre_roll),
                        shadow: l.shadow.unwrap_or(Some(i) == key) && l.kind != LightKind::Ambient,
                        softness: self.prop(&l.softness, id, &format!("lights.{i}.softness"), start, None, pre_roll),
                        range: l.range.as_ref().map(|r| self.prop(r, id, &format!("lights.{i}.range"), start, None, pre_roll)),
                        aim: match &l.aim {
                            Some(a) => self.prop(a, id, &format!("lights.{i}.aim"), start, None, pre_roll),
                            None => Prop::constant(centre),
                        },
                        size: match &l.size {
                            Some(a) => self.prop(a, id, &format!("lights.{i}.size"), start, None, pre_roll),
                            None => Prop::constant(crate::value::Vec2([300.0, 300.0])),
                        },
                        disc: l.shape == Some(crate::document::AreaShape::Disc),
                    }
                })
                .collect()
        };
        let env = scene.environment.clone().unwrap_or_default();
        let look = scene.look.clone().or_else(|| self.windows.project.settings.look.clone());
        CLighting {
            lights,
            preset: env.preset,
            intensity: self.prop(&env.intensity, id, "environment.intensity", start, None, pre_roll),
            rotation: self.prop(&env.rotation, id, "environment.rotation", start, None, pre_roll),
            sky: env.sky.as_ref().map(|p| self.prop(p, id, "environment.sky", start, None, pre_roll)),
            horizon: env.horizon.as_ref().map(|p| self.prop(p, id, "environment.horizon", start, None, pre_roll)),
            ground: env.ground.as_ref().map(|p| self.prop(p, id, "environment.ground", start, None, pre_roll)),
            image: env.image.as_deref().map(std::sync::Arc::from),
            tone: match look.as_ref().map(|l| l.tone).unwrap_or_default() {
                crate::document::ToneMap::Standard => 0,
                crate::document::ToneMap::Filmic => 1,
                crate::document::ToneMap::Agx => 2,
            },
            exposure: match look.as_ref() {
                Some(l) => self.prop(&l.exposure, id, "look.exposure", start, None, pre_roll),
                None => Prop::constant(0.0),
            },
        }
    }
}

fn default_rig() -> Vec<CLight> {
    let light = |vector: [f64; 3], intensity: f64, shadow: bool| CLight {
        kind: LightKind::Directional,
        vector: Prop::constant(crate::value::Vec3(unit(vector))),
        color: Prop::constant(Color::WHITE),
        intensity: Prop::constant(intensity),
        shadow,
        softness: Prop::constant(0.35),
        range: None,
        aim: Prop::constant(crate::value::Vec3([0.0; 3])),
        size: Prop::constant(crate::value::Vec2([0.0; 2])),
        disc: false,
    };
    // The key casts shadows; the rim only outlines.
    vec![light([-0.55, -0.8, -0.7], 1.0, true), light([0.75, -0.35, 0.8], 0.7, false)]
}

pub(super) struct CLight {
    kind: LightKind,
    /// Directional: toward the light (any length); point: position.
    vector: Prop<crate::value::Vec3>,
    color: Prop<Color>,
    intensity: Prop<f64>,
    shadow: bool,
    softness: Prop<f64>,
    range: Option<Prop<f64>>,
    aim: Prop<crate::value::Vec3>,
    size: Prop<crate::value::Vec2>,
    disc: bool,
}

pub(super) struct CLighting {
    lights: Vec<CLight>,
    preset: EnvironmentPreset,
    intensity: Prop<f64>,
    rotation: Prop<f64>,
    sky: Option<Prop<Color>>,
    horizon: Option<Prop<Color>>,
    ground: Option<Prop<Color>>,
    image: Option<std::sync::Arc<str>>,
    tone: u32,
    exposure: Prop<f64>,
}

impl CLighting {
    pub(super) fn at(&self, tc: f64) -> Lighting {
        let lights = self
            .lights
            .iter()
            .map(|l| {
                let c = clamp_color(l.color.at(tc));
                let v = l.vector.at(tc).0;
                let vector = if matches!(l.kind, LightKind::Point | LightKind::Area) { v } else { unit(v) };
                let (mut u, mut w) = ([0.0; 3], [0.0; 3]);
                if l.kind == LightKind::Area {
                    // Half extents across the panel; u × w faces the aim.
                    let a = l.aim.at(tc).0;
                    let n = unit([a[0] - v[0], a[1] - v[1], a[2] - v[2]]);
                    let up = if n[1].abs() > 0.99 { [0.0, 0.0, 1.0] } else { [0.0, -1.0, 0.0] };
                    let ud = unit(cross(n, up));
                    let wd = cross(n, ud);
                    let size = l.size.at(tc).0;
                    let (hw, hh) = (size[0].max(1.0) / 2.0, size[1].max(1.0) / 2.0);
                    u = [ud[0] * hw, ud[1] * hw, ud[2] * hw];
                    w = [wd[0] * hh, wd[1] * hh, wd[2] * hh];
                }
                LightState {
                    kind: l.kind,
                    vector,
                    color: linear(c, l.intensity.at(tc).max(0.0) * c.0[3]),
                    shadow: l.shadow,
                    softness: l.softness.at(tc).clamp(0.0, 1.0),
                    range: l.range.as_ref().map(|r| r.at(tc).max(1.0)).unwrap_or(0.0),
                    u,
                    v: w,
                    disc: l.disc,
                }
            })
            .collect();
        let k = self.intensity.at(tc).max(0.0);
        let rot = self.rotation.at(tc).to_radians();
        let g = |v: f64| Color([v, v, v, 1.0]);
        // (sky, horizon, ground, lobes: direction, angular radius, colour × strength).
        // Lobe sizes are set so their exact fill light (as traced) matches
        // the look the presets were designed with.
        type L = ([f64; 3], f64, Color, f64);
        let (sky, horizon, ground, lobes): (Color, Color, Color, Vec<L>) = match self.preset {
            EnvironmentPreset::Studio => (
                g(0.42),
                g(0.30),
                g(0.06),
                vec![([0.0, -1.0, -0.25], 0.262, g(1.0), 2.4), ([-1.0, -0.25, -0.35], 0.141, g(1.0), 1.8), ([1.0, -0.15, 0.25], 0.115, g(1.0), 1.5)],
            ),
            EnvironmentPreset::Soft => (g(0.85), g(0.75), g(0.5), vec![([0.0, -1.0, -0.4], 0.419, g(1.0), 1.4)]),
            EnvironmentPreset::Sunset => (
                Color([0.30, 0.38, 0.62, 1.0]),
                Color([1.0, 0.58, 0.32, 1.0]),
                Color([0.14, 0.09, 0.07, 1.0]),
                vec![([-0.85, -0.12, 0.5], 0.064, Color([1.0, 0.72, 0.45, 1.0]), 6.0)],
            ),
            EnvironmentPreset::Night => (
                g(0.03),
                g(0.05),
                g(0.0),
                vec![([-1.0, -0.2, -0.1], 0.052, g(1.0), 4.0), ([1.0, -0.35, 0.45], 0.045, g(1.0), 3.5), ([0.0, -1.0, 0.1], 0.128, g(1.0), 1.4)],
            ),
            EnvironmentPreset::None => (g(0.0), g(0.0), g(0.0), Vec::new()),
        };
        let turn = |d: [f64; 3]| {
            let (s, c) = rot.sin_cos();
            unit([d[0] * c - d[2] * s, d[1], d[0] * s + d[2] * c])
        };
        let env = if self.image.is_some() {
            // The image replaces the preset's colours and lobes.
            EnvState { sky: [0.0; 3], horizon: [0.0; 3], ground: [0.0; 3], lobes: Vec::new(), image: self.image.clone(), image_intensity: k, image_rotation: rot }
        } else {
            EnvState {
                sky: linear(self.sky.as_ref().map(|p| clamp_color(p.at(tc))).unwrap_or(sky), k),
                horizon: linear(self.horizon.as_ref().map(|p| clamp_color(p.at(tc))).unwrap_or(horizon), k),
                ground: linear(self.ground.as_ref().map(|p| clamp_color(p.at(tc))).unwrap_or(ground), k),
                lobes: lobes.into_iter().map(|(d, size, c, s)| Lobe { dir: turn(d), size, color: linear(c, s * k) }).collect(),
                image: None,
                image_intensity: 0.0,
                image_rotation: 0.0,
            }
        };
        Lighting { lights, env, tone: self.tone, exposure: self.exposure.at(tc).clamp(-16.0, 16.0).exp2() }
    }
}

/// Screen pixels per layer unit at the mesh's origin (largest axis), in
/// half-octave steps.
fn screen_scale(world: Mat4, camera: &Camera3) -> f64 {
    let mv = camera.view.then(world);
    let m = mv.0;
    let z = m[11];
    let axis = |c: usize| (m[c] * m[c] + m[4 + c] * m[4 + c] + m[8 + c] * m[8 + c]).sqrt();
    let k = camera.focal / z.max(1.0) * axis(0).max(axis(1)).max(axis(2));
    if !(k.is_finite() && k > 0.0) {
        return 1.0;
    }
    (k.log2() * 2.0).round().exp2().sqrt().clamp(1.0 / 16.0, 32.0)
}

impl CMesh {
    pub(super) fn params_at(&self, t: f64, screen: f64) -> Arc<MeshParams> {
        let path = |(paths, index): &(Vec<Arc<str>>, Prop<f64>)| path_morph(paths, index.at(t));
        let shape = match &self.shape {
            CMeshShape::Sphere(r) => MeshShape::Sphere { radius: r.at(t) },
            CMeshShape::Plane(size) => {
                let s = size.at(t);
                MeshShape::Plane { size: [s.x(), s.y()] }
            }
            CMeshShape::Box(size, depth, bevel) => {
                let s = size.at(t);
                MeshShape::Box { size: [s.x(), s.y(), depth.at(t)], bevel: bevel.at(t) }
            }
            CMeshShape::Cylinder(r, top, h, b) => {
                let r = r.at(t);
                MeshShape::Cylinder { radius: r, top: top.as_ref().map(|x| x.at(t)).unwrap_or(r), height: h.at(t), bevel: b.at(t) }
            }
            CMeshShape::Capsule(r, h) => MeshShape::Capsule { radius: r.at(t), height: h.at(t) },
            CMeshShape::Torus(r, tube, angles, c) => {
                let a = angles.at(t);
                MeshShape::Torus { radius: r.at(t), tube: tube.at(t), angles: [a.x(), a.y()], caps: *c }
            }
            CMeshShape::Extrude(p, depth, bevel) => MeshShape::Extrude { path: path(p), depth: depth.at(t), bevel: bevel.at(t) },
            CMeshShape::Revolve(p, angles, axis_y) => {
                let a = angles.at(t);
                MeshShape::Revolve { profile: path(p), angles: [a.x(), a.y()], axis_y: *axis_y }
            }
            CMeshShape::Sweep { path: p, radius, width, profile, trim, caps, head } => {
                let tr = trim.at(t);
                MeshShape::Sweep { path: path(p), radius: radius.at(t), width: width.at(t), profile: *profile, trim: [tr.x(), tr.y()], caps: *caps, head: head.at(t) }
            }
            CMeshShape::Gltf { src, mesh, primitive, unit, frame, bounds } => {
                MeshShape::Gltf { src: frame_path(src, frame.at(t)), mesh: *mesh, primitive: *primitive, unit: *unit, bounds: *bounds }
            }
        };
        let displace = self.displace.as_ref().map(|(kind, scale, octaves, seed, amount, drift, src)| {
            let how = match src {
                Some(src) => crate::mesh::Displacement::Image(src.clone()),
                None => crate::mesh::Displacement::Noise(crate::noise::Field { kind: *kind, scale: scale.at(t).max(0.5), octaves: (*octaves).clamp(1, 8), seed: *seed, offset: [drift[0] * t, drift[1] * t, drift[2] * t] }),
            };
            (how, amount.at(t))
        });
        let params = MeshParams { shape, segments: self.segments, gap: self.gap.at(t), detail: self.detail, spans: self.spans.clone(), screen, displace };
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        match &*last {
            Some(p) if **p == params => p.clone(),
            _ => {
                let p = Arc::new(params);
                *last = Some(p.clone());
                p
            }
        }
    }

    pub(super) fn state(&self, t: f64, world: Mat4, camera: Camera3, lens: Option<[f64; 2]>) -> MeshState {
        MeshState {
            lens,
            params: self.params_at(t, screen_scale(world, &camera)),
            world,
            camera,
            materials: std::array::from_fn(|i| self.materials[i].at(t)),
            colors: self.colors.iter().map(|c| clamp_color(c.at(t))).collect(),
            own_color: self.own_color,
            part_colors: std::array::from_fn(|i| self.part_colors[i].iter().map(|c| clamp_color(c.at(t))).collect()),
            lighting: None,
        }
    }
}

impl MeshState {
    /// Canvas bounds of the mesh's local box (None when it crosses the camera).
    pub fn screen_bounds(&self) -> Option<[f64; 4]> {
        let (lo, hi) = self.params.local_bounds()?;
        let m = self.camera.project.then(self.world);
        let mut b = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
        for k in 0..8 {
            let c = [if k & 1 == 0 { lo[0] } else { hi[0] }, if k & 2 == 0 { lo[1] } else { hi[1] }, if k & 4 == 0 { lo[2] } else { hi[2] }];
            let [x, y, _, w] = m.apply(c[0], c[1], c[2]);
            if w <= 1e-6 {
                return None;
            }
            b = [b[0].min(x / w), b[1].min(y / w), b[2].max(x / w), b[3].max(y / w)];
        }
        Some(b)
    }
}

/// Gives every mesh in `layers` (groups and matte sources included) the
/// scene's lighting.
pub(super) fn light_meshes(layers: &mut [LayerState], lighting: &Arc<Lighting>) {
    for l in layers {
        match &mut l.content {
            Content::Mesh(m) if m.lighting.is_none() => m.lighting = Some(lighting.clone()),
            Content::Group(children) => light_meshes(children, lighting),
            _ => {}
        }
        if let Some(src) = l.matte.as_mut().and_then(|m| m.source.as_mut()) {
            light_meshes(std::slice::from_mut(src), lighting);
        }
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::time::Fps;

    pub(crate) fn compiled(scene: serde_json::Value) -> Compiled {
        let mut p = Project::new(640, 360, Fps::whole(30), 2.0);
        let mut s = scene;
        s["id"] = serde_json::json!("s1");
        p.scenes.push(serde_json::from_value(s).unwrap());
        let c = Compiled::new(&p);
        assert!(c.errors.is_empty(), "{:?}", c.errors);
        c
    }

    fn mesh(c: &Compiled, t: f64, id: &str) -> MeshState {
        c.frame(t).layers.iter().find_map(|l| match &l.content {
            Content::Mesh(m) if &*l.id == id => Some((**m).clone()),
            _ => None,
        }).unwrap()
    }

    #[test]
    fn lights_move() {
        let c = compiled(serde_json::json!({"lights": [
            {"type": "point", "position": {"keys": [{"at": 0, "v": [0, 0, -100]}, {"at": 1, "v": [600, 0, -100]}]}},
            {"from": {"keys": [{"at": 0, "v": [-1, 0, 0]}, {"at": 1, "v": [1, 0, 0]}]}}],
            "layers": [{"id": "m", "kind": "mesh", "mesh": {"type": "sphere"}}]}));
        let (a, b) = (mesh(&c, 0.0, "m"), mesh(&c, 1.0, "m"));
        let (la, lb) = (a.lighting.unwrap(), b.lighting.unwrap());
        assert_eq!(la.lights[0].vector, [0.0, 0.0, -100.0]);
        assert_eq!(lb.lights[0].vector, [600.0, 0.0, -100.0]);
        assert!((la.lights[1].vector[0] + 1.0).abs() < 1e-9 && (lb.lights[1].vector[0] - 1.0).abs() < 1e-9);
    }

    #[test]
    fn depth_scales_with_animated_scale_and_parents() {
        let c = compiled(serde_json::json!({"layers": [
            {"id": "null", "kind": "shape", "shape": "circle", "size": 1, "transform": {"scale": {"keys": [{"at": 0, "v": 1}, {"at": 1, "v": 2}]}}},
            {"id": "m", "kind": "mesh", "mesh": {"type": "sphere"}, "parent": "null",
             "transform": {"scale": {"keys": [{"at": 0, "v": 0.5}, {"at": 1, "v": 1}]}, "scale_z": 0.5}}]}));
        let z = |t: f64| {
            let w = mesh(&c, t, "m").world.0;
            (w[2] * w[2] + w[6] * w[6] + w[10] * w[10]).sqrt()
        };
        // Depth scale = parent 1 × own 0.5 × scale_z 0.5, then 2 × 1 × 0.5.
        assert!((z(0.0) - 0.25).abs() < 1e-9 && (z(1.0) - 1.0).abs() < 1e-9, "{} {}", z(0.0), z(1.0));
    }
}

#[cfg(test)]
mod more_tests {
    use super::tests::compiled;

    #[test]
    fn check_says_when_a_bevel_is_too_big() {
        let c = compiled(serde_json::json!({"layers": [
            {"id": "b", "kind": "mesh", "mesh": {"type": "box", "size": [100, 100], "depth": 40, "bevel": 35}},
            {"id": "ok", "kind": "mesh", "mesh": {"type": "box", "size": [100, 100], "depth": 40, "bevel": 10}},
            {"id": "x", "kind": "mesh", "mesh": {"type": "extrude", "d": {"shape": "rect", "size": [200, 20]}, "depth": 80, "bevel": 15}}]}));
        let mut p = crate::document::Project::new(640, 360, crate::time::Fps::whole(30), 2.0);
        p.scenes.push(serde_json::from_value(serde_json::json!({"id": "s1", "layers": []})).unwrap());
        let report = crate::check::check(&p, &c, &Default::default());
        let mesh: Vec<_> = report.issues.iter().filter(|i| i.kind == "mesh").collect();
        assert_eq!(mesh.len(), 2, "{mesh:?}");
        assert!(mesh.iter().any(|i| i.element.as_deref() == Some("b") && i.message.contains("bevel 35.0 is too big and is drawn as 20.0")));
        assert!(mesh.iter().any(|i| i.element.as_deref() == Some("x") && i.message.contains("narrowest")));
    }
}

#[cfg(test)]
mod tessellation_tests {
    use super::tests::compiled;
    use crate::compile::Content;

    #[test]
    fn tessellation_follows_the_drawn_size() {
        // The same small revolve drawn at 1× and scaled 16×.
        let c = compiled(serde_json::json!({"layers": [
            {"id": "small", "kind": "mesh", "mesh": {"type": "revolve", "profile": "M 0 -10 C 12 -8 12 8 0 10", "axis": "z"}},
            {"id": "big", "kind": "mesh", "mesh": {"type": "revolve", "profile": "M 0 -10 C 12 -8 12 8 0 10", "axis": "z"}, "transform": {"scale": 16}}]}));
        let tris = |id: &str| {
            let f = c.frame(0.0);
            let m = f.layers.iter().find_map(|l| match &l.content { Content::Mesh(m) if &*l.id == id => Some(m.clone()), _ => None }).unwrap();
            (m.params.screen, crate::mesh::generate(&m.params).unwrap().tris.len())
        };
        let ((s1, n1), (s16, n16)) = (tris("small"), tris("big"));
        assert!((s1 - 1.0).abs() < 1e-9 && (s16 - 16.0).abs() < 1e-9, "{s1} {s16}");
        // Drawn 16× bigger, the circle around the axis has ~16× the steps
        // (one every ~9 px on screen), not the same 24.
        assert!(n16 > n1 * 8, "{n1} → {n16}");
    }
}

/// A glTF sequence's file for `frame`: `{frame}` becomes the frame number,
/// `{frame:04}` the number zero-padded to 4 digits; other paths unchanged.
pub fn frame_path(src: &str, frame: f64) -> String {
    let Some(open) = src.find("{frame") else { return src.to_string() };
    let Some(close) = src[open..].find('}').map(|k| open + k) else { return src.to_string() };
    let n = frame.floor().max(0.0) as u64;
    let width = src[open + 6..close].trim_start_matches(':').parse::<usize>().unwrap_or(0);
    format!("{}{:0width$}{}", &src[..open], n, &src[close + 1..], width = width)
}

#[cfg(test)]
mod gltf_tests {
    use super::frame_path;

    #[test]
    fn sequence_paths_take_the_frame() {
        assert_eq!(frame_path("assets/splash/s_{frame:04}.glb", 12.7), "assets/splash/s_0012.glb");
        assert_eq!(frame_path("a/{frame}.glb", 3.0), "a/3.glb");
        assert_eq!(frame_path("a/model.glb", 3.0), "a/model.glb");
    }
}
