//! Hardware parity against PyTorch layer dumps from
//! ignore/re-use-tools/dump_reference.py. Run alone:
//! LAHZA_REUSE_REF=<dump dir> cargo test --release reuse_audio -- --ignored --nocapture --test-threads=1
use super::*;
use std::path::{Path, PathBuf};

const MODEL_PATH: &str = "assets/audio/re-use/model.safetensors";

fn npy(dir: &Path, name: &str) -> (Vec<usize>, Vec<f32>) {
    let bytes = std::fs::read(dir.join(format!("{name}.npy"))).unwrap_or_else(|e| panic!("{name}: {e}"));
    assert_eq!(&bytes[..6], b"\x93NUMPY");
    let (header_len, start) = if bytes[6] == 1 {
        (u16::from_le_bytes([bytes[8], bytes[9]]) as usize, 10)
    } else {
        (u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize, 12)
    };
    let header = std::str::from_utf8(&bytes[start..start + header_len]).unwrap();
    assert!(header.contains("'descr': '<f4'") && header.contains("'fortran_order': False"), "{header}");
    let shape_text = header.split("'shape': (").nth(1).unwrap().split(')').next().unwrap();
    let shape: Vec<usize> = shape_text.split(',').filter_map(|s| s.trim().parse().ok()).collect();
    let data = bytes[start + header_len..]
        .chunks_exact(4)
        .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
        .collect::<Vec<_>>();
    assert_eq!(data.len(), shape.iter().product::<usize>());
    (shape, data)
}

/// [c][t][f] → [t][f][c].
fn channel_last(data: &[f32], shape: &[usize]) -> Vec<f32> {
    let (c, t, f) = (shape[0], shape[1], shape[2]);
    let mut out = vec![0.0; data.len()];
    for ci in 0..c {
        for ti in 0..t {
            for fi in 0..f {
                out[(ti * f + fi) * c + ci] = data[(ci * t + ti) * f + fi];
            }
        }
    }
    out
}
/// [f][t] → [t][f].
fn time_major(data: &[f32], f: usize, t: usize) -> Vec<f32> {
    let mut out = vec![0.0; data.len()];
    for fi in 0..f {
        for ti in 0..t {
            out[ti * f + fi] = data[fi * t + ti];
        }
    }
    out
}

#[derive(Debug)]
struct Diff {
    max_abs: f64,
    rel_l2: f64,
    ref_max: f64,
    worst: usize,
}
fn diff(ours: &[f32], reference: &[f32], wrap: bool) -> Diff {
    assert_eq!(ours.len(), reference.len());
    let (mut max_abs, mut err2, mut ref2, mut ref_max, mut worst) = (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0);
    for (i, (&a, &b)) in ours.iter().zip(reference).enumerate() {
        let mut d = (a as f64 - b as f64).abs();
        if wrap {
            d = d.rem_euclid(2.0 * std::f64::consts::PI);
            d = d.min(2.0 * std::f64::consts::PI - d);
        }
        if !d.is_finite() || d > max_abs {
            max_abs = if d.is_finite() { d } else { f64::INFINITY };
            worst = i;
        }
        err2 += d * d;
        ref2 += (b as f64) * (b as f64);
        ref_max = ref_max.max((b as f64).abs());
    }
    Diff { max_abs, rel_l2: (err2 / ref2.max(1e-30)).sqrt(), ref_max, worst }
}

struct Report(Vec<(String, Diff)>);
impl Report {
    fn add(&mut self, name: impl Into<String>, d: Diff) {
        let name = name.into();
        eprintln!(
            "{name:<34} max|Δ| {:>10.3e}  rel-L2 {:>10.3e}  max|ref| {:>9.3e}  @{}",
            d.max_abs, d.rel_l2, d.ref_max, d.worst
        );
        self.0.push((name, d));
    }
}

fn setup() -> (gpu::Context, Engine, PathBuf) {
    let reference = PathBuf::from(std::env::var_os("LAHZA_REUSE_REF").expect("LAHZA_REUSE_REF"));
    let context = unsafe { gpu::Context::init(gpu::ContextDesc::default()) }.expect("Vulkan context");
    eprintln!("GPU: {:?}", context.device_information());
    let model = std::fs::read(MODEL_PATH).unwrap();
    let engine = Engine::new(&context, &model, std::env::var_os("LAHZA_REUSE_F32").is_none()).unwrap();
    eprintln!("activation storage: {}", if engine.uses_f16 { "f16" } else { "f32" });
    (context, engine, reference)
}

