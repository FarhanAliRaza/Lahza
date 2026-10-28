//! Shared editor colors, branding, and background presets.

use gpui::{
    div, img, linear_color_stop, linear_gradient, prelude::*, px, rgb, svg, AnyElement,
    point, Background, BoxShadow, FontFeatures, FontWeight, Hsla, Window, WindowAppearance,
};
use std::sync::{
    atomic::{AtomicBool, AtomicU8, Ordering},
    Arc, OnceLock,
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
                .text_color(accent()),
        )
        .child(
            svg()
                .path("brand/wordmark-urdu-accent.svg")
                .absolute()
                .inset_0()
                .size_full()
                .text_color(brand()),
        )
        .into_any_element()
}

/// Latin-script wordmark, used in the editor window; the recorder keeps the
/// Urdu wordmark from `brand_wordmark`.
/// The PNG is dark ink, so dark mode falls back to the tintable Urdu mark.
pub(super) fn brand_wordmark_latin(width: f32, height: f32) -> AnyElement {
    if is_dark() {
        return brand_wordmark(height * 250.0 / 80.0, height);
    }
    img("brand/wordmark.png")
        .w(px(width))
        .h(px(height))
        .flex_none()
        .into_any_element()
}

/// Shared credit and support links for the capture window and editor sidebar.
pub(super) fn credit_footer() -> AnyElement {
    div()
        .flex_none()
        .flex()
        .items_center()
        .justify_between()
        .py_3()
        .border_t_1()
        .border_color(line())
        .text_xs()
        .text_color(muted())
        .child(
            div()
                .flex()
                .items_center()
                .gap_1()
                .child("Created with")
                .child(svg().path("icons/heart.svg").size(px(12.0)).flex_none().text_color(brand()))
                .child("by")
                .child(
                    div()
                        .id("credit-linkedin")
                        .py_1()
                        .cursor_pointer()
                        .text_color(accent())
                        .underline()
                        .hover(|style| style.text_color(accent_hover()))
                        .child("Farhan")
                        .on_click(|_, _, cx| cx.open_url("https://www.linkedin.com/in/farhanaliraza/")),
                ),
        )
        .child(
            div()
                .id("credit-issues")
                .py_1()
                .cursor_pointer()
                .hover(|style| style.text_color(ink()))
                .child("Report an issue ↗")
                .on_click(|_, _, cx| cx.open_url("https://github.com/FarhanAliRaza/Lahza/issues")),
        )
        .into_any_element()
}

// ---------------------------------------------------------------------------
// Appearance
// ---------------------------------------------------------------------------

/// Whether the system asks for dark. Every window sets it from its
/// appearance (`follow_appearance`), and `PREFERENCE` can override it, so
/// token lookups stay two relaxed loads instead of threading a theme
/// through every render call.
static SYSTEM_DARK: AtomicBool = AtomicBool::new(false);
static PREFERENCE: AtomicU8 = AtomicU8::new(ThemePreference::System as u8);

/// The user's Appearance choice, saved in the config directory.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(crate) enum ThemePreference {
    System = 0,
    Light = 1,
    Dark = 2,
}

impl ThemePreference {
    pub(crate) const ALL: [Self; 3] = [Self::System, Self::Light, Self::Dark];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }

    pub(crate) fn icon(self) -> &'static str {
        match self {
            Self::System => "icons/monitor.svg",
            Self::Light => "icons/sun.svg",
            Self::Dark => "icons/moon.svg",
        }
    }

    pub(crate) fn next(self) -> Self {
        match self {
            Self::System => Self::Light,
            Self::Light => Self::Dark,
            Self::Dark => Self::System,
        }
    }

    fn path() -> std::path::PathBuf {
        crate::recording::presets::config_root().join("appearance")
    }
}

pub(crate) fn theme_preference() -> ThemePreference {
    match PREFERENCE.load(Ordering::Relaxed) {
        1 => ThemePreference::Light,
        2 => ThemePreference::Dark,
        _ => ThemePreference::System,
    }
}

/// Reads the saved Appearance choice once at startup; missing means System.
pub(crate) fn load_theme_preference() {
    let saved = std::fs::read_to_string(ThemePreference::path()).unwrap_or_default();
    let preference = ThemePreference::ALL
        .into_iter()
        .find(|preference| preference.label().eq_ignore_ascii_case(saved.trim()))
        .unwrap_or(ThemePreference::System);
    PREFERENCE.store(preference as u8, Ordering::Relaxed);
}

