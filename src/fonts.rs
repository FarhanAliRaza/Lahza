/// Embedded so annotations look the same without a system font installation.
// Supply real faces: GPUI's Linux renderer does not synthesize missing styles.
pub(crate) const HANDWRITTEN_FONTS: &[&[u8]] = &[
    include_bytes!("../assets/fonts/ComicNeue-Regular.ttf"),
    include_bytes!("../assets/fonts/ComicNeue-Bold.ttf"),
    include_bytes!("../assets/fonts/ComicNeue-Italic.ttf"),
    include_bytes!("../assets/fonts/ComicNeue-BoldItalic.ttf"),
];
pub(crate) const HANDWRITTEN_FAMILY: &str = "Comic Neue";
