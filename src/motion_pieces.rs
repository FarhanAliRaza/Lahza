//! Motion pieces in the studio: motion projects that live inside the open
//! recording (`motion/<name>.lahza`) and play on its timeline, over the
//! video or covering it (see `lahza_motion_core::recording`).
//!
//! The studio keeps them simple: one lane of named bars, a few options in
//! the inspector, and frames drawn over the preview. Frames render on a
//! worker thread that owns one motion `Renderer` per piece; the newest
//! request replaces a waiting one. Export renders them at export size on
//! a thread of its own and hands them to the compositor as the canvas
//! overlay, under the canvas annotations. Their sound joins the mix.
//!
//! Agents add and change pieces over MCP; the studio polls the edit
//! document and the piece folders and reloads what changed.

use crate::recording::media::AudioClip;
use gpui::RenderImage;
use image::RgbaImage;
use lahza_motion_core::compile::Compiled;
use lahza_motion_core::document::Project;
use lahza_motion_core::io;
use lahza_motion_core::recording::{self, MotionPiece, Placement};
use lahza_motion_render::{RenderOptions, Renderer};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::time::SystemTime;

/// A placed piece with its project loaded.
#[derive(Clone)]
pub(crate) struct LoadedPiece {
    pub piece: MotionPiece,
    pub dir: PathBuf,
    pub project: Project,
    pub compiled: Arc<Compiled>,
    stamp: SystemTime,
    audio_signature: String,
    /// Sounds in the piece's own time.
    audio: Vec<(PathBuf, AudioClip)>,
}

impl LoadedPiece {
    pub fn duration(&self) -> f64 {
        self.project.settings.duration
    }
    pub fn end(&self) -> f64 {
        self.piece.start + self.duration()
    }
    pub fn active_at(&self, t: f64) -> bool {
        !self.piece.hidden && t >= self.piece.start && t < self.end()
    }
    fn size(&self) -> (f64, f64) {
        (self.project.settings.width as f64, self.project.settings.height as f64)
    }
}

fn load_piece(rec: &Path, piece: MotionPiece, previous: Option<&LoadedPiece>) -> Result<LoadedPiece, String> {
    let dir = piece.dir(rec);
    let stamp = crate::motion_editor::stamp(&dir);
    if let Some(prev) = previous.filter(|p| p.dir == dir && p.stamp == stamp) {
        return Ok(LoadedPiece { piece, ..prev.clone() });
    }
    let project = io::load(&dir).map_err(|e| format!("{}: {e}", piece.name))?;
    let compiled = Arc::new(Compiled::new(&project));
    let audio_signature = crate::motion_sound::signature(&project, &compiled);
    let audio = match previous.filter(|p| p.dir == dir && p.audio_signature == audio_signature) {
        Some(prev) => prev.audio.clone(),
        None => crate::motion_sound::clips(&project, &compiled, &dir),
    };
    Ok(LoadedPiece { piece, dir, project, compiled, stamp, audio_signature, audio })
}

fn rgba8(c: lahza_motion_core::value::Color) -> [u8; 4] {
    c.0.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)
}

/// Newest modification of the recording's edit document.
fn edit_stamp(rec: &Path) -> SystemTime {
    [recording::DRAFT_FILE, recording::EDIT_FILE]
        .iter()
        .filter_map(|f| std::fs::metadata(rec.join(f)).and_then(|m| m.modified()).ok())
        .max()
        .unwrap_or(SystemTime::UNIX_EPOCH)
}

/// One drawn full-screen piece, ready for the preview canvas.
pub(crate) struct PieceFrame {
    pub image: Arc<RenderImage>,
    pub placement: Placement,
}

pub(crate) struct MotionPieces {
    pub recording: PathBuf,
    pub items: Vec<LoadedPiece>,
    pub errors: Vec<String>,
    edit_stamp: SystemTime,
    worker: Worker,
    /// Full-screen pieces, drawn over the preview canvas.
    pub frames: Vec<PieceFrame>,
    /// Over-the-video pieces in the recording's own space, for the
    /// compositor's media overlay: (frame key, layer).
    media_frame: Option<(u64, Arc<RgbaImage>)>,
    /// The media layer merged with the annotation layer, by their keys.
    merged: Option<((u64, u64), Arc<RgbaImage>)>,
    last_request: u64,
    request_key: u64,
}

