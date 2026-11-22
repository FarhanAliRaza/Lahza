//! Made-up footage with the answer known: a textured plane moved by a
//! given homography over a background that moves another way.

use crate::image::Gray;
use crate::plane::{Options, PlaneTracker};
use crate::rect;
use crate::{Quad, H3};

fn hash(x: i64, y: i64, seed: u64) -> f64 {
    let mut h = (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ (y as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F) ^ seed.wrapping_mul(0x1656_67B1_9E37_79F9);
    h ^= h >> 31;
    h = h.wrapping_mul(0x7FB5_D329_728E_A185);
    h ^= h >> 27;
    h = h.wrapping_mul(0x81DA_DEF4_BC2D_D44D);
    h ^= h >> 33;
    (h >> 11) as f64 / (1u64 << 53) as f64
}

/// Smooth noise in 0–1, a few octaves: wood grain and things on a table,
/// near enough.
fn texture(x: f64, y: f64, seed: u64) -> f64 {
    let mut v = 0.0;
    let mut amp = 0.5;
    let mut cell = 48.0;
    for o in 0..4 {
        let (fx, fy) = (x / cell, y / cell);
        let (ix, iy) = (fx.floor(), fy.floor());
        let (tx, ty) = (fx - ix, fy - iy);
        let (sx, sy) = (tx * tx * (3.0 - 2.0 * tx), ty * ty * (3.0 - 2.0 * ty));
        let (ix, iy, s) = (ix as i64, iy as i64, seed + o);
        let top = hash(ix, iy, s) + (hash(ix + 1, iy, s) - hash(ix, iy, s)) * sx;
        let bottom = hash(ix, iy + 1, s) + (hash(ix + 1, iy + 1, s) - hash(ix, iy + 1, s)) * sx;
        v += amp * (top + (bottom - top) * sy);
        amp *= 0.5;
        cell *= 0.5;
    }
    let mut v = v / 0.9375;
    // Things with edges, scattered: a block or a disc in some cells.
    let (cx, cy) = ((x / 37.0).floor() as i64, (y / 37.0).floor() as i64);
    let pick = hash(cx, cy, seed + 50);
    if pick < 0.45 {
        let (ox, oy) = ((cx as f64 + 0.3 + 0.4 * hash(cx, cy, seed + 51)) * 37.0, (cy as f64 + 0.3 + 0.4 * hash(cx, cy, seed + 52)) * 37.0);
        let r = 5.0 + 6.0 * hash(cx, cy, seed + 53);
        // Edges a pixel and a half soft, as a lens leaves them (a hard
        // edge would land on a third of a pixel, sample by sample, and
        // the answer the tests hold the tracker to would be that coarse).
        let d = if pick < 0.2 { ((x - ox).abs() - r).max((y - oy).abs() - r * 0.7) } else { ((x - ox).powi(2) + (y - oy).powi(2)).sqrt() - r };
        let cover = (0.5 - d / 1.5).clamp(0.0, 1.0);
        let cover = cover * cover * (3.0 - 2.0 * cover);
        v += (0.15 + 0.7 * hash(cx, cy, seed + 54) - v) * cover;
    }
    v
}

fn inside(q: &Quad, x: f64, y: f64) -> bool {
    (0..4).all(|i| {
        let (a, b) = (q[i], q[(i + 1) % 4]);
        (b[0] - a[0]) * (y - a[1]) - (b[1] - a[1]) * (x - a[0]) >= 0.0
    })
}

struct Shot {
    w: usize,
    h: usize,
    /// The plane's region in its own picture (frame 0's coordinates).
    region: Quad,
    noise: f64,
}

impl Shot {
    /// The frame in which the plane has moved by `plane` and the rest by
    /// `back`, both from frame 0.
    fn frame(&self, plane: H3, back: H3, index: u64, gain: f64, bias: f64) -> Gray {
        let (pi, bi) = (plane.inverse().unwrap(), back.inverse().unwrap());
        let mut g = Gray::new(self.w, self.h);
        // Nine samples a pixel: a lens does not give stair-stepped edges.
        let taps = [-1.0 / 3.0, 0.0, 1.0 / 3.0];
        for y in 0..self.h {
            for x in 0..self.w {
                let mut v = 0.0;
                for dy in taps {
                    for dx in taps {
                        let (sx, sy) = (x as f64 + dx, y as f64 + dy);
                        let p = pi.apply([sx, sy]);
                        v += if inside(&self.region, p[0], p[1]) {
                            texture(p[0], p[1], 7)
                        } else {
                            let b = bi.apply([sx, sy]);
                            texture(b[0], b[1], 99)
                        };
                    }
                }
                let v = v / 9.0;
                let n = (hash(x as i64, y as i64, 1000 + index) - 0.5) * 2.0 * self.noise;
                g.px[y * self.w + x] = ((v * 200.0 + 20.0) * gain + bias + n).clamp(0.0, 255.0) as f32;
            }
        }
        g
    }
}

fn around(c: [f64; 2], m: H3) -> H3 {
    H3([1.0, 0.0, c[0], 0.0, 1.0, c[1], 0.0, 0.0, 1.0]).then(m).then(H3([1.0, 0.0, -c[0], 0.0, 1.0, -c[1], 0.0, 0.0, 1.0]))
}

/// A camera-like move of the plane at time `t` (0–1): a slide, a turn, a
/// change of size and some perspective.
fn motion(t: f64, centre: [f64; 2], slide: [f64; 2], turn: f64, zoom: f64, tilt: [f64; 2]) -> H3 {
    let (s, c) = (turn * t).to_radians().sin_cos();
    let z = 1.0 + (zoom - 1.0) * t;
    let m = H3([c * z, -s * z, 0.0, s * z, c * z, 0.0, tilt[0] * t, tilt[1] * t, 1.0]);
    H3([1.0, 0.0, slide[0] * t, 0.0, 1.0, slide[1] * t, 0.0, 0.0, 1.0]).then(around(centre, m))
}

fn worst(a: &Quad, b: &Quad) -> f64 {
    (0..4).map(|i| ((a[i][0] - b[i][0]).powi(2) + (a[i][1] - b[i][1]).powi(2)).sqrt()).fold(0.0, f64::max)
}

const REGION: Quad = [[110.0, 60.0], [220.0, 70.0], [215.0, 140.0], [100.0, 130.0]];

fn shot() -> Shot {
    Shot { w: 320, h: 180, region: REGION, noise: 2.0 }
}

#[test]
fn four_corners_make_a_homography() {
    let to: Quad = [[10.0, 20.0], [300.0, 40.0], [280.0, 170.0], [30.0, 150.0]];
    let h = H3::from_quads(&REGION, &to).unwrap();
    assert!(worst(&h.quad(&REGION), &to) < 1e-9);
    let back = h.inverse().unwrap();
    assert!(worst(&back.quad(&to), &REGION) < 1e-9);
    // Three corners in a line make none.
    assert!(H3::from_quads(&[[0.0, 0.0], [1.0, 0.0], [2.0, 0.0], [0.0, 1.0]], &to).is_none());
}

/// A plane that slides, turns, grows and tilts over forty frames, on a
/// background that goes the other way, is held to a fraction of a pixel
/// against the marked frame.
#[test]
fn a_moving_plane_is_followed() {
    let s = shot();
    let centre = [160.0, 100.0];
    let first = s.frame(H3::IDENTITY, H3::IDENTITY, 0, 1.0, 0.0);
    let mut tracker = PlaneTracker::new(&first, REGION, Options::default()).unwrap();
    let mut worst_error = 0.0f64;
    for i in 1..=40u64 {
        let t = i as f64 / 40.0;
        let plane = motion(t, centre, [30.0, -18.0], 14.0, 1.25, [0.0006, -0.0004]);
        let back = motion(t, centre, [-25.0, 10.0], -6.0, 0.9, [0.0, 0.0]);
        let out = tracker.step(&s.frame(plane, back, i, 1.0, 0.0));
        assert!(out.found, "frame {i}: lost (score {:.3})", out.score);
        let e = worst(&out.corners, &plane.quad(&REGION));
        worst_error = worst_error.max(e);
    }
    assert!(worst_error < 0.25, "worst corner error {worst_error:.3} px");
    assert_eq!(tracker.rekeys, 0, "the template should have held");
}

/// Backwards is the same as forwards: the frames are only given in the
/// other order.
#[test]
fn a_track_runs_backwards_from_the_marked_frame() {
    let s = shot();
    let centre = [160.0, 100.0];
    let at = |i: u64| motion(i as f64 / 20.0, centre, [24.0, 12.0], -10.0, 1.1, [0.0003, 0.0002]);
    // Marked on frame 20; the corners there.
    let marked = at(20).quad(&REGION);
    let mut tracker = PlaneTracker::new(&s.frame(at(20), H3::IDENTITY, 20, 1.0, 0.0), marked, Options::default()).unwrap();
    for i in (0..20u64).rev() {
        let out = tracker.step(&s.frame(at(i), H3::IDENTITY, i, 1.0, 0.0));
        assert!(out.found, "frame {i}");
        let e = worst(&out.corners, &at(i).quad(&REGION));
        assert!(e < 0.3, "frame {i}: {e:.3} px");
    }
}

/// The light changes (brighter, then dimmer and lifted) and a thing
/// passes over a quarter of the region: the plane stays where it is.
#[test]
fn a_changing_light_and_a_passing_hand_do_not_move_it() {
    let s = shot();
    let centre = [160.0, 100.0];
    let first = s.frame(H3::IDENTITY, H3::IDENTITY, 0, 1.0, 0.0);
    let mut tracker = PlaneTracker::new(&first, REGION, Options::default()).unwrap();
    let mut least_agree = 1.0f64;
    for i in 1..=30u64 {
        let t = i as f64 / 30.0;
        let plane = motion(t, centre, [12.0, 8.0], 5.0, 1.05, [0.0002, 0.0]);
        let gain = 1.0 + 0.35 * (t * 6.0).sin();
        let mut frame = s.frame(plane, H3::IDENTITY, i, gain, 20.0 * t);
        // A flat grey block crossing the region.
        let (bx, by) = ((90.0 + 110.0 * t) as usize, 75usize);
        for y in by..by + 45 {
            for x in bx..(bx + 40).min(s.w) {
                frame.px[y * s.w + x] = 60.0;
            }
        }
        let out = tracker.step(&frame);
        assert!(out.found, "frame {i}: lost (score {:.3})", out.score);
        let e = worst(&out.corners, &plane.quad(&REGION));
        assert!(e < 0.6, "frame {i}: {e:.3} px (agree {:.2})", out.agree);
        least_agree = least_agree.min(out.agree);
    }
    assert!(least_agree < 0.9, "the block should have been told apart: {least_agree:.2}");
}

/// Twenty-five pixels a frame in a 320-pixel frame, with a start and a
/// stop: more than the pyramid reaches from the last place.
#[test]
fn a_fast_move_is_caught() {
    let s = Shot { w: 320, h: 180, region: [[40.0, 60.0], [120.0, 62.0], [118.0, 120.0], [38.0, 118.0]], noise: 2.0 };
    let first = s.frame(H3::IDENTITY, H3::IDENTITY, 0, 1.0, 0.0);
    let mut tracker = PlaneTracker::new(&first, s.region, Options::default()).unwrap();
    let mut x = 0.0;
    for i in 1..=8u64 {
        // Still for two frames, then at once fast.
        if i > 2 {
            x += 25.0;
        }
        let plane = H3([1.0, 0.0, x, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]);
        let out = tracker.step(&s.frame(plane, H3::IDENTITY, i, 1.0, 0.0));
        assert!(out.found, "frame {i}: lost at x {x}");
        let e = worst(&out.corners, &plane.quad(&s.region));
        assert!(e < 0.3, "frame {i}: {e:.3} px");
    }
}

/// The camera pushes in until the region is three times the size: no
/// new template (the frame is read from further down its pyramid, like
/// against like), so nothing is carried over, and the corners hold.
#[test]
fn a_push_in_is_held_against_the_marked_frame() {
    let s = Shot { w: 640, h: 360, region: [[260.0, 140.0], [380.0, 144.0], [376.0, 220.0], [256.0, 216.0]], noise: 2.0 };
    let centre = [320.0, 180.0];
    let first = s.frame(H3::IDENTITY, H3::IDENTITY, 0, 1.0, 0.0);
    let mut tracker = PlaneTracker::new(&first, s.region, Options::default()).unwrap();
    let mut worst_error = 0.0f64;
    for i in (2..=60u64).step_by(2) {
        let plane = motion(i as f64 / 60.0, centre, [0.0, 0.0], 0.0, 3.0, [0.0, 0.0]);
        let out = tracker.step(&s.frame(plane, plane, i, 1.0, 0.0));
        assert!(out.found, "frame {i}");
        // In the marked frame's pixels: the region is up to three times
        // the size here.
        let size = 1.0 + 2.0 * i as f64 / 60.0;
        worst_error = worst_error.max(worst(&out.corners, &plane.quad(&s.region)) / size);
    }
    assert_eq!(tracker.rekeys, 0);
    assert!(worst_error < 0.2, "worst corner error {worst_error:.3} px of the marked frame");
}

/// Pushed in to five times the size, the template is taken again on the
/// way, and the track goes on.
#[test]
fn far_past_its_size_the_template_is_taken_again() {
    let s = Shot { w: 640, h: 360, region: [[275.0, 150.0], [365.0, 152.0], [363.0, 208.0], [273.0, 206.0]], noise: 2.0 };
    let centre = [320.0, 180.0];
    let first = s.frame(H3::IDENTITY, H3::IDENTITY, 0, 1.0, 0.0);
    let mut tracker = PlaneTracker::new(&first, s.region, Options::default()).unwrap();
    for i in 1..=40u64 {
        let zoom = 1.0 + 4.0 * i as f64 / 40.0;
        let plane = motion(1.0, centre, [0.0, 0.0], 0.0, zoom, [0.0, 0.0]);
        let out = tracker.step(&s.frame(plane, plane, i, 1.0, 0.0));
        assert!(out.found, "frame {i} (zoom {zoom:.2})");
        let e = worst(&out.corners, &plane.quad(&s.region)) / zoom;
        assert!(e < 0.5, "frame {i}: {e:.3} px of the marked frame");
    }
    assert!(tracker.rekeys >= 1, "the template was never taken again");
}

/// A cut to another picture is not found, and the plane is found again
/// when the shot comes back.
#[test]
fn a_cut_away_is_not_found_and_the_shot_is_found_again() {
    let s = shot();
    let first = s.frame(H3::IDENTITY, H3::IDENTITY, 0, 1.0, 0.0);
    let mut tracker = PlaneTracker::new(&first, REGION, Options::default()).unwrap();
    let slide = |x: f64| H3([1.0, 0.0, x, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0]);
    assert!(tracker.step(&s.frame(slide(2.0), H3::IDENTITY, 1, 1.0, 0.0)).found);
    // Another picture altogether.
    let other = Shot { w: 320, h: 180, region: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]], noise: 2.0 };
    for i in 2..5u64 {
        let away = other.frame(H3::IDENTITY, slide(1000.0 * i as f64), i, 1.0, 0.0);
        let out = tracker.step(&away);
        assert!(!out.found, "frame {i}: found in another picture (score {:.3})", out.score);
    }
    let out = tracker.step(&s.frame(slide(6.0), H3::IDENTITY, 5, 1.0, 0.0));
    assert!(out.found, "not found again (score {:.3})", out.score);
    assert!(worst(&out.corners, &slide(6.0).quad(&REGION)) < 0.3);
}

