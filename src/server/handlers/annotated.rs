//! Star-marked images and PSF visualizations, generated off the request
//! path like previews.

use super::*;

// Annotated (star-marked) image endpoint. Same async model as the preview:
// cache hit → 200 PNG; miss → enqueue on the interactive queue and 202.
#[axum::debug_handler(state = Arc<AppState>)]
pub async fn get_annotated_image(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db_id, image_id)): Path<(String, i32)>,
    Query(options): Query<PreviewOptions>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let size = options.size.as_deref().unwrap_or("screen");
    let max_stars = options.max_stars.unwrap_or(1000) as usize;

    let (image, file_only, target_name) = resolve_image_meta(&ctx, image_id)?;
    let mapping_revision = rendered_artifact_mapping_revision(&ctx, &image)?;
    let copy = resolve_copy_async(&ctx.0, &image, options.copy.as_deref()).await?;
    let cache_key = with_copy(
        annotated_cache_key(
            &image,
            &file_only,
            size,
            max_stars,
            mapping_revision.as_deref(),
        ),
        copy.as_ref(),
    );
    let cache_path = artifact_cache_path(&ctx, "annotated", &cache_key, state.preview_encoding())?;
    if let Some(copy) = copy {
        if cache_path.exists() {
            return serve_cached_png(&cache_path, &headers).await;
        }
        state.enqueue_preview(crate::server::preview_queue::GenJob {
            fits_path: copy.path,
            cache_path,
            kind: crate::server::preview_queue::GenKind::Annotated {
                max_stars,
                size: size.to_string(),
            },
            encoding: state.preview_encoding(),
        });
        return Ok(generating_response());
    }

    let delayed_probe = ctx.delayed_file_probe(image.id);
    if delayed_probe == crate::server::database_context::DelayedFileProbe::NotPending
        && cache_path.exists()
    {
        return serve_cached_png(&cache_path, &headers).await;
    }

    if delayed_probe == crate::server::database_context::DelayedFileProbe::Wait {
        return Ok(generating_response());
    }
    let source = if delayed_probe == crate::server::database_context::DelayedFileProbe::Probe {
        find_fits_file_cached_async(&ctx.0, &image, &target_name, &file_only).await
    } else {
        find_fits_file_async(&ctx.0, &image, &target_name, &file_only).await
    };
    let fits_path = match source {
        Ok(path) => path,
        Err(AppError::NotFound)
            if delayed_probe == crate::server::database_context::DelayedFileProbe::Probe =>
        {
            return Ok(generating_response());
        }
        Err(error) => return Err(error),
    };
    if !ctx.delayed_source_is_stable(image.id, &fits_path) {
        return Ok(generating_response());
    }
    if delayed_probe == crate::server::database_context::DelayedFileProbe::Probe
        && tracked_cache_matches_source(&state, &cache_path, &fits_path)?
    {
        if ctx.complete_delayed_file_probe(image.id, &fits_path) {
            return serve_cached_png(&cache_path, &headers).await;
        }
        return Ok(generating_response());
    }
    state.enqueue_preview(crate::server::preview_queue::GenJob {
        fits_path,
        cache_path,
        kind: crate::server::preview_queue::GenKind::Annotated {
            max_stars,
            size: size.to_string(),
        },
        encoding: state.preview_encoding(),
    });
    Ok(generating_response())
}

/// One artifact whose readiness the frontend wants to know, sent in a batch so
/// a grid of generating images produces a single poll instead of one per image.
#[derive(Debug, Deserialize)]
pub struct GenStatusItem {
    pub image_id: i32,
    /// A calibrated or registered copy's uuid, to render it instead of the
    /// light's own file.
    #[serde(default)]
    pub copy: Option<String>,
    /// "preview" (default) or "annotated".
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub size: Option<String>,
    #[serde(default)]
    pub stretch: Option<bool>,
    #[serde(default)]
    pub midtone: Option<f64>,
    #[serde(default)]
    pub shadow: Option<f64>,
    #[serde(default)]
    pub max_stars: Option<u32>,
    /// Must match the flag the `<img>` requested, or the poll would report on
    /// a different artifact than the one the browser is waiting for.
    #[serde(default)]
    pub color: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct GenerationStatusRequest {
    pub requests: Vec<GenStatusItem>,
}

#[derive(Debug, Serialize)]
pub struct GenerationStatusBatch {
    /// Parallel to the request's `requests`.
    pub statuses: Vec<crate::server::preview_queue::GenerationStatus>,
}

pub(super) const MAX_DELAYED_FILE_PROBES_PER_STATUS_BATCH: usize = 8;

pub(super) struct GenerationStatusLookup<'a> {
    images: &'a HashMap<i32, crate::models::AcquiredImage>,
    target_names: &'a HashMap<i32, String>,
    mapped_sources: &'a crate::server::remote_upload::MappedLightSources,
}