impl MotionPieces {
    pub(crate) fn load(recording: &Path) -> Self {
        let mut pieces = Self {
            recording: recording.to_path_buf(),
            items: Vec::new(),
            errors: Vec::new(),
            edit_stamp: SystemTime::UNIX_EPOCH,
            worker: Worker::new(),
            frames: Vec::new(),
            media_frame: None,
            merged: None,
            last_request: 0,
            request_key: 0,
        };
        pieces.reload();
        pieces
    }

    fn reload(&mut self) {
        self.edit_stamp = edit_stamp(&self.recording);
        let previous = std::mem::take(&mut self.items);
        self.errors.clear();
        for piece in recording::read_pieces(&self.recording) {
            let prev = previous.iter().find(|p| p.piece.id == piece.id);
            match load_piece(&self.recording, piece, prev) {
                Ok(p) => self.items.push(p),
                Err(e) => self.errors.push(e),
            }
        }
        // Force a new frame request.
        self.last_request = 0;
    }

    /// Reloads when the edit document or a piece changed on disk.
    pub(crate) fn poll(&mut self) -> bool {
        let changed = edit_stamp(&self.recording) != self.edit_stamp
            || self.items.iter().any(|p| crate::motion_editor::stamp(&p.dir) != p.stamp);
        if changed {
            self.reload();
        }
        changed
    }

    /// Writes the pieces to the edit draft (the studio's own edits).
    pub(crate) fn save(&mut self, pieces: Vec<MotionPiece>) -> Result<(), String> {
        recording::write_pieces(&self.recording, &pieces)?;
        self.reload();
        Ok(())
    }

    pub(crate) fn pieces(&self) -> Vec<MotionPiece> {
        self.items.iter().map(|p| p.piece.clone()).collect()
    }

    pub(crate) fn get(&self, id: &str) -> Option<&LoadedPiece> {
        self.items.iter().find(|p| p.piece.id == id)
    }

    pub(crate) fn any_active(&self, t: f64, placement: Placement) -> bool {
        self.items.iter().any(|p| p.active_at(t) && p.piece.placement == placement)
    }

    /// Asks for the frames at editor time `t`: full-screen pieces `width`
    /// pixels across the preview canvas, over-the-video pieces into a
    /// `media` sized layer. A no-op when nothing changed.
    pub(crate) fn request(&mut self, t: f64, width: f64, media: Option<(u32, u32)>, proxy: bool) {
        let jobs: Vec<PieceJob> = self
            .items
            .iter()
            .filter(|p| p.active_at(t) && (p.piece.placement == Placement::Full || media.is_some()))
            .map(|p| {
                let scale = match (p.piece.placement, media) {
                    (Placement::Overlay, Some((w, h))) => fit(Placement::Overlay, p.size(), (w as f64, h as f64)).0,
                    _ => width / p.size().0 * if proxy { 0.5 } else { 1.0 },
                };
                PieceJob {
                    dir: p.dir.clone(),
                    compiled: p.compiled.clone(),
                    t: t - p.piece.start,
                    scale: scale.clamp(0.05, 1.0),
                    opacity: p.piece.opacity as f32,
                    placement: p.piece.placement,
                    background: rgba8(p.project.settings.background),
                    blur: !proxy,
                }
            })
            .collect();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        media.hash(&mut hasher);
        for j in &jobs {
            j.dir.hash(&mut hasher);
            (Arc::as_ptr(&j.compiled) as usize).hash(&mut hasher);
            j.t.to_bits().hash(&mut hasher);
            j.scale.to_bits().hash(&mut hasher);
            j.opacity.to_bits().hash(&mut hasher);
            (j.placement as u8).hash(&mut hasher);
        }
        let signature = hasher.finish() | 1;
        if signature == self.last_request {
            return;
        }
        self.last_request = signature;
        self.request_key += 1;
        self.worker.request(Request { key: self.request_key, jobs, media });
    }

    /// True when new frames are waiting for `take_frames`.
    pub(crate) fn frames_ready(&self) -> bool {
        self.worker.ready()
    }

