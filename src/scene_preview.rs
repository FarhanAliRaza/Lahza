//! Bounded background previews, independent of the UI clock. Playback uses an
//! aspect-preserving proxy; settled frames use native pixels and GPU enhancement.
use crate::recording::scene::{FrameInput, PointerOverlay, SceneCompositor, SceneStyle};
use crate::recording::viewport::ViewportFrame;
use gpui::RenderImage;
use image::RgbaImage;
use std::{path::PathBuf, sync::{Arc, Condvar, Mutex}, thread::JoinHandle};

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
    pub high_quality: bool,
}

impl PreviewKey {
    pub(crate) fn time(&self) -> f64 { f64::from_bits(self.time_bits) }

    /// Older playback frames are usable, but frames from another edit,
    /// canvas, source, or playback session must never replace the preview.
    pub(crate) fn compatible(&self, other: &Self) -> bool {
        if self.high_quality || other.high_quality { return self == other; }
        self.same_scene(other) && self.generation == other.generation
    }

    pub(crate) fn same_scene(&self, other: &Self) -> bool {
        self.same_media(other) && self.style == other.style
            && self.canvas == other.canvas && self.source_size == other.source_size
    }

    /// A displayed frame may briefly scale across a playback proxy change;
    /// worker completions must still pass the stricter canvas/generation key.
    pub(crate) fn same_media(&self, other: &Self) -> bool {
        self.media == other.media
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

struct FailedPreview {
    key: PreviewKey,
    message: String,
}

#[derive(Default)]
struct Mailbox {
    pending: Option<PreviewJob>,
    completed: Option<CompletedPreview>,
    failure: Option<FailedPreview>,
    requested: Option<PreviewKey>,
    epoch: u64,
    stopped: bool,
    active: bool,
}

struct WorkerThread {
    mailbox: Arc<(Mutex<Mailbox>, Condvar)>,
    handle: JoinHandle<()>,
}

/// Closing a project only signals its worker. Keep the thread until it has
/// actually destroyed its renderer, so application shutdown can join even
/// workers belonging to projects that were closed earlier.
struct WorkerRegistry {
    workers: Mutex<Vec<WorkerThread>>,
}

static WORKERS: WorkerRegistry = WorkerRegistry::new();

impl WorkerRegistry {
    const fn new() -> Self { Self { workers: Mutex::new(Vec::new()) } }

    fn drain(&self) {
        // Registration uses this same lock. No worker needs it to finish;
        // holding it prevents a new thread from escaping the shutdown drain.
        let mut workers = self.workers.lock().unwrap_or_else(|error| error.into_inner());
        for worker in workers.iter() { stop_worker(&worker.mailbox); }
        for worker in workers.drain(..) { join_worker(worker.handle); }
    }
}

fn stop_worker(mailbox: &Arc<(Mutex<Mailbox>, Condvar)>) {
    let mut state = mailbox.0.lock().unwrap_or_else(|error| error.into_inner());
    state.stopped = true;
    state.pending = None;
    state.completed = None;
    state.failure = None;
    mailbox.1.notify_one();
}

fn join_worker(handle: JoinHandle<()>) {
    if handle.join().is_err() {
        eprintln!("Preview worker panicked while shutting down");
    }
}

/// Must run synchronously before GPUI and the process begin GPU teardown.
/// Waiting for mailbox.active is insufficient: renderer destruction happens
/// after that flag is cleared, at the end of the worker closure.
pub(crate) fn shutdown_workers() { WORKERS.drain(); }

/// At most one frame computes, one waits, and one awaits presentation. New
/// requests replace the waiting frame, so playback never builds up latency.
pub(crate) struct PreviewWorker {
    mailbox: Arc<(Mutex<Mailbox>, Condvar)>,
}

impl PreviewWorker {
    pub(crate) fn new() -> std::io::Result<Self> {
        let mut cache: Option<(SceneStyle, (u32, u32), (u32, u32), SceneCompositor)> = None;
        let mut renderer = None;
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
            let scene = &cache.as_ref()?.3;
            let input = FrameInput {
                source: &job.source, overlay: job.overlay.as_deref(), viewport: key.viewport,
                pointer: job.pointer.as_ref(), camera: job.camera.as_deref(),
            };
            let renderer = renderer.get_or_insert_with(crate::recording::scene::gpu::Renderer::shared);
            let frame = if key.high_quality {
                renderer.compose(scene, input, key.media_visible, None)
            } else {
                renderer.compose_fast(scene, input, key.media_visible, None)
            };
            Some(crate::cached_render_image(frame))
        })
    }

    fn spawn(compose: impl FnMut(&PreviewJob) -> Option<Arc<RenderImage>> + Send + 'static) -> std::io::Result<Self> {
        Self::spawn_in(compose, &WORKERS)
    }

