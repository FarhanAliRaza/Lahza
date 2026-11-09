//! Image textures: decoded once, premultiplied, with a 2×2 box mip chain,
//! packed into one texel pool that the CPU and GPU paths sample alike.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Longest side kept; larger images are downscaled on load.
pub const MAX_SIDE: u32 = 4096;
/// Texel budget of the pool (u32 RGBA8 each).
pub const MAX_TEXELS: usize = 48 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Texture {
    pub offset: u32,
    pub width: u32,
    pub height: u32,
    pub levels: u32,
}

pub struct Images {
    root: Option<PathBuf>,
    pub texels: Vec<u32>,
    entries: HashMap<String, Option<Texture>>,
    pub generation: u64,
    pub warnings: Vec<String>,
    /// Sources looked up since `begin_frame`; `compact` keeps only these.
    used: HashSet<String>,
    /// Registered pixels with no file behind them, never evicted.
    pinned: HashSet<String>,
    /// An image did not fit in the pool since `begin_frame`.
    pub overflowed: bool,
    /// Images refused since `begin_frame` (no room): source, size, texels with mips.
    refused: Vec<(String, u32, u32, usize)>,
    /// Time spent loading images since the renderer last took it.
    pub(crate) busy: std::time::Duration,
    luts: HashMap<String, Option<std::sync::Arc<(usize, Vec<[f32; 4]>)>>>,
}

pub fn pack(c: [u8; 4]) -> u32 {
    c[0] as u32 | (c[1] as u32) << 8 | (c[2] as u32) << 16 | (c[3] as u32) << 24
}

impl Images {
    pub fn new(root: Option<&Path>) -> Self {
        Self { root: root.map(Path::to_path_buf), texels: Vec::new(), entries: HashMap::new(), generation: 1, warnings: Vec::new(), used: HashSet::new(), pinned: HashSet::new(), overflowed: false, refused: Vec::new(), luts: HashMap::new(), busy: std::time::Duration::ZERO }
    }

    pub fn begin_frame(&mut self) {
        self.used.clear();
        self.overflowed = false;
        self.refused.clear();
    }

    /// Why this frame's images do not fit the pool: what they need against
    /// `MAX_TEXELS`, the largest of them, and what to do (an error for the
    /// project's author to fix; nothing is shrunk or dropped silently).
    pub fn overflow_report(&self) -> String {
        let mut all: Vec<(String, u32, u32, usize)> = self
            .entries
            .iter()
            .filter(|(src, _)| self.used.contains(*src))
            .filter_map(|(src, e)| e.map(|t| (src.clone(), t.width, t.height, mip_texels(t.width, t.height, t.levels))))
            .collect();
        all.extend(self.refused.iter().cloned());
        all.sort_by(|a, b| b.3.cmp(&a.3));
        let need: usize = all.iter().map(|a| a.3).sum();
        let m = |n: usize| format!("{:.1}M", n as f64 / 1e6);
        let largest: Vec<String> = all.iter().take(6).map(|(s, w, h, n)| format!("{s} {w}×{h} ({})", m(*n))).collect();
        format!(
            "image memory budget exceeded: this frame's {} images need {} texels (with mips) of the {} pool. Largest: {}. \
             Make these textures smaller (a texture needs no more pixels than it covers on screen) or show fewer images at once.",
            all.len(),
            m(need),
            m(MAX_TEXELS),
            largest.join(", ")
        )
    }

    /// Drops every texture the current frame has not used and packs the rest
    /// to the front of the pool, so the frame's missing images fit when it
    /// is built again. Offsets change, so lists built before are stale.
    pub fn compact(&mut self) {
        let mut texels = Vec::new();
        let old = std::mem::take(&mut self.texels);
        self.entries.retain(|src, e| match e {
            Some(t) if self.used.contains(src) || self.pinned.contains(src) => {
                let n = mip_texels(t.width, t.height, t.levels);
                let at = t.offset as usize;
                t.offset = texels.len() as u32;
                texels.extend_from_slice(&old[at..at + n]);
                true
            }
            Some(_) => false,
            None => true,
        });
        self.texels = texels;
        self.generation += 1;
    }

    pub fn get(&mut self, src: &str) -> Option<Texture> {
        if !self.used.contains(src) {
            self.used.insert(src.to_string());
        }
        if let Some(e) = self.entries.get(src) {
            return *e;
        }
        let started = std::time::Instant::now();
        let path = match &self.root {
            Some(root) => root.join(src),
            None => PathBuf::from(src),
        };
        let texture = match image::open(&path) {
            Ok(img) => {
                let mut img = img.to_rgba8();
                if img.width().max(img.height()) > MAX_SIDE {
                    let k = MAX_SIDE as f64 / img.width().max(img.height()) as f64;
                    img = image::imageops::resize(
                        &img,
                        ((img.width() as f64 * k).round() as u32).max(1),
                        ((img.height() as f64 * k).round() as u32).max(1),
                        image::imageops::FilterType::Triangle,
                    );
                }
                let texture = self.insert_rgba(&img);
                if texture.is_none() {
                    // Pool full: not cached, so it loads once there is room;
                    // recorded for `overflow_report`.
                    let (w, h) = img.dimensions();
                    if !self.refused.iter().any(|r| r.0 == src) {
                        self.refused.push((src.to_string(), w, h, mip_texels(w, h, (32 - w.max(h).leading_zeros()).min(12))));
                    }
                    return None;
                }
                texture
            }
            Err(e) => {
                self.warnings.push(format!("image '{src}': {e}"));
                None
            }
        };
        self.busy += started.elapsed();
        self.entries.insert(src.to_string(), texture);
        texture
    }

