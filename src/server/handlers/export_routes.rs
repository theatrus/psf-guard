//! Exporting a database's frames: an archive, a local folder, or a
//! server-side export with its progress.

use super::*;

/// `GET /api/db/{db_id}/export` — stream selected non-rejected lights and
/// safely matched raw calibration frames as an uncompressed (store-mode) zip,
/// laid out exactly like the CLI export. FITS doesn't compress, so store mode
/// streams at wire speed with no server-side staging. Read-only, so it is not
/// management-gated.
pub async fn export_archive_route(
    ctx: DbContext,
    Query(query): Query<ExportQuery>,
) -> Result<axum::response::Response, AppError> {
    use crate::commands::export::{plan_export_in, ExportOptions};
    use axum::body::Body;
    use axum::http::header;

    let options = ExportOptions {
        include_pending: query.include_pending,
        project_id: query.project_id,
        target_id: query.target_id,
        filter_name: query.filter_name.clone(),
        layout: query.layout,
        exclude_uncalibrated: crate::calibration::exclude_uncalibrated_enabled(),
        ..Default::default()
    };
    // A referenced download is the runner scripts alone: nothing is copied,
    // the scripts name the frames where they are.
    let placement = query
        .placement
        .unwrap_or(crate::commands::export::Placement::Copy);
    let files = wbpp_files(
        placement,
        query.local_root.clone(),
        query.remote_root.clone(),
        options.layout,
    )?;
    let wbpp_options = query.wbpp_options();

    // Plan on a blocking thread: it queries the DB and reads each light's
    // header. Files resolve through the cached directory tree, not a fresh
    // walk. Use a DEDICATED read-only connection — the header reads take
    // seconds on network storage, and holding the shared request-connection
    // mutex for that long would block every other API call on this DB
    // (same rule as the import job and the background file-check refresh).
    let plan_ctx = ctx.0.clone();
    let plan = tokio::task::spawn_blocking(move || {
        let conn = open_scheduler_connection_with_flags(
            &plan_ctx.database_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
        )
        .map_err(|e| anyhow::anyhow!("opening {}: {e}", plan_ctx.database_path))?;
        let tree = plan_ctx.get_directory_tree()?;
        plan_export_in(&conn, &tree, &options)
    })
    .await
    .map_err(|e| AppError::InternalError(format!("export planning task: {e}")))?
    .map_err(|e| AppError::InternalError(format!("planning export: {e}")))?;

    if plan.items.is_empty() {
        return Err(AppError::BadRequest(format!(
            "nothing to export ({} matching rows had missing files, {} lights can't be \
             calibrated)",
            plan.missing.len(),
            plan.uncalibrated.len()
        )));
    }

    let layout = query.layout;
    let scripts_only = !placement.places_files();
    let filename = format!(
        "psf-guard-{}-{}{}.zip",
        if scripts_only {
            "wbpp-scripts"
        } else {
            "export"
        },
        ctx.id,
        query
            .target_id
            .map(|t| format!("-target{}", t))
            .unwrap_or_default()
    );
    let total_files = if scripts_only { 0 } else { plan.items.len() };
    let db_id = ctx.id.clone();

    // Zip writer feeds one end of a duplex pipe; the response body streams
    // the other. A mid-stream file error truncates the download (logged) —
    // the client sees a corrupt archive rather than a silent partial success.
    let (writer, reader) = tokio::io::duplex(1 << 20);
    tokio::spawn(async move {
        use async_zip::tokio::write::ZipFileWriter;
        use async_zip::{Compression, ZipEntryBuilder};

        let mut zip = ZipFileWriter::with_tokio(writer);
        for item in plan.items.iter().filter(|_| !scripts_only) {
            let entry_name = item.relative_dest.to_string_lossy().replace('\\', "/");
            let bytes = match tokio::fs::read(&item.source).await {
                Ok(bytes) => bytes,
                Err(e) => {
                    tracing::warn!(
                        "📦 export db={}: aborting stream, {} unreadable: {}",
                        db_id,
                        item.source.display(),
                        e
                    );
                    return;
                }
            };
            let entry = ZipEntryBuilder::new(entry_name.into(), Compression::Stored);
            if let Err(e) = zip.write_entry_whole(entry, &bytes).await {
                tracing::warn!("📦 export db={}: zip write failed: {}", db_id, e);
                return;
            }
        }
        // A download has no summary to read, so the lights left out because
        // they can't be calibrated are listed in a note beside the frames.
        if let Some(note) = plan.uncalibrated_note() {
            let entry = ZipEntryBuilder::new(
                crate::commands::export::UNCALIBRATED_NOTE
                    .to_string()
                    .into(),
                Compression::Stored,
            );
            if let Err(e) = zip.write_entry_whole(entry, note.as_bytes()).await {
                tracing::warn!("📦 export db={}: zip write failed: {}", db_id, e);
                return;
            }
        }
        if let Some(note) = plan.flat_note() {
            let entry = ZipEntryBuilder::new(
                crate::commands::export::FLATS_NOTE.to_string().into(),
                Compression::Stored,
            );
            if let Err(e) = zip.write_entry_whole(entry, note.as_bytes()).await {
                tracing::warn!("📦 export db={}: zip write failed: {}", db_id, e);
                return;
            }
        }
        // The runner scripts ride along in the zip, so an export unpacked on
        // the machine PixInsight runs on already knows how to hand itself to
        // WBPP. A zip has no destination path, so nothing here can be
        // inexpressible the way a comma in a local path can be.
        if layout == crate::commands::export::ExportLayout::Wbpp {
            use crate::commands::export::wbpp;
            let spec = wbpp::WbppScriptSpec {
                run: wbpp::WbppRun::default(),
                files,
                options: wbpp_options,
                bpp_main: None,
                stale_frames: Vec::new(),
            };
            let scripts = [
                ("run-wbpp.sh", wbpp::shell_script(&plan, &spec)),
                ("run-wbpp.cmd", wbpp::batch_script(&plan, &spec)),
                (wbpp::JS_RUNNER, wbpp::js_runner(&plan, &spec)),
            ];
            for (name, body) in scripts {
                let entry =
                    ZipEntryBuilder::new(name.into(), Compression::Stored).unix_permissions(0o755);
                if let Err(e) = zip.write_entry_whole(entry, body.as_bytes()).await {
                    tracing::warn!("📦 export db={}: zip write failed: {}", db_id, e);
                    return;
                }
            }
        }
        if let Err(e) = zip.close().await {
            tracing::warn!("📦 export db={}: zip finalize failed: {}", db_id, e);
        } else {
            tracing::info!(
                "📦 export db={}: streamed {} file(s){}",
                db_id,
                total_files,
                if scripts_only {
                    " (runner scripts only)"
                } else {
                    ""
                }
            );
        }
    });

    let body = Body::from_stream(tokio_util::io::ReaderStream::new(reader));
    axum::response::Response::builder()
        .header(header::CONTENT_TYPE, "application/zip")
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{}\"", filename),
        )
        .body(body)
        .map_err(|e| AppError::InternalError(format!("building response: {e}")))
}

