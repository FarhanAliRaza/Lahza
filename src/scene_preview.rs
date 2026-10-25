//! Bounded CPU playback previews, independent of the UI clock. Oversized
//! canvases use an aspect-preserving proxy; paused previews stay exact.
use crate::recording::scene::{FrameInput, PointerOverlay, SceneCompositor, SceneStyle};
use crate::recording::viewport::ViewportFrame;
use gpui::RenderImage;
use image::RgbaImage;
use std::{path::PathBuf, sync::{Arc, Condvar, Mutex}};

/// Full-frame glass composition measured ~42 ms at 1080p in the development
/// build. A 1600x900 pixel budget targets a 30 fps playback frame budget while
/// leaving normal editor canvases and all paused/export frames unchanged.
pub(crate) fn playback_canvas_size(canvas: (u32, u32), playing: bool) -> (u32, u32) {
    const PIXEL_BUDGET: u64 = 1600 * 900;
    let area = u64::from(canvas.0) * u64::from(canvas.1);
    if !playing || area <= PIXEL_BUDGET { return canvas; }
    let scale = (PIXEL_BUDGET as f64 / area as f64).sqrt();
    let mut width = ((canvas.0 as f64 * scale + 1e-8).floor() as u32).max(1);
    let mut height = ((canvas.1 as f64 * scale + 1e-8).floor() as u32).max(1);
    let aspect = canvas.0 as f64 / canvas.1 as f64;
    // Round the second axis from the first, rather than letting two floor
    // operations stretch very wide or tall frames in opposite directions.
    if width as f64 / height as f64 > aspect {
        width = ((height as f64 * aspect).round() as u32).max(1).min(width);
    } else {
        height = ((width as f64 / aspect).round() as u32).max(1).min(height);
    }
    (width, height)
}

#[derive(Clone, PartialEq)]
pub(crate) enum PreviewMedia {
    Video(PathBuf),
    Image(usize),
}

#[derive(Clone, PartialEq)]
pub(crate) struct PreviewKey {
    pub style: SceneStyle,
    pub canvas: (u32, u32),
    pub source_size: (u32, u32),
    pub source: usize,
    pub time_bits: u64,
    pub viewport: ViewportFrame,
    pub overlay: u64,
    pub pointer: bool,
    pub media_visible: bool,
    pub generation: u64,
    pub media: PreviewMedia,
}

impl PreviewKey {
    pub(crate) fn time(&self) -> f64 { f64::from_bits(self.time_bits) }

    /// Older playback frames are usable, but frames from another edit,
    /// canvas, source, or playback session must never replace the preview.
    pub(crate) fn compatible(&self, other: &Self) -> bool {
        self.same_scene(other) && self.generation == other.generation
    }

    pub(crate) fn same_scene(&self, other: &Self) -> bool {
        self.same_media(other) && self.canvas == other.canvas
    }

    /// A displayed frame may briefly scale across a playback proxy change;
    /// worker completions must still pass the stricter canvas/generation key.
    pub(crate) fn same_media(&self, other: &Self) -> bool {
        self.style == other.style && self.source_size == other.source_size
            && self.media == other.media
    }
}

pub(crate) struct PreviewJob {
    pub key: PreviewKey,
    pub source: Arc<RgbaImage>,
    pub overlay: Option<Arc<RgbaImage>>,
    pub pointer: Option<PointerOverlay>,
    pub camera: Option<Arc<RgbaImage>>,
}

pub(crate) struct CompletedPreview {
    pub key: PreviewKey,
    pub image: Arc<RenderImage>,
}

#[derive(Default)]
struct Mailbox {
    pending: Option<PreviewJob>,
    completed: Option<CompletedPreview>,
    requested: Option<PreviewKey>,
    epoch: u64,
    stopped: bool,
}

/// At most one frame computes, one waits, and one awaits presentation. New
/// requests replace the waiting frame, so playback never builds up latency.
pub(crate) struct PreviewWorker {
    mailbox: Arc<(Mutex<Mailbox>, Condvar)>,
}

