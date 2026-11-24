//! Target Scheduler project, target, and exposure-plan views and edits.

use axum::{
    extract::{Path, State},
    Json,
};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

use crate::db::Database;
use crate::server::api::{ApiResponse, UpdateProjectRequest, UpdateTargetRequest};
use crate::server::extract::DbContext;
use crate::server::handlers::AppError;
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
    /// Target Scheduler flats automation for this project: 0 = off,
    /// 1..N = after every N target sessions, 100 = on target completion,
    /// 200 = immediately after the session.
    pub flats_handling: i32,
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

/// Every setting of an exposure template a person may change. The filter
/// stays: exposure plans and frames already captured depend on it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateExposureTemplateRequest {
    pub name: String,
    /// None, or -1, uses the camera's default.
    pub gain: Option<i32>,
    pub offset: Option<i32>,
    pub bin: i32,
    pub readout_mode: Option<i32>,
    pub default_exposure: f64,
    /// 0 = nighttime, 1 = astronomical, 2 = nautical, 3 = civil.
    pub twilight_level: i32,
    /// Minutes the twilight limit moves: later at dusk, earlier at dawn.
    pub minutes_offset: i32,
    /// 0 turns the humidity limit off.
    pub maximum_humidity: f64,
    /// -1 uses the project's dither setting.
    pub dither_every: i32,
    pub moon_avoidance_enabled: bool,
    pub moon_avoidance_separation: f64,
    pub moon_avoidance_width: i32,
    pub moon_relax_scale: f64,
    pub moon_relax_min_altitude: f64,
    pub moon_relax_max_altitude: f64,
    pub moon_down_enabled: bool,
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
    let flats_handling = if has_column(conn, "project", "flatsHandling") {
        "COALESCE(flatsHandling, 0)"
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
                COALESCE(enablegrader, 1), {is_mosaic}, {flats_handling}
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
            flats_handling: row.get(19)?,
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
    read_templates(
        conn,
        "WHERE et.profileId = ?1
            OR EXISTS (
                SELECT 1 FROM exposureplan linked_ep
                JOIN target linked_target ON linked_target.Id = linked_ep.targetid
                WHERE linked_ep.exposureTemplateId = et.Id
                  AND linked_target.projectid = ?2
            )",
        params![profile_id, project_id],
    )
}

/// The exposure template columns older Target Scheduler schemas may lack,
/// in the order `read_templates` selects them: the column, the value an
/// absent or empty one reads as, and what the setting is called.
const OPTIONAL_TEMPLATE_COLUMNS: [(&str, &str, &str); 12] = [
    ("twilightlevel", "0", "twilight"),
    ("moonavoidanceenabled", "0", "Moon avoidance"),
    ("moonavoidanceseparation", "60", "Moon separation"),
    ("moonavoidancewidth", "7", "Moon avoidance width"),
    ("maximumhumidity", "0", "maximum humidity"),
    ("defaultexposure", "60", "default exposure"),
    ("moonrelaxscale", "0", "Moon relaxation"),
    (
        "moonrelaxmaxaltitude",
        "5",
        "upper Moon relaxation altitude",
    ),
    (
        "moonrelaxminaltitude",
        "-15",
        "lower Moon relaxation altitude",
    ),
    ("moondownenabled", "0", "Moon must be down"),
    ("ditherevery", "-1", "dither every"),
    ("minutesOffset", "0", "twilight offset"),
];

