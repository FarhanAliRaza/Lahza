//! Measuring a reference: `track` follows a target through a video and
//! `fit_curve` turns sampled motion into one of the engine's eases, so a
//! move seen in a reference becomes keys without ad-hoc scripts.
//! `shading` measures how much lighting lives inside shapes, so `compare`
//! can tell a flat replica of a lit shot.

use lahza_motion_core::curve::{Curve, Ease, PowerMode, Preset, PRESETS};
use lahza_motion_core::time::{Duration, Fps, TimeBase};
use serde_json::{json, Value};
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};

fn round(v: f64) -> f64 {
    (v * 10000.0).round() / 10000.0
}

// ---------------------------------------------------------------- track

/// What to follow.
#[derive(Clone, Debug)]
pub enum Target {
    /// Every pixel within `tolerance` (RGB distance, 0–441) of `rgb`,
    /// optionally inside `region` [x, y, w, h] (source pixels).
    Color { rgb: [f64; 3], tolerance: f64, region: Option<[f64; 4]> },
    /// The blob of the colour under the point in the first frame, followed
    /// as the connected region nearest its last centroid.
    Point { x: f64, y: f64, tolerance: f64 },
    /// The picture inside the box in the first frame, followed by template
    /// matching (translation only) within `search` pixels per frame.
    Box { rect: [f64; 4], search: f64 },
}

impl Target {
    pub fn from_json(v: &Value) -> Result<Self, String> {
        let tolerance = v.get("tolerance").and_then(Value::as_f64).unwrap_or(40.0).max(0.0);
        let nums = |key: &str, n: usize| -> Option<Vec<f64>> {
            let a = v.get(key)?.as_array()?;
            let out: Vec<f64> = a.iter().filter_map(Value::as_f64).collect();
            (out.len() == n).then_some(out)
        };
        if let Some(hex) = v.get("color").and_then(Value::as_str) {
            let c = lahza_motion_core::value::Color::parse(hex)?;
            let region = nums("region", 4).map(|r| [r[0], r[1], r[2], r[3]]);
            return Ok(Target::Color { rgb: [c.0[0] * 255.0, c.0[1] * 255.0, c.0[2] * 255.0], tolerance, region });
        }
        if let Some(b) = nums("box", 4) {
            let search = v.get("search").and_then(Value::as_f64).unwrap_or(b[2].max(b[3])).max(1.0);
            return Ok(Target::Box { rect: [b[0], b[1], b[2], b[3]], search });
        }
        if let Some(p) = nums("point", 2) {
            return Ok(Target::Point { x: p[0], y: p[1], tolerance });
        }
        Err("target: {\"color\": \"#ff3355\", \"tolerance\": 40, \"region\"?: [x, y, w, h]}, {\"point\": [x, y]} or {\"box\": [x, y, w, h], \"search\"?: px}".into())
    }
}

pub struct VideoInfo {
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub duration: f64,
}

pub fn probe(path: &Path) -> Result<VideoInfo, String> {
    let out = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries", "stream=width,height,avg_frame_rate,r_frame_rate:format=duration", "-of", "json"])
        .arg(path)
        .output()
        .map_err(|e| format!("cannot run ffprobe: {e}"))?;
    let v: Value = serde_json::from_slice(&out.stdout).map_err(|e| format!("ffprobe: {e}"))?;
    let s = v["streams"].get(0).ok_or_else(|| format!("{} has no video stream", path.display()))?;
    let ratio = |s: &str| -> Option<f64> {
        let (a, b) = s.split_once('/')?;
        let (a, b): (f64, f64) = (a.parse().ok()?, b.parse().ok()?);
        (a > 0.0 && b > 0.0).then_some(a / b)
    };
    let fps = s["avg_frame_rate"].as_str().and_then(ratio).or_else(|| s["r_frame_rate"].as_str().and_then(ratio)).unwrap_or(25.0);
    Ok(VideoInfo {
        width: s["width"].as_u64().unwrap_or(0) as u32,
        height: s["height"].as_u64().unwrap_or(0) as u32,
        fps,
        duration: v["format"]["duration"].as_str().and_then(|d| d.parse().ok()).unwrap_or(0.0),
    })
}

/// One tracked frame, in source pixels and degrees (clockwise on screen).
#[derive(Clone, Debug, Default)]
pub struct Sample {
    pub t: f64,
    pub frame: i64,
    pub found: bool,
    pub x: f64,
    pub y: f64,
    pub bbox: [f64; 4],
    pub area: f64,
    pub angle: f64,
    pub color: [f64; 3],
}

impl Sample {
    pub fn to_json(&self) -> Value {
        if !self.found {
            return json!({"t": round(self.t), "frame": self.frame, "found": false});
        }
        let hex = format!("#{:02x}{:02x}{:02x}", self.color[0].round() as u8, self.color[1].round() as u8, self.color[2].round() as u8);
        json!({"t": round(self.t), "frame": self.frame, "found": true, "x": round(self.x), "y": round(self.y),
               "bbox": self.bbox.map(round), "area": round(self.area), "angle": round(self.angle), "color": hex})
    }
}

/// Image moments of a set of pixels (processing resolution).
#[derive(Default)]
struct Moments {
    n: f64,
    sx: f64,
    sy: f64,
    sxx: f64,
    syy: f64,
    sxy: f64,
    min: [f64; 2],
    max: [f64; 2],
    rgb: [f64; 3],
}

impl Moments {
    fn add(&mut self, x: usize, y: usize, px: &[u8]) {
        let (fx, fy) = (x as f64 + 0.5, y as f64 + 0.5);
        if self.n == 0.0 {
            self.min = [fx - 0.5, fy - 0.5];
            self.max = [fx + 0.5, fy + 0.5];
        }
        self.n += 1.0;
        self.sx += fx;
        self.sy += fy;
        self.sxx += fx * fx;
        self.syy += fy * fy;
        self.sxy += fx * fy;
        self.min = [self.min[0].min(fx - 0.5), self.min[1].min(fy - 0.5)];
        self.max = [self.max[0].max(fx + 0.5), self.max[1].max(fy + 0.5)];
        for c in 0..3 {
            self.rgb[c] += px[c] as f64;
        }
    }

    /// Scaled back to source pixels by `s`.
    fn sample(&self, t: f64, frame: i64, s: f64) -> Sample {
        if self.n == 0.0 {
            return Sample { t, frame, ..Default::default() };
        }
        let (cx, cy) = (self.sx / self.n, self.sy / self.n);
        let mu20 = self.sxx / self.n - cx * cx;
        let mu02 = self.syy / self.n - cy * cy;
        let mu11 = self.sxy / self.n - cx * cy;
        Sample {
            t,
            frame,
            found: true,
            x: cx * s,
            y: cy * s,
            bbox: [self.min[0] * s, self.min[1] * s, (self.max[0] - self.min[0]) * s, (self.max[1] - self.min[1]) * s],
            area: self.n * s * s,
            angle: 0.5 * (2.0 * mu11).atan2(mu20 - mu02).to_degrees(),
            color: self.rgb.map(|c| c / self.n),
        }
    }
}

fn close(px: &[u8], rgb: [f64; 3], tol2: f64) -> bool {
    let d: f64 = (0..3).map(|c| (px[c] as f64 - rgb[c]).powi(2)).sum();
    d <= tol2
}

