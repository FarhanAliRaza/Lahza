//! Fonts, signed-distance glyph atlas and text layout.
//!
//! Glyphs are rendered once into an SDF atlas at `ATLAS_EM` pixels per em,
//! computed exactly from the flattened outline (distance to the nearest
//! edge, sign from the non-zero winding rule). The GPU then draws glyphs at
//! any size, scale and rotation from that atlas, so per-glyph animation
//! never re-rasterises text.

use lahza_motion_core::compile::{Affine, GlyphState, TextState};
use lahza_motion_core::document::Align;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const ATLAS_EM: f32 = 96.0;
/// Distance range stored either side of the edge, in atlas texels.
pub const SPREAD: f32 = 12.0;
pub const ATLAS_SIZE: u32 = 2048;

const INTER_REGULAR: &[u8] = include_bytes!("../../../assets/fonts/Inter-Regular.ttf");
const INTER_MEDIUM: &[u8] = include_bytes!("../../../assets/fonts/Inter-Medium.ttf");
const INTER_SEMIBOLD: &[u8] = include_bytes!("../../../assets/fonts/Inter-SemiBold.ttf");

pub const BUNDLED: &[&str] = &["Inter", "Inter Medium", "Inter SemiBold"];

struct Font {
    data: Arc<Vec<u8>>,
    units_per_em: f32,
    ascender: f32,
    cap_height: f32,
}

impl Font {
    fn face(&self) -> ttf_parser::Face<'_> {
        ttf_parser::Face::parse(&self.data, 0).expect("font parsed when loaded")
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AtlasGlyph {
    /// Atlas rectangle (texels), including the spread margin.
    pub rect: [u32; 4],
    /// Offset of the rect's top-left from the pen position, in atlas texels
    /// (y down, relative to the baseline).
    pub origin: [f32; 2],
    pub empty: bool,
}

pub struct Fonts {
    root: Option<PathBuf>,
    fonts: Vec<Font>,
    by_name: HashMap<String, usize>,
    glyphs: HashMap<(usize, u16), AtlasGlyph>,
    /// One byte per texel: 0.5 + d/(2·SPREAD), inside > 0.5.
    pub atlas: Vec<u8>,
    /// Bumped whenever atlas texels change (GPU re-upload).
    pub generation: u64,
    shelf: (u32, u32, u32),
    pub warnings: Vec<String>,
    layouts: HashMap<LayoutKey, Arc<Layout>>,
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct LayoutKey {
    text: Arc<str>,
    font: Arc<str>,
    size: u64,
    tracking: u64,
    line_height: u64,
    align: Align,
}

/// A laid-out text block, in layer space at font size, centred on the
/// origin (horizontally per `align`, vertically on the cap height).
pub struct Layout {
    pub glyphs: Vec<PlacedGlyph>,
    /// Block bounds [x0, y0, x1, y1].
    pub bounds: [f64; 4],
    /// Per char: unit pivot point (layer space).
    pub unit_centers: HashMap<u32, (f64, f64)>,
}

#[derive(Clone, Copy, Debug)]
pub struct PlacedGlyph {
    pub char_index: usize,
    pub atlas: AtlasGlyph,
    /// Pen position (baseline) in layer space.
    pub pen: [f64; 2],
    /// Layer units per atlas texel.
    pub scale: f64,
}

impl Fonts {
    pub fn new(root: Option<&Path>) -> Self {
        let mut fonts = Self {
            root: root.map(Path::to_path_buf),
            fonts: Vec::new(),
            by_name: HashMap::new(),
            glyphs: HashMap::new(),
            atlas: vec![0; (ATLAS_SIZE * ATLAS_SIZE) as usize],
            generation: 1,
            shelf: (0, 0, 0),
            warnings: Vec::new(),
            layouts: HashMap::new(),
        };
        for (name, data) in BUNDLED.iter().zip([INTER_REGULAR, INTER_MEDIUM, INTER_SEMIBOLD]) {
            let index = fonts.add(data.to_vec()).expect("bundled fonts parse");
            fonts.by_name.insert(name.to_string(), index);
        }
        fonts.by_name.insert("Inter Regular".into(), 0);
        fonts
    }