/// How the WBPP runner of an export finds its frames, from the placement
/// asked for. A referenced export places nothing, so it is only the runner:
/// that needs the WBPP layout, and the request is refused without it.
pub(super) fn wbpp_files(
    placement: crate::commands::export::Placement,
    local_root: Option<String>,
    remote_root: Option<String>,
    layout: crate::commands::export::ExportLayout,
) -> Result<crate::commands::export::wbpp::WbppFiles, AppError> {
    use crate::commands::export::{wbpp::WbppFiles, ExportLayout, Placement};
    if placement != Placement::Reference {
        return Ok(WbppFiles::Placed);
    }
    if layout != ExportLayout::Wbpp {
        return Err(AppError::BadRequest(
            "Referencing frames in place only produces the WBPP runner, so it needs the \
             WBPP layout."
                .into(),
        ));
    }
    let given = |root: Option<String>| {
        root.map(|root| root.trim().to_string())
            .filter(|root| !root.is_empty())
    };
    Ok(WbppFiles::Referenced {
        local_root: given(local_root).map(std::path::PathBuf::from),
        remote_root: given(remote_root),
    })
}

/// `POST /api/db/{db_id}/export/local` — run the folder export on the
/// server's own filesystem (copy or hardlink), for desktop/Tauri mode where
/// server and user share a machine. Same selection and layout as the CLI
/// `export` command and the zip stream. Management-gated: a remote client
/// could otherwise write files to arbitrary server paths.
pub async fn export_local_route(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Json(req): Json<LocalExportRequest>,
) -> Result<Json<ApiResponse<crate::commands::export::ExportSummary>>, AppError> {
    use crate::commands::export::{execute_plan_with, plan_export_in, ExportOptions, Placement};

    require_database_management_allowed(&state)?;
    let dest = req.dest.trim().to_string();
    if dest.is_empty() {
        return Err(AppError::BadRequest("dest must not be empty".into()));
    }

    let options = ExportOptions {
        include_pending: req.include_pending,
        project_id: req.project_id,
        target_id: req.target_id,
        filter_name: req.filter_name.clone(),
        layout: req.layout,
        exclude_uncalibrated: crate::calibration::exclude_uncalibrated_enabled(),
        ..Default::default()
    };
    // `placement` supersedes the older `link` flag, whose default was to
    // hardlink.
    let placement = req.placement.unwrap_or(if req.link.unwrap_or(true) {
        Placement::Hardlink
    } else {
        Placement::Copy
    });
    let files = wbpp_files(
        placement,
        req.local_root.clone(),
        req.remote_root.clone(),
        options.layout,
    )?;
    let dry_run = req.dry_run;

    // Plan + place on a blocking thread with a dedicated read-only
    // connection (same rule as the zip stream: never hold the shared
    // request-connection mutex through a directory walk).
    let plan_ctx = ctx.0.clone();
    let summary = tokio::task::spawn_blocking(move || {
        let conn = open_scheduler_connection_with_flags(
            &plan_ctx.database_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
        )
        .map_err(|e| anyhow::anyhow!("opening {}: {e}", plan_ctx.database_path))?;
        let tree = plan_ctx.get_directory_tree()?;
        let plan = plan_export_in(&conn, &tree, &options)?;
        let summary = execute_plan_with(
            &plan,
            std::path::Path::new(&dest),
            placement,
            dry_run,
            &mut |_, _| {},
        );
        if options.layout == crate::commands::export::ExportLayout::Wbpp && !dry_run {
            use crate::commands::export::{wbpp, write_wbpp_scripts};
            // With frames placed, a runner that cannot be written is worth
            // logging but never worth failing the export over. A referenced
            // export IS the runner, so there it is the failure.
            let spec = wbpp::WbppScriptSpec {
                run: wbpp::WbppRun::default(),
                files: files.clone(),
                options: req.wbpp.clone().unwrap_or_default(),
                bpp_main: None,
                stale_frames: Vec::new(),
            };
            if let Err(error) = write_wbpp_scripts(&plan, std::path::Path::new(&dest), &spec) {
                if !placement.places_files() {
                    return Err(error);
                }
                tracing::warn!("No WBPP runner script: {error}");
            }
        }
        Ok::<_, anyhow::Error>(summary)
    })
    .await
    .map_err(|e| AppError::InternalError(format!("export task: {e}")))?
    .map_err(|e| AppError::InternalError(format!("export: {e}")))?;

    tracing::info!(
        "📤 Local export db={}: planned={} copied={} linked={} symlinked={} referenced={} skipped={} missing={} uncalibrated={} errors={}{}",
        ctx.id,
        summary.planned,
        summary.copied,
        summary.linked,
        summary.symlinked,
        summary.referenced,
        summary.skipped_existing,
        summary.missing,
        summary.uncalibrated,
        summary.errors,
        if dry_run { " (dry-run)" } else { "" }
    );
    Ok(Json(ApiResponse::success(summary)))
}

