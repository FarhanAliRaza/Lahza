//! How the motion preview is drawn: rasterised, rasterised with the sky's
//! light, or path traced in the fast mode. One choice for the app, kept
//! with its other settings, read by both preview workers (the motion
//! editor's and the recording's pieces).
//!
//! The fast mode builds each frame on the ones before it. Playing
//! accumulates; a frame that does not follow the last one drawn (a scrub,
//! an edit, a change of size) is warmed up when the preview is still, or
//! starts afresh when it plays. A drag always draws the raster proxy.

use lahza_motion_core::compile::Compiled;
use lahza_motion_render::pt::TraceOptions;
use lahza_motion_render::Renderer;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

/// Frames drawn and thrown away before a still frame with no history
/// (what `snapshot` uses by default).
pub(crate) const WARM_FRAMES: u32 = 8;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) enum Quality {
    #[default]
    Raster,
    /// Rasterised meshes lit by how much of the sky each point sees.
    RasterSky,
    /// Path traced, the fast mode.
    TracedFast,
}

impl Quality {
    pub(crate) const ALL: [Quality; 3] = [Quality::Raster, Quality::RasterSky, Quality::TracedFast];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Quality::Raster => "Raster",
            Quality::RasterSky => "Raster + sky",
            Quality::TracedFast => "Traced",
        }
    }

    pub(crate) fn hint(self) -> &'static str {
        match self {
            Quality::Raster => "Rasterised preview (the quickest)",
            Quality::RasterSky => "Rasterised, with meshes lit by how much sky each point sees (needs a GPU with ray queries)",
            Quality::TracedFast => "Path traced, fast: builds up while playing, warms up after a scrub or an edit (needs a GPU with ray queries); a drag draws the raster proxy",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Quality::Raster => "raster",
            Quality::RasterSky => "raster-sky",
            Quality::TracedFast => "traced-fast",
        }
    }

    /// Whether it needs a GPU with ray queries.
    pub(crate) fn traces(self) -> bool {
        self != Quality::Raster
    }
}

static QUALITY: AtomicU8 = AtomicU8::new(0);

fn path() -> std::path::PathBuf {
    crate::recording::presets::config_root().join("motion-preview")
}

pub(crate) fn quality() -> Quality {
    Quality::ALL[QUALITY.load(Ordering::Relaxed) as usize % 3]
}

fn read(path: &std::path::Path) -> Quality {
    let kept = std::fs::read_to_string(path).unwrap_or_default();
    Quality::ALL.into_iter().find(|q| q.name() == kept.trim()).unwrap_or_default()
}

fn write(path: &std::path::Path, q: Quality) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, q.name());
}

/// Reads the kept choice (at start).
pub(crate) fn load_quality() {
    QUALITY.store(read(&path()) as u8, Ordering::Relaxed);
}

/// Sets the choice and keeps it.
pub(crate) fn set_quality(q: Quality) {
    QUALITY.store(q as u8, Ordering::Relaxed);
    write(&path(), q);
}

/// What is done about the frames before this one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum History {
    /// The frame follows the last one drawn: build on it.
    Keep,
    /// It does not, and the preview plays: start afresh, no waiting.
    Forget,
    /// It does not, and the preview is still: draw this many frames before
    /// it first.
    Warm(u32),
}

/// One frame as a preview worker asks its renderer for it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Ask {
    pub path: Option<TraceOptions>,
    pub sky: bool,
    /// The raster's motion blur (the fast mode's is not asked for).
    pub blur: bool,
    /// Half size.
    pub proxy: bool,
}

/// The options for a frame at `quality` while the preview plays or
/// something is dragged. A drag is always the raster proxy: it never
/// waits for a traced frame.
pub(crate) fn ask(quality: Quality, playing: bool, dragging: bool) -> Ask {
    if dragging {
        return Ask { path: None, sky: false, blur: false, proxy: true };
    }
    let proxy = playing;
    match quality {
        Quality::Raster => Ask { path: None, sky: false, blur: !proxy, proxy },
        Quality::RasterSky => Ask { path: None, sky: true, blur: !proxy, proxy },
        Quality::TracedFast => Ask { path: Some(TraceOptions { fast: true, temporal: true, spp: 0, ..Default::default() }), sky: false, blur: false, proxy },
    }
}

/// The last frame a renderer drew, as far as history goes.
#[derive(Clone)]
pub(crate) struct Drawn {
    compiled: Arc<Compiled>,
    frame: i64,
    scale: f64,
    ask: Ask,
}

