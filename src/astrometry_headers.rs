//! Normalized astrometry-related FITS headers with explicit provenance.
//!
//! FITS writers use several names and representations for pointing and image
//! scale. Keeping the normalization here prevents catalog association,
//! solving, and sequence analysis from quietly interpreting the same header in
//! different ways.

use std::path::Path;

use seiza_fits::HeaderValue;
use serde::{Deserialize, Serialize};

/// A normalized value and the FITS header(s) used to obtain it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Provenanced<T> {
    pub value: T,
    /// Header keywords in derivation order. A directly read value has one
    /// source; a camera-geometry scale normally has three or four.
    pub sources: Vec<String>,
    /// Human-readable derivation when the value was not copied directly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub derivation: Option<String>,
}

impl<T> Provenanced<T> {
    fn direct(value: T, source: &str) -> Self {
        Self {
            value,
            sources: vec![source.to_string()],
            derivation: None,
        }
    }

    fn derived(value: T, sources: Vec<String>, derivation: &str) -> Self {
        Self {
            value,
            sources,
            derivation: Some(derivation.to_string()),
        }
    }
}

/// Astrometry and field-geometry facts normalized from a FITS primary header.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FitsAstrometryHeaders {
    pub object_name: Option<Provenanced<String>>,
    /// Approximate image/mount center in ICRS degrees.
    pub center_ra_deg: Option<Provenanced<f64>>,
    pub center_dec_deg: Option<Provenanced<f64>>,
    pub pixel_scale_arcsec_per_pixel: Option<Provenanced<f64>>,
    pub width: Option<Provenanced<u32>>,
    pub height: Option<Provenanced<u32>>,
    pub capture_time: Option<Provenanced<String>>,
    pub focal_length_mm: Option<Provenanced<f64>>,
    pub pixel_size_um: Option<Provenanced<f64>>,
    pub binning_x: Option<Provenanced<f64>>,
}

impl FitsAstrometryHeaders {
    /// Read only the FITS header blocks, without touching the pixel payload.
    pub fn from_path(path: &Path) -> Result<Self, seiza_fits::FitsError> {
        seiza_fits::read_header(path).map(|headers| Self::from_headers(&headers))
    }

    /// Normalize an already-parsed FITS header.
    pub fn from_headers(headers: &[(String, HeaderValue)]) -> Self {
        let object_name = find_text(headers, &["OBJECT"]);
        let capture_time = find_text(headers, &["DATE-OBS", "DATEOBS"]);
        let width = find_u32(headers, &["NAXIS1"]);
        let height = find_u32(headers, &["NAXIS2"]);
        let focal_length_mm = find_positive_f64(headers, &["FOCALLEN", "FOCAL"]);
        let pixel_size_um = find_positive_f64(headers, &["XPIXSZ", "PIXSIZE", "PIXELSIZE"]);
        let binning_x = find_positive_f64(headers, &["XBINNING", "BINNING"]);

        let center_ra_deg =
            find_coordinate(headers, &["RA", "OBJCTRA", "OBJRA", "TELRA"], parse_ra_deg);
        let center_dec_deg = find_coordinate(
            headers,
            &["DEC", "OBJCTDEC", "OBJDEC", "TELDEC"],
            parse_dec_deg,
        );

        let pixel_scale_arcsec_per_pixel = wcs_pixel_scale(headers)
            .or_else(|| explicit_pixel_scale(headers))
            .or_else(|| {
                camera_geometry_scale(
                    focal_length_mm.as_ref(),
                    pixel_size_um.as_ref(),
                    binning_x.as_ref(),
                )
            });

        Self {
            object_name,
            center_ra_deg,
            center_dec_deg,
            pixel_scale_arcsec_per_pixel,
            width,
            height,
            capture_time,
            focal_length_mm,
            pixel_size_um,
            binning_x,
        }
    }
}

fn find_header<'a>(
    headers: &'a [(String, HeaderValue)],
    names: &[&str],
) -> Option<(&'a str, &'a HeaderValue)> {
    names.iter().find_map(|wanted| {
        headers
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(wanted))
            .map(|(name, value)| (name.as_str(), value))
    })
}

