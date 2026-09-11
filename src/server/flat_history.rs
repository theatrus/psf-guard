//! Explicit Target Scheduler flat-coverage invalidation, not calibration grading.
//!
//! Scheduler history rows have no GUID. Their origin, local row ID and capture
//! fingerprint identify one observed generation; PSF Guard never matches files
//! to it or edits the scheduler's native table on the server.

use std::{collections::HashSet, sync::Arc};

use axum::{
    extract::{Query, State},
    http::HeaderMap,
    Json,
};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    api::ApiResponse,
    database_context::{open_scheduler_connection_with_flags, DatabaseContext},
    extract::DbContext,
    handlers::{require_database_management_allowed, AppError},
    remote_audit::{AuditAction, AuditOutcome, AuditRecord},
    remote_sync::{authenticated_catalog, require_catalog},
    state::AppState,
};

pub const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;
const MAX_RECORDS: usize = 1_000;
const TABLE: &str = "psf_guard_scheduler_flat_history";

#[derive(Debug, Clone, Deserialize)]
pub struct Scope {
    pub protocol_version: u32,
    pub catalog_id: String,
    pub origin_id: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct CaptureRecord {
    pub source_row_id: i64,
    pub fingerprint: String,
    pub target_guid: Option<String>,
    pub target_name: Option<String>,
    pub profile_id: String,
    pub light_session_date: Option<i64>,
    #[serde(default)]
    pub light_session_id: i64,
    pub flats_taken_date: Option<i64>,
    pub flats_type: Option<String>,
    pub filter_name: Option<String>,
    pub gain: Option<i64>,
    pub offset: Option<i64>,
    pub bin: Option<i64>,
    pub readout_mode: Option<i64>,
    pub rotation: Option<f64>,
    pub roi: Option<f64>,
}

#[derive(Debug, Deserialize)]
pub struct SnapshotRequest {
    #[serde(flatten)]
    pub scope: Scope,
    pub source_name: String,
    pub records: Vec<CaptureRecord>,
}

#[derive(Debug, Serialize)]
pub struct SnapshotResponse {
    pub catalog_id: String,
    pub origin_id: String,
    pub received: usize,
}

#[derive(Debug, Deserialize)]
pub struct PendingRequest {
    #[serde(flatten)]
    pub scope: Scope,
    pub target_guid: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Decision {
    pub record_id: String,
    pub source_row_id: i64,
    pub fingerprint: String,
    pub target_guid: Option<String>,
    pub reason: String,
}

#[derive(Debug, Serialize)]
pub struct PendingResponse {
    pub catalog_id: String,
    pub origin_id: String,
    pub decisions: Vec<Decision>,
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryState {
    Recorded,
    Pending,
    Removed,
    Absent,
    Conflict,
    Superseded,
}

impl HistoryState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Recorded => "recorded",
            Self::Pending => "pending",
            Self::Removed => "removed",
            Self::Absent => "absent",
            Self::Conflict => "conflict",
            Self::Superseded => "superseded",
        }
    }
    fn parse(value: &str) -> Result<Self, AppError> {
        match value {
            "recorded" => Ok(Self::Recorded),
            "pending" => Ok(Self::Pending),
            "removed" => Ok(Self::Removed),
            "absent" => Ok(Self::Absent),
            "conflict" => Ok(Self::Conflict),
            "superseded" => Ok(Self::Superseded),
            _ => Err(AppError::DatabaseError(
                "invalid stored flat-history state".into(),
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AcknowledgeStatus {
    Removed,
    Absent,
    Conflict,
}

impl AcknowledgeStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Removed => "removed",
            Self::Absent => "absent",
            Self::Conflict => "conflict",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct AcknowledgeResult {
    pub record_id: String,
    pub source_row_id: i64,
    pub fingerprint: String,
    pub status: AcknowledgeStatus,
    pub detail: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct AcknowledgeRequest {
    #[serde(flatten)]
    pub scope: Scope,
    pub results: Vec<AcknowledgeResult>,
}

#[derive(Debug, Serialize)]
pub struct AcknowledgeResponse {
    pub catalog_id: String,
    pub origin_id: String,
    pub acknowledged: usize,
}

#[derive(Debug, Serialize)]
pub struct HistoryRecord {
    pub record_id: String,
    pub origin_id: String,
    pub source_name: String,
    #[serde(flatten)]
    pub capture: CaptureRecord,
    pub state: HistoryState,
    pub reason: Option<String>,
    pub invalidated_at: Option<i64>,
    pub last_seen: i64,
    pub acknowledged_at: Option<i64>,
    pub detail: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct ListQuery {
    pub limit: Option<usize>,
    pub offset: Option<usize>,
    pub state: Option<HistoryState>,
    pub q: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct HistoryList {
    pub records: Vec<HistoryRecord>,
    pub total: usize,
}

#[derive(Debug, Deserialize)]
pub struct InvalidateRequest {
    pub record_ids: Vec<String>,
    pub reason: String,
}

#[derive(Debug, Serialize)]
pub struct InvalidateResponse {
    pub invalidated: usize,
}

fn bad(message: &str) -> AppError {
    AppError::BadRequest(message.into())
}

fn uuid(value: &str, field: &str) -> Result<(), AppError> {
    let parsed = Uuid::parse_str(value).map_err(|_| bad(&format!("{field} must be a UUID")))?;
    if parsed.is_nil() || value != parsed.hyphenated().to_string() {
        return Err(bad(&format!(
            "{field} must be a nonzero lowercase hyphenated UUID"
        )));
    }
    Ok(())
}

fn text(value: &str, field: &str, max: usize, required: bool) -> Result<(), AppError> {
    if value.len() > max || value.contains('\0') || (required && value.trim().is_empty()) {
        return Err(bad(&format!("{field} is empty or too long")));
    }
    Ok(())
}

fn optional_text(value: &Option<String>, field: &str, max: usize) -> Result<(), AppError> {
    if let Some(value) = value {
        text(value, field, max, false)?;
    }
    Ok(())
}

fn identity(source_row_id: i64, fingerprint: &str) -> Result<(), AppError> {
    if source_row_id <= 0 {
        return Err(bad("source_row_id must be positive"));
    }
    if fingerprint.len() != 64
        || !fingerprint
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(bad("fingerprint must be lowercase SHA-256 hex"));
    }
    Ok(())
}

fn validate_scope(scope: &Scope, ctx: &DatabaseContext) -> Result<(), AppError> {
    require_catalog(ctx, &scope.catalog_id)?;
    if scope.protocol_version != 1 {
        return Err(bad("unsupported flat-history protocol version"));
    }
    uuid(&scope.origin_id, "origin_id")
}

fn validate_capture(record: &CaptureRecord) -> Result<(), AppError> {
    identity(record.source_row_id, &record.fingerprint)?;
    if let Some(guid) = &record.target_guid {
        uuid(guid, "target_guid")?;
    }
    optional_text(&record.target_name, "target_name", 512)?;
    text(&record.profile_id, "profile_id", 128, true)?;
    optional_text(&record.flats_type, "flats_type", 64)?;
    optional_text(&record.filter_name, "filter_name", 128)?;
    if record.rotation.is_some_and(|value| !value.is_finite())
        || record.roi.is_some_and(|value| !value.is_finite())
    {
        return Err(bad("rotation and roi must be finite"));
    }
    Ok(())
}

fn bounded_count(count: usize) -> Result<(), AppError> {
    if count > MAX_RECORDS {
        return Err(bad(
            "at most 1000 flat-history records are allowed per request",
        ));
    }
    Ok(())
}

fn schema_exists(conn: &Connection) -> Result<bool, AppError> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
        [TABLE],
        |row| row.get(0),
    )
    .map_err(AppError::db)
}

fn ensure_schema(conn: &Connection) -> Result<(), AppError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS psf_guard_scheduler_flat_history (
            record_id TEXT PRIMARY KEY,
            origin_id TEXT NOT NULL,
            source_row_id INTEGER NOT NULL,
            fingerprint TEXT NOT NULL,
            target_guid TEXT,
            source_name TEXT NOT NULL,
            record_json TEXT NOT NULL,
            state TEXT NOT NULL CHECK(state IN ('recorded','pending','removed','absent','conflict','superseded')),
            reason TEXT,
            invalidated_at INTEGER,
            last_seen INTEGER NOT NULL,
            acknowledged_at INTEGER,
            detail TEXT,
            UNIQUE(origin_id, source_row_id, fingerprint)
        );
        CREATE INDEX IF NOT EXISTS idx_psf_guard_flat_history_pending
            ON psf_guard_scheduler_flat_history(origin_id, state, target_guid, invalidated_at, record_id);
        CREATE INDEX IF NOT EXISTS idx_psf_guard_flat_history_state
            ON psf_guard_scheduler_flat_history(state, last_seen);"
    ).map_err(AppError::db)
}

fn read_capture(value: &str) -> Result<CaptureRecord, AppError> {
    serde_json::from_str(value)
        .map_err(|_| AppError::DatabaseError("invalid stored flat-history capture".into()))
}

fn same_capture(left: &CaptureRecord, right: &CaptureRecord) -> bool {
    // A target rename is presentation metadata, not a new capture generation.
    let mut left = left.clone();
    let mut right = right.clone();
    left.target_name = None;
    right.target_name = None;
    left == right
}

fn store_snapshot(
    conn: &mut Connection,
    request: &SnapshotRequest,
    now: i64,
) -> Result<usize, AppError> {
    bounded_count(request.records.len())?;
    text(&request.source_name, "source_name", 256, true)?;
    let mut ids = HashSet::new();
    for record in &request.records {
        validate_capture(record)?;
        if !ids.insert(record.source_row_id) {
            return Err(bad("snapshot contains duplicate source row IDs"));
        }
    }
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(AppError::db)?;
    ensure_schema(&tx)?;
    for record in &request.records {
        let stored: Option<String> = tx.query_row(
            "SELECT record_json FROM psf_guard_scheduler_flat_history WHERE origin_id=?1 AND source_row_id=?2 AND fingerprint=?3",
            params![request.scope.origin_id, record.source_row_id, record.fingerprint], |row| row.get(0),
        ).optional().map_err(AppError::db)?;
        let record_json = serde_json::to_string(record).map_err(AppError::db)?;
        if let Some(stored) = stored {
            if !same_capture(&read_capture(&stored)?, record) {
                return Err(AppError::Conflict(
                    "flat-history fingerprint was reused for different capture metadata".into(),
                ));
            }
            // Replayed old generations remain terminal or superseded. Decisions
            // never inherit the telescope's view of whether flats are still good.
            tx.execute(
                "UPDATE psf_guard_scheduler_flat_history SET source_name=?1, record_json=?2, last_seen=?3
                 WHERE origin_id=?4 AND source_row_id=?5 AND fingerprint=?6",
                params![request.source_name, record_json, now, request.scope.origin_id, record.source_row_id, record.fingerprint],
            ).map_err(AppError::db)?;
        } else {
            tx.execute(
                "UPDATE psf_guard_scheduler_flat_history SET state='superseded'
                 WHERE origin_id=?1 AND source_row_id=?2 AND state='recorded'",
                params![request.scope.origin_id, record.source_row_id],
            )
            .map_err(AppError::db)?;
            tx.execute(
                "INSERT INTO psf_guard_scheduler_flat_history
                 (record_id,origin_id,source_row_id,fingerprint,target_guid,source_name,record_json,state,last_seen)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,'recorded',?8)",
                params![Uuid::new_v4().to_string(), request.scope.origin_id, record.source_row_id, record.fingerprint,
                    record.target_guid, request.source_name, record_json, now],
            ).map_err(AppError::db)?;
        }
    }
    tx.commit().map_err(AppError::db)?;
    Ok(request.records.len())
}

fn pending_decisions(
    conn: &Connection,
    request: &PendingRequest,
) -> Result<Vec<Decision>, AppError> {
    if let Some(guid) = &request.target_guid {
        uuid(guid, "target_guid")?;
    }
    if !schema_exists(conn)? {
        return Ok(Vec::new());
    }
    let mut statement = conn.prepare(
        "SELECT record_id,source_row_id,fingerprint,target_guid,reason FROM psf_guard_scheduler_flat_history
         WHERE origin_id=?1 AND state='pending' AND (?2 IS NULL OR target_guid=?2)
         ORDER BY invalidated_at,record_id LIMIT 1000"
    ).map_err(AppError::db)?;
    statement
        .query_map(
            params![request.scope.origin_id, request.target_guid],
            |row| {
                Ok(Decision {
                    record_id: row.get(0)?,
                    source_row_id: row.get(1)?,
                    fingerprint: row.get(2)?,
                    target_guid: row.get(3)?,
                    reason: row.get(4)?,
                })
            },
        )
        .map_err(AppError::db)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(AppError::db)
}

fn acknowledge_records(
    conn: &mut Connection,
    request: &AcknowledgeRequest,
    now: i64,
) -> Result<usize, AppError> {
    bounded_count(request.results.len())?;
    let mut ids = HashSet::new();
    for result in &request.results {
        uuid(&result.record_id, "record_id")?;
        identity(result.source_row_id, &result.fingerprint)?;
        optional_text(&result.detail, "detail", 1024)?;
        if !ids.insert(&result.record_id) {
            return Err(bad("acknowledgement contains duplicate record IDs"));
        }
    }
    if request.results.is_empty() {
        return Ok(0);
    }
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(AppError::db)?;
    if !schema_exists(&tx)? {
        return Err(AppError::NotFound);
    }
    for result in &request.results {
        let stored: Option<(String, i64, String, String)> = tx.query_row(
            "SELECT origin_id,source_row_id,fingerprint,state FROM psf_guard_scheduler_flat_history WHERE record_id=?1",
            [&result.record_id], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?)),
        ).optional().map_err(AppError::db)?;
        let (origin, row_id, fingerprint, state) = stored.ok_or(AppError::NotFound)?;
        if origin != request.scope.origin_id {
            return Err(AppError::Forbidden(
                "flat-history record belongs to a different origin".into(),
            ));
        }
        if row_id != result.source_row_id || fingerprint != result.fingerprint {
            return Err(AppError::Conflict(
                "flat-history acknowledgement identity does not match".into(),
            ));
        }
        if state == result.status.as_str()
            || (matches!(state.as_str(), "removed" | "absent")
                && matches!(
                    result.status,
                    AcknowledgeStatus::Removed | AcknowledgeStatus::Absent
                ))
        {
            // A replay after deletion observes absence. Keep the first audit
            // result instead of failing this otherwise successful batch.
            continue;
        }
        if state != "pending" {
            return Err(AppError::Conflict(
                "flat-history decision is not pending".into(),
            ));
        }
        tx.execute(
            "UPDATE psf_guard_scheduler_flat_history SET state=?1,acknowledged_at=?2,detail=?3 WHERE record_id=?4",
            params![result.status.as_str(), now, result.detail, result.record_id],
        ).map_err(AppError::db)?;
    }
    tx.commit().map_err(AppError::db)?;
    Ok(request.results.len())
}

