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
    /// Exposure midpoint timestamp when the FITS writer provides one.
    pub exposure_mid_time: Option<Provenanced<String>>,
    /// Explicit shutter-close timestamp when present.
    pub exposure_end_time: Option<Provenanced<String>>,
    /// Exposure duration in seconds.
    pub exposure_seconds: Option<Provenanced<f64>>,
    /// Topocentric observing site needed for satellite propagation.
    pub observer: Option<Provenanced<FitsObserverLocation>>,
    pub focal_length_mm: Option<Provenanced<f64>>,
    pub pixel_size_um: Option<Provenanced<f64>>,
    pub binning_x: Option<Provenanced<f64>>,
    /// A usable TAN WCS assembled from standard FITS WCS keywords. FITS
    /// reference pixels are converted from one-based to Seiza's zero-based
    /// pixel coordinates here, once, so every downstream caller agrees.
    pub embedded_wcs: Option<Provenanced<FitsWcsHeaders>>,
}

/// Geodetic observing site from FITS headers. Longitude is east-positive.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FitsObserverLocation {
    pub latitude_deg: f64,
    pub longitude_deg: f64,
    pub altitude_m: f64,
}

/// Serializable TAN WCS facts extracted from a FITS primary header.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FitsWcsHeaders {
    pub crval: [f64; 2],
    pub crpix: [f64; 2],
    pub cd: [[f64; 2]; 2],
    pub ctype: [String; 2],
    pub cunit: [String; 2],
    pub radesys: String,
    pub equinox: f64,
}

impl FitsAstrometryHeaders {
    /// Read only the FITS header blocks, without touching the pixel payload.
    pub fn from_path(path: &Path) -> Result<Self, seiza_fits::FitsError> {
        seiza_fits::read_header(path).map(|headers| Self::from_headers(&headers))
    }

    /// Normalize an already-parsed FITS header.
    pub fn from_headers(headers: &[(String, HeaderValue)]) -> Self {
        let object_name = find_text(headers, &["OBJECT"]);
        let capture_time = find_text(headers, &["DATE-BEG", "DATE-OBS", "DATEOBS"]);
        let exposure_mid_time = find_text(headers, &["DATE-AVG", "DATEAVG"]);
        let exposure_end_time = find_text(headers, &["DATE-END", "DATEEND"]);
        let exposure_seconds = find_positive_f64(headers, &["EXPTIME", "EXPOSURE"]);
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
        let embedded_wcs = embedded_wcs(headers);
        let observer = fits_observer(headers);

        Self {
            object_name,
            center_ra_deg,
            center_dec_deg,
            pixel_scale_arcsec_per_pixel,
            width,
            height,
            capture_time,
            exposure_mid_time,
            exposure_end_time,
            exposure_seconds,
            observer,
            focal_length_mm,
            pixel_size_um,
            binning_x,
            embedded_wcs,
        }
    }
}

fn fits_observer(headers: &[(String, HeaderValue)]) -> Option<Provenanced<FitsObserverLocation>> {
    let latitude = find_coordinate(headers, &["SITELAT", "LAT-OBS", "OBSGEO-B"], parse_dec_deg)?;
    let longitude = find_coordinate(
        headers,
        &["SITELONG", "SITELON", "LONG-OBS", "OBSGEO-L"],
        parse_longitude_deg,
    )?;
    let altitude = find_f64(
        headers,
        &["SITEELEV", "SITEELEVATION", "ALT-OBS", "OBSGEO-H"],
    );
    let mut sources = latitude.sources;
    sources.extend(longitude.sources);
    let altitude_m = altitude.as_ref().map_or(0.0, |value| value.value);
    if let Some(altitude) = altitude {
        sources.extend(altitude.sources);
    }
    Some(Provenanced::derived(
        FitsObserverLocation {
            latitude_deg: latitude.value,
            longitude_deg: longitude.value,
            altitude_m,
        },
        sources,
        "geodetic observing site; missing altitude defaults to 0 m",
    ))
}

fn parse_longitude_deg(input: &str) -> Option<f64> {
    let value = input.trim();
    if let Ok(degrees) = value.parse::<f64>() {
        return (degrees.is_finite() && (-360.0..=360.0).contains(&degrees))
            .then(|| ((degrees + 180.0).rem_euclid(360.0)) - 180.0);
    }
    let parts = sexagesimal_parts(value)?;
    if parts.major > 360.0 {
        return None;
    }
    let magnitude = parts.major + parts.minutes / 60.0 + parts.seconds / 3600.0;
    if magnitude > 360.0 {
        return None;
    }
    let degrees = if parts.negative {
        -magnitude
    } else {
        magnitude
    };
    Some(((degrees + 180.0).rem_euclid(360.0)) - 180.0)
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
    find_positive_f64(
        headers,
        &[
            "PIXSCALE", "SCALE", "SECPIX", "SECPIX1", "SECPIX2", "PIXSCAL1",
        ],
    )
}

