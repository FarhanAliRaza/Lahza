// Takes a mesh the GPU's mesh pool holds (`pool.rs`: one the project's
// Slang generated there) into the path tracer's arrays, so the CPU never
// has it: per triangle, its point ids into the indices and its record as
// the tag the tracer reads (`mesh::tag`: part, segment, two-sided). The
// points' rows are the same in both and are copied as they are.

struct FromPool {
    // The triangles' first row in the pool, how many, where their
    // indices start and where their tags do.
    rows: vec4<u32>,
}

var<uniform> u_from: FromPool;
var<storage, read> b_pool: array<vec4<u32>>;
var<storage, read_write> b_idx: array<u32>;
var<storage, read_write> b_tags: array<u32>;

@compute @workgroup_size(64)
fn from_pool(@builtin(global_invocation_id) id: vec3<u32>) {
    let j = id.x;
    if j >= u_from.rows.y {
        return;
    }
    let rec = b_pool[u_from.rows.x + j];
    let at = u_from.rows.z + j * 3u;
    b_idx[at] = rec.x;
    b_idx[at + 1u] = rec.y;
    b_idx[at + 2u] = rec.z;
    // Pool record: segment (bits 0..16), part (16..19), two-sided (22).
    let packed = rec.w;
    b_tags[u_from.rows.w + j] = ((packed >> 16u) & 7u) | ((packed & 0xffffu) << 8u) | (((packed >> 22u) & 1u) << 31u);
}
