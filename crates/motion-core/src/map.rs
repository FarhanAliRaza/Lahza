//! The `map` component: Natural Earth 1:50m (public domain, bundled as
//! `data/ne50m.bin`, see tools/map/build_map_data.py) in Web Mercator.
//!
//! World coordinates are "zoom 0" pixels: the whole world is `size[0]` px
//! wide. The camera is two layers: `zoom` scales about the view's centre
//! (in log space, so zooms feel even) and `pan` moves the world under it.
//! Countries are paths in world coordinates, parented to `pan`; they are
//! clipped to the union of everything the camera shows and simplified to
//! half a pixel at the deepest zoom. Border widths and labels are
//! counter-scaled with the zoom, so they keep their size on screen.

use serde_json::{json, Value};
use std::sync::OnceLock;

pub struct Country {
    pub code: String,
    pub name: String,
    pub label: (f64, f64),
    pub rings: Vec<Vec<(f64, f64)>>,
}

pub struct Place {
    pub name: String,
    pub country: String,
    pub lon: f64,
    pub lat: f64,
    pub rank: u8,
}

pub struct MapData {
    pub countries: Vec<Country>,
    pub lakes: Vec<Vec<Vec<(f64, f64)>>>,
    pub places: Vec<Place>,
}

static DATA: OnceLock<MapData> = OnceLock::new();

pub fn data() -> &'static MapData {
    DATA.get_or_init(|| parse(include_bytes!("../data/ne50m.bin")).expect("bundled map data"))
}

fn parse(b: &[u8]) -> Option<MapData> {
    struct R<'a>(&'a [u8], usize);
    impl R<'_> {
        fn take(&mut self, n: usize) -> Option<&[u8]> {
            let s = self.0.get(self.1..self.1 + n)?;
            self.1 += n;
            Some(s)
        }
        fn u32(&mut self) -> Option<u32> {
            Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
        }
        fn i32(&mut self) -> Option<i32> {
            Some(i32::from_le_bytes(self.take(4)?.try_into().ok()?))
        }
        fn i16(&mut self) -> Option<i16> {
            Some(i16::from_le_bytes(self.take(2)?.try_into().ok()?))
        }
        fn f32(&mut self) -> Option<f32> {
            Some(f32::from_le_bytes(self.take(4)?.try_into().ok()?))
        }
        fn str(&mut self) -> Option<String> {
            let n = u16::from_le_bytes(self.take(2)?.try_into().ok()?) as usize;
            Some(String::from_utf8_lossy(self.take(n)?).into_owned())
        }
        fn rings(&mut self) -> Option<Vec<Vec<(f64, f64)>>> {
            let n = self.u32()?;
            let mut out = Vec::with_capacity(n as usize);
            for _ in 0..n {
                let m = self.u32()?;
                let (mut x, mut y) = (self.i32()?, self.i32()?);
                let mut ring = Vec::with_capacity(m as usize);
                ring.push((x as f64 / 1000.0, y as f64 / 1000.0));
                for _ in 1..m {
                    let dx = self.i16()?;
                    let dy = self.i16()?;
                    if dx == i16::MIN {
                        x = self.i32()?;
                        y = self.i32()?;
                    } else {
                        x += dx as i32;
                        y += dy as i32;
                    }
                    ring.push((x as f64 / 1000.0, y as f64 / 1000.0));
                }
                out.push(ring);
            }
            Some(out)
        }
    }
    let mut r = R(b, 0);
    if r.take(4)? != b"LZNE" || r.u32()? != 1 {
        return None;
    }
    let mut countries = Vec::new();
    for _ in 0..r.u32()? {
        let code = r.str()?;
        let name = r.str()?;
        let label = (r.f32()? as f64, r.f32()? as f64);
        let rings = r.rings()?;
        // Dependencies can share their country's code (Australia's
        // territories are AUS too): one country, named and labelled after
        // its largest part, so every child id stays unique.
        let points = |rs: &Vec<Vec<(f64, f64)>>| rs.iter().map(Vec::len).sum::<usize>();
        match countries.iter_mut().find(|c: &&mut Country| c.code == code) {
            Some(c) => {
                if points(&rings) > points(&c.rings) {
                    c.name = name;
                    c.label = label;
                }
                c.rings.extend(rings);
            }
            None => countries.push(Country { code, name, label, rings }),
        }
    }
    let mut lakes = Vec::new();
    for _ in 0..r.u32()? {
        let _name = r.str()?;
        lakes.push(r.rings()?);
    }
    let mut places = Vec::new();
    for _ in 0..r.u32()? {
        let name = r.str()?;
        let country = r.str()?;
        let (lon, lat) = (r.f32()? as f64, r.f32()? as f64);
        let rank = r.take(1)?[0];
        places.push(Place { name, country, lon, lat, rank });
    }
    Some(MapData { countries, lakes, places })
}

