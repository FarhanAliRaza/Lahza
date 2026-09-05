//! Swaps captured cursor bitmaps for the high-resolution versions shipped by
//! the user's Xcursor theme, mirroring how Screen Studio replaces known
//! system cursors when the cursor is enlarged, and records which shape each
//! captured cursor is so it can be redrawn in another cursor style.

use super::{
    cursor_assets::CursorShape,
    model::{NormalizedPoint, PointerArtwork},
};
use base64::Engine;
use image::RgbaImage;
use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    process::Command,
};

const XCURSOR_MAGIC: &[u8; 4] = b"Xcur";
const XCURSOR_IMAGE_TYPE: u32 = 0xfffd_0002;
const IMAGE_HEADER_LEN: usize = 36;
/// Largest theme image worth loading; the biggest sizes shipped by common
/// themes are 96-128 px.
const MAX_THEME_EDGE: u32 = 256;
/// Mean per-channel difference (0-255) below which a captured bitmap is
/// considered the same cursor as a theme image.
const MATCH_THRESHOLD: f64 = 10.0;

/// One frame of a theme cursor at one nominal size.
struct ThemeImage {
    nominal_size: u32,
    hotspot: (u32, u32),
    /// Premultiplied RGBA, as stored in the Xcursor file.
    pixels: RgbaImage,
}

/// Every frame of one cursor shape, in file order.
struct ThemeCursor {
    /// File names (and symlink aliases) the theme uses for this cursor.
    names: Vec<String>,
    images: Vec<ThemeImage>,
}

impl ThemeCursor {
    fn shape(&self) -> Option<CursorShape> {
        self.names
            .iter()
            .find_map(|name| CursorShape::from_xcursor_name(name))
    }
}

/// Tags each captured artwork that matches a theme cursor with that cursor's
/// shape and replaces its image with the largest available rendition.
/// Anchor points move with the replacement; reference sizes are untouched so
/// the cursor keeps its recorded proportion.
pub fn upgrade_artwork(artwork: &mut [PointerArtwork]) {
    if artwork.is_empty() {
        return;
    }
    let cursors = load_theme_cursors(&theme_search_dirs());
    if cursors.is_empty() {
        return;
    }
    for item in artwork {
        let Some(captured) = decode_png(&item.image_data_base64) else {
            continue;
        };
        let Some((cursor, frame_index)) = matching_cursor(&cursors, &captured) else {
            continue;
        };
        item.shape = cursor.shape();
        let Some(best) = sharper_frame(cursor, frame_index, captured.width()) else {
            continue;
        };
        let Some(encoded) = encode_png(&best.straight_pixels()) else {
            continue;
        };
        item.image_data_base64 = encoded;
        item.anchor_point = NormalizedPoint {
            x: f64::from(best.hotspot.0) / f64::from(best.pixels.width()),
            y: f64::from(best.hotspot.1) / f64::from(best.pixels.height()),
        };
    }
}

/// The theme cursor (and frame index within its size) whose same-sized
/// image is closest to the captured bitmap.
fn matching_cursor<'a>(
    cursors: &'a [ThemeCursor],
    captured: &RgbaImage,
) -> Option<(&'a ThemeCursor, usize)> {
    let premultiplied = premultiply(captured);
    let mut best: Option<(f64, &ThemeCursor, usize)> = None;
    for cursor in cursors {
        let mut frame_index = 0;
        let mut seen_size = None;
        for image in &cursor.images {
            // Frames of one size arrive consecutively; track the index of
            // this frame within its size so animations line up.
            if seen_size != Some(image.nominal_size) {
                seen_size = Some(image.nominal_size);
                frame_index = 0;
            }
            if image.pixels.dimensions() == captured.dimensions() {
                let score = mean_difference(&image.pixels, &premultiplied);
                if score < MATCH_THRESHOLD && best.is_none_or(|(current, _, _)| score < current) {
                    best = Some((score, cursor, frame_index));
                }
            }
            frame_index += 1;
        }
    }
    best.map(|(_, cursor, frame_index)| (cursor, frame_index))
}

