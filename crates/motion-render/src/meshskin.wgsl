// Skins a mesh in the mesh pool (`Source::Skinned`): what
// `lahza_motion_core::skin::skin` does on the CPU. The mesh before its
// pose (`SkinBase`) has four rows a point: [position, u] [normal, v]
// [its joints] [their weights], or six with eight joints a point: [four
// more joints] [their weights]. The pose (`skin::Posed`) is three rows a
// joint, the rows of J · B; or five for dual quaternions: [the real
// quaternion] [the dual] and the three rows of what is left of J · B (its
// scale). Every point writes its two rows of the skinned mesh:
// [position, u] [normal, v]. The triangles are the bind mesh's, copied.

struct SkinPass {
    // Points, the bind mesh's first row, the skinned mesh's first row, the
    // pose's first row.
    counts: vec4<u32>,
    // Joints the pose has, rows a point of the bind mesh (4 or 6), and
    // whether the pose is dual quaternions.
    more: vec4<u32>,
}

var<uniform> u_skin: SkinPass;
var<storage, read_write> b_rows: array<vec4<f32>>;

// `v` turned by the unit quaternion `q` (`skin::turn32`).
fn turn(q: vec4<f32>, v: vec3<f32>) -> vec3<f32> {
    let c = cross(q.xyz, v);
    return v + 2.0 * cross(q.xyz, c + q.w * v);
}

@compute @workgroup_size(64)
fn skin_points(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x;
    if i >= u_skin.counts.x {
        return;
    }
    let srows = u_skin.more.y;
    let src = u_skin.counts.y + i * srows;
    let a = b_rows[src];
    let b = b_rows[src + 1u];
    let fours = (srows - 2u) / 2u;
    let p = vec4<f32>(a.xyz, 1.0);
    var q = vec3<f32>(0.0);
    var d = vec3<f32>(0.0);
    var n = b.xyz;
    if u_skin.more.z == 0u {
        for (var f = 0u; f < fours; f++) {
            let joints = bitcast<vec4<u32>>(b_rows[src + 2u + f * 2u]);
            let weights = b_rows[src + 3u + f * 2u];
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
        }
        let l = length(d);
        if l > 0.0 {
            n = d / l;
        }
    } else {
        // The scales blended as matrices, the turns and moves as one
        // screw, each joint's quaternion on the side of the first's.
        var ps = vec3<f32>(0.0);
        var ns = vec3<f32>(0.0);
        var br = vec4<f32>(0.0);
        var bd = vec4<f32>(0.0);
        var pivot = vec4<f32>(0.0);
        var any = false;
        for (var f = 0u; f < fours; f++) {
            let joints = bitcast<vec4<u32>>(b_rows[src + 2u + f * 2u]);
            let weights = b_rows[src + 3u + f * 2u];
            for (var k = 0u; k < 4u; k++) {
                let w = weights[k];
                if w == 0.0 || joints[k] >= u_skin.more.x {
                    continue;
                }
                let m = u_skin.counts.w + joints[k] * 5u;
                let real = b_rows[m];
                let r0 = b_rows[m + 2u].xyz;
                let r1 = b_rows[m + 3u].xyz;
                let r2 = b_rows[m + 4u].xyz;
                ps += w * vec3<f32>(dot(r0, a.xyz), dot(r1, a.xyz), dot(r2, a.xyz));
                ns += w * vec3<f32>(dot(r0, b.xyz), dot(r1, b.xyz), dot(r2, b.xyz));
                if !any {
                    pivot = real;
                    any = true;
                }
                var s = w;
                if dot(real, pivot) < 0.0 {
                    s = -w;
                }
                br += s * real;
                bd += s * b_rows[m + 1u];
            }
        }
        let l = length(br);
        q = a.xyz;
        if l > 0.0 {
            br = br / l;
            bd = bd / l;
            let t = 2.0 * (br.w * bd.xyz - bd.w * br.xyz + cross(br.xyz, bd.xyz));
            q = turn(br, ps) + t;
            let dn = turn(br, ns);
            let ln = length(dn);
            if ln > 0.0 {
                n = dn / ln;
            }
        }
    }
    let dst = u_skin.counts.z + i * 2u;
    b_rows[dst] = vec4<f32>(q, a.w);
    b_rows[dst + 1u] = vec4<f32>(n, b.w);
}
