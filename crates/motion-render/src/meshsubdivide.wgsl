// Splits every triangle of a mesh in the pool into four at its edges'
// midpoints, as `mesh::subdivide` does on the CPU, for a deformer that
// asks for it of a mesh the GPU generated (`Source::DeformedGenerated`).
// From the mesh at `from` (points, then triangles) to the one at `to`:
// its points are the first mesh's (copied as they are), then three a
// triangle, the midpoints of its edges; its triangles four a triangle.
//
// An edge two triangles share has one midpoint in the triangles: that of
// the triangle first to claim the edge in a hash table (`sub_edges`),
// which both look up (`sub_split`). The other triangle's midpoint of that
// edge is the same point, in no triangle.

struct SubPass {
    // The first mesh's points and triangles, its first row, the second's.
    counts: vec4<u32>,
    // The table's places − 1 (a power of two of them).
    more: vec4<u32>,
}

var<uniform> u_sub: SubPass;
var<storage, read_write> b_rows: array<vec4<f32>>;
// Per place: the edge that has it + 1 (0: free); an edge is 3 × its
// triangle + which of its three (0: a–b, 1: b–c, 2: c–a).
var<storage, read_write> b_table: array<atomic<u32>>;

fn triangle(j: u32) -> vec4<u32> {
    return bitcast<vec4<u32>>(b_rows[u_sub.counts.z + u_sub.counts.x * 2u + j]);
}

// An edge's two points, the lower first.
fn ends(edge: u32) -> vec2<u32> {
    let t = triangle(edge / 3u);
    var e = vec2<u32>(t.x, t.y);
    if edge % 3u == 1u {
        e = vec2<u32>(t.y, t.z);
    } else if edge % 3u == 2u {
        e = vec2<u32>(t.z, t.x);
    }
    return vec2<u32>(min(e.x, e.y), max(e.x, e.y));
}

// The edge that stands for `edge`'s two points (itself when `claim` and
// no edge has them yet).
fn owner(edge: u32, claim: bool) -> u32 {
    let e = ends(edge);
    var h = e.x * 0x9E3779B1u;
    h = (h ^ (h >> 15u)) + e.y * 0x85EBCA77u;
    var slot = (h ^ (h >> 13u)) & u_sub.more.x;
    for (var k = 0u; k <= u_sub.more.x; k++) {
        var held = atomicLoad(&b_table[slot]);
        if held == 0u && claim {
            let r = atomicCompareExchangeWeak(&b_table[slot], 0u, edge + 1u);
            if r.exchanged {
                return edge;
            }
            held = r.old_value;
            if held == 0u {
                // (A spurious failure: this place again.)
                continue;
            }
        }
        if held == 0u {
            return edge;
        }
        if all(ends(held - 1u) == e) {
            return held - 1u;
        }
        slot = (slot + 1u) & u_sub.more.x;
    }
    return edge;
}

// Every edge of every triangle: its midpoint, and its claim on the edge.
@compute @workgroup_size(64)
fn sub_edges(@builtin(global_invocation_id) id: vec3<u32>) {
    let j = id.x;
    if j >= u_sub.counts.y {
        return;
    }
    let points = u_sub.counts.x;
    let t = triangle(j);
    for (var c = 0u; c < 3u; c++) {
        let edge = j * 3u + c;
        var a = t[c];
        var b = t[(c + 1u) % 3u];
        // (A triangle naming points the mesh does not have was made
        // empty where it was generated.)
        a = min(a, points - 1u);
        b = min(b, points - 1u);
        let pa = b_rows[u_sub.counts.z + a * 2u];
        let na = b_rows[u_sub.counts.z + a * 2u + 1u];
        let pb = b_rows[u_sub.counts.z + b * 2u];
        let nb = b_rows[u_sub.counts.z + b * 2u + 1u];
        var n = na.xyz + nb.xyz;
        let l = sqrt(n.x * n.x + n.y * n.y + n.z * n.z);
        if l > 1e-12 {
            n = n / l;
        } else {
            n = vec3<f32>(0.0);
        }
        let at = u_sub.counts.w + (points + edge) * 2u;
        b_rows[at] = vec4<f32>((pa.xyz + pb.xyz) / 2.0, (pa.w + pb.w) / 2.0);
        b_rows[at + 1u] = vec4<f32>(n, (na.w + nb.w) / 2.0);
        owner(edge, true);
    }
}

// Every triangle into four, between its points and its edges' midpoints.
@compute @workgroup_size(64)
fn sub_split(@builtin(global_invocation_id) id: vec3<u32>) {
    let j = id.x;
    if j >= u_sub.counts.y {
        return;
    }
    let points = u_sub.counts.x;
    let t = triangle(j);
    let ab = points + owner(j * 3u, false);
    let bc = points + owner(j * 3u + 1u, false);
    let ca = points + owner(j * 3u + 2u, false);
    // The record: segment, part and sides as they were; a new triangle's
    // edges are real where they lie on a real edge of the old one (bit k:
    // the edge opposite point k; the middle triangle's are not).
    let rest = t.w & ~(7u << 19u);
    let e = (t.w >> 19u) & 7u;
    let e_bc = e & 1u;
    let e_ca = (e >> 1u) & 1u;
    let e_ab = (e >> 2u) & 1u;
    let at = u_sub.counts.w + (points + u_sub.counts.y * 3u) * 2u + j * 4u;
    b_rows[at] = bitcast<vec4<f32>>(vec4<u32>(t.x, ab, ca, rest | (((e_ca << 1u) | (e_ab << 2u)) << 19u)));
    b_rows[at + 1u] = bitcast<vec4<f32>>(vec4<u32>(ab, t.y, bc, rest | ((e_bc | (e_ab << 2u)) << 19u)));
    b_rows[at + 2u] = bitcast<vec4<f32>>(vec4<u32>(ca, bc, t.z, rest | ((e_bc | (e_ca << 1u)) << 19u)));
    b_rows[at + 3u] = bitcast<vec4<f32>>(vec4<u32>(ab, bc, ca, rest));
}
