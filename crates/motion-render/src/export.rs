//! Export: frames are rendered on the GPU (or CPU) and piped as raw RGBA
//! to ffmpeg on a writer thread, so encoding overlaps rendering. MP4 uses
//! NVENC or VAAPI when a short probe encode succeeds (the same selection
//! as `crates/export-encoder`), else libx264. Audio clips are mixed in, with
//! music ducked under narration when `duck_db` is set.

use crate::pt::TraceOptions;
use crate::{RenderOptions, Renderer};
use lahza_motion_core::compile::Compiled;
use lahza_motion_core::document::{AudioClip, Project, RendererKind, Settings};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Quality {
    /// Half resolution, no motion blur, fast encoder settings.
    Draft,
    Final,
}

#[derive(Clone, Debug)]
pub struct ExportOptions {
    pub output: PathBuf,
    pub quality: Quality,
    /// [from, to) seconds; default the whole project.
    pub range: Option<(f64, f64)>,
    pub audio: bool,
    pub render: RenderOptions,
    /// Overrides the project's `renderer` setting (`render --pt`).
    pub renderer: Option<RendererKind>,
    /// Overrides the project's `samples` (paths per pixel).
    pub samples: Option<u32>,
    /// Path tracing: filter the noise (default true).
    pub denoise: bool,
    /// Output size relative to the project's (1 = full; 0.25 for quick
    /// checks), on top of draft's half size.
    pub scale: f64,
    /// Render every Nth frame only (1 = all), played at fps / N, so a long
    /// shot can be checked quickly; the timing stays the project's.
    pub every: u32,
}

/// The path tracer's options for an export, or None to rasterise: the
/// override, else the project's `renderer`; draft quality traces a quarter
/// of the samples (at least 4).
pub fn trace_options(settings: &Settings, quality: Quality, renderer: Option<RendererKind>, samples: Option<u32>) -> Option<TraceOptions> {
    if renderer.or(settings.renderer) != Some(RendererKind::Path) {
        return None;
    }
    let spp = samples.or(settings.samples).unwrap_or(TraceOptions::default().spp).max(1);
    let spp = if quality == Quality::Draft { (spp / 4).max(4).min(spp) } else { spp };
    Some(TraceOptions { spp, ..Default::default() })
}

#[derive(Default)]
pub struct Progress {
    pub done: AtomicU64,
    pub total: AtomicU64,
    pub cancel: AtomicBool,
    pub status: Mutex<String>,
}

impl Progress {
    pub fn fraction(&self) -> f64 {
        let total = self.total.load(Ordering::Relaxed);
        if total == 0 {
            0.0
        } else {
            self.done.load(Ordering::Relaxed) as f64 / total as f64
        }
    }
    fn set_status(&self, s: impl Into<String>) {
        *self.status.lock().unwrap_or_else(|e| e.into_inner()) = s.into();
    }
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct ExportReport {
    pub output: PathBuf,
    pub frames: u64,
    pub seconds: f64,
    pub render_fps: f64,
    pub realtime_factor: f64,
    pub backend: String,
    pub encoder: String,
    pub cached_frames: u64,
    pub warnings: Vec<String>,
    /// Assets that were not drawn (see `Renderer::dropped`): errors.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub dropped: Vec<String>,
    /// With `RenderOptions::profile`: where the time went (`Profile::report`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<serde_json::Value>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Encoder {
    Nvenc,
    Vaapi(PathBuf),
    X264,
}

fn probe(encoder: &Encoder) -> bool {
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-v", "error", "-nostdin"]);
    if let Encoder::Vaapi(dev) = encoder {
        cmd.arg("-vaapi_device").arg(dev);
    }
    cmd.args(["-f", "lavfi", "-i", "color=c=black:s=256x256:r=30,format=rgba"]);
    configure_h264(&mut cmd, encoder, Quality::Draft);
    cmd.args(["-frames:v", "3", "-an", "-f", "null", "-"]).stdout(Stdio::null()).stderr(Stdio::null());
    let Ok(mut child) = cmd.spawn() else { return false };
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if started.elapsed() < Duration::from_secs(5) => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

fn h264_encoder() -> Encoder {
    static CHOICE: OnceLock<Encoder> = OnceLock::new();
    CHOICE
        .get_or_init(|| {
            let mut candidates = vec![Encoder::Nvenc];
            let mut nodes: Vec<PathBuf> = std::fs::read_dir("/dev/dri")
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("renderD")))
                .collect();
            nodes.sort();
            candidates.extend(nodes.into_iter().map(Encoder::Vaapi));
            candidates.into_iter().find(probe).unwrap_or(Encoder::X264)
        })
        .clone()
}