fn value_text(value: &HeaderValue) -> Option<&str> {
    match value {
        HeaderValue::String(value) | HeaderValue::Raw(value) => Some(value.trim()),
        _ => None,
    }
}

fn find_text(headers: &[(String, HeaderValue)], names: &[&str]) -> Option<Provenanced<String>> {
    let (source, value) = find_header(headers, names)?;
    let value = value_text(value)?.trim();
    (!value.is_empty()).then(|| Provenanced::direct(value.to_string(), source))
}

fn header_f64(value: &HeaderValue) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value_text(value)?.replace(['D', 'd'], "E").parse().ok())
        .filter(|value| value.is_finite())
}

fn find_f64(headers: &[(String, HeaderValue)], names: &[&str]) -> Option<Provenanced<f64>> {
    let (source, value) = find_header(headers, names)?;
    header_f64(value).map(|value| Provenanced::direct(value, source))
}

fn find_positive_f64(
    headers: &[(String, HeaderValue)],
    names: &[&str],
) -> Option<Provenanced<f64>> {
    find_f64(headers, names).filter(|value| value.value > 0.0)
}

fn find_u32(headers: &[(String, HeaderValue)], names: &[&str]) -> Option<Provenanced<u32>> {
    let (source, value) = find_header(headers, names)?;
    let value = match value {
        HeaderValue::Integer(value) => u32::try_from(*value).ok(),
        HeaderValue::Float(value) if value.is_finite() && value.fract() == 0.0 => {
            u32::try_from(*value as i64).ok()
        }
        HeaderValue::String(value) | HeaderValue::Raw(value) => value.trim().parse().ok(),
        _ => None,
    }?;
    (value > 0).then(|| Provenanced::direct(value, source))
}

fn find_coordinate(
    headers: &[(String, HeaderValue)],
    names: &[&str],
    parse: fn(&str) -> Option<f64>,
) -> Option<Provenanced<f64>> {
    for wanted in names {
        let Some((source, value)) = find_header(headers, &[*wanted]) else {
            continue;
        };
        let parsed = match value {
            HeaderValue::Integer(value) => parse(&value.to_string()),
            HeaderValue::Float(value) => parse(&value.to_string()),
            HeaderValue::String(value) | HeaderValue::Raw(value) => parse(value),
            HeaderValue::Logical(_) => None,
        };
        if let Some(value) = parsed {
            return Some(Provenanced::direct(value, source));
        }
    }
    None
}

/// Parse right ascension to degrees. Plain numeric values are interpreted as
/// degrees; sexagesimal values containing separators or hour markers are
/// interpreted as hours.
pub fn parse_ra_deg(input: &str) -> Option<f64> {
    let value = input.trim();
    if value.is_empty() {
        return None;
    }
    if let Ok(degrees) = value.parse::<f64>() {
        return (degrees.is_finite() && (0.0..=360.0).contains(&degrees))
            .then(|| degrees.rem_euclid(360.0));
    }

    let parts = sexagesimal_parts(value)?;
    if parts.negative || !(0.0..=24.0).contains(&parts.major) {
        return None;
    }
    let hours = parts.major + parts.minutes / 60.0 + parts.seconds / 3600.0;
    (hours <= 24.0).then(|| (hours * 15.0).rem_euclid(360.0))
}

/// Parse declination to signed degrees. Plain and sexagesimal forms are
/// accepted; the sign on `-00` is preserved.
pub fn parse_dec_deg(input: &str) -> Option<f64> {
    let value = input.trim();
    if value.is_empty() {
        return None;
    }
    if let Ok(degrees) = value.parse::<f64>() {
        return (degrees.is_finite() && (-90.0..=90.0).contains(&degrees)).then_some(degrees);
    }

    let parts = sexagesimal_parts(value)?;
    if parts.major > 90.0 {
        return None;
    }
    let magnitude = parts.major + parts.minutes / 60.0 + parts.seconds / 3600.0;
    if magnitude > 90.0 {
        return None;
    }
    Some(if parts.negative {
        -magnitude
    } else {
        magnitude
    })
}