/// The same frame at the cursor's largest size, when it is sharper than the
/// captured bitmap.
fn sharper_frame(
    cursor: &ThemeCursor,
    frame_index: usize,
    captured_width: u32,
) -> Option<&ThemeImage> {
    let largest = cursor.images.iter().map(|image| image.nominal_size).max()?;
    let smallest_bigger = cursor
        .images
        .iter()
        .filter(|image| image.nominal_size == largest)
        .nth(frame_index)
        .or_else(|| {
            cursor
                .images
                .iter()
                .find(|image| image.nominal_size == largest)
        })?;
    // Only swap when the theme actually has something sharper.
    (smallest_bigger.pixels.width() > captured_width).then_some(smallest_bigger)
}

impl ThemeImage {
    fn straight_pixels(&self) -> RgbaImage {
        let mut image = self.pixels.clone();
        for pixel in image.pixels_mut() {
            let alpha = u32::from(pixel[3]);
            if alpha == 0 || alpha == 255 {
                continue;
            }
            for channel in &mut pixel.0[..3] {
                *channel = ((u32::from(*channel) * 255 + alpha / 2) / alpha).min(255) as u8;
            }
        }
        image
    }
}

fn premultiply(image: &RgbaImage) -> RgbaImage {
    let mut out = image.clone();
    for pixel in out.pixels_mut() {
        let alpha = u32::from(pixel[3]);
        for channel in &mut pixel.0[..3] {
            *channel = ((u32::from(*channel) * alpha + 127) / 255) as u8;
        }
    }
    out
}

fn mean_difference(a: &RgbaImage, b: &RgbaImage) -> f64 {
    let total: u64 = a
        .as_raw()
        .iter()
        .zip(b.as_raw())
        .map(|(x, y)| u64::from(x.abs_diff(*y)))
        .sum();
    total as f64 / a.as_raw().len().max(1) as f64
}

fn decode_png(base64_data: &str) -> Option<RgbaImage> {
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(base64_data.as_bytes())
        .ok()?;
    Some(image::load_from_memory(&bytes).ok()?.into_rgba8())
}

fn encode_png(image: &RgbaImage) -> Option<String> {
    let mut png = std::io::Cursor::new(Vec::new());
    image.write_to(&mut png, image::ImageFormat::Png).ok()?;
    Some(base64::engine::general_purpose::STANDARD.encode(png.into_inner()))
}

/// Theme directories to scan, most specific first: the configured theme and
/// everything it inherits, then the usual fallbacks.
fn theme_search_dirs() -> Vec<PathBuf> {
    let mut names: Vec<String> = Vec::new();
    if let Ok(name) = std::env::var("XCURSOR_THEME") {
        if !name.is_empty() {
            names.push(name);
        }
    }
    if let Some(name) = gsettings_cursor_theme() {
        names.push(name);
    }
    names.extend(["Adwaita".to_string(), "default".to_string()]);

    let roots = icon_roots();
    let mut dirs = Vec::new();
    let mut visited = HashSet::new();
    let mut queue = names;
    while let Some(name) = queue.first().cloned() {
        queue.remove(0);
        if !visited.insert(name.clone()) || dirs.len() > 12 {
            continue;
        }
        for root in &roots {
            let theme_dir = root.join(&name);
            let cursors = theme_dir.join("cursors");
            if cursors.is_dir() {
                dirs.push(cursors);
            }
            if let Ok(index) = fs::read_to_string(theme_dir.join("index.theme")) {
                for line in index.lines() {
                    if let Some(rest) = line.trim().strip_prefix("Inherits=") {
                        queue.extend(rest.split(',').map(|s| s.trim().to_string()));
                    }
                }
            }
        }
    }
    dirs
}

fn gsettings_cursor_theme() -> Option<String> {
    let output = Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "cursor-theme"])
        .output()
        .ok()?;
    let value = String::from_utf8(output.stdout).ok()?;
    let name = value.trim().trim_matches('\'').to_string();
    (!name.is_empty()).then_some(name)
}

fn icon_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        roots.push(PathBuf::from(&home).join(".icons"));
        roots.push(PathBuf::from(&home).join(".local/share/icons"));
    }
    if let Ok(path) = std::env::var("XCURSOR_PATH") {
        roots.extend(path.split(':').filter(|p| !p.is_empty()).map(PathBuf::from));
    }
    roots.push(PathBuf::from("/usr/share/icons"));
    roots.push(PathBuf::from("/usr/local/share/icons"));
    roots.push(PathBuf::from("/usr/share/pixmaps"));
    roots
}

