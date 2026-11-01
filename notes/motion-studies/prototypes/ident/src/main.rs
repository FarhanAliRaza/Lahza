//! Lahza ident: an original flat-geometric motion piece rendered by a tiny
//! vector engine (tiny-skia), frames in parallel, piped to ffmpeg.
//!   lahza-ident film out.mp4        render the film
//!   lahza-ident stills dir t1 t2 ..  write PNG stills
use rayon::prelude::*;
use std::f32::consts::{PI, TAU};
use std::io::Write;
use std::process::{Command, Stdio};
use tiny_skia::*;

const W: u32 = 1920;
const H: u32 = 1080;
const FPS: f32 = 60.0;
const DURATION: f32 = 16.3;
const CX: f32 = 960.0;
const CY: f32 = 540.0;
const BLUR_SAMPLES: usize = 5;
const SHUTTER: f32 = 0.5 / FPS;

const NAVY: u32 = 0x10103a;
const WHITE: u32 = 0xfffdf7;
const RED: u32 = 0xe04a5c;
const CYAN: u32 = 0x8ad6da;
const BLUE: u32 = 0x4683c1;
const YELLOW: u32 = 0xf6d10b;
const CREAM: u32 = 0xf7ead8;
const GREEN: u32 = 0x0c5f2c;
const ORANGE: u32 = 0xe2690f;
const SHADOW: u32 = 0x07071c;
const DOTS: [u32; 6] = [WHITE, RED, CYAN, BLUE, YELLOW, CREAM];

// ------------------------------------------------------------------ math
fn clamp01(v: f32) -> f32 { v.clamp(0.0, 1.0) }
fn prog(t: f32, a: f32, b: f32) -> f32 { clamp01((t - a) / (b - a)) }
fn lerp(a: f32, b: f32, p: f32) -> f32 { a + (b - a) * p }
fn ease_io(p: f32) -> f32 { if p < 0.5 { 4.0 * p * p * p } else { 1.0 - (-2.0 * p + 2.0).powi(3) / 2.0 } }
fn ease_out(p: f32) -> f32 { 1.0 - (1.0 - p).powi(3) }
fn ease_in(p: f32) -> f32 { p * p * p }
fn ease_back(p: f32) -> f32 { let c = 1.9; 1.0 + (c + 1.0) * (p - 1.0).powi(3) + c * (p - 1.0).powi(2) }
fn ease_bounce(p: f32) -> f32 {
    let (n, d) = (7.5625, 2.75);
    if p < 1.0 / d { n * p * p }
    else if p < 2.0 / d { let p = p - 1.5 / d; n * p * p + 0.75 }
    else if p < 2.5 / d { let p = p - 2.25 / d; n * p * p + 0.9375 }
    else { let p = p - 2.625 / d; n * p * p + 0.984375 }
}
fn hash(i: u32, salt: u32) -> f32 {
    let mut x = i.wrapping_mul(0x9E37_79B9) ^ salt.wrapping_mul(0x85EB_CA6B);
    x ^= x >> 16; x = x.wrapping_mul(0x7feb_352d); x ^= x >> 15; x = x.wrapping_mul(0x846c_a68b); x ^= x >> 16;
    (x & 0xff_ffff) as f32 / 16_777_216.0
}

// ------------------------------------------------------------------ canvas
struct Canvas { pm: Pixmap, cam: Transform }

fn paint(color: u32, alpha: f32) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8((color >> 16) as u8, (color >> 8) as u8, color as u8, (clamp01(alpha) * 255.0) as u8);
    p.anti_alias = true;
    p
}