    /// A 3D LUT from a .cube file: (size, entries with red fastest).
    pub fn lut(&mut self, path: &str) -> Option<std::sync::Arc<(usize, Vec<[f32; 4]>)>> {
        if let Some(l) = self.luts.get(path) {
            return l.clone();
        }
        let full = match &self.root {
            Some(r) => r.join(path),
            None => PathBuf::from(path),
        };
        let parsed = std::fs::read_to_string(&full).map_err(|e| e.to_string()).and_then(|t| parse_cube(&t));
        let lut = match parsed {
            Ok(l) => Some(std::sync::Arc::new(l)),
            Err(e) => {
                self.warnings.push(format!("LUT '{path}': {e}"));
                None
            }
        };
        self.luts.insert(path.to_string(), lut.clone());
        lut
    }

    /// Registers in-memory pixels under `name` (contact sheets, tests).
    pub fn insert_named(&mut self, name: &str, img: &image::RgbaImage) -> Option<Texture> {
        let texture = self.insert_rgba(img);
        self.entries.insert(name.to_string(), texture);
        self.pinned.insert(name.to_string());
        texture
    }

    /// Adds straight-alpha RGBA pixels; returns None when the pool is full.
    pub fn insert_rgba(&mut self, img: &image::RgbaImage) -> Option<Texture> {
        let (w, h) = img.dimensions();
        if w == 0 || h == 0 {
            return None;
        }
        let mut level: Vec<[f32; 4]> = img
            .pixels()
            .map(|p| {
                let a = p.0[3] as f32 / 255.0;
                [p.0[0] as f32 * a, p.0[1] as f32 * a, p.0[2] as f32 * a, p.0[3] as f32]
            })
            .collect();
        let mut total = 0usize;
        let (mut lw, mut lh) = (w, h);
        let mut levels = 0;
        loop {
            total += (lw * lh) as usize;
            levels += 1;
            if lw == 1 && lh == 1 || levels == 12 {
                break;
            }
            lw = (lw / 2).max(1);
            lh = (lh / 2).max(1);
        }
        if self.texels.len() + total > MAX_TEXELS {
            self.overflowed = true;
            return None;
        }
        let offset = self.texels.len() as u32;
        let (mut lw, mut lh) = (w, h);
        for l in 0..levels {
            self.texels.extend(level.iter().map(|c| pack(c.map(|v| v.round().clamp(0.0, 255.0) as u8))));
            if l + 1 == levels {
                break;
            }
            let (nw, nh) = ((lw / 2).max(1), (lh / 2).max(1));
            let mut next = vec![[0.0f32; 4]; (nw * nh) as usize];
            for y in 0..nh {
                for x in 0..nw {
                    let mut acc = [0.0; 4];
                    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                        let sx = (x * 2 + dx).min(lw - 1);
                        let sy = (y * 2 + dy).min(lh - 1);
                        let p = level[(sy * lw + sx) as usize];
                        for k in 0..4 {
                            acc[k] += p[k] * 0.25;
                        }
                    }
                    next[(y * nw + x) as usize] = acc;
                }
            }
            level = next;
            lw = nw;
            lh = nh;
        }
        self.generation += 1;
        Some(Texture { offset, width: w, height: h, levels })
    }
}

/// Texels of a texture and its mip chain, as `insert_rgba` lays them out.
fn mip_texels(width: u32, height: u32, levels: u32) -> usize {
    let (mut w, mut h, mut n) = (width, height, 0usize);
    for _ in 0..levels {
        n += (w * h) as usize;
        w = (w / 2).max(1);
        h = (h / 2).max(1);
    }
    n
}

/// Parses an Adobe/Resolve .cube 3D LUT (red varies fastest).
pub fn parse_cube(text: &str) -> Result<(usize, Vec<[f32; 4]>), String> {
    let mut size = 0usize;
    let mut entries = Vec::new();
    let (mut lo, mut hi) = ([0.0f32; 3], [1.0f32; 3]);
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("TITLE") {
            continue;
        }
        let parts: Vec<&str> = line.split_whitespace().collect();
        match parts[0] {
            "LUT_3D_SIZE" => size = parts.get(1).and_then(|v| v.parse().ok()).ok_or("bad LUT_3D_SIZE")?,
            "LUT_1D_SIZE" => return Err("1D LUTs are not supported".into()),
            "DOMAIN_MIN" => lo = [1, 2, 3].map(|i| parts.get(i).and_then(|v| v.parse().ok()).unwrap_or(0.0)),
            "DOMAIN_MAX" => hi = [1, 2, 3].map(|i| parts.get(i).and_then(|v| v.parse().ok()).unwrap_or(1.0)),
            _ => {
                let v: Vec<f32> = parts.iter().filter_map(|p| p.parse().ok()).collect();
                if v.len() == 3 {
                    entries.push([(v[0] - lo[0]) / (hi[0] - lo[0]), (v[1] - lo[1]) / (hi[1] - lo[1]), (v[2] - lo[2]) / (hi[2] - lo[2]), 1.0]);
                }
            }
        }
    }
    if size < 2 || size > 128 || entries.len() != size * size * size {
        return Err(format!("expected LUT_3D_SIZE N with N³ entries, got size {size} and {} entries", entries.len()));
    }
    Ok((size, entries))
}
