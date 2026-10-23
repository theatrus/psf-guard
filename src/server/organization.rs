//! Previewed, atomic changes to catalog target and project membership.

use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use axum::{extract::State, Json};
use rusqlite::{params, types::Value, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::api::ApiResponse;
use super::database_context::{
    open_scheduler_connection, open_scheduler_connection_with_flags, DatabaseContext,
};
use super::extract::DbContext;
use super::handlers::AppError;
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
    /// Move a whole target, with its frames, exposure plans and stack
    /// previews, to another project.
    MoveTarget {
        target_id: i32,
        destination: TargetMoveDestination,
    },
}

/// Where a moved target goes: an existing project of any profile, or a new
/// project in the target's own profile. Exactly one field is set.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TargetMoveDestination {
    #[serde(default)]
    pub project_id: Option<i32>,
    #[serde(default)]
    pub new_project_name: Option<String>,
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
    /// Set when a moved target changes Target Scheduler profile.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_change: Option<OrganizationProfileChange>,
    /// Exposure templates of the destination profile that a moved target's
    /// exposure plans will use as they are.
    pub exposure_templates_reused: usize,
    /// Exposure templates copied into the destination profile for a moved
    /// target's exposure plans.
    pub exposure_templates_copied: usize,
    /// For a target that changes profile: each exposure template its plans
    /// use, and the destination profile's template they will use instead.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub exposure_template_changes: Vec<OrganizationTemplateChange>,
    /// The Planning panel a moved target was activated as, when a plan
    /// manages it. The move ends that management.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub planning_panel_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OrganizationProfileChange {
    pub from_profile_id: String,
    pub to_profile_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct OrganizationTemplateChange {
    pub source_template_name: String,
    pub destination_template_name: String,
    /// True when the destination profile already has a matching template;
    /// false when the move copies the source template there.
    pub reused: bool,
}

#[derive(Debug, Serialize)]
pub struct OrganizationResult {
    /// The project the source target was in before the change.
    pub source_project_id: i32,
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
    /// Target Scheduler state: 0 Draft, 1 Active, 2 Inactive, 3 Closed.
    pub state: Option<i32>,
}

#[derive(Debug, Serialize)]
pub struct OrganizationTarget {
    pub id: i32,
    pub project_id: i32,
    pub name: String,
}

pub async fn destinations(
    ctx: DbContext,
) -> Result<Json<ApiResponse<OrganizationDestinations>>, AppError> {
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
    let state = if has_column(conn, "project", "state")? {
        "state"
    } else {
        "NULL"
    };
    let mut stmt = conn
        .prepare(&format!(
            "SELECT Id, name, profileId, {state} FROM project ORDER BY name, Id"
        ))
        .map_err(AppError::db)?;
    let projects = stmt
        .query_map([], |row| {
            Ok(OrganizationProject {
                id: row.get(0)?,
                name: row.get(1)?,
                profile_id: row.get(2)?,
                state: row.get(3)?,
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
    ctx: DbContext,
    Json(operation): Json<OrganizationOperation>,
) -> Result<Json<ApiResponse<OrganizationPreview>>, AppError> {
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
    let context = ctx.0.clone();
    let result = tokio::task::spawn_blocking(move || {
        let _organization_guard = context.organization_mutex.lock().map_err(AppError::db)?;
        let mut conn = open_scheduler_connection(&context.database_path).map_err(AppError::db)?;
        let result = apply_organization(&mut conn, &request)?;
        after_apply(&context, &conn, &request.operation, &result);
        Ok::<_, AppError>(result)
    })
    .await
    .map_err(|error| AppError::InternalError(format!("organization apply: {error}")))??;
    // Frames moved between targets or projects: the cards of both sides
    // are out of date.
    state.auto_stacks.touch_database(
        &ctx.id,
        crate::server::stack_preview::automatic::RefreshReason::Sync,
    );
    Ok(Json(ApiResponse::success(result)))
}

/// Move a target to another project without a separate preview, for
/// `PUT /targets/{id}` with a `project_id`. `edit` makes the request's other
/// changes, such as a new name, first. The edit, the checks of a previewed
/// move and the move run in one transaction, so a refused move changes
/// nothing. The caller refreshes the automatic stacks.
pub(crate) fn move_target_to_project(
    context: &DatabaseContext,
    target_id: i32,
    project_id: i32,
    edit: impl FnOnce(&Connection) -> Result<(), AppError>,
) -> Result<OrganizationResult, AppError> {
    let _organization_guard = context.organization_mutex.lock().map_err(AppError::db)?;
    let mut conn = open_scheduler_connection(&context.database_path).map_err(AppError::db)?;
    let operation = OrganizationOperation::MoveTarget {
        target_id,
        destination: TargetMoveDestination {
            project_id: Some(project_id),
            new_project_name: None,
        },
    };
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(AppError::db)?;
    edit(&tx)?;
    let plan = plan_organization(&tx, &operation)?;
    let result = apply_plan(&tx, &operation, &plan)?;
    tx.commit().map_err(AppError::db)?;
    after_apply(context, &conn, &operation, &result);
    Ok(result)
}

/// Bring what lives outside the catalog up to date after a change: the
/// Library navigation, and a moved target's stack previews.
fn after_apply(
    context: &DatabaseContext,
    conn: &Connection,
    operation: &OrganizationOperation,
    result: &OrganizationResult,
) {
    if let Err(error) = context.refresh_organization_navigation(conn) {
        tracing::warn!(db = %context.id, "Organization succeeded but navigation refresh failed: {error}");
    }
    if matches!(operation, OrganizationOperation::MoveTarget { .. })
        && result.source_project_id != result.project_id
    {
        let moved = crate::server::stack_preview::move_target_stacks(
            context,
            result.target_id,
            result.source_project_id,
            result.project_id,
        );
        tracing::info!(db = %context.id, target_id = result.target_id, stack_entries = moved,
            "Moved a target's stack previews with it");
    }
    tracing::info!(db = %context.id, target_id = result.target_id, images_moved = result.images_moved,
        "Applied catalog organization");
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
    /// For a target moving to another profile: which exposure template each
    /// of its exposure plans uses there.
    template_moves: Vec<TemplateMove>,
    destination_profile_id: String,
    /// A moved target's GUID, when it has one.
    target_guid: Option<String>,
}

/// One source exposure template of a target that changes profile, and the
/// destination profile's matching template, if there is one. Without a
/// match, the move copies the source template into the destination profile.
#[derive(Debug, Clone, PartialEq)]
struct TemplateMove {
    source_template_id: i64,
    source_name: String,
    destination_template_id: Option<i64>,
    destination_name: Option<String>,
    exposure_plan_ids: Vec<i64>,
    /// The source template's default exposure, used by its plans whose own
    /// exposure is blank or not positive.
    source_default_exposure: Option<f64>,
    /// The plans of this template that use the template's default exposure.
    default_exposure_plan_ids: Vec<i64>,
}

const DIRECTOR_TARGET_TABLE: &str = "psf_guard_director_target";
const DIRECTOR_PLAN_TABLE: &str = "psf_guard_director_plan";
const COLLABORATION_PLAN_TABLE: &str = "psf_guard_collaboration_plan";
const REMOVED_IMAGE_TABLE: &str = "psf_guard_removed_image";

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

/// A profile ID shortened to its first eight characters, enough to tell
/// profiles apart in a message.
fn profile_tag(profile_id: &str) -> String {
    profile_id.chars().take(8).collect()
}

fn has_table(conn: &Connection, table: &str) -> Result<bool, AppError> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?)",
        [table],
        |row| row.get(0),
    )
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
        OrganizationOperation::MoveTarget { target_id, .. } => *target_id,
    };
    let source = target_identity(conn, source_id)?;
    let mut destination_profile_id = source.profile_id.clone();
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
        profile_change: None,
        exposure_templates_reused: 0,
        exposure_templates_copied: 0,
        exposure_template_changes: Vec::new(),
        planning_panel_id: None,
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
        OrganizationOperation::MoveTarget { destination, .. } => {
            match (&destination.project_id, &destination.new_project_name) {
                (Some(project_id), None) => {
                    if *project_id == source.project_id {
                        return Err(invalid(format!(
                            "{} is already in {}; choose another project",
                            source.name, source.project_name
                        )));
                    }
                    let (project_name, profile_id) = project_identity(conn, *project_id)?;
                    preview.destination_project_id = Some(*project_id);
                    preview.destination_project_name = project_name;
                    destination_profile_id = profile_id;
                }
                (None, Some(project_name)) => {
                    preview.destination_project_name = name(project_name)?;
                    let exists: bool = conn.query_row(
                        "SELECT EXISTS(SELECT 1 FROM project WHERE profileId = ? AND lower(trim(name)) = lower(?))",
                        params![source.profile_id, preview.destination_project_name], |row| row.get(0),
                    ).map_err(AppError::db)?;
                    if exists {
                        return Err(invalid("a project with this name already exists in this profile; choose that project instead"));
                    }
                }
                _ => return Err(invalid("choose an existing project or a new project name")),
            }
            if let Some(project_id) = preview.destination_project_id {
                let clash: bool = conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM target WHERE projectid = ? AND Id <> ? AND lower(trim(name)) = lower(trim(?)))",
                    params![project_id, source.id, source.name], |row| row.get(0),
                ).map_err(AppError::db)?;
                if clash {
                    return Err(invalid(format!(
                        "{} already has a target named {}. To combine the two targets, use Merge target instead.",
                        preview.destination_project_name, source.name.trim()
                    )));
                }
            }
            preview.destination_target_id = Some(source.id);
            preview.destination_target_name = source.name.clone();
            None
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
    let mut template_moves = Vec::new();
    let mut target_guid = None;
    let mut evidence = String::new();
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
        OrganizationOperation::MoveTarget { .. } => {
            let target_move = plan_target_move(
                conn,
                &source,
                images.len(),
                &destination_profile_id,
                &mut preview,
            )?;
            template_moves = target_move.template_moves;
            target_guid = target_move.target_guid;
            evidence = target_move.evidence;
        }
    }
    let extra = format!(
        "{destination_profile_id}\u{0}{template_moves:?}\u{0}{:?}\u{0}{evidence}",
        preview.planning_panel_id
    );
    preview.fingerprint = fingerprint(conn, operation, &source, &preview, extra.as_bytes())?;
    Ok(OrganizationPlan {
        preview,
        source,
        images,
        source_plan_counts,
        template_moves,
        destination_profile_id,
        target_guid,
    })
}

