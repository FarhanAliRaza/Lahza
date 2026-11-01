//! Showcase film rendered straight through Lahza's export engine.
//! LAHZA_FILM_PROJECT=<recording.lahzarec> LAHZA_FILM_OUT=<out.mp4>
//!   cargo test --release showcase_film -- --ignored --nocapture

use super::{
    clips::RecordingClipTimeline,
    export::{export_scene, ExportFormat, ExportProgress, SceneExportRequest, SceneSource},
    model::{NormalizedPoint, RecordingSession},
    pointer_timeline::PointerTimeline,
    scene::{SceneBackground, SceneStyle, SceneTransform, WindowFrame},
    video::probe_media,
    viewport::{
        CameraMotion, MotionEasing, MotionStyle, Tilt, TransformMotion, ViewportTimeline, ZoomCue,
        ZoomPresentation,
    },
};
use lahza_annotations::svg::render_svg_layer;
use std::{sync::Arc, time::Instant};

const CLICKS: [(f64, f64, f64); 4] = [
    (3.872, 0.4756, 0.3819),
    (5.984, 0.6020, 0.5118),
    (11.093, 0.3910, 0.4563),
    (13.378, 0.4968, 0.0563),
];
const END: f64 = 17.0;

fn clamp01(v: f64) -> f64 { v.clamp(0.0, 1.0) }
fn progress(t: f64, a: f64, b: f64) -> f64 { clamp01((t - a) / (b - a)) }
fn ease_out(p: f64) -> f64 { 1.0 - (1.0 - p).powi(3) }
fn ease_in_out(p: f64) -> f64 { if p < 0.5 { 4.0 * p * p * p } else { 1.0 - (-2.0 * p + 2.0).powi(3) / 2.0 } }
/// 0 -> 1 over [a, a+fade], hold, 1 -> 0 over [b-fade, b].
fn window(t: f64, a: f64, b: f64, fade: f64) -> f64 {
    ease_out(progress(t, a, a + fade)) * (1.0 - ease_in_out(progress(t, b - fade, b)))
}

fn point(x: f64, y: f64) -> NormalizedPoint { NormalizedPoint { x, y } }

fn camera_cues() -> Vec<ZoomCue> {
    let swung = SceneTransform {
        scale: 0.70, rotation_x: 20.0, rotation_y: -34.0, rotation_z: 5.0,
        perspective: 0.75, position_y: 0.06, ..SceneTransform::IDENTITY
    };
    let mut open = ZoomCue::pinned(0.0, 2.7, 1.0, point(0.5, 0.5));
    open.transform = Some(TransformMotion { start: swung, end: SceneTransform::IDENTITY, keep_end_state: false });
    open.easing = MotionEasing::Cinematic;
    open.skips_easing = true;

    let mut first = ZoomCue::pinned(3.3, 5.5, 2.3, point(CLICKS[0].1, CLICKS[0].2));
    first.easing = MotionEasing::Snappy;
    first.camera_motion = CameraMotion::Focused;

    let mut drift = ZoomCue::pinned(5.75, 10.6, 1.9, point(CLICKS[1].1, CLICKS[1].2));
    drift.motion = MotionStyle::ZoomIn;
    drift.pan_to = Some(point(0.42, 0.34));
    drift.easing = MotionEasing::Cinematic;
    drift.tilt = Some(Tilt { x: 7.0, y: -12.0, z: 0.0 });

    let mut loupe = ZoomCue::pinned(10.85, 12.7, 2.4, point(CLICKS[2].1, CLICKS[2].2));
    loupe.presentation = ZoomPresentation::GlassLoupe;
    loupe.start_animation_early = true;

    let mut top = ZoomCue::pinned(13.0, 14.5, 2.6, point(CLICKS[3].1, 0.12));
    top.easing = MotionEasing::Snappy;

    let away = SceneTransform {
        scale: 0.6, rotation_x: 16.0, rotation_y: 30.0, rotation_z: -4.0,
        perspective: 0.75, position_y: -0.16, ..SceneTransform::IDENTITY
    };
    let mut outro = ZoomCue::pinned(14.8, END, 1.0, point(0.5, 0.5));
    outro.transform = Some(TransformMotion { start: SceneTransform::IDENTITY, end: away, keep_end_state: true });
    outro.easing = MotionEasing::Cinematic;
    outro.skips_easing = true;

    vec![open, first, drift, loupe, top, outro]
}

