//! Importing frames into a database: the folders to choose from, the
//! import job and its progress, and automatic import.

use super::*;

/// Whether every requested directory sits at or below one of the
/// database's configured image roots, after resolving symlinks on both
/// sides. Paths that fail to resolve are NOT within the roots.
pub(super) fn dirs_within_roots(requested: &[String], roots: &[String]) -> bool {
    let resolved_roots: Vec<std::path::PathBuf> = roots
        .iter()
        .filter_map(|root| dunce::canonicalize(root).ok())
        .collect();
    if resolved_roots.is_empty() {
        return false;
    }
    requested.iter().all(|dir| {
        dunce::canonicalize(dir)
            .is_ok_and(|resolved| resolved_roots.iter().any(|root| resolved.starts_with(root)))
    })
}

/// One directory in the import folder listing.
#[derive(Debug, serde::Serialize)]
pub struct ImportFolder {
    pub path: String,
    pub name: String,
    pub children: Vec<ImportFolder>,
}

pub(super) fn list_subfolders(dir: &std::path::Path, depth: usize) -> Vec<ImportFolder> {
    if depth == 0 {
        return Vec::new();
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut folders: Vec<ImportFolder> = entries
        .flatten()
        .filter(|entry| entry.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                return None;
            }
            let path = entry.path();
            Some(ImportFolder {
                name,
                children: list_subfolders(&path, depth - 1),
                path: path.to_string_lossy().into_owned(),
            })
        })
        .collect();
    folders.sort_by(|a, b| a.name.cmp(&b.name));
    folders
}

/// `GET /api/db/{db_id}/import/folders` — the configured image roots and
/// two levels of their subdirectories, for choosing what an import scans.
pub async fn get_import_folders(
    ctx: DbContext,
) -> Result<Json<ApiResponse<Vec<ImportFolder>>>, AppError> {
    let roots = ctx.image_dirs.clone();
    let listing = tokio::task::spawn_blocking(move || {
        roots
            .iter()
            .map(|root| {
                let path = std::path::Path::new(root);
                ImportFolder {
                    name: path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| root.clone()),
                    path: root.clone(),
                    children: list_subfolders(path, 2),
                }
            })
            .collect::<Vec<_>>()
    })
    .await
    .map_err(|e| AppError::InternalError(format!("folder listing: {e}")))?;
    Ok(Json(ApiResponse::success(listing)))
}

/// `POST /api/db/{db_id}/import` — start a background FITS import into an
/// existing database. One import runs per database at a time.
pub async fn start_import_route(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Json(req): Json<ImportRequest>,
) -> Result<Json<ApiResponse<ImportStatusResponse>>, AppError> {
    // Importing into a configured database is ordinary work. Naming the
    // directories to read is not: that is a caller-chosen server path, so it
    // needs the same consent as registry CRUD. Without the flag we import
    // only from directories the operator already configured.
    let dirs = match req.image_dirs {
        Some(dirs) if !dirs.is_empty() => {
            // A folder inside an already-configured root was consented to
            // when the root was; only a NEW server path needs the
            // management grant.
            if !dirs_within_roots(&dirs, &ctx.image_dirs) {
                require_database_management_allowed(&state)?;
            }
            dirs
        }
        _ => ctx.image_dirs.clone(),
    };
    if dirs.is_empty() {
        return Err(AppError::BadRequest(
            "no image directories: pass image_dirs or configure them on the database".into(),
        ));
    }
    for dir in &dirs {
        if !std::path::Path::new(dir).is_dir() {
            return Err(AppError::BadRequest(format!(
                "image directory does not exist: {}",
                dir
            )));
        }
    }

    let options = crate::commands::import::ImportOptions {
        time_gap_days: req
            .time_gap_days
            .unwrap_or(crate::commands::import::grouping::DEFAULT_TIME_GAP_DAYS),
        profile_id: req.profile_id,
        dry_run: req.dry_run,
        attach_existing: req.attach_existing.unwrap_or(true),
        match_radius_deg: req
            .match_radius_deg
            .unwrap_or(crate::commands::import::DEFAULT_MATCH_RADIUS_DEG),
        // The import job applies the database's setting.
        match_by_position: true,
        scope: req.scope.unwrap_or_default(),
        skip_processed: req.skip_processed.unwrap_or(false),
        // The database's setting decides; the import job applies it.
        pair_derivatives: true,
        accept_other_rigs: req.accept_other_rigs.unwrap_or(false),
        only_new: false,
        // The import job applies the database's light filters.
        light_filter: Default::default(),
        // A person asked for this import, and previews it first.
        new_targets: crate::commands::import::NewTargets::Create,
    };
    let started = spawn_import_job(
        &state,
        ctx.0.clone(),
        dirs,
        options,
        req.backfill.unwrap_or(false),
        req.fill_metadata.unwrap_or(true),
    );
    Ok(Json(ApiResponse::success(ImportStatusResponse {
        started,
        progress: crate::server::import_job::progress_snapshot(&ctx.import_job),
    })))
}

