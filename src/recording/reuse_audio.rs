//! NVIDIA NemotronLabs RE-USE (SEMamba, 30 time/frequency bidirectional Mamba
//! blocks) speech enhancement on blade compute shaders, so it runs on any
//! Vulkan GPU without ONNX Runtime or CUDA. Provenance: assets/audio/RE_USE.md.
//!
//! One `Engine::enhance` call is one inference.py forward pass over a segment
//! (InstanceNorm statistics span the whole segment). Work is submitted per
//! stage and waited on, so a caller on a worker thread never holds the GPU for
//! more than one stage at a time.
use blade_graphics as gpu;
use bytemuck::{Pod, Zeroable};
use gpu::ShaderData as _;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    ops::Range,
    path::Path,
    sync::{Mutex, OnceLock, TryLockError},
    time::Duration,
};

pub const MODEL_ID: &str = "nemotronlabs-re-use-31f82f2e-06badaf4-v1";
const MODEL_SHA256: &str = "06badaf4029b297f97f7fe48d62990b48d5482608210df577354e0d4e0ed70e1";
const MODEL: &[u8] = include_bytes!("../../assets/audio/re-use/model.safetensors");
#[used]
static LICENSE: &str = include_str!("../../assets/audio/re-use/LICENSE");
pub const RATE: usize = 48_000;
/// Long recordings are enhanced in segments of this many samples with a
/// quarter overlap, combined as inference_chunk.py does (Hann-weighted
/// overlap-add, last segment aligned to the end). Shorter audio is one
/// inference.py pass. A segment holds ~1.4 GB of f16-encoder activations.
const SEGMENT: usize = RATE * 10;
const SEGMENT_HOP: usize = SEGMENT * 3 / 4;
const NFFT: usize = 1920;
const HOP: usize = 240;
const BINS: u32 = 961;
const FP: u32 = BINS + 2;
const FC: u32 = (FP - 3) / 2 + 1;
const NONE: u32 = u32::MAX;
const PARAM_SLOT: usize = 256;
const X_GROUPS: u32 = 32768;
/// Multiply-accumulates per GEMM dispatch (~3 ms on an RTX 3060), so other
/// GPU clients can interleave at dispatch boundaries on every vendor.
const DISPATCH_MACS: u64 = 1 << 32;
/// Tensor offsets in the weight buffer are padded to whole vec4s.
const ALIGN: usize = 4;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Params {
    m: u32,
    k: u32,
    ntiles: u32,
    epi: u32,
    out_t: u32,
    out_f: u32,
    in_t: u32,
    in_f: u32,
    kt: u32,
    kf: u32,
    dil_t: u32,
    dil_f: u32,
    st: u32,
    sf: u32,
    pt: u32,
    pf: u32,
    cin: u32,
    nbuf: u32,
    in_stride: u32,
    a_off: u32,
    a_group: u32,
    a_mode: u32,
    w_off: u32,
    w_stride: u32,
    bias_off: u32,
    group_n: u32,
    out_stride: u32,
    out_off: u32,
    rt: u32,
    rf: u32,
    sub: u32,
    part_off: u32,
    norm_off: u32,
    alpha_off: u32,
    fin_off: u32,
    fin_n: u32,
    crop_t: u32,
    crop_f: u32,
    seq_len: u32,
    seq_stride: u32,
    step_stride: u32,
    n_seq: u32,
    mamba_off: u32,
    gamma_off: u32,
    beta_off: u32,
    count: u32,
    x_groups: u32,
    aux_stride: u32,
    tile_off: u32,
}
impl Params {
    fn new() -> Self {
        Self {
            bias_off: NONE,
            kt: 1,
            kf: 1,
            dil_t: 1,
            dil_f: 1,
            st: 1,
            sf: 1,
            nbuf: 1,
            rt: 1,
            rf: 1,
            sub: 1,
            group_n: 64,
            ntiles: 1,
            ..Zeroable::zeroed()
        }
    }
}

const EPI_STATS: u32 = 0;
const EPI_FINAL: u32 = 1;
const EPI_PLAIN: u32 = 2;
const EPI_GATE: u32 = 3;
const EPI_RESID: u32 = 4;
const EPI_LN: u32 = 5;

#[derive(blade_macros::ShaderData)]
struct Data {
    p: gpu::BufferPiece,
    b_w: gpu::BufferPiece,
    b_a0: gpu::BufferPiece,
    b_a1: gpu::BufferPiece,
    b_a2: gpu::BufferPiece,
    b_a3: gpu::BufferPiece,
    b_out: gpu::BufferPiece,
    b_aux: gpu::BufferPiece,
    b_part: gpu::BufferPiece,
    b_norm: gpu::BufferPiece,
    b_wav: gpu::BufferPiece,
    b_fout: gpu::BufferPiece,
}

// ---------------------------------------------------------------- weights

struct Safetensors<'a> {
    data: &'a [u8],
    tensors: HashMap<String, (Vec<usize>, Range<usize>)>,
}
impl<'a> Safetensors<'a> {
    fn parse(bytes: &'a [u8]) -> Result<Self, String> {
        let header = bytes
            .get(..8)
            .map(|b| u64::from_le_bytes(b.try_into().unwrap()) as usize)
            .ok_or("truncated safetensors")?;
        let json: serde_json::Map<String, serde_json::Value> = serde_json::from_slice(
            bytes.get(8..8 + header).ok_or("truncated safetensors header")?,
        )
        .map_err(|e| e.to_string())?;
        let data = &bytes[8 + header..];
        let mut tensors = HashMap::new();
        for (name, info) in json {
            if name == "__metadata__" {
                continue;
            }
            if info["dtype"] != "F32" {
                return Err(format!("{name}: expected F32"));
            }
            let shape: Vec<usize> = serde_json::from_value(info["shape"].clone())
                .map_err(|e| e.to_string())?;
            let offsets: [usize; 2] = serde_json::from_value(info["data_offsets"].clone())
                .map_err(|e| e.to_string())?;
            if offsets[1] > data.len() || offsets[1] - offsets[0] != shape.iter().product::<usize>() * 4 {
                return Err(format!("{name}: bad data offsets"));
            }
            tensors.insert(name, (shape, offsets[0]..offsets[1]));
        }
        Ok(Self { data, tensors })
    }
    fn get(&self, name: &str, shape: &[usize]) -> Result<Vec<f32>, String> {
        let (actual, range) = self
            .tensors
            .get(name)
            .ok_or_else(|| format!("missing tensor {name}"))?;
        if actual != shape {
            return Err(format!("{name}: shape {actual:?}, expected {shape:?}"));
        }
        Ok(self.data[range.clone()]
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
            .collect())
    }
}

#[derive(Default)]
struct Pack(Vec<f32>);
impl Pack {
    fn push(&mut self, values: &[f32]) -> u32 {
        while self.0.len() % ALIGN != 0 {
            self.0.push(0.0);
        }
        let offset = self.0.len() as u32;
        self.0.extend_from_slice(values);
        offset
    }
}

