// NVIDIA NemotronLabs RE-USE (SEMamba) inference. Provenance: assets/audio/RE_USE.md.
// Activations are channel-last ([position][channel]); positions are row-major
// (time, frequency). Each pipeline variant reads `In` and writes `Out`
// activations: f16 for the full-resolution encoder on devices with native
// half support, f32 elsewhere (Mamba intermediates exceed the f16 range).
// All arithmetic and accumulation is f32.
alias In = f32;
alias Out = f32;

struct Params {
    m: u32, k: u32, ntiles: u32, epi: u32,
    out_t: u32, out_f: u32, in_t: u32, in_f: u32,
    kt: u32, kf: u32, dil_t: u32, dil_f: u32,
    st: u32, sf: u32, pt: u32, pf: u32,
    cin: u32, nbuf: u32, in_stride: u32, a_off: u32,
    a_group: u32, a_mode: u32, w_off: u32, w_stride: u32,
    bias_off: u32, group_n: u32, out_stride: u32, out_off: u32,
    rt: u32, rf: u32, sub: u32, part_off: u32,
    norm_off: u32, alpha_off: u32, fin_off: u32, fin_n: u32,
    crop_t: u32, crop_f: u32, seq_len: u32, seq_stride: u32,
    step_stride: u32, n_seq: u32, mamba_off: u32, gamma_off: u32,
    beta_off: u32, count: u32, x_groups: u32, aux_stride: u32,
    tile_off: u32,
}

var<storage, read> p: Params;
var<storage, read> b_w: array<f32>;
var<storage, read> b_a0: array<In>;
var<storage, read> b_a1: array<In>;
var<storage, read> b_a2: array<In>;
var<storage, read> b_a3: array<In>;
var<storage, read_write> b_out: array<Out>;
var<storage, read> b_aux: array<Out>;
var<storage, read_write> b_part: array<vec4<f32>>;
var<storage, read_write> b_norm: array<f32>;
var<storage, read> b_wav: array<f32>;
var<storage, read_write> b_fout: array<f32>;

const NONE: u32 = 0xffffffffu;
const NFFT: u32 = 1920u;
const HALF: u32 = 960u;
const HOP: u32 = 240u;
const LOG2E: f32 = 1.4426950408889634;

// Epilogues of `gemm`.
const EPI_STATS: u32 = 0u; // + bias, store (optionally subsampled rows), InstanceNorm partials
const EPI_FINAL: u32 = 1u; // + bias, InstanceNorm + PReLU, 64→1 (or 2 → atan2) projection
const EPI_PLAIN: u32 = 2u;
const EPI_GATE: u32 = 3u; // out *= silu(acc)
const EPI_RESID: u32 = 4u; // acc + aux
const EPI_LN: u32 = 5u; // out += LayerNorm(acc + bias)

fn log1p_(x: f32) -> f32 {
    let u = 1.0 + x;
    if u == 1.0 { return x; }
    return log(u) * (x / (u - 1.0));
}
fn expm1_(x: f32) -> f32 {
    if abs(x) < 1e-5 { return x + 0.5 * x * x; }
    let u = exp(x);
    if u == 1.0 { return x; }
    let um1 = u - 1.0;
    if um1 == -1.0 { return -1.0; }
    if x > 80.0 { return u; }
    return um1 * (x / log(u));
}
fn silu(x: f32) -> f32 { return x / (1.0 + exp(-x)); }
fn softplus(x: f32) -> f32 { return select(x, log1p_(exp(x)), x <= 20.0); }
fn angle(y: f32, x: f32) -> f32 {
    if x == 0.0 && y == 0.0 { return 0.0; }
    return atan2(y, x);
}

fn read_a(b: u32, i: u32) -> f32 {
    switch b {
        case 0u: { return f32(b_a0[i]); }
        case 1u: { return f32(b_a1[i]); }
        case 2u: { return f32(b_a2[i]); }
        default: { return f32(b_a3[i]); }
    }
}