/// Web Mercator in world px (the world is `w` px wide).
pub fn project(lon: f64, lat: f64, w: f64) -> (f64, f64) {
    let lat = lat.clamp(-85.05, 85.05).to_radians();
    let x = (lon + 180.0) / 360.0 * w;
    let y = (1.0 - (lat.tan() + 1.0 / lat.cos()).ln() / std::f64::consts::PI) / 2.0 * w;
    (x, y)
}

/// A ring clipped to a rectangle (Sutherland–Hodgman).
fn clip(ring: &[(f64, f64)], r: [f64; 4]) -> Vec<(f64, f64)> {
    let mut pts = ring.to_vec();
    for edge in 0..4 {
        if pts.is_empty() {
            break;
        }
        let inside = |p: (f64, f64)| match edge {
            0 => p.0 >= r[0],
            1 => p.0 <= r[2],
            2 => p.1 >= r[1],
            _ => p.1 <= r[3],
        };
        let cross = |a: (f64, f64), b: (f64, f64)| -> (f64, f64) {
            let t = match edge {
                0 => (r[0] - a.0) / (b.0 - a.0),
                1 => (r[2] - a.0) / (b.0 - a.0),
                2 => (r[1] - a.1) / (b.1 - a.1),
                _ => (r[3] - a.1) / (b.1 - a.1),
            };
            (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t)
        };
        let mut out = Vec::with_capacity(pts.len());
        for i in 0..pts.len() {
            let (a, b) = (pts[(i + pts.len() - 1) % pts.len()], pts[i]);
            match (inside(a), inside(b)) {
                (true, true) => out.push(b),
                (true, false) => out.push(cross(a, b)),
                (false, true) => {
                    out.push(cross(a, b));
                    out.push(b);
                }
                _ => {}
            }
        }
        pts = out;
    }
    pts
}

/// Douglas–Peucker.
fn simplify(pts: &[(f64, f64)], eps: f64) -> Vec<(f64, f64)> {
    if pts.len() < 4 {
        return pts.to_vec();
    }
    let mut keep = vec![false; pts.len()];
    keep[0] = true;
    keep[pts.len() - 1] = true;
    let mut stack = vec![(0, pts.len() - 1)];
    while let Some((a, b)) = stack.pop() {
        let (p, q) = (pts[a], pts[b]);
        let (dx, dy) = (q.0 - p.0, q.1 - p.1);
        let len = (dx * dx + dy * dy).sqrt();
        let (mut best, mut at) = (0.0, 0);
        for (i, r) in pts.iter().enumerate().take(b).skip(a + 1) {
            // A closed ring starts and ends on the same point: distance
            // from that point.
            let d = if len < 1e-12 { ((r.0 - p.0).powi(2) + (r.1 - p.1).powi(2)).sqrt() } else { ((r.0 - p.0) * dy - (r.1 - p.1) * dx).abs() / len };
            if d > best {
                best = d;
                at = i;
            }
        }
        if best > eps {
            keep[at] = true;
            stack.push((a, at));
            stack.push((at, b));
        }
    }
    pts.iter().zip(keep).filter(|(_, k)| *k).map(|(p, _)| *p).collect()
}

fn path_d(rings: &[Vec<(f64, f64)>]) -> String {
    let mut d = String::new();
    for r in rings {
        for (i, p) in r.iter().enumerate() {
            d.push_str(&format!("{}{:.3} {:.3} ", if i == 0 { "M" } else { "L" }, p.0, p.1));
        }
        d.push_str("Z ");
    }
    d
}

/// Segments of a bow; a great circle is sampled twice as finely.
const ROUTE_SEGMENTS: usize = 64;
/// Bows and routes keep this share of the world's width from its edges.
const ROUTE_MARGIN: f64 = 0.02;