/// Conv2d (+ InstanceNorm2d + PReLU), repacked as GEMM B[k][out], k = tap * cin + ic.
#[derive(Clone, Copy)]
struct ConvW {
    w: u32,
    b: u32,
    gamma: u32,
    beta: u32,
    alpha: u32,
    cout: u32,
}
fn conv(t: &Safetensors, pack: &mut Pack, prefix: &str, conv: &str, shape: [usize; 4]) -> Result<ConvW, String> {
    let [cout, cin, kh, kw] = shape;
    let w = t.get(&format!("{prefix}.{conv}.weight"), &shape)?;
    let mut b = vec![0.0; cin * kh * kw * cout];
    for o in 0..cout {
        for i in 0..cin {
            for tap in 0..kh * kw {
                b[(tap * cin + i) * cout + o] = w[(o * cin + i) * kh * kw + tap];
            }
        }
    }
    Ok(ConvW {
        w: pack.push(&b),
        b: pack.push(&t.get(&format!("{prefix}.{conv}.bias"), &[cout])?),
        gamma: pack.push(&t.get(&format!("{prefix}.1.weight"), &[64])?),
        beta: pack.push(&t.get(&format!("{prefix}.1.bias"), &[64])?),
        alpha: pack.push(&t.get(&format!("{prefix}.2.weight"), &[64])?),
        cout: cout as u32,
    })
}
fn transpose(w: &[f32], rows: usize, cols: usize) -> Vec<f32> {
    let mut out = vec![0.0; w.len()];
    for r in 0..rows {
        for c in 0..cols {
            out[c * rows + r] = w[r * cols + c];
        }
    }
    out
}

/// One bidirectional MambaBlock; both directions share each dispatch.
#[derive(Clone, Copy)]
struct MambaW {
    block: u32,
    in_x: u32,
    in_z: u32,
    xproj: u32,
    out: u32,
    proj_w: u32,
    proj_b: u32,
    ln_g: u32,
    ln_b: u32,
}
fn mamba(t: &Safetensors, pack: &mut Pack, prefix: &str) -> Result<MambaW, String> {
    let dirs = ["forward_blocks", "backward_blocks"];
    let get = |dir: &str, name: &str, shape: &[usize]| t.get(&format!("{prefix}.{dir}.{name}"), shape);
    let mut block = vec![0.0f32; 13824];
    let (mut in_x, mut in_z) = (vec![0.0; 64 * 512], vec![0.0; 64 * 512]);
    let mut xproj = vec![0.0; 256 * 72];
    let mut out = vec![0.0; 256 * 128];
    for (d, dir) in dirs.iter().enumerate() {
        let conv_w = get(dir, "conv1d.weight", &[256, 1, 4])?;
        let conv_b = get(dir, "conv1d.bias", &[256])?;
        let dt_w = get(dir, "dt_proj.weight", &[256, 4])?;
        let dt_b = get(dir, "dt_proj.bias", &[256])?;
        let a_log = get(dir, "A_log", &[256, 16])?;
        let dd = get(dir, "D", &[256])?;
        for c in 0..256 {
            let col = d * 256 + c;
            block[col * 4..col * 4 + 4].copy_from_slice(&conv_w[c * 4..c * 4 + 4]);
            block[2048 + col] = conv_b[c];
            block[2560 + col * 4..2560 + col * 4 + 4].copy_from_slice(&dt_w[c * 4..c * 4 + 4]);
            block[4608 + col] = dt_b[c];
            for s in 0..16 {
                // As the CUDA scan: A = -exp(A_log) in f32, pre-scaled for exp2.
                block[5120 + col * 16 + s] = -a_log[c * 16 + s].exp() * std::f32::consts::LOG2_E;
            }
            block[13312 + col] = dd[c];
        }
        let in_proj = get(dir, "in_proj.weight", &[512, 64])?;
        for o in 0..256 {
            for i in 0..64 {
                in_x[i * 512 + d * 256 + o] = in_proj[o * 64 + i];
                in_z[i * 512 + d * 256 + o] = in_proj[(256 + o) * 64 + i];
            }
        }
        let x_proj = get(dir, "x_proj.weight", &[36, 256])?;
        for o in 0..36 {
            for i in 0..256 {
                xproj[i * 72 + d * 36 + o] = x_proj[o * 256 + i];
            }
        }
        let out_proj = get(dir, "out_proj.weight", &[64, 256])?;
        for o in 0..64 {
            for i in 0..256 {
                out[i * 128 + d * 64 + o] = out_proj[o * 256 + i];
            }
        }
    }
    Ok(MambaW {
        block: pack.push(&block),
        in_x: pack.push(&in_x),
        in_z: pack.push(&in_z),
        xproj: pack.push(&xproj),
        out: pack.push(&out),
        proj_w: pack.push(&transpose(&t.get(&format!("{prefix}.output_proj.weight"), &[64, 128])?, 64, 128)),
        proj_b: pack.push(&t.get(&format!("{prefix}.output_proj.bias"), &[64])?),
        ln_g: pack.push(&t.get(&format!("{prefix}.norm.weight"), &[64])?),
        ln_b: pack.push(&t.get(&format!("{prefix}.norm.bias"), &[64])?),
    })
}

fn dense(t: &Safetensors, pack: &mut Pack, prefix: &str) -> Result<[ConvW; 4], String> {
    let mut layers = Vec::new();
    for i in 0..4 {
        layers.push(conv(t, pack, &format!("{prefix}.dense_block.dense_block.{i}"), "0", [64, 64 * (i + 1), 3, 3])?);
    }
    Ok(layers.try_into().ok().unwrap())
}

struct DecoderW {
    dense: [ConvW; 4],
    up1: ConvW,
    up2: ConvW,
    fin: u32,
    phase: bool,
}
fn decoder(t: &Safetensors, pack: &mut Pack, prefix: &str, phase: bool) -> Result<DecoderW, String> {
    let dense = dense(t, pack, prefix)?;
    let up1 = conv(t, pack, &format!("{prefix}.up_conv1"), "0.conv", [128, 64, 1, 3])?;
    // up_conv2 runs on the (freq, time)-transposed tensor: its (1,3) kernel spans time.
    let up2 = conv(t, pack, &format!("{prefix}.up_conv2"), "0.conv", [256, 64, 1, 3])?;
    let mut fin = Vec::new();
    let heads: &[&str] = if phase { &["phase_conv_r", "phase_conv_i"] } else { &["final_conv"] };
    for head in heads {
        fin.extend(t.get(&format!("{prefix}.{head}.weight"), &[1, 64, 1, 1])?);
        fin.extend(t.get(&format!("{prefix}.{head}.bias"), &[1])?);
    }
    Ok(DecoderW { dense, up1, up2, fin: pack.push(&fin), phase })
}