#[test]
#[ignore = "requires Vulkan and PyTorch reference dumps; see module docs"]
fn hardware_layers_match_pytorch() {
    let (context, mut engine, dir) = setup();
    let ctx = &context;
    let (_, wav) = npy(&dir, "input_wav");
    let g = engine.prepare(ctx, wav.len()).unwrap();
    eprintln!("{g:?}, device bytes {:.1} MiB", engine.device_bytes() as f64 / 1048576.0);
    let (t, tp, tc) = (g.t as usize, g.tp as usize, g.tc as usize);
    let (f, fp, fc) = (BINS as usize, FP as usize, FC as usize);
    let mut report = Report(Vec::new());
    let half = engine.uses_f16;
    let load_cl = |name: &str| {
        let (shape, data) = npy(&dir, name);
        channel_last(&data, &shape)
    };
    let only = std::env::var("LAHZA_REUSE_ONLY").unwrap_or_default();
    let want = |section: &str| only.is_empty() || only.split(',').any(|s| s == section);

    if want("stft") {
        engine.write(ctx, Buf::Wav, &wav, false);
        engine.run_stages(ctx, &["stft"]).unwrap();
        let input = engine.read(ctx, Buf::Input, tp * fp * 2, true);
        let (_, mag) = npy(&dir, "stft_mag");
        let (_, pha) = npy(&dir, "stft_pha");
        let pick = |ch: usize| -> Vec<f32> {
            (0..t).flat_map(|ti| (0..f).map(move |fi| (ti, fi))).map(|(ti, fi)| input[(ti * fp + fi) * 2 + ch]).collect()
        };
        report.add("stft log1p|X|", diff(&pick(0), &time_major(&mag, f, t), false));
        let (ours, reference) = (pick(1), time_major(&pha, f, t));
        report.add("stft phase (wrapped)", diff(&ours, &reference, true));
        let mags = time_major(&mag, f, t);
        let mut bad: Vec<(f32, f32)> = Vec::new();
        for i in 0..ours.len() {
            let d = (ours[i] - reference[i]).abs().rem_euclid(std::f32::consts::TAU);
            if d.min(std::f32::consts::TAU - d) > 1e-3 {
                bad.push((mags[i].exp_m1(), d.min(std::f32::consts::TAU - d)));
            }
        }
        if dir.join("stft_pha_f64.npy").exists() {
            let exact = time_major(&npy(&dir, "stft_pha_f64").1, f, t);
            let count = |x: &[f32]| x.iter().zip(&exact).filter(|(a, b)| {
                let d = (**a - **b).abs().rem_euclid(std::f32::consts::TAU);
                d.min(std::f32::consts::TAU - d) > 1e-3
            }).count();
            let (vulkan, torch) = (count(&ours), count(&reference));
            eprintln!("  vs exact f64 STFT: phase |Δ|>1e-3 at {vulkan} bins (Vulkan) vs {torch} bins (PyTorch)");
            // Phase of near-silent bins is f32 FFT rounding noise in either
            // implementation; require ours to be no noisier than cuFFT.
            assert!(vulkan <= torch, "Vulkan STFT phase noisier than cuFFT");
        }
        bad.sort_by(|a, b| b.0.total_cmp(&a.0));
        eprintln!("  phase |Δ|>1e-3 at {} of {} bins; largest |X| among them {:?}", bad.len(), ours.len(), bad.first());
    }

    let model_input = || {
        let (_, mag) = npy(&dir, "stft_mag");
        let (_, pha) = npy(&dir, "stft_pha");
        let mut input = vec![0.0; tp * fp * 2];
        for fi in 0..f {
            for ti in 0..t {
                input[(ti * fp + fi) * 2] = mag[fi * t + ti];
                input[(ti * fp + fi) * 2 + 1] = pha[fi * t + ti];
            }
        }
        input
    };
    if want("enc") {
        engine.write(ctx, Buf::Input, &model_input(), true);
        engine.run_stages(ctx, &["enc_conv1"]).unwrap();
        let ours = engine.read(ctx, Buf::Wide, tp * fp * 64, true);
        let reference = load_cl("enc_conv1");
        report.add("enc_conv1", diff(&ours, &reference, false));
        engine.write(ctx, Buf::Wide, &reference, true);
        drop(reference);
        let outs = [Buf::S0, Buf::S1, Buf::S2, Buf::S3];
        for i in 0..4 {
            for j in 0..i {
                let r = load_cl(&format!("enc_dense{j}"));
                engine.write(ctx, outs[j], &r, true);
            }
            engine.run_stages(ctx, &[&format!("enc_dense{i}")]).unwrap();
            let r = load_cl(&format!("enc_dense{i}"));
            if i < 3 {
                let ours = engine.read(ctx, outs[i], tp * fp * 64, true);
                report.add(format!("enc_dense{i}"), diff(&ours, &r, false));
            } else {
                let ours = engine.read(ctx, Buf::S3, tc * fp * 64, true);
                let rows: Vec<f32> = (0..tc).flat_map(|row| r[row * 4 * fp * 64..(row * 4 + 1) * fp * 64].iter().copied()).collect();
                report.add("enc_dense3 (rows t%4=0)", diff(&ours, &rows, false));
            }
        }
        let r = load_cl("enc_dense3");
        let rows: Vec<f32> = (0..tc).flat_map(|row| r[row * 4 * fp * 64..(row * 4 + 1) * fp * 64].iter().copied()).collect();
        drop(r);
        engine.write(ctx, Buf::S3, &rows, true);
        engine.run_stages(ctx, &["enc_conv2"]).unwrap();
        let ours = engine.read(ctx, Buf::X, tc * fc * 64, false);
        report.add("enc_conv2", diff(&ours, &load_cl("enc_conv2"), false));
    }

    if want("mamba0") {
        // Block-0 time MambaBlock internals, forward direction (sequence = bin).
        let x = load_cl("enc_conv2");
        engine.write(ctx, Buf::X, &x, false);
        let plan = &engine.resources.as_ref().unwrap().plan;
        let mark = |step: &str| plan.marks.iter().find(|m| m.0 == format!("tf00_time.{step}")).unwrap().1;
        let end = plan.stages.iter().find(|s| s.0 == "tf00_time").unwrap().1.end;
        let steps = [mark("in"), mark("xproj"), mark("scan"), mark("gate"), mark("out"), mark("ln"), end];
        // (b f) t c → [t][f][c]
        let seq_major = |name: &str, width: usize, take: usize| -> Vec<f32> {
            let (shape, data) = npy(&dir, name);
            assert_eq!(shape, vec![fc, tc, width], "{name}");
            let mut out = vec![0.0; tc * fc * take];
            for fi in 0..fc {
                for ti in 0..tc {
                    for c in 0..take {
                        out[(ti * fc + fi) * take + c] = data[(fi * tc + ti) * width + c];
                    }
                }
            }
            out
        };
        let pick = |values: &[f32], stride: usize, take: usize| -> Vec<f32> {
            values.chunks_exact(stride).flat_map(|row| row[..take].iter().copied()).collect()
        };
        engine.run_ops(ctx, steps[0]..steps[1]).unwrap();
        let xi = engine.read(ctx, Buf::Wide, tc * fc * 512, false);
        report.add("tf00 time fw in_proj (x)", diff(&pick(&xi, 512, 256), &seq_major("m0_in_proj", 512, 256), false));
        engine.run_ops(ctx, steps[1]..steps[2]).unwrap();
        let dbc = engine.read(ctx, Buf::S0, tc * fc * 72, false);
        report.add("tf00 time fw x_proj", diff(&pick(&dbc, 72, 36), &seq_major("m0_x_proj", 36, 36), false));
        engine.run_ops(ctx, steps[2]..steps[3]).unwrap();
        let y = engine.read(ctx, Buf::Wide, tc * fc * 512, false);
        report.add("tf00 time fw scan", diff(&pick(&y, 512, 256), &seq_major("m0_scan", 256, 256), false));
        engine.run_ops(ctx, steps[3]..steps[5]).unwrap();
        let o = engine.read(ctx, Buf::S1, tc * fc * 128, false);
        let out: Vec<f32> = o.chunks_exact(128).zip(x.chunks_exact(64)).flat_map(|(o, x)| (0..64).map(move |c| o[c] - x[c])).collect();
        report.add("tf00 time fw mamba out", diff(&out, &seq_major("m0_out", 64, 64), false));
        engine.run_ops(ctx, steps[5]..steps[6]).unwrap();
        let ours = engine.read(ctx, Buf::X, tc * fc * 64, false);
        let block: Vec<f32> = ours.iter().zip(&x).map(|(a, b)| a - b).collect();
        report.add("tf00 time MambaBlock", diff(&block, &seq_major("tfmamba00_time_mambablock", 64, 64), false));
    }

    if want("tf") {
        for k in 0..30 {
            let input = if k == 0 { "enc_conv2".to_string() } else { format!("tfmamba{:02}", k - 1) };
            engine.write(ctx, Buf::X, &load_cl(&input), false);
            engine.run_stages(ctx, &[&format!("tf{k:02}_time"), &format!("tf{k:02}_freq")]).unwrap();
            let ours = engine.read(ctx, Buf::X, tc * fc * 64, false);
            report.add(format!("tfmamba{k:02}"), diff(&ours, &load_cl(&format!("tfmamba{k:02}")), false));
        }
    }

    for prefix in ["mag", "pha"] {
        if !want(prefix) {
            continue;
        }
        let outs = [Buf::S0, Buf::S1, Buf::S2, Buf::S3];
        engine.write(ctx, Buf::X, &load_cl("tfmamba29"), false);
        for i in 0..4 {
            for j in 0..i {
                engine.write(ctx, outs[j], &load_cl(&format!("{prefix}_dense{j}")), false);
            }
            engine.run_stages(ctx, &[&format!("{prefix}_dense{i}")]).unwrap();
            let ours = engine.read(ctx, outs[i], tc * fc * 64, false);
            report.add(format!("{prefix}_dense{i}"), diff(&ours, &load_cl(&format!("{prefix}_dense{i}")), false));
        }
        engine.write(ctx, Buf::S3, &load_cl(&format!("{prefix}_dense3")), false);
        engine.run_stages(ctx, &[&format!("{prefix}_up1")]).unwrap();
        let ours = engine.read(ctx, Buf::Wide, tc * 2 * fc * 64, false);
        let up1 = load_cl(&format!("{prefix}_up1"));
        report.add(format!("{prefix}_up1"), diff(&ours, &up1, false));
        engine.write(ctx, Buf::Wide, &up1, false);
        drop(up1);
        engine.run_stages(ctx, &[&format!("{prefix}_final")]).unwrap();
        let spec = engine.read(ctx, Buf::Spec, 2 * t * f, false);
        let (name, range, wrap) = if prefix == "mag" { ("model_mag", 0..t * f, false) } else { ("model_pha", t * f..2 * t * f, true) };
        let (_, r) = npy(&dir, name);
        report.add(format!("{prefix} decoder output"), diff(&spec[range], &time_major(&r, f, t), wrap));
    }

    if want("istft") {
        let (_, mag) = npy(&dir, "model_mag");
        let (_, pha) = npy(&dir, "model_pha");
        let mut spec = time_major(&mag, f, t);
        spec.extend(time_major(&pha, f, t));
        engine.write(ctx, Buf::Spec, &spec, false);
        engine.run_stages(ctx, &["istft"]).unwrap();
        let out = engine.read(ctx, Buf::Out, wav.len(), false);
        report.add("istft waveform", diff(&out, &npy(&dir, "output_wav").1, false));
    }

    if want("trace") {
        engine.write(ctx, Buf::Wav, &wav, false);
        for name in engine.stage_names() {
            engine.run_stages(ctx, &[&name]).unwrap();
            let (buf, n) = if name.starts_with("tf") || name == "enc_conv2" {
                (Buf::X, tc * fc * 64)
            } else if name == "stft" {
                (Buf::Input, tp * fp * 2)
            } else if name.ends_with("_final") || name == "istft" {
                (Buf::Spec, 2 * t * f)
            } else {
                continue;
            };
            let values = engine.read(ctx, buf, n, buf == Buf::Input);
            let bad = values.iter().filter(|v| !v.is_finite()).count();
            let max = values.iter().filter(|v| v.is_finite()).fold(0f32, |m, v| m.max(v.abs()));
            eprintln!("  trace {name:<12} non-finite {bad:>9}  max|x| {max:.3e}");
            if bad > 0 {
                let range = engine.resources.as_ref().unwrap().plan.stages.iter().find(|s| s.0 == name).unwrap().1.clone();
                engine.write(ctx, Buf::X, &load_cl(if name == "tf02_freq" { "tfmamba01" } else { "enc_conv2" }), false);
                if name == "tf02_freq" {
                    engine.run_stages(ctx, &["tf02_time"]).unwrap();
                }
                let plan = &engine.resources.as_ref().unwrap().plan;
                let mut steps: Vec<usize> = plan.marks.iter().filter(|m| m.0.starts_with(&format!("{name}."))).map(|m| m.1).collect();
                steps.push(range.end);
                for (w, (buf, width)) in steps.windows(2).zip([(Buf::Wide, 512), (Buf::S0, 72), (Buf::Wide, 512), (Buf::Wide, 512), (Buf::S1, 128), (Buf::X, 64)]) {
                    engine.run_ops(ctx, w[0]..w[1]).unwrap();
                    let values = engine.read(ctx, buf, tc * fc * width, false);
                    let bad = values.iter().filter(|v| !v.is_finite()).count();
                    let max = values.iter().filter(|v| v.is_finite()).fold(0f32, |m, v| m.max(v.abs()));
                    eprintln!("    ops {:?} {buf:?}: non-finite {bad}, max|x| {max:.3e}", w[0]..w[1]);
                }
                break;
            }
        }
    }

    if want("bench") {
        let seconds = wav.len() as f64 / RATE as f64;
        for run in 0..3 {
            let start = std::time::Instant::now();
            engine.enhance(ctx, &wav, &|| false, &mut |_| {}).unwrap();
            let elapsed = start.elapsed().as_secs_f64();
            eprintln!("run {run}: {elapsed:.3} s for {seconds:.3} s audio, RTF {:.4}", elapsed / seconds);
        }
        let mut groups: Vec<(String, f64)> = Vec::new();
        for name in engine.stage_names() {
            let start = std::time::Instant::now();
            engine.run_stages(ctx, &[&name]).unwrap();
            let elapsed = start.elapsed().as_secs_f64();
            let group = name.trim_start_matches(|c: char| c == 't' || c == 'f' || c.is_ascii_digit()).to_string();
            let group = if name.starts_with("tf") { format!("tf{group}") } else { name.trim_end_matches(char::is_numeric).to_string() };
            match groups.iter_mut().find(|g| g.0 == group) {
                Some(g) => g.1 += elapsed,
                None => groups.push((group, elapsed)),
            }
        }
        let marks = engine.resources.as_ref().unwrap().plan.marks.clone();
        let stages = engine.resources.as_ref().unwrap().plan.stages.clone();
        for stage in ["tf00_time", "tf00_freq"] {
            let range = stages.iter().find(|s| s.0 == stage).unwrap().1.clone();
            let mut steps: Vec<(String, usize)> = marks.iter().filter(|m| m.0.starts_with(&format!("{stage}."))).cloned().collect();
            steps.push(("end".into(), range.end));
            for w in steps.windows(2) {
                let start = std::time::Instant::now();
                engine.run_ops(ctx, w[0].1..w[1].1).unwrap();
                eprintln!("  {:<18} {:>3} dispatches {:>8.2} ms", w[0].0, w[1].1 - w[0].1, start.elapsed().as_secs_f64() * 1000.0);
            }
        }
        for (group, secs) in groups {
            eprintln!("  {group:<12} {:>8.1} ms", secs * 1000.0);
        }
    }

    if want("e2e") {
        let start = std::time::Instant::now();
        let out = engine.enhance(ctx, &wav, &|| false, &mut |_| {}).unwrap();
        eprintln!("end-to-end {:.3} s for {:.3} s audio", start.elapsed().as_secs_f64(), wav.len() as f64 / RATE as f64);
        let spec = engine.read(ctx, Buf::Spec, 2 * t * f, false);
        report.add("e2e model magnitude", diff(&spec[..t * f], &time_major(&npy(&dir, "model_mag").1, f, t), false));
        report.add("e2e model phase (wrapped)", diff(&spec[t * f..], &time_major(&npy(&dir, "model_pha").1, f, t), true));
        report.add("e2e waveform", diff(&out, &npy(&dir, "output_wav").1, false));
        if let Ok((_, official)) = std::panic::catch_unwind(|| npy(&dir, "official_flac")) {
            report.add("e2e waveform vs official .flac", diff(&out, &official, false));
        }
        if let Some(path) = std::env::var_os("LAHZA_REUSE_WAV_OUT") {
            write_wav(Path::new(&path), &out);
        }
    }
    engine.destroy(ctx);
    // Phase comparisons are covered by the exact-STFT check and the waveform.
    let (layer, waveform) = if half { (2e-3, 1e-3) } else { (1e-4, 1e-4) };
    for (name, d) in &report.0 {
        if name.contains("phase") {
            continue;
        }
        let limit = if name.contains("waveform") { waveform } else { layer };
        let limit = if name.contains("official") { limit + 3e-4 } else { limit };
        assert!(d.rel_l2 < limit, "{name}: rel-L2 {:.3e} exceeds {limit:.0e}", d.rel_l2);
    }
}

