//! Target Scheduler project, target, and exposure-plan views and edits.

use axum::{
    extract::{Path, State},
    Json,
};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

use crate::db::Database;
use crate::server::api::{ApiResponse, UpdateProjectRequest, UpdateTargetRequest};
use crate::server::extract::DbContext;
use crate::server::handlers::{require_database_management_allowed, AppError};
use crate::server::state::AppState;

#[derive(Debug, Serialize)]
pub struct ProjectSchedulerResponse {
    pub id: i32,
    pub profile_id: String,
    pub name: String,
    pub description: Option<String>,
    pub state: i32,
    pub priority: i32,
    pub created_at: Option<i64>,
    pub active_at: Option<i64>,
    pub inactive_at: Option<i64>,
    pub minimum_time: i32,
    pub minimum_altitude: f64,
    pub maximum_altitude: f64,
    pub use_custom_horizon: bool,
    pub horizon_offset: f64,
    pub meridian_window: i32,
    pub filter_switch_frequency: i32,
    pub dither_every: i32,
    pub enable_grader: bool,
    pub is_mosaic: bool,
    /// All shared exposure templates for this project's Target Scheduler
    /// profile, plus any legacy template already linked to this project.
    pub exposure_templates: Vec<ExposureTemplateResponse>,
    pub targets: Vec<SchedulerTargetResponse>,
}

#[derive(Debug, Serialize)]
pub struct ExposureTemplateResponse {
    pub id: i32,
    pub profile_id: String,
    pub name: String,
    pub filter_name: String,
    pub gain: Option<i32>,
    pub offset: Option<i32>,
    pub bin: Option<i32>,
    pub readout_mode: Option<i32>,
    pub twilight_level: i32,
    pub moon_avoidance_enabled: bool,
    pub moon_avoidance_separation: f64,
    pub moon_avoidance_width: i32,
    pub maximum_humidity: f64,
    pub default_exposure: f64,
    pub moon_relax_scale: f64,
    pub moon_relax_max_altitude: f64,
    pub moon_relax_min_altitude: f64,
    pub moon_down_enabled: bool,
    pub dither_every: i32,
    pub minutes_offset: i32,
    pub plan_count: i32,
}

#[derive(Debug, Serialize)]
pub struct SchedulerTargetResponse {
    pub id: i32,
    pub name: String,
    pub active: bool,
    pub ra_hours: f64,
    pub dec_degrees: f64,
    pub epoch_code: i32,
    pub rotation: f64,
    pub roi: f64,
    pub exposure_plans: Vec<ExposurePlanResponse>,
}

#[derive(Debug, Serialize)]
pub struct ExposurePlanResponse {
    pub id: i32,
    pub exposure_template_id: i32,
    pub template_name: String,
    pub filter_name: String,
    pub gain: Option<i32>,
    pub offset: Option<i32>,
    pub bin: Option<i32>,
    pub readout_mode: Option<i32>,
    pub exposure: f64,
    pub desired: i32,
    pub acquired: i32,
    pub accepted: i32,
    pub enabled: bool,
}

#[derive(Debug, Deserialize)]
pub struct CreateExposurePlanRequest {
    /// Reuse this exact profile-scoped template. When absent, the settings
    /// below match an existing template or create a new one.
    #[serde(default)]
    pub exposure_template_id: Option<i32>,
    #[serde(default)]
    pub filter_name: Option<String>,
    #[serde(default)]
    pub template_name: Option<String>,
    #[serde(default)]
    pub gain: Option<i32>,
    #[serde(default)]
    pub offset: Option<i32>,
    #[serde(default)]
    pub bin: Option<i32>,
    #[serde(default)]
    pub readout_mode: Option<i32>,
    pub exposure: f64,
    pub desired: i32,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Debug, Deserialize)]