/// A playing preview drops the frames its renderer is too slow for: up
/// to this many frames on still count as following.
const PLAY_SKIP: i64 = 8;

/// What to do about history for the frame at `t`, given the last frame
/// drawn. Frames follow when the project, the size and the options are
/// the same and the frame is the same one or a little later: two frames
/// when still (a step), `PLAY_SKIP` when playing.
pub(crate) fn history(last: Option<&Drawn>, compiled: &Arc<Compiled>, t: f64, scale: f64, now: Ask, playing: bool) -> History {
    if now.path.is_none() {
        return History::Keep;
    }
    let frame = compiled.fps().nearest_frame(t);
    let follows = last.is_some_and(|l| Arc::ptr_eq(&l.compiled, compiled) && l.scale == scale && l.ask == now && (0..=if playing { PLAY_SKIP } else { 2 }).contains(&(frame - l.frame)));
    match (follows, playing) {
        (true, _) => History::Keep,
        (false, true) => History::Forget,
        (false, false) => History::Warm(WARM_FRAMES),
    }
}

/// Draws the frame at `t` as asked, and remembers it in `last`. `Err`
/// when the renderer cannot draw at all. The second value says the frame
/// was asked traced (or with the sky) on a GPU that cannot.
pub(crate) fn draw(renderer: &mut Renderer, last: &mut Option<Drawn>, compiled: &Arc<Compiled>, t: f64, scale: f64, now: Ask, playing: bool) -> Result<(image::RgbaImage, bool), String> {
    renderer.set_scale(scale);
    renderer.set_motion_blur(now.blur);
    renderer.set_path(now.path);
    renderer.set_sky(now.sky);
    let step = history(last.as_ref(), compiled, t, scale, now, playing);
    let image = match step {
        History::Keep => renderer.try_render(compiled, t),
        History::Forget => {
            renderer.forget_history();
            renderer.try_render(compiled, t)
        }
        History::Warm(frames) => renderer.try_render_warm(compiled, t, frames),
    };
    match image {
        Ok(image) => {
            *last = Some(Drawn { compiled: compiled.clone(), frame: compiled.fps().nearest_frame(t), scale, ask: now });
            Ok((image, (now.path.is_some() || now.sky) && !renderer.can_trace()))
        }
        Err(e) => {
            *last = None;
            Err(e)
        }
    }
}

/// What to tell the person when the preview could not be drawn as chosen.
pub(crate) const CANNOT_TRACE: &str = "This GPU has no ray queries: the preview is rasterised.";

#[cfg(test)]
mod tests {
    use super::*;
    use lahza_motion_core::document::Project;
    use lahza_motion_core::time::Fps;

    fn compiled() -> Arc<Compiled> {
        let mut p = Project::new(64, 36, Fps::whole(30), 4.0);
        p.scenes.push(lahza_motion_core::document::Scene::new("s1"));
        Arc::new(Compiled::new(&p))
    }

    #[test]
    fn a_drag_is_the_raster_proxy_whatever_is_chosen() {
        for q in Quality::ALL {
            assert_eq!(ask(q, false, true), Ask { path: None, sky: false, blur: false, proxy: true }, "{q:?}");
            assert_eq!(ask(q, true, true), ask(q, false, true));
        }
        // Still: full size; the raster with its motion blur, and the sky
        // or the fast trace when chosen. Playing: half size.
        assert_eq!(ask(Quality::Raster, false, false), Ask { path: None, sky: false, blur: true, proxy: false });
        assert_eq!(ask(Quality::Raster, true, false), Ask { path: None, sky: false, blur: false, proxy: true });
        assert_eq!(ask(Quality::RasterSky, false, false), Ask { path: None, sky: true, blur: true, proxy: false });
        let traced = ask(Quality::TracedFast, false, false);
        assert!(traced.path.is_some_and(|o| o.fast && o.temporal && o.spp == 0) && !traced.sky && !traced.proxy);
        assert!(ask(Quality::TracedFast, true, false).proxy);
    }

