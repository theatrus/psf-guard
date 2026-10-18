//! Photometric color calibration of RGB and LRGB previews against Gaia.
//!
//! Gaia stars are placed on the grid every channel is registered to: the
//! reference frame's fresh pixel solve takes a star to that frame's pixels,
//! and the reference stack's recorded sky layout takes it on to the stack's.
//! Seiza then measures each star in the three linear channels and fits the
//! gains that render a star of the white reference's color neutral.
//!
//! Calibration runs on the registered, normalized linear channels, before
//! any stretch. For LRGB it calibrates the color channels before luminance
//! joins them. Narrowband palettes are false color and never calibrate.

use seiza_stacking::{
    apply_color_gains, calibrate_color, ColorCalibrationOptions, ColorReferenceStar,
    GaiaColorSource, LinearImage, SOLAR_BP_RP,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt::Write as _;

use super::StackSkyOrientation;
use crate::astrometry::{AstrometryCatalogFileSignature, AstrometrySolutionResponse};

/// The installed Gaia photometry catalog stops at G 15.
const GAIA_MAX_MAGNITUDE: f32 = 15.0;

/// Gaia RUWE at and above which a star's colour is not trusted; Seiza's own
/// threshold, applied here to catalogs that carry RUWE.
const MAX_COLOR_RUWE: f32 = 1.4;

/// What a color request asks of calibration. RGB and LRGB requests without
/// this get the defaults: calibrated, sky neutral, white like the Sun.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct StackColorCalibration {
    #[serde(default = "enabled")]
    pub enabled: bool,
    /// Offset each channel so the sky is the same gray in all three.
    #[serde(default = "enabled")]
    pub neutralize_background: bool,
    #[serde(default)]
    pub white_reference: StackColorWhiteReference,
}

const fn enabled() -> bool {
    true
}

impl Default for StackColorCalibration {
    fn default() -> Self {
        Self {
            enabled: true,
            neutralize_background: true,
            white_reference: StackColorWhiteReference::Solar,
        }
    }
}

impl StackColorCalibration {
    pub(super) fn validate(&self) -> Result<(), String> {
        let bp_rp = self.white_reference.bp_rp();
        if !bp_rp.is_finite() || !(-0.5..=4.0).contains(&bp_rp) {
            return Err("The white reference's Gaia BP-RP color must be between -0.5 and 4".into());
        }
        Ok(())
    }

    fn options(&self) -> ColorCalibrationOptions {
        ColorCalibrationOptions {
            white_bp_rp: self.white_reference.bp_rp(),
            neutralize_background: self.neutralize_background,
            ..ColorCalibrationOptions::default()
        }
    }
}

/// The star color rendered white.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StackColorWhiteReference {
    /// A G2V star like the Sun.
    #[default]
    Solar,
    /// A Gaia BP-RP color.
    BpRp(f32),
}

impl StackColorWhiteReference {
    fn bp_rp(self) -> f32 {
        match self {
            Self::Solar => SOLAR_BP_RP,
            Self::BpRp(value) => value,
        }
    }
}

/// What a calibration rests on, fixed when the job is prepared. It is part
/// of the job's cache identity, so a new solve or a newly installed catalog
/// makes a new preview.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StackColorCalibrationBasis {
    /// The frame the reference stack was registered to.
    pub reference_image_id: Option<i32>,
    /// SHA-256 of that frame's fresh pixel solve.
    pub solution_fingerprint: Option<String>,
    /// The Gaia photometry file, when one is installed.
    pub catalog: Option<AstrometryCatalogFileSignature>,
}

/// Why a preview was not color calibrated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StackColorCalibrationSkipReason {
    /// The reference frame has no fresh pixel solve.
    NoPlateSolve,
    /// No Gaia photometry catalog is installed.
    NoGaiaCatalog,
    /// Too few Gaia stars measured cleanly for the fits.
    TooFewStars,
    /// Anything else; `detail` says what.
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StackColorCalibrationSkip {
    pub reason: StackColorCalibrationSkipReason,
    pub detail: String,
}

impl StackColorCalibrationSkip {
    fn new(reason: StackColorCalibrationSkipReason, detail: impl Into<String>) -> Self {
        Self {
            reason,
            detail: detail.into(),
        }
    }
}