#[test]
fn a_flat_region_is_refused() {
    let flat = Gray { w: 320, h: 180, px: vec![128.0; 320 * 180] };
    let err = PlaneTracker::new(&flat, REGION, Options::default()).err().unwrap();
    assert!(err.contains("flat"), "{err}");
    let s = shot();
    let first = s.frame(H3::IDENTITY, H3::IDENTITY, 0, 1.0, 0.0);
    // Corners that cross.
    let crossed: Quad = [REGION[0], REGION[2], REGION[1], REGION[3]];
    assert!(PlaneTracker::new(&first, crossed, Options::default()).is_err());
}

// --------------------------------------------------------------- still

/// A camera that has not moved, with a thing sliding across a third of
/// the picture, is still; the same picture a pixel and a half over is
/// not, nor is another picture.
#[test]
fn a_still_camera_is_told_from_one_that_moved() {
    use crate::still::{StillTest, STILL};
    // The "plane" here is a thing that moves in front of a still scene.
    let s = Shot { w: 320, h: 180, region: [[90.0, 40.0], [200.0, 40.0], [200.0, 150.0], [90.0, 150.0]], noise: 2.0 };
    let slide = |x: f64, y: f64| H3([1.0, 0.0, x, 0.0, 1.0, y, 0.0, 0.0, 1.0]);
    let test = StillTest::new(&s.frame(H3::IDENTITY, H3::IDENTITY, 0, 1.0, 0.0));
    for i in 1..6u64 {
        let moved_thing = s.frame(slide(14.0 * i as f64, 3.0 * i as f64), H3::IDENTITY, i, 1.0 + 0.05 * i as f64, 4.0);
        let share = test.share(&moved_thing);
        assert!(share >= 3.0 * STILL, "frame {i}: a still camera read as moved ({share:.2})");
    }
    for (x, y) in [(1.5, 0.0), (0.0, -1.5), (4.0, 3.0), (30.0, 10.0)] {
        let share = test.share(&s.frame(slide(x, y), slide(x, y), 9, 1.0, 0.0));
        assert!(share < STILL, "a camera that moved ({x}, {y}) read as still ({share:.2})");
    }
    let other = Shot { w: 320, h: 180, region: [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]], noise: 2.0 };
    assert!(test.share(&other.frame(H3::IDENTITY, slide(500.0, 900.0), 3, 1.0, 0.0)) < STILL / 2.0);
}