    fn add(&mut self, data: Vec<u8>) -> Option<usize> {
        let face = ttf_parser::Face::parse(&data, 0).ok()?;
        let units_per_em = face.units_per_em() as f32;
        let ascender = face.ascender() as f32;
        let cap_height = face.capital_height().map(|c| c as f32).unwrap_or(ascender * 0.7);
        drop(face);
        self.fonts.push(Font { data: Arc::new(data), units_per_em, ascender, cap_height });
        Some(self.fonts.len() - 1)
    }

    /// A bundled name, or a file relative to the project folder.
    pub fn resolve(&mut self, name: &str) -> usize {
        if let Some(i) = self.by_name.get(name) {
            return *i;
        }
        let loaded = self.root.as_ref().map(|r| r.join(name)).filter(|p| p.is_file()).and_then(|p| std::fs::read(p).ok());
        let index = match loaded.and_then(|data| self.add(data)) {
            Some(i) => i,
            None => {
                self.warnings.push(format!("font '{name}' not found; using Inter SemiBold"));
                2
            }
        };
        self.by_name.insert(name.to_string(), index);
        index
    }

    fn glyph(&mut self, font: usize, id: u16) -> AtlasGlyph {
        if let Some(g) = self.glyphs.get(&(font, id)) {
            return *g;
        }
        let g = self.rasterize(font, id);
        self.glyphs.insert((font, id), g);
        g
    }

    fn rasterize(&mut self, font: usize, id: u16) -> AtlasGlyph {
        let f = &self.fonts[font];
        let scale = ATLAS_EM / f.units_per_em;
        let mut outline = Outline { segments: Vec::new(), start: (0.0, 0.0), pen: (0.0, 0.0), scale };
        let face = f.face();
        let Some(bbox) = face.outline_glyph(ttf_parser::GlyphId(id), &mut outline) else {
            return AtlasGlyph { empty: true, ..Default::default() };
        };
        drop(face);
        // Glyph space (y up) → texel space (y down) with the spread margin.
        let x0 = (bbox.x_min as f32 * scale).floor() - SPREAD;
        let y1 = (bbox.y_max as f32 * scale).ceil() + SPREAD;
        let w = ((bbox.x_max as f32 * scale).ceil() + SPREAD - x0).max(1.0) as u32;
        let h = (y1 - ((bbox.y_min as f32 * scale).floor() - SPREAD)).max(1.0) as u32;
        let Some((ax, ay)) = self.allocate(w, h) else {
            // Atlas full: start over (glyphs are re-rasterised on demand).
            self.glyphs.clear();
            self.atlas.fill(0);
            self.shelf = (0, 0, 0);
            self.layouts.clear();
            return self.rasterize(font, id);
        };
        let segs: Vec<[f32; 4]> = outline
            .segments
            .iter()
            .map(|s| [s[0] - x0, y1 - s[1], s[2] - x0, y1 - s[3]])
            .collect();
        let mut winding = vec![0i32; w as usize];
        for ty in 0..h {
            let py = ty as f32 + 0.5;
            // Non-zero winding per texel from crossings of this row.
            let mut crossings: Vec<(f32, i32)> = Vec::new();
            for s in &segs {
                let (ya, yb) = (s[1], s[3]);
                if (ya <= py) != (yb <= py) {
                    let t = (py - ya) / (yb - ya);
                    crossings.push((s[0] + t * (s[2] - s[0]), if yb > ya { 1 } else { -1 }));
                }
            }
            crossings.sort_by(|a, b| a.0.total_cmp(&b.0));
            winding.fill(0);
            let mut acc = 0;
            let mut ci = 0;
            for (tx, wv) in winding.iter_mut().enumerate() {
                let px = tx as f32 + 0.5;
                while ci < crossings.len() && crossings[ci].0 < px {
                    acc += crossings[ci].1;
                    ci += 1;
                }
                *wv = acc;
            }
            let row_lo = py - SPREAD;
            let row_hi = py + SPREAD;
            let near: Vec<&[f32; 4]> = segs.iter().filter(|s| s[1].max(s[3]) >= row_lo && s[1].min(s[3]) <= row_hi).collect();
            for tx in 0..w {
                let px = tx as f32 + 0.5;
                let mut best = SPREAD * SPREAD;
                for s in &near {
                    if s[0].min(s[2]) > px + SPREAD || s[0].max(s[2]) < px - SPREAD {
                        continue;
                    }
                    best = best.min(segment_distance2(px, py, s));
                }
                let mut d = best.sqrt();
                if winding[tx as usize] != 0 {
                    d = -d;
                }
                let v = (0.5 - d / (2.0 * SPREAD)).clamp(0.0, 1.0);
                self.atlas[((ay + ty) * ATLAS_SIZE + ax + tx) as usize] = (v * 255.0).round() as u8;
            }
        }
        self.generation += 1;
        AtlasGlyph { rect: [ax, ay, ax + w, ay + h], origin: [x0, -y1], empty: false }
    }