// Mamba input branch u = silu(causal depthwise conv(x)) for one (position,
// channel). The backward direction is the same conv over reversed time.
fn mamba_u(row: u32, col: u32, dir: u32) -> f32 {
    let mo = p.mamba_off;
    let l = select(row % p.out_f, row / p.out_f, p.step_stride != 1u);
    var acc = b_w[mo + 2048u + col];
    for (var j = 0u; j < 4u; j++) {
        let back = 3u - j;
        var ok: bool; var pos: u32;
        if dir == 0u {
            ok = l >= back; pos = row - back * p.step_stride;
        } else {
            ok = l + back < p.seq_len; pos = row + back * p.step_stride;
        }
        if ok { acc += b_w[mo + col * 4u + j] * f32(b_a0[pos * p.in_stride + col]); }
    }
    return silu(acc);
}

var<workgroup> sa: array<vec4<f32>, 256>; // [k 16][row/4 16]
var<workgroup> sb: array<vec4<f32>, 256>; // [k 16][col/4 16]
var<workgroup> tile: array<f32, 4160>; // [64][65]
var<workgroup> rowv: array<vec2<f32>, 64>;

struct TileLoad { a: vec4<f32>, w: vec4<f32> }
// This invocation's share of one 16-deep K step: A for 4 rows at column
// tid % 16, and 4 consecutive weights of row tid / 16.
fn gemm_load(k0: u32, tid: u32, nt: u32, m0: u32, rbt: array<i32, 4>, rbf: array<i32, 4>, rok: array<bool, 4>) -> TileLoad {
    var r: TileLoad;
    let k = k0 + tid % 16u;
    let kin = k < p.k;
    let lrow = tid / 16u;
    let a_base = p.a_off + nt * p.a_group;
    if p.a_mode == 0u {
        let cin_total = p.cin * p.nbuf;
        let tap = k / cin_total;
        let c = k % cin_total;
        let b = c / p.cin;
        let ic = c % p.cin;
        let dt = i32((tap / p.kf) * p.dil_t);
        let df = i32((tap % p.kf) * p.dil_f);
        for (var i = 0u; i < 4u; i++) {
            let it = rbt[i] + dt;
            let jf = rbf[i] + df;
            if kin && rok[i] && it >= 0 && it < i32(p.in_t) && jf >= 0 && jf < i32(p.in_f) {
                r.a[i] = read_a(b, (u32(it) * p.in_f + u32(jf)) * p.in_stride + a_base + ic);
            }
        }
    } else {
        for (var i = 0u; i < 4u; i++) {
            if kin && rok[i] { r.a[i] = mamba_u(m0 + lrow + 16u * i, a_base + k, nt); }
        }
    }
    let kb = k0 + lrow;
    let n4 = (tid % 16u) * 4u;
    if kb < p.k {
        let row = p.w_off + kb * p.w_stride + nt * p.group_n;
        for (var j = 0u; j < 4u; j++) {
            if n4 + j < p.group_n { r.w[j] = b_w[row + n4 + j]; }
        }
    }
    return r;
}

