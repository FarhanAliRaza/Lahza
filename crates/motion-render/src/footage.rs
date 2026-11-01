//! Footage: video files and image sequences as per-frame textures.
//!
//! Each video source keeps one ffmpeg process streaming raw RGBA from the
//! last position: playback and export read frames in order (no seeking),
//! a jump backwards or far ahead restarts the stream at the new time
//! (scrubbing). Recently used frames stay in a small cache, so held
//! frames, stepped layers and motion-blur sub-samples cost nothing.
//!
//! Frames go into a per-frame texture pool (`dynamic`) that is rebuilt for
//! every output frame and uploaded to its own GPU buffer, so footage never
//! fills the static image pool.

use crate::images::{pack, Texture};
use std::collections::{HashMap, VecDeque};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::Arc;

/// Longest side decoded; larger footage is scaled down by ffmpeg.
const MAX_SIDE: u32 = 3840;
/// Frames kept per source.
const CACHE_FRAMES: usize = 24;
/// Reading ahead this many frames is cheaper than a restart.
const MAX_SKIP: i64 = 48;

#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct MediaInfo {
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub duration: f64,
    pub frames: u64,
    pub has_audio: bool,
    pub has_alpha: bool,
    pub codec: String,
    /// "video" or "sequence".
    pub kind: &'static str,
}

/// Probes a video file with ffprobe.
pub fn probe(path: &Path) -> Result<MediaInfo, String> {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-show_streams", "-show_format", "-of", "json"])
        .arg(path)
        .output()
        .map_err(|e| format!("cannot run ffprobe (install ffmpeg): {e}"))?;
    if !out.status.success() {
        return Err(format!("{}: {}", path.display(), String::from_utf8_lossy(&out.stderr).trim()));
    }
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(|e| e.to_string())?;
    let streams = v["streams"].as_array().cloned().unwrap_or_default();
    let video = streams.iter().find(|s| s["codec_type"] == "video").ok_or_else(|| format!("{} has no video stream", path.display()))?;
    let rate = |s: &str| -> f64 {
        let (n, d) = s.split_once('/').unwrap_or((s, "1"));
        let (n, d) = (n.parse::<f64>().unwrap_or(0.0), d.parse::<f64>().unwrap_or(1.0));
        if d > 0.0 { n / d } else { 0.0 }
    };
    let mut fps = rate(video["avg_frame_rate"].as_str().unwrap_or("0/1"));
    if !(fps > 0.0 && fps < 1000.0) {
        fps = rate(video["r_frame_rate"].as_str().unwrap_or("30/1"));
    }
    let duration = video["duration"].as_str().or(v["format"]["duration"].as_str()).and_then(|d| d.parse::<f64>().ok()).unwrap_or(0.0);
    let pix = video["pix_fmt"].as_str().unwrap_or("");
    let codec = video["codec_name"].as_str().unwrap_or("").to_string();
    let alpha_tag = video["tags"]["alpha_mode"].as_str() == Some("1") || video["tags"]["ALPHA_MODE"].as_str() == Some("1");
    let has_alpha = alpha_tag || pix.contains('a') && (pix.starts_with("yuva") || pix.starts_with("rgba") || pix.starts_with("argb") || pix.starts_with("bgra") || pix.starts_with("gbrap") || pix.starts_with("pal8") || pix == "ya8" || pix.starts_with("ya16"));
    Ok(MediaInfo {
        width: video["width"].as_u64().unwrap_or(0) as u32,
        height: video["height"].as_u64().unwrap_or(0) as u32,
        fps: if fps > 0.0 { fps } else { 30.0 },
        duration,
        frames: video["nb_frames"].as_str().and_then(|n| n.parse().ok()).unwrap_or((duration * fps).round() as u64),
        has_audio: streams.iter().any(|s| s["codec_type"] == "audio"),
        has_alpha,
        codec,
        kind: "video",
    })
}

