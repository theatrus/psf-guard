//! Pixel solves of generated mosaic FITS, kept beside that exact artifact.
//! These are not acquisition-image evidence and never enter the catalog cache.

use super::{stretch::write_json_atomic, StackSkyOrientation};
use crate::astrometry::{
    AstrometryAnalysis, AstrometryAnalysisStatus, AstrometryAttemptOutcome, AstrometryContext,
    AstrometrySolutionResponse, AstrometrySolveMode,
};
use std::path::{Path, PathBuf};

// The astrometry engine's response needs an ID. Zero is local to this
// artifact's private cache, never an ID in the acquisition database.
const CANVAS_ID: i32 = 0;

pub(super) fn is_mosaic(orientation: Option<&StackSkyOrientation>) -> bool {
    orientation.is_some_and(|orientation| orientation.source == "mosaic canvas")
}

fn cache_dir(path: &Path) -> PathBuf {
    path.with_extension("canvas-solve")
}

fn read(path: &Path) -> Option<AstrometryAnalysis> {
    let bytes = std::fs::read(crate::astrometry::persisted_analysis_path(
        &cache_dir(path),
        CANVAS_ID,
    ))
    .ok()?;
    let analysis: AstrometryAnalysis = serde_json::from_slice(&bytes).ok()?;
    (analysis.source_fingerprint == crate::astrometry::source_fingerprint(path).ok()?)
        .then_some(analysis)
}

/// Cheap cache read for request preparation and staleness checks. A header
/// WCS alone is not a pixel solve; a solve of a replaced FITS is not current.
pub(super) fn solution(path: &Path) -> Option<AstrometrySolutionResponse> {
    let analysis = read(path)?;
    let provenance = analysis.solver_provenance.as_ref()?;
    if analysis.status != AstrometryAnalysisStatus::Solved
        || !matches!(
            analysis.mode,
            Some(AstrometrySolveMode::Hinted | AstrometrySolveMode::Blind)
        )
        || !analysis.solve_attempt.as_ref().is_some_and(|attempt| {
            attempt.outcome == AstrometryAttemptOutcome::Solved
                && attempt.cacheable
                && attempt.image_quality_evidence
        })
        || provenance.seiza_version != crate::astrometry::SEIZA_VERSION
        || provenance.seiza_fits_version != crate::astrometry::SEIZA_FITS_VERSION
    {
        return None;
    }
    analysis.solution
}

pub(super) fn failure(path: &Path) -> Option<String> {
    read(path)?.error
}

/// Run only on a tracked job's compute pool. The engine validates cached
/// attempts against the current source, solver and catalogs. Operational
/// failures are recorded for diagnostics but are retried next time.
pub(super) fn ensure(astrometry: &AstrometryContext, path: &Path) -> Result<bool, String> {
    let mut fresh = astrometry.analyze_image(CANVAS_ID, path, None)?;
    // Prefer pixel evidence even if a linear canvas retained a header WCS.
    fresh.solution = None;
    fresh.pointing = None;
    fresh.mode = None;
    let cached = astrometry.with_cached_solution(&cache_dir(path), fresh);
    if cached
        .solve_attempt
        .as_ref()
        .is_some_and(|attempt| attempt.cacheable)
    {
        return Ok(false);
    }
    let analysis = astrometry.solve_image_for_quality(CANVAS_ID, path, None)?;
    let destination = crate::astrometry::persisted_analysis_path(&cache_dir(path), CANVAS_ID);
    // Unlike the acquisition evidence cache, this artifact-local record also
    // keeps operational errors so the calibration can explain a failed solve.
    write_json_atomic(&destination, &analysis)?;
    if analysis.solution.is_none() {
        tracing::warn!(path = %path.display(), error = ?analysis.error, "Mosaic canvas plate solve did not succeed");
    }
    Ok(true)
}

