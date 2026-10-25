//! Listing images and one image's details, and grading them.

use super::*;

pub async fn get_images(
    ctx: DbContext,
    Query(params): Query<ImageQuery>,
) -> Result<Json<ApiResponse<Vec<ImageResponse>>>, AppError> {
    use crate::server::frame_shapes::{Candidate, SetupKey};

    // Read the catalog under the connection's lock, then let it go before
    // any file is touched for the frames' sizes.
    let (mut response, setups, candidates) = {
        let conn = ctx.db();
        let conn = conn.lock().map_err(AppError::db)?;
        let db = Database::new(&conn);

        let ambiguous_names = db.ambiguous_project_names().map_err(AppError::db)?;

        // Convert status string to GradingStatus enum
        let status_filter = params.status.as_ref().and_then(|s| match s.as_str() {
            "pending" => Some(GradingStatus::Pending),
            "accepted" => Some(GradingStatus::Accepted),
            "rejected" => Some(GradingStatus::Rejected),
            _ => None,
        });

        let offset = params.offset.unwrap_or(0).max(0) as usize;
        let limit = params.limit.unwrap_or(100).max(0) as usize;
        let filter_name = params
            .filter_name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty());
        let images = match filter_name {
            // The scoped query cannot filter by filter name, so page after it.
            Some(wanted) => db
                .query_images_scoped(status_filter, params.project_id, params.target_id, None, 0)
                .map_err(AppError::db)?
                .into_iter()
                .filter(|(image, _, _)| image.filter_name.eq_ignore_ascii_case(wanted))
                .skip(offset)
                .take(limit)
                .collect(),
            None => db
                .query_images_scoped(
                    status_filter,
                    params.project_id,
                    params.target_id,
                    Some(limit),
                    offset,
                )
                .map_err(AppError::db)?,
        };

        let mut exposure_groups = std::collections::HashMap::new();
        let projects: std::collections::HashSet<_> = images
            .iter()
            .map(|(image, _, _)| image.project_id)
            .collect();
        for project_id in projects {
            exposure_groups.insert(
                project_id,
                crate::server::exposure_groups::cached_groups(
                    &ctx,
                    &conn,
                    project_id,
                    params.mosaic,
                )?,
            );
        }

        let guids: Vec<&str> = images
            .iter()
            .filter_map(|(image, _, _)| image.guid.as_deref())
            .collect();
        let mut copies = crate::frame_derivatives::derivatives_for_lights(&conn, &guids)
            .map_err(|error| AppError::InternalError(format!("reading copies: {error:#}")))?;
        // Each light's observing night, split at its rig's local noon, so every
        // view groups and marks nights from this one answer.
        let light_nights = crate::server::sky_coverage::cached_light_nights(
            &ctx,
            &conn,
            nights_view(params.nights.as_deref()),
        )?;

        // Each frame's camera setup, and a few frames of each to read the size
        // from. A delayed remote upload may still be arriving: not one of them.
        let mut setups = Vec::with_capacity(images.len());
        let mut candidates: std::collections::HashMap<SetupKey, Vec<Candidate>> =
            std::collections::HashMap::new();

        let response: Vec<ImageResponse> = images
            .into_iter()
            .map(|(img, proj_name, target_name)| {
                let copies = img
                    .guid
                    .as_deref()
                    .and_then(|guid| copies.remove(guid))
                    .unwrap_or_default()
                    .iter()
                    .map(ImageCopy::from)
                    .collect();
                let metadata: serde_json::Value = serde_json::from_str(&img.metadata)
                    .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

                let setup = SetupKey::of(&img, &metadata);
                let file_name = metadata["FileName"]
                    .as_str()
                    .and_then(|name| name.split(&['\\', '/'][..]).next_back())
                    .filter(|name| !name.is_empty());
                if let Some(file_name) = file_name
                    && !ctx.requires_delayed_file_identity(img.id)
                {
                    let frames = candidates.entry(setup.clone()).or_default();
                    if frames.len() < 3 {
                        frames.push(Candidate {
                            image: img.clone(),
                            target_name: target_name.clone(),
                            file_name: file_name.to_string(),
                        });
                    }
                }
                setups.push(setup);

                let project_display_name = match img.profile_id.as_ref() {
                    Some(profile_id) => {
                        display_project_name(&proj_name, profile_id, &ambiguous_names)
                    }
                    None => proj_name.clone(),
                };

                ImageResponse {
                    id: img.id,
                    exposure_group: exposure_groups
                        .get(&img.project_id)
                        .and_then(|groups| groups.by_image.get(&img.id))
                        .cloned(),
                    project_id: img.project_id,
                    project_name: proj_name,
                    project_display_name,
                    target_id: img.target_id,
                    target_name,
                    night: img
                        .acquired_date
                        .map(|at| light_nights.night_of(img.profile_id.as_deref(), at)),
                    acquired_date: img.acquired_date,
                    filter_name: img.filter_name,
                    grading_status: img.grading_status,
                    reject_reason: img.reject_reason,
                    metadata,
                    filesystem_path: None, // Not calculated for bulk operations for performance
                    copies,
                    frame_size: None,
                }
            })
            .collect();
        (response, setups, candidates)
    };

    // One header per camera setup, kept for the life of the server.
    let resolver_ctx = std::sync::Arc::clone(&ctx.0);
    let sizes = crate::server::frame_shapes::sizes_for(
        &ctx.frame_shapes,
        candidates,
        move |frame: &Candidate| {
            find_fits_file(
                &resolver_ctx,
                &frame.image,
                &frame.target_name,
                &frame.file_name,
            )
            .ok()
        },
    )
    .await;
    for (image, setup) in response.iter_mut().zip(&setups) {
        image.frame_size = sizes.get(setup).copied();
    }

    Ok(Json(ApiResponse::success(response)))
}