impl PreviewWorker {
    pub(crate) fn new() -> std::io::Result<Self> {
        let mut cache: Option<(SceneStyle, (u32, u32), (u32, u32), SceneCompositor)> = None;
        Self::spawn(move |job| {
            let key = &job.key;
            let rebuild = cache.as_ref().is_none_or(|(style, canvas, source, _)| {
                *style != key.style || *canvas != key.canvas || *source != key.source_size
            });
            if rebuild {
                let compositor = match cache.as_ref() {
                    Some((_, _, _, previous)) => previous.rebuild(&key.style, key.canvas.0,
                        key.canvas.1, key.source_size.0, key.source_size.1),
                    None => SceneCompositor::new(&key.style, key.canvas.0, key.canvas.1,
                        key.source_size.0, key.source_size.1),
                }.ok()?;
                cache = Some((key.style.clone(), key.canvas, key.source_size, compositor));
            }
            let frame = cache.as_ref()?.3.compose_layers(FrameInput {
                source: &job.source, overlay: job.overlay.as_deref(), viewport: key.viewport,
                pointer: job.pointer.as_ref(), camera: job.camera.as_deref(),
            }, key.media_visible, None);
            Some(crate::cached_render_image(frame))
        })
    }

    fn spawn(mut compose: impl FnMut(&PreviewJob) -> Option<Arc<RenderImage>> + Send + 'static) -> std::io::Result<Self> {
        let mailbox = Arc::new((Mutex::new(Mailbox::default()), Condvar::new()));
        let shared = mailbox.clone();
        std::thread::Builder::new().name("lahza-scene-preview".into()).spawn(move || loop {
            let (job, epoch) = {
                let (lock, wake) = &*shared;
                let mut state = lock.lock().unwrap();
                while state.pending.is_none() && !state.stopped {
                    state = wake.wait(state).unwrap();
                }
                if state.stopped { break; }
                (state.pending.take().unwrap(), state.epoch)
            };
            // Expensive composition and RGBA -> BGRA conversion never hold
            // the mailbox lock or execute on the application's UI thread.
            let image = compose(&job);
            let mut state = shared.0.lock().unwrap();
            if state.stopped { break; }
            if state.epoch == epoch {
                if let Some(image) = image {
                    state.completed = Some(CompletedPreview { key: job.key, image });
                }
            }
        })?;
        Ok(Self { mailbox })
    }

    pub(crate) fn request(&self, job: PreviewJob) {
        let mut state = self.mailbox.0.lock().unwrap();
        if state.requested.as_ref() == Some(&job.key) { return; }
        if state.requested.as_ref().is_some_and(|old| !old.compatible(&job.key) || job.key.time() < old.time()) {
            state.epoch = state.epoch.wrapping_add(1);
            state.completed = None;
        }
        state.requested = Some(job.key.clone());
        state.pending = Some(job);
        self.mailbox.1.notify_one();
    }

    pub(crate) fn take_completed(&self, current: &PreviewKey) -> Option<CompletedPreview> {
        self.mailbox.0.lock().unwrap().completed.take()
            .filter(|frame| frame.key.compatible(current))
    }

    pub(crate) fn invalidate(&self) {
        let mut state = self.mailbox.0.lock().unwrap();
        if state.requested.take().is_some() {
            state.epoch = state.epoch.wrapping_add(1);
            state.pending = None;
            state.completed = None;
        }
    }
}

impl Drop for PreviewWorker {
    fn drop(&mut self) {
        let mut state = self.mailbox.0.lock().unwrap();
        state.stopped = true;
        state.pending = None;
        state.completed = None;
        self.mailbox.1.notify_one();
        // Do not join: closing a project must not wait for an in-flight frame.
        // The worker owns its inputs and exits as soon as that frame finishes.
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::mpsc, time::{Duration, Instant}};