fn list_records(conn: &Connection, query: &ListQuery) -> Result<HistoryList, AppError> {
    let limit = query.limit.unwrap_or(100);
    let offset = query.offset.unwrap_or(0);
    if !(1..=1000).contains(&limit) || offset > i64::MAX as usize {
        return Err(bad("invalid flat-history pagination"));
    }
    optional_text(&query.q, "q", 200)?;
    if !schema_exists(conn)? {
        return Ok(HistoryList {
            records: Vec::new(),
            total: 0,
        });
    }
    let search = query
        .q
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| {
            format!(
                "%{}%",
                value
                    .replace('\\', "\\\\")
                    .replace('%', "\\%")
                    .replace('_', "\\_")
            )
        });
    let state = query.state.map(HistoryState::as_str);
    let filter = "WHERE (?1 IS NULL OR state=?1) AND (?2 IS NULL
        OR source_name LIKE ?2 ESCAPE '\\'
        OR json_extract(record_json,'$.target_name') LIKE ?2 ESCAPE '\\'
        OR json_extract(record_json,'$.filter_name') LIKE ?2 ESCAPE '\\'
        OR json_extract(record_json,'$.profile_id') LIKE ?2 ESCAPE '\\')";
    // Read the total and page from one snapshot while a remote snapshot arrives.
    let tx = conn.unchecked_transaction().map_err(AppError::db)?;
    let total: i64 = tx
        .query_row(
            &format!("SELECT COUNT(*) FROM {TABLE} {filter}"),
            params![state, search],
            |row| row.get(0),
        )
        .map_err(AppError::db)?;
    let total = usize::try_from(total)
        .map_err(|_| AppError::DatabaseError("invalid flat-history record count".into()))?;
    let records = {
        let mut statement = tx.prepare(&format!(
            "SELECT record_id,origin_id,source_name,record_json,state,reason,invalidated_at,last_seen,acknowledged_at,detail
             FROM {TABLE} {filter} ORDER BY json_extract(record_json,'$.flats_taken_date') DESC,record_id LIMIT ?3 OFFSET ?4"
        )).map_err(AppError::db)?;
        let mut rows = statement
            .query(params![state, search, limit as i64, offset as i64])
            .map_err(AppError::db)?;
        let mut records = Vec::new();
        while let Some(row) = rows.next().map_err(AppError::db)? {
            records.push(HistoryRecord {
                record_id: row.get(0).map_err(AppError::db)?,
                origin_id: row.get(1).map_err(AppError::db)?,
                source_name: row.get(2).map_err(AppError::db)?,
                capture: read_capture(&row.get::<_, String>(3).map_err(AppError::db)?)?,
                state: HistoryState::parse(&row.get::<_, String>(4).map_err(AppError::db)?)?,
                reason: row.get(5).map_err(AppError::db)?,
                invalidated_at: row.get(6).map_err(AppError::db)?,
                last_seen: row.get(7).map_err(AppError::db)?,
                acknowledged_at: row.get(8).map_err(AppError::db)?,
                detail: row.get(9).map_err(AppError::db)?,
            });
        }
        records
    };
    tx.commit().map_err(AppError::db)?;
    Ok(HistoryList { records, total })
}

