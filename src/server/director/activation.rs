//! Push a project's framing and plan into each participating rig database:
//! one Target Scheduler project, a target per panel, an exposure plan per rig
//! objective. Preview performs the same writes and rolls them back, so what
//! the operator reviews is what Apply commits. Captures and grades are never
//! touched; removed panels leave their targets behind.

use super::*;
use crate::ts_schema::new_guid;
use psf_guard_director_core::{
    bandpass::frames_for_hours,
    framing::{FramingRequest, Panel},
};
use psf_guard_director_meta::{
    activation::{ActivatedPlan, ActivatedRig, ActivatedTarget, Activation},
    catalog::ProjectMapping,
    framing::FramingDraft,
    plan::{Contribution, Goal, Objective, PlanDraft},
    CatalogIdentity,
};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, TransactionBehavior};
use std::{
    collections::BTreeMap,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Serialize)]
struct Change {
    kind: &'static str,
    action: &'static str,
    name: String,
    detail: String,
}

#[derive(Serialize)]
struct RigReport {
    rig: NamedIdentity,
    catalog_slug: Option<String>,
    catalog_name: String,
    profile_id: Option<String>,
    changes: Vec<Change>,
    warnings: Vec<String>,
    applied: bool,
}

#[derive(Serialize)]
pub(super) struct Report {
    project: NamedIdentity,
    framing_revision: u64,
    plan_revision: u64,
    panels: usize,
    rigs: Vec<RigReport>,
    warnings: Vec<String>,
    preview_digest: String,
    applied: bool,
    activation_revision: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Apply {
    preview_digest: String,
}

#[derive(Serialize)]
pub(super) struct Last {
    activation: Option<Activation>,
}

pub(super) enum ActivationError {
    Api(Error),
    NotReady(&'static str),
}
impl From<Error> for ActivationError {
    fn from(error: Error) -> Self {
        Self::Api(error)
    }
}
impl From<StoreError> for ActivationError {
    fn from(error: StoreError) -> Self {
        Self::Api(error.into())
    }
}
impl From<rusqlite::Error> for ActivationError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Api(StoreError::Sqlite(error).into())
    }
}
impl IntoResponse for ActivationError {
    fn into_response(self) -> Response {
        match self {
            Self::Api(error) => error.into_response(),
            Self::NotReady(message) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(ApiResponse::<()>::error(message.into())),
            )
                .into_response(),
        }
    }
}