#[axum::debug_handler(state = Arc<AppState>)]
pub async fn get_image(
    ctx: DbContext,
    Path((_db_id, image_id)): Path<(String, i32)>,
) -> Result<Json<ApiResponse<ImageResponse>>, AppError> {
    use crate::image_analysis::FitsImage;

    // Get image data from database first (before any async operations)
    let (image, proj_name, target_name, mut metadata, ambiguous_names, exposure_group) = {
        let conn = ctx.db();
        let conn = conn.lock().map_err(AppError::db)?;
        let db = Database::new(&conn);

        let ambiguous_names = db.ambiguous_project_names().map_err(AppError::db)?;

        let images = db.get_images_by_ids(&[image_id]).map_err(AppError::db)?;

        let image = images.into_iter().next().ok_or(AppError::NotFound)?;

        // Get project and target names
        let scoped_images = db
            .query_images_scoped(None, Some(image.project_id), Some(image.target_id), None, 0)
            .map_err(AppError::db)?;

        let (_, proj_name, target_name) = scoped_images
            .into_iter()
            .find(|(img, _, _)| img.id == image_id)
            .ok_or(AppError::NotFound)?;

        let metadata: serde_json::Value = serde_json::from_str(&image.metadata)
            .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

        let exposure_group =
            crate::server::exposure_groups::cached_project_groups(&ctx, &conn, image.project_id)?
                .by_image
                .get(&image.id)
                .cloned();
        (
            image,
            proj_name,
            target_name,
            metadata,
            ambiguous_names,
            exposure_group,
        )
    }; // Database connection is dropped here

    // Try to resolve the filesystem path for the FITS file
    let delayed_probe = ctx.delayed_file_probe(image.id);
    let resolved_fits_path = if let Some(filename) = metadata["FileName"].as_str() {
        let file_only = filename.split(&['\\', '/'][..]).next_back();
        if delayed_probe == crate::server::database_context::DelayedFileProbe::Wait {
            None
        } else if let Some(file_only) = file_only {
            let resolved =
                if delayed_probe == crate::server::database_context::DelayedFileProbe::Probe {
                    find_fits_file_cached_async(&ctx.0, &image, &target_name, file_only).await
                } else {
                    find_fits_file_async(&ctx.0, &image, &target_name, file_only).await
                };
            // Detail metadata remains usable for a missing or ambiguous
            // source; preview generation-status carries the terminal message.
            resolved.ok().filter(|path| {
                delayed_probe != crate::server::database_context::DelayedFileProbe::Probe
                    || ctx.delayed_source_is_stable(image.id, path)
            })
        } else {
            None
        }
    } else {
        None
    };

    let filesystem_path_string = resolved_fits_path
        .as_ref()
        .map(|path| path.to_string_lossy().to_string());

    // Statistics are keyed by both scheduler identity and the resolved source
    // fingerprint. A same-ID remote path update or a copy that resumes after a
    // stable prefix therefore cannot reuse statistics from the old pixels.
    let stats_cache = resolved_fits_path.as_ref().and_then(|fits_path| {
        source_file_cache_token(fits_path).map(|source_token| {
            let filename = format!(
                "stats_v2_{}_{}_{}_{}_{}_{}.json",
                image_id,
                image.project_id,
                image.target_id,
                image.acquired_date.unwrap_or(0),
                image_file_identity_cache_token(&image),
                source_token,
            );
            (
                ctx.cache_dir_path.join("stats").join(filename),
                source_token,
            )
        })
    });
    if let Some((stats_cache_path, _)) = stats_cache.as_ref()
        && let Some(parent) = stats_cache_path.parent()
    {
        let _ = tokio::fs::create_dir_all(parent).await;
    }

    let cached_fits_stats = if let Some((stats_cache_path, _)) = stats_cache.as_ref() {
        let cached = tokio::fs::read_to_string(stats_cache_path)
            .await
            .ok()
            .and_then(|cached_data| serde_json::from_str::<serde_json::Value>(&cached_data).ok());
        if cached.is_none() && stats_cache_path.is_file() {
            let _ = tokio::fs::remove_file(stats_cache_path).await;
        }
        cached
    } else {
        None
    };
    let fits_stats = if cached_fits_stats.is_some() {
        cached_fits_stats
    } else if let (Some(fits_path), Some((stats_cache_path, source_token))) =
        (resolved_fits_path.as_ref(), stats_cache.as_ref())
    {
        // Calculate statistics from FITS file
        if let Ok(fits) = FitsImage::from_file(fits_path) {
            let stats = fits.calculate_basic_statistics();

            // Extract temperature and camera model from FITS headers
            let temperature = FitsImage::extract_temperature(fits_path);
            let camera_model = FitsImage::extract_camera_model(fits_path);

            let mut stats_json = serde_json::json!({
                "Min": stats.min,
                "Max": stats.max,
                "Mean": stats.mean,
                "Median": stats.median,
                "StdDev": stats.std_dev,
                "Mad": stats.mad
            });

            // Add temperature if available
            if let Some(temp) = temperature {
                stats_json["Temperature"] = serde_json::json!(temp);
            }

            // Add camera model if available
            if let Some(camera) = camera_model {
                stats_json["Camera"] = serde_json::json!(camera);
            }

            // Do not publish statistics if the source changed while it was
            // decoded. The next request will use the new fingerprint key.
            if source_file_cache_token(fits_path).as_ref() != Some(source_token) {
                None
            } else {
                if let Ok(cached_data) = serde_json::to_string(&stats_json) {
                    let _ = write_cache_atomic(stats_cache_path, cached_data.as_bytes()).await;
                }
                Some(stats_json)
            }
        } else {
            None
        }
    } else {
        None
    };

    // Merge statistics into metadata if available
    if let (Some(stats), Some(metadata_obj)) = (fits_stats, metadata.as_object_mut())
        && let Some(stats_obj) = stats.as_object()
    {
        for (key, value) in stats_obj {
            metadata_obj.insert(key.clone(), value.clone());
        }
    }

    let project_display_name = match image.profile_id.as_ref() {
        Some(profile_id) => display_project_name(&proj_name, profile_id, &ambiguous_names),
        None => proj_name.clone(),
    };
    let copies = image_copies_on_disk(&ctx.0, &image).await?;

    let response = ImageResponse {
        exposure_group,
        id: image.id,
        project_id: image.project_id,
        project_name: proj_name,
        project_display_name,
        target_id: image.target_id,
        target_name,
        // The grid's list carries the night; the detail walks its groups.
        night: None,
        acquired_date: image.acquired_date,
        filter_name: image.filter_name,
        grading_status: image.grading_status,
        reject_reason: image.reject_reason,
        metadata,
        filesystem_path: filesystem_path_string,
        copies,
        // The detail draws the frame itself; only the grid needs the hint.
        frame_size: None,
    };

    Ok(Json(ApiResponse::success(response)))
}

