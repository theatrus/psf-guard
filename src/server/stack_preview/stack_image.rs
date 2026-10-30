//! The image viewer's frame tools on a stack's own linear FITS: plate
//! solving, star marks and the PSF view. Each sits beside the stack's FITS
//! download, as its pixel window does, so the stack inspector offers what the
//! frame viewer offers.
//!
//! A stack's solve is kept beside that stack's FITS, as a mosaic canvas's is
//! (`canvas_astrometry`). It is not acquisition-image evidence and never
//! enters the catalog's astrometry cache.

use super::*;
use crate::astrometry::{AstrometryAnalysis, PixelSolveHint};
use crate::server::sky_coverage::StackSolution;
use axum::http::HeaderMap;

// The astrometry engine's response needs an ID. Zero is local to this
// stack's private cache, never an ID in the acquisition database.
const STACK_ID: i32 = 0;

/// A stack's own linear FITS, and what its frames say about where it points.
struct StackImage {
    path: PathBuf,
    /// The kind and job, and the group of a mono stack, for cache keys.
    key: String,
    /// The plate solution of a solved frame, carried onto the stack's grid.
    carried: Option<StackSolution>,
    /// The frame the stack was registered on.
    reference_image_id: Option<i32>,
}

fn mono(
    state: &Arc<AppState>,
    context: &DatabaseContext,
    job_id: &str,
    group_index: usize,
) -> Result<StackImage, AppError> {
    let job = stack_job(state, context, job_id)?;
    let group = job
        .groups
        .into_iter()
        .find(|group| group.index == group_index && group.state == StackGroupState::Ready)
        .ok_or(AppError::NotFound)?;
    Ok(StackImage {
        path: fits_path(&context.stack_root, job_id, group_index),
        key: format!("mono-{job_id}-{group_index}"),
        carried: crate::server::sky_coverage::mono_stack_solution(context, &group),
        reference_image_id: group.reference_image_id,
    })
}

fn colour(context: &DatabaseContext, job_id: &str) -> Result<StackImage, AppError> {
    validate_job_id(job_id)?;
    let job = std::fs::read(color::color_manifest_path(&context.stack_root, job_id))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<color::StackColorJob>(&bytes).ok())
        .filter(|job| job.state == StackJobState::Completed)
        .ok_or(AppError::NotFound)?;
    Ok(StackImage {
        path: color::color_fits_path(&context.stack_root, job_id),
        key: format!("colour-{job_id}"),
        carried: crate::server::sky_coverage::colour_stack_solution(context, &job),
        reference_image_id: crate::server::sky_coverage::colour_reference_image(context, &job),
    })
}

async fn resolve(
    state: &Arc<AppState>,
    ctx: &DbContext,
    stack: StackRef,
) -> Result<StackImage, AppError> {
    let state = Arc::clone(state);
    let context = Arc::clone(&ctx.0);
    tokio::task::spawn_blocking(move || match stack {
        StackRef::Mono {
            job_id,
            group_index,
        } => mono(&state, &context, &job_id, group_index),
        StackRef::Colour { job_id } => colour(&context, &job_id),
    })
    .await
    .map_err(|_| AppError::InternalError("The stack lookup stopped".into()))?
}

enum StackRef {
    Mono { job_id: String, group_index: usize },
    Colour { job_id: String },
}

/// Where a solve of the stack's pixels lives.
fn solve_cache_dir(path: &FsPath) -> PathBuf {
    path.with_extension("stack-solve")
}

/// The stack's header analysis, with its pixel solve when one is kept and
/// still describes this FITS. A WCS the stack kept from its reference frame
/// is left out: it may not fit the stack's grid, and a solve of the stack's
/// own pixels replaces it.
fn header_analysis(
    astrometry: &crate::astrometry::AstrometryContext,
    stack: &StackImage,
    expected_target: Option<(f64, f64)>,
) -> Result<AstrometryAnalysis, String> {
    let mut fresh = astrometry.analyze_image(STACK_ID, &stack.path, expected_target)?;
    fresh.solution = None;
    fresh.pointing = None;
    fresh.mode = None;
    Ok(astrometry.with_cached_solution(&solve_cache_dir(&stack.path), fresh))
}