// out[m][n] = Σ_k A(m,k) W[k][n] over a 64×64 tile. A is an implicit
// convolution gather (a 1×1 kernel makes it a linear layer) over up to four
// concatenated channel-last inputs, or the Mamba u branch. The next K step is
// loaded into registers while the current one is multiplied.
@compute @workgroup_size(256)
fn gemm(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) tid: u32) {
    // Long GEMMs are split into dispatches of p.count row tiles from tile_off.
    let local = wg.z * p.x_groups + wg.x;
    let mt = p.tile_off + local;
    let nt = wg.y;
    if local >= p.count || mt * 64u >= p.m { return; }
    let m0 = mt * 64u;
    let tx = tid % 16u;
    let ty = tid / 16u;
    let kk = tid % 16u;
    let lrow = tid / 16u;
    var rbt: array<i32, 4>;
    var rbf: array<i32, 4>;
    var rok: array<bool, 4>;
    for (var i = 0u; i < 4u; i++) {
        let m = m0 + lrow + 16u * i;
        rok[i] = m < p.m;
        rbt[i] = i32((m / p.out_f) * p.st) - i32(p.pt);
        rbf[i] = i32((m % p.out_f) * p.sf) - i32(p.pf);
    }
    var acc: array<vec4<f32>, 4>;
    var next = gemm_load(0u, tid, nt, m0, rbt, rbf, rok);
    for (var k0 = 0u; k0 < p.k; k0 += 16u) {
        for (var i = 0u; i < 4u; i++) {
            let r = lrow + 16u * i;
            sa[kk * 16u + r / 4u][r % 4u] = next.a[i];
        }
        sb[tid] = next.w;
        workgroupBarrier();
        if k0 + 16u < p.k { next = gemm_load(k0 + 16u, tid, nt, m0, rbt, rbf, rok); }
        for (var q = 0u; q < 16u; q++) {
            let a = sa[q * 16u + ty];
            let w = sb[q * 16u + tx];
            acc[0] += a.x * w;
            acc[1] += a.y * w;
            acc[2] += a.z * w;
            acc[3] += a.w * w;
        }
        workgroupBarrier();
    }

    let valid_rows = min(64u, p.m - m0);
    switch p.epi {
        case 0u, 1u, 5u: {
            for (var i = 0u; i < 4u; i++) {
                for (var j = 0u; j < 4u; j++) {
                    let c = tx * 4u + j;
                    var v = acc[i][j];
                    if p.bias_off != NONE { v += b_w[p.bias_off + nt * p.group_n + c]; }
                    if p.epi == EPI_FINAL {
                        v = (v - b_norm[p.norm_off + c]) * b_norm[p.norm_off + 64u + c] + b_w[p.beta_off + c];
                        v = select(b_w[p.alpha_off + c] * v, v, v >= 0.0);
                    }
                    tile[(ty * 4u + i) * 65u + c] = select(0.0, v, ty * 4u + i < valid_rows);
                }
            }
            workgroupBarrier();
            if p.epi == EPI_STATS {
                if tid < 64u {
                    var mean = 0.0;
                    for (var r = 0u; r < valid_rows; r++) { mean += tile[r * 65u + tid]; }
                    mean /= f32(valid_rows);
                    var m2 = 0.0;
                    for (var r = 0u; r < valid_rows; r++) {
                        let d = tile[r * 65u + tid] - mean;
                        m2 += d * d;
                    }
                    b_part[p.part_off + (mt * p.ntiles + nt) * 64u + tid] = vec4<f32>(f32(valid_rows), mean, m2, 0.0);
                }
                if p.sub != 0u {
                    for (var i = 0u; i < 4u; i++) {
                        let m = m0 + ty * 4u + i;
                        if m >= p.m { continue; }
                        let pt_ = (m / p.out_f) * p.rt + select(0u, nt, p.rt > 1u);
                        let pf_ = (m % p.out_f) * p.rf + select(0u, nt, p.rf > 1u);
                        if pt_ % p.sub != 0u { continue; }
                        let base = ((pt_ / p.sub) * p.out_f * p.rf + pf_) * 64u + tx * 4u;
                        for (var j = 0u; j < 4u; j++) {
                            b_out[base + j] = Out(tile[(ty * 4u + i) * 65u + tx * 4u + j]);
                        }
                    }
                }
            } else if p.epi == EPI_FINAL {
                if tid < valid_rows {
                    var d0 = b_w[p.fin_off + 64u];
                    var d1 = b_w[p.fin_off + 129u];
                    for (var c = 0u; c < 64u; c++) {
                        let v = tile[tid * 65u + c];
                        d0 += v * b_w[p.fin_off + c];
                        if p.fin_n == 2u { d1 += v * b_w[p.fin_off + 65u + c]; }
                    }
                    let m = m0 + tid;
                    let pt_ = (m / p.out_f) * p.rt + nt;
                    let pf_ = m % p.out_f;
                    if pt_ < p.crop_t && pf_ < p.crop_f {
                        b_fout[p.out_off + pt_ * p.crop_f + pf_] = select(d0, angle(d1, d0), p.fin_n == 2u);
                    }
                }
            } else {
                if tid < 64u {
                    var mean = 0.0;
                    for (var c = 0u; c < 64u; c++) { mean += tile[tid * 65u + c]; }
                    mean /= 64.0;
                    var var_ = 0.0;
                    for (var c = 0u; c < 64u; c++) {
                        let d = tile[tid * 65u + c] - mean;
                        var_ += d * d;
                    }
                    rowv[tid] = vec2<f32>(mean, 1.0 / sqrt(var_ / 64.0 + 1e-5));
                }
                workgroupBarrier();
                for (var i = 0u; i < 4u; i++) {
                    let r = ty * 4u + i;
                    if r >= valid_rows { continue; }
                    for (var j = 0u; j < 4u; j++) {
                        let c = tx * 4u + j;
                        let y = (tile[r * 65u + c] - rowv[r].x) * rowv[r].y * b_w[p.gamma_off + c] + b_w[p.beta_off + c];
                        let idx = (m0 + r) * 64u + c;
                        b_out[idx] = Out(y + f32(b_out[idx]));
                    }
                }
            }
        }
        default: {
            for (var i = 0u; i < 4u; i++) {
                let m = m0 + ty * 4u + i;
                if m >= p.m { continue; }
                for (var j = 0u; j < 4u; j++) {
                    let c = tx * 4u + j;
                    if c >= p.group_n { continue; }
                    let gc = nt * p.group_n + c;
                    let idx = m * p.out_stride + p.out_off + gc;
                    var v = acc[i][j];
                    if p.bias_off != NONE { v += b_w[p.bias_off + gc]; }
                    if p.epi == EPI_GATE {
                        v = f32(b_out[idx]) * silu(v);
                    } else if p.epi == EPI_RESID {
                        v += f32(b_aux[m * p.aux_stride + gc % 64u]);
                    }
                    b_out[idx] = Out(v);
                }
            }
        }
    }
}