struct Weights {
    tables: u32,
    conv1: ConvW,
    enc: [ConvW; 4],
    conv2: ConvW,
    tf: Vec<[MambaW; 2]>,
    mag: DecoderW,
    pha: DecoderW,
}
fn load_weights(bytes: &[u8]) -> Result<(Weights, Vec<f32>), String> {
    let t = Safetensors::parse(bytes)?;
    let mut pack = Pack::default();
    let mut tables = Vec::with_capacity(NFFT * 3);
    for n in 0..NFFT {
        let angle = 2.0 * std::f64::consts::PI * n as f64 / NFFT as f64;
        // Exact zeros at quarter turns keep DC/Nyquist bins purely real, as in cuFFT.
        let exact = |v: f64| if v.abs() < 1e-12 { 0.0 } else { v as f32 };
        tables.push(exact(angle.cos()));
        tables.push(exact(angle.sin()));
    }
    // torch.hann_window (periodic), evaluated in f32 as PyTorch does.
    let step = (2.0 * std::f64::consts::PI / NFFT as f64) as f32;
    tables.extend((0..NFFT).map(|n| 0.5 - 0.5 * (n as f32 * step).cos()));
    let weights = Weights {
        tables: pack.push(&tables),
        conv1: conv(&t, &mut pack, "dense_encoder.dense_conv_1", "0", [64, 2, 1, 1])?,
        enc: dense(&t, &mut pack, "dense_encoder")?,
        conv2: conv(&t, &mut pack, "dense_encoder.dense_conv_2", "0", [64, 64, 1, 3])?,
        tf: (0..30)
            .map(|i| {
                Ok([
                    mamba(&t, &mut pack, &format!("TSMamba.{i}.time_mamba"))?,
                    mamba(&t, &mut pack, &format!("TSMamba.{i}.freq_mamba"))?,
                ])
            })
            .collect::<Result<_, String>>()?,
        mag: decoder(&t, &mut pack, "mask_decoder", false)?,
        pha: decoder(&t, &mut pack, "phase_decoder", true)?,
    };
    Ok((weights, pack.0))
}

// ---------------------------------------------------------------- plan

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Geometry {
    pub samples: u32,
    /// STFT frames.
    pub t: u32,
    /// Zero-padded model input frames (t + 2) and bins (961 + 2).
    pub tp: u32,
    /// Encoder output frames; the encoder output has 481 bins.
    pub tc: u32,
}
impl Geometry {
    pub fn new(samples: usize) -> Result<Self, String> {
        // Reflect padding needs more samples than half an FFT frame.
        if samples <= NFFT / 2 || samples > 60 * 60 * RATE {
            return Err("RE-USE segment length out of range".into());
        }
        let t = (1 + samples / HOP) as u32;
        let tp = t + 2;
        Ok(Self { samples: samples as u32, t, tp, tc: (tp - 1) / 4 + 1 })
    }
    fn full(&self) -> u32 {
        self.tp * FP
    }
    fn core(&self) -> u32 {
        self.tc * FC
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Buf {
    /// Input samples (f32).
    Wav,
    /// [tp][963][log1p magnitude, phase].
    Input,
    /// Encoder input e; Mamba XI/Y [tc·481][512]; decoder up_conv1; iSTFT frames (f32).
    Wide,
    /// Dense outputs x0..x2; then Mamba dbc [tc·481][72] and O [tc·481][128].
    S0,
    S1,
    S2,
    /// Encoder x3 rows t % 4 == 0 [tc][963][64]; decoder x3.
    S3,
    /// TF-block activations [tc][481][64].
    X,
    /// Model magnitude then phase, each [t][961] (f32).
    Spec,
    /// Output samples (f32).
    Out,
    Part,
    Norm,
}
const BUFS: [Buf; 12] = [
    Buf::Wav,
    Buf::Input,
    Buf::Wide,
    Buf::S0,
    Buf::S1,
    Buf::S2,
    Buf::S3,
    Buf::X,
    Buf::Spec,
    Buf::Out,
    Buf::Part,
    Buf::Norm,
];

#[derive(Clone, Copy)]
enum Kind {
    Gemm,
    NormStats,
    NormApply,
    Stft,
    IstftFrames,
    IstftOla,
    Scan,
}

/// Pipeline variants by activation storage read → written.
const FULL: usize = 0;
const HALF: usize = 1;
const HALF_TO_FULL: usize = 2;
const VARIANTS: [(&str, &str); 3] = [("f32", "f32"), ("f16", "f16"), ("f16", "f32")];

#[derive(Clone, Copy)]
struct Op {
    kind: Kind,
    params: Params,
    a: [Buf; 4],
    out: Buf,
    aux: Buf,
    wav: Buf,
    fout: Buf,
    groups: [u32; 3],
    variant: usize,
}
impl Op {
    fn new(kind: Kind, params: Params) -> Self {
        Self {
            kind,
            params,
            a: [Buf::Norm; 4],
            out: Buf::Part,
            aux: Buf::Norm,
            wav: Buf::Norm,
            fout: Buf::Part,
            groups: [1, 1, 1],
            variant: usize::MAX,
        }
    }
}

fn spread(groups: u32, params: &mut Params) -> [u32; 3] {
    let x = groups.clamp(1, X_GROUPS);
    params.x_groups = x;
    [x, 1, groups.div_ceil(x).max(1)]
}

/// Implicit-GEMM convolution layer description.
struct ConvSpec {
    inputs: Vec<Buf>,
    in_tf: (u32, u32),
    cin: u32,
    out_tf: (u32, u32),
    kernel: (u32, u32),
    dil_t: u32,
    stride: (u32, u32),
    pad: (u32, u32),
    shuffle: (u32, u32),
}

struct Builder<'a> {
    w: &'a Weights,
    g: Geometry,
    ops: Vec<Op>,
    stages: Vec<(String, Range<usize>)>,
    /// First op of each named step inside a stage (for parity tests).
    marks: Vec<(String, usize)>,
    norms: u32,
    part_entries: u32,
    /// Store the full-resolution encoder activations as f16.
    half: bool,
    variant: usize,
}
impl Builder<'_> {
    fn mark(&mut self, step: &str) {
        let stage = self.stages.last().map_or("", |s| s.0.as_str());
        self.marks.push((format!("{stage}.{step}"), self.ops.len()));
    }
    fn push(&mut self, mut op: Op) {
        if op.variant == usize::MAX {
            op.variant = self.variant;
        }
        if !matches!(op.kind, Kind::Gemm) {
            self.ops.push(op);
            return;
        }
        let p = op.params;
        let tiles = p.m.div_ceil(64);
        let per_tile = 64 * 64 * u64::from(p.k) * u64::from(p.ntiles);
        let step = (DISPATCH_MACS / per_tile).clamp(1, u64::from(tiles)) as u32;
        for start in (0..tiles).step_by(step as usize) {
            let mut piece = op;
            piece.params.tile_off = start;
            piece.params.count = step.min(tiles - start);
            piece.groups = spread(piece.params.count, &mut piece.params);
            piece.groups[1] = p.ntiles;
            self.ops.push(piece);
        }
    }
    fn stage(&mut self, name: impl Into<String>) {
        let end = self.ops.len();
        if let Some(last) = self.stages.last_mut() {
            if last.1.end == usize::MAX {
                last.1.end = end;
            }
        }
        self.stages.push((name.into(), end..usize::MAX));
    }
    fn finish(&mut self) {
        let end = self.ops.len();
        if let Some(last) = self.stages.last_mut() {
            last.1.end = end;
        }
    }