/// `GET /api/db/{db_id}/autoimport` — the automatic import's settings,
/// schedule, and last run.
pub async fn get_autoimport_status(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
) -> Result<Json<ApiResponse<crate::server::autoimport::AutoImportStatus>>, AppError> {
    Ok(Json(ApiResponse::success(state.autoimport.status(&ctx))))
}

/// `POST /api/db/{db_id}/autoimport/run` — run the automatic import now,
/// with its configured scope, from the configured folders. Works whether or
/// not a schedule is set; the settings still decide what a run takes.
pub async fn run_autoimport_now(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
) -> Result<Json<ApiResponse<ImportStatusResponse>>, AppError> {
    let settings = ctx.autoimport.clone().unwrap_or_default();
    let started = crate::server::autoimport::start(
        &state,
        &ctx.0,
        &settings,
        crate::server::autoimport::Reason::Manual,
        std::time::Instant::now(),
    )
    .map_err(|error| AppError::BadRequest(error.to_string()))?;
    if !started {
        return Err(AppError::Conflict(
            "an import is already running for this database".into(),
        ));
    }
    Ok(Json(ApiResponse::success(ImportStatusResponse {
        started,
        progress: crate::server::import_job::progress_snapshot(&ctx.import_job),
    })))
}

/// `GET /api/db/{db_id}/import` — import job progress (1s poll).
pub async fn get_import_progress(
    ctx: DbContext,
) -> Result<Json<ApiResponse<ImportStatusResponse>>, AppError> {
    let progress = crate::server::import_job::progress_snapshot(&ctx.import_job);
    Ok(Json(ApiResponse::success(ImportStatusResponse {
        started: progress.running,
        progress,
    })))
}

/// Filesystem-safe stem for a managed database file, derived from the
/// user-facing name (`My Rig!` → `my-rig`).
pub(super) fn sanitize_file_stem(name: &str) -> String {
    let mut stem: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    while stem.contains("--") {
        stem = stem.replace("--", "-");
    }
    let stem = stem.trim_matches('-').to_string();
    if stem.is_empty() {
        "database".to_string()
    } else {
        stem
    }
}