/// An image sequence: `name_####.png` (Blender's `#` padding) or a folder.
fn sequence_frames(root: &Path, src: &str) -> Option<Vec<PathBuf>> {
    let path = root.join(src);
    let mut files: Vec<PathBuf> = if path.is_dir() {
        std::fs::read_dir(&path).ok()?.flatten().map(|e| e.path()).filter(|p| is_image(p)).collect()
    } else if src.contains('#') {
        let file = Path::new(src).file_name()?.to_str()?;
        let dir = root.join(Path::new(src).parent().unwrap_or(Path::new("")));
        let (prefix, rest) = file.split_at(file.find('#')?);
        let hashes = rest.chars().take_while(|c| *c == '#').count();
        let suffix = &rest[hashes..];
        std::fs::read_dir(&dir)
            .ok()?
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                let n = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                n.len() >= prefix.len() + suffix.len()
                    && n.starts_with(prefix)
                    && n.ends_with(suffix)
                    && n[prefix.len()..n.len() - suffix.len()].chars().all(|c| c.is_ascii_digit())
                    && n.len() - prefix.len() - suffix.len() >= hashes
            })
            .collect()
    } else {
        return None;
    };
    // Natural order by the frame number.
    files.sort_by_key(|p| {
        let n = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let digits: String = n.chars().rev().take_while(|c| c.is_ascii_digit()).collect::<Vec<_>>().into_iter().rev().collect();
        (digits.parse::<u64>().unwrap_or(0), n.to_string())
    });
    (!files.is_empty()).then_some(files)
}

fn is_image(p: &Path) -> bool {
    matches!(
        p.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref(),
        Some("png" | "jpg" | "jpeg" | "webp" | "exr" | "tif" | "tiff" | "bmp" | "tga")
    )
}

struct Stream {
    child: Child,
    out: ChildStdout,
    /// Frame index the next read returns.
    next: i64,
}

impl Drop for Stream {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Video {
    path: PathBuf,
    info: MediaInfo,
    /// Decoded size.
    w: u32,
    h: u32,
    stream: Option<Stream>,
    alpha_decoder: bool,
}

enum Source {
    Video(Box<Video>),
    Sequence { frames: Vec<PathBuf> },
    Missing,
}

pub struct Footage {
    root: Option<PathBuf>,
    sources: HashMap<String, Source>,
    cache: HashMap<String, VecDeque<(i64, Arc<image::RgbaImage>)>>,
    /// This output frame's textures (rebuilt every frame).
    pub dynamic: Vec<u32>,
    placed: HashMap<(String, i64), Texture>,
    pub warnings: Vec<String>,
    pub decoded: u64,
    pub seeks: u64,
}

impl Footage {
    pub fn new(root: Option<&Path>) -> Self {
        Self {
            root: root.map(Path::to_path_buf),
            sources: HashMap::new(),
            cache: HashMap::new(),
            dynamic: Vec::new(),
            placed: HashMap::new(),
            warnings: Vec::new(),
            decoded: 0,
            seeks: 0,
        }
    }

    /// Starts a new output frame: the dynamic pool is rebuilt.
    pub fn begin_frame(&mut self) {
        self.dynamic.clear();
        self.placed.clear();
    }

    fn root(&self) -> PathBuf {
        self.root.clone().unwrap_or_else(|| PathBuf::from("."))
    }

    fn source(&mut self, src: &str) -> &mut Source {
        if !self.sources.contains_key(src) {
            let root = self.root();
            let source = if let Some(frames) = sequence_frames(&root, src) {
                Source::Sequence { frames }
            } else {
                let path = root.join(src);
                match probe(&path) {
                    Ok(info) => {
                        let k = (MAX_SIDE as f64 / info.width.max(info.height).max(1) as f64).min(1.0);
                        let w = ((info.width as f64 * k).round() as u32).max(1);
                        let h = ((info.height as f64 * k).round() as u32).max(1);
                        // VP8/VP9 keep alpha in a side stream only libvpx decodes.
                        let alpha_decoder = info.has_alpha && (info.codec == "vp9" || info.codec == "vp8");
                        Source::Video(Box::new(Video { path, info, w, h, stream: None, alpha_decoder }))
                    }
                    Err(e) => {
                        self.warnings.push(format!("footage '{src}': {e}"));
                        Source::Missing
                    }
                }
            };
            self.sources.insert(src.to_string(), source);
        }
        self.sources.get_mut(src).unwrap()
    }