    fn spawn_in(mut compose: impl FnMut(&PreviewJob) -> Option<Arc<RenderImage>> + Send + 'static,
        registry: &WorkerRegistry) -> std::io::Result<Self> {
        let mut workers = registry.workers.lock().unwrap_or_else(|error| error.into_inner());
        // A finished thread has completed all captured resource destructors.
        // Reap it without making project creation wait for an active worker.
        for index in (0..workers.len()).rev() {
            if workers[index].handle.is_finished() {
                join_worker(workers.swap_remove(index).handle);
            }
        }
        let mailbox = Arc::new((Mutex::new(Mailbox::default()), Condvar::new()));
        let shared = mailbox.clone();
        let handle = std::thread::Builder::new().name("lahza-scene-preview".into()).spawn(move || loop {
            let (job, epoch) = {
                let (lock, wake) = &*shared;
                let mut state = lock.lock().unwrap();
                while state.pending.is_none() && !state.stopped {
                    state = wake.wait(state).unwrap();
                }
                if state.stopped { break; }
                state.active = true;
                (state.pending.take().unwrap(), state.epoch)
            };
            // Expensive composition and RGBA -> BGRA conversion never hold
            // the mailbox lock or execute on the application's UI thread.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| compose(&job)));
            let mut state = shared.0.lock().unwrap();
            state.active = false;
            if state.stopped { break; }
            if state.epoch == epoch {
                match result {
                    Ok(Some(image)) => {
                        state.failure = None;
                        state.completed = Some(CompletedPreview { key: job.key, image });
                    }
                    failed => {
                        state.completed = None;
                        state.failure = Some(FailedPreview {
                            key: job.key,
                            message: match failed {
                                Ok(None) => "Could not render the preview. Change the frame or scene to try again.",
                                _ => "Preview rendering failed unexpectedly. Change the frame or scene to try again.",
                            }.into(),
                        });
                    }
                }
            }
        })?;
        workers.push(WorkerThread { mailbox: mailbox.clone(), handle });
        Ok(Self { mailbox })
    }