/// Click ripples in media space: they ride the camera with the screen.
fn ripple_layer(t: f64) -> Option<Arc<image::RgbaImage>> {
    let (w, h) = (1720.0, 720.0);
    let mut shapes = String::new();
    for (time, x, y) in CLICKS {
        let p = (t - time) / 0.75;
        if !(0.0..1.0).contains(&p) { continue; }
        let (cx, cy) = (x * w, y * h);
        for (delay, size, width) in [(0.0, 70.0, 3.0), (0.12, 46.0, 2.0)] {
            let q = clamp01((p - delay) / (1.0 - delay));
            if q <= 0.0 { continue; }
            let r = 4.0 + ease_out(q) * size;
            let a = (1.0 - q).powi(2);
            shapes += &format!(
                r##"<circle cx="{cx:.1}" cy="{cy:.1}" r="{r:.1}" fill="none" stroke="#9fd8ff" stroke-width="{width}" opacity="{a:.3}" filter="url(#g)"/>
<circle cx="{cx:.1}" cy="{cy:.1}" r="{r:.1}" fill="none" stroke="#ffffff" stroke-width="{:.1}" opacity="{a:.3}"/>"##,
                width * 0.45
            );
        }
        let flash = (1.0 - ease_out(clamp01(p * 2.5))) * 0.55;
        shapes += &format!(r##"<circle cx="{cx:.1}" cy="{cy:.1}" r="22" fill="url(#core)" opacity="{flash:.3}"/>"##);
    }
    if shapes.is_empty() { return None; }
    let svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}">
<defs><filter id="g" x="-50%" y="-50%" width="200%" height="200%"><feGaussianBlur stdDeviation="5"/></filter>
<radialGradient id="core"><stop offset="0" stop-color="#ffffff"/><stop offset="0.4" stop-color="#9fd8ff" stop-opacity="0.6"/><stop offset="1" stop-color="#9fd8ff" stop-opacity="0"/></radialGradient></defs>
{shapes}</svg>"##
    );
    render_svg_layer(&svg, w as u32, h as u32).ok().map(Arc::new)
}

fn caption(t: f64, start: f64, text: &str) -> String {
    let a = window(t, start, start + 2.1, 0.35);
    if a <= 0.0 { return String::new(); }
    let rise = (1.0 - ease_out(progress(t, start, start + 0.5))) * 26.0;
    let width = 34.0 + text.len() as f64 * 15.2;
    format!(
        r##"<g opacity="{a:.3}" transform="translate(96 {y:.1})">
<rect x="0" y="-44" width="{width:.0}" height="64" rx="32" fill="#0b0f1c" fill-opacity="0.62" stroke="#ffffff" stroke-opacity="0.14"/>
<circle cx="28" cy="-12" r="6" fill="#7cc4ff"/>
<text x="48" y="-3" font-family="Noto Sans" font-weight="600" font-size="27" fill="#f2f5ff">{text}</text></g>"##,
        y = 990.0 + rise
    )
}

/// Canvas layer: letterbox, opening title, captions, light sweep, end card.
fn canvas_layer(t: f64) -> Option<Arc<image::RgbaImage>> {
    let (w, h) = (1920.0, 1080.0);
    let mut out = String::new();

    // Anamorphic letterbox that opens after the title and closes for the end card.
    let bar = 138.0 * (1.0 - ease_in_out(progress(t, 2.1, 3.0)) + ease_in_out(progress(t, 14.7, 15.6)));
    if bar > 0.5 {
        out += &format!(r##"<rect width="{w}" height="{bar:.1}" fill="#000"/><rect y="{:.1}" width="{w}" height="{bar:.1}" fill="#000"/>"##, h - bar);
    }

    // Opening title: words resolve out of blur while the card swings flat.
    let title = window(t, 0.25, 2.55, 0.5);
    if title > 0.0 {
        let words = [("Every moment,", 0.25, "Noto Sans", "700", "", 92.0, 470.0),
                     ("directed.", 0.75, "P052", "700", "italic", 128.0, 600.0)];
        for (text, start, family, weight, style, size, y) in words {
            let p = ease_out(progress(t, start, start + 0.9));
            let blur = (1.0 - p) * 14.0;
            let spacing = (1.0 - p) * 0.3 * size;
            let lift = (1.0 - p) * 30.0;
            let a = p * title;
            out += &format!(
                r##"<filter id="b{start}" x="-20%" y="-50%" width="140%" height="200%"><feGaussianBlur stdDeviation="{blur:.2}"/></filter>
<g opacity="{a:.3}" transform="translate(0 {lift:.1})">
<text x="960" y="{y}" text-anchor="middle" font-family="{family}" font-weight="{weight}" font-style="{style}" font-size="{size}" letter-spacing="{spacing:.1}" fill="#7cc4ff" opacity="0.55" filter="url(#glow)">{text}</text>
<text x="960" y="{y}" text-anchor="middle" font-family="{family}" font-weight="{weight}" font-style="{style}" font-size="{size}" letter-spacing="{spacing:.1}" fill="#ffffff" filter="url(#b{start})">{text}</text></g>"##
            );
        }
    }

    // Light sweep across the frame as the title clears.
    let sweep = progress(t, 2.35, 3.25);
    if sweep > 0.0 && sweep < 1.0 {
        let x = -700.0 + ease_in_out(sweep) * 3300.0;
        out += &format!(r##"<rect x="{x:.0}" y="-400" width="420" height="1900" fill="url(#sweep)" transform="rotate(18 960 540)"/>"##);
    }

    out += &caption(t, 3.55, "Every click, followed.");
    out += &caption(t, 6.3, "A camera that moves like film.");
    out += &caption(t, 11.0, "A glass loupe for the details.");
    out += &caption(t, 13.2, "Rendered on your GPU.");

    // End card.
    let end = ease_out(progress(t, 15.25, 16.2));
    if end > 0.0 {
        let spacing = (1.0 - end) * 60.0 + 10.0;
        let blur = (1.0 - end) * 10.0;
        let tag = ease_out(progress(t, 15.85, 16.6));
        out += &format!(
            r##"<filter id="eb" x="-30%" y="-60%" width="160%" height="220%"><feGaussianBlur stdDeviation="{blur:.2}"/></filter>
<text x="960" y="560" text-anchor="middle" font-family="Noto Sans" font-weight="700" font-size="150" letter-spacing="{spacing:.1}" fill="#7cc4ff" opacity="{:.3}" filter="url(#glow)">Lahza</text>
<text x="960" y="560" text-anchor="middle" font-family="Noto Sans" font-weight="700" font-size="150" letter-spacing="{spacing:.1}" fill="#ffffff" opacity="{end:.3}" filter="url(#eb)">Lahza</text>
<text x="960" y="{:.1}" text-anchor="middle" font-family="P052" font-style="italic" font-size="44" fill="#d8e2ff" opacity="{tag:.3}">Record a moment. Direct a film.</text>"##,
            end * 0.6,
            640.0 + (1.0 - tag) * 16.0
        );
    }

    if out.is_empty() { return None; }
    let svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}">
<defs><filter id="glow" x="-30%" y="-60%" width="160%" height="220%"><feGaussianBlur stdDeviation="18"/></filter>
<linearGradient id="sweep" x1="0" x2="1"><stop offset="0" stop-color="#fff" stop-opacity="0"/><stop offset="0.5" stop-color="#e8f2ff" stop-opacity="0.16"/><stop offset="1" stop-color="#fff" stop-opacity="0"/></linearGradient></defs>
{out}</svg>"##
    );
    render_svg_layer(&svg, w as u32, h as u32).ok().map(Arc::new)
}