/// A straight-line fit of an instrumental color against Gaia BP-RP.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct StackColorFit {
    /// Instrumental color, in magnitudes, at BP-RP = 0.
    pub intercept: f64,
    /// Magnitudes of instrumental color per magnitude of BP-RP.
    pub slope: f64,
    /// Robust scatter of the stars about the line, in magnitudes.
    pub scatter: f64,
    pub stars: usize,
}

impl From<seiza_stacking::ColorFit> for StackColorFit {
    fn from(fit: seiza_stacking::ColorFit) -> Self {
        Self {
            intercept: fit.intercept,
            slope: fit.slope,
            scatter: fit.scatter,
            stars: fit.stars,
        }
    }
}

/// The gains and offsets a calibration applied, and the evidence for them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StackColorCalibrationReport {
    /// Multipliers for R, G, B on the normalized channels; green's is 1.
    pub gains: [f32; 3],
    /// Offsets added after the gains to neutralize the sky; zero without it.
    pub offsets: [f32; 3],
    /// Each channel's sky median before calibration.
    pub background: [f32; 3],
    pub white_bp_rp: f32,
    /// `-2.5 log10(R / G)` against BP-RP.
    pub red_fit: StackColorFit,
    /// `-2.5 log10(B / G)` against BP-RP.
    pub blue_fit: StackColorFit,
    pub aperture_radius: f64,
    /// Gaia stars on the image, and those measured cleanly in all channels.
    pub stars_offered: usize,
    pub stars_measured: usize,
    /// Stars behind the weaker of the two fits.
    pub stars_used: usize,
}

/// What color calibration did for one preview.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum StackColorCalibrationOutcome {
    Calibrated(StackColorCalibrationReport),
    Skipped(StackColorCalibrationSkip),
}

/// How Gaia stars reach the composition grid: the reference frame's solve,
/// that frame's size, and the reference stack's layout.
#[derive(Debug, Clone)]
pub(super) struct StarPlacement {
    pub(super) wcs: seiza::Wcs,
    pub(super) orientation: StackSkyOrientation,
}

/// A prepared calibration: what it rests on, and either the way to place
/// stars or the reason it cannot run.
#[derive(Debug, Clone)]
pub(super) struct ColorCalibrationPlan {
    pub(super) settings: StackColorCalibration,
    pub(super) basis: StackColorCalibrationBasis,
    pub(super) placement: Result<StarPlacement, StackColorCalibrationSkip>,
}

/// Plan the calibration of a composition registered to `reference`'s grid.
pub(super) fn plan(
    settings: StackColorCalibration,
    reference_image_id: Option<i32>,
    orientation: Option<&StackSkyOrientation>,
    solution: Option<&AstrometrySolutionResponse>,
    catalog: Option<AstrometryCatalogFileSignature>,
) -> ColorCalibrationPlan {
    let solution_fingerprint = solution.map(solution_fingerprint);
    let basis = StackColorCalibrationBasis {
        reference_image_id,
        solution_fingerprint,
        catalog: catalog.clone(),
    };
    let placement = match (solution, orientation) {
        (None, _) => Err(StackColorCalibrationSkip::new(
            StackColorCalibrationSkipReason::NoPlateSolve,
            "The reference frame has no plate solve",
        )),
        (Some(_), None) => Err(StackColorCalibrationSkip::new(
            StackColorCalibrationSkipReason::Failed,
            "The reference stack records no sky layout; rebuild it",
        )),
        (Some(_), Some(_)) if catalog.is_none() => Err(StackColorCalibrationSkip::new(
            StackColorCalibrationSkipReason::NoGaiaCatalog,
            "Gaia colors are not installed",
        )),
        (Some(solution), Some(orientation)) => Ok(StarPlacement {
            wcs: crate::astrometry::wcs_from_response(&solution.wcs),
            orientation: orientation.clone(),
        }),
    };
    ColorCalibrationPlan {
        settings,
        basis,
        placement,
    }
}

fn solution_fingerprint(solution: &AstrometrySolutionResponse) -> String {
    let mut hasher = Sha256::new();
    hasher.update(
        serde_json::to_vec(&(&solution.wcs, solution.image_width, solution.image_height))
            .expect("a WCS serializes"),
    );
    let mut fingerprint = String::with_capacity(64);
    for byte in hasher.finalize() {
        write!(&mut fingerprint, "{byte:02x}").expect("writing to a String cannot fail");
    }
    fingerprint
}