/// A light's copies, each checked against the image folders.
pub(super) async fn image_copies_on_disk(
    ctx: &Arc<DatabaseContext>,
    image: &crate::models::AcquiredImage,
) -> Result<Vec<ImageCopy>, AppError> {
    let Some(guid) = image.guid.clone() else {
        return Ok(Vec::new());
    };
    let ctx = Arc::clone(ctx);
    tokio::task::spawn_blocking(move || {
        let records = {
            let connection = ctx.db();
            let connection = connection.lock().map_err(AppError::db)?;
            crate::frame_derivatives::derivatives_for_light(&connection, &guid)
                .map_err(|error| AppError::InternalError(format!("reading copies: {error:#}")))?
        };
        if records.is_empty() {
            return Ok(Vec::new());
        }
        let tree = ctx.get_directory_tree().map_err(|error| {
            AppError::InternalError(format!("listing image folders: {error:#}"))
        })?;
        Ok(records
            .iter()
            .map(|record| ImageCopy {
                available: Some(
                    tree.find_file(&record.file_name)
                        .and_then(|candidates| {
                            crate::frame_derivatives::resolve_record_path(record, candidates)
                        })
                        .is_some(),
                ),
                ..ImageCopy::from(record)
            })
            .collect())
    })
    .await
    .map_err(|error| AppError::InternalError(format!("copy lookup failed: {error}")))?
}

