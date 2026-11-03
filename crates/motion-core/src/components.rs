//! Components: parameters plus a generator that expands to ordinary
//! layers (design: high-level pieces stay editable).
//!
//! A component layer (`"kind": "component"`) holds `component` (its type),
//! `params` and `overrides`. At compile time its generator turns the params
//! into plain layers with stable ids, drawn like a comp's children and
//! addressed as `<component>/<child>` (`map/IRN.fill`, `caps/c3.text`).
//! Editing a child stores a merge patch in `overrides[child]` (any property,
//! keyframes included), applied on top of every regeneration — like an
//! instance override in Figma. `detach` turns the component into a comp of
//! plain layers. Generators are deterministic: same params and markers,
//! same layers.

use crate::markers::Markers;
use crate::time::Fps;
use serde_json::{json, Map, Value};

/// What a generator may read besides its params.
pub struct Ctx<'a> {
    pub width: f64,
    pub height: f64,
    pub fps: Fps,
    pub markers: &'a Markers,
}

pub struct Kind {
    pub name: &'static str,
    pub summary: &'static str,
    /// Params: (name, what, default).
    pub params: &'static [(&'static str, &'static str, &'static str)],
}

pub const KINDS: &[Kind] = &[
    Kind {
        name: "captions",
        summary: "Word-synced captions from a word track (narration): chunks by length, sentence ends and pauses; each chunk enters, each word lights up as it is spoken. Canvas coordinates. Children c1, c2, … (and c1-box with style boxed).",
        params: &[
            ("track", "marker track of timed words", "\"word\""),
            ("from / to", "time window (seconds or references)", "all words"),
            ("max_chars", "characters per chunk", "32"),
            ("line_chars", "characters per line (longer chunks wrap to two lines)", "22"),
            ("pause", "a gap this long (s) starts a new chunk", "0.45"),
            ("hold", "seconds a chunk stays after its last word", "0.35"),
            ("style", "bold (outlined), boxed (on a dark box) or minimal (shadow)", "\"bold\""),
            ("position", "centre of the caption [x, y]", "[w/2, 0.78 h]"),
            ("keep_out", "[x, y, w, h] region captions must not cover (moved below or above it)", "none"),
            ("size, font, color, highlight", "text size px, font, colour, colour of the spoken word", "h/28, \"Inter SemiBold\", white, #FFD400"),
            ("entrance", "pop, fade or rise", "\"pop\""),
            ("uppercase", "capitalise", "false"),
        ],
    },
    Kind {
        name: "card",
        summary: "A paper card with grain, a soft shadow and two strips of tape, holding a kicker, a title and a body. Centred on the component's position. Children paper, tape-l, tape-r, kicker, title, body.",
        params: &[
            ("size", "[w, h]", "[760, 480]"),
            ("kicker, title, body", "texts (body wraps)", "\"\""),
            ("paper, ink, accent", "colours", "#f5efe2, #1d1b19, #c8102e"),
            ("tape", "draw the tape", "true"),
            ("rotation", "degrees", "-2"),
            ("at", "entrance time (drops in); none: no entrance", "none"),
        ],
    },
    Kind {
        name: "callout",
        summary: "A label with a leader line to a point: a dot pops on the point, the line draws to the label, the label appears. Canvas coordinates. Children dot, line, label (and box).",
        params: &[
            ("point", "[x, y] pointed at", "required"),
            ("label_at", "[x, y] of the label's centre", "required"),
            ("text", "label text", "required"),
            ("at", "when the dot lands", "0"),
            ("color, text_color, size, font", "line/dot colour, label colour, text size, font", "white, white, 40, \"Inter SemiBold\""),
            ("box", "a dark box behind the label", "true"),
        ],
    },
    Kind {
        name: "counter",
        summary: "An animated number: counts from `from` to `to` over `dur` from `at`, formatted with decimals, a thousands separator, prefix and suffix. Centred on the component's position. One child per displayed value (v0, v1, …).",
        params: &[
            ("from, to", "values", "0, required"),
            ("at, dur", "start and length of the count", "0, \"1s\""),
            ("decimals, separator, prefix, suffix", "format", "0, \",\", \"\", \"\""),
            ("ease", "how it counts", "\"glide\""),
            ("size, font, color", "text", "120, \"Inter SemiBold\", white"),
        ],
    },
    Kind {
        name: "chart",
        summary: "A line or bar chart from data, with axes, labels and a draw-on. Centred on the component's position. Children x-axis, y-axis, line (or bar1…), dot, label1…, value-last.",
        params: &[
            ("type", "line or bar", "\"line\""),
            ("data", "values, or [x, y] pairs", "required"),
            ("labels", "x labels (one per value)", "none"),
            ("size", "[w, h] of the plot", "[800, 500]"),
            ("min, max", "y range", "from the data (min 0)"),
            ("at, dur", "draw-on start and length", "0, \"1.2s\""),
            ("color, axis_color, text_color", "colours", "#FFD400, #ffffff66, white"),
            ("value_format", "suffix after the last value's label, e.g. \"%\"", "\"\""),
        ],
    },
];

pub fn kind(name: &str) -> Option<&'static Kind> {
    KINDS.iter().find(|k| k.name == name)
}

