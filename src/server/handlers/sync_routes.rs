//! Syncing one database into another: preview, apply and the scheduler
//! sync routes.

use super::*;

/// `POST /api/databases/{db_id}/sync` — preview or run one safe scheduler
/// database sync. The path database is the local working copy. A pull reads
/// the peer and fills the local copy; a planning push reads the local copy and
/// updates planning settings in the peer.
pub async fn sync_database_route(
    State(state): State<Arc<AppState>>,
    Path(db_id): Path<String>,
    Json(req): Json<SchedulerSyncRequest>,
) -> Result<Json<ApiResponse<SchedulerSyncResponse>>, AppError> {
    let _apply_guard = if req.dry_run {
        None
    } else {
        Some(state.sync_apply_lock.lock().await)
    };
    let response = execute_scheduler_sync(&state, &db_id, req).await?;
    Ok(Json(ApiResponse::success(response)))
}

pub(super) async fn execute_scheduler_sync(
    state: &Arc<AppState>,
    db_id: &str,
    req: SchedulerSyncRequest,
) -> Result<SchedulerSyncResponse, AppError> {
    execute_scheduler_sync_guarded(state, db_id, req, None, SyncGuardMode::None)
        .await
        .map(|(response, _)| response)
}

pub(crate) enum SyncGuardMode {
    None,
    Preview,
    Apply {
        destination_fingerprint: String,
    },
    /// Activation has a frozen, project-scoped source rather than a whole catalog.
    PlanningPreview,
    PlanningApply {
        destination_fingerprint: String,
    },
}

#[derive(Debug)]
pub(super) struct StaleSyncPreview;

impl std::fmt::Display for StaleSyncPreview {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("sync preview destination changed")
    }
}

impl std::error::Error for StaleSyncPreview {}

pub(super) async fn execute_scheduler_sync_guarded(
    state: &Arc<AppState>,
    db_id: &str,
    req: SchedulerSyncRequest,
    source_override: Option<PathBuf>,
    guard_mode: SyncGuardMode,
) -> Result<(SchedulerSyncResponse, Option<String>), AppError> {
    if db_id == req.peer_db_id {
        return Err(AppError::BadRequest(
            "Choose two different databases to sync.".into(),
        ));
    }

    let local = state.get_database(db_id).ok_or(AppError::NotFound)?;
    let (source_path, destination_path) = sync_endpoint_paths(state, db_id, &req)?;
    let (source_id, destination_id) = match req.kind {
        SchedulerSyncKind::Pull => (req.peer_db_id.clone(), local.id.clone()),
        _ => (local.id.clone(), req.peer_db_id.clone()),
    };
    let source_path = source_override.unwrap_or(source_path);
    execute_scheduler_sync_paths(
        state,
        source_path,
        destination_path,
        source_id,
        destination_id,
        req,
        guard_mode,
    )
    .await
}

/// Cap a change-line list for transport; the tail line names the overflow.
pub(super) fn capped_change_lines(mut lines: Vec<String>) -> Vec<String> {
    const MAX_CHANGE_LINES: usize = 400;
    if lines.len() > MAX_CHANGE_LINES {
        let more = lines.len() - MAX_CHANGE_LINES;
        lines.truncate(MAX_CHANGE_LINES);
        lines.push(format!("… and {more} more"));
    }
    lines
}

pub(super) fn grade_label(status: i32) -> &'static str {
    match status {
        1 => "Accepted",
        2 => "Rejected",
        _ => "Pending",
    }
}