#[test]
#[ignore = "set LAHZA_FILM_PROJECT and LAHZA_FILM_OUT to render the showcase film"]
fn showcase_film() {
    let session = RecordingSession {
        directory: std::env::var_os("LAHZA_FILM_PROJECT").expect("recording directory").into(),
    };
    let destination: std::path::PathBuf = std::env::var_os("LAHZA_FILM_OUT").expect("output path").into();
    let info = probe_media(&session.screen_path()).unwrap();
    let clips = RecordingClipTimeline::full(info.duration);
    let capture = session.read_pointer_capture().unwrap();

    let mut style = SceneStyle::default();
    style.background = SceneBackground::Gradient { colors: [0x0a1024, 0x1b1145, 0x040507], angle_degrees: 135.0 };
    style.background_noise = 22;
    style.vignette = 55;
    style.window_frame = WindowFrame::Off;
    style.padding = 9;
    style.corners = 14;
    style.shadow = 75;
    style.shadow_style = 2;
    style.shadow_color = 0x3d6bff;
    style.aspect = Some(16.0 / 9.0);
    style.enhance_screen = true;

    let pointer = PointerTimeline::build_with_clip_timeline(
        capture.clone(), info.duration, info.width as f64, info.height as f64,
        style.pointer.timeline_options(), Some(&clips),
    );
    let viewport = ViewportTimeline::build(&camera_cues(), &pointer, &clips, &capture);
    let mut request = SceneExportRequest::new(destination.clone(), ExportFormat::Mp4, 1080, style, viewport, END);
    request.frame_rate = 60.0;
    request.motion_blur = true;
    request.include_audio = false;
    request.overlay = Some(Box::new(|time, _viewport| ripple_layer(time)));
    request.canvas_overlay = Some(Box::new(canvas_layer));

    let progress = ExportProgress::default();
    let start = Instant::now();
    export_scene(
        SceneSource::Video { media: session.screen_path(), clips, pointer: Some(pointer), camera: None },
        &mut request,
        &progress,
    )
    .unwrap();
    eprintln!(
        "showcase: {END:.1}s at 1920x1080/60fps in {:.1}s, compositor={:?}, encoder={:?} -> {}",
        start.elapsed().as_secs_f64(), progress.compositor_label(), progress.encoder_label(), destination.display()
    );
}

#[test]
#[ignore = "writes overlay stills for review"]
fn showcase_film_overlay_stills() {
    let dir: std::path::PathBuf = std::env::var_os("LAHZA_FILM_OUT").expect("output dir").into();
    std::fs::create_dir_all(&dir).unwrap();
    for t in [0.6, 1.4, 2.6, 4.0, 7.0, 15.6, 16.8] {
        if let Some(layer) = canvas_layer(t) { layer.save(dir.join(format!("canvas_{t}.png"))).unwrap(); }
    }
    if let Some(layer) = ripple_layer(CLICKS[0].0 + 0.2) { layer.save(dir.join("ripple.png")).unwrap(); }
}