pub(super) async fn last(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<Json<ApiResponse<Last>>, Error> {
    let activation = enabled(&state)?
        .run(move |store| {
            store.project(id)?.ok_or(StoreError::NotFound)?;
            store.activation(id)
        })
        .await?;
    Ok(Json(ApiResponse::success(Last { activation })))
}

pub(super) async fn preview(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<Json<ApiResponse<Report>>, ActivationError> {
    execute(state, id, None).await
}

pub(super) async fn apply(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Json(request): Json<Apply>,
) -> Result<Json<ApiResponse<Report>>, ActivationError> {
    if request.preview_digest.len() != 64
        || !request
            .preview_digest
            .bytes()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    {
        return Err(Error::Invalid.into());
    }
    execute(state, id, Some(request.preview_digest)).await
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// A participating rig's registered database, found by catalog identity.
struct RigCatalog {
    identity: CatalogIdentity,
    context: Arc<DatabaseContext>,
}
use crate::server::database_context::DatabaseContext;

async fn execute(
    state: Arc<AppState>,
    id: Uuid,
    expected: Option<String>,
) -> Result<Json<ApiResponse<Report>>, ActivationError> {
    let service = enabled(&state)?;
    let catalogs: Vec<Arc<DatabaseContext>> = state
        .databases
        .read()
        .map_err(|_| Error::Internal)?
        .values()
        .cloned()
        .collect();
    let metadata_permit = service
        .admission
        .clone()
        .try_acquire_owned()
        .map_err(|_| Error::Busy)?;
    let catalog_permit = service
        .discovery_admission
        .clone()
        .try_acquire_owned()
        .map_err(|_| Error::Busy)?;
    let report = tokio::task::spawn_blocking(move || {
        let _permits = (metadata_permit, catalog_permit);
        let applying = expected.is_some();
        let mut store = service.store.lock().map_err(|_| Error::Internal)?;
        let project = store.project(id)?.ok_or(Error::Missing)?;
        let framing = store
            .framing_draft(id)?
            .ok_or(ActivationError::NotReady("Save a framing first."))?;
        let panel = framing.panel.ok_or(ActivationError::NotReady(
            "Choose a panel rig or enter a panel size in Framing first.",
        ))?;
        let plan = store
            .plan_draft(id)?
            .filter(|plan| !plan.objectives.is_empty())
            .ok_or(ActivationError::NotReady(
                "Save a plan with at least one objective first.",
            ))?;
        let panels = FramingRequest {
            center: framing.center,
            position_angle_degrees: framing.position_angle_degrees,
            panel,
            mosaic: framing.mosaic,
            overlays: vec![],
            view: None,
        }
        .preview()
        .map_err(|_| Error::Invalid)?
        .panels;
        let mut by_rig: BTreeMap<Uuid, Vec<&Contribution>> = BTreeMap::new();
        for contribution in plan.contributions.iter().filter(|c| c.enabled) {
            by_rig.entry(contribution.rig_id).or_default().push(contribution);
        }
        if by_rig.is_empty() {
            return Err(ActivationError::NotReady(
                "Tick at least one rig in the plan first.",
            ));
        }
        // Which registered database each participating rig is bound to.
        let mut rig_catalogs: BTreeMap<Uuid, RigCatalog> = BTreeMap::new();
        for context in &catalogs {
            let Some(identity) = read_identity(&context.database_path) else {
                continue;
            };
            if let Some(binding) = store.catalog_rig(identity.id)?
                && by_rig.contains_key(&binding.rig.id)
            {
                rig_catalogs.insert(
                    binding.rig.id,
                    RigCatalog {
                        identity,
                        context: context.clone(),
                    },
                );
            }
        }
        let now = now_ms();
        let mut reports = Vec::new();
        let mut pending: Vec<(Uuid, Connection, ActivatedRig, bool)> = Vec::new();
        for (rig_id, contributions) in &by_rig {
            let rig = store.rig(*rig_id)?.ok_or(Error::Missing)?;
            let Some(catalog) = rig_catalogs.get(rig_id) else {
                reports.push(RigReport {
                    rig,
                    catalog_slug: None,
                    catalog_name: String::new(),
                    profile_id: None,
                    changes: vec![],
                    warnings: vec![
                        "Rig has no registered database on this server; push it through Sync later."
                            .into(),
                    ],
                    applied: false,
                });
                continue;
            };
            let mut warnings = Vec::new();
            let profile = store.rig_profile(*rig_id)?;
            if profile.as_ref().is_none_or(|p| p.configuration.is_none()) {
                warnings.push("The N.I.N.A. plugin has not reported this rig's camera yet; the Target Scheduler plugin can still run these rows.".into());
            }
            // A preview performs the same writes and rolls them back, so the
            // connection is read-write either way; nothing lands without Apply.
            let mut connection = super::super::database_context::open_scheduler_connection_with_flags(
                FilePath::new(&catalog.context.database_path),
                OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
            .map_err(StoreError::from)?;
            connection.busy_timeout(Duration::from_secs(2))?;
            let outcome = {
                let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
                let existing_link = store
                    .catalog_project_mappings(catalog.identity.id, None, 256)?
                    .items
                    .into_iter()
                    .find(|mapping| mapping.project_id == id)
                    .map(|mapping| mapping.source_project_guid);
                let outcome = write_rig_inner(
                    &tx,
                    &Inputs {
                        project: &project,
                        framing: &framing,
                        plan: &plan,
                        panels: &panels,
                        contributions,
                        rig_id: *rig_id,
                        catalog: catalog.identity,
                        existing_link,
                        instance: service.instance_id,
                        now,
                    },
                );
                match outcome {
                    Ok(outcome) => {
                        // Hold the writes uncommitted until the digest is checked.
                        if applying {
                            tx.commit_pending()?;
                        } else {
                            tx.rollback()?;
                        }
                        Some(outcome)
                    }
                    Err(RigError::Skip(reason)) => {
                        tx.rollback()?;
                        warnings.push(reason);
                        None
                    }
                    Err(RigError::Failed(error)) => return Err(error.into()),
                }
            };
            let Some(outcome) = outcome else {
                reports.push(RigReport {
                    rig,
                    catalog_slug: Some(catalog.context.id.clone()),
                    catalog_name: catalog.context.name.clone(),
                    profile_id: None,
                    changes: vec![],
                    warnings,
                    applied: false,
                });
                continue;
            };
            reports.push(RigReport {
                rig,
                catalog_slug: Some(catalog.context.id.clone()),
                catalog_name: catalog.context.name.clone(),
                profile_id: Some(outcome.record.profile_id.clone()),
                changes: outcome.changes,
                warnings,
                applied: false,
            });
            pending.push((*rig_id, connection, outcome.record, outcome.created_project));
        }
        let mut warnings = Vec::new();
        if pending.is_empty() {
            warnings.push("No rig database can take this plan yet.".into());
        }
        let bytes = serde_json::to_vec(&(
            "activation-v1",
            id,
            framing.revision,
            plan.revision,
            reports
                .iter()
                .map(|r| (r.rig.id, &r.catalog_slug, &r.profile_id, &r.changes, &r.warnings))
                .collect::<Vec<_>>(),
            service.instance_id,
            catalogs
                .iter()
                .map(|c| (&c.id, &c.database_path))
                .collect::<Vec<_>>(),
        ))
        .map_err(|_| Error::Internal)?;
        let digest = catalog_discovery::digest(&bytes);
        if expected.as_ref().is_some_and(|value| value != &digest) {
            return Err(Error::Conflict.into());
        }
        let mut activation_revision = None;
        if applying {
            let mut rigs = Vec::new();
            let mut links = Vec::new();
            for (rig_id, connection, record, created_project) in pending {
                connection.execute_batch("COMMIT").map_err(|error| {
                    tracing::error!(%error, "Director activation commit failed");
                    Error::Internal
                })?;
                if let Some(report) = reports.iter_mut().find(|r| r.rig.id == rig_id) {
                    report.applied = true;
                }
                if created_project {
                    links.push(ProjectMapping {
                        catalog_id: record.catalog_id,
                        source_project_guid: record.project_guid,
                        source_profile_id: record.profile_id.clone(),
                        project_id: id,
                        rig_id,
                    });
                }
                rigs.push(record);
            }
            let recorded = store.record_activation(&Activation {
                project_id: id,
                revision: 0,
                framing_revision: framing.revision,
                plan_revision: plan.revision,
                coordinator_instance_id: service.instance_id,
                applied_at_ms: now,
                rigs,
            })?;
            activation_revision = Some(recorded.revision);
            for link in links {
                if let Err(error) = store.link_catalog_project(&link) {
                    tracing::warn!(error = ?error, "Activated project could not be linked in meta");
                    warnings.push(format!(
                        "The new project in {} was written but could not be linked under Project planning links; link it there by hand.",
                        reports
                            .iter()
                            .find(|r| r.rig.id == link.rig_id)
                            .map(|r| r.catalog_name.as_str())
                            .unwrap_or("its database")
                    ));
                }
            }
        }
        Ok::<_, ActivationError>(Report {
            project,
            framing_revision: framing.revision,
            plan_revision: plan.revision,
            panels: panels.len(),
            rigs: reports,
            warnings,
            preview_digest: digest,
            applied: applying,
            activation_revision,
        })
    })
    .await
    .map_err(|error| {
        tracing::error!(%error, "Director activation worker failed");
        Error::Internal
    })??;
    Ok(Json(ApiResponse::success(report)))
}

/// Keep a transaction's writes without committing: end the `Transaction`
/// guard's ownership while SQLite still holds the open transaction. The caller
/// issues the real COMMIT after the preview digest is verified.
trait CommitPending {
    fn commit_pending(self) -> rusqlite::Result<()>;
}
impl CommitPending for rusqlite::Transaction<'_> {
    fn commit_pending(self) -> rusqlite::Result<()> {
        // Dropping a Transaction rolls back; forgetting it leaves the
        // connection inside the open transaction, which is what we want here.
        std::mem::forget(self);
        Ok(())
    }
}

fn read_identity(path: &str) -> Option<CatalogIdentity> {
    let connection = super::super::database_context::open_scheduler_connection_with_flags(
        FilePath::new(path),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    connection.busy_timeout(Duration::from_secs(1)).ok()?;
    crate::catalog_identity::read(&connection).ok().flatten()
}

struct Inputs<'a> {
    project: &'a NamedIdentity,
    framing: &'a FramingDraft,
    plan: &'a PlanDraft,
    panels: &'a [Panel],
    contributions: &'a [&'a Contribution],
    rig_id: Uuid,
    catalog: CatalogIdentity,
    existing_link: Option<Uuid>,
    instance: Uuid,
    now: u64,
}

struct Outcome {
    changes: Vec<Change>,
    record: ActivatedRig,
    created_project: bool,
}

/// A rig that cannot take the plan carries the reason the operator sees;
/// anything else is a real failure.
enum RigError {
    Skip(String),
    Failed(Error),
}
impl From<rusqlite::Error> for RigError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Failed(StoreError::Sqlite(error).into())
    }
}
impl From<Error> for RigError {
    fn from(error: Error) -> Self {
        Self::Failed(error)
    }
}

