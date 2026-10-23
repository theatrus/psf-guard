//! Syncing one database into another: preview, apply and the scheduler
//! sync routes.

use super::*;
use crate::concurrency::Stop;

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
    /// A dry run, rolled back. Its stop interrupts the run's SQLite work.
    Preview {
        stop: Stop,
    },
    Apply {
        destination_fingerprint: String,
    },
    /// Activation has a frozen, project-scoped source rather than a whole catalog.
    PlanningPreview {
        stop: Stop,
    },
    PlanningApply {
        destination_fingerprint: String,
    },
}

impl SyncGuardMode {
    /// A preview's stop; `None` for a run that writes.
    fn preview_stop(&self) -> Option<&Stop> {
        match self {
            Self::Preview { stop } | Self::PlanningPreview { stop } => Some(stop),
            _ => None,
        }
    }
}

/// A preview stopped before it finished: a newer preview of the same sync
/// replaced it, or a person stopped it.
#[derive(Debug)]
pub(crate) struct PreviewStopped;

impl std::fmt::Display for PreviewStopped {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("sync preview stopped")
    }
}

impl std::error::Error for PreviewStopped {}

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
    if error.downcast_ref::<PreviewStopped>().is_some() {
        return AppError::Conflict("This sync preview was stopped before it finished.".into());
    }
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