fn invalidate_records(
    conn: &mut Connection,
    request: &InvalidateRequest,
    now: i64,
) -> Result<usize, AppError> {
    bounded_count(request.record_ids.len())?;
    if request.record_ids.is_empty() {
        return Err(bad("no flat-history records selected"));
    }
    text(&request.reason, "reason", 1024, true)?;
    let mut ids = HashSet::new();
    for id in &request.record_ids {
        uuid(id, "record_id")?;
        if !ids.insert(id) {
            return Err(bad("duplicate flat-history record IDs"));
        }
    }
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(AppError::db)?;
    if !schema_exists(&tx)? {
        return Err(AppError::NotFound);
    }
    let mut invalidated = 0;
    for id in &request.record_ids {
        let state: Option<String> = tx
            .query_row(
                "SELECT state FROM psf_guard_scheduler_flat_history WHERE record_id=?1",
                [id],
                |row| row.get(0),
            )
            .optional()
            .map_err(AppError::db)?;
        match state.as_deref() {
            None => return Err(AppError::NotFound),
            Some("pending") => continue,
            Some("recorded") => {}
            _ => {
                return Err(AppError::Conflict(
                    "only currently recorded flat history can be invalidated".into(),
                ))
            }
        }
        invalidated += tx.execute(
            "UPDATE psf_guard_scheduler_flat_history SET state='pending',reason=?1,invalidated_at=?2 WHERE record_id=?3",
            params![request.reason.trim(),now,id],
        ).map_err(AppError::db)?;
    }
    tx.commit().map_err(AppError::db)?;
    Ok(invalidated)
}