pub(super) fn source_file_cache_token(path: &FsPath) -> Option<String> {
    let metadata = std::fs::metadata(path).ok()?;
    let mut hasher = Sha256::new();
    hasher.update(
        std::fs::canonicalize(path)
            .unwrap_or_else(|_| path.to_path_buf())
            .to_string_lossy()
            .as_bytes(),
    );
    hasher.update([0]);
    hasher.update(metadata.len().to_le_bytes());
    if let Ok(modified) = metadata.modified()
        && let Ok(since_epoch) = modified.duration_since(std::time::UNIX_EPOCH)
    {
        hasher.update(since_epoch.as_nanos().to_le_bytes());
    }
    Some(finish_cache_token(hasher))
}

pub async fn update_image_grade(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db_id, image_id)): Path<(String, i32)>,
    Json(request): Json<UpdateGradeRequest>,
) -> Result<Json<ApiResponse<()>>, AppError> {
    let conn = ctx.db();
    let conn = conn.lock().map_err(AppError::db)?;
    let db = Database::new(&conn);

    let status = match request.status.as_str() {
        "pending" => GradingStatus::Pending,
        "accepted" => GradingStatus::Accepted,
        "rejected" => GradingStatus::Rejected,
        _ => return Err(AppError::BadRequest("Invalid status".to_string())),
    };

    db.update_grading_status(image_id, status, request.reason.as_deref())
        .map_err(AppError::db)?;
    note_grade_changes(&state, &ctx, &db, &[image_id]);

    Ok(Json(ApiResponse::success(())))
}

/// A grade change queues a refresh of the project's remembered stack
/// previews, after the grading delay so an interactive pass settles first,
/// and a calibration health pass.
pub(super) fn note_grade_changes(
    state: &Arc<AppState>,
    ctx: &DatabaseContext,
    db: &Database,
    image_ids: &[i32],
) {
    // A rejected light leaves the lights calibration health judges. The
    // pass works out only the projects whose key changed, and accepting a
    // light changes none.
    crate::server::calibration_health::request(state, &ctx.database_path);
    let projects: std::collections::HashSet<i32> = match db.get_images_by_ids(image_ids) {
        Ok(images) => images.into_iter().map(|image| image.project_id).collect(),
        Err(error) => {
            tracing::warn!("could not find the projects of regraded images: {error}");
            return;
        }
    };
    state.auto_stacks.touch_projects(
        &ctx.id,
        projects,
        crate::server::stack_preview::automatic::RefreshReason::Grade,
    );
}

