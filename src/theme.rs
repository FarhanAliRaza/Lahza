//! Shared editor colors, branding, and background presets.

use gpui::{
    div, hsla, img, linear_color_stop, linear_gradient, prelude::*, px, rgb, svg, AnyElement,
    Background, Hsla,
};

pub(super) fn brand_wordmark(width: f32, height: f32) -> AnyElement {
    div()
        .relative()
        .w(px(width))
        .h(px(height))
        .flex_none()
        .child(
            svg()
                .path("brand/wordmark-urdu-ink.svg")
                .absolute()
                .inset_0()
                .size_full()
                .text_color(rgb(0x272727)),
        )
        .child(
            svg()
                .path("brand/wordmark-urdu-accent.svg")
                .absolute()
                .inset_0()
                .size_full()
                .text_color(rgb(0xd03734)),
        )
        .into_any_element()
}

/// Latin-script wordmark, used in the editor window; the recorder keeps the
/// Urdu wordmark from `brand_wordmark`.
pub(super) fn brand_wordmark_latin(width: f32, height: f32) -> AnyElement {
    img("brand/wordmark.png")
        .w(px(width))
        .h(px(height))
        .flex_none()
        .into_any_element()
}

pub(super) fn ink() -> Hsla {
    hsla(220.0 / 360.0, 0.13, 0.12, 1.0)
}

pub(super) fn muted() -> Hsla {
    hsla(220.0 / 360.0, 0.06, 0.43, 1.0)
}

pub(super) fn line() -> Hsla {
    hsla(220.0 / 360.0, 0.08, 0.88, 1.0)
}

pub(super) fn panel() -> Hsla {
    hsla(0.0, 0.0, 0.985, 1.0)
}

pub(super) fn blue() -> Hsla {
    hsla(211.0 / 360.0, 0.95, 0.55, 1.0)
}

pub(super) const SOLID_BACKGROUNDS: [(&str, u32); 16] = [
    ("Black", 0x050506),
    ("White", 0xf5f5f0),
    ("Graphite", 0x2b2e36),
    ("Red", 0xf03b47),
    ("Orange", 0xf78429),
    ("Yellow", 0xf5ba3b),
    ("Green", 0x3b9c5c),
    ("Blue", 0x2980e0),
    ("Purple", 0x7a42e8),
    ("Blush", 0xeda89e),
    ("Mint", 0xa8e6ba),
    ("Sky", 0xa1c9f0),
    ("Lavender", 0xccc2eb),
    ("Peach", 0xfaccb0),
    ("Sage", 0xbdd1b3),
    ("Sand", 0xe8dec2),
];

#[derive(Clone, Copy)]
pub(super) struct GradientPreset {
    pub(super) title: &'static str,
    pub(super) colors: [u32; 3],
    pub(super) angle: f32,
}

pub(super) const GRADIENT_BACKGROUNDS: [GradientPreset; 16] = [
    GradientPreset {
        title: "Aurora",
        colors: [0xfa4f94, 0x6652f2, 0x4ad6cc],
        angle: 135.0,
    },
    GradientPreset {
        title: "Cobalt",
        colors: [0x0a0d80, 0x4230ed, 0x6babfa],
        angle: 160.0,
    },
    GradientPreset {
        title: "Peach",
        colors: [0xfa615c, 0xfcb55c, 0xe654a6],
        angle: 135.0,
    },
    GradientPreset {
        title: "Glass",
        colors: [0xdef2f0, 0x75c4db, 0x4087ed],
        angle: 135.0,
    },
    GradientPreset {
        title: "Plasma",
        colors: [0x140538, 0x591fd6, 0xf2426b],
        angle: 225.0,
    },
    GradientPreset {
        title: "Mango",
        colors: [0xfcbf33, 0xf55436, 0xab30e3],
        angle: 135.0,
    },
    GradientPreset {
        title: "Mist",
        colors: [0xf0f0eb, 0xcce0f0, 0xf2c2b3],
        angle: 135.0,
    },
    GradientPreset {
        title: "Lagoon",
        colors: [0x144d8a, 0x40a3b8, 0xb3ebc7],
        angle: 45.0,
    },
    GradientPreset {
        title: "Ember",
        colors: [0x2e0814, 0xdb2b2e, 0xffab40],
        angle: 135.0,
    },
    GradientPreset {
        title: "Violet",
        colors: [0x3d1482, 0x9638f0, 0xf56bbd],
        angle: 160.0,
    },
    GradientPreset {
        title: "Sea Glass",
        colors: [0x6edbbf, 0x409ecc, 0x3859bf],
        angle: 135.0,
    },
    GradientPreset {
        title: "Citrus",
        colors: [0xfce84d, 0x70c74a, 0x1f946b],
        angle: 225.0,
    },
    GradientPreset {
        title: "Amethyst",
        colors: [0x1a1447, 0x5926a6, 0xc263f2],
        angle: 45.0,
    },
    GradientPreset {
        title: "Sorbet",
        colors: [0xff7d82, 0xffbd7a, 0x8fc7fa],
        angle: 135.0,
    },
    GradientPreset {
        title: "Mineral",
        colors: [0xedf5f2, 0xa3b8d1, 0x546b8c],
        angle: 135.0,
    },
    GradientPreset {
        title: "Dawn",
        colors: [0xfa9ec4, 0xfad178, 0x6bb5f5],
        angle: 45.0,
    },
];