fn write_wav(path: &Path, samples: &[f32]) {
    let mut bytes = Vec::with_capacity(44 + samples.len() * 4);
    let data = (samples.len() * 4) as u32;
    bytes.extend(b"RIFF");
    bytes.extend((36 + data).to_le_bytes());
    bytes.extend(b"WAVEfmt ");
    bytes.extend(16u32.to_le_bytes());
    bytes.extend(3u16.to_le_bytes());
    bytes.extend(1u16.to_le_bytes());
    bytes.extend((RATE as u32).to_le_bytes());
    bytes.extend((RATE as u32 * 4).to_le_bytes());
    bytes.extend(4u16.to_le_bytes());
    bytes.extend(32u16.to_le_bytes());
    bytes.extend(b"data");
    bytes.extend(data.to_le_bytes());
    for s in samples {
        bytes.extend(s.to_le_bytes());
    }
    std::fs::write(path, bytes).unwrap();
}

#[test]
fn shader_variants_validate() {
    for (input, output) in VARIANTS {
        let types = format!("alias In = {input};\nalias Out = {output};");
        let types = if (input, output) == ("f32", "f32") { types } else { format!("enable f16;\n{types}") };
        let source = include_str!("reuse_audio.wgsl").replacen("alias In = f32;\nalias Out = f32;", &types, 1);
        let module = naga::front::wgsl::parse_str(&source).unwrap_or_else(|e| panic!("{}", e.emit_to_string(&source)));
        let flags = naga::valid::ValidationFlags::all() ^ naga::valid::ValidationFlags::BINDINGS;
        naga::valid::Validator::new(flags, naga::valid::Capabilities::SHADER_FLOAT16)
            .validate(&module)
            .unwrap_or_else(|e| panic!("{input}→{output}: {e:?}"));
    }
}