pub(super) fn open_scheduler_sync_connections(
    source_path: &FsPath,
    destination_path: &FsPath,
) -> anyhow::Result<(rusqlite::Connection, rusqlite::Connection)> {
    use rusqlite::OpenFlags;

    let source =
        open_scheduler_connection_with_flags(source_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .with_context(|| {
                format!(
                    "opening source scheduler database {}",
                    source_path.display()
                )
            })?;
    let destination =
        open_scheduler_connection_with_flags(destination_path, OpenFlags::SQLITE_OPEN_READ_WRITE)
            .with_context(|| {
            format!(
                "opening destination scheduler database {}",
                destination_path.display()
            )
        })?;

    Ok((source, destination))
}

pub(super) fn classify_scheduler_sync_failure(
    error: &anyhow::Error,
    source_path: &str,
    destination_path: &str,
) -> AppError {
    if error.downcast_ref::<StaleSyncPreview>().is_some() {
        return AppError::Conflict(
            "This preview is stale because the destination database changed. Preview again.".into(),
        );
    }

    let message = format!(
        "syncing source scheduler database {source_path} into destination scheduler database {destination_path}: {error:#}"
    );
    if error_is_sqlite_contention(error) {
        AppError::DatabaseError(format!(
            "{message}; a scheduler database remained busy or locked after the lock wait expired"
        ))
    } else if error_has_sqlite_failure(error) {
        AppError::DatabaseError(message)
    } else {
        AppError::BadRequest(message)
    }
}

pub(super) fn classify_sync_snapshot_failure(error: anyhow::Error, source_path: &str) -> AppError {
    let message =
        format!("creating a scheduler sync snapshot from source database {source_path}: {error:#}");
    let contention = if error_is_sqlite_contention(&error)
        || crate::server::sync_preview::error_is_sync_snapshot_contention(&error)
    {
        "; the source scheduler database remained busy or locked after the lock wait expired"
    } else {
        ""
    };
    // The peer path comes from the server's registry and the snapshot path is
    // server-owned. Open, backup, mkdir, and rename failures are operational;
    // none are malformed request data.
    AppError::DatabaseError(format!("{message}{contention}"))
}

#[cfg(test)]
mod scheduler_sync_error_tests;

pub(crate) async fn execute_scheduler_sync_paths(
    state: &Arc<AppState>,
    source_path: PathBuf,
    destination_path: PathBuf,
    source_id: String,
    destination_id: String,
    req: SchedulerSyncRequest,
    guard_mode: SyncGuardMode,
) -> Result<(SchedulerSyncResponse, Option<String>), AppError> {
    use crate::commands::sync::{
        parse_status, sync_grades, sync_grades_in_transaction, sync_planning,
        sync_planning_in_transaction, sync_pull, sync_pull_in_transaction, PlanningOptions,
        PullOptions, SyncGradesOptions,
    };
    use rusqlite::{Connection, Transaction, TransactionBehavior};

    let scoped_planning = matches!(
        &guard_mode,
        SyncGuardMode::PlanningPreview | SyncGuardMode::PlanningApply { .. }
    );
    if scoped_planning && !matches!(req.kind, SchedulerSyncKind::PushPlanning) {
        return Err(AppError::BadRequest(
            "A planning guard requires a planning transfer.".into(),
        ));
    }
    let fingerprint_queries = crate::server::sync_preview::fingerprint_queries(&req);
    let kind = req.kind;
    let dry_run = req.dry_run;
    let project = req.project;
    let target = req.target;
    let status_filter = if matches!(kind, SchedulerSyncKind::PushGrades) {
        req.status
            .as_deref()
            .map(parse_status)
            .transpose()
            .map_err(|error| AppError::BadRequest(error.to_string()))?
    } else {
        None
    };
    let reviewed_only = req.reviewed_only;
    let with_image_data = req.with_image_data.unwrap_or(true);
    let destination_id_for_cache = destination_id.clone();
    let source_path_for_error = source_path.display().to_string();
    let destination_path_for_error = destination_path.display().to_string();
    // The calibration frames before a pull, so flat measuring is asked only
    // for the rigs it touched.
    let flats_before = if !dry_run && matches!(kind, SchedulerSyncKind::Pull) {
        let path = destination_path.display().to_string();
        tokio::task::spawn_blocking(move || crate::server::flat_measure::rig_state_at(&path))
            .await
            .ok()
            .flatten()
    } else {
        None
    };

    let response = tokio::task::spawn_blocking(
        move || -> anyhow::Result<(
            SchedulerSyncResponse,
            Option<String>,
            Vec<crate::commands::sync::ChangedAcquiredImage>,
        )> {
            let source_canon =
                std::fs::canonicalize(&source_path).unwrap_or_else(|_| source_path.clone());
            let destination_canon = std::fs::canonicalize(&destination_path)
                .unwrap_or_else(|_| destination_path.clone());
            anyhow::ensure!(
                source_canon != destination_canon,
                "The two configured entries point to the same database file."
            );

            let (source, destination) =
                open_scheduler_sync_connections(&source_path, &destination_path)?;

            let run = |transaction: Option<&Transaction<'_>>| -> anyhow::Result<(
                SchedulerSyncResponse,
                Vec<crate::commands::sync::ChangedAcquiredImage>,
            )> {
                    let (response, changed_image_ids) = match kind {
                        SchedulerSyncKind::Pull => {
                            let options = PullOptions {
                                dry_run,
                                with_image_data,
                                project_filter: project.clone(),
                            };
                        let mut summary = match transaction {
                                Some(transaction) => {
                                    sync_pull_in_transaction(&source, transaction, &options)?
                                }
                                None => sync_pull(&source, &destination, &options)?,
                            };
                        let changed_images = std::mem::take(&mut summary.changed_acquiredimages);
                            (SchedulerSyncResponse {
                                kind,
                                dry_run,
                                source_db_id: source_id,
                                destination_db_id: destination_id,
                                exposuretemplate: (&summary.exposuretemplate).into(),
                                project: (&summary.project).into(),
                                ruleweight: (&summary.ruleweight).into(),
                                target: (&summary.target).into(),
                                exposureplan: (&summary.exposureplan).into(),
                                acquiredimage: Some((&summary.acquiredimage).into()),
                                imagedata: summary
                                    .imagedata_synced
                                    .then(|| (&summary.imagedata).into()),
                                grades: None,
                                calibration: Some(SchedulerSyncCalibrationCounts {
                                    frames: (&summary.calibration_frames).into(),
                                    grades: (&summary.calibration_grades).into(),
                                    kept_out: summary.calibration_choices.kept_out,
                                    used_anyway: summary.calibration_choices.used_anyway,
                                    cleared: summary.calibration_choices.cleared,
                                    flat_groups: summary.calibration_choices.flat_groups,
                                    flat_lights: summary.calibration_choices.flat_lights,
                                }),
                                grade_filled: summary.grade_filled,
                                grade_preserved: summary.grade_preserved,
                                adopted: summary.adopted,
                                imagedata_bytes: summary.imagedata_bytes,
                                total_inserted: summary.total_inserted(),
                                total_updated: summary.total_updated(),
                                changes: capped_change_lines(summary.changes),
                            }, changed_images)
                        }
                        SchedulerSyncKind::PushPlanning => {
                            let options = PlanningOptions {
                                dry_run,
                                project_filter: project.clone(),
                            };
                            let summary = match transaction {
                                Some(transaction) => {
                                    sync_planning_in_transaction(&source, transaction, &options)?
                                }
                                None => sync_planning(&source, &destination, &options)?,
                            };
                            (SchedulerSyncResponse {
                                kind,
                                dry_run,
                                source_db_id: source_id,
                                destination_db_id: destination_id,
                                exposuretemplate: (&summary.exposuretemplate).into(),
                                project: (&summary.project).into(),
                                ruleweight: (&summary.ruleweight).into(),
                                target: (&summary.target).into(),
                                exposureplan: (&summary.exposureplan).into(),
                                acquiredimage: None,
                                imagedata: None,
                                grades: None,
                                calibration: None,
                                grade_filled: 0,
                                grade_preserved: 0,
                                adopted: 0,
                                imagedata_bytes: 0,
                                total_inserted: summary.total_inserted(),
                                total_updated: summary.total_updated(),
                                changes: capped_change_lines(summary.changes),
                            }, Vec::new())
                        }
                        SchedulerSyncKind::PushGrades => {
                            let options = SyncGradesOptions {
                                status_filter,
                                reviewed_only,
                                project_filter: project.clone(),
                                target_filter: target.clone(),
                                dry_run,
                            };
                            let summary = match transaction {
                                Some(transaction) => {
                                    sync_grades_in_transaction(&source, transaction, &options)?
                                }
                                None => sync_grades(&source, &destination, &options)?,
                            };
                            (SchedulerSyncResponse {
                                kind,
                                dry_run,
                                source_db_id: source_id,
                                destination_db_id: destination_id,
                                exposuretemplate: SchedulerSyncTableCounts::default(),
                                project: SchedulerSyncTableCounts::default(),
                                ruleweight: SchedulerSyncTableCounts::default(),
                                target: SchedulerSyncTableCounts::default(),
                                exposureplan: SchedulerSyncTableCounts::default(),
                                acquiredimage: None,
                                imagedata: None,
                                grades: Some(SchedulerSyncGradeCounts {
                                    source_considered: summary.source_considered,
                                    source_no_guid: summary.source_no_guid,
                                    matched: summary.matched,
                                    changed: summary.changed,
                                    unchanged: summary.unchanged,
                                    unmatched_source: summary.unmatched_source,
                                    destination_only: summary.dest_only,
                                    duplicate_guids: summary.duplicate_guids,
                                    transitions: summary.transitions,
                                }),
                                calibration: None,
                                grade_filled: 0,
                                grade_preserved: 0,
                                adopted: 0,
                                imagedata_bytes: 0,
                                total_inserted: 0,
                                total_updated: summary.changed,
                                changes: capped_change_lines(
                                    summary
                                        .changes
                                        .iter()
                                        .map(|change| {
                                            let reason = change
                                                .reason
                                                .as_deref()
                                                .map(|reason| format!(" ({reason})"))
                                                .unwrap_or_default();
                                            format!(
                                                "grade {}: {} → {}{}",
                                                change.guid,
                                                grade_label(change.from),
                                                grade_label(change.to),
                                                reason,
                                            )
                                        })
                                        .collect(),
                                ),
                            }, Vec::new())
                        }
                    };
                    Ok((response, changed_image_ids))
                };

            let fingerprint = |destination: &Connection| {
                if scoped_planning {
                    crate::server::sync_preview::planning_destination_fingerprint(&source, destination)
                } else {
                    crate::server::sync_preview::connection_fingerprint(destination, &fingerprint_queries)
                }
            };
            match guard_mode {
                SyncGuardMode::None => {
                    let (response, changed_image_ids) = run(None)?;
                    Ok((response, None, changed_image_ids))
                }
                SyncGuardMode::Preview | SyncGuardMode::PlanningPreview => {
                    // A pull or planning preview writes, then rolls back; a
                    // grade preview only reads.
                    let behavior = if matches!(kind, SchedulerSyncKind::PushGrades) {
                        TransactionBehavior::Deferred
                    } else {
                        TransactionBehavior::Immediate
                    };
                    let transaction = Transaction::new_unchecked(&destination, behavior)?;
                    let fingerprint = fingerprint(&transaction)?;
                    let (response, _) = run(Some(&transaction))?;
                    transaction.rollback()?;
                    Ok((response, Some(fingerprint), Vec::new()))
                }
                SyncGuardMode::Apply { destination_fingerprint }
                | SyncGuardMode::PlanningApply { destination_fingerprint } => {
                    let transaction =
                        Transaction::new_unchecked(&destination, TransactionBehavior::Immediate)?;
                    let fingerprint = fingerprint(&transaction)?;
                    if fingerprint != destination_fingerprint {
                        return Err(StaleSyncPreview.into());
                    }
                    let (response, changed_image_ids) = run(Some(&transaction))?;
                    transaction.commit()?;
                    Ok((response, None, changed_image_ids))
                }
            }
        },
    )
    .await
    .map_err(|error| AppError::InternalError(format!("scheduler sync task failed: {error}")))?
    .map_err(|error| {
        classify_scheduler_sync_failure(
            &error,
            &source_path_for_error,
            &destination_path_for_error,
        )
    })?;

    let (response, fingerprint, changed_images) = response;
    let new_lights = !changed_images.is_empty();
    if !dry_run
        && let Some(destination_ctx) = state.get_database(&destination_id_for_cache)
        && let Err(error) = tokio::task::spawn_blocking(move || {
            destination_ctx.register_remote_image_arrivals(&changed_images);
            let _ = destination_ctx.ensure_cache_available();
        })
        .await
    {
        // The database transaction already committed. A cache bookkeeping
        // panic must not report the apply as failed and invite a duplicate
        // retry; the ordinary refresh remains the fallback.
        tracing::error!(
            "scheduler cache update panicked for db={}: {}",
            destination_id_for_cache,
            error
        );
    }
    if !dry_run {
        state.auto_stacks.touch_database(
            &destination_id_for_cache,
            crate::server::stack_preview::automatic::RefreshReason::Sync,
        );
        crate::server::quality_arrival::note_arrival(state, &destination_id_for_cache);
        // Synced calibration frames are graded, and then every project's
        // calibration health is judged again.
        if let Some(destination) = state.get_database(&destination_id_for_cache) {
            crate::server::calibration_quality::request(state, &destination.database_path);
            if matches!(kind, SchedulerSyncKind::Pull) {
                let (state, path) = (Arc::clone(state), destination.database_path.clone());
                let _ = tokio::task::spawn_blocking(move || {
                    crate::server::flat_measure::queue_change(
                        &state,
                        &path,
                        flats_before.as_ref(),
                        new_lights,
                    )
                })
                .await;
            }
        }
    }
    Ok((response, fingerprint))
}

