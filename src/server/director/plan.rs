//! Plan drafts and the rig templates they bind to. Objectives say what the
//! project wants; contributions say which rig shoots it with which Target
//! Scheduler template. Nothing here writes a rig database or authorizes work.

use super::*;
use psf_guard_director_core::bandpass::{bandpass_for_filter, Bandpass};
use psf_guard_director_meta::plan::PlanDraft;
use psf_guard_director_meta::SYNC_PLACED_PROFILE;
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
    pub(super) moon: psf_guard_director_core::moon::MoonPolicy,
    pub(super) bandpass: Bandpass,
}

#[derive(Serialize)]
pub(super) struct TemplateList {
    catalog_slug: String,
    catalog_name: String,
    rig: Option<NamedIdentity>,
    templates: Vec<Template>,
    /// Templates left out because Director cannot plan with them, named.
    warnings: Vec<String>,
    /// Templates of the database's other N.I.N.A. profiles, left out: the
    /// rig shoots with its own profile's.
    other_profile_templates: usize,
}

/// A rig database's templates, and the ones left out of them.
pub(super) struct Templates {
    pub(super) usable: Vec<Template>,
    pub(super) unusable: Vec<Unusable>,
}

/// A template whose Moon avoidance is on with settings Director's planner
/// refuses, such as a relax range that ends below where it starts.
pub(super) struct Unusable {
    pub(super) id: i64,
    pub(super) name: String,
}

impl Unusable {
    pub(super) fn warning(&self) -> String {
        format!(
            "Template {}: its Moon avoidance settings are outside what Director supports, so it is left out. Fix them in Target Scheduler.",
            self.name
        )
    }
}

/// The exposure templates a rig shoots with ([`rig_templates`]), with the
/// bandpass each filter name resolves to. Read-only; no template is created.
pub(super) async fn templates(
    State(state): State<Arc<AppState>>,
    Path(slug): Path<String>,
) -> Result<Json<ApiResponse<TemplateList>>, Error> {
    let service = enabled(&state)?;
    let catalog = state.get_database(&slug).ok_or(Error::Missing)?;
    let list = service
        .clone()
        .with_reader(move |store| {
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
            let rig = store.catalog_rig(identity.id)?.map(|binding| binding.rig);
            let (templates, other_profile_templates) = rig_templates(&connection, &catalog.id)?;
            Ok::<_, Error>(TemplateList {
                catalog_slug: catalog.id.clone(),
                catalog_name: catalog.name.clone(),
                rig,
                templates: templates.usable,
                warnings: templates.unusable.iter().map(Unusable::warning).collect(),
                other_profile_templates,
            })
        })
        .await?;
    Ok(Json(ApiResponse::success(list)))
}

/// The N.I.N.A. profile a rig's templates come from: the one activation
/// writes the rig's projects under, which its Sync plugin reports, else the
/// one most of the database's projects use. `None` when the database cannot
/// tell, as one without a projects table, or when that is PSF Guard's own
/// waiting profile, which holds only what PSF Guard wrote.
pub(super) fn rig_profile(connection: &Connection, catalog_id: &str) -> Option<String> {
    let reported = crate::server::rig_plugins::reported_profile(catalog_id);
    match super::activation::resolve_profile(connection, reported.as_deref()) {
        Ok(Some(profile)) if !profile.eq_ignore_ascii_case(SYNC_PLACED_PROFILE) => Some(profile),
        Ok(_) => None,
        Err(error) => {
            tracing::debug!("{catalog_id}: no N.I.N.A. profile for its templates: {error}");
            None
        }
    }
}

/// The templates a rig shoots with, and how many of the database's other
/// N.I.N.A. profiles' templates were left out. One Target Scheduler database
/// may serve several profiles with templates of the same names but other
/// settings. A rig uses its own profile's, and those PSF Guard wrote under
/// its waiting profile; without a known profile, every template.
pub(super) fn rig_templates(
    connection: &Connection,
    catalog_id: &str,
) -> Result<(Templates, usize), Error> {
    let profile = rig_profile(connection, catalog_id);
    let templates = read_templates_of(connection, profile.as_deref())?;
    let Some(profile) = profile else {
        return Ok((templates, 0));
    };
    let others: i64 = connection
        .query_row(
            "SELECT count(*) FROM exposuretemplate
             WHERE TRIM(IFNULL(filtername, '')) <> ''
               AND lower(IFNULL(profileId, '')) NOT IN (lower(?1), lower(?2))",
            [profile.as_str(), SYNC_PLACED_PROFILE],
            |row| row.get(0),
        )
        .map_err(StoreError::from)?;
    Ok((templates, usize::try_from(others).unwrap_or(0)))
}