/// What the region has of its own: most of it on a textured plane (its
/// smoother stretches do not count), next to none on a blank one with
/// two things standing on it.
#[test]
fn a_blank_region_is_told_from_a_textured_one() {
    let s = shot();
    let first = s.frame(H3::IDENTITY, H3::IDENTITY, 0, 1.0, 0.0);
    let textured = PlaneTracker::new(&first, REGION, Options::default()).unwrap().texture();
    assert!(textured > 0.5, "{textured}");
    let mut blank = Gray { w: 320, h: 180, px: vec![200.0; 320 * 180] };
    for (bx, by) in [(120usize, 80usize), (180, 100)] {
        for y in by..by + 14 {
            for x in bx..bx + 12 {
                blank.px[y * 320 + x] = if (x + y) % 5 < 2 { 60.0 } else { 120.0 };
            }
        }
    }
    let texture = PlaneTracker::new(&blank, REGION, Options::default()).unwrap().texture();
    assert!(texture < 0.15, "{texture}");
}

/// A thing with a pattern of its own crosses a third of the region,
/// moving against the plane: the step from the frame before leaves it
/// out (it is noted as cover), and the plane is still held to a
/// fraction of a pixel.
#[test]
fn a_patterned_thing_crossing_the_region_does_not_carry_it_off() {
    let s = Shot { w: 320, h: 180, region: [[70.0, 40.0], [250.0, 44.0], [246.0, 150.0], [66.0, 146.0]], noise: 2.0 };
    let centre = [160.0, 95.0];
    let first = s.frame(H3::IDENTITY, H3::IDENTITY, 0, 1.0, 0.0);
    let mut tracker = PlaneTracker::new(&first, s.region, Options::default()).unwrap();
    for i in 1..=40u64 {
        let t = i as f64 / 40.0;
        let plane = motion(t, centre, [10.0, -6.0], 6.0, 1.08, [0.0002, 0.0001]);
        let mut frame = s.frame(plane, H3::IDENTITY, i, 1.0, 0.0);
        // A textured block, 60 × 70, sliding the other way.
        let (bx, by) = ((230.0 - 170.0 * t) as usize, 60usize);
        for y in by..by + 70 {
            for x in bx..(bx + 60).min(s.w) {
                frame.px[y * s.w + x] = (texture((x - bx) as f64 * 1.7, (y - by) as f64 * 1.7, 300) * 200.0 + 20.0) as f32;
            }
        }
        let out = tracker.step(&frame);
        assert!(out.found, "frame {i}: lost");
        let e = worst(&out.corners, &plane.quad(&s.region));
        assert!(e < 0.6, "frame {i}: {e:.3} px (match {:.2})", out.agree);
    }
}