    fn allocate(&mut self, w: u32, h: u32) -> Option<(u32, u32)> {
        let (mut x, mut y, mut row) = self.shelf;
        if x + w > ATLAS_SIZE {
            x = 0;
            y += row;
            row = 0;
        }
        if y + h > ATLAS_SIZE || w > ATLAS_SIZE {
            return None;
        }
        self.shelf = (x + w + 1, y, row.max(h + 1));
        Some((x, y))
    }

    /// Lays out a text block (cached by content and metrics).
    pub fn layout(&mut self, text: &TextState) -> Arc<Layout> {
        let key = LayoutKey {
            text: text.text.clone(),
            font: text.font.clone(),
            size: text.size.to_bits(),
            tracking: text.tracking.to_bits(),
            line_height: text.line_height.to_bits(),
            align: text.align,
        };
        if let Some(l) = self.layouts.get(&key) {
            return l.clone();
        }
        let font = self.resolve(&text.font);
        let (units, cap) = (self.fonts[font].units_per_em as f64, self.fonts[font].cap_height as f64);
        let px = text.size / units;
        let atlas_scale = text.size / ATLAS_EM as f64;
        let tracking = text.tracking / 1000.0 * text.size;
        let lines: Vec<&str> = text.text.split('\n').collect();
        let line_step = text.line_height * text.size;
        let mut glyphs = Vec::new();
        let mut char_index = 0usize;
        let mut x_min = f64::INFINITY;
        let mut x_max = f64::NEG_INFINITY;
        let mut boxes: HashMap<u32, [f64; 4]> = HashMap::new();
        let n = lines.len() as f64;
        for (li, line) in lines.iter().enumerate() {
            let baseline = cap * px / 2.0 + (li as f64 - (n - 1.0) / 2.0) * line_step;
            let ids: Vec<(char, u16, f64)> = {
                let face = self.fonts[font].face();
                line.chars()
                    .map(|c| {
                        let g = face.glyph_index(c).map(|g| g.0).unwrap_or(0);
                        let adv = face.glyph_hor_advance(ttf_parser::GlyphId(g)).unwrap_or(0) as f64 * px;
                        (c, g, adv)
                    })
                    .collect()
            };
            let count = ids.len();
            let width: f64 = ids.iter().map(|g| g.2).sum::<f64>() + tracking * count.saturating_sub(1) as f64;
            let mut pen = match text.align {
                Align::Left => 0.0,
                Align::Center => -width / 2.0,
                Align::Right => -width,
            };
            x_min = x_min.min(pen);
            x_max = x_max.max(pen + width);
            for (c, g, adv) in ids {
                let unit = text.pivots.get(char_index).copied().unwrap_or(u32::MAX);
                if unit != u32::MAX {
                    let b = boxes.entry(unit).or_insert([f64::INFINITY, baseline - cap * px, f64::NEG_INFINITY, baseline]);
                    b[0] = b[0].min(pen);
                    b[2] = b[2].max(pen + adv);
                }
                if !c.is_whitespace() {
                    let atlas = self.glyph(font, g);
                    if !atlas.empty {
                        glyphs.push(PlacedGlyph { char_index, atlas, pen: [pen, baseline], scale: atlas_scale });
                    }
                }
                pen += adv + tracking;
                char_index += 1;
            }
            char_index += 1; // the '\n'
        }
        let ascent = self.fonts[font].ascender as f64 * px;
        let first_baseline = cap * px / 2.0 - (n - 1.0) / 2.0 * line_step;
        let last_baseline = cap * px / 2.0 + (n - 1.0) / 2.0 * line_step;
        if !x_min.is_finite() {
            (x_min, x_max) = (0.0, 0.0);
        }
        let layout = Arc::new(Layout {
            glyphs,
            bounds: [x_min, first_baseline - ascent, x_max, last_baseline + 0.25 * text.size],
            unit_centers: boxes.into_iter().map(|(u, b)| (u, ((b[0] + b[2]) / 2.0, (b[1] + b[3]) / 2.0))).collect(),
        });
        self.layouts.insert(key, layout.clone());
        layout
    }