var<workgroup> red: array<vec3<f32>, 256>;
fn chan(a: vec3<f32>, b: vec3<f32>) -> vec3<f32> {
    let n = a.x + b.x;
    if n == 0.0 { return a; }
    let d = b.y - a.y;
    return vec3<f32>(n, a.y + d * (b.x / n), a.z + b.z + d * d * (a.x * b.x / n));
}
// InstanceNorm statistics for channel wg.x, combined from per-tile partials.
@compute @workgroup_size(256)
fn norm_stats(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) tid: u32) {
    let c = wg.x;
    var s = vec3<f32>(0.0);
    for (var e = tid; e < p.count; e += 256u) {
        let q = b_part[p.part_off + e * 64u + c];
        s = chan(s, q.xyz);
    }
    red[tid] = s;
    workgroupBarrier();
    for (var w = 128u; w > 0u; w >>= 1u) {
        if tid < w { red[tid] = chan(red[tid], red[tid + w]); }
        workgroupBarrier();
    }
    if tid == 0u {
        let r = red[0];
        b_norm[p.norm_off + c] = r.y;
        b_norm[p.norm_off + 64u + c] = b_w[p.gamma_off + c] / sqrt(r.z / r.x + 1e-5);
    }
}

// In-place InstanceNorm affine + PReLU over p.m positions × 64 channels.
@compute @workgroup_size(256)
fn norm_apply(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) tid: u32) {
    let i = (wg.z * p.x_groups + wg.x) * 256u + tid;
    if i >= p.m * 64u { return; }
    let c = i % 64u;
    var v = (f32(b_out[i]) - b_norm[p.norm_off + c]) * b_norm[p.norm_off + 64u + c] + b_w[p.beta_off + c];
    v = select(b_w[p.alpha_off + c] * v, v, v >= 0.0);
    b_out[i] = Out(v);
}

var<workgroup> frame: array<f32, 1920>;
var<workgroup> twiddle: array<vec2<f32>, 1920>;
var<workgroup> spec: array<vec2<f32>, 961>;
var<workgroup> zeros: atomic<u32>;