pub(super) fn sync_endpoint_paths(
    state: &AppState,
    local_db_id: &str,
    request: &SchedulerSyncRequest,
) -> Result<(PathBuf, PathBuf), AppError> {
    if local_db_id == request.peer_db_id {
        return Err(AppError::BadRequest(
            "Choose two different databases to sync.".into(),
        ));
    }
    let local = state.get_database(local_db_id).ok_or(AppError::NotFound)?;
    let peer_path = if let Some(owner) = request.peer_db_id.strip_prefix("ts:") {
        let registry = crate::db_registry::DbRegistry::load_or_init(&require_registry_path(state)?)
            .map_err(|e| AppError::InternalError(e.to_string()))?;
        let settings = registry
            .find(owner)
            .and_then(|e| e.ts_sync.as_ref())
            .ok_or(AppError::NotFound)?;
        PathBuf::from(&settings.source_path)
    } else {
        PathBuf::from(
            &state
                .get_database(&request.peer_db_id)
                .ok_or(AppError::NotFound)?
                .database_path,
        )
    };
    let local_path = PathBuf::from(&local.database_path);
    Ok(match request.kind {
        SchedulerSyncKind::Pull => (peer_path, local_path),
        _ => (local_path, peer_path),
    })
}

/// Create a server-owned dry preview. Apply is a separate endpoint keyed by
/// the returned opaque preview ID.
pub async fn preview_sync_database_route(
    State(state): State<Arc<AppState>>,
    Path(db_id): Path<String>,
    Json(mut request): Json<SchedulerSyncRequest>,
) -> Result<Json<ApiResponse<SchedulerSyncPreviewResponse>>, AppError> {
    request.dry_run = true;
    let (source_path, _destination_path) = sync_endpoint_paths(&state, &db_id, &request)?;
    let source_path_for_error = source_path.display().to_string();
    let snapshot_state = state.clone();
    let source_snapshot_file = tokio::task::spawn_blocking(move || {
        snapshot_state
            .sync_previews
            .create_source_snapshot(&source_path)
    })
    .await
    .map_err(|error| AppError::InternalError(format!("sync snapshot task failed: {error}")))?
    .map_err(|error| classify_sync_snapshot_failure(error, &source_path_for_error))?;
    let source_snapshot_path = state
        .sync_previews
        .source_snapshot_path_for_file(&source_snapshot_file)
        .map_err(|error| AppError::InternalError(format!("{error:#}")))?;
    let preview = execute_scheduler_sync_guarded(
        &state,
        &db_id,
        request.clone(),
        Some(source_snapshot_path),
        SyncGuardMode::Preview,
    )
    .await;
    let (result, destination_fingerprint) = match preview {
        Ok((result, Some(fingerprint))) => (result, fingerprint),
        Ok(_) => unreachable!("preview execution always returns a fingerprint"),
        Err(error) => {
            state
                .sync_previews
                .remove_source_snapshot(&source_snapshot_file);
            return Err(error);
        }
    };

    let record = state
        .sync_previews
        .store(
            db_id,
            request,
            source_snapshot_file.clone(),
            destination_fingerprint,
            result.clone(),
        )
        .map_err(|error| {
            state
                .sync_previews
                .remove_source_snapshot(&source_snapshot_file);
            AppError::InternalError(format!("saving scheduler sync preview: {error}"))
        })?;
    Ok(Json(ApiResponse::success(SchedulerSyncPreviewResponse {
        preview_id: record.id,
        created_at: record.created_at,
        expires_at: record.expires_at,
        result,
    })))
}