// ---------------------------------------------------------------- rect

fn rot_x(deg: f64) -> [[f64; 3]; 3] {
    let (s, c) = deg.to_radians().sin_cos();
    [[1.0, 0.0, 0.0], [0.0, c, -s], [0.0, s, c]]
}

fn rot_y(deg: f64) -> [[f64; 3]; 3] {
    let (s, c) = deg.to_radians().sin_cos();
    [[c, 0.0, s], [0.0, 1.0, 0.0], [-s, 0.0, c]]
}

fn rot_z(deg: f64) -> [[f64; 3]; 3] {
    let (s, c) = deg.to_radians().sin_cos();
    [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]]
}

fn mul(a: [[f64; 3]; 3], b: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    std::array::from_fn(|r| std::array::from_fn(|c| (0..3).map(|k| a[r][k] * b[k][c]).sum()))
}

/// A 300 × 200 rectangle seen by a camera turned by (x, y, z) degrees.
fn view(rx: f64, ry: f64, rz: f64, t: [f64; 3], focal: f64, centre: [f64; 2]) -> (Quad, Quad, rect::Pose) {
    let world: Quad = [[-150.0, -100.0], [150.0, -100.0], [150.0, 100.0], [-150.0, 100.0]];
    let pose = rect::Pose { r: mul(rot_z(rz), mul(rot_x(rx), rot_y(ry))), t };
    let image = world.map(|p| pose.project([p[0], p[1], 0.0], centre, focal).unwrap());
    (world, image, pose)
}