/// Applies and saves the Appearance choice. Callers redraw their window;
/// `set_theme_preference` also redraws every other one.
pub(crate) fn store_theme_preference(preference: ThemePreference) {
    PREFERENCE.store(preference as u8, Ordering::Relaxed);
    let path = ThemePreference::path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, preference.label().to_ascii_lowercase());
}

pub(crate) fn set_theme_preference(preference: ThemePreference, cx: &mut gpui::App) {
    store_theme_preference(preference);
    cx.refresh_windows();
}

#[inline]
/// GNOME's Animations switch (`org.gnome.desktop.interface
/// enable-animations`), read once. Decorative motion such as preset and
/// hover demos stays still when it is off; missing gsettings means on.
pub(crate) fn animations_enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::process::Command::new("gsettings")
            .args(["get", "org.gnome.desktop.interface", "enable-animations"])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .is_none_or(|output| String::from_utf8_lossy(&output.stdout).trim() != "false")
    })
}

pub(crate) fn is_dark() -> bool {
    match theme_preference() {
        ThemePreference::System => SYSTEM_DARK.load(Ordering::Relaxed),
        ThemePreference::Light => false,
        ThemePreference::Dark => true,
    }
}

pub(crate) fn set_appearance(appearance: WindowAppearance) {
    SYSTEM_DARK.store(
        matches!(appearance, WindowAppearance::Dark | WindowAppearance::VibrantDark),
        Ordering::Relaxed,
    );
}

/// Adopt the window's current appearance and redraw it whenever the system
/// switches between light and dark.
pub(crate) fn follow_appearance(window: &mut Window) {
    set_appearance(window.appearance());
    window
        .observe_window_appearance(|window, _| {
            set_appearance(window.appearance());
            window.refresh();
        })
        .detach();
}

#[inline]
fn pick(light: Hsla, dark: Hsla) -> Hsla {
    if is_dark() { dark } else { light }
}

const fn hsl(h: f32, s: f32, l: f32, a: f32) -> Hsla {
    Hsla { h: h / 360.0, s, l, a }
}

// ---------------------------------------------------------------------------
// Color tokens. Chrome only: annotation palettes, background presets and
// other user content keep their literal colors.
// ---------------------------------------------------------------------------

/// Window chrome: top bar, inspector, timeline, dialogs.
pub(crate) fn surface() -> Hsla {
    pick(hsl(0.0, 0.0, 1.0, 1.0), hsl(240.0, 0.05, 0.11, 1.0))
}

/// Slightly recessed chrome, e.g. the launcher body and sheets.
pub(super) fn panel() -> Hsla {
    pick(hsl(0.0, 0.0, 0.985, 1.0), hsl(240.0, 0.05, 0.13, 1.0))
}

/// Menus, popovers and toasts: the highest elevation.
pub(crate) fn popover() -> Hsla {
    pick(hsl(0.0, 0.0, 1.0, 1.0), hsl(240.0, 0.05, 0.17, 1.0))
}

/// The well the canvas sits in; darkest surface in dark mode.
pub(crate) fn canvas_well() -> Hsla {
    pick(hsl(220.0, 0.10, 0.93, 1.0), hsl(240.0, 0.06, 0.07, 1.0))
}

/// Resting fill of buttons, segmented tracks, value boxes and sliders.
pub(crate) fn control_bg() -> Hsla {
    pick(hsl(240.0, 0.04, 0.945, 1.0), hsl(240.0, 0.05, 0.19, 1.0))
}

pub(crate) fn control_hover() -> Hsla {
    pick(hsl(240.0, 0.05, 0.90, 1.0), hsl(240.0, 0.05, 0.24, 1.0))
}

/// Raised segment inside a `control_bg` track.
pub(crate) fn control_selected() -> Hsla {
    pick(hsl(0.0, 0.0, 1.0, 1.0), hsl(240.0, 0.05, 0.30, 1.0))
}

pub(super) fn line() -> Hsla {
    pick(hsl(220.0, 0.08, 0.88, 1.0), hsl(240.0, 0.05, 0.24, 1.0))
}

/// Hairline ring for elevated surfaces; translucent so it works on any fill.
pub(crate) fn ring() -> Hsla {
    pick(hsl(0.0, 0.0, 0.0, 0.08), hsl(0.0, 0.0, 1.0, 0.10))
}