pub async fn get_sync_database_preview_route(
    State(state): State<Arc<AppState>>,
    Path((db_id, preview_id)): Path<(String, String)>,
) -> Result<Json<ApiResponse<SchedulerSyncPreviewResponse>>, AppError> {
    let record = state
        .sync_previews
        .get(&preview_id)
        .map_err(|error| AppError::InternalError(format!("loading sync preview: {error}")))?
        .filter(|record| record.local_db_id == db_id && !record.activation)
        .ok_or(AppError::NotFound)?;
    Ok(Json(ApiResponse::success(SchedulerSyncPreviewResponse {
        preview_id: record.id,
        created_at: record.created_at,
        expires_at: record.expires_at,
        result: record.result,
    })))
}

pub async fn delete_sync_database_preview_route(
    State(state): State<Arc<AppState>>,
    Path((db_id, preview_id)): Path<(String, String)>,
) -> Result<Json<ApiResponse<bool>>, AppError> {
    let deleted = state
        .sync_previews
        .discard(&preview_id, &db_id)
        .map_err(|error| AppError::InternalError(format!("deleting sync preview: {error}")))?;
    Ok(Json(ApiResponse::success(deleted)))
}

/// Every unexpired staged transfer for this catalog, wherever it was
/// created. The UI's own previews live in its session, but a remote
/// client's push also parks a preview here, and without this listing the
/// operator has no way to see or act on it.
pub async fn list_sync_database_previews_route(
    State(state): State<Arc<AppState>>,
    Path(db_id): Path<String>,
) -> Result<Json<ApiResponse<Vec<SchedulerSyncPreviewListEntry>>>, AppError> {
    let records = state
        .sync_previews
        .list(&db_id)
        .map_err(|error| AppError::InternalError(format!("listing sync previews: {error}")))?;
    let entries = records
        .into_iter()
        .map(|record| SchedulerSyncPreviewListEntry {
            preview_id: record.id,
            kind: crate::server::remote_sync::kind_label(record.request.kind).to_string(),
            source: record.request.peer_db_id,
            created_at: record.created_at,
            expires_at: record.expires_at,
            result: record.result,
        })
        .collect();
    Ok(Json(ApiResponse::success(entries)))
}