#[test]
fn a_rectangle_gives_the_lens_its_shape_and_the_pose() {
    let (centre, height, f) = ([960.0, 540.0], 1080.0, 1400.0);
    let views = [(55.0, 20.0, 5.0), (48.0, -30.0, -8.0), (62.0, 12.0, 15.0), (40.0, 35.0, 0.0)];
    let mut quads = Vec::new();
    for &(rx, ry, rz) in &views {
        let (world, image, pose) = view(rx, ry, rz, [40.0, 30.0, 900.0], f, centre);
        quads.push(image);
        // The pose, from the lens.
        let got = rect::pose(&world, &image, centre, f).unwrap();
        for r in 0..3 {
            for c in 0..3 {
                assert!((got.r[r][c] - pose.r[r][c]).abs() < 1e-6, "rotation {r},{c}");
            }
            assert!((got.t[r] - pose.t[r]).abs() < 1e-4, "translation {r}");
        }
        // One view is enough for the lens.
        let lens = rect::focal(&[image], centre, height, None).unwrap();
        assert!((lens.focal - f).abs() < 1e-3, "one view: {}", lens.focal);
    }
    let lens = rect::focal(&quads, centre, height, None).unwrap();
    assert!((lens.focal - f).abs() < 1e-3, "{}", lens.focal);
    assert!(lens.strength > 0.01, "{}", lens.strength);
    let a = rect::aspect(&quads, centre, height, lens.focal).unwrap();
    assert!((a - 1.5).abs() < 1e-6, "{a}");
}

