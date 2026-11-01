//! Image textures: decoded once, premultiplied, with a 2×2 box mip chain,
//! packed into one texel pool that the CPU and GPU paths sample alike.

use std::collections::HashMap;
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
}

pub fn pack(c: [u8; 4]) -> u32 {
    c[0] as u32 | (c[1] as u32) << 8 | (c[2] as u32) << 16 | (c[3] as u32) << 24
}

impl Images {
    pub fn new(root: Option<&Path>) -> Self {
        Self { root: root.map(Path::to_path_buf), texels: Vec::new(), entries: HashMap::new(), generation: 1, warnings: Vec::new() }
    }

    pub fn get(&mut self, src: &str) -> Option<Texture> {
        if let Some(e) = self.entries.get(src) {
            return *e;
        }
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
                self.insert_rgba(&img)
            }
            Err(e) => {
                self.warnings.push(format!("image '{src}': {e}"));
                None
            }
        };
        self.entries.insert(src.to_string(), texture);
        texture
    }

    /// Registers in-memory pixels under `name` (contact sheets, tests).
    pub fn insert_named(&mut self, name: &str, img: &image::RgbaImage) -> Option<Texture> {
        let texture = self.insert_rgba(img);
        self.entries.insert(name.to_string(), texture);
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
            self.warnings.push("image memory budget reached; some images are not drawn".into());
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
