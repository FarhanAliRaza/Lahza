# Optional neural screen enhancement

The bundled model is **Anime4K v3.2 Upscale CNN x2 S** by bloc97, licensed
under MIT. It was designed for animation/line art, not trained specifically on
UI screenshots. It is not Screen Studio's model and makes no parity claim.

- Source: <https://github.com/bloc97/Anime4K/blob/28535a4aff3deb55707a36c2f2d05009e0cb7fb3/glsl/Upscale/Anime4K_Upscale_CNN_x2_S.glsl>
- Original GLSL SHA-256: `4c53ec2e287908f7ee7bcb266b0170421626d663576468b7d7dafc62962649a4`
- Copyright (c) 2019–2021 bloc97. Full permission notice: [LICENSE](LICENSE).

The original 18.6 KB shader is retained as provenance and a test oracle;
it is never executed or compiled by Lahza. `anime4k-cnn-x2-s.f32` contains its
unchanged learned coefficients as little-endian float32. Each of four layers
has 18 column-major 4×4 matrices (positive input then negative input, x-major
3×3 tap order) followed by four biases. Layer one's unused negative matrices
are zero. The implementation in `screen_upscale.wgsl` is an independent tiled
compute port. Four-times output applies this same 2× network twice, with the
second pass's learned residual attenuated to 20%. Full-strength repetition
produced ringing and broken strokes on a small UI text fixture; the attenuation
is a Lahza rendering choice and does not change the licensed weights.

Lahza retains bilinearly interpolated source alpha instead of adding the
network's luma residual to alpha. The residual is added equally to RGB, then
clamped to the valid range. Inference is optional and bounded; original source
pixels remain available whenever enhancement is unavailable.
