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
    json!({
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
            "\"word:borders\"", "\"word:borders@2\"", "\"word#42\"", "\"sentence#3.end\"",
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
            "blend": "normal | add | multiply | screen",
            "effects": [
                {"type": "blur", "radius": "sigma in px (animatable)"},
                {"type": "shadow", "color": "default #00000080", "offset": "[x, y] px (default [0, 12])", "radius": "blur sigma (default 12)"},
                {"type": "glow", "color": "tint (white keeps the layer's colours)", "radius": "sigma (default 12)", "intensity": "default 1"}
            ],
            "effects_note": "effects apply to the layer as one picture; on a comp they apply to the flattened comp (like an AE precomp)",
            "matte": {"layer": "sibling layer id used as a track matte (not drawn itself)", "mode": "alpha | alpha_inverted | luma | luma_inverted"},
            "parent": "transform parent: sibling layer id",
            "hidden": "bool"
        },
        "layer_kinds": {
            "shape": {
                "shape": "circle | rect | capsule | ring | arc | polygon | star | line | path",
                "d": "path only: SVG path data in layer space (M L H V C S Q T A Z, absolute or relative); fill uses non-zero winding; trim draws the stroke on",
                "size": "[w, h] (default [100, 100]); circle/ring/arc/polygon/star use size[0] as diameter",
                "radius": "corner rounding (rect, polygon, star)", "thickness": "ring, arc and line width (default 10)",
                "sides": "polygon sides / star points (default 5)", "inner": "star inner radius ratio (default 0.5)",
                "angles": "arc [from, to] degrees, 0 = up, clockwise (default [0, 270])",
                "trim": "line, ring, arc draw-on [from, to] in 0..1",
                "fill": "colour (animatable) or {gradient: linear|radial, from, to, stops: [[0, colour], ...]}",
                "stroke": {"color": "colour", "width": "px"}
            },
            "text": {
                "text": "string; \\n breaks lines", "font": "Inter | Inter Medium | Inter SemiBold | project-relative .ttf/.otf",
                "size": "px (default 96)", "color": "colour", "tracking": "1/1000 em", "line_height": 1.15, "align": "left | center | right",
                "animators": [{
                    "unit": "glyph | word | line", "order": "forward | reverse | center | edges | random", "seed": 0,
                    "stagger": "per-unit delay (default \"1f\")", "at": "time of the first unit's anchor", "anchor": "default start",
                    "lead": "frames", "dur": "per-unit duration", "ease": "default enter",
                    "from": {"x": 0, "y": 0, "scale": 1, "rotation": 0, "opacity": 1}, "to": "same fields (default identity)",
                    "range": "[first, last] 1-based units"
                }]
            },
            "image": {"src": "project-relative PNG/JPEG/WebP", "size": "[w, h] display px (default natural)"},
            "comp": {"layers": "nested layers; transform and stepping apply to all children; with opacity < 1, a blend mode, effects or a matte the comp is drawn as one picture"}
        },
        "scene": {
            "id": "stable id", "start": "time (cuts on markers lead −1 f by default)", "lead": "frames",
            "end": "time (default next scene's start or the project end)", "duration": "duration",
            "pre_roll": "open this far into the animation", "background": "colour",
            "transition": {"type": "fade | push | slide | zoom | wipe | whip", "dur": "default \"10f\", centred on the cut", "ease": "default gesture (peak speed on the cut)", "direction": "left | right | up | down"},
            "camera": {"position": "[x, y] point shown at the canvas centre", "zoom": "number", "rotation": "degrees"},
            "motion_blur": {"shutter": "degrees (180 film-like)", "samples": 8},
            "step_fps": "stepped clock for all layers (camera stays smooth)", "layers": "bottom first"
        },
        "text_shaping": "HarfBuzz shaping (kerning, ligatures, marks) with bidi runs; use a project font for scripts Inter lacks (Arabic, Urdu, Devanagari…)",
        "edit_ops": ["add", "add_scene", "set", "remove", "move", "key", "anchor", "group", "ungroup"],
        "style_packs": styles_json(),
    })
}