    /// Which source frames this output frame uses (frame-cache key).
    pub fn signature(&self) -> Vec<u8> {
        let mut keys: Vec<(&String, &i64)> = self.placed.keys().map(|(s, i)| (s, i)).collect();
        keys.sort();
        let mut out = Vec::new();
        for (s, i) in keys {
            out.extend_from_slice(s.as_bytes());
            out.extend_from_slice(&i.to_le_bytes());
        }
        out
    }

    /// Facts about a source (size, fps, duration, alpha, audio).
    pub fn info(&mut self, src: &str) -> Option<MediaInfo> {
        match self.source(src) {
            Source::Video(v) => Some(v.info.clone()),
            Source::Sequence { frames } => {
                let n = frames.len();
                let first = frames[0].clone();
                let dims = image::image_dimensions(&first).ok()?;
                Some(MediaInfo {
                    width: dims.0,
                    height: dims.1,
                    fps: 0.0,
                    duration: 0.0,
                    frames: n as u64,
                    has_audio: false,
                    has_alpha: true,
                    codec: first.extension().and_then(|e| e.to_str()).unwrap_or("").into(),
                    kind: "sequence",
                })
            }
            Source::Missing => None,
        }
    }

    /// The texture for `src` at `source_time` seconds, placed in this
    /// frame's dynamic pool (`fps` is the rate of image sequences).
    pub fn frame(&mut self, src: &str, source_time: f64, looped: bool, trim: f64, seq_fps: f64) -> Option<Texture> {
        let (index, count) = match self.source(src) {
            Source::Video(v) => {
                let fps = v.info.fps;
                let count = (v.info.frames.max(1)) as i64;
                (time_to_index(source_time, fps, looped, trim, count), count)
            }
            Source::Sequence { frames } => {
                let count = frames.len() as i64;
                (time_to_index(source_time, seq_fps.max(1e-3), looped, trim, count), count)
            }
            Source::Missing => return None,
        };
        let index = index.clamp(0, count - 1);
        if let Some(t) = self.placed.get(&(src.to_string(), index)) {
            return Some(*t);
        }
        let image = self.decode(src, index)?;
        let texture = place(&mut self.dynamic, &image);
        self.placed.insert((src.to_string(), index), texture);
        Some(texture)
    }

    fn cached(&self, src: &str, index: i64) -> Option<Arc<image::RgbaImage>> {
        self.cache.get(src)?.iter().find(|(i, _)| *i == index).map(|(_, f)| f.clone())
    }

    fn remember(&mut self, src: &str, index: i64, image: Arc<image::RgbaImage>) {
        let c = self.cache.entry(src.to_string()).or_default();
        c.push_back((index, image));
        while c.len() > CACHE_FRAMES {
            c.pop_front();
        }
    }