    /// Swaps in finished frames; returns the replaced GPU images for release.
    pub(crate) fn take_frames(&mut self) -> Vec<Arc<RenderImage>> {
        match self.worker.take() {
            Some(done) if done.key <= self.request_key => {
                self.media_frame = done.media.map(|m| (done.key, Arc::new(m)));
                std::mem::replace(&mut self.frames, done.canvas).into_iter().map(|f| f.image).collect()
            }
            _ => Vec::new(),
        }
    }

    /// The compositor's media overlay at `t`: the over-the-video pieces
    /// under `annotations` (signature, layer). Unchanged when none plays.
    pub(crate) fn media_overlay(&mut self, t: f64, annotations: Option<(u64, Arc<RgbaImage>)>) -> Option<(u64, Arc<RgbaImage>)> {
        if !self.any_active(t, Placement::Overlay) {
            return annotations;
        }
        let Some((key, layer)) = self.media_frame.clone() else { return annotations };
        let cache_key = (key, annotations.as_ref().map_or(0, |(s, _)| *s));
        if let Some((cached, merged)) = &self.merged {
            if *cached == cache_key {
                return Some((key.rotate_left(32) ^ cache_key.1 ^ 0x6d6f74696f6e, merged.clone()));
            }
        }
        let merged = match annotations {
            None => layer,
            Some((_, a)) => {
                let mut base = (*layer).clone();
                let a = if a.dimensions() == base.dimensions() { (*a).clone() } else { image::imageops::resize(&*a, base.width(), base.height(), image::imageops::FilterType::Triangle) };
                over(&mut base, &a, 0, 0, 1.0);
                Arc::new(base)
            }
        };
        self.merged = Some((cache_key, merged.clone()));
        Some((key.rotate_left(32) ^ cache_key.1 ^ 0x6d6f74696f6e, merged))
    }

    /// Every piece's sound, placed on the video timeline.
    pub(crate) fn timeline_audio(&self) -> Vec<(PathBuf, AudioClip)> {
        let mut out = Vec::new();
        for p in self.items.iter().filter(|p| !p.piece.hidden) {
            for (path, clip) in &p.audio {
                let mut clip = clip.clone();
                let end = clip.start + clip.duration;
                clip.start += p.piece.start;
                clip.duration = end.min(p.duration()) - (clip.start - p.piece.start);
                if clip.duration > 0.01 {
                    clip.id = uuid::Uuid::new_v4();
                    out.push((path.clone(), clip));
                }
            }
        }
        out
    }

    fn placed(&self, placement: Placement) -> Vec<LoadedPiece> {
        self.items.iter().filter(|p| !p.piece.hidden && p.piece.placement == placement).cloned().collect()
    }

    /// Export, canvas side: full-screen pieces at `height` pixels on the
    /// `aspect` canvas, under `inner` (the canvas annotations).
    pub(crate) fn export_canvas_overlay(&self, inner: Option<OverlaySource>, height: u32, aspect: f64) -> Option<OverlaySource> {
        let items = self.placed(Placement::Full);
        if items.is_empty() {
            return inner;
        }
        let height = height.max(2);
        let width = ((height as f64 * aspect).round() as u32).max(2);
        let (ask, answers) = export_thread(items, width, height);
        let mut inner = inner;
        Some(Box::new(move |t| {
            ask.send(t).ok()?;
            let pieces = answers.recv().ok()?;
            under(pieces, inner.as_mut().and_then(|s| s(t)))
        }))
    }

    /// Export, media side: over-the-video pieces in the recording's space
    /// (`media` pixels), under `inner` (the media annotations). The
    /// compositor then applies zooms and camera moves to both.
    pub(crate) fn export_media_overlay(&self, inner: Option<MediaOverlaySource>, media: (u32, u32)) -> Option<MediaOverlaySource> {
        let items = self.placed(Placement::Overlay);
        if items.is_empty() {
            return inner;
        }
        let (ask, answers) = export_thread(items, media.0.max(2), media.1.max(2));
        let mut inner = inner;
        Some(Box::new(move |t, viewport| {
            ask.send(t).ok()?;
            let pieces = answers.recv().ok()?;
            under(pieces, inner.as_mut().and_then(|s| s(t, viewport)))
        }))
    }
}

type OverlaySource = crate::recording::export::OverlaySource;
type MediaOverlaySource = crate::recording::export::MediaOverlaySource;

