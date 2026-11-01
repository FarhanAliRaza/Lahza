//! NVIDIA NemotronLabs RE-USE (SEMamba, 30 time/frequency bidirectional Mamba
//! blocks) speech enhancement on blade compute shaders, so it runs on any
//! Vulkan GPU without ONNX Runtime or CUDA. Provenance: assets/audio/RE_USE.md.
//!
//! One `Engine::enhance` call is one inference.py forward pass over a segment
//! (InstanceNorm statistics span the whole segment). Work is submitted per
//! stage and waited on, so a caller on a worker thread never holds the GPU for
//! more than one stage at a time.
#![allow(dead_code)]

mod reuse_audio;
pub use reuse_audio::*;
