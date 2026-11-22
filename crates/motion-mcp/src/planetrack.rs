//! A plane followed through a video, and the scene camera it implies.
//!
//! `track` decodes the video once at a small size and follows the marked
//! region both ways from the frame it was marked on. `solve` turns the
//! corners into the scene's camera, frame by frame: the marked rectangle
//! becomes a rectangle on the scene's z = 0, the footage stays in screen
//! space, and whatever is 3D in the scene stands on the plane.

use lahza_motion_core::camfit::{self, CameraParams, Free, Mark};
use lahza_motion_core::space;
use lahza_motion_track::image::Gray;
use lahza_motion_track::plane::{Options, PlaneTracker};
use lahza_motion_track::still::{StillTest, STILL};
use lahza_motion_track::{convex, rect, signed_area2, Quad, H3};
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};

use crate::measure::{probe, VideoInfo};

/// One frame of a track; corners in source pixels.
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    pub frame: i64,
    pub t: f64,
    pub found: bool,
    pub corners: Quad,
    pub score: f64,
    pub agree: f64,
    pub visible: f64,
}

pub struct Track {
    pub info: VideoInfo,
    /// The size the frames were followed at.
    pub size: (u32, u32),
    pub frames: Vec<Frame>,
    /// Index of the marked frame in `frames`.
    pub marked: usize,
    /// Times the template was taken again (each adds a little drift).
    pub retakes: usize,
    /// Frames on which the camera had not moved since the marked frame:
    /// the corners are the marked ones there.
    pub still: usize,
    /// How much of the region has something on it (0–1, see
    /// `PlaneTracker::texture`).
    pub texture: f64,
    /// Seconds spent, decoding included.
    pub seconds: f64,
}

/// Frames kept for the way back from the marked frame, at most.
const BACK_BYTES: usize = 1 << 30;

/// The width to follow at: 640, or more when the region is small, so its
/// short side is about a hundred pixels; never more than the source.
fn analysis_width(source: u32, quad: &Quad) -> u32 {
    let side = (0..4).map(|i| {
        let (a, b) = (quad[i], quad[(i + 1) % 4]);
        ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
    }).fold(f64::MAX, f64::min).max(1.0);
    let scale = (640.0 / source as f64).max(96.0 / side).min(1.0);
    (((source as f64 * scale / 2.0).round() as u32) * 2).clamp(16, source.max(16)).min(1920)
}

/// A surface with this much of its own on it (`PlaneTracker::texture`)
/// is followed by its look even under a still camera: its own track is
/// then finer than "it has not moved".
const TEXTURED: f64 = 0.6;

/// One frame. Under a still camera (`stood`) a surface with little of
/// its own on it is where it was marked (None: the marked corners); one
/// with texture is followed all the same, and held only if that fails.
fn one(tracker: &mut PlaneTracker, frame: &Gray, stood: bool, texture: f64) -> Option<lahza_motion_track::plane::Sample> {
    if stood && texture < TEXTURED {
        tracker.hold(frame);
        return None;
    }
    let s = tracker.step(frame);
    if stood && !s.found {
        tracker.hold(frame);
        return None;
    }
    Some(s)
}