/// `top` (resized to fit) over `base`; either alone when the other is None.
fn under(base: Option<RgbaImage>, top: Option<Arc<RgbaImage>>) -> Option<Arc<RgbaImage>> {
    match (base, top) {
        (None, top) => top,
        (Some(mut base), top) => {
            if let Some(top) = top {
                let top = if top.dimensions() == base.dimensions() {
                    (*top).clone()
                } else {
                    image::imageops::resize(&*top, base.width(), base.height(), image::imageops::FilterType::Triangle)
                };
                over(&mut base, &top, 0, 0, 1.0);
            }
            Some(Arc::new(base))
        }
    }
}

// ------------------------------------------------------------------ worker

#[derive(Clone)]
struct PieceJob {
    dir: PathBuf,
    compiled: Arc<Compiled>,
    t: f64,
    scale: f64,
    opacity: f32,
    placement: Placement,
    background: [u8; 4],
    blur: bool,
}

struct Request {
    key: u64,
    jobs: Vec<PieceJob>,
    media: Option<(u32, u32)>,
}

struct Done {
    key: u64,
    canvas: Vec<PieceFrame>,
    media: Option<RgbaImage>,
}

#[derive(Default)]
struct Mailbox {
    job: Option<Request>,
    done: Option<Done>,
    quit: bool,
}

/// One preview thread with a renderer per piece folder.
struct Worker {
    shared: Arc<(Mutex<Mailbox>, Condvar)>,
}

impl Worker {
    fn new() -> Self {
        let shared: Arc<(Mutex<Mailbox>, Condvar)> = Arc::default();
        let inner = shared.clone();
        std::thread::Builder::new()
            .name("lahza-motion-pieces".into())
            .spawn(move || {
                let mut renderers: HashMap<PathBuf, Renderer> = HashMap::new();
                loop {
                    let request = {
                        let mut mailbox = inner.0.lock().unwrap();
                        while mailbox.job.is_none() && !mailbox.quit {
                            mailbox = inner.1.wait(mailbox).unwrap();
                        }
                        if mailbox.quit {
                            return;
                        }
                        mailbox.job.take().unwrap()
                    };
                    let mut canvas = Vec::new();
                    let mut media = request.media.map(|(w, h)| RgbaImage::new(w, h));
                    for job in &request.jobs {
                        let mut pixels = render_piece(&mut renderers, job);
                        match (job.placement, media.as_mut()) {
                            (Placement::Overlay, Some(layer)) => {
                                let x = (layer.width() as i64 - pixels.width() as i64) / 2;
                                let y = (layer.height() as i64 - pixels.height() as i64) / 2;
                                over(layer, &pixels, x, y, 1.0);
                            }
                            _ => {
                                // GPUI uploads BGRA.
                                for p in pixels.chunks_exact_mut(4) {
                                    p.swap(0, 2);
                                }
                                canvas.push(PieceFrame { image: Arc::new(RenderImage::new(vec![image::Frame::new(pixels)])), placement: job.placement });
                            }
                        }
                    }
                    let any_media = request.jobs.iter().any(|j| j.placement == Placement::Overlay);
                    inner.0.lock().unwrap().done = Some(Done { key: request.key, canvas, media: media.filter(|_| any_media) });
                }
            })
            .expect("motion pieces thread");
        Self { shared }
    }

    fn request(&self, request: Request) {
        self.shared.0.lock().unwrap().job = Some(request);
        self.shared.1.notify_one();
    }

    fn ready(&self) -> bool {
        self.shared.0.lock().unwrap().done.is_some()
    }

    fn take(&self) -> Option<Done> {
        self.shared.0.lock().unwrap().done.take()
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.shared.0.lock().unwrap().quit = true;
        self.shared.1.notify_one();
    }
}

/// Renders one piece: straight RGBA, opacity applied; a full-frame piece
/// sits on its opaque background (black when the background is clear).
fn render_piece(renderers: &mut HashMap<PathBuf, Renderer>, job: &PieceJob) -> RgbaImage {
    let renderer = renderers.entry(job.dir.clone()).or_insert_with(|| Renderer::new(Some(&job.dir), RenderOptions::default()));
    renderer.set_scale(job.scale);
    renderer.set_motion_blur(job.blur);
    let mut pixels = renderer.render(&job.compiled, job.t);
    if job.placement == Placement::Full {
        let [br, bg, bb, ba] = job.background;
        let bg = if ba == 255 { [br, bg, bb] } else { [0, 0, 0] };
        for p in pixels.chunks_exact_mut(4) {
            let a = p[3] as u32;
            for c in 0..3 {
                p[c] = ((p[c] as u32 * a + bg[c] as u32 * (255 - a)) / 255) as u8;
            }
            p[3] = 255;
        }
    }
    if job.opacity < 1.0 {
        let o = job.opacity.clamp(0.0, 1.0);
        for p in pixels.chunks_exact_mut(4) {
            p[3] = (p[3] as f32 * o).round() as u8;
        }
    }
    pixels
}