struct SexagesimalParts {
    negative: bool,
    major: f64,
    minutes: f64,
    seconds: f64,
}

fn sexagesimal_parts(input: &str) -> Option<SexagesimalParts> {
    let trimmed = input.trim();
    let negative = trimmed.starts_with('-');
    let normalized: String = trimmed
        .trim_start_matches(['+', '-'])
        .chars()
        .map(|character| match character {
            '0'..='9' | '.' => character,
            _ => ' ',
        })
        .collect();
    let values: Vec<f64> = normalized
        .split_whitespace()
        .map(str::parse)
        .collect::<Result<_, _>>()
        .ok()?;
    if values.is_empty() || values.len() > 3 {
        return None;
    }
    let minutes = values.get(1).copied().unwrap_or(0.0);
    let seconds = values.get(2).copied().unwrap_or(0.0);
    if !values.iter().all(|value| value.is_finite())
        || !(0.0..60.0).contains(&minutes)
        || !(0.0..60.0).contains(&seconds)
    {
        return None;
    }
    Some(SexagesimalParts {
        negative,
        major: values[0],
        minutes,
        seconds,
    })
}

fn wcs_pixel_scale(headers: &[(String, HeaderValue)]) -> Option<Provenanced<f64>> {
    let cd11 = find_f64(headers, &["CD1_1"]);
    let cd12 = find_f64(headers, &["CD1_2"]);
    let cd21 = find_f64(headers, &["CD2_1"]);
    let cd22 = find_f64(headers, &["CD2_2"]);
    if let (Some(cd11), Some(cd12), Some(cd21), Some(cd22)) = (cd11, cd12, cd21, cd22) {
        let determinant = cd11.value * cd22.value - cd12.value * cd21.value;
        let scale = determinant.abs().sqrt() * 3600.0;
        if scale.is_finite() && scale > 0.0 {
            return Some(Provenanced::derived(
                scale,
                [cd11, cd12, cd21, cd22]
                    .into_iter()
                    .flat_map(|value| value.sources)
                    .collect(),
                "3600 * sqrt(abs(det(CD)))",
            ));
        }
    }

    let cdelt1 = find_f64(headers, &["CDELT1"]);
    let cdelt2 = find_f64(headers, &["CDELT2"]);
    match (cdelt1, cdelt2) {
        (Some(x), Some(y)) => {
            let scale = (x.value * y.value).abs().sqrt() * 3600.0;
            (scale.is_finite() && scale > 0.0).then(|| {
                Provenanced::derived(
                    scale,
                    [x, y].into_iter().flat_map(|value| value.sources).collect(),
                    "3600 * sqrt(abs(CDELT1 * CDELT2))",
                )
            })
        }
        (Some(value), None) | (None, Some(value)) => {
            let scale = value.value.abs() * 3600.0;
            (scale.is_finite() && scale > 0.0)
                .then(|| Provenanced::derived(scale, value.sources, "3600 * abs(CDELT)"))
        }
        (None, None) => None,
    }
}

fn explicit_pixel_scale(headers: &[(String, HeaderValue)]) -> Option<Provenanced<f64>> {
    find_positive_f64(headers, &["PIXSCALE", "SECPIX", "PIXSCAL1"])
}