/// Follows `corners` (source pixels, on the frame at `at` seconds) from
/// `from` to `to`. On a frame where the camera has not moved since the
/// marked frame the corners are the marked ones, unless `moving` says
/// the surface itself moves in the scene (a thing carried, a held
/// screen), when only its own look is followed.
#[allow(clippy::too_many_arguments)]
pub fn track(video: &Path, corners: Quad, at: f64, from: f64, to: Option<f64>, width: Option<u32>, moving: bool, opts: Options) -> Result<Track, String> {
    let started = std::time::Instant::now();
    let info = probe(video)?;
    if info.width == 0 || info.height == 0 {
        return Err(format!("{}: unknown frame size", video.display()));
    }
    if !convex(&corners) {
        return Err("corners: four [x, y] going round the region in order".into());
    }
    let from = from.max(0.0);
    if at < from - 1e-6 || to.is_some_and(|to| at > to + 1e-6) {
        return Err("\"at\" (the frame the corners were marked on) must be between \"from\" and \"to\"".into());
    }
    let w = width.map(|w| (w / 2 * 2).clamp(16, info.width.max(16))).unwrap_or_else(|| analysis_width(info.width, &corners));
    let h = ((info.height as f64 * w as f64 / info.width as f64 / 2.0).round() as u32 * 2).max(2);
    let (sx, sy) = (w as f64 / info.width as f64, h as f64 / info.height as f64);
    // Pixel centres: source ↔ the size followed at.
    let down = |q: &Quad| q.map(|p| [(p[0] + 0.5) * sx - 0.5, (p[1] + 0.5) * sy - 0.5]);
    let up = |q: &Quad| q.map(|p| [(p[0] + 0.5) / sx - 0.5, (p[1] + 0.5) / sy - 0.5]);
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-v", "error", "-nostdin", "-ss", &format!("{from:.6}"), "-i"]).arg(video);
    if let Some(to) = to {
        cmd.args(["-t", &format!("{:.6}", (to - from).max(0.0))]);
    }
    let mut child = cmd
        .args(["-vf", &format!("scale={w}:{h}:flags=area"), "-fps_mode", "passthrough", "-f", "rawvideo", "-pix_fmt", "gray", "-"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("cannot run ffmpeg: {e}"))?;
    let mut out = child.stdout.take().ok_or("ffmpeg gave no output")?;
    let (wu, hu) = (w as usize, h as usize);
    let first = (from * info.fps).round() as i64;
    let marked = ((at - from) * info.fps).round().max(0.0) as usize;
    if marked.saturating_mul(wu * hu) > BACK_BYTES {
        let _ = child.kill();
        return Err(format!("{marked} frames lie between \"from\" and the marked frame, more than is kept to follow backwards: mark the corners on an earlier frame, or track the stretch in two calls"));
    }
    let quad = down(&corners);
    let sample = |i: usize, s: lahza_motion_track::plane::Sample| Frame {
        frame: first + i as i64,
        t: (first + i as i64) as f64 / info.fps.max(1e-6),
        found: s.found,
        corners: up(&s.corners),
        score: s.score,
        agree: s.agree,
        visible: s.visible,
    };
    let mut frames: Vec<Frame> = Vec::new();
    let mut back: Vec<Vec<u8>> = Vec::with_capacity(marked);
    let mut buf = vec![0u8; wu * hu];
    let mut tracker: Option<PlaneTracker> = None;
    let mut still_test: Option<StillTest> = None;
    let (mut retakes, mut still, mut texture) = (0, 0usize, 0.0);
    let held = |i: usize| Frame { frame: first + i as i64, t: (first + i as i64) as f64 / info.fps.max(1e-6), found: true, corners, score: 1.0, agree: 1.0, visible: 1.0 };
    let mut i = 0usize;
    while out.read_exact(&mut buf).is_ok() {
        if i < marked {
            back.push(buf.clone());
        } else if i == marked {
            let frame = Gray::from_luma8(wu, hu, &buf);
            let test = (!moving).then(|| StillTest::new(&frame));
            // Backwards first, over the frames kept.
            let mut backwards = PlaneTracker::new(&frame, quad, opts)?;
            texture = backwards.texture();
            let mut before: Vec<Frame> = Vec::with_capacity(back.len());
            for (k, b) in back.iter().enumerate().rev() {
                let g = Gray::from_luma8(wu, hu, b);
                let stood = test.as_ref().is_some_and(|t| t.share(&g) >= STILL);
                still += stood as usize;
                before.push(match one(&mut backwards, &g, stood, texture) {
                    Some(s) => sample(k, s),
                    None => held(k),
                });
            }
            before.reverse();
            retakes += backwards.rekeys;
            back = Vec::new();
            frames = before;
            frames.push(held(i));
            tracker = Some(PlaneTracker::new(&frame, quad, opts)?);
            still_test = test;
        } else if let Some(tracker) = tracker.as_mut() {
            let g = Gray::from_luma8(wu, hu, &buf);
            let share = still_test.as_ref().map_or(0.0, |t| t.share(&g));
            if std::env::var_os("LAHZA_TRACK_DEBUG").is_some() {
                eprintln!("still: frame {i} share {share:.3}");
            }
            let stood = share >= STILL;
            still += stood as usize;
            frames.push(match one(tracker, &g, stood, texture) {
                Some(s) => sample(i, s),
                None => held(i),
            });
        }
        i += 1;
    }
    let _ = child.wait();
    let Some(tracker) = tracker else {
        return Err(if i == 0 { format!("{}: no frames could be read from {from} s", video.display()) } else { format!("the video ends before the marked frame ({at} s)") });
    };
    retakes += tracker.rekeys;
    Ok(Track { info, size: (w, h), frames, marked, retakes, still, texture, seconds: started.elapsed().as_secs_f64() })
}

/// The scene camera a track implies.
pub struct Solve {
    pub fov: f64,
    /// "given", "measured" (from the rectangle's perspective, or from
    /// the camera's motion) or "assumed" (neither could tell).
    pub lens: &'static str,
    /// How much perspective the rectangle's measure rests on (see
    /// `rect::Lens`).
    pub strength: f64,
    /// How the surface's lie was read: "rectangle" (the marked corners
    /// are a real rectangle's) or "motion" (four points of no known
    /// shape, the tilt from the camera's own motion).
    pub plane: &'static str,
    /// The marked shape's width ÷ height on the surface (a rectangle's
    /// proportions; for points, their bounding box's).
    pub aspect: f64,
    /// The marked corners on the scene's z = 0, in the order marked.
    pub points: Quad,
    /// The rectangle round them on z = 0 (the same when they are one).
    pub rect: Quad,
    /// One camera per frame found: (index into the track's frames, the
    /// camera, reprojection error in canvas pixels).
    pub cameras: Vec<(usize, CameraParams, f64)>,
}

/// Perspective under this says too little about the lens.
const MIN_STRENGTH: f64 = 0.004;

/// What the corners are taken to be.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Corners {
    /// A real rectangle's, if that fits the track; else four points.
    Auto,
    Rectangle,
    /// Four points on the surface, of no known shape.
    Points,
}