async fn database_task<T: Send + 'static>(
    ctx: Arc<DatabaseContext>,
    write: bool,
    task: impl FnOnce(&mut Connection) -> Result<T, AppError> + Send + 'static,
) -> Result<T, AppError> {
    tokio::task::spawn_blocking(move || {
        let flags = if write {
            OpenFlags::SQLITE_OPEN_READ_WRITE
        } else {
            OpenFlags::SQLITE_OPEN_READ_ONLY
        };
        let mut conn = open_scheduler_connection_with_flags(&ctx.database_path, flags)
            .map_err(AppError::db)?;
        task(&mut conn)
    })
    .await
    .map_err(|_| AppError::InternalError("flat-history database task failed".into()))?
}

fn audit<T>(
    state: &AppState,
    catalog_id: &str,
    scope: &Scope,
    action: AuditAction,
    operation: &str,
    result: &Result<T, AppError>,
) {
    let outcome = match result {
        Ok(_) => AuditOutcome::Ok,
        Err(AppError::DatabaseError(_) | AppError::InternalError(_)) => AuditOutcome::Failed,
        Err(_) => AuditOutcome::Refused,
    };
    state.remote_audit.record(
        catalog_id,
        action,
        outcome,
        AuditRecord {
            operation: Some(operation),
            source_id: Some(&scope.origin_id),
            ..Default::default()
        },
    );
    tracing::info!(catalog=catalog_id,origin=%scope.origin_id,operation,?outcome,"flat-history sync");
}

