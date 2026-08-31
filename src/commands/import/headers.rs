//! Per-frame FITS header extraction for the import pipeline.
//!
//! Only headers are read — never pixel data — so scanning thousands of frames
//! stays I/O bound. Field names follow N.I.N.A.'s FITS writer.

use crate::astrometry_headers::{parse_dec_deg, parse_ra_deg};
use seiza_fits::HeaderValue;
use std::path::{Path, PathBuf};

/// Everything import needs to know about one FITS file, straight from its
/// headers. All fields except `path` are optional: frames with missing
/// headers still import, they just group more coarsely.
#[derive(Debug, Clone, Default)]
pub struct FrameMeta {
    pub path: PathBuf,
    /// False when the FITS header could not be parsed at all; such frames are
    /// counted and skipped rather than imported as empty rows.
    pub readable: bool,
    /// IMAGETYP, uppercased ("LIGHT", "DARK", "FLAT", "BIAS", ...).
    pub image_type: Option<String>,
    /// The output of processing (an integration master or a PixInsight
    /// calibrated/registered intermediate) rather than an acquisition.
    /// Skipped by import unless explicitly included.
    pub processed: bool,
    pub object: Option<String>,
    pub filter: Option<String>,
    /// DATE-OBS as epoch seconds (UTC).
    pub timestamp: Option<i64>,
    /// DATE-OBS original text, for the metadata JSON.
    pub date_obs: Option<String>,
    /// DATE-LOC original text. N.I.N.A. directory templates use this local
    /// value for observing-night (`DATEMINUS12`) folders.
    pub date_local: Option<String>,
    pub exposure_s: Option<f64>,
    pub gain: Option<i64>,
    pub offset: Option<i64>,
    pub binning_x: Option<i64>,
    pub binning_y: Option<i64>,
    pub readout_mode: Option<i64>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub channels: Option<i64>,
    pub bayer_pattern: Option<String>,
    pub bayer_x_offset: Option<i64>,
    pub bayer_y_offset: Option<i64>,
    pub ra_deg: Option<f64>,
    pub dec_deg: Option<f64>,
    pub telescope: Option<String>,
    pub camera: Option<String>,
    pub focal_length_mm: Option<f64>,
    pub camera_temp: Option<f64>,
    pub camera_target_temp: Option<f64>,
    pub focuser_position: Option<i64>,
    pub focuser_temp: Option<f64>,
    pub rotator_position: Option<f64>,
    pub pier_side: Option<String>,
    pub airmass: Option<f64>,
}

impl FrameMeta {
    /// True when the frame should be imported as an acquired light frame.
    /// Calibration frames (dark/flat/bias) have no place in a scheduler DB.
    /// A missing IMAGETYP is treated as a light: plenty of processed archives
    /// strip it, and lights are what people point the importer at.
    pub fn is_light(&self) -> bool {
        match &self.image_type {
            None => true,
            Some(t) => t.contains("LIGHT"),
        }
    }

