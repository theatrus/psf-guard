//! PSF Guard's own readers take a PixInsight XISF frame through
//! `psf_studio_core::image_io`, on the same scale as a camera frame.

use psf_guard::astrometry_headers::FitsAstrometryHeaders;
use psf_guard::FitsImage;
use seiza_fits::{HeaderValue, WriteHeaderCard};
use std::path::{Path, PathBuf};

/// A 4 x 3 XISF light of M31 through Ha, its samples spread over 0..=1, as
/// PixInsight writes a normalized frame.
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

#[test]
fn xisf_frames_reach_the_shared_astrometry_header_reader() {
    let directory = tempfile::tempdir().unwrap();
    let path = write_sample_xisf(directory.path(), "light.xisf");
    let headers = FitsAstrometryHeaders::from_path(&path).expect("XISF headers should normalize");
    assert_eq!(
        headers.object_name.map(|value| value.value),
        Some("M31".into())
    );
    assert_eq!(headers.width.map(|value| value.value), Some(4));
    assert_eq!(headers.height.map(|value| value.value), Some(3));
}

/// The measured value the grader actually compares across frames. Before
/// the conversion this read about 0.01 against a camera frame's thousands.
#[test]
fn a_normalized_frame_reports_adu_a_camera_frame_can_be_compared_with() {
    let directory = tempfile::tempdir().unwrap();
    let path = write_sample_xisf(directory.path(), "light.xisf");

    let frame = FitsImage::from_file(&path).unwrap();
    let statistics = frame.calculate_basic_statistics();
    let median_adu = frame.stored_to_adu(statistics.median);
    assert!(
        (1000.0..65536.0).contains(&median_adu),
        "median should read as 16-bit ADU, got {median_adu}"
    );
}