/// Where to look for the stack, and at what scale: a solved frame's solution
/// carried onto the stack's grid, else the reference frame's pointing and
/// pixel scale. A stack drizzled or binned has a scale of its own, which the
/// sizes of the two images give.
fn solve_hint(stack: &StackImage, reference: Option<&FsPath>) -> Option<PixelSolveHint> {
    if let Some(carried) = &stack.carried {
        return Some(PixelSolveHint {
            center: (carried.center_ra_deg, carried.center_dec_deg),
            scale_arcsec_per_pixel: carried.pixel_scale_arcsec_per_pixel,
            source: "stack_frames",
        });
    }
    let frame = crate::astrometry_headers::FitsAstrometryHeaders::from_path(reference?).ok()?;
    let center = frame
        .embedded_wcs
        .as_ref()
        .map(|wcs| (wcs.value.crval[0], wcs.value.crval[1]))
        .or_else(|| {
            frame
                .center_ra_deg
                .as_ref()
                .zip(frame.center_dec_deg.as_ref())
                .map(|(ra, dec)| (ra.value, dec.value))
        })?;
    let frame_scale = frame.pixel_scale_arcsec_per_pixel.as_ref()?.value;
    let stack_headers =
        crate::astrometry_headers::FitsAstrometryHeaders::from_path(&stack.path).ok()?;
    let diagonal = |width: u32, height: u32| f64::from(width).hypot(f64::from(height));
    let ratio = frame
        .width
        .as_ref()
        .zip(frame.height.as_ref())
        .zip(
            stack_headers
                .width
                .as_ref()
                .zip(stack_headers.height.as_ref()),
        )
        .map(
            |((frame_width, frame_height), (stack_width, stack_height))| {
                diagonal(frame_width.value, frame_height.value)
                    / diagonal(stack_width.value, stack_height.value)
            },
        )
        .filter(|ratio| ratio.is_finite() && *ratio > 0.0)
        .unwrap_or(1.0);
    // A crop or a turn keeps the frame's pixel scale; only a change of size
    // well past a crop is a drizzle or a binning.
    let scale = if (0.75..=1.0 / 0.75).contains(&ratio) {
        frame_scale
    } else {
        frame_scale * ratio
    };
    Some(PixelSolveHint {
        center,
        scale_arcsec_per_pixel: scale,
        source: "reference_frame",
    })
}

async fn astrometry(
    state: &Arc<AppState>,
    ctx: &DbContext,
    stack: StackRef,
    solve: bool,
) -> Result<Json<ApiResponse<AstrometryAnalysis>>, AppError> {
    let _solve_guard = if solve {
        Some(ctx.astrometry_solve_mutex.lock().await)
    } else {
        None
    };
    let stack = resolve(state, ctx, stack).await?;
    // The reference frame gives the expected target and, when no frame is
    // solved, the hint. A frame whose file is gone gives neither.
    let (reference, expected_target) = match stack.reference_image_id {
        Some(id) => crate::server::handlers::resolve_astrometry_input(&ctx.0, id)
            .await
            .map(|(path, expected)| (Some(path), expected))
            .unwrap_or((None, None)),
        None => (None, None),
    };
    let astrometry = Arc::clone(&state.astrometry);
    let guard = state.begin_named_interactive_job("a stack plate solve");
    let analysis = tokio::task::spawn_blocking(move || {
        let _guard = guard;
        let cached = header_analysis(&astrometry, &stack, expected_target)?;
        if !solve || cached.solution.is_some() {
            return Ok(cached);
        }
        let hint = solve_hint(&stack, reference.as_deref());
        let analysis = astrometry.solve_stack(STACK_ID, &stack.path, expected_target, hint)?;
        if analysis
            .solve_attempt
            .as_ref()
            .is_some_and(|attempt| attempt.cacheable)
        {
            crate::astrometry::persist_pixel_analysis(&solve_cache_dir(&stack.path), &analysis)?;
        }
        Ok::<_, String>(analysis)
    })
    .await
    .map_err(|error| AppError::InternalError(format!("Stack plate solve task failed: {error}")))?
    .map_err(AppError::BadRequest)?;
    Ok(Json(ApiResponse::success(analysis)))
}

/// The star marks a frame's `annotated` route draws, on a stack.
#[derive(Debug, Deserialize)]
pub struct StarMarksQuery {
    /// `screen`, `large` (the default) or `original`.
    size: Option<String>,
    max_stars: Option<usize>,
}

