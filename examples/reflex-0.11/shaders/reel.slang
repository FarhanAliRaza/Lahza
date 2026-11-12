// Slang for the Reflex 0.11 reel (build.py --slang): one backdrop and two glazes.
import lahza;

// Backdrop: slow ribbons of colour, like silk under studio light. Noise warps
// the picture's own coordinates, a second noise picks between the two ground
// colours, and a sine of it lays the accent in soft folds.
struct silk_params {
    float4 ground = float4(0.04, 0.035, 0.08, 1);
    float4 fold = float4(0.16, 0.12, 0.37, 1);
    float4 accent = float4(0.43, 0.34, 0.81, 1);
    float speed = 1;
    float seed = 0;
}
float4 silk(float2 uv, float2 px, float t, silk_params p) {
    float2 size = px / uv;
    float2 q = float2(uv.x * size.x / size.y, uv.y) * 1.25 + p.seed * 3.7;
    float tt = t * 0.07 * p.speed;
    float2 w = float2(fbm(float3(q * 0.9, tt), 4), fbm(float3(q * 0.9 + 5.2, tt + 3.1), 4));
    float2 r = q + 1.7 * (w - 0.5);
    float n = fbm(float3(r * 0.75, tt * 1.3 + 7.0), 5);
    float folds = 0.5 + 0.5 * sin(6.2831853 * (n * 1.7 + r.y * 0.3));
    float3 c = lerp(p.ground.rgb, p.fold.rgb, smoothstep(0.3, 0.75, n));
    c = lerp(c, p.accent.rgb, pow(folds, 4.0) * smoothstep(0.35, 0.8, n) * 0.6);
    float grain = hash(int3(int(px.x), int(px.y), int(t * 30.0)));
    return float4(c + (grain - 0.5) * 0.022, 1.0);
}

// Glaze: a ceramic body with pigment pulled through it. Pale veins and
// deeper pools follow warped noise; the glaze is glossier where it pooled,
// and a few dark flecks sit in it.
// params: [feature size px, strength 0..1]
Surface glaze(SurfaceInput s) {
    Surface o = s.material;
    float size = max(s.params[0], 1.0);
    float k = s.params[1];
    float3 q = s.position / size;
    float3 w = float3(fbm(q + 3.1, 4), fbm(q + 11.7, 4), fbm(q + 23.3, 4)) - 0.5;
    float n = fbm(q * 0.8 + 2.2 * w, 5);
    float vein = pow(0.5 + 0.5 * sin(6.2831853 * (n * 2.2 + q.y * 0.15)), 3.0);
    float3 base = o.color;
    o.color = lerp(base, lerp(base, float3(1.0, 1.0, 1.0), 0.5), vein * k);
    o.color = lerp(o.color, base * 0.5, smoothstep(0.55, 0.8, n) * k * 0.7);
    float fleck = hash(int3(floor(s.position * 0.5)));
    o.color = o.color * (1.0 - 0.14 * step(0.97, fleck) * k);
    o.roughness = clamp(o.roughness - (n - 0.45) * 0.3 * k, 0.06, 1.0);
    return o;
}

// Pearl: the colour shifts from lilac to a warm white toward grazing angles.
// params: [strength 0..1]
Surface pearl(SurfaceInput s) {
    Surface o = s.material;
    float f = pow(1.0 - saturate(dot(s.normal, s.view)), 1.6);
    float band = 0.5 + 0.5 * sin(f * 6.0 + fbm(s.position / 70.0, 3) * 4.0);
    float3 tint = lerp(float3(0.74, 0.66, 1.0), float3(1.0, 0.88, 0.82), band);
    o.color = lerp(o.color, tint, saturate(f * 1.25) * s.params[0]);
    return o;
}