/// The component's children as layer JSON (plain ids, no overrides).
pub fn generate(component: &str, params: &Value, ctx: &Ctx) -> Result<Vec<Value>, String> {
    let p = Params(params);
    match component {
        "captions" => captions(&p, ctx),
        "card" => card(&p),
        "callout" => callout(&p),
        "counter" => counter(&p, ctx),
        "chart" => chart(&p),
        other => Err(format!("unknown component '{other}': {}", KINDS.iter().map(|k| k.name).collect::<Vec<_>>().join(", "))),
    }
}

/// Children with overrides applied; a `null` override removes a child.
pub fn children(component: &str, params: &Value, overrides: &Map<String, Value>, ctx: &Ctx) -> Result<Vec<Value>, String> {
    let mut out = Vec::new();
    for mut child in generate(component, params, ctx)? {
        let id = child["id"].as_str().unwrap_or("").to_string();
        match overrides.get(&id) {
            Some(Value::Null) => continue,
            Some(patch) => merge_patch(&mut child, patch),
            None => {}
        }
        out.push(child);
    }
    Ok(out)
}

/// A project with its components expanded (None when it has none).
pub fn expand_project(project: &crate::document::Project, errors: &mut Vec<String>) -> Option<crate::document::Project> {
    use crate::document::{Layer, LayerKind};
    fn has(layers: &[Layer]) -> bool {
        layers.iter().any(|l| matches!(l.kind, LayerKind::Component(_)) || has(l.children()))
    }
    if !project.scenes.iter().any(|s| has(&s.layers)) {
        return None;
    }
    let ctx = Ctx { width: project.settings.width as f64, height: project.settings.height as f64, fps: project.settings.fps, markers: &project.markers };
    fn expand(layers: &mut [Layer], ctx: &Ctx, errors: &mut Vec<String>) {
        for layer in layers {
            if let LayerKind::Component(c) = &layer.kind {
                let children = build_children(&layer.id, c, ctx).unwrap_or_else(|e| {
                    errors.push(format!("{}: {e}", layer.id));
                    Vec::new()
                });
                layer.kind = LayerKind::Comp(crate::document::CompLayer { layers: children });
            } else if let Some(children) = layer.children_mut() {
                expand(children, ctx, errors);
            }
        }
    }
    let mut p = project.clone();
    for scene in &mut p.scenes {
        expand(&mut scene.layers, &ctx, errors);
    }
    Some(p)
}

/// The component's children as layers, with ids `<component>/<child>`.
pub fn build_children(id: &str, c: &crate::document::ComponentLayer, ctx: &Ctx) -> Result<Vec<crate::document::Layer>, String> {
    let mut json = children(&c.component, &c.params, &c.overrides, ctx)?;
    let prefix = format!("{id}/");
    rename(&mut json, &prefix, false);
    json.into_iter()
        .map(|j| {
            let child = j["id"].as_str().unwrap_or("").to_string();
            let mut layer = crate::document::Layer::from_json(j).map_err(|e| format!("child {child}: {e}"))?;
            layer.id = format!("{prefix}{child}");
            Ok(layer)
        })
        .collect()
}

/// Prefixes children's references to one another (`parent`,
/// `matte.layer`, `follow.layer`) and, with `ids`, their ids too.
pub fn rename(children: &mut [Value], prefix: &str, ids: bool) {
    let rename_ids = ids;
    let ids: Vec<String> = children.iter().filter_map(|c| c["id"].as_str().map(String::from)).collect();
    let fix = |v: &mut Value| {
        if let Some(s) = v.as_str() {
            if ids.iter().any(|i| i == s) {
                *v = json!(format!("{prefix}{s}"));
            }
        }
    };
    for c in children.iter_mut() {
        if rename_ids {
            if let Some(id) = c.get_mut("id") {
                fix(id);
            }
        }
        if let Some(v) = c.get_mut("parent") {
            fix(v);
        }
        if let Some(v) = c.pointer_mut("/matte/layer") {
            fix(v);
        }
        if let Some(v) = c.pointer_mut("/follow/layer") {
            fix(v);
        }
    }
}

/// RFC 7386 merge patch.
pub fn merge_patch(target: &mut Value, patch: &Value) {
    match patch {
        Value::Object(p) => {
            if !target.is_object() {
                *target = json!({});
            }
            let t = target.as_object_mut().unwrap();
            for (k, v) in p {
                if v.is_null() {
                    t.remove(k);
                } else {
                    merge_patch(t.entry(k.clone()).or_insert(Value::Null), v);
                }
            }
        }
        other => *target = other.clone(),
    }
}

/// The merge patch that turns `base` into `new` (None: no difference).
pub fn diff_patch(base: &Value, new: &Value) -> Option<Value> {
    match (base, new) {
        (Value::Object(b), Value::Object(n)) => {
            let mut patch = Map::new();
            for (k, nv) in n {
                match b.get(k) {
                    Some(bv) => {
                        if let Some(d) = diff_patch(bv, nv) {
                            patch.insert(k.clone(), d);
                        }
                    }
                    None => {
                        patch.insert(k.clone(), nv.clone());
                    }
                }
            }
            for k in b.keys() {
                if !n.contains_key(k) {
                    patch.insert(k.clone(), Value::Null);
                }
            }
            (!patch.is_empty()).then_some(Value::Object(patch))
        }
        (b, n) if b == n => None,
        (_, n) => Some(n.clone()),
    }
}