impl Canvas {
    fn new() -> Self { Self { pm: Pixmap::new(W, H).unwrap(), cam: Transform::identity() } }
    fn bg(&mut self, color: u32) {
        self.pm.fill(Color::from_rgba8((color >> 16) as u8, (color >> 8) as u8, color as u8, 255));
    }
    fn fill(&mut self, path: Option<Path>, color: u32, alpha: f32, mask: Option<&Mask>) {
        if let Some(path) = path { self.pm.fill_path(&path, &paint(color, alpha), FillRule::Winding, self.cam, mask); }
    }
    fn circle(&mut self, x: f32, y: f32, r: f32, color: u32) { self.circle_a(x, y, r, color, 1.0) }
    fn circle_a(&mut self, x: f32, y: f32, r: f32, color: u32, a: f32) {
        if r > 0.05 { self.fill(PathBuilder::from_circle(x, y, r), color, a, None) }
    }
    fn ellipse(&mut self, x: f32, y: f32, rx: f32, ry: f32, color: u32) {
        if rx > 0.05 && ry > 0.05 { self.fill(Rect::from_ltrb(x - rx, y - ry, x + rx, y + ry).map(PathBuilder::from_oval).flatten(), color, 1.0, None) }
    }
    fn poly(&mut self, pts: &[(f32, f32)], color: u32) { self.fill(poly_path(pts), color, 1.0, None) }
    fn rect(&mut self, l: f32, t: f32, r: f32, b: f32, color: u32) {
        if r > l && b > t { self.fill(Rect::from_ltrb(l, t, r, b).map(PathBuilder::from_rect), color, 1.0, None) }
    }
    /// A thick line with round caps (a "pill").
    fn capsule(&mut self, a: (f32, f32), b: (f32, f32), width: f32, color: u32) {
        let mut pb = PathBuilder::new();
        pb.move_to(a.0, a.1);
        pb.line_to(b.0 + 0.01, b.1);
        if let Some(path) = pb.finish() {
            let stroke = Stroke { width, line_cap: LineCap::Round, ..Default::default() };
            self.pm.stroke_path(&path, &paint(color, 1.0), &stroke, self.cam, None);
        }
    }
    fn arc(&mut self, cx: f32, cy: f32, r: f32, start: f32, sweep: f32, width: f32, color: u32) {
        let steps = ((sweep.abs() / TAU) * 160.0).ceil().max(2.0) as usize;
        let mut pb = PathBuilder::new();
        for i in 0..=steps {
            let a = start + sweep * i as f32 / steps as f32;
            let (x, y) = (cx + r * a.cos(), cy + r * a.sin());
            if i == 0 { pb.move_to(x, y) } else { pb.line_to(x, y) }
        }
        if let Some(path) = pb.finish() {
            let cap = if sweep.abs() >= TAU - 1e-3 { LineCap::Butt } else { LineCap::Round };
            let stroke = Stroke { width, line_cap: cap, ..Default::default() };
            self.pm.stroke_path(&path, &paint(color, 1.0), &stroke, self.cam, None);
        }
    }
    /// Paint the intersection of circle A and circle B in `color`.
    fn intersection(&mut self, a: (f32, f32, f32), b: (f32, f32, f32), color: u32) {
        let mut mask = Mask::new(W, H).unwrap();
        if let Some(p) = PathBuilder::from_circle(a.0, a.1, a.2) { mask.fill_path(&p, FillRule::Winding, true, self.cam); }
        let path = PathBuilder::from_circle(b.0, b.1, b.2);
        self.fill(path, color, 1.0, Some(&mask));
    }
}

fn poly_path(pts: &[(f32, f32)]) -> Option<Path> {
    let mut pb = PathBuilder::new();
    for (i, &(x, y)) in pts.iter().enumerate() { if i == 0 { pb.move_to(x, y) } else { pb.line_to(x, y) } }
    pb.close();
    pb.finish()
}

fn rot(p: (f32, f32), c: (f32, f32), deg: f32) -> (f32, f32) {
    let (s, co) = deg.to_radians().sin_cos();
    let (dx, dy) = (p.0 - c.0, p.1 - c.1);
    (c.0 + dx * co - dy * s, c.1 + dx * s + dy * co)
}

fn camera(scale: f32, deg: f32) -> Transform {
    Transform::from_translate(CX, CY).pre_rotate(deg).pre_scale(scale, scale).pre_translate(-CX, -CY)
}

// ------------------------------------------------------------------ scene data
const COLS: u32 = 15;
const ROWS: u32 = 9;
const N: u32 = COLS * ROWS;

fn grid_pos(i: u32) -> (f32, f32) {
    let (c, r) = ((i % COLS) as f32, (i / COLS) as f32);
    (CX + (c - (COLS - 1) as f32 / 2.0) * 124.0, CY + (r - (ROWS - 1) as f32 / 2.0) * 104.0)
}
fn dot_color(i: u32) -> u32 { DOTS[(hash(i, 3) * 6.0) as usize % 6] }
fn burst_end(i: u32) -> (f32, f32) {
    let a = hash(i, 1) * TAU;
    let d = 220.0 + hash(i, 2).sqrt() * 900.0;
    (CX + a.cos() * d, CY + a.sin() * d * 0.75)
}