fn camera_geometry_scale(
    focal_length_mm: Option<&Provenanced<f64>>,
    pixel_size_um: Option<&Provenanced<f64>>,
    binning_x: Option<&Provenanced<f64>>,
) -> Option<Provenanced<f64>> {
    let focal_length_mm = focal_length_mm?;
    let pixel_size_um = pixel_size_um?;
    let binning = binning_x.map_or(1.0, |value| value.value);
    let scale = 206.265 * pixel_size_um.value * binning / focal_length_mm.value;
    if !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    let mut sources = Vec::new();
    sources.extend(focal_length_mm.sources.iter().cloned());
    sources.extend(pixel_size_um.sources.iter().cloned());
    if let Some(binning_x) = binning_x {
        sources.extend(binning_x.sources.iter().cloned());
    }
    Some(Provenanced::derived(
        scale,
        sources,
        "206.265 * pixel_size_um * binning_x / focal_length_mm",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(values: &[(&str, HeaderValue)]) -> Vec<(String, HeaderValue)> {
        values
            .iter()
            .map(|(name, value)| ((*name).to_string(), value.clone()))
            .collect()
    }

    #[test]
    fn parses_numeric_and_sexagesimal_coordinates() {
        assert_eq!(parse_ra_deg("180.25"), Some(180.25));
        assert!((parse_ra_deg("12:30:00").unwrap() - 187.5).abs() < 1e-10);
        assert!((parse_ra_deg("21h 18m").unwrap() - 319.5).abs() < 1e-10);
        assert!((parse_dec_deg("-00 30 00").unwrap() + 0.5).abs() < 1e-10);
        assert!((parse_dec_deg("+43° 57′ 00″").unwrap() - 43.95).abs() < 1e-10);
        assert_eq!(parse_dec_deg("91"), None);
        assert_eq!(parse_ra_deg("25:00:00"), None);
    }

    #[test]
    fn normalizes_header_priority_and_provenance() {
        let parsed = FitsAstrometryHeaders::from_headers(&headers(&[
            ("OBJECT", HeaderValue::String("M 31".into())),
            ("RA", HeaderValue::String("00:42:44.3".into())),
            ("OBJCTRA", HeaderValue::String("12:00:00".into())),
            ("DEC", HeaderValue::String("+41:16:09".into())),
            ("NAXIS1", HeaderValue::Integer(6248)),
            ("NAXIS2", HeaderValue::Integer(4176)),
            ("PIXSCALE", HeaderValue::Float(1.42)),
        ]));

        assert_eq!(parsed.object_name.unwrap().value, "M 31");
        assert!((parsed.center_ra_deg.unwrap().value - 10.6845833333).abs() < 1e-8);
        assert_eq!(parsed.center_dec_deg.unwrap().sources, ["DEC"]);
        assert_eq!(parsed.pixel_scale_arcsec_per_pixel.unwrap().value, 1.42);
        assert_eq!(parsed.width.unwrap().value, 6248);
        assert_eq!(parsed.height.unwrap().value, 4176);
    }

    #[test]
    fn wcs_scale_wins_over_explicit_and_camera_geometry() {
        let parsed = FitsAstrometryHeaders::from_headers(&headers(&[
            ("CD1_1", HeaderValue::Float(-1.0 / 3600.0)),
            ("CD1_2", HeaderValue::Float(0.0)),
            ("CD2_1", HeaderValue::Float(0.0)),
            ("CD2_2", HeaderValue::Float(1.0 / 3600.0)),
            ("PIXSCALE", HeaderValue::Float(2.0)),
            ("XPIXSZ", HeaderValue::Float(3.76)),
            ("XBINNING", HeaderValue::Integer(2)),
            ("FOCALLEN", HeaderValue::Float(550.0)),
        ]));

        let scale = parsed.pixel_scale_arcsec_per_pixel.unwrap();
        assert!((scale.value - 1.0).abs() < 1e-10);
        assert_eq!(scale.sources, ["CD1_1", "CD1_2", "CD2_1", "CD2_2"]);
    }

    #[test]
    fn derives_camera_geometry_scale_with_binning() {
        let parsed = FitsAstrometryHeaders::from_headers(&headers(&[
            ("XPIXSZ", HeaderValue::Float(3.76)),
            ("XBINNING", HeaderValue::Integer(2)),
            ("FOCALLEN", HeaderValue::Float(550.0)),
        ]));

        let scale = parsed.pixel_scale_arcsec_per_pixel.unwrap();
        let expected = 206.265 * 3.76 * 2.0 / 550.0;
        assert!((scale.value - expected).abs() < 1e-10);
        assert_eq!(scale.sources, ["FOCALLEN", "XPIXSZ", "XBINNING"]);
    }
}