fn embedded_wcs(headers: &[(String, HeaderValue)]) -> Option<Provenanced<FitsWcsHeaders>> {
    let crval1 = find_f64(headers, &["CRVAL1"])?;
    let crval2 = find_f64(headers, &["CRVAL2"])?;
    if !(0.0..=360.0).contains(&crval1.value) || !(-90.0..=90.0).contains(&crval2.value) {
        return None;
    }
    let crpix1 = find_f64(headers, &["CRPIX1"])?;
    let crpix2 = find_f64(headers, &["CRPIX2"])?;
    let ctype1 = find_text(headers, &["CTYPE1"])?;
    let ctype2 = find_text(headers, &["CTYPE2"])?;
    // Seiza's WCS implementation is a linear TAN projection in degrees. Do
    // not silently accept TAN-SIP/TPV or other distorted projections: their
    // coefficients would be discarded and the resulting overlay would look
    // authoritative while being wrong away from the reference pixel.
    if !ctype1.value.trim().eq_ignore_ascii_case("RA---TAN")
        || !ctype2.value.trim().eq_ignore_ascii_case("DEC--TAN")
        || has_distortion_keywords(headers)
    {
        return None;
    }

    let (cd, mut matrix_sources, matrix_derivation) = wcs_cd_matrix(headers)?;
    let cunit1 = find_text(headers, &["CUNIT1"])
        .unwrap_or_else(|| Provenanced::derived("deg".to_string(), Vec::new(), "FITS default"));
    let cunit2 = find_text(headers, &["CUNIT2"])
        .unwrap_or_else(|| Provenanced::derived("deg".to_string(), Vec::new(), "FITS default"));
    if !cunit1.value.trim().eq_ignore_ascii_case("deg")
        || !cunit2.value.trim().eq_ignore_ascii_case("deg")
    {
        return None;
    }
    let radesys = find_text(headers, &["RADESYS", "RADECSYS"])
        .unwrap_or_else(|| Provenanced::derived("ICRS".to_string(), Vec::new(), "default frame"));
    if !radesys.value.trim().eq_ignore_ascii_case("ICRS") {
        return None;
    }
    let equinox = find_f64(headers, &["EQUINOX"])
        .unwrap_or_else(|| Provenanced::derived(2000.0, Vec::new(), "default equinox"));
    if !equinox.value.is_finite() || (equinox.value - 2000.0).abs() > 1e-9 {
        return None;
    }

    let mut sources = Vec::new();
    sources.extend(crval1.sources);
    sources.extend(crval2.sources);
    sources.extend(crpix1.sources);
    sources.extend(crpix2.sources);
    sources.extend(ctype1.sources.clone());
    sources.extend(ctype2.sources.clone());
    sources.append(&mut matrix_sources);
    sources.extend(cunit1.sources.clone());
    sources.extend(cunit2.sources.clone());
    sources.extend(radesys.sources.clone());
    sources.extend(equinox.sources.clone());
    sources.sort();
    sources.dedup();

    Some(Provenanced::derived(
        FitsWcsHeaders {
            crval: [crval1.value.rem_euclid(360.0), crval2.value],
            // FITS CRPIX is one-based. Seiza WCS and the browser overlay use
            // zero-based pixel coordinates.
            crpix: [crpix1.value - 1.0, crpix2.value - 1.0],
            cd,
            ctype: [ctype1.value, ctype2.value],
            cunit: [cunit1.value, cunit2.value],
            radesys: radesys.value,
            equinox: equinox.value,
        },
        sources,
        matrix_derivation,
    ))
}