// ------------------------------------------------------------------ scenes
fn spark(c: &mut Canvas, t: f32) {
    c.bg(NAVY);
    // A full-frame white disc collapses into a single point.
    if t < 1.16 {
        let r = lerp(1250.0, 24.0, ease_io(prog(t, 0.0, 0.5)));
        let pulse = 1.0 + 0.25 * (prog(t, 0.95, 1.16) * PI).sin();
        c.circle(CX, CY, r * pulse, WHITE);
    }
    // A red stroke spirals in and closes a ring around it.
    let p = prog(t, 0.22, 0.98);
    if p > 0.0 && t < 1.16 {
        let e = ease_io(p);
        let r = lerp(760.0, 64.0, ease_out(p));
        let sweep = lerp(0.35, TAU, e);
        let start = lerp(-2.2, 2.6, e);
        let width = lerp(70.0, 30.0, e) * (1.0 + 0.35 * (prog(t, 0.98, 1.16) * PI).sin());
        c.arc(CX, CY, r, start, sweep, width, RED);
    }
    // The ring bursts into streaking confetti.
    let q = prog(t, 1.12, 1.62);
    if q > 0.0 {
        for i in 0..N {
            let end = burst_end(i);
            let e = ease_out(q);
            let (x, y) = (lerp(CX, end.0, e), lerp(CY, end.1, e));
            let len = (1.0 - e) * 70.0;
            let (dx, dy) = (end.0 - CX, end.1 - CY);
            let n = (dx * dx + dy * dy).sqrt().max(1.0);
            c.capsule((x - dx / n * len, y - dy / n * len), (x, y), 9.0 + hash(i, 5) * 5.0, dot_color(i));
        }
    }
}

fn dot_position(i: u32, t: f32) -> (f32, f32) {
    let delay = hash(i, 7) * 0.3;
    let e = ease_io(prog(t, 1.55 + delay, 2.45 + delay));
    let (s, g) = (burst_end(i), grid_pos(i));
    let wave = 9.0 * prog(t, 2.3, 2.8) * ((g.0 * 0.012) - t * 7.0).sin();
    (lerp(s.0, g.0, e), lerp(s.1, g.1, e) + wave)
}

fn grid(c: &mut Canvas, t: f32) {
    c.bg(NAVY);
    let turn = ease_io(prog(t, 3.55, 4.62));
    c.cam = camera(lerp(1.0, 2.1, turn), lerp(0.0, 74.0, turn));
    let shadow = 1.0 - prog(t, 3.6, 4.1);
    for i in 0..N {
        let (x, y) = dot_position(i, t);
        let (col, row) = (i % COLS, i / COLS);
        let d = (col + row) as f32 * 0.011;
        let grow = ease_back(prog(t, 2.95 + d, 3.4 + d));
        let h = grow * lerp(54.0, 70.0, turn);
        let r = 11.0;
        if h < 0.5 {
            c.circle(x, y, r, dot_color(i));
            continue;
        }
        if shadow > 0.0 {
            let s = (x + h * 0.85, y + h * 0.32);
            c.capsule((x, y), s, r * 2.0, SHADOW);
        }
        c.ellipse(x, y, r, r * 0.42, CYAN);
        c.rect(x - r, y - h, x + r, y, CYAN);
        c.ellipse(x, y - h, r, r * 0.42, dot_color(i));
    }
}

struct Tube { row: usize, x: f32, len: f32, back: u32 }
const ROW_Y: [f32; 3] = [170.0, 548.0, 925.0];
const ROW_R: [f32; 3] = [84.0, 104.0, 84.0];
const ROW_V: [f32; 3] = [300.0, 170.0, 360.0];

fn tubes() -> Vec<Tube> {
    let mut out = Vec::new();
    for row in 0..3 {
        let mut x = -300.0 + hash(row as u32, 11) * 300.0;
        let mut k = 0;
        while x < 3600.0 {
            let len = 700.0 + hash(row as u32 * 31 + k, 12) * 600.0;
            out.push(Tube { row, x, len, back: [BLUE, RED, CREAM, BLUE][k as usize % 4] });
            x += len + 150.0 + hash(row as u32 * 17 + k, 13) * 160.0;
            k += 1;
        }
    }
    out
}