    #[test]
    fn playing_accumulates_and_a_scrub_or_an_edit_warms_up() {
        let c = compiled();
        let fps = c.fps();
        let traced = ask(Quality::TracedFast, false, false);
        let playing = ask(Quality::TracedFast, true, false);
        let at = |frame: i64, ask: Ask, scale: f64| Drawn { compiled: c.clone(), frame, scale, ask };
        // Nothing drawn yet: warm up when still, start afresh when playing.
        assert_eq!(history(None, &c, 1.0, 1.0, traced, false), History::Warm(WARM_FRAMES));
        assert_eq!(history(None, &c, 1.0, 0.5, playing, true), History::Forget);
        // The next frame, or the one after (a skipped frame), or the same
        // one again: builds on the last.
        for step in 0..=2 {
            assert_eq!(history(Some(&at(30, playing, 0.5)), &c, fps.time_of(30 + step), 0.5, playing, true), History::Keep, "+{step}");
        }
        // A jump, forwards or back.
        assert_eq!(history(Some(&at(30, playing, 0.5)), &c, fps.time_of(30 + PLAY_SKIP), 0.5, playing, true), History::Keep, "frames dropped while playing");
        assert_eq!(history(Some(&at(30, playing, 0.5)), &c, fps.time_of(31 + PLAY_SKIP), 0.5, playing, true), History::Forget);
        assert_eq!(history(Some(&at(30, playing, 0.5)), &c, fps.time_of(10), 0.5, playing, true), History::Forget, "the loop back to the start");
        assert_eq!(history(Some(&at(30, traced, 1.0)), &c, fps.time_of(29), 1.0, traced, false), History::Warm(WARM_FRAMES));
        assert_eq!(history(Some(&at(30, traced, 1.0)), &c, fps.time_of(90), 1.0, traced, false), History::Warm(WARM_FRAMES));
        // An edit (another compiled project), another size (the pause
        // after playing), or other options (after a drag's raster frames).
        let edited = compiled();
        assert_eq!(history(Some(&at(30, traced, 1.0)), &edited, fps.time_of(30), 1.0, traced, false), History::Warm(WARM_FRAMES));
        assert_eq!(history(Some(&at(30, playing, 0.5)), &c, fps.time_of(30), 1.0, traced, false), History::Warm(WARM_FRAMES));
        assert_eq!(history(Some(&at(30, ask(Quality::TracedFast, false, true), 0.5)), &c, fps.time_of(30), 1.0, traced, false), History::Warm(WARM_FRAMES));
        // The raster keeps nothing: never a warm-up.
        for q in [Quality::Raster, Quality::RasterSky] {
            assert_eq!(history(None, &c, 1.0, 1.0, ask(q, false, false), false), History::Keep);
        }
        assert_eq!(history(Some(&at(30, traced, 1.0)), &c, 2.0, 0.5, ask(Quality::TracedFast, false, true), false), History::Keep, "a drag's frame");
    }

    /// The whole of it on the CPU renderer, which cannot trace: every
    /// choice still draws a frame, says so when tracing was asked, and a
    /// drag does not.
    #[test]
    fn every_choice_draws_and_says_when_it_could_not_trace() {
        let c = compiled();
        let mut renderer = Renderer::new(None, lahza_motion_render::RenderOptions { backend: lahza_motion_render::Backend::Cpu, ..Default::default() });
        let mut last = None;
        for (q, could_not) in [(Quality::Raster, false), (Quality::RasterSky, true), (Quality::TracedFast, true)] {
            let (image, fell_back) = draw(&mut renderer, &mut last, &c, 1.0, 1.0, ask(q, false, false), false).unwrap();
            assert_eq!((image.width(), image.height(), fell_back), (64, 36, could_not), "{q:?}");
            // What was asked reached the renderer.
            let o = renderer.options();
            assert_eq!((o.path.is_some_and(|p| p.fast), o.sky), (q == Quality::TracedFast, q == Quality::RasterSky), "{q:?}");
            let (image, fell_back) = draw(&mut renderer, &mut last, &c, 1.0, 0.5, ask(q, false, true), false).unwrap();
            assert_eq!((image.width(), fell_back), (32, false), "a drag: the half-size raster");
            assert!(renderer.options().path.is_none() && !renderer.options().sky);
        }
        assert!(last.is_some());
    }