// ---------------------------------------------------------------- helpers

struct Params<'a>(&'a Value);

impl Params<'_> {
    fn get(&self, k: &str) -> Option<&Value> {
        self.0.get(k).filter(|v| !v.is_null())
    }
    fn f(&self, k: &str, d: f64) -> f64 {
        self.get(k).and_then(Value::as_f64).unwrap_or(d)
    }
    fn s<'a>(&'a self, k: &str, d: &'a str) -> &'a str {
        self.get(k).and_then(Value::as_str).unwrap_or(d)
    }
    fn b(&self, k: &str, d: bool) -> bool {
        self.get(k).and_then(Value::as_bool).unwrap_or(d)
    }
    fn v(&self, k: &str, d: Value) -> Value {
        self.get(k).cloned().unwrap_or(d)
    }
    fn xy(&self, k: &str) -> Option<(f64, f64)> {
        let a = self.get(k)?.as_array()?;
        Some((a.first()?.as_f64()?, a.get(1)?.as_f64()?))
    }
    fn need_xy(&self, k: &str) -> Result<(f64, f64), String> {
        self.xy(k).ok_or_else(|| format!("needs \"{k}\": [x, y]"))
    }
}

/// `at` plus `frames` (a number, or a reference with an offset).
fn plus(at: &Value, frames: f64) -> Value {
    if frames == 0.0 {
        return at.clone();
    }
    match at {
        Value::Number(n) => json!(n.as_f64().unwrap_or(0.0) + frames / 30.0),
        Value::String(s) => json!(if frames >= 0.0 { format!("{s} + {frames}f") } else { format!("{s} - {}f", -frames) }),
        _ => json!(frames / 30.0),
    }
}

/// Splits `text` into lines of at most `n` characters, at spaces.
fn wrap(text: &str, n: usize) -> String {
    let mut lines: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        match lines.last_mut() {
            Some(l) if l.chars().count() + 1 + word.chars().count() <= n => {
                l.push(' ');
                l.push_str(word);
            }
            _ => lines.push(word.to_string()),
        }
    }
    lines.join("\n")
}

/// Rough text width: Inter averages about 0.55 em per character.
fn text_width(text: &str, size: f64) -> f64 {
    text.lines().map(|l| l.chars().count()).max().unwrap_or(0) as f64 * size * 0.55
}

fn format_number(v: f64, decimals: usize, sep: &str) -> String {
    let s = format!("{:.*}", decimals, v.abs());
    let (int, frac) = s.split_once('.').map(|(a, b)| (a.to_string(), Some(b.to_string()))).unwrap_or((s.clone(), None));
    let mut grouped = String::new();
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            grouped.push_str(sep);
        }
        grouped.push(c);
    }
    let sign = if v < 0.0 && s.chars().any(|c| c.is_ascii_digit() && c != '0') { "-" } else { "" };
    match frac {
        Some(f) => format!("{sign}{grouped}.{f}"),
        None => format!("{sign}{grouped}"),
    }
}

// --------------------------------------------------------------- captions