pub(super) fn parse_grading_status(status: &str) -> Result<GradingStatus, AppError> {
    match status {
        "pending" => Ok(GradingStatus::Pending),
        "accepted" => Ok(GradingStatus::Accepted),
        "rejected" => Ok(GradingStatus::Rejected),
        _ => Err(AppError::BadRequest(format!("Invalid status: {status}"))),
    }
}

pub(super) fn grading_status_label(status: i32) -> &'static str {
    match status {
        1 => "accepted",
        2 => "rejected",
        _ => "pending",
    }
}

/// Grade many images in one request and one database transaction. A grid
/// selection of thousands of frames was previously one HTTP round trip and
/// one SQLite write transaction per image. Returns the grades it replaced
/// so the client records undo state without fetching every image first.
pub async fn batch_update_image_grades(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Json(request): Json<BatchGradeRequest>,
) -> Result<Json<ApiResponse<BatchGradeResponse>>, AppError> {
    const MAX_BATCH: usize = 20_000;
    if request.updates.is_empty() {
        return Err(AppError::BadRequest("No updates in batch".to_string()));
    }
    if request.updates.len() > MAX_BATCH {
        return Err(AppError::BadRequest(format!(
            "Batch exceeds {MAX_BATCH} images; split the request"
        )));
    }
    let mut updates = Vec::with_capacity(request.updates.len());
    for entry in &request.updates {
        updates.push((
            entry.image_id,
            parse_grading_status(&entry.status)?,
            entry.reason.clone(),
        ));
    }

    Ok(Json(ApiResponse::success(apply_grades(
        &state, &ctx, &updates,
    )?)))
}

/// Write grades and return what they replaced, for undo; then refresh what
/// depends on grades.
pub(super) fn apply_grades(
    state: &Arc<AppState>,
    ctx: &DbContext,
    updates: &[(i32, GradingStatus, Option<String>)],
) -> Result<BatchGradeResponse, AppError> {
    if updates.is_empty() {
        return Ok(BatchGradeResponse {
            updated: 0,
            previous: Vec::new(),
        });
    }
    let conn = ctx.db();
    let conn = conn.lock().map_err(AppError::db)?;
    let db = Database::new(&conn);

    // Record what the batch replaces before writing. Chunk the reads to
    // stay far below SQLite's bound-variable limit.
    let ids: Vec<i32> = updates.iter().map(|(id, _, _)| *id).collect();
    let mut previous = Vec::with_capacity(ids.len());
    for chunk in ids.chunks(500) {
        for image in db.get_images_by_ids(chunk).map_err(AppError::db)? {
            previous.push(BatchGradeEntry {
                image_id: image.id,
                status: grading_status_label(image.grading_status).to_string(),
                reason: image.reject_reason.clone(),
            });
        }
    }

    db.batch_update_grading_status(updates)
        .map_err(AppError::db)?;
    let changed: Vec<i32> = previous.iter().map(|entry| entry.image_id).collect();
    note_grade_changes(state, ctx, &db, &changed);

    Ok(BatchGradeResponse {
        updated: updates.len(),
        previous,
    })
}

/// Shared DB lookup for the image handlers: the acquired-image row, the FITS
/// basename (from `metadata.FileName`), and the target name.
pub(crate) fn resolve_image_meta(
    ctx: &DatabaseContext,
    image_id: i32,
) -> Result<(crate::models::AcquiredImage, String, String), AppError> {
    let conn = ctx.db();
    let conn = conn.lock().map_err(AppError::db)?;
    let db = Database::new(&conn);

    let image = db
        .get_images_by_ids(&[image_id])
        .map_err(AppError::db)?
        .into_iter()
        .next()
        .ok_or(AppError::NotFound)?;

    let target = db
        .get_targets_by_ids(&[image.target_id])
        .map_err(AppError::db)?
        .into_iter()
        .next()
        .ok_or(AppError::NotFound)?;
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

    Ok((image, file_only, target_name))
}