/// A screen-space bow from `p0` to `p1` as they sit on the flat map (no
/// wrapping): a parabola `lift` × the distance high at its middle,
/// toward the top of the map for a positive `lift` and the bottom for a
/// negative one (for a vertical route: left of the direction of travel),
/// lowered where it would leave the map.
fn bow(p0: (f64, f64), p1: (f64, f64), lift: f64, world: f64) -> Vec<(f64, f64)> {
    let (dx, dy) = (p1.0 - p0.0, p1.1 - p0.1);
    let len = (dx * dx + dy * dy).sqrt();
    let chord = |t: f64| (p0.0 + dx * t, p0.1 + dy * t);
    if len < 1e-9 {
        return vec![p0, p1];
    }
    // The normal left of travel, turned to point up (−y) unless vertical,
    // and down for a negative lift.
    let mut n = (dy / len, -dx / len);
    if n.1 > 1e-9 {
        n = (-n.0, -n.1);
    }
    if lift < 0.0 {
        n = (-n.0, -n.1);
    }
    let shape = |t: f64| 4.0 * t * (1.0 - t);
    // The highest bow that stays inside the map's margins.
    let (lo, hi) = (world * ROUTE_MARGIN, world * (1.0 - ROUTE_MARGIN));
    let mut h = lift.abs() * len;
    for k in 1..ROUTE_SEGMENTS {
        let t = k as f64 / ROUTE_SEGMENTS as f64;
        let (c, s) = (chord(t), shape(t));
        for (c, o) in [(c.0, n.0 * s), (c.1, n.1 * s)] {
            if o > 1e-12 {
                h = h.min((hi - c) / o);
            } else if o < -1e-12 {
                h = h.min((lo - c) / o);
            }
        }
    }
    let h = h.max(0.0);
    (0..=ROUTE_SEGMENTS)
        .map(|k| {
            let t = k as f64 / ROUTE_SEGMENTS as f64;
            let (c, s) = (chord(t), shape(t) * h);
            (c.0 + n.0 * s, c.1 + n.1 * s)
        })
        .collect()
}

/// The great circle from `a` to `b` ([lon, lat]) on the flat map, as
/// subpaths: split where it crosses the date line (it leaves one edge and
/// comes back in at the other) and cut where it runs past the map's top
/// or bottom (Mercator's ±85.05°) near a pole, never a segment across the
/// map.
fn great_circle(a: (f64, f64), b: (f64, f64), world: f64) -> Vec<Vec<(f64, f64)>> {
    const LAT: f64 = 85.05;
    let to3 = |(lon, lat): (f64, f64)| {
        let (l, p) = (lon.to_radians(), lat.to_radians());
        [p.cos() * l.cos(), p.cos() * l.sin(), p.sin()]
    };
    let (va, vb) = (to3(a), to3(b));
    let omega = (va[0] * vb[0] + va[1] * vb[1] + va[2] * vb[2]).clamp(-1.0, 1.0).acos();
    let n = ROUTE_SEGMENTS * 2;
    let samples: Vec<(f64, f64)> = (0..=n)
        .map(|k| {
            let t = k as f64 / n as f64;
            let (sa, sb) = if omega < 1e-9 { (1.0 - t, t) } else { (((1.0 - t) * omega).sin() / omega.sin(), (t * omega).sin() / omega.sin()) };
            let v = [sa * va[0] + sb * vb[0], sa * va[1] + sb * vb[1], sa * va[2] + sb * vb[2]];
            (v[1].atan2(v[0]).to_degrees(), v[2].atan2((v[0] * v[0] + v[1] * v[1]).sqrt()).to_degrees())
        })
        .collect();
    let on = |lat: f64| lat.abs() <= LAT;
    let mut lines: Vec<Vec<(f64, f64)>> = Vec::new();
    let mut cur: Vec<(f64, f64)> = Vec::new();
    if on(samples[0].1) {
        cur.push(project(samples[0].0, samples[0].1, world));
    }
    for w in samples.windows(2) {
        let (p, mut q) = (w[0], w[1]);
        // Longitudes unwrapped: q may run past ±180 (the date line).
        if q.0 - p.0 > 180.0 {
            q.0 -= 360.0;
        } else if p.0 - q.0 > 180.0 {
            q.0 += 360.0;
        }
        let (p_on, q_on) = (on(p.1), on(q.1));
        if !p_on && !q_on {
            continue;
        }
        let at = |f: f64| (p.0 + (q.0 - p.0) * f, p.1 + (q.1 - p.1) * f);
        // The part of the step on the map: it leaves or comes back at the
        // top or bottom edge.
        let s = if p_on { p } else { at((LAT.copysign(p.1) - p.1) / (q.1 - p.1)) };
        let e = if q_on { q } else { at((LAT.copysign(q.1) - p.1) / (q.1 - p.1)) };
        if !p_on {
            cur.push(project(wrap(s.0), s.1, world));
        }
        if e.0.abs() > 180.0 && s.0.abs() <= 180.0 {
            // Across the date line: out at one edge, in at the other.
            let edge = 180f64.copysign(e.0);
            let lat = s.1 + (e.1 - s.1) * (edge - s.0) / (e.0 - s.0);
            cur.push(project(edge, lat, world));
            lines.push(std::mem::take(&mut cur));
            cur.push(project(-edge, lat, world));
        }
        cur.push(project(wrap(e.0), e.1, world));
        if !q_on {
            lines.push(std::mem::take(&mut cur));
        }
    }
    lines.push(cur);
    lines.retain(|l| l.len() >= 2);
    lines
}

