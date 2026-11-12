//! Image textures: decoded, premultiplied, with a 2×2 box mip chain,
//! packed into one texel pool that the CPU and GPU paths sample alike.
//!
//! A texture is kept at the size it shows, not the size of its file: each
//! request says how many texels across the picture needs this frame
//! (`get_sized`), and the pool keeps the file halved until one more
//! halving would be too few. A 4K map on a pebble costs a few thousand
//! texels; when the pebble fills the frame the map is loaded again, larger.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// Longest side kept; larger images are downscaled on load.
pub const MAX_SIDE: u32 = 4096;
/// Texel budget of the pool (u32 RGBA8 each) where nothing says more: the
/// CPU renderer's, and the least a GPU's is (`budget_for`).
pub const MAX_TEXELS: usize = 48 * 1024 * 1024;
/// The most the pool is given however large the GPU: its texels are also
/// held, and built, on the CPU.
pub const MOST_TEXELS: usize = 192 * 1024 * 1024;
/// Bytes of pictures' mip chains kept for loading them again at another
/// size (`Images::decoded`): three 4K maps.
const DECODED_BYTES: usize = 256 << 20;

/// A picture's whole mip chain as the pool holds texels (premultiplied
/// RGBA8, packed): each level the 2×2 box of the one above, down to one
/// texel. A copy of the picture at any size is a run of its levels.
struct Chain {
    /// Per level: its size and where its texels start.
    levels: Vec<(u32, u32, usize)>,
    texels: Vec<u32>,
}