    pub(crate) fn request(&self, job: PreviewJob) {
        let mut state = self.mailbox.0.lock().unwrap();
        if state.requested.as_ref() == Some(&job.key) { return; }
        state.failure = None;
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

    /// A failed key stays deduplicated after its error is consumed. A later
    /// frame or edit can recover without a busy retry loop on the same input.
    pub(crate) fn take_failure(&self, current: &PreviewKey) -> Option<String> {
        self.mailbox.0.lock().unwrap().failure.take()
            .filter(|failure| failure.key.compatible(current))
            .map(|failure| failure.message)
    }

    pub(crate) fn invalidate(&self) {
        let mut state = self.mailbox.0.lock().unwrap();
        if state.requested.take().is_some() {
            state.epoch = state.epoch.wrapping_add(1);
            state.pending = None;
            state.completed = None;
            state.failure = None;
        }
    }

    pub(crate) fn has_completed(&self) -> bool {
        let state = self.mailbox.0.lock().unwrap();
        state.completed.is_some() || state.failure.is_some()
    }

    pub(crate) fn has_work(&self) -> bool {
        let state = self.mailbox.0.lock().unwrap();
        state.active || state.pending.is_some() || state.completed.is_some() || state.failure.is_some()
    }

    #[cfg(test)]
    pub(crate) fn is_current(&self, key: &PreviewKey) -> bool {
        self.mailbox.0.lock().unwrap().requested.as_ref() == Some(key)
    }
}

impl Drop for PreviewWorker {
    fn drop(&mut self) {
        stop_worker(&self.mailbox);
        // Do not join: closing a project must not wait for an in-flight frame.
        // The registry joins any remaining threads during application quit.
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
                media: PreviewMedia::Image(1), high_quality: false,
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
    fn settled_previews_reject_stale_playback_seek_and_source_completions() {
        // Production no longer explicitly invalidates the worker on pause.
        // Quality transitions and settled-frame identity must do that work,
        // even when media, style, canvas, and playback generation stay equal.
        for change in 0..4 {
            let (started, starts) = mpsc::channel();
            let (permit, permits) = mpsc::channel();
            let worker = PreviewWorker::spawn(move |job| {
                started.send(()).unwrap();
                permits.recv().unwrap();
                Some(crate::cached_render_image((*job.source).clone()))
            }).unwrap();
            let mut old = job(1.0);
            old.key.high_quality = change != 0;
            worker.request(old);
            starts.recv_timeout(Duration::from_secs(2)).unwrap();

            let mut next = job(1.0);
            next.key.high_quality = change != 3;
            match change {
                0 => {} // Playback finishes; request an exact settled frame.
                1 => next.key.time_bits = 2.0f64.to_bits(),
                2 => next.key.source += 1, // Same time, freshly decoded pixels.
                _ => {} // Playback resumes before settled rendering completes.
            }
            let key = next.key.clone();
            worker.request(next);
            permit.send(()).unwrap();
            starts.recv_timeout(Duration::from_secs(2)).unwrap();
            assert!(worker.take_completed(&key).is_none(),
                "stale completion survived quality/settled change {change}");
            permit.send(()).unwrap();
            assert!(ready(&worker, &key).key == key,
                "the replacement must match the complete requested key");
        }
    }

    #[test]
    fn failed_and_panicked_previews_report_once_stop_polling_and_recover_on_new_input() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        for panic_on_failure in [false, true] {
            let attempts = Arc::new(AtomicUsize::new(0));
            let count = attempts.clone();
            let worker = PreviewWorker::spawn(move |job| {
                count.fetch_add(1, Ordering::SeqCst);
                if job.key.time() == 1.0 {
                    assert!(!panic_on_failure, "synthetic preview compositor failure");
                    return None;
                }
                Some(crate::cached_render_image((*job.source).clone()))
            }).unwrap();
            let mut first = job(1.0);
            first.key.high_quality = true;
            let key = first.key.clone();
            worker.request(first);
            let deadline = Instant::now() + Duration::from_secs(2);
            while !worker.has_completed() {
                assert!(Instant::now() < deadline, "a failed frame must wake the UI poller");
                std::thread::sleep(Duration::from_millis(1));
            }
            assert!(!worker.mailbox.0.lock().unwrap().active,
                "a rendering error must release the active slot");
            assert!(worker.take_completed(&key).is_none());
            assert!(worker.take_failure(&key).is_some());
            assert!(worker.take_failure(&key).is_none(), "report each error once");
            assert!(!worker.has_work(), "the UI must stop polling after consuming the error");

            let mut repeated = job(1.0);
            repeated.key = key;
            worker.request(repeated);
            assert!(!worker.has_work(), "failed input must not cause a busy retry loop");
            assert_eq!(attempts.load(Ordering::SeqCst), 1);

            let mut next = job(2.0);
            next.key.high_quality = true;
            let next_key = next.key.clone();
            worker.request(next);
            assert_eq!(ready(&worker, &next_key).key.time(), 2.0);
            assert_eq!(attempts.load(Ordering::SeqCst), 2);
            assert!(!worker.has_work());
            assert!(worker.take_failure(&next_key).is_none());
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
    fn shutdown_joins_closed_projects_through_renderer_destruction() {
        struct RendererDrop {
            started: mpsc::Sender<()>,
            permit: mpsc::Receiver<()>,
        }
        impl Drop for RendererDrop {
            fn drop(&mut self) {
                self.started.send(()).unwrap();
                self.permit.recv().unwrap();
            }
        }

        let registry = Arc::new(WorkerRegistry::new());
        let (started, starts) = mpsc::channel();
        let (permit, permits) = mpsc::channel();
        let (destroying, destroys) = mpsc::channel();
        let (release_destructor, destructor_permit) = mpsc::channel();
        let renderer = RendererDrop { started: destroying, permit: destructor_permit };
        let worker = PreviewWorker::spawn_in(move |_| {
            let _keep_renderer_alive = &renderer;
            started.send(()).unwrap();
            permits.recv().unwrap();
            None
        }, &registry).unwrap();
        worker.request(job(1.0));
        starts.recv_timeout(Duration::from_secs(2)).unwrap();
        drop(worker); // This project has already closed when app quit begins.

        let (drained, drains) = mpsc::channel();
        let drain = std::thread::spawn(move || {
            registry.drain();
            drained.send(()).unwrap();
        });
        permit.send(()).unwrap();
        destroys.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(matches!(drains.try_recv(), Err(mpsc::TryRecvError::Empty)),
            "shutdown returned while the renderer destructor was still running");
        release_destructor.send(()).unwrap();
        drains.recv_timeout(Duration::from_secs(2)).unwrap();
        drain.join().unwrap();
    }

    #[test]
    fn shutdown_stops_idle_workers_and_finishes_after_a_destructor_panic() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        struct RendererDrop(Arc<AtomicUsize>, bool);
        impl Drop for RendererDrop {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
                assert!(!self.1, "test renderer teardown panic");
            }
        }
        let registry = WorkerRegistry::new();
        let destroyed = Arc::new(AtomicUsize::new(0));
        let workers: Vec<_> = [true, false].into_iter().map(|panic| {
            let renderer = RendererDrop(destroyed.clone(), panic);
            PreviewWorker::spawn_in(move |_| {
                let _keep_renderer_alive = &renderer;
                None
            }, &registry).unwrap()
        }).collect();
        registry.drain();
        assert_eq!(destroyed.load(Ordering::SeqCst), 2);
        assert!(registry.workers.lock().unwrap().is_empty());
        drop(workers);
    }

    #[test]
    fn background_fast_composition_matches_cpu_with_rounding() {
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
        let actual_bytes = actual.image.as_bytes(0).unwrap();
        let expected_bytes = expected.as_bytes(0).unwrap();
        let differences: Vec<_> = actual_bytes.iter().zip(expected_bytes)
            .map(|(a,b)| a.abs_diff(*b)).collect();
        assert_eq!(actual_bytes.len(), expected_bytes.len());
        assert!(differences.iter().all(|difference| *difference <= 2), "GPU and CPU preview differ beyond rounding");
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
