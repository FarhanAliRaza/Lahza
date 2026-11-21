// Readies a mesh the GPU generated in its pool for a deformer it also
// runs (`Source::DeformedGenerated`): what `slang_mesh::deform_base` does
// on the CPU. Copies of one point (those whose positions are the same to
// 1/64) are moved along one normal, the unit sum of theirs, so they stay
// together.
//
// `weld_sum`: every point finds its place in a hash table by its position
// (the first point to claim a place stands for the others there) and adds
// its normal. `weld_write`: every point writes its three rows of the mesh
// the deformer reads: [position, u] [that normal, v] [its own normal, 0].
// The sums are atomic (a float behind a compare-and-swap), in no fixed
// order: a normal may differ from run to run in its last bits.

struct WeldPass {
    // Points, the generated mesh's first row, the readied mesh's first
    // row, the table's places − 1 (a power of two of them).
    counts: vec4<u32>,
}

var<uniform> u_weld: WeldPass;
var<storage, read_write> b_rows: array<vec4<f32>>;
// Per place: [the point standing for it + 1 (0: free), the sum's x, y, z].
var<storage, read_write> b_table: array<atomic<u32>>;

// A position to 1/64, rounded half away from zero as the CPU rounds.
fn cell(p: vec3<f32>) -> vec3<i32> {
    let q = p * 64.0;
    return vec3<i32>(sign(q) * floor(abs(q) + vec3<f32>(0.5)));
}

fn hash(c: vec3<i32>) -> u32 {
    var h = bitcast<u32>(c.x) * 0x9E3779B1u;
    h = (h ^ (h >> 15u)) + bitcast<u32>(c.y) * 0x85EBCA77u;
    h = (h ^ (h >> 13u)) + bitcast<u32>(c.z) * 0xC2B2AE3Du;
    return h ^ (h >> 16u);
}

fn position(i: u32) -> vec3<f32> {
    return b_rows[u_weld.counts.y + i * 2u].xyz;
}

// The place of point `i`'s position, claimed for it when `claim` and no
// point has it yet.
fn place(i: u32, claim: bool) -> u32 {
    let c = cell(position(i));
    var slot = hash(c) & u_weld.counts.w;
    // (The table has twice the points' places: a free one is always near.)
    for (var k = 0u; k <= u_weld.counts.w; k++) {
        var owner = atomicLoad(&b_table[slot * 4u]);
        if owner == 0u && claim {
            let r = atomicCompareExchangeWeak(&b_table[slot * 4u], 0u, i + 1u);
            if r.exchanged {
                return slot;
            }
            owner = r.old_value;
            if owner == 0u {
                // (A spurious failure: this place again.)
                continue;
            }
        }
        if owner == 0u {
            return slot;
        }
        if all(cell(position(owner - 1u)) == c) {
            return slot;
        }
        slot = (slot + 1u) & u_weld.counts.w;
    }
    return slot;
}

fn add(at: u32, x: f32) {
    if x == 0.0 {
        return;
    }
    var old = atomicLoad(&b_table[at]);
    loop {
        let r = atomicCompareExchangeWeak(&b_table[at], old, bitcast<u32>(bitcast<f32>(old) + x));
        if r.exchanged {
            break;
        }
        old = r.old_value;
    }
}

@compute @workgroup_size(64)
fn weld_sum(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x;
    if i >= u_weld.counts.x {
        return;
    }
    let n = b_rows[u_weld.counts.y + i * 2u + 1u].xyz;
    let slot = place(i, true);
    add(slot * 4u + 1u, n.x);
    add(slot * 4u + 2u, n.y);
    add(slot * 4u + 3u, n.z);
}

@compute @workgroup_size(64)
fn weld_write(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x;
    if i >= u_weld.counts.x {
        return;
    }
    let a = b_rows[u_weld.counts.y + i * 2u];
    let b = b_rows[u_weld.counts.y + i * 2u + 1u];
    let slot = place(i, false);
    var w = vec3<f32>(bitcast<f32>(atomicLoad(&b_table[slot * 4u + 1u])), bitcast<f32>(atomicLoad(&b_table[slot * 4u + 2u])), bitcast<f32>(atomicLoad(&b_table[slot * 4u + 3u])));
    let l = sqrt(w.x * w.x + w.y * w.y + w.z * w.z);
    if l > 1e-12 {
        w = w / l;
    } else {
        w = vec3<f32>(0.0);
    }
    let at = u_weld.counts.z + i * 3u;
    b_rows[at] = a;
    b_rows[at + 1u] = vec4<f32>(w, b.w);
    b_rows[at + 2u] = vec4<f32>(b.xyz, 0.0);
}