    /// Atlas texel → layer space transform of one glyph, with its unit's
    /// animation applied about the unit's centre.
    pub fn glyph_transform(layout: &Layout, text: &TextState, g: &PlacedGlyph) -> (Affine, f64) {
        let base = Affine::translate(g.pen[0], g.pen[1])
            .then(Affine::scale(g.scale, g.scale))
            .then(Affine::translate(g.atlas.origin[0] as f64 - g.atlas.rect[0] as f64, g.atlas.origin[1] as f64 - g.atlas.rect[1] as f64));
        let Some(state) = text.glyphs.get(g.char_index) else { return (base, 1.0) };
        if *state == GlyphState::IDENTITY {
            return (base, 1.0);
        }
        let unit = text.pivots.get(g.char_index).copied().unwrap_or(u32::MAX);
        let (cx, cy) = layout.unit_centers.get(&unit).copied().unwrap_or((g.pen[0], g.pen[1]));
        let animate = Affine::translate(cx + state.x, cy + state.y)
            .then(Affine::rotate(state.rotation))
            .then(Affine::scale(state.scale, state.scale))
            .then(Affine::translate(-cx, -cy));
        (animate.then(base), state.opacity)
    }
}

fn segment_distance2(px: f32, py: f32, s: &[f32; 4]) -> f32 {
    let (dx, dy) = (s[2] - s[0], s[3] - s[1]);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 { (((px - s[0]) * dx + (py - s[1]) * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
    let (ex, ey) = (s[0] + t * dx - px, s[1] + t * dy - py);
    ex * ex + ey * ey
}

struct Outline {
    segments: Vec<[f32; 4]>,
    start: (f32, f32),
    pen: (f32, f32),
    scale: f32,
}

impl Outline {
    fn line(&mut self, x: f32, y: f32) {
        let (x, y) = (x * self.scale, y * self.scale);
        if (x, y) != self.pen {
            self.segments.push([self.pen.0, self.pen.1, x, y]);
        }
        self.pen = (x, y);
    }
}

impl ttf_parser::OutlineBuilder for Outline {
    fn move_to(&mut self, x: f32, y: f32) {
        self.pen = (x * self.scale, y * self.scale);
        self.start = self.pen;
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.line(x, y);
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let (x0, y0) = (self.pen.0 / self.scale, self.pen.1 / self.scale);
        const N: usize = 8;
        for i in 1..=N {
            let t = i as f32 / N as f32;
            let m = 1.0 - t;
            self.line(m * m * x0 + 2.0 * m * t * x1 + t * t * x, m * m * y0 + 2.0 * m * t * y1 + t * t * y);
        }
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let (x0, y0) = (self.pen.0 / self.scale, self.pen.1 / self.scale);
        const N: usize = 12;
        for i in 1..=N {
            let t = i as f32 / N as f32;
            let m = 1.0 - t;
            let a = m * m * m;
            let b = 3.0 * m * m * t;
            let c = 3.0 * m * t * t;
            let d = t * t * t;
            self.line(a * x0 + b * x1 + c * x2 + d * x, a * y0 + b * y1 + c * y2 + d * y);
        }
    }
    fn close(&mut self) {
        let (sx, sy) = (self.start.0 / self.scale, self.start.1 / self.scale);
        self.line(sx, sy);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glyph_sdf_has_inside_and_outside() {
        let mut fonts = Fonts::new(None);
        let face = fonts.fonts[2].face();
        let id = face.glyph_index('O').unwrap().0;
        drop(face);
        let g = fonts.glyph(2, id);
        let [x0, y0, x1, y1] = g.rect;
        let at = |x: u32, y: u32| fonts.atlas[(y * ATLAS_SIZE + x) as usize];
        // Corner of the padded rect is outside; the stroke of the O (left
        // side, vertical middle) is inside; the counter (centre) is outside.
        assert!(at(x0, y0) < 100);
        let mid_y = (y0 + y1) / 2;
        let row: Vec<u8> = (x0..x1).map(|x| at(x, mid_y)).collect();
        assert!(row.iter().any(|v| *v > 160), "{row:?}");
        assert!(row[row.len() / 2] < 128, "counter of O must be outside");
    }
}