/// What a target move does besides moving the target and its exposures.
struct TargetMovePlan {
    template_moves: Vec<TemplateMove>,
    target_guid: Option<String>,
    /// Counts the preview depends on, for the fingerprint.
    evidence: String,
}

fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// Check a target move and describe it in `preview`: the profile rule, the
/// exposure templates a profile change maps, and what the move ends in
/// Planning and collaboration.
fn plan_target_move(
    conn: &Connection,
    source: &TargetIdentity,
    exposures: usize,
    destination_profile_id: &str,
    preview: &mut OrganizationPreview,
) -> Result<TargetMovePlan, AppError> {
    preview.exposure_plans_moved = count_rows(conn, "exposureplan", "targetid", source.id)?;
    let removed_rejects = restorable_removed_rejects(conn, source.id)?;
    let mut template_moves = Vec::new();
    if destination_profile_id != source.profile_id {
        let total = exposures + removed_rejects;
        if total > 0 {
            let removed = if removed_rejects == 0 {
                String::new()
            } else {
                format!(
                    ", including {} you can still restore",
                    plural(removed_rejects, "removed reject", "removed rejects")
                )
            };
            return Err(invalid(format!(
                "{} has {}{removed}, captured with Target Scheduler profile {}. {} belongs to profile {}. \
                 A target with exposures can move only to a project of its own profile, because each exposure's \
                 profile selects its calibration frames and nights. To move this target, first move its exposures \
                 to another target of profile {} with Move exposures.",
                source.name,
                plural(total, "exposure", "exposures"),
                profile_tag(&source.profile_id),
                preview.destination_project_name,
                profile_tag(destination_profile_id),
                profile_tag(&source.profile_id),
            )));
        }
        template_moves = plan_template_moves(conn, source.id, destination_profile_id)?;
        preview.exposure_templates_reused = template_moves
            .iter()
            .filter(|template| template.destination_template_id.is_some())
            .count();
        preview.exposure_templates_copied =
            template_moves.len() - preview.exposure_templates_reused;
        preview.exposure_template_changes = template_moves
            .iter()
            .map(|template| OrganizationTemplateChange {
                source_template_name: template.source_name.clone(),
                destination_template_name: template
                    .destination_name
                    .clone()
                    .unwrap_or_else(|| template.source_name.clone()),
                reused: template.destination_template_id.is_some(),
            })
            .collect();
        preview.profile_change = Some(OrganizationProfileChange {
            from_profile_id: source.profile_id.clone(),
            to_profile_id: destination_profile_id.to_string(),
        });
        preview.warnings.push(format!(
            "The target moves from profile {} to profile {}. Its exposure plans use exposure templates of profile {}: \
             matching templates are reused, and the others are copied.",
            profile_tag(&source.profile_id),
            profile_tag(destination_profile_id),
            profile_tag(destination_profile_id)
        ));
        let defaulted: usize = template_moves
            .iter()
            .filter(|template| {
                template.destination_template_id.is_some()
                    && template.source_default_exposure.is_some()
            })
            .map(|template| template.default_exposure_plan_ids.len())
            .sum();
        if defaulted > 0 {
            preview.warnings.push(format!(
                "{} the default exposure of {}. The move writes that exposure into {}, so it stays the same \
                 with the destination profile's template.",
                plural(defaulted, "exposure plan uses", "exposure plans use"),
                if defaulted == 1 { "its template" } else { "their templates" },
                if defaulted == 1 { "the plan" } else { "each plan" },
            ));
        }
    }
    preview.warnings.push(
        "The target keeps its name, framing, exposure plans, flat history and exposure order. \
         Its exposures and stack previews move with it."
            .into(),
    );
    if removed_rejects > 0 {
        preview.warnings.push(format!(
            "This target has {}. Restoring {} returns {} to {}.",
            plural(removed_rejects, "removed reject", "removed rejects"),
            if removed_rejects == 1 { "it" } else { "them" },
            if removed_rejects == 1 { "it" } else { "them" },
            preview.destination_project_name
        ));
    }
    let destination_has_exposures = match preview.destination_project_id {
        Some(project_id) => conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM acquiredimage WHERE projectId = ?)",
                [project_id],
                |row| row.get::<_, bool>(0),
            )
            .map_err(AppError::db)?,
        None => false,
    };
    if preview.destination_project_id.is_none() {
        preview.warnings.push(format!(
            "The new project copies the settings of {}, including its stack settings, and starts inactive.",
            source.project_name
        ));
    }
    if exposures == 0 && !destination_has_exposures {
        preview.warnings.push(format!(
            "{} has no exposures, and the Library lists only projects with exposures. With Planning on, {} \
             appears under Plans with nothing captured yet. Otherwise it appears once an exposure arrives.",
            preview.destination_project_name, preview.destination_project_name
        ));
    }
    let target_guid = if has_column(conn, "target", "guid")? {
        conn.query_row("SELECT guid FROM target WHERE Id = ?", [source.id], |row| {
            row.get::<_, Option<String>>(0)
        })
        .map_err(AppError::db)?
        .filter(|guid| !guid.is_empty())
    } else {
        None
    };
    let mut open_assignments = 0usize;
    if let Some(guid) = &target_guid {
        if has_table(conn, DIRECTOR_TARGET_TABLE)? {
            preview.planning_panel_id = conn
                .query_row(
                    "SELECT panel_id FROM psf_guard_director_target WHERE target_guid = ?",
                    [guid],
                    |row| row.get(0),
                )
                .optional()
                .map_err(AppError::db)?;
        }
        if has_table(conn, COLLABORATION_PLAN_TABLE)? {
            open_assignments = conn
                .query_row(
                    "SELECT COUNT(*) FROM psf_guard_collaboration_plan
                     WHERE target_guid = ? COLLATE NOCASE AND end_at_ms IS NULL",
                    [guid],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(AppError::db)? as usize;
        }
    }
    if preview.planning_panel_id.is_some() {
        preview.warnings.push(format!(
            "A Planning plan activated this target as one of its panels. After the move, the plan no longer \
             manages the target, and the plan's next activation can add a new target for that panel to {}.",
            source.project_name
        ));
    }
    if open_assignments > 0 {
        preview.warnings.push(
            "The target is part of an open collaboration assignment. The move ends the target's part in it; \
             the assignment's other targets are not changed."
                .into(),
        );
    }
    Ok(TargetMovePlan {
        template_moves,
        target_guid,
        evidence: format!(
            "{removed_rejects}\u{0}{open_assignments}\u{0}{destination_has_exposures}"
        ),
    })
}

