//! Token-authenticated database sync for remote N.I.N.A. clients.
//!
//! The wire format is deliberately database-shaped: a client freezes selected
//! Target Scheduler tables into a JSON bundle, while PSF Guard materializes
//! that bundle as a temporary SQLite source and delegates every merge
//! decision to the existing local database sync engine.
//!
//! `payload_sha256` is a courtesy checksum, not a credential. It carries no
//! key, so it proves nothing about who built the bundle — the bearer token
//! does that, and TLS plus `Content-Length` already cover truncation. We
//! therefore accept it, echo it on export, and do not verify it: enforcing it
//! would mean pinning a canonical JSON encoding, so reordering one struct
//! field would reject every plugin already in the field.

use axum::{
    extract::{Path, State},
    http::{header::AUTHORIZATION, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use base64::Engine as _;
use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{
    params_from_iter,
    types::{Value, ValueRef},
    Connection, OpenFlags,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap},
    path::{Path as FsPath, PathBuf},
    sync::{Arc, Mutex},
};
use uuid::Uuid;

use crate::server::{
    api::{
        ApiRefreshStatus, ApiResponse, SchedulerSyncKind, SchedulerSyncRequest,
        SchedulerSyncResponse, SchedulerSyncTableCounts,
    },
    database_context::DatabaseContext,
    handlers::{execute_scheduler_sync_paths, AppError, SyncGuardMode},
    remote_audit::{AuditAction, AuditOutcome, AuditRecord},
    state::AppState,
};

pub const MAX_SYNC_BODY_BYTES: usize = 512 * 1024 * 1024;
const MAX_BUNDLE_ROWS: usize = 1_000_000;
const PROTOCOL_VERSION: u32 = 1;
/// How long a built export stays fetchable, matching the preview lifetime.
const EXPORT_LIFETIME_SECS: i64 = 30 * 60;
const MAX_RETAINED_EXPORTS: usize = 16;
const MAX_RETAINED_PREVIEW_JOBS: usize = 32;
/// Stands in for the catalog in an audit entry written before the token
/// identified one — a bad token names no database.
const UNKNOWN_CATALOG: &str = "-";
const PLANNING_TABLES: &[&str] = &[
    "exposuretemplate",
    "project",
    "ruleweight",
    "target",
    "exposureplan",
];
const MERGE_TABLES: &[&str] = &[
    "exposuretemplate",
    "project",
    "ruleweight",
    "target",
    "exposureplan",
    "acquiredimage",
    "imagedata",
];
/// A grade push reads its source rows through the scheduler's own
/// project/target join, so the bundle has to carry those two tables even
/// though nothing in them is ever written. Without them the materialized
/// source is not a Target Scheduler database and the read fails outright.
const GRADE_TABLES: &[&str] = &["project", "target", "acquiredimage"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncOperation {
    Merge,
    PushPlanning,
    PushGrades,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogIdentity {
    pub id: String,
    pub product: String,
    pub product_version: String,
    pub schema_version: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleColumn {
    pub name: String,
    pub declared_type: String,
    pub not_null: bool,
    pub primary_key: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleRow {
    pub values: Vec<WireValue>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BundleTable {
    pub columns: Vec<BundleColumn>,
    pub rows: Vec<BundleRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireValueKind {
    Null,
    Integer,
    Real,
    Text,
    Blob,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireValue {
    pub kind: WireValueKind,
    pub value: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogBundle {
    pub protocol_version: u32,
    pub bundle_id: String,
    pub created_at_utc: String,
    pub operation: SyncOperation,
    pub source: CatalogIdentity,
    pub tables: BTreeMap<String, BundleTable>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload_sha256: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreatePreviewRequest {
    pub protocol_version: u32,
    pub catalog_id: String,
    pub operation: SyncOperation,
    pub bundle: CatalogBundle,
}

#[derive(Debug, Deserialize)]
pub struct CreateExportRequest {
    pub protocol_version: u32,
    pub catalog_id: String,
    pub operation: SyncOperation,
    #[serde(default = "default_true")]
    pub reviewed_only: bool,
    /// Merge exports only: include the `imagedata` thumbnail BLOBs. Off by
    /// default — thumbnails dominate a catalog's size (a hundred-plus
    /// megabytes on a season of captures), and a client that wants them
    /// must ask.
    #[serde(default)]
    pub include_thumbnails: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Serialize)]
pub struct SyncCatalogCapability {
    pub id: String,
    pub name: String,
    pub readable: bool,
    pub writable: bool,
}

#[derive(Debug, Serialize)]
pub struct SyncCapabilities {
    pub protocol_version: u32,
    pub product: &'static str,
    pub product_version: &'static str,
    pub capabilities: Vec<&'static str>,
    pub catalogs: Vec<SyncCatalogCapability>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SyncPreview {
    pub preview_id: String,
    pub state: &'static str,
    pub expires_at: String,
    pub summary: BTreeMap<String, i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PreviewJob {
    pub job_id: String,
    pub state: &'static str,
    /// Where a running job currently is: "materializing" (writing the
    /// bundle snapshot), then "comparing" (dry-run against the catalog).
    /// Ready and failed jobs carry no phase.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<&'static str>,
    pub preview: Option<SyncPreview>,
    pub error: Option<String>,
}

struct StoredPreviewJob {
    catalog_id: String,
    created_at: i64,
    sequence: u64,
    job: PreviewJob,
}

#[derive(Default)]
pub struct PreviewJobStore {
    entries: Mutex<HashMap<String, StoredPreviewJob>>,
    next_sequence: std::sync::atomic::AtomicU64,
}

impl PreviewJobStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn start(
        &self,
        catalog_id: String,
        requested_id: Option<String>,
    ) -> Result<(PreviewJob, bool), AppError> {
        let now = unix_seconds();
        let sequence = self
            .next_sequence
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut entries = self.lock()?;
        entries.retain(|_, stored| stored.created_at + EXPORT_LIFETIME_SECS > now);
        if let Some(existing) = requested_id.as_ref().and_then(|id| entries.get(id))
            && existing.catalog_id == catalog_id
        {
            return Ok((existing.job.clone(), false));
        }
        while entries.len() >= MAX_RETAINED_PREVIEW_JOBS {
            let Some(oldest) = entries
                .iter()
                .min_by_key(|(_, stored)| stored.sequence)
                .map(|(id, _)| id.clone())
            else {
                break;
            };
            entries.remove(&oldest);
        }
        let job = PreviewJob {
            job_id: requested_id.unwrap_or_else(|| Uuid::new_v4().to_string()),
            state: "running",
            phase: Some("materializing"),
            preview: None,
            error: None,
        };
        entries.insert(
            job.job_id.clone(),
            StoredPreviewJob {
                catalog_id,
                created_at: now,
                sequence,
                job: job.clone(),
            },
        );
        Ok((job, true))
    }

    fn finish(&self, job_id: &str, result: Result<SyncPreview, AppError>) {
        let Ok(mut entries) = self.entries.lock() else {
            tracing::error!("remote preview job lock poisoned while finishing {job_id}");
            return;
        };
        let Some(stored) = entries.get_mut(job_id) else {
            return;
        };
        stored.job.phase = None;
        match result {
            Ok(preview) => {
                stored.job.state = "ready";
                stored.job.preview = Some(preview);
            }
            Err(error) => {
                stored.job.state = "failed";
                stored.job.error = Some(detail_of(&error));
            }
        }
    }

    /// Record where a running job is. Missing or finished jobs are ignored.
    fn set_phase(&self, job_id: &str, phase: &'static str) {
        let Ok(mut entries) = self.entries.lock() else {
            return;
        };
        if let Some(stored) = entries.get_mut(job_id)
            && stored.job.state == "running"
        {
            stored.job.phase = Some(phase);
        }
    }

    fn get(&self, id: &str, catalog_id: &str) -> Result<Option<PreviewJob>, AppError> {
        let now = unix_seconds();
        let mut entries = self.lock()?;
        let Some(stored) = entries.get(id) else {
            return Ok(None);
        };
        if stored.created_at + EXPORT_LIFETIME_SECS <= now {
            entries.remove(id);
            return Ok(None);
        }
        if stored.catalog_id != catalog_id {
            return Ok(None);
        }
        Ok(Some(stored.job.clone()))
    }

    fn lock(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, HashMap<String, StoredPreviewJob>>, AppError> {
        self.entries.lock().map_err(|error| {
            AppError::InternalError(format!("remote preview job lock poisoned: {error}"))
        })
    }
}

#[derive(Debug, Serialize)]
pub struct SyncApplyResult {
    pub state: &'static str,
    pub summary: BTreeMap<String, i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SyncExport {
    pub export_id: String,
    pub state: &'static str,
    pub bundle: Option<CatalogBundle>,
    pub error: Option<String>,
}

struct StoredExport {
    catalog_id: String,
    created_at: i64,
    /// Insertion order. `created_at` is whole seconds, so a burst of exports
    /// all carry the same one and cannot be ranked by it — ordering on the
    /// timestamp alone would evict by UUID, which is to say at random.
    sequence: u64,
    export: SyncExport,
}

/// Built export bundles, held so a client can re-fetch one it lost. Bundles
/// carry whole tables, so this is capacity-bound and time-bound: expired
/// entries go first, then the least recently built.
#[derive(Default)]
pub struct ExportStore {
    entries: Mutex<HashMap<String, StoredExport>>,
    next_sequence: std::sync::atomic::AtomicU64,
}

impl ExportStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn insert(&self, catalog_id: String, export: SyncExport) -> Result<(), AppError> {
        let now = unix_seconds();
        let sequence = self
            .next_sequence
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut entries = self.lock()?;
        entries.retain(|_, stored| stored.created_at + EXPORT_LIFETIME_SECS > now);
        while entries.len() >= MAX_RETAINED_EXPORTS {
            let Some(oldest) = entries
                .iter()
                .min_by_key(|(_, stored)| stored.sequence)
                .map(|(id, _)| id.clone())
            else {
                break;
            };
            entries.remove(&oldest);
        }
        entries.insert(
            export.export_id.clone(),
            StoredExport {
                catalog_id,
                created_at: now,
                sequence,
                export,
            },
        );
        Ok(())
    }

    fn get(&self, id: &str, catalog_id: &str) -> Result<Option<SyncExport>, AppError> {
        let now = unix_seconds();
        let mut entries = self.lock()?;
        let Some(stored) = entries.get(id) else {
            return Ok(None);
        };
        if stored.created_at + EXPORT_LIFETIME_SECS <= now {
            entries.remove(id);
            return Ok(None);
        }
        if stored.catalog_id != catalog_id {
            return Ok(None);
        }
        Ok(Some(stored.export.clone()))
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, HashMap<String, StoredExport>>, AppError> {
        self.entries.lock().map_err(|error| {
            AppError::InternalError(format!("remote export lock poisoned: {error}"))
        })
    }
}

pub async fn capabilities(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<ApiResponse<SyncCapabilities>>, AppError> {
    let catalog = authenticated_catalog(&state, &headers, AuditAction::Capabilities)?;
    let mut capabilities = vec![
        "merge",
        "push_planning",
        "push_grades",
        "preview_apply",
        "preview_refresh",
        "async_preview_jobs",
        "exports",
    ];
    if catalog
        .remote_image_upload
        .as_ref()
        .is_some_and(|config| config.enabled)
        && catalog.remote_image_upload_dir.is_some()
    {
        capabilities.push("image_upload");
    }
    Ok(Json(ApiResponse::success(SyncCapabilities {
        protocol_version: PROTOCOL_VERSION,
        product: "PSF Guard",
        product_version: env!("CARGO_PKG_VERSION"),
        capabilities,
        catalogs: vec![SyncCatalogCapability {
            id: catalog.id.clone(),
            name: catalog.name.clone(),
            readable: true,
            writable: true,
        }],
    })))
}

pub async fn create_preview(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<CreatePreviewRequest>,
) -> Result<Response, AppError> {
    let catalog = authenticated_catalog(&state, &headers, AuditAction::Preview)?;
    let respond_async = headers
        .get("prefer")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(',')
                .any(|preference| preference.trim().eq_ignore_ascii_case("respond-async"))
        });
    if respond_async {
        let idempotency_key = headers
            .get("idempotency-key")
            .and_then(|value| value.to_str().ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| digest_hex(format!("{}:{value}", catalog.id).as_bytes()));
        let (job, started) = state
            .remote_preview_jobs
            .start(catalog.id.clone(), idempotency_key)?;
        if !started {
            return Ok((
                StatusCode::ACCEPTED,
                Json(ApiResponse::success_with_status(
                    job,
                    ApiRefreshStatus::Loading,
                )),
            )
                .into_response());
        }
        let job_id = job.job_id.clone();
        let background_state = Arc::clone(&state);
        tokio::spawn(async move {
            let result = create_preview_inner(
                Arc::clone(&background_state),
                catalog,
                request,
                Some(job_id.clone()),
            )
            .await;
            background_state.remote_preview_jobs.finish(&job_id, result);
        });
        return Ok((
            StatusCode::ACCEPTED,
            Json(ApiResponse::success_with_status(
                job,
                ApiRefreshStatus::Loading,
            )),
        )
            .into_response());
    }

    let preview = create_preview_inner(state, catalog, request, None).await?;
    Ok(Json(ApiResponse::success(preview)).into_response())
}

async fn create_preview_inner(
    state: Arc<AppState>,
    catalog: Arc<DatabaseContext>,
    request: CreatePreviewRequest,
    job_id: Option<String>,
) -> Result<SyncPreview, AppError> {
    let started = std::time::Instant::now();
    let operation = request.operation;
    let row_count: usize = request
        .bundle
        .tables
        .values()
        .map(|table| table.rows.len())
        .sum();
    // Copy the identifiers the audit log needs before the bundle moves, so
    // auditing never forces the whole payload to be cloned.
    let bundle_id = request.bundle.bundle_id.clone();
    let source_id = request.bundle.source.id.clone();
    let audit = |outcome,
                 detail: Option<&str>,
                 preview_id: Option<&str>,
                 summary: BTreeMap<String, i64>| {
        state.remote_audit.record(
            &catalog.id,
            AuditAction::Preview,
            outcome,
            AuditRecord {
                operation: Some(operation_label(operation)),
                source_id: Some(&source_id),
                bundle_id: Some(&bundle_id),
                preview_id,
                detail,
                summary,
            },
        );
    };
    let refuse = |message: String| {
        audit(AuditOutcome::Refused, Some(&message), None, BTreeMap::new());
        AppError::BadRequest(message)
    };
    require_protocol(request.protocol_version)?;
    require_catalog(&catalog, &request.catalog_id)?;
    if operation != request.bundle.operation {
        return Err(refuse(
            "request operation does not match bundle operation".into(),
        ));
    }
    if let Err(error) = validate_bundle(&request.bundle) {
        return Err(match error {
            AppError::BadRequest(message) => refuse(message),
            other => other,
        });
    }

    let destination_path = PathBuf::from(&catalog.database_path);
    let snapshot_file = state
        .sync_previews
        .create_empty_source_snapshot()
        .map_err(internal)?;
    let snapshot_path = state
        .sync_previews
        .source_snapshot_path_for_file(&snapshot_file)
        .map_err(internal)?;
    let materialize_path = snapshot_path.clone();
    let template_path = destination_path.clone();
    let bundle = request.bundle;
    if let Err(error) = tokio::task::spawn_blocking(move || {
        materialize_bundle(&materialize_path, &template_path, &bundle)
    })
    .await
    .map_err(|error| AppError::InternalError(format!("bundle task failed: {error}")))?
    {
        state.sync_previews.remove_source_snapshot(&snapshot_file);
        return Err(refuse(format!("invalid catalog bundle: {error:#}")));
    }
    let materialize_duration = started.elapsed();
    tracing::info!(
        "remote sync preview materialized catalog={} bundle={bundle_id}          operation={} rows={row_count} in {materialize_duration:.2?}",
        catalog.id,
        operation_label(operation),
    );
    if let Some(job_id) = job_id.as_deref() {
        state.remote_preview_jobs.set_phase(job_id, "comparing");
    }

    let sync_request = scheduler_request(operation, source_id.clone(), true);
    let execution = execute_scheduler_sync_paths(
        &state,
        snapshot_path,
        destination_path,
        source_id.clone(),
        catalog.id.clone(),
        sync_request.clone(),
        SyncGuardMode::Preview,
    )
    .await;
    let (result, fingerprint) = match execution {
        Ok((result, Some(fingerprint))) => (result, fingerprint),
        Ok(_) => unreachable!("preview execution returns a fingerprint"),
        Err(error) => {
            state.sync_previews.remove_source_snapshot(&snapshot_file);
            audit(
                outcome_of(&error),
                Some(&detail_of(&error)),
                None,
                BTreeMap::new(),
            );
            return Err(error);
        }
    };
    let record = state
        .sync_previews
        .store(
            catalog.id.clone(),
            sync_request,
            snapshot_file.clone(),
            fingerprint,
            result.clone(),
        )
        .map_err(|error| {
            state.sync_previews.remove_source_snapshot(&snapshot_file);
            internal(error)
        })?;
    let summary = result_summary(&result);
    audit(AuditOutcome::Ok, None, Some(&record.id), summary.clone());
    tracing::info!(
        "remote sync preview ready catalog={} bundle={bundle_id} operation={}          rows={row_count} compare={:.2?} total={:.2?} preview={}",
        catalog.id,
        operation_label(operation),
        started.elapsed().saturating_sub(materialize_duration),
        started.elapsed(),
        record.id,
    );
    Ok(SyncPreview {
        preview_id: record.id,
        state: "ready",
        expires_at: unix_timestamp(record.expires_at),
        summary,
    })
}

pub async fn get_preview_job(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(job_id): Path<String>,
) -> Result<Json<ApiResponse<PreviewJob>>, AppError> {
    let catalog = authenticated_catalog(&state, &headers, AuditAction::Preview)?;
    let job = state
        .remote_preview_jobs
        .get(&job_id, &catalog.id)?
        .ok_or(AppError::NotFound)?;
    Ok(Json(ApiResponse::success(job)))
}

pub async fn get_preview(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(preview_id): Path<String>,
) -> Result<Json<ApiResponse<SyncPreview>>, AppError> {
    let catalog = authenticated_catalog(&state, &headers, AuditAction::Preview)?;
    let record = state
        .sync_previews
        .get(&preview_id)
        .map_err(internal)?
        .filter(|record| record.local_db_id == catalog.id)
        .ok_or(AppError::NotFound)?;
    Ok(Json(ApiResponse::success(SyncPreview {
        preview_id: record.id,
        state: "ready",
        expires_at: unix_timestamp(record.expires_at),
        summary: result_summary(&record.result),
    })))
}

pub async fn apply_preview(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(preview_id): Path<String>,
) -> Result<Json<ApiResponse<SyncApplyResult>>, AppError> {
    let catalog = authenticated_catalog(&state, &headers, AuditAction::Apply)?;
    let audit = |outcome, operation, detail: Option<&str>, summary| {
        state.remote_audit.record(
            &catalog.id,
            AuditAction::Apply,
            outcome,
            AuditRecord {
                operation,
                preview_id: Some(&preview_id),
                detail,
                summary,
                ..Default::default()
            },
        );
    };
    let _apply_guard = state.sync_apply_lock.lock().await;
    let Some(record) = state
        .sync_previews
        .claim(&preview_id, &catalog.id)
        .map_err(internal)?
    else {
        audit(
            AuditOutcome::Refused,
            None,
            Some("no such preview"),
            BTreeMap::new(),
        );
        return Err(unknown_preview());
    };
    let operation = Some(kind_label(record.request.kind));
    let source_path = state
        .sync_previews
        .source_snapshot_path(&record)
        .map_err(internal)?;
    let mut request = record.request.clone();
    request.dry_run = false;
    let result = execute_scheduler_sync_paths(
        &state,
        source_path,
        PathBuf::from(&catalog.database_path),
        request.peer_db_id.clone(),
        catalog.id.clone(),
        request,
        SyncGuardMode::Apply {
            destination_fingerprint: record.destination_fingerprint.clone(),
        },
    )
    .await
    .map(|(result, _)| result);

    let result = match result {
        Ok(result) => result,
        Err(error) => {
            // The apply wrote nothing, so the preview is still good source
            // data. Keep it: for a remote client, discarding it here means
            // re-uploading the whole bundle to get back to this point. A stale
            // destination is fixed by refreshing this same preview.
            if let Err(restore_error) = state.sync_previews.restore(&record) {
                tracing::warn!("could not restore sync preview {preview_id}: {restore_error:#}");
            }
            audit(
                outcome_of(&error),
                operation,
                Some(&detail_of(&error)),
                BTreeMap::new(),
            );
            return Err(error);
        }
    };
    state
        .sync_previews
        .remove_source_snapshot(&record.source_snapshot_file);
    let summary = result_summary(&result);
    audit(AuditOutcome::Ok, operation, None, summary.clone());
    Ok(Json(ApiResponse::success(SyncApplyResult {
        state: "applied",
        summary,
    })))
}

/// Re-run a kept preview against the destination as it now stands.
///
/// The path back from a stale-preview conflict. Apply refuses when the
/// destination moved under a preview, and the client must review the merge
/// again — but the source data is already on the server, so make it re-review
/// that, not re-upload it.
pub async fn refresh_preview(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(preview_id): Path<String>,
) -> Result<Json<ApiResponse<SyncPreview>>, AppError> {
    let catalog = authenticated_catalog(&state, &headers, AuditAction::PreviewRefresh)?;
    let audit = |outcome, operation, detail: Option<&str>, summary| {
        state.remote_audit.record(
            &catalog.id,
            AuditAction::PreviewRefresh,
            outcome,
            AuditRecord {
                operation,
                preview_id: Some(&preview_id),
                detail,
                summary,
                ..Default::default()
            },
        );
    };
    // Refresh rewrites a record an apply may be claiming, so it takes the same
    // lock, and takes it before reading. Creating a preview needs none of this
    // — that record is new and nothing else can be holding it.
    let _apply_guard = state.sync_apply_lock.lock().await;
    let Some(record) = state
        .sync_previews
        .get(&preview_id)
        .map_err(internal)?
        .filter(|record| record.local_db_id == catalog.id)
    else {
        audit(
            AuditOutcome::Refused,
            None,
            Some("no such preview"),
            BTreeMap::new(),
        );
        return Err(unknown_preview());
    };
    let operation = Some(kind_label(record.request.kind));
    let source_path = state
        .sync_previews
        .source_snapshot_path(&record)
        .map_err(internal)?;
    let mut request = record.request.clone();
    request.dry_run = true;
    let execution = execute_scheduler_sync_paths(
        &state,
        source_path,
        PathBuf::from(&catalog.database_path),
        request.peer_db_id.clone(),
        catalog.id.clone(),
        request,
        SyncGuardMode::Preview,
    )
    .await;
    let (result, fingerprint) = match execution {
        Ok((result, Some(fingerprint))) => (result, fingerprint),
        Ok(_) => unreachable!("preview execution returns a fingerprint"),
        Err(error) => {
            audit(
                outcome_of(&error),
                operation,
                Some(&detail_of(&error)),
                BTreeMap::new(),
            );
            return Err(error);
        }
    };
    let refreshed = state
        .sync_previews
        .refresh(&record, fingerprint, result.clone())
        .map_err(internal)?;
    let summary = result_summary(&result);
    audit(AuditOutcome::Ok, operation, None, summary.clone());
    Ok(Json(ApiResponse::success(SyncPreview {
        preview_id: refreshed.id,
        state: "ready",
        expires_at: unix_timestamp(refreshed.expires_at),
        summary,
    })))
}

/// Response header carrying the SHA-256 of the exact response body bytes.
///
/// The in-bundle `payload_sha256` is a courtesy field a client can only
/// re-check by reproducing this server's serialization byte for byte, which
/// no other JSON writer does. A client that wants integrity should hash the
/// raw body it received and compare it against this header — the same
/// contract the upload path already uses for `X-Content-SHA256` requests.
const CONTENT_SHA256_HEADER: &str = "x-content-sha256";

/// Serialize an export envelope once and stamp the digest of those exact
/// bytes on the response, so a client can verify without re-serializing.
fn export_response(export: SyncExport) -> Result<Response, AppError> {
    let body = serde_json::to_vec(&ApiResponse::success(export))
        .map_err(|error| AppError::InternalError(format!("serializing export: {error}")))?;
    let digest = digest_hex(&body);
    Response::builder()
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .header(CONTENT_SHA256_HEADER, digest)
        .body(axum::body::Body::from(body))
        .map_err(|error| AppError::InternalError(format!("building export response: {error}")))
}

pub async fn create_export(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<CreateExportRequest>,
) -> Result<Response, AppError> {
    let catalog = authenticated_catalog(&state, &headers, AuditAction::Export)?;
    require_protocol(request.protocol_version)?;
    require_catalog(&catalog, &request.catalog_id)?;
    let database_path = PathBuf::from(&catalog.database_path);
    let catalog_id = catalog.id.clone();
    let operation = request.operation;
    let reviewed_only = request.reviewed_only;
    let include_thumbnails = request.include_thumbnails;
    let audit = |outcome, detail: Option<&str>, summary| {
        state.remote_audit.record(
            &catalog.id,
            AuditAction::Export,
            outcome,
            AuditRecord {
                operation: Some(operation_label(operation)),
                detail,
                summary,
                ..Default::default()
            },
        );
    };
    let bundle = match tokio::task::spawn_blocking(move || {
        export_bundle(
            &database_path,
            &catalog_id,
            operation,
            reviewed_only,
            include_thumbnails,
        )
    })
    .await
    .map_err(|error| AppError::InternalError(format!("export task failed: {error}")))?
    {
        Ok(bundle) => bundle,
        Err(error) => {
            let message = format!("creating export: {error:#}");
            audit(AuditOutcome::Failed, Some(&message), BTreeMap::new());
            return Err(AppError::BadRequest(message));
        }
    };
    let summary = bundle
        .tables
        .iter()
        .map(|(name, table)| (format!("{name}_rows"), table.rows.len() as i64))
        .collect();
    let export = SyncExport {
        export_id: Uuid::new_v4().to_string(),
        state: "ready",
        bundle: Some(bundle),
        error: None,
    };
    state
        .remote_exports
        .insert(catalog.id.clone(), export.clone())?;
    audit(AuditOutcome::Ok, Some(&export.export_id), summary);
    export_response(export)
}

pub async fn get_export(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(export_id): Path<String>,
) -> Result<Response, AppError> {
    let catalog = authenticated_catalog(&state, &headers, AuditAction::Export)?;
    let export = state
        .remote_exports
        .get(&export_id, &catalog.id)?
        .ok_or_else(|| {
            AppError::NotFoundMessage(
                "no such export. Exports are kept for 30 minutes and capped, so \
                 an older one may have been dropped — create a new export."
                    .into(),
            )
        })?;
    export_response(export)
}

fn authenticated_catalog(
    state: &AppState,
    headers: &HeaderMap,
    action: AuditAction,
) -> Result<Arc<DatabaseContext>, AppError> {
    // A caller who fails here has no catalog to attribute the attempt to, but
    // the attempt is exactly what an operator wants to see: a run of these is
    // what a guessed or stolen token looks like from the server side.
    let refuse = |catalog_id: &str, message: &str| {
        state.remote_audit.record(
            catalog_id,
            action,
            AuditOutcome::Refused,
            AuditRecord {
                detail: Some(message),
                ..Default::default()
            },
        );
        AppError::Forbidden(message.to_string())
    };
    let token = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|header| header.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .ok_or_else(|| refuse(UNKNOWN_CATALOG, "Bearer token required"))?;
    let mut matches = state.all_databases().into_iter().filter(|catalog| {
        catalog
            .remote_image_upload
            .as_ref()
            .is_some_and(|config| config.token_is_configured() && config.token_matches(token))
    });
    let catalog = matches
        .next()
        .ok_or_else(|| refuse(UNKNOWN_CATALOG, "Invalid API token"))?;
    if matches.next().is_some() {
        return Err(refuse(
            &catalog.id,
            "API token is configured for more than one database",
        ));
    }
    // Sync is a separate grant from image upload. A key configured before
    // this protocol existed authenticates, but reaches nothing until the
    // operator opts the database in.
    if !catalog
        .remote_image_upload
        .as_ref()
        .is_some_and(|config| config.sync_enabled)
    {
        return Err(refuse(
            &catalog.id,
            "remote scheduler sync is disabled for this database",
        ));
    }
    Ok(catalog)
}

fn require_protocol(version: u32) -> Result<(), AppError> {
    if version == PROTOCOL_VERSION {
        Ok(())
    } else {
        Err(AppError::BadRequest(format!(
            "unsupported sync protocol version {version}"
        )))
    }
}

fn require_catalog(catalog: &DatabaseContext, requested: &str) -> Result<(), AppError> {
    if catalog.id == requested {
        Ok(())
    } else {
        Err(AppError::Forbidden(
            "API token does not grant access to the requested catalog".into(),
        ))
    }
}

fn validate_bundle(bundle: &CatalogBundle) -> Result<(), AppError> {
    validate_bundle_within(bundle, MAX_BUNDLE_ROWS)
}

/// The row budget is a parameter so a test can exercise the limit without
/// building a million rows.
fn validate_bundle_within(bundle: &CatalogBundle, max_rows: usize) -> Result<(), AppError> {
    require_protocol(bundle.protocol_version)?;
    Uuid::parse_str(&bundle.bundle_id)
        .map_err(|_| AppError::BadRequest("bundle_id must be a UUID".into()))?;
    DateTime::parse_from_rfc3339(&bundle.created_at_utc)
        .map_err(|_| AppError::BadRequest("created_at_utc must be RFC 3339".into()))?;
    if bundle.source.schema_version < 22 {
        return Err(AppError::BadRequest(
            "Target Scheduler schema 22 or newer is required".into(),
        ));
    }
    for required in required_tables(bundle.operation) {
        if !bundle.tables.contains_key(*required) {
            return Err(AppError::BadRequest(format!(
                "bundle is missing the {required} table, which this operation needs"
            )));
        }
    }

    let allowed = allowed_tables(bundle.operation);
    let mut row_count = 0usize;
    for (name, table) in &bundle.tables {
        if !allowed.contains(&name.as_str()) || !valid_identifier(name) {
            return Err(AppError::BadRequest(format!(
                "table {name} is not syncable for this operation"
            )));
        }
        if table.columns.is_empty()
            || table.columns.len() > 256
            || table
                .columns
                .iter()
                .any(|column| !valid_identifier(&column.name))
        {
            return Err(AppError::BadRequest(format!(
                "table {name} has invalid columns"
            )));
        }
        for row in &table.rows {
            if row.values.len() != table.columns.len() {
                return Err(AppError::BadRequest(format!(
                    "table {name} contains a row with the wrong value count"
                )));
            }
        }
        row_count = row_count.saturating_add(table.rows.len());
    }
    if row_count > max_rows {
        return Err(AppError::BadRequest(format!(
            "bundle exceeds the {max_rows} row limit"
        )));
    }
    Ok(())
}

pub(crate) fn materialize_bundle(
    path: &FsPath,
    template_path: &FsPath,
    bundle: &CatalogBundle,
) -> anyhow::Result<()> {
    let mut connection = Connection::open(path)?;
    // The snapshot is scratch state: on a crash it is discarded and the
    // client re-uploads, so buying durability with fsyncs is pure waste.
    connection.execute_batch(
        "PRAGMA journal_mode=DELETE; PRAGMA foreign_keys=OFF; PRAGMA synchronous=OFF;",
    )?;
    // One transaction for the whole snapshot. Row inserts outside a
    // transaction each autocommit — a large grades bundle meant thousands
    // of journal round-trips — and a failure mid-bundle left a partial
    // snapshot file behind. Now it is one commit, and an error rolls the
    // whole materialization back to an empty database.
    let transaction = connection.transaction()?;
    let mut template = None;
    for table_name in allowed_tables(bundle.operation) {
        if let Some(table) = bundle.tables.get(*table_name) {
            create_bundle_table(&transaction, table_name, table)?;
            insert_bundle_rows(&transaction, table_name, table)?;
        } else {
            // An omitted optional table still has to exist for the sync
            // engine to read, so borrow the destination's own DDL. One
            // connection serves every miss.
            if template.is_none() {
                template = Some(Connection::open_with_flags(
                    template_path,
                    OpenFlags::SQLITE_OPEN_READ_ONLY,
                )?);
            }
            let template = template.as_ref().expect("template connection is open");
            create_empty_table_from_template(&transaction, template, table_name)?;
        }
    }
    transaction.pragma_update(None, "user_version", bundle.source.schema_version)?;
    transaction.commit()?;
    Ok(())
}

fn create_bundle_table(
    connection: &Connection,
    name: &str,
    table: &BundleTable,
) -> anyhow::Result<()> {
    let columns = table
        .columns
        .iter()
        .map(|column| {
            let mut sql = format!(
                "{} {}",
                quote_identifier(&column.name),
                sqlite_affinity(&column.declared_type)
            );
            if column.primary_key {
                sql.push_str(" PRIMARY KEY");
            }
            if column.not_null {
                sql.push_str(" NOT NULL");
            }
            sql
        })
        .collect::<Vec<_>>()
        .join(", ");
    connection.execute_batch(&format!(
        "CREATE TABLE {} ({columns})",
        quote_identifier(name)
    ))?;
    Ok(())
}

fn create_empty_table_from_template(
    destination: &Connection,
    template: &Connection,
    name: &str,
) -> anyhow::Result<()> {
    let sql: String = template.query_row(
        "SELECT sql FROM sqlite_master WHERE type='table' AND lower(name)=lower(?1)",
        [name],
        |row| row.get(0),
    )?;
    destination.execute_batch(&sql)?;
    Ok(())
}

fn insert_bundle_rows(
    connection: &Connection,
    name: &str,
    table: &BundleTable,
) -> anyhow::Result<()> {
    let columns = table
        .columns
        .iter()
        .map(|column| quote_identifier(&column.name))
        .collect::<Vec<_>>()
        .join(", ");
    let placeholders = (1..=table.columns.len())
        .map(|index| format!("?{index}"))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "INSERT INTO {} ({columns}) VALUES ({placeholders})",
        quote_identifier(name)
    );
    let mut statement = connection.prepare(&sql)?;
    for row in &table.rows {
        let values = row
            .values
            .iter()
            .map(wire_to_sqlite)
            .collect::<anyhow::Result<Vec<_>>>()?;
        statement.execute(params_from_iter(values))?;
    }
    Ok(())
}

fn wire_to_sqlite(value: &WireValue) -> anyhow::Result<Value> {
    Ok(match value.kind {
        WireValueKind::Null => Value::Null,
        WireValueKind::Integer => Value::Integer(
            value
                .value
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("integer wire value is empty"))?
                .parse()?,
        ),
        WireValueKind::Real => Value::Real(
            value
                .value
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("real wire value is empty"))?
                .parse()?,
        ),
        WireValueKind::Text => Value::Text(value.value.clone().unwrap_or_default()),
        WireValueKind::Blob => Value::Blob(
            base64::engine::general_purpose::STANDARD.decode(
                value
                    .value
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("blob wire value is empty"))?,
            )?,
        ),
    })
}

pub(crate) fn export_bundle(
    database_path: &FsPath,
    catalog_id: &str,
    operation: SyncOperation,
    reviewed_only: bool,
    include_thumbnails: bool,
) -> anyhow::Result<CatalogBundle> {
    let connection = Connection::open_with_flags(database_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let schema_version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    anyhow::ensure!(
        schema_version >= 22,
        "Target Scheduler schema 22 or newer is required"
    );
    let mut tables = BTreeMap::new();
    let mut row_count = 0usize;
    for name in allowed_tables(operation) {
        // Thumbnails are optional payload, not identity: the receiver's
        // materializer creates an empty imagedata table and the merge is
        // additive, so omitting them only skips copying blobs.
        if *name == "imagedata" && !include_thumbnails {
            continue;
        }
        // Only acquiredimage narrows: reviewed_only means "the rows I have
        // actually graded". project and target ride along whole because the
        // grade read joins against them.
        let where_clause =
            (operation == SyncOperation::PushGrades && reviewed_only && *name == "acquiredimage")
                .then_some("gradingStatus <> 0");
        let table = read_bundle_table(&connection, name, where_clause)?;
        // Bound the export the same way we bound an import. A merge pulls
        // every acquiredimage row and every imagedata blob, so an unbounded
        // build is how the server runs itself out of memory answering one
        // request.
        row_count = row_count.saturating_add(table.rows.len());
        anyhow::ensure!(
            row_count <= MAX_BUNDLE_ROWS,
            "this database exceeds the {MAX_BUNDLE_ROWS} row export limit; \
             narrow the operation or sync in smaller pieces"
        );
        tables.insert((*name).to_string(), table);
    }
    let mut bundle = CatalogBundle {
        protocol_version: PROTOCOL_VERSION,
        bundle_id: Uuid::new_v4().to_string(),
        created_at_utc: Utc::now().to_rfc3339_opts(SecondsFormat::AutoSi, true),
        operation,
        source: CatalogIdentity {
            id: catalog_id.to_string(),
            product: "PSF Guard / N.I.N.A. Target Scheduler".into(),
            product_version: env!("CARGO_PKG_VERSION").into(),
            schema_version,
        },
        tables,
        payload_sha256: None,
    };
    let payload = serde_json::to_vec(&bundle)?;
    bundle.payload_sha256 = Some(digest_hex(&payload));
    Ok(bundle)
}

fn read_bundle_table(
    connection: &Connection,
    name: &str,
    where_clause: Option<&str>,
) -> anyhow::Result<BundleTable> {
    let mut schema_statement =
        connection.prepare(&format!("PRAGMA table_info({})", quote_identifier(name)))?;
    let columns = schema_statement
        .query_map([], |row| {
            Ok(BundleColumn {
                name: row.get(1)?,
                declared_type: row.get(2)?,
                not_null: row.get::<_, i64>(3)? != 0,
                primary_key: row.get::<_, i64>(5)? != 0,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    anyhow::ensure!(!columns.is_empty(), "table {name} is missing");
    let select = columns
        .iter()
        .map(|column| quote_identifier(&column.name))
        .collect::<Vec<_>>()
        .join(", ");
    let order = columns
        .iter()
        .find(|column| column.name.eq_ignore_ascii_case("Id"))
        .map(|column| format!(" ORDER BY {}", quote_identifier(&column.name)))
        .unwrap_or_default();
    let sql = format!(
        "SELECT {select} FROM {}{}{}",
        quote_identifier(name),
        where_clause
            .map(|clause| format!(" WHERE {clause}"))
            .unwrap_or_default(),
        order
    );
    let mut statement = connection.prepare(&sql)?;
    let column_count = columns.len();
    let rows = statement
        .query_map([], |row| {
            let mut values = Vec::with_capacity(column_count);
            for index in 0..column_count {
                values.push(sqlite_to_wire(row.get_ref(index)?));
            }
            Ok(BundleRow { values })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(BundleTable { columns, rows })
}

fn sqlite_to_wire(value: ValueRef<'_>) -> WireValue {
    let (kind, value) = match value {
        ValueRef::Null => (WireValueKind::Null, None),
        ValueRef::Integer(value) => (WireValueKind::Integer, Some(value.to_string())),
        ValueRef::Real(value) => (WireValueKind::Real, Some(value.to_string())),
        ValueRef::Text(value) => (
            WireValueKind::Text,
            Some(String::from_utf8_lossy(value).into_owned()),
        ),
        ValueRef::Blob(value) => (
            WireValueKind::Blob,
            Some(base64::engine::general_purpose::STANDARD.encode(value)),
        ),
    };
    WireValue { kind, value }
}

pub(crate) fn scheduler_request(
    operation: SyncOperation,
    source_id: String,
    dry_run: bool,
) -> SchedulerSyncRequest {
    SchedulerSyncRequest {
        peer_db_id: source_id,
        kind: match operation {
            SyncOperation::Merge => SchedulerSyncKind::Pull,
            SyncOperation::PushPlanning => SchedulerSyncKind::PushPlanning,
            SyncOperation::PushGrades => SchedulerSyncKind::PushGrades,
        },
        dry_run,
        with_image_data: Some(operation == SyncOperation::Merge),
        project: None,
        target: None,
        status: None,
        reviewed_only: false,
    }
}

pub(crate) fn result_summary(result: &SchedulerSyncResponse) -> BTreeMap<String, i64> {
    let mut summary = BTreeMap::from([
        ("total_inserted".into(), result.total_inserted as i64),
        ("total_updated".into(), result.total_updated as i64),
        ("grade_filled".into(), result.grade_filled as i64),
        ("grade_preserved".into(), result.grade_preserved as i64),
        ("imagedata_bytes".into(), result.imagedata_bytes as i64),
    ]);
    for (name, counts) in [
        ("exposuretemplate", &result.exposuretemplate),
        ("project", &result.project),
        ("ruleweight", &result.ruleweight),
        ("target", &result.target),
        ("exposureplan", &result.exposureplan),
    ] {
        add_table_summary(&mut summary, name, counts);
    }
    if let Some(counts) = &result.acquiredimage {
        add_table_summary(&mut summary, "acquiredimage", counts);
    }
    if let Some(counts) = &result.imagedata {
        add_table_summary(&mut summary, "imagedata", counts);
    }
    if let Some(grades) = &result.grades {
        summary.insert("grades_matched".into(), grades.matched as i64);
        summary.insert("grades_changed".into(), grades.changed as i64);
        summary.insert("grades_unchanged".into(), grades.unchanged as i64);
    }
    summary
}

fn add_table_summary(
    summary: &mut BTreeMap<String, i64>,
    name: &str,
    counts: &SchedulerSyncTableCounts,
) {
    summary.insert(format!("{name}_inserted"), counts.inserted as i64);
    summary.insert(format!("{name}_updated"), counts.updated as i64);
    summary.insert(format!("{name}_unchanged"), counts.unchanged as i64);
    summary.insert(format!("{name}_skipped"), counts.skipped as i64);
}

pub(crate) fn operation_label(operation: SyncOperation) -> &'static str {
    match operation {
        SyncOperation::Merge => "merge",
        SyncOperation::PushPlanning => "push_planning",
        SyncOperation::PushGrades => "push_grades",
    }
}

/// The stored preview keeps the engine's own `SchedulerSyncKind`, so map back
/// to the protocol name the client used.
pub(crate) fn kind_label(kind: SchedulerSyncKind) -> &'static str {
    match kind {
        SchedulerSyncKind::Pull => "merge",
        SchedulerSyncKind::PushPlanning => "push_planning",
        SchedulerSyncKind::PushGrades => "push_grades",
    }
}

/// A 404 that says why, since "not found" on a preview usually means it
/// expired or was already applied — not that the token is wrong.
fn unknown_preview() -> AppError {
    AppError::NotFoundMessage(
        "no such preview. Previews last 30 minutes and each applies once, so \
         create a new preview."
            .into(),
    )
}

/// Whether the server turned this request away on purpose, or broke.
fn outcome_of(error: &AppError) -> AuditOutcome {
    match error {
        AppError::BadRequest(_)
        | AppError::Conflict(_)
        | AppError::Forbidden(_)
        | AppError::NotFound
        | AppError::NotFoundMessage(_) => AuditOutcome::Refused,
        _ => AuditOutcome::Failed,
    }
}

fn detail_of(error: &AppError) -> String {
    match error {
        AppError::NotFound => "not found".into(),
        AppError::NotFoundMessage(message)
        | AppError::DatabaseError(message)
        | AppError::BadRequest(message)
        | AppError::Conflict(message)
        | AppError::Forbidden(message)
        | AppError::InternalError(message) => message.clone(),
        AppError::NotImplemented => "not implemented".into(),
    }
}

fn allowed_tables(operation: SyncOperation) -> &'static [&'static str] {
    match operation {
        SyncOperation::Merge => MERGE_TABLES,
        SyncOperation::PushPlanning => PLANNING_TABLES,
        SyncOperation::PushGrades => GRADE_TABLES,
    }
}

/// Tables without which the operation is a silent no-op. Anything else in
/// `allowed_tables` may be omitted; the materializer creates it empty from the
/// destination schema, and the sync engine then finds nothing to do.
fn required_tables(operation: SyncOperation) -> &'static [&'static str] {
    match operation {
        SyncOperation::Merge => &["project", "target", "acquiredimage"],
        SyncOperation::PushPlanning => &["project", "target", "exposureplan"],
        SyncOperation::PushGrades => GRADE_TABLES,
    }
}

fn sqlite_affinity(declared_type: &str) -> &'static str {
    let upper = declared_type.to_ascii_uppercase();
    if upper.contains("INT") {
        "INTEGER"
    } else if upper.contains("CHAR") || upper.contains("CLOB") || upper.contains("TEXT") {
        "TEXT"
    } else if upper.contains("BLOB") || upper.is_empty() {
        "BLOB"
    } else if upper.contains("REAL") || upper.contains("FLOA") || upper.contains("DOUB") {
        "REAL"
    } else {
        "NUMERIC"
    }
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn quote_identifier(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

fn digest_hex(value: &[u8]) -> String {
    use std::fmt::Write as _;

    let digest = Sha256::digest(value);
    let mut result = String::with_capacity(64);
    for byte in digest {
        write!(&mut result, "{byte:02x}").expect("writing to a String cannot fail");
    }
    result
}

fn unix_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn unix_timestamp(value: i64) -> String {
    DateTime::<Utc>::from_timestamp(value, 0)
        .expect("sync preview timestamps are valid")
        .to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn internal(error: impl std::fmt::Display) -> AppError {
    AppError::InternalError(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wire_int(value: i64) -> WireValue {
        WireValue {
            kind: WireValueKind::Integer,
            value: Some(value.to_string()),
        }
    }

    fn wire_text(value: &str) -> WireValue {
        WireValue {
            kind: WireValueKind::Text,
            value: Some(value.to_string()),
        }
    }

    fn column(name: &str, declared: &str, primary_key: bool) -> BundleColumn {
        BundleColumn {
            name: name.to_string(),
            declared_type: declared.to_string(),
            not_null: false,
            primary_key,
        }
    }

    /// A grades bundle with `rows` acquiredimage rows, ids taken from `ids`.
    fn big_grades_bundle(ids: impl Iterator<Item = i64>) -> CatalogBundle {
        let guid_table = BundleTable {
            columns: vec![column("guid", "TEXT", false)],
            rows: vec![BundleRow {
                values: vec![wire_text("a-guid")],
            }],
        };
        let image_table = BundleTable {
            columns: vec![
                column("Id", "INTEGER", true),
                column("guid", "TEXT", false),
                column("gradingStatus", "INTEGER", false),
            ],
            rows: ids
                .map(|id| BundleRow {
                    values: vec![wire_int(id), wire_text(&format!("guid-{id}")), wire_int(1)],
                })
                .collect(),
        };
        let mut tables = BTreeMap::new();
        tables.insert("project".to_string(), guid_table.clone());
        tables.insert("target".to_string(), guid_table);
        tables.insert("acquiredimage".to_string(), image_table);
        CatalogBundle {
            protocol_version: PROTOCOL_VERSION,
            bundle_id: "bundle-1".to_string(),
            created_at_utc: "2026-08-18T00:00:00Z".to_string(),
            operation: SyncOperation::PushGrades,
            source: CatalogIdentity {
                id: "source".to_string(),
                product: "test".to_string(),
                product_version: "1".to_string(),
                schema_version: 23,
            },
            tables,
            payload_sha256: None,
        }
    }

    fn temp_snapshot(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("psf-guard-materialize-tests");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("{name}-{}.sqlite", Uuid::new_v4()));
        let _ = std::fs::remove_file(&path);
        path
    }

    #[test]
    fn materializes_thousands_of_rows_in_one_transaction() {
        let bundle = big_grades_bundle(0..5_000);
        let path = temp_snapshot("bulk");
        let template = temp_snapshot("template");
        Connection::open(&template).unwrap();

        let started = std::time::Instant::now();
        materialize_bundle(&path, &template, &bundle).unwrap();
        let elapsed = started.elapsed();

        let connection = Connection::open(&path).unwrap();
        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM acquiredimage", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 5_000);
        let version: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 23);
        // One transaction, not one commit per row: even a slow disk finishes
        // 5000 rows in well under a second. The per-row autocommit this
        // replaces took minutes at this size on rotational storage.
        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "materialization took {elapsed:?}"
        );
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&template);
    }

    #[test]
    fn failed_materialization_rolls_back_to_an_empty_snapshot() {
        // Two rows share a primary key, so the second insert fails midway
        // through the bundle. The transaction must leave nothing behind —
        // a partial snapshot would let a later step read half a bundle.
        let bundle = big_grades_bundle([1, 2, 3, 3, 4].into_iter());
        let path = temp_snapshot("rollback");
        let template = temp_snapshot("rollback-template");
        Connection::open(&template).unwrap();

        let error = materialize_bundle(&path, &template, &bundle).unwrap_err();
        assert!(
            error.to_string().to_lowercase().contains("unique"),
            "unexpected error: {error:#}"
        );

        let connection = Connection::open(&path).unwrap();
        let tables: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(tables, 0, "rollback left tables in the snapshot");
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(&template);
    }

    #[test]
    fn merge_exports_skip_thumbnails_unless_asked() {
        let path = temp_snapshot("export-src");
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "PRAGMA user_version = 23;
                 CREATE TABLE project (Id INTEGER PRIMARY KEY, guid TEXT);
                 CREATE TABLE target (Id INTEGER PRIMARY KEY, guid TEXT);
                 CREATE TABLE exposuretemplate (Id INTEGER PRIMARY KEY, guid TEXT);
                 CREATE TABLE exposureplan (Id INTEGER PRIMARY KEY, guid TEXT);
                 CREATE TABLE ruleweight (Id INTEGER PRIMARY KEY, name TEXT);
                 CREATE TABLE acquiredimage (Id INTEGER PRIMARY KEY, gradingStatus INTEGER, guid TEXT);
                 CREATE TABLE imagedata (Id INTEGER PRIMARY KEY, imagedata BLOB, acquiredimageid INTEGER);
                 INSERT INTO project VALUES (1, 'pg');
                 INSERT INTO target VALUES (1, 'tg');
                 INSERT INTO acquiredimage VALUES (1, 1, 'ig');
                 INSERT INTO imagedata VALUES (1, X'0102030405', 1);",
            )
            .unwrap();
        drop(connection);

        // Thumbnails dominate bundle size; a merge export leaves them out
        // unless the client opts in.
        let lean = export_bundle(&path, "cat", SyncOperation::Merge, false, false).unwrap();
        assert!(!lean.tables.contains_key("imagedata"));
        assert_eq!(lean.tables["acquiredimage"].rows.len(), 1);

        let full = export_bundle(&path, "cat", SyncOperation::Merge, false, true).unwrap();
        assert_eq!(full.tables["imagedata"].rows.len(), 1);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn preview_jobs_report_phases_until_finished() {
        let store = PreviewJobStore::new();
        let (job, started) = store.start("catalog".to_string(), None).unwrap();
        assert!(started);
        assert_eq!(job.state, "running");
        assert_eq!(job.phase, Some("materializing"));

        store.set_phase(&job.job_id, "comparing");
        let running = store.get(&job.job_id, "catalog").unwrap().unwrap();
        assert_eq!(running.phase, Some("comparing"));

        store.finish(
            &job.job_id,
            Ok(SyncPreview {
                preview_id: "preview-1".to_string(),
                state: "ready",
                expires_at: "2026-08-18T00:30:00Z".to_string(),
                summary: BTreeMap::new(),
            }),
        );
        let finished = store.get(&job.job_id, "catalog").unwrap().unwrap();
        assert_eq!(finished.state, "ready");
        assert_eq!(finished.phase, None);

        // A phase set after completion must not resurrect a running look.
        store.set_phase(&job.job_id, "comparing");
        let still = store.get(&job.job_id, "catalog").unwrap().unwrap();
        assert_eq!(still.phase, None);
    }

    #[tokio::test]
    async fn export_response_header_matches_the_body_bytes() {
        // The client hashes the raw body it received and compares it to the
        // header — the only digest check that survives two JSON writers.
        let export = SyncExport {
            export_id: "export-1".into(),
            state: "ready",
            bundle: Some(grades_bundle(&grade_tables(), "")),
            error: None,
        };
        let response = export_response(export).unwrap();
        let header = response
            .headers()
            .get(CONTENT_SHA256_HEADER)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(header, digest_hex(&body));
        assert_eq!(header.len(), 64);
    }

    fn grades_bundle(tables: &str, digest: &str) -> CatalogBundle {
        let json = format!(
            r#"{{
            "protocol_version":1,
            "bundle_id":"b03b8ab1-ce43-4a87-a4fb-68497394cedb",
            "created_at_utc":"2026-07-23T12:00:00+00:00",
            "operation":"push_grades",
            "source":{{
                "id":"source",
                "product":"Target Scheduler",
                "product_version":"5.9.6.0",
                "schema_version":23
            }},
            "tables":{tables}
            {digest}
        }}"#
        );
        serde_json::from_str(&json).unwrap()
    }

    const GUID_COLUMN: &str =
        r#"{"name":"guid","declared_type":"TEXT","not_null":false,"primary_key":false}"#;
    const GUID_ROW: &str = r#"{"values":[{"kind":"text","value":"a-guid"}]}"#;

    /// Every table a grade push needs: the two it joins against, and the one
    /// it actually reads.
    fn grade_tables() -> String {
        format!(
            r#"{{
            "project":{{"columns":[{GUID_COLUMN}],"rows":[{GUID_ROW}]}},
            "target":{{"columns":[{GUID_COLUMN}],"rows":[{GUID_ROW}]}},
            "acquiredimage":{{"columns":[{GUID_COLUMN}],"rows":[{GUID_ROW}]}}
        }}"#
        )
    }

    #[test]
    fn accepts_a_bundle_whose_digest_is_absent_or_stale() {
        // The digest is advisory. A plugin that omits it, or that computes it
        // over a different JSON encoding than ours, still syncs.
        validate_bundle(&grades_bundle(&grade_tables(), "")).unwrap();
        validate_bundle(&grades_bundle(
            &grade_tables(),
            r#","payload_sha256":"0000000000000000000000000000000000000000000000000000000000000000""#,
        ))
        .unwrap();
    }

    #[test]
    fn rejects_a_bundle_missing_the_table_its_operation_needs() {
        // A grade push without acquiredimage would apply cleanly and change
        // nothing, which reads to the client as a successful sync.
        let tables = format!(
            r#"{{
            "project":{{"columns":[{GUID_COLUMN}],"rows":[]}},
            "target":{{"columns":[{GUID_COLUMN}],"rows":[]}}
        }}"#
        );
        let error = validate_bundle(&grades_bundle(&tables, "")).unwrap_err();
        assert!(
            matches!(&error, AppError::BadRequest(message) if message.contains("acquiredimage")),
            "expected a 400 naming the table, got {error:?}"
        );
    }

    #[test]
    fn counts_rows_across_every_table_against_the_budget() {
        // Two tables of one row each: within a budget of two, over a budget
        // of one. The bound has to be the bundle's total, not any one table's.
        let bundle: CatalogBundle = serde_json::from_str(&format!(
            r#"{{
            "protocol_version":1,
            "bundle_id":"b03b8ab1-ce43-4a87-a4fb-68497394cedb",
            "created_at_utc":"2026-07-23T12:00:00+00:00",
            "operation":"push_planning",
            "source":{{"id":"s","product":"p","product_version":"1","schema_version":23}},
            "tables":{{
                "project":{{"columns":[{GUID_COLUMN}],"rows":[{GUID_ROW}]}},
                "target":{{"columns":[{GUID_COLUMN}],"rows":[{GUID_ROW}]}},
                "exposureplan":{{"columns":[{GUID_COLUMN}],"rows":[]}}
            }}
        }}"#
        ))
        .unwrap();

        validate_bundle_within(&bundle, 2).unwrap();
        let error = validate_bundle_within(&bundle, 1).unwrap_err();
        assert!(
            matches!(&error, AppError::BadRequest(message) if message.contains("row limit")),
            "expected a 400 about the row limit, got {error:?}"
        );
    }

    #[test]
    fn stored_previews_report_the_operation_the_client_asked_for() {
        // The audit log names the protocol operation, but a stored preview
        // only kept the engine's own kind. Round-trip every operation so a
        // merge is never recorded under the engine's name for it.
        for operation in [
            SyncOperation::Merge,
            SyncOperation::PushPlanning,
            SyncOperation::PushGrades,
        ] {
            let request = scheduler_request(operation, "source".into(), true);
            assert_eq!(kind_label(request.kind), operation_label(operation));
        }
    }

    #[test]
    fn rejects_a_table_outside_the_operation() {
        // exposureplan belongs to a planning push, never to a grade push.
        let tables = format!(
            r#"{{
            "exposureplan":{{"columns":[{GUID_COLUMN}],"rows":[]}},
            "project":{{"columns":[{GUID_COLUMN}],"rows":[]}},
            "target":{{"columns":[{GUID_COLUMN}],"rows":[]}},
            "acquiredimage":{{"columns":[{GUID_COLUMN}],"rows":[]}}
        }}"#
        );
        let error = validate_bundle(&grades_bundle(&tables, "")).unwrap_err();
        assert!(
            matches!(&error, AppError::BadRequest(message) if message.contains("exposureplan")),
            "expected a 400 naming the table, got {error:?}"
        );
    }
}