fn draw_tube(c: &mut Canvas, x0: f32, x1: f32, y: f32, r: f32, back: u32, roll: f32) {
    c.ellipse(x0, y, r * 0.34, r, back);
    c.rect(x0, y - r, x1, y + r, CYAN);
    // A cream band travels along the tube so it reads as rolling.
    let span = x1 - x0;
    let bx = x0 + r * 0.5 + (roll.rem_euclid(1.0)) * (span - r * 1.4);
    c.rect(bx - 44.0, y - r, bx, y + r, CREAM);
    c.ellipse(bx, y, r * 0.34, r, CREAM);
    c.ellipse(bx + 2.0, y, r * 0.34, r * 0.98, CYAN);
    c.rect(bx + 2.0, y - r * 0.98, bx + 2.0 + r * 0.34, y + r * 0.98, CYAN);
    c.ellipse(x1, y, r * 0.34, r, WHITE);
}

const BALLS: [(u32, f32); 6] = [(BLUE, 34.0), (WHITE, 30.0), (CREAM, 33.0), (RED, 28.0), (YELLOW, 31.0), (CYAN, 36.0)];

fn ball_on_tube(k: usize, t: f32) -> Option<(f32, f32)> {
    let t0 = 5.45 + k as f32 * 0.16;
    if t < t0 { return None; }
    let r = BALLS[k].1;
    let top = ROW_Y[1] - ROW_R[1] - r;
    let x = 760.0 + (hash(k as u32, 21) - 0.5) * 760.0 - (t - t0) * 60.0;
    let fall = prog(t, t0, t0 + 0.35);
    if fall < 1.0 { return Some((x, lerp(-60.0, top, ease_in(fall)))); }
    let hop = ((t - t0 - 0.35) * (4.2 + hash(k as u32, 22) * 2.0) * PI).sin().abs();
    Some((x, top - hop * (110.0 + hash(k as u32, 23) * 120.0)))
}

fn rollers(c: &mut Canvas, t: f32) {
    c.bg(NAVY);
    let settle = ease_out(prog(t, 4.62, 5.4));
    c.cam = camera(lerp(1.35, 1.0, settle), lerp(-16.0, 0.0, settle));
    let exit = ease_in(prog(t, 7.05, 7.75));
    for tube in tubes() {
        let travel = (t - 4.62) * ROW_V[tube.row] + exit * 2600.0;
        let (x0, x1) = (tube.x - travel, tube.x - travel + tube.len);
        if x1 < -200.0 || x0 > 2200.0 { continue; }
        draw_tube(c, x0, x1, ROW_Y[tube.row], ROW_R[tube.row], tube.back, (t - 4.62) * 0.55 + hash(tube.x as u32, 9));
    }
    c.cam = Transform::identity();
    for k in 0..BALLS.len() {
        if let Some((x, y)) = ball_on_tube(k, t) {
            let gather = ease_io(prog(t, 7.2, 7.75));
            let (x, y) = (lerp(x, CX, gather), lerp(y, CY, gather));
            c.circle(x, y, BALLS[k].1, BALLS[k].0);
        }
    }
}

fn orbit(c: &mut Canvas, t: f32, front: bool, scale_in: f32) {
    for k in 0..7u32 {
        let base = hash(k, 31) * TAU;
        let a = base + (t - 7.4) * (1.1 + hash(k, 32) * 0.9);
        let depth = a.sin();
        if (depth >= 0.0) != front { continue; }
        let spread = lerp(3.2, 1.0, scale_in);
        let (rx, ry) = (190.0 + hash(k, 33) * 80.0, 70.0 + hash(k, 34) * 40.0);
        let p = rot((CX + a.cos() * rx * spread, CY + depth * ry * spread), (CX, CY), -24.0);
        let r = (16.0 + hash(k, 35) * 34.0) * (1.0 + 0.25 * depth);
        c.circle(p.0, p.1, r, [CYAN, BLUE, CREAM, YELLOW, RED, CYAN, CREAM][k as usize]);
    }
}

fn gather(c: &mut Canvas, t: f32) {
    c.bg(NAVY);
    let s = ease_out(prog(t, 7.35, 8.0));
    orbit(c, t, false, s);
    c.circle(CX, CY, 112.0 * ease_back(prog(t, 7.35, 7.85)), WHITE);
    orbit(c, t, true, s);
    // Cyan floods outward from behind the sphere.
    let flood = ease_io(prog(t, 8.75, 9.3));
    if flood > 0.0 {
        c.circle(CX, CY, flood * 1200.0, CYAN);
        c.circle(CX, CY, 112.0 + flood * 40.0, NAVY);
        c.circle(CX, CY, 112.0 * (1.0 - flood * 0.15), WHITE);
    }
}