pub(super) fn consume_delayed_batch_probe(
    probes: &HashMap<i32, crate::server::database_context::DelayedFileProbe>,
    image_id: i32,
    consumed_probe_ids: &mut std::collections::HashSet<i32>,
) -> crate::server::database_context::DelayedFileProbe {
    let probe = probes
        .get(&image_id)
        .copied()
        .unwrap_or(crate::server::database_context::DelayedFileProbe::NotPending);
    if probe == crate::server::database_context::DelayedFileProbe::Probe
        && !consumed_probe_ids.insert(image_id)
    {
        crate::server::database_context::DelayedFileProbe::Wait
    } else {
        probe
    }
}

/// POST /api/db/{db_id}/images/generation-status
///
/// Batch readiness poll for on-demand previews / annotated images. For each
/// item: cached → `ready`; generating → `generating`; failed → `error`;
/// unknown → enqueue (idempotent) and report `generating`. Coalesces a whole
/// grid's polling into one request instead of one-per-image.
pub async fn post_generation_status(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Json(req): Json<GenerationStatusRequest>,
) -> Result<Json<ApiResponse<GenerationStatusBatch>>, AppError> {
    // Batch the DB work: one images lookup + one targets lookup for the whole
    // request, instead of two locked point queries per item. A DB error fails
    // the whole batch with a 5xx — which the frontend simply retries on the
    // next poll tick — so a transient lock never collapses into a permanent
    // per-tile "error"; only an id genuinely absent from the results is
    // terminal.
    let ids: Vec<i32> = req.requests.iter().map(|r| r.image_id).collect();
    let (images_by_id, target_names, mapped_sources): (
        HashMap<i32, crate::models::AcquiredImage>,
        HashMap<i32, String>,
        crate::server::remote_upload::MappedLightSources,
    ) = {
        let conn = ctx.db();
        let conn = conn.lock().map_err(AppError::db)?;
        let db = Database::new(&conn);
        let images = db.get_images_by_ids(&ids).map_err(AppError::db)?;
        let target_ids: Vec<i32> = images.iter().map(|i| i.target_id).collect();
        let targets = db.get_targets_by_ids(&target_ids).map_err(AppError::db)?;
        let mapped_sources = crate::server::remote_upload::mapped_light_sources(
            &conn,
            &ctx.image_dir_paths,
            images.iter(),
        )?;
        (
            images.into_iter().map(|i| (i.id, i)).collect(),
            targets.into_iter().map(|t| (t.id, t.name)).collect(),
            mapped_sources,
        )
    };

    let delayed_probes =
        ctx.delayed_file_probes_for_batch(&ids, MAX_DELAYED_FILE_PROBES_PER_STATUS_BATCH);
    let ctx = Arc::clone(&ctx.0);
    let requests = req.requests;
    let statuses = tokio::task::spawn_blocking(move || {
        let mut canonical_roots = None;
        let mut consumed_probe_ids = std::collections::HashSet::new();
        let lookup = GenerationStatusLookup {
            images: &images_by_id,
            target_names: &target_names,
            mapped_sources: &mapped_sources,
        };
        requests
            .iter()
            .map(|item| {
                let delayed_probe = consume_delayed_batch_probe(
                    &delayed_probes,
                    item.image_id,
                    &mut consumed_probe_ids,
                );
                status_for_item(
                    &state,
                    &ctx,
                    item,
                    &lookup,
                    delayed_probe,
                    &mut canonical_roots,
                )
            })
            .collect()
    })
    .await
    .map_err(|error| {
        tracing::error!("generation-status filesystem task failed: {error}");
        AppError::InternalError("Preview status could not be checked".to_string())
    })?;
    Ok(Json(ApiResponse::success(GenerationStatusBatch {
        statuses,
    })))
}