/// A longitude in [−180, 180].
fn wrap(lon: f64) -> f64 {
    (lon + 180.0).rem_euclid(360.0) - 180.0
}

struct CamKey {
    at: Value,
    center: (f64, f64),
    zoom: f64,
    ease: Value,
    dur: Value,
    anchor: Value,
}

fn lonlat(v: &Value) -> Option<(f64, f64)> {
    let a = v.as_array()?;
    Some((a.first()?.as_f64()?, a.get(1)?.as_f64()?))
}

fn slug(name: &str) -> String {
    name.chars().filter_map(|c| if c.is_ascii_alphanumeric() { Some(c.to_ascii_lowercase()) } else if c == ' ' || c == '-' { Some('-') } else { None }).collect()
}

/// A place by name (case-insensitive), or [lon, lat].
pub fn locate(v: &Value) -> Result<(f64, f64), String> {
    if let Some(p) = lonlat(v) {
        return Ok(p);
    }
    let name = v.as_str().or_else(|| v.get("name").and_then(Value::as_str)).ok_or("a place is a name or [lon, lat]")?;
    if let (Some(lon), Some(lat)) = (v.get("lon").and_then(Value::as_f64), v.get("lat").and_then(Value::as_f64)) {
        return Ok((lon, lat));
    }
    let key = name.to_lowercase();
    data()
        .places
        .iter()
        .find(|p| p.name.to_lowercase() == key)
        .map(|p| (p.lon, p.lat))
        .or_else(|| data().countries.iter().find(|c| c.name.to_lowercase() == key || c.code.eq_ignore_ascii_case(name)).map(|c| c.label))
        .ok_or_else(|| format!("no place or country named '{name}' in Natural Earth 1:50m: give [lon, lat] or {{\"name\", \"lon\", \"lat\"}}"))
}