fn captions(p: &Params, ctx: &Ctx) -> Result<Vec<Value>, String> {
    let track_name = p.s("track", "word");
    let Some(track) = ctx.markers.get(track_name) else {
        return Err(format!("captions: no '{track_name}' track (narration_import builds it)"));
    };
    let resolve = |k: &str| -> Result<Option<f64>, String> {
        match p.get(k) {
            None => Ok(None),
            Some(v) => {
                let r: crate::timeref::TimeRef = serde_json::from_value(v.clone()).map_err(|e| format!("captions {k}: {e}"))?;
                let base = crate::time::TimeBase::new(ctx.fps);
                r.resolve(ctx.markers, &base, &mut crate::timeref::NoElements).map(Some).map_err(|e| format!("captions {k}: {e}"))
            }
        }
    };
    let (from, to) = (resolve("from")?.unwrap_or(f64::NEG_INFINITY), resolve("to")?.unwrap_or(f64::INFINITY));
    let words: Vec<(f64, f64, String)> = track
        .markers
        .iter()
        .filter(|m| m.t >= from && m.t < to)
        .filter_map(|m| Some((m.t, m.end.unwrap_or(m.t + 0.2), m.label.clone()?)))
        .collect();
    let max_chars = p.f("max_chars", 32.0) as usize;
    let line_chars = p.f("line_chars", 22.0) as usize;
    let pause = p.f("pause", 0.45);
    let hold = p.f("hold", 0.35);
    let upper = p.b("uppercase", false);
    // Chunks: break at sentence ends, pauses and the length limit.
    let mut chunks: Vec<Vec<usize>> = Vec::new();
    for (i, w) in words.iter().enumerate() {
        let start_new = match chunks.last() {
            None => true,
            Some(c) => {
                let last = &words[*c.last().unwrap()];
                let len: usize = c.iter().map(|&k| words[k].2.chars().count() + 1).sum::<usize>() + w.2.chars().count();
                last.2.trim_end_matches(['"', '\'', ')', '”']).ends_with(['.', '!', '?']) || w.0 - last.1 > pause || len > max_chars
            }
        };
        if start_new {
            chunks.push(vec![i]);
        } else {
            chunks.last_mut().unwrap().push(i);
        }
    }
    let size = p.f("size", (ctx.height / 28.0).round());
    let style = p.s("style", "bold");
    let font = p.s("font", "Inter SemiBold");
    let color = p.v("color", json!("#ffffff"));
    let highlight = p.v("highlight", json!("#FFD400"));
    let (mut x, mut y) = p.xy("position").unwrap_or((ctx.width / 2.0, ctx.height * 0.78));
    let entrance = p.s("entrance", "pop");
    let frame = 1.0 / ctx.fps.as_f64();
    let mut out = Vec::new();
    let lines_max = chunks.iter().map(|c| c.iter().map(|&k| words[k].2.chars().count() + 1).sum::<usize>()).max().unwrap_or(0).div_ceil(line_chars.max(1)).clamp(1, 2);
    // Keep the captions off a region (key content).
    if let Some(ko) = p.get("keep_out").and_then(Value::as_array) {
        let k: Vec<f64> = ko.iter().filter_map(Value::as_f64).collect();
        if k.len() == 4 {
            let half = size * lines_max as f64 * 0.7;
            if y + half > k[1] && y - half < k[1] + k[3] {
                let below = k[1] + k[3] + half + size * 0.4;
                y = if below + half <= ctx.height * 0.95 { below } else { k[1] - half - size * 0.4 };
            }
            let _ = &mut x;
        }
    }
    for (n, chunk) in chunks.iter().enumerate() {
        let id = format!("c{}", n + 1);
        let first = &words[chunk[0]];
        let last = &words[*chunk.last().unwrap()];
        let start = (first.0 - 2.0 * frame).max(0.0);
        let next = chunks.get(n + 1).map(|c| words[c[0]].0 - 2.0 * frame);
        let end = match next {
            Some(nx) => nx.min(last.1 + hold).max(start + frame),
            None => last.1 + hold,
        };
        let mut text: String = chunk.iter().map(|&k| words[k].2.as_str()).collect::<Vec<_>>().join(" ");
        if upper {
            text = text.to_uppercase();
        }
        let text = if text.chars().count() > line_chars { wrap_balanced(&text) } else { text };
        // Two lines rise so the bottom one stays inside the title-safe area.
        let lines = text.lines().count() as f64;
        let y = y.min(ctx.height * 0.94 - size * 1.15 * lines / 2.0 - size * 0.3);
        let r = |t: f64| (t * 1000.0).round() / 1000.0;
        // Each word lights up when spoken and goes back when the next one is.
        let mut animators = Vec::new();
        for (k, &wi) in chunk.iter().enumerate() {
            let w = &words[wi];
            animators.push(json!({"unit": "word", "range": [k + 1, k + 1], "at": r(w.0), "dur": "2f", "to": {"color": highlight}}));
            let off = chunk.get(k + 1).map(|&nx| words[nx].0).unwrap_or(w.1 + hold * 0.5);
            animators.push(json!({"unit": "word", "range": [k + 1, k + 1], "at": r(off), "dur": "3f", "to": {"color": color}}));
        }
        let (scale_keys, opacity_keys, pos) = match entrance {
            "fade" => (json!([1, 1]), json!({"keys": [{"at": r(start), "from": 0, "v": 1, "ease": "enter", "dur": "4f", "anchor": "start"}]}), json!([x, y])),
            "rise" => (
                json!([1, 1]),
                json!({"keys": [{"at": r(start), "from": 0, "v": 1, "ease": "enter", "dur": "5f", "anchor": "start"}]}),
                json!({"keys": [{"at": r(start), "from": [x, y + size * 0.4], "v": [x, y], "ease": "settle", "dur": "8f", "anchor": "start"}]}),
            ),
            _ => (
                json!({"keys": [{"at": r(start), "from": [0.82, 0.82], "v": [1, 1], "ease": "pop", "dur": "7f", "anchor": "start"}]}),
                json!({"keys": [{"at": r(start), "from": 0, "v": 1, "ease": "enter", "dur": "3f", "anchor": "start"}]}),
                json!([x, y]),
            ),
        };
        if style == "boxed" {
            let (bw, bh) = (text_width(&text, size) + size * 1.2, size * (1.35 * text.lines().count() as f64) + size * 0.6);
            out.push(json!({"id": format!("{id}-box"), "kind": "shape", "shape": "rect", "size": [bw, bh], "radius": size * 0.25,
                "fill": "#000000c0", "time": {"start": r(start), "end": r(end)}, "transform": {"position": pos.clone(), "scale": scale_keys.clone(), "opacity": opacity_keys.clone()}}));
        }
        let mut layer = json!({"id": id, "kind": "text", "text": text, "font": font, "size": size, "color": color, "align": "center",
            "line_height": 1.15, "animators": animators,
            "time": {"start": r(start), "end": r(end)}, "transform": {"position": pos, "scale": scale_keys, "opacity": opacity_keys}});
        match style {
            "minimal" => layer["effects"] = json!([{"type": "shadow", "color": "#000000aa", "offset": [0, size * 0.06], "radius": size * 0.2}]),
            "boxed" => {}
            _ => layer["stroke"] = json!({"color": "#000000", "width": (size * 0.12).round(), "align": "outside"}),
        }
        out.push(layer);
    }
    Ok(out)
}

