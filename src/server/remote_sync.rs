//! Token-authenticated database sync for remote N.I.N.A. clients.
//!
//! The wire format is deliberately database-shaped: a client freezes selected
//! Target Scheduler tables into a signed JSON bundle, while PSF Guard
//! materializes that bundle as a temporary SQLite source and delegates every
//! merge decision to the existing local database sync engine.

use axum::{
    extract::{Path, State},
    http::{header::AUTHORIZATION, HeaderMap},
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
    sync::{Arc, Mutex, OnceLock},
};
use uuid::Uuid;

use crate::server::{
    api::{
        ApiResponse, SchedulerSyncKind, SchedulerSyncRequest, SchedulerSyncResponse,
        SchedulerSyncTableCounts,
    },
    database_context::DatabaseContext,
    handlers::{execute_scheduler_sync_paths, AppError, SyncGuardMode},
    state::AppState,
};

pub const MAX_SYNC_BODY_BYTES: usize = 128 * 1024 * 1024;
const MAX_BUNDLE_ROWS: usize = 250_000;
const PROTOCOL_VERSION: u32 = 1;
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

static EXPORTS: OnceLock<Mutex<HashMap<String, (String, SyncExport)>>> = OnceLock::new();

pub async fn capabilities(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<ApiResponse<SyncCapabilities>>, AppError> {
    let catalog = authenticated_catalog(&state, &headers)?;
    Ok(Json(ApiResponse::success(SyncCapabilities {
        protocol_version: PROTOCOL_VERSION,
        product: "PSF Guard",
        product_version: env!("CARGO_PKG_VERSION"),
        capabilities: vec![
            "merge",
            "push_planning",
            "push_grades",
            "preview_apply",
            "exports",
            "image_upload",
        ],
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
) -> Result<Json<ApiResponse<SyncPreview>>, AppError> {
    let catalog = authenticated_catalog(&state, &headers)?;
    require_protocol(request.protocol_version)?;
    require_catalog(&catalog, &request.catalog_id)?;
    if request.operation != request.bundle.operation {
        return Err(AppError::BadRequest(
            "request operation does not match bundle operation".into(),
        ));
    }
    validate_bundle(&request.bundle)?;

    let destination_path = PathBuf::from(&catalog.database_path);
    let bundle = request.bundle;
    let source_id = bundle.source.id.clone();
    let operation = request.operation;
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
    if let Err(error) = tokio::task::spawn_blocking(move || {
        materialize_bundle(&materialize_path, &template_path, &bundle)
    })
    .await
    .map_err(|error| AppError::InternalError(format!("bundle task failed: {error}")))?
    {
        state.sync_previews.remove_source_snapshot(&snapshot_file);
        return Err(AppError::BadRequest(format!(
            "invalid catalog bundle: {error:#}"
        )));
    }

    let sync_request = scheduler_request(operation, source_id.clone(), true);
    let execution = execute_scheduler_sync_paths(
        &state,
        snapshot_path,
        destination_path,
        source_id,
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
    Ok(Json(ApiResponse::success(SyncPreview {
        preview_id: record.id,
        state: "ready",
        expires_at: unix_timestamp(record.expires_at),
        summary: result_summary(&result),
    })))
}

pub async fn get_preview(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(preview_id): Path<String>,
) -> Result<Json<ApiResponse<SyncPreview>>, AppError> {
    let catalog = authenticated_catalog(&state, &headers)?;
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
    let catalog = authenticated_catalog(&state, &headers)?;
    let _apply_guard = state.sync_apply_lock.lock().await;
    let record = state
        .sync_previews
        .claim(&preview_id, &catalog.id)
        .map_err(internal)?
        .ok_or(AppError::NotFound)?;
    let source_path = state
        .sync_previews
        .source_snapshot_path(&record)
        .map_err(internal)?;
    let mut request = record.request;
    request.dry_run = false;
    let result = execute_scheduler_sync_paths(
        &state,
        source_path,
        PathBuf::from(&catalog.database_path),
        request.peer_db_id.clone(),
        catalog.id.clone(),
        request,
        SyncGuardMode::Apply {
            destination_fingerprint: record.destination_fingerprint,
        },
    )
    .await
    .map(|(result, _)| result);
    state
        .sync_previews
        .remove_source_snapshot(&record.source_snapshot_file);
    let result = result?;
    Ok(Json(ApiResponse::success(SyncApplyResult {
        state: "applied",
        summary: result_summary(&result),
    })))
}

pub async fn create_export(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(request): Json<CreateExportRequest>,
) -> Result<Json<ApiResponse<SyncExport>>, AppError> {
    let catalog = authenticated_catalog(&state, &headers)?;
    require_protocol(request.protocol_version)?;
    require_catalog(&catalog, &request.catalog_id)?;
    let database_path = PathBuf::from(&catalog.database_path);
    let catalog_id = catalog.id.clone();
    let operation = request.operation;
    let reviewed_only = request.reviewed_only;
    let bundle = tokio::task::spawn_blocking(move || {
        export_bundle(&database_path, &catalog_id, operation, reviewed_only)
    })
    .await
    .map_err(|error| AppError::InternalError(format!("export task failed: {error}")))?
    .map_err(|error| AppError::BadRequest(format!("creating export: {error:#}")))?;
    let export = SyncExport {
        export_id: Uuid::new_v4().to_string(),
        state: "ready",
        bundle: Some(bundle),
        error: None,
    };
    let mut exports = exports().lock().map_err(|error| {
        AppError::InternalError(format!("remote export lock poisoned: {error}"))
    })?;
    if exports.len() >= 16
        && let Some(key) = exports.keys().next().cloned()
    {
        exports.remove(&key);
    }
    exports.insert(
        export.export_id.clone(),
        (catalog.id.clone(), export.clone()),
    );
    drop(exports);
    Ok(Json(ApiResponse::success(export)))
}

pub async fn get_export(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(export_id): Path<String>,
) -> Result<Json<ApiResponse<SyncExport>>, AppError> {
    let catalog = authenticated_catalog(&state, &headers)?;
    let exports = exports().lock().map_err(|error| {
        AppError::InternalError(format!("remote export lock poisoned: {error}"))
    })?;
    let export = exports
        .get(&export_id)
        .filter(|(catalog_id, _)| catalog_id == &catalog.id)
        .map(|(_, export)| export.clone())
        .ok_or(AppError::NotFound)?;
    Ok(Json(ApiResponse::success(export)))
}

fn authenticated_catalog(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<Arc<DatabaseContext>, AppError> {
    let header = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| AppError::Forbidden("Bearer token required".into()))?;
    let token = header
        .strip_prefix("Bearer ")
        .map(str::trim)
        .filter(|token| !token.is_empty())
        .ok_or_else(|| AppError::Forbidden("Bearer token required".into()))?;
    let mut matches = state.all_databases().into_iter().filter(|catalog| {
        catalog
            .remote_image_upload
            .as_ref()
            .is_some_and(|config| config.token_is_configured() && config.token_matches(token))
    });
    let catalog = matches
        .next()
        .ok_or_else(|| AppError::Forbidden("Invalid API token".into()))?;
    if matches.next().is_some() {
        return Err(AppError::Forbidden(
            "API token is configured for more than one database".into(),
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
    let digest = bundle
        .payload_sha256
        .as_deref()
        .filter(|digest| digest.len() == 64)
        .ok_or_else(|| AppError::BadRequest("bundle digest is missing".into()))?;
    let mut unsigned = bundle.clone();
    unsigned.payload_sha256 = None;
    let payload = serde_json::to_vec(&unsigned)
        .map_err(|error| AppError::BadRequest(format!("serializing bundle: {error}")))?;
    let actual = digest_hex(&payload);
    if !constant_time_eq(actual.as_bytes(), digest.as_bytes()) {
        return Err(AppError::BadRequest("bundle digest is invalid".into()));
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
    if row_count > MAX_BUNDLE_ROWS {
        return Err(AppError::BadRequest(format!(
            "bundle exceeds the {MAX_BUNDLE_ROWS} row limit"
        )));
    }
    Ok(())
}

fn materialize_bundle(
    path: &FsPath,
    template_path: &FsPath,
    bundle: &CatalogBundle,
) -> anyhow::Result<()> {
    let connection = Connection::open(path)?;
    connection.execute_batch("PRAGMA journal_mode=DELETE; PRAGMA foreign_keys=OFF;")?;
    for table_name in allowed_tables(bundle.operation) {
        if let Some(table) = bundle.tables.get(*table_name) {
            create_bundle_table(&connection, table_name, table)?;
            insert_bundle_rows(&connection, table_name, table)?;
        } else if bundle.operation != SyncOperation::PushGrades {
            create_empty_table_from_template(&connection, template_path, table_name)?;
        }
    }
    connection.pragma_update(None, "user_version", bundle.source.schema_version)?;
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
    template_path: &FsPath,
    name: &str,
) -> anyhow::Result<()> {
    let template = Connection::open_with_flags(template_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
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

fn export_bundle(
    database_path: &FsPath,
    catalog_id: &str,
    operation: SyncOperation,
    reviewed_only: bool,
) -> anyhow::Result<CatalogBundle> {
    let connection = Connection::open_with_flags(database_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let schema_version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    anyhow::ensure!(
        schema_version >= 22,
        "Target Scheduler schema 22 or newer is required"
    );
    let mut tables = BTreeMap::new();
    for name in allowed_tables(operation) {
        let selected = if operation == SyncOperation::PushGrades {
            Some(&["guid", "gradingStatus", "rejectreason"][..])
        } else {
            None
        };
        let where_clause = (operation == SyncOperation::PushGrades && reviewed_only)
            .then_some("gradingStatus <> 0");
        tables.insert(
            (*name).to_string(),
            read_bundle_table(&connection, name, selected, where_clause)?,
        );
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
    selected: Option<&[&str]>,
    where_clause: Option<&str>,
) -> anyhow::Result<BundleTable> {
    let mut schema_statement =
        connection.prepare(&format!("PRAGMA table_info({})", quote_identifier(name)))?;
    let all_columns = schema_statement
        .query_map([], |row| {
            Ok(BundleColumn {
                name: row.get(1)?,
                declared_type: row.get(2)?,
                not_null: row.get::<_, i64>(3)? != 0,
                primary_key: row.get::<_, i64>(5)? != 0,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let columns = if let Some(selected) = selected {
        selected
            .iter()
            .map(|wanted| {
                all_columns
                    .iter()
                    .find(|column| column.name.eq_ignore_ascii_case(wanted))
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("table {name} is missing column {wanted}"))
            })
            .collect::<anyhow::Result<Vec<_>>>()?
    } else {
        all_columns
    };
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

fn scheduler_request(
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

fn result_summary(result: &SchedulerSyncResponse) -> BTreeMap<String, i64> {
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

fn allowed_tables(operation: SyncOperation) -> &'static [&'static str] {
    match operation {
        SyncOperation::Merge => MERGE_TABLES,
        SyncOperation::PushPlanning => PLANNING_TABLES,
        SyncOperation::PushGrades => &["acquiredimage"],
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

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0u8, |difference, (left, right)| difference | (left ^ right))
        == 0
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

fn unix_timestamp(value: i64) -> String {
    DateTime::<Utc>::from_timestamp(value, 0)
        .expect("sync preview timestamps are valid")
        .to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn exports() -> &'static Mutex<HashMap<String, (String, SyncExport)>> {
    EXPORTS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn internal(error: impl std::fmt::Display) -> AppError {
    AppError::InternalError(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_the_plugin_canonical_digest_fixture() {
        let json = r#"{
            "protocol_version":1,
            "bundle_id":"b03b8ab1-ce43-4a87-a4fb-68497394cedb",
            "created_at_utc":"2026-07-23T12:00:00+00:00",
            "operation":"push_grades",
            "source":{
                "id":"source",
                "product":"Target Scheduler",
                "product_version":"5.9.6.0",
                "schema_version":23
            },
            "tables":{},
            "payload_sha256":"5d18d681485f57377c33dffc26dd1b08d79448ec2c819dee08ebdd22ad278d42"
        }"#;
        let bundle: CatalogBundle = serde_json::from_str(json).unwrap();
        validate_bundle(&bundle).unwrap();

        let mut changed = bundle;
        changed.source.product_version = "different".into();
        assert!(validate_bundle(&changed).is_err());
    }
}