/// Cache key for a stretched preview PNG. Must stay identical between the
/// preview handler, the status endpoint, and the pre-generation path so all
/// three address the same file.
/// Cache key for a preview PNG.
///
/// Shared with background pre-generation rather than reimplemented there:
/// two constructions of one key drift, and when they do the mismatch is
/// silent — the wrong artifact is served, or a warmed one is never found.
#[allow(clippy::too_many_arguments)]
pub(crate) fn preview_cache_key(
    image: &crate::models::AcquiredImage,
    file_only: &str,
    size: &str,
    stretch: bool,
    midtone: f64,
    shadow: f64,
    color: bool,
    mapping_revision: Option<&str>,
) -> String {
    let mut key = format!(
        "{}_{}_{}_{}_{}_{}_{}_{}_{}_{}",
        image.id,
        image.project_id,
        image.target_id,
        image.acquired_date.unwrap_or(0),
        file_only.replace(&['.', ' ', '-'][..], "_"),
        size,
        if stretch { "stretch" } else { "linear" },
        (midtone * 10000.0) as i32,
        (shadow * 10000.0) as i32,
        image_file_identity_cache_token(image),
    );
    if let Some(revision) = mapping_revision {
        let _ = write!(&mut key, "_mapping_{revision}");
    }
    // The colour marker remains last, so existing unmapped keys do not move.
    if color {
        key.push_str("_color");
    }
    key
}

pub(super) fn image_file_identity_cache_token(image: &crate::models::AcquiredImage) -> String {
    let mut hasher = Sha256::new();
    hasher.update(image.metadata.as_bytes());
    hasher.update([0]);
    hasher.update(image.filter_name.as_bytes());
    hasher.update([0]);
    hasher.update(image.acquired_date.unwrap_or_default().to_le_bytes());
    finish_cache_token(hasher)
}

pub(super) fn finish_cache_token(hasher: Sha256) -> String {
    let digest = hasher.finalize();
    let mut token = String::with_capacity(16);
    for byte in digest.iter().take(8) {
        let _ = write!(&mut token, "{byte:02x}");
    }
    token
}