/// Straight-alpha `src` over `dst` at (x, y), clipped.
fn over(dst: &mut RgbaImage, src: &RgbaImage, x: i64, y: i64, opacity: f32) {
    let (dw, dh) = (dst.width() as i64, dst.height() as i64);
    let sw = src.width() as usize;
    for sy in 0..src.height() as i64 {
        let ty = y + sy;
        if ty < 0 || ty >= dh {
            continue;
        }
        let srow = &src.as_raw()[sy as usize * sw * 4..(sy as usize + 1) * sw * 4];
        for sx in 0..sw as i64 {
            let tx = x + sx;
            if tx < 0 || tx >= dw {
                continue;
            }
            let s = &srow[sx as usize * 4..sx as usize * 4 + 4];
            let sa = s[3] as f32 / 255.0 * opacity;
            if sa <= 0.0 {
                continue;
            }
            let d = dst.get_pixel_mut(tx as u32, ty as u32);
            let da = d.0[3] as f32 / 255.0;
            let oa = sa + da * (1.0 - sa);
            for c in 0..3 {
                let v = (s[c] as f32 * sa + d.0[c] as f32 * da * (1.0 - sa)) / oa.max(1e-6);
                d.0[c] = v.round().clamp(0.0, 255.0) as u8;
            }
            d.0[3] = (oa * 255.0).round() as u8;
        }
    }
}

/// Where a `pw × ph` piece goes on a `w × h` canvas: overlay pieces fit
/// inside, full-frame pieces cover it. Returns the scale and top-left.
pub(crate) fn fit(placement: Placement, (pw, ph): (f64, f64), (w, h): (f64, f64)) -> (f64, f64, f64) {
    let scale = match placement {
        Placement::Overlay => (w / pw).min(h / ph),
        Placement::Full => (w / pw).max(h / ph),
    };
    (scale, (w - pw * scale) / 2.0, (h - ph * scale) / 2.0)
}

