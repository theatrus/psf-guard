//! The file format generated previews are cached in.
//!
//! PNG is exact and the default. JPEG trades that for size: a night's worth
//! of previews is the largest thing PSF Guard writes, and an operator short of
//! disk may prefer a cache a fraction of the size.
//!
//! The trade is real and worth stating plainly. JPEG is lossy in exactly the
//! places this tool asks you to look — it smooths the faint, high-frequency
//! detail that noise, hot pixels, and marginal stars live in, so a frame can
//! look cleaner in the viewer than it is on disk. Grading measurements are
//! taken from the FITS and never from a preview, so nothing scored changes;
//! what changes is what your eye is given to check the score against.

use anyhow::{Context, Result};
use image::{codecs::jpeg::JpegEncoder, ColorType, ImageEncoder};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::BufWriter,
    path::{Path, PathBuf},
};

/// Quality when none is configured. High enough that the artifacts stay out
/// of the way at normal viewing scales, and still a small fraction of PNG.
pub const DEFAULT_JPEG_QUALITY: u8 = 88;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PreviewFormat {
    /// Exact by default: a viewer should not quietly show something other
    /// than what the pixels say unless someone asked for that trade.
    #[default]
    Png,
    Jpeg,
}

impl PreviewFormat {
    /// Parse a configured name, listing what is accepted when it is not one.
    pub fn parse(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "png" => Ok(Self::Png),
            "jpeg" | "jpg" => Ok(Self::Jpeg),
            other => anyhow::bail!("unknown preview format '{other}'; use png or jpeg"),
        }
    }

    /// File extension. Formats differ here so both can sit in the cache at
    /// once: changing the setting cannot serve one as the other, and reverting
    /// finds the old artifacts still valid.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
        }
    }

    pub fn content_type(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
        }
    }

    /// The format an artifact already on disk is in, read from its name
    /// rather than from configuration — what was written is what must be
    /// served, whatever the setting says now.
    pub fn of_path(path: &Path) -> Self {
        match path.extension().and_then(|value| value.to_str()) {
            Some(extension) if extension.eq_ignore_ascii_case("jpg") => Self::Jpeg,
            Some(extension) if extension.eq_ignore_ascii_case("jpeg") => Self::Jpeg,
            _ => Self::Png,
        }
    }

    /// Swap a path's extension for this format's.
    pub fn with_extension(self, path: &Path) -> PathBuf {
        path.with_extension(self.extension())
    }
}

/// How previews are encoded: the format, and JPEG's quality when it applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreviewEncoding {
    pub format: PreviewFormat,
    pub jpeg_quality: u8,
}

impl Default for PreviewEncoding {
    fn default() -> Self {
        Self {
            format: PreviewFormat::default(),
            jpeg_quality: DEFAULT_JPEG_QUALITY,
        }
    }
}

impl PreviewEncoding {
    pub fn png() -> Self {
        Self::default()
    }

    pub fn jpeg(quality: u8) -> Self {
        Self {
            format: PreviewFormat::Jpeg,
            // Below about 50 the ringing around stars is bad enough to read as
            // a feature; above 100 is not a thing.
            jpeg_quality: quality.clamp(50, 100),
        }
    }

