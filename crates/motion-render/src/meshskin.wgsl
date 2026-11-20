// Skins a mesh in the mesh pool (`Source::Skinned`): what
// `lahza_motion_core::skin::skin` does on the CPU. The mesh before its
// pose (`SkinBase`) has four rows a point: [position, u] [normal, v]
// [its joints] [their weights]; the pose is three rows a joint, the rows
// of J · B (`skin::matrices`). Every point writes its two rows of the
// skinned mesh: [position, u] [normal, v]. The triangles are the bind
// mesh's, copied.

struct SkinPass {
    // Points, the bind mesh's first row, the skinned mesh's first row, the
    // pose's first row.
    counts: vec4<u32>,
    // Joints the pose has.
    more: vec4<u32>,
}

var<uniform> u_skin: SkinPass;
var<storage, read_write> b_rows: array<vec4<f32>>;

@compute @workgroup_size(64)
fn skin_points(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x;
    if i >= u_skin.counts.x {
        return;
    }
    let src = u_skin.counts.y + i * 4u;
    let a = b_rows[src];
    let b = b_rows[src + 1u];
    let joints = bitcast<vec4<u32>>(b_rows[src + 2u]);
    let weights = b_rows[src + 3u];
    let p = vec4<f32>(a.xyz, 1.0);
    var q = vec3<f32>(0.0);
    var d = vec3<f32>(0.0);
    for (var k = 0u; k < 4u; k++) {
        let w = weights[k];
        if w == 0.0 || joints[k] >= u_skin.more.x {
            continue;
        }
        let m = u_skin.counts.w + joints[k] * 3u;
        let r0 = b_rows[m];
        let r1 = b_rows[m + 1u];
        let r2 = b_rows[m + 2u];
        q += w * vec3<f32>(dot(r0, p), dot(r1, p), dot(r2, p));
        d += w * vec3<f32>(dot(r0.xyz, b.xyz), dot(r1.xyz, b.xyz), dot(r2.xyz, b.xyz));
    }
    let l = length(d);
    var n = b.xyz;
    if l > 0.0 {
        n = d / l;
    }
    let dst = u_skin.counts.z + i * 2u;
    b_rows[dst] = vec4<f32>(q, a.w);
    b_rows[dst + 1u] = vec4<f32>(n, b.w);
}
