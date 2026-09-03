//! Vector cursor artwork used when the editor re-renders the recorded
//! pointer in a chosen cursor style, the way Cap swaps the captured cursor
//! for its SVG reproductions of the system cursors.  Only the everyday
//! shapes are shipped: arrow, pointing hand, open/closed hand and I-beam.

use super::{model::NormalizedPoint, pointer_timeline::PointerBitmap};
use image::RgbaImage;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex},
};

/// Which artwork set the pointer is drawn with.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CursorFamily {
    /// The cursor bitmap captured during the recording.
    #[default]
    Recorded,
    MacOs,
    MacOsTahoe,
    Windows,
}

impl CursorFamily {
    pub const ALL: [CursorFamily; 4] = [
        CursorFamily::Recorded,
        CursorFamily::MacOs,
        CursorFamily::MacOsTahoe,
        CursorFamily::Windows,
    ];
}

/// Platform-neutral cursor shape, derived from the Xcursor name the captured
/// bitmap matched.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CursorShape {
    #[default]
    Arrow,
    PointingHand,
    OpenHand,
    ClosedHand,
    IBeam,
}

impl CursorShape {
    /// Maps the file names used by Xcursor themes (and their common legacy
    /// aliases) to a shape.
    pub fn from_xcursor_name(name: &str) -> Option<Self> {
        Some(match name {
            "default" | "left_ptr" | "arrow" | "top_left_arrow" => Self::Arrow,
            "pointer" | "hand" | "hand2" | "pointing_hand" => Self::PointingHand,
            "grab" | "openhand" | "hand1" => Self::OpenHand,
            "grabbing" | "closedhand" | "dnd-move" | "dnd-none" => Self::ClosedHand,
            "text" | "xterm" | "ibeam" => Self::IBeam,
            _ => return None,
        })
    }
}

struct Asset {
    svg: &'static str,
    /// Hotspot as a fraction of the SVG box.
    hotspot: (f64, f64),
}

/// Artwork for a shape in a family; shapes a family lacks fall back to its
/// arrow so there is always something to draw.
fn asset(family: CursorFamily, shape: CursorShape) -> Option<Asset> {
    let (svg, hotspot) = match (family, shape) {
        (CursorFamily::Recorded, _) => return None,
        (CursorFamily::MacOs, CursorShape::Arrow) => (
            include_str!("../../assets/cursors/macos/arrow.svg"),
            (0.302, 0.226),
        ),
        (CursorFamily::MacOs, CursorShape::PointingHand) => (
            include_str!("../../assets/cursors/macos/pointing_hand.svg"),
            (0.342, 0.172),
        ),
        (CursorFamily::MacOs, CursorShape::OpenHand) => (
            include_str!("../../assets/cursors/macos/open_hand.svg"),
            (0.5, 0.5),
        ),
        (CursorFamily::MacOs, CursorShape::ClosedHand) => (
            include_str!("../../assets/cursors/macos/closed_hand.svg"),
            (0.5, 0.5),
        ),
        (CursorFamily::MacOs, CursorShape::IBeam) => (
            include_str!("../../assets/cursors/macos/ibeam.svg"),
            (0.484, 0.520),
        ),
        (CursorFamily::MacOsTahoe, CursorShape::Arrow) => (
            include_str!("../../assets/cursors/tahoe/default.svg"),
            (0.320, 0.192),
        ),
        (CursorFamily::MacOsTahoe, CursorShape::PointingHand) => (
            include_str!("../../assets/cursors/tahoe/pointer.svg"),
            (0.425, 0.167),
        ),
        (CursorFamily::MacOsTahoe, CursorShape::OpenHand) => (
            include_str!("../../assets/cursors/tahoe/grab.svg"),
            (0.543, 0.515),
        ),
        (CursorFamily::MacOsTahoe, CursorShape::ClosedHand) => (
            include_str!("../../assets/cursors/tahoe/grabbing.svg"),
            (0.539, 0.498),
        ),
        (CursorFamily::MacOsTahoe, CursorShape::IBeam) => (
            include_str!("../../assets/cursors/tahoe/text.svg"),
            (0.493, 0.464),
        ),
        (CursorFamily::Windows, CursorShape::Arrow) => (
            include_str!("../../assets/cursors/windows/arrow.svg"),
            (0.288, 0.189),
        ),
        (CursorFamily::Windows, CursorShape::PointingHand) => (
            include_str!("../../assets/cursors/windows/hand.svg"),
            (0.441, 0.143),
        ),
        (CursorFamily::Windows, CursorShape::IBeam) => (
            include_str!("../../assets/cursors/windows/ibeam.svg"),
            (0.490, 0.471),
        ),
        (CursorFamily::Windows, CursorShape::OpenHand | CursorShape::ClosedHand) => {
            return asset(family, CursorShape::Arrow)
        }
    };
    Some(Asset { svg, hotspot })
}

