//! Previewed, atomic changes to catalog target and project membership.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use axum::{extract::State, Json};
use rusqlite::{params, types::Value, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::api::ApiResponse;
use super::database_context::{open_scheduler_connection, open_scheduler_connection_with_flags};
use super::extract::DbContext;
use super::handlers::{require_database_management_allowed, AppError};
use super::state::AppState;

const MAX_IMAGES: usize = 50_000;

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum OrganizationOperation {
    MergeTargets {
        source_target_id: i32,
        destination_target_id: i32,
    },
    MoveImages {
        image_ids: Vec<i32>,
        destination: OrganizationDestination,
    },
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OrganizationDestination {
    pub target_id: Option<i32>,
    pub project_id: Option<i32>,
    pub new_project_name: Option<String>,
    pub new_target_name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OrganizationApplyRequest {
    pub operation: OrganizationOperation,
    pub expected_fingerprint: String,
}

#[derive(Debug, Serialize)]
pub struct OrganizationPreview {
    pub fingerprint: String,
    pub source_project_id: i32,
    pub source_project_name: String,
    pub source_target_id: i32,
    pub source_target_name: String,
    pub destination_project_id: Option<i32>,
    pub destination_project_name: String,
    pub destination_target_id: Option<i32>,
    pub destination_target_name: String,
    pub images_moved: usize,
    pub exposure_plans_moved: usize,
    pub exposure_plans_created: usize,
    pub warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct OrganizationResult {
    pub project_id: i32,
    pub target_id: i32,
    pub images_moved: usize,
}

#[derive(Debug, Serialize)]
pub struct OrganizationDestinations {
    pub projects: Vec<OrganizationProject>,
    pub targets: Vec<OrganizationTarget>,
}

#[derive(Debug, Serialize)]
pub struct OrganizationProject {
    pub id: i32,
    pub name: String,
    pub profile_id: String,
}

#[derive(Debug, Serialize)]
pub struct OrganizationTarget {
    pub id: i32,
    pub project_id: i32,
    pub name: String,
}

pub async fn destinations(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
) -> Result<Json<ApiResponse<OrganizationDestinations>>, AppError> {
    require_database_management_allowed(&state)?;
    let path = ctx.database_path.clone();
    let choices = tokio::task::spawn_blocking(move || {
        let mut conn = open_scheduler_connection_with_flags(
            &path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
        )
        .map_err(AppError::db)?;
        let tx = conn.transaction().map_err(AppError::db)?;
        let choices = destination_choices(&tx)?;
        tx.rollback().map_err(AppError::db)?;
        Ok::<_, AppError>(choices)
    })
    .await
    .map_err(|error| AppError::InternalError(format!("organization destinations: {error}")))??;
    Ok(Json(ApiResponse::success(choices)))
}

fn destination_choices(conn: &Connection) -> Result<OrganizationDestinations, AppError> {
    let mut stmt = conn
        .prepare("SELECT Id, name, profileId FROM project ORDER BY name, Id")
        .map_err(AppError::db)?;
    let projects = stmt
        .query_map([], |row| {
            Ok(OrganizationProject {
                id: row.get(0)?,
                name: row.get(1)?,
                profile_id: row.get(2)?,
            })
        })
        .map_err(AppError::db)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(AppError::db)?;
    let mut stmt = conn.prepare("SELECT t.Id, t.projectid, t.name FROM target t JOIN project p ON p.Id = t.projectid ORDER BY t.name, t.Id").map_err(AppError::db)?;
    let targets = stmt
        .query_map([], |row| {
            Ok(OrganizationTarget {
                id: row.get(0)?,
                project_id: row.get(1)?,
                name: row.get(2)?,
            })
        })
        .map_err(AppError::db)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(AppError::db)?;
    Ok(OrganizationDestinations { projects, targets })
}

pub async fn preview(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Json(operation): Json<OrganizationOperation>,
) -> Result<Json<ApiResponse<OrganizationPreview>>, AppError> {
    require_database_management_allowed(&state)?;
    let path = ctx.database_path.clone();
    let preview = tokio::task::spawn_blocking(move || {
        let mut conn = open_scheduler_connection_with_flags(
            &path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
        )
        .map_err(AppError::db)?;
        let tx = conn.transaction().map_err(AppError::db)?;
        let plan = plan_organization(&tx, &operation)?;
        tx.rollback().map_err(AppError::db)?;
        Ok::<_, AppError>(plan.preview)
    })
    .await
    .map_err(|error| AppError::InternalError(format!("organization preview: {error}")))??;
    Ok(Json(ApiResponse::success(preview)))
}

pub async fn apply(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Json(request): Json<OrganizationApplyRequest>,
) -> Result<Json<ApiResponse<OrganizationResult>>, AppError> {
    require_database_management_allowed(&state)?;
    let context = ctx.0.clone();
    let result = tokio::task::spawn_blocking(move || {
        let _organization_guard = context.organization_mutex.lock().map_err(AppError::db)?;
        let mut conn = open_scheduler_connection(&context.database_path).map_err(AppError::db)?;
        let result = apply_organization(&mut conn, &request)?;
        if let Err(error) = context.refresh_organization_navigation(&conn) {
            tracing::warn!(db = %context.id, "Organization succeeded but navigation refresh failed: {error}");
        }
        tracing::info!(db = %context.id, target_id = result.target_id, images_moved = result.images_moved,
            "Applied catalog organization");
        Ok::<_, AppError>(result)
    })
    .await
    .map_err(|error| AppError::InternalError(format!("organization apply: {error}")))??;
    Ok(Json(ApiResponse::success(result)))
}

#[derive(Debug)]
struct TargetIdentity {
    id: i32,
    name: String,
    project_id: i32,
    project_name: String,
    profile_id: String,
}

#[derive(Debug)]
struct ImageMembership {
    id: i32,
    exposure_id: Option<i64>,
}

struct OrganizationPlan {
    preview: OrganizationPreview,
    source: TargetIdentity,
    images: Vec<ImageMembership>,
    source_plan_counts: BTreeMap<i64, usize>,
}

fn invalid(message: impl Into<String>) -> AppError {
    AppError::BadRequest(message.into())
}

fn target_identity(conn: &Connection, target_id: i32) -> Result<TargetIdentity, AppError> {
    conn.query_row(
        "SELECT t.Id, t.name, p.Id, p.name, p.profileId
         FROM target t JOIN project p ON p.Id = t.projectid WHERE t.Id = ?",
        [target_id],
        |row| {
            Ok(TargetIdentity {
                id: row.get(0)?,
                name: row.get(1)?,
                project_id: row.get(2)?,
                project_name: row.get(3)?,
                profile_id: row.get(4)?,
            })
        },
    )
    .optional()
    .map_err(AppError::db)?
    .ok_or_else(|| invalid(format!("target {target_id} not found")))
}

fn project_identity(conn: &Connection, project_id: i32) -> Result<(String, String), AppError> {
    conn.query_row(
        "SELECT name, profileId FROM project WHERE Id = ?",
        [project_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .optional()
    .map_err(AppError::db)?
    .ok_or_else(|| invalid(format!("project {project_id} not found")))
}

fn name(value: &str) -> Result<String, AppError> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > 200 || value.chars().any(char::is_control) {
        return Err(invalid(
            "names must contain 1 to 200 characters without control characters",
        ));
    }
    Ok(value.to_string())
}

fn columns(conn: &Connection, table: &str) -> Result<Vec<String>, AppError> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({})", quote(table)))
        .map_err(AppError::db)?;
    stmt.query_map([], |row| row.get::<_, String>(1))
        .map_err(AppError::db)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(AppError::db)
}

fn has_column(conn: &Connection, table: &str, column: &str) -> Result<bool, AppError> {
    Ok(columns(conn, table)?
        .iter()
        .any(|name| name.eq_ignore_ascii_case(column)))
}

fn quote(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

fn plan_organization(
    conn: &Connection,
    operation: &OrganizationOperation,
) -> Result<OrganizationPlan, AppError> {
    let source_id = match operation {
        OrganizationOperation::MergeTargets {
            source_target_id, ..
        } => *source_target_id,
        OrganizationOperation::MoveImages { image_ids, .. } => {
            if image_ids.is_empty() || image_ids.len() > MAX_IMAGES {
                return Err(invalid(format!("select between 1 and {MAX_IMAGES} images")));
            }
            if image_ids.iter().any(|id| *id <= 0)
                || image_ids.iter().collect::<HashSet<_>>().len() != image_ids.len()
            {
                return Err(invalid("image IDs must be positive and unique"));
            }
            conn.query_row(
                "SELECT targetId FROM acquiredimage WHERE Id = ?",
                [image_ids[0]],
                |row| row.get(0),
            )
            .optional()
            .map_err(AppError::db)?
            .ok_or_else(|| invalid("one or more selected images no longer exist"))?
        }
    };
    let source = target_identity(conn, source_id)?;
    let mut preview = OrganizationPreview {
        fingerprint: String::new(),
        source_project_id: source.project_id,
        source_project_name: source.project_name.clone(),
        source_target_id: source.id,
        source_target_name: source.name.clone(),
        destination_project_id: None,
        destination_project_name: String::new(),
        destination_target_id: None,
        destination_target_name: String::new(),
        images_moved: 0,
        exposure_plans_moved: 0,
        exposure_plans_created: 0,
        warnings: vec!["A later Target Scheduler sync can restore the scheduler's target and project assignments.".into()],
    };
    let destination_target = match operation {
        OrganizationOperation::MergeTargets {
            destination_target_id,
            ..
        } => Some(*destination_target_id),
        OrganizationOperation::MoveImages { destination, .. } => {
            let valid = matches!(
                (
                    &destination.target_id,
                    &destination.project_id,
                    &destination.new_project_name,
                    &destination.new_target_name
                ),
                (Some(_), None, None, None)
                    | (None, Some(_), None, Some(_))
                    | (None, None, Some(_), Some(_))
            );
            if !valid {
                return Err(invalid("choose an existing target, an existing project with a new target name, or a new project and target"));
            }
            if let Some(target_name) = &destination.new_target_name {
                preview.destination_target_name = name(target_name)?;
            }
            if let Some(project_id) = destination.project_id {
                let (project_name, profile_id) = project_identity(conn, project_id)?;
                if profile_id != source.profile_id {
                    return Err(invalid(
                        "cannot organize images across Target Scheduler profiles",
                    ));
                }
                preview.destination_project_id = Some(project_id);
                preview.destination_project_name = project_name;
                let exists: bool = conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM target WHERE projectid = ? AND lower(trim(name)) = lower(?))",
                    params![project_id, preview.destination_target_name], |row| row.get(0),
                ).map_err(AppError::db)?;
                if exists {
                    return Err(invalid("a target with this name already exists in the destination project; choose that target instead"));
                }
            }
            if let Some(project_name) = &destination.new_project_name {
                preview.destination_project_name = name(project_name)?;
                let exists: bool = conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM project WHERE profileId = ? AND lower(trim(name)) = lower(?))",
                    params![source.profile_id, preview.destination_project_name], |row| row.get(0),
                ).map_err(AppError::db)?;
                if exists {
                    return Err(invalid("a project with this name already exists in this profile; choose that project instead"));
                }
            }
            destination.target_id
        }
    };
    if let Some(destination_id) = destination_target {
        if source.id == destination_id {
            return Err(invalid("source and destination targets must differ"));
        }
        let destination = target_identity(conn, destination_id)?;
        if source.profile_id != destination.profile_id {
            return Err(invalid(
                "cannot organize images across Target Scheduler profiles",
            ));
        }
        preview.destination_project_id = Some(destination.project_id);
        preview.destination_project_name = destination.project_name;
        preview.destination_target_id = Some(destination.id);
        preview.destination_target_name = destination.name;
    }

    let has_exposure = has_column(conn, "acquiredimage", "exposureId")?;
    let exposure = if has_exposure { "exposureId" } else { "NULL" };
    let has_profile = has_column(conn, "acquiredimage", "profileId")?;
    let profile = if has_profile { "profileId" } else { "NULL" };
    let mut stmt = conn.prepare(&format!(
        "SELECT Id, projectId, {exposure}, {profile} FROM acquiredimage WHERE targetId = ? ORDER BY Id"
    )).map_err(AppError::db)?;
    let selected: Option<HashSet<i32>> = match operation {
        OrganizationOperation::MoveImages { image_ids, .. } => {
            Some(image_ids.iter().copied().collect())
        }
        _ => None,
    };
    let mut images = Vec::new();
    let mut source_plan_counts = BTreeMap::new();
    let rows = stmt
        .query_map([source.id], |row| {
            Ok((
                row.get::<_, i32>(0)?,
                row.get::<_, i32>(1)?,
                row.get::<_, Option<i64>>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })
        .map_err(AppError::db)?;
    for row in rows {
        let (id, project_id, exposure_id, profile_id) = row.map_err(AppError::db)?;
        if selected.as_ref().is_some_and(|ids| !ids.contains(&id)) {
            continue;
        }
        if project_id != source.project_id
            || profile_id.as_ref().is_some_and(|p| p != &source.profile_id)
        {
            return Err(invalid("selected images have inconsistent project or profile references; repair the catalog before organizing"));
        }
        if let Some(exposure_id) = exposure_id.filter(|id| *id > 0) {
            *source_plan_counts.entry(exposure_id).or_insert(0usize) += 1;
        }
        images.push(ImageMembership { id, exposure_id });
    }
    if selected
        .as_ref()
        .is_some_and(|ids| ids.len() != images.len())
    {
        return Err(invalid(
            "all selected images must still exist and belong to one source target",
        ));
    }
    for plan_id in source_plan_counts.keys() {
        let owner: Option<(i32, String)> = conn
            .query_row(
                "SELECT targetid, profileId FROM exposureplan WHERE Id = ?",
                [plan_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(AppError::db)?;
        if owner != Some((source.id, source.profile_id.clone())) {
            return Err(invalid("selected images refer to missing or inconsistent exposure plans; repair the catalog before organizing"));
        }
    }
    preview.images_moved = images.len();
    match operation {
        OrganizationOperation::MergeTargets { .. } => {
            preview.exposure_plans_moved = count_rows(conn, "exposureplan", "targetid", source.id)?;
            preview.warnings.push("Source exposure plans and flat history move. Source exposure-order overrides are discarded. The destination keeps its explicit order and rebuilds its filter cadence.".into());
        }
        OrganizationOperation::MoveImages { .. } => {
            preview.exposure_plans_created = source_plan_counts.len();
            if !source_plan_counts.is_empty() && !has_column(conn, "exposureplan", "enabled")? {
                return Err(invalid("splitting linked exposures requires Target Scheduler exposure plans with an enabled column; upgrade the scheduler database first"));
            }
            preview.warnings.push("Source acquisition goals stay unchanged. Moved exposures get disabled plans with no acquisition goal; flat history stays with the source.".into());
            if preview.destination_target_id.is_none() {
                preview
                    .warnings
                    .push("The new target copies the source framing and starts inactive.".into());
            }
            if preview.destination_project_id.is_none() {
                preview
                    .warnings
                    .push("The new project copies the source settings and starts inactive.".into());
            }
        }
    }
    preview.fingerprint = fingerprint(conn, operation, &source, &preview)?;
    Ok(OrganizationPlan {
        preview,
        source,
        images,
        source_plan_counts,
    })
}

fn count_rows(conn: &Connection, table: &str, parent: &str, id: i32) -> Result<usize, AppError> {
    if !has_column(conn, table, parent)? {
        return Ok(0);
    }
    conn.query_row(
        &format!(
            "SELECT COUNT(*) FROM {} WHERE {} = ?",
            quote(table),
            quote(parent)
        ),
        [id],
        |row| Ok(row.get::<_, i64>(0)? as usize),
    )
    .map_err(AppError::db)
}

fn fingerprint(
    conn: &Connection,
    operation: &OrganizationOperation,
    source: &TargetIdentity,
    preview: &OrganizationPreview,
) -> Result<String, AppError> {
    let mut digest = Sha256::new();
    digest.update(
        serde_json::to_vec(operation)
            .map_err(|error| AppError::InternalError(error.to_string()))?,
    );
    let destination_target = preview.destination_target_id.unwrap_or(source.id);
    let destination_project = preview.destination_project_id.unwrap_or(source.project_id);
    for (table, parent, ids) in [
        ("project", "Id", [source.project_id, destination_project]),
        ("target", "Id", [source.id, destination_target]),
        ("acquiredimage", "targetId", [source.id, destination_target]),
        ("exposureplan", "targetid", [source.id, destination_target]),
        ("flathistory", "targetId", [source.id, destination_target]),
        (
            "overrideexposureorderitem",
            "targetid",
            [source.id, destination_target],
        ),
        (
            "filtercadenceitem",
            "targetid",
            [source.id, destination_target],
        ),
        (
            "ruleweight",
            "projectid",
            [source.project_id, destination_project],
        ),
    ] {
        if !has_column(conn, table, parent)? {
            continue;
        }
        digest.update(table.as_bytes());
        let selected_columns = if table == "acquiredimage" {
            columns(conn, table)?
                .into_iter()
                .filter(|column| {
                    [
                        "Id",
                        "projectId",
                        "targetId",
                        "gradingStatus",
                        "exposureId",
                        "profileId",
                        "guid",
                        "acquireddate",
                    ]
                    .iter()
                    .any(|field| column.eq_ignore_ascii_case(field))
                })
                .map(|column| quote(&column))
                .collect::<Vec<_>>()
                .join(", ")
        } else {
            "*".to_string()
        };
        let query = format!(
            "SELECT {selected_columns} FROM {} WHERE {} IN (?, ?) ORDER BY Id",
            quote(table),
            quote(parent)
        );
        hash_query(conn, &mut digest, &query, ids)?;
    }
    if has_column(conn, "exposureplan", "exposureTemplateId")? {
        hash_query(conn, &mut digest,
            "SELECT * FROM exposuretemplate WHERE Id IN (SELECT exposureTemplateId FROM exposureplan WHERE targetid IN (?, ?)) ORDER BY Id",
            [source.id, destination_target])?;
    }
    Ok(digest
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn hash_query(
    conn: &Connection,
    digest: &mut Sha256,
    sql: &str,
    ids: [i32; 2],
) -> Result<(), AppError> {
    let mut stmt = conn.prepare(sql).map_err(AppError::db)?;
    let count = stmt.column_count();
    for column in stmt.column_names() {
        digest.update((column.len() as u64).to_le_bytes());
        digest.update(column.as_bytes());
    }
    let mut rows = stmt.query(ids).map_err(AppError::db)?;
    while let Some(row) = rows.next().map_err(AppError::db)? {
        digest.update([0xff]);
        for index in 0..count {
            match row.get_ref(index).map_err(AppError::db)? {
                rusqlite::types::ValueRef::Null => digest.update([0]),
                rusqlite::types::ValueRef::Integer(value) => {
                    digest.update([1]);
                    digest.update(value.to_le_bytes());
                }
                rusqlite::types::ValueRef::Real(value) => {
                    digest.update([2]);
                    digest.update(value.to_le_bytes());
                }
                rusqlite::types::ValueRef::Text(value) => {
                    digest.update([3]);
                    digest.update((value.len() as u64).to_le_bytes());
                    digest.update(value);
                }
                rusqlite::types::ValueRef::Blob(value) => {
                    digest.update([4]);
                    digest.update((value.len() as u64).to_le_bytes());
                    digest.update(value);
                }
            }
        }
    }
    Ok(())
}

fn clone_row(
    conn: &Connection,
    table: &str,
    id: i64,
    overrides: &[(&str, Value)],
) -> Result<i64, AppError> {
    let names: Vec<_> = columns(conn, table)?
        .into_iter()
        .filter(|column| !column.eq_ignore_ascii_case("Id"))
        .collect();
    let mut values = Vec::new();
    let expressions: Vec<_> = names
        .iter()
        .map(|column| {
            if column.eq_ignore_ascii_case("guid") {
                values.push(Value::Text(crate::ts_schema::new_guid()));
                "?".to_string()
            } else if let Some((_, value)) = overrides
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(column))
            {
                values.push(value.clone());
                "?".to_string()
            } else {
                quote(column)
            }
        })
        .collect();
    values.push(Value::Integer(id));
    let sql = format!(
        "INSERT INTO {} ({}) SELECT {} FROM {} WHERE Id = ?",
        quote(table),
        names
            .iter()
            .map(|column| quote(column))
            .collect::<Vec<_>>()
            .join(", "),
        expressions.join(", "),
        quote(table)
    );
    if conn
        .execute(&sql, rusqlite::params_from_iter(values))
        .map_err(AppError::db)?
        != 1
    {
        return Err(invalid(format!("source {table} row disappeared")));
    }
    Ok(conn.last_insert_rowid())
}

fn apply_organization(
    conn: &mut Connection,
    request: &OrganizationApplyRequest,
) -> Result<OrganizationResult, AppError> {
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(AppError::db)?;
    let plan = plan_organization(&tx, &request.operation)?;
    if request.expected_fingerprint.len() != 64
        || request.expected_fingerprint != plan.preview.fingerprint
    {
        return Err(AppError::Conflict("The affected catalog records changed since preview. Preview the organization again before applying.".into()));
    }
    let project_id = match plan.preview.destination_project_id {
        Some(id) => id,
        None => {
            let now = chrono::Utc::now().timestamp();
            let id = clone_row(
                &tx,
                "project",
                i64::from(plan.source.project_id),
                &[
                    (
                        "name",
                        Value::Text(plan.preview.destination_project_name.clone()),
                    ),
                    ("state", Value::Integer(2)),
                    ("createdate", Value::Integer(now)),
                    ("activedate", Value::Null),
                    ("inactivedate", Value::Integer(now)),
                ],
            )?;
            if has_column(&tx, "ruleweight", "projectid")? {
                let mut stmt = tx
                    .prepare("SELECT Id FROM ruleweight WHERE projectid = ? ORDER BY Id")
                    .map_err(AppError::db)?;
                let rules = stmt
                    .query_map([plan.source.project_id], |row| row.get::<_, i64>(0))
                    .map_err(AppError::db)?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(AppError::db)?;
                for rule in rules {
                    clone_row(
                        &tx,
                        "ruleweight",
                        rule,
                        &[("projectid", Value::Integer(id))],
                    )?;
                }
            }
            i32::try_from(id).map_err(|_| invalid("new project ID exceeds supported range"))?
        }
    };
    let target_id = match plan.preview.destination_target_id {
        Some(id) => id,
        None => {
            let id = clone_row(
                &tx,
                "target",
                i64::from(plan.source.id),
                &[
                    (
                        "name",
                        Value::Text(plan.preview.destination_target_name.clone()),
                    ),
                    ("projectid", Value::Integer(i64::from(project_id))),
                    ("active", Value::Integer(0)),
                    ("overrideExposureOrder", Value::Null),
                    ("unusedOEO", Value::Null),
                ],
            )?;
            i32::try_from(id).map_err(|_| invalid("new target ID exceeds supported range"))?
        }
    };
    match &request.operation {
        OrganizationOperation::MergeTargets { .. } => {
            let destination_plans = exposure_plan_ids(&tx, target_id)?;
            tx.execute(
                "UPDATE acquiredimage SET targetId = ?, projectId = ? WHERE targetId = ?",
                params![target_id, project_id, plan.source.id],
            )
            .map_err(AppError::db)?;
            for table in ["exposureplan", "flathistory"] {
                if has_column(&tx, table, "targetid")? {
                    tx.execute(
                        &format!(
                            "UPDATE {} SET targetid = ? WHERE targetid = ?",
                            quote(table)
                        ),
                        params![target_id, plan.source.id],
                    )
                    .map_err(AppError::db)?;
                }
            }
            for table in ["overrideexposureorderitem", "filtercadenceitem"] {
                if has_column(&tx, table, "targetid")? {
                    tx.execute(
                        &format!("DELETE FROM {} WHERE targetid = ?", quote(table)),
                        [plan.source.id],
                    )
                    .map_err(AppError::db)?;
                }
            }
            remap_exposure_order(&tx, target_id, &destination_plans)?;
            tx.execute("DELETE FROM target WHERE Id = ?", [plan.source.id])
                .map_err(AppError::db)?;
        }
        OrganizationOperation::MoveImages { .. } => {
            let mut plan_map = BTreeMap::new();
            for (&source_plan, &moved_count) in &plan.source_plan_counts {
                let destination_plan = clone_row(
                    &tx,
                    "exposureplan",
                    source_plan,
                    &[
                        ("targetid", Value::Integer(i64::from(target_id))),
                        ("desired", Value::Integer(0)),
                        ("acquired", Value::Integer(moved_count as i64)),
                        ("accepted", Value::Integer(0)),
                        ("enabled", Value::Integer(0)),
                    ],
                )?;
                tx.execute("UPDATE exposureplan SET acquired = MAX(0, COALESCE(acquired, 0) - ?) WHERE Id = ?", params![moved_count as i64, source_plan]).map_err(AppError::db)?;
                plan_map.insert(source_plan, destination_plan);
            }
            let has_exposure = has_column(&tx, "acquiredimage", "exposureId")?;
            let sql = if has_exposure {
                "UPDATE acquiredimage SET targetId = ?1, projectId = ?2, exposureId = ?3 WHERE Id = ?4 AND targetId = ?5"
            } else {
                "UPDATE acquiredimage SET targetId = ?1, projectId = ?2 WHERE Id = ?3 AND targetId = ?4"
            };
            let mut stmt = tx.prepare(sql).map_err(AppError::db)?;
            for image in &plan.images {
                let exposure_id = image
                    .exposure_id
                    .map(|id| plan_map.get(&id).copied().unwrap_or(id));
                let changed = if has_exposure {
                    stmt.execute(params![
                        target_id,
                        project_id,
                        exposure_id,
                        image.id,
                        plan.source.id
                    ])
                } else {
                    stmt.execute(params![target_id, project_id, image.id, plan.source.id])
                }
                .map_err(AppError::db)?;
                if changed != 1 {
                    return Err(AppError::Conflict(
                        "A selected image changed while applying the organization.".into(),
                    ));
                }
            }
        }
    }
    if has_column(&tx, "filtercadenceitem", "targetid")? {
        tx.execute(
            "DELETE FROM filtercadenceitem WHERE targetid = ?",
            [target_id],
        )
        .map_err(AppError::db)?;
    }
    if has_column(&tx, "acquiredimage", "exposureId")?
        && has_column(&tx, "exposureplan", "accepted")?
    {
        let mut stmt = tx.prepare("SELECT exposureId, COUNT(*) FROM acquiredimage WHERE targetId IN (?, ?) AND gradingStatus = 1 AND exposureId > 0 GROUP BY exposureId").map_err(AppError::db)?;
        let accepted = stmt
            .query_map(params![plan.source.id, target_id], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(AppError::db)?
            .collect::<Result<BTreeMap<_, _>, _>>()
            .map_err(AppError::db)?;
        let affected_plans = exposure_plan_ids(&tx, plan.source.id)?
            .into_iter()
            .chain(exposure_plan_ids(&tx, target_id)?)
            .collect::<HashSet<_>>();
        let mut update = tx
            .prepare("UPDATE exposureplan SET accepted = ? WHERE Id = ?")
            .map_err(AppError::db)?;
        for plan_id in affected_plans {
            update
                .execute(params![
                    accepted.get(&plan_id).copied().unwrap_or(0),
                    plan_id
                ])
                .map_err(AppError::db)?;
        }
    }
    tx.commit().map_err(AppError::db)?;
    Ok(OrganizationResult {
        project_id,
        target_id,
        images_moved: plan.images.len(),
    })
}

fn exposure_plan_ids(conn: &Connection, target_id: i32) -> Result<Vec<i64>, AppError> {
    if !has_column(conn, "exposureplan", "targetid")? {
        return Ok(Vec::new());
    }
    let mut stmt = conn
        .prepare("SELECT Id FROM exposureplan WHERE targetid = ? ORDER BY Id")
        .map_err(AppError::db)?;
    stmt.query_map([target_id], |row| row.get(0))
        .map_err(AppError::db)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(AppError::db)
}

fn remap_exposure_order(
    conn: &Connection,
    target_id: i32,
    old_plans: &[i64],
) -> Result<(), AppError> {
    if !has_column(conn, "overrideexposureorderitem", "referenceIdx")? {
        return Ok(());
    }
    let new_plans = exposure_plan_ids(conn, target_id)?;
    let mut stmt = conn.prepare("SELECT Id, referenceIdx FROM overrideexposureorderitem WHERE targetid = ? AND referenceIdx >= 0").map_err(AppError::db)?;
    let orders = stmt
        .query_map([target_id], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)? as usize))
        })
        .map_err(AppError::db)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(AppError::db)?;
    for (order_id, old_index) in orders {
        if let Some(plan_id) = old_plans.get(old_index)
            && let Some(new_index) = new_plans.iter().position(|id| id == plan_id)
        {
            conn.execute(
                "UPDATE overrideexposureorderitem SET referenceIdx = ? WHERE Id = ?",
                params![new_index as i64, order_id],
            )
            .map_err(AppError::db)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        crate::ts_schema::apply_schema(&conn).unwrap();
        conn.execute_batch(
            "PRAGMA foreign_keys = ON;
             INSERT INTO project (Id, profileId, name, state, guid) VALUES
                (1, 'p', 'Source project', 1, 'source-project'),
                (2, 'p', 'Destination project', 1, 'destination-project'),
                (3, 'other', 'Other profile', 1, 'other-project');
             INSERT INTO target (Id, name, active, ra, dec, epochcode, rotation, roi, projectid, guid) VALUES
                (1, 'M81 alias', 1, 9.9, 69.1, 2, 30, 100, 1, 'source-target'),
                (3, 'M81', 1, 9.8, 69.2, 2, 45, 80, 2, 'destination-target'),
                (4, 'Other', 1, 5, 5, 2, 0, 100, 3, 'other-target');
             INSERT INTO exposuretemplate (Id, profileId, name, filtername, guid)
                VALUES (1, 'p', 'Luminance', 'L', 'template');
             INSERT INTO exposureplan (Id, profileId, exposure, desired, acquired, accepted, targetid, exposureTemplateId, enabled, guid) VALUES
                (1, 'p', 60, 30, 10, 9, 1, 1, 1, 'source-plan'),
                (10, 'p', 60, 50, 20, 17, 3, 1, 1, 'destination-plan');
             INSERT INTO acquiredimage (Id, projectId, targetId, acquireddate, filtername, gradingStatus, metadata, rejectreason, profileId, exposureId, guid) VALUES
                (1, 1, 1, 1000, 'L', 0, '{\"FileName\":\"source1.fits\"}', NULL, 'p', 1, 'image-1'),
                (2, 1, 1, 2000, 'L', 1, '{\"FileName\":\"source2.fits\"}', NULL, 'p', 1, 'image-2'),
                (3, 1, 1, 3000, 'L', 2, '{\"FileName\":\"source3.fits\"}', 'Cloud', 'p', 1, 'image-3'),
                (4, 2, 3, 4000, 'L', 1, '{\"FileName\":\"destination.fits\"}', NULL, 'p', 10, 'image-4');
             INSERT INTO ruleweight (Id, name, weight, projectid) VALUES (1, 'Meridian', 5, 1);
             INSERT INTO flathistory (Id, targetId, profileId) VALUES (1, 1, 'p'), (2, 3, 'p');
             INSERT INTO overrideexposureorderitem (Id, targetid, [order], action, referenceIdx)
                VALUES (1, 1, 0, 0, 0), (2, 3, 0, 0, 0);
             INSERT INTO filtercadenceitem (Id, targetid, [order], action, referenceIdx)
                VALUES (1, 1, 0, 0, 0), (2, 3, 0, 0, 0);",
        ).unwrap();
        conn
    }

    fn destination(target_id: i32) -> OrganizationDestination {
        OrganizationDestination {
            target_id: Some(target_id),
            project_id: None,
            new_project_name: None,
            new_target_name: None,
        }
    }

    fn preview_and_apply(
        conn: &mut Connection,
        operation: OrganizationOperation,
    ) -> OrganizationResult {
        let expected_fingerprint = plan_organization(conn, &operation)
            .unwrap()
            .preview
            .fingerprint;
        apply_organization(
            conn,
            &OrganizationApplyRequest {
                operation,
                expected_fingerprint,
            },
        )
        .unwrap()
    }

    fn one<T: rusqlite::types::FromSql>(conn: &Connection, sql: &str) -> T {
        conn.query_row(sql, [], |row| row.get(0)).unwrap()
    }

    fn assert_links(conn: &Connection) {
        assert_eq!(one::<i64>(conn, "SELECT COUNT(*) FROM acquiredimage ai LEFT JOIN target t ON t.Id = ai.targetId WHERE t.Id IS NULL OR ai.projectId <> t.projectid"), 0);
        assert_eq!(one::<i64>(conn, "SELECT COUNT(*) FROM acquiredimage ai LEFT JOIN exposureplan ep ON ep.Id = ai.exposureId WHERE ai.exposureId > 0 AND (ep.Id IS NULL OR ep.targetid <> ai.targetId)"), 0);
        assert_eq!(
            one::<i64>(conn, "SELECT COUNT(*) FROM pragma_foreign_key_check"),
            0
        );
    }

    #[test]
    fn merge_preserves_plan_identity_and_destination_order() {
        let mut conn = catalog();
        let operation = OrganizationOperation::MergeTargets {
            source_target_id: 1,
            destination_target_id: 3,
        };
        let plan = plan_organization(&conn, &operation).unwrap();
        assert_eq!(plan.preview.images_moved, 3);
        assert_eq!(plan.preview.exposure_plans_moved, 1);
        assert_eq!(
            one::<i64>(&conn, "SELECT COUNT(*) FROM target WHERE Id = 1"),
            1,
            "preview must not delete the source"
        );
        let result = preview_and_apply(&mut conn, operation);
        assert_eq!(
            (result.project_id, result.target_id, result.images_moved),
            (2, 3, 3)
        );
        assert_eq!(
            one::<i64>(&conn, "SELECT COUNT(*) FROM target WHERE Id = 1"),
            0
        );
        assert_eq!(
            one::<i64>(&conn, "SELECT COUNT(*) FROM project WHERE Id = 1"),
            1
        );
        assert_eq!(
            one::<String>(&conn, "SELECT guid FROM exposureplan WHERE Id = 1"),
            "source-plan"
        );
        assert_eq!(
            one::<i64>(&conn, "SELECT targetid FROM exposureplan WHERE Id = 1"),
            3
        );
        assert_eq!(
            one::<i64>(&conn, "SELECT acquired FROM exposureplan WHERE Id = 1"),
            10
        );
        assert_eq!(
            one::<i64>(&conn, "SELECT desired FROM exposureplan WHERE Id = 1"),
            30
        );
        assert_eq!(
            one::<i64>(&conn, "SELECT SUM(accepted) FROM exposureplan"),
            2
        );
        assert_eq!(
            one::<i64>(
                &conn,
                "SELECT referenceIdx FROM overrideexposureorderitem WHERE Id = 2"
            ),
            1
        );
        assert_eq!(
            one::<i64>(
                &conn,
                "SELECT COUNT(*) FROM overrideexposureorderitem WHERE targetid = 1"
            ),
            0
        );
        assert_eq!(
            one::<i64>(&conn, "SELECT COUNT(*) FROM filtercadenceitem"),
            0
        );
        assert_eq!(
            one::<i64>(&conn, "SELECT COUNT(*) FROM flathistory WHERE targetId = 3"),
            2
        );
        assert_eq!(one::<f64>(&conn, "SELECT ra FROM target WHERE Id = 3"), 9.8);
        assert_eq!(
            one::<String>(&conn, "SELECT rejectreason FROM acquiredimage WHERE Id = 3"),
            "Cloud"
        );
        assert_links(&conn);
    }

    #[test]
    fn selected_move_creates_disabled_plans_and_preserves_records() {
        let mut conn = catalog();
        let metadata: String = one(&conn, "SELECT metadata FROM acquiredimage WHERE Id = 2");
        let result = preview_and_apply(
            &mut conn,
            OrganizationOperation::MoveImages {
                image_ids: vec![1, 2],
                destination: destination(3),
            },
        );
        assert_eq!(result.images_moved, 2);
        assert_eq!(one::<i64>(&conn, "SELECT COUNT(*) FROM exposureplan"), 3);
        let plan_id: i64 = one(&conn, "SELECT exposureId FROM acquiredimage WHERE Id = 2");
        assert_ne!(
            plan_id, 10,
            "matching existing plans keep their separate identity"
        );
        assert_eq!(
            one::<i64>(&conn, "SELECT acquired FROM exposureplan WHERE Id = 1"),
            8
        );
        assert_eq!(
            one::<i64>(&conn, "SELECT desired FROM exposureplan WHERE Id = 1"),
            30
        );
        assert_eq!(
            one::<i64>(&conn, "SELECT accepted FROM exposureplan WHERE Id = 1"),
            0
        );
        let clone: (i64, i64, i64, i64, String) = conn
            .query_row(
                "SELECT desired, acquired, accepted, enabled, guid FROM exposureplan WHERE Id = ?",
                [plan_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!((clone.0, clone.1, clone.2, clone.3), (0, 2, 1, 0));
        assert_ne!(clone.4, "source-plan");
        assert_eq!(
            one::<String>(&conn, "SELECT metadata FROM acquiredimage WHERE Id = 2"),
            metadata
        );
        assert_eq!(
            one::<String>(&conn, "SELECT guid FROM acquiredimage WHERE Id = 2"),
            "image-2"
        );
        assert_eq!(
            one::<i64>(&conn, "SELECT targetId FROM acquiredimage WHERE Id = 3"),
            1
        );
        assert_eq!(
            one::<i64>(&conn, "SELECT targetId FROM flathistory WHERE Id = 1"),
            1
        );
        assert_links(&conn);
    }

    #[test]
    fn new_project_and_target_clone_optional_fields_with_fresh_identity() {
        let mut conn = catalog();
        conn.execute_batch("ALTER TABLE project ADD COLUMN customValue TEXT; UPDATE project SET customValue = 'project extra' WHERE Id = 1; ALTER TABLE target ADD COLUMN customValue TEXT; UPDATE target SET customValue = 'target extra' WHERE Id = 1;").unwrap();
        let result = preview_and_apply(
            &mut conn,
            OrganizationOperation::MoveImages {
                image_ids: vec![2],
                destination: OrganizationDestination {
                    target_id: None,
                    project_id: None,
                    new_project_name: Some("New campaign".into()),
                    new_target_name: Some("New target".into()),
                },
            },
        );
        let project: (String, String, i64, String, String) = conn
            .query_row(
                "SELECT name, profileId, state, guid, customValue FROM project WHERE Id = ?",
                [result.project_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(
            (
                project.0.as_str(),
                project.1.as_str(),
                project.2,
                project.4.as_str()
            ),
            ("New campaign", "p", 2, "project extra")
        );
        assert_ne!(project.3, "source-project");
        let target: (String, f64, f64, i64, String, String) = conn
            .query_row(
                "SELECT name, ra, dec, active, guid, customValue FROM target WHERE Id = ?",
                [result.target_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(
            (
                target.0.as_str(),
                target.1,
                target.2,
                target.3,
                target.5.as_str()
            ),
            ("New target", 9.9, 69.1, 0, "target extra")
        );
        assert_ne!(target.4, "source-target");
        let rules: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM ruleweight WHERE projectid = ?",
                [result.project_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(rules, 1);
        assert_links(&conn);
    }

    #[test]
    fn new_target_in_existing_project_and_null_plan_reference() {
        let mut conn = catalog();
        conn.execute(
            "UPDATE acquiredimage SET exposureId = NULL WHERE Id = 1",
            [],
        )
        .unwrap();
        let result = preview_and_apply(
            &mut conn,
            OrganizationOperation::MoveImages {
                image_ids: vec![1],
                destination: OrganizationDestination {
                    target_id: None,
                    project_id: Some(2),
                    new_project_name: None,
                    new_target_name: Some("Separate frame".into()),
                },
            },
        );
        assert_eq!(result.project_id, 2);
        assert_eq!(one::<i64>(&conn, "SELECT COUNT(*) FROM exposureplan"), 2);
        assert_eq!(
            one::<Option<i64>>(&conn, "SELECT exposureId FROM acquiredimage WHERE Id = 1"),
            None
        );
        assert_links(&conn);
    }

    #[test]
    fn stale_preview_and_invalid_selections_are_atomic() {
        let mut conn = catalog();
        let operation = OrganizationOperation::MoveImages {
            image_ids: vec![1, 2],
            destination: destination(3),
        };
        let fingerprint = plan_organization(&conn, &operation)
            .unwrap()
            .preview
            .fingerprint;
        conn.execute(
            "UPDATE acquiredimage SET gradingStatus = 2 WHERE Id = 2",
            [],
        )
        .unwrap();
        assert!(matches!(
            apply_organization(
                &mut conn,
                &OrganizationApplyRequest {
                    operation,
                    expected_fingerprint: fingerprint
                }
            ),
            Err(AppError::Conflict(_))
        ));
        assert_eq!(
            one::<i64>(
                &conn,
                "SELECT COUNT(*) FROM acquiredimage WHERE targetId = 1"
            ),
            3
        );
        assert_eq!(one::<i64>(&conn, "SELECT COUNT(*) FROM exposureplan"), 2);
        for image_ids in [vec![], vec![1, 1], vec![1, 999], vec![1, 4]] {
            assert!(matches!(
                plan_organization(
                    &conn,
                    &OrganizationOperation::MoveImages {
                        image_ids,
                        destination: destination(3)
                    }
                ),
                Err(AppError::BadRequest(_))
            ));
        }
        assert!(matches!(
            plan_organization(
                &conn,
                &OrganizationOperation::MergeTargets {
                    source_target_id: 1,
                    destination_target_id: 1
                }
            ),
            Err(AppError::BadRequest(_))
        ));
        assert!(matches!(
            plan_organization(
                &conn,
                &OrganizationOperation::MergeTargets {
                    source_target_id: 1,
                    destination_target_id: 4
                }
            ),
            Err(AppError::BadRequest(_))
        ));
        assert_links(&conn);
    }

    #[test]
    fn destination_planning_changes_invalidate_preview_but_image_metadata_does_not() {
        let mut conn = catalog();
        let operation = OrganizationOperation::MergeTargets {
            source_target_id: 1,
            destination_target_id: 3,
        };
        let before = plan_organization(&conn, &operation)
            .unwrap()
            .preview
            .fingerprint;
        conn.execute(
            "UPDATE acquiredimage SET metadata = ? WHERE Id = 1",
            ["{\"FileName\":\"updated.fits\"}"],
        )
        .unwrap();
        assert_eq!(
            plan_organization(&conn, &operation)
                .unwrap()
                .preview
                .fingerprint,
            before
        );
        conn.execute("UPDATE exposureplan SET desired = 60 WHERE Id = 10", [])
            .unwrap();
        assert!(matches!(
            apply_organization(
                &mut conn,
                &OrganizationApplyRequest {
                    operation,
                    expected_fingerprint: before
                }
            ),
            Err(AppError::Conflict(_))
        ));
    }

    #[test]
    fn bad_destination_and_orphan_plan_are_rejected_before_writes() {
        let conn = catalog();
        for destination in [
            OrganizationDestination {
                target_id: Some(3),
                project_id: Some(2),
                new_project_name: None,
                new_target_name: None,
            },
            OrganizationDestination {
                target_id: None,
                project_id: Some(2),
                new_project_name: None,
                new_target_name: Some("M81".into()),
            },
            OrganizationDestination {
                target_id: None,
                project_id: Some(2),
                new_project_name: None,
                new_target_name: Some("  ".into()),
            },
            OrganizationDestination {
                target_id: None,
                project_id: None,
                new_project_name: Some("Source project".into()),
                new_target_name: Some("Other".into()),
            },
        ] {
            assert!(matches!(
                plan_organization(
                    &conn,
                    &OrganizationOperation::MoveImages {
                        image_ids: vec![1],
                        destination
                    }
                ),
                Err(AppError::BadRequest(_))
            ));
        }
        conn.execute("UPDATE acquiredimage SET exposureId = 999 WHERE Id = 1", [])
            .unwrap();
        assert!(matches!(
            plan_organization(
                &conn,
                &OrganizationOperation::MoveImages {
                    image_ids: vec![1],
                    destination: destination(3)
                }
            ),
            Err(AppError::BadRequest(_))
        ));
        assert_eq!(one::<i64>(&conn, "SELECT COUNT(*) FROM target"), 3);
        assert_eq!(one::<i64>(&conn, "SELECT COUNT(*) FROM project"), 3);
    }

    #[test]
    fn destinations_include_empty_projects_and_targets() {
        let conn = catalog();
        conn.execute("INSERT INTO project (Id, profileId, name, state, guid) VALUES (20, 'p', 'Empty project', 2, 'empty-project')", []).unwrap();
        let choices = destination_choices(&conn).unwrap();
        assert!(choices.projects.iter().any(|project| project.id == 20));
        assert!(choices.targets.iter().any(|target| target.id == 4));
    }
}