fn has_column(conn: &Connection, table: &str, column: &str) -> rusqlite::Result<bool> {
    let mut statement = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let names = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(names.iter().any(|name| name.eq_ignore_ascii_case(column)))
}

fn write_rig_inner(tx: &Connection, inputs: &Inputs<'_>) -> Result<Outcome, RigError> {
    for (table, column) in [
        ("project", "guid"),
        ("target", "guid"),
        ("exposureplan", "guid"),
        ("exposureplan", "enabled"),
        ("exposuretemplate", "guid"),
    ] {
        if !has_column(tx, table, column)? {
            return Err(RigError::Skip(
                "Database needs Target Scheduler schema 22 or newer before Director can write plans."
                    .into(),
            ));
        }
    }
    let profile_id = resolve_profile(tx)?.ok_or_else(|| {
        RigError::Skip(
            "Database has no N.I.N.A. profile yet; open it in Target Scheduler once, or import a frame."
                .into(),
        )
    })?;
    tx.execute_batch(
        "CREATE TABLE IF NOT EXISTS main.psf_guard_director_project(
            project_guid TEXT PRIMARY KEY NOT NULL,
            global_project_id TEXT NOT NULL,
            coordinator_instance_id TEXT NOT NULL,
            activation_revision INTEGER NOT NULL,
            applied_at_ms INTEGER NOT NULL) STRICT;
         CREATE TABLE IF NOT EXISTS main.psf_guard_director_target(
            target_guid TEXT PRIMARY KEY NOT NULL,
            project_guid TEXT NOT NULL,
            panel_id TEXT NOT NULL,
            framing_revision INTEGER NOT NULL,
            UNIQUE(project_guid, panel_id)) STRICT;
         CREATE TABLE IF NOT EXISTS main.psf_guard_director_plan(
            exposureplan_guid TEXT PRIMARY KEY NOT NULL,
            target_guid TEXT NOT NULL,
            contribution_id TEXT NOT NULL,
            objective_id TEXT NOT NULL,
            bandpass_id TEXT NOT NULL,
            purpose TEXT NOT NULL,
            required_frames INTEGER NOT NULL,
            plan_revision INTEGER NOT NULL,
            UNIQUE(target_guid, contribution_id)) STRICT;",
    )?;
    let mut changes = Vec::new();
    let project_name = inputs.project.name.clone();
    let target_base = if inputs.framing.target_name.is_empty() {
        inputs.project.name.clone()
    } else {
        inputs.framing.target_name.clone()
    };
    let mosaic = inputs.panels.len() > 1;

    // The Target Scheduler project row: ours from a previous activation, the
    // source project already linked to this global project, or a new one.
    let owned: Option<String> = tx
        .query_row(
            "SELECT project_guid FROM psf_guard_director_project WHERE global_project_id=?1",
            [inputs.project.id.to_string()],
            |row| row.get(0),
        )
        .optional()?;
    let mut created_project = false;
    let project_guid = match owned {
        Some(guid) => {
            if project_row(tx, &guid)?.is_some() {
                tx.execute(
                    "UPDATE project SET isMosaic=?2, state=CASE WHEN state=0 THEN 1 ELSE state END WHERE guid=?1",
                    params![guid, i32::from(mosaic)],
                )?;
                changes.push(Change {
                    kind: "project",
                    action: "unchanged",
                    name: project_name.clone(),
                    detail: "Director's project from the last activation".into(),
                });
                guid
            } else {
                let guid = insert_project(tx, &profile_id, &project_name, mosaic, Some(&guid))?;
                changes.push(Change {
                    kind: "project",
                    action: "create",
                    name: project_name.clone(),
                    detail: "the project row from the last activation was gone; created again"
                        .into(),
                });
                guid
            }
        }
        None => match inputs
            .existing_link
            .map(|guid| guid.to_string())
            .filter(|guid| project_row(tx, guid).ok().flatten().is_some())
        {
            Some(guid) => {
                tx.execute(
                    "UPDATE project SET isMosaic=CASE WHEN ?2=1 THEN 1 ELSE isMosaic END WHERE guid=?1",
                    params![guid, i32::from(mosaic)],
                )?;
                changes.push(Change {
                    kind: "project",
                    action: "update",
                    name: project_row(tx, &guid)?.map(|(_, name)| name).unwrap_or_default(),
                    detail: "the existing project already linked to this plan; Director now owns its planning rows".into(),
                });
                guid
            }
            None => {
                created_project = true;
                let guid = insert_project(tx, &profile_id, &project_name, mosaic, None)?;
                changes.push(Change {
                    kind: "project",
                    action: "create",
                    name: project_name.clone(),
                    detail: "new Target Scheduler project, Active".into(),
                });
                guid
            }
        },
    };
    tx.execute(
        "INSERT INTO psf_guard_director_project(project_guid,global_project_id,coordinator_instance_id,activation_revision,applied_at_ms)
         VALUES(?1,?2,?3,?4,?5)
         ON CONFLICT(project_guid) DO UPDATE SET activation_revision=excluded.activation_revision, applied_at_ms=excluded.applied_at_ms",
        params![
            project_guid,
            inputs.project.id.to_string(),
            inputs.instance.to_string(),
            inputs.plan.revision as i64,
            inputs.now as i64
        ],
    )?;
    let (project_row_id, _) = project_row(tx, &project_guid)?.ok_or(Error::Internal)?;

    // Targets: one per panel, matched by panel id.
    let mut targets = Vec::new();
    for panel in inputs.panels {
        let name = if mosaic {
            format!("{target_base} {}", panel.footprint.id)
        } else {
            target_base.clone()
        };
        let ra_hours = panel.footprint.center.ra_degrees / 15.0;
        let dec = panel.footprint.center.dec_degrees;
        let rotation = inputs.framing.position_angle_degrees;
        let detail = format!(
            "{} {}, angle {rotation:.1}°",
            format_ra(ra_hours),
            format_dec(dec)
        );
        let owned: Option<String> = tx
            .query_row(
                "SELECT target_guid FROM psf_guard_director_target WHERE project_guid=?1 AND panel_id=?2",
                params![project_guid, panel.footprint.id],
                |row| row.get(0),
            )
            .optional()?;
        let target_guid = match owned {
            Some(guid) => {
                let existing: Option<(f64, f64, f64, String)> = tx
                    .query_row(
                        "SELECT ra, dec, rotation, name FROM target WHERE guid=?1",
                        [&guid],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                    )
                    .optional()?;
                match existing {
                    Some((ra0, dec0, rot0, name0)) => {
                        let same = (ra0 - ra_hours).abs() < 1e-7
                            && (dec0 - dec).abs() < 1e-6
                            && (rot0 - rotation).abs() < 1e-3;
                        if !same {
                            tx.execute(
                                "UPDATE target SET ra=?2, dec=?3, rotation=?4, projectid=?5 WHERE guid=?1",
                                params![guid, ra_hours, dec, rotation, project_row_id],
                            )?;
                        }
                        changes.push(Change {
                            kind: "target",
                            action: if same { "unchanged" } else { "update" },
                            name: name0,
                            detail,
                        });
                    }
                    None => {
                        insert_target(tx, &guid, &name, ra_hours, dec, rotation, project_row_id)?;
                        changes.push(Change {
                            kind: "target",
                            action: "create",
                            name: name.clone(),
                            detail,
                        });
                    }
                }
                guid
            }
            None => {
                let guid = new_guid();
                insert_target(tx, &guid, &name, ra_hours, dec, rotation, project_row_id)?;
                changes.push(Change {
                    kind: "target",
                    action: "create",
                    name: name.clone(),
                    detail,
                });
                guid
            }
        };
        tx.execute(
            "INSERT INTO psf_guard_director_target(target_guid,project_guid,panel_id,framing_revision) VALUES(?1,?2,?3,?4)
             ON CONFLICT(target_guid) DO UPDATE SET framing_revision=excluded.framing_revision",
            params![
                target_guid,
                project_guid,
                panel.footprint.id,
                inputs.framing.revision as i64
            ],
        )?;
        targets.push((panel.footprint.id.clone(), target_guid, name));
    }

    // Exposure plans: one per contribution and target.
    let mut plans = Vec::new();
    for contribution in inputs.contributions {
        let objective = inputs
            .plan
            .objectives
            .iter()
            .find(|o| o.id == contribution.objective_id)
            .ok_or(Error::Invalid)?;
        let Some(frames) = required_frames(objective, contribution) else {
            return Err(RigError::Skip(format!(
                "Objective {} has no reachable frame count at {} s.",
                objective.bandpass_id, contribution.exposure_seconds
            )));
        };
        let template_id = resolve_template(tx, &profile_id, contribution)?;
        for (_, target_guid, target_name) in &targets {
            let target_row: i64 = tx.query_row(
                "SELECT Id FROM target WHERE guid=?1",
                [target_guid],
                |row| row.get(0),
            )?;
            let name = format!("{target_name} · {}", contribution.template.name);
            let detail = format!("{frames} frames of {} s", contribution.exposure_seconds);
            let owned: Option<String> = tx
                .query_row(
                    "SELECT exposureplan_guid FROM psf_guard_director_plan WHERE target_guid=?1 AND contribution_id=?2",
                    params![target_guid, contribution.id.to_string()],
                    |row| row.get(0),
                )
                .optional()?;
            let plan_guid = match owned {
                Some(guid) => {
                    let existing: Option<(f64, i64, i64, i64)> = tx
                        .query_row(
                            "SELECT exposure, desired, exposureTemplateId, COALESCE(enabled,1) FROM exposureplan WHERE guid=?1",
                            [&guid],
                            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                        )
                        .optional()?;
                    match existing {
                        Some((exposure0, desired0, template0, enabled0)) => {
                            let same = (exposure0 - contribution.exposure_seconds).abs() < 1e-6
                                && desired0 == i64::from(frames)
                                && template0 == template_id
                                && enabled0 == 1;
                            if !same {
                                tx.execute(
                                    "UPDATE exposureplan SET exposure=?2, desired=?3, exposureTemplateId=?4, enabled=1, targetid=?5 WHERE guid=?1",
                                    params![guid, contribution.exposure_seconds, i64::from(frames), template_id, target_row],
                                )?;
                            }
                            changes.push(Change {
                                kind: "plan",
                                action: if same { "unchanged" } else { "update" },
                                name: name.clone(),
                                detail,
                            });
                        }
                        None => {
                            insert_plan(
                                tx,
                                &guid,
                                &profile_id,
                                contribution.exposure_seconds,
                                frames,
                                target_row,
                                template_id,
                            )?;
                            changes.push(Change {
                                kind: "plan",
                                action: "create",
                                name: name.clone(),
                                detail,
                            });
                        }
                    }
                    guid
                }
                None => {
                    let guid = new_guid();
                    insert_plan(
                        tx,
                        &guid,
                        &profile_id,
                        contribution.exposure_seconds,
                        frames,
                        target_row,
                        template_id,
                    )?;
                    changes.push(Change {
                        kind: "plan",
                        action: "create",
                        name: name.clone(),
                        detail,
                    });
                    guid
                }
            };
            tx.execute(
                "INSERT INTO psf_guard_director_plan(exposureplan_guid,target_guid,contribution_id,objective_id,bandpass_id,purpose,required_frames,plan_revision)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8)
                 ON CONFLICT(exposureplan_guid) DO UPDATE SET required_frames=excluded.required_frames, plan_revision=excluded.plan_revision, bandpass_id=excluded.bandpass_id, purpose=excluded.purpose",
                params![
                    plan_guid,
                    target_guid,
                    contribution.id.to_string(),
                    objective.id.to_string(),
                    objective.bandpass_id,
                    objective.purpose,
                    i64::from(frames),
                    inputs.plan.revision as i64
                ],
            )?;
            plans.push(ActivatedPlan {
                contribution_id: contribution.id,
                objective_id: objective.id,
                target_guid: Uuid::parse_str(target_guid).map_err(|_| Error::Internal)?,
                exposureplan_guid: Uuid::parse_str(&plan_guid).map_err(|_| Error::Internal)?,
                required_frames: frames,
            });
        }
    }
    Ok(Outcome {
        changes,
        record: ActivatedRig {
            rig_id: inputs.rig_id,
            catalog_id: inputs.catalog.id,
            project_guid: Uuid::parse_str(&project_guid).map_err(|_| Error::Internal)?,
            profile_id,
            targets: targets
                .into_iter()
                .map(|(panel_id, guid, _)| {
                    Ok(ActivatedTarget {
                        panel_id,
                        target_guid: Uuid::parse_str(&guid).map_err(|_| Error::Internal)?,
                    })
                })
                .collect::<Result<Vec<_>, Error>>()?,
            plans,
        },
        created_project,
    })
}