impl StarPlacement {
    /// Where a sky position lands on the composition grid, zero-based with
    /// pixel centers on whole numbers.
    pub(super) fn grid_pixel(&self, ra: f64, dec: f64) -> Option<(f64, f64)> {
        let (x, y) = self.wcs.world_to_pixel(ra, dec)?;
        Some(self.orientation.source_to_output.apply(x, y))
    }

    /// The sky circle that covers the whole grid: its center and radius in
    /// degrees.
    fn field(&self) -> ((f64, f64), f64) {
        let width = self.orientation.output_width as f64;
        let height = self.orientation.output_height as f64;
        let to_sky = |x: f64, y: f64| {
            let (x, y) = self.orientation.source_to_output.inverse_apply(x, y);
            self.wcs.pixel_to_world(x, y)
        };
        let center = to_sky((width - 1.0) / 2.0, (height - 1.0) / 2.0);
        let radius = [
            (-0.5, -0.5),
            (width - 0.5, -0.5),
            (-0.5, height - 0.5),
            (width - 0.5, height - 0.5),
        ]
        .into_iter()
        .map(|(x, y)| {
            let corner = to_sky(x, y);
            crate::astrometry::angular_separation_deg(center.0, center.1, corner.0, corner.1)
        })
        .fold(0.0_f64, f64::max);
        (center, radius * 1.02)
    }

    /// Every source that lands on the grid. Sources without a trusted color
    /// stay as neighbours: they cannot calibrate, but still disqualify the
    /// calibrators they crowd.
    pub(super) fn place(&self, sources: &[GaiaColorSource]) -> Vec<ColorReferenceStar> {
        let width = self.orientation.output_width as f64;
        let height = self.orientation.output_height as f64;
        sources
            .iter()
            .filter_map(|source| {
                let bp_rp = source
                    .bp_rp
                    .filter(|colour| colour.is_finite())
                    .filter(|_| source.ruwe.is_none_or(|ruwe| ruwe < MAX_COLOR_RUWE));
                // Catalog positions are already at the catalog's epoch.
                let (x, y) = self.grid_pixel(source.ra, source.dec)?;
                (x >= -0.5 && y >= -0.5 && x < width - 0.5 && y < height - 0.5).then_some(
                    ColorReferenceStar {
                        x,
                        y,
                        g: source.g,
                        bp_rp,
                    },
                )
            })
            .collect()
    }

    /// The catalog's stars over the grid.
    pub(super) fn gaia_sources(
        &self,
        catalog: &seiza::catalog::PhotometryCatalog,
    ) -> Vec<GaiaColorSource> {
        let ((ra, dec), radius) = self.field();
        catalog
            .cone_search(ra, dec, radius, GAIA_MAX_MAGNITUDE)
            .into_iter()
            .map(|star| GaiaColorSource {
                ra: star.ra,
                dec: star.dec,
                pmra: None,
                pmdec: None,
                g: star.g,
                // An unreliable colour still marks a neighbour.
                bp_rp: star.calibration_bp_rp(),
                ruwe: None,
            })
            .collect()
    }
}

/// Calibrate the red, green and blue channels in place from `stars` on
/// their shared grid. A calibration that cannot run leaves every channel
/// as it was.
pub(super) fn calibrate_channels(
    channels: [&mut LinearImage; 3],
    stars: &[ColorReferenceStar],
    settings: &StackColorCalibration,
) -> StackColorCalibrationOutcome {
    let options = settings.options();
    let coloured = stars.iter().filter(|star| star.bp_rp.is_some()).count();
    if coloured < options.minimum_stars {
        return StackColorCalibrationOutcome::Skipped(StackColorCalibrationSkip::new(
            StackColorCalibrationSkipReason::TooFewStars,
            format!(
                "{coloured} Gaia stars with colors on the image; at least {} are needed",
                options.minimum_stars
            ),
        ));
    }
    let [red, green, blue] = channels;
    let rgb = match interleave(red, green, blue) {
        Ok(rgb) => rgb,
        Err(error) => {
            return StackColorCalibrationOutcome::Skipped(StackColorCalibrationSkip::new(
                StackColorCalibrationSkipReason::Failed,
                error,
            ))
        }
    };
    let calibration = match calibrate_color(&rgb, stars, &options) {
        Ok(calibration) => calibration,
        Err(error) => {
            let detail = error.to_string();
            // Seiza's refusals for a thin field all count stars.
            let reason = if detail.contains("stars") {
                StackColorCalibrationSkipReason::TooFewStars
            } else {
                StackColorCalibrationSkipReason::Failed
            };
            return StackColorCalibrationOutcome::Skipped(StackColorCalibrationSkip::new(
                reason, detail,
            ));
        }
    };
    let mut rgb = rgb;
    if let Err(error) = apply_color_gains(&mut rgb.data, calibration.gains, calibration.offsets) {
        return StackColorCalibrationOutcome::Skipped(StackColorCalibrationSkip::new(
            StackColorCalibrationSkipReason::Failed,
            error.to_string(),
        ));
    }
    for (channel, image) in [red, green, blue].into_iter().enumerate() {
        for (sample, pixel) in image.data.iter_mut().zip(rgb.data.chunks_exact(3)) {
            *sample = pixel[channel];
        }
    }
    StackColorCalibrationOutcome::Calibrated(StackColorCalibrationReport {
        gains: calibration.gains,
        offsets: calibration.offsets,
        background: calibration.background,
        white_bp_rp: calibration.white_bp_rp,
        red_fit: calibration.red_fit.into(),
        blue_fit: calibration.blue_fit.into(),
        aperture_radius: calibration.aperture_radius,
        stars_offered: calibration.stars_offered,
        stars_measured: calibration.stars_measured,
        stars_used: calibration.red_fit.stars.min(calibration.blue_fit.stars),
    })
}