    pub fn basename(&self) -> String {
        self.path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

/// Read one frame's headers. Unreadable files yield a `FrameMeta` with only
/// `path` set (the caller decides whether to skip or report them).
pub fn read_frame_meta(path: &Path) -> FrameMeta {
    read_frame_meta_named(path, path)
}

/// [`read_frame_meta`] for a file whose own path does not name its container,
/// such as the temporary a remote upload streams into. `declared` picks the
/// decoder; `path` supplies the bytes.
pub fn read_frame_meta_named(path: &Path, declared: &Path) -> FrameMeta {
    let mut meta = FrameMeta {
        path: path.to_path_buf(),
        ..Default::default()
    };
    let Ok(headers) = crate::image_io::read_header_named(path, declared) else {
        return meta;
    };
    meta.readable = true;
    meta.processed = crate::image_io::is_processing_artifact(path, declared, &headers);

    let find = |names: &[&str]| -> Option<&HeaderValue> {
        names.iter().find_map(|wanted| {
            headers
                .iter()
                .find(|(name, _)| name.eq_ignore_ascii_case(wanted))
                .map(|(_, value)| value)
        })
    };
    let text = |names: &[&str]| -> Option<String> {
        find(names)
            .and_then(value_text)
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string)
    };
    // A header that parses to NaN or an infinity is a header that did not
    // record anything, and is read as absent rather than as a reading.
    // Matching treats an absent value as "cannot rule anything out"; a
    // non-finite one left in place would compare false against everything, or
    // — once the comparison moved to a crate that reads non-finite as unknown
    // — against nothing, which silently paired a light of no known
    // temperature with a dark of any temperature at all.
    let f64_of = |names: &[&str]| {
        find(names)
            .and_then(HeaderValue::as_f64)
            .filter(|value| value.is_finite())
    };
    let i64_of = |names: &[&str]| {
        find(names).and_then(|v| {
            v.as_i64()
                .or_else(|| v.as_f64().filter(|f| f.fract() == 0.0).map(|f| f as i64))
        })
    };
    let coordinate = |names: &[&str], parse: fn(&str) -> Option<f64>| -> Option<f64> {
        names.iter().find_map(|wanted| {
            let value = find(&[*wanted])?;
            match value {
                HeaderValue::Integer(v) => parse(&v.to_string()),
                HeaderValue::Float(v) => parse(&v.to_string()),
                HeaderValue::String(v) | HeaderValue::Raw(v) => parse(v),
                HeaderValue::Logical(_) => None,
            }
        })
    };

    meta.image_type = text(&["IMAGETYP", "FRAME"]).map(|t| t.to_uppercase());
    meta.object = text(&["OBJECT"]);
    meta.filter = text(&["FILTER", "FILTERNAME"]);
    meta.date_obs = text(&["DATE-OBS", "DATE-LOC"]);
    meta.date_local = text(&["DATE-LOC"]);
    meta.timestamp = meta.date_obs.as_deref().and_then(parse_fits_datetime);
    meta.exposure_s = f64_of(&["EXPTIME", "EXPOSURE"]).filter(|v| *v > 0.0);
    meta.gain = i64_of(&["GAIN"]);
    meta.offset = i64_of(&["OFFSET"]);
    meta.binning_x = i64_of(&["XBINNING"]).filter(|v| *v > 0);
    meta.binning_y = i64_of(&["YBINNING"]).filter(|v| *v > 0);
    // N.I.N.A. writes READOUTM as the mode's display *name*; only a numeric
    // value can round-trip into TS's integer column.
    meta.readout_mode = i64_of(&["READOUTM", "READOUT", "READMODE"]);
    meta.width = i64_of(&["NAXIS1"]).filter(|v| *v > 0);
    meta.height = i64_of(&["NAXIS2"]).filter(|v| *v > 0);
    meta.channels = i64_of(&["NAXIS3"]).filter(|v| *v > 0).or(Some(1));
    meta.bayer_pattern = text(&["BAYERPAT"]).map(|value| value.to_ascii_uppercase());
    meta.bayer_x_offset = i64_of(&["XBAYROFF"]);
    meta.bayer_y_offset = i64_of(&["YBAYROFF"]);
    meta.ra_deg = coordinate(&["RA", "OBJCTRA", "OBJRA", "TELRA"], parse_ra_deg);
    meta.dec_deg = coordinate(&["DEC", "OBJCTDEC", "OBJDEC", "TELDEC"], parse_dec_deg);
    meta.telescope = text(&["TELESCOP"]);
    meta.camera = text(&["INSTRUME"]);
    meta.focal_length_mm = f64_of(&["FOCALLEN", "FOCAL"]).filter(|v| *v > 0.0);
    meta.camera_temp = f64_of(&["CCD-TEMP", "CCDTEMP"]);
    meta.camera_target_temp = f64_of(&["SET-TEMP", "SETTEMP"]);
    meta.focuser_position = i64_of(&["FOCPOS", "FOCUSPOS"]);
    meta.focuser_temp = f64_of(&["FOCTEMP", "FOCUSTEM"]);
    meta.rotator_position = f64_of(&["ROTATANG", "ROTATOR"]);
    meta.pier_side = text(&["PIERSIDE"]);
    meta.airmass = f64_of(&["AIRMASS"]).filter(|v| *v >= 1.0);
    meta
}

fn value_text(value: &HeaderValue) -> Option<&str> {
    match value {
        HeaderValue::String(v) | HeaderValue::Raw(v) => Some(v.trim()),
        _ => None,
    }
}

/// Parse a FITS DATE-OBS style timestamp into epoch seconds (assumed UTC, as
/// N.I.N.A. writes DATE-OBS).
pub(crate) fn parse_fits_datetime(s: &str) -> Option<i64> {
    let s = s.trim();
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f") {
        return Some(dt.and_utc().timestamp());
    }
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(dt.timestamp());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn light_frame_detection() {
        let mut meta = FrameMeta::default();
        assert!(meta.is_light(), "missing IMAGETYP treated as light");
        meta.image_type = Some("LIGHT FRAME".into());
        assert!(meta.is_light());
        meta.image_type = Some("LIGHT".into());
        assert!(meta.is_light());
        for cal in ["DARK", "FLAT", "BIAS", "DARK FRAME"] {
            meta.image_type = Some(cal.into());
            assert!(!meta.is_light(), "{cal} must not import");
        }
    }

    #[test]
    fn parses_nina_timestamps() {
        // N.I.N.A. writes 7 fractional digits and no zone designator.
        assert!(parse_fits_datetime("2026-07-01T05:40:25.6971960").is_some());
        assert!(parse_fits_datetime("2024-01-15T22:00:00Z").is_some());
        assert!(parse_fits_datetime("not a date").is_none());
    }
}
