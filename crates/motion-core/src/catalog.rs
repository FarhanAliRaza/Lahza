//! The catalog (MCP resource `lahza://catalog`): every preset, layer kind,
//! effect, additive track and style pack with parameters and defaults.
//! Generated from the same definitions the engine uses.

use crate::curve::PRESETS;
use serde_json::{json, Value};

pub const SCHEMA: &str = include_str!("../docs/schema.md");
pub const GUIDE: &str = include_str!("../docs/guide.md");

pub struct StylePack {
    pub name: &'static str,
    pub description: &'static str,
    /// Longest allowed still picture, seconds.
    pub max_hold: f64,
    pub background: &'static str,
    pub palette: &'static [&'static str],
    pub step_fps: Option<f64>,
    pub defaults: &'static str,
    pub rules: &'static [&'static str],
}

pub const STYLE_PACKS: &[StylePack] = &[
    StylePack {
        name: "explainer.evidence",
        description: "Vox-like evidence explainer: documents and screens marked up by hand",
        max_hold: 3.0,
        background: "#f4f1ea",
        palette: &["#f4f1ea", "#111111", "#ffd23f"],
        step_fps: Some(12.0),
        defaults: "elements stepped at 12 fps, camera smooth; entrances 'enter'; slow camera 'push'",
        rules: &["no still held over 3 s", "one highlight colour", "narration describes what is on screen"],
    },
    StylePack {
        name: "explainer.map",
        description: "Harris-like map explainer: muted paper, one accent",
        max_hold: 3.0,
        background: "#e9e2d0",
        palette: &["#e9e2d0", "#2b2b2b", "#c8102e"],
        step_fps: None,
        defaults: "slow pushes ('push'), no snaps; labels appear as places are named (anchor to word markers)",
        rules: &["no still held over 3 s", "labels on word markers"],
    },
    StylePack {
        name: "ident.flat",
        description: "IMEX-like flat ident: flat palette, one scene per bar",
        max_hold: 2.0,
        background: "#f7f3e8",
        palette: &["#f7f3e8", "#ff5a36", "#2d5bff", "#ffc800", "#121212"],
        step_fps: None,
        defaults: "'land' springs on landings, 'glide' moves; anchor peaks to beats",
        rules: &["one scene per bar", "springs only on landings and scale"],
    },
    StylePack {
        name: "product.ui",
        description: "Showreel UI style: exp-lag snaps, glass cards, tiny staggers",
        max_hold: 2.0,
        background: "#0d0d12",
        palette: &["#0d0d12", "#ffffff", "#7b61ff", "#00d4a6"],
        step_fps: None,
        defaults: "'snap' and 'glide', 1–3 f staggers, residual drift on everything",
        rules: &["nothing stops dead", "click is the transition"],
    },
    StylePack {
        name: "collage",
        description: "Paper collage: tinted paper, stepped 8/12 fps",
        max_hold: 3.0,
        background: "#efe6d8",
        palette: &["#efe6d8", "#1d1d1b", "#e94f37", "#3f88c5"],
        step_fps: Some(8.0),
        defaults: "fragments at ~82% opacity, layers stepped at 8 fps, type at 12 fps",
        rules: &["never pure white paper", "held frames pixel-identical"],
    },
    StylePack {
        name: "promo.kinetic",
        description: "Motion Array-like kinetic promo: strict beat grid",
        max_hold: 1.5,
        background: "#0b0b0b",
        palette: &["#0b0b0b", "#ffffff", "#d7ff3a", "#ff3d7f"],
        step_fps: None,
        defaults: "per-glyph 'enter' with 1 f stagger, 'pop' on scale, cuts lead −1 f, drift after settling",
        rules: &["events on the beat grid within ±1 f", "cuts lead the beat by 1 f"],
    },
];

pub fn style_pack(name: &str) -> Option<&'static StylePack> {
    STYLE_PACKS.iter().find(|p| p.name == name)
}