/// Resolve one status item to a `GenerationStatus` from pre-fetched image /
/// target maps, enqueuing generation when the artifact is neither cached nor
/// already known to the queue. An id absent from `images_by_id` is a genuinely
/// unknown image (permanent `error`); transient DB failures are handled one
/// level up (the batch 5xx → frontend retry).
pub(super) fn status_for_item(
    state: &Arc<AppState>,
    ctx: &DatabaseContext,
    item: &GenStatusItem,
    lookup: &GenerationStatusLookup<'_>,
    delayed_probe: crate::server::database_context::DelayedFileProbe,
    canonical_roots: &mut Option<Vec<PathBuf>>,
) -> crate::server::preview_queue::GenerationStatus {
    use crate::server::preview_queue::{GenJob, GenKind, GenerationState, GenerationStatus};

    let err = |msg: &str| GenerationStatus {
        state: GenerationState::Error,
        error: Some(msg.to_string()),
    };
    let generating = || GenerationStatus {
        state: GenerationState::Generating,
        error: None,
    };
    // One read of each, used for both the path this polls and the job it may
    // enqueue, so a setting change mid-request cannot split the two.
    let encoding = state.preview_encoding();
    let color_default = state.preview_color_default();

    let Some(image) = lookup.images.get(&item.image_id) else {
        return err("image not found");
    };
    let Some(target_name) = lookup.target_names.get(&image.target_id) else {
        return err("target not found");
    };
    let Some(file_only) = filename_from_metadata(&image.metadata) else {
        return err("no filename in metadata");
    };
    if lookup.mapped_sources.is_invalid(image.id) {
        return err("remote image provenance is invalid");
    }
    let mapping_revision = lookup
        .mapped_sources
        .get(&image.id)
        .and_then(|source| source.revision.strip_prefix("mapping:").map(str::to_owned));

    let copy = match item.copy.as_deref() {
        Some(uuid) => match resolve_copy(ctx, image, uuid) {
            Ok(copy) => Some(copy),
            Err(_) => return err("copy not found"),
        },
        None => None,
    };
    let delayed_probe = if copy.is_some() {
        crate::server::database_context::DelayedFileProbe::NotPending
    } else {
        delayed_probe
    };
    let size = item.size.clone().unwrap_or_else(|| "screen".to_string());
    let (cache_path, kind) = match item.kind.as_deref() {
        Some("annotated") => {
            let max_stars = item.max_stars.unwrap_or(1000) as usize;
            let key = with_copy(
                annotated_cache_key(
                    image,
                    &file_only,
                    &size,
                    max_stars,
                    mapping_revision.as_deref(),
                ),
                copy.as_ref(),
            );
            match artifact_cache_path(ctx, "annotated", &key, encoding) {
                Ok(p) => (
                    p,
                    GenKind::Annotated {
                        max_stars,
                        size: size.clone(),
                    },
                ),
                Err(_) => return err("cache error"),
            }
        }
        _ => {
            let stretch = item.stretch.unwrap_or(true);
            let midtone = item.midtone.unwrap_or(0.2);
            let shadow = item.shadow.unwrap_or(-2.8);
            let color = item.color.unwrap_or(color_default);
            let key = with_copy(
                preview_cache_key(
                    image,
                    &file_only,
                    &size,
                    stretch,
                    midtone,
                    shadow,
                    color,
                    mapping_revision.as_deref(),
                ),
                copy.as_ref(),
            );
            match artifact_cache_path(ctx, "previews", &key, encoding) {
                Ok(p) => (
                    p,
                    GenKind::Preview {
                        midtone,
                        shadow,
                        max_dimensions: crate::server::preview_queue::max_dimensions_for_size(
                            &size,
                        ),
                        color,
                    },
                ),
                Err(_) => return err("cache error"),
            }
        }
    };

    if let Some(status) = state.preview_queue.status(&cache_path) {
        match status.state {
            GenerationState::Ready
                if delayed_probe
                    == crate::server::database_context::DelayedFileProbe::NotPending =>
            {
                return status;
            }
            GenerationState::Generating => return status,
            GenerationState::Ready | GenerationState::Error => {}
        }
    }

    if delayed_probe == crate::server::database_context::DelayedFileProbe::Wait {
        return generating();
    }

    // Neither cached, in-flight, nor errored: ensure it's enqueued to generate.
    let source = if let Some(copy) = &copy {
        Ok(copy.path.clone())
    } else if delayed_probe == crate::server::database_context::DelayedFileProbe::Probe {
        let canonical_roots = canonical_roots.get_or_insert_with(|| canonical_image_roots(ctx));
        find_fits_file_cached_with_roots(ctx, image, target_name, &file_only, canonical_roots)
    } else {
        find_fits_file(ctx, image, target_name, &file_only)
    };
    match source {
        Ok(fits_path) => {
            if !ctx.delayed_source_is_stable(image.id, &fits_path) {
                return generating();
            }
            if delayed_probe == crate::server::database_context::DelayedFileProbe::Probe {
                match tracked_cache_matches_source(state, &cache_path, &fits_path) {
                    Ok(true) => {
                        if ctx.complete_delayed_file_probe(image.id, &fits_path) {
                            return state
                                .preview_queue
                                .status(&cache_path)
                                .unwrap_or_else(generating);
                        }
                        return generating();
                    }
                    Ok(false) => {}
                    Err(_) => return err("cached preview could not be refreshed"),
                }
            }
            if let Some(status) = state
                .preview_queue
                .status_for_source(&cache_path, &fits_path)
            {
                if status.state == GenerationState::Error
                    && delayed_probe == crate::server::database_context::DelayedFileProbe::Probe
                {
                    return generating();
                }
                return status;
            }
            state.enqueue_preview(GenJob {
                fits_path,
                cache_path,
                kind,
                encoding,
            });
            generating()
        }
        Err(AppError::Conflict(message)) => err(&message),
        Err(AppError::NotFound)
            if delayed_probe == crate::server::database_context::DelayedFileProbe::Probe =>
        {
            generating()
        }
        Err(AppError::NotFound) => err("source file not found"),
        Err(_) => err("source file is unavailable"),
    }
}