pub struct UpdateExposurePlanRequest {
    pub exposure: f64,
    pub desired: i32,
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

fn has_column(conn: &Connection, table: &str, column: &str) -> bool {
    conn.prepare(&format!("PRAGMA table_info({table})"))
        .and_then(|mut stmt| {
            let rows = stmt.query_map([], |row| row.get::<_, String>(1))?;
            Ok(rows.flatten().any(|name| name.eq_ignore_ascii_case(column)))
        })
        .unwrap_or(false)
}

fn project_details(
    conn: &Connection,
    project_id: i32,
) -> rusqlite::Result<Option<ProjectSchedulerResponse>> {
    let maximum_altitude = if has_column(conn, "project", "maximumAltitude") {
        "COALESCE(maximumAltitude, 0)"
    } else {
        "0.0"
    };
    let is_mosaic = if has_column(conn, "project", "isMosaic") {
        "COALESCE(isMosaic, 0)"
    } else {
        "0"
    };
    let sql = format!(
        "SELECT Id, profileId, name, description, COALESCE(state, 0),
                COALESCE(priority, 1), createdate, activedate, inactivedate,
                COALESCE(minimumtime, 30), COALESCE(minimumaltitude, 0),
                {maximum_altitude}, COALESCE(usecustomhorizon, 0),
                COALESCE(horizonoffset, 0), COALESCE(meridianwindow, 0),
                COALESCE(filterswitchfrequency, 0), COALESCE(ditherevery, 0),
                COALESCE(enablegrader, 1), {is_mosaic}
         FROM project WHERE Id = ?"
    );
    conn.query_row(&sql, [project_id], |row| {
        Ok(ProjectSchedulerResponse {
            id: row.get(0)?,
            profile_id: row.get(1)?,
            name: row.get(2)?,
            description: row.get(3)?,
            state: row.get(4)?,
            priority: row.get(5)?,
            created_at: row.get(6)?,
            active_at: row.get(7)?,
            inactive_at: row.get(8)?,
            minimum_time: row.get(9)?,
            minimum_altitude: row.get(10)?,
            maximum_altitude: row.get(11)?,
            use_custom_horizon: row.get::<_, i32>(12)? != 0,
            horizon_offset: row.get(13)?,
            meridian_window: row.get(14)?,
            filter_switch_frequency: row.get(15)?,
            dither_every: row.get(16)?,
            enable_grader: row.get::<_, i32>(17)? != 0,
            is_mosaic: row.get::<_, i32>(18)? != 0,
            exposure_templates: Vec::new(),
            targets: Vec::new(),
        })
    })
    .optional()
}

fn templates_for_project(
    conn: &Connection,
    profile_id: &str,
    project_id: i32,
) -> rusqlite::Result<Vec<ExposureTemplateResponse>> {
    let mut stmt = conn.prepare(
        "SELECT et.Id, et.profileId, et.name, et.filtername,
                et.gain, et.offset, et.bin, et.readoutmode,
                COALESCE(et.twilightlevel, 0),
                COALESCE(et.moonavoidanceenabled, 0),
                COALESCE(et.moonavoidanceseparation, 60),
                COALESCE(et.moonavoidancewidth, 7),
                COALESCE(et.maximumhumidity, 0),
                COALESCE(et.defaultexposure, 60),
                COALESCE(et.moonrelaxscale, 0),
                COALESCE(et.moonrelaxmaxaltitude, 5),
                COALESCE(et.moonrelaxminaltitude, -15),
                COALESCE(et.moondownenabled, 0),
                COALESCE(et.ditherevery, -1),
                COALESCE(et.minutesOffset, 0),
                COUNT(ep.Id)
         FROM exposuretemplate et
         LEFT JOIN exposureplan ep ON ep.exposureTemplateId = et.Id
         WHERE et.profileId = ?1
            OR EXISTS (
                SELECT 1 FROM exposureplan linked_ep
                JOIN target linked_target ON linked_target.Id = linked_ep.targetid
                WHERE linked_ep.exposureTemplateId = et.Id
                  AND linked_target.projectid = ?2
            )
         GROUP BY et.Id
         ORDER BY et.filtername, et.name, et.Id",
    )?;
    stmt.query_map(params![profile_id, project_id], |row| {
        Ok(ExposureTemplateResponse {
            id: row.get(0)?,
            profile_id: row.get(1)?,
            name: row.get(2)?,
            filter_name: row.get(3)?,
            gain: row.get(4)?,
            offset: row.get(5)?,
            bin: row.get(6)?,
            readout_mode: row.get(7)?,
            twilight_level: row.get(8)?,
            moon_avoidance_enabled: row.get::<_, i32>(9)? != 0,
            moon_avoidance_separation: row.get(10)?,
            moon_avoidance_width: row.get(11)?,
            maximum_humidity: row.get(12)?,
            default_exposure: row.get(13)?,
            moon_relax_scale: row.get(14)?,
            moon_relax_max_altitude: row.get(15)?,
            moon_relax_min_altitude: row.get(16)?,
            moon_down_enabled: row.get::<_, i32>(17)? != 0,
            dither_every: row.get(18)?,
            minutes_offset: row.get(19)?,
            plan_count: row.get(20)?,
        })
    })?
    .collect()
}

fn targets_for_project(
    conn: &Connection,
    project_id: i32,
) -> rusqlite::Result<Vec<SchedulerTargetResponse>> {
    let mut stmt = conn.prepare(
        "SELECT Id, name, active, COALESCE(ra, 0), COALESCE(dec, 0),
                epochcode, COALESCE(rotation, 0), COALESCE(roi, 100)
         FROM target WHERE projectid = ? ORDER BY name, Id",
    )?;
    let mut targets = stmt
        .query_map([project_id], |row| {
            Ok(SchedulerTargetResponse {
                id: row.get(0)?,
                name: row.get(1)?,
                active: row.get::<_, i32>(2)? != 0,
                ra_hours: row.get(3)?,
                dec_degrees: row.get(4)?,
                epoch_code: row.get(5)?,
                rotation: row.get(6)?,
                roi: row.get(7)?,
                exposure_plans: Vec::new(),
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut plans = plans_for_project(conn, project_id)?;
    for target in &mut targets {
        target.exposure_plans = plans.remove(&target.id).unwrap_or_default();
    }
    Ok(targets)
}

fn plans_for_project(
    conn: &Connection,
    project_id: i32,
) -> rusqlite::Result<HashMap<i32, Vec<ExposurePlanResponse>>> {
    let enabled = if has_column(conn, "exposureplan", "enabled") {
        "COALESCE(ep.enabled, 1)"
    } else {
        "1"
    };
    let sql = format!(
        "SELECT ep.targetid, ep.Id, ep.exposureTemplateId, et.name, et.filtername,
                et.gain, et.offset, et.bin, et.readoutmode, ep.exposure,
                COALESCE(ep.desired, 0), COALESCE(ep.acquired, 0),
                COALESCE(ep.accepted, 0), {enabled}
         FROM exposureplan ep
         JOIN exposuretemplate et ON et.Id = ep.exposureTemplateId
         JOIN target t ON t.Id = ep.targetid
         WHERE t.projectid = ? ORDER BY et.filtername, ep.exposure, ep.Id"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([project_id], |row| {
        Ok((
            row.get::<_, i32>(0)?,
            ExposurePlanResponse {
                id: row.get(1)?,
                exposure_template_id: row.get(2)?,
                template_name: row.get(3)?,
                filter_name: row.get(4)?,
                gain: row.get(5)?,
                offset: row.get(6)?,
                bin: row.get(7)?,
                readout_mode: row.get(8)?,
                exposure: row.get(9)?,
                desired: row.get(10)?,
                acquired: row.get(11)?,
                accepted: row.get(12)?,
                enabled: row.get::<_, i32>(13)? != 0,
            },
        ))
    })?;
    let mut plans = HashMap::<i32, Vec<ExposurePlanResponse>>::new();
    for row in rows {
        let (target_id, plan) = row?;
        plans.entry(target_id).or_default().push(plan);
    }
    Ok(plans)
}

fn plans_for_target(
    conn: &Connection,
    target_id: i32,
) -> rusqlite::Result<Vec<ExposurePlanResponse>> {
    let enabled = if has_column(conn, "exposureplan", "enabled") {
        "COALESCE(ep.enabled, 1)"
    } else {
        "1"
    };
    let sql = format!(
        "SELECT ep.Id, ep.exposureTemplateId, et.name, et.filtername,
                et.gain, et.offset, et.bin, et.readoutmode, ep.exposure,
                COALESCE(ep.desired, 0), COALESCE(ep.acquired, 0),
                COALESCE(ep.accepted, 0), {enabled}
         FROM exposureplan ep
         JOIN exposuretemplate et ON et.Id = ep.exposureTemplateId
         WHERE ep.targetid = ? ORDER BY et.filtername, ep.exposure, ep.Id"
    );
    let mut stmt = conn.prepare(&sql)?;
    stmt.query_map([target_id], |row| {
        Ok(ExposurePlanResponse {
            id: row.get(0)?,
            exposure_template_id: row.get(1)?,
            template_name: row.get(2)?,
            filter_name: row.get(3)?,
            gain: row.get(4)?,
            offset: row.get(5)?,
            bin: row.get(6)?,
            readout_mode: row.get(7)?,
            exposure: row.get(8)?,
            desired: row.get(9)?,
            acquired: row.get(10)?,
            accepted: row.get(11)?,
            enabled: row.get::<_, i32>(12)? != 0,
        })
    })?
    .collect()
}

pub async fn get_project_scheduler(
    ctx: DbContext,
    Path((_db_id, project_id)): Path<(String, i32)>,
) -> Result<Json<ApiResponse<ProjectSchedulerResponse>>, AppError> {
    let conn = ctx.db();
    let conn = conn.lock().map_err(AppError::db)?;
    let mut project = project_details(&conn, project_id)
        .map_err(AppError::db)?
        .ok_or_else(|| AppError::BadRequest(format!("project {project_id} not found")))?;
    project.exposure_templates =
        templates_for_project(&conn, &project.profile_id, project_id).map_err(AppError::db)?;
    project.targets = targets_for_project(&conn, project_id).map_err(AppError::db)?;
    Ok(Json(ApiResponse::success(project)))
}

pub fn update_project(
    ctx: DbContext,
    project_id: i32,
    req: UpdateProjectRequest,
) -> Result<(), AppError> {
    if let Some(name) = &req.name
        && name.trim().is_empty()
    {
        return Err(AppError::BadRequest("name must not be empty".into()));
    }
    if req.state.is_some_and(|value| !(0..=3).contains(&value)) {
        return Err(AppError::BadRequest("state must be between 0 and 3".into()));
    }
    if req.priority.is_some_and(|value| !(0..=2).contains(&value)) {
        return Err(AppError::BadRequest(
            "priority must be between 0 and 2".into(),
        ));
    }
    if req
        .minimum_altitude
        .is_some_and(|value| !(-90.0..=90.0).contains(&value))
        || req
            .maximum_altitude
            .is_some_and(|value| !(-90.0..=90.0).contains(&value))
    {
        return Err(AppError::BadRequest(
            "altitude must be between -90 and 90 degrees".into(),
        ));
    }
    if req.minimum_time.is_some_and(|value| value < 0)
        || req.meridian_window.is_some_and(|value| value < 0)
        || req.filter_switch_frequency.is_some_and(|value| value < 0)
        || req.dither_every.is_some_and(|value| value < 0)
    {
        return Err(AppError::BadRequest(
            "time, window, filter switch, and dither values must not be negative".into(),
        ));
    }
    let conn = ctx.db();
    let mut conn = conn.lock().map_err(AppError::db)?;
    if req.maximum_altitude.is_some() && !has_column(&conn, "project", "maximumAltitude") {
        return Err(AppError::BadRequest(
            "maximum altitude requires a newer Target Scheduler schema".into(),
        ));
    }
    if req.is_mosaic.is_some() && !has_column(&conn, "project", "isMosaic") {
        return Err(AppError::BadRequest(
            "mosaic projects require a newer Target Scheduler schema".into(),
        ));
    }
    let tx = conn.transaction().map_err(AppError::db)?;
    let exists = tx
        .query_row("SELECT 1 FROM project WHERE Id = ?", [project_id], |_| {
            Ok(())
        })
        .optional()
        .map_err(AppError::db)?
        .is_some();
    if !exists {
        return Err(AppError::BadRequest(format!(
            "project {project_id} not found"
        )));
    }
    tx.execute(
        "UPDATE project SET
            name = COALESCE(?1, name), description = COALESCE(?2, description),
            state = COALESCE(?3, state), priority = COALESCE(?4, priority),
            minimumtime = COALESCE(?5, minimumtime),
            minimumaltitude = COALESCE(?6, minimumaltitude),
            usecustomhorizon = COALESCE(?7, usecustomhorizon),
            horizonoffset = COALESCE(?8, horizonoffset),
            meridianwindow = COALESCE(?9, meridianwindow),
            filterswitchfrequency = COALESCE(?10, filterswitchfrequency),
            ditherevery = COALESCE(?11, ditherevery),
            enablegrader = COALESCE(?12, enablegrader)
         WHERE Id = ?13",
        params![
            req.name.as_deref().map(str::trim),
            req.description,
            req.state,
            req.priority,
            req.minimum_time,
            req.minimum_altitude,
            req.use_custom_horizon.map(i32::from),
            req.horizon_offset,
            req.meridian_window,
            req.filter_switch_frequency,
            req.dither_every,
            req.enable_grader.map(i32::from),
            project_id,
        ],
    )
    .map_err(AppError::db)?;
    if let Some(maximum_altitude) = req.maximum_altitude {
        tx.execute(
            "UPDATE project SET maximumAltitude = ? WHERE Id = ?",
            params![maximum_altitude, project_id],
        )
        .map_err(AppError::db)?;
    }
    if let Some(is_mosaic) = req.is_mosaic {
        tx.execute(
            "UPDATE project SET isMosaic = ? WHERE Id = ?",
            params![i32::from(is_mosaic), project_id],
        )
        .map_err(AppError::db)?;
    }
    tx.commit().map_err(AppError::db)?;
    Ok(())
}

pub fn update_target_fields(
    db: &Database<'_>,
    target_id: i32,
    req: &UpdateTargetRequest,
) -> Result<(), AppError> {
    if req
        .ra_hours
        .is_some_and(|value| !value.is_finite() || !(0.0..24.0).contains(&value))
    {
        return Err(AppError::BadRequest(
            "RA must be at least 0 and less than 24 hours".into(),
        ));
    }
    if req
        .dec_degrees
        .is_some_and(|value| !value.is_finite() || !(-90.0..=90.0).contains(&value))
    {
        return Err(AppError::BadRequest(
            "Dec must be between -90 and 90 degrees".into(),
        ));
    }
    if req
        .roi
        .is_some_and(|value| !value.is_finite() || value <= 0.0)
    {
        return Err(AppError::BadRequest("ROI must be greater than zero".into()));
    }
    if req.rotation.is_some_and(|value| !value.is_finite()) {
        return Err(AppError::BadRequest("rotation must be finite".into()));
    }
    let changed = db
        .update_target_scheduler_fields(
            target_id,
            req.active,
            req.ra_hours,
            req.dec_degrees,
            req.epoch_code,
            req.rotation,
            req.roi,
        )
        .map_err(AppError::db)?;
    if !changed {
        return Err(AppError::BadRequest(format!(
            "target {target_id} not found"
        )));
    }
    Ok(())
}

pub async fn create_exposure_plan(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db_id, target_id)): Path<(String, i32)>,
    Json(req): Json<CreateExposurePlanRequest>,
) -> Result<Json<ApiResponse<ExposurePlanResponse>>, AppError> {
    require_database_management_allowed(&state)?;
    let filter_name = req.filter_name.as_deref().unwrap_or_default().trim();
    if req.exposure_template_id.is_none() && filter_name.is_empty() {
        return Err(AppError::BadRequest("filter name must not be empty".into()));
    }
    validate_plan_values(req.exposure, req.desired)?;
    let bin = req.bin.unwrap_or(1);
    if req.exposure_template_id.is_none() && bin <= 0 {
        return Err(AppError::BadRequest("bin must be greater than zero".into()));
    }
    let gain = req.gain.unwrap_or(-1);
    let offset = req.offset.unwrap_or(-1);
    let readout_mode = req.readout_mode.unwrap_or(-1);

    let conn = ctx.db();
    let mut conn = conn.lock().map_err(AppError::db)?;
    if !has_column(&conn, "exposureplan", "guid")
        || !has_column(&conn, "exposuretemplate", "guid")
        || !has_column(&conn, "exposureplan", "enabled")
    {
        return Err(AppError::BadRequest(
            "creating exposure plans requires Target Scheduler schema 22 or newer".into(),
        ));
    }
    let tx = conn.transaction().map_err(AppError::db)?;
    let (profile_id, project_id): (String, i32) = tx
        .query_row(
            "SELECT p.profileId, p.Id
             FROM target t JOIN project p ON p.Id = t.projectid WHERE t.Id = ?",
            [target_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(AppError::db)?
        .ok_or_else(|| AppError::BadRequest(format!("target {target_id} not found")))?;
    let template_id: Option<i32> = if let Some(template_id) = req.exposure_template_id {
        let id = tx
            .query_row(
                "SELECT et.Id FROM exposuretemplate et
                 WHERE et.Id = ?1 AND (
                    et.profileId = ?2 OR EXISTS (
                        SELECT 1 FROM exposureplan linked_ep
                        JOIN target linked_target ON linked_target.Id = linked_ep.targetid
                        WHERE linked_ep.exposureTemplateId = et.Id
                          AND linked_target.projectid = ?3
                    )
                 )",
                params![template_id, profile_id, project_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(AppError::db)?
            .ok_or_else(|| {
                AppError::BadRequest(format!(
                    "exposure template {template_id} is not available to the target project"
                ))
            })?;
        Some(id)
    } else {
        tx.query_row(
            "SELECT Id FROM exposuretemplate
             WHERE profileId = ?1 AND filtername = ?2 AND gain IS ?3
               AND offset IS ?4 AND IFNULL(bin, 1) = ?5 AND readoutmode IS ?6
             ORDER BY Id LIMIT 1",
            params![profile_id, filter_name, gain, offset, bin, readout_mode],
            |row| row.get(0),
        )
        .optional()
        .map_err(AppError::db)?
    };
    let template_id = if let Some(id) = template_id {
        id
    } else {
        let template_name = req
            .template_name
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .unwrap_or(filter_name);
        tx.execute(
            "INSERT INTO exposuretemplate (
                profileId, name, filtername, gain, offset, bin, readoutmode,
                twilightlevel, moonavoidanceenabled, moonavoidanceseparation,
                moonavoidancewidth, maximumhumidity, defaultexposure,
                moonrelaxscale, moonrelaxmaxaltitude, moonrelaxminaltitude,
                moondownenabled, ditherevery, minutesOffset, guid
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, 0, 60, 7, 0, ?8, 0, 5, -15, 0, -1, 0, ?9)",
            params![
                profile_id,
                template_name,
                filter_name,
                gain,
                offset,
                bin,
                readout_mode,
                60.0,
                crate::ts_schema::new_guid()
            ],
        )
        .map_err(AppError::db)?;
        tx.last_insert_rowid() as i32
    };
    tx.execute(
        "INSERT INTO exposureplan (
            profileId, exposure, desired, acquired, accepted, targetid,
            exposureTemplateId, enabled, guid
         ) VALUES (?1, ?2, ?3, 0, 0, ?4, ?5, ?6, ?7)",
        params![
            profile_id,
            req.exposure,
            req.desired,
            target_id,
            template_id,
            i32::from(req.enabled),
            crate::ts_schema::new_guid()
        ],
    )
    .map_err(AppError::db)?;
    let plan_id = tx.last_insert_rowid() as i32;
    tx.commit().map_err(AppError::db)?;

    let plan = plans_for_target(&conn, target_id)
        .map_err(AppError::db)?
        .into_iter()
        .find(|plan| plan.id == plan_id)
        .ok_or_else(|| {
            AppError::InternalError("created exposure plan could not be reloaded".into())
        })?;
    Ok(Json(ApiResponse::success(plan)))
}

pub async fn update_exposure_plan(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db_id, plan_id)): Path<(String, i32)>,
    Json(req): Json<UpdateExposurePlanRequest>,
) -> Result<Json<ApiResponse<serde_json::Value>>, AppError> {
    require_database_management_allowed(&state)?;
    validate_plan_values(req.exposure, req.desired)?;
    let conn = ctx.db();
    let conn = conn.lock().map_err(AppError::db)?;
    if !has_column(&conn, "exposureplan", "enabled") {
        return Err(AppError::BadRequest(
            "enabling exposure plans requires a newer Target Scheduler schema".into(),
        ));
    }
    let changed = conn
        .execute(
            "UPDATE exposureplan SET exposure = ?, desired = ?, enabled = ? WHERE Id = ?",
            params![req.exposure, req.desired, i32::from(req.enabled), plan_id],
        )
        .map_err(AppError::db)?;
    if changed == 0 {
        return Err(AppError::BadRequest(format!(
            "exposure plan {plan_id} not found"
        )));
    }
    Ok(Json(ApiResponse::success(
        serde_json::json!({ "updated": true }),
    )))
}

fn validate_plan_values(exposure: f64, desired: i32) -> Result<(), AppError> {
    if !exposure.is_finite() || (exposure != -1.0 && exposure <= 0.0) {
        return Err(AppError::BadRequest(
            "exposure must be -1 for the template default or greater than zero".into(),
        ));
    }
    if desired < 0 {
        return Err(AppError::BadRequest("desired must not be negative".into()));
    }
    Ok(())
}