pub(super) fn ensure_in_job(
    state: &super::AppState,
    path: &Path,
    priority: crate::concurrency::Priority,
) -> Result<bool, String> {
    let headers = crate::astrometry_headers::FitsAstrometryHeaders::from_path(path)
        .map_err(|error| error.to_string())?;
    let pixels = headers
        .width
        .as_ref()
        .zip(headers.height.as_ref())
        .and_then(|(width, height)| (width.value as usize).checked_mul(height.value as usize));
    let policy = state.worker_policy();
    let budget = crate::concurrency::plan_workers(None, &policy, priority, pixels);
    let lease = state.lease_workers(priority, budget.workers);
    let pool = crate::concurrency::ComputePool::take("mosaic-astrometry", lease.workers)
        .map_err(|error| error.to_string())?;
    pool.install(|| ensure(&state.astrometry, path))
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::astrometry::{AstrometryConfig, AstrometrySolveAttempt, AstrometrySolverProvenance};

    pub(in crate::server::stack_preview) fn save_pixel_solution(path: &Path) {
        let context = AstrometryContext::new(AstrometryConfig {
            data_dir: Some("/nonexistent-mosaic-test-catalogs".into()),
            ..Default::default()
        });
        let mut analysis = context.analyze_image(CANVAS_ID, path, None).unwrap();
        analysis.status = AstrometryAnalysisStatus::Solved;
        analysis.mode = Some(AstrometrySolveMode::Hinted);
        analysis.solution =
            Some(crate::server::stack_preview::color_calibration::tests::solution());
        analysis.solve_attempt = Some(AstrometrySolveAttempt {
            outcome: AstrometryAttemptOutcome::Solved,
            modes_attempted: vec![AstrometrySolveMode::Hinted],
            detected_stars: Some(100),
            duration_ms: 1,
            cacheable: true,
            image_quality_evidence: true,
        });
        analysis.solver_provenance = Some(AstrometrySolverProvenance {
            seiza_version: crate::astrometry::SEIZA_VERSION.into(),
            seiza_fits_version: crate::astrometry::SEIZA_FITS_VERSION.into(),
            detection_backend: "fixture".into(),
            star_catalog: crate::astrometry::AstrometryCatalogFileSignature {
                name: "stars".into(),
                path: "/test/stars.bin".into(),
                format: "fixture".into(),
                size_bytes: 1,
                modified_unix_seconds: 1,
                modified_subsec_nanos: 0,
                sha256: None,
            },
            blind_index: None,
        });
        write_json_atomic(
            &crate::astrometry::persisted_analysis_path(&cache_dir(path), CANVAS_ID),
            &analysis,
        )
        .unwrap();
    }

    #[test]
    fn only_a_current_pixel_solution_of_this_canvas_is_used() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("mosaic.fits");
        let image = seiza_stacking::LinearImage::new(1000, 800, 1, vec![100.0; 800_000]).unwrap();
        seiza_stacking::write_processed_image_fits_f32(&path, &image, &[], &[]).unwrap();
        save_pixel_solution(&path);
        assert!(solution(&path).is_some());
        let cached_path = crate::astrometry::persisted_analysis_path(&cache_dir(&path), CANVAS_ID);
        let mut cached = read(&path).unwrap();
        cached.mode = Some(AstrometrySolveMode::EmbeddedWcs);
        write_json_atomic(&cached_path, &cached).unwrap();
        assert!(
            solution(&path).is_none(),
            "embedded WCS must not become pixel evidence"
        );
        save_pixel_solution(&path);
        let mut cached = read(&path).unwrap();
        cached.solver_provenance.as_mut().unwrap().seiza_version = "old".into();
        write_json_atomic(&cached_path, &cached).unwrap();
        assert!(
            solution(&path).is_none(),
            "a changed solver invalidates the cache"
        );
        save_pixel_solution(&path);
        use std::io::Write;
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"changed")
            .unwrap();
        assert!(
            solution(&path).is_none(),
            "a replaced canvas invalidates its solve"
        );
    }

    #[test]
    fn installing_solver_resources_changes_the_retry_identity() {
        let root = tempfile::tempdir().unwrap();
        let context = AstrometryContext::new(AstrometryConfig {
            data_dir: Some(root.path().display().to_string()),
            stars: Some("stars.bin".into()),
            blind_index: Some("blind.idx".into()),
            objects: Some("objects.bin".into()),
            ..Default::default()
        });
        let missing = context.solve_retry_fingerprint();
        assert_eq!(missing, context.solve_retry_fingerprint());
        std::fs::write(root.path().join("stars.bin"), b"catalog signature").unwrap();
        let installed = context.solve_retry_fingerprint();
        assert_ne!(missing, installed);
        assert_eq!(installed, context.solve_retry_fingerprint());
    }

    #[test]
    fn missing_solver_resources_are_reported_and_can_be_retried() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("mosaic.fits");
        let image = seiza_stacking::LinearImage::new(16, 16, 1, vec![100.0; 256]).unwrap();
        seiza_stacking::write_processed_image_fits_f32(&path, &image, &[], &[]).unwrap();
        let context = AstrometryContext::new(AstrometryConfig {
            data_dir: Some(root.path().join("missing").display().to_string()),
            ..Default::default()
        });
        assert!(ensure(&context, &path).unwrap());
        assert!(solution(&path).is_none());
        assert!(failure(&path).unwrap().contains("star catalog unavailable"));
        assert!(
            ensure(&context, &path).unwrap(),
            "operational failures must not suppress a later retry"
        );
    }

    #[test]
    #[ignore = "requires PSF_GUARD_MOSAIC_COLOR_FITS; copies the real mosaic into an isolated cache"]
    fn real_mosaic_is_solved_on_its_own_canvas() {
        let source = std::env::var("PSF_GUARD_MOSAIC_COLOR_FITS").unwrap();
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("mosaic.fits");
        std::fs::copy(source, &path).unwrap();
        let context = AstrometryContext::new(AstrometryConfig::default());
        let policy = crate::concurrency::WorkerPolicy::default();
        let budget = crate::concurrency::plan_workers(
            None,
            &policy,
            crate::concurrency::Priority::Interactive,
            None,
        );
        let pool = crate::concurrency::ComputePool::take("mosaic-real-validation", budget.workers)
            .unwrap();
        assert!(pool.install(|| ensure(&context, &path)).unwrap());
        let solved =
            solution(&path).expect("the integrated mosaic must have a pixel-derived solution");
        let headers = crate::astrometry_headers::FitsAstrometryHeaders::from_path(&path).unwrap();
        assert_eq!(solved.image_width, headers.width.unwrap().value);
        assert_eq!(solved.image_height, headers.height.unwrap().value);
        assert!(solved.matched_stars >= 20, "{solved:?}");
        assert!(
            !pool.install(|| ensure(&context, &path)).unwrap(),
            "a second call must reuse the pixel solve"
        );
        println!(
            "Mosaic canvas: {} × {}, {} matched stars, {:.3} arcsec RMS",
            solved.image_width, solved.image_height, solved.matched_stars, solved.rms_arcsec
        );
    }
}