    pub fn extension(self) -> &'static str {
        self.format.extension()
    }

    /// Write one already-rendered 8-bit image.
    ///
    /// PNG keeps its best compression: the cost is CPU at generation time,
    /// paid once, against bytes on disk kept for as long as the cache lives.
    pub fn write(
        self,
        path: &Path,
        samples: &[u8],
        width: u32,
        height: u32,
        color: ColorType,
    ) -> Result<()> {
        let file = File::create(path)
            .with_context(|| format!("Failed to create output file: {}", path.display()))?;
        let writer = BufWriter::new(file);
        match self.format {
            PreviewFormat::Png => {
                use image::codecs::png::{CompressionType, FilterType, PngEncoder};
                PngEncoder::new_with_quality(writer, CompressionType::Best, FilterType::Adaptive)
                    .write_image(samples, width, height, color.into())
                    .with_context(|| format!("Failed to write PNG {}", path.display()))
            }
            PreviewFormat::Jpeg => {
                // JPEG has no greyscale-with-alpha or 16-bit form; everything
                // reaching here is L8 or RGB8, which it does have.
                JpegEncoder::new_with_quality(writer, self.jpeg_quality)
                    .write_image(samples, width, height, color.into())
                    .with_context(|| format!("Failed to write JPEG {}", path.display()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_configured_name_is_read_generously_and_refused_clearly() {
        assert_eq!(PreviewFormat::parse("PNG").unwrap(), PreviewFormat::Png);
        assert_eq!(PreviewFormat::parse(" jpg ").unwrap(), PreviewFormat::Jpeg);
        assert_eq!(PreviewFormat::parse("jpeg").unwrap(), PreviewFormat::Jpeg);
        let error = PreviewFormat::parse("webp").unwrap_err().to_string();
        assert!(error.contains("png or jpeg"), "{error}");
    }

    #[test]
    fn an_artifact_is_served_as_what_it_was_written_as() {
        // Changing the setting must not relabel files already on disk.
        assert_eq!(
            PreviewFormat::of_path(Path::new("/cache/a.png")),
            PreviewFormat::Png
        );
        assert_eq!(
            PreviewFormat::of_path(Path::new("/cache/a.jpg")),
            PreviewFormat::Jpeg
        );
        assert_eq!(
            PreviewFormat::of_path(Path::new("/cache/a.JPEG")),
            PreviewFormat::Jpeg
        );
    }

    #[test]
    fn the_two_formats_never_share_a_path() {
        // So switching the setting misses rather than serving the wrong bytes,
        // and switching back finds the originals intact.
        let key = Path::new("/cache/previews/42_screen_stretch");
        assert_ne!(
            PreviewFormat::Png.with_extension(key),
            PreviewFormat::Jpeg.with_extension(key)
        );
    }

    #[test]
    fn jpeg_quality_stays_in_a_range_that_can_be_looked_at() {
        assert_eq!(PreviewEncoding::jpeg(10).jpeg_quality, 50);
        assert_eq!(PreviewEncoding::jpeg(200).jpeg_quality, 100);
        assert_eq!(PreviewEncoding::jpeg(85).jpeg_quality, 85);
    }

    /// A stretched sub as the viewer sees one: smooth sky, real noise, a few
    /// stars. Not flat — PNG would crush that and prove nothing — and not
    /// pure noise either, which is JPEG's worst case and equally unlike the
    /// thing being cached.
    fn stretched_frame(width: u32, height: u32) -> Vec<u8> {
        (0..width * height)
            .map(|index| {
                let (x, y) = (index % width, index / width);
                let mut hash = index.wrapping_mul(0x9E37_79B1);
                hash ^= hash >> 15;
                hash = hash.wrapping_mul(0x85EB_CA6B);
                hash ^= hash >> 13;
                let noise = (hash & 0x1F) as f32 - 15.0;
                let gradient = 40.0 + (x as f32 + y as f32) * 0.05;
                let star = STAR_SITES
                    .iter()
                    .map(|&(sx, sy)| {
                        let (dx, dy) = (x as f32 - sx, y as f32 - sy);
                        200.0 * (-(dx * dx + dy * dy) / 8.0).exp()
                    })
                    .sum::<f32>();
                (gradient + noise + star).clamp(0.0, 255.0) as u8
            })
            .collect()
    }

    const STAR_SITES: [(f32, f32); 6] = [
        (40.0, 60.0),
        (120.0, 30.0),
        (200.0, 150.0),
        (70.0, 190.0),
        (170.0, 90.0),
        (230.0, 220.0),
    ];

    #[test]
    fn jpeg_is_smaller_than_png_for_the_same_frame() {
        // The whole reason for the option.
        let directory = tempfile::tempdir().unwrap();
        let (width, height) = (256u32, 256u32);
        let samples = stretched_frame(width, height);

        let png = directory.path().join("a.png");
        let jpeg = directory.path().join("a.jpg");
        PreviewEncoding::png()
            .write(&png, &samples, width, height, ColorType::L8)
            .unwrap();
        PreviewEncoding::jpeg(DEFAULT_JPEG_QUALITY)
            .write(&jpeg, &samples, width, height, ColorType::L8)
            .unwrap();

        let (png_bytes, jpeg_bytes) = (
            std::fs::metadata(&png).unwrap().len(),
            std::fs::metadata(&jpeg).unwrap().len(),
        );
        println!("png {png_bytes} bytes, jpeg {jpeg_bytes} bytes");
        assert!(
            jpeg_bytes < png_bytes,
            "jpeg {jpeg_bytes} should be smaller than png {png_bytes}"
        );
    }

    #[test]
    fn colour_previews_encode_in_both_formats() {
        let directory = tempfile::tempdir().unwrap();
        let (width, height) = (16u32, 16u32);
        let samples = vec![120u8; (width * height * 3) as usize];
        for encoding in [PreviewEncoding::png(), PreviewEncoding::jpeg(88)] {
            let path = directory.path().join(format!("c.{}", encoding.extension()));
            encoding
                .write(&path, &samples, width, height, ColorType::Rgb8)
                .unwrap();
            let decoded = image::open(&path).unwrap();
            assert_eq!(decoded.width(), width);
            assert_eq!(decoded.height(), height);
        }
    }
}