async fn star_marks(
    state: &Arc<AppState>,
    ctx: &DbContext,
    stack: StackRef,
    query: StarMarksQuery,
    headers: &HeaderMap,
) -> Result<Response, AppError> {
    let size = query.size.unwrap_or_else(|| "large".to_string());
    if !matches!(size.as_str(), "screen" | "large" | "original") {
        return Err(AppError::BadRequest(format!("unknown image size '{size}'")));
    }
    let max_stars = query.max_stars.unwrap_or(1000).clamp(1, 10_000);
    let stack = resolve(state, ctx, stack).await?;
    let token =
        crate::server::handlers::source_file_cache_token(&stack.path).ok_or(AppError::NotFound)?;
    let encoding = state.preview_encoding();
    let cache_path = crate::server::handlers::artifact_cache_path(
        ctx,
        "stack_annotated",
        &format!("{}_{size}_{max_stars}_{token}", stack.key),
        encoding,
    )?;
    if !cache_path.is_file() {
        let guard = state.begin_named_interactive_job("stack star marks");
        let source = stack.path.clone();
        let destination = cache_path.clone();
        tokio::task::spawn_blocking(move || {
            let _guard = guard;
            render_atomically(&source, &token, &destination, |temporary| {
                crate::server::preview_queue::generate_annotated(
                    &source, temporary, &size, max_stars, encoding,
                )
            })
        })
        .await
        .map_err(|error| AppError::InternalError(format!("Star marks task failed: {error}")))?
        .map_err(|error| AppError::InternalError(format!("Failed to mark stars: {error}")))?;
    }
    crate::server::handlers::serve_cached_png(&cache_path, headers).await
}

/// The PSF view a frame's `psf` route draws, on a stack.
#[derive(Debug, Deserialize)]
pub struct PsfQuery {
    num_stars: Option<usize>,
    psf_type: Option<String>,
    sort_by: Option<String>,
    grid_cols: Option<usize>,
    selection: Option<String>,
}

async fn psf(
    state: &Arc<AppState>,
    ctx: &DbContext,
    stack: StackRef,
    query: PsfQuery,
    headers: &HeaderMap,
) -> Result<Response, AppError> {
    use crate::psf_fitting::PSFType;
    let num_stars = query.num_stars.unwrap_or(9).clamp(1, 100);
    let psf_type_name = query.psf_type.unwrap_or_else(|| "moffat".to_string());
    let psf_type: PSFType = psf_type_name.parse().unwrap_or(PSFType::Moffat4);
    let sort_by = query.sort_by.unwrap_or_else(|| "r2".to_string());
    let selection = query.selection.unwrap_or_else(|| "top-n".to_string());
    let grid_cols = query.grid_cols;
    let stack = resolve(state, ctx, stack).await?;
    let token =
        crate::server::handlers::source_file_cache_token(&stack.path).ok_or(AppError::NotFound)?;
    let key = format!(
        "{}_{num_stars}_{psf_type_name}_{sort_by}_{selection}_{}_{token}",
        stack.key,
        grid_cols.map_or_else(|| "auto".to_string(), |columns| columns.to_string()),
    );
    let cache = ctx.cache();
    cache.ensure_category_dir("stack_psf").map_err(|error| {
        AppError::InternalError(format!("Failed to create cache directory: {error}"))
    })?;
    let cache_path = cache.get_cached_path("stack_psf", &key, "png");
    if !cache_path.is_file() {
        let guard = state.begin_named_interactive_job("a stack PSF view");
        let source = stack.path.clone();
        let destination = cache_path.clone();
        tokio::task::spawn_blocking(move || {
            let _guard = guard;
            render_atomically(&source, &token, &destination, |temporary| {
                use image::codecs::png::{CompressionType, FilterType, PngEncoder};
                use image::{ColorType, ImageEncoder};
                let fits = crate::image_analysis::FitsImage::from_file(&source)?;
                let rgba = crate::commands::visualize_psf_multi_common::create_psf_multi_image(
                    &fits, num_stars, psf_type, &sort_by, grid_cols, &selection,
                )?;
                let writer = std::io::BufWriter::new(std::fs::File::create(temporary)?);
                PngEncoder::new_with_quality(writer, CompressionType::Best, FilterType::Adaptive)
                    .write_image(&rgba, rgba.width(), rgba.height(), ColorType::Rgba8.into())?;
                Ok(())
            })
        })
        .await
        .map_err(|error| AppError::InternalError(format!("PSF view task failed: {error}")))?
        .map_err(|error| {
            AppError::InternalError(format!("Failed to generate PSF visualization: {error}"))
        })?;
    }
    crate::server::handlers::serve_cached_png(&cache_path, headers).await
}