/// `POST /api/db/{db_id}/export/server` — run the export into the
/// operator-configured export directory as a background job. Unlike
/// `/export/local`, the caller never names a path: the operator consented
/// to writes here when configuring the directory, so no management grant
/// is needed. Reflinks where the filesystem supports it, copies elsewhere.
pub async fn start_server_export_route(
    ctx: DbContext,
    Json(req): Json<crate::server::api::ServerExportRequest>,
) -> Result<Json<ApiResponse<crate::server::api::ExportStatusResponse>>, AppError> {
    use crate::commands::export::{
        execute_plan_with, plan_export_in, sanitize_component, ExportOptions, Placement,
    };
    use crate::server::export_job;

    let Some(export_root) = ctx.0.export_dir.clone() else {
        return Err(AppError::BadRequest(
            "No export directory is configured for this database. Set one in Settings, \
             or edit the registry's export_dir."
                .into(),
        ));
    };
    let dest = match req.subdirectory.as_deref().map(str::trim) {
        Some(subdirectory) if !subdirectory.is_empty() => {
            export_root.join(sanitize_component(subdirectory))
        }
        _ => export_root,
    };
    let scope = req
        .scope_label
        .clone()
        .filter(|label| !label.trim().is_empty())
        .unwrap_or_else(|| "selection".into());

    let options = ExportOptions {
        include_pending: req.include_pending,
        project_id: req.project_id,
        target_id: req.target_id,
        filter_name: req.filter_name.clone(),
        layout: req.layout,
        exclude_uncalibrated: crate::calibration::exclude_uncalibrated_enabled(),
        ..Default::default()
    };
    let placement = req.placement.unwrap_or(Placement::Reflink);
    let files = wbpp_files(
        placement,
        req.local_root.clone(),
        req.remote_root.clone(),
        options.layout,
    )?;

    let wbpp_options = req.wbpp.clone().unwrap_or_default();
    let store = ctx.0.export_job.clone();
    if !export_job::try_begin(&store, dest.display().to_string(), scope) {
        return Ok(Json(ApiResponse::success(
            crate::server::api::ExportStatusResponse {
                started: false,
                progress: export_job::progress_snapshot(&store),
            },
        )));
    }

    let job_ctx = ctx.0.clone();
    let job_store = store.clone();
    tokio::task::spawn_blocking(move || {
        let run = || -> anyhow::Result<()> {
            let conn = open_scheduler_connection_with_flags(
                &job_ctx.database_path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
            )
            .map_err(|e| anyhow::anyhow!("opening {}: {e}", job_ctx.database_path))?;
            let tree = job_ctx.get_directory_tree()?;
            let plan = plan_export_in(&conn, &tree, &options)?;
            std::fs::create_dir_all(&dest)
                .map_err(|e| anyhow::anyhow!("creating {}: {e}", dest.display()))?;
            export_job::set_stage(&job_store, "placing");
            export_job::set_placement_totals(&job_store, plan.items.len(), 0);
            let summary =
                execute_plan_with(&plan, &dest, placement, false, &mut |placed, total| {
                    export_job::set_placement_totals(&job_store, total, placed);
                });
            if options.layout == crate::commands::export::ExportLayout::Wbpp {
                use crate::commands::export::{wbpp, write_wbpp_scripts};
                export_job::set_stage(&job_store, "scripts");
                // With frames placed, a runner that cannot be written is
                // worth logging but never worth failing over. A referenced
                // export IS the runner, so there it is the failure.
                let spec = wbpp::WbppScriptSpec {
                    run: wbpp::WbppRun::default(),
                    files: files.clone(),
                    options: wbpp_options.clone(),
                    bpp_main: None,
                    stale_frames: Vec::new(),
                };
                if let Err(error) = write_wbpp_scripts(&plan, &dest, &spec) {
                    if !placement.places_files() {
                        return Err(error);
                    }
                    tracing::warn!("No WBPP runner script: {error}");
                }
            }
            tracing::info!(
                "📤 Server export db={}: planned={} reflinked={} copied={} symlinked={} referenced={} skipped={} missing={} uncalibrated={} errors={} -> {}",
                job_ctx.id,
                summary.planned,
                summary.reflinked,
                summary.copied,
                summary.symlinked,
                summary.referenced,
                summary.skipped_existing,
                summary.missing,
                summary.uncalibrated,
                summary.errors,
                dest.display(),
            );
            export_job::complete_export(&job_store, summary);
            Ok(())
        };
        if let Err(error) = run() {
            tracing::warn!("Server export failed: {error:#}");
            export_job::finish(&job_store, Some(format!("{error:#}")));
        }
    });

    Ok(Json(ApiResponse::success(
        crate::server::api::ExportStatusResponse {
            started: true,
            progress: export_job::progress_snapshot(&store),
        },
    )))
}

/// `GET /api/db/{db_id}/export/server` — progress of the export job.
pub async fn get_server_export_progress(
    ctx: DbContext,
) -> Result<Json<ApiResponse<crate::server::api::ExportStatusResponse>>, AppError> {
    let progress = crate::server::export_job::progress_snapshot(&ctx.0.export_job);
    Ok(Json(ApiResponse::success(
        crate::server::api::ExportStatusResponse {
            started: progress.running,
            progress,
        },
    )))
}

#[derive(Debug, Deserialize)]
pub struct CalibrationReportQuery {
    /// Count rejected lights too; the report leaves them out by default.
    #[serde(default)]
    pub include_rejected: bool,
    /// The view's night split, `rig` or `catalog`, over the `light_nights`
    /// setting, so the report names the nights the view groups by.
    #[serde(default)]
    pub nights: Option<String>,
}

/// A view's night split from its link, over the `light_nights` setting.
#[derive(Debug, Default, Deserialize)]
pub struct NightsQuery {
    #[serde(default)]
    pub nights: Option<String>,
}