pub async fn snapshot(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<SnapshotRequest>,
) -> Result<Json<ApiResponse<SnapshotResponse>>, AppError> {
    let ctx = authenticated_catalog(&state, &headers, AuditAction::FlatHistorySnapshot)?;
    let catalog_id = ctx.id.clone();
    let scope = request.scope.clone();
    let result = async {
        validate_scope(&scope, &ctx)?;
        let received = database_task(ctx, true, move |conn| {
            store_snapshot(conn, &request, chrono::Utc::now().timestamp())
        })
        .await?;
        Ok(Json(ApiResponse::success(SnapshotResponse {
            catalog_id: scope.catalog_id.clone(),
            origin_id: scope.origin_id.clone(),
            received,
        })))
    }
    .await;
    audit(
        &state,
        &catalog_id,
        &scope,
        AuditAction::FlatHistorySnapshot,
        "flat_history_snapshot",
        &result,
    );
    result
}

pub async fn pending(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<PendingRequest>,
) -> Result<Json<ApiResponse<PendingResponse>>, AppError> {
    let ctx = authenticated_catalog(&state, &headers, AuditAction::FlatHistoryPending)?;
    let catalog_id = ctx.id.clone();
    let scope = request.scope.clone();
    let result = async {
        validate_scope(&scope, &ctx)?;
        let decisions =
            database_task(ctx, false, move |conn| pending_decisions(conn, &request)).await?;
        Ok(Json(ApiResponse::success(PendingResponse {
            catalog_id: scope.catalog_id.clone(),
            origin_id: scope.origin_id.clone(),
            decisions,
        })))
    }
    .await;
    audit(
        &state,
        &catalog_id,
        &scope,
        AuditAction::FlatHistoryPending,
        "flat_history_pending",
        &result,
    );
    result
}

