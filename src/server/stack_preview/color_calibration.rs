//! Photometric color calibration of RGB and LRGB previews against Gaia.
//!
//! Seiza does the work (`seiza_stacking::calibrate_and_stretch` and its
//! calibration half, `calibrate_color_image`). This module plans it and
//! gives Seiza the solution of the image it calibrates: the reference
//! frame's fresh pixel solve, or a solve of the generated mosaic canvas,
//! carried through the stack's recorded layout and composition crop with
//! `seiza_stacking::transform_wcs`.
//!
//! Narrowband palettes are false color and never calibrate.

use seiza_stacking::{
    AffineTransform, ColorCalibrationOptions, GaiaColors, LinearImage, ReferenceRegion, SOLAR_BP_RP,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt::Write as _;

use super::StackSkyOrientation;
use crate::astrometry::{AstrometryCatalogFileSignature, AstrometrySolutionResponse};

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

    pub(super) fn options(&self) -> ColorCalibrationOptions {
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
    /// A generated mosaic has its own pixel solve, without an acquired-image ID.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub mosaic_canvas: bool,
    /// Available solver resources while an unsolved mosaic is awaiting a retry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mosaic_retry_fingerprint: Option<String>,
    /// The acquired reference frame; absent for a generated mosaic canvas.
    pub reference_image_id: Option<i32>,
    /// SHA-256 of the reference-frame or mosaic-canvas pixel solve.
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
    /// The generated mosaic canvas has no usable pixel solve.
    NoMosaicSolve,
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
    /// The channels had stretches of their own and were stretched one by
    /// one after calibration, which pulls each toward its own median and
    /// undoes most of the balance.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stretched_separately: bool,
}

/// What color calibration did for one preview.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum StackColorCalibrationOutcome {
    Calibrated(StackColorCalibrationReport),
    Skipped(StackColorCalibrationSkip),
}

/// How Gaia stars reach the composition grid: its pixel solve and layout.
/// A mosaic's solution is already on the integrated canvas.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct StarPlacement {
    #[serde(with = "wcs_form")]
    pub(super) wcs: seiza::Wcs,
    pub(super) orientation: StackSkyOrientation,
}

