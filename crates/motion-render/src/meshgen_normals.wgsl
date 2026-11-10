// Smooth normals for a mesh the project's Slang generated in the mesh
// pool without `<function>_normal` (`mesh_gen.rs`): every triangle adds
// its face normal (its area's weight) to its three points, as
// `mesh::face_normals` does on the CPU. `gen_points` (meshgen.wgsl) then
// makes the sums unit normals.
//
// Triangles run in parallel, so the sums are atomic: the pool as words,
// each sum a float behind a compare-and-swap. The order of a point's
// additions is not fixed, so a normal may differ from run to run in its
// last bits.

struct GenPass {
    counts: vec4<u32>,
    more: vec4<u32>,
    lo: vec4<f32>,
    hi: vec4<f32>,
}

var<uniform> u_gen: GenPass;
var<storage, read_write> b_words: array<atomic<u32>>;

fn number(x: f32) -> bool {
    return (bitcast<u32>(x) & 0x7f800000u) != 0x7f800000u;
}

fn word(at: u32) -> f32 {
    return bitcast<f32>(atomicLoad(&b_words[at]));
}

fn position(row: u32) -> vec3<f32> {
    return vec3<f32>(word(row * 4u), word(row * 4u + 1u), word(row * 4u + 2u));
}

fn add(at: u32, x: f32) {
    if x == 0.0 {
        return;
    }
    var old = atomicLoad(&b_words[at]);
    loop {
        let r = atomicCompareExchangeWeak(&b_words[at], old, bitcast<u32>(bitcast<f32>(old) + x));
        if r.exchanged {
            break;
        }
        old = r.old_value;
    }
}

@compute @workgroup_size(64)
fn gen_face_normals(@builtin(global_invocation_id) id: vec3<u32>) {
    let j = id.x;
    if j >= u_gen.counts.y {
        return;
    }
    let points = u_gen.counts.x;
    let base = u_gen.counts.z;
    let at = (base + points * 2u + j) * 4u;
    let k = vec3<u32>(atomicLoad(&b_words[at]), atomicLoad(&b_words[at + 1u]), atomicLoad(&b_words[at + 2u]));
    if k.x >= points || k.y >= points || k.z >= points {
        return;
    }
    let p0 = position(base + k.x * 2u);
    let n = cross(position(base + k.y * 2u) - p0, position(base + k.z * 2u) - p0);
    // A point that is not a number is reported as itself (`gen_points`),
    // not through its neighbours' normals.
    if !number(n.x) || !number(n.y) || !number(n.z) {
        return;
    }
    for (var c = 0u; c < 3u; c++) {
        let row = (base + k[c] * 2u + 1u) * 4u;
        add(row, n.x);
        add(row + 1u, n.y);
        add(row + 2u, n.z);
    }
}