/// Frame-pacing proxy for the editor: an independent Vulkan context (as the
/// GPUI renderer is) submits a small compute pass every 16 ms and records how
/// long each takes to complete, idle and while a segment is enhanced.
#[test]
#[ignore = "requires Vulkan; run alone with --ignored --nocapture --test-threads=1"]
fn hardware_concurrent_client_latency() {
    use std::sync::{atomic::{AtomicBool, Ordering}, Arc};
    #[derive(blade_macros::ShaderData)]
    struct ProbeData {
        probe: gpu::BufferPiece,
    }
    fn probe(stop: Arc<AtomicBool>) -> Vec<f64> {
        let context = unsafe { gpu::Context::init(gpu::ContextDesc::default()) }.unwrap();
        let shader = context.create_shader(gpu::ShaderDesc {
            source: "var<storage, read_write> probe: array<f32>;
                @compute @workgroup_size(256) fn main(@builtin(global_invocation_id) id: vec3<u32>) {
                    probe[id.x] = probe[id.x] * 0.5 + f32(id.x);
                }",
        });
        let pipeline = context.create_compute_pipeline(gpu::ComputePipelineDesc {
            name: "probe",
            data_layouts: &[&ProbeData::layout()],
            compute: shader.at("main"),
        });
        // A 1080p RGBA float pass: 2 M elements.
        let buffer = context.create_buffer(gpu::BufferDesc { name: "probe", size: 8 << 20, memory: gpu::Memory::Device });
        let mut encoder = context.create_command_encoder(gpu::CommandEncoderDesc { name: "probe", buffer_count: 1 });
        let mut latencies = Vec::new();
        while !stop.load(Ordering::Relaxed) {
            let frame = std::time::Instant::now();
            encoder.start();
            {
                let mut compute = encoder.compute("probe");
                let mut pass = compute.with(&pipeline);
                pass.bind(0, &ProbeData { probe: buffer.into() });
                pass.dispatch([(2 << 20) / 256, 1, 1]);
            }
            let sync = context.submit(&mut encoder);
            context.wait_for(&sync, 10_000);
            latencies.push(frame.elapsed().as_secs_f64() * 1000.0);
            std::thread::sleep(std::time::Duration::from_millis(16).saturating_sub(frame.elapsed()));
        }
        context.destroy_buffer(buffer);
        context.destroy_command_encoder(&mut encoder);
        latencies
    }
    fn summary(name: &str, mut v: Vec<f64>) {
        v.sort_by(f64::total_cmp);
        let q = |p: f64| v[((v.len() - 1) as f64 * p) as usize];
        eprintln!("{name:<22} frames {:>4}  p50 {:>6.2} ms  p95 {:>6.2} ms  p99 {:>6.2} ms  max {:>6.2} ms  >16ms {}",
            v.len(), q(0.5), q(0.95), q(0.99), v[v.len() - 1], v.iter().filter(|&&x| x > 16.0).count());
    }
    let reference = PathBuf::from(std::env::var_os("LAHZA_REUSE_REF").expect("LAHZA_REUSE_REF"));
    let (_, wav) = npy(&reference, "input_wav");
    let stop = Arc::new(AtomicBool::new(false));
    let idle = { let s = stop.clone(); std::thread::spawn(move || probe(s)) };
    std::thread::sleep(std::time::Duration::from_secs(3));
    stop.store(true, Ordering::Relaxed);
    summary("idle", idle.join().unwrap());

    let context = unsafe { gpu::Context::init(gpu::ContextDesc::default()) }.unwrap();
    let model = std::fs::read(MODEL_PATH).unwrap();
    let mut engine = Engine::new(&context, &model, std::env::var_os("LAHZA_REUSE_F32").is_none()).unwrap();
    engine.prepare(&context, wav.len()).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let busy = { let s = stop.clone(); std::thread::spawn(move || probe(s)) };
    std::thread::sleep(std::time::Duration::from_millis(500));
    let start = std::time::Instant::now();
    engine.enhance(&context, &wav, &|| false, &mut |_| {}).unwrap();
    let elapsed = start.elapsed().as_secs_f64();
    stop.store(true, Ordering::Relaxed);
    summary("during enhancement", busy.join().unwrap());
    eprintln!("enhancement {elapsed:.3} s for {:.3} s audio", wav.len() as f64 / RATE as f64);
    engine.destroy(&context);
}

