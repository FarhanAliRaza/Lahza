//! Times the CPU (or GPU) render of a project, stage by stage:
//!
//!     cargo run --release -p lahza-motion-render --example profile -- PROJECT.lahza [FROM TO] [--gpu]
use lahza_motion_core::compile::Compiled;
use lahza_motion_render::{Backend, RenderOptions, Renderer};
use std::path::Path;
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = Path::new(&args[0]);
    let project = lahza_motion_core::io::load(dir).expect("project");
    let compiled = Compiled::new(&project);
    let fps = project.settings.fps;
    let (from, to) = match (args.get(1).and_then(|a| a.parse::<f64>().ok()), args.get(2).and_then(|a| a.parse::<f64>().ok())) {
        (Some(a), Some(b)) => (a, b),
        _ => (0.0, project.settings.duration),
    };
    let backend = if args.iter().any(|a| a == "--gpu") { Backend::Auto } else { Backend::Cpu };
    let mut r = Renderer::new(Some(dir), RenderOptions { backend, ..Default::default() });
    let (first, last) = (fps.frame_at(from), fps.frame_at(to));
    let started = Instant::now();
    let mut slowest = (0.0, 0.0);
    let mut per_scene: std::collections::BTreeMap<String, (f64, u32)> = Default::default();
    for n in first..last {
        let t = fps.time_of(n);
        let f0 = Instant::now();
        let frame = compiled.frame(t);
        let compile_ms = f0.elapsed().as_secs_f64() * 1000.0;
        let img = r.render(&compiled, t);
        std::hint::black_box(&img);
        let ms = f0.elapsed().as_secs_f64() * 1000.0;
        if ms > slowest.1 {
            slowest = (t, ms);
        }
        let e = per_scene.entry(frame.scene.as_deref().unwrap_or("").to_string()).or_default();
        e.0 += ms;
        e.1 += 1;
        if std::env::var("VERBOSE").is_ok() {
            println!("t={t:.3} {ms:.1} ms (compile {compile_ms:.1})");
        }
    }
    let total = started.elapsed().as_secs_f64();
    let frames = (last - first) as f64;
    println!("{} frames in {total:.2} s = {:.1} ms/frame ({:.2} fps) on {}", frames, total * 1000.0 / frames, frames / total, r.backend_label());
    println!("build {:.1} ms/frame, raster {:.1} ms/frame, cached {}", r.stats.build_ms / frames, r.stats.raster_ms / frames, r.stats.cached);
    for (s, (ms, n)) in &per_scene {
        println!("  scene {s}: {:.1} ms/frame over {n} frames", ms / *n as f64);
    }
    println!("slowest frame t={:.3}: {:.1} ms", slowest.0, slowest.1);
}