/// Corners half a pixel off: the lens is still within a few percent when
/// there is perspective to read.
#[test]
fn the_lens_holds_with_corners_a_little_off() {
    let (centre, height, f) = ([960.0, 540.0], 1080.0, 1400.0);
    let mut quads = Vec::new();
    for i in 0..30 {
        let a = i as f64;
        let (_, mut image, _) = view(50.0 + a * 0.3, -20.0 + a * 1.2, a * 0.2, [20.0, 10.0, 700.0], f, centre);
        for (k, p) in image.iter_mut().enumerate() {
            p[0] += hash(i, k as i64, 5) - 0.5;
            p[1] += hash(i, k as i64, 6) - 0.5;
        }
        quads.push(image);
    }
    let lens = rect::focal(&quads, centre, height, None).unwrap();
    assert!((lens.focal - f).abs() / f < 0.04, "{}", lens.focal);
}

/// Seen square on (one pair of sides level in the picture), a rectangle
/// of unknown proportions says nothing of the lens; with its proportions
/// it does.
#[test]
fn square_on_the_lens_needs_the_proportions() {
    let (centre, height, f) = ([960.0, 540.0], 1080.0, 1400.0);
    let (_, image, _) = view(55.0, 0.0, 0.0, [0.0, 0.0, 900.0], f, centre);
    let blind = rect::focal(&[image], centre, height, None);
    assert!(blind.is_none_or(|l| l.strength < 1e-6), "{blind:?}");
    let lens = rect::focal(&[image], centre, height, Some(1.5)).unwrap();
    assert!((lens.focal - f).abs() < 1e-3, "{}", lens.focal);
}

