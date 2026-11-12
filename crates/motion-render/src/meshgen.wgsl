// Finishes a mesh the project's Slang generated in the mesh pool
// (`mesh_gen.rs`): what `SlangShape::generate` and `Pooled::write` do to a
// mesh generated on the CPU. The author's kernel left, per point,
// [position, u] [normal, v] and, per triangle, [point ids, segment], as
// the functions returned them.
//
// `gen_points`: unit normals (the faces' sum where the author gives none,
// `meshgen_normals.wgsl`), planar UVs where the author gives none.
// `gen_triangles`: the triangle's record as the raster reads it (segment,
// part, edges, sides). Values that are not numbers and triangles naming
// points the mesh does not have are counted in `b_status` (the render
// reports them) and drawn as nothing.

struct GenPass {
    // Points, triangles, the mesh's first row in the pool, flags.
    counts: vec4<u32>,
    // The triangle record's fixed bits (part, edges, two-sided), the
    // mesh's slot in `b_status`.
    more: vec4<u32>,
    // The layer's `bounds` (planar UVs spread over them).
    lo: vec4<f32>,
    hi: vec4<f32>,
}

var<uniform> u_gen: GenPass;
var<storage, read_write> b_rows: array<vec4<f32>>;
// Per mesh: [bad points, bad triangles, ~(first bad point), ~(first bad
// triangle)] (complemented so that 0 is "none" and max keeps the first).
var<storage, read_write> b_status: array<atomic<u32>>;

const HAS_NORMALS: u32 = 1u;
const HAS_UVS: u32 = 2u;
const TWO_SIDED: u32 = 4u;

fn number(x: f32) -> bool {
    return (bitcast<u32>(x) & 0x7f800000u) != 0x7f800000u;
}

fn numbers(v: vec3<f32>) -> bool {
    return number(v.x) && number(v.y) && number(v.z);
}

fn fault(kind: u32, index: u32) {
    let slot = u_gen.more.y * 4u;
    atomicAdd(&b_status[slot + kind], 1u);
    atomicMax(&b_status[slot + 2u + kind], ~index);
}

@compute @workgroup_size(64)
fn gen_points(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x;
    if i >= u_gen.counts.x {
        return;
    }
    let flags = u_gen.counts.w;
    let at = u_gen.counts.z + i * 2u;
    var a = b_rows[at];
    var b = b_rows[at + 1u];
    // (Summed face normals of points that are not numbers are not numbers
    // either: the point is what is wrong.)
    var ok = numbers(a.xyz) && numbers(b.xyz);
    if (flags & HAS_UVS) != 0u {
        ok = ok && number(a.w) && number(b.w);
    }
    if !ok {
        fault(0u, i);
        a = vec4<f32>(0.0);
        b = vec4<f32>(0.0);
    }
    var n = b.xyz;
    let l = sqrt(n.x * n.x + n.y * n.y + n.z * n.z);
    if l > 1e-12 {
        n = n / l;
    } else {
        n = vec3<f32>(0.0);
    }
    var uv = vec2<f32>(a.w, b.w);
    if (flags & HAS_UVS) == 0u {
        // Planar, by the axis the point faces.
        let q = (a.xyz - u_gen.lo.xyz) / max(u_gen.hi.xyz - u_gen.lo.xyz, vec3<f32>(1e-6));
        let f = abs(n);
        if f.z >= f.x && f.z >= f.y {
            uv = q.xy;
        } else if f.x >= f.y {
            uv = q.zy;
        } else {
            uv = q.xz;
        }
    }
    b_rows[at] = vec4<f32>(a.xyz, uv.x);
    b_rows[at + 1u] = vec4<f32>(n, uv.y);
}