fn required_frames(objective: &Objective, contribution: &Contribution) -> Option<u32> {
    match objective.goal {
        Goal::Hours { value } => frames_for_hours(value, contribution.exposure_seconds),
        Goal::Frames { value } => Some(value),
    }
}

/// The profile that owns the database's projects, else its only preference
/// row, else the templates' profile. Target Scheduler shows a project only
/// under its own profile, so a fresh GUID would hide everything we write.
fn resolve_profile(tx: &Connection) -> rusqlite::Result<Option<String>> {
    let dominant: Option<String> = tx
        .query_row(
            "SELECT profileId FROM project WHERE profileId IS NOT NULL AND profileId<>'' GROUP BY profileId ORDER BY COUNT(*) DESC, profileId LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if dominant.is_some() {
        return Ok(dominant);
    }
    if has_column(tx, "profilepreference", "profileId")? {
        let mut statement = tx.prepare("SELECT DISTINCT profileId FROM profilepreference")?;
        let profiles: Vec<String> = statement
            .query_map([], |row| row.get(0))?
            .collect::<Result<_, _>>()?;
        if profiles.len() == 1 {
            return Ok(profiles.into_iter().next());
        }
    }
    tx.query_row(
        "SELECT profileId FROM exposuretemplate WHERE profileId IS NOT NULL AND profileId<>'' GROUP BY profileId ORDER BY COUNT(*) DESC, profileId LIMIT 1",
        [],
        |row| row.get(0),
    )
    .optional()
}