pub(super) fn star_detection_cache_key(
    image: &crate::models::AcquiredImage,
    file_only: &str,
    source_token: &str,
) -> String {
    format!(
        "stars_v4_{}_{}_{}_{}_{}_{}_{}",
        image.id,
        image.project_id,
        image.target_id,
        image.acquired_date.unwrap_or(0),
        file_only.replace(&['.', ' ', '-'][..], "_"),
        image_file_identity_cache_token(image),
        source_token,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn psf_multi_cache_key(
    image: &crate::models::AcquiredImage,
    file_only: &str,
    num_stars: usize,
    psf_type: &str,
    sort_by: &str,
    selection: &str,
    grid_cols: Option<usize>,
    source_token: &str,
) -> String {
    format!(
        "psf_multi_v2_{}_{}_{}_{}_{}_{}_{}_{}_{}_{}_{}_{}",
        image.id,
        image.project_id,
        image.target_id,
        image.acquired_date.unwrap_or(0),
        file_only.replace(&['.', ' ', '-'][..], "_"),
        num_stars,
        psf_type,
        sort_by,
        selection,
        grid_cols.unwrap_or(0),
        image_file_identity_cache_token(image),
        source_token,
    )
}

/// Cache key for an annotated (star-marked) PNG. Same stability requirement.
pub(crate) fn annotated_cache_key(
    image: &crate::models::AcquiredImage,
    file_only: &str,
    size: &str,
    max_stars: usize,
    mapping_revision: Option<&str>,
) -> String {
    // v3: stars now carry HFR labels (v2: telescope-class presets), so
    // earlier renders are stale.
    let mut key = format!(
        "annotated_v4_{}_{}_{}_{}_{}_{}_{}_{}",
        image.id,
        image.project_id,
        image.target_id,
        image.acquired_date.unwrap_or(0),
        file_only.replace(&['.', ' ', '-'][..], "_"),
        size,
        max_stars,
        image_file_identity_cache_token(image),
    );
    if let Some(revision) = mapping_revision {
        let _ = write!(&mut key, "_mapping_{revision}");
    }
    key
}

/// A light's calibrated or registered copy, found on disk for a view.
pub(crate) struct ResolvedCopy {
    pub record: crate::frame_derivatives::DerivativeRecord,
    pub path: PathBuf,
    /// The file's fingerprint, so a rewritten copy renders anew.
    pub token: String,
}

/// Find copy `uuid` of this light. Not found when the copy belongs to
/// another light, when the light has no guid, or when no file of its name
/// and size is under the image roots.
pub(crate) fn resolve_copy(
    ctx: &DatabaseContext,
    image: &crate::models::AcquiredImage,
    uuid: &str,
) -> Result<ResolvedCopy, AppError> {
    let guid = image.guid.as_deref().ok_or(AppError::NotFound)?;
    let record = {
        let connection = ctx.db();
        let connection = connection.lock().map_err(AppError::db)?;
        crate::frame_derivatives::record_by_uuid(&connection, uuid)
            .map_err(|error| AppError::InternalError(format!("reading copy: {error:#}")))?
    }
    .filter(|record| record.acquired_image_guid == guid)
    .ok_or(AppError::NotFound)?;
    let tree = ctx
        .get_directory_tree()
        .map_err(|error| AppError::InternalError(format!("listing image folders: {error:#}")))?;
    let path = tree
        .find_file(&record.file_name)
        .and_then(|candidates| crate::frame_derivatives::resolve_record_path(&record, candidates))
        .ok_or(AppError::NotFound)?;
    let token = source_file_cache_token(&path)
        .ok_or_else(|| AppError::Conflict("The copy is not ready".to_string()))?;
    Ok(ResolvedCopy {
        record,
        path,
        token,
    })
}

pub(super) async fn resolve_copy_async(
    ctx: &Arc<DatabaseContext>,
    image: &crate::models::AcquiredImage,
    uuid: Option<&str>,
) -> Result<Option<ResolvedCopy>, AppError> {
    let Some(uuid) = uuid else {
        return Ok(None);
    };
    let ctx = Arc::clone(ctx);
    let image = image.clone();
    let uuid = uuid.to_string();
    tokio::task::spawn_blocking(move || resolve_copy(&ctx, &image, &uuid).map(Some))
        .await
        .map_err(|error| AppError::InternalError(format!("copy lookup failed: {error}")))?
}

/// A rendered artifact of a copy lives beside the light's own under a key
/// that names the copy and its file fingerprint, so the light's keys stay
/// where they are and a rewritten copy renders anew.
pub(crate) fn with_copy(key: String, copy: Option<&ResolvedCopy>) -> String {
    match copy {
        Some(copy) => format!("{key}_copy_{}_{}", copy.record.derivative_uuid, copy.token),
        None => key,
    }
}

/// The durable upload mapping is part of rendered-artifact identity. A worker
/// that started before a remap keeps writing its old key, while new requests
/// immediately address the new mapping's key.
pub(crate) fn rendered_artifact_mapping_revision(
    ctx: &DatabaseContext,
    image: &crate::models::AcquiredImage,
) -> Result<Option<String>, AppError> {
    use crate::server::remote_upload::{resolve_mapped_light_file, MappedLightFile};

    let mapping = {
        let connection = ctx.db();
        let connection = connection.lock().map_err(AppError::db)?;
        resolve_mapped_light_file(&connection, &ctx.image_dir_paths, image)?
    };
    match mapping {
        MappedLightFile::Absent => Ok(None),
        MappedLightFile::Invalid => Err(AppError::Conflict(
            "remote image provenance no longer matches this scheduler row".into(),
        )),
        MappedLightFile::Missing { artifact_revision }
        | MappedLightFile::Ready {
            artifact_revision, ..
        } => Ok(Some(artifact_revision)),
    }
}

/// Resolve the on-disk cache path for a preview/annotated artifact, creating
/// the category dir. `category` is `"previews"` or `"annotated"`.
/// Where an artifact lives, in the format new ones are written in.
///
/// The extension differs per format, so a PNG cache and a JPEG cache coexist:
/// changing the setting misses and regenerates rather than serving one as the
/// other, and changing it back finds the originals still valid.
pub(crate) fn artifact_cache_path(
    ctx: &DatabaseContext,
    category: &str,
    key: &str,
    encoding: crate::preview_format::PreviewEncoding,
) -> Result<PathBuf, AppError> {
    let cm = ctx.cache();
    cm.ensure_category_dir(category)
        .map_err(|e| AppError::InternalError(format!("Failed to create cache directory: {}", e)))?;
    Ok(cm.get_cached_path(category, key, encoding.extension()))
}

/// Serve a cached image, labelled as whatever it was written as rather than as
/// whatever the setting says now.
pub(crate) async fn serve_cached_png(
    cache_path: &std::path::Path,
    request_headers: &HeaderMap,
) -> Result<Response, AppError> {
    // The cache culls least recently used previews first when its volume
    // fills, so a preview someone looks at is marked as used.
    crate::server::cache_budget::note_served_soon(cache_path);
    let metadata = tokio::fs::metadata(cache_path)
        .await
        .map_err(|_| AppError::InternalError("Failed to read cache".to_string()))?;
    let mut etag_hash = Sha256::new();
    etag_hash.update(cache_path.to_string_lossy().as_bytes());
    etag_hash.update(metadata.len().to_le_bytes());
    if let Ok(modified) = metadata.modified()
        && let Ok(elapsed) = modified.duration_since(std::time::UNIX_EPOCH)
    {
        etag_hash.update(elapsed.as_secs().to_le_bytes());
        etag_hash.update(elapsed.subsec_nanos().to_le_bytes());
    }
    let digest = etag_hash.finalize();
    let mut etag = String::with_capacity(26);
    etag.push('"');
    for byte in digest.iter().take(12) {
        let _ = write!(&mut etag, "{byte:02x}");
    }
    etag.push('"');
    let not_modified = request_headers
        .get(IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|values| {
            values.split(',').any(|value| {
                let value = value.trim();
                value == etag || value.strip_prefix("W/") == Some(etag.as_str())
            })
        });

    let mut response = if not_modified {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        let buffer = tokio::fs::read(cache_path)
            .await
            .map_err(|_| AppError::InternalError("Failed to read cache".to_string()))?;
        (
            StatusCode::OK,
            [(
                CONTENT_TYPE,
                crate::preview_format::PreviewFormat::of_path(cache_path).content_type(),
            )],
            buffer,
        )
            .into_response()
    };
    // The URL names a scheduler row, not the current upload mapping. Require
    // revalidation, then let unchanged artifacts complete with a bodyless 304.
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    response.headers_mut().insert(
        ETAG,
        HeaderValue::from_str(&etag)
            .map_err(|_| AppError::InternalError("Invalid cache ETag".to_string()))?,
    );
    Ok(response)
}

pub(super) async fn write_cache_atomic(path: &FsPath, contents: &[u8]) -> std::io::Result<()> {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("cache");
    let temporary =
        path.with_extension(format!("{extension}.tmp.{}", uuid::Uuid::new_v4().simple()));
    if let Err(error) = tokio::fs::write(&temporary, contents).await {
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(error);
    }
    if let Err(error) = tokio::fs::rename(&temporary, path).await {
        let _ = tokio::fs::remove_file(&temporary).await;
        if !path.is_file() {
            return Err(error);
        }
    }
    Ok(())
}

pub(super) fn tracked_cache_matches_source(
    state: &AppState,
    cache_path: &FsPath,
    source_path: &FsPath,
) -> Result<bool, AppError> {
    if !cache_path.is_file() {
        return Ok(false);
    }
    if state
        .preview_queue
        .cached_source_matches(cache_path, source_path)
    {
        return Ok(true);
    }
    if let Err(error) = std::fs::remove_file(cache_path) {
        tracing::warn!(
            "Could not invalidate a preview whose delayed source changed: {}",
            error
        );
        return Err(AppError::InternalError(
            "Cached preview could not be refreshed".to_string(),
        ));
    }
    state.preview_queue.forget_completed_source(cache_path);
    Ok(false)
}

/// The immediate "not ready — poll for it" response on a cache miss. `<img>`
/// treats the non-image body as an error and the frontend then batch-polls the
/// generation-status endpoint.
pub(crate) fn generating_response() -> Response {
    (
        StatusCode::ACCEPTED,
        [(CACHE_CONTROL, "no-store")],
        Json(ApiResponse::success(
            crate::server::preview_queue::GenerationStatus {
                state: crate::server::preview_queue::GenerationState::Generating,
                error: None,
            },
        )),
    )
        .into_response()
}