    fn gemm_params(&self, spec: &ConvSpec, cw: &ConvW) -> Params {
        let mut p = Params::new();
        p.m = spec.out_tf.0 * spec.out_tf.1;
        p.nbuf = spec.inputs.len() as u32;
        p.cin = spec.cin;
        p.k = spec.kernel.0 * spec.kernel.1 * spec.cin * p.nbuf;
        p.ntiles = cw.cout / 64;
        p.out_t = spec.out_tf.0;
        p.out_f = spec.out_tf.1;
        p.in_t = spec.in_tf.0;
        p.in_f = spec.in_tf.1;
        p.kt = spec.kernel.0;
        p.kf = spec.kernel.1;
        p.dil_t = spec.dil_t;
        p.st = spec.stride.0;
        p.sf = spec.stride.1;
        p.pt = spec.pad.0;
        p.pf = spec.pad.1;
        p.in_stride = spec.cin;
        p.w_off = cw.w;
        p.w_stride = cw.cout;
        p.bias_off = cw.b;
        p.rt = spec.shuffle.0;
        p.rf = spec.shuffle.1;
        p.gamma_off = cw.gamma;
        p.beta_off = cw.beta;
        p.alpha_off = cw.alpha;
        p
    }
    fn gemm_op(&self, mut p: Params, inputs: &[Buf]) -> Op {
        let mtiles = p.m.div_ceil(64);
        let mut groups = spread(mtiles, &mut p);
        groups[1] = p.ntiles;
        let mut op = Op::new(Kind::Gemm, p);
        for (slot, input) in op.a.iter_mut().zip(inputs) {
            *slot = *input;
        }
        op.groups = groups;
        op
    }
    /// Conv + InstanceNorm statistics; `out` receives raw values for rows with
    /// t % sub == 0 (sub 0: statistics only). Returns the norm slot.
    fn conv_stats(&mut self, spec: &ConvSpec, cw: &ConvW, out: Buf, sub: u32) -> u32 {
        let mut p = self.gemm_params(spec, cw);
        p.epi = EPI_STATS;
        p.sub = sub;
        let entries = p.m.div_ceil(64) * p.ntiles;
        self.part_entries = self.part_entries.max(entries);
        let mut op = self.gemm_op(p, &spec.inputs);
        op.out = out;
        self.push(op);
        let norm = self.norms * 128;
        self.norms += 1;
        let mut s = Params::new();
        s.count = entries;
        s.norm_off = norm;
        s.gamma_off = cw.gamma;
        let mut op = Op::new(Kind::NormStats, s);
        op.groups = [64, 1, 1];
        self.push(op);
        norm
    }
    fn norm_apply(&mut self, buf: Buf, positions: u32, cw: &ConvW, norm: u32) {
        let mut p = Params::new();
        p.m = positions;
        p.norm_off = norm;
        p.beta_off = cw.beta;
        p.alpha_off = cw.alpha;
        let groups = spread((positions * 64).div_ceil(256), &mut p);
        let mut op = Op::new(Kind::NormApply, p);
        op.out = buf;
        op.groups = groups;
        op.variant = if self.variant == HALF_TO_FULL { FULL } else { self.variant };
        self.push(op);
    }
    fn conv_layer(&mut self, spec: ConvSpec, cw: &ConvW, out: Buf, sub: u32) {
        let norm = self.conv_stats(&spec, cw, out, sub);
        let rows = (spec.out_tf.0 * spec.shuffle.0).div_ceil(sub);
        self.norm_apply(out, rows * spec.out_tf.1 * spec.shuffle.1, cw, norm);
    }
    fn dense_block(&mut self, prefix: &str, layers: &[ConvW; 4], e: Buf, tf: (u32, u32), last: Buf, last_sub: u32) {
        let outs = [Buf::S0, Buf::S1, Buf::S2, last];
        for i in 0..4 {
            self.stage(format!("{prefix}_dense{i}"));
            let mut inputs: Vec<Buf> = outs[..i].iter().rev().copied().collect();
            inputs.push(e);
            let d = 1 << i;
            let sub = if i == 3 { last_sub } else { 1 };
            self.conv_layer(
                ConvSpec {
                    inputs,
                    in_tf: tf,
                    cin: 64,
                    out_tf: tf,
                    kernel: (3, 3),
                    dil_t: d,
                    stride: (1, 1),
                    pad: (d, 1),
                    shuffle: (1, 1),
                },
                &layers[i],
                outs[i],
                sub,
            );
        }
    }

    fn linear(&self, a: Buf, in_stride: u32, k: u32, n_total: u32, group_n: u32, w: u32, epi: u32) -> Op {
        let g = self.g;
        let mut p = Params::new();
        p.m = g.core();
        p.k = k;
        p.cin = k;
        p.in_stride = in_stride;
        p.ntiles = n_total / group_n;
        p.group_n = group_n;
        p.out_t = g.tc;
        p.out_f = FC;
        p.in_t = g.tc;
        p.in_f = FC;
        p.w_off = w;
        p.w_stride = n_total;
        p.epi = epi;
        self.gemm_op(p, &[a])
    }
    fn mamba_block(&mut self, mw: &MambaW, time: bool) {
        let g = self.g;
        let (seq_len, n_seq, seq_stride, step_stride) =
            if time { (g.tc, FC, 1, FC) } else { (FC, g.tc, FC, 1) };
        self.mark("in");
        // XI = x · W_x (both directions).
        let mut op = self.linear(Buf::X, 64, 64, 512, 64, mw.in_x, EPI_PLAIN);
        op.out = Buf::Wide;
        op.params.out_stride = 512;
        self.push(op);
        self.mark("xproj");
        // dbc = x_proj(silu(conv1d(XI))), conv evaluated on the fly.
        let mut op = self.linear(Buf::Wide, 512, 256, 72, 36, mw.xproj, EPI_PLAIN);
        op.params.a_mode = 1;
        op.params.a_group = 256;
        op.params.mamba_off = mw.block;
        op.params.seq_len = seq_len;
        op.params.step_stride = step_stride;
        op.params.out_stride = 72;
        op.out = Buf::S0;
        self.push(op);
        self.mark("scan");
        // Selective scan: Y overwrites XI.
        let mut p = Params::new();
        p.seq_len = seq_len;
        p.n_seq = n_seq;
        p.seq_stride = seq_stride;
        p.step_stride = step_stride;
        p.mamba_off = mw.block;
        let mut op = Op::new(Kind::Scan, p);
        op.out = Buf::Wide;
        op.a[1] = Buf::S0;
        op.groups = [n_seq, 32, 1];
        self.push(op);
        self.mark("gate");
        // Y *= silu(x · W_z).
        let mut op = self.linear(Buf::X, 64, 64, 512, 64, mw.in_z, EPI_GATE);
        op.out = Buf::Wide;
        op.params.out_stride = 512;
        self.push(op);
        self.mark("out");
        // O = [out_proj_f(Y_f), out_proj_b(Y_b)] + x.
        let mut op = self.linear(Buf::Wide, 512, 256, 128, 64, mw.out, EPI_RESID);
        op.params.a_group = 256;
        op.params.out_stride = 128;
        op.params.aux_stride = 64;
        op.out = Buf::S1;
        op.aux = Buf::X;
        self.push(op);
        self.mark("ln");
        // x += LayerNorm(output_proj(O)).
        let mut op = self.linear(Buf::S1, 128, 128, 64, 64, mw.proj_w, EPI_LN);
        op.params.bias_off = mw.proj_b;
        op.params.gamma_off = mw.ln_g;
        op.params.beta_off = mw.ln_b;
        op.out = Buf::X;
        self.push(op);
    }
    fn decoder(&mut self, prefix: &str, dw: &DecoderW) {
        let g = self.g;
        self.dense_block(prefix, &dw.dense, Buf::X, (g.tc, FC), Buf::S3, 1);
        self.stage(format!("{prefix}_up1"));
        self.conv_layer(
            ConvSpec {
                inputs: vec![Buf::S3],
                in_tf: (g.tc, FC),
                cin: 64,
                out_tf: (g.tc, FC),
                kernel: (1, 3),
                dil_t: 1,
                stride: (1, 1),
                pad: (0, 1),
                shuffle: (1, 2),
            },
            &dw.up1,
            Buf::Wide,
            1,
        );
        self.stage(format!("{prefix}_final"));
        let spec = ConvSpec {
            inputs: vec![Buf::Wide],
            in_tf: (g.tc, 2 * FC),
            cin: 64,
            out_tf: (g.tc, 2 * FC),
            kernel: (3, 1),
            dil_t: 1,
            stride: (1, 1),
            pad: (1, 0),
            shuffle: (4, 1),
        };
        let norm = self.conv_stats(&spec, &dw.up2, Buf::Part, 0);
        let mut p = self.gemm_params(&spec, &dw.up2);
        p.epi = EPI_FINAL;
        p.norm_off = norm;
        p.fin_off = dw.fin;
        p.fin_n = if dw.phase { 2 } else { 1 };
        p.crop_t = g.t;
        p.crop_f = BINS;
        p.out_off = if dw.phase { g.t * BINS } else { 0 };
        let mut op = self.gemm_op(p, &spec.inputs);
        op.fout = Buf::Spec;
        self.push(op);
    }