/// A prepared calibration: what it rests on, and either the way to place
/// stars or the reason it cannot run.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
        mosaic_canvas: super::canvas_astrometry::is_mosaic(orientation),
        mosaic_retry_fingerprint: None,
        reference_image_id,
        solution_fingerprint,
        catalog: catalog.clone(),
    };
    let placement = match (solution, orientation) {
        (None, Some(orientation)) if super::canvas_astrometry::is_mosaic(Some(orientation)) => {
            Err(StackColorCalibrationSkip::new(
                if catalog.is_none() {
                    StackColorCalibrationSkipReason::NoGaiaCatalog
                } else {
                    StackColorCalibrationSkipReason::NoMosaicSolve
                },
                if catalog.is_none() {
                    "Gaia colors are not installed"
                } else {
                    "The mosaic canvas has no usable pixel solve; PSF Guard solves it when color calibration runs"
                },
            ))
        }
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

impl From<seiza_stacking::ColorCalibrationOutcome> for StackColorCalibrationOutcome {
    fn from(outcome: seiza_stacking::ColorCalibrationOutcome) -> Self {
        use seiza_stacking::ColorCalibrationSkipReason as Seiza;
        match outcome {
            seiza_stacking::ColorCalibrationOutcome::Calibrated(calibration) => {
                Self::Calibrated(StackColorCalibrationReport {
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
                    stretched_separately: false,
                })
            }
            seiza_stacking::ColorCalibrationOutcome::Skipped(skip) => {
                let reason = match skip.reason {
                    Seiza::NoSolution => StackColorCalibrationSkipReason::NoPlateSolve,
                    Seiza::NoCatalog => StackColorCalibrationSkipReason::NoGaiaCatalog,
                    Seiza::NoStars | Seiza::TooFewStars => {
                        StackColorCalibrationSkipReason::TooFewStars
                    }
                    Seiza::NotRgb | Seiza::InvalidOptions | Seiza::FitFailed => {
                        StackColorCalibrationSkipReason::Failed
                    }
                };
                Self::Skipped(StackColorCalibrationSkip::new(reason, skip.detail))
            }
        }
    }
}

impl StackColorCalibrationOutcome {
    /// A skip for something PSF Guard found before Seiza ran.
    pub(super) fn skipped(
        reason: StackColorCalibrationSkipReason,
        detail: impl Into<String>,
    ) -> Self {
        Self::Skipped(StackColorCalibrationSkip::new(reason, detail))
    }

    pub(super) fn is_calibrated(&self) -> bool {
        matches!(self, Self::Calibrated(_))
    }
}

impl StarPlacement {
    /// The solution of the grid every channel is registered to, or of the
    /// part of it a composition kept: the reference frame's solve carried
    /// through the stack's layout and the crop.
    pub(super) fn wcs_on(&self, crop: Option<ReferenceRegion>) -> Result<seiza::Wcs, String> {
        let mut source_to_image = self.orientation.source_to_output;
        if let Some(region) = crop {
            source_to_image = source_to_image.then(AffineTransform {
                translation_x: -(region.x as f64),
                translation_y: -(region.y as f64),
                ..AffineTransform::IDENTITY
            });
        }
        seiza_stacking::transform_wcs(&self.wcs, source_to_image)
            .map_err(|error| format!("The reference solve does not carry to the stack: {error}"))
    }
}

/// Calibrate the registered red, green and blue channels in place against
/// `catalog`, through `wcs`, the solution of their shared grid. Seiza
/// leaves the channels as they were when it cannot calibrate.
pub(super) fn calibrate_channels(
    channels: [&mut LinearImage; 3],
    wcs: &seiza::Wcs,
    catalog: &seiza::catalog::PhotometryCatalog,
    settings: &StackColorCalibration,
    clipped: Option<&LinearImage>,
) -> StackColorCalibrationOutcome {
    let [red, green, blue] = channels;
    let mut rgb = match interleave(red, green, blue) {
        Ok(rgb) => rgb,
        Err(error) => {
            return StackColorCalibrationOutcome::skipped(
                StackColorCalibrationSkipReason::Failed,
                error,
            )
        }
    };
    let outcome = seiza_stacking::calibrate_color_image(
        &mut rgb,
        Some(wcs),
        GaiaColors::Catalog(catalog),
        &settings.options(),
    );
    if outcome.calibration().is_some() {
        neutralize_clipped(&mut rgb, clipped);
        for (channel, image) in [red, green, blue].into_iter().enumerate() {
            for (sample, pixel) in image.data.iter_mut().zip(rgb.data.as_chunks::<3>().0) {
                *sample = pixel[channel];
            }
        }
    }
    outcome.into()
}

/// What an OSC stack offers color calibration: its reference-frame or
/// mosaic-canvas pixel solve, sky layout, and installed Gaia catalog, read
/// when the stack's processing is requested.
#[derive(Debug, Clone)]
pub struct StackCalibrationSource {
    pub(super) solve_error: Option<String>,
    pub(super) mosaic_retry_fingerprint: Option<String>,
    pub(super) reference_image_id: Option<i32>,
    pub(super) orientation: Option<StackSkyOrientation>,
    pub(super) solution: Option<AstrometrySolutionResponse>,
    pub(super) catalog: Option<AstrometryCatalogFileSignature>,
}

impl StackCalibrationSource {
    /// What a calibration made now would rest on.
    pub(super) fn basis(&self) -> StackColorCalibrationBasis {
        StackColorCalibrationBasis {
            mosaic_canvas: super::canvas_astrometry::is_mosaic(self.orientation.as_ref()),
            mosaic_retry_fingerprint: self.mosaic_retry_fingerprint.clone(),
            reference_image_id: self.reference_image_id,
            solution_fingerprint: self.solution.as_ref().map(solution_fingerprint),
            catalog: self.catalog.clone(),
        }
    }

    pub(super) fn plan(&self, settings: StackColorCalibration) -> ColorCalibrationPlan {
        let mut plan = plan(
            settings,
            self.reference_image_id,
            self.orientation.as_ref(),
            self.solution.as_ref(),
            self.catalog.clone(),
        );
        plan.basis.mosaic_retry_fingerprint = self.mosaic_retry_fingerprint.clone();
        if let Err(skip) = &mut plan.placement
            && skip.reason == StackColorCalibrationSkipReason::NoMosaicSolve
            && let Some(error) = &self.solve_error
        {
            skip.detail = format!("The mosaic canvas could not be plate solved: {error}");
        }
        plan
    }
}

/// Why a one-shot-color result is out of date, if it is: the solve or the
/// Gaia catalog its calibration rested on (`recorded`) is not what
/// `current` offers now. A result recorded before the basis was kept is
/// judged by its outcome instead: a skip for a missing catalog or solve is
/// out of date once there is one. Text in the card's "Out of date — …"
/// form, without the action.
pub(super) fn calibration_outdated(
    recorded: Option<&StackColorCalibrationBasis>,
    outcome: Option<&StackColorCalibrationOutcome>,
    current: &StackCalibrationSource,
) -> Option<&'static str> {
    let now = current.basis();
    let Some(recorded) = recorded else {
        return match outcome {
            Some(StackColorCalibrationOutcome::Skipped(skip))
                if now.mosaic_canvas
                    && skip.reason == StackColorCalibrationSkipReason::NoPlateSolve =>
            {
                Some("Out of date — mosaic color calibration now uses the canvas plate solve")
            }
            Some(StackColorCalibrationOutcome::Skipped(skip))
                if skip.reason == StackColorCalibrationSkipReason::NoGaiaCatalog
                    && now.catalog.is_some() =>
            {
                Some("Out of date — Gaia colors were installed")
            }
            Some(StackColorCalibrationOutcome::Skipped(skip))
                if skip.reason == StackColorCalibrationSkipReason::NoPlateSolve
                    && now.solution_fingerprint.is_some() =>
            {
                Some("Out of date — the reference frame is now plate solved")
            }
            _ => None,
        };
    };
    if recorded.mosaic_canvas != now.mosaic_canvas {
        return Some("Out of date — mosaic color calibration now uses the canvas plate solve");
    }
    match (&recorded.catalog, &now.catalog) {
        (None, Some(_)) => return Some("Out of date — Gaia colors were installed"),
        (Some(_), None) => return Some("Out of date — Gaia colors were removed"),
        (Some(then), Some(now)) if then != now => return Some("Out of date — Gaia colors changed"),
        _ => {}
    }
    if recorded.mosaic_retry_fingerprint != now.mosaic_retry_fingerprint {
        return Some("Out of date — the mosaic plate-solving resources changed");
    }
    if recorded.reference_image_id != now.reference_image_id {
        return Some("Out of date — the reference frame changed");
    }
    match (&recorded.solution_fingerprint, &now.solution_fingerprint) {
        (None, Some(_)) => Some("Out of date — the reference frame is now plate solved"),
        (then, now) if then != now => {
            Some("Out of date — the reference frame's plate solve changed")
        }
        _ => None,
    }
}