impl Chain {
    fn build(img: &image::RgbaImage) -> Self {
        let (mut lw, mut lh) = img.dimensions();
        let mut level: Vec<[f32; 4]> = img
            .pixels()
            .map(|p| {
                let a = p.0[3] as f32 / 255.0;
                [p.0[0] as f32 * a, p.0[1] as f32 * a, p.0[2] as f32 * a, p.0[3] as f32]
            })
            .collect();
        let mut chain = Chain { levels: Vec::new(), texels: Vec::with_capacity(level.len() * 4 / 3 + 16) };
        loop {
            chain.levels.push((lw, lh, chain.texels.len()));
            chain.texels.extend(level.iter().map(|c| pack(c.map(|v| v.round().clamp(0.0, 255.0) as u8))));
            if lw <= 1 && lh <= 1 {
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
            (lw, lh) = (nw, nh);
        }
        chain
    }
}

/// The pool's budget on a GPU with `device_memory` bytes of its own whose
/// storage buffers may be `max_buffer` bytes (0: unknown): a sixteenth of
/// the memory (768 MB of a 12 GB card, four times the fixed 192 MB that a
/// film of scanned rocks overflowed), within what one buffer may hold.
pub fn budget_for(device_memory: u64, max_buffer: u64) -> usize {
    let by_memory = (device_memory / 16 / 4) as usize;
    let by_buffer = if max_buffer == 0 { MAX_TEXELS } else { (max_buffer / 4) as usize };
    by_memory.min(by_buffer).clamp(MAX_TEXELS, MOST_TEXELS)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Texture {
    pub offset: u32,
    /// The size it is held at.
    pub width: u32,
    pub height: u32,
    pub levels: u32,
    /// Its size as a picture (what an image layer without a `size` shows
    /// it at): the file's, scaled down to `MAX_SIDE` when larger.
    pub natural: [u32; 2],
}

/// The size a picture of `natural` size is held at to show `want` texels
/// along its longer side: halved while the halved size still has them.
pub fn held_size(natural: [u32; 2], want: u32) -> [u32; 2] {
    let (mut w, mut h) = (natural[0].max(1), natural[1].max(1));
    while w.max(h) / 2 >= want.max(1) && w.max(h) > 1 {
        w = (w / 2).max(1);
        h = (h / 2).max(1);
    }
    [w, h]
}

/// Texels a picture of `natural` size takes in the pool, mips included,
/// to show `want` texels along its longer side.
pub fn held_texels(natural: [u32; 2], want: u32) -> usize {
    let [w, h] = held_size(natural, want);
    mip_texels(w, h, mip_levels(w, h))
}

/// A file's size as a picture (see `Texture::natural`), from its header.
pub fn natural_size(path: &Path) -> Option<[u32; 2]> {
    let (w, h) = image::image_dimensions(path).ok()?;
    let k = (MAX_SIDE as f64 / w.max(h) as f64).min(1.0);
    Some([((w as f64 * k).round() as u32).max(1), ((h as f64 * k).round() as u32).max(1)])
}

fn mip_levels(w: u32, h: u32) -> u32 {
    (32 - w.max(h).max(1).leading_zeros()).min(12)
}

pub struct Images {
    root: Option<PathBuf>,
    pub texels: Vec<u32>,
    entries: HashMap<String, Option<Texture>>,
    pub generation: u64,
    /// Counts `compact`s: until the next, texels are only added (what the
    /// GPU holds of the pool stays where it is).
    pub packed: u64,
    pub warnings: Vec<String>,
    /// Sources looked up since `begin_frame`; `compact` keeps only these.
    used: HashSet<String>,
    /// The most texels along its longer side each was asked for since
    /// `begin_frame`: `compact` lets go of the ones held larger.
    wanted: HashMap<String, u32>,
    /// Texels the pool may hold (`set_budget`).
    budget: usize,
    /// Registered pixels with no file behind them, never evicted.
    pinned: HashSet<String>,
    /// An image did not fit in the pool since `begin_frame`.
    pub overflowed: bool,
    /// Images refused since `begin_frame` (no room): source, size, texels with mips.
    refused: Vec<(String, u32, u32, usize)>,
    /// Time spent loading images since the renderer last took it.
    pub(crate) busy: std::time::Duration,
    luts: HashMap<String, Option<std::sync::Arc<(usize, Vec<[f32; 4]>)>>>,
    /// Files' sizes as pictures, read once from their headers.
    naturals: HashMap<String, Option<[u32; 2]>>,
    /// The mip chains of the pictures loaded last, newest last, up to
    /// `DECODED_BYTES`: a map that grows on screen is loaded again larger
    /// a few times, and decoded and halved once.
    decoded: Vec<(String, Chain)>,
}

pub fn pack(c: [u8; 4]) -> u32 {
    c[0] as u32 | (c[1] as u32) << 8 | (c[2] as u32) << 16 | (c[3] as u32) << 24
}

impl Images {
    pub fn new(root: Option<&Path>) -> Self {
        Self {
            root: root.map(Path::to_path_buf),
            texels: Vec::new(),
            entries: HashMap::new(),
            generation: 1,
            packed: 0,
            warnings: Vec::new(),
            used: HashSet::new(),
            wanted: HashMap::new(),
            budget: MAX_TEXELS,
            pinned: HashSet::new(),
            overflowed: false,
            refused: Vec::new(),
            luts: HashMap::new(),
            naturals: HashMap::new(),
            decoded: Vec::new(),
            busy: std::time::Duration::ZERO,
        }
    }

    /// Texels the pool may hold: `MAX_TEXELS` unless the renderer's GPU
    /// has room for more (`budget_for`).
    pub fn set_budget(&mut self, texels: usize) {
        self.budget = texels.max(1);
    }

    pub fn budget(&self) -> usize {
        self.budget
    }

    pub fn begin_frame(&mut self) {
        self.used.clear();
        self.wanted.clear();
        self.overflowed = false;
        self.refused.clear();
    }

    /// Why this frame's images do not fit the pool: what they need against
    /// its budget, the largest of them, and what to do (an error for the
    /// project's author to fix; nothing is dropped silently).
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
            "image memory budget exceeded: this frame's {} images need {} texels (with mips, at the sizes they show) of the {} pool. Largest: {}. \
             Show fewer or smaller images at once.",
            all.len(),
            m(need),
            m(self.budget),
            largest.join(", ")
        )
    }