    fn build(mut self) -> Plan {
        let g = self.g;
        let w = self.w;
        self.variant = if self.half { HALF } else { FULL };
        self.stage("stft");
        let mut p = Params::new();
        p.count = g.samples;
        p.in_t = g.t;
        p.out_t = g.tp;
        p.out_f = FP;
        p.in_f = BINS;
        p.w_off = w.tables;
        let groups = spread(g.tp, &mut p);
        let mut op = Op::new(Kind::Stft, p);
        op.wav = Buf::Wav;
        op.out = Buf::Input;
        op.groups = groups;
        self.push(op);

        self.stage("enc_conv1");
        let full = (g.tp, FP);
        self.conv_layer(
            ConvSpec {
                inputs: vec![Buf::Input],
                in_tf: full,
                cin: 2,
                out_tf: full,
                kernel: (1, 1),
                dil_t: 1,
                stride: (1, 1),
                pad: (0, 0),
                shuffle: (1, 1),
            },
            &w.conv1,
            Buf::Wide,
            1,
        );
        self.dense_block("enc", &w.enc, Buf::Wide, full, Buf::S3, 4);
        self.stage("enc_conv2");
        self.variant = if self.half { HALF_TO_FULL } else { FULL };
        self.conv_layer(
            ConvSpec {
                inputs: vec![Buf::S3],
                in_tf: (g.tc, FP),
                cin: 64,
                out_tf: (g.tc, FC),
                kernel: (1, 3),
                dil_t: 1,
                stride: (1, 2),
                pad: (0, 0),
                shuffle: (1, 1),
            },
            &w.conv2,
            Buf::X,
            1,
        );
        self.variant = FULL;
        for (i, block) in w.tf.iter().enumerate() {
            self.stage(format!("tf{i:02}_time"));
            self.mamba_block(&block[0], true);
            self.stage(format!("tf{i:02}_freq"));
            self.mamba_block(&block[1], false);
        }
        self.decoder("mag", &w.mag);
        self.decoder("pha", &w.pha);

        self.stage("istft");
        let mut p = Params::new();
        p.in_t = g.t;
        p.in_f = BINS;
        p.out_off = g.t * BINS;
        p.w_off = w.tables;
        let groups = spread(g.t, &mut p);
        let mut op = Op::new(Kind::IstftFrames, p);
        op.wav = Buf::Spec;
        op.fout = Buf::Wide;
        op.groups = groups;
        self.push(op);
        let mut p = Params::new();
        p.count = g.samples;
        p.in_t = g.t;
        p.w_off = w.tables;
        let groups = spread(g.samples.div_ceil(256), &mut p);
        let mut op = Op::new(Kind::IstftOla, p);
        op.wav = Buf::Wide;
        op.fout = Buf::Out;
        op.groups = groups;
        self.push(op);
        self.finish();
        Plan {
            geometry: g,
            ops: self.ops,
            stages: self.stages,
            marks: self.marks,
            norms: self.norms,
            part_entries: self.part_entries,
        }
    }
}

struct Plan {
    geometry: Geometry,
    ops: Vec<Op>,
    stages: Vec<(String, Range<usize>)>,
    marks: Vec<(String, usize)>,
    norms: u32,
    part_entries: u32,
}
impl Plan {
    /// Bytes per buffer; `store` is the activation element size.
    /// Bytes per buffer; `half` is the encoder activation element size.
    fn sizes(&self, half: usize) -> HashMap<Buf, usize> {
        let g = self.geometry;
        let (full, core) = (g.full() as usize, g.core() as usize);
        let up1 = g.tc as usize * 2 * FC as usize;
        let encoder = full * 64 * half;
        BUFS.iter()
            .map(|&b| {
                let size = match b {
                    Buf::Wav | Buf::Out => g.samples as usize * 4,
                    Buf::Input => full * 2 * half,
                    Buf::Wide => encoder
                        .max(core * 512 * 4)
                        .max(up1 * 64 * 4)
                        .max(g.t as usize * NFFT * 4),
                    Buf::S0 => encoder.max(core * 72 * 4),
                    Buf::S1 => encoder.max(core * 128 * 4),
                    Buf::S2 => encoder.max(core * 64 * 4),
                    Buf::S3 => (g.tc as usize * FP as usize * 64 * half).max(core * 64 * 4),
                    Buf::X => core * 64 * 4,
                    Buf::Spec => 2 * g.t as usize * BINS as usize * 4,
                    Buf::Part => self.part_entries as usize * 64 * 16,
                    Buf::Norm => self.norms as usize * 128 * 4,
                };
                (b, size.max(16))
            })
            .collect()
    }
}

// ---------------------------------------------------------------- engine

struct Allocation {
    buffer: gpu::Buffer,
    size: usize,
}
fn allocate(context: &gpu::Context, size: usize, memory: gpu::Memory, name: &str) -> Allocation {
    let size = size.max(16);
    Allocation {
        buffer: context.create_buffer(gpu::BufferDesc { name, size: size as u64, memory }),
        size,
    }
}