pub fn styles_json() -> Value {
    Value::Array(
        STYLE_PACKS
            .iter()
            .map(|p| {
                json!({
                    "name": p.name, "description": p.description, "background": p.background,
                    "palette": p.palette, "step_fps": p.step_fps, "max_hold_seconds": p.max_hold,
                    "defaults": p.defaults, "rules": p.rules,
                })
            })
            .collect(),
    )
}

pub fn catalog_json() -> Value {
    let components: Vec<Value> = crate::components::KINDS
        .iter()
        .map(|k| {
            let params: Vec<Value> = k.params.iter().map(|(n, what, d)| json!({"name": n, "what": what, "default": d})).collect();
            json!({"component": k.name, "summary": k.summary, "params": params})
        })
        .collect();
    let presets: Vec<Value> = PRESETS
        .iter()
        .map(|p| {
            json!({
                "name": p.name, "model": p.model, "measured": p.measured, "use_for": p.use_for,
                "default_dur_seconds": p.duration,
                "raw": serde_json::to_value(p.preset.raw()).unwrap_or_default(),
            })
        })
        .collect();
    let material = json!({"preset": "matte | plastic | glossy (default) | coated (lacquer: crisp reflections over colour) | metal | chrome | brushed (spun metal) | velvet (fabric sheen) | glass (refracts what is behind) | emissive | flat (unlit flat colour) | shadow (invisible except shadows: drops shadows onto 2D layers) | wireframe (edges only: 3D viewport look)",
                    "specular": "0..1 dielectric reflection (0.5 default = 4% head-on; 0 none)", "specular_tint": "colour: tints a dielectric's reflection and a metal's edges (Blender F82 tint)",
                    "anisotropy": "0..1 brushed/spun metal: grain around the mesh's axis, highlights stretch across it", "anisotropy_rotation": "degrees the grain turns",
                    "sheen": "0..1 soft grazing light (velvet, fabric)", "sheen_color": "colour (default white)", "sheen_roughness": "0..1 (default 0.5)",
                    "clearcoat_color": "colour the coat tints what is under it (candy paint)", "dispersion": "glass, path tracing: 1/Abbe (0.02 glass, 0.05 flint, more: stylised rainbows)",
                    "ior": "glass: index of refraction (default 1.45)", "thickness": "glass: px light travels inside (default 60)",
                    "colors": "[colour, …] per piece, in turn, for this part (over the layer's colors)", "wireframe": "px: draw edges instead of the surface (preset wireframe)", "fill": "wireframe face colour (hidden-line; omit for see-through wires)",
                    "color": "base colour (animatable)", "metallic": "0..1", "roughness": "0 mirror … 1 diffuse", "clearcoat": "0..1 lacquer layer", "clearcoat_roughness": "0..1",
                    "emissive": "colour it glows with", "emissive_strength": "multiplier", "opacity": "0..1", "transmission": "0..1 glass (background shows through, edges reflect)", "unlit": "bool", "faceted": "bool: low-poly normals"});
    let mut catalog = json!({
        "presets": presets,
        "raw_curves": [
            {"type": "linear"}, {"type": "hold", "note": "jump at the end of the move"},
            {"type": "bezier", "p": "[x1, y1, x2, y2] like CSS cubic-bezier"},
            {"type": "power", "n": "exponent", "mode": "in | out | in-out"},
            {"type": "exp", "r": "kept fraction of the remaining distance per project frame (0..1)", "tau": "or a time constant like \"4f\""},
            {"type": "spring", "zeta": "damping ratio", "hz": "natural frequency"},
            {"type": "bounce", "bounces": 3, "ratio": 0.43, "exponent": 2.75},
            {"type": "slam", "n": 2}
        ],
        "anchors": {
            "start": "the move starts on the reference",
            "peak": "its maximum speed is on the reference (sync the peak, not the start)",
            "land": "it first reaches 98% on the reference (contact for springs and bounces)",
            "end": "it arrives on the reference (default)"
        },
        "time_references": [
            "2.5 (seconds)", "\"48f\"", "\"beat:17\"", "\"bar:5\"", "\"triplet:12\"", "\"half:3\"", "\"sixteenth:9\"",
            "\"hit:3\"", "\"stop:2\"", "\"drop:1\"", "\"onset.low:4\"", "\"music.start:1\"",
            "\"cue:strike\"", "\"cue:strike.end\"", "\"line:hook\"", "\"word:borders\"", "\"word:borders@2\"", "\"word:'81'\"", "\"word#42\"", "\"sentence#3.end\"",
            "\"#title.end + 0.25\"", "\"bar:5 - 2f\"", "\"beat:9 + 0.5b\""
        ],
        "durations": "number = seconds; strings: \"8f\" frames, \"0.25s\", \"120ms\", \"1b\" beats, \"1bar\"; lead: bare numbers are frames",
        "key": {
            "at": "time reference", "v": "target value", "from": "start value (optional; makes a one-key entrance)",
            "ease": "preset name or raw curve (default linear)", "anchor": "start | peak | land | end (default end)",
            "lead": "frames (default 0)", "dur": "move duration (default: preset's natural duration when anchored, else from the previous key)",
            "space": "linear | log (zoom implies log)"
        },
        "additive": [
            {"type": "drift", "rate": "value per unit", "per": "second | frame", "mode": "add | mul (mul: 0.01 = +1% per unit, compounding)", "from": "time (default: first move's start)", "until": "time"},
            {"type": "wiggle", "hz": "noise frequency", "amp": "value", "seed": 0, "octaves": 1},
            {"type": "sine", "hz": "frequency", "amp": "value", "phase": "turns"}
        ],
        "layer_common": {
            "id": "stable id (letters, digits, _ -)", "name": "label",
            "transform": {"position": "[x, y] canvas px (default [0,0])", "anchor": "[x, y] pivot in layer space", "scale": "[sx, sy] or number", "rotation": "degrees clockwise", "opacity": "0..1"},
            "time": {"start": "time (default parent start)", "end": "time", "duration": "duration", "step_fps": "stepped clock, e.g. 12 (0 = off)", "step_phase": "seconds", "offset": "comps: children see t - offset"},
            "blend": "normal | add | multiply | screen | overlay | darken | lighten | color_dodge | color_burn | hard_light | soft_light | difference | exclusion | subtract | divide | hue | saturation | color | luminosity",
            "effects": [
                {"type": "blur", "radius": "sigma in px (animatable)"},
                {"type": "shadow", "color": "default #00000080", "offset": "[x, y] px (default [0, 12])", "radius": "blur sigma (default 12)"},
                {"type": "glow", "color": "tint (white keeps the layer's colours)", "radius": "sigma (default 12)", "intensity": "default 1"},
                {"type": "exposure", "stops": "+1 doubles"},
                {"type": "brightness_contrast", "brightness": "−1…1", "contrast": "−1…1"},
                {"type": "levels", "in_black": 0, "in_white": 1, "gamma": 1, "out_black": 0, "out_white": 1},
                {"type": "hue_saturation", "hue": "degrees", "saturation": "−1 (grey)…+1", "lightness": "−1…1"},
                {"type": "tint", "black": "colour", "white": "colour", "amount": 1},
                {"type": "gradient_map", "stops": "[[0, colour], [1, colour]]", "amount": 1},
                {"type": "invert", "amount": 1},
                {"type": "fill", "color": "colour", "amount": 1},
                {"type": "lut", "path": "project-relative .cube 3D LUT", "amount": 1},
                {"type": "displace", "amount": "px", "scale": "feature size px", "speed": "evolution per second", "seed": 0},
                {"type": "directional_blur", "angle": "degrees", "length": "px"},
                {"type": "rgb_split", "amount": "px", "angle": "degrees"},
                {"type": "vignette", "amount": 0.5, "softness": 0.5},
                {"type": "grain", "amount": 0.08, "size": "px", "seed": 0}
            ],
            "effects_order": "shadow (below), then the layer through displace → directional_blur → rgb_split → colour effects in list order → vignette → grain, then glow (above); blur applies to the layer's picture first",
            "effects_note": "effects apply to the layer as one picture; on a comp they apply to the flattened comp (like an AE precomp)",
            "matte": {"layer": "sibling layer id used as a track matte (not drawn itself)", "mode": "alpha | alpha_inverted | luma | luma_inverted"},
            "space": "world (default) | screen: screen ignores the scene camera's zoom/orbit/dolly and stays put on screen, drawn in layer order (titles, captions and finishing layers over a moving 3D camera)",
            "three_d": "bool: place the layer in 3D (uses transform.z, rotation_x, rotation_y and the scene camera's fov/orbit/dolly); 3D layers sort back to front among their neighbours; children of a 3D comp lie on its plane",
            "transform_3d": {"z": "depth, + is away from the viewer", "rotation_x": "degrees, + tilts the top away", "rotation_y": "degrees, + turns the right side away"},
            "parent": "transform parent: sibling layer id",
            "follow": {"layer": "sibling shape \"path\" layer to ride (replaces position, z and parent)", "at": "head (trim end, default) | tail (trim start) | fraction 0..1 of the full length (animatable)", "rotate": "bool: also turn along the path (added to rotation)", "offset": "[x, y] px from the point (along the path frame when rotate)"},
            "hidden": "bool"
        },
        "layer_kinds": {
            "shape": {
                "shape": "circle | rect | capsule | ring | arc | polygon | star | line | path | field",
                "d": "path only: SVG path data in layer space (M L H V C S Q T A Z, absolute or relative), or a primitive {\"shape\": \"circle|rect|capsule|polygon|star\", \"size\", \"radius\", \"sides\", \"inner\"}, or keys that MORPH: {\"keys\": [{\"at\", \"v\": path or primitive, \"ease\", \"anchor\", \"lead\", \"dur\"}]} (outlines are resampled and aligned, so any two shapes morph); fill uses non-zero winding; trim draws the stroke on",
                "size": "[w, h] (default [100, 100]); circle/ring/arc/polygon/star use size[0] as diameter",
                "radius": "corner rounding (rect, polygon, star)", "thickness": "ring, arc and line width (default 10)",
                "sides": "polygon sides / star points (default 5)", "inner": "star inner radius ratio (default 0.5)",
                "angles": "arc [from, to] degrees, 0 = up, clockwise (default [0, 270])",
                "trim": "line, ring, arc draw-on [from, to] in 0..1",
                "fill": "colour (animatable) or {gradient: linear|radial, from, to, stops: [[0, colour], ...], units: layer|box}; units \"box\": from/to as fractions of the shape's box ([0,0] top-left, [1,1] bottom-right; a path's bounds) so the gradient follows an animated size (radials become ellipses); default \"layer\" px",
                "points": "field only: [{\"position\": [x, y] layer space (animatable), \"color\": colour (animatable), \"radius\": px the colour holds (animatable, default 200)}, …] blended smoothly across the size box (mesh/blob gradient background)",
                "softness": "field: how far colours blend (default 1; 0.25 crisp, 2 very soft; animatable)",
                "warp": "field: noise displacement px of the blend (default 0; animatable)", "warp_scale": "field: warp noise feature size px (default 300)",
                "stroke": {"color": "colour (default white)", "width": "px", "gradient": "path strokes: {\"stops\": [[0, colour], [1, colour]], \"along\": \"length\"} colour along the full path's length (0 = start, 1 = end; trim reveals it without sliding), replaces color; works with caps, joins, dashes, feathers", "cap": "round | butt | square (default round)", "join": "round | miter | bevel (default round)", "miter_limit": "default 4", "dash": "[dash, gap, …] px", "dash_offset": "px, animatable (marching ants)", "feather": "px faded at the far end (the head while trim draws it on)", "feather_start": "px faded at the start (a fading tail)", "note": "cap, join, dash and feathers apply to path strokes"}
            },
            "text": {
                "text": "string; \\n breaks lines", "font": "Inter | Inter Medium | Inter SemiBold | project-relative .ttf/.otf",
                "size": "px (default 96)", "color": "colour (a transparent colour with a stroke gives outline-only type)", "tracking": "1/1000 em", "line_height": 1.15, "align": "left | center | right",
                "stroke": {"color": "colour (animatable)", "width": "px (animatable, default 4)", "align": "outside (default; drawn under all fills, so touching letters share one outline) | center | inside (both over the fill)"},
                "weight": "faux bold: px the glyph edges move outward, negative thins (animatable)",
                "edge": {"color": "rim colour (default white)", "width": "px inside the edge (default 3)", "angle": "where the light comes from, degrees, 0 = above, clockwise (animatable)", "note": "rim light: the colour over the fill along edges facing the light, fading inward"},
                "outline_reach": "weight + outside stroke reach at most ~0.11 em past the glyph outline (larger values are clamped)",
                "animators": [{
                    "unit": "glyph | word | line",
                    "order": "forward | reverse | center | edges | random, or by position: {\"axis\": \"x\" | \"y\" | degrees (sweep direction, 0 = rightward, clockwise), \"from\": \"start\" | \"end\" | \"center\"} or {\"radial\": [fx, fy] (centre as fractions of the text block)}; each unit's delay = its position (0 first … 1 last) × stagger × (units − 1); add \"feather\": 0..1 (fraction of the text extent) to reveal opacity as a soft wipe through the glyphs (one feathered animator per layer; other channels stay per unit)",
                    "seed": 0,
                    "stagger": "per-unit delay (default \"1f\")", "at": "time of the first unit's anchor", "anchor": "default start",
                    "lead": "frames", "dur": "per-unit duration", "ease": "default enter",
                    "from": {"x": 0, "y": 0, "scale": 1, "rotation": 0, "opacity": 1}, "to": "same fields (default identity)",
                    "range": "[first, last] 1-based units"
                }]
            },
            "image": {"src": "project-relative PNG/JPEG/WebP", "size": "[w, h] display px (default natural)"},
            "video": {
                "src": "video file (mp4/mov/webm/mkv; alpha from ProRes 4444, VP8/VP9 alpha, PNG/QT-RLE), an image sequence \"shot_####.png\" (Blender style) or a folder of frames; import with media_import or make with blender_run",
                "size": "[w, h] display px (default the footage size)", "trim": "source seconds at the layer start", "speed": "rate (default 1)",
                "loop": "restart at trim when the source ends (default: hold the last frame)", "time_remap": "animatable source seconds (freeze, ramps, reverse); overrides trim/speed",
                "fps": "image sequences: frames per second (default the project's)", "volume_db": "sound level in the mix", "mute": "bool"
            },
            "comp": {"layers": "nested layers; transform and stepping apply to all children; with opacity < 1, a blend mode, effects or a matte the comp is drawn as one picture"},
            "instancer": {
                "note": "a cloner: draws its layers (the template, like a comp) once per layout point, centred on it; use it instead of many hand-placed layers (dot fields, rings of petals, rows of bars). Copies share the template's ids. Max 50000 copies",
                "layers": "the template (any layers, comps included); its own transform and keys apply inside each copy",
                "layout": [
                    {"type": "grid", "columns": "count", "rows": "count", "spacing": "[x, y] px or number (default 50, animatable)", "hex": "bool: odd rows shift half a cell"},
                    {"type": "radial", "count": "copies", "radius": "px (default 200, animatable)", "start": "degrees, 0 = up, clockwise (animatable)", "sweep": "degrees (default 360; less includes both ends)", "align": "bool: turn each copy to its angle (its up points outward)"},
                    {"type": "line", "count": "copies", "from": "[x, y] (default [-500, 0])", "to": "[x, y] (default [500, 0])", "align": "bool: x axis along the line"},
                    {"type": "random", "count": "copies", "size": "[w, h] (default [1000, 1000])", "area": "box | circle", "seed": 0},
                    {"type": "path", "count": "copies", "layer": "sibling shape \"path\" layer (its morph and transform)", "offset": "fraction moved along the path (animatable; closed paths wrap)", "align": "bool: turn along the path"}
                ],
                "layout_space": "the instancer's own space: its position is the layout's centre",
                "stagger": {"each": "delay per rank (duration)", "total": "or spread all ranks over this duration", "order": "forward | reverse | center (rings out from the middle) | edges | random", "seed": 0, "note": "copy k sees time t − delay: its keys and window start later"},
                "random": {"seed": 0, "scale": "[min, max] multiplier", "rotation": "[min, max] degrees", "opacity": "[min, max] multiplier", "position": "[x, y] ± jitter px", "time": "[min, max] extra delay seconds", "color": "[colour, colour]: a random mix (replaces the palette)"},
                "palette": "[colour, …] replacing the template's solid fill, stroke and text colours (alpha kept)", "palette_order": "cycle | random",
                "fields": [{
                    "type": "radial (center, radius, falloff px: default the radius) | linear (center, angle degrees, range px: ramps 0→1 along angle) | noise (size px, speed per second, seed) | wave (center, at time, speed px/s, wavelength px, cycles: crests (default 1, 0 = endless), decay px)",
                    "deltas": {"position": "[x, y] px", "scale": "added to the copy's scale (1 doubles it)", "rotation": "degrees", "opacity": "added (−1 hides)", "color": "mixed in"},
                    "shaping": {"ease": "curve applied to the 0..1 value", "invert": "bool", "amount": "weight 0..1 (default 1)"},
                    "note": "each field gives every copy a 0..1 value at its layout point and adds value × delta; fields apply in list order; every parameter and delta is animatable (keys) except at, seed, cycles and decay"
                }]
            }
            ,
            "mesh": {
                "note": "a lit 3D object rendered in the engine (no Blender): always 3D, sorts by depth with 3D layers; meshes next to each other in the layer list share a depth buffer and intersect correctly. Space: x right, y down, z away; centred on the layer origin; transform rotation_x/rotation_y/rotation/z/scale place it (points: scale, then rotation, then rotation_x, then rotation_y, then position/z; depth scales with scale × scale_z). Build real objects this way instead of faking 3D with flat shapes",
                "mesh": [
                    {"type": "sphere", "radius": "px (default 100)"},
                    {"type": "plane", "size": "[w, h] (default [100, 100]): flat rectangle, both sides; floors and backdrops (rotation_x 90 lays it flat)"},
                    {"type": "box", "size": "[w, h] (default [100, 100])", "depth": "px (default 100)", "bevel": "edge radius px"},
                    {"type": "cylinder", "radius": "px (default 50)", "top": "top radius (cone/frustum; default = radius)", "height": "along y (default 100)", "bevel": "rim radius px"},
                    {"type": "capsule", "radius": "px (default 50)", "height": "end to end along y (default 200)"},
                    {"type": "torus", "radius": "to the tube centre (default 100)", "tube": "tube radius (default 20)", "angles": "[from, to] degrees like an arc: 0 = up, clockwise (default [0, 360]); animate for a ring that draws on", "caps": "flat (default) | round: ends of an open arc"},
                    {"type": "extrude", "d": "SVG path, {\"shape\": …} primitive or morph keys: closed outline (holes kept)", "depth": "px (default 40)", "bevel": "rounded edge px"},
                    {"type": "revolve", "profile": "path in (x = distance from axis, y = along it)", "angles": "[from, to] degrees", "axis": "y (standing, default) | z (facing the camera)"},
                    {"type": "sweep", "d": "path in the layer plane", "radius": "tube radius (default 10)", "profile": "circle (default) | square | ribbon", "width": "ribbon width (default 40)", "trim": "[from, to] 0..1 drawn part (animatable: tubes that grow)", "caps": "round (default) | flat", "head": "radius of a ball leading the tube (0 = none)"}
                ],
                "segments": "split rings, revolves and sweeps into this many pieces (default 1)", "gap": "degrees between pieces (sweeps: fraction of the length)", "spans": "[relative size, …] of the pieces in turn (default equal)",
                "colors": "[colour, …] one per piece in turn (animatable)",
                "material": material,
                "parts": {"front": "material over `material` for the part (rings/revolves: facing −axis)", "back": "…", "side": "… (rings/revolves: facing away from the axis)", "bevel": "…", "caps": "…", "inside": "rings/revolves: facing the axis"},
                "detail": "tessellation multiplier (default 1)",
                "depth": "meshes and flat 3D layers in the same layer list share one depth test (planes cut through meshes, any order); see-through meshes (opacity, glass, blend) draw after the opaque ones",
                "path_tracing": "export only: project settings.renderer \"path\" (samples: paths per pixel, default 64, drafts a quarter) or render --pt / snapshot --pt (--spp N): ray-traced shadows, bounced light, reflections of the scene, refraction of meshes behind glass; same camera, so meshes stand where the raster draws them. Needs a GPU with ray queries, else rasterised with a warning; lists with wireframes, effects (depth of field too), mattes or blend modes on 3D layers are rasterised"
            }
        },
        "scene": {
            "id": "stable id", "start": "time (cuts on markers lead −1 f by default)", "lead": "frames",
            "end": "time (default next scene's start or the project end)", "duration": "duration",
            "pre_roll": "open this far into the animation", "background": "colour (animatable)",
            "transition": {"type": "fade | push | slide | zoom | wipe | whip", "dur": "default \"10f\", centred on the cut", "ease": "default gesture (peak speed on the cut)", "direction": "left | right | up | down"},
            "camera": {"position": "[x, y] point of interest shown at the canvas centre", "zoom": "number", "rotation": "degrees (roll)", "fov": "3D: vertical degrees (39.6 keeps z = 0 at 1:1)", "orbit": "3D: [yaw, pitch] degrees around the point of interest", "dolly": "3D: px toward the point of interest", "shift": "3D lens shift [x, y] px: moves the 3D picture without turning the camera (verticals stay parallel)", "aperture": "3D depth of field: blur radius px per 100 px of depth from the focus plane (0 = off; animatable)", "focus": "3D depth of field: sharp plane as a depth like a layer's z (0 = point of interest, default; animatable for rack focus) or a top-level layer id to keep sharp"},
            "lights": [{"type": "directional (default) | point | ambient", "from": "directional: [x, y, z] toward the light (y down, z away: [-1, -1, -1] = upper left, in front)", "position": "point: [x, y, z] world px (animatable; from is too)", "color": "colour (animatable)", "intensity": "animatable (default 1)", "shadow": "bool: cast soft shadows (default: the first directional/point light only)", "softness": "0 sharp … 1 very soft (default 0.35, animatable; path tracing: the light's size)", "range": "point: px where it has faded to nothing (smooth; default none: even light, animatable)", "note": "for mesh layers; default rig: key from the upper left front (casts shadows) + rim from behind right"}],
            "environment": {"preset": "studio (default) | soft | sunset | night | none", "intensity": "animatable", "rotation": "degrees about the vertical axis", "sky": "colour (animatable)", "horizon": "colour (animatable)", "ground": "colour (animatable)", "note": "what glossy meshes reflect and their ambient light"},
            "motion_blur": {"shutter": "degrees (180 film-like)", "samples": 8},
            "step_fps": "stepped clock for all layers (camera stays smooth)", "layers": "bottom first"
        },
        "text_shaping": "HarfBuzz shaping (kerning, ligatures, marks) with bidi runs; use a project font for scripts Inter lacks (Arabic, Urdu, Devanagari…)",
        "edit_ops": ["add", "add_scene", "set", "remove", "move", "key", "anchor", "group", "ungroup"],
        "style_packs": styles_json(),
    });
    catalog["components"] = Value::from(components);
    catalog["component_layer"] = json!("{\"kind\": \"component\", \"component\": <name>, \"params\": {…}} plus the usual transform/time/effects. It expands to ordinary layers, addressed as <id>/<child> (timeline_get lists them): set/key/anchor/remove on a child (\"map/IRN.fill\", \"caps/c3.text\") is kept in `overrides` and survives changes to params; {\"op\": \"detach\", \"id\": <id>} turns it into a comp of plain layers.");
    catalog
}