    /// Drops every texture the current frame has not used and packs the rest
    /// to the front of the pool, so the frame's missing images fit when it
    /// is built again. Offsets change, so lists built before are stale.
    /// Textures held larger than this frame asked for are dropped too:
    /// they load again at the size it needs.
    pub fn compact(&mut self) {
        let mut texels = Vec::new();
        let old = std::mem::take(&mut self.texels);
        let oversized = |src: &String, t: &Texture| self.wanted.get(src).is_some_and(|want| held_size(t.natural, *want) != [t.width, t.height]);
        self.entries.retain(|src, e| match e {
            Some(t) if self.pinned.contains(src) || (self.used.contains(src) && !oversized(src, t)) => {
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
        self.packed += 1;
    }

    /// The image's size as a picture (`Texture::natural`), without
    /// loading it: of the copy held, else from the file's header.
    pub fn natural(&mut self, src: &str) -> Option<[u32; 2]> {
        if let Some(Some(t)) = self.entries.get(src) {
            return Some(t.natural);
        }
        if let Some(n) = self.naturals.get(src) {
            return *n;
        }
        let path = match &self.root {
            Some(root) => root.join(src),
            None => PathBuf::from(src),
        };
        let n = natural_size(&path);
        self.naturals.insert(src.to_string(), n);
        n
    }

    /// The image at its full size (a picture whose every texel counts: a
    /// density grid's slices).
    pub fn get(&mut self, src: &str) -> Option<Texture> {
        self.get_sized(src, u32::MAX)
    }

    /// The image, held with at least `want` texels along its longer side
    /// (or all it has): what it needs to show sharp this frame. Held
    /// smaller than that, it is loaded again.
    pub fn get_sized(&mut self, src: &str, want: u32) -> Option<Texture> {
        if !self.used.contains(src) {
            self.used.insert(src.to_string());
        }
        // (Asked for by every layer that shows it, every sub-frame: no
        // new string where it is known.)
        let want = match self.wanted.get_mut(src) {
            Some(most) => {
                *most = (*most).max(want);
                *most
            }
            None => {
                self.wanted.insert(src.to_string(), want);
                want
            }
        };
        match self.entries.get(src) {
            Some(Some(t)) => {
                let need = held_size(t.natural, want);
                if t.width >= need[0] && t.height >= need[1] {
                    return Some(*t);
                }
            }
            Some(None) => return None,
            None => {}
        }
        let started = std::time::Instant::now();
        let path = match &self.root {
            Some(root) => root.join(src),
            None => PathBuf::from(src),
        };
        let decoded = match self.decoded.iter().position(|d| d.0 == src) {
            Some(at) => Ok(self.decoded.remove(at).1),
            None => image::open(&path).map(|img| {
                let img = img.to_rgba8();
                let k = (MAX_SIDE as f64 / img.width().max(img.height()) as f64).min(1.0);
                let natural = [((img.width() as f64 * k).round() as u32).max(1), ((img.height() as f64 * k).round() as u32).max(1)];
                if (natural[0], natural[1]) != img.dimensions() {
                    Chain::build(&image::imageops::resize(&img, natural[0], natural[1], image::imageops::FilterType::Triangle))
                } else {
                    Chain::build(&img)
                }
            }),
        };
        let texture = match decoded {
            Ok(img) => {
                let natural = [img.levels[0].0, img.levels[0].1];
                // Halved as its mips are, so the copy held is the mip of
                // the whole picture at that size, texel for texel.
                let [w, h] = held_size(natural, want);
                let mut skip = 0;
                while (natural[0] >> skip).max(1) > w || (natural[1] >> skip).max(1) > h {
                    skip += 1;
                }
                let texture = self.insert_chain(&img, skip).map(|t| Texture { natural, ..t });
                // Kept for the next size it is asked for, the oldest making way.
                self.decoded.push((src.to_string(), img));
                let bytes = |d: &[(String, Chain)]| d.iter().map(|d| d.1.texels.len() * 4).sum::<usize>();
                while self.decoded.len() > 1 && bytes(&self.decoded) > DECODED_BYTES {
                    self.decoded.remove(0);
                }
                if texture.is_none() {
                    // Pool full: the entry stays as it was (not there, or
                    // held smaller), so it loads once there is room;
                    // recorded for `overflow_report`.
                    if !self.refused.iter().any(|r| r.0 == src) {
                        self.refused.push((src.to_string(), w, h, mip_texels(w, h, mip_levels(w, h))));
                    }
                    self.busy += started.elapsed();
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
        // (A smaller copy it replaces stays in the pool until `compact`.)
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
        if img.width() == 0 || img.height() == 0 {
            return None;
        }
        self.insert_chain(&Chain::build(img), 0)
    }

    /// Adds a picture from its `skip`-th mip down (0: whole): the texture
    /// is that mip and the mips below it (twelve levels at most). None
    /// when the pool is full.
    fn insert_chain(&mut self, chain: &Chain, skip: u32) -> Option<Texture> {
        let first = (skip as usize).min(chain.levels.len() - 1);
        let (w, h, from) = chain.levels[first];
        let levels = (chain.levels.len() - first).min(12);
        let to = chain.levels.get(first + levels).map_or(chain.texels.len(), |l| l.2);
        if self.texels.len() + (to - from) > self.budget {
            self.overflowed = true;
            return None;
        }
        let offset = self.texels.len() as u32;
        self.texels.extend_from_slice(&chain.texels[from..to]);
        self.generation += 1;
        Some(Texture { offset, width: w, height: h, levels: levels as u32, natural: [chain.levels[0].0, chain.levels[0].1] })
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

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lahza-images-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A picture is held at the size it shows: halved until one more
    /// halving would be too few texels; asked for larger, it is loaded
    /// again; asked for smaller, the copy held serves.
    #[test]
    fn a_texture_is_held_at_the_size_it_shows() {
        let d = dir("held");
        image::RgbaImage::from_pixel(1024, 512, image::Rgba([200, 100, 50, 255])).save(d.join("map.png")).unwrap();
        let mut pool = Images::new(Some(&d));
        assert_eq!(pool.natural("map.png"), Some([1024, 512]));
        assert!(pool.texels.is_empty(), "its size is read from the header, not by loading it");
        let small = pool.get_sized("map.png", 100).unwrap();
        assert_eq!((small.width, small.height, small.natural), (128, 64, [1024, 512]), "100 texels across: 128 x 64 has them, 64 x 32 has not");
        assert_eq!(pool.texels.len(), held_texels([1024, 512], 100));
        assert!(pool.texels.len() < 1024 * 512 / 40, "a fortieth of the file's texels: {}", pool.texels.len());
        let generation = pool.generation;
        assert_eq!(pool.get_sized("map.png", 60), Some(small), "smaller is served by the copy held");
        assert_eq!(pool.generation, generation);
        let medium = pool.get_sized("map.png", 300).unwrap();
        assert_eq!((medium.width, medium.height), (512, 256), "asked for larger, it is loaded again");
        let full = pool.get("map.png").unwrap();
        assert_eq!((full.width, full.height, full.levels), (1024, 512, 11));
        assert_eq!(pool.get_sized("map.png", 10), Some(full));
        // Its colour survives the shrinking (premultiplied RGBA8, packed).
        assert_eq!(pool.texels[small.offset as usize], pack([200, 100, 50, 255]));
        std::fs::remove_dir_all(&d).ok();
    }

    /// When the pool is full, what this frame did not ask for goes, and so
    /// do copies larger than it asked for (they load again at its size).
    #[test]
    fn compacting_lets_go_of_copies_larger_than_the_frame_needs() {
        let d = dir("compact");
        image::RgbaImage::from_pixel(512, 512, image::Rgba([10, 20, 30, 255])).save(d.join("a.png")).unwrap();
        image::RgbaImage::from_pixel(512, 512, image::Rgba([40, 50, 60, 255])).save(d.join("b.png")).unwrap();
        let mut pool = Images::new(Some(&d));
        // Room for one of them whole (349 525 texels with its mips).
        pool.set_budget(400_000);
        pool.begin_frame();
        assert!(pool.get("a.png").is_some());
        assert!(!pool.overflowed);
        // The next frame shows both small: b does not fit beside a whole.
        pool.begin_frame();
        assert!(pool.get_sized("a.png", 64).is_some());
        assert!(pool.get_sized("b.png", 64).is_some(), "64 x 64 fits beside a");
        assert!(!pool.overflowed);
        // Then both at 400 texels: a is held whole, b needs 512 x 512 too.
        pool.begin_frame();
        assert!(pool.get_sized("a.png", 400).is_some());
        assert!(pool.get_sized("b.png", 400).is_none() && pool.overflowed, "two whole do not fit");
        assert!(pool.overflow_report().contains("b.png 512×512"), "{}", pool.overflow_report());
        // And both at 100: after compacting, a's whole copy is let go and
        // both fit at 128 x 128.
        pool.begin_frame();
        pool.get_sized("a.png", 100);
        pool.get_sized("b.png", 100);
        pool.compact();
        pool.begin_frame();
        let (a, b) = (pool.get_sized("a.png", 100).unwrap(), pool.get_sized("b.png", 100).unwrap());
        assert_eq!((a.width, b.width), (128, 128));
        assert_eq!(pool.texels.len(), 2 * held_texels([512, 512], 100));
        assert_eq!(pool.texels[a.offset as usize], pack([10, 20, 30, 255]));
        assert_eq!(pool.texels[b.offset as usize], pack([40, 50, 60, 255]));
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn the_budget_follows_the_gpus_memory() {
        let gb = 1u64 << 30;
        assert_eq!(budget_for(0, 0), MAX_TEXELS, "unknown: the fixed pool");
        assert_eq!(budget_for(2 * gb, 4 * gb), MAX_TEXELS, "a small card keeps it");
        assert_eq!(budget_for(12 * gb, 4 * gb), 12 * (1 << 30) / 64, "a sixteenth of a 12 GB card's memory, in texels");
        assert_eq!(budget_for(48 * gb, 4 * gb), MOST_TEXELS);
        assert_eq!(budget_for(12 * gb, 256 << 20), (256 << 20) / 4, "within one buffer");
    }
}