/// Re-run one preview's dry run against the catalog as it stands now, so a
/// preview gone stale (the fingerprint moved) becomes applicable again
/// without the remote client re-uploading its bundle.
pub async fn refresh_sync_database_preview_route(
    State(state): State<Arc<AppState>>,
    Path((db_id, preview_id)): Path<(String, String)>,
) -> Result<Json<ApiResponse<SchedulerSyncPreviewResponse>>, AppError> {
    // Refresh rewrites a record an apply may be claiming: same lock as apply.
    let _apply_guard = state.sync_apply_lock.lock().await;
    let record = state
        .sync_previews
        .get(&preview_id)
        .map_err(|error| AppError::InternalError(format!("loading sync preview: {error}")))?
        .filter(|record| record.local_db_id == db_id && !record.activation)
        .ok_or(AppError::NotFound)?;
    let source_snapshot = state
        .sync_previews
        .source_snapshot_path(&record)
        .map_err(|error| AppError::InternalError(format!("{error:#}")))?;
    let mut request = record.request.clone();
    request.dry_run = true;
    let (result, fingerprint) = execute_scheduler_sync_guarded(
        &state,
        &db_id,
        request,
        Some(source_snapshot),
        SyncGuardMode::Preview,
    )
    .await?;
    let fingerprint = fingerprint
        .ok_or_else(|| AppError::InternalError("preview refresh returned no fingerprint".into()))?;
    let refreshed = state
        .sync_previews
        .refresh(&record, fingerprint, result)
        .map_err(|error| AppError::InternalError(format!("storing refreshed preview: {error}")))?;
    Ok(Json(ApiResponse::success(SchedulerSyncPreviewResponse {
        preview_id: refreshed.id,
        created_at: refreshed.created_at,
        expires_at: refreshed.expires_at,
        result: refreshed.result,
    })))
}

