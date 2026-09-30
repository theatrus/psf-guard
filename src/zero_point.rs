//! Photometric zero point of a plate-solved frame.
//!
//! A frame's zero point is the catalog magnitude of a star that gives one ADU
//! per second in it. Thin cloud, haze, dew and low altitude all lower it, and
//! unlike cross-frame transparency it needs no reference frame: two nights,
//! two sessions, or two sides of a meridian flip compare directly.
//!
//! The fluxes must be seeing-independent (wide-aperture flux of unsaturated
//! stars) and in physical ADU. Catalog stars are placed with the
//! frame's own fresh solve, so no cross-frame matching is involved.

use serde::{Deserialize, Serialize};

/// Bump when the matching or statistics below change.
pub const ZERO_POINT_ALGORITHM_VERSION: u32 = 1;

/// Largest distance, in pixels, between a projected catalog star and a
/// measured one. The solve has no distortion terms, so wide fields need a
/// little room at the corners.
const MATCH_RADIUS_PX: f64 = 3.0;
/// A second catalog star this close to a measured one makes the match
/// ambiguous: the measured flux may hold both.
const CROWDING_RADIUS_PX: f64 = 6.0;
/// Fewest matched stars for a zero point.
const MINIMUM_MATCHES: usize = 10;
/// Catalog stars requested around the field, brightest first. The measured
/// stars are the brightest unsaturated ones, so the bright end suffices.
const CATALOG_LIMIT: usize = 20_000;

/// A measured frame zero point.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ZeroPoint {
    /// Catalog magnitude of a star giving one ADU per second.
    pub magnitude: f64,
    /// Matched stars behind the median.
    pub stars: usize,
    /// Robust spread of the per-star values, in magnitudes.
    pub spread: f64,
    /// [`ZERO_POINT_ALGORITHM_VERSION`] when measured.
    pub version: u32,
}

/// A star measured in the frame: position in pixels and flux in ADU.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MeasuredStar {
    pub x: f64,
    pub y: f64,
    pub flux_adu: f64,
}

/// Match catalog stars placed by `wcs` to `measured` stars and take the
/// median of `m + 2.5·log10(flux / exposure)`.
///
/// `catalog` should hold one photometric band (Gaia G); a catalog mixing
/// bands gives a zero point that drifts with star color.
pub fn measure(
    wcs: &seiza::Wcs,
    width: usize,
    height: usize,
    measured: &[MeasuredStar],
    exposure_s: f64,
    catalog: &dyn seiza::catalog::StarCatalog,
) -> Option<ZeroPoint> {
    if !(exposure_s.is_finite() && exposure_s > 0.0) || width == 0 || height == 0 {
        return None;
    }
    let measured: Vec<MeasuredStar> = measured
        .iter()
        .copied()
        .filter(|star| star.flux_adu.is_finite() && star.flux_adu > 0.0)
        .collect();
    if measured.len() < MINIMUM_MATCHES {
        return None;
    }
    let (center_ra, center_dec) = wcs.pixel_to_world(width as f64 / 2.0, height as f64 / 2.0);
    let radius_deg = wcs
        .footprint(width as u32, height as u32)
        .iter()
        .map(|&(ra, dec)| angular_distance_deg(center_ra, center_dec, ra, dec))
        .fold(0.0_f64, f64::max);
    if !radius_deg.is_finite() || radius_deg <= 0.0 {
        return None;
    }
    let placed: Vec<(f64, f64, f32)> = catalog
        .cone_search(center_ra, center_dec, radius_deg, CATALOG_LIMIT)
        .into_iter()
        .filter_map(|star| {
            let (x, y) = wcs.world_to_pixel(star.ra, star.dec)?;
            (x >= 0.0 && y >= 0.0 && x < width as f64 && y < height as f64)
                .then_some((x, y, star.mag))
        })
        .collect();

    let mut values = Vec::new();
    for star in &measured {
        let mut nearest: Option<(f64, f32)> = None;
        let mut neighbours = 0;
        for &(x, y, mag) in &placed {
            let distance = (x - star.x).hypot(y - star.y);
            if distance <= CROWDING_RADIUS_PX {
                neighbours += 1;
            }
            if distance <= MATCH_RADIUS_PX && nearest.is_none_or(|(best, _)| distance < best) {
                nearest = Some((distance, mag));
            }
        }
        if let (Some((_, mag)), 1) = (nearest, neighbours) {
            values.push(f64::from(mag) + 2.5 * (star.flux_adu / exposure_s).log10());
        }
    }
    if values.len() < MINIMUM_MATCHES {
        return None;
    }
    let magnitude = median(&mut values);
    let mut deviations: Vec<f64> = values
        .iter()
        .map(|value| (value - magnitude).abs())
        .collect();
    let spread = 1.4826 * median(&mut deviations);
    Some(ZeroPoint {
        magnitude,
        stars: values.len(),
        spread,
        version: ZERO_POINT_ALGORITHM_VERSION,
    })
}

