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
    if !playing {return canvas;}
    bounded_canvas(canvas,1600*900)
}

/// Multiple shutter samples share a fixed playback work budget. Paused
/// previews and exports retain their requested canvas and full sample count.
pub(crate) fn temporal_playback_canvas_size(canvas:(u32,u32), samples:usize)->(u32,u32) {
    bounded_canvas(canvas, (2*1600*900)/samples.max(1) as u64)
}

fn bounded_canvas(canvas:(u32,u32), pixel_budget:u64)->(u32,u32) {
    let area = u64::from(canvas.0) * u64::from(canvas.1);
    if area <= pixel_budget { return canvas; }
    let scale = (pixel_budget as f64 / area as f64).sqrt();
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
    pub presentation_canvas: (u32,u32),
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
    pub motion: Option<Arc<crate::recording::motion_blur::MotionPlan>>,
    /// Nonzero during one continuous edit (a drag, scrub or camera handoff).
    /// Its frames may stand in for each other like playback frames, so
    /// pointer moves faster than a compose no longer cancel every frame.
    pub gesture: u64,
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
        self.same_media(other)
            && (self.style == other.style || (self.gesture != 0 && self.gesture == other.gesture))
            && self.presentation_canvas == other.presentation_canvas && self.source_size == other.source_size
    }

    /// A displayed frame may briefly scale across a playback proxy change;
    /// worker completions must still pass the stricter canvas/generation key.
    pub(crate) fn same_media(&self, other: &Self) -> bool {
        self.media == other.media
    }
}

pub(crate) struct PreviewJob {
    pub playing: bool,
    pub key: PreviewKey,
    pub source: Arc<RgbaImage>,
    pub overlay: Option<Arc<RgbaImage>>,
    pub pointer: Option<PointerOverlay>,
    pub camera: Option<Arc<RgbaImage>>,
    pub screen_video: Option<crate::recording::motion_blur::VideoSource>,
    pub camera_video: Option<crate::recording::motion_blur::VideoSource>,
    pub recording_duration: f64,
    pub temporal_overlay: Option<TemporalOverlay>,
}

