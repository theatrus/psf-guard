//! Plan drafts and the rig templates they bind to. Objectives say what the
//! project wants; contributions say which rig shoots it with which Target
//! Scheduler template. Nothing here writes a rig database or authorizes work.

use super::*;
use psf_guard_director_core::bandpass::{bandpass_for_filter, Bandpass};
use psf_guard_director_meta::plan::PlanDraft;
use rusqlite::{Connection, OpenFlags};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Serialize)]
pub(super) struct Template {
    pub(super) id: i64,
    pub(super) guid: Option<Uuid>,
    pub(super) profile_id: String,
    pub(super) name: String,
    pub(super) filter_name: String,
    pub(super) gain: Option<i32>,
    pub(super) offset: Option<i32>,
    pub(super) bin: Option<i32>,
    pub(super) readout_mode: Option<i32>,
    pub(super) default_exposure: f64,
    pub(super) bandpass: Bandpass,
}

#[derive(Serialize)]
pub(super) struct TemplateList {
    catalog_slug: String,
    catalog_name: String,
    rig: Option<NamedIdentity>,
    templates: Vec<Template>,
}

/// Every exposure template in the rig database, across profiles, with the
/// bandpass its filter name resolves to. Read-only; no template is created.
pub(super) async fn templates(
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
) -> Result<Json<ApiResponse<TemplateList>>, Error> {
    let service = enabled(&state)?;
    let catalog = state.get_database(&slug).ok_or(Error::Missing)?;
    let catalog_permit = admit(&service.discovery_admission).await?;
    let metadata_permit = admit(&service.admission).await?;
    let list = tokio::task::spawn_blocking(move || {
        let _permits = (catalog_permit, metadata_permit);
        let connection = super::super::database_context::open_scheduler_connection_with_flags(
            FilePath::new(&catalog.database_path),
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(StoreError::from)
        .map_err(Error::from)?;
        connection
            .busy_timeout(Duration::from_secs(2))
            .map_err(StoreError::from)
            .map_err(Error::from)?;
        let identity = crate::catalog_identity::read(&connection)?.unwrap_or_else(|| {
            super::derived_identity(service.instance_id, &catalog.database_path)
        });
        let rig = {
            let store = service.store.lock().map_err(|_| Error::Internal)?;
            store.catalog_rig(identity.id)?.map(|binding| binding.rig)
        };
        let templates = read_templates(&connection)?;
        Ok::<_, Error>(TemplateList {
            catalog_slug: catalog.id.clone(),
            catalog_name: catalog.name.clone(),
            rig,
            templates,
        })
    })
    .await
    .map_err(|error| {
        tracing::error!(%error, "Director template listing failed");
        Error::Internal
    })??;
    Ok(Json(ApiResponse::success(list)))
}

pub(super) fn read_templates(connection: &Connection) -> Result<Vec<Template>, Error> {
    let has = |column: &str| {
        connection
            .prepare("PRAGMA table_info(exposuretemplate)")
            .and_then(|mut statement| {
                let names = statement
                    .query_map([], |row| row.get::<_, String>(1))?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(names.iter().any(|name| name.eq_ignore_ascii_case(column)))
            })
            .unwrap_or(false)
    };
    if !has("filtername") {
        return Ok(vec![]);
    }
    let guid = if has("guid") { "guid" } else { "NULL" };
    let readout = if has("readoutmode") {
        "readoutmode"
    } else {
        "NULL"
    };
    let default_exposure = if has("defaultexposure") {
        "COALESCE(defaultexposure, 60)"
    } else {
        "60"
    };
    let mut statement = connection
        .prepare(&format!(
            "SELECT Id, {guid}, profileId, name, filtername, gain, offset, bin, {readout}, {default_exposure}
             FROM exposuretemplate ORDER BY filtername, name, Id LIMIT 512"
        ))
        .map_err(StoreError::from)?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<i32>>(5)?,
                row.get::<_, Option<i32>>(6)?,
                row.get::<_, Option<i32>>(7)?,
                row.get::<_, Option<i32>>(8)?,
                row.get::<_, Option<f64>>(9)?,
            ))
        })
        .map_err(StoreError::from)?;
    let mut templates = Vec::new();
    for row in rows {
        let (id, guid, profile_id, name, filter_name, gain, offset, bin, readout_mode, exposure) =
            row.map_err(StoreError::from)?;
        let filter_name = filter_name.unwrap_or_default();
        if filter_name.trim().is_empty() {
            continue;
        }
        templates.push(Template {
            id,
            guid: guid.and_then(|value| Uuid::parse_str(&value).ok()),
            profile_id: profile_id.unwrap_or_default(),
            name: name.unwrap_or_else(|| filter_name.clone()),
            bandpass: bandpass_for_filter(&filter_name),
            filter_name,
            gain: gain.filter(|value| *value >= 0),
            offset: offset.filter(|value| *value >= 0),
            bin: bin.filter(|value| *value > 0),
            readout_mode: readout_mode.filter(|value| *value >= 0),
            default_exposure: exposure.filter(|value| *value > 0.0).unwrap_or(60.0),
        });
    }
    Ok(templates)
}

#[derive(Serialize)]
pub(super) struct PlanView {
    project: NamedIdentity,
    plan: Option<PlanDraft>,
}

pub(super) async fn get_plan(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<Json<ApiResponse<PlanView>>, Error> {
    let view = enabled(&state)?
        .run(move |store| {
            let project = store.project(id)?.ok_or(StoreError::NotFound)?;
            let plan = store.plan_draft(id)?;
            Ok(PlanView { project, plan })
        })
        .await?;
    Ok(Json(ApiResponse::success(view)))
}

pub(super) async fn put_plan(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Json(mut plan): Json<PlanDraft>,
) -> Result<Json<ApiResponse<PlanView>>, Error> {
    if plan.project_id != id {
        return Err(Error::Invalid);
    }
    plan.updated_at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let view = enabled(&state)?
        .run(move |store| {
            let project = store.project(id)?.ok_or(StoreError::NotFound)?;
            let expected = plan.revision;
            let saved = store.save_plan_draft(&plan, expected)?;
            Ok(PlanView {
                project,
                plan: Some(saved),
            })
        })
        .await?;
    Ok(Json(ApiResponse::success(view)))
}