/// The exposure template table's column names, in lower case.
fn template_columns(conn: &Connection) -> rusqlite::Result<std::collections::HashSet<String>> {
    conn.prepare("PRAGMA table_info(exposuretemplate)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .map(|name| name.map(|name| name.to_ascii_lowercase()))
        .collect()
}

/// Exposure templates matching `filter`, a `WHERE` clause over `et`, with
/// how many exposure plans use each.
fn read_templates(
    conn: &Connection,
    filter: &str,
    values: impl rusqlite::Params,
) -> rusqlite::Result<Vec<ExposureTemplateResponse>> {
    let columns = template_columns(conn)?;
    let optional = OPTIONAL_TEMPLATE_COLUMNS
        .iter()
        .map(|(column, default, _)| {
            if columns.contains(&column.to_ascii_lowercase()) {
                format!("COALESCE(et.{column}, {default})")
            } else {
                (*default).to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join(",\n                ");
    let mut stmt = conn.prepare(&format!(
        "SELECT et.Id, et.profileId, et.name, et.filtername,
                et.gain, et.offset, et.bin, et.readoutmode,
                {optional},
                COUNT(ep.Id)
         FROM exposuretemplate et
         LEFT JOIN exposureplan ep ON ep.exposureTemplateId = et.Id
         {filter}
         GROUP BY et.Id
         ORDER BY et.filtername, et.name, et.Id"
    ))?;
    stmt.query_map(values, |row| {
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
    if let Some(flats_handling) = req.flats_handling {
        if !has_column(&conn, "project", "flatsHandling") {
            return Err(AppError::BadRequest(
                "flats handling requires a newer Target Scheduler schema".into(),
            ));
        }
        // Valid values per Target Scheduler: 0 = off, 1..=7 = session
        // cadence, 100 = target completion, 200 = immediate.
        if !((0..=7).contains(&flats_handling) || flats_handling == 100 || flats_handling == 200) {
            return Err(AppError::BadRequest(
                "flats handling must be 0-7, 100 (target completion), or 200 (immediate)".into(),
            ));
        }
    }
    // Immediate: this reads before it writes, and such a transaction cannot
    // wait for another connection's write lock (see SCHEDULER_BUSY_TIMEOUT).
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(AppError::db)?;
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
    if let Some(flats_handling) = req.flats_handling {
        tx.execute(
            "UPDATE project SET flatsHandling = ? WHERE Id = ?",
            params![flats_handling, project_id],
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
    State(_state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db_id, target_id)): Path<(String, i32)>,
    Json(req): Json<CreateExposurePlanRequest>,
) -> Result<Json<ApiResponse<ExposurePlanResponse>>, AppError> {
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
    // Immediate, as in `update_project`: a background pass may be writing.
    let tx = conn
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(AppError::db)?;
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
    State(_state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db_id, plan_id)): Path<(String, i32)>,
    Json(req): Json<UpdateExposurePlanRequest>,
) -> Result<Json<ApiResponse<serde_json::Value>>, AppError> {
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

/// `GET /api/db/{db_id}/exposure-templates`: every exposure template in
/// the database, for every N.I.N.A. profile.
pub async fn list_exposure_templates(
    ctx: DbContext,
) -> Result<Json<ApiResponse<Vec<ExposureTemplateResponse>>>, AppError> {
    let conn = ctx.db();
    let conn = conn.lock().map_err(AppError::db)?;
    let templates = read_templates(&conn, "", []).map_err(AppError::db)?;
    Ok(Json(ApiResponse::success(templates)))
}

/// `PUT /api/db/{db_id}/exposure-templates/{template_id}`: change a
/// template's capture, twilight, humidity, dither and Moon settings.
pub async fn update_exposure_template(
    ctx: DbContext,
    Path((_db_id, template_id)): Path<(String, i32)>,
    Json(req): Json<UpdateExposureTemplateRequest>,
) -> Result<Json<ApiResponse<ExposureTemplateResponse>>, AppError> {
    let name = validate_template(&req)?;
    let conn = ctx.db();
    let conn = conn.lock().map_err(AppError::db)?;
    let columns = template_columns(&conn).map_err(AppError::db)?;
    // The optional settings, in OPTIONAL_TEMPLATE_COLUMNS order.
    let optional: [rusqlite::types::Value; 12] = [
        i64::from(req.twilight_level).into(),
        i64::from(req.moon_avoidance_enabled).into(),
        req.moon_avoidance_separation.into(),
        i64::from(req.moon_avoidance_width).into(),
        req.maximum_humidity.into(),
        req.default_exposure.into(),
        req.moon_relax_scale.into(),
        req.moon_relax_max_altitude.into(),
        req.moon_relax_min_altitude.into(),
        i64::from(req.moon_down_enabled).into(),
        i64::from(req.dither_every).into(),
        i64::from(req.minutes_offset).into(),
    ];
    let mut assignments = vec![
        "name = ?".to_owned(),
        "gain = ?".to_owned(),
        "offset = ?".to_owned(),
        "bin = ?".to_owned(),
        "readoutmode = ?".to_owned(),
    ];
    let mut values: Vec<rusqlite::types::Value> = vec![
        name.into(),
        i64::from(req.gain.unwrap_or(-1)).into(),
        i64::from(req.offset.unwrap_or(-1)).into(),
        i64::from(req.bin).into(),
        i64::from(req.readout_mode.unwrap_or(-1)).into(),
    ];
    for ((column, default, label), value) in OPTIONAL_TEMPLATE_COLUMNS.iter().zip(optional) {
        if columns.contains(&column.to_ascii_lowercase()) {
            assignments.push(format!("{column} = ?"));
            values.push(value);
        } else if !same_value(&value, default) {
            // An older Target Scheduler has no such setting; storing
            // nothing would look like a save that did not happen.
            return Err(AppError::BadRequest(format!(
                "this Target Scheduler database has no {label} setting; update Target Scheduler in N.I.N.A. to change it"
            )));
        }
    }
    values.push(i64::from(template_id).into());
    let changed = conn
        .execute(
            &format!(
                "UPDATE exposuretemplate SET {} WHERE Id = ?",
                assignments.join(", ")
            ),
            rusqlite::params_from_iter(values),
        )
        .map_err(AppError::db)?;
    if changed == 0 {
        return Err(AppError::BadRequest(format!(
            "exposure template {template_id} not found"
        )));
    }
    let template = read_templates(&conn, "WHERE et.Id = ?1", [template_id])
        .map_err(AppError::db)?
        .pop()
        .ok_or_else(|| {
            AppError::BadRequest(format!("exposure template {template_id} not found"))
        })?;
    Ok(Json(ApiResponse::success(template)))
}

#[derive(Debug, Serialize)]
pub struct DeleteExposureTemplateResponse {
    pub id: i64,
    pub name: String,
    /// Whether syncs carry the deletion to other catalogs and the rig. A
    /// template without a GUID cannot be matched there.
    pub synced: bool,
}

/// Delete an exposure template no exposure plan uses. Syncs carry the
/// deletion to other catalogs and to the rig, which delete their copy when
/// no plan there uses it; see `crate::commands::template_removal`.
pub async fn delete_exposure_template(
    ctx: DbContext,
    Path((_db_id, template_id)): Path<(String, i64)>,
) -> Result<Json<ApiResponse<DeleteExposureTemplateResponse>>, AppError> {
    use crate::commands::template_removal::{remove, Refusal};
    let conn = ctx.db();
    let mut conn = conn.lock().map_err(AppError::db)?;
    match remove(&mut conn, template_id).map_err(|error| AppError::db(format!("{error:#}")))? {
        Ok(removed) => Ok(Json(ApiResponse::success(DeleteExposureTemplateResponse {
            id: removed.id,
            name: removed.name,
            synced: removed.synced,
        }))),
        Err(Refusal::Missing) => Err(AppError::NotFoundMessage(format!(
            "exposure template {template_id} not found"
        ))),
        Err(Refusal::InUse { name, plans }) => Err(AppError::Conflict(format!(
            "{name} is used by {plans} exposure {}; change or delete {} first",
            if plans == 1 { "plan" } else { "plans" },
            if plans == 1 {
                "that plan"
            } else {
                "those plans"
            },
        ))),
    }
}

/// Whether `value` is the setting an absent column reads as.
fn same_value(value: &rusqlite::types::Value, default: &str) -> bool {
    let Ok(default) = default.parse::<f64>() else {
        return false;
    };
    match value {
        rusqlite::types::Value::Integer(value) => *value as f64 == default,
        rusqlite::types::Value::Real(value) => *value == default,
        _ => false,
    }
}

/// The template's trimmed name, or why its settings are refused. The
/// limits are Target Scheduler's own.
fn validate_template(req: &UpdateExposureTemplateRequest) -> Result<String, AppError> {
    let refuse = |message: &str| Err(AppError::BadRequest(message.into()));
    let within =
        |value: f64, low: f64, high: f64| value.is_finite() && (low..=high).contains(&value);
    let name = req.name.trim();
    if name.is_empty() || name.chars().count() > 256 || name.chars().any(char::is_control) {
        return refuse("the template name must be 1 to 256 characters, with no line breaks");
    }
    for (label, value) in [
        ("gain", req.gain),
        ("offset", req.offset),
        ("readout mode", req.readout_mode),
    ] {
        if value.is_some_and(|value| value < -1) {
            return refuse(&format!(
                "{label} must be -1 for the camera default, or zero or more"
            ));
        }
    }
    if !(1..=16).contains(&req.bin) {
        return refuse("binning must be between 1 and 16");
    }
    if !req.default_exposure.is_finite() || req.default_exposure <= 0.0 {
        return refuse("the default exposure must be greater than zero");
    }
    if !(0..=3).contains(&req.twilight_level) {
        return refuse("twilight must be nighttime, astronomical, nautical or civil");
    }
    if !(-720..=720).contains(&req.minutes_offset) {
        return refuse("the twilight offset must be between -720 and 720 minutes");
    }
    if !within(req.maximum_humidity, 0.0, 100.0) {
        return refuse("maximum humidity must be between 0 and 100 percent");
    }
    if req.dither_every < -1 {
        return refuse("dither every must be -1 for the project setting, or zero or more");
    }
    // Target Scheduler ignores the Moon fields while avoidance is off, and a
    // template saved in N.I.N.A. may hold any values there.
    let moon = psf_guard_director_core::moon::MoonPolicy {
        enabled: req.moon_avoidance_enabled,
        separation_degrees: req.moon_avoidance_separation,
        width_days: f64::from(req.moon_avoidance_width),
        relax_degrees_per_degree: req.moon_relax_scale,
        relax_min_altitude_degrees: req.moon_relax_min_altitude,
        relax_max_altitude_degrees: req.moon_relax_max_altitude,
        moon_down: req.moon_down_enabled,
    };
    if moon.enabled && moon.validate().is_err() {
        return refuse(
            "Moon avoidance needs a separation of 0 to 180 degrees, a width of 1 to 14 whole days, \
             a relaxation of 0 to 180, and a lower Moon altitude below the upper one, both between -90 and 90 degrees",
        );
    }
    Ok(name.to_owned())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_details_reads_flats_handling_and_tolerates_old_schemas() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE project (
                Id INTEGER PRIMARY KEY, profileId TEXT, name TEXT,
                description TEXT, state INTEGER, priority INTEGER,
                createdate INTEGER, activedate INTEGER, inactivedate INTEGER,
                minimumtime INTEGER, minimumaltitude REAL,
                usecustomhorizon INTEGER, horizonoffset REAL,
                meridianwindow INTEGER, filterswitchfrequency INTEGER,
                ditherevery INTEGER, enablegrader INTEGER,
                flatsHandling INTEGER
             );
             INSERT INTO project VALUES
                (1, 'p', 'M 31', NULL, 1, 5, NULL, NULL, NULL,
                 30, 0, 0, 0, 0, 0, 0, 1, 100);",
        )
        .unwrap();
        let details = project_details(&conn, 1).unwrap().unwrap();
        assert_eq!(details.flats_handling, 100);

        // A pre-v11 schema has no flatsHandling column: read as Off.
        let old = Connection::open_in_memory().unwrap();
        old.execute_batch(
            "CREATE TABLE project (
                Id INTEGER PRIMARY KEY, profileId TEXT, name TEXT,
                description TEXT, state INTEGER, priority INTEGER,
                createdate INTEGER, activedate INTEGER, inactivedate INTEGER,
                minimumtime INTEGER, minimumaltitude REAL,
                usecustomhorizon INTEGER, horizonoffset REAL,
                meridianwindow INTEGER, filterswitchfrequency INTEGER,
                ditherevery INTEGER, enablegrader INTEGER
             );
             INSERT INTO project VALUES
                (1, 'p', 'M 31', NULL, 1, 5, NULL, NULL, NULL,
                 30, 0, 0, 0, 0, 0, 0, 1);",
        )
        .unwrap();
        let details = project_details(&old, 1).unwrap().unwrap();
        assert_eq!(details.flats_handling, 0);
    }
}

/// Exposure template edits through the HTTP API.
#[cfg(test)]
mod template_http_tests {
    use crate::server::state::AppState;
    use axum::{body::Body, http::Request, Router};
    use serde_json::{json, Value};
    use std::sync::Arc;
    use tower::ServiceExt;

    fn rig() -> (tempfile::TempDir, std::path::PathBuf, Router) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("rig.sqlite");
        let conn = rusqlite::Connection::open(&path).unwrap();
        crate::ts_schema::apply_schema(&conn).unwrap();
        conn.execute_batch(
            "INSERT INTO exposuretemplate (Id,profileId,name,filtername,gain,offset,bin,readoutmode,
                defaultexposure,twilightlevel,moonavoidanceenabled,moonavoidanceseparation,
                moonavoidancewidth,maximumhumidity,moonrelaxscale,moonrelaxmaxaltitude,
                moonrelaxminaltitude,moondownenabled,ditherevery,minutesOffset,guid)
             VALUES (1,'p','Ha 300','Ha',-1,-1,1,-1,300,0,0,60,7,0,0,5,-15,0,-1,0,'ha'),
                    (2,'other','OIII','OIII',100,30,1,-1,300,0,0,60,7,0,0,5,-15,0,-1,0,'oiii');",
        )
        .unwrap();
        drop(conn);
        let catalog = crate::server::database_context::DatabaseContext::new(
            "rig".into(),
            "Rig".into(),
            path.display().to_string(),
            vec![dir.path().display().to_string()],
            None,
            None,
            None,
            dir.path().join("cache"),
        )
        .unwrap();
        let state = AppState::new_for_test(rusqlite::Connection::open_in_memory().unwrap());
        {
            let mut databases = state.databases.write().unwrap();
            databases.clear();
            databases.insert(catalog.id.clone(), Arc::new(catalog));
        }
        let app = Router::new().nest("/api", crate::server::api_router(Arc::new(state)));
        (dir, path, app)
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

    fn edit() -> Value {
        json!({
            "name": " Ha 300 dark sky ", "gain": 139, "offset": 21, "bin": 2, "readout_mode": null,
            "default_exposure": 240.0, "twilight_level": 1, "minutes_offset": 15,
            "maximum_humidity": 85.0, "dither_every": 3,
            "moon_avoidance_enabled": true, "moon_avoidance_separation": 120.0,
            "moon_avoidance_width": 10, "moon_relax_scale": 2.0,
            "moon_relax_min_altitude": -10.0, "moon_relax_max_altitude": 10.0,
            "moon_down_enabled": true
        })
    }

    #[tokio::test]
    async fn a_template_lists_for_every_profile_and_saves_its_moon_settings() {
        let (_dir, path, app) = rig();
        let (status, body) = send(&app, "GET", "/api/db/rig/exposure-templates", Value::Null).await;
        assert_eq!(status, 200, "{body}");
        let names: Vec<_> = body["data"]
            .as_array()
            .unwrap()
            .iter()
            .map(|template| template["name"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(names, ["Ha 300", "OIII"]);

        let (status, body) = send(&app, "PUT", "/api/db/rig/exposure-templates/1", edit()).await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["data"]["name"], "Ha 300 dark sky");
        assert_eq!(body["data"]["moon_avoidance_separation"], 120.0);
        assert_eq!(body["data"]["filter_name"], "Ha");

        let row: String = rusqlite::Connection::open(&path)
            .unwrap()
            .query_row(
                "SELECT json_array(name, gain, bin, readoutmode, defaultexposure, twilightlevel,
                        maximumhumidity, moonavoidanceenabled, moonavoidancewidth,
                        moonrelaxscale, moonrelaxminaltitude, moondownenabled,
                        ditherevery, minutesOffset, filtername)
                 FROM exposuretemplate WHERE Id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&row).unwrap(),
            json!([
                "Ha 300 dark sky",
                139,
                2,
                -1,
                240.0,
                1,
                85.0,
                1,
                10,
                2.0,
                -10.0,
                1,
                3,
                15,
                "Ha"
            ])
        );
    }

    #[tokio::test]
    async fn a_template_edit_is_refused_out_of_range() {
        let (_dir, _path, app) = rig();
        let mut bad = edit();
        bad["moon_relax_min_altitude"] = json!(20.0);
        let (status, body) = send(&app, "PUT", "/api/db/rig/exposure-templates/1", bad).await;
        assert_eq!(status, 400, "{body}");
        assert!(
            body["error"]
                .as_str()
                .unwrap()
                .contains("lower Moon altitude below the upper one"),
            "{body}"
        );

        let mut filter = edit();
        filter["filter_name"] = json!("OIII");
        let (status, _) = send(&app, "PUT", "/api/db/rig/exposure-templates/1", filter).await;
        assert_eq!(status, 422, "the filter is not editable");

        let (status, _) = send(&app, "PUT", "/api/db/rig/exposure-templates/9", edit()).await;
        assert_eq!(status, 400);
    }

    #[tokio::test]
    async fn moon_values_count_only_with_avoidance_on() {
        let (_dir, path, app) = rig();
        let mut off = edit();
        off["moon_avoidance_enabled"] = json!(false);
        off["moon_relax_min_altitude"] = json!(20.0);
        let (status, body) = send(&app, "PUT", "/api/db/rig/exposure-templates/1", off).await;
        assert_eq!(status, 200, "{body}");
        let stored: f64 = rusqlite::Connection::open(&path)
            .unwrap()
            .query_row(
                "SELECT moonrelaxminaltitude FROM exposuretemplate WHERE Id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            stored, 20.0,
            "values Target Scheduler ignores are kept as sent"
        );
    }

    #[tokio::test]
    async fn an_older_schema_lists_templates_and_refuses_only_settings_it_cannot_store() {
        let (_dir, path, app) = rig();
        rusqlite::Connection::open(&path)
            .unwrap()
            .execute_batch("ALTER TABLE exposuretemplate DROP COLUMN minutesOffset")
            .unwrap();
        let (status, body) = send(&app, "GET", "/api/db/rig/exposure-templates", Value::Null).await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["data"][0]["minutes_offset"], 0);

        let (status, body) = send(&app, "PUT", "/api/db/rig/exposure-templates/1", edit()).await;
        assert_eq!(status, 400);
        assert!(
            body["error"]
                .as_str()
                .unwrap()
                .contains("no twilight offset setting"),
            "{body}"
        );

        let mut plain = edit();
        plain["minutes_offset"] = json!(0);
        let (status, body) = send(&app, "PUT", "/api/db/rig/exposure-templates/1", plain).await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(body["data"]["gain"], 139);
    }

    #[tokio::test]
    async fn deletes_an_unused_template_and_refuses_one_a_plan_uses() {
        let (_dir, path, app) = rig();
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "INSERT INTO project (Id, profileId, name, state, guid) VALUES (1, 'p', 'M31', 1, 'pg');
             INSERT INTO target (Id, name, active, epochcode, projectid, guid) VALUES (10, 'M31', 1, 0, 1, 'tg');
             INSERT INTO exposureplan (Id, profileId, exposure, desired, acquired, accepted, targetid, exposureTemplateId, guid)
               VALUES (100, 'p', 300, 10, 0, 0, 10, 1, 'eg');
             UPDATE exposuretemplate SET guid = '33333333-3333-4333-8333-333333333333' WHERE Id = 2;",
        )
        .unwrap();

        let (status, body) = send(
            &app,
            "DELETE",
            "/api/db/rig/exposure-templates/1",
            Value::Null,
        )
        .await;
        assert_eq!(status, 409, "{body}");
        assert_eq!(
            body["error"],
            "Ha 300 is used by 1 exposure plan; change or delete that plan first"
        );

        let (status, body) = send(
            &app,
            "DELETE",
            "/api/db/rig/exposure-templates/2",
            Value::Null,
        )
        .await;
        assert_eq!(status, 200, "{body}");
        assert_eq!(
            body["data"],
            json!({ "id": 2, "name": "OIII", "synced": true })
        );
        let recorded: String = conn
            .query_row(
                "SELECT template_guid FROM psf_guard_removed_template",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(recorded, "33333333-3333-4333-8333-333333333333");

        let (status, _) = send(
            &app,
            "DELETE",
            "/api/db/rig/exposure-templates/2",
            Value::Null,
        )
        .await;
        assert_eq!(status, 404);
        let (_, body) = send(&app, "GET", "/api/db/rig/exposure-templates", Value::Null).await;
        assert_eq!(body["data"].as_array().unwrap().len(), 1);
    }
}