/// Production path: a duplicated-mono stereo file longer than one segment,
/// against the official inference_chunk.py with the same segment settings
/// (--chunk_size_in_seconds 10 --hop_length_portion 0.75).
#[test]
#[ignore = "requires Vulkan and LAHZA_REUSE_REF containing input_wav.npy and official_chunk.npy"]
fn hardware_enhance_file_matches_official_chunking() {
    let dir = PathBuf::from(std::env::var_os("LAHZA_REUSE_REF").expect("LAHZA_REUSE_REF"));
    let (_, wav) = npy(&dir, "input_wav");
    let (_, official) = npy(&dir, "official_chunk");
    assert!(wav.len() > SEGMENT, "fixture must exercise segmenting");
    let scratch = std::env::temp_dir().join(format!("lahza-reuse-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();
    let (input, output) = (scratch.join("in.f32"), scratch.join("out.f32"));
    let stereo: Vec<u8> = wav.iter().flat_map(|v| [v.to_le_bytes(), v.to_le_bytes()]).flatten().collect();
    std::fs::write(&input, stereo).unwrap();
    let mut last = 0.0;
    let start = std::time::Instant::now();
    enhance_file(&input, &output, 2, &|| false, &mut |f| {
        assert!(f >= last, "progress went backwards");
        last = f;
    }, &mut |notice| eprintln!("notice: {notice}"))
    .unwrap();
    let elapsed = start.elapsed().as_secs_f64();
    let bytes = std::fs::read(&output).unwrap();
    std::fs::remove_dir_all(&scratch).unwrap();
    let out: Vec<f32> = bytes.chunks_exact(4).map(|b| f32::from_le_bytes(b.try_into().unwrap())).collect();
    assert_eq!(out.len(), wav.len() * 2);
    assert_eq!(last, 1.0);
    let left: Vec<f32> = out.iter().step_by(2).copied().collect();
    let right: Vec<f32> = out.iter().skip(1).step_by(2).copied().collect();
    assert_eq!(left, right);
    let d = diff(&left, &official, false);
    eprintln!(
        "enhance_file {elapsed:.3} s for {:.3} s stereo audio; vs official chunked .flac: max|Δ| {:.3e} rel-L2 {:.3e}",
        wav.len() as f64 / RATE as f64, d.max_abs, d.rel_l2
    );
    assert!(d.rel_l2 < 1e-3, "rel-L2 {:.3e}", d.rel_l2);
}

/// Run with VK_ICD_FILENAMES pointing at lavapipe only: enhancement still
/// completes and reports CPU_WARNING. On a GPU it must not warn.
#[test]
#[ignore = "requires Vulkan; set VK_ICD_FILENAMES to choose the device"]
fn hardware_enhance_file_warns_only_on_cpu_devices() {
    let scratch = std::env::temp_dir().join(format!("lahza-reuse-cpu-{}", std::process::id()));
    std::fs::create_dir_all(&scratch).unwrap();
    let (input, output) = (scratch.join("in.f32"), scratch.join("out.f32"));
    let tone: Vec<u8> = (0..RATE / 2).flat_map(|i| (0.1 * (i as f32 * 0.05).sin()).to_le_bytes()).collect();
    std::fs::write(&input, tone).unwrap();
    let mut notices = Vec::new();
    enhance_file(&input, &output, 1, &|| false, &mut |_| {}, &mut |n| notices.push(n.to_string())).unwrap();
    let produced = std::fs::metadata(&output).unwrap().len();
    std::fs::remove_dir_all(&scratch).unwrap();
    assert_eq!(produced, (RATE / 2 * 4) as u64);
    let software = gpu_engine().lock().unwrap().as_ref().unwrap().software;
    eprintln!("software device: {software}; notices: {notices:?}");
    assert_eq!(notices, if software { vec![CPU_WARNING.to_string()] } else { Vec::new() });
}

/// Same input → bit-identical output, including after a different-length
/// segment has resized and reused the engine's buffers.
#[test]
#[ignore = "requires Vulkan"]
fn hardware_enhancement_is_deterministic_across_segment_lengths() {
    let context = unsafe { gpu::Context::init(gpu::ContextDesc::default()) }.unwrap();
    let model = std::fs::read(MODEL_PATH).unwrap();
    let mut engine = Engine::new(&context, &model, true).unwrap();
    let mut state = 12345u32;
    let mut noise = |n: usize| -> Vec<f32> {
        (0..n).map(|i| {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            0.2 * (i as f32 * 0.031).sin() + 0.05 * (state >> 8) as f32 / (1u32 << 24) as f32 - 0.025
        }).collect()
    };
    let a = noise(RATE);
    let b = noise(RATE * 3 + 77);
    let first = engine.enhance(&context, &a, &|| false, &mut |_| {}).unwrap();
    let again = engine.enhance(&context, &a, &|| false, &mut |_| {}).unwrap();
    engine.enhance(&context, &b, &|| false, &mut |_| {}).unwrap();
    let after_resize = engine.enhance(&context, &a, &|| false, &mut |_| {}).unwrap();
    let max = |x: &[f32], y: &[f32]| x.iter().zip(y).map(|(p, q)| (p - q).abs()).fold(0f32, f32::max);
    eprintln!("repeat max|Δ| {:e}; after resize max|Δ| {:e}", max(&first, &again), max(&first, &after_resize));
    engine.destroy(&context);
    assert_eq!(first, again, "repeat run differs");
    assert_eq!(first, after_resize, "buffer reuse after a resize changes the result");
}