/// Apply one unexpired preview after proving that neither catalog changed.
pub async fn apply_sync_database_preview_route(
    State(state): State<Arc<AppState>>,
    Path((db_id, preview_id)): Path<(String, String)>,
) -> Result<Json<ApiResponse<SchedulerSyncResponse>>, AppError> {
    let _apply_guard = state.sync_apply_lock.lock().await;
    let record = state
        .sync_previews
        .claim(&preview_id, &db_id)
        .map_err(|error| AppError::InternalError(format!("loading sync preview: {error}")))?
        .ok_or(AppError::NotFound)?;

    let source_snapshot = state
        .sync_previews
        .source_snapshot_path(&record)
        .map_err(|error| AppError::InternalError(format!("{error:#}")))?;

    let mut request = record.request.clone();
    request.dry_run = false;
    let result = execute_scheduler_sync_guarded(
        &state,
        &db_id,
        request,
        Some(source_snapshot),
        SyncGuardMode::Apply {
            destination_fingerprint: record.destination_fingerprint.clone(),
        },
    )
    .await
    .map(|(result, _)| result);
    let result = match result {
        Ok(result) => result,
        Err(error) => {
            // Nothing was written, so the snapshot still holds exactly the
            // source rows the user reviewed. Put the preview back rather than
            // making them rebuild it to retry.
            if let Err(restore_error) = state.sync_previews.restore(&record) {
                tracing::warn!("could not restore sync preview {preview_id}: {restore_error:#}");
            }
            return Err(error);
        }
    };
    state
        .sync_previews
        .remove_source_snapshot(&record.source_snapshot_file);
    Ok(Json(ApiResponse::success(result)))
}