/// A tumbling hemisphere: dome half-disc plus a rim ellipse that flips.
fn hemisphere(c: &mut Canvas, x: f32, y: f32, r: f32, axis_deg: f32, phase: f32) {
    let k = phase.cos();
    let mut pb = PathBuilder::new();
    let steps = 64;
    for i in 0..=steps {
        let a = PI * i as f32 / steps as f32;
        let p = rot((x + r * a.cos(), y + r * a.sin()), (x, y), axis_deg);
        if i == 0 { pb.move_to(p.0, p.1) } else { pb.line_to(p.0, p.1) }
    }
    for i in 0..=steps {
        let a = PI + PI * i as f32 / steps as f32;
        let dome = if k < 0.0 { 1.0 } else { k.abs().max(0.02) };
        let p = rot((x + r * a.cos(), y + r * a.sin() * dome), (x, y), axis_deg);
        pb.line_to(p.0, p.1);
    }
    pb.close();
    c.fill(pb.finish(), WHITE, 1.0, None);
    if k > 0.0 {
        // The flat face is turned toward us.
        let mut pb = PathBuilder::new();
        for i in 0..=steps * 2 {
            let a = TAU * i as f32 / (steps * 2) as f32;
            let p = rot((x + r * a.cos(), y + r * a.sin() * k), (x, y), axis_deg);
            if i == 0 { pb.move_to(p.0, p.1) } else { pb.line_to(p.0, p.1) }
        }
        pb.close();
        c.fill(pb.finish(), BLUE, 1.0, None);
        let mut pb = PathBuilder::new();
        for i in 0..=steps * 2 {
            let a = TAU * i as f32 / (steps * 2) as f32;
            let p = rot((x + r * 0.84 * a.cos(), y + r * 0.84 * a.sin() * k + r * 0.06 * k), (x, y), axis_deg);
            if i == 0 { pb.move_to(p.0, p.1) } else { pb.line_to(p.0, p.1) }
        }
        pb.close();
        c.fill(pb.finish(), CREAM, 1.0, None);
        let rim = rot((x, y - r * k * 0.15), (x, y), axis_deg);
        c.circle(rim.0, rim.1, 14.0, YELLOW);
    } else {
        let rim = rot((x - r * 0.3, y + r * 0.35), (x, y), axis_deg);
        c.circle(rim.0, rim.1, 14.0, YELLOW);
    }
}

fn lens(c: &mut Canvas, t: f32) {
    c.bg(CYAN);
    // Wobbling navy body.
    let settle = ease_io(prog(t, 10.45, 10.95));
    let mut pts = Vec::new();
    for i in 0..96 {
        let a = TAU * i as f32 / 96.0;
        let wob = (1.0 - settle) * (8.0 * (3.0 * a + t * 3.0).sin() + 6.0 * (5.0 * a - t * 2.0).sin());
        let r = lerp(205.0, 150.0, settle) + wob;
        pts.push((CX + r * a.cos(), CY + r * a.sin()));
    }
    c.poly(&pts, NAVY);
    let tumble = prog(t, 9.25, 10.6);
    let axis = lerp(35.0, -15.0, ease_io(tumble));
    if settle < 1.0 {
        hemisphere(c, CX + 18.0 * (1.0 - settle), CY - 10.0 * (1.0 - settle), lerp(122.0, 108.0, settle), axis, ease_io(tumble) * TAU * 1.5 + PI * 0.35);
    }
    if settle > 0.0 { c.circle_a(CX, CY, 108.0, WHITE, settle); }
    for k in 0..4u32 {
        let a = hash(k, 41) * TAU + t * 0.6;
        let d = 230.0 + hash(k, 42) * 140.0;
        let bob = 12.0 * (t * 2.0 + k as f32).sin();
        c.circle(CX + a.cos() * d, CY + a.sin() * d * 0.7 + bob, 13.0 + hash(k, 43) * 12.0, [RED, CREAM, YELLOW, RED][k as usize]);
    }
    // White blooms from the lens centre into the next scene.
    let bloom = ease_in(prog(t, 10.95, 11.35));
    if bloom > 0.0 { c.circle(CX, CY, lerp(108.0, 1250.0, bloom), WHITE); }
}