pub fn generate(p: &Value, width: f64, height: f64) -> Result<Vec<Value>, String> {
    let get = |k: &str| p.get(k).filter(|v| !v.is_null());
    let (w, h) = get("size").and_then(lonlat).unwrap_or((width, height));
    let world = w;
    // Camera keys.
    let cam = get("camera").cloned().unwrap_or_else(|| json!({"center": [0, 20], "zoom": 0}));
    let raw: Vec<Value> = match cam.get("keys").and_then(Value::as_array) {
        Some(k) => k.clone(),
        None => vec![cam.clone()],
    };
    let mut keys = Vec::new();
    for k in &raw {
        let center = match k.get("center") {
            Some(c) => locate(c)?,
            None => keys.last().map(|k: &CamKey| k.center).unwrap_or((0.0, 20.0)),
        };
        keys.push(CamKey {
            at: k.get("at").cloned().unwrap_or(json!(0)),
            center,
            zoom: k.get("zoom").and_then(Value::as_f64).or_else(|| keys.last().map(|k| k.zoom)).unwrap_or(0.0),
            ease: k.get("ease").cloned().unwrap_or(json!("zoom")),
            dur: k.get("dur").cloned().unwrap_or(json!("1s")),
            anchor: k.get("anchor").cloned().unwrap_or(json!("end")),
        });
    }
    let animated = cam.get("keys").is_some();
    let scales: Vec<f64> = keys.iter().map(|k| 2f64.powf(k.zoom)).collect();
    let smax = scales.iter().cloned().fold(1.0, f64::max);
    // Everything the camera shows, with a margin.
    let mut view = [f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY];
    for (k, s) in keys.iter().zip(&scales) {
        let c = project(k.center.0, k.center.1, world);
        let (hw, hh) = (w / 2.0 / s * 1.25, h / 2.0 / s * 1.25);
        view = [view[0].min(c.0 - hw), view[1].min(c.1 - hh), view[2].max(c.0 + hw), view[3].max(c.1 + hh)];
    }
    let eps = get("detail").and_then(Value::as_f64).unwrap_or(0.35) / smax;
    // A value that keeps its size on screen: `px` at every camera key.
    // `pair`: as [v, v] (a scale) rather than a number (a width).
    let screen = |px: f64, pair: bool| -> Value {
        if !animated {
            let v = px / scales[0];
            return if pair { json!([v, v]) } else { json!(v) };
        }
        let ks: Vec<Value> = keys
            .iter()
            .zip(&scales)
            .map(|(k, s)| {
                let mut key = json!({"at": k.at, "v": if pair { json!([px / s, px / s]) } else { json!(px / s) }, "ease": k.ease, "dur": k.dur, "anchor": k.anchor, "space": "log"});
                if px / s <= 0.0 {
                    key.as_object_mut().unwrap().remove("space");
                }
                key
            })
            .collect();
        json!({"keys": ks})
    };
    let land = get("land").cloned().unwrap_or(json!("#e4ddcc"));
    let water = get("water").cloned().unwrap_or(json!("#b9cfd9"));
    let border = get("border").cloned().unwrap_or(json!({}));
    let border_color = border.get("color").cloned().unwrap_or(json!("#8c8270"));
    let border_px = border.get("width").and_then(Value::as_f64).unwrap_or(1.5);
    let fills = get("fills").and_then(Value::as_object).cloned().unwrap_or_default();
    let mut out = vec![json!({"id": "water", "kind": "shape", "shape": "rect", "size": [w, h], "fill": water, "transform": {"position": [w / 2.0, h / 2.0]}})];
    // The camera.
    let (zoom_t, pan_t) = if animated {
        let zk: Vec<Value> = keys.iter().zip(&scales).map(|(k, s)| json!({"at": k.at, "v": [s, s], "ease": k.ease, "dur": k.dur, "anchor": k.anchor, "space": "log"})).collect();
        let pk: Vec<Value> = keys
            .iter()
            .map(|k| {
                let c = project(k.center.0, k.center.1, world);
                json!({"at": k.at, "v": [-c.0, -c.1], "ease": k.ease, "dur": k.dur, "anchor": k.anchor})
            })
            .collect();
        (json!({"position": [w / 2.0, h / 2.0], "scale": {"keys": zk}}), json!({"position": {"keys": pk}}))
    } else {
        let c = project(keys[0].center.0, keys[0].center.1, world);
        (json!({"position": [w / 2.0, h / 2.0], "scale": [scales[0], scales[0]]}), json!({"position": [-c.0, -c.1]}))
    };
    out.push(json!({"id": "zoom", "kind": "shape", "shape": "rect", "size": [0, 0], "transform": zoom_t}));
    out.push(json!({"id": "pan", "kind": "shape", "shape": "rect", "size": [0, 0], "parent": "zoom", "transform": pan_t}));
    let shape = |rings: &[Vec<(f64, f64)>]| -> Vec<Vec<(f64, f64)>> {
        rings
            .iter()
            .filter_map(|r| {
                let projected: Vec<(f64, f64)> = r.iter().map(|&(lon, lat)| project(lon, lat, world)).collect();
                let (mut x0, mut y0, mut x1, mut y1) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
                for p in &projected {
                    (x0, y0, x1, y1) = (x0.min(p.0), y0.min(p.1), x1.max(p.0), y1.max(p.1));
                }
                // Off view, or smaller than the detail at the deepest zoom.
                if x1 < view[0] || x0 > view[2] || y1 < view[1] || y0 > view[3] || (x1 - x0).max(y1 - y0) < eps * 2.0 {
                    return None;
                }
                let clipped = if x0 < view[0] || y0 < view[1] || x1 > view[2] || y1 > view[3] { clip(&projected, view) } else { projected };
                let s = simplify(&clipped, eps);
                (s.len() >= 3).then_some(s)
            })
            .collect()
    };
    let stroke_width = screen(border_px, false);
    for c in &data().countries {
        let rings = shape(&c.rings);
        if rings.is_empty() {
            continue;
        }
        let fill = fills.get(&c.code).cloned().unwrap_or_else(|| land.clone());
        out.push(json!({"id": c.code, "name": c.name, "kind": "shape", "shape": "path", "d": path_d(&rings), "fill": fill,
            "stroke": {"color": border_color, "width": stroke_width}, "parent": "pan"}));
    }
    if get("lakes").and_then(Value::as_bool).unwrap_or(true) {
        let rings: Vec<Vec<(f64, f64)>> = data().lakes.iter().flat_map(|l| shape(l)).collect();
        if !rings.is_empty() {
            out.push(json!({"id": "lakes", "kind": "shape", "shape": "path", "d": path_d(&rings), "fill": water, "parent": "pan"}));
        }
    }
    // Labels and places keep their size on screen.
    let label = get("label").cloned().unwrap_or(json!({}));
    let lsize = label.get("size").and_then(Value::as_f64).unwrap_or(h / 48.0);
    let lcolor = label.get("color").cloned().unwrap_or(json!("#3a352c"));
    let lfont = label.get("font").and_then(Value::as_str).unwrap_or("Inter SemiBold");
    let upper = label.get("uppercase").and_then(Value::as_bool).unwrap_or(true);
    let counter = screen(1.0, true);
    for code in get("labels").and_then(Value::as_array).cloned().unwrap_or_default() {
        let code = code.as_str().unwrap_or("");
        let Some(c) = data().countries.iter().find(|c| c.code == code) else {
            return Err(format!("labels: no country '{code}' (ISO A3 codes, e.g. IRN)"));
        };
        let at = project(c.label.0, c.label.1, world);
        let text = if upper { c.name.to_uppercase() } else { c.name.clone() };
        out.push(json!({"id": format!("{code}-label"), "kind": "text", "text": text, "font": lfont, "size": lsize, "color": lcolor, "align": "center", "tracking": 80,
            "parent": "pan", "transform": {"position": [at.0, at.1], "scale": counter}}));
    }
    for (i, place) in get("places").and_then(Value::as_array).cloned().unwrap_or_default().iter().enumerate() {
        let (lon, lat) = locate(place)?;
        let name = place.as_str().or_else(|| place.get("name").and_then(Value::as_str)).map(String::from).unwrap_or_else(|| format!("place{}", i + 1));
        let id = format!("p-{}", slug(&name));
        let at = project(lon, lat, world);
        let dot = place.get("size").and_then(Value::as_f64).unwrap_or(lsize * 0.5);
        let color = place.get("color").cloned().unwrap_or(json!("#c8102e"));
        let mut d = json!({"id": id, "kind": "shape", "shape": "circle", "size": dot, "fill": color, "stroke": {"color": "#ffffff", "width": dot * 0.18},
            "parent": "pan", "transform": {"position": [at.0, at.1], "scale": counter}});
        // A place with `at` appears then: its dot and its label (a parent
        // passes on its transform, not its opacity).
        let entrance = place.get("at").map(|t| json!({"keys": [{"at": t, "from": 0, "v": 1, "ease": "pop", "dur": "6f", "anchor": "start"}]}));
        if let Some(e) = &entrance {
            d["transform"]["opacity"] = e.clone();
        }
        out.push(d);
        if place.get("label").and_then(Value::as_bool).unwrap_or(true) {
            let mut label = json!({"id": format!("{id}-label"), "kind": "text", "text": name, "font": lfont, "size": lsize * 0.9, "color": lcolor, "align": "left",
                "parent": id, "transform": {"position": [dot * 0.9, 0]}});
            if let Some(e) = entrance {
                label["transform"]["opacity"] = e;
            }
            out.push(label);
        }
    }
    for (i, arc) in get("arcs").and_then(Value::as_array).cloned().unwrap_or_default().iter().enumerate() {
        let a = locate(arc.get("from").ok_or("an arc needs \"from\"")?)?;
        let b = locate(arc.get("to").ok_or("an arc needs \"to\"")?)?;
        let id = arc.get("id").and_then(Value::as_str).map(String::from).unwrap_or_else(|| format!("arc{}", i + 1));
        let lift = arc.get("lift").and_then(Value::as_f64).unwrap_or(0.2);
        let lines = match arc.get("route").and_then(Value::as_str).unwrap_or("bow") {
            "bow" => vec![bow(project(a.0, a.1, world), project(b.0, b.1, world), lift, world)],
            "great_circle" => great_circle(a, b, world),
            other => return Err(format!("arc '{id}': route '{other}' is \"bow\" or \"great_circle\"")),
        };
        // Open polylines, origin to destination: `trim` draws across the
        // subpaths in order, so the draw runs on through a split.
        let d: String = lines.iter().flat_map(|l| l.iter().enumerate().map(|(k, p)| format!("{}{:.3} {:.3} ", if k == 0 { "M" } else { "L" }, p.0, p.1))).collect();
        let at = arc.get("at").cloned().unwrap_or(json!(0));
        let dur = arc.get("dur").cloned().unwrap_or(json!("1s"));
        out.push(json!({"id": id, "kind": "shape", "shape": "path", "d": d.trim(), "parent": "pan",
            "stroke": {"color": arc.get("color").cloned().unwrap_or(json!("#c8102e")), "width": screen(arc.get("width").and_then(Value::as_f64).unwrap_or(5.0), false), "cap": "round"},
            "trim": {"keys": [{"at": at, "from": [0, 0], "v": [0, 1], "ease": "glide", "dur": dur, "anchor": "start"}]}}));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_loads_and_finds_places() {
        let d = data();
        assert!(d.countries.len() > 200);
        let mut codes: Vec<&str> = d.countries.iter().map(|c| c.code.as_str()).collect();
        codes.sort();
        let n = codes.len();
        codes.dedup();
        assert_eq!(codes.len(), n, "country codes are unique (child ids)");
        assert_eq!(d.countries.iter().find(|c| c.code == "AUS").unwrap().name, "Australia");
        let iran = d.countries.iter().find(|c| c.code == "IRN").unwrap();
        assert_eq!(iran.name, "Iran");
        assert!(iran.rings.iter().map(Vec::len).sum::<usize>() > 300);
        let tehran = locate(&json!("Tehran")).unwrap();
        assert!((tehran.0 - 51.4).abs() < 0.5 && (tehran.1 - 35.7).abs() < 0.5, "{tehran:?}");
        assert!(locate(&json!("Nowhere-ville")).is_err());
        let (x, y) = project(0.0, 0.0, 1000.0);
        assert!((x - 500.0).abs() < 1e-9 && (y - 500.0).abs() < 1e-9);
    }

    #[test]
    fn closed_rings_survive_simplification() {
        let ring: Vec<(f64, f64)> = (0..=40).map(|i| { let a = i as f64 / 40.0 * std::f64::consts::TAU; (a.cos() * 10.0, a.sin() * 10.0) }).collect();
        assert!(simplify(&ring, 0.1).len() > 10);
        let out = generate(&json!({"camera": {"center": [52, 28], "zoom": 2.2}}), 1080.0, 1920.0).unwrap();
        assert!(out.iter().any(|c| c["id"] == "IRN"), "Iran is in view");
    }

    /// An arc's subpaths, read back from its path data.
    fn arc_lines(route: &str, world: f64) -> Vec<Vec<(f64, f64)>> {
        let out = generate(&json!({"arcs": [{"from": "Karachi", "to": "San Francisco", "route": route}]}), world, world).unwrap();
        let d = out.iter().find(|c| c["id"] == "arc1").unwrap()["d"].as_str().unwrap().to_string();
        let mut lines: Vec<Vec<(f64, f64)>> = Vec::new();
        for pair in d.split_whitespace().collect::<Vec<_>>().chunks(2) {
            let (cmd, x) = pair[0].split_at(1);
            if cmd == "M" {
                lines.push(Vec::new());
            }
            lines.last_mut().unwrap().push((x.parse().unwrap(), pair[1].parse().unwrap()));
        }
        lines
    }

    #[test]
    fn pacific_arcs_never_jump_across_the_map() {
        let world = 1000.0;
        let (ka, sf) = (project(67.01, 24.86, world), project(-122.42, 37.77, world));
        for route in ["bow", "great_circle"] {
            let lines = arc_lines(route, world);
            for l in &lines {
                for w in l.windows(2) {
                    let len = ((w[1].0 - w[0].0).powi(2) + (w[1].1 - w[0].1).powi(2)).sqrt();
                    assert!(len < world * 0.5, "{route}: a segment {len:.0} px long, {w:?}");
                }
            }
            let (first, last) = (lines[0][0], *lines.last().unwrap().last().unwrap());
            assert!((first.0 - ka.0).abs() < 1.0 && (first.1 - ka.1).abs() < 1.0, "{route} starts at Karachi: {first:?}");
            assert!((last.0 - sf.0).abs() < 1.0 && (last.1 - sf.1).abs() < 1.0, "{route} ends at San Francisco: {last:?}");
        }
        let bow = arc_lines("bow", world);
        assert_eq!(bow.len(), 1);
        assert!(bow[0].iter().all(|p| (0.0..=world).contains(&p.0) && (0.0..=world).contains(&p.1)), "the bow stays on the map");
        // Arched up toward the top of the map, `lift` (0.2) × the distance high.
        let mid = bow[0][ROUTE_SEGMENTS / 2];
        let chord = ((ka.0 + sf.0) / 2.0, (ka.1 + sf.1) / 2.0);
        let len = ((sf.0 - ka.0).powi(2) + (sf.1 - ka.1).powi(2)).sqrt();
        assert!(mid.1 < chord.1 && (((mid.0 - chord.0).powi(2) + (mid.1 - chord.1).powi(2)).sqrt() - 0.2 * len).abs() < 1.0, "{mid:?} vs {chord:?}");
        // The true route leaves at the date line and comes back at the other edge.
        let gc = arc_lines("great_circle", world);
        assert_eq!(gc.len(), 2, "{gc:?}");
        let (out, back) = (*gc[0].last().unwrap(), gc[1][0]);
        assert!((out.0 - world).abs() < 1e-6 && back.0.abs() < 1e-6 && (out.1 - back.1).abs() < 1e-6, "{out:?} {back:?}");
        assert!(gc.iter().flatten().all(|p| p.1 > 0.0), "below the top edge (peaks near 82°N)");
    }

    #[test]
    fn bows_stay_on_the_map_and_routes_stop_at_its_edges() {
        let world = 1000.0;
        // A big lift on a long route is lowered to keep inside the map.
        let lines = bow(project(-150.0, 60.0, world), project(150.0, 60.0, world), 1.0, world);
        let top = lines.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
        assert!((top - world * ROUTE_MARGIN).abs() < 1.0, "{top}");
        // Negative lift arches down.
        let down = bow(project(0.0, 0.0, world), project(40.0, 0.0, world), -0.3, world);
        assert!(down[ROUTE_SEGMENTS / 2].1 > project(0.0, 0.0, world).1);
        // Over the pole: leaves at the top edge and comes back down from it.
        let polar = great_circle((10.0, 70.0), (-170.0, 70.0), world);
        assert_eq!(polar.len(), 2, "{polar:?}");
        assert!(polar[0].last().unwrap().1.abs() < 0.5 && polar[1][0].1.abs() < 0.5, "{:?} {:?}", polar[0].last(), polar[1][0]);
        assert!(generate(&json!({"arcs": [{"from": "Karachi", "to": "Tokyo", "route": "rhumb"}]}), world, world).is_err());
    }

    #[test]
    fn clipping_keeps_what_is_in_view() {
        let square = vec![(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
        let c = clip(&square, [5.0, -1.0, 20.0, 20.0]);
        let xs: Vec<f64> = c.iter().map(|p| p.0).collect();
        assert!(xs.iter().all(|x| *x >= 5.0) && xs.contains(&10.0));
    }
}
