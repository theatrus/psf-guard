//! Saving a rendered picture to a file.

use crate::cpu::CpuImage;
use std::path::Path;

/// The file kinds an export writes, by extension, with what each holds.
pub const FORMATS: [(&str, &str); 4] = [
    ("tif", "TIFF, 16 bits per channel"),
    ("png", "PNG, 16 bits per channel"),
    ("fits", "FITS, 32-bit floats"),
    ("xisf", "XISF, 32-bit floats"),
];

/// Writes `image` to `path` in the format its extension names, through
/// Seiza: TIFF or PNG at 16 bits per channel, clamped to 0 to 1, or FITS
/// or XISF as 32-bit floats as they are, FITS rows top-down.
pub fn save(path: &Path, image: &CpuImage) -> Result<(), String> {
    seiza_image_io::save_f32(
        path,
        image.width as usize,
        image.height as usize,
        seiza_fits::F32ImageData::RgbInterleaved(&image.rgb),
        &seiza_image_io::SaveOptions {
            fits_top_down: true,
            ..Default::default()
        },
    )
    .map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tiff_keeps_sixteen_bits() {
        let folder =
            std::env::temp_dir().join(format!("studio-canvas-export-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let path = folder.join("picture.tif");
        // Two values one 16-bit step apart, which 8 bits could not tell apart.
        let step = 1.0 / 65535.0;
        let image = CpuImage {
            width: 2,
            height: 1,
            rgb: vec![0.5, 0.5, 0.5, 0.5 + step, 0.5 + step, 0.5 + step],
        };
        save(&path, &image).unwrap();
        let read = seiza_image_io::open(&path).unwrap();
        let samples = &read.samples;
        assert!((samples[0] - 0.5).abs() < step);
        assert!(samples[3] > samples[0]);
        std::fs::remove_dir_all(&folder).unwrap();
    }
}