// Centered, reflect-padded Hann STFT of frame wg.x (n_fft 1920, hop 240),
// written as the zero-padded [Tp][Fp][log1p |X|, ∠X] model input.
@compute @workgroup_size(256)
fn stft(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) tid: u32) {
    let t = wg.z * p.x_groups + wg.x;
    if t >= p.out_t { return; }
    let n_samples = i32(p.count);
    for (var n = tid; n < NFFT; n += 256u) {
        twiddle[n] = vec2<f32>(b_w[p.w_off + 2u * n], b_w[p.w_off + 2u * n + 1u]);
        if t < p.in_t {
            var i = i32(t * HOP + n) - i32(HALF);
            if i < 0 { i = -i; }
            if i >= n_samples { i = 2 * (n_samples - 1) - i; }
            frame[n] = b_wav[u32(i)] * b_w[p.w_off + 2u * NFFT + n];
        }
    }
    workgroupBarrier();
    for (var k = tid; k < p.out_f; k += 256u) {
        var mag = 0.0;
        var pha = 0.0;
        if t < p.in_t && k < p.in_f {
            var re = 0.0;
            var im = 0.0;
            var idx = 0u;
            for (var n = 0u; n < NFFT; n++) {
                let w = twiddle[idx];
                re += frame[n] * w.x;
                im -= frame[n] * w.y;
                idx += k;
                if idx >= NFFT { idx -= NFFT; }
            }
            mag = log1p_(sqrt(re * re + im * im));
            pha = angle(im, re);
        }
        let o = (t * p.out_f + k) * 2u;
        b_out[o] = Out(mag);
        b_out[o + 1u] = Out(pha);
    }
}

// Inverse real DFT of frame wg.x times the synthesis window. Frames where more
// than half the bins have zero linear magnitude are silenced, as in inference.py.
@compute @workgroup_size(256)
fn istft_frames(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) tid: u32) {
    let t = wg.z * p.x_groups + wg.x;
    if t >= p.in_t { return; }
    let f = p.in_f;
    if tid == 0u { atomicStore(&zeros, 0u); }
    workgroupBarrier();
    for (var k = tid; k < f; k += 256u) {
        if !(b_wav[t * f + k] > 0.0) { atomicAdd(&zeros, 1u); }
    }
    workgroupBarrier();
    let silent = atomicLoad(&zeros) * 2u > f;
    for (var k = tid; k < f; k += 256u) {
        let amp = select(b_wav[t * f + k], 0.0, silent);
        let mag = expm1_(max(amp, 0.0));
        let ph = b_wav[p.out_off + t * f + k];
        spec[k] = vec2<f32>(mag * cos(ph), mag * sin(ph));
    }
    for (var n = tid; n < NFFT; n += 256u) {
        twiddle[n] = vec2<f32>(b_w[p.w_off + 2u * n], b_w[p.w_off + 2u * n + 1u]);
    }
    workgroupBarrier();
    for (var n = tid; n < NFFT; n += 256u) {
        var acc = 0.0;
        var idx = n;
        for (var k = 1u; k < HALF; k++) {
            let w = twiddle[idx];
            acc += spec[k].x * w.x - spec[k].y * w.y;
            idx += n;
            if idx >= NFFT { idx -= NFFT; }
        }
        let nyquist = select(spec[HALF].x, -spec[HALF].x, (n & 1u) == 1u);
        let x = (spec[0].x + nyquist + 2.0 * acc) / f32(NFFT);
        b_fout[t * NFFT + n] = x * b_w[p.w_off + 2u * NFFT + n];
    }
}

// Overlap-add with window-envelope normalization, trimmed to the input length.
@compute @workgroup_size(256)
fn istft_ola(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) tid: u32) {
    let i = (wg.z * p.x_groups + wg.x) * 256u + tid;
    if i >= p.count { return; }
    let frames = p.in_t;
    if i >= HOP * (frames - 1u) {
        b_fout[p.out_off + i] = 1e-8;
        return;
    }
    let q = i + HALF;
    let hi = min(q / HOP, frames - 1u);
    let lo = select(0u, (q - (NFFT - 1u) + HOP - 1u) / HOP, q >= NFFT - 1u);
    var sum = 0.0;
    var env = 0.0;
    for (var t = lo; t <= hi; t++) {
        let n = q - t * HOP;
        let w = b_w[p.w_off + 2u * NFFT + n];
        sum += b_wav[t * NFFT + n];
        env += w * w;
    }
    b_fout[p.out_off + i] = sum / env;
}

var<workgroup> xi_sh: array<array<f32, 19>, 16>;
var<workgroup> u_sh: array<array<f32, 16>, 16>;
var<workgroup> d_sh: array<array<f32, 16>, 16>;
var<workgroup> dt_sh: array<vec4<f32>, 16>;
var<workgroup> bc_sh: array<array<vec2<f32>, 16>, 16>; // [step][state] (B, C)
var<workgroup> contrib: array<array<array<f32, 17>, 16>, 16>; // [channel][step][state]