/// What it costs: a 640 × 360 frame, a region a third of it wide.
#[test]
#[ignore]
fn measure_the_cost_of_a_frame() {
    let s = Shot { w: 640, h: 360, region: [[200.0, 120.0], [430.0, 130.0], [420.0, 260.0], [190.0, 250.0]], noise: 2.0 };
    let centre = [320.0, 190.0];
    let frames: Vec<(H3, Gray)> = (0..=120u64).map(|i| {
        let plane = motion(i as f64 / 120.0, centre, [60.0, -30.0], 12.0, 1.3, [0.0003, -0.0002]);
        (plane, s.frame(plane, H3::IDENTITY, i, 1.0, 0.0))
    }).collect();
    let mut tracker = PlaneTracker::new(&frames[0].1, s.region, Options::default()).unwrap();
    let start = std::time::Instant::now();
    let mut worst_error = 0.0f64;
    for (plane, frame) in &frames[1..] {
        let out = tracker.step(frame);
        assert!(out.found);
        worst_error = worst_error.max(worst(&out.corners, &plane.quad(&s.region)));
    }
    let ms = start.elapsed().as_secs_f64() * 1000.0 / 120.0;
    println!("{ms:.2} ms a frame at 640 x 360 ({:.0} frames a second), worst corner {worst_error:.3} px, {} retakes", 1000.0 / ms, tracker.rekeys);
}

// ---------------------------------------------------------------- tilt