#[derive(Clone, Copy)]
pub(super) struct BackgroundPreset {
    pub(super) name: &'static str,
    pub(super) wallpaper_tab: usize,
    pub(super) wallpaper_asset: &'static str,
    pub(super) color_index: usize,
    pub(super) gradient_index: usize,
    pub(super) padding: u8,
    pub(super) shadow: u8,
    pub(super) corners: u8,
    pub(super) shadow_style: usize,
    pub(super) aspect_ratio: usize,
    pub(super) border: bool,
    pub(super) border_color: usize,
    pub(super) border_thickness: u8,
    pub(super) border_opacity: u8,
}

pub(super) const BACKGROUND_PRESETS: [BackgroundPreset; 5] = [
    BackgroundPreset {
        name: "Frosted Lake",
        wallpaper_tab: 2,
        wallpaper_asset: "wallpapers/uihssn/uihssn-2.jpeg",
        color_index: 7,
        gradient_index: 0,
        padding: 8,
        shadow: 14,
        corners: 2,
        shadow_style: 1,
        aspect_ratio: 0,
        border: false,
        border_color: 3,
        border_thickness: 12,
        border_opacity: 30,
    },
    BackgroundPreset {
        name: "Sunset Glass",
        wallpaper_tab: 2,
        wallpaper_asset: "wallpapers/uihssn/uihssn-4.jpeg",
        color_index: 4,
        gradient_index: 4,
        padding: 14,
        shadow: 32,
        corners: 10,
        shadow_style: 0,
        aspect_ratio: 0,
        border: false,
        border_color: 0,
        border_thickness: 0,
        border_opacity: 0,
    },
    BackgroundPreset {
        name: "Midnight",
        wallpaper_tab: 0,
        wallpaper_asset: "wallpapers/uihssn/uihssn-12.jpg",
        color_index: 0,
        gradient_index: 0,
        padding: 10,
        shadow: 38,
        corners: 7,
        shadow_style: 2,
        aspect_ratio: 0,
        border: true,
        border_color: 4,
        border_thickness: 5,
        border_opacity: 55,
    },
    BackgroundPreset {
        name: "Clean White",
        wallpaper_tab: 0,
        wallpaper_asset: "wallpapers/uihssn/uihssn-12.jpg",
        color_index: 1,
        gradient_index: 0,
        padding: 7,
        shadow: 18,
        corners: 4,
        shadow_style: 0,
        aspect_ratio: 0,
        border: false,
        border_color: 3,
        border_thickness: 0,
        border_opacity: 0,
    },
    BackgroundPreset {
        name: "Aurora",
        wallpaper_tab: 1,
        wallpaper_asset: "wallpapers/uihssn/uihssn-12.jpg",
        color_index: 7,
        gradient_index: 6,
        padding: 12,
        shadow: 26,
        corners: 12,
        shadow_style: 1,
        aspect_ratio: 0,
        border: true,
        border_color: 2,
        border_thickness: 4,
        border_opacity: 40,
    },
];

pub(super) const CURATED_WALLPAPERS: [&str; 17] = [
    "wallpapers/uihssn/uihssn-2.jpeg",
    "wallpapers/uihssn/uihssn-3.jpeg",
    "wallpapers/uihssn/uihssn-4.jpeg",
    "wallpapers/uihssn/uihssn-5.jpeg",
    "wallpapers/uihssn/uihssn-6.jpg",
    "wallpapers/uihssn/uihssn-7.png",
    "wallpapers/uihssn/uihssn-9.jpg",
    "wallpapers/uihssn/uihssn-10.jpg",
    "wallpapers/uihssn/uihssn-11.jpg",
    "wallpapers/uihssn/uihssn-12.jpg",
    "wallpapers/uihssn/uihssn-13.jpg",
    "wallpapers/uihssn/uihssn-14.jpg",
    "wallpapers/fayaz/blue-skies.jpg",
    "wallpapers/fayaz/canyon.jpg",
    "wallpapers/fayaz/golden-gate-bridge.jpg",
    "wallpapers/fayaz/India.jpg",
    "wallpapers/fayaz/nyc.jpg",
];

pub(super) fn gradient_layers(preset: GradientPreset) -> (Background, Background) {
    let middle = Hsla::from(rgb(preset.colors[1]));
    (
        linear_gradient(
            preset.angle,
            linear_color_stop(rgb(preset.colors[0]), 0.0),
            linear_color_stop(middle, 1.0),
        ),
        linear_gradient(
            preset.angle,
            linear_color_stop(middle.opacity(0.0), 0.35),
            linear_color_stop(rgb(preset.colors[2]), 1.0),
        ),
    )
}