fn angular_distance_deg(ra1: f64, dec1: f64, ra2: f64, dec2: f64) -> f64 {
    let (ra1, dec1, ra2, dec2) = (
        ra1.to_radians(),
        dec1.to_radians(),
        ra2.to_radians(),
        dec2.to_radians(),
    );
    let a = ((dec2 - dec1) / 2.0).sin().powi(2)
        + dec1.cos() * dec2.cos() * ((ra2 - ra1) / 2.0).sin().powi(2);
    (2.0 * a.sqrt().min(1.0).asin()).to_degrees()
}

fn median(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    if values.len().is_multiple_of(2) {
        (values[middle - 1] + values[middle]) / 2.0
    } else {
        values[middle]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use seiza::catalog::{CatalogStar, StarCatalog};

    struct FixedCatalog(Vec<CatalogStar>);

    impl StarCatalog for FixedCatalog {
        fn cone_search(&self, _: f64, _: f64, _: f64, limit: usize) -> Vec<CatalogStar> {
            self.0.iter().copied().take(limit).collect()
        }
    }

    /// One arcsecond per pixel, north up, centred on RA 315°, Dec +68°.
    fn wcs() -> seiza::Wcs {
        seiza::Wcs {
            crval: (315.0, 68.0),
            crpix: (500.0, 400.0),
            cd: [[-1.0 / 3600.0, 0.0], [0.0, 1.0 / 3600.0]],
            sip: None,
        }
    }

    /// Stars on a grid with magnitudes 10.0, 10.1, ... and the flux a frame
    /// with the given zero point and exposure would record.
    fn field(zero_point: f64, exposure_s: f64) -> (FixedCatalog, Vec<MeasuredStar>) {
        let wcs = wcs();
        let mut catalog = Vec::new();
        let mut measured = Vec::new();
        for index in 0..40 {
            let (x, y) = (
                60.0 + (index % 8) as f64 * 110.0,
                60.0 + (index / 8) as f64 * 150.0,
            );
            let (ra, dec) = wcs.pixel_to_world(x, y);
            let mag = 10.0 + index as f64 * 0.1;
            catalog.push(CatalogStar {
                ra,
                dec,
                mag: mag as f32,
            });
            measured.push(MeasuredStar {
                x: x + 0.8,
                y: y - 0.5,
                flux_adu: exposure_s * 10_f64.powf((zero_point - mag) / 2.5),
            });
        }
        (FixedCatalog(catalog), measured)
    }

    #[test]
    fn recovers_the_zero_point_regardless_of_exposure() {
        for exposure in [120.0, 300.0] {
            let (catalog, measured) = field(24.3, exposure);
            let zero_point = measure(&wcs(), 1000, 800, &measured, exposure, &catalog).unwrap();
            assert!((zero_point.magnitude - 24.3).abs() < 1e-3, "{zero_point:?}");
            assert_eq!(zero_point.stars, 40);
            assert!(zero_point.spread < 1e-3);
        }
    }

    #[test]
    fn haze_lowers_the_zero_point() {
        let (catalog, clear) = field(24.3, 300.0);
        let hazy: Vec<MeasuredStar> = clear
            .iter()
            .map(|star| MeasuredStar {
                flux_adu: star.flux_adu * 0.5,
                ..*star
            })
            .collect();
        let clear = measure(&wcs(), 1000, 800, &clear, 300.0, &catalog).unwrap();
        let hazy = measure(&wcs(), 1000, 800, &hazy, 300.0, &catalog).unwrap();
        assert!((clear.magnitude - hazy.magnitude - 2.5 * 2_f64.log10()).abs() < 1e-3);
    }

    #[test]
    fn crowded_and_unmatched_stars_are_left_out() {
        let (mut catalog, mut measured) = field(24.3, 300.0);
        // A faint neighbour 4 px from the first star makes it ambiguous.
        let (ra, dec) = wcs().pixel_to_world(64.0, 60.0);
        catalog.0.push(CatalogStar { ra, dec, mag: 15.0 });
        // A measured star with no catalog counterpart.
        measured.push(MeasuredStar {
            x: 990.0,
            y: 790.0,
            flux_adu: 1.0e6,
        });
        let zero_point = measure(&wcs(), 1000, 800, &measured, 300.0, &catalog).unwrap();
        assert_eq!(zero_point.stars, 39);
        assert!((zero_point.magnitude - 24.3).abs() < 1e-3);
    }

    #[test]
    fn too_few_matches_give_no_zero_point() {
        let (catalog, measured) = field(24.3, 300.0);
        assert!(measure(&wcs(), 1000, 800, &measured[..9], 300.0, &catalog).is_none());
        assert!(measure(&wcs(), 1000, 800, &measured, 0.0, &catalog).is_none());
    }
}