/// What one scheduler sync produced, for the work the server does after it.
struct SyncRun {
    response: SchedulerSyncResponse,
    /// The destination fingerprint a preview recorded.
    fingerprint: Option<String>,
    /// Frames the sync inserted or updated, whose files may arrive later.
    changed_images: Vec<crate::commands::sync::ChangedAcquiredImage>,
    /// GUIDs of the destination projects the sync deleted because the
    /// source catalog deleted them.
    deleted_projects: Vec<String>,
}

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
        SyncGuardMode::PlanningPreview { .. } | SyncGuardMode::PlanningApply { .. }
    );
    if scoped_planning && !matches!(req.kind, SchedulerSyncKind::PushPlanning) {
        return Err(AppError::BadRequest(
            "A planning guard requires a planning transfer.".into(),
        ));
    }
    let fingerprint_queries = crate::server::sync_preview::fingerprint_queries(&req);
    // A planning push into a registered catalog writes a PSF Guard catalog,
    // which records a project it deletes as a tombstone. A push into a
    // Target Scheduler file (a `ts:` connection, Director file delivery)
    // writes no PSF Guard table there.
    let destination_is_catalog = state.get_database(&destination_id).is_some();
    let planning_destination = if destination_is_catalog && !scoped_planning {
        crate::commands::sync::SyncDestination::Catalog
    } else {
        crate::commands::sync::SyncDestination::Scheduler
    };
    // What keeps the sync from deleting a project its source deleted. A
    // grade push deletes nothing.
    let deletions = if matches!(req.kind, SchedulerSyncKind::PushGrades) {
        crate::commands::sync::SourceDeletionRules::default()
    } else {
        // A `ts:<slug>` connection is the rig file of catalog `<slug>`,
        // whose projects Planning links: its collaborations protect them.
        let planning_slug = if destination_is_catalog {
            Some(destination_id.as_str())
        } else {
            destination_id.strip_prefix("ts:")
        };
        crate::server::project_removal::sync_deletion_rules(state, planning_slug).await?
    };
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

    let response = tokio::task::spawn_blocking(move || -> anyhow::Result<SyncRun> {
        let source_canon =
            std::fs::canonicalize(&source_path).unwrap_or_else(|_| source_path.clone());
        let destination_canon =
            std::fs::canonicalize(&destination_path).unwrap_or_else(|_| destination_path.clone());
        anyhow::ensure!(
            source_canon != destination_canon,
            "The two configured entries point to the same database file."
        );

        let (source, destination) =
            open_scheduler_sync_connections(&source_path, &destination_path)?;
        let preview_stop = guard_mode.preview_stop().cloned();
        if let Some(stop) = &preview_stop {
            stop.interrupts(&source);
            stop.interrupts(&destination);
        }
        // An interrupted statement fails with SQLite's interrupt error;
        // a preview reports that it stopped instead.
        let stopped = |error: anyhow::Error| match &preview_stop {
            Some(stop) if stop.requested() => anyhow::Error::new(PreviewStopped),
            _ => error,
        };

        let run = |transaction: Option<&Transaction<'_>>| -> anyhow::Result<(
            SchedulerSyncResponse,
            Vec<crate::commands::sync::ChangedAcquiredImage>,
            Vec<String>,
        )> {
            let (response, changed_image_ids, deleted_projects) = match kind {
                SchedulerSyncKind::Pull => {
                    let options = PullOptions {
                        dry_run,
                        with_image_data,
                        project_filter: project.clone(),
                        deletions: deletions.clone(),
                    };
                    let mut summary = match transaction {
                        Some(transaction) => {
                            sync_pull_in_transaction(&source, transaction, &options)?
                        }
                        None => sync_pull(&source, &destination, &options)?,
                    };
                    let changed_images = std::mem::take(&mut summary.changed_acquiredimages);
                    let deleted_projects = std::mem::take(&mut summary.deleted_project_guids);
                    (
                        SchedulerSyncResponse {
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
                        },
                        changed_images,
                        deleted_projects,
                    )
                }
                SchedulerSyncKind::PushPlanning => {
                    let options = PlanningOptions {
                        dry_run,
                        project_filter: project.clone(),
                        destination: planning_destination,
                        deletions: deletions.clone(),
                    };
                    let mut summary = match transaction {
                        Some(transaction) => {
                            sync_planning_in_transaction(&source, transaction, &options)?
                        }
                        None => sync_planning(&source, &destination, &options)?,
                    };
                    let deleted_projects = std::mem::take(&mut summary.deleted_project_guids);
                    (
                        SchedulerSyncResponse {
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
                        },
                        Vec::new(),
                        deleted_projects,
                    )
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
                    (
                        SchedulerSyncResponse {
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
                        },
                        Vec::new(),
                        Vec::new(),
                    )
                }
            };
            Ok((response, changed_image_ids, deleted_projects))
        };

        let fingerprint = |destination: &Connection| {
            if scoped_planning {
                crate::server::sync_preview::planning_destination_fingerprint(&source, destination)
            } else {
                crate::server::sync_preview::connection_fingerprint(
                    destination,
                    &fingerprint_queries,
                )
            }
        };
        match guard_mode {
            SyncGuardMode::None => {
                let (response, changed_images, deleted_projects) = run(None)?;
                Ok(SyncRun {
                    response,
                    fingerprint: None,
                    changed_images,
                    deleted_projects,
                })
            }
            SyncGuardMode::Preview { .. } | SyncGuardMode::PlanningPreview { .. } => {
                // A pull or planning preview writes, then rolls back; a
                // grade preview only reads.
                let behavior = if matches!(kind, SchedulerSyncKind::PushGrades) {
                    TransactionBehavior::Deferred
                } else {
                    TransactionBehavior::Immediate
                };
                let transaction = Transaction::new_unchecked(&destination, behavior)
                    .map_err(|error| stopped(error.into()))?;
                let fingerprint = fingerprint(&transaction).map_err(stopped)?;
                // Dropping the transaction rolls it back, stopped or not.
                let (response, _, _) = run(Some(&transaction)).map_err(stopped)?;
                transaction.rollback()?;
                Ok(SyncRun {
                    response,
                    fingerprint: Some(fingerprint),
                    changed_images: Vec::new(),
                    deleted_projects: Vec::new(),
                })
            }
            SyncGuardMode::Apply {
                destination_fingerprint,
            }
            | SyncGuardMode::PlanningApply {
                destination_fingerprint,
            } => {
                let transaction =
                    Transaction::new_unchecked(&destination, TransactionBehavior::Immediate)?;
                let fingerprint = fingerprint(&transaction)?;
                if fingerprint != destination_fingerprint {
                    return Err(StaleSyncPreview.into());
                }
                let (response, changed_images, deleted_projects) = run(Some(&transaction))?;
                transaction.commit()?;
                Ok(SyncRun {
                    response,
                    fingerprint: None,
                    changed_images,
                    deleted_projects,
                })
            }
        }
    })
    .await
    .map_err(|error| AppError::InternalError(format!("scheduler sync task failed: {error}")))?
    .map_err(|error| {
        classify_scheduler_sync_failure(&error, &source_path_for_error, &destination_path_for_error)
    })?;

    let SyncRun {
        response,
        fingerprint,
        changed_images,
        deleted_projects,
    } = response;
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
    if !dry_run && destination_is_catalog && !deleted_projects.is_empty() {
        // Planning forgets each project the sync deleted, after the commit,
        // as it does after Delete project in the Library.
        crate::server::project_removal::forget_synced_deletions(
            state,
            &destination_id_for_cache,
            &deleted_projects,
        )
        .await;
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

/// What a sync preview does, for the jobs route: `Previewing a pull from
/// Rig`.
fn sync_preview_label(state: &AppState, request: &SchedulerSyncRequest) -> String {
    let peer = state
        .get_database(&request.peer_db_id)
        .map(|peer| peer.name.clone())
        .unwrap_or_else(|| request.peer_db_id.clone());
    match request.kind {
        SchedulerSyncKind::Pull => format!("Previewing a pull from {peer}"),
        SchedulerSyncKind::PushPlanning => format!("Previewing planning sent to {peer}"),
        SchedulerSyncKind::PushGrades => format!("Previewing grades sent to {peer}"),
    }
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
    // A newer preview from this database's page stops this one.
    let run =
        state
            .previews
            .sync
            .start(&state, db_id.clone(), sync_preview_label(&state, &request));
    let replaced = |run: &crate::server::job_line::latest::LatestRun<String>| {
        (!run.is_latest())
            .then(|| AppError::Conflict("A newer preview of this sync replaced this one.".into()))
    };
    let source_path_for_error = source_path.display().to_string();
    let snapshot_state = state.clone();
    let snapshot_stop = run.stop().clone();
    let source_snapshot_file = tokio::task::spawn_blocking(move || {
        snapshot_state
            .sync_previews
            .create_source_snapshot(&source_path, &snapshot_stop)
    })
    .await
    .map_err(|error| AppError::InternalError(format!("sync snapshot task failed: {error}")))?
    .map_err(|error| {
        if error
            .downcast_ref::<crate::server::sync_preview::SnapshotStopped>()
            .is_some()
        {
            replaced(&run).unwrap_or_else(|| {
                AppError::Conflict("This sync preview was stopped before it finished.".into())
            })
        } else {
            classify_sync_snapshot_failure(error, &source_path_for_error)
        }
    })?;
    let source_snapshot_path = state
        .sync_previews
        .source_snapshot_path_for_file(&source_snapshot_file)
        .map_err(|error| AppError::InternalError(format!("{error:#}")))?;
    let preview = execute_scheduler_sync_guarded(
        &state,
        &db_id,
        request.clone(),
        Some(source_snapshot_path),
        SyncGuardMode::Preview {
            stop: run.stop().clone(),
        },
    )
    .await;
    // A replaced preview keeps nothing: the newer one's result is the one
    // the page shows.
    let preview = match replaced(&run) {
        Some(error) => Err(error),
        None => preview,
    };
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
        SyncGuardMode::Preview { stop: Stop::new() },
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
