//! One door for reading an image frame, whichever container holds it.
//!
//! PSF Guard reads FITS and XISF. Seiza decodes both into the same
//! [`seiza_fits::FitsImage`] with the same FITS-style header cards, so nothing
//! downstream — statistics, star detection, stretching, solving, stacking —
//! needs to know which one it opened. Route every read and every "is this a
//! frame?" test through here so a new container only has to be added once.

#[cfg(test)]
use seiza_fits::WriteHeaderCard;
use seiza_fits::{FitsImage, HeaderValue};
use std::path::Path;

/// File extensions PSF Guard treats as image frames, matched case-insensitively.
///
/// `fts` is the old 8.3-era spelling of `fits`; N.I.N.A. and PixInsight both
/// still read it, so a catalog that contains one should not silently lose it.
pub const IMAGE_EXTENSIONS: &[&str] = &["fits", "fit", "fts", "xisf"];

/// Failure to read an image, whatever container it came from.
#[derive(Debug)]
pub enum ImageError {
    Fits(seiza_fits::FitsError),
    Xisf(seiza_xisf::XisfError),
}

impl std::fmt::Display for ImageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Fits(error) => write!(f, "{error}"),
            Self::Xisf(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for ImageError {}

impl From<seiza_fits::FitsError> for ImageError {
    fn from(error: seiza_fits::FitsError) -> Self {
        Self::Fits(error)
    }
}

impl From<seiza_xisf::XisfError> for ImageError {
    fn from(error: seiza_xisf::XisfError) -> Self {
        Self::Xisf(error)
    }
}

/// Whether this extension names a frame container, with or without its
/// leading dot (`xisf` and `.xisf` both answer yes).
///
/// Config lists extensions dotted, so take both spellings rather than make
/// every caller remember which one it holds.
pub fn is_image_extension(extension: &str) -> bool {
    let extension = extension.strip_prefix('.').unwrap_or(extension);
    IMAGE_EXTENSIONS
        .iter()
        .any(|known| extension.eq_ignore_ascii_case(known))
}

/// Whether this filename carries an image extension.
pub fn has_image_extension(filename: &str) -> bool {
    is_image_path(Path::new(filename))
}

/// Whether this path carries an image extension.
///
/// Extension only: a scan reads thousands of names and cannot afford to open
/// each one to sniff its signature.
pub fn is_image_path(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(is_image_extension)
}

/// The filename without its image extension, for naming derived output.
///
/// A name that is not an image comes back whole.
pub fn strip_image_extension(filename: &str) -> &str {
    if !has_image_extension(filename) {
        return filename;
    }
    filename
        .rsplit_once('.')
        .map(|(base, _)| base)
        .unwrap_or(filename)
}

/// Read the header cards without decoding pixels.
pub fn read_header(path: &Path) -> Result<Vec<(String, HeaderValue)>, ImageError> {
    read_header_named(path, path)
}

/// [`read_header`] for a file whose own path does not name its container.
///
/// A remote upload streams into a temporary file inside a scanned image root.
/// That file must not carry a frame extension — a scan would pick it up
/// mid-write — so the decoder is chosen from the name the client declared
/// while the bytes are read from `path`.
pub fn read_header_named(
    path: &Path,
    declared: impl AsRef<Path>,
) -> Result<Vec<(String, HeaderValue)>, ImageError> {
    if seiza_xisf::is_xisf_path(declared.as_ref()) {
        Ok(seiza_xisf::read_header(path)?)
    } else {
        Ok(seiza_fits::read_header(path)?)
    }
}

/// Decode a frame's pixels and headers.
pub fn open(path: &Path) -> Result<FitsImage, ImageError> {
    if seiza_xisf::is_xisf_path(path) {
        Ok(seiza_xisf::open(path)?)
    } else {
        Ok(FitsImage::open(path)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn recognizes_every_image_extension_in_any_case() {
        for name in [
            "frame.fits",
            "frame.FITS",
            "frame.fit",
            "frame.FIT",
            "frame.fts",
            "frame.xisf",
            "frame.XISF",
            "frame.Xisf",
        ] {
            assert!(has_image_extension(name), "{name} should be an image");
        }
    }

    #[test]
    fn rejects_sidecars_and_extensionless_names() {
        for name in ["frame.json", "frame.txt", "frame.png", "frame", "frame."] {
            assert!(!has_image_extension(name), "{name} should not be an image");
        }
    }

    #[test]
    fn strips_only_image_extensions() {
        assert_eq!(strip_image_extension("frame.fits"), "frame");
        assert_eq!(strip_image_extension("frame.XISF"), "frame");
        assert_eq!(
            strip_image_extension("m31.2026-01-01.fit"),
            "m31.2026-01-01"
        );
        assert_eq!(strip_image_extension("notes.json"), "notes.json");
    }

    /// A sample 4x3 mono XISF light frame, written by the same XISF writer a
    /// reader in the wild would meet. Generating it beats checking in a blob:
    /// the fixture cannot drift away from the format the crate speaks.
    fn write_sample_xisf(directory: &Path, name: &str) -> PathBuf {
        let pixels: Vec<f32> = (0..12).map(|index| index as f32 / 11.0).collect();
        let path = directory.join(name);
        seiza_xisf::write_f32_image(
            &path,
            4,
            3,
            seiza_fits::F32ImageData::Mono(&pixels),
            &[
                WriteHeaderCard::new("IMAGETYP", HeaderValue::String("LIGHT".into())),
                WriteHeaderCard::new("OBJECT", HeaderValue::String("M31".into())),
                WriteHeaderCard::new("FILTER", HeaderValue::String("Ha".into())),
                WriteHeaderCard::new("EXPTIME", HeaderValue::Float(300.0)),
            ],
        )
        .expect("sample XISF should write");
        path
    }

    fn write_temp(name: &str, bytes: &[u8]) -> (tempfile::TempDir, PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        (directory, path)
    }

    #[test]
    fn opens_an_xisf_frame_through_the_shared_reader() {
        let directory = tempfile::tempdir().unwrap();
        let path = write_sample_xisf(directory.path(), "light.xisf");
        let image = open(&path).expect("XISF should decode");
        assert_eq!((image.width, image.height, image.planes), (4, 3, 1));
        assert!(
            matches!(image.pixels, seiza_fits::Pixels::F32(ref samples) if samples.len() == 12)
        );
    }

    #[test]
    fn reads_xisf_headers_without_decoding_pixels() {
        let directory = tempfile::tempdir().unwrap();
        let path = write_sample_xisf(directory.path(), "light.xisf");
        let headers = read_header(&path).expect("XISF headers should parse");
        let text = |keyword: &str| {
            headers
                .iter()
                .find(|(name, _)| name == keyword)
                .and_then(|(_, value)| value.as_str())
                .map(str::to_string)
        };
        assert_eq!(text("OBJECT").as_deref(), Some("M31"));
        assert_eq!(text("FILTER").as_deref(), Some("Ha"));
        assert_eq!(text("IMAGETYP").as_deref(), Some("LIGHT"));
        assert_eq!(
            headers
                .iter()
                .find(|(name, _)| name == "EXPTIME")
                .and_then(|(_, value)| value.as_f64()),
            Some(300.0)
        );
    }

    #[test]
    fn xisf_frames_reach_the_shared_astrometry_header_reader() {
        let directory = tempfile::tempdir().unwrap();
        let path = write_sample_xisf(directory.path(), "light.xisf");
        let headers = crate::astrometry_headers::FitsAstrometryHeaders::from_path(&path)
            .expect("XISF headers should normalize");
        assert_eq!(
            headers.object_name.map(|value| value.value),
            Some("M31".into())
        );
        assert_eq!(headers.width.map(|value| value.value), Some(4));
        assert_eq!(headers.height.map(|value| value.value), Some(3));
    }

    /// A remote upload streams into an extensionless temporary inside a
    /// scanned image root. The decoder has to come from the declared name, or
    /// the temporary would need a frame extension and a concurrent scan would
    /// pick it up mid-write.
    #[test]
    fn a_declared_name_picks_the_decoder_for_an_extensionless_file() {
        let directory = tempfile::tempdir().unwrap();
        let written = write_sample_xisf(directory.path(), "light.xisf");
        let bare = directory.path().join(".tmpAbC123");
        std::fs::rename(&written, &bare).unwrap();

        assert!(
            !is_image_path(&bare),
            "the temporary must stay invisible to scans"
        );
        assert!(
            read_header(&bare).is_err(),
            "without the declared name this is read as FITS"
        );

        let headers = read_header_named(&bare, "light.xisf").expect("declared name should decode");
        assert_eq!(
            headers
                .iter()
                .find(|(name, _)| name == "OBJECT")
                .and_then(|(_, value)| value.as_str()),
            Some("M31")
        );
    }

    #[test]
    fn reports_which_container_failed() {
        let (_directory, path) = write_temp("broken.xisf", b"not an xisf file at all");
        let error = open(&path).expect_err("a non-XISF body must fail");
        assert!(matches!(error, ImageError::Xisf(_)), "{error}");

        let (_directory, path) = write_temp("broken.fits", b"not a fits file at all");
        let error = open(&path).expect_err("a non-FITS body must fail");
        assert!(matches!(error, ImageError::Fits(_)), "{error}");
    }
}