fn project_row(tx: &Connection, guid: &str) -> rusqlite::Result<Option<(i64, String)>> {
    tx.query_row(
        "SELECT Id, name FROM project WHERE guid=?1",
        [guid],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
    .optional()
}

fn insert_project(
    tx: &Connection,
    profile_id: &str,
    name: &str,
    mosaic: bool,
    guid: Option<&str>,
) -> rusqlite::Result<String> {
    let guid = guid.map(str::to_owned).unwrap_or_else(new_guid);
    tx.execute(
        "INSERT INTO project (
            profileId, name, description, state, priority, createdate,
            activedate, inactivedate, minimumtime, minimumaltitude,
            maximumAltitude, usecustomhorizon, horizonoffset, meridianwindow,
            filterswitchfrequency, ditherevery, enablegrader, isMosaic,
            flatsHandling, smartexposureorder, guid
        ) VALUES (?1, ?2, ?3, 1, 1, ?4, ?4, NULL, 30, 0, 0, 0, 0, 0, 0, 0, 1, ?5, 0, 0, ?6)",
        params![
            profile_id,
            name,
            "Planned by PSF Guard Director",
            chrono::Utc::now().timestamp(),
            i32::from(mosaic),
            guid,
        ],
    )?;
    let project_id = tx.last_insert_rowid();
    for (rule, weight) in crate::commands::import::DEFAULT_RULE_WEIGHTS {
        tx.execute(
            "INSERT INTO ruleweight (name, weight, projectid) VALUES (?1, ?2, ?3)",
            params![rule, weight, project_id],
        )?;
    }
    Ok(guid)
}