/// Where footage drawn at `scale` canvas pixels per source pixel, with
/// its corner at `offset`, puts a source pixel on the canvas. A track's
/// corners count pixels (pixel 0's middle is at 0); the canvas is
/// measured from its corner (that middle is at half a pixel), as the
/// scene's own coordinates are.
pub fn placed(scale: f64, offset: [f64; 2]) -> H3 {
    H3([scale, 0.0, 0.5 * scale + offset[0], 0.0, scale, 0.5 * scale + offset[1], 0.0, 0.0, 1.0])
}

/// Solves the camera for a track. `place` puts a source pixel of the
/// footage on the canvas (`placed`, or its layer's own placement). `size`: the marked shape's height in the
/// scene (default: what makes the camera stand at its natural distance
/// on the marked frame, dolly 0).
///
/// The surface's lie comes from the marked corners being a rectangle's
/// (one frame is enough), or, when they are not, from the camera's own
/// motion (`tilt`), which needs the camera to have changed its place.
#[allow(clippy::too_many_arguments)]
pub fn solve(track: &Track, canvas: (f64, f64), place: H3, fov: Option<f64>, aspect: Option<f64>, size: Option<f64>, corners: Corners) -> Result<Solve, String> {
    let rectangle = if corners == Corners::Points { None } else { Some(solve_as(track, canvas, place, fov, aspect, size, false, false)) };
    // A real rectangle's corners leave next to nothing; four points that
    // are not one cannot be seen as one from every frame.
    let fits = |s: &Solve| s.cameras.iter().map(|c| c.2).sum::<f64>() / s.cameras.len().max(1) as f64;
    match (corners, rectangle) {
        (Corners::Rectangle, Some(r)) => r,
        (Corners::Auto, Some(Ok(r))) if fits(&r) <= 1.0 => Ok(r),
        (_, rectangle) => {
            // Asked for points, the motion has to settle the tilt; as a
            // second try after a rectangle that does not fit, it only has
            // to fit much better (a lens given a little wrong leaves both
            // off, the rectangle far more).
            let by_motion = solve_as(track, canvas, place, fov, aspect, size, true, corners == Corners::Auto);
            match (rectangle, by_motion) {
                (Some(Ok(r)), Ok(m)) => Ok(if fits(&m) < fits(&r) * 0.5 { m } else { r }),
                (_, Ok(m)) => Ok(m),
                (Some(r), Err(_)) => r,
                (None, Err(e)) => Err(e),
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn solve_as(track: &Track, canvas: (f64, f64), place: H3, fov: Option<f64>, aspect: Option<f64>, size: Option<f64>, by_motion: bool, unsure_too: bool) -> Result<Solve, String> {
    let (w, h) = canvas;
    // The lens looks through the middle of the footage, which need not
    // be the middle of the canvas: the scene camera's lens shift makes up
    // the difference.
    let middle = [w / 2.0, h / 2.0];
    let centre = place.apply([track.info.width as f64 / 2.0 - 0.5, track.info.height as f64 / 2.0 - 0.5]);
    let shift = [centre[0] - middle[0], centre[1] - middle[1]];
    // Clockwise as seen, so the scene's −z is the side the camera is on.
    let clockwise = signed_area2(&track.frames[track.marked].corners) > 0.0;
    let on_canvas = |f: &Frame| -> Quad {
        let q = f.corners.map(|p| place.apply(p));
        if clockwise { q } else { [q[0], q[3], q[2], q[1]] }
    };
    let found: Vec<usize> = (0..track.frames.len()).filter(|&i| track.frames[i].found).collect();
    // Up to a hundred and twenty views spread over the track.
    let step = (found.len() / 120).max(1);
    let views: Vec<Quad> = found.iter().step_by(step).map(|&i| on_canvas(&track.frames[i])).collect();
    let marked = on_canvas(&track.frames[track.marked]);
    // The marked shape on the surface, one unit tall, about its middle;
    // and the lens.
    let (focal, lens, strength, unit): (f64, &'static str, f64, Quad) = if by_motion {
        if track.still + 1 >= track.frames.len() {
            return Err("the camera does not move, so nothing says how the surface lies: mark the corners of a real rectangle on it".into());
        }
        let known = fov.map(|fov| space::focal(h, fov));
        let mut all = views.clone();
        all.push(marked);
        let tilt = lahza_motion_track::tilt::from_motion(&all, all.len() - 1, centre, h, known).ok_or("the surface's tilt could not be read from the track")?;
        if !tilt.sure() && !unsure_too {
            // The tilt stands out but nothing fits: the lens is wrong.
            if fov.is_some() && tilt.other > 4.0 * tilt.error {
                return Err(format!("with the lens given, no way the surface could lie fits the track (the best leaves {:.1} px): leave \"fov\" out to have it measured from the camera's motion", tilt.error));
            }
            return Err(format!("the camera's motion does not say how the surface lies (the best tilt leaves {:.2} px, a typical other {:.2} px): it has to change its place, not only turn. Mark the corners of a real rectangle on the surface instead", tilt.error, tilt.other));
        }
        let (mut lo, mut hi) = ([f64::MAX; 2], [f64::MIN; 2]);
        for p in &tilt.shape {
            for k in 0..2 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        let tall = (hi[1] - lo[1]).max(1e-9);
        let unit = tilt.shape.map(|p| [(p[0] - (lo[0] + hi[0]) / 2.0) / tall, (p[1] - (lo[1] + hi[1]) / 2.0) / tall]);
        (tilt.focal, if fov.is_some() { "given" } else { "measured" }, 0.0, unit)
    } else {
        let measured = rect::focal(&views, centre, h, aspect);
        let (focal, lens, strength) = match (fov, measured) {
            (Some(fov), m) => (space::focal(h, fov), "given", m.map_or(0.0, |m| m.strength)),
            (None, Some(m)) if m.strength >= MIN_STRENGTH => (m.focal, "measured", m.strength),
            (None, m) => (space::focal(h, 39.6), "assumed", m.map_or(0.0, |m| m.strength)),
        };
        let aspect = aspect.or_else(|| rect::aspect(&views, centre, h, focal)).filter(|a| a.is_finite() && *a > 0.01 && *a < 100.0).ok_or("the rectangle's proportions could not be read: give \"aspect\" (its real width ÷ height)")?;
        (focal, lens, strength, [[-aspect / 2.0, -0.5], [aspect / 2.0, -0.5], [aspect / 2.0, 0.5], [-aspect / 2.0, 0.5]])
    };
    let fov = (2.0 * (h / 2.0 / focal).atan()).to_degrees();
    if !(1.0..=170.0).contains(&fov) {
        return Err(format!("the lens comes out at {fov:.1}° of view: give \"fov\""));
    }
    let aspect = {
        let (x, y) = (unit.iter().map(|p| p[0]), unit.iter().map(|p| p[1]));
        (x.clone().fold(f64::MIN, f64::max) - x.fold(f64::MAX, f64::min)) / (y.clone().fold(f64::MIN, f64::max) - y.fold(f64::MAX, f64::min)).max(1e-9)
    };
    // The size at which the camera stands at its natural distance.
    let pose = rect::pose(&unit, &marked, centre, focal).ok_or("the marked corners are not seen from in front")?;
    let size = match size {
        Some(s) if s > 0.0 => s,
        _ => {
            let c = camfit::from_pose(pose.r, pose.t, w, h, fov).ok_or("the marked surface is seen edge on")?;
            let distance = space::focal(h, fov) - c.dolly;
            space::focal(h, fov) / distance.max(1e-9)
        }
    };
    let world: Quad = unit.map(|p| [middle[0] + p[0] * size, middle[1] + p[1] * size]);
    let rect: Quad = [[middle[0] - aspect * size / 2.0, middle[1] - size / 2.0], [middle[0] + aspect * size / 2.0, middle[1] - size / 2.0], [middle[0] + aspect * size / 2.0, middle[1] + size / 2.0], [middle[0] - aspect * size / 2.0, middle[1] + size / 2.0]];
    let free = Free { fov: false, shift: false, roll: true, ..Free::default() };
    let mut cameras: Vec<(usize, CameraParams, f64)> = Vec::with_capacity(found.len());
    // From the marked frame outwards, each way, so every frame starts
    // from its neighbour's camera.
    let at = found.iter().position(|&i| i >= track.marked).unwrap_or(0);
    let turn = found.len() - at;
    let order: Vec<usize> = found[at..].iter().copied().chain(found[..at].iter().rev().copied()).collect();
    let mut near: Option<CameraParams> = None;
    let mut first_camera: Option<CameraParams> = None;
    for (n, &i) in order.iter().enumerate() {
        if n == turn {
            // Turning back: from the marked frame's camera again.
            near = first_camera;
        }
        let image = on_canvas(&track.frames[i]);
        let marks: Vec<Mark> = (0..4).map(|k| Mark { world: [world[k][0], world[k][1], 0.0], image: image[k] }).collect();
        let closed = rect::pose(&world, &image, centre, focal).and_then(|p| camfit::from_pose(p.r, p.t, w, h, fov)).map(|c| CameraParams { shift, ..c });
        let mut best: Option<(CameraParams, f64)> = None;
        // The neighbour's camera first: of two that fit as well, the one
        // that carries on.
        for start in [near, closed].into_iter().flatten() {
            let (c, e) = camfit::fit(w, h, &marks, &start, free);
            if best.is_none_or(|b| e < b.1 * 0.95 - 1e-6) {
                best = Some((c, e));
            }
        }
        let Some((mut c, e)) = best else { continue };
        if let Some(n) = near {
            // Angles go on from the neighbour's, not round the other way.
            let wind = |v: f64, n: f64| v + ((n - v) / 360.0).round() * 360.0;
            c.orbit[0] = wind(c.orbit[0], n.orbit[0]);
            c.rotation = wind(c.rotation, n.rotation);
        }
        near = Some(c);
        first_camera.get_or_insert(c);
        cameras.push((i, c, e));
    }
    cameras.sort_by_key(|c| c.0);
    if cameras.is_empty() {
        return Err("no frame of the track gave a camera".into());
    }
    Ok(Solve { fov, lens, strength, plane: if by_motion { "motion" } else { "rectangle" }, aspect, points: world, rect, cameras })
}