    /// The preview's sequence through a renderer that can trace: a still
    /// frame warms up, the next builds on it, a drag is the raster proxy,
    /// and the frame after the drag warms up again. Needs a GPU with ray
    /// queries; on Mesa's software Vulkan run it with
    /// `LAHZA_MOTION_SOFTWARE_GPU=1 ... -- --ignored --nocapture` (frames go
    /// to `$LAHZA_WORKFLOW_ARTIFACTS` when set).
    #[test]
    #[ignore = "requires a GPU with ray queries (lavapipe: LAHZA_MOTION_SOFTWARE_GPU=1)"]
    fn a_traced_preview_warms_up_builds_on_and_gives_way_to_a_drag() {
        use serde_json::json;
        let mut p = Project::new(160, 90, Fps::whole(30), 2.0);
        p.scenes.push(lahza_motion_core::document::Scene::new("s1"));
        let p = lahza_motion_core::edit::apply(&p, &[
            json!({"op": "add", "scene": "s1", "id": "floor", "kind": "mesh", "mesh": {"type": "plane", "size": [900, 900]}, "material": {"preset": "matte", "color": "#d8d0c0"}, "transform": {"position": [80, 70], "rotation_x": 90}}),
            json!({"op": "add", "scene": "s1", "id": "ball", "kind": "mesh", "mesh": {"type": "sphere", "radius": 18}, "material": {"preset": "plastic", "color": "#d03734"},
                "transform": {"position": {"keys": [{"at": 0, "v": [50, 52]}, {"at": 2, "v": [110, 52]}]}}}),
        ])
        .unwrap()
        .project;
        let c = Arc::new(Compiled::new(&p));
        assert!(c.errors.is_empty(), "{:?}", c.errors);
        let allow = std::env::var_os("LAHZA_MOTION_SOFTWARE_GPU").is_some();
        let mut renderer = Renderer::new(None, lahza_motion_render::RenderOptions { backend: lahza_motion_render::Backend::Auto, allow_software_gpu: allow, ..Default::default() });
        if !renderer.can_trace() {
            eprintln!("skipping: no path tracer ({})", renderer.backend_label());
            return;
        }
        let out = std::env::var_os("LAHZA_WORKFLOW_ARTIFACTS").map(std::path::PathBuf::from);
        let fps = c.fps();
        let traced = ask(Quality::TracedFast, false, false);
        let mut last = None;
        let mut step = |name: &str, renderer: &mut Renderer, last: &mut Option<Drawn>, frame: i64, scale: f64, now: Ask, expect: History| {
            assert_eq!(history(last.as_ref(), &c, fps.time_of(frame), scale, now, false), expect, "{name}");
            let t = std::time::Instant::now();
            let (image, fell_back) = draw(renderer, last, &c, fps.time_of(frame), scale, now, false).unwrap();
            println!("{name}: {} x {} in {:.2} s ({})", image.width(), image.height(), t.elapsed().as_secs_f64(), renderer.backend_label());
            assert!(!fell_back, "{name}: {:?}", renderer.warnings());
            if let Some(dir) = &out {
                image.save(dir.join(format!("preview-{name}.png"))).unwrap();
            }
            image
        };
        let still = step("1-still-warmed", &mut renderer, &mut last, 20, 1.0, traced, History::Warm(WARM_FRAMES));
        assert!(renderer.backend_label().contains("path traced, fast"), "{}", renderer.backend_label());
        let next = step("2-next-builds-on-it", &mut renderer, &mut last, 21, 1.0, traced, History::Keep);
        let drag = step("3-drag-raster-proxy", &mut renderer, &mut last, 21, 0.5, ask(Quality::TracedFast, false, true), History::Keep);
        assert_eq!((still.width(), next.width(), drag.width()), (160, 160, 80));
        assert!(!renderer.backend_label().contains("path traced"), "the drag's frame is rasterised");
        let after = step("4-after-the-drag-warmed", &mut renderer, &mut last, 21, 1.0, traced, History::Warm(WARM_FRAMES));
        // With the sky: rasterised, and it can.
        let sky = step("5-raster-with-sky", &mut renderer, &mut last, 21, 1.0, ask(Quality::RasterSky, false, false), History::Keep);
        let raster = step("6-raster", &mut renderer, &mut last, 21, 1.0, ask(Quality::Raster, false, false), History::Keep);
        let differs = |a: &image::RgbaImage, b: &image::RgbaImage| a.pixels().zip(b.pixels()).filter(|(x, y)| x.0.iter().zip(y.0).any(|(p, q)| p.abs_diff(q) > 8)).count();
        assert!(differs(&after, &raster) > 200, "the traced frame is not the raster one");
        assert!(differs(&sky, &raster) > 50, "nor is the one lit by the sky");
        assert!(after.pixels().any(|p| p.0[..3] != [0, 0, 0]));
    }

    #[test]
    fn the_choice_is_kept() {
        let file = std::env::temp_dir().join(format!("lahza-preview-quality-{}", std::process::id())).join("motion-preview");
        assert_eq!(read(&file), Quality::Raster, "nothing kept yet");
        for q in Quality::ALL {
            write(&file, q);
            assert_eq!(read(&file), q);
        }
        std::fs::write(&file, "nonsense").unwrap();
        assert_eq!(read(&file), Quality::Raster);
        std::fs::remove_dir_all(file.parent().unwrap()).ok();
    }
}