/// Render to a temporary file beside `destination` and publish it, unless the
/// stack was rebuilt meanwhile.
fn render_atomically(
    source: &FsPath,
    token: &str,
    destination: &FsPath,
    render: impl FnOnce(&FsPath) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let temporary = destination.with_extension(format!(
        "{}.tmp.{}",
        destination
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("png"),
        uuid::Uuid::new_v4().simple()
    ));
    let result = render(&temporary).and_then(|()| {
        anyhow::ensure!(
            crate::server::handlers::source_file_cache_token(source).as_deref() == Some(token),
            "the stack changed while it was drawn"
        );
        std::fs::rename(&temporary, destination).map_err(Into::into)
    });
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

/// GET …/stack-previews/{job}/{group}/fits/astrometry: the mono stack's
/// header analysis, with its own pixel solve once there is one.
pub async fn get_mono_astrometry(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db_id, job_id, group_index)): Path<(String, String, usize)>,
) -> Result<Json<ApiResponse<AstrometryAnalysis>>, AppError> {
    astrometry(
        &state,
        &ctx,
        StackRef::Mono {
            job_id,
            group_index,
        },
        false,
    )
    .await
}

/// POST …/stack-previews/{job}/{group}/fits/astrometry: solve the mono
/// stack's own pixels.
pub async fn solve_mono_astrometry(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db_id, job_id, group_index)): Path<(String, String, usize)>,
) -> Result<Json<ApiResponse<AstrometryAnalysis>>, AppError> {
    astrometry(
        &state,
        &ctx,
        StackRef::Mono {
            job_id,
            group_index,
        },
        true,
    )
    .await
}

/// GET …/stack-previews/{job}/{group}/fits/annotated: the mono stack with its
/// stars marked.
pub async fn mono_star_marks(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db_id, job_id, group_index)): Path<(String, String, usize)>,
    Query(query): Query<StarMarksQuery>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    star_marks(
        &state,
        &ctx,
        StackRef::Mono {
            job_id,
            group_index,
        },
        query,
        &headers,
    )
    .await
}

/// GET …/stack-previews/{job}/{group}/fits/psf: the mono stack's PSF view.
pub async fn mono_psf(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db_id, job_id, group_index)): Path<(String, String, usize)>,
    Query(query): Query<PsfQuery>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    psf(
        &state,
        &ctx,
        StackRef::Mono {
            job_id,
            group_index,
        },
        query,
        &headers,
    )
    .await
}

/// GET …/stack-previews/color/{job}/fits/astrometry: the colour stack's
/// header analysis, with its own pixel solve once there is one.
pub async fn get_colour_astrometry(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db_id, job_id)): Path<(String, String)>,
) -> Result<Json<ApiResponse<AstrometryAnalysis>>, AppError> {
    astrometry(&state, &ctx, StackRef::Colour { job_id }, false).await
}

/// POST …/stack-previews/color/{job}/fits/astrometry: solve the colour
/// stack's own pixels.
pub async fn solve_colour_astrometry(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db_id, job_id)): Path<(String, String)>,
) -> Result<Json<ApiResponse<AstrometryAnalysis>>, AppError> {
    astrometry(&state, &ctx, StackRef::Colour { job_id }, true).await
}

/// GET …/stack-previews/color/{job}/fits/annotated: the colour stack's
/// luminance with its stars marked.
pub async fn colour_star_marks(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db_id, job_id)): Path<(String, String)>,
    Query(query): Query<StarMarksQuery>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    star_marks(&state, &ctx, StackRef::Colour { job_id }, query, &headers).await
}