struct Pipelines {
    gemm: gpu::ComputePipeline,
    norm_stats: gpu::ComputePipeline,
    norm_apply: gpu::ComputePipeline,
    stft: gpu::ComputePipeline,
    istft_frames: gpu::ComputePipeline,
    istft_ola: gpu::ComputePipeline,
    scan: gpu::ComputePipeline,
}

struct Resources {
    plan: Plan,
    buffers: HashMap<Buf, Allocation>,
    params: Allocation,
}

pub struct Engine {
    /// Indexed by precision variant (FULL, HALF, HALF_TO_FULL).
    pipelines: Vec<Pipelines>,
    weights: Weights,
    weight_buffer: Allocation,
    pub uses_f16: bool,
    encoder: gpu::CommandEncoder,
    resources: Option<Resources>,
}

impl Engine {
    pub fn new(context: &gpu::Context, model: &[u8], allow_f16: bool) -> Result<Self, String> {
        let (weights, packed) = load_weights(model)?;
        let native_f16 = allow_f16 && context.capabilities().shader_f16;
        let source = |variant: usize| {
            let (input, output) = VARIANTS[variant];
            let types = format!("alias In = {input};\nalias Out = {output};");
            let types = if variant == FULL { types } else { format!("enable f16;\n{types}") };
            include_str!("reuse_audio.wgsl").replacen("alias In = f32;\nalias Out = f32;", &types, 1)
        };
        let mut shaders = vec![context.try_create_shader(gpu::ShaderDesc { source: &source(FULL) })?];
        if native_f16 {
            // Devices that advertise f16 but reject the module fall back to f32.
            let half: Result<Vec<_>, _> = [HALF, HALF_TO_FULL]
                .iter()
                .map(|&v| context.try_create_shader(gpu::ShaderDesc { source: &source(v) }))
                .collect();
            if let Ok(half) = half {
                shaders.extend(half);
            }
        }
        let uses_f16 = shaders.len() == VARIANTS.len();
        let layout = Data::layout();
        let pipelines = shaders
            .iter()
            .map(|shader| {
                shader.check_struct_size::<Params>();
                let pipeline = |entry: &str| {
                    context.create_compute_pipeline(gpu::ComputePipelineDesc {
                        name: "RE-USE",
                        data_layouts: &[&layout],
                        compute: shader.at(entry),
                    })
                };
                Pipelines {
                    gemm: pipeline("gemm"),
                    norm_stats: pipeline("norm_stats"),
                    norm_apply: pipeline("norm_apply"),
                    stft: pipeline("stft"),
                    istft_frames: pipeline("istft_frames"),
                    istft_ola: pipeline("istft_ola"),
                    scan: pipeline("scan"),
                }
            })
            .collect();
        let bytes: &[u8] = bytemuck::cast_slice(&packed);
        let weight_buffer = allocate(context, bytes.len(), gpu::Memory::Device, "RE-USE weights");
        let mut encoder = context.create_command_encoder(gpu::CommandEncoderDesc { name: "RE-USE", buffer_count: 1 });
        upload(context, &mut encoder, weight_buffer.buffer, bytes);
        Ok(Self { pipelines, weights, weight_buffer, uses_f16, encoder, resources: None })
    }

    /// Prepares buffers and the dispatch list for a segment of `samples`.
    pub fn prepare(&mut self, context: &gpu::Context, samples: usize) -> Result<Geometry, String> {
        let g = Geometry::new(samples)?;
        if self.resources.as_ref().is_some_and(|r| r.plan.geometry == g) {
            return Ok(g);
        }
        let plan = Builder {
            w: &self.weights,
            g,
            ops: Vec::new(),
            stages: Vec::new(),
            marks: Vec::new(),
            norms: 0,
            part_entries: 0,
            half: self.uses_f16,
            variant: FULL,
        }
        .build();
        let sizes = plan.sizes(if self.uses_f16 { 2 } else { 4 });
        let mut buffers = HashMap::new();
        if let Some(old) = self.resources.take() {
            // Reuse buffers that are already large enough.
            for (b, a) in old.buffers {
                if a.size >= sizes[&b] {
                    buffers.insert(b, a);
                } else {
                    context.destroy_buffer(a.buffer);
                }
            }
            context.destroy_buffer(old.params.buffer);
        }
        for (&b, &size) in &sizes {
            buffers.entry(b).or_insert_with(|| allocate(context, size, gpu::Memory::Device, "RE-USE activations"));
        }
        let params = allocate(context, plan.ops.len() * PARAM_SLOT, gpu::Memory::Shared, "RE-USE params");
        for (i, op) in plan.ops.iter().enumerate() {
            let bytes = bytemuck::bytes_of(&op.params);
            unsafe {
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), params.buffer.data().add(i * PARAM_SLOT), bytes.len());
            }
        }
        context.sync_buffer(params.buffer);
        self.resources = Some(Resources { plan, buffers, params });
        Ok(g)
    }

    /// Device memory held by weights and the current segment's buffers.
    #[cfg(test)]
    pub fn device_bytes(&self) -> usize {
        self.weight_buffer.size
            + self.resources.as_ref().map_or(0, |r| r.buffers.values().map(|a| a.size).sum::<usize>() + r.params.size)
    }

    #[cfg(test)]
    pub fn stage_names(&self) -> Vec<String> {
        self.resources.as_ref().map_or_else(Vec::new, |r| r.plan.stages.iter().map(|s| s.0.clone()).collect())
    }

    fn piece(&self, b: Buf) -> gpu::BufferPiece {
        self.resources.as_ref().unwrap().buffers[&b].buffer.into()
    }

    /// Records and runs the named stages, one submission each.
    #[cfg(test)]
    pub fn run_stages(&mut self, context: &gpu::Context, names: &[&str]) -> Result<(), String> {
        for name in names {
            let range = self
                .resources
                .as_ref()
                .and_then(|r| r.plan.stages.iter().find(|s| s.0 == *name))
                .map(|s| s.1.clone())
                .ok_or_else(|| format!("unknown stage {name}"))?;
            self.run_ops(context, range)?;
        }
        Ok(())
    }

    pub fn run_ops(&mut self, context: &gpu::Context, range: Range<usize>) -> Result<(), String> {
        let resources = self.resources.as_ref().ok_or("RE-USE segment not prepared")?;
        self.encoder.start();
        for index in range {
            let op = &resources.plan.ops[index];
            let data = Data {
                p: resources.params.buffer.at((index * PARAM_SLOT) as u64),
                b_w: self.weight_buffer.buffer.into(),
                b_a0: self.piece(op.a[0]),
                b_a1: self.piece(op.a[1]),
                b_a2: self.piece(op.a[2]),
                b_a3: self.piece(op.a[3]),
                b_out: self.piece(op.out),
                b_aux: self.piece(op.aux),
                b_part: self.piece(Buf::Part),
                b_norm: self.piece(Buf::Norm),
                b_wav: self.piece(op.wav),
                b_fout: self.piece(op.fout),
            };
            let pipeline = match op.kind {
                Kind::Gemm => &self.pipelines[op.variant].gemm,
                Kind::NormStats => &self.pipelines[op.variant].norm_stats,
                Kind::NormApply => &self.pipelines[op.variant].norm_apply,
                Kind::Stft => &self.pipelines[op.variant].stft,
                Kind::IstftFrames => &self.pipelines[op.variant].istft_frames,
                Kind::IstftOla => &self.pipelines[op.variant].istft_ola,
                Kind::Scan => &self.pipelines[op.variant].scan,
            };
            let mut compute = self.encoder.compute("RE-USE");
            let mut pass = compute.with(pipeline);
            pass.bind(0, &data);
            pass.dispatch(op.groups);
        }
        let sync = context.submit(&mut self.encoder);
        if !context.wait_for(&sync, 60_000) {
            return Err("RE-USE GPU stage timed out".into());
        }
        Ok(())
    }

    /// Writes f32 values into a buffer, converting to the storage type for
    /// activation buffers.
    /// Writes f32 values into a buffer; `half` marks an encoder activation
    /// buffer, stored as f16 when the engine uses half storage.
    pub fn write(&mut self, context: &gpu::Context, b: Buf, values: &[f32], half: bool) {
        let bytes = if half && self.uses_f16 {
            values.iter().flat_map(|&v| f16_bits(v).to_le_bytes()).collect()
        } else {
            bytemuck::cast_slice(values).to_vec()
        };
        let target = self.resources.as_ref().unwrap().buffers[&b].buffer;
        upload(context, &mut self.encoder, target, &bytes);
    }

    pub fn read(&mut self, context: &gpu::Context, b: Buf, count: usize, half: bool) -> Vec<f32> {
        let size = if half && self.uses_f16 { 2 } else { 4 };
        let source = self.piece(b);
        let bytes = download(context, &mut self.encoder, source, count * size);
        if size == 2 {
            bytes.chunks_exact(2).map(|c| f16_value(u16::from_le_bytes([c[0], c[1]]))).collect()
        } else {
            bytes.chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect()
        }
    }

    /// Enhances one segment (one inference.py forward pass).
    pub fn enhance(
        &mut self,
        context: &gpu::Context,
        samples: &[f32],
        cancelled: &dyn Fn() -> bool,
        progress: &mut dyn FnMut(f64),
    ) -> Result<Vec<f32>, String> {
        self.prepare(context, samples.len())?;
        self.write(context, Buf::Wav, samples, false);
        let stages: Vec<Range<usize>> = self.resources.as_ref().unwrap().plan.stages.iter().map(|s| s.1.clone()).collect();
        let count = stages.len();
        for (index, range) in stages.into_iter().enumerate() {
            check(cancelled)?;
            self.run_ops(context, range)?;
            progress((index + 1) as f64 / count as f64);
        }
        Ok(self.read(context, Buf::Out, samples.len(), false))
    }

    /// Call only after every submission has completed.
    pub fn destroy(&mut self, context: &gpu::Context) {
        for p in &mut self.pipelines {
            for pipeline in [
                &mut p.gemm,
                &mut p.norm_stats,
                &mut p.norm_apply,
                &mut p.stft,
                &mut p.istft_frames,
                &mut p.istft_ola,
                &mut p.scan,
            ] {
                context.destroy_compute_pipeline(pipeline);
            }
        }
        if let Some(r) = self.resources.take() {
            for a in r.buffers.into_values() {
                context.destroy_buffer(a.buffer);
            }
            context.destroy_buffer(r.params.buffer);
        }
        context.destroy_buffer(self.weight_buffer.buffer);
        context.destroy_command_encoder(&mut self.encoder);
    }
}

