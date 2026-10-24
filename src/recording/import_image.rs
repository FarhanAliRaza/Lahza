//! Still-image imports share content detection and decoder fallback.
use super::video::VideoError;
use image::{DynamicImage, ImageError, ImageFormat, ImageReader};
use std::{path::Path, process::Command};

pub(crate) fn decode(source: &Path) -> Result<DynamicImage, VideoError> {
    let reader = ImageReader::open(source)?.with_guessed_format()?;
    if reader.format().is_none() {
        return Err(VideoError::InvalidMedia("unsupported image format".into()));
    }
    match reader.decode() {
        Ok(image) => Ok(image),
        // The image crate recognizes AVIF, but its default features do not
        // include an AVIF decoder. FFmpeg already supplies that decoder for
        // video; use it for a single still frame without requiring duration.
        Err(ImageError::Unsupported(_)) => decode_with_ffmpeg(source),
        Err(error) => Err(VideoError::InvalidMedia(format!("could not read image: {error}"))),
    }
}

fn decode_with_ffmpeg(source: &Path) -> Result<DynamicImage, VideoError> {
    let output = Command::new("ffmpeg")
        .args(["-v", "error", "-nostdin", "-i"])
        .arg(source)
        .args([
            "-frames:v", "1", "-an", "-c:v", "png", "-pix_fmt", "rgba",
            "-f", "image2pipe", "pipe:1",
        ])
        .output()?;
    if !output.status.success() {
        return Err(VideoError::InvalidMedia(format!(
            "could not read image: {}", String::from_utf8_lossy(&output.stderr).trim(),
        )));
    }
    image::load_from_memory_with_format(&output.stdout, ImageFormat::Png)
        .map_err(|error| VideoError::InvalidMedia(format!("could not read image: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, path::PathBuf};

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!("lahza-import-image-{}", uuid::Uuid::new_v4()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn sniffed_png_imports_without_an_extension_and_preserves_alpha() {
        let fixture = Fixture::new();
        let original = image::RgbaImage::from_fn(8, 6, |x, y| {
            image::Rgba([x as u8 * 30, y as u8 * 40, 50, (x + y) as u8 * 15])
        });
        for name in ["image", "image.jpg"] {
            let path = fixture.0.join(name);
            original.save_with_format(&path, ImageFormat::Png).unwrap();
            assert_eq!(decode(&path).unwrap().to_rgba8(), original);
        }
    }

    #[test]
    fn unsupported_and_corrupt_inputs_remain_image_errors() {
        let fixture = Fixture::new();
        for name in ["not-an-image.mp4", "corrupt.png"] {
            let path = fixture.0.join(name);
            fs::write(&path, b"not image data").unwrap();
            assert!(matches!(decode(&path), Err(VideoError::InvalidMedia(_))));
        }
    }

    #[test]
    fn avif_decodes_as_a_still_without_a_duration() {
        let fixture = Fixture::new();
        let source = fixture.0.join("source.png");
        let avif = fixture.0.join("photo.avif");
        image::RgbaImage::from_pixel(64, 48, image::Rgba([10, 180, 20, 255]))
            .save(&source).unwrap();
        let output = Command::new("ffmpeg")
            .args(["-v", "error", "-nostdin", "-i"])
            .arg(&source)
            .args([
                "-frames:v", "1", "-c:v", "libaom-av1", "-cpu-used", "8",
                "-crf", "0", "-still-picture", "1", "-pix_fmt", "yuv444p",
            ])
            .arg(&avif).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        let decoded = decode(&avif).unwrap().to_rgba8();
        assert_eq!(decoded.dimensions(), (64, 48));
        let center = decoded.get_pixel(32, 24);
        for (actual, expected) in center.0.into_iter().zip([10u8, 180, 20, 255]) {
            assert!(actual.abs_diff(expected) <= 3, "decoded color: {center:?}");
        }
        // Content detection must recognize AVIF even when the suffix is lost.
        let renamed = fixture.0.join("photo");
        fs::rename(avif, &renamed).unwrap();
        assert_eq!(decode(&renamed).unwrap().to_rgba8(), decoded);
    }
}
