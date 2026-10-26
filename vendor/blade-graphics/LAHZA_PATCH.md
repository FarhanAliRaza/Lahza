# Lahza patch to blade-graphics 0.7.1

Vendored from the crates.io 0.7.1 archive. Crate source commit: `88b6c64cd7a32dd933acc4e5266808a07143ce84`.
The upstream MIT license is included as LICENSE (the registry archive omitted
the workspace-root license). Nonessential demonstration images are omitted.
The only functional changes expose optional `Capabilities::shader_f16`, query
and enable Vulkan shaderFloat16 when supported, and grant Naga FLOAT16 validation
only on an enabled device. GLES/Metal remain false and use existing f32 shaders.
No 16-bit buffer storage is enabled; neural inputs and outputs remain f32/u32.