    fn decode(&mut self, src: &str, index: i64) -> Option<Arc<image::RgbaImage>> {
        if let Some(f) = self.cached(src, index) {
            return Some(f);
        }
        let image = match self.source(src) {
            Source::Sequence { frames } => {
                let path = frames.get(index as usize)?.clone();
                match image::open(&path) {
                    Ok(img) => Arc::new(img.to_rgba8()),
                    Err(e) => {
                        self.warnings.push(format!("{}: {e}", path.display()));
                        return None;
                    }
                }
            }
            Source::Video(v) => match read_video(v, index) {
                Some((frame, seeked)) => {
                    self.seeks += seeked as u64;
                    frame
                }
                // Past the real end (container frame counts can be off):
                // hold the last frame decoded.
                None => return self.cache.get(src).and_then(|c| c.iter().max_by_key(|(i, _)| *i).map(|(_, f)| f.clone())),
            },
            Source::Missing => return None,
        };
        self.decoded += 1;
        self.remember(src, index, image.clone());
        Some(image)
    }
}

fn time_to_index(t: f64, fps: f64, looped: bool, trim: f64, count: i64) -> i64 {
    // A small epsilon keeps exact frame times on their own frame.
    let mut i = ((t * fps) + 1e-6).floor() as i64;
    if looped && count > 0 {
        let start = ((trim * fps) + 1e-6).floor() as i64;
        let len = (count - start.clamp(0, count - 1)).max(1);
        if i >= count {
            i = start + (i - start).rem_euclid(len);
        }
    }
    i
}

fn spawn(v: &Video, index: i64) -> Option<Stream> {
    let t = index as f64 / v.info.fps;
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-v", "error", "-nostdin"]);
    if v.alpha_decoder {
        cmd.args(["-c:v", if v.info.codec == "vp8" { "libvpx" } else { "libvpx-vp9" }]);
    }
    // Seek a hair before the frame so rounding never lands one late.
    cmd.args(["-ss", &format!("{:.6}", (t - 0.25 / v.info.fps).max(0.0))]).arg("-i").arg(&v.path);
    let mut vf = format!("fps={}", v.info.fps);
    if v.w != v.info.width || v.h != v.info.height {
        vf.push_str(&format!(",scale={}:{}:flags=bicubic", v.w, v.h));
    }
    cmd.args(["-an", "-vf", &vf, "-f", "rawvideo", "-pix_fmt", "rgba", "-"]).stdout(Stdio::piped()).stderr(Stdio::null());
    let mut child = cmd.spawn().ok()?;
    let out = child.stdout.take()?;
    Some(Stream { child, out, next: index })
}

/// Reads frame `index`, reusing the running stream when it is at or a
/// little before it. Returns the frame and whether a seek was needed.
fn read_video(v: &mut Video, index: i64) -> Option<(Arc<image::RgbaImage>, bool)> {
    let mut seeked = false;
    let usable = v.stream.as_ref().is_some_and(|s| s.next <= index && index - s.next <= MAX_SKIP);
    if !usable {
        v.stream = spawn(v, index);
        seeked = true;
    }
    let bytes = (v.w * v.h * 4) as usize;
    let mut buf = vec![0u8; bytes];
    loop {
        let s = v.stream.as_mut()?;
        if s.out.read_exact(&mut buf).is_err() {
            v.stream = None;
            return None;
        }
        let at = s.next;
        s.next += 1;
        if at == index {
            break;
        }
    }
    image::RgbaImage::from_raw(v.w, v.h, buf).map(|i| (Arc::new(i), seeked))
}

/// Premultiplies `img` into `pool` with a 2×2 box mip chain.
fn place(pool: &mut Vec<u32>, img: &image::RgbaImage) -> Texture {
    let (w, h) = img.dimensions();
    let offset = pool.len() as u32;
    let mut level: Vec<[f32; 4]> = img
        .pixels()
        .map(|p| {
            let a = p.0[3] as f32 / 255.0;
            [p.0[0] as f32 * a, p.0[1] as f32 * a, p.0[2] as f32 * a, p.0[3] as f32]
        })
        .collect();
    let (mut lw, mut lh) = (w, h);
    let mut levels = 0;
    loop {
        pool.extend(level.iter().map(|c| pack(c.map(|v| (v + 0.5).clamp(0.0, 255.0) as u8))));
        levels += 1;
        if lw == 1 && lh == 1 || levels == 12 {
            break;
        }
        let (nw, nh) = ((lw / 2).max(1), (lh / 2).max(1));
        let mut next = vec![[0.0f32; 4]; (nw * nh) as usize];
        for y in 0..nh {
            for x in 0..nw {
                let mut acc = [0.0; 4];
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let p = level[((y * 2 + dy).min(lh - 1) * lw + (x * 2 + dx).min(lw - 1)) as usize];
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
    Texture { offset, width: w, height: h, levels }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looping_and_holding() {
        assert_eq!(time_to_index(0.0, 30.0, false, 0.0, 10), 0);
        assert_eq!(time_to_index(1.0 / 30.0, 30.0, false, 0.0, 10), 1);
        assert_eq!(time_to_index(0.5, 30.0, true, 0.0, 10), 5);
        // Loop region is frames 3..9: frame 12 wraps to 5.
        assert_eq!(time_to_index(12.0 / 30.0, 30.0, true, 0.1, 10), 5);
    }
}