// PSF multi image parameters
#[derive(Deserialize)]
pub struct PsfMultiOptions {
    /// A calibrated or registered copy's uuid, to measure it instead.
    pub copy: Option<String>,
    pub num_stars: Option<usize>,
    pub psf_type: Option<String>,
    pub sort_by: Option<String>,
    pub grid_cols: Option<usize>,
    pub selection: Option<String>,
}

#[axum::debug_handler(state = Arc<AppState>)]
pub async fn get_psf_visualization(
    ctx: DbContext,
    Path((_db_id, image_id)): Path<(String, i32)>,
    Query(options): Query<PsfMultiOptions>,
) -> Result<impl IntoResponse, AppError> {
    use crate::commands::visualize_psf_multi_common::create_psf_multi_image;
    use crate::image_analysis::FitsImage;
    use crate::psf_fitting::PSFType;
    use image::codecs::png::{CompressionType, FilterType, PngEncoder};
    use image::{ColorType, ImageEncoder};

    // Get image metadata from database
    let (image, file_only, target_name) = {
        let conn = ctx.db();
        let conn = conn.lock().map_err(AppError::db)?;
        let db = Database::new(&conn);

        let images = db.get_images_by_ids(&[image_id]).map_err(AppError::db)?;

        let image = images.into_iter().next().ok_or(AppError::NotFound)?;

        // Get target name
        let targets = db
            .get_targets_by_ids(&[image.target_id])
            .map_err(AppError::db)?;

        let target = targets.into_iter().next().ok_or(AppError::NotFound)?;
        let target_name = target.name.clone();

        let metadata: serde_json::Value = serde_json::from_str(&image.metadata)
            .map_err(|_| AppError::BadRequest("Invalid metadata".to_string()))?;

        let filename = metadata["FileName"]
            .as_str()
            .ok_or_else(|| AppError::BadRequest("No filename in metadata".to_string()))?;

        let file_only = filename
            .split(&['\\', '/'][..])
            .next_back()
            .ok_or_else(|| AppError::BadRequest("Invalid filename format".to_string()))?
            .to_string();

        (image, file_only, target_name)
    };

    // Parse parameters
    let num_stars = options.num_stars.unwrap_or(9);
    let psf_type_str = options.psf_type.as_deref().unwrap_or("moffat").to_string();
    let sort_by = options.sort_by.as_deref().unwrap_or("r2").to_string();
    let selection = options.selection.as_deref().unwrap_or("top-n").to_string();
    let grid_cols = options.grid_cols;

    let psf_type: PSFType = psf_type_str.parse().unwrap_or(PSFType::Moffat4);

    let (fits_path, source_token) =
        match resolve_copy_async(&ctx.0, &image, options.copy.as_deref()).await? {
            // A copy's results sit beside the light's under its own token.
            Some(copy) => (
                copy.path,
                format!("{}_copy_{}", copy.token, copy.record.derivative_uuid),
            ),
            None => {
                let fits_path =
                    find_fits_file_async(&ctx.0, &image, &target_name, &file_only).await?;
                let source_token = source_file_cache_token(&fits_path)
                    .ok_or_else(|| AppError::Conflict("Source file is not ready".to_string()))?;
                (fits_path, source_token)
            }
        };

    // Create comprehensive cache key for PSF multi image
    let cache_key = psf_multi_cache_key(
        &image,
        &file_only,
        num_stars,
        &psf_type_str,
        &sort_by,
        &selection,
        grid_cols,
        &source_token,
    );
    let cache_manager = ctx.cache();
    cache_manager
        .ensure_category_dir("psf_multi")
        .map_err(|e| AppError::InternalError(format!("Failed to create cache directory: {}", e)))?;
    let cache_path = cache_manager.get_cached_path("psf_multi", &cache_key, "png");

    // Check if cached version exists
    if cache_manager.is_cached(&cache_path) {
        // Serve from cache
        let mut file = File::open(&cache_path)
            .await
            .map_err(|_| AppError::InternalError("Failed to read cache".to_string()))?;

        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer)
            .await
            .map_err(|_| AppError::InternalError("Failed to read file".to_string()))?;

        return Ok((
            StatusCode::OK,
            [
                (CONTENT_TYPE, "image/png"),
                (CACHE_CONTROL, "max-age=86400"), // TODO: Use configurable cache expiry
            ],
            buffer,
        ));
    }

    // Move expensive operations to spawn_blocking
    let fits_path_clone = fits_path.clone();
    let cache_path_clone = cache_path.clone();
    let source_token_clone = source_token.clone();
    tokio::task::spawn_blocking(move || {
        let temporary =
            cache_path_clone.with_extension(format!("png.tmp.{}", uuid::Uuid::new_v4().simple()));
        // Load FITS file
        let fits = FitsImage::from_file(&fits_path_clone)
            .map_err(|e| anyhow::anyhow!("Failed to load FITS: {}", e))?;

        // Create PSF multi visualization using the common function
        let rgba_image =
            create_psf_multi_image(&fits, num_stars, psf_type, &sort_by, grid_cols, &selection)
                .map_err(|e| anyhow::anyhow!("Failed to create PSF visualization: {}", e))?;

        // Save to cache
        let cache_file = std::fs::File::create(&temporary)
            .map_err(|e| anyhow::anyhow!("Failed to create cache file: {}", e))?;
        let writer = std::io::BufWriter::new(cache_file);
        let encoder =
            PngEncoder::new_with_quality(writer, CompressionType::Best, FilterType::Adaptive);

        encoder
            .write_image(
                &rgba_image,
                rgba_image.width(),
                rgba_image.height(),
                ColorType::Rgba8.into(),
            )
            .map_err(|e| anyhow::anyhow!("Failed to encode PNG: {}", e))?;

        if source_file_cache_token(&fits_path_clone).as_deref() != Some(source_token_clone.as_str())
        {
            let _ = std::fs::remove_file(&temporary);
            anyhow::bail!("source file changed while rendering");
        }
        if let Err(error) = std::fs::rename(&temporary, &cache_path_clone) {
            let _ = std::fs::remove_file(&temporary);
            if !cache_path_clone.is_file() {
                return Err(error.into());
            }
        }

        Ok::<(), anyhow::Error>(())
    })
    .await
    .map_err(|e| AppError::InternalError(format!("PSF visualization task panicked: {}", e)))?
    .map_err(|e| AppError::InternalError(format!("Failed to generate PSF visualization: {}", e)))?;

    // Read the cached file
    let png_buffer = tokio::fs::read(&cache_path)
        .await
        .map_err(|_| AppError::InternalError("Failed to read generated PNG".to_string()))?;

    Ok((
        StatusCode::OK,
        [
            (CONTENT_TYPE, "image/png"),
            (CACHE_CONTROL, "max-age=86400"), // Cache for 1 day
        ],
        png_buffer,
    ))
}

#[cfg(test)]
mod delayed_ready_tests;