pub(super) fn ink() -> Hsla {
    pick(hsl(220.0, 0.13, 0.12, 1.0), hsl(240.0, 0.05, 0.93, 1.0))
}

/// Field labels: one step quieter than `ink`.
pub(crate) fn ink_secondary() -> Hsla {
    pick(hsl(220.0, 0.08, 0.28, 1.0), hsl(240.0, 0.04, 0.80, 1.0))
}

pub(super) fn muted() -> Hsla {
    pick(hsl(220.0, 0.06, 0.43, 1.0), hsl(240.0, 0.04, 0.64, 1.0))
}

/// Primary actions, selection, switches and links: the wordmark ink.
pub(crate) fn accent() -> Hsla {
    pick(hsl(0.0, 0.0, 0.153, 1.0), hsl(240.0, 0.05, 0.93, 1.0))
}

pub(crate) fn accent_hover() -> Hsla {
    pick(hsl(0.0, 0.0, 0.25, 1.0), hsl(240.0, 0.05, 0.82, 1.0))
}

/// Text and icons drawn on an `accent` fill.
pub(crate) fn on_accent() -> Hsla {
    pick(hsl(0.0, 0.0, 1.0, 1.0), hsl(240.0, 0.06, 0.10, 1.0))
}

/// Background of selected rows and cards.
pub(crate) fn accent_tint() -> Hsla {
    pick(hsl(0.0, 0.0, 0.153, 0.07), hsl(0.0, 0.0, 1.0, 0.10))
}

/// Brand red: reserved for "now" (playhead, record, live, stop).
pub(crate) fn brand() -> Hsla {
    hsl(1.15, 0.624, 0.51, 1.0)
}

pub(crate) fn brand_hover() -> Hsla {
    pick(hsl(1.0, 0.61, 0.45, 1.0), hsl(1.0, 0.70, 0.58, 1.0))
}

/// Destructive actions and errors; darker than `brand` so it reads differently.
pub(crate) fn danger() -> Hsla {
    pick(hsl(4.0, 0.77, 0.40, 1.0), hsl(4.0, 0.85, 0.72, 1.0))
}

pub(crate) fn danger_tint() -> Hsla {
    pick(hsl(4.0, 0.86, 0.94, 1.0), hsl(4.0, 0.70, 0.50, 0.18))
}

/// Cautionary text, e.g. filler words in a transcript.
pub(crate) fn warning() -> Hsla {
    pick(hsl(26.0, 0.90, 0.37, 1.0), hsl(36.0, 0.90, 0.62, 1.0))
}

/// 2 px keyboard focus ring.
pub(crate) fn focus_ring() -> Hsla {
    pick(hsl(220.0, 0.13, 0.12, 0.55), hsl(240.0, 0.05, 0.93, 0.6))
}

/// Selection outlines and guides drawn over user content, and text selection
/// highlights. Blue stays here because it must read on any image.
pub(crate) fn selection() -> Hsla {
    hsl(211.0, 0.95, 0.55, 1.0)
}

pub(crate) fn selection_fill() -> Hsla {
    pick(hsl(211.0, 0.95, 0.55, 0.28), hsl(211.0, 0.90, 0.60, 0.38))
}

/// Timeline lane fills; all carry white labels at 4.5:1 or better.
pub(crate) fn lane_video() -> Hsla {
    hsl(217.0, 0.75, 0.45, 1.0)
}

pub(crate) fn lane_zoom() -> Hsla {
    hsl(22.0, 0.85, 0.40, 1.0)
}

pub(crate) fn lane_clip() -> Hsla {
    hsl(188.0, 0.70, 0.30, 1.0)
}

pub(crate) fn lane_camera() -> Hsla {
    hsl(272.0, 0.53, 0.51, 1.0)
}

/// Imported music and sound effects; matches the waveform's green.
pub(crate) fn lane_audio() -> Hsla {
    hsl(152.0, 0.60, 0.30, 1.0)
}

// ---------------------------------------------------------------------------
// Spacing, sizes and radii (px)
// ---------------------------------------------------------------------------

/// Spacing scale: 4 / 8 / 12 / 16. Inspector sections break at 24: the
/// 12 px tab gap on each side of a hairline.
pub(crate) const SPACE_1: f32 = 4.0;
pub(crate) const SPACE_2: f32 = 8.0;
pub(crate) const SPACE_3: f32 = 12.0;
pub(crate) const SPACE_4: f32 = 16.0;