    #[test]
    fn playback_pixel_budget_preserves_aspect_without_upscaling_or_changing_paused_frames() {
        for canvas in [(320, 180), (960, 540), (900, 1600), (1200, 1200)] {
            assert_eq!(playback_canvas_size(canvas, true), canvas);
        }
        assert_eq!(playback_canvas_size((3840, 2160), true), (1600, 900));
        assert_eq!(playback_canvas_size((2160, 3840), true), (900, 1600));
        for canvas in [(3840, 2160), (2160, 3840), (3001, 3001), (5000, 1000)] {
            let proxy = playback_canvas_size(canvas, true);
            assert!(u64::from(proxy.0) * u64::from(proxy.1) <= 1600 * 900);
            assert!(proxy.0 <= canvas.0 && proxy.1 <= canvas.1);
            let ratio_error = (proxy.0 as f64 / proxy.1 as f64 - canvas.0 as f64 / canvas.1 as f64).abs();
            assert!(ratio_error < 0.005, "{canvas:?} -> {proxy:?}");
            assert_eq!(playback_canvas_size(canvas, false), canvas);
        }
        let original = job(1.0).key;
        let mut next = original.clone();
        next.canvas = (1600, 900);
        next.generation += 1;
        assert!(original.same_media(&next), "keep the displayed frame while the proxy warms up");
        assert!(!original.compatible(&next), "reject stale worker results across that transition");
    }

    fn job(time: f64) -> PreviewJob {
        PreviewJob {
            key: PreviewKey {
                style: SceneStyle::default(), canvas: (64, 48), source_size: (32, 24),
                source: 1, time_bits: time.to_bits(), viewport: ViewportFrame::default(),
                overlay: 0, pointer: false, media_visible: true, generation: 1,
                media: PreviewMedia::Image(1),
            },
            source: Arc::new(RgbaImage::from_pixel(32, 24, image::Rgba([30, 60, 90, 255]))),
            overlay: None, pointer: None, camera: None,
        }
    }