/// GET …/stack-previews/color/{job}/fits/psf: the colour stack's PSF view,
/// from its luminance.
pub async fn colour_psf(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db_id, job_id)): Path<(String, String)>,
    Query(query): Query<PsfQuery>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    psf(&state, &ctx, StackRef::Colour { job_id }, query, &headers).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_stack(path: &FsPath, width: usize, height: usize) {
        let image = seiza_stacking::LinearImage::new(width, height, 1, vec![100.0; width * height])
            .unwrap();
        seiza_stacking::write_processed_image_fits_f32(path, &image, &[], &[]).unwrap();
    }

    fn write_frame(path: &FsPath, width: usize, height: usize) {
        let cards = [
            ("RA", seiza_fits::HeaderValue::Float(130.1)),
            ("DEC", seiza_fits::HeaderValue::Float(19.66)),
            ("FOCALLEN", seiza_fits::HeaderValue::Float(530.0)),
            ("XPIXSZ", seiza_fits::HeaderValue::Float(3.76)),
            ("YPIXSZ", seiza_fits::HeaderValue::Float(3.76)),
        ]
        .map(|(key, value)| (key.to_string(), value));
        let image = seiza_stacking::LinearImage::new(width, height, 1, vec![100.0; width * height])
            .unwrap();
        let extra = cards
            .iter()
            .map(|(key, value)| seiza_fits::WriteHeaderCard::new(key, value.clone()))
            .collect::<Vec<_>>();
        seiza_stacking::write_processed_image_fits_f32(path, &image, &[], &extra).unwrap();
    }

    fn stack(path: PathBuf, carried: Option<StackSolution>) -> StackImage {
        StackImage {
            path,
            key: "mono-test-0".into(),
            carried,
            reference_image_id: Some(1),
        }
    }

    #[test]
    fn a_solved_frame_carried_onto_the_stack_is_the_hint() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("stack.fits");
        write_stack(&path, 400, 300);
        let carried = StackSolution {
            image_width: 400,
            image_height: 300,
            center_ra_deg: 130.0,
            center_dec_deg: 19.5,
            pixel_scale_arcsec_per_pixel: 1.5,
            wcs: crate::astrometry::WcsResponse {
                crval: [130.0, 19.5],
                crpix: [199.5, 149.5],
                cd: [[-1.5 / 3600.0, 0.0], [0.0, 1.5 / 3600.0]],
                ctype: ["RA---TAN".into(), "DEC--TAN".into()],
                cunit: ["deg".into(), "deg".into()],
                radesys: "ICRS".into(),
                equinox: 2000.0,
            },
        };
        let hint = solve_hint(&stack(path, Some(carried)), None).unwrap();
        assert_eq!(hint.center, (130.0, 19.5));
        assert_eq!(hint.scale_arcsec_per_pixel, 1.5);
        assert_eq!(hint.source, "stack_frames");
    }

    #[test]
    fn without_a_solved_frame_the_reference_frame_points_the_solve() {
        let root = tempfile::tempdir().unwrap();
        let frame = root.path().join("frame.fits");
        write_frame(&frame, 400, 300);
        let frame_scale = crate::astrometry_headers::FitsAstrometryHeaders::from_path(&frame)
            .unwrap()
            .pixel_scale_arcsec_per_pixel
            .unwrap()
            .value;

        // Cropped and turned: the frame's own scale.
        let cropped = root.path().join("cropped.fits");
        write_stack(&cropped, 280, 390);
        let hint = solve_hint(&stack(cropped, None), Some(&frame)).unwrap();
        assert_eq!(hint.center, (130.1, 19.66));
        assert_eq!(hint.scale_arcsec_per_pixel, frame_scale);
        assert_eq!(hint.source, "reference_frame");

        // Drizzled twice: half the frame's scale.
        let drizzled = root.path().join("drizzled.fits");
        write_stack(&drizzled, 800, 600);
        let hint = solve_hint(&stack(drizzled, None), Some(&frame)).unwrap();
        assert!((hint.scale_arcsec_per_pixel - frame_scale / 2.0).abs() < 1e-9);

        // No frame to point it: no hint, and a blind solve if one is set up.
        let alone = root.path().join("alone.fits");
        write_stack(&alone, 400, 300);
        assert_eq!(solve_hint(&stack(alone, None), None), None);
    }

    #[test]
    fn a_stack_solve_stays_beside_its_fits() {
        let path = FsPath::new("/stacks/job/0.fits");
        assert_eq!(
            solve_cache_dir(path),
            FsPath::new("/stacks/job/0.stack-solve")
        );
    }
}