/// Four points of no particular shape on a plane, seen from places a
/// camera moved through, each half a pixel off: with the lens known the
/// plane's normal comes back within a degree; with the lens searched
/// too, within two degrees, and the lens itself only roughly (within an
/// eighth: four points say little of it).
#[test]
fn the_tilt_and_the_lens_come_from_the_camera_moving() {
    use crate::tilt;
    let (centre, height, f) = ([960.0, 540.0], 1080.0, 1400.0);
    let world: Quad = [[-140.0, -90.0], [170.0, -120.0], [120.0, 110.0], [-160.0, 60.0]];
    let mut views = Vec::new();
    let mut first = None;
    for i in 0..24 {
        let a = i as f64;
        // The camera goes round the points and in, and looks at them.
        let pose = rect::Pose { r: mul(rot_z(a * 0.4), mul(rot_x(52.0 - a * 0.5), rot_y(-22.0 + a * 1.8))), t: [30.0 - a * 3.0, 15.0 + a, 900.0 - a * 9.0] };
        first.get_or_insert(pose);
        views.push(std::array::from_fn(|k| {
            let p = pose.project([world[k][0], world[k][1], 0.0], centre, f).unwrap();
            [p[0] + hash(i, k as i64, 21) - 0.5, p[1] + hash(i, k as i64, 22) - 0.5]
        }));
    }
    // The plane's normal toward the camera, in the first view's space.
    let r = first.unwrap().r;
    let truth = [-r[0][2], -r[1][2], -r[2][2]];
    let angle = |n: [f64; 3]| (n[0] * truth[0] + n[1] * truth[1] + n[2] * truth[2]).clamp(-1.0, 1.0).acos().to_degrees();
    // The lens known.
    let known = tilt::from_motion(&views, 0, centre, height, Some(f)).unwrap();
    assert!(known.sure(), "{known:?}");
    assert!(angle(known.normal) < 1.0, "{:.2}° off: {known:?}", angle(known.normal));
    // The lens searched too.
    let found = tilt::from_motion(&views, 0, centre, height, None).unwrap();
    assert!(found.sure(), "{found:?}");
    assert!(angle(found.normal) < 2.0, "{:.2}° off: {found:?}", angle(found.normal));
    assert!((found.focal - f).abs() / f < 0.125, "lens {} for {f}", found.focal);
    // The shape is the points' own, up to its size and a turn: its sides
    // are in the world's proportions.
    let side = |q: &Quad, k: usize| ((q[k][0] - q[(k + 1) % 4][0]).powi(2) + (q[k][1] - q[(k + 1) % 4][1]).powi(2)).sqrt();
    for k in 1..4 {
        let (got, want) = (side(&known.shape, k) / side(&known.shape, 0), side(&world, k) / side(&world, 0));
        assert!((got - want).abs() < 0.02, "side {k}: {got:.3} for {want:.3}");
    }
}

/// A camera that only turns on the spot shows nothing of the tilt: no
/// claim is made.
#[test]
fn a_camera_turning_on_the_spot_says_nothing_of_the_tilt() {
    use crate::tilt;
    let (centre, height, f) = ([960.0, 540.0], 1080.0, 1400.0);
    let world: Quad = [[-140.0, -90.0], [170.0, -120.0], [120.0, 110.0], [-160.0, 60.0]];
    let base = rect::Pose { r: mul(rot_x(50.0), rot_y(-15.0)), t: [20.0, 10.0, 900.0] };
    let views: Vec<Quad> = (0..20).map(|i| {
        // The same place, the camera turned: every point's ray turned.
        let turn = mul(rot_y(i as f64 * 0.6), rot_x(i as f64 * 0.3));
        let pose = rect::Pose { r: mul(turn, base.r), t: std::array::from_fn(|r| (0..3).map(|c| turn[r][c] * base.t[c]).sum()) };
        std::array::from_fn(|k| pose.project([world[k][0], world[k][1], 0.0], centre, f).unwrap())
    }).collect();
    let got = tilt::from_motion(&views, 0, centre, height, Some(f)).unwrap();
    assert!(!got.sure(), "{got:?}");
}