/// Follows `target` through `video` from `from` to `to` seconds. Frames
/// are decoded by ffmpeg at `width` pixels (default the source width, at
/// most 960); results are in source pixels.
pub fn track(video: &Path, target: &Target, from: f64, to: Option<f64>, width: Option<u32>) -> Result<(VideoInfo, Vec<Sample>), String> {
    let info = probe(video)?;
    if info.width == 0 || info.height == 0 {
        return Err(format!("{}: unknown frame size", video.display()));
    }
    let w = width.unwrap_or(info.width.min(960)).clamp(16, info.width.max(16));
    let h = ((info.height as f64 * w as f64 / info.width as f64 / 2.0).round() as u32 * 2).max(2);
    let s = info.width as f64 / w as f64;
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-v", "error", "-nostdin", "-ss", &format!("{:.6}", from.max(0.0)), "-i"]).arg(video);
    if let Some(to) = to {
        cmd.args(["-t", &format!("{:.6}", (to - from).max(0.0))]);
    }
    let mut child = cmd
        .args(["-vf", &format!("scale={w}:{h}:flags=area"), "-fps_mode", "passthrough", "-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("cannot run ffmpeg: {e}"))?;
    let mut out = child.stdout.take().ok_or("ffmpeg gave no output")?;
    let (wu, hu) = (w as usize, h as usize);
    let mut buf = vec![0u8; wu * hu * 3];
    let mut samples = Vec::new();
    let first_frame = (from.max(0.0) * info.fps).round() as i64;
    // Per-target state.
    let mut seed: Option<[f64; 3]> = None;
    let mut last: Option<(f64, f64)> = None;
    let mut template: Option<(Vec<f32>, usize, usize)> = None;
    let mut i = 0i64;
    while out.read_exact(&mut buf).is_ok() {
        let frame = first_frame + i;
        let t = frame as f64 / info.fps;
        i += 1;
        let sample = match *target {
            Target::Color { rgb, tolerance, region } => {
                let (x0, y0, x1, y1) = match region {
                    Some([x, y, rw, rh]) => (
                        ((x / s).floor().max(0.0) as usize).min(wu),
                        ((y / s).floor().max(0.0) as usize).min(hu),
                        (((x + rw) / s).ceil().max(0.0) as usize).min(wu),
                        (((y + rh) / s).ceil().max(0.0) as usize).min(hu),
                    ),
                    None => (0, 0, wu, hu),
                };
                let tol2 = tolerance * tolerance;
                let mut m = Moments::default();
                for y in y0..y1 {
                    for x in x0..x1 {
                        let px = &buf[(y * wu + x) * 3..][..3];
                        if close(px, rgb, tol2) {
                            m.add(x, y, px);
                        }
                    }
                }
                m.sample(t, frame, s)
            }
            Target::Point { x, y, tolerance } => {
                let (px, py) = (((x / s) as usize).min(wu - 1), ((y / s) as usize).min(hu - 1));
                let rgb = *seed.get_or_insert_with(|| {
                    let mut c = [0.0; 3];
                    let mut n = 0.0;
                    for yy in py.saturating_sub(1)..=(py + 1).min(hu - 1) {
                        for xx in px.saturating_sub(1)..=(px + 1).min(wu - 1) {
                            for k in 0..3 {
                                c[k] += buf[(yy * wu + xx) * 3 + k] as f64;
                            }
                            n += 1.0;
                        }
                    }
                    c.map(|v| v / n)
                });
                let (lx, ly) = last.unwrap_or((px as f64 + 0.5, py as f64 + 0.5));
                let m = blob(&buf, wu, hu, rgb, tolerance * tolerance, (lx, ly));
                let sample = m.sample(t, frame, s);
                if sample.found {
                    last = Some((sample.x / s, sample.y / s));
                }
                sample
            }
            Target::Box { rect, search } => {
                let gray: Vec<f32> = buf.chunks_exact(3).map(|p| 0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32).collect();
                let (bw, bh) = (((rect[2] / s).round() as usize).clamp(2, wu), ((rect[3] / s).round() as usize).clamp(2, hu));
                let (tpl, tw, th) = template.get_or_insert_with(|| {
                    let (bx, by) = (((rect[0] / s).round().max(0.0) as usize).min(wu - bw), ((rect[1] / s).round().max(0.0) as usize).min(hu - bh));
                    last = Some((bx as f64, by as f64));
                    let mut v = Vec::with_capacity(bw * bh);
                    for y in by..by + bh {
                        v.extend_from_slice(&gray[y * wu + bx..y * wu + bx + bw]);
                    }
                    (v, bw, bh)
                });
                let (lx, ly) = last.unwrap_or((0.0, 0.0));
                let r = (search / s).ceil() as i64;
                let ssd = |ox: i64, oy: i64| -> f64 {
                    if ox < 0 || oy < 0 || ox as usize + *tw > wu || oy as usize + *th > hu {
                        return f64::INFINITY;
                    }
                    let mut e = 0.0f64;
                    for y in 0..*th {
                        let row = &gray[(oy as usize + y) * wu + ox as usize..][..*tw];
                        for (a, b) in row.iter().zip(&tpl[y * *tw..(y + 1) * *tw]) {
                            e += ((a - b) * (a - b)) as f64;
                        }
                    }
                    e
                };
                let (cx, cy) = (lx.round() as i64, ly.round() as i64);
                let mut best = (f64::INFINITY, cx, cy);
                for oy in cy - r..=cy + r {
                    for ox in cx - r..=cx + r {
                        let e = ssd(ox, oy);
                        if e < best.0 {
                            best = (e, ox, oy);
                        }
                    }
                }
                if !best.0.is_finite() {
                    Sample { t, frame, ..Default::default() }
                } else {
                    // Sub-pixel: a parabola through the neighbours.
                    let refine = |a: f64, b: f64, c: f64| if a.is_finite() && c.is_finite() && a - 2.0 * b + c > 1e-9 { (0.5 * (a - c) / (a - 2.0 * b + c)).clamp(-0.5, 0.5) } else { 0.0 };
                    let dx = refine(ssd(best.1 - 1, best.2), best.0, ssd(best.1 + 1, best.2));
                    let dy = refine(ssd(best.1, best.2 - 1), best.0, ssd(best.1, best.2 + 1));
                    let (fx, fy) = (best.1 as f64 + dx, best.2 as f64 + dy);
                    last = Some((fx, fy));
                    let mut rgb = [0.0; 3];
                    for y in best.2 as usize..best.2 as usize + *th {
                        for x in best.1 as usize..best.1 as usize + *tw {
                            for k in 0..3 {
                                rgb[k] += buf[(y * wu + x) * 3 + k] as f64;
                            }
                        }
                    }
                    let n = (*tw * *th) as f64;
                    Sample {
                        t,
                        frame,
                        found: true,
                        x: (fx + *tw as f64 / 2.0) * s,
                        y: (fy + *th as f64 / 2.0) * s,
                        bbox: [fx * s, fy * s, *tw as f64 * s, *th as f64 * s],
                        area: n * s * s,
                        angle: 0.0,
                        color: rgb.map(|c| c / n),
                    }
                }
            }
        };
        samples.push(sample);
    }
    let _ = child.kill();
    let _ = child.wait();
    if samples.is_empty() {
        return Err(format!("no frames decoded from {} at {from}s", video.display()));
    }
    Ok((info, samples))
}

/// The 4-connected region of pixels close to `rgb` that holds, or lies
/// nearest to, `near` (processing pixels).
fn blob(buf: &[u8], w: usize, h: usize, rgb: [f64; 3], tol2: f64, near: (f64, f64)) -> Moments {
    let mask: Vec<bool> = buf.chunks_exact(3).map(|p| close(p, rgb, tol2)).collect();
    let mut seed = None;
    let mut best = f64::INFINITY;
    for (i, m) in mask.iter().enumerate() {
        if *m {
            let (x, y) = ((i % w) as f64 + 0.5, (i / w) as f64 + 0.5);
            let d = (x - near.0).powi(2) + (y - near.1).powi(2);
            if d < best {
                best = d;
                seed = Some(i);
            }
        }
    }
    let mut m = Moments::default();
    let Some(seed) = seed else { return m };
    let mut seen = vec![false; mask.len()];
    let mut stack = vec![seed];
    seen[seed] = true;
    while let Some(i) = stack.pop() {
        let (x, y) = (i % w, i / w);
        m.add(x, y, &buf[i * 3..i * 3 + 3]);
        let mut push = |j: usize| {
            if mask[j] && !seen[j] {
                seen[j] = true;
                stack.push(j);
            }
        };
        if x > 0 {
            push(i - 1);
        }
        if x + 1 < w {
            push(i + 1);
        }
        if y > 0 {
            push(i - w);
        }
        if y + 1 < h {
            push(i + w);
        }
    }
    m
}

/// Keys ready for `edit_apply`: position (`scale` × source px + `offset`),
/// scale (relative to the first found frame's size, times `base_scale`) and
/// rotation (degrees relative to the first found frame, unwrapped), one key
/// per frame, at `at + (t − from)` on the timeline.
pub fn keys(samples: &[Sample], from: f64, at: f64, scale: f64, offset: [f64; 2], base_scale: f64, rotate: bool) -> Value {
    let found: Vec<&Sample> = samples.iter().filter(|s| s.found).collect();
    let Some(first) = found.first() else { return json!({}) };
    let when = |s: &Sample| round(at + s.t - from);
    let position: Vec<Value> = found.iter().map(|s| json!({"at": when(s), "v": [round(s.x * scale + offset[0]), round(s.y * scale + offset[1])]})).collect();
    let scale_keys: Vec<Value> = found.iter().map(|s| json!({"at": when(s), "v": round(base_scale * (s.area / first.area.max(1e-9)).sqrt())})).collect();
    let mut out = json!({"transform.position": {"keys": position}, "transform.scale": {"keys": scale_keys}});
    if rotate {
        // Moments give an axis (mod 180°): unwrap to the nearest turn.
        let mut prev = first.angle;
        let mut rotation = Vec::new();
        for s in &found {
            let mut a = s.angle;
            while a - prev > 90.0 {
                a -= 180.0;
            }
            while a - prev < -90.0 {
                a += 180.0;
            }
            prev = a;
            rotation.push(json!({"at": when(s), "v": round(a - first.angle)}));
        }
        out["transform.rotation"] = json!({"keys": rotation});
    }
    out
}

// ------------------------------------------------------------ fit_curve

/// One ease family and its continuous parameters.
#[derive(Clone, Copy, Debug)]
enum Family {
    Linear,
    Power(PowerMode),
    Exp,
    Spring,
    Bezier,
    Bounce(u32),
    Preset(Preset),
}

impl Family {
    /// Parameters are searched in a log space where that keeps them positive.
    fn ease(self, p: &[f64]) -> Ease {
        match self {
            Family::Linear => Ease::Linear,
            Family::Power(mode) => Ease::Power { n: p[0].exp().clamp(1.01, 12.0), mode },
            Family::Exp => Ease::Exp { r: None, tau: Some(Duration::Seconds(p[0].exp().clamp(0.003, 5.0))) },
            Family::Spring => Ease::Spring { zeta: p[0].clamp(0.05, 2.0), hz: p[1].exp().clamp(0.2, 30.0) },
            Family::Bezier => Ease::Bezier([p[0].clamp(0.0, 1.0), p[1].clamp(-1.0, 2.0), p[2].clamp(0.0, 1.0), p[3].clamp(-1.0, 2.0)]),
            Family::Bounce(n) => Ease::Bounce { bounces: n, ratio: p[0].clamp(0.05, 0.95), exponent: p[1].clamp(1.0, 6.0) },
            Family::Preset(preset) => Ease::Preset(preset),
        }
    }

    fn starts(self) -> Vec<Vec<f64>> {
        match self {
            Family::Linear | Family::Preset(_) => vec![vec![]],
            Family::Power(_) => [1.5f64, 2.5, 4.0, 6.0].iter().map(|n| vec![n.ln()]).collect(),
            Family::Exp => [0.03f64, 0.08, 0.2, 0.5].iter().map(|t| vec![t.ln()]).collect(),
            Family::Spring => [(0.3, 2.0f64), (0.5, 4.0), (0.7, 6.0), (0.4, 1.2), (0.9, 3.0)].iter().map(|(z, f)| vec![*z, f.ln()]).collect(),
            Family::Bezier => vec![vec![0.25, 0.1, 0.25, 1.0], vec![0.42, 0.0, 0.58, 1.0], vec![0.0, 0.0, 0.58, 1.0], vec![0.42, 0.0, 1.0, 1.0], vec![0.2, 0.0, 0.0, 1.0]],
            Family::Bounce(_) => vec![vec![0.43, 2.75], vec![0.3, 2.0], vec![0.55, 3.5]],
        }
    }

    fn name(self) -> String {
        match self {
            Family::Linear => "linear".into(),
            Family::Power(PowerMode::In) => "power in".into(),
            Family::Power(PowerMode::Out) => "power out".into(),
            Family::Power(PowerMode::InOut) => "power in-out".into(),
            Family::Exp => "exp".into(),
            Family::Spring => "spring".into(),
            Family::Bezier => "bezier".into(),
            Family::Bounce(n) => format!("bounce ×{n}"),
            Family::Preset(p) => p.info().name.into(),
        }
    }

    /// Named presets read best in a document, then fewer parameters.
    fn simplicity(self) -> u8 {
        match self {
            Family::Preset(_) => 0,
            Family::Linear => 1,
            Family::Power(_) | Family::Exp => 2,
            Family::Spring | Family::Bounce(_) => 3,
            Family::Bezier => 4,
        }
    }
}

/// Least-squares `from`/`to` (v ≈ a(1 − p) + b·p) for fixed timing and
/// ease, and the squared error.
fn solve(samples: &[(f64, f64)], curve: &Curve, t0: f64, dur: f64) -> (f64, f64, f64) {
    let ps: Vec<f64> = samples.iter().map(|(t, _)| curve.progress(t - t0, dur)).collect();
    let (mut spp, mut spq, mut sqq, mut svp, mut svq) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for ((_, v), p) in samples.iter().zip(&ps) {
        let q = 1.0 - p;
        spp += p * p;
        spq += p * q;
        sqq += q * q;
        svp += v * p;
        svq += v * q;
    }
    let det = sqq * spp - spq * spq;
    let (a, b) = if det.abs() > 1e-12 { ((svq * spp - svp * spq) / det, (sqq * svp - spq * svq) / det) } else { (samples[0].1, samples[samples.len() - 1].1) };
    let sse = samples.iter().zip(&ps).map(|((_, v), p)| (v - (a + (b - a) * p)).powi(2)).sum();
    (a, b, sse)
}

fn nelder_mead(f: &dyn Fn(&[f64]) -> f64, start: &[f64], step: &[f64], iterations: usize) -> (Vec<f64>, f64) {
    let n = start.len();
    let mut simplex: Vec<(Vec<f64>, f64)> = vec![(start.to_vec(), f(start))];
    for i in 0..n {
        let mut p = start.to_vec();
        p[i] += step[i];
        let v = f(&p);
        simplex.push((p, v));
    }
    for _ in 0..iterations {
        simplex.sort_by(|a, b| a.1.total_cmp(&b.1));
        let centroid: Vec<f64> = (0..n).map(|i| simplex[..n].iter().map(|s| s.0[i]).sum::<f64>() / n as f64).collect();
        let worst = simplex[n].clone();
        let at = |k: f64| -> Vec<f64> { (0..n).map(|i| centroid[i] + k * (worst.0[i] - centroid[i])).collect() };
        let r = at(-1.0);
        let fr = f(&r);
        if fr < simplex[0].1 {
            let e = at(-2.0);
            let fe = f(&e);
            simplex[n] = if fe < fr { (e, fe) } else { (r, fr) };
        } else if fr < simplex[n - 1].1 {
            simplex[n] = (r, fr);
        } else {
            let c = at(0.5);
            let fc = f(&c);
            if fc < worst.1 {
                simplex[n] = (c, fc);
            } else {
                let best = simplex[0].0.clone();
                for s in simplex.iter_mut().skip(1) {
                    s.0 = (0..n).map(|i| best[i] + 0.5 * (s.0[i] - best[i])).collect();
                    s.1 = f(&s.0);
                }
            }
        }
    }
    simplex.sort_by(|a, b| a.1.total_cmp(&b.1));
    simplex.swap_remove(0)
}

struct Fit {
    family: Family,
    params: Vec<f64>,
    start: f64,
    dur: f64,
    a: f64,
    b: f64,
    sse: f64,
}

/// Fits samples `(t, value)` of one move (values are numbers, or vectors
/// fitted on their progress along the line from the first to the last
/// sample) with each of the engine's eases, searching start, duration and
/// the ease's parameters (Nelder–Mead from several starts; from/to solved
/// by least squares). Returns the best fit with the key to use, then the
/// runners-up. A named preset or simpler ease wins when its error is
/// within 10% (and 0.2% of the travel) of the best.
pub fn fit_curve(samples: &[(f64, Vec<f64>)], fps: Fps) -> Result<Value, String> {
    if samples.len() < 4 {
        return Err("fit_curve needs at least 4 samples".into());
    }
    let dim = samples[0].1.len();
    if dim == 0 || samples.iter().any(|s| s.1.len() != dim || !s.0.is_finite() || s.1.iter().any(|x| !x.is_finite())) {
        return Err("samples are [t, value] with the same kind of value (a number or [x, y, …]) throughout".into());
    }
    let mut samples = samples.to_vec();
    samples.sort_by(|a, b| a.0.total_cmp(&b.0));
    let v0 = samples[0].1.clone();
    let d: Vec<f64> = samples[samples.len() - 1].1.iter().zip(&v0).map(|(b, a)| b - a).collect();
    let dd: f64 = d.iter().map(|x| x * x).sum();
    if dd < 1e-18 {
        return Err("the first and last samples are the same: nothing moves".into());
    }
    let scalar: Vec<(f64, f64)> = samples
        .iter()
        .map(|(t, v)| (*t, if dim == 1 { v[0] } else { v.iter().zip(&v0).zip(&d).map(|((x, a), dx)| (x - a) * dx).sum::<f64>() / dd }))
        .collect();
    let (s0, s1) = (scalar[0].1, scalar[scalar.len() - 1].1);
    let travel = (s1 - s0).abs();
    // Initial timing: where the move leaves 2% and reaches 98%.
    let progress = |v: f64| (v - s0) / (s1 - s0);
    let t_start = scalar.iter().find(|(_, v)| progress(*v) > 0.02).map_or(scalar[0].0, |s| s.0);
    let t_end = scalar.iter().rev().find(|(_, v)| progress(*v) < 0.98).map_or(scalar[scalar.len() - 1].0, |s| s.0);
    let frame = fps.frame();
    let t0_guess = t_start - frame;
    let d_guess = (t_end - t0_guess + frame).max(2.0 * frame);
    let base = TimeBase::new(fps);
    let mut families = vec![Family::Linear, Family::Power(PowerMode::In), Family::Power(PowerMode::Out), Family::Power(PowerMode::InOut), Family::Exp, Family::Spring, Family::Bezier];
    families.extend((1..=4).map(Family::Bounce));
    families.extend(PRESETS.iter().map(|p| Family::Preset(p.preset)));
    let mut fits: Vec<Fit> = Vec::new();
    for family in families {
        let objective = |x: &[f64]| -> f64 {
            match Curve::compile(&family.ease(&x[2..]), &base) {
                Ok(curve) => solve(&scalar, &curve, x[0], x[1].exp()).2,
                Err(_) => f64::MAX,
            }
        };
        let mut best: Option<(Vec<f64>, f64)> = None;
        for params in family.starts() {
            for dscale in [0.7, 1.0, 1.5, 2.5] {
                let mut x = vec![t0_guess, (d_guess * dscale).ln()];
                x.extend(&params);
                let mut step = vec![2.0 * frame, 0.25];
                step.extend(params.iter().map(|p| if p.abs() > 0.5 { 0.2 * p.abs() } else { 0.1 }));
                let (mut p, mut v) = nelder_mead(&objective, &x, &step, 200);
                // Restart once from the result: the simplex stalls.
                let small: Vec<f64> = step.iter().map(|s| s * 0.3).collect();
                let (p2, v2) = nelder_mead(&objective, &p, &small, 200);
                if v2 < v {
                    (p, v) = (p2, v2);
                }
                if best.as_ref().is_none_or(|b| v < b.1) {
                    best = Some((p, v));
                }
            }
        }
        let Some((p, _)) = best else { continue };
        let Ok(curve) = Curve::compile(&family.ease(&p[2..]), &base) else { continue };
        let (a, b, sse) = solve(&scalar, &curve, p[0], p[1].exp());
        fits.push(Fit { family, params: p[2..].to_vec(), start: p[0], dur: p[1].exp(), a, b, sse });
    }
    let n = scalar.len() as f64;
    let rms = |sse: f64| (sse / n).sqrt();
    let lowest = fits.iter().map(|f| f.sse).fold(f64::MAX, f64::min);
    let close = |f: &Fit| rms(f.sse) <= rms(lowest) * 1.1 + 0.002 * travel;
    fits.sort_by(|a, b| {
        let (ca, cb) = (close(a), close(b));
        cb.cmp(&ca).then(if ca && cb { a.family.simplicity().cmp(&b.family.simplicity()).then(a.sse.total_cmp(&b.sse)) } else { a.sse.total_cmp(&b.sse) })
    });
    let value_at = |x: f64| -> Value {
        if dim == 1 {
            json!(round(x))
        } else {
            json!(v0.iter().zip(&d).map(|(a, dx)| round(a + x * dx)).collect::<Vec<_>>())
        }
    };
    let scale = if dim == 1 { 1.0 } else { dd.sqrt() };
    let describe = |f: &Fit| -> Value {
        let ease = serde_json::to_value(f.family.ease(&f.params)).unwrap_or_default();
        json!({
            "ease": ease, "family": f.family.name(),
            "start": round(f.start), "end": round(f.start + f.dur), "duration": round(f.dur),
            "from": value_at(f.a), "to": value_at(f.b),
            "rms_error": round(rms(f.sse) * scale), "relative_error": round(rms(f.sse) / travel.max(1e-12)),
            "key": {"at": round(f.start), "anchor": "start", "dur": round(f.dur), "from": value_at(f.a), "v": value_at(f.b), "ease": ease},
        })
    };
    let best = fits.first().ok_or("no ease fits these samples")?;
    Ok(json!({
        "best": describe(best),
        "others": fits[1..].iter().take(6).map(|f| json!({"family": f.family.name(), "relative_error": round(rms(f.sse) / travel.max(1e-12)),
            "ease": serde_json::to_value(f.family.ease(&f.params)).unwrap_or_default()})).collect::<Vec<_>>(),
        "samples": samples.len(),
    }))
}

// -------------------------------------------------------------- shading

/// Sobel magnitude of the luminance, 0–~1000 (a 255 step reads ~1020; a
/// step of 15 levels reads 60).
pub(crate) fn edges(img: &image::RgbaImage) -> (Vec<f32>, u32, u32) {
    let (w, h) = img.dimensions();
    let l = luma(img);
    let at = |x: u32, y: u32| l[(y * w + x) as usize];
    let mut e = vec![0f32; (w * h) as usize];
    for y in 1..h.saturating_sub(1) {
        for x in 1..w.saturating_sub(1) {
            let gx = at(x + 1, y - 1) + 2.0 * at(x + 1, y) + at(x + 1, y + 1) - at(x - 1, y - 1) - 2.0 * at(x - 1, y) - at(x - 1, y + 1);
            let gy = at(x - 1, y + 1) + 2.0 * at(x, y + 1) + at(x + 1, y + 1) - at(x - 1, y - 1) - 2.0 * at(x, y - 1) - at(x + 1, y - 1);
            e[(y * w + x) as usize] = (gx * gx + gy * gy).sqrt();
        }
    }
    (e, w, h)
}

fn luma(img: &image::RgbaImage) -> Vec<f32> {
    img.pixels().map(|p| 0.2126 * p.0[0] as f32 + 0.7152 * p.0[1] as f32 + 0.0722 * p.0[2] as f32).collect()
}

/// A Sobel magnitude above this is a drawn edge (a luminance step of 15+
/// levels): shape outlines, posterisation bands, lines. Its surroundings are
/// left out of the shading measure.
pub const SHADING_EDGE: f32 = 60.0;
/// Per-pixel slopes below this (luminance levels per 1% of the frame width)
/// count as flat: compression noise, banding and faint vignettes.
pub const SHADING_FLOOR: f64 = 0.5;
/// A reference whose interior shading is below this is flat itself (2D
/// graphics, line art): no flatness verdict either way.
pub const SHADING_MEANINGFUL: f64 = 1.0;
/// Ours ÷ reference below this is "flatter than the reference".
pub const SHADING_FLAT_RATIO: f64 = 0.5;
/// Fewer interior pixels than this share of the frame: nothing to measure
/// (line art, thin type). Low enough that thin glossy tubes and rings
/// still count at the full run's 270 px.
pub const SHADING_MIN_INTERIOR: f64 = 0.005;

/// How much shading lives *inside* shapes: the mean low-to-mid-frequency
/// luminance slope (levels per 1% of the frame width) over interior pixels,
/// those away from drawn edges and not the background. A lit 3D object
/// (gradients across its faces, soft highlights) reads several units; flat
/// fills, posterised polygons and line art read about 0. `None` when the
/// frame has almost no interior to measure.
///
/// Edges: Sobel above [`SHADING_EDGE`], dilated by b + 1 px (b = width /
/// 135, at least 1) so a shape's own outline barely leaks in while thin
/// lit objects (tubes, rings with highlight streaks) keep an interior. Background: pixels within 24 levels (per channel) of the most
/// common colour. Slope: central difference over ±b px of the luminance
/// box-blurred by b px; slopes under [`SHADING_FLOOR`] count as 0.
pub fn shading(img: &image::RgbaImage) -> Option<f64> {
    let (w, h) = img.dimensions();
    let (wu, hu) = (w as usize, h as usize);
    if w < 16 || h < 16 {
        return None;
    }
    let b = ((w as f64 / 135.0).round() as usize).max(1);
    let reach = b + 1;
    let (e, _, _) = edges(img);
    // Edge mask dilated by `reach` (separable max via running counts).
    let edge: Vec<bool> = e.iter().map(|&v| v > SHADING_EDGE).collect();
    let dilate = |src: &[bool], horizontal: bool| -> Vec<bool> {
        let mut out = vec![false; wu * hu];
        let (lines, len) = if horizontal { (hu, wu) } else { (wu, hu) };
        let idx = |line: usize, i: usize| if horizontal { line * wu + i } else { i * wu + line };
        for line in 0..lines {
            let mut prefix = vec![0u32; len + 1];
            for i in 0..len {
                prefix[i + 1] = prefix[i] + src[idx(line, i)] as u32;
            }
            for i in 0..len {
                let (a, z) = (i.saturating_sub(reach), (i + reach + 1).min(len));
                out[idx(line, i)] = prefix[z] > prefix[a];
            }
        }
        out
    };
    let near_edge = dilate(&dilate(&edge, true), false);
    // Background: the most common colour (4 bits per channel).
    let mut bins = vec![(0u32, [0u64; 3]); 4096];
    for p in img.pixels() {
        let k = ((p.0[0] as usize >> 4) << 8) | ((p.0[1] as usize >> 4) << 4) | (p.0[2] as usize >> 4);
        bins[k].0 += 1;
        for c in 0..3 {
            bins[k].1[c] += p.0[c] as u64;
        }
    }
    let (count, sum) = bins.iter().max_by_key(|b| b.0).copied().unwrap_or_default();
    let bg = sum.map(|s| s as f64 / count.max(1) as f64);
    let background = |p: &image::Rgba<u8>| (0..3).all(|c| (p.0[c] as f64 - bg[c]).abs() <= 24.0);
    // Luminance box-blurred by b (separable, clamped at the borders).
    let l = luma(img);
    let blur = |src: &[f32], horizontal: bool| -> Vec<f32> {
        let mut out = vec![0f32; wu * hu];
        for y in 0..hu {
            for x in 0..wu {
                let mut s = 0.0;
                for k in -(b as i64)..=b as i64 {
                    let (sx, sy) = if horizontal {
                        ((x as i64 + k).clamp(0, wu as i64 - 1) as usize, y)
                    } else {
                        (x, (y as i64 + k).clamp(0, hu as i64 - 1) as usize)
                    };
                    s += src[sy * wu + sx];
                }
                out[y * wu + x] = s / (2 * b + 1) as f32;
            }
        }
        out
    };
    let smooth = blur(&blur(&l, true), false);
    let per_percent = w as f64 / 100.0;
    let (mut total, mut interior) = (0.0f64, 0usize);
    for y in reach..hu.saturating_sub(reach) {
        for x in reach..wu.saturating_sub(reach) {
            let i = y * wu + x;
            if near_edge[i] || background(img.get_pixel(x as u32, y as u32)) {
                continue;
            }
            interior += 1;
            let gx = (smooth[i + b] - smooth[i - b]) as f64 / (2 * b) as f64;
            let gy = (smooth[i + b * wu] - smooth[i - b * wu]) as f64 / (2 * b) as f64;
            let g = (gx * gx + gy * gy).sqrt() * per_percent;
            if g >= SHADING_FLOOR {
                total += g;
            }
        }
    }
    if (interior as f64) < SHADING_MIN_INTERIOR * (wu * hu) as f64 {
        return None;
    }
    Some(total / interior as f64)
}

/// Ours ÷ reference interior shading, or `None` when the reference has no
/// meaningful shading (both flat 2D graphics: nothing to flag). Ours with
/// no interior at all counts as flat.
pub fn shading_ratio(ours: Option<f64>, reference: Option<f64>) -> Option<f64> {
    let r = reference?;
    (r >= SHADING_MEANINGFUL).then(|| ours.unwrap_or(0.0) / r)
}

/// "ours is flatter than the reference (shading 34% of the reference's)"
/// when the ratio is below [`SHADING_FLAT_RATIO`].
pub fn flatness_verdict(ratio: Option<f64>) -> Option<String> {
    let r = ratio?;
    (r < SHADING_FLAT_RATIO).then(|| format!("ours is flatter than the reference (shading {:.0}% of the reference's)", r * 100.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn moves(ease: &Ease, start: f64, dur: f64, from: f64, to: f64, fps: u32, frames: usize) -> Vec<(f64, Vec<f64>)> {
        let curve = Curve::compile(ease, &TimeBase::new(Fps::whole(fps))).unwrap();
        (0..frames)
            .map(|i| {
                let t = i as f64 / fps as f64;
                (t, vec![from + (to - from) * curve.progress(t - start, dur)])
            })
            .collect()
    }

    #[test]
    fn fits_recover_the_engines_own_eases() {
        let cases = [
            (Ease::Power { n: 3.0, mode: PowerMode::Out }, "power out", 0.4),
            (Ease::Spring { zeta: 0.35, hz: 3.0 }, "spring", 1.2),
            (Ease::Exp { r: None, tau: Some(Duration::Seconds(0.12)) }, "exp", 0.8),
            (Ease::Power { n: 2.0, mode: PowerMode::InOut }, "power in-out", 0.6),
        ];
        for (ease, family, dur) in cases {
            let samples = moves(&ease, 0.3, dur, 100.0, 400.0, 60, 120);
            let out = fit_curve(&samples, Fps::whole(60)).unwrap();
            let best = &out["best"];
            assert!(best["relative_error"].as_f64().unwrap() < 0.005, "{family}: {out:#}");
            assert!((best["start"].as_f64().unwrap() - 0.3).abs() < 0.02, "{family}: {best:#}");
            assert!((best["from"].as_f64().unwrap() - 100.0).abs() < 1.5 && (best["to"].as_f64().unwrap() - 400.0).abs() < 1.5, "{family}: {best:#}");
            // The same family, or a preset made of it that fits as well.
            let f = best["family"].as_str().unwrap();
            let same = f == family
                || Preset::by_name(f).is_some_and(|p| match (p.raw(), &ease) {
                    (Ease::Power { mode: a, .. }, Ease::Power { mode: b, .. }) => a == *b,
                    (Ease::Exp { .. }, Ease::Exp { .. }) | (Ease::Spring { .. }, Ease::Spring { .. }) => true,
                    _ => false,
                });
            assert!(same, "{family} fitted as {f}: {best:#}");
        }
        // A spring's parameters come back.
        let samples = moves(&Ease::Spring { zeta: 0.35, hz: 3.0 }, 0.3, 1.2, 0.0, 1.0, 60, 120);
        let best = &fit_curve(&samples, Fps::whole(60)).unwrap()["best"];
        assert!((best["ease"]["zeta"].as_f64().unwrap() - 0.35).abs() < 0.03, "{best:#}");
        assert!((best["ease"]["hz"].as_f64().unwrap() - 3.0).abs() < 0.15, "{best:#}");
    }

    #[test]
    fn vector_samples_fit_along_their_line() {
        let curve = Curve::compile(&Ease::Preset(Preset::Glide), &TimeBase::new(Fps::whole(25))).unwrap();
        let samples: Vec<(f64, Vec<f64>)> = (0..40)
            .map(|i| {
                let t = i as f64 / 25.0;
                let p = curve.progress(t - 0.2, 0.68);
                (t, vec![200.0 + 600.0 * p, 500.0 - 300.0 * p])
            })
            .collect();
        let out = fit_curve(&samples, Fps::whole(25)).unwrap();
        let best = &out["best"];
        assert_eq!(best["family"], "glide", "{out:#}");
        assert_eq!(best["key"]["from"], json!([200.0, 500.0]), "{best:#}");
        assert_eq!(best["key"]["v"], json!([800.0, 200.0]), "{best:#}");
        // The key parses as an engine key.
        Ease::from_json(&best["key"]["ease"]).unwrap();
    }

    #[test]
    fn rejects_too_few_samples() {
        assert!(fit_curve(&[(0.0, vec![1.0])], Fps::whole(30)).is_err());
    }
}

// ---------------------------------------------------------------- measure

/// Decodes `video` from `from` to `to` at w × h (RGB), calling `f` with each
/// frame's time and pixels.
pub fn frames(video: &Path, from: f64, to: Option<f64>, w: u32, h: u32, mut f: impl FnMut(f64, &[u8])) -> Result<VideoInfo, String> {
    let info = probe(video)?;
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-v", "error", "-nostdin", "-ss", &format!("{:.6}", from.max(0.0)), "-i"]).arg(video);
    if let Some(to) = to {
        cmd.args(["-t", &format!("{:.6}", (to - from).max(0.0))]);
    }
    let mut child = cmd
        .args(["-vf", &format!("scale={w}:{h}:flags=area"), "-fps_mode", "passthrough", "-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("cannot run ffmpeg: {e}"))?;
    let mut out = child.stdout.take().ok_or("ffmpeg gave no output")?;
    let mut buf = vec![0u8; (w * h * 3) as usize];
    let first = (from.max(0.0) * info.fps).round() as i64;
    let mut i = 0i64;
    while out.read_exact(&mut buf).is_ok() {
        f((first + i) as f64 / info.fps.max(1e-6), &buf);
        i += 1;
    }
    let _ = child.wait();
    Ok(info)
}

fn gray(buf: &[u8]) -> Vec<f32> {
    buf.chunks_exact(3).map(|p| 0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32).collect()
}

fn mean_abs(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| (x - y).abs()).sum::<f32>() / a.len().max(1) as f32
}

/// Edge energy: mean |gradient| (sharp pictures high, blurred low).
fn edge_energy(g: &[f32], w: usize, h: usize) -> f32 {
    let mut s = 0.0;
    for y in 0..h - 1 {
        for x in 0..w - 1 {
            let i = y * w + x;
            s += (g[i + 1] - g[i]).abs() + (g[i + w] - g[i]).abs();
        }
    }
    s / ((w - 1) * (h - 1)).max(1) as f32
}

/// Cuts and transitions, from frame differences at 64 × 36: a cut changes
/// the picture in one frame and keeps it; a flash jumps in brightness and
/// comes back; a dissolve is a run of frames that are blends of the frames
/// around it; a whip is a run of blurred, fast-changing frames.
pub fn cuts(video: &Path, from: f64, to: Option<f64>) -> Result<Value, String> {
    let (w, h) = (64usize, 36usize);
    let mut fr: Vec<(f64, Vec<f32>)> = Vec::new();
    let info = frames(video, from, to, w as u32, h as u32, |t, buf| fr.push((t, gray(buf))))?;
    if fr.len() < 3 {
        return Ok(json!({"events": [], "frames": fr.len()}));
    }
    let d: Vec<f32> = (0..fr.len()).map(|i| if i == 0 { 0.0 } else { mean_abs(&fr[i].1, &fr[i - 1].1) }).collect();
    let mut sorted = d[1..].to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let median = sorted[sorted.len() / 2];
    let high = (median * 4.0).max(6.0);
    let lum = |i: usize| fr[i].1.iter().sum::<f32>() / fr[i].1.len() as f32;
    let mut events = Vec::new();
    let mut i = 1;
    while i < fr.len() {
        if d[i] < high {
            i += 1;
            continue;
        }
        // A run of high differences [i, j).
        let mut j = i + 1;
        while j < fr.len() && d[j] >= high * 0.5 {
            j += 1;
        }
        let before = i - 1;
        let after = j.min(fr.len() - 1);
        let span = j - i;
        let back = mean_abs(&fr[before].1, &fr[after].1);
        let t = fr[i].0;
        let jump = (lum(i) - lum(before)).abs();
        let (kind, extra) = if back < high * 0.5 && jump > 8.0 {
            ("flash", json!({"brightness": round((lum(i) - lum(before)) as f64)}))
        } else if span == 1 {
            ("cut", json!({}))
        } else {
            // Blends of the frames around the run: a dissolve.
            let residual = (i..j)
                .map(|k| {
                    let (a, b) = (&fr[before].1, &fr[after].1);
                    // Best mix of a and b for frame k.
                    let (mut num, mut den) = (0.0f32, 0.0f32);
                    for p in 0..a.len() {
                        let (x, y) = (fr[k].1[p] - b[p], a[p] - b[p]);
                        num += x * y;
                        den += y * y;
                    }
                    let alpha = (num / den.max(1e-6)).clamp(0.0, 1.0);
                    fr[k].1.iter().zip(a.iter().zip(b)).map(|(v, (x, y))| (v - (alpha * x + (1.0 - alpha) * y)).abs()).sum::<f32>() / a.len() as f32
                })
                .fold(0.0f32, f32::max);
            let sharp = (edge_energy(&fr[before].1, w, h) + edge_energy(&fr[after].1, w, h)) / 2.0;
            let blur = (i..j).map(|k| edge_energy(&fr[k].1, w, h)).fold(f32::MAX, f32::min);
            if residual < 4.0 {
                ("dissolve", json!({"end": round(fr[after].0)}))
            } else if blur < sharp * 0.6 {
                ("whip", json!({"end": round(fr[after].0)}))
            } else {
                ("motion", json!({"end": round(fr[after].0)}))
            }
        };
        let mut e = json!({"t": round(t), "frame": (t * info.fps).round(), "kind": kind, "strength": round(d[i] as f64)});
        if let (Some(o), Some(x)) = (e.as_object_mut(), extra.as_object()) {
            o.extend(x.clone());
        }
        events.push(e);
        i = j;
    }
    Ok(json!({"events": events, "frames": fr.len(), "fps": info.fps, "threshold": round(high as f64)}))
}

/// Motion events: where the picture starts and stops changing (cuts left
/// out), with the box of what changed, at 160 px wide.
pub fn events(video: &Path, from: f64, to: Option<f64>) -> Result<Value, String> {
    let info0 = probe(video)?;
    let w = 160usize;
    let h = ((info0.height as f64 * w as f64 / info0.width.max(1) as f64).round() as usize).max(2);
    let mut fr: Vec<(f64, Vec<f32>)> = Vec::new();
    let info = frames(video, from, to, w as u32, h as u32, |t, buf| fr.push((t, gray(buf))))?;
    let s = info.width as f64 / w as f64;
    let mut out = Vec::new();
    let mut moving: Option<(f64, [usize; 4])> = None;
    for k in 1..fr.len() {
        let (a, b) = (&fr[k - 1].1, &fr[k].1);
        let mut bb = [usize::MAX, usize::MAX, 0, 0];
        let mut n = 0usize;
        for y in 0..h {
            for x in 0..w {
                if (a[y * w + x] - b[y * w + x]).abs() > 12.0 {
                    n += 1;
                    bb = [bb[0].min(x), bb[1].min(y), bb[2].max(x), bb[3].max(y)];
                }
            }
        }
        let frac = n as f64 / (w * h) as f64;
        // A cut changes most of the frame at once: not an event.
        let is_cut = frac > 0.6;
        let active = frac > 0.002 && !is_cut;
        let t = fr[k].0;
        match (&mut moving, active) {
            (None, true) => moving = Some((t, bb)),
            (Some((_, b0)), true) => *b0 = [b0[0].min(bb[0]), b0[1].min(bb[1]), b0[2].max(bb[2]), b0[3].max(bb[3])],
            (Some((t0, b0)), false) => {
                let r = [b0[0] as f64 * s, b0[1] as f64 * s, (b0[2] + 1 - b0[0]) as f64 * s, (b0[3] + 1 - b0[1]) as f64 * s];
                out.push(json!({"start": round(*t0), "end": round(t), "region": r.map(round)}));
                moving = None;
            }
            _ => {}
        }
    }
    if let Some((t0, b0)) = moving {
        let r = [b0[0] as f64 * s, b0[1] as f64 * s, (b0[2] + 1 - b0[0]) as f64 * s, (b0[3] + 1 - b0[1]) as f64 * s];
        out.push(json!({"start": round(t0), "end": round(fr.last().map(|f| f.0).unwrap_or(t0)), "region": r.map(round)}));
    }
    Ok(json!({"events": out}))
}

fn otsu(v: &[f32]) -> f32 {
    let mut hist = [0usize; 256];
    for x in v {
        hist[(*x).clamp(0.0, 255.0) as usize] += 1;
    }
    let total = v.len() as f64;
    let sum: f64 = hist.iter().enumerate().map(|(i, c)| i as f64 * *c as f64).sum();
    let (mut wb, mut sb, mut best, mut thr) = (0.0, 0.0, 0.0, 128.0);
    for (i, c) in hist.iter().enumerate() {
        wb += *c as f64;
        if wb == 0.0 {
            continue;
        }
        let wf = total - wb;
        if wf == 0.0 {
            break;
        }
        sb += i as f64 * *c as f64;
        let (mb, mf) = (sb / wb, (sum - sb) / wf);
        let between = wb * wf * (mb - mf) * (mb - mf);
        if between > best {
            best = between;
            thr = i as f64 + 0.5;
        }
    }
    thr as f32
}

/// Text size ("ink") in `region` [x, y, w, h] at time `at`: the ink (the
/// minority side of an Otsu threshold) is split into lines by its row
/// profile; each line's top, bottom and height (px), and the median stroke
/// width.
pub fn ink(video: &Path, at: f64, region: Option<[f64; 4]>) -> Result<Value, String> {
    let info0 = probe(video)?;
    let (w, h) = (info0.width.max(2), info0.height.max(2));
    let mut frame: Option<Vec<u8>> = None;
    frames(video, at, Some(at + 0.5 / info0.fps.max(1.0)), w, h, |_, buf| {
        if frame.is_none() {
            frame = Some(buf.to_vec());
        }
    })?;
    let buf = frame.ok_or("no frame there")?;
    let [rx, ry, rw, rh] = region.unwrap_or([0.0, 0.0, w as f64, h as f64]);
    let (x0, y0) = (rx.max(0.0) as usize, ry.max(0.0) as usize);
    let (x1, y1) = (((rx + rw) as usize).min(w as usize), ((ry + rh) as usize).min(h as usize));
    if x1 <= x0 || y1 <= y0 {
        return Err("region is empty".into());
    }
    let g: Vec<f32> = (y0..y1).flat_map(|y| (x0..x1).map(move |x| (x, y))).map(|(x, y)| {
        let p = &buf[(y * w as usize + x) * 3..];
        0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32
    }).collect();
    let (cw, ch) = (x1 - x0, y1 - y0);
    let thr = otsu(&g);
    let above = g.iter().filter(|v| **v > thr).count();
    // Ink is the smaller side (light text on dark or dark on light).
    let light_ink = above * 2 < g.len();
    let is_ink = |v: f32| if light_ink { v > thr } else { v <= thr };
    let rows: Vec<usize> = (0..ch).map(|y| (0..cw).filter(|&x| is_ink(g[y * cw + x])).count()).collect();
    let mut lines = Vec::new();
    let mut y = 0;
    while y < ch {
        if rows[y] == 0 {
            y += 1;
            continue;
        }
        let top = y;
        while y < ch && rows[y] > 0 {
            y += 1;
        }
        let height = y - top;
        if height >= 2 {
            lines.push(json!({"top": y0 + top, "bottom": y0 + y, "height_px": height}));
        }
    }
    let mut runs = Vec::new();
    for yy in 0..ch {
        let mut run = 0;
        for xx in 0..cw {
            if is_ink(g[yy * cw + xx]) {
                run += 1;
            } else if run > 0 {
                runs.push(run);
                run = 0;
            }
        }
    }
    runs.sort_unstable();
    let stroke = runs.get(runs.len() / 2).copied().unwrap_or(0);
    Ok(json!({"lines": lines, "stroke_px": stroke, "ink": if light_ink { "light" } else { "dark" }, "threshold": round(thr as f64)}))
}

/// Caption timing from the caption band's pixels: when the band holds
/// caption text (pixels within `tolerance` of the text colour, default
/// white) and when that text changes. `region` is the band [x, y, w, h]
/// (default the bottom fifth).
pub fn captions(video: &Path, from: f64, to: Option<f64>, region: Option<[f64; 4]>, color: [f64; 3], tolerance: f64) -> Result<Value, String> {
    let info0 = probe(video)?;
    let (w, h) = (320u32, ((info0.height as f64 * 320.0 / info0.width.max(1) as f64).round() as u32).max(2));
    let s = info0.width as f64 / w as f64;
    let band = region.unwrap_or([0.0, info0.height as f64 * 0.8, info0.width as f64, info0.height as f64 * 0.2]).map(|v| v / s);
    let (bx0, by0) = (band[0].max(0.0) as usize, band[1].max(0.0) as usize);
    let (bx1, by1) = (((band[0] + band[2]) as usize).min(w as usize), ((band[1] + band[3]) as usize).min(h as usize));
    let tol2 = tolerance * tolerance;
    let mut masks: Vec<(f64, Vec<bool>)> = Vec::new();
    frames(video, from, to, w, h, |t, buf| {
        let m: Vec<bool> = (by0..by1)
            .flat_map(|y| (bx0..bx1).map(move |x| (x, y)))
            .map(|(x, y)| {
                let p = &buf[(y * w as usize + x) * 3..];
                (0..3).map(|c| (p[c] as f64 - color[c]).powi(2)).sum::<f64>() <= tol2
            })
            .collect();
        masks.push((t, m));
    })?;
    let area = ((bx1 - bx0) * (by1 - by0)).max(1) as f64;
    let mut out = Vec::new();
    let mut cur: Option<(f64, usize)> = None;
    for k in 0..masks.len() {
        let count = masks[k].1.iter().filter(|b| **b).count();
        let has_text = count as f64 / area > 0.005;
        let changed = cur.is_some_and(|(_, at)| {
            let (a, b) = (&masks[k].1, &masks[at].1);
            let union = a.iter().zip(b).filter(|(x, y)| **x || **y).count();
            let diff = a.iter().zip(b).filter(|(x, y)| x != y).count();
            union > 0 && diff as f64 / union as f64 > 0.3
        });
        match cur {
            Some((t0, _)) if !has_text || changed => {
                out.push(json!({"start": round(t0), "end": round(masks[k].0)}));
                cur = if has_text { Some((masks[k].0, k)) } else { None };
            }
            None if has_text => cur = Some((masks[k].0, k)),
            _ => {}
        }
    }
    if let Some((t0, _)) = cur {
        out.push(json!({"start": round(t0), "end": round(masks.last().map(|x| x.0 + 1.0 / info0.fps.max(1.0)).unwrap_or(t0))}));
    }
    Ok(json!({"captions": out}))
}

// ---------------------------------------------------------------- SSIM

/// Cells across and down in the SSIM grid.
pub const SSIM_GRID: usize = 4;

/// Structural similarity of two equal-size pictures' luminance (Wang et
/// al. 2004: Gaussian window σ 1.5, K₁ 0.01, K₂ 0.03), optionally inside
/// `region` [x, y, w, h] (px of these pictures). Returns the mean SSIM
/// and the mean of each cell of a [`SSIM_GRID`]² grid over the region,
/// row by row (1 identical; around 0 unrelated).
pub fn ssim(a: &image::RgbaImage, b: &image::RgbaImage, region: Option<[u32; 4]>) -> (f64, Vec<f64>) {
    let (iw, ih) = a.dimensions();
    let [rx, ry, rw, rh] = region.unwrap_or([0, 0, iw, ih]);
    let (rx, ry) = (rx.min(iw.saturating_sub(1)), ry.min(ih.saturating_sub(1)));
    let (w, h) = (rw.clamp(1, iw - rx) as usize, rh.clamp(1, ih - ry) as usize);
    let crop = |img: &image::RgbaImage| -> Vec<f32> {
        let mut out = Vec::with_capacity(w * h);
        for y in 0..h {
            for x in 0..w {
                let p = img.get_pixel(rx + x as u32, ry + y as u32).0;
                out.push(0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32);
            }
        }
        out
    };
    let (x, y) = (crop(a), crop(b));
    let xx: Vec<f32> = x.iter().map(|v| v * v).collect();
    let yy: Vec<f32> = y.iter().map(|v| v * v).collect();
    let xy: Vec<f32> = x.iter().zip(&y).map(|(a, b)| a * b).collect();
    let [mx, my, sxx, syy, sxy] = [&x, &y, &xx, &yy, &xy].map(|v| gauss(v, w, h));
    let (c1, c2) = ((0.01f32 * 255.0).powi(2), (0.03f32 * 255.0).powi(2));
    let mut cells = vec![(0.0f64, 0usize); SSIM_GRID * SSIM_GRID];
    let mut total = 0.0f64;
    for j in 0..h {
        let cy = j * SSIM_GRID / h;
        for i in 0..w {
            let k = j * w + i;
            let (ux, uy) = (mx[k], my[k]);
            let (vx, vy, cxy) = ((sxx[k] - ux * ux).max(0.0), (syy[k] - uy * uy).max(0.0), sxy[k] - ux * uy);
            let s = ((2.0 * ux * uy + c1) * (2.0 * cxy + c2)) / ((ux * ux + uy * uy + c1) * (vx + vy + c2));
            total += s as f64;
            let cell = &mut cells[cy * SSIM_GRID + i * SSIM_GRID / w];
            cell.0 += s as f64;
            cell.1 += 1;
        }
    }
    let grid = cells.iter().map(|&(s, n)| if n == 0 { f64::NAN } else { s / n as f64 }).collect();
    (total / (w * h) as f64, grid)
}

/// Gaussian blur, σ 1.5 over ±5 px, edges clamped.
fn gauss(v: &[f32], w: usize, h: usize) -> Vec<f32> {
    let k: [f32; 11] = std::array::from_fn(|i| (-((i as f32 - 5.0).powi(2)) / (2.0 * 1.5 * 1.5)).exp());
    let sum: f32 = k.iter().sum();
    let k = k.map(|x| x / sum);
    let mut tmp = vec![0.0f32; w * h];
    for y in 0..h {
        let row = &v[y * w..(y + 1) * w];
        for x in 0..w {
            tmp[y * w + x] = (0..11).map(|i| k[i] * row[(x as i64 + i as i64 - 5).clamp(0, w as i64 - 1) as usize]).sum();
        }
    }
    let mut out = vec![0.0f32; w * h];
    for y in 0..h {
        for x in 0..w {
            out[y * w + x] = (0..11).map(|i| k[i] * tmp[(y as i64 + i as i64 - 5).clamp(0, h as i64 - 1) as usize * w + x]).sum();
        }
    }
    out
}

/// A set of frames' SSIM summarised as `score.py` does: the mean, the 5th
/// percentile (the bad moments), and the worst grid cell over the set (the
/// cell whose mean SSIM is lowest: where in the frame it differs).
pub fn ssim_summary(frames: &[(f64, Vec<f64>)]) -> Value {
    if frames.is_empty() {
        return Value::Null;
    }
    let mut v: Vec<f64> = frames.iter().map(|f| f.0).collect();
    v.sort_by(f64::total_cmp);
    let mean = v.iter().sum::<f64>() / v.len() as f64;
    let p05 = v[((v.len() - 1) as f64 * 0.05).floor() as usize];
    let cells = frames[0].1.len();
    let mut worst = (0usize, f64::INFINITY);
    for c in 0..cells {
        let vals: Vec<f64> = frames.iter().map(|f| f.1[c]).filter(|x| x.is_finite()).collect();
        if vals.is_empty() {
            continue;
        }
        let m = vals.iter().sum::<f64>() / vals.len() as f64;
        if m < worst.1 {
            worst = (c, m);
        }
    }
    let (col, row) = (worst.0 % SSIM_GRID, worst.0 / SSIM_GRID);
    json!({"mean": round(mean), "p05": round(p05), "min": round(v[0]),
           "worst_cell": {"column": col, "row": row, "ssim": round(worst.1.min(1.0))}})
}