/// Deinterleave an RGB image into three mono channels.
pub(super) fn deinterleave(rgb: &LinearImage) -> Result<[LinearImage; 3], String> {
    if rgb.channels != 3 {
        return Err("An RGB image has three channels".into());
    }
    let plane = |channel: usize| {
        let data = rgb
            .data
            .chunks_exact(3)
            .map(|pixel| pixel[channel])
            .collect();
        LinearImage::new(rgb.width, rgb.height, 1, data).map_err(|error| error.to_string())
    };
    Ok([plane(0)?, plane(1)?, plane(2)?])
}

/// Interleave three mono channels on one grid into an RGB image.
pub(super) fn interleave(
    red: &LinearImage,
    green: &LinearImage,
    blue: &LinearImage,
) -> Result<LinearImage, String> {
    let channels = [red, green, blue];
    if channels
        .iter()
        .any(|image| image.channels != 1 || image.width != red.width || image.height != red.height)
    {
        return Err("Color calibration needs three mono channels on one grid".into());
    }
    let mut data = Vec::with_capacity(red.data.len() * 3);
    for index in 0..red.data.len() {
        data.extend(channels.iter().map(|image| image.data[index]));
    }
    LinearImage::new(red.width, red.height, 3, data).map_err(|error| error.to_string())
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    fn orientation(width: usize, height: usize) -> StackSkyOrientation {
        StackSkyOrientation {
            convention: seiza_stacking::SKY_ORIENTATION_NAME.into(),
            version: seiza_stacking::SKY_ORIENTATION_VERSION,
            source: "test".into(),
            output_width: width,
            output_height: height,
            source_to_output: seiza_stacking::AffineTransform::IDENTITY,
        }
    }

    /// One arcsecond pixels, north up and east left, (56.75, 24.1) at
    /// pixel (500, 400).
    fn wcs() -> seiza::Wcs {
        seiza::Wcs::from_center_scale_rotation((56.75, 24.1), (500.0, 400.0), 1.0, 0.0, false)
    }

    #[test]
    fn a_sky_position_lands_on_the_grid_through_the_solve_and_the_stack_layout() {
        let wcs = wcs();
        // A star 20 pixels left of and 30 above the solve's reference point.
        let (ra, dec) = wcs.pixel_to_world(480.0, 370.0);
        let identity = StarPlacement {
            wcs: wcs.clone(),
            orientation: orientation(1000, 800),
        };
        let (x, y) = identity.grid_pixel(ra, dec).unwrap();
        assert!(
            (x - 480.0).abs() < 1e-6 && (y - 370.0).abs() < 1e-6,
            "{x} {y}"
        );

        // A stack published half a turn round, and drizzled 2x: the same
        // star sits at 2 * (999 - 480), 2 * (799 - 370) on the 2000 x 1600
        // grid.
        let turned = StarPlacement {
            wcs,
            orientation: StackSkyOrientation {
                source_to_output: seiza_stacking::AffineTransform {
                    matrix: [[-2.0, 0.0], [0.0, -2.0]],
                    translation_x: 2.0 * 999.0,
                    translation_y: 2.0 * 799.0,
                },
                ..orientation(2000, 1600)
            },
        };
        let (x, y) = turned.grid_pixel(ra, dec).unwrap();
        assert!(
            (x - 1038.0).abs() < 1e-6 && (y - 858.0).abs() < 1e-6,
            "{x} {y}"
        );

        let stars = turned.place(&[
            GaiaColorSource {
                ra,
                dec,
                pmra: None,
                pmdec: None,
                g: 11.0,
                bp_rp: Some(0.8),
                ruwe: None,
            },
            // Off the grid entirely.
            GaiaColorSource {
                ra: ra + 2.0,
                dec,
                pmra: None,
                pmdec: None,
                g: 11.0,
                bp_rp: Some(0.8),
                ruwe: None,
            },
            // A likely binary keeps no color.
            GaiaColorSource {
                ra,
                dec: dec + 0.01,
                pmra: None,
                pmdec: None,
                g: 11.0,
                bp_rp: Some(0.8),
                ruwe: Some(2.0),
            },
        ]);
        assert_eq!(stars.len(), 2);
        assert!((stars[0].x - 1038.0).abs() < 1e-6);
        assert_eq!(stars[0].bp_rp, Some(0.8));
        assert_eq!(stars[1].bp_rp, None);

        // The cone covers the grid's corners.
        let ((center_ra, center_dec), radius) = turned.field();
        let corner = distance_to_source_corner(&turned, center_ra, center_dec);
        assert!(
            radius >= corner && radius < corner * 1.05,
            "{radius} {corner}"
        );
    }

    fn distance_to_source_corner(placement: &StarPlacement, ra: f64, dec: f64) -> f64 {
        let (corner_ra, corner_dec) = placement.wcs.pixel_to_world(-0.5, -0.5);
        crate::astrometry::angular_separation_deg(ra, dec, corner_ra, corner_dec)
    }

    /// Three registered mono channels whose camera records red at 0.6 and
    /// blue at 1.4 times a neutral response for a Sun-colored star, with
    /// redder stars brighter in red; Seiza's own test field, split by
    /// channel. Gaia stars are placed through `placement`.
    fn field(placement: &StarPlacement) -> ([LinearImage; 3], Vec<GaiaColorSource>) {
        let (width, height) = (
            placement.orientation.output_width,
            placement.orientation.output_height,
        );
        let sky = [120.0_f32, 100.0, 90.0];
        let mut data = [
            vec![0.0_f32; width * height],
            vec![0.0_f32; width * height],
            vec![0.0_f32; width * height],
        ];
        for (channel, plane) in data.iter_mut().enumerate() {
            for (index, value) in plane.iter_mut().enumerate() {
                let noise = ((index * 3 + channel) * 7919 % 13) as f32 * 0.3;
                *value = sky[channel] + noise;
            }
        }
        let mut sources = Vec::new();
        for index in 0..140 {
            let x = 30.0 + ((index * 7919) % (width - 60)) as f64 + ((index % 7) as f64) * 0.13;
            let y = 30.0 + ((index * 6271) % (height - 60)) as f64 + ((index % 5) as f64) * 0.17;
            let bp_rp = 0.1 + ((index * 37) % 160) as f32 / 100.0;
            let g = 9.0 + ((index * 13) % 40) as f32 / 10.0;
            let flux = 2.0e5 * 10f64.powf(-0.4 * f64::from(g - 9.0));
            let delta = f64::from(bp_rp - SOLAR_BP_RP);
            let ratios = [
                0.6 * 10f64.powf(0.4 * 0.5 * delta),
                1.0,
                1.4 * 10f64.powf(-0.4 * 0.8 * delta),
            ];
            let sigma = 1.5_f64;
            for py in (y as usize).saturating_sub(10)..(y as usize + 11).min(height) {
                for px in (x as usize).saturating_sub(10)..(x as usize + 11).min(width) {
                    let r2 = (px as f64 - x).powi(2) + (py as f64 - y).powi(2);
                    let profile = (-r2 / (2.0 * sigma * sigma)).exp()
                        / (2.0 * std::f64::consts::PI * sigma * sigma);
                    for (channel, plane) in data.iter_mut().enumerate() {
                        plane[py * width + px] += (flux * ratios[channel] * profile) as f32;
                    }
                }
            }
            let (source_x, source_y) = placement.orientation.source_to_output.inverse_apply(x, y);
            let (ra, dec) = placement.wcs.pixel_to_world(source_x, source_y);
            sources.push(GaiaColorSource {
                ra,
                dec,
                pmra: None,
                pmdec: None,
                g,
                bp_rp: Some(bp_rp),
                ruwe: None,
            });
        }
        let [red, green, blue] = data;
        (
            [
                LinearImage::new(width, height, 1, red).unwrap(),
                LinearImage::new(width, height, 1, green).unwrap(),
                LinearImage::new(width, height, 1, blue).unwrap(),
            ],
            sources,
        )
    }

    #[test]
    fn calibration_recovers_known_channel_gains_through_the_stack_layout() {
        let placement = StarPlacement {
            wcs: wcs(),
            orientation: StackSkyOrientation {
                source_to_output: seiza_stacking::AffineTransform {
                    matrix: [[-1.0, 0.0], [0.0, -1.0]],
                    translation_x: 1199.0,
                    translation_y: 999.0,
                },
                ..orientation(1200, 1000)
            },
        };
        let ([mut red, mut green, mut blue], sources) = field(&placement);
        let stars = placement.place(&sources);
        assert_eq!(stars.len(), sources.len());
        let outcome = calibrate_channels(
            [&mut red, &mut green, &mut blue],
            &stars,
            &StackColorCalibration::default(),
        );
        let StackColorCalibrationOutcome::Calibrated(report) = outcome else {
            panic!("{outcome:?}");
        };
        assert!((report.gains[0] - 1.0 / 0.6).abs() < 0.02, "{report:?}");
        assert_eq!(report.gains[1], 1.0);
        assert!((report.gains[2] - 1.0 / 1.4).abs() < 0.02, "{report:?}");
        assert!(report.stars_used >= 20, "{report:?}");
        // The channels now carry the gains, and the sky is one gray.
        let median = |image: &LinearImage| {
            let mut values = image.data.clone();
            values.sort_by(f32::total_cmp);
            values[values.len() / 2]
        };
        let (r, g, b) = (median(&red), median(&green), median(&blue));
        assert!((r - g).abs() < 1.0 && (b - g).abs() < 1.0, "{r} {g} {b}");
    }

    #[test]
    fn stars_come_from_the_installed_gaia_catalog() {
        let placement = StarPlacement {
            wcs: wcs(),
            orientation: orientation(1200, 1000),
        };
        let ([mut red, mut green, mut blue], sources) = field(&placement);
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("stars-gaia-photometry.bin");
        let mut builder = seiza::catalog::PhotometryCatalogBuilder::new(90, 2016.0, "test");
        for source in &sources {
            builder.add(seiza::catalog::PhotometricStar {
                ra: source.ra,
                dec: source.dec,
                g: source.g,
                bp_rp: source.bp_rp,
                reliable: true,
            });
        }
        // A star far off the field, which the cone leaves out.
        builder.add(seiza::catalog::PhotometricStar {
            ra: 120.0,
            dec: -60.0,
            g: 8.0,
            bp_rp: Some(0.8),
            reliable: true,
        });
        builder.write_to(&path).unwrap();
        let catalog = seiza::catalog::PhotometryCatalog::open(&path).unwrap();

        let found = placement.gaia_sources(&catalog);
        assert_eq!(found.len(), sources.len());
        let stars = placement.place(&found);
        let outcome = calibrate_channels(
            [&mut red, &mut green, &mut blue],
            &stars,
            &StackColorCalibration::default(),
        );
        let StackColorCalibrationOutcome::Calibrated(report) = outcome else {
            panic!("{outcome:?}");
        };
        assert!((report.gains[0] - 1.0 / 0.6).abs() < 0.03, "{report:?}");
        assert!((report.gains[2] - 1.0 / 1.4).abs() < 0.03, "{report:?}");
    }

    #[test]
    fn a_field_with_too_few_stars_is_left_alone_and_says_why() {
        let placement = StarPlacement {
            wcs: wcs(),
            orientation: orientation(1200, 1000),
        };
        let ([mut red, mut green, mut blue], sources) = field(&placement);
        let before = [red.clone(), green.clone(), blue.clone()];
        let stars = placement.place(&sources[..8]);
        let outcome = calibrate_channels(
            [&mut red, &mut green, &mut blue],
            &stars,
            &StackColorCalibration::default(),
        );
        let StackColorCalibrationOutcome::Skipped(skip) = outcome else {
            panic!("{outcome:?}");
        };
        assert_eq!(skip.reason, StackColorCalibrationSkipReason::TooFewStars);
        assert_eq!([red, green, blue], before);

        // Enough stars to try, but they all sit in one corner too crowded
        // to measure: Seiza's own refusal also counts as too few stars.
        let crowded = (0..30)
            .map(|index| ColorReferenceStar {
                x: 40.0 + (index % 6) as f64,
                y: 40.0 + (index / 6) as f64,
                g: 10.0,
                bp_rp: Some(0.8),
            })
            .collect::<Vec<_>>();
        let (mut red, mut green, mut blue) =
            (before[0].clone(), before[1].clone(), before[2].clone());
        let outcome = calibrate_channels(
            [&mut red, &mut green, &mut blue],
            &crowded,
            &StackColorCalibration::default(),
        );
        let StackColorCalibrationOutcome::Skipped(skip) = outcome else {
            panic!("{outcome:?}");
        };
        assert_eq!(skip.reason, StackColorCalibrationSkipReason::TooFewStars);
        assert_eq!([red, green, blue], before);
    }

    #[test]
    fn a_missing_solve_layout_or_catalog_plans_a_skip() {
        let catalog = AstrometryCatalogFileSignature {
            name: "gaia_photometry".into(),
            path: "/catalogs/stars-gaia-photometry.bin".into(),
            format: "SEIZAPH1".into(),
            size_bytes: 1,
            modified_unix_seconds: 1,
            modified_subsec_nanos: 0,
            sha256: None,
        };
        let layout = orientation(1000, 800);
        let settings = StackColorCalibration::default();

        let no_solve = plan(
            settings,
            Some(7),
            Some(&layout),
            None,
            Some(catalog.clone()),
        );
        assert_eq!(
            no_solve.placement.unwrap_err().reason,
            StackColorCalibrationSkipReason::NoPlateSolve
        );
        assert_eq!(no_solve.basis.solution_fingerprint, None);

        let solution = solution();
        let no_catalog = plan(settings, Some(7), Some(&layout), Some(&solution), None);
        assert_eq!(
            no_catalog.placement.unwrap_err().reason,
            StackColorCalibrationSkipReason::NoGaiaCatalog
        );

        let no_layout = plan(
            settings,
            Some(7),
            None,
            Some(&solution),
            Some(catalog.clone()),
        );
        assert_eq!(
            no_layout.placement.unwrap_err().reason,
            StackColorCalibrationSkipReason::Failed
        );

        let ready = plan(
            settings,
            Some(7),
            Some(&layout),
            Some(&solution),
            Some(catalog),
        );
        assert!(ready.placement.is_ok());
        assert!(ready.basis.solution_fingerprint.is_some());
        assert_ne!(ready.basis, no_catalog.basis);
    }

    #[test]
    fn white_references_outside_the_star_range_are_refused() {
        assert!(StackColorCalibration::default().validate().is_ok());
        for value in [f32::NAN, -1.0, 5.0] {
            let settings = StackColorCalibration {
                white_reference: StackColorWhiteReference::BpRp(value),
                ..StackColorCalibration::default()
            };
            assert!(settings.validate().is_err());
        }
        let parsed: StackColorCalibration =
            serde_json::from_str(r#"{"white_reference":{"bp_rp":0.65}}"#).unwrap();
        assert!(parsed.enabled && parsed.neutralize_background);
        assert_eq!(parsed.white_reference, StackColorWhiteReference::BpRp(0.65));
    }

    pub(in crate::server::stack_preview) fn solution() -> AstrometrySolutionResponse {
        let wcs = wcs();
        AstrometrySolutionResponse {
            center_ra_deg: 56.75,
            center_dec_deg: 24.1,
            pixel_scale_arcsec_per_pixel: 1.0,
            matched_stars: 100,
            rms_arcsec: 0.5,
            image_width: 1000,
            image_height: 800,
            wcs: crate::astrometry::WcsResponse {
                crval: [wcs.crval.0, wcs.crval.1],
                crpix: [wcs.crpix.0, wcs.crpix.1],
                cd: wcs.cd,
                ctype: ["RA---TAN".into(), "DEC--TAN".into()],
                cunit: ["deg".into(), "deg".into()],
                radesys: "ICRS".into(),
                equinox: 2000.0,
            },
            footprint: Vec::new(),
            objects: Vec::new(),
            catalog_version: None,
            capture_time: None,
        }
    }
}