struct Gpu {
    context: gpu::Context,
    engine: Engine,
    /// A CPU Vulkan implementation (lavapipe): ~50 s per audio second.
    software: bool,
}
fn gpu_engine() -> &'static Mutex<Option<Gpu>> {
    static ENGINE: OnceLock<Mutex<Option<Gpu>>> = OnceLock::new();
    ENGINE.get_or_init(|| Mutex::new(None))
}
fn load_gpu() -> Result<Gpu, String> {
    std::hint::black_box(LICENSE);
    if format!("{:x}", Sha256::digest(MODEL)) != MODEL_SHA256 {
        return Err("Bundled speech enhancement model is corrupt".into());
    }
    let context = unsafe { gpu::Context::init(gpu::ContextDesc::default()) }
        .map_err(|e| format!("Noise reduction needs Vulkan ({e:?})"))?;
    let software = context.device_information().is_software_emulated;
    if software {
        eprintln!("RE-USE is running on a CPU Vulkan device: {:?}", context.device_information());
    }
    let allow_f16 = std::env::var_os("LAHZA_REUSE_F32").is_none();
    let engine = Engine::new(&context, MODEL, allow_f16)?;
    Ok(Gpu { context, engine, software })
}
fn check(cancelled: &dyn Fn() -> bool) -> Result<(), String> {
    if cancelled() {
        Err("Audio enhancement cancelled".into())
    } else {
        Ok(())
    }
}
fn read_frames(file: &mut File, start: usize, frames: usize, channels: usize) -> Result<Vec<f32>, String> {
    file.seek(SeekFrom::Start((start * channels * 4) as u64)).map_err(|e| e.to_string())?;
    let mut bytes = vec![0; frames * channels * 4];
    file.read_exact(&mut bytes).map_err(|e| e.to_string())?;
    let values: Vec<f32> = bytes.chunks_exact(4).map(|b| f32::from_le_bytes(b.try_into().unwrap())).collect();
    if values.iter().any(|v| !v.is_finite()) {
        return Err("Audio contains non-finite samples".into());
    }
    Ok(values)
}
fn write_frames(file: &mut File, values: &[f32]) -> Result<(), String> {
    let bytes: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
    file.write_all(&bytes).map_err(|e| e.to_string())
}

pub const CPU_WARNING: &str =
    "Removing background noise on the CPU — no GPU found, this is very slow";