/// Control heights.
pub(crate) const CONTROL_SM: f32 = 28.0;
pub(crate) const CONTROL_MD: f32 = 32.0;
pub(crate) const CONTROL_LG: f32 = 36.0;

/// Corner radii. A control nested inside a padded container uses
/// `inner_radius(outer, padding)` so the corners stay concentric.
pub(crate) const RADIUS_XS: f32 = 2.0;
pub(crate) const RADIUS_SM: f32 = 4.0;
pub(crate) const RADIUS_MD: f32 = 6.0;
pub(crate) const RADIUS_LG: f32 = 8.0;

pub(crate) const fn inner_radius(outer: f32, padding: f32) -> f32 {
    let inner = outer - padding;
    if inner < RADIUS_XS { RADIUS_XS } else { inner }
}

// ---------------------------------------------------------------------------
// Type roles, tabular figures and elevation
// ---------------------------------------------------------------------------

/// Inter with tabular figures, for numbers that change while you watch.
fn tabular_features() -> FontFeatures {
    static FEATURES: OnceLock<FontFeatures> = OnceLock::new();
    FEATURES
        .get_or_init(|| FontFeatures(Arc::new(vec![("tnum".into(), 1)])))
        .clone()
}

/// One-call text roles. Weights are 400 / 500 / 600 only.
pub(crate) trait TextRoles: Styled + Sized {
    /// Panel title: 14 / 600.
    fn panel_title(self) -> Self {
        self.text_size(px(14.0)).font_weight(FontWeight::SEMIBOLD).text_color(ink())
    }
    /// Section heading: 13 / 600.
    fn section_title(self) -> Self {
        self.text_size(px(13.0)).font_weight(FontWeight::SEMIBOLD).text_color(ink())
    }
    /// Row label: 13 / 500.
    fn row_label(self) -> Self {
        self.text_size(px(13.0)).font_weight(FontWeight::MEDIUM).text_color(ink())
    }
    /// Field label above a control: 12 / 500, secondary ink.
    fn field_label(self) -> Self {
        self.text_size(px(12.0)).font_weight(FontWeight::MEDIUM).text_color(ink_secondary())
    }
    /// Hint under a control: 12 / 400, muted, 16 px leading.
    fn hint(self) -> Self {
        self.text_size(px(12.0))
            .font_weight(FontWeight::NORMAL)
            .line_height(px(16.0))
            .text_color(muted())
    }
    /// Micro label (badges, ruler, pills): 11 / 500.
    fn micro(self) -> Self {
        self.text_size(px(11.0)).font_weight(FontWeight::MEDIUM)
    }
    /// Fixed-width digits so changing values do not jitter.
    fn tabular(mut self) -> Self {
        self.text_style().get_or_insert_with(Default::default).font_features =
            Some(tabular_features());
        self
    }
    /// Resting card: hairline ring plus a 1 px shadow.
    fn card(self) -> Self {
        self.border_1().border_color(ring()).shadow(card_shadow())
    }
    /// Floating layer (menus, toasts, popovers): ring plus a layered shadow.
    fn float(self) -> Self {
        self.border_1().border_color(ring()).bg(popover()).shadow(float_shadow())
    }
    /// 2 px focus ring.
    fn focus_ring(self) -> Self {
        self.border_2().border_color(focus_ring())
    }
}

impl<T: Styled + Sized> TextRoles for T {}

fn shadow(y: f32, blur: f32, alpha: f32) -> BoxShadow {
    // Dark surfaces separate by ring and lightness; keep shadows faint there.
    let alpha = if is_dark() { alpha * 1.6 } else { alpha };
    BoxShadow {
        color: hsl(0.0, 0.0, 0.0, alpha.min(0.6)),
        offset: point(px(0.0), px(y)),
        blur_radius: px(blur),
        spread_radius: px(0.0),
    }
}

fn card_shadow() -> Vec<BoxShadow> {
    vec![shadow(1.0, 2.0, 0.05)]
}

fn float_shadow() -> Vec<BoxShadow> {
    vec![shadow(2.0, 4.0, 0.06), shadow(8.0, 24.0, 0.12)]
}

/// The canvas in its well: a contact shadow plus a soft ambient layer.
pub(crate) fn canvas_shadow() -> Vec<BoxShadow> {
    vec![shadow(1.0, 2.0, 0.06), shadow(4.0, 8.0, 0.06), shadow(16.0, 40.0, 0.10)]
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