@compute @workgroup_size(64)
fn gen_triangles(@builtin(global_invocation_id) id: vec3<u32>) {
    let j = id.x;
    if j >= u_gen.counts.y {
        return;
    }
    let points = u_gen.counts.x;
    let base = u_gen.counts.z;
    let at = base + points * 2u + j;
    let rec = b_rows[at];
    let k = vec3<u32>(bitcast<u32>(rec.x), bitcast<u32>(rec.y), bitcast<u32>(rec.z));
    var packed = (bitcast<u32>(rec.w) & 0xffffu) | u_gen.more.x;
    if k.x >= points || k.y >= points || k.z >= points {
        fault(1u, j);
        // No area: nothing draws it.
        b_rows[at] = vec4<f32>(0.0, 0.0, 0.0, bitcast<f32>(packed));
        return;
    }
    if (u_gen.counts.w & TWO_SIDED) == 0u {
        // Which winding faces out: the side its points' normals are on.
        let p0 = b_rows[base + k.x * 2u].xyz;
        let p1 = b_rows[base + k.y * 2u].xyz;
        let p2 = b_rows[base + k.z * 2u].xyz;
        let n = b_rows[base + k.x * 2u + 1u].xyz + b_rows[base + k.y * 2u + 1u].xyz + b_rows[base + k.z * 2u + 1u].xyz;
        if dot(cross(p1 - p0, p2 - p0), n) >= 0.0 {
            packed |= 1u << 23u;
        } else {
            packed |= 2u << 23u;
        }
    }
    b_rows[at].w = bitcast<f32>(packed);
}

// A mesh a deformer moved (`Source::Deformed`): the author's kernel left
// its points [position, u] [0, v], `gen_face_normals` summed the moved
// faces into the normals, and its triangles are the records of the mesh it
// was moved from. `more.z` is that mesh's first row: three rows a point,
// the third its own normal before the move.

// Unit normals on the side they were on before the move (as
// `mesh::face_normals` keeps them); a position that is not a number is
// counted and drawn at the origin.
@compute @workgroup_size(64)
fn deform_points(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x;
    if i >= u_gen.counts.x {
        return;
    }
    let at = u_gen.counts.z + i * 2u;
    var a = b_rows[at];
    let b = b_rows[at + 1u];
    if !numbers(a.xyz) {
        fault(0u, i);
        b_rows[at] = vec4<f32>(0.0, 0.0, 0.0, a.w);
        b_rows[at + 1u] = vec4<f32>(0.0, 0.0, 0.0, b.w);
        return;
    }
    var n = b.xyz;
    let l = sqrt(n.x * n.x + n.y * n.y + n.z * n.z);
    if l > 1e-12 && number(l) {
        n = n / l;
    } else {
        n = vec3<f32>(0.0);
    }
    let before = b_rows[u_gen.more.z + i * 3u + 2u].xyz;
    if dot(n, before) < 0.0 {
        n = -n;
    }
    b_rows[at + 1u] = vec4<f32>(n, b.w);
}

// Which winding faces out now, from the moved points' normals; the rest
// of the record stays as it was. Where a point of the mesh moved to a
// value that is not a number (`deform_points` counted it), the mesh is
// refused whole, as the CPU refuses it: every triangle is made empty.
@compute @workgroup_size(64)
fn deform_triangles(@builtin(global_invocation_id) id: vec3<u32>) {
    let j = id.x;
    if j >= u_gen.counts.y {
        return;
    }
    let points = u_gen.counts.x;
    let base = u_gen.counts.z;
    let at = base + points * 2u + j;
    let rec = b_rows[at];
    let k = vec3<u32>(bitcast<u32>(rec.x), bitcast<u32>(rec.y), bitcast<u32>(rec.z));
    var packed = bitcast<u32>(rec.w) & ~(3u << 23u);
    if atomicLoad(&b_status[u_gen.more.y * 4u]) != 0u {
        b_rows[at] = vec4<f32>(0.0, 0.0, 0.0, bitcast<f32>(packed));
        return;
    }
    if ((packed >> 22u) & 1u) == 0u && k.x < points && k.y < points && k.z < points {
        let p0 = b_rows[base + k.x * 2u].xyz;
        let p1 = b_rows[base + k.y * 2u].xyz;
        let p2 = b_rows[base + k.z * 2u].xyz;
        let n = b_rows[base + k.x * 2u + 1u].xyz + b_rows[base + k.y * 2u + 1u].xyz + b_rows[base + k.z * 2u + 1u].xyz;
        if dot(cross(p1 - p0, p2 - p0), n) >= 0.0 {
            packed |= 1u << 23u;
        } else {
            packed |= 2u << 23u;
        }
    }
    b_rows[at].w = bitcast<f32>(packed);
}