/// Calibrate a linear RGB stack, as a one-shot-color camera made it, on its
/// own grid. Returns the calibrated copy, or `None` when calibration did
/// not run, with what happened. `clipped`, the stack's clipped map on the
/// same grid, renders the calibrated copy's clipped pixels neutral, so the
/// channels' gains do not tint a clipped star core.
pub(super) fn calibrate_stack(
    plan: &ColorCalibrationPlan,
    image: &LinearImage,
    clipped: Option<&LinearImage>,
    open_catalog: &dyn Fn() -> Result<std::sync::Arc<seiza::catalog::PhotometryCatalog>, String>,
) -> (Option<LinearImage>, StackColorCalibrationOutcome) {
    use StackColorCalibrationSkipReason as Reason;

    let placement = match &plan.placement {
        Ok(placement) => placement,
        Err(skip) => return (None, StackColorCalibrationOutcome::Skipped(skip.clone())),
    };
    if image.channels != 3 {
        return (
            None,
            StackColorCalibrationOutcome::skipped(Reason::Failed, "The stack is not RGB"),
        );
    }
    if (image.width, image.height)
        != (
            placement.orientation.output_width,
            placement.orientation.output_height,
        )
    {
        return (
            None,
            StackColorCalibrationOutcome::skipped(
                Reason::Failed,
                "The stack's sky layout does not match its pixels; rebuild it",
            ),
        );
    }
    let wcs = match placement.wcs_on(None) {
        Ok(wcs) => wcs,
        Err(error) => {
            return (
                None,
                StackColorCalibrationOutcome::skipped(Reason::Failed, error),
            )
        }
    };
    let catalog = match open_catalog() {
        Ok(catalog) => catalog,
        Err(error) => {
            return (
                None,
                StackColorCalibrationOutcome::skipped(
                    Reason::NoGaiaCatalog,
                    format!("Gaia colors could not be opened: {error}"),
                ),
            )
        }
    };
    let mut calibrated = image.clone();
    let outcome: StackColorCalibrationOutcome = seiza_stacking::calibrate_color_image(
        &mut calibrated,
        Some(&wcs),
        GaiaColors::Catalog(&catalog),
        &plan.settings.options(),
    )
    .into();
    if outcome.is_calibrated() {
        neutralize_clipped(&mut calibrated, clipped);
        (Some(calibrated), outcome)
    } else {
        (None, outcome)
    }
}