struct ScanLoad { x: f32, b: vec2<f32>, dt: f32 }
fn scan_pos(seq: u32, step: u32, dir: u32) -> u32 {
    let l = select(p.seq_len - 1u - step, step, dir == 0u);
    return seq * p.seq_stride + l * p.step_stride;
}
// This invocation's share of a 16-step chunk: x for (channel, step s), and
// one B/C pair and one dt rank value shared by the workgroup.
fn scan_load(seq: u32, dir: u32, col: u32, c0: u32, tid: u32) -> ScanLoad {
    var r: ScanLoad;
    let s = tid % 16u;
    if c0 + s < p.seq_len {
        r.x = f32(b_out[scan_pos(seq, c0 + s, dir) * 512u + col]);
    }
    let step = c0 + tid / 16u;
    if step < p.seq_len {
        let o = scan_pos(seq, step, dir) * 72u + dir * 36u;
        r.b = vec2<f32>(f32(b_a1[o + 4u + s]), f32(b_a1[o + 20u + s]));
        if s < 4u { r.dt = f32(b_a1[o + s]); }
    }
    return r;
}

// Selective scan, one invocation per (sequence, direction, channel, state)
// with the state in a register. Workgroups hold 16 channels × 16 states,
// share each step's B/C/dt loads, prefetch the next 16-step chunk while
// computing the current one, and reduce over states once per chunk.
// Y overwrites XI in place: each x is read once, before its step's output.
@compute @workgroup_size(256)
fn scan(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) tid: u32) {
    let seq = wg.x;
    let dir = wg.y / 16u;
    let lc = tid / 16u;
    let s = tid % 16u;
    let ch = (wg.y % 16u) * 16u + lc;
    let col = dir * 256u + ch;
    let mo = p.mamba_off;
    let a2 = b_w[mo + 5120u + col * 16u + s];
    let dd = b_w[mo + 13312u + col];
    let cw = vec4<f32>(b_w[mo + col * 4u], b_w[mo + col * 4u + 1u], b_w[mo + col * 4u + 2u], b_w[mo + col * 4u + 3u]);
    let cb = b_w[mo + 2048u + col];
    let dw = vec4<f32>(b_w[mo + 2560u + col * 4u], b_w[mo + 2560u + col * 4u + 1u], b_w[mo + 2560u + col * 4u + 2u], b_w[mo + 2560u + col * 4u + 3u]);
    let db = b_w[mo + 4608u + col];
    let len = p.seq_len;
    if s < 3u { xi_sh[lc][s] = 0.0; }
    var h = 0.0;
    var next = scan_load(seq, dir, col, 0u, tid);
    for (var c0 = 0u; c0 < len; c0 += 16u) {
        xi_sh[lc][3u + s] = next.x;
        bc_sh[lc][s] = next.b;
        if s < 4u { dt_sh[lc][s] = next.dt; }
        workgroupBarrier();
        if c0 + 16u < len { next = scan_load(seq, dir, col, c0 + 16u, tid); }
        let valid = c0 + s < len;
        let x = xi_sh[lc][3u + s];
        let u = silu(cb + cw.x * xi_sh[lc][s] + cw.y * xi_sh[lc][s + 1u] + cw.z * xi_sh[lc][s + 2u] + cw.w * x);
        u_sh[lc][s] = select(0.0, u, valid);
        d_sh[lc][s] = select(0.0, softplus(dot(dw, dt_sh[s]) + db), valid);
        workgroupBarrier();
        for (var i = 0u; i < 16u; i++) {
            let d = d_sh[lc][i];
            let bc = bc_sh[i][s];
            h = exp2(d * a2) * h + d * bc.x * u_sh[lc][i];
            contrib[lc][i][s] = h * bc.y;
        }
        workgroupBarrier();
        if valid {
            var y = 0.0;
            for (var q = 0u; q < 16u; q++) { y += contrib[lc][s][q]; }
            b_out[scan_pos(seq, c0 + s, dir) * 512u + col] = Out(y + dd * u_sh[lc][s]);
        }
        if s < 3u { xi_sh[lc][s] = xi_sh[lc][16u + s]; }
        workgroupBarrier();
    }
}