/// Input/output are interleaved 48 kHz f32 PCM; output duration and channels
/// are exact. Runs on the caller's (worker) thread with its own Vulkan context;
/// GPU memory is bounded by one segment and host memory by two. Without a GPU
/// it still runs on a CPU Vulkan device (lavapipe), reporting `CPU_WARNING`
/// through `notice` first.
pub fn enhance_file(
    input: &Path,
    output: &Path,
    channels: usize,
    cancelled: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(f64),
    notice: &mut dyn FnMut(&str),
) -> Result<(), String> {
    check(cancelled)?;
    if !(1..=2).contains(&channels) {
        return Err("Speech enhancement expects mono or stereo audio".into());
    }
    #[cfg(unix)]
    let same_file = {
        use std::os::unix::fs::MetadataExt;
        match (std::fs::metadata(input), std::fs::metadata(output)) {
            (Ok(i), Ok(o)) => i.dev() == o.dev() && i.ino() == o.ino(),
            _ => false,
        }
    };
    #[cfg(not(unix))]
    let same_file = false;
    if input == output || same_file {
        return Err("Speech enhancement output must differ from its input".into());
    }
    let mut source = File::open(input).map_err(|e| e.to_string())?;
    let bytes = source.metadata().map_err(|e| e.to_string())?.len();
    if bytes % (4 * channels) as u64 != 0 {
        return Err("Audio has an incomplete PCM frame".into());
    }
    let total = usize::try_from(bytes / (4 * channels) as u64).map_err(|_| "Audio is too long for this platform")?;
    let mut target = File::create(output).map_err(|e| e.to_string())?;
    if total == 0 {
        progress(1.0);
        return Ok(());
    }
    let mut shared = loop {
        check(cancelled)?;
        match gpu_engine().try_lock() {
            Ok(guard) => break guard,
            Err(TryLockError::WouldBlock) => std::thread::sleep(Duration::from_millis(20)),
            Err(TryLockError::Poisoned(e)) => break e.into_inner(),
        }
    };
    if shared.is_none() {
        *shared = Some(load_gpu()?);
    }
    let Gpu { context, engine, software } = shared.as_mut().unwrap();
    if *software {
        notice(CPU_WARNING);
    }

    let segment = total.min(SEGMENT);
    let starts: Vec<usize> = if total <= SEGMENT {
        vec![0]
    } else {
        let count = (total - SEGMENT).div_ceil(SEGMENT_HOP) + 1;
        (0..count).map(|i| if i + 1 < count { i * SEGMENT_HOP } else { total - SEGMENT }).collect()
    };
    // inference_chunk.py's synthesis window: periodic Hann clamped at 1e-7.
    let window: Vec<f32> = if starts.len() == 1 {
        vec![1.0; segment]
    } else {
        (0..segment)
            .map(|n| (0.5 - 0.5 * (2.0 * std::f64::consts::PI * n as f64 / segment as f64).cos()).max(1e-7) as f32)
            .collect()
    };
    // Weighted sums for frames [written, written + sum.len() / channels).
    let mut written = 0;
    let mut sum: Vec<f32> = Vec::new();
    let mut weight: Vec<f32> = Vec::new();
    let flush = |upto: usize, written: &mut usize, sum: &mut Vec<f32>, weight: &mut Vec<f32>, target: &mut File| {
        let frames = upto - *written;
        let values: Vec<f32> = sum[..frames * channels].iter().zip(&weight[..frames * channels]).map(|(s, w)| s / w).collect();
        sum.drain(..frames * channels);
        weight.drain(..frames * channels);
        *written = upto;
        write_frames(target, &values)
    };
    let pieces = starts.len();
    for (index, &start) in starts.iter().enumerate() {
        check(cancelled)?;
        flush(start, &mut written, &mut sum, &mut weight, &mut target)?;
        let values = read_frames(&mut source, start, segment, channels)?;
        // Very short audio is zero-padded to the STFT's minimum length.
        let padded = segment.max(NFFT);
        let identical = channels == 2 && values.chunks_exact(2).all(|f| f[0].to_bits() == f[1].to_bits());
        let mut enhanced = vec![Vec::new(); channels];
        let passes = if identical { 1 } else { channels };
        for channel in 0..passes {
            let mut mono: Vec<f32> = (0..segment).map(|i| values[i * channels + channel]).collect();
            mono.resize(padded, 0.0);
            let done = (index * passes + channel) as f64;
            let total_passes = (pieces * passes) as f64;
            let mut out = engine.enhance(context, &mono, cancelled, &mut |f| progress((done + f) / total_passes))?;
            out.truncate(segment);
            enhanced[channel] = out;
        }
        if identical {
            enhanced[1] = enhanced[0].clone();
        }
        let needed = (start + segment - written) * channels;
        sum.resize(needed, 0.0);
        weight.resize(needed, 0.0);
        for i in 0..segment {
            for channel in 0..channels {
                let at = (start + i - written) * channels + channel;
                sum[at] += enhanced[channel][i] * window[i];
                weight[at] += window[i];
            }
        }
    }
    flush(total, &mut written, &mut sum, &mut weight, &mut target)?;
    target.sync_all().map_err(|e| e.to_string())?;
    progress(1.0);
    Ok(())
}

fn upload(context: &gpu::Context, encoder: &mut gpu::CommandEncoder, target: gpu::Buffer, bytes: &[u8]) {
    let staging = allocate(context, bytes.len(), gpu::Memory::Shared, "RE-USE upload");
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), staging.buffer.data(), bytes.len()) };
    context.sync_buffer(staging.buffer);
    encoder.start();
    encoder.transfer("RE-USE upload").copy_buffer_to_buffer(staging.buffer.into(), target.into(), bytes.len() as u64);
    let sync = context.submit(encoder);
    context.wait_for(&sync, 60_000);
    context.destroy_buffer(staging.buffer);
}
fn download(context: &gpu::Context, encoder: &mut gpu::CommandEncoder, source: gpu::BufferPiece, len: usize) -> Vec<u8> {
    let staging = allocate(context, len, gpu::Memory::Shared, "RE-USE download");
    encoder.start();
    encoder.transfer("RE-USE download").copy_buffer_to_buffer(source, staging.buffer.into(), len as u64);
    let sync = context.submit(encoder);
    context.wait_for(&sync, 60_000);
    context.sync_buffer(staging.buffer);
    let bytes = unsafe { std::slice::from_raw_parts(staging.buffer.data(), len) }.to_vec();
    context.destroy_buffer(staging.buffer);
    bytes
}

fn f16_bits(value: f32) -> u16 {
    let x = value.to_bits();
    let sign = ((x >> 16) & 0x8000) as u16;
    let exp = ((x >> 23) & 0xff) as i32;
    let mant = x & 0x7f_ffff;
    if exp == 0xff {
        return sign | 0x7c00 | if mant != 0 { 0x200 } else { 0 };
    }
    let e = exp - 127 + 15;
    if e >= 0x1f {
        return sign | 0x7c00;
    }
    if e <= 0 {
        if e < -10 {
            return sign;
        }
        let m = mant | 0x80_0000;
        let shift = (14 - e) as u32;
        let half = 1 << (shift - 1);
        let rounded = (m + half - 1 + ((m >> shift) & 1)) >> shift;
        return sign | rounded as u16;
    }
    let rounded = mant + 0xfff + ((mant >> 13) & 1);
    let (e, m) = if rounded & 0x80_0000 != 0 { (e + 1, 0) } else { (e, rounded >> 13) };
    if e >= 0x1f {
        return sign | 0x7c00;
    }
    sign | ((e as u16) << 10) | m as u16
}
fn f16_value(bits: u16) -> f32 {
    let sign = if bits & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = ((bits >> 10) & 0x1f) as i32;
    let mant = (bits & 0x3ff) as f32;
    sign * match exp {
        0 => mant * 2f32.powi(-24),
        0x1f => if mant == 0.0 { f32::INFINITY } else { f32::NAN },
        e => (1.0 + mant / 1024.0) * 2f32.powi(e - 15),
    }
}

#[cfg(test)]
#[path = "reuse_audio_tests.rs"]
mod tests;
