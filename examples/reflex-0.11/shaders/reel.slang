// Slang for the Reflex 0.11 reel (build.py --slang).
import lahza;

float cell_hash(float2 p) {
    uint2 q = uint2(int2(floor(p)) + 4096);
    uint h = (q.x * 1597334677u) ^ (q.y * 3812015801u);
    h = h * 747796405u + 2891336453u;
    h ^= h >> 16; h *= 2246822519u; h ^= h >> 13;
    return float(h & 0xffffffu) / 16777216.0;
}

// Backdrop of the 0.11 shot: a dark stage of 48 px blocks, some of them
// lit in the brand colour by a slow drift of noise, brightest low in the
// frame behind the numerals.
struct aurora_params { float4 tint = float4(0.43, 0.34, 0.81, 1); float energy = 1; }
float4 aurora(float2 uv, float2 px, float t, aurora_params p) {
    float2 cell = floor(px / 48.0);
    float2 in_cell = frac(px / 48.0);
    float n = fbm(float3(cell * 0.11, t * 0.22), 4);
    float lit = smoothstep(0.52, 0.78, n) * (0.35 + 0.65 * cell_hash(cell));
    float edge = min(min(in_cell.x, 1.0 - in_cell.x), min(in_cell.y, 1.0 - in_cell.y));
    float block = smoothstep(0.02, 0.10, edge);
    float pool = exp(-4.0 * length((uv - float2(0.5, 0.72)) * float2(1.0, 1.9)));
    float3 base = float3(0.035, 0.035, 0.047) + p.tint.rgb * 0.22 * pool;
    float3 c = base + p.tint.rgb * lit * block * (0.10 + 0.55 * pool) * p.energy;
    return float4(c, 1.0);
}

// The numerals' face: violet lacquer cut into the mark's block grid, with
// seams that glow as a band of light climbs the surface.
// params: [cell px, reveal 0..1]
Surface circuit(SurfaceInput s) {
    Surface o = s.material;
    float size = max(s.params[0], 4.0);
    float2 g = s.position.xy / size;
    float2 in_cell = frac(g);
    float seam = 1.0 - smoothstep(0.03, 0.09, min(min(in_cell.x, 1.0 - in_cell.x), min(in_cell.y, 1.0 - in_cell.y)));
    float tone = cell_hash(floor(g));
    o.color = o.color * (0.78 + 0.44 * tone);
    o.roughness = lerp(0.12, 0.34, tone);
    float band = exp(-pow((frac(-s.position.y / 620.0 + s.time * 0.45) - 0.5) * 7.0, 2.0));
    o.emissive = float3(0.73, 0.65, 1.0) * seam * (0.25 + 2.2 * band) * s.params[1];
    return o;
}