pub async fn acknowledge(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<AcknowledgeRequest>,
) -> Result<Json<ApiResponse<AcknowledgeResponse>>, AppError> {
    let ctx = authenticated_catalog(&state, &headers, AuditAction::FlatHistoryAcknowledge)?;
    let catalog_id = ctx.id.clone();
    let scope = request.scope.clone();
    let result = async {
        validate_scope(&scope, &ctx)?;
        let acknowledged = database_task(ctx, true, move |conn| {
            acknowledge_records(conn, &request, chrono::Utc::now().timestamp())
        })
        .await?;
        Ok(Json(ApiResponse::success(AcknowledgeResponse {
            catalog_id: scope.catalog_id.clone(),
            origin_id: scope.origin_id.clone(),
            acknowledged,
        })))
    }
    .await;
    audit(
        &state,
        &catalog_id,
        &scope,
        AuditAction::FlatHistoryAcknowledge,
        "flat_history_acknowledge",
        &result,
    );
    result
}

pub async fn list(
    ctx: DbContext,
    Query(query): Query<ListQuery>,
) -> Result<Json<ApiResponse<HistoryList>>, AppError> {
    database_task(ctx.0, false, move |conn| list_records(conn, &query))
        .await
        .map(ApiResponse::success)
        .map(Json)
}

pub async fn invalidate(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Json(request): Json<InvalidateRequest>,
) -> Result<Json<ApiResponse<InvalidateResponse>>, AppError> {
    require_database_management_allowed(&state)?;
    let invalidated = database_task(ctx.0, true, move |conn| {
        invalidate_records(conn, &request, chrono::Utc::now().timestamp())
    })
    .await?;
    Ok(Json(ApiResponse::success(InvalidateResponse {
        invalidated,
    })))
}

#[cfg(test)]
mod tests;
