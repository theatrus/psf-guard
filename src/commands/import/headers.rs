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
    pub object: Option<String>,
    pub filter: Option<String>,
    /// DATE-OBS as epoch seconds (UTC).
    pub timestamp: Option<i64>,
    /// DATE-OBS original text, for the metadata JSON.
    pub date_obs: Option<String>,
    pub exposure_s: Option<f64>,
    pub gain: Option<i64>,
    pub offset: Option<i64>,
    pub binning_x: Option<i64>,
    pub binning_y: Option<i64>,
    pub readout_mode: Option<i64>,
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
    let mut meta = FrameMeta {
        path: path.to_path_buf(),
        ..Default::default()
    };
    let Ok(headers) = seiza_fits::read_header(path) else {
        return meta;
    };
    meta.readable = true;

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
    let f64_of = |names: &[&str]| find(names).and_then(HeaderValue::as_f64);
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
    meta.timestamp = meta.date_obs.as_deref().and_then(parse_fits_datetime);
    meta.exposure_s = f64_of(&["EXPTIME", "EXPOSURE"]).filter(|v| *v > 0.0);
    meta.gain = i64_of(&["GAIN"]);
    meta.offset = i64_of(&["OFFSET"]);
    meta.binning_x = i64_of(&["XBINNING"]).filter(|v| *v > 0);
    meta.binning_y = i64_of(&["YBINNING"]).filter(|v| *v > 0);
    // N.I.N.A. writes READOUTM as the mode's display *name*; only a numeric
    // value can round-trip into TS's integer column.
    meta.readout_mode = i64_of(&["READOUTM", "READOUT", "READMODE"]);
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