fn overlap(c: &mut Canvas, t: f32) {
    c.bg(WHITE);
    let speed = 4.09;
    let zoom = (speed * (t - 12.2).max(0.0)).exp();
    let open = ease_back(prog(t, 11.25, 11.85));
    let drift = ease_io(prog(t, 11.8, 12.6));
    c.cam = camera(zoom, lerp(0.0, -10.0, drift) + (zoom.ln()) * 6.0);
    let sp = |x: f32, lift: f32| (CX + x * open, CY + lift * drift);
    let yellow = (CX, CY, 150.0 * open.max(0.01));
    let mut groups = Vec::new();
    for side in [-1.0f32, 1.0] {
        let b = sp(side * 228.0, side * 26.0);
        let y2 = sp(side * 338.0, side * 38.0);
        let cy = sp(side * 400.0, side * 44.0);
        groups.push(((b.0, b.1, 108.0 * open), (y2.0, y2.1, 52.0 * open), (cy.0, cy.1, 22.0 * open)));
    }
    for &(blue, small, tiny) in &groups {
        c.circle(tiny.0, tiny.1, tiny.2, CYAN);
        c.circle(small.0, small.1, small.2, YELLOW);
        c.intersection(small, tiny, NAVY);
        c.circle(blue.0, blue.1, blue.2, BLUE);
        c.intersection(blue, small, RED);
    }
    c.circle(yellow.0, yellow.1, yellow.2, YELLOW);
    for &(blue, _, _) in &groups { c.intersection(yellow, blue, NAVY); }
    // The zoom tunnel: nested discs that each fill the frame in turn.
    let tunnel = [CREAM, CYAN, YELLOW, WHITE, CREAM];
    let accents = [BLUE, RED, NAVY, GREEN, BLUE];
    for (j, &color) in tunnel.iter().enumerate() {
        let r = 150.0 * 0.27f32.powi(j as i32 + 1) * open;
        let a = (35.0 + 75.0 * j as f32).to_radians();
        c.circle(CX + a.cos() * r * 1.6, CY + a.sin() * r * 1.6, r * 0.42, accents[j]);
        c.circle(CX, CY, r, color);
    }
}

/// Block letters for LAHZA, each built from rectangles and polygons.
fn logo(c: &mut Canvas, t: f32) {
    c.bg(WHITE);
    let (cap, s) = (190.0, 50.0);
    let widths = [128.0, 176.0, 160.0, 150.0, 176.0];
    let colors = [RED, BLUE, GREEN, ORANGE, RED];
    let gap = 18.0;
    let total: f32 = widths.iter().sum::<f32>() + gap * 4.0;
    let shift = -95.0 * ease_io(prog(t, 14.8, 15.4));
    let mut x = CX - total / 2.0 + shift;
    let base = CY + cap / 2.0 + 30.0;
    for (i, &w) in widths.iter().enumerate() {
        let start = 14.05 + i as f32 * 0.075;
        let g = ease_back(prog(t, start, start + 0.45));
        let h = cap * g;
        let top = base - h;
        let col = colors[i];
        match i {
            0 => { // L
                c.rect(x, top, x + s, base, col);
                let bw = w * ease_back(prog(t, start + 0.12, start + 0.5));
                c.rect(x, base - s, x + bw, base, col);
            }
            1 | 4 => { // A
                let apex = (x + w / 2.0, top);
                let mut pb = PathBuilder::new();
                pb.move_to(x, base); pb.line_to(apex.0, apex.1); pb.line_to(x + w, base); pb.close();
                let inner_top = top + s * 1.9 * g;
                pb.move_to(x + s * 1.15, base); pb.line_to(apex.0, inner_top); pb.line_to(x + w - s * 1.15, base); pb.close();
                if let Some(path) = pb.finish() { c.pm.fill_path(&path, &paint(col, 1.0), FillRule::EvenOdd, c.cam, None); }
                let bar = ease_out(prog(t, start + 0.2, start + 0.5));
                let by = base - cap * 0.36;
                let outer = (w / 2.0) * ((by - s * 0.32 - top) / (base - top).max(1.0)).max(0.0);
                let half = (w * 0.31).min(outer) * bar;
                c.rect(apex.0 - half, by - s * 0.32, apex.0 + half, by + s * 0.32, col);
            }
            2 => { // H
                c.rect(x, top, x + s, base, col);
                c.rect(x + w - s, top, x + w, base, col);
                let bar = ease_out(prog(t, start + 0.15, start + 0.5));
                c.rect(x + s, base - cap * 0.58, x + s + (w - 2.0 * s) * bar, base - cap * 0.58 + s, col);
            }
            _ => { // Z
                let bw = w * g;
                c.rect(x, base - cap, x + bw, base - cap + s, col);
                c.rect(x + w - bw, base - s, x + w, base, col);
                let d = ease_out(prog(t, start + 0.18, start + 0.52));
                if d > 0.0 {
                    let from = (x + w - s * 1.1, base - cap + s);
                    let to = (x, base - s);
                    let end = (lerp(from.0, to.0, d), lerp(from.1, to.1, d));
                    c.poly(&[from, (from.0 + s * 1.1, from.1), (end.0 + s * 1.1, end.1), end], col);
                }
            }
        }
        x += w + gap;
    }
    // Three dots drop in above the word: a moment.
    let dots = [(RED, -70.0), (BLUE, 0.0), (GREEN, 70.0)];
    for (k, &(col, dx)) in dots.iter().enumerate() {
        let start = 14.85 + k as f32 * 0.09;
        let p = prog(t, start, start + 0.7);
        if p <= 0.0 { continue; }
        let y = lerp(-80.0, base - 24.0, ease_bounce(p));
        let squash: f32 = if p > 0.33 && p < 0.42 { 0.82 } else { 1.0 };
        c.ellipse(CX + total / 2.0 + shift + 40.0 + (dx + 70.0) * 0.86, y, 24.0 / squash.sqrt(), 24.0 * squash, col);
    }
}