/// Render neutral the pixels of calibrated RGB `image` that `clipped` marks,
/// when there is a map. One that does not fit the image, as a stack whose
/// map was written for another grid, is logged and left out: the image
/// stays calibrated, only its clipped cores keep the gains' tint.
pub(super) fn neutralize_clipped(image: &mut LinearImage, clipped: Option<&LinearImage>) {
    if let Some(clipped) = clipped
        && let Err(error) = seiza_stacking::neutralize_clipped(image, clipped)
    {
        tracing::warn!("Not rendering clipped pixels neutral: {error}");
    }
}

/// Note on a calibrated outcome that the stretch after it treats each
/// channel on its own, which undoes most of the balance.
pub(super) fn mark_stretched_separately(outcome: &mut StackColorCalibrationOutcome) {
    if let StackColorCalibrationOutcome::Calibrated(report) = outcome {
        report.stretched_separately = true;
    }
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

    #[test]
    fn mosaic_calibration_uses_the_canvas_solve_without_a_reference_image() {
        let directory = tempfile::tempdir().unwrap();
        let layout = StackSkyOrientation {
            source: "mosaic canvas".into(),
            ..orientation(1200, 1000)
        };
        let placement = StarPlacement {
            wcs: wcs(),
            orientation: layout.clone(),
        };
        let ([mut red, mut green, mut blue], stars) = field(&placement);
        let gaia = catalog(directory.path(), &stars);
        let signature = AstrometryCatalogFileSignature {
            name: "gaia_photometry".into(),
            path: directory
                .path()
                .join("stars-gaia-photometry.bin")
                .display()
                .to_string(),
            format: "SEIZAPH1".into(),
            size_bytes: 1,
            modified_unix_seconds: 1,
            modified_subsec_nanos: 0,
            sha256: None,
        };
        let mut solved = solution();
        solved.image_width = 1200;
        solved.image_height = 1000;
        let plan = plan(
            StackColorCalibration::default(),
            None,
            Some(&layout),
            Some(&solved),
            Some(signature),
        );
        assert!(plan.basis.mosaic_canvas);
        assert_eq!(plan.basis.reference_image_id, None);
        let stack = interleave(&red, &green, &blue).unwrap();
        let placement = plan.placement.as_ref().unwrap();
        let (ra, dec) = wcs().pixel_to_world(300.0, 200.0);
        let (x, y) = placement
            .wcs_on(Some(ReferenceRegion {
                x: 100,
                y: 80,
                width: 1000,
                height: 800,
            }))
            .unwrap()
            .world_to_pixel(ra, dec)
            .unwrap();
        assert!(
            (x - 200.0).abs() < 1e-6 && (y - 120.0).abs() < 1e-6,
            "{x} {y}"
        );
        let outcome = calibrate_channels(
            [&mut red, &mut green, &mut blue],
            &placement.wcs_on(None).unwrap(),
            &gaia,
            &StackColorCalibration::default(),
            None,
        );
        let StackColorCalibrationOutcome::Calibrated(report) = outcome else {
            panic!("{outcome:?}");
        };
        assert!(report.stars_used >= 20);
        assert!((report.gains[0] - 1.0 / 0.6).abs() < 0.03);
        assert!((report.gains[2] - 1.0 / 1.4).abs() < 0.03);
        let gaia = std::sync::Arc::new(gaia);
        let (calibrated, outcome) = calibrate_stack(&plan, &stack, None, &|| Ok(gaia.clone()));
        assert!(calibrated.is_some(), "{outcome:?}");
        assert!(matches!(
            outcome,
            StackColorCalibrationOutcome::Calibrated(_)
        ));
    }

    #[test]
    fn an_unsolved_mosaic_reports_the_canvas_and_invalidates_legacy_previews() {
        let layout = StackSkyOrientation {
            source: "mosaic canvas".into(),
            ..orientation(1000, 800)
        };
        let settings = StackColorCalibration::default();
        let missing = plan(settings, None, Some(&layout), None, None);
        assert_eq!(
            missing.placement.unwrap_err().reason,
            StackColorCalibrationSkipReason::NoGaiaCatalog
        );
        let signature = AstrometryCatalogFileSignature {
            name: "gaia_photometry".into(),
            path: "/test/gaia.bin".into(),
            format: "SEIZAPH1".into(),
            size_bytes: 1,
            modified_unix_seconds: 1,
            modified_subsec_nanos: 0,
            sha256: None,
        };
        let current = StackCalibrationSource {
            solve_error: None,
            mosaic_retry_fingerprint: None,
            reference_image_id: None,
            orientation: Some(layout),
            solution: None,
            catalog: Some(signature),
        };
        let missing = current.plan(settings);
        assert_eq!(
            missing.placement.unwrap_err().reason,
            StackColorCalibrationSkipReason::NoMosaicSolve
        );
        let mut old_basis = current.basis();
        old_basis.mosaic_canvas = false;
        assert!(calibration_outdated(Some(&old_basis), None, &current).is_some());
        assert_eq!(
            calibration_outdated(Some(&current.basis()), None, &current),
            None
        );
        let mut retry = current.clone();
        retry.mosaic_retry_fingerprint = Some("new solver resources".into());
        retry.solve_error = Some("star catalog unavailable".into());
        assert_eq!(
            calibration_outdated(Some(&current.basis()), None, &retry),
            Some("Out of date — the mosaic plate-solving resources changed")
        );
        assert!(retry
            .plan(settings)
            .placement
            .unwrap_err()
            .detail
            .contains("star catalog unavailable"));
        let mut without_gaia = current.basis();
        without_gaia.catalog = None;
        assert_eq!(
            calibration_outdated(Some(&without_gaia), None, &retry),
            Some("Out of date — Gaia colors were installed")
        );
    }
    use seiza_stacking::GaiaColorSource;

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

    /// A stack published half a turn round and drizzled 2x, from the
    /// 1000 x 800 reference frame `wcs` solves.
    fn turned() -> StarPlacement {
        StarPlacement {
            wcs: wcs(),
            orientation: StackSkyOrientation {
                source_to_output: seiza_stacking::AffineTransform {
                    matrix: [[-1.0, 0.0], [0.0, -1.0]],
                    translation_x: 999.0,
                    translation_y: 799.0,
                }
                .then(seiza_stacking::AffineTransform::scale_about_pixel_centres(
                    2.0,
                )),
                ..orientation(2000, 1600)
            },
        }
    }

    #[test]
    fn a_sky_position_lands_on_the_composed_image_through_the_layout_and_crop() {
        let placement = turned();
        // A star at reference-frame pixel (480, 370).
        let (ra, dec) = placement.wcs.pixel_to_world(480.0, 370.0);
        // Turned, it is at (519, 429); drizzled 2x, that pixel's centre is
        // at 2 * 519 + 0.5, 2 * 429 + 0.5 on the stack's grid.
        let grid = placement.wcs_on(None).unwrap();
        let (x, y) = grid.world_to_pixel(ra, dec).unwrap();
        assert!(
            (x - 1038.5).abs() < 1e-6 && (y - 858.5).abs() < 1e-6,
            "{x} {y}"
        );
        // A composition cropped at (100, 50) moves it by that much.
        let cropped = placement
            .wcs_on(Some(ReferenceRegion {
                x: 100,
                y: 50,
                width: 1800,
                height: 1500,
            }))
            .unwrap();
        let (x, y) = cropped.world_to_pixel(ra, dec).unwrap();
        assert!(
            (x - 938.5).abs() < 1e-6 && (y - 808.5).abs() < 1e-6,
            "{x} {y}"
        );
    }

    /// Three registered mono channels whose camera records red at 0.6 and
    /// blue at 1.4 times a neutral response for a Sun-colored star, with
    /// redder stars brighter in red, and the Gaia sources for their stars.
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

    /// A Gaia photometry catalog file holding `sources`.
    fn catalog(
        directory: &std::path::Path,
        sources: &[GaiaColorSource],
    ) -> seiza::catalog::PhotometryCatalog {
        let path = directory.join("stars-gaia-photometry.bin");
        let mut builder = seiza::catalog::PhotometryCatalogBuilder::new(90, 2016.0, "test");
        for source in sources {
            builder.add(seiza::catalog::PhotometricStar {
                ra: source.ra,
                dec: source.dec,
                g: source.g,
                bp_rp: source.bp_rp,
                reliable: true,
            });
        }
        builder.write_to(&path).unwrap();
        seiza::catalog::PhotometryCatalog::open(&path).unwrap()
    }

    #[test]
    fn registered_channels_calibrate_against_the_installed_catalog() {
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
        let directory = tempfile::tempdir().unwrap();
        let catalog = catalog(directory.path(), &sources);
        let outcome = calibrate_channels(
            [&mut red, &mut green, &mut blue],
            &placement.wcs_on(None).unwrap(),
            &catalog,
            &StackColorCalibration::default(),
            None,
        );
        let StackColorCalibrationOutcome::Calibrated(report) = outcome else {
            panic!("{outcome:?}");
        };
        assert!((report.gains[0] - 1.0 / 0.6).abs() < 0.03, "{report:?}");
        assert_eq!(report.gains[1], 1.0);
        assert!((report.gains[2] - 1.0 / 1.4).abs() < 0.03, "{report:?}");
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
    fn a_field_with_too_few_stars_is_left_alone_and_says_why() {
        let placement = StarPlacement {
            wcs: wcs(),
            orientation: orientation(1200, 1000),
        };
        let ([mut red, mut green, mut blue], sources) = field(&placement);
        let before = [red.clone(), green.clone(), blue.clone()];
        let directory = tempfile::tempdir().unwrap();
        let catalog = catalog(directory.path(), &sources[..8]);
        let outcome = calibrate_channels(
            [&mut red, &mut green, &mut blue],
            &placement.wcs_on(None).unwrap(),
            &catalog,
            &StackColorCalibration::default(),
            None,
        );
        let StackColorCalibrationOutcome::Skipped(skip) = outcome else {
            panic!("{outcome:?}");
        };
        assert_eq!(skip.reason, StackColorCalibrationSkipReason::TooFewStars);
        assert_eq!([red, green, blue], before);
    }

    #[test]
    fn a_one_shot_color_stack_calibrates_on_its_own_grid_or_says_why_not() {
        let placement = StarPlacement {
            wcs: wcs(),
            orientation: orientation(1200, 1000),
        };
        let ([red, green, blue], sources) = field(&placement);
        let stack = interleave(&red, &green, &blue).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let catalog = std::sync::Arc::new(catalog(directory.path(), &sources));
        let signature = AstrometryCatalogFileSignature {
            name: "gaia_photometry".into(),
            path: "/catalogs/stars-gaia-photometry.bin".into(),
            format: "SEIZAPH1".into(),
            size_bytes: 1,
            modified_unix_seconds: 1,
            modified_subsec_nanos: 0,
            sha256: None,
        };
        let mut solution = tests::solution();
        solution.image_width = 1200;
        solution.image_height = 1000;
        let source = StackCalibrationSource {
            solve_error: None,
            mosaic_retry_fingerprint: None,
            reference_image_id: Some(7),
            orientation: Some(orientation(1200, 1000)),
            solution: Some(solution),
            catalog: Some(signature),
        };
        let open = || Ok(std::sync::Arc::clone(&catalog));
        let (calibrated, outcome) = calibrate_stack(
            &source.plan(StackColorCalibration::default()),
            &stack,
            None,
            &open,
        );
        let StackColorCalibrationOutcome::Calibrated(report) = &outcome else {
            panic!("{outcome:?}");
        };
        assert!((report.gains[0] - 1.0 / 0.6).abs() < 0.03, "{report:?}");
        assert!((report.gains[2] - 1.0 / 1.4).abs() < 0.03, "{report:?}");
        let calibrated = calibrated.unwrap();
        assert_ne!(calibrated, stack);

        // A clipped map renders its fully clipped pixel neutral, at its
        // brightest channel, and leaves every other pixel as calibrated.
        let brightest = (0..stack.width * stack.height)
            .max_by(|&a, &b| stack.data[3 * a].total_cmp(&stack.data[3 * b]))
            .unwrap();
        let mut map = vec![0.0_f32; stack.width * stack.height];
        map[brightest] = 1.0;
        let map = LinearImage::new(stack.width, stack.height, 1, map).unwrap();
        let (neutral, _) = calibrate_stack(
            &source.plan(StackColorCalibration::default()),
            &stack,
            Some(&map),
            &open,
        );
        let neutral = neutral.unwrap();
        let pixel = |image: &LinearImage, at: usize| -> [f32; 3] {
            image.data[3 * at..3 * at + 3].try_into().unwrap()
        };
        let before = pixel(&calibrated, brightest);
        let top = before.iter().copied().fold(f32::MIN, f32::max);
        assert_ne!(before, [top; 3], "the test pixel must start tinted");
        for value in pixel(&neutral, brightest) {
            assert!(
                (value - top).abs() <= 1.0e-5 * top.abs(),
                "{value} against {top}"
            );
        }
        for at in (0..stack.width * stack.height).filter(|&at| at != brightest) {
            assert_eq!(pixel(&neutral, at), pixel(&calibrated, at), "pixel {at}");
        }
        let mut unlinked = outcome;
        mark_stretched_separately(&mut unlinked);
        let StackColorCalibrationOutcome::Calibrated(report) = unlinked else {
            unreachable!()
        };
        assert!(report.stretched_separately);

        // No solve: nothing to calibrate, and the stack is left to its
        // uncalibrated stretch.
        let unsolved = StackCalibrationSource {
            solution: None,
            ..source
        };
        let unopened = || -> Result<std::sync::Arc<seiza::catalog::PhotometryCatalog>, String> {
            panic!("an unsolved stack opens no catalog")
        };
        let (calibrated, outcome) = calibrate_stack(
            &unsolved.plan(StackColorCalibration::default()),
            &stack,
            None,
            &unopened,
        );
        assert!(calibrated.is_none());
        assert_eq!(
            outcome,
            StackColorCalibrationOutcome::skipped(
                StackColorCalibrationSkipReason::NoPlateSolve,
                "The reference frame has no plate solve"
            )
        );
    }

    #[test]
    fn a_new_solve_or_catalog_puts_a_color_stack_out_of_date() {
        let signature = |size_bytes| AstrometryCatalogFileSignature {
            name: "gaia_photometry".into(),
            path: "/catalogs/stars-gaia-photometry.bin".into(),
            format: "SEIZAPH1".into(),
            size_bytes,
            modified_unix_seconds: 1,
            modified_subsec_nanos: 0,
            sha256: None,
        };
        let source = |solved: bool, catalog: Option<u64>| StackCalibrationSource {
            solve_error: None,
            mosaic_retry_fingerprint: None,
            reference_image_id: Some(7),
            orientation: Some(orientation(1000, 800)),
            solution: solved.then(tests::solution),
            catalog: catalog.map(signature),
        };
        let ready = source(true, Some(10));
        let recorded = ready.basis();

        // Unchanged: not out of date.
        assert_eq!(calibration_outdated(Some(&recorded), None, &ready), None);
        // A catalog installed, replaced or removed since.
        let before = source(true, None).basis();
        assert_eq!(
            calibration_outdated(Some(&before), None, &ready),
            Some("Out of date — Gaia colors were installed")
        );
        assert_eq!(
            calibration_outdated(Some(&recorded), None, &source(true, Some(11))),
            Some("Out of date — Gaia colors changed")
        );
        assert_eq!(
            calibration_outdated(Some(&recorded), None, &source(true, None)),
            Some("Out of date — Gaia colors were removed")
        );
        // A solve made since, or a different one.
        let unsolved = source(false, Some(10)).basis();
        assert_eq!(
            calibration_outdated(Some(&unsolved), None, &ready),
            Some("Out of date — the reference frame is now plate solved")
        );
        let mut resolved = ready.clone();
        resolved.solution.as_mut().unwrap().wcs.crval[0] += 0.01;
        assert_eq!(
            calibration_outdated(Some(&recorded), None, &resolved),
            Some("Out of date — the reference frame's plate solve changed")
        );

        // Recorded before the basis was kept: judged by its skip.
        let skipped = |reason| StackColorCalibrationOutcome::skipped(reason, "detail");
        assert_eq!(
            calibration_outdated(
                None,
                Some(&skipped(StackColorCalibrationSkipReason::NoGaiaCatalog)),
                &ready
            ),
            Some("Out of date — Gaia colors were installed")
        );
        assert_eq!(
            calibration_outdated(
                None,
                Some(&skipped(StackColorCalibrationSkipReason::NoPlateSolve)),
                &ready
            ),
            Some("Out of date — the reference frame is now plate solved")
        );
        assert_eq!(
            calibration_outdated(
                None,
                Some(&skipped(StackColorCalibrationSkipReason::TooFewStars)),
                &ready
            ),
            None
        );
        assert_eq!(calibration_outdated(None, None, &ready), None);
    }

    #[test]
    fn seiza_skips_map_onto_the_reasons_the_card_gives() {
        use seiza_stacking::{ColorCalibrationSkip, ColorCalibrationSkipReason as Seiza};
        for (reason, expected) in [
            (
                Seiza::NoSolution,
                StackColorCalibrationSkipReason::NoPlateSolve,
            ),
            (
                Seiza::NoCatalog,
                StackColorCalibrationSkipReason::NoGaiaCatalog,
            ),
            (Seiza::NoStars, StackColorCalibrationSkipReason::TooFewStars),
            (
                Seiza::TooFewStars,
                StackColorCalibrationSkipReason::TooFewStars,
            ),
            (Seiza::NotRgb, StackColorCalibrationSkipReason::Failed),
            (
                Seiza::InvalidOptions,
                StackColorCalibrationSkipReason::Failed,
            ),
            (Seiza::FitFailed, StackColorCalibrationSkipReason::Failed),
        ] {
            let outcome: StackColorCalibrationOutcome =
                seiza_stacking::ColorCalibrationOutcome::Skipped(ColorCalibrationSkip {
                    reason,
                    detail: "detail".into(),
                })
                .into();
            assert_eq!(
                outcome,
                StackColorCalibrationOutcome::skipped(expected, "detail")
            );
        }
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

/// A plate solve as a composition's spec writes it down: Seiza's `Wcs`
/// field for field.
mod wcs_form {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    #[derive(Serialize, Deserialize)]
    struct SipForm {
        order: u8,
        a: Vec<f64>,
        b: Vec<f64>,
        ap: Vec<f64>,
        bp: Vec<f64>,
    }

    #[derive(Serialize, Deserialize)]
    struct WcsForm {
        crval: (f64, f64),
        crpix: (f64, f64),
        cd: [[f64; 2]; 2],
        sip: Option<SipForm>,
    }

    pub(super) fn serialize<S: Serializer>(
        wcs: &seiza::Wcs,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        WcsForm {
            crval: wcs.crval,
            crpix: wcs.crpix,
            cd: wcs.cd,
            sip: wcs.sip.as_ref().map(|sip| SipForm {
                order: sip.order,
                a: sip.a.clone(),
                b: sip.b.clone(),
                ap: sip.ap.clone(),
                bp: sip.bp.clone(),
            }),
        }
        .serialize(serializer)
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<seiza::Wcs, D::Error> {
        let form = WcsForm::deserialize(deserializer)?;
        Ok(seiza::Wcs {
            crval: form.crval,
            crpix: form.crpix,
            cd: form.cd,
            sip: form.sip.map(|sip| seiza::Sip {
                order: sip.order,
                a: sip.a,
                b: sip.b,
                ap: sip.ap,
                bp: sip.bp,
            }),
        })
    }
}