/// First non-existing `<base>/<stem>[-N].sqlite`.
pub(super) fn unique_db_file(base: &std::path::Path, stem: &str) -> std::path::PathBuf {
    let candidate = base.join(format!("{stem}.sqlite"));
    if !candidate.exists() {
        return candidate;
    }
    for n in 2.. {
        let candidate = base.join(format!("{stem}-{n}.sqlite"));
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!()
}

/// Launch the singleton import job for one database: header scan → one-shot
/// import transaction. Optional quality work starts afterwards with the
/// background worker budget and does not delay import completion.
/// Returns false when a job is already running for this database.
pub(crate) fn spawn_import_job(
    state: &Arc<AppState>,
    ctx: Arc<DatabaseContext>,
    dirs: Vec<String>,
    options: crate::commands::import::ImportOptions,
    backfill: bool,
    fill_metadata: bool,
) -> bool {
    spawn_import_job_with_trigger(state, ctx, dirs, options, backfill, fill_metadata, "manual")
}

pub(crate) fn spawn_import_job_with_trigger(
    state: &Arc<AppState>,
    ctx: Arc<DatabaseContext>,
    dirs: Vec<String>,
    options: crate::commands::import::ImportOptions,
    backfill: bool,
    fill_metadata: bool,
    trigger: &str,
) -> bool {
    use crate::server::import_job as job;

    if state.shutdown.is_stopping() {
        return false;
    }
    if !job::try_begin(&ctx.import_job, dirs.clone()) {
        return false;
    }
    job::set_trigger(&ctx.import_job, trigger);
    tracing::info!(
        "📥 Import started for db={} ({} director{})",
        ctx.id,
        dirs.len(),
        if dirs.len() == 1 { "y" } else { "ies" }
    );

    let state = state.clone();
    // A stop lets an import already writing its transaction finish.
    let work = state.shutdown.hold(format!("import (db={})", ctx.id));
    tokio::spawn(async move {
        let _work = work;
        let job_store = ctx.import_job.clone();
        let _import_guard = ctx.image_import_mutex.lock().await;

        let blocking_ctx = ctx.clone();
        let blocking_options = options.clone();
        let dir_paths: Vec<std::path::PathBuf> =
            dirs.iter().map(std::path::PathBuf::from).collect();
        let shutdown = Arc::clone(&state.shutdown);
        let blocking_state = Arc::clone(&state);
        let import_result = tokio::task::spawn_blocking(move || {
            // The calibration frames before, so flat measuring is asked only
            // for the rigs this import touched.
            let flats_before =
                crate::server::flat_measure::rig_state_at(&blocking_ctx.database_path);
            run_import_blocking(
                &blocking_state.autoimport,
                &blocking_ctx,
                &dir_paths,
                &blocking_options,
                &|| shutdown.is_stopping(),
            )
            .map(|outcome| (outcome, flats_before))
        })
        .await;

        let (outcome, flats_before) = match import_result {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(e)) => {
                tracing::warn!("📥 Import for db={} failed: {:#}", ctx.id, e);
                job::finish(&job_store, Some(format!("{:#}", e)));
                return;
            }
            Err(join_err) => {
                tracing::error!("📥 Import task for db={} panicked: {}", ctx.id, join_err);
                job::finish(
                    &job_store,
                    Some(format!("import task panicked: {join_err}")),
                );
                return;
            }
        };

        tracing::info!(
            "📥 Import for db={} done: {} imported, {} projects, {} targets{}",
            ctx.id,
            outcome.imported,
            outcome.projects_created,
            outcome.targets_created,
            if outcome.dry_run { " (dry-run)" } else { "" }
        );
        let mut target_ids = outcome.created_target_ids.clone();
        for id in &outcome.attached_target_ids {
            if !target_ids.contains(id) {
                target_ids.push(*id);
            }
        }
        let dry_run = outcome.dry_run;
        let imported_any = outcome.imported > 0;
        job::complete_import(&job_store, outcome);
        if !dry_run {
            state.auto_stacks.touch_database(
                &ctx.id,
                crate::server::stack_preview::automatic::RefreshReason::Arrival,
            );
        }

        // New rows reference files the DB-based file cache hasn't seen; kick
        // the normal background refresh so the UI resolves them promptly.
        let _ = ctx.ensure_cache_available();

        // New calibration frames are graded before a stack or an export
        // relies on them; with none to measure this regrades the library.
        if !dry_run {
            crate::server::calibration_quality::request(&state, &ctx.database_path);
            // The flat sessions it touched are measured once grading is done,
            // and new lights shot during a change are asked which side they
            // are on.
            let (flat_state, path) = (Arc::clone(&state), ctx.database_path.clone());
            let _ = tokio::task::spawn_blocking(move || {
                crate::server::flat_measure::queue_change(
                    &flat_state,
                    &path,
                    flats_before.as_ref(),
                    imported_any,
                )
            })
            .await;
        }

        // Quality analysis is a general database maintenance job, not an
        // import stage. An opt-in import only queues the changed targets;
        // otherwise a database that analyzes new frames queues them itself.
        // A backfill already running refuses this one; the arrival queue
        // then picks the frames up once it finishes, if the database asks.
        let queued = backfill
            && !target_ids.is_empty()
            && spawn_quality_backfill(&state, ctx.clone(), target_ids, false, fill_metadata);
        if !queued && !dry_run && imported_any {
            crate::server::quality_arrival::note_arrival(&state, &ctx.id);
        }
    });
    true
}