/// Authored media marks travel to the render worker once, then animate at
/// each shutter time without rasterizing subframes on the UI thread.
pub(crate) struct TemporalOverlay {
    pub marks: Vec<(crate::AnnotationMark, bool)>,
    pub size: (u32, u32),
}
impl TemporalOverlay {
    fn render(&self, time: f64, viewport: ViewportFrame, crop: crate::CropRect)
        -> Option<Arc<RgbaImage>> {
        let marks: Vec<_> = self.marks.iter().filter_map(|(mark, live)| {
            crate::timed::editor_mark(mark, time, *live).map(|mark|
                crate::timed::in_cropped_media_space(mark, viewport, crop))
        }).collect();
        if marks.is_empty() { return None; }
        crate::scene_ui::render_annotation_layer(&marks, self.size.0, self.size.1).map(Arc::new)
    }
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
enum ComposeFailure { Failed(String), Superseded }

pub(crate) struct PreviewWorker {
    mailbox: Arc<(Mutex<Mailbox>, Condvar)>,
}

impl PreviewWorker {
    pub(crate) fn new() -> std::io::Result<Self> {
        let mut cache: Option<(SceneStyle, (u32, u32), (u32, u32), SceneCompositor)> = None;
        let mut renderer = None;
        let mut screen_frames = crate::recording::motion_blur::FrameCache::default();
        let mut camera_frames = crate::recording::motion_blur::FrameCache::default();
        Self::spawn_in_checked(move |job, cancelled| {
            let mut render_error=None;
            let mut superseded=false;
            let output=(|| {
            let key = &job.key;
            if let Some(plan) = key.motion.as_ref() {
                use crate::recording::{motion_blur::Accumulator, scene::gpu::RenderQuality};
                let temporal = plan.samples.len() > 1;
                if !temporal && (job.playing || key.high_quality) && key.style.animation.blur_amount() > 0.0 {
                    if let (Some(video), Some(time)) = (&job.screen_video, plan.samples.first().and_then(|sample| sample.source_time)) {
                        screen_frames.warm(video, time).ok()?;
                    }
                    if let (Some(video), Some(time)) = (&job.camera_video, plan.samples.first().and_then(|sample| sample.source_time)) {
                        camera_frames.warm(video, time).ok()?;
                    }
                }
                #[cfg(test)] let trace_started=std::time::Instant::now();
                let renderer=renderer.get_or_insert_with(crate::recording::scene::gpu::Renderer::shared);
                let mut accumulator:Option<Accumulator>=None;
                // A live shutter is one transaction. Retain every exact screen
                // and camera picture before drawing its first sample, so the
                // transport cannot evict a later sample during GPU work. The
                // playing planner caps this vector at six Arc pairs; repeated
                // intervals share pixels. Paused/export sampling stays exact
                // and does not retain an additional high-quality frame batch.
                let mut retained=Vec::new();
                if temporal && job.playing {
                    let mut screens=Vec::with_capacity(plan.samples.len());
                    for sample in &plan.samples {
                        if cancelled() {return None;}
                        let source=match (&job.screen_video,sample.source_time) {
                            (Some(video),Some(time))=>match screen_frames.playing_frame(video,time,&||cancelled()).ok()? {
                                Some(frame)=>frame,
                                None=>{superseded=true;return None;}
                            },
                            _=>job.source.clone(),
                        };
                        screens.push(source);
                    }
                    // Pin screen buffers before any independent camera seek;
                    // camera preroll must not age later screen samples out of
                    // the transport ring while this batch is being prepared.
                    for (sample,source) in plan.samples.iter().zip(screens) {
                        if cancelled() {return None;}
                        let camera=match (&job.camera_video,sample.source_time) {
                            (Some(video),Some(time)) if sample.camera.opacity()>0.0 && time<job.recording_duration
                                && camera_frames.contains_time(video,time).ok()?=>
                                    Some(camera_frames.frame(video,time,&||cancelled()).ok()?),
                            _=>None,
                        };
                        retained.push((source,camera));
                    }
                }
                for (index,sample) in plan.samples.iter().enumerate() {
                    if cancelled() { return None; }
                    let (source,camera)=if let Some((source,camera))=retained.get(index) {
                        (source.clone(),camera.clone())
                    } else {
                        let source=if temporal {
                            match (&job.screen_video,sample.source_time) {
                                (Some(video),Some(time))=>screen_frames.frame(video,time,&||cancelled()).ok()?,
                                _=>job.source.clone(),
                            }
                        } else {job.source.clone()};
                        let camera_in_range=match (&job.camera_video,sample.source_time) {
                            (Some(video),Some(time))=>time<job.recording_duration && camera_frames.contains_time(video,time).ok()?,
                            _=>false,
                        };
                        let camera=if sample.camera.opacity()>0.0 && camera_in_range {
                            if temporal {match (&job.camera_video,sample.source_time) {
                                (Some(video),Some(time))=>Some(camera_frames.frame(video,time,&||cancelled()).ok()?),
                                _=>job.camera.clone(),
                            }} else {job.camera.clone()}
                        } else {None};
                        (source,camera)
                    };
                    let mut style=key.style.clone();style.camera=sample.camera;
                    style.pointer.motion_blur_amount=Some(if temporal {0.0}else{style.animation.blur_amount()});
                    let size=source.dimensions();
                    if cache.as_ref().is_none_or(|(old,canvas,source,_)|*old!=style || *canvas!=key.canvas || *source!=size) {
                        let scene=match cache.as_ref(){
                            Some((_,_,_,previous))=>previous.rebuild(&style,key.canvas.0,key.canvas.1,size.0,size.1),
                            None=>SceneCompositor::new(&style,key.canvas.0,key.canvas.1,size.0,size.1),
                        }.ok()?;
                        cache=Some((style,key.canvas,size,scene));
                    }
                    let sampled_overlay = job.temporal_overlay.as_ref().and_then(|marks|
                        marks.render(sample.time, cache.as_ref()?.3.camera_viewport(sample.viewport), key.style.source_crop));
                    let overlay = if job.temporal_overlay.is_some() { sampled_overlay.as_deref() }
                        else { job.overlay.as_deref() };
                    let frame=renderer.compose_with_quality(&cache.as_ref()?.3,FrameInput {
                        source:&source,overlay,viewport:sample.viewport,
                        pointer:sample.pointer.as_ref(),camera:camera.as_deref(),
                    },key.media_visible,None,RenderQuality {enhance:key.high_quality,
                        motion_pixels:plan.motion_pixels,trail_pixels:plan.trail_pixels});
                    if let Some(error)=renderer.take_camera_error() {
                        render_error=Some(error);return None;
                    }
                    if let Some(sum)=&mut accumulator {sum.add(&frame);} else {accumulator=Some(Accumulator::new(frame));}
                }
                #[cfg(test)] if temporal && std::env::var_os("LAHZA_TRACE_TEMPORAL").is_some() {
                    eprintln!("temporal composed t={:.6} samples={} canvas={:?} total={:?}",key.time(),plan.samples.len(),key.canvas,trace_started.elapsed());
                }
                return accumulator.map(|sum|crate::cached_render_image(sum.finish()));
            }
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
            if let Some(error)=renderer.take_camera_error() {render_error=Some(error);return None;}
            Some(crate::cached_render_image(frame))
            })();
            if superseded {return Err(ComposeFailure::Superseded);}
            match render_error {Some(error)=>Err(ComposeFailure::Failed(error)),None=>Ok(output)}
        }, &WORKERS)
    }