fn insert_target(
    tx: &Connection,
    guid: &str,
    name: &str,
    ra_hours: f64,
    dec: f64,
    rotation: f64,
    project_id: i64,
) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO target (name, active, ra, dec, epochcode, rotation, roi, projectid, guid)
         VALUES (?1, 1, ?2, ?3, 2, ?4, 100, ?5, ?6)",
        params![name, ra_hours, dec, rotation, project_id, guid],
    )?;
    Ok(())
}

fn insert_plan(
    tx: &Connection,
    guid: &str,
    profile_id: &str,
    exposure: f64,
    frames: u32,
    target_id: i64,
    template_id: i64,
) -> rusqlite::Result<()> {
    tx.execute(
        "INSERT INTO exposureplan (profileId, exposure, desired, acquired, accepted, targetid, exposureTemplateId, enabled, guid)
         VALUES (?1, ?2, ?3, 0, 0, ?4, ?5, 1, ?6)",
        params![profile_id, exposure, i64::from(frames), target_id, template_id, guid],
    )?;
    Ok(())
}

/// The chosen template by id or GUID when it still exists with the same
/// filter, else the profile's template with the same settings, else a new one.
fn resolve_template(
    tx: &Connection,
    profile_id: &str,
    contribution: &Contribution,
) -> Result<i64, RigError> {
    let choice = &contribution.template;
    if let Some(id) = choice.template_id {
        let found: Option<String> = tx
            .query_row(
                "SELECT filtername FROM exposuretemplate WHERE Id=?1",
                [id],
                |row| row.get(0),
            )
            .optional()?;
        if found.is_some_and(|filter| filter.eq_ignore_ascii_case(&choice.filter_name)) {
            return Ok(id);
        }
    }
    if let Some(guid) = choice.template_guid {
        let found: Option<(i64, String)> = tx
            .query_row(
                "SELECT Id, filtername FROM exposuretemplate WHERE guid=?1",
                [guid.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if let Some((id, filter)) = found
            && filter.eq_ignore_ascii_case(&choice.filter_name)
        {
            return Ok(id);
        }
    }
    let gain = choice.gain.unwrap_or(-1);
    let offset = choice.offset.unwrap_or(-1);
    let bin = choice.bin.unwrap_or(1);
    let readout = choice.readout_mode.unwrap_or(-1);
    let existing: Option<i64> = tx
        .query_row(
            "SELECT Id FROM exposuretemplate
             WHERE profileId = ?1 AND filtername = ?2
               AND IFNULL(gain, -1) = ?3 AND IFNULL(offset, -1) = ?4
               AND IFNULL(bin, 1) = ?5 AND IFNULL(readoutmode, -1) = ?6
             ORDER BY Id LIMIT 1",
            params![profile_id, choice.filter_name, gain, offset, bin, readout],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(id) = existing {
        return Ok(id);
    }
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
            if choice.name.is_empty() {
                choice.filter_name.clone()
            } else {
                choice.name.clone()
            },
            choice.filter_name,
            gain,
            offset,
            bin,
            readout,
            contribution.exposure_seconds,
            new_guid(),
        ],
    )?;
    Ok(tx.last_insert_rowid())
}

fn format_ra(hours: f64) -> String {
    let h = hours.floor();
    let m = ((hours - h) * 60.0).floor();
    format!("{h:02.0}h {m:02.0}m")
}

fn format_dec(dec: f64) -> String {
    let sign = if dec < 0.0 { '-' } else { '+' };
    let abs = dec.abs();
    let d = abs.floor();
    let m = ((abs - d) * 60.0).floor();
    format!("{sign}{d:02.0}° {m:02.0}′")
}