/// Rasterized cursors keyed by family, shape and pixel height.  Zoom
/// animations ask for a run of nearby heights, so the cache is bounded.
type CacheKey = (CursorFamily, CursorShape, u32);
static CACHE: LazyLock<Mutex<HashMap<CacheKey, Arc<PointerBitmap>>>> =
    LazyLock::new(Mutex::default);
const CACHE_LIMIT: usize = 128;

/// The shape's artwork rendered `height` pixels tall (premultiplied), or
/// `None` for the recorded family.
pub fn rasterize(
    family: CursorFamily,
    shape: CursorShape,
    height: u32,
) -> Option<Arc<PointerBitmap>> {
    let height = height.clamp(1, 1024);
    let key = (family, shape, height);
    if let Some(bitmap) = CACHE.lock().ok()?.get(&key) {
        return Some(bitmap.clone());
    }
    let asset = asset(family, shape)?;
    let bitmap = Arc::new(render(
        &asset,
        height,
        format!("{family:?}/{shape:?}/{height}"),
    )?);
    let mut cache = CACHE.lock().ok()?;
    if cache.len() >= CACHE_LIMIT {
        cache.clear();
    }
    cache.insert(key, bitmap.clone());
    Some(bitmap)
}

fn render(asset: &Asset, height: u32, id: String) -> Option<PointerBitmap> {
    let tree = resvg::usvg::Tree::from_str(asset.svg, &resvg::usvg::Options::default()).ok()?;
    let size = tree.size();
    let scale = height as f32 / size.height();
    let width = (size.width() * scale).round().max(1.0) as u32;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height)?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );
    // tiny-skia pixmaps are premultiplied RGBA, which is what the painter
    // samples.
    let image = RgbaImage::from_raw(width, height, pixmap.take())?;
    Some(PointerBitmap {
        id,
        image,
        anchor: NormalizedPoint {
            x: asset.hotspot.0,
            y: asset.hotspot.1,
        },
        reference_width: 0.0,
        reference_height: 0.0,
        shape: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_family_renders_every_shape() {
        for family in [
            CursorFamily::MacOs,
            CursorFamily::MacOsTahoe,
            CursorFamily::Windows,
        ] {
            for shape in [
                CursorShape::Arrow,
                CursorShape::PointingHand,
                CursorShape::OpenHand,
                CursorShape::ClosedHand,
                CursorShape::IBeam,
            ] {
                let bitmap = rasterize(family, shape, 60).unwrap();
                assert_eq!(bitmap.image.height(), 60);
                let opaque = bitmap.image.pixels().filter(|p| p[3] > 0).count();
                assert!(opaque > 100, "{family:?}/{shape:?} rendered empty");
            }
        }
        assert!(rasterize(CursorFamily::Recorded, CursorShape::Arrow, 60).is_none());
    }

    #[test]
    fn xcursor_names_map_to_shapes() {
        assert_eq!(
            CursorShape::from_xcursor_name("left_ptr"),
            Some(CursorShape::Arrow)
        );
        assert_eq!(
            CursorShape::from_xcursor_name("hand2"),
            Some(CursorShape::PointingHand)
        );
        assert_eq!(
            CursorShape::from_xcursor_name("xterm"),
            Some(CursorShape::IBeam)
        );
        assert_eq!(
            CursorShape::from_xcursor_name("grabbing"),
            Some(CursorShape::ClosedHand)
        );
        assert_eq!(CursorShape::from_xcursor_name("watch"), None);
    }
}