type CdMatrix = ([[f64; 2]; 2], Vec<String>, &'static str);

fn wcs_cd_matrix(headers: &[(String, HeaderValue)]) -> Option<CdMatrix> {
    let cd11 = find_f64(headers, &["CD1_1"]);
    let cd12 = find_f64(headers, &["CD1_2"]);
    let cd21 = find_f64(headers, &["CD2_1"]);
    let cd22 = find_f64(headers, &["CD2_2"]);
    if let (Some(cd11), Some(cd12), Some(cd21), Some(cd22)) = (cd11, cd12, cd21, cd22) {
        let matrix = [[cd11.value, cd12.value], [cd21.value, cd22.value]];
        if valid_cd(matrix) {
            return Some((
                matrix,
                [cd11, cd12, cd21, cd22]
                    .into_iter()
                    .flat_map(|value| value.sources)
                    .collect(),
                "direct FITS CD matrix",
            ));
        }
    }

    let cdelt1 = find_f64(headers, &["CDELT1"]);
    let cdelt2 = find_f64(headers, &["CDELT2"]);
    if let (Some(cdelt1), Some(cdelt2)) = (cdelt1, cdelt2) {
        let pc11 = find_f64(headers, &["PC1_1"]);
        let pc12 = find_f64(headers, &["PC1_2"]);
        let pc21 = find_f64(headers, &["PC2_1"]);
        let pc22 = find_f64(headers, &["PC2_2"]);
        let has_pc = pc11.is_some() || pc12.is_some() || pc21.is_some() || pc22.is_some();
        if has_pc {
            let matrix = [
                [
                    cdelt1.value * pc11.as_ref().map_or(1.0, |v| v.value),
                    cdelt1.value * pc12.as_ref().map_or(0.0, |v| v.value),
                ],
                [
                    cdelt2.value * pc21.as_ref().map_or(0.0, |v| v.value),
                    cdelt2.value * pc22.as_ref().map_or(1.0, |v| v.value),
                ],
            ];
            if valid_cd(matrix) {
                let mut sources = [cdelt1.sources, cdelt2.sources].concat();
                for pc in [pc11, pc12, pc21, pc22].into_iter().flatten() {
                    sources.extend(pc.sources);
                }
                return Some((matrix, sources, "FITS PC matrix scaled by CDELT"));
            }
        }

        let rotation = find_f64(headers, &["CROTA2", "CROTA1"]);
        let angle = rotation
            .as_ref()
            .map_or(0.0, |value| value.value)
            .to_radians();
        let (sin, cos) = angle.sin_cos();
        let matrix = [
            [cdelt1.value * cos, -cdelt2.value * sin],
            [cdelt1.value * sin, cdelt2.value * cos],
        ];
        if valid_cd(matrix) {
            let mut sources = [cdelt1.sources, cdelt2.sources].concat();
            if let Some(rotation) = rotation {
                sources.extend(rotation.sources);
            }
            return Some((matrix, sources, "legacy FITS CDELT/CROTA matrix"));
        }
    }

    None
}

fn has_distortion_keywords(headers: &[(String, HeaderValue)]) -> bool {
    headers.iter().any(|(name, _)| {
        let name = name.trim().to_ascii_uppercase();
        matches!(
            name.as_str(),
            "A_ORDER" | "B_ORDER" | "AP_ORDER" | "BP_ORDER"
        ) || name.starts_with("A_")
            || name.starts_with("B_")
            || name.starts_with("AP_")
            || name.starts_with("BP_")
            || name.starts_with("PV1_")
            || name.starts_with("PV2_")
    })
}

fn valid_cd(matrix: [[f64; 2]; 2]) -> bool {
    matrix.into_iter().flatten().all(f64::is_finite)
        && (matrix[0][0] * matrix[1][1] - matrix[0][1] * matrix[1][0]).abs() > 1e-15
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
    fn normalizes_exposure_bounds_and_observer_with_provenance() {
        let parsed = FitsAstrometryHeaders::from_headers(&headers(&[
            (
                "DATE-BEG",
                HeaderValue::String("2026-07-19T05:12:00.000Z".into()),
            ),
            (
                "DATE-END",
                HeaderValue::String("2026-07-19T05:13:30.000Z".into()),
            ),
            ("EXPTIME", HeaderValue::Float(90.0)),
            ("SITELAT", HeaderValue::String("+34:12:00".into())),
            ("SITELONG", HeaderValue::Float(241.75)),
            ("SITEELEV", HeaderValue::Float(1234.0)),
        ]));

        assert_eq!(parsed.capture_time.unwrap().sources, ["DATE-BEG"]);
        assert_eq!(parsed.exposure_end_time.unwrap().sources, ["DATE-END"]);
        assert_eq!(parsed.exposure_seconds.unwrap().value, 90.0);
        let observer = parsed.observer.unwrap();
        assert!((observer.value.latitude_deg - 34.2).abs() < 1e-10);
        assert!((observer.value.longitude_deg + 118.25).abs() < 1e-10);
        assert_eq!(observer.value.altitude_m, 1234.0);
        assert_eq!(observer.sources, ["SITELAT", "SITELONG", "SITEELEV"]);
    }

    #[test]
    fn normalizes_exposure_midpoint_with_provenance() {
        let parsed = FitsAstrometryHeaders::from_headers(&headers(&[(
            "DATE-AVG",
            HeaderValue::String("2026-05-21T07:13:45.3551363".into()),
        )]));

        let midpoint = parsed.exposure_mid_time.unwrap();
        assert_eq!(midpoint.value, "2026-05-21T07:13:45.3551363");
        assert_eq!(midpoint.sources, ["DATE-AVG"]);
    }

    #[test]
    fn observer_defaults_missing_altitude_but_requires_both_coordinates() {
        let parsed = FitsAstrometryHeaders::from_headers(&headers(&[
            ("OBSGEO-B", HeaderValue::Float(-31.2)),
            ("OBSGEO-L", HeaderValue::Float(149.1)),
        ]));
        let observer = parsed.observer.unwrap();
        assert_eq!(observer.value.altitude_m, 0.0);
        assert_eq!(observer.sources, ["OBSGEO-B", "OBSGEO-L"]);

        let missing_longitude =
            FitsAstrometryHeaders::from_headers(&headers(&[("SITELAT", HeaderValue::Float(34.0))]));
        assert!(missing_longitude.observer.is_none());
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

    #[test]
    fn builds_legacy_cdelt_crota_wcs_and_converts_crpix_to_zero_based() {
        let parsed = FitsAstrometryHeaders::from_headers(&headers(&[
            ("CRVAL1", HeaderValue::Float(10.669674399)),
            ("CRVAL2", HeaderValue::Float(41.268310106)),
            ("CRPIX1", HeaderValue::Float(1920.0)),
            ("CRPIX2", HeaderValue::Float(1080.0)),
            ("CTYPE1", HeaderValue::String("RA---TAN".into())),
            ("CTYPE2", HeaderValue::String("DEC--TAN".into())),
            ("CDELT1", HeaderValue::Float(0.0003820370496)),
            ("CDELT2", HeaderValue::Float(0.0003820370496)),
            ("CROTA2", HeaderValue::Float(1.4146201508)),
            ("RADESYS", HeaderValue::String("ICRS".into())),
            ("EQUINOX", HeaderValue::Float(2000.0)),
        ]));

        let wcs = parsed.embedded_wcs.unwrap();
        assert_eq!(wcs.value.crpix, [1919.0, 1079.0]);
        assert_eq!(wcs.value.radesys, "ICRS");
        let scale = (wcs.value.cd[0][0] * wcs.value.cd[1][1]
            - wcs.value.cd[0][1] * wcs.value.cd[1][0])
            .abs()
            .sqrt()
            * 3600.0;
        assert!((scale - 1.37533337856).abs() < 1e-8);
        assert_eq!(
            wcs.derivation.as_deref(),
            Some("legacy FITS CDELT/CROTA matrix")
        );
    }

    #[test]
    fn rejects_scale_rotation_and_parity_without_a_fits_wcs_matrix() {
        let parsed = FitsAstrometryHeaders::from_headers(&headers(&[
            ("CRVAL1", HeaderValue::Float(313.11278835943)),
            ("CRVAL2", HeaderValue::Float(43.9080204789476)),
            ("CRPIX1", HeaderValue::Float(4788.0)),
            ("CRPIX2", HeaderValue::Float(3194.0)),
            ("CTYPE1", HeaderValue::String("RA---TAN".into())),
            ("CTYPE2", HeaderValue::String("DEC--TAN".into())),
            ("PIXSCALE", HeaderValue::Float(1.258)),
            ("ANGLE", HeaderValue::Float(359.5)),
            ("FLIPPED", HeaderValue::Logical(true)),
        ]));

        assert!(parsed.embedded_wcs.is_none());
        assert_eq!(parsed.pixel_scale_arcsec_per_pixel.unwrap().value, 1.258);
    }

    #[test]
    fn rejects_distorted_non_degree_and_non_icrs_wcs() {
        let base = [
            ("CRVAL1", HeaderValue::Float(130.1)),
            ("CRVAL2", HeaderValue::Float(19.6)),
            ("CRPIX1", HeaderValue::Float(100.0)),
            ("CRPIX2", HeaderValue::Float(100.0)),
            ("CD1_1", HeaderValue::Float(-0.0003)),
            ("CD1_2", HeaderValue::Float(0.0)),
            ("CD2_1", HeaderValue::Float(0.0)),
            ("CD2_2", HeaderValue::Float(0.0003)),
        ];

        for extra in [
            vec![
                ("CTYPE1", HeaderValue::String("RA---TAN-SIP".into())),
                ("CTYPE2", HeaderValue::String("DEC--TAN-SIP".into())),
                ("A_ORDER", HeaderValue::Integer(2)),
                ("B_ORDER", HeaderValue::Integer(2)),
            ],
            vec![
                ("CTYPE1", HeaderValue::String("RA---TAN".into())),
                ("CTYPE2", HeaderValue::String("DEC--TAN".into())),
                ("CUNIT1", HeaderValue::String("rad".into())),
                ("CUNIT2", HeaderValue::String("rad".into())),
            ],
            vec![
                ("CTYPE1", HeaderValue::String("RA---TAN".into())),
                ("CTYPE2", HeaderValue::String("DEC--TAN".into())),
                ("RADESYS", HeaderValue::String("FK5".into())),
            ],
        ] {
            let mut values = base.to_vec();
            values.extend(extra);
            assert!(FitsAstrometryHeaders::from_headers(&headers(&values))
                .embedded_wcs
                .is_none());
        }
    }
}
