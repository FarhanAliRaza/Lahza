/// Embedded so annotations look the same without a system font installation.
// Supply real faces: GPUI's Linux renderer does not synthesize missing styles.
pub const HANDWRITTEN_FONTS: &[&[u8]] = &[
    include_bytes!("../assets/fonts/ShantellSansInformal-Regular.ttf"),
    include_bytes!("../assets/fonts/ShantellSansInformal-Bold.ttf"),
    include_bytes!("../assets/fonts/ShantellSansInformal-Italic.ttf"),
    include_bytes!("../assets/fonts/ShantellSansInformal-BoldItalic.ttf"),
];
pub const HANDWRITTEN_FAMILY: &str = "Shantell Sans Informal";

/// Bundled and system fonts loaded once and shared by every SVG render (loading them
/// per frame would dominate watermark and annotation rendering).
pub fn shared_fontdb() -> std::sync::Arc<resvg::usvg::fontdb::Database> {
    static FONTS: std::sync::OnceLock<std::sync::Arc<resvg::usvg::fontdb::Database>> =
        std::sync::OnceLock::new();
    FONTS
        .get_or_init(|| {
            let mut database = resvg::usvg::fontdb::Database::new();
            database.load_system_fonts();
            for bytes in HANDWRITTEN_FONTS {
                database.load_font_data(bytes.to_vec());
            }
            std::sync::Arc::new(database)
        })
        .clone()
}