    #[cfg(test)]
    fn spawn(compose: impl FnMut(&PreviewJob) -> Option<Arc<RenderImage>> + Send + 'static) -> std::io::Result<Self> {
        Self::spawn_in(compose, &WORKERS)
    }

    #[cfg(test)]
    fn spawn_in(mut compose: impl FnMut(&PreviewJob) -> Option<Arc<RenderImage>> + Send + 'static,
        registry: &WorkerRegistry) -> std::io::Result<Self> {
        Self::spawn_in_cancellable(move |job, _| compose(job), registry)
    }

    #[cfg(test)]
    fn spawn_in_cancellable(mut compose: impl FnMut(&PreviewJob, &dyn Fn() -> bool) -> Option<Arc<RenderImage>> + Send + 'static,
        registry: &WorkerRegistry) -> std::io::Result<Self> {
        Self::spawn_in_checked(move |job,cancelled|Ok(compose(job,cancelled)),registry)
    }

    fn spawn_in_checked(mut compose: impl FnMut(&PreviewJob, &dyn Fn() -> bool) -> Result<Option<Arc<RenderImage>>,ComposeFailure> + Send + 'static,
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
            let cancelled = || {
                let state = shared.0.lock().unwrap_or_else(|error| error.into_inner());
                state.stopped || state.epoch != epoch
            };
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| compose(&job, &cancelled)));
            let mut state = shared.0.lock().unwrap();
            state.active = false;
            if state.stopped { break; }
            if state.epoch == epoch {
                match result {
                    Ok(Err(ComposeFailure::Superseded)) => {}
                    Ok(Ok(Some(image))) => {
                        state.failure = None;
                        state.completed = Some(CompletedPreview { key: job.key, image });
                    }
                    failed => {
                        state.completed = None;
                        state.failure = Some(FailedPreview {
                            key: job.key,
                            message: match failed {
                                Ok(Err(ComposeFailure::Failed(message))) => message,
                                Ok(Ok(None)) => "Could not render the preview. Change the frame or scene to try again.".into(),
                                _ => "Preview rendering failed unexpectedly. Change the frame or scene to try again.".into(),
                            },
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

    #[cfg(test)]
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
    fn pinned_full_split_annotations_share_the_effective_viewport_in_preview_and_export() {
        use crate::recording::scene::{CameraLayout, CameraSplitDirection, SceneBackground};
        let mark = crate::AnnotationMark {
            tool: crate::Tool::FilledRectangle,
            start: crate::NormPoint { x: 0.2, y: 0.2 },
            end: crate::NormPoint { x: 0.4, y: 0.4 },
            pinned: true, color: 0xff0000, stroke_width: 3.0,
            ..Default::default()
        };
        let temporal = TemporalOverlay { marks: vec![(mark.clone(), false)], size: (320, 180) };
        let mut export = crate::scene_ui::overlay_source_for(vec![mark], 320, 180).unwrap();
        let source = RgbaImage::from_pixel(320, 180, image::Rgba([0, 0, 0, 255]));
        for direction in [CameraSplitDirection::Horizontal, CameraSplitDirection::Vertical] {
            let mut style = SceneStyle::default();
            style.background = SceneBackground::Solid(0x000000);
            style.camera.layout = CameraLayout::Split;
            style.camera.split_full = true;
            style.camera.split_direction = direction;
            let scene = SceneCompositor::new(&style, 320, 180, 320, 180).unwrap();
            for zoom in [1.0, 1.8] {
                let raw = ViewportFrame { magnification: zoom, ..Default::default() };
                let effective = scene.camera_viewport(raw);
                assert_ne!(effective.fill, raw.fill);
                let preview = temporal.render(0.5, effective, crate::CropRect::UNIT).unwrap();
                let exported = export(0.5, effective).unwrap();
                assert_eq!(preview, exported, "preview/export share camera cover crop");
                assert_ne!(preview, temporal.render(0.5, raw, crate::CropRect::UNIT).unwrap(),
                    "the original viewport would drift pinned annotations");
                let output = scene.compose(FrameInput { source: &source, overlay: Some(&preview),
                    viewport: raw, pointer: None, camera: None });
                let media = scene.geometry().media;
                let mut red = Vec::new();
                for (x, y, p) in output.enumerate_pixels() {
                    if p[0] > 180 && p[1] < 60 && p[2] < 60 {
                        red.push(((x as f64 - media.x) / media.width,
                            (y as f64 - media.y) / media.height));
                    }
                }
                assert!(!red.is_empty());
                let x = red.iter().map(|p| p.0).sum::<f64>() / red.len() as f64;
                let y = red.iter().map(|p| p.1).sum::<f64>() / red.len() as f64;
                assert!((x - 0.3).abs() < 0.025 && (y - 0.3).abs() < 0.025,
                    "pinned rectangle must stay at 30% of the visible media: {direction:?}, {zoom}, {x},{y}");
            }
        }
    }

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
            playing: true,
            key: PreviewKey {
                style: SceneStyle::default(), canvas: (64, 48), presentation_canvas:(64,48), source_size: (32, 24),
                source: 1, time_bits: time.to_bits(), viewport: ViewportFrame::default(),
                overlay: 0, pointer: false, media_visible: true, generation: 1,
                media: PreviewMedia::Image(1), high_quality: false, motion: None, gesture: 0,
            },
            source: Arc::new(RgbaImage::from_pixel(32, 24, image::Rgba([30, 60, 90, 255]))),
            overlay: None, pointer: None, camera: None,
            screen_video:None,camera_video:None,recording_duration:0.0, temporal_overlay:None,
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
                4 => {next.key.canvas = (80, 60);next.key.presentation_canvas=(80,60);}
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
    fn expired_playback_jobs_drop_without_failure_and_accept_the_next_frame() {
        let registry=WorkerRegistry::new();
        let worker=PreviewWorker::spawn_in_checked(|job,_| {
            if job.key.time()==1.0 {return Err(ComposeFailure::Superseded);}
            Ok(Some(crate::cached_render_image((*job.source).clone())))
        },&registry).unwrap();
        let first=job(1.0);let key=first.key.clone();worker.request(first);
        let until=Instant::now()+Duration::from_secs(2);
        while worker.has_work() {
            assert!(Instant::now()<until);std::thread::sleep(Duration::from_millis(1));
        }
        assert!(worker.take_failure(&key).is_none());
        assert!(worker.take_completed(&key).is_none());
        let next=job(2.0);let key=next.key.clone();worker.request(next);
        assert_eq!(ready(&worker,&key).key.time(),2.0);
        registry.drain();
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
    fn shutter_preview_bounds_total_pixel_work_without_stretching() {
        for canvas in [(1600,900),(900,1600),(2400,600)] {
            for samples in 1..=6 {
                let result=temporal_playback_canvas_size(canvas,samples);
                assert!(result.0<=canvas.0 && result.1<=canvas.1);
                assert!(u64::from(result.0)*u64::from(result.1)*samples as u64<=2*1600*900);
                assert!((result.0 as f64/result.1 as f64-canvas.0 as f64/canvas.1 as f64).abs()<0.005);
            }
        }
        assert_eq!(temporal_playback_canvas_size((1600,900),2),(1600,900));
        let a=job(1.0).key;let mut b=a.clone();b.canvas=(32,24);
        assert!(a.compatible(&b),"adaptive raster size must not starve forward playback");
        b.presentation_canvas=(80,60);
        assert!(!a.compatible(&b),"a real canvas edit still invalidates the worker");
    }

    #[test]
    fn superseded_shutters_and_shutdown_cancel_inflight_decoding() {
        let registry=WorkerRegistry::new();
        let (started, starts)=mpsc::channel();
        let (cancelled_tx, cancelled_rx)=mpsc::channel();
        let worker=PreviewWorker::spawn_in_cancellable(move |job,cancelled| {
            started.send(job.key.time()).unwrap();
            let deadline=Instant::now()+Duration::from_secs(2);
            while !cancelled() {
                assert!(Instant::now()<deadline,"worker did not receive cancellation");
                std::thread::sleep(Duration::from_millis(1));
            }
            cancelled_tx.send(job.key.time()).unwrap();
            None
        },&registry).unwrap();
        worker.request(job(2.0));
        assert_eq!(starts.recv_timeout(Duration::from_secs(2)).unwrap(),2.0);
        worker.request(job(1.0)); // A backward seek invalidates the running shutter.
        assert_eq!(cancelled_rx.recv_timeout(Duration::from_secs(2)).unwrap(),2.0);
        assert_eq!(starts.recv_timeout(Duration::from_secs(2)).unwrap(),1.0);
        registry.drain();
        assert_eq!(cancelled_rx.recv_timeout(Duration::from_secs(2)).unwrap(),1.0);
        assert!(registry.workers.lock().unwrap().is_empty());
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
                input.key.presentation_canvas = canvas;
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