/// Loads every distinct cursor file under the given `cursors/` directories.
/// Themes alias most shapes through symlinks, so files are de-duplicated by
/// their resolved path.
fn load_theme_cursors(dirs: &[PathBuf]) -> Vec<ThemeCursor> {
    let mut seen: HashMap<PathBuf, usize> = HashMap::new();
    let mut cursors: Vec<ThemeCursor> = Vec::new();
    for dir in dirs {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(resolved) = fs::canonicalize(&path) else {
                continue;
            };
            let name = entry.file_name().to_string_lossy().into_owned();
            if let Some(&index) = seen.get(&resolved) {
                cursors[index].names.push(name);
                continue;
            }
            if let Some(mut cursor) = load_xcursor_file(&resolved) {
                cursor.names.push(name);
                seen.insert(resolved, cursors.len());
                cursors.push(cursor);
            }
        }
    }
    cursors
}

fn load_xcursor_file(path: &Path) -> Option<ThemeCursor> {
    let bytes = fs::read(path).ok()?;
    parse_xcursor(&bytes)
}

fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    bytes
        .get(offset..offset + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// Parses the Xcursor container format: a table of contents pointing at
/// ARGB image chunks, one per (size, frame).
fn parse_xcursor(bytes: &[u8]) -> Option<ThemeCursor> {
    if bytes.get(0..4)? != XCURSOR_MAGIC {
        return None;
    }
    let toc_count = read_u32(bytes, 12)? as usize;
    let mut images = Vec::new();
    for index in 0..toc_count.min(4096) {
        let entry = 16 + index * 12;
        let kind = read_u32(bytes, entry)?;
        let position = read_u32(bytes, entry + 8)? as usize;
        if kind != XCURSOR_IMAGE_TYPE {
            continue;
        }
        let chunk_type = read_u32(bytes, position + 4)?;
        if chunk_type != XCURSOR_IMAGE_TYPE {
            continue;
        }
        let nominal_size = read_u32(bytes, position + 8)?;
        let width = read_u32(bytes, position + 16)?;
        let height = read_u32(bytes, position + 20)?;
        let hot_x = read_u32(bytes, position + 24)?;
        let hot_y = read_u32(bytes, position + 28)?;
        if width == 0 || height == 0 || width > MAX_THEME_EDGE || height > MAX_THEME_EDGE {
            continue;
        }
        let data_start = position + IMAGE_HEADER_LEN;
        let data_len = (width * height * 4) as usize;
        let data = bytes.get(data_start..data_start + data_len)?;
        let mut pixels = Vec::with_capacity(data_len);
        for pixel in data.chunks_exact(4) {
            // Xcursor stores little-endian ARGB, premultiplied.
            pixels.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
        }
        images.push(ThemeImage {
            nominal_size,
            hotspot: (hot_x.min(width - 1), hot_y.min(height - 1)),
            pixels: RgbaImage::from_raw(width, height, pixels)?,
        });
    }
    (!images.is_empty()).then_some(ThemeCursor {
        names: Vec::new(),
        images,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn xcursor_bytes(images: &[(u32, u32, u32, u32, [u8; 4])]) -> Vec<u8> {
        // (nominal, width, height, hot, ARGB fill)
        let mut bytes = Vec::new();
        bytes.extend_from_slice(XCURSOR_MAGIC);
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&0x1_0000u32.to_le_bytes());
        bytes.extend_from_slice(&(images.len() as u32).to_le_bytes());
        let mut position = 16 + images.len() * 12;
        let mut chunks = Vec::new();
        for (nominal, width, height, hot, fill) in images {
            bytes.extend_from_slice(&XCURSOR_IMAGE_TYPE.to_le_bytes());
            bytes.extend_from_slice(&nominal.to_le_bytes());
            bytes.extend_from_slice(&(position as u32).to_le_bytes());
            let mut chunk = Vec::new();
            for value in [
                IMAGE_HEADER_LEN as u32,
                XCURSOR_IMAGE_TYPE,
                *nominal,
                1,
                *width,
                *height,
                *hot,
                *hot,
                0,
            ] {
                chunk.extend_from_slice(&value.to_le_bytes());
            }
            for _ in 0..width * height {
                // little-endian ARGB word: B, G, R, A in memory
                chunk.extend_from_slice(&[fill[3], fill[2], fill[1], fill[0]]);
            }
            position += chunk.len();
            chunks.push(chunk);
        }
        for chunk in chunks {
            bytes.extend_from_slice(&chunk);
        }
        bytes
    }

    fn solid(width: u32, rgba: [u8; 4]) -> RgbaImage {
        RgbaImage::from_pixel(width, width, image::Rgba(rgba))
    }

    #[test]
    fn parses_xcursor_images_and_converts_argb_to_rgba() {
        let bytes = xcursor_bytes(&[(24, 24, 24, 3, [255, 10, 20, 30])]);
        let cursor = parse_xcursor(&bytes).unwrap();
        assert_eq!(cursor.images.len(), 1);
        let image = &cursor.images[0];
        assert_eq!(image.nominal_size, 24);
        assert_eq!(image.hotspot, (3, 3));
        assert_eq!(image.pixels.get_pixel(0, 0).0, [10, 20, 30, 255]);
    }

    #[test]
    fn matching_cursor_is_replaced_by_its_largest_size() {
        let bytes = xcursor_bytes(&[
            (24, 24, 24, 2, [255, 200, 0, 0]),
            (96, 96, 96, 8, [255, 200, 0, 0]),
        ]);
        let other = xcursor_bytes(&[(24, 24, 24, 2, [255, 0, 0, 200])]);
        let cursors = vec![
            parse_xcursor(&bytes).unwrap(),
            parse_xcursor(&other).unwrap(),
        ];

        let captured = solid(24, [200, 0, 0, 255]);
        let (cursor, frame) = matching_cursor(&cursors, &captured).unwrap();
        let best = sharper_frame(cursor, frame, captured.width()).unwrap();
        assert_eq!(best.pixels.width(), 96);
        assert_eq!(best.hotspot, (8, 8));

        let unknown = solid(24, [0, 255, 0, 255]);
        assert!(matching_cursor(&cursors, &unknown).is_none());
        let already_large = solid(96, [200, 0, 0, 255]);
        let (cursor, frame) = matching_cursor(&cursors, &already_large).unwrap();
        assert!(sharper_frame(cursor, frame, already_large.width()).is_none());
    }

    #[test]
    fn upgrade_rewrites_anchor_for_the_replacement() {
        let mut artwork = vec![PointerArtwork {
            artwork_id: "a".into(),
            image_data_base64: encode_png(&solid(24, [200, 0, 0, 255])).unwrap(),
            anchor_point: NormalizedPoint { x: 0.1, y: 0.1 },
            reference_width: 0.0125,
            reference_height: 0.0222,
            shape: None,
        }];
        let cursors = vec![parse_xcursor(&xcursor_bytes(&[
            (24, 24, 24, 2, [255, 200, 0, 0]),
            (96, 96, 96, 24, [255, 200, 0, 0]),
        ]))
        .unwrap()];
        let captured = decode_png(&artwork[0].image_data_base64).unwrap();
        let (cursor, frame) = matching_cursor(&cursors, &captured).unwrap();
        let best = sharper_frame(cursor, frame, captured.width()).unwrap();
        artwork[0].anchor_point = NormalizedPoint {
            x: f64::from(best.hotspot.0) / f64::from(best.pixels.width()),
            y: f64::from(best.hotspot.1) / f64::from(best.pixels.height()),
        };
        assert!((artwork[0].anchor_point.x - 0.25).abs() < 1e-9);
        assert!((artwork[0].reference_width - 0.0125).abs() < 1e-9);
    }

    #[test]
    fn matched_cursor_reports_its_shape_from_theme_aliases() {
        let mut cursor = parse_xcursor(&xcursor_bytes(&[(24, 24, 24, 2, [255, 1, 2, 3])])).unwrap();
        cursor.names = vec!["e29fe10f".into(), "hand2".into(), "pointer".into()];
        assert_eq!(cursor.shape(), Some(CursorShape::PointingHand));
    }
}