    fn ready(worker: &PreviewWorker, key: &PreviewKey) -> CompletedPreview {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(frame) = worker.take_completed(key) { return frame; }
            assert!(Instant::now() < deadline, "preview did not complete");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn blocked_compositor_keeps_only_the_latest_waiting_frame() {
        let (started, starts) = mpsc::channel();
        let (permit, permits) = mpsc::channel();
        let worker = PreviewWorker::spawn(move |job| {
            started.send(job.key.time()).unwrap();
            permits.recv().unwrap();
            Some(crate::cached_render_image((*job.source).clone()))
        }).unwrap();
        worker.request(job(1.0));
        assert_eq!(starts.recv_timeout(Duration::from_secs(2)).unwrap(), 1.0);
        for time in 2..=100 { worker.request(job(time as f64)); }
        permit.send(()).unwrap();
        assert_eq!(starts.recv_timeout(Duration::from_secs(2)).unwrap(), 100.0);
        assert_eq!(ready(&worker, &job(100.0).key).key.time(), 1.0,
            "a ready frame must remain presentable while a newer frame computes");
        permit.send(()).unwrap();
        assert_eq!(ready(&worker, &job(100.0).key).key.time(), 100.0);
        assert!(starts.try_recv().is_err(), "intermediate jobs must not be queued");
    }

    #[test]
    fn pause_restart_project_style_size_and_loop_changes_reject_inflight_results() {
        for change in 0..6 {
            let (started, starts) = mpsc::channel();
            let (permit, permits) = mpsc::channel();
            let worker = PreviewWorker::spawn(move |job| {
                started.send(()).unwrap();
                permits.recv().unwrap();
                Some(crate::cached_render_image((*job.source).clone()))
            }).unwrap();
            worker.request(job(1.0));
            starts.recv_timeout(Duration::from_secs(2)).unwrap();
            let mut next = job(2.0);
            match change {
                0 => worker.invalidate(),
                1 => next.key.generation += 1,
                2 => next.key.media = PreviewMedia::Video(PathBuf::from("another-project")),
                3 => next.key.style.padding += 1,
                4 => next.key.canvas = (80, 60),
                _ => next.key.time_bits = 0.0f64.to_bits(),
            }
            let key = next.key.clone();
            worker.request(next);
            permit.send(()).unwrap();
            starts.recv_timeout(Duration::from_secs(2)).unwrap();
            assert!(worker.take_completed(&key).is_none(), "stale result survived change {change}");
            permit.send(()).unwrap();
            assert_eq!(ready(&worker, &key).key.time(), key.time());
        }
    }

    #[test]
    fn dropping_preview_does_not_wait_for_a_busy_compositor() {
        let (started, starts) = mpsc::channel();
        let (permit, permits) = mpsc::channel();
        let (finished, finishes) = mpsc::channel();
        let worker = PreviewWorker::spawn(move |_| {
            started.send(()).unwrap();
            permits.recv().unwrap();
            finished.send(()).unwrap();
            None
        }).unwrap();
        worker.request(job(1.0));
        starts.recv_timeout(Duration::from_secs(2)).unwrap();
        let start = Instant::now();
        drop(worker);
        let elapsed = start.elapsed();
        permit.send(()).unwrap();
        finishes.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(elapsed < Duration::from_millis(100), "drop waited for rendering: {elapsed:?}");
    }

    #[test]
    fn background_composition_matches_the_exact_paused_frame() {
        let worker = PreviewWorker::new().unwrap();
        let mut input = job(1.25);
        input.key.viewport.loupe = Some(crate::recording::viewport::LoupeFrame {
            center: crate::recording::model::NormalizedPoint { x: 0.5, y: 0.5 },
            radius: 0.3, magnification: 1.8, bevel: 0.2, chromatic_aberration: 0.5, presence: 1.0,
        });
        let key = input.key.clone();
        let expected = SceneCompositor::new(&key.style, 64, 48, 32, 24).unwrap()
            .compose_layers(FrameInput { source: &input.source, overlay: None, viewport: key.viewport,
                pointer: None, camera: None }, true, None);
        let expected = crate::cached_render_image(expected);
        worker.request(input);
        let actual = ready(&worker, &key);
        assert_eq!(actual.key.time(), 1.25);
        assert_eq!(actual.image.as_bytes(0), expected.as_bytes(0));
    }

    #[test]
    #[ignore = "manual whole-frame playback composition benchmark"]
    fn benchmark_glass_preview_worker_completion_and_enqueue_cost() {
        let source = Arc::new(RgbaImage::from_fn(1920, 1080, |x, y| {
            image::Rgba([(x % 251) as u8, (y % 241) as u8, ((x + y) % 239) as u8, 255])
        }));
        for canvas in [(960, 540), (1600, 900), (1920, 1080)] {
            let worker = PreviewWorker::new().unwrap();
            let mut totals = Vec::new();
            let mut enqueue = Vec::new();
            for frame in 0..15 {
                let mut input = job(frame as f64 / 60.0);
                input.source = source.clone();
                input.key.canvas = canvas;
                input.key.source_size = source.dimensions();
                input.key.viewport.loupe = Some(crate::recording::viewport::LoupeFrame {
                    center: crate::recording::model::NormalizedPoint { x: 0.3 + frame as f64 * 0.02, y: 0.5 },
                    radius: 0.35, magnification: 1.8, bevel: 0.2, chromatic_aberration: 0.5, presence: 1.0,
                });
                let key = input.key.clone();
                let started = Instant::now();
                worker.request(input);
                let request_time = started.elapsed().as_secs_f64() * 1000.0;
                let completed = ready(&worker, &key);
                assert_eq!(completed.key.time(), key.time());
                if frame >= 3 {
                    enqueue.push(request_time);
                    totals.push(started.elapsed().as_secs_f64() * 1000.0);
                }
            }
            totals.sort_by(f64::total_cmp);
            enqueue.sort_by(f64::total_cmp);
            eprintln!("Glass preview {}x{}: completion median {:.2} ms, p95 {:.2} ms; enqueue median {:.3} ms, max {:.3} ms (12 warm frames, full composition and BGRA conversion)",
                canvas.0, canvas.1, totals[totals.len() / 2], totals[totals.len() * 95 / 100],
                enqueue[enqueue.len() / 2], enqueue[enqueue.len() - 1]);
        }
    }
}