/// Stages 1+2 of the import job, on a blocking thread: collect files, scan
/// headers (with progress), then run the single import transaction on a
/// dedicated connection (never the shared request connection). A server stop
/// (`stopping`) before the transaction leaves the catalog as it was.
pub(super) fn run_import_blocking(
    autoimport: &crate::server::autoimport::AutoImportScheduler,
    ctx: &DatabaseContext,
    dirs: &[std::path::PathBuf],
    options: &crate::commands::import::ImportOptions,
    stopping: &(dyn Fn() -> bool + Sync),
) -> anyhow::Result<crate::commands::import::ImportOutcome> {
    use crate::commands::import as imp;
    use crate::server::import_job as job;
    use rayon::prelude::*;

    // The database's scan rules apply to every import from its folders:
    // the patterns here, before any header is read, and the light filters
    // in the import itself.
    let path_filter = ctx
        .scan_rules
        .path_filter(&ctx.image_dir_paths)
        .map_err(|error| anyhow::anyhow!("scan rules: {error:#}"))?;
    let collected = imp::collect_image_files(dirs, &path_filter)?;
    let scan_rule_report = collected.skipped;
    let mut files = collected.files;
    let mut prefiltered = 0usize;
    let light_filter = ctx.scan_rules.light_filter();
    // Lights the filters left out of an earlier run, unchanged since.
    let mut left_out = Vec::new();
    if options.only_new && !files.is_empty() {
        let conn = open_scheduler_connection_with_flags(
            &ctx.database_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
        )
        .map_err(|e| anyhow::anyhow!("opening {} for import: {}", ctx.database_path, e))?;
        let known = imp::known_files(&conn, &files)?;
        prefiltered = known.len();
        files.retain(|path| !known.contains(path));
        left_out = crate::server::autoimport::take_left_out(
            autoimport,
            &ctx.database_path,
            &light_filter,
            dirs,
            &mut files,
        );
        job::set_prefiltered(&ctx.import_job, prefiltered);
        if files.is_empty() {
            // Nothing new: no header reads, no transaction, no migration.
            job::set_scan_totals(&ctx.import_job, 0, 0);
            let mut outcome = imp::ImportOutcome {
                scanned: prefiltered,
                skipped_existing: prefiltered,
                dry_run: options.dry_run,
                scan_rules: scan_rule_report,
                new_targets: options.new_targets,
                ..Default::default()
            };
            imp::count_left_out(&mut outcome, &left_out);
            return Ok(outcome);
        }
    }
    job::set_scan_totals(&ctx.import_job, files.len(), 0);

    let store = ctx.import_job.clone();
    let frames: Option<Vec<imp::headers::FrameMeta>> = files
        .par_iter()
        .map(|path| {
            if stopping() {
                return None;
            }
            let meta = imp::headers::read_frame_meta(path);
            store.write().unwrap().progress.scanned_files += 1;
            Some(meta)
        })
        .collect();
    let stopped = || anyhow::anyhow!("the server stopped before the import was written");
    let frames = frames.ok_or_else(stopped)?;
    if stopping() {
        return Err(stopped());
    }

    job::set_stage(&ctx.import_job, "importing");
    let mut conn = open_scheduler_connection_with_flags(
        &ctx.database_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(|e| anyhow::anyhow!("opening {} for import: {}", ctx.database_path, e))?;
    let mut options = options.clone();
    options.pair_derivatives &= ctx.pair_calibrated_copies.is_on();
    options.light_filter = light_filter;
    options.match_by_position = ctx.scan_rules.match_by_position;
    let mut outcome = imp::import_frames(&mut conn, frames, &options)?;
    if options.only_new {
        crate::server::autoimport::remember_left_out(
            autoimport,
            &ctx.database_path,
            &light_filter,
            &outcome.left_out_files,
        );
    }
    outcome.scanned += prefiltered;
    outcome.skipped_existing += prefiltered;
    outcome.scan_rules = scan_rule_report;
    imp::count_left_out(&mut outcome, &left_out);
    Ok(outcome)
}

/// Write scan-measured star count and HFR into `acquiredimage.metadata` for
/// frames that imported without them, so an imported catalog carries the same
/// star metadata a N.I.N.A. catalog does. `pending` is `(image id, filename,
/// metadata JSON as read, mapped source revision)`. The patch fills only missing keys — a measured
/// N.I.N.A. value is never overwritten — and a row that changed since it was
/// read is left alone.
pub(super) fn fill_missing_star_metadata(
    ctx: &DatabaseContext,
    pending: &[(i32, String, String, Option<String>)],
) -> usize {
    use crate::server::spatial_scan as scan;

    let patches: Vec<(i32, &String, String, Option<String>)> = pending
        .iter()
        .filter_map(|(image_id, filename, metadata, mapped_source_revision)| {
            let entry = scan::valid_quality_entry_for_source(
                &ctx.spatial_metrics,
                *image_id,
                filename,
                mapped_source_revision.as_deref(),
            )
            // HocusFocus's stand-in values stay in the scan cache: written
            // into the catalog they would read as N.I.N.A.'s own scale.
            .filter(|entry| entry.detector == scan::QUALITY_DETECTOR)?;
            scan::star_metrics_metadata_patch(
                metadata,
                entry.star_count,
                entry.avg_hfr,
                entry.source_revision.as_deref(),
            )
            .map(|updated| (*image_id, metadata, updated, mapped_source_revision.clone()))
        })
        .collect();
    if patches.is_empty() {
        return 0;
    }

    // A dedicated connection: this runs on a blocking scan thread and must
    // not tie up the shared request connection.
    let mut conn =
        match crate::server::database_context::open_scheduler_connection(&ctx.database_path) {
            Ok(conn) => conn,
            Err(e) => {
                tracing::warn!("📐 Star-metadata fill skipped for db={}: {}", ctx.id, e);
                return 0;
            }
        };
    // Reserve the writer before re-reading upload provenance. This makes the
    // source check and metadata CAS one serializable operation with a remote
    // upload's mapping update and stale-metadata cleanup.
    let transaction = match conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
    {
        Ok(transaction) => transaction,
        Err(e) => {
            tracing::warn!("📐 Star-metadata fill skipped for db={}: {}", ctx.id, e);
            return 0;
        }
    };
    let image_ids = patches
        .iter()
        .map(|(image_id, _, _, _)| *image_id)
        .collect::<Vec<_>>();
    let images = match Database::new(&transaction).get_images_by_ids(&image_ids) {
        Ok(images) => images,
        Err(e) => {
            tracing::warn!("📐 Star-metadata fill skipped for db={}: {}", ctx.id, e);
            return 0;
        }
    };
    let mapped_sources = match crate::server::remote_upload::mapped_light_sources(
        &transaction,
        &ctx.image_dir_paths,
        images.iter(),
    ) {
        Ok(sources) => sources,
        Err(e) => {
            tracing::warn!(
                "📐 Star-metadata provenance check skipped for db={}: {:?}",
                ctx.id,
                e
            );
            return 0;
        }
    };
    let filled = {
        let db = Database::new(&transaction);
        let mut filled = 0usize;
        for (image_id, expected, updated, expected_mapped_source) in &patches {
            if mapped_sources.quality_revision(*image_id) != expected_mapped_source.as_deref() {
                continue;
            }
            match db.update_image_metadata_if_unchanged(*image_id, expected, updated) {
                Ok(true) => filled += 1,
                Ok(false) => {} // concurrent writer wins; skip quietly
                Err(e) => {
                    tracing::warn!("📐 Star-metadata fill failed for image {}: {}", image_id, e)
                }
            }
        }
        filled
    };
    if let Err(e) = transaction.commit() {
        tracing::warn!(
            "📐 Star-metadata fill commit failed for db={}: {}",
            ctx.id,
            e
        );
        return 0;
    }
    if filled > 0 {
        tracing::info!(
            "📐 Filled measured star metrics into {} imported image(s) (db={})",
            filled,
            ctx.id
        );
    }
    filled
}

/// Queue the database-wide quality analysis of `target_ids`. False when an
/// analysis is already queued or running, or the server is shutting down.
pub(crate) fn spawn_quality_backfill(
    state: &Arc<AppState>,
    ctx: Arc<DatabaseContext>,
    target_ids: Vec<i32>,
    force: bool,
    fill_metadata: bool,
) -> bool {
    crate::server::quality_scans::queue_analysis(state, &ctx, target_ids, force, fill_metadata)
}