fn render_at(t: f32) -> Pixmap {
    let mut c = Canvas::new();
    match t {
        t if t < 1.6 => spark(&mut c, t),
        t if t < 4.62 => { if t < 1.65 { spark(&mut c, t) } else { grid(&mut c, t) } }
        t if t < 7.35 => rollers(&mut c, t),
        t if t < 9.3 => gather(&mut c, t),
        t if t < 11.25 => lens(&mut c, t),
        t if t < 14.05 => overlap(&mut c, t),
        t => logo(&mut c, t),
    }
    c.pm
}

/// Average several shutter samples for motion blur.
fn frame(f: u32) -> Vec<u8> {
    let t = f as f32 / FPS;
    let mut acc = vec![0u32; (W * H * 4) as usize];
    for k in 0..BLUR_SAMPLES {
        let dt = (k as f32 / (BLUR_SAMPLES - 1) as f32 - 0.5) * SHUTTER;
        let pm = render_at((t + dt).max(0.0));
        for (a, &v) in acc.iter_mut().zip(pm.data()) { *a += v as u32; }
    }
    acc.into_iter().map(|v| (v / BLUR_SAMPLES as u32) as u8).collect()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("stills") => {
            let dir = &args[2];
            std::fs::create_dir_all(dir).unwrap();
            args[3..].par_iter().for_each(|t| {
                let time: f32 = t.parse().unwrap();
                render_at(time).save_png(format!("{dir}/t_{t}.png")).unwrap();
            });
        }
        Some("film") => {
            let out = &args[2];
            let frames = (DURATION * FPS) as u32;
            let start = std::time::Instant::now();
            let mut ff = Command::new("ffmpeg")
                .args(["-v", "error", "-y", "-f", "rawvideo", "-pix_fmt", "rgba", "-s", &format!("{W}x{H}"), "-r", "60", "-i", "-",
                       "-c:v", "h264_nvenc", "-preset", "p7", "-tune", "hq", "-rc", "vbr", "-cq", "16", "-b:v", "0",
                       "-pix_fmt", "yuv420p", "-movflags", "+faststart", out])
                .stdin(Stdio::piped()).spawn().expect("ffmpeg");
            let stdin = ff.stdin.as_mut().unwrap();
            for chunk in (0..frames).collect::<Vec<_>>().chunks(48) {
                let rendered: Vec<Vec<u8>> = chunk.par_iter().map(|&f| frame(f)).collect();
                for data in rendered { stdin.write_all(&data).unwrap(); }
            }
            drop(ff.stdin.take());
            assert!(ff.wait().unwrap().success());
            eprintln!("rendered {frames} frames ({DURATION}s @ 60fps, {BLUR_SAMPLES}x motion blur) in {:.1}s", start.elapsed().as_secs_f32());
        }
        _ => eprintln!("usage: lahza-ident film <out.mp4> | stills <dir> <t>..."),
    }
}