fn configure_h264(cmd: &mut Command, encoder: &Encoder, quality: Quality) {
    let q = if quality == Quality::Draft { "26" } else { "18" };
    match encoder {
        Encoder::Nvenc => {
            cmd.args(["-c:v", "h264_nvenc", "-preset", "p4", "-rc", "vbr", "-cq", q, "-b:v", "0", "-pix_fmt", "yuv420p"]);
        }
        Encoder::Vaapi(_) => {
            cmd.args(["-vf", "format=nv12,hwupload", "-c:v", "h264_vaapi", "-rc_mode", "CQP", "-qp", q]);
        }
        Encoder::X264 => {
            let preset = if quality == Quality::Draft { "ultrafast" } else { "veryfast" };
            cmd.args(["-c:v", "libx264", "-preset", preset, "-crf", q, "-pix_fmt", "yuv420p"]);
        }
    }
}

fn audio_inputs(project: &Project, compiled: &Compiled, dir: &Path, start: f64) -> (Vec<String>, Option<String>) {
    let mut args = Vec::new();
    let mut parts = Vec::new();
    let mut labels = Vec::new();
    let mut index = 1;
    let mut add = |clip: &AudioClip, label: &str, args: &mut Vec<String>, parts: &mut Vec<String>| -> Option<String> {
        let path = dir.join(&clip.path);
        if !path.is_file() {
            return None;
        }
        let at = compiled.resolve(&project.markers, &clip.at).unwrap_or(0.0) - start;
        let skip = clip.trim + (-at).max(0.0);
        args.extend(["-ss".into(), format!("{skip:.6}")]);
        if let Some(length) = clip.length {
            args.extend(["-t".into(), format!("{:.6}", (length - (-at).max(0.0)).max(0.001))]);
        }
        args.extend(["-i".into(), path.to_string_lossy().into_owned()]);
        let delay = (at.max(0.0) * 1000.0).round() as u64;
        let name = format!("{label}{index}");
        // Fades, in the clip's own time (before the delay): what plays is
        // its length, or the rest of the file after the trim.
        let skipped = (-at).max(0.0);
        let mut fades = String::new();
        if clip.fade_in > skipped {
            fades.push_str(&format!("afade=t=in:st=0:d={:.6},", clip.fade_in - skipped));
        }
        if clip.fade_out > 0.0 {
            let total = clip.length.or_else(|| crate::footage::probe_sound(&path).ok().map(|(d, _)| d - clip.trim));
            if let Some(total) = total {
                let played = (total - skipped).max(0.0);
                let fade = clip.fade_out.min(played);
                fades.push_str(&format!("afade=t=out:st={:.6}:d={:.6},", (played - fade).max(0.0), fade.max(0.001)));
            }
        }
        parts.push(format!("[{index}:a]aresample=48000,{fades}adelay={delay}:all=1,volume={:.3}dB[{name}]", clip.gain_db));
        index += 1;
        Some(name)
    };
    let music = project.audio.music.as_ref().and_then(|c| add(c, "m", &mut args, &mut parts).map(|n| (n, c.duck_db)));
    // The voice: the single `vo` clip and/or the narration lines, as one
    // bus (it drives the music's ducking).
    let voices: Vec<String> = project.audio.voice().filter_map(|c| add(c, "v", &mut args, &mut parts)).collect();
    let vo = match voices.len() {
        0 => None,
        1 => voices.into_iter().next(),
        n => {
            let inputs: String = voices.iter().map(|l| format!("[{l}]")).collect();
            parts.push(format!("{inputs}amix=inputs={n}:normalize=0:dropout_transition=0[vbus]"));
            Some("vbus".to_string())
        }
    };
    for clip in &project.audio.sfx {
        if let Some(n) = add(clip, "s", &mut args, &mut parts) {
            labels.push(n);
        }
    }
    // Sound of footage layers.
    for clip in compiled.footage_audio() {
        let path = dir.join(&*clip.src);
        if !crate::footage::probe(&path).is_ok_and(|i| i.has_audio) {
            continue;
        }
        let from = clip.start - start;
        let skip = clip.trim + (-from).max(0.0) * clip.speed;
        let length = (clip.end - clip.start.max(start)).max(0.0) * clip.speed;
        if length <= 0.0 {
            continue;
        }
        args.extend(["-ss".into(), format!("{skip:.6}"), "-t".into(), format!("{length:.6}"), "-i".into(), path.to_string_lossy().into_owned()]);
        // atempo takes 0.5–2 per stage; chain stages for other speeds.
        let mut tempo = Vec::new();
        let mut k = clip.speed.abs().clamp(0.01, 100.0);
        while k > 2.0 {
            tempo.push("atempo=2".to_string());
            k /= 2.0;
        }
        while k < 0.5 {
            tempo.push("atempo=0.5".to_string());
            k *= 2.0;
        }
        if (k - 1.0).abs() > 1e-6 {
            tempo.push(format!("atempo={k:.6}"));
        }
        let delay = (from.max(0.0) * 1000.0).round() as u64;
        let name = format!("f{index}");
        let mut chain = vec!["aresample=48000".to_string()];
        chain.extend(tempo);
        chain.push(format!("adelay={delay}:all=1"));
        chain.push(format!("volume={:.3}dB", clip.gain_db));
        parts.push(format!("[{index}:a]{}[{name}]", chain.join(",")));
        index += 1;
        labels.push(name);
    }
    match (music, &vo) {
        (Some((m, Some(duck))), Some(v)) => {
            // Narration drives a compressor on the music: about `duck` dB.
            let ratio = (10f64.powf(duck.abs() / 20.0)).clamp(2.0, 20.0);
            parts.push(format!("[{v}]asplit=2[vk][vm]"));
            parts.push(format!("[{m}][vk]sidechaincompress=threshold=0.03:ratio={ratio:.2}:attack=30:release=400[md]"));
            labels.push("md".into());
            labels.push("vm".into());
        }
        (m, v) => {
            labels.extend(m.map(|m| m.0));
            labels.extend(v.clone());
        }
    }
    if labels.is_empty() {
        return (Vec::new(), None);
    }
    let inputs: String = labels.iter().map(|l| format!("[{l}]")).collect();
    parts.push(format!("{inputs}amix=inputs={}:normalize=0:dropout_transition=0[aout]", labels.len()));
    (args, Some(parts.join(";")))
}

