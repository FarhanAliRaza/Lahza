//! Lahza's capture, editing, compositing, and export engine. Each module
//! is its own crate under `crates/`; this crate re-exports them under the
//! module names the app uses and runs the tests that span several layers.

pub use lahza_viewport::CropRect;

pub use lahza_animation as animation;
pub use lahza_area as area;
pub use lahza_area_indicator as area_indicator;
pub use lahza_audio_dsp as audio_dsp;
pub use lahza_audio_mode as audio_mode;
pub use lahza_audio_processing as audio_processing;
pub use lahza_backgrounds as backgrounds;
pub use lahza_camera_matting as camera_matting;
pub use lahza_camera_playback as camera_playback;
pub use lahza_camera_preview as camera_preview;
pub use lahza_capture_exclusion as capture_exclusion;
pub use lahza_clips as clips;
pub use lahza_cursor_assets as cursor_assets;
pub use lahza_cursor_theme as cursor_theme;
pub use lahza_export as export;
pub use lahza_import_image as import_image;
pub use lahza_input as input;
pub use lahza_loupe as loupe;
pub use lahza_media as media;
pub use lahza_model as model;
pub use lahza_motion as motion;
pub use lahza_motion_blur as motion_blur;
pub use lahza_native as native;
pub use lahza_overlays as overlays;
pub use lahza_playback as playback;
pub use lahza_pointer as pointer;
pub use lahza_pointer_timeline as pointer_timeline;
pub use lahza_presets as presets;
pub use lahza_project as project;
pub use lahza_rect as rect;
pub use lahza_reuse_audio as reuse_audio;
pub use lahza_scene as scene;
pub use lahza_scene_model::{camera_layout, camera_timeline, scene_model};
pub use lahza_screen_upscale as screen_upscale;
pub use lahza_session as session;
pub use lahza_speech_diagnostics as speech_diagnostics;
pub use lahza_speech_runtime as speech_runtime;
pub use lahza_speech_setup as speech_setup;
pub use lahza_studio_overlays as studio_overlays;
pub use lahza_teleprompter as teleprompter;
pub use lahza_templates as templates;
pub use lahza_transcript as transcript;
pub use lahza_transcription as transcription;
pub use lahza_video::{media_bank, video};
pub use lahza_viewport as viewport;

#[cfg(test)]
mod media_tests;
#[cfg(test)]
mod playback_tests;
#[cfg(test)]
mod video_tests;