/// Two lines of about equal length.
fn wrap_balanced(text: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    let total: usize = text.chars().count();
    let mut best = (usize::MAX, 1);
    let mut len = 0;
    for i in 1..words.len() {
        len += words[i - 1].chars().count() + 1;
        let diff = (len as isize - (total as isize - len as isize)).unsigned_abs();
        if diff < best.0 {
            best = (diff, i);
        }
    }
    format!("{}\n{}", words[..best.1].join(" "), words[best.1..].join(" "))
}

// ------------------------------------------------------------------- card

fn card(p: &Params) -> Result<Vec<Value>, String> {
    let (w, h) = p.xy("size").unwrap_or((760.0, 480.0));
    let paper = p.v("paper", json!("#f5efe2"));
    let ink = p.v("ink", json!("#1d1b19"));
    let accent = p.v("accent", json!("#c8102e"));
    let rot = p.f("rotation", -2.0);
    let pad = w * 0.08;
    let left = -w / 2.0 + pad;
    let (pos, r) = match p.get("at") {
        Some(at) => (
            json!({"keys": [{"at": at, "from": [0, -80], "v": [0, 0], "ease": "land", "dur": "10f", "anchor": "start"}]}),
            json!({"keys": [{"at": at, "from": rot - 6.0, "v": rot, "ease": "settle", "dur": "12f", "anchor": "start"}]}),
        ),
        None => (json!([0, 0]), json!(rot)),
    };
    let mut out = vec![json!({"id": "paper", "kind": "shape", "shape": "rect", "size": [w, h], "radius": 6, "fill": paper,
        "effects": [{"type": "shadow", "color": "#00000059", "offset": [0, 18], "radius": 28}, {"type": "grain", "amount": 0.07, "size": 1.6, "seed": 3}],
        "transform": {"position": pos, "rotation": r}})];
    let mut y = -h / 2.0 + pad;
    let mut text = |id: &str, s: &str, size: f64, font: &str, color: &Value, tracking: f64, chars: usize| {
        if s.is_empty() {
            return;
        }
        let s = wrap(s, chars);
        let lines = s.lines().count() as f64;
        let ty = y + size * 0.6 * lines;
        // Parented to the paper: they rotate and drop in with it.
        out.push(json!({"id": id, "kind": "text", "text": s, "font": font, "size": size, "color": color, "align": "left", "tracking": tracking,
            "line_height": 1.2, "transform": {"position": [left, ty]}, "parent": "paper"}));
        y += size * 1.25 * lines + size * 0.35;
    };
    let kicker = p.s("kicker", "").to_uppercase();
    text("kicker", &kicker, (h * 0.065).max(20.0), "Inter SemiBold", &accent, 120.0, ((w - 2.0 * pad) / ((h * 0.065).max(20.0) * 0.7)) as usize);
    let tsize = (h * 0.13).max(30.0);
    text("title", p.s("title", ""), tsize, "Inter SemiBold", &ink, 0.0, ((w - 2.0 * pad) / (tsize * 0.55)) as usize);
    let bsize = (h * 0.075).max(22.0);
    text("body", p.s("body", ""), bsize, "Inter", &ink, 0.0, ((w - 2.0 * pad) / (bsize * 0.52)) as usize);
    if p.b("tape", true) {
        for (id, x, a) in [("tape-l", -w / 2.0 + w * 0.12, -9.0), ("tape-r", w / 2.0 - w * 0.12, 8.0)] {
            out.push(json!({"id": id, "kind": "shape", "shape": "rect", "size": [w * 0.2, h * 0.09], "fill": "#e9dcb8d0",
                "effects": [{"type": "grain", "amount": 0.08, "size": 1.4, "seed": 5}],
                "transform": {"position": [x, -h / 2.0], "rotation": a}, "parent": "paper"}));
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------- callout

fn callout(p: &Params) -> Result<Vec<Value>, String> {
    let (px, py) = p.need_xy("point")?;
    let (lx, ly) = p.need_xy("label_at")?;
    let text = p.get("text").and_then(Value::as_str).ok_or("callout needs \"text\"")?;
    let at = p.v("at", json!(0));
    let color = p.v("color", json!("#ffffff"));
    let tcolor = p.v("text_color", json!("#ffffff"));
    let size = p.f("size", 40.0);
    let font = p.s("font", "Inter SemiBold");
    let (tw, th) = (text_width(text, size) + size * 0.9, size * 1.6);
    // The line ends at the label box's edge facing the point.
    let (dx, dy) = (px - lx, py - ly);
    let k = if dx.abs() * th > dy.abs() * tw { (tw / 2.0) / dx.abs().max(1e-6) } else { (th / 2.0) / dy.abs().max(1e-6) };
    let (ex, ey) = (lx + dx * k.min(1.0), ly + dy * k.min(1.0));
    let mut out = vec![
        json!({"id": "dot", "kind": "shape", "shape": "circle", "size": size * 0.45, "fill": color,
            "transform": {"position": [px, py], "scale": {"keys": [{"at": at, "from": [0, 0], "v": [1, 1], "ease": "pop", "dur": "8f", "anchor": "start"}]}}}),
        json!({"id": "line", "kind": "shape", "shape": "path", "d": format!("M {px} {py} L {ex} {ey}"),
            "stroke": {"color": color, "width": (size * 0.08).max(2.0), "cap": "round"},
            "trim": {"keys": [{"at": plus(&at, 4.0), "from": [0, 0], "v": [0, 1], "ease": "glide", "dur": "10f", "anchor": "start"}]}}),
    ];
    let appear = json!({"keys": [{"at": plus(&at, 12.0), "from": 0, "v": 1, "ease": "enter", "dur": "6f", "anchor": "start"}]});
    if p.b("box", true) {
        out.push(json!({"id": "box", "kind": "shape", "shape": "rect", "size": [tw, th], "radius": size * 0.2, "fill": "#000000b8",
            "transform": {"position": [lx, ly], "opacity": appear.clone()}}));
    }
    out.push(json!({"id": "label", "kind": "text", "text": text, "font": font, "size": size, "color": tcolor, "align": "center",
        "transform": {"position": [lx, ly], "opacity": appear}}));
    Ok(out)
}

// ---------------------------------------------------------------- counter

fn counter(p: &Params, ctx: &Ctx) -> Result<Vec<Value>, String> {
    let from = p.f("from", 0.0);
    let to = p.get("to").and_then(Value::as_f64).ok_or("counter needs \"to\"")?;
    let at = p.v("at", json!(0));
    let dur = match p.get("dur") {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(1.0),
        Some(Value::String(s)) => crate::time::Duration::parse(s).map_err(|e| e.to_string())?.seconds(&crate::time::TimeBase::new(ctx.fps)).map_err(|e| e.to_string())?,
        _ => 1.0,
    };
    let decimals = p.f("decimals", 0.0) as usize;
    let sep = p.s("separator", ",");
    let (prefix, suffix) = (p.s("prefix", ""), p.s("suffix", ""));
    let ease: crate::curve::Ease = serde_json::from_value(json!(p.s("ease", "glide"))).map_err(|e| format!("counter ease: {e}"))?;
    let curve = crate::curve::Curve::compile(&ease, &crate::time::TimeBase::new(ctx.fps)).map_err(|e| e.to_string())?;
    let size = p.f("size", 120.0);
    let font = p.s("font", "Inter SemiBold");
    let color = p.v("color", json!("#ffffff"));
    let fps = ctx.fps.as_f64();
    let frames = (dur * fps).ceil().max(1.0) as usize;
    let mut out = Vec::new();
    let mut last_text = String::new();
    // One layer per shown value, each on for the frames it is shown.
    let mut spans: Vec<(String, usize, usize)> = Vec::new();
    for f in 0..=frames {
        let progress = curve.progress(f as f64 / fps, dur);
        let text = format!("{prefix}{}{suffix}", format_number(from + (to - from) * progress, decimals, sep));
        if text != last_text {
            spans.push((text.clone(), f, f + 1));
            last_text = text;
        } else if let Some(s) = spans.last_mut() {
            s.2 = f + 1;
        }
    }
    let n = spans.len();
    for (i, (text, f0, f1)) in spans.into_iter().enumerate() {
        let start = plus(&at, f0 as f64 * 30.0 / fps);
        let mut time = json!({"start": start});
        if i + 1 < n {
            time["end"] = plus(&at, f1 as f64 * 30.0 / fps);
        } else if i == 0 {
            time = json!({"start": at.clone()});
        }
        out.push(json!({"id": format!("v{i}"), "kind": "text", "text": text, "font": font, "size": size, "color": color, "align": "center", "time": time}));
    }
    // Before the count, show the start value.
    if let Some(first) = out.first_mut() {
        first["time"].as_object_mut().unwrap().remove("start");
    }
    Ok(out)
}

// ------------------------------------------------------------------ chart

fn chart(p: &Params) -> Result<Vec<Value>, String> {
    let data = p.get("data").and_then(Value::as_array).ok_or("chart needs \"data\"")?;
    let points: Vec<(f64, f64)> = data
        .iter()
        .enumerate()
        .map(|(i, v)| match v {
            Value::Array(a) => (a.first().and_then(Value::as_f64).unwrap_or(i as f64), a.get(1).and_then(Value::as_f64).unwrap_or(0.0)),
            v => (i as f64, v.as_f64().unwrap_or(0.0)),
        })
        .collect();
    if points.is_empty() {
        return Err("chart: empty data".into());
    }
    let kind = p.s("type", "line");
    let (w, h) = p.xy("size").unwrap_or((800.0, 500.0));
    let ymax = p.f("max", points.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max));
    let ymin = p.f("min", points.iter().map(|p| p.1).fold(0.0f64, f64::min));
    let (xmin, xmax) = (points.iter().map(|p| p.0).fold(f64::INFINITY, f64::min), points.iter().map(|p| p.0).fold(f64::NEG_INFINITY, f64::max));
    let at = p.v("at", json!(0));
    let dur = p.v("dur", json!("1.2s"));
    let color = p.v("color", json!("#FFD400"));
    let axis = p.v("axis_color", json!("#ffffff66"));
    let tcolor = p.v("text_color", json!("#ffffff"));
    let n = points.len();
    let sx = |x: f64| -w / 2.0 + if kind == "bar" { (x - xmin + 0.5) / (xmax - xmin + 1.0) * w } else if xmax > xmin { (x - xmin) / (xmax - xmin) * w } else { w / 2.0 };
    let sy = |y: f64| h / 2.0 - (y - ymin) / (ymax - ymin).max(1e-9) * h;
    let fsize = (h * 0.06).max(18.0);
    let mut out = vec![
        json!({"id": "x-axis", "kind": "shape", "shape": "path", "d": format!("M {} {} L {} {}", -w / 2.0, h / 2.0, w / 2.0, h / 2.0), "stroke": {"color": axis, "width": 3}}),
        json!({"id": "y-axis", "kind": "shape", "shape": "path", "d": format!("M {} {} L {} {}", -w / 2.0, h / 2.0, -w / 2.0, -h / 2.0), "stroke": {"color": axis, "width": 3}}),
    ];
    if kind == "bar" {
        let bw = w / n as f64 * 0.62;
        for (i, (x, y)) in points.iter().enumerate() {
            let top = sy(*y);
            let bh = (h / 2.0 - top).max(0.0);
            // Bars grow from the axis: the anchor is at the bottom edge.
            out.push(json!({"id": format!("bar{}", i + 1), "kind": "shape", "shape": "rect", "size": [bw, bh], "fill": color,
                "transform": {"anchor": [0, bh / 2.0], "position": [sx(*x), h / 2.0],
                    "scale": {"keys": [{"at": plus(&at, i as f64 * 3.0), "from": [1, 0], "v": [1, 1], "ease": "settle", "dur": "14f", "anchor": "start"}]}}}));
        }
    } else {
        let d: Vec<String> = points.iter().enumerate().map(|(i, (x, y))| format!("{} {:.2} {:.2}", if i == 0 { "M" } else { "L" }, sx(*x), sy(*y))).collect();
        out.push(json!({"id": "line", "kind": "shape", "shape": "path", "d": d.join(" "), "stroke": {"color": color, "width": (h * 0.012).max(4.0), "cap": "round", "join": "round"},
            "trim": {"keys": [{"at": at, "from": [0, 0], "v": [0, 1], "ease": "glide", "dur": dur, "anchor": "start"}]}}));
        out.push(json!({"id": "dot", "kind": "shape", "shape": "circle", "size": (h * 0.035).max(12.0), "fill": color, "follow": {"layer": "line", "at": "head"},
            "transform": {"opacity": {"keys": [{"at": at, "from": 0, "v": 1, "ease": "enter", "dur": "3f", "anchor": "start"}]}}}));
    }
    if let Some(labels) = p.get("labels").and_then(Value::as_array) {
        for (i, (l, (x, _))) in labels.iter().zip(&points).enumerate() {
            if let Some(t) = l.as_str() {
                out.push(json!({"id": format!("label{}", i + 1), "kind": "text", "text": t, "font": "Inter", "size": fsize, "color": tcolor, "align": "center",
                    "transform": {"position": [sx(*x), h / 2.0 + fsize * 1.1]}}));
            }
        }
    }
    let (lx, ly) = points[n - 1];
    out.push(json!({"id": "value-last", "kind": "text", "text": format!("{}{}", format_number(ly, if ly.fract() == 0.0 { 0 } else { 1 }, ","), p.s("value_format", "")),
        "font": "Inter SemiBold", "size": fsize * 1.6, "color": color, "align": "center",
        "transform": {"position": [sx(lx), sy(ly) - fsize * 1.6], "opacity": {"keys": [{"at": plus(&at, 30.0), "from": 0, "v": 1, "ease": "enter", "dur": "6f", "anchor": "start"}]}}}));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn patches_round_trip() {
        let base = json!({"id": "IRN", "fill": "#ccc", "transform": {"position": [1, 2], "opacity": 1}});
        let new = json!({"id": "IRN", "fill": {"keys": [{"at": 1, "v": "#f00"}]}, "transform": {"position": [1, 2]}});
        let patch = diff_patch(&base, &new).unwrap();
        let mut b = base.clone();
        merge_patch(&mut b, &patch);
        assert_eq!(b, new);
        assert_eq!(diff_patch(&base, &base), None);
    }

    #[test]
    fn numbers_format() {
        assert_eq!(format_number(1234567.0, 0, ","), "1,234,567");
        assert_eq!(format_number(-12.345, 1, ","), "-12.3");
        assert_eq!(format_number(999.0, 0, " "), "999");
    }

    fn project() -> crate::document::Project {
        let mut p = crate::document::Project::new(1080, 1920, crate::time::Fps::whole(30), 6.0);
        p.scenes.push(crate::document::Scene::new("s1"));
        let words = ["One", "bomb.", "Over", "Iran.", "It", "hit", "81", "cities."];
        p.markers.insert(crate::markers::MarkerTrack {
            name: "word".into(),
            source: None,
            markers: words.iter().enumerate().map(|(i, w)| crate::markers::Marker { t: 0.5 + i as f64 * 0.4, end: Some(0.8 + i as f64 * 0.4), label: Some(w.to_string()), ..Default::default() }).collect(),
        });
        p
    }

    #[test]
    fn overrides_survive_regeneration_and_detach_gives_plain_layers() {
        use crate::edit::apply;
        let p = project();
        let p = apply(&p, &[json!({"op": "add", "scene": "s1", "id": "card", "kind": "component", "component": "card",
            "params": {"title": "Fordow", "body": "Built 80 m under a mountain.", "kicker": "Target 1"}, "transform": {"position": [540, 700]}})]).unwrap().project;
        let c = crate::compile::Compiled::new(&p);
        assert!(c.errors.is_empty(), "{:?}", c.errors);
        // Keyframe a child: the change is kept as an override.
        let a = apply(&p, &[json!({"op": "key", "target": "card/title.color", "at": 1.0, "v": "#ff0000"})]).unwrap();
        let v = serde_json::to_value(&a.project).unwrap();
        let ov = &v["scenes"][0]["layers"][0]["overrides"]["title"];
        assert!(ov["color"]["keys"].is_array(), "{ov}");
        // New params regenerate the children; the override stays.
        let b = apply(&a.project, &[json!({"op": "set", "target": "card.params.title", "value": "Natanz"})]).unwrap();
        let c = crate::compile::Compiled::new(&b.project);
        assert!(c.errors.is_empty(), "{:?}", c.errors);
        let v = serde_json::to_value(&b.project).unwrap();
        assert!(v["scenes"][0]["layers"][0]["overrides"]["title"]["color"]["keys"].is_array());
        let expanded = expand_project(&b.project, &mut Vec::new()).unwrap();
        let comp = &expanded.scenes[0].layers[0];
        let title = comp.children().iter().find(|l| l.id == "card/title").unwrap();
        let tv = serde_json::to_value(title).unwrap();
        assert_eq!(tv["text"], "Natanz");
        assert!(tv["color"]["keys"].is_array(), "the override is applied on top: {tv}");
        // Removing a child, then undoing it.
        let r = apply(&b.project, &[json!({"op": "remove", "id": "card/tape-l"})]).unwrap();
        let ex = expand_project(&r.project, &mut Vec::new()).unwrap();
        assert!(!ex.scenes[0].layers[0].children().iter().any(|l| l.id == "card/tape-l"));
        let back = apply(&r.project, &r.inverse).unwrap().project;
        assert_eq!(serde_json::to_value(&back).unwrap(), serde_json::to_value(&b.project).unwrap());
        // Detach: plain layers, overrides baked in.
        let d = apply(&b.project, &[json!({"op": "detach", "id": "card"})]).unwrap();
        let v = serde_json::to_value(&d.project).unwrap();
        let layer = &v["scenes"][0]["layers"][0];
        assert_eq!(layer["kind"], "comp");
        let kids: Vec<&str> = layer["layers"].as_array().unwrap().iter().map(|l| l["id"].as_str().unwrap()).collect();
        assert!(kids.contains(&"card-title") && kids.contains(&"card-paper"), "{kids:?}");
        let title = layer["layers"].as_array().unwrap().iter().find(|l| l["id"] == "card-title").unwrap();
        assert_eq!(title["parent"], "card-paper", "references follow the new ids");
        assert!(title["color"]["keys"].is_array());
        let c = crate::compile::Compiled::new(&d.project);
        assert!(c.errors.is_empty(), "{:?}", c.errors);
        let undone = apply(&d.project, &d.inverse).unwrap().project;
        assert_eq!(serde_json::to_value(&undone).unwrap(), serde_json::to_value(&b.project).unwrap());
    }

    #[test]
    fn captions_chunk_and_light_up_the_spoken_word() {
        let mut p = project();
        p = crate::edit::apply(&p, &[json!({"op": "add", "scene": "s1", "id": "caps", "kind": "component", "component": "captions", "params": {"max_chars": 14}})]).unwrap().project;
        let c = crate::compile::Compiled::new(&p);
        assert!(c.errors.is_empty(), "{:?}", c.errors);
        let ex = expand_project(&p, &mut Vec::new()).unwrap();
        let texts: Vec<String> = ex.scenes[0].layers[0].children().iter().map(|l| serde_json::to_value(l).unwrap()["text"].as_str().unwrap_or("").to_string()).collect();
        // Sentence ends and the length limit chunk the words.
        assert_eq!(texts, vec!["One bomb.", "Over Iran.", "It hit 81", "cities."]);
        // At "Iran" (1.7 s) its glyphs take the highlight colour.
        let frame = c.frame(1.75);
        // A plain comp's children are flattened into the frame.
        let cap = frame.layers.iter().find(|l| l.id.as_ref() == "caps/c2");
        let crate::compile::Content::Text(t) = &cap.expect("chunk 2 is on").content else { panic!() };
        let lit: Vec<bool> = t.glyphs.iter().map(|g| g.color.is_some_and(|c| (c.0[2] - 0.0).abs() < 0.05)).collect();
        assert!(lit[5..9].iter().all(|x| *x), "'Iran' is yellow: {lit:?}");
        assert!(!lit[0], "'Over' is not");
    }
}