/// Renders and encodes. Blocks until done; poll `progress` from another thread.
pub fn export(project: &Project, compiled: &Compiled, dir: &Path, options: &ExportOptions, progress: &Progress) -> Result<ExportReport, String> {
    if !compiled.errors.is_empty() {
        return Err(format!("the project has errors; run check: {}", compiled.errors.join("; ")));
    }
    let mut render = options.render;
    render.path = render.path.filter(|_| options.renderer != Some(RendererKind::Raster)).or_else(|| trace_options(&compiled.settings, options.quality, options.renderer, options.samples));
    // Frames go out in order: each builds on the last one's samples.
    render.path = render.path.map(|o| TraceOptions { temporal: true, denoise: o.denoise && options.denoise, ..o });
    if options.quality == Quality::Draft {
        render.scale *= 0.5;
        render.motion_blur = false;
    }
    if options.scale > 0.0 && options.scale.is_finite() {
        render.scale *= options.scale.min(1.0);
    }
    let every = options.every.max(1);
    let mut renderer = Renderer::new(Some(dir), render);
    let fps = compiled.fps();
    let (from, to) = options.range.unwrap_or((0.0, compiled.settings.duration));
    let first = fps.nearest_frame(from.max(0.0));
    let last = fps.nearest_frame(to.min(compiled.settings.duration)).max(first + 1);
    let total = ((last - first) as u64).div_ceil(every as u64);
    progress.total.store(total, Ordering::Relaxed);
    progress.done.store(0, Ordering::Relaxed);
    let w = ((compiled.settings.width as f64 * renderer.options().scale).round() as u32).max(2) & !1;
    let h = ((compiled.settings.height as f64 * renderer.options().scale).round() as u32).max(2) & !1;
    let ext = options.output.extension().and_then(|e| e.to_str()).unwrap_or("mp4").to_ascii_lowercase();
    if let Some(parent) = options.output.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-v", "error", "-nostdin", "-y"]);
    let encoder = if ext == "mp4" || ext == "mov" { Some(h264_encoder()) } else { None };
    if let Some(Encoder::Vaapi(dev)) = &encoder {
        cmd.arg("-vaapi_device").arg(dev);
    }
    cmd.args(["-f", "rawvideo", "-pix_fmt", "rgba", "-s", &format!("{w}x{h}"), "-framerate", &format!("{}/{}", fps.num, fps.den as u64 * every as u64), "-i", "-"]);
    let start_time = fps.time_of(first);
    let (audio_args, audio_filter) = if options.audio && ext != "gif" { audio_inputs(project, compiled, dir, start_time) } else { (Vec::new(), None) };
    cmd.args(&audio_args);
    let encoder_label;
    match ext.as_str() {
        "gif" => {
            cmd.args(["-filter_complex", "[0:v]split[a][b];[a]palettegen=stats_mode=diff[p];[b][p]paletteuse=dither=sierra2_4a"]);
            encoder_label = "GIF".to_string();
        }
        "webm" => {
            if let Some(f) = &audio_filter {
                cmd.args(["-filter_complex", f, "-map", "0:v", "-map", "[aout]", "-c:a", "libopus", "-b:a", "160k"]);
            }
            let crf = if options.quality == Quality::Draft { "40" } else { "30" };
            cmd.args(["-c:v", "libvpx-vp9", "-b:v", "0", "-crf", crf, "-row-mt", "1", "-deadline", "good", "-cpu-used", "4", "-pix_fmt", "yuv420p"]);
            encoder_label = "VP9".to_string();
        }
        _ => {
            if let Some(f) = &audio_filter {
                cmd.args(["-filter_complex", f, "-map", "0:v", "-map", "[aout]", "-c:a", "aac", "-b:a", "192k"]);
            }
            let enc = encoder.clone().unwrap_or(Encoder::X264);
            configure_h264(&mut cmd, &enc, options.quality);
            cmd.args(["-movflags", "+faststart"]);
            encoder_label = match enc {
                Encoder::Nvenc => "H.264 · NVENC".into(),
                Encoder::Vaapi(_) => "H.264 · VAAPI".into(),
                Encoder::X264 => "H.264 · libx264".into(),
            };
        }
    }
    cmd.args(["-t", &format!("{:.6}", total as f64 * fps.frame() * every as f64)]);
    cmd.arg(&options.output).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("cannot start ffmpeg: {e}"))?;
    let mut stdin = child.stdin.take().ok_or("ffmpeg has no stdin")?;
    let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(3);
    let writer = std::thread::spawn(move || -> std::io::Result<()> {
        for frame in rx {
            stdin.write_all(&frame)?;
        }
        Ok(())
    });
    progress.set_status(format!("rendering on {}", renderer.backend_label()));
    let started = Instant::now();
    let mut failed = None;
    for n in (first..last).step_by(every as usize) {
        if progress.cancel.load(Ordering::Relaxed) {
            failed = Some("cancelled".to_string());
            break;
        }
        let frame = renderer.render(compiled, fps.time_of(n));
        let frame = if frame.dimensions() != (w, h) {
            image::imageops::resize(&frame, w, h, image::imageops::FilterType::Triangle)
        } else {
            frame
        };
        let sending = Instant::now();
        if tx.send(frame.into_raw()).is_err() {
            failed = Some("ffmpeg stopped accepting frames".into());
            break;
        }
        if let Some(p) = renderer.profile.as_mut() {
            // Blocks only while the encoder is three frames behind.
            p.stage("wait for the encoder", sending);
        }
        progress.done.fetch_add(1, Ordering::Relaxed);
    }
    drop(tx);
    let write_result = writer.join().map_err(|_| "writer thread panicked".to_string())?;
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    let seconds = started.elapsed().as_secs_f64();
    if let Some(reason) = failed {
        let _ = std::fs::remove_file(&options.output);
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("{reason}{}", if stderr.trim().is_empty() { String::new() } else { format!(": {}", stderr.trim()) }));
    }
    if !output.status.success() || write_result.is_err() {
        return Err(format!("ffmpeg failed: {}", String::from_utf8_lossy(&output.stderr).trim()));
    }
    progress.set_status("done");
    let render_fps = total as f64 / seconds.max(1e-9);
    Ok(ExportReport {
        output: options.output.clone(),
        frames: total,
        seconds,
        render_fps,
        realtime_factor: render_fps / fps.as_f64(),
        backend: renderer.backend_label(),
        encoder: encoder_label,
        cached_frames: renderer.stats.cached,
        warnings: renderer.warnings(),
        dropped: renderer.dropped(),
        profile: renderer.profile.as_ref().map(|p| p.report()),
    })
}