/// The export side: a thread with its own renderers, asked one time at a
/// time, answering with the pieces composited on a `width × height` layer
/// (None when none is active).
fn export_thread(items: Vec<LoadedPiece>, width: u32, height: u32) -> (mpsc::Sender<f64>, mpsc::Receiver<Option<RgbaImage>>) {
    let (ask, asked) = mpsc::channel::<f64>();
    let (answer, answers) = mpsc::channel();
    std::thread::Builder::new()
        .name("lahza-motion-export".into())
        .spawn(move || {
            let mut renderers: HashMap<PathBuf, Renderer> = HashMap::new();
            for t in asked {
                let active: Vec<&LoadedPiece> = items.iter().filter(|p| p.active_at(t)).collect();
                if active.is_empty() {
                    if answer.send(None).is_err() {
                        return;
                    }
                    continue;
                }
                let mut layer = RgbaImage::new(width, height);
                for p in active {
                    let (scale, x, y) = fit(p.piece.placement, p.size(), (width as f64, height as f64));
                    let job = PieceJob {
                        dir: p.dir.clone(),
                        compiled: p.compiled.clone(),
                        t: t - p.piece.start,
                        scale,
                        opacity: 1.0,
                        placement: p.piece.placement,
                        background: rgba8(p.project.settings.background),
                        blur: true,
                    };
                    let frame = render_piece(&mut renderers, &job);
                    over(&mut layer, &frame, x.round() as i64, y.round() as i64, p.piece.opacity as f32);
                }
                if answer.send(Some(layer)).is_err() {
                    return;
                }
            }
        })
        .expect("motion export thread");
    (ask, answers)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_pieces_fit_inside_and_full_pieces_cover() {
        let (s, x, y) = fit(Placement::Overlay, (1920.0, 1080.0), (1000.0, 1000.0));
        assert!((s - 1000.0 / 1920.0).abs() < 1e-9 && x.abs() < 1e-9 && y > 0.0);
        let (s, x, _) = fit(Placement::Full, (1920.0, 1080.0), (1000.0, 1000.0));
        assert!((s - 1000.0 / 1080.0).abs() < 1e-9 && x < 0.0);
    }

    #[test]
    fn straight_alpha_over() {
        let mut dst = RgbaImage::from_pixel(1, 1, image::Rgba([0, 0, 255, 255]));
        let src = RgbaImage::from_pixel(1, 1, image::Rgba([255, 0, 0, 128]));
        over(&mut dst, &src, 0, 0, 1.0);
        let p = dst.get_pixel(0, 0).0;
        assert!((p[0] as i32 - 128).abs() <= 1 && (p[2] as i32 - 127).abs() <= 1 && p[3] == 255, "{p:?}");
    }

    #[test]
    fn pieces_render_for_export_in_their_space_and_their_sound_moves_with_them() {
        use lahza_motion_mcp::tools::Toolbox;
        use serde_json::json;
        let root = std::env::temp_dir().join(format!("lahza-pieces-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let rec = root.join("t.lahzarec");
        std::fs::create_dir_all(&rec).unwrap();
        std::fs::write(rec.join("edit.json"), r#"{"formatVersion": 6}"#).unwrap();
        assert!(std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-f", "lavfi", "-i", "sine=frequency=330:duration=3", "-y"])
            .arg(root.join("tone.wav"))
            .status()
            .unwrap()
            .success());
        let mut tb = Toolbox::new();
        tb.cwd = root.clone();
        let rect = |colour: &str| json!({"ops": [{"op": "add_scene", "id": "s"},
            {"op": "add", "id": "r", "kind": "shape", "shape": "rect", "size": [100, 100], "fill": colour, "transform": {"position": [160, 90]}}]});
        tb.call("project_create", json!({"path": "over", "recording": "t.lahzarec", "width": 320, "height": 180, "duration": 2, "background": "#00000000", "music": "tone.wav"})).unwrap();
        tb.call("edit_apply", rect("#ff0000")).unwrap();
        tb.call("recording_place", json!({"start": 1.0, "placement": "overlay"})).unwrap();
        tb.call("project_create", json!({"path": "full", "recording": "t.lahzarec", "width": 320, "height": 180, "duration": 1, "background": "#0000ff"})).unwrap();
        tb.call("edit_apply", rect("#00ff00")).unwrap();
        tb.call("recording_place", json!({"start": 3.0, "placement": "full"})).unwrap();

        let pieces = MotionPieces::load(&rec);
        assert_eq!(pieces.items.len(), 2, "{:?}", pieces.errors);
        // Over the video: in the recording's space, transparent around it.
        let mut media = pieces.export_media_overlay(None, (320, 180)).unwrap();
        let viewport = crate::recording::viewport::ViewportFrame::default();
        assert!(media(0.5, viewport).is_none(), "not playing yet");
        let layer = media(1.5, viewport).unwrap();
        assert_eq!(layer.get_pixel(160, 90).0, [255, 0, 0, 255]);
        assert_eq!(layer.get_pixel(10, 10).0[3], 0);
        assert!(media(1.5, viewport).is_some() && media(3.5, viewport).is_none(), "full pieces are not in the media layer");
        // Full screen: covers the canvas, on its own background.
        let mut canvas = pieces.export_canvas_overlay(None, 180, 16.0 / 9.0).unwrap();
        assert!(canvas(1.5).is_none());
        let frame = canvas(3.5).unwrap();
        assert_eq!(frame.dimensions(), (320, 180));
        assert_eq!(frame.get_pixel(160, 90).0, [0, 255, 0, 255]);
        assert_eq!(frame.get_pixel(5, 5).0, [0, 0, 255, 255]);
        // The music starts with its piece and ends with it.
        let audio = pieces.timeline_audio();
        assert_eq!(audio.len(), 1);
        assert!((audio[0].1.start - 1.0).abs() < 1e-9 && (audio[0].1.duration - 2.0).abs() < 0.01, "{:?}", audio[0].1);
        std::fs::remove_dir_all(&root).ok();
    }
}
