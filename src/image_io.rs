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

/// The scale a normalized frame is placed on: full-well for 16-bit data.
///
/// PSF Guard compares background and flux across frames in physical ADU, and
/// almost every frame it meets is 16-bit camera data. A PixInsight float
/// frame that declares itself normalized has no ADU of its own, so the
/// nearest honest thing is to put it on the same scale as its neighbours.
const NORMALIZED_FULL_SCALE: f32 = 65535.0;

/// Decode a frame's pixels and headers.
///
/// A float XISF frame that declares `bounds="0:1"` is placed on a 16-bit
/// scale on the way through. PixInsight normalizes float images, so such a
/// frame's samples run 0..1 where a camera frame's run in the thousands, and
/// leaving them alone would make every cross-frame background and flux
/// comparison meaningless — quality screening would read the normalized frame
/// as a near-black outlier and its neighbours as blown. Only an exact `0:1`
/// is converted; seiza declines any other declared range, because writers
/// disagree about what it means.
pub fn open(path: &Path) -> Result<FitsImage, ImageError> {
    if seiza_xisf::is_xisf_path(path) {
        let mut read = seiza_xisf::read_image(path)?;
        read.rescale_normalized_to(NORMALIZED_FULL_SCALE);
        Ok(read.image)
    } else {
        Ok(FitsImage::open(path)?)
    }
}

/// Open a frame as linear samples for calibration and stacking, with the same
/// normalization [`open`] applies.
///
/// Stacking compares frames against a reference, so one normalized frame
/// among camera frames skews normalization and rejection for the whole group.
/// A no-op for FITS and for any XISF that does not declare itself normalized,
/// which is why every stacking read goes through here rather than picking and
/// choosing.
pub fn open_linear_frame(
    path: impl AsRef<Path>,
) -> Result<seiza_stacking::FitsFrame, seiza_stacking::Error> {
    let mut frame = seiza_stacking::FitsFrame::open(path)?;
    if frame.bounds == Some((0.0, 1.0)) {
        for sample in &mut frame.image.data {
            *sample *= NORMALIZED_FULL_SCALE;
        }
        frame.bounds = Some((0.0, f64::from(NORMALIZED_FULL_SCALE)));
    }
    Ok(frame)
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
    ///
    /// The samples span 0..1, so the writer declares `bounds="0:1"` and the
    /// frame reads back as a normalized one.
    fn write_sample_xisf(directory: &Path, name: &str) -> PathBuf {
        write_xisf_spanning(directory, name, 0.0, 1.0)
    }

    /// The same frame with its samples spread evenly over `low..=high`.
    fn write_xisf_spanning(directory: &Path, name: &str, low: f32, high: f32) -> PathBuf {
        let pixels: Vec<f32> = (0..12)
            .map(|index| low + (high - low) * index as f32 / 11.0)
            .collect();
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

    /// The finding this exists for: PixInsight normalizes float images, so a
    /// frame declaring `0:1` has samples four orders of magnitude below a
    /// camera frame's, and every cross-frame ADU comparison built on it is
    /// meaningless.
    #[test]
    fn a_normalized_xisf_frame_lands_on_a_sixteen_bit_scale() {
        let directory = tempfile::tempdir().unwrap();
        let path = write_sample_xisf(directory.path(), "light.xisf");

        let seiza_fits::Pixels::F32(samples) = open(&path).unwrap().pixels else {
            panic!("expected float samples");
        };
        assert_eq!(samples.first().copied(), Some(0.0));
        assert_eq!(samples.last().copied(), Some(65535.0));
    }

    /// Only an exact `0:1` is treated as normalized. Any other declared range
    /// is ambiguous — this crate's own stack output declares the observed
    /// minimum and maximum — so the samples must survive untouched.
    #[test]
    fn a_physical_xisf_frame_passes_through_untouched() {
        let directory = tempfile::tempdir().unwrap();
        let path = write_xisf_spanning(directory.path(), "light.xisf", 100.0, 30000.0);

        let seiza_fits::Pixels::F32(samples) = open(&path).unwrap().pixels else {
            panic!("expected float samples");
        };
        assert_eq!(samples.first().copied(), Some(100.0));
        assert_eq!(samples.last().copied(), Some(30000.0));
    }

    /// The measured value the grader actually compares across frames. Before
    /// the conversion this read about 0.01 against a camera frame's thousands.
    #[test]
    fn a_normalized_frame_reports_adu_a_camera_frame_can_be_compared_with() {
        let directory = tempfile::tempdir().unwrap();
        let path = write_sample_xisf(directory.path(), "light.xisf");

        let frame = crate::image_analysis::FitsImage::from_file(&path).unwrap();
        let statistics = frame.calculate_basic_statistics();
        let median_adu = frame.stored_to_adu(statistics.median);
        assert!(
            (1000.0..65536.0).contains(&median_adu),
            "median should read as 16-bit ADU, got {median_adu}"
        );
    }

    /// Stacking normalizes every frame against a reference, so one normalized
    /// frame among camera frames would skew the whole group.
    #[test]
    fn the_stacking_reader_applies_the_same_conversion() {
        let directory = tempfile::tempdir().unwrap();
        let normalized = write_sample_xisf(directory.path(), "normalized.xisf");
        let frame = open_linear_frame(&normalized).unwrap();
        assert_eq!(frame.image.data.first().copied(), Some(0.0));
        assert_eq!(frame.image.data.last().copied(), Some(65535.0));

        let physical = write_xisf_spanning(directory.path(), "physical.xisf", 100.0, 30000.0);
        let frame = open_linear_frame(&physical).unwrap();
        assert_eq!(frame.image.data.first().copied(), Some(100.0));
        assert_eq!(frame.image.data.last().copied(), Some(30000.0));
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