/// Every template with a filter. One whose Moon avoidance cannot be planned
/// is set apart by name rather than failing the rig's whole list.
pub(super) fn read_templates(connection: &Connection) -> Result<Templates, Error> {
    read_templates_of(connection, None)
}

/// [`read_templates`], of one N.I.N.A. profile and PSF Guard's waiting one
/// when `profile` is given.
fn read_templates_of(connection: &Connection, profile: Option<&str>) -> Result<Templates, Error> {
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
        return Ok(Templates {
            usable: vec![],
            unusable: vec![],
        });
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
             FROM exposuretemplate
             WHERE ?1 IS NULL OR lower(IFNULL(profileId, '')) IN (lower(?1), lower(?2))
             ORDER BY filtername, name, Id LIMIT 512"
        ))
        .map_err(StoreError::from)?;
    let rows = statement
        .query_map(rusqlite::params![profile, SYNC_PLACED_PROFILE], |row| {
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
    let mut templates = Templates {
        usable: Vec::new(),
        unusable: Vec::new(),
    };
    for row in rows {
        let (id, guid, profile_id, name, filter_name, gain, offset, bin, readout_mode, exposure) =
            row.map_err(StoreError::from)?;
        let filter_name = filter_name.unwrap_or_default();
        if filter_name.trim().is_empty() {
            continue;
        }
        let name = name.unwrap_or_else(|| filter_name.clone());
        let moon = match read_moon_policy(connection, id) {
            Ok(moon) => moon,
            Err(error) if is_unusable_moon(&error) => {
                templates.unusable.push(Unusable { id, name });
                continue;
            }
            Err(error) => return Err(StoreError::from(error).into()),
        };
        templates.usable.push(Template {
            id,
            guid: guid.and_then(|value| Uuid::parse_str(&value).ok()),
            profile_id: profile_id.unwrap_or_default(),
            name,
            bandpass: bandpass_for_filter(&filter_name),
            filter_name,
            gain: gain.filter(|value| *value >= 0),
            offset: offset.filter(|value| *value >= 0),
            bin: bin.filter(|value| *value > 0),
            readout_mode: readout_mode.filter(|value| *value >= 0),
            default_exposure: exposure.filter(|value| *value > 0.0).unwrap_or(60.0),
            moon,
        });
    }
    Ok(templates)
}

const UNUSABLE_MOON: &str = "Invalid exposure template Moon avoidance settings";

/// Whether `read_moon_policy` failed on the template's settings rather than
/// on the database.
pub(super) fn is_unusable_moon(error: &rusqlite::Error) -> bool {
    matches!(error, rusqlite::Error::FromSqlConversionFailure(_, _, inner) if inner.to_string() == UNUSABLE_MOON)
}

pub(super) fn read_moon_policy(
    connection: &Connection,
    id: i64,
) -> rusqlite::Result<psf_guard_director_core::moon::MoonPolicy> {
    let columns = connection
        .prepare("PRAGMA table_info(exposuretemplate)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    let values = [
        ("moonavoidanceenabled", "0"),
        ("moonavoidanceseparation", "60"),
        ("moonavoidancewidth", "7"),
        ("moonrelaxscale", "0"),
        ("moonrelaxminaltitude", "-15"),
        ("moonrelaxmaxaltitude", "5"),
        ("moondownenabled", "0"),
    ]
    .map(|(name, default)| {
        if columns
            .iter()
            .any(|column| column.eq_ignore_ascii_case(name))
        {
            format!("COALESCE({name}, {default})")
        } else {
            default.to_owned()
        }
    });
    let mut policy = connection.query_row(
        &format!(
            "SELECT {} FROM exposuretemplate WHERE Id=?1",
            values.join(",")
        ),
        [id],
        |row| {
            Ok(psf_guard_director_core::moon::MoonPolicy {
                enabled: row.get::<_, i64>(0)? != 0,
                separation_degrees: row.get(1)?,
                width_days: row.get(2)?,
                relax_degrees_per_degree: row.get(3)?,
                relax_min_altitude_degrees: row.get(4)?,
                relax_max_altitude_degrees: row.get(5)?,
                moon_down: row.get::<_, i64>(6)? != 0,
            })
        },
    )?;
    // Target Scheduler ignores the other Moon fields while avoidance is off,
    // and so does Director's planner; values outside its ranges then stand
    // for nothing and read as the defaults.
    if !policy.enabled && policy.validate().is_err() {
        policy = psf_guard_director_core::moon::MoonPolicy {
            moon_down: policy.moon_down,
            ..Default::default()
        };
    }
    policy.validate().map_err(|_| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Real,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                UNUSABLE_MOON,
            )),
        )
    })?;
    Ok(policy)
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
        .query(move |store| {
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