/// Removed rejects of a target that Restore can still bring back: their
/// saved rows have not been purged.
fn restorable_removed_rejects(conn: &Connection, target_id: i32) -> Result<usize, AppError> {
    if !has_column(conn, REMOVED_IMAGE_TABLE, "target_id")? {
        return Ok(0);
    }
    let purged = if has_column(conn, REMOVED_IMAGE_TABLE, "purged_at")? {
        " AND purged_at IS NULL"
    } else {
        ""
    };
    conn.query_row(
        &format!("SELECT COUNT(*) FROM psf_guard_removed_image WHERE target_id = ?{purged}"),
        [target_id],
        |row| row.get::<_, i64>(0),
    )
    .map(|count| count as usize)
    .map_err(AppError::db)
}

/// For a target that moves to another profile: each exposure template its
/// exposure plans use, and the destination profile's template with the same
/// filter, gain, offset, binning and readout mode, if there is one. Blank
/// values match blank values, and a blank binning matches binning 1, as when
/// an exposure plan is added.
fn plan_template_moves(
    conn: &Connection,
    target_id: i32,
    destination_profile_id: &str,
) -> Result<Vec<TemplateMove>, AppError> {
    if !has_column(conn, "exposureplan", "exposureTemplateId")? {
        return Ok(Vec::new());
    }
    let mut stmt = conn
        .prepare(
            "SELECT Id, exposureTemplateId FROM exposureplan
             WHERE targetid = ? AND exposureTemplateId IS NOT NULL ORDER BY Id",
        )
        .map_err(AppError::db)?;
    let mut plans_by_template: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
    for row in stmt
        .query_map([target_id], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(AppError::db)?
    {
        let (plan_id, template_id) = row.map_err(AppError::db)?;
        plans_by_template
            .entry(template_id)
            .or_default()
            .push(plan_id);
    }
    // Compare only the settings this database's templates record.
    let mut compared = vec![("filtername", "filtername IS ?")];
    for (column, condition) in [
        ("gain", "gain IS ?"),
        ("offset", "offset IS ?"),
        ("bin", "IFNULL(bin, 1) = IFNULL(?, 1)"),
        ("readoutmode", "readoutmode IS ?"),
    ] {
        if has_column(conn, "exposuretemplate", column)? {
            compared.push((column, condition));
        }
    }
    let settings_sql = format!(
        "SELECT {} FROM exposuretemplate WHERE Id = ?",
        compared
            .iter()
            .map(|(column, _)| quote(column))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let match_sql = format!(
        "SELECT Id FROM exposuretemplate WHERE profileId = ? AND {} ORDER BY Id LIMIT 1",
        compared
            .iter()
            .map(|(_, condition)| *condition)
            .collect::<Vec<_>>()
            .join(" AND ")
    );
    let has_default = has_column(conn, "exposuretemplate", "defaultexposure")?;
    let mut moves = Vec::new();
    for (source_template_id, exposure_plan_ids) in plans_by_template {
        let settings: Vec<Value> = conn
            .query_row(&settings_sql, [source_template_id], |row| {
                (0..compared.len())
                    .map(|index| row.get::<_, Value>(index))
                    .collect()
            })
            .optional()
            .map_err(AppError::db)?
            .ok_or_else(|| {
                invalid(format!(
                    "an exposure plan of this target refers to exposure template {source_template_id}, \
                     which no longer exists; repair the catalog before moving the target"
                ))
            })?;
        let mut values = vec![Value::Text(destination_profile_id.to_string())];
        values.extend(settings);
        let destination_template_id = conn
            .query_row(&match_sql, rusqlite::params_from_iter(values), |row| {
                row.get::<_, i64>(0)
            })
            .optional()
            .map_err(AppError::db)?;
        let template_name = |id: i64| -> Result<String, AppError> {
            conn.query_row(
                "SELECT name FROM exposuretemplate WHERE Id = ?",
                [id],
                |row| row.get::<_, Option<String>>(0),
            )
            .map(|name| name.unwrap_or_default())
            .map_err(AppError::db)
        };
        let source_default_exposure = if has_default {
            conn.query_row(
                "SELECT defaultexposure FROM exposuretemplate WHERE Id = ?",
                [source_template_id],
                |row| row.get::<_, Option<f64>>(0),
            )
            .map_err(AppError::db)?
        } else {
            None
        };
        let mut default_exposure_plan_ids = Vec::new();
        for plan_id in &exposure_plan_ids {
            let uses_default: bool = conn
                .query_row(
                    "SELECT exposure IS NULL OR exposure <= 0 FROM exposureplan WHERE Id = ?",
                    [plan_id],
                    |row| row.get(0),
                )
                .map_err(AppError::db)?;
            if uses_default {
                default_exposure_plan_ids.push(*plan_id);
            }
        }
        moves.push(TemplateMove {
            source_template_id,
            source_name: template_name(source_template_id)?,
            destination_template_id,
            destination_name: destination_template_id.map(template_name).transpose()?,
            exposure_plan_ids,
            source_default_exposure,
            default_exposure_plan_ids,
        });
    }
    Ok(moves)
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
    extra: &[u8],
) -> Result<String, AppError> {
    let mut digest = Sha256::new();
    digest.update(
        serde_json::to_vec(operation)
            .map_err(|error| AppError::InternalError(error.to_string()))?,
    );
    digest.update(extra);
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
        // Grades do not change what a target move does.
        let moving_target = matches!(operation, OrganizationOperation::MoveTarget { .. });
        let selected_columns = if table == "acquiredimage" {
            columns(conn, table)?
                .into_iter()
                .filter(|column| !(moving_target && column.eq_ignore_ascii_case("gradingStatus")))
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
    let result = apply_plan(&tx, &request.operation, &plan)?;
    tx.commit().map_err(AppError::db)?;
    Ok(result)
}

/// Write a checked plan. The caller holds the transaction and commits it.
fn apply_plan(
    tx: &Connection,
    operation: &OrganizationOperation,
    plan: &OrganizationPlan,
) -> Result<OrganizationResult, AppError> {
    let project_id = match plan.preview.destination_project_id {
        Some(id) => id,
        None => {
            let now = chrono::Utc::now().timestamp();
            let id = clone_row(
                tx,
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
            let new_project =
                i32::try_from(id).map_err(|_| invalid("new project ID exceeds supported range"))?;
            crate::server::exposure_groups::copy_project_settings(
                tx,
                plan.source.project_id,
                new_project,
            )?;
            if has_column(tx, "ruleweight", "projectid")? {
                let mut stmt = tx
                    .prepare("SELECT Id FROM ruleweight WHERE projectid = ? ORDER BY Id")
                    .map_err(AppError::db)?;
                let rules = stmt
                    .query_map([plan.source.project_id], |row| row.get::<_, i64>(0))
                    .map_err(AppError::db)?
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(AppError::db)?;
                for rule in rules {
                    clone_row(tx, "ruleweight", rule, &[("projectid", Value::Integer(id))])?;
                }
            }
            i32::try_from(id).map_err(|_| invalid("new project ID exceeds supported range"))?
        }
    };
    let target_id = match plan.preview.destination_target_id {
        Some(id) => id,
        None => {
            let id = clone_row(
                tx,
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
    match operation {
        OrganizationOperation::MergeTargets { .. } => {
            let destination_plans = exposure_plan_ids(tx, target_id)?;
            tx.execute(
                "UPDATE acquiredimage SET targetId = ?, projectId = ? WHERE targetId = ?",
                params![target_id, project_id, plan.source.id],
            )
            .map_err(AppError::db)?;
            for table in ["exposureplan", "flathistory"] {
                if has_column(tx, table, "targetid")? {
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
                if has_column(tx, table, "targetid")? {
                    tx.execute(
                        &format!("DELETE FROM {} WHERE targetid = ?", quote(table)),
                        [plan.source.id],
                    )
                    .map_err(AppError::db)?;
                }
            }
            remap_exposure_order(tx, target_id, &destination_plans)?;
            tx.execute("DELETE FROM target WHERE Id = ?", [plan.source.id])
                .map_err(AppError::db)?;
        }
        OrganizationOperation::MoveImages { .. } => {
            let mut plan_map = BTreeMap::new();
            for (&source_plan, &moved_count) in &plan.source_plan_counts {
                let destination_plan = clone_row(
                    tx,
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
            let has_exposure = has_column(tx, "acquiredimage", "exposureId")?;
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
        OrganizationOperation::MoveTarget { .. } => {
            move_target(tx, plan, project_id)?;
            return Ok(OrganizationResult {
                source_project_id: plan.source.project_id,
                project_id,
                target_id,
                images_moved: plan.images.len(),
            });
        }
    }
    if has_column(tx, "filtercadenceitem", "targetid")? {
        tx.execute(
            "DELETE FROM filtercadenceitem WHERE targetid = ?",
            [target_id],
        )
        .map_err(AppError::db)?;
    }
    if has_column(tx, "acquiredimage", "exposureId")? && has_column(tx, "exposureplan", "accepted")?
    {
        let mut stmt = tx.prepare("SELECT exposureId, COUNT(*) FROM acquiredimage WHERE targetId IN (?, ?) AND gradingStatus = 1 AND exposureId > 0 GROUP BY exposureId").map_err(AppError::db)?;
        let accepted = stmt
            .query_map(params![plan.source.id, target_id], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
            })
            .map_err(AppError::db)?
            .collect::<Result<BTreeMap<_, _>, _>>()
            .map_err(AppError::db)?;
        let affected_plans = exposure_plan_ids(tx, plan.source.id)?
            .into_iter()
            .chain(exposure_plan_ids(tx, target_id)?)
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
    Ok(OrganizationResult {
        source_project_id: plan.source.project_id,
        project_id,
        target_id,
        images_moved: plan.images.len(),
    })
}

/// Move a target and its exposures to `project_id`. A target that changes
/// profile takes its exposure plans and flat history along, each plan on a
/// template of the destination profile. Removed rejects record the new
/// project. Planning's records of the target as a panel are deleted, and its
/// open collaboration assignments end.
fn move_target(
    conn: &Connection,
    plan: &OrganizationPlan,
    project_id: i32,
) -> Result<(), AppError> {
    let target_id = plan.source.id;
    conn.execute(
        "UPDATE target SET projectid = ? WHERE Id = ?",
        params![project_id, target_id],
    )
    .map_err(AppError::db)?;
    conn.execute(
        "UPDATE acquiredimage SET projectId = ? WHERE targetId = ?",
        params![project_id, target_id],
    )
    .map_err(AppError::db)?;
    if has_column(conn, REMOVED_IMAGE_TABLE, "target_id")?
        && has_column(conn, REMOVED_IMAGE_TABLE, "project_id")?
    {
        conn.execute(
            "UPDATE psf_guard_removed_image SET project_id = ? WHERE target_id = ?",
            params![project_id, target_id],
        )
        .map_err(AppError::db)?;
    }
    let profile = &plan.destination_profile_id;
    if *profile != plan.source.profile_id {
        for template in &plan.template_moves {
            // A plan without an exposure of its own uses its template's
            // default; keep that exposure when the template is replaced.
            if template.destination_template_id.is_some()
                && let Some(exposure) = template.source_default_exposure
            {
                for exposure_plan in &template.default_exposure_plan_ids {
                    conn.execute(
                        "UPDATE exposureplan SET exposure = ? WHERE Id = ?",
                        params![exposure, exposure_plan],
                    )
                    .map_err(AppError::db)?;
                }
            }
            let destination_template = match template.destination_template_id {
                Some(id) => id,
                None => clone_row(
                    conn,
                    "exposuretemplate",
                    template.source_template_id,
                    &[("profileId", Value::Text(profile.clone()))],
                )?,
            };
            for exposure_plan in &template.exposure_plan_ids {
                conn.execute(
                    "UPDATE exposureplan SET exposureTemplateId = ? WHERE Id = ?",
                    params![destination_template, exposure_plan],
                )
                .map_err(AppError::db)?;
            }
        }
        if has_column(conn, "exposureplan", "profileId")? {
            conn.execute(
                "UPDATE exposureplan SET profileId = ? WHERE targetid = ?",
                params![profile, target_id],
            )
            .map_err(AppError::db)?;
        }
        if has_column(conn, "flathistory", "profileId")?
            && has_column(conn, "flathistory", "targetId")?
        {
            conn.execute(
                "UPDATE flathistory SET profileId = ? WHERE targetId = ?",
                params![profile, target_id],
            )
            .map_err(AppError::db)?;
        }
    }
    if let Some(guid) = &plan.target_guid {
        for table in [DIRECTOR_TARGET_TABLE, DIRECTOR_PLAN_TABLE] {
            if has_table(conn, table)? {
                conn.execute(
                    &format!("DELETE FROM {} WHERE target_guid = ?", quote(table)),
                    [guid],
                )
                .map_err(AppError::db)?;
            }
        }
        if has_table(conn, COLLABORATION_PLAN_TABLE)? {
            conn.execute(
                "UPDATE psf_guard_collaboration_plan SET end_at_ms = ?
                 WHERE target_guid = ? COLLATE NOCASE AND end_at_ms IS NULL",
                params![chrono::Utc::now().timestamp_millis(), guid],
            )
            .map_err(AppError::db)?;
        }
    }
    Ok(())
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

    fn move_target(
        target_id: i32,
        project_id: Option<i32>,
        new_project_name: Option<&str>,
    ) -> OrganizationOperation {
        OrganizationOperation::MoveTarget {
            target_id,
            destination: TargetMoveDestination {
                project_id,
                new_project_name: new_project_name.map(str::to_string),
            },
        }
    }

    fn bad_request(result: Result<OrganizationPlan, AppError>) -> String {
        match result {
            Err(AppError::BadRequest(message)) => message,
            Err(other) => panic!("expected a bad request, got {other:?}"),
            Ok(_) => panic!("expected a bad request, got a plan"),
        }
    }

    /// A target with no frames in project 1 (profile `p`), with one plan on
    /// the Luminance template and one on an Ha template of profile `p`.
    /// Profile `other` has a Luminance template with the same settings and
    /// an Ha template with another gain.
    fn add_frameless_target(conn: &Connection) {
        conn.execute_batch(
            "INSERT INTO target (Id, name, active, ra, dec, epochcode, rotation, roi, projectid, guid) VALUES
                (5, 'NGC 2403', 1, 7.6, 65.6, 2, 0, 100, 1, 'frameless-target');
             INSERT INTO exposuretemplate (Id, profileId, name, filtername, gain, offset, bin, readoutmode, guid) VALUES
                (2, 'p', 'Ha 300', 'Ha', 100, 10, 1, 0, 'ha-template'),
                (3, 'other', 'Lum', 'L', NULL, NULL, 1, NULL, 'other-lum-template'),
                (4, 'other', 'Ha high gain', 'Ha', 200, 10, 1, 0, 'other-ha-template');
             INSERT INTO exposureplan (Id, profileId, exposure, desired, acquired, accepted, targetid, exposureTemplateId, enabled, guid) VALUES
                (20, 'p', 120, 40, 0, 0, 5, 1, 1, 'frameless-lum-plan'),
                (21, 'p', 300, 20, 0, 0, 5, 2, 1, 'frameless-ha-plan'),
                (22, 'p', 600, 10, 0, 0, 5, 2, 1, 'frameless-ha-long-plan');
             INSERT INTO flathistory (Id, targetId, profileId) VALUES (3, 5, 'p');",
        )
        .unwrap();
    }

    #[test]
    fn moved_target_takes_its_frames_and_keeps_its_plans_within_the_profile() {
        let mut conn = catalog();
        let operation = move_target(1, Some(2), None);
        let plan = plan_organization(&conn, &operation).unwrap();
        assert_eq!(plan.preview.images_moved, 3);
        assert_eq!(plan.preview.exposure_plans_moved, 1);
        assert_eq!(plan.preview.destination_target_id, Some(1));
        assert!(plan.preview.profile_change.is_none());
        assert_eq!(
            one::<i64>(&conn, "SELECT projectid FROM target WHERE Id = 1"),
            1,
            "preview must not move the target"
        );
        let result = preview_and_apply(&mut conn, operation);
        assert_eq!(
            (
                result.source_project_id,
                result.project_id,
                result.target_id,
                result.images_moved
            ),
            (1, 2, 1, 3)
        );
        assert_eq!(
            one::<i64>(&conn, "SELECT projectid FROM target WHERE Id = 1"),
            2
        );
        assert_eq!(
            one::<i64>(
                &conn,
                "SELECT COUNT(*) FROM acquiredimage WHERE targetId = 1 AND projectId = 2"
            ),
            3
        );
        assert_eq!(
            one::<String>(&conn, "SELECT targetid || '/' || profileId || '/' || exposureTemplateId || '/' || accepted FROM exposureplan WHERE Id = 1"),
            "1/p/1/9",
            "exposure plans stay as they were"
        );
        assert_eq!(
            one::<i64>(
                &conn,
                "SELECT COUNT(*) FROM overrideexposureorderitem WHERE targetid = 1"
            ),
            1
        );
        assert_eq!(
            one::<i64>(
                &conn,
                "SELECT COUNT(*) FROM filtercadenceitem WHERE targetid = 1"
            ),
            1
        );
        assert_eq!(
            one::<String>(&conn, "SELECT profileId FROM flathistory WHERE Id = 1"),
            "p"
        );
        assert_eq!(one::<i64>(&conn, "SELECT COUNT(*) FROM project"), 3);
        assert_links(&conn);
    }

    #[test]
    fn a_target_with_frames_cannot_change_profile() {
        let conn = catalog();
        let message = bad_request(plan_organization(&conn, &move_target(1, Some(3), None)));
        assert!(message.contains("has 3 exposures, captured"), "{message}");
        assert!(message.contains("its own profile"), "{message}");
        assert!(message.contains("with Move exposures"), "{message}");
        assert_eq!(
            one::<i64>(&conn, "SELECT projectid FROM target WHERE Id = 1"),
            1
        );
    }

    #[test]
    fn a_frameless_target_changes_profile_reusing_and_copying_templates() {
        let mut conn = catalog();
        add_frameless_target(&conn);
        let operation = move_target(5, Some(3), None);
        let preview = plan_organization(&conn, &operation).unwrap().preview;
        assert_eq!(preview.images_moved, 0);
        assert_eq!(preview.exposure_plans_moved, 3);
        assert_eq!(
            (
                preview.exposure_templates_reused,
                preview.exposure_templates_copied
            ),
            (1, 1)
        );
        let change = preview.profile_change.unwrap();
        assert_eq!(
            (
                change.from_profile_id.as_str(),
                change.to_profile_id.as_str()
            ),
            ("p", "other")
        );
        let templates_before = one::<i64>(&conn, "SELECT COUNT(*) FROM exposuretemplate");

        let result = preview_and_apply(&mut conn, operation);

        assert_eq!((result.source_project_id, result.project_id), (1, 3));
        assert_eq!(
            one::<i64>(&conn, "SELECT projectid FROM target WHERE Id = 5"),
            3
        );
        assert_eq!(
            one::<String>(
                &conn,
                "SELECT profileId || '/' || exposureTemplateId FROM exposureplan WHERE Id = 20"
            ),
            "other/3",
            "the matching Luminance template is reused"
        );
        let copied: i64 = one(
            &conn,
            "SELECT exposureTemplateId FROM exposureplan WHERE Id = 21",
        );
        assert_eq!(
            one::<i64>(
                &conn,
                "SELECT exposureTemplateId FROM exposureplan WHERE Id = 22"
            ),
            copied,
            "plans that shared a template share its copy"
        );
        assert!(![2, 4].contains(&copied));
        assert_eq!(
            one::<i64>(&conn, "SELECT COUNT(*) FROM exposuretemplate"),
            templates_before + 1
        );
        let template: (String, String, i64, String) = conn
            .query_row(
                "SELECT profileId, filtername, gain, guid FROM exposuretemplate WHERE Id = ?",
                [copied],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            (template.0.as_str(), template.1.as_str(), template.2),
            ("other", "Ha", 100)
        );
        assert_ne!(template.3, "ha-template");
        assert_eq!(
            one::<String>(&conn, "SELECT profileId FROM exposuretemplate WHERE Id = 2"),
            "p",
            "the source template stays in its profile"
        );
        assert_eq!(
            one::<String>(&conn, "SELECT profileId FROM exposureplan WHERE Id = 21"),
            "other"
        );
        assert_eq!(
            one::<String>(&conn, "SELECT profileId FROM flathistory WHERE Id = 3"),
            "other"
        );
        assert_eq!(
            one::<String>(&conn, "SELECT profileId FROM flathistory WHERE Id = 1"),
            "p"
        );
        assert_links(&conn);
    }

    #[test]
    fn a_target_moves_to_a_new_project_copied_from_its_own() {
        let mut conn = catalog();
        let message = bad_request(plan_organization(
            &conn,
            &move_target(1, None, Some(" destination project ")),
        ));
        assert!(message.contains("already exists"), "{message}");
        let preview = plan_organization(&conn, &move_target(1, None, Some("Spring campaign")))
            .unwrap()
            .preview;
        assert_eq!(preview.destination_project_id, None);
        assert!(preview
            .warnings
            .iter()
            .any(|warning| warning.contains("starts inactive")));

        let result = preview_and_apply(&mut conn, move_target(1, None, Some("Spring campaign")));

        assert!(result.project_id > 3);
        let project: (String, String, i64, String) = conn
            .query_row(
                "SELECT name, profileId, state, guid FROM project WHERE Id = ?",
                [result.project_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            (project.0.as_str(), project.1.as_str(), project.2),
            ("Spring campaign", "p", 2)
        );
        assert_ne!(project.3, "source-project");
        let rules: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM ruleweight WHERE projectid = ?",
                [result.project_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(rules, 1);
        assert_eq!(
            one::<i64>(&conn, "SELECT COUNT(*) FROM ruleweight WHERE projectid = 1"),
            1,
            "the source project keeps its rule weights"
        );
        assert_eq!(result.target_id, 1);
        let moved: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM acquiredimage WHERE targetId = 1 AND projectId = ?",
                [result.project_id],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(moved, 3);
        assert_eq!(
            one::<String>(&conn, "SELECT guid FROM target WHERE Id = 1"),
            "source-target",
            "the target keeps its identity"
        );
        assert_links(&conn);
    }

    #[test]
    fn a_name_clash_or_unclear_destination_is_refused() {
        let conn = catalog();
        conn.execute("UPDATE target SET name = ' m81 ' WHERE Id = 1", [])
            .unwrap();
        let message = bad_request(plan_organization(&conn, &move_target(1, Some(2), None)));
        assert!(message.contains("Merge target"), "{message}");
        assert!(message.contains("Destination project"), "{message}");
        bad_request(plan_organization(&conn, &move_target(1, Some(1), None)));
        bad_request(plan_organization(&conn, &move_target(1, None, None)));
        bad_request(plan_organization(
            &conn,
            &move_target(1, Some(2), Some("Both")),
        ));
        bad_request(plan_organization(&conn, &move_target(1, Some(99), None)));
        bad_request(plan_organization(&conn, &move_target(99, Some(2), None)));
        // A target with another name moves; only names collide.
        conn.execute("UPDATE target SET name = 'M81 alias' WHERE Id = 1", [])
            .unwrap();
        plan_organization(&conn, &move_target(1, Some(2), None)).unwrap();
    }

    #[test]
    fn a_stale_target_move_preview_is_refused() {
        let mut conn = catalog();
        let operation = move_target(1, Some(2), None);
        let fingerprint = plan_organization(&conn, &operation)
            .unwrap()
            .preview
            .fingerprint;
        conn.execute("UPDATE exposureplan SET desired = 99 WHERE Id = 1", [])
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
            one::<i64>(&conn, "SELECT projectid FROM target WHERE Id = 1"),
            1
        );

        // A new matching template in the destination profile changes what
        // the move would do, so it needs a new preview too.
        add_frameless_target(&conn);
        let operation = move_target(5, Some(3), None);
        let fingerprint = plan_organization(&conn, &operation)
            .unwrap()
            .preview
            .fingerprint;
        conn.execute(
            "INSERT INTO exposuretemplate (Id, profileId, name, filtername, gain, offset, bin, readoutmode, guid)
             VALUES (9, 'other', 'Ha 300', 'Ha', 100, 10, NULL, 0, 'late-template')",
            [],
        )
        .unwrap();
        assert!(matches!(
            apply_organization(
                &mut conn,
                &OrganizationApplyRequest {
                    operation: operation.clone(),
                    expected_fingerprint: fingerprint
                }
            ),
            Err(AppError::Conflict(_))
        ));
        let preview = plan_organization(&conn, &operation).unwrap().preview;
        assert_eq!(
            (
                preview.exposure_templates_reused,
                preview.exposure_templates_copied
            ),
            (2, 0),
            "a blank binning matches binning 1"
        );
    }

    #[test]
    fn a_moved_target_leaves_its_planning_panel() {
        let mut conn = catalog();
        conn.execute_batch(
            "CREATE TABLE psf_guard_director_target(
                target_guid TEXT PRIMARY KEY NOT NULL, project_guid TEXT NOT NULL,
                panel_id TEXT NOT NULL, framing_revision INTEGER NOT NULL);
             CREATE TABLE psf_guard_director_plan(
                exposureplan_guid TEXT PRIMARY KEY NOT NULL, target_guid TEXT NOT NULL,
                contribution_id TEXT NOT NULL, objective_id TEXT NOT NULL, bandpass_id TEXT NOT NULL,
                purpose TEXT NOT NULL, required_frames INTEGER NOT NULL, plan_revision INTEGER NOT NULL);
             INSERT INTO psf_guard_director_target VALUES
                ('source-target', 'source-project', 'r1c1', 1),
                ('destination-target', 'destination-project', 'r1c1', 1);
             INSERT INTO psf_guard_director_plan VALUES
                ('source-plan', 'source-target', 'c', 'o', 'L', 'light', 30, 1),
                ('destination-plan', 'destination-target', 'c', 'o', 'L', 'light', 50, 1);",
        )
        .unwrap();
        let preview = plan_organization(&conn, &move_target(1, Some(2), None))
            .unwrap()
            .preview;
        assert_eq!(preview.planning_panel_id.as_deref(), Some("r1c1"));
        assert!(preview
            .warnings
            .iter()
            .any(|warning| warning.contains("no longer manages the target")));

        preview_and_apply(&mut conn, move_target(1, Some(2), None));

        assert_eq!(
            one::<String>(
                &conn,
                "SELECT group_concat(target_guid) FROM psf_guard_director_target"
            ),
            "destination-target"
        );
        assert_eq!(
            one::<String>(
                &conn,
                "SELECT group_concat(exposureplan_guid) FROM psf_guard_director_plan"
            ),
            "destination-plan"
        );
        // A move reports a panel only while Planning has a record of one.
        let preview = plan_organization(&conn, &move_target(3, Some(1), None))
            .unwrap()
            .preview;
        assert_eq!(preview.planning_panel_id.as_deref(), Some("r1c1"));
        conn.execute("DELETE FROM psf_guard_director_target", [])
            .unwrap();
        let preview = plan_organization(&conn, &move_target(3, Some(1), None))
            .unwrap()
            .preview;
        assert_eq!(preview.planning_panel_id, None);
    }

    fn removed_reject(conn: &Connection, guid: &str, target_id: i32, project_id: i32) {
        crate::commands::reject_removal::ensure_schema(conn).unwrap();
        conn.execute(
            "INSERT INTO psf_guard_removed_image (acquired_image_guid, acquired_image_id, project_id, target_id,
                batch_id, removed_at, trash_until, rows_json, files_json)
             VALUES (?, 900, ?, ?, 'batch', 1, 2, '{}', '[]')",
            params![guid, project_id, target_id],
        )
        .unwrap();
    }

    #[test]
    fn removed_rejects_count_as_exposures_and_follow_the_target() {
        let mut conn = catalog();
        add_frameless_target(&conn);
        removed_reject(&conn, "removed-5", 5, 1);
        let message = bad_request(plan_organization(&conn, &move_target(5, Some(3), None)));
        assert!(
            message.contains("has 1 exposure, including 1 removed reject you can still restore"),
            "{message}"
        );
        // Once its saved rows are purged, a removed reject cannot come back.
        conn.execute("UPDATE psf_guard_removed_image SET purged_at = 3", [])
            .unwrap();
        plan_organization(&conn, &move_target(5, Some(3), None)).unwrap();

        removed_reject(&conn, "removed-1", 1, 1);
        let preview = plan_organization(&conn, &move_target(1, Some(2), None))
            .unwrap()
            .preview;
        assert!(preview
            .warnings
            .iter()
            .any(|warning| warning.contains("Restoring it returns it to Destination project")));
        preview_and_apply(&mut conn, move_target(1, Some(2), None));
        assert_eq!(
            one::<i64>(
                &conn,
                "SELECT project_id FROM psf_guard_removed_image WHERE acquired_image_guid = 'removed-1'"
            ),
            2
        );
        assert_eq!(
            one::<i64>(
                &conn,
                "SELECT project_id FROM psf_guard_removed_image WHERE acquired_image_guid = 'removed-5'"
            ),
            1
        );
    }

    #[test]
    fn a_moved_target_ends_its_open_collaboration_assignments() {
        let mut conn = catalog();
        conn.execute_batch(
            "CREATE TABLE psf_guard_collaboration_plan(
                exposureplan_guid TEXT NOT NULL, target_guid TEXT NOT NULL, import_id TEXT NOT NULL,
                source_digest TEXT NOT NULL, panel_index INTEGER NOT NULL, filter TEXT NOT NULL,
                exposure_ms INTEGER NOT NULL, start_at_ms INTEGER NOT NULL, end_at_ms INTEGER,
                desired_frames INTEGER NOT NULL, PRIMARY KEY(exposureplan_guid, import_id, source_digest));
             INSERT INTO psf_guard_collaboration_plan VALUES
                ('source-plan', 'SOURCE-TARGET', 'open', 'd', 0, 'L', 60000, 1, NULL, 10),
                ('source-plan', 'source-target', 'ended', 'd', 0, 'L', 60000, 1, 5, 10),
                ('destination-plan', 'destination-target', 'open', 'd', 0, 'L', 60000, 1, NULL, 10);",
        )
        .unwrap();
        let preview = plan_organization(&conn, &move_target(1, Some(2), None))
            .unwrap()
            .preview;
        assert!(preview
            .warnings
            .iter()
            .any(|warning| warning.contains("open collaboration assignment")));
        let other = plan_organization(&conn, &move_target(3, Some(1), None))
            .unwrap()
            .preview;
        assert!(other
            .warnings
            .iter()
            .any(|warning| warning.contains("open collaboration assignment")));

        preview_and_apply(&mut conn, move_target(1, Some(2), None));

        assert!(
            one::<i64>(
                &conn,
                "SELECT end_at_ms FROM psf_guard_collaboration_plan WHERE import_id = 'open' AND exposureplan_guid = 'source-plan'"
            ) > 1_000_000_000_000
        );
        assert_eq!(
            one::<i64>(
                &conn,
                "SELECT end_at_ms FROM psf_guard_collaboration_plan WHERE import_id = 'ended'"
            ),
            5
        );
        assert_eq!(
            one::<Option<i64>>(
                &conn,
                "SELECT end_at_ms FROM psf_guard_collaboration_plan WHERE exposureplan_guid = 'destination-plan'"
            ),
            None
        );
    }

    #[test]
    fn a_reused_template_keeps_a_plans_default_exposure_and_the_preview_lists_templates() {
        let mut conn = catalog();
        add_frameless_target(&conn);
        conn.execute_batch(
            "UPDATE exposuretemplate SET defaultexposure = 120 WHERE Id = 1;
             UPDATE exposuretemplate SET defaultexposure = 30 WHERE Id = 3;
             UPDATE exposureplan SET exposure = -1 WHERE Id = 20;",
        )
        .unwrap();
        let preview = plan_organization(&conn, &move_target(5, Some(3), None))
            .unwrap()
            .preview;
        let changes: Vec<(&str, &str, bool)> = preview
            .exposure_template_changes
            .iter()
            .map(|change| {
                (
                    change.source_template_name.as_str(),
                    change.destination_template_name.as_str(),
                    change.reused,
                )
            })
            .collect();
        assert_eq!(
            changes,
            [("Luminance", "Lum", true), ("Ha 300", "Ha 300", false)]
        );
        assert!(preview
            .warnings
            .iter()
            .any(|warning| warning.contains("1 exposure plan uses the default exposure")));
        assert!(
            preview
                .warnings
                .iter()
                .any(|warning| warning.contains("Other profile has no exposures")),
            "{:?}",
            preview.warnings
        );

        preview_and_apply(&mut conn, move_target(5, Some(3), None));

        assert_eq!(
            one::<f64>(&conn, "SELECT exposure FROM exposureplan WHERE Id = 20"),
            120.0
        );
        assert_eq!(
            one::<f64>(&conn, "SELECT exposure FROM exposureplan WHERE Id = 21"),
            300.0
        );
    }

    #[test]
    fn a_target_with_exposures_does_not_warn_of_an_empty_destination() {
        let conn = catalog();
        let preview = plan_organization(&conn, &move_target(1, None, Some("Spring")))
            .unwrap()
            .preview;
        assert!(!preview
            .warnings
            .iter()
            .any(|warning| warning.contains("has no exposures")));
    }

    #[test]
    fn a_new_project_copies_the_stack_settings_of_the_targets_project() {
        let mut conn = catalog();
        conn.execute_batch(
            "CREATE TABLE psf_guard_project_processing (project_key TEXT PRIMARY KEY,
                split_exposure_groups INTEGER NOT NULL, process_folder TEXT, split_chosen INTEGER);
             INSERT INTO psf_guard_project_processing VALUES ('guid:source-project', 0, 'iris', 1);",
        )
        .unwrap();
        let result = preview_and_apply(&mut conn, move_target(1, None, Some("Spring campaign")));
        let settings =
            crate::server::exposure_groups::load_settings(&conn, result.project_id).unwrap();
        assert!(!settings.split_exposure_groups);
        assert_eq!(settings.process_folder.as_deref(), Some("iris"));
        assert_eq!(
            one::<i64>(&conn, "SELECT COUNT(*) FROM psf_guard_project_processing"),
            2
        );
    }

    #[test]
    fn a_grade_change_after_a_target_move_preview_does_not_require_a_new_preview() {
        let mut conn = catalog();
        let operation = move_target(1, Some(2), None);
        let expected_fingerprint = plan_organization(&conn, &operation)
            .unwrap()
            .preview
            .fingerprint;
        conn.execute(
            "UPDATE acquiredimage SET gradingStatus = 2 WHERE Id = 1",
            [],
        )
        .unwrap();
        apply_organization(
            &mut conn,
            &OrganizationApplyRequest {
                operation,
                expected_fingerprint,
            },
        )
        .unwrap();
        assert_eq!(
            one::<i64>(&conn, "SELECT projectid FROM target WHERE Id = 1"),
            2
        );
    }
}

/// Target moves through the HTTP API, with stack indices on disk.
#[cfg(test)]
mod http_tests {
    use crate::server::state::AppState;
    use axum::{body::Body, http::Request, Router};
    use serde_json::{json, Value};
    use std::sync::Arc;
    use tower::ServiceExt;

    struct Rig {
        _dir: tempfile::TempDir,
        path: std::path::PathBuf,
        stacks: std::path::PathBuf,
        app: Router,
    }

    fn rig() -> Rig {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rig.sqlite");
        let images = dir.path().join("images");
        std::fs::create_dir_all(&images).unwrap();
        let conn = rusqlite::Connection::open(&path).unwrap();
        crate::ts_schema::apply_schema(&conn).unwrap();
        conn.execute_batch(
            "INSERT INTO project (Id,profileId,name,state,guid) VALUES
                (1,'p','Bubble',1,'pg1'), (2,'p','Cassiopeia',1,'pg2'), (3,'other','Other rig',1,'pg3');
             INSERT INTO target (Id,name,active,ra,dec,epochcode,projectid,guid) VALUES
                (1,'Bubble Nebula',1,23.3,61.2,0,1,'tg1'), (2,'Bubble Nebula',1,23.3,61.2,0,2,'tg2');
             INSERT INTO exposuretemplate (Id,profileId,name,filtername,guid) VALUES (1,'p','SII','SII','eg');
             INSERT INTO exposureplan (Id,profileId,exposure,desired,acquired,accepted,targetid,exposureTemplateId,guid)
                VALUES (1,'p',300,50,1,0,1,1,'lg');
             INSERT INTO acquiredimage (Id,projectId,targetId,acquireddate,filtername,gradingStatus,metadata,profileId,exposureId,guid)
                VALUES (1,1,1,1000,'SII',0,'{}','p',1,'ig1');",
        )
        .unwrap();
        drop(conn);
        let catalog = crate::server::database_context::DatabaseContext::new(
            "rig".into(),
            "Rig".into(),
            path.display().to_string(),
            vec![images.display().to_string()],
            None,
            None,
            None,
            dir.path().join("cache"),
        )
        .unwrap();
        let stacks = crate::server::storage::stacks(&catalog.stack_root);
        std::fs::create_dir_all(&stacks).unwrap();
        std::fs::write(
            stacks.join("latest-project-1.json"),
            json!({"project_id": 1, "database_id": "rig", "groups": [
                {"job_id": "sii", "created_unix_seconds": 1,
                 "group": {"index": 0, "target_id": 1, "filter_name": "SII"}}]})
            .to_string(),
        )
        .unwrap();
        let state = AppState::new_for_test(rusqlite::Connection::open_in_memory().unwrap());
        {
            let mut databases = state.databases.write().unwrap();
            databases.clear();
            databases.insert(catalog.id.clone(), Arc::new(catalog));
        }
        let app = Router::new().nest("/api", crate::server::api_router(Arc::new(state)));
        Rig {
            _dir: dir,
            path,
            stacks,
            app,
        }
    }

    async fn send(app: &Router, method: &str, uri: &str, body: Value) -> (u16, Value) {
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .header("host", "localhost")
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let response = app.clone().oneshot(request).await.unwrap();
        let status = response.status().as_u16();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (
            status,
            serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        )
    }

    fn target(rig: &Rig) -> (i32, String) {
        rusqlite::Connection::open(&rig.path)
            .unwrap()
            .query_row(
                "SELECT projectid, name FROM target WHERE Id = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap()
    }

    fn index_jobs(rig: &Rig, project: i32) -> Vec<String> {
        let path = rig.stacks.join(format!("latest-project-{project}.json"));
        let Ok(bytes) = std::fs::read(path) else {
            return Vec::new();
        };
        let index: Value = serde_json::from_slice(&bytes).unwrap();
        index["groups"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["job_id"].as_str().unwrap().to_string())
            .collect()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn put_refuses_a_move_without_changing_anything_else() {
        let rig = rig();
        let (status, body) = send(
            &rig.app,
            "PUT",
            "/api/db/rig/targets/1",
            json!({ "name": "Renamed", "active": false, "project_id": 3 }),
        )
        .await;
        assert_eq!(status, 400, "{body}");
        assert!(
            body["error"].as_str().unwrap().contains("has 1 exposure"),
            "{body}"
        );
        assert_eq!(target(&rig), (1, "Bubble Nebula".to_string()));
        let active: bool = rusqlite::Connection::open(&rig.path)
            .unwrap()
            .query_row("SELECT active FROM target WHERE Id = 1", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert!(active, "a refused move leaves the other changes unmade");

        let (status, body) = send(
            &rig.app,
            "PUT",
            "/api/db/rig/targets/1",
            json!({ "project_id": 2 }),
        )
        .await;
        assert_eq!(status, 400, "{body}");
        assert!(
            body["error"].as_str().unwrap().contains("Merge target"),
            "{body}"
        );
        assert_eq!(index_jobs(&rig, 1), ["sii"]);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stack_previews_follow_a_target_moved_by_put_and_by_apply() {
        let rig = rig();
        let (status, body) = send(
            &rig.app,
            "PUT",
            "/api/db/rig/targets/1",
            json!({ "name": "Bubble Nebula west", "project_id": 2 }),
        )
        .await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["data"]["images_moved"], 1);
        assert_eq!(target(&rig), (2, "Bubble Nebula west".to_string()));
        assert_eq!(index_jobs(&rig, 2), ["sii"]);
        assert!(index_jobs(&rig, 1).is_empty());

        let operation = json!({ "kind": "move_target", "target_id": 1,
            "destination": { "project_id": 1 } });
        let (status, preview) = send(
            &rig.app,
            "POST",
            "/api/db/rig/organization/preview",
            operation.clone(),
        )
        .await;
        assert_eq!(status, 200, "{preview}");
        let fingerprint = preview["data"]["fingerprint"].as_str().unwrap();
        let (status, result) = send(
            &rig.app,
            "POST",
            "/api/db/rig/organization/apply",
            json!({ "operation": operation, "expected_fingerprint": fingerprint }),
        )
        .await;
        assert_eq!(status, 200, "{result}");
        assert_eq!(result["data"]["source_project_id"], 2);
        assert_eq!(target(&rig).0, 1);
        assert_eq!(index_jobs(&rig, 1), ["sii"]);
        assert!(index_jobs(&rig, 2).is_empty());
    }
}
