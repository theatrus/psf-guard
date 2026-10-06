//! Push a project's framing and plan into each participating rig database:
//! one Target Scheduler project, a target per panel, an exposure plan per rig
//! objective. Preview performs the same writes and rolls them back, so what
//! the operator reviews is what Apply commits. Captures and grades are never
//! touched; removed panels leave their targets behind.

use super::*;
use crate::{
    commands::sync::{sync_remote, RemoteDirection, RemoteSyncOptions},
    db_registry::PeerEntry,
    server::peers::registered_peers,
    ts_schema::new_guid,
};
use psf_guard_director_core::{
    bandpass::frames_for_hours,
    framing::{FramingRequest, Panel},
};
use psf_guard_director_meta::{
    activation::{ActivatedPlan, ActivatedRig, ActivatedTarget, Activation, InactiveRig},
    catalog::ProjectMapping,
    framing::FramingDraft,
    plan::{Contribution, Goal, Objective, PlanDraft},
    preferences::{ResolvedScheduling, SchedulingValues},
    CatalogIdentity,
};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, TransactionBehavior};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Serialize)]
struct Change {
    kind: &'static str,
    action: &'static str,
    name: String,
    detail: String,
}

/// A rig whose real database lives on another PSF Guard: the local copy is
/// written like any other, then its planning rows travel by Sync.
#[derive(Clone, Serialize)]
struct Push {
    peer_id: String,
    peer_name: String,
    /// True once the peer applied the planning rows.
    applied: bool,
    summary: BTreeMap<String, i64>,
    error: Option<String>,
}

impl Push {
    fn planned(peer: &PeerEntry) -> Self {
        Self {
            peer_id: peer.id.clone(),
            peer_name: peer.name.clone(),
            applied: false,
            summary: BTreeMap::new(),
            error: None,
        }
    }
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
    /// Set when the rig's profile names a Sync peer; filled in after Apply.
    push: Option<Push>,
}

#[derive(Serialize)]
struct PushedRig {
    rig: NamedIdentity,
    catalog_name: String,
    push: Push,
}

#[derive(Serialize)]
pub(super) struct PushReport {
    project: NamedIdentity,
    activation_revision: u64,
    rigs: Vec<PushedRig>,
    warnings: Vec<String>,
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
        .query(move |store| {
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

/// PSF Guard's own tables in a rig database, in plain SQL: the file is
/// shared with N.I.N.A. and Target Scheduler, so nothing here may need a
/// newer SQLite than theirs. Type CHECKs stand in for `STRICT`.
const DIRECTOR_TABLES: [(&str, &str); 3] = [
    (
        "psf_guard_director_project",
        "CREATE TABLE IF NOT EXISTS main.psf_guard_director_project(
            project_guid TEXT PRIMARY KEY NOT NULL,
            global_project_id TEXT NOT NULL,
            coordinator_instance_id TEXT NOT NULL,
            activation_revision INTEGER NOT NULL CHECK(typeof(activation_revision)='integer'),
            applied_at_ms INTEGER NOT NULL CHECK(typeof(applied_at_ms)='integer'));",
    ),
    (
        "psf_guard_director_target",
        "CREATE TABLE IF NOT EXISTS main.psf_guard_director_target(
            target_guid TEXT PRIMARY KEY NOT NULL,
            project_guid TEXT NOT NULL,
            panel_id TEXT NOT NULL,
            framing_revision INTEGER NOT NULL CHECK(typeof(framing_revision)='integer'),
            UNIQUE(project_guid, panel_id));",
    ),
    (
        "psf_guard_director_plan",
        "CREATE TABLE IF NOT EXISTS main.psf_guard_director_plan(
            exposureplan_guid TEXT PRIMARY KEY NOT NULL,
            target_guid TEXT NOT NULL,
            contribution_id TEXT NOT NULL,
            objective_id TEXT NOT NULL,
            bandpass_id TEXT NOT NULL,
            purpose TEXT NOT NULL,
            required_frames INTEGER NOT NULL CHECK(typeof(required_frames)='integer'),
            plan_revision INTEGER NOT NULL CHECK(typeof(plan_revision)='integer'),
            UNIQUE(target_guid, contribution_id));",
    ),
];

/// Create the side tables, first rebuilding any an earlier build made
/// `STRICT`, rows intact.
fn ensure_director_tables(tx: &Connection) -> Result<(), RigError> {
    for (name, ddl) in DIRECTOR_TABLES {
        crate::catalog_identity::relax_strict_table(tx, name, ddl)
            .map_err(|error| RigError::Failed(Error::from(error)))?;
        tx.execute_batch(ddl)?;
    }
    Ok(())
}

/// The rig side tables, for tests outside Director that read them.
#[cfg(test)]
pub(crate) fn create_director_tables(conn: &Connection) {
    assert!(ensure_director_tables(conn).is_ok());
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
    // A preview only reads; applying writes every participating rig database.
    let service = if expected.is_some() {
        writable(&state)?
    } else {
        enabled(&state)?
    };
    let catalogs: Vec<Arc<DatabaseContext>> = state
        .databases
        .read()
        .map_err(|_| Error::Internal)?
        .values()
        .cloned()
        .collect();
    let catalog_permit = admit(&service.discovery_admission).await?;
    let peers = registered_peers(&state);
    let known_peers = peers.clone();
    let mut report = service.clone().with_writer(move |store| {
        let _catalog_permit = catalog_permit;
        let applying = expected.is_some();
        let project = store.project(id)?.ok_or(Error::Missing)?;
        if store.collaboration_requires_admission(id)? {
            return Err(ActivationError::NotReady(
                "This collaboration import is an inactive draft. Remote constraints, exact panels and a local nightly budget must be admitted before acquisition.",
            ));
        }
        let framing = store
            .framing_draft(id)?
            .ok_or(ActivationError::NotReady("Save a framing first."))?;
        if framing.panel.is_none() && framing.rig_framings.is_empty() {
            return Err(ActivationError::NotReady(
                "Choose a panel rig or enter a panel size in Framing first.",
            ));
        }
        let plan = store
            .plan_draft(id)?
            .filter(|plan| !plan.objectives.is_empty())
            .ok_or(ActivationError::NotReady(
                "Save a plan with at least one objective first.",
            ))?;
        // The shared framing's panels; a rig framed on its own has its own.
        let panels = match framing.panel {
            Some(panel) => FramingRequest {
                center: framing.center,
                position_angle_degrees: framing.position_angle_degrees,
                panel,
                mosaic: framing.mosaic,
                overlays: vec![],
                view: None,
            }
            .preview()
            .map_err(|_| Error::Invalid)?
            .panels,
            None => vec![],
        };
        let mut by_rig: BTreeMap<Uuid, Vec<&Contribution>> = BTreeMap::new();
        for contribution in plan.contributions.iter().filter(|c| c.enabled) {
            by_rig.entry(contribution.rig_id).or_default().push(contribution);
        }
        // The last activation: which rigs had rows, and which projects it
        // set Inactive when their rig was turned off.
        let previous = store.activation(id)?;
        // Contributions turned off on a rig that stays on: their rows are
        // turned off. A rig with all of them off has its project set
        // Inactive instead.
        let mut off_by_rig: BTreeMap<Uuid, Vec<&Contribution>> = BTreeMap::new();
        for contribution in plan.contributions.iter().filter(|c| !c.enabled) {
            off_by_rig.entry(contribution.rig_id).or_default().push(contribution);
        }
        // A rig is off when the plan holds it only with contributions turned
        // off, or no longer holds it at all while the last activation gave it
        // rows (the switch may remove its contributions or turn them off).
        let off_rigs: std::collections::BTreeSet<Uuid> = off_by_rig
            .keys()
            .copied()
            .chain(previous.iter().flat_map(|p| {
                p.rigs.iter().map(|r| r.rig_id).chain(p.inactive_rigs.iter().map(|r| r.rig_id))
            }))
            .filter(|rig| !by_rig.contains_key(rig))
            .collect();
        // A rig this activation cannot reach keeps its last record, so its
        // rows stay accounted for and a later activation can still turn it
        // off.
        let previous_rig = |rig: Uuid| previous.as_ref().and_then(|p| p.rigs.iter().find(|r| r.rig_id == rig).cloned());
        let mut carried: Vec<ActivatedRig> = Vec::new();
        if by_rig.is_empty() {
            return Err(ActivationError::NotReady(
                "Tick at least one rig in the plan first.",
            ));
        }
        // Every objective should have some rig on every shared panel; say
        // where not. A rig framed on its own covers its own grid.
        let mut coverage_warnings = Vec::new();
        let on_shared = |c: &&&Contribution| framing.rig_framing(c.rig_id).is_none();
        for objective in &plan.objectives {
            let uncovered: Vec<&str> = panels
                .iter()
                .map(|p| p.footprint.id.as_str())
                .filter(|panel| {
                    !by_rig.values().flatten().filter(on_shared).any(|c| {
                        c.objective_id == objective.id
                            && (c.panel_ids.is_empty() || c.panel_ids.iter().any(|id| id == panel))
                    })
                })
                .collect();
            let anywhere = by_rig
                .values()
                .flatten()
                .any(|c| c.objective_id == objective.id);
            if !anywhere {
                coverage_warnings.push(format!("No rig shoots {} on any panel.", objective.bandpass_id));
            } else if !uncovered.is_empty() && uncovered.len() < panels.len() {
                coverage_warnings.push(format!(
                    "No rig shoots {} on panel{} {}.",
                    objective.bandpass_id,
                    if uncovered.len() == 1 { "" } else { "s" },
                    uncovered.join(", ")
                ));
            } else if !panels.is_empty() && uncovered.len() == panels.len() && !by_rig.values().flatten().filter(on_shared).any(|c| c.objective_id == objective.id) && by_rig.keys().any(|rig| framing.rig_framing(*rig).is_none()) {
                coverage_warnings.push(format!("No rig on the shared framing shoots {}.", objective.bandpass_id));
            }
        }
        // Which registered database each participating rig is bound to. A
        // hand-copied file with the same identity is named and never written.
        let found = identified_catalogs(&catalogs, service.instance_id);
        coverage_warnings.extend(found.duplicates.iter().cloned());
        let mut rig_catalogs: BTreeMap<Uuid, RigCatalog> = BTreeMap::new();
        for (identity, context) in found.iter() {
            if let Some(binding) = store.catalog_rig(identity.id)?
                && (by_rig.contains_key(&binding.rig.id) || off_rigs.contains(&binding.rig.id))
            {
                rig_catalogs.insert(
                    binding.rig.id,
                    RigCatalog {
                        identity: *identity,
                        context: context.clone(),
                    },
                );
            }
        }
        let now = now_ms();
        let mut reports = Vec::new();
        let mut pending: Vec<(Uuid, Connection, Option<ActivatedRig>, bool)> = Vec::new();
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
                        "Rig has no registered database on this server; pull its database from its peer, then name the peer under Rigs, Setup."
                            .into(),
                    ],
                    applied: false,
                    push: None,
                });
                carried.extend(previous_rig(*rig_id));
                continue;
            };
            let mut warnings = Vec::new();
            let profile = store.rig_profile(*rig_id)?;
            // Global, then this rig's site, the rig, and this project.
            let scheduling = store.effective_observing_preferences(*rig_id, Some(id))?.scheduling;
            // This rig's own layout, or the shared one; its field stands in
            // for a panel size its own framing leaves unset.
            let field = profile
                .as_ref()
                .and_then(|p| p.optics.as_ref())
                .and_then(|optics| optics.value.field_of_view().ok())
                .map(|fov| psf_guard_director_core::framing::PanelSize {
                    width_degrees: fov.width_degrees,
                    height_degrees: fov.height_degrees,
                });
            let Some(layout) = framing.layout_for(*rig_id, field) else {
                reports.push(RigReport {
                    rig,
                    catalog_slug: Some(catalog.context.id.clone()),
                    catalog_name: catalog.context.name.clone(),
                    profile_id: None,
                    changes: vec![],
                    warnings: vec![
                        "No panel size for this rig: give its framing a size in Framing, or set its optics under Rigs, Setup.".into(),
                    ],
                    applied: false,
                    push: None,
                });
                carried.extend(previous_rig(*rig_id));
                continue;
            };
            let rig_panels = if layout.own {
                FramingRequest {
                    center: layout.center,
                    position_angle_degrees: layout.position_angle_degrees,
                    panel: layout.panel,
                    mosaic: layout.mosaic,
                    overlays: vec![],
                    view: None,
                }
                .preview()
                .map_err(|_| Error::Invalid)?
                .panels
            } else {
                panels.clone()
            };
            if profile.as_ref().is_none_or(|p| p.configuration.is_none()) {
                warnings.push("The N.I.N.A. plugin has not reported this rig's camera yet; the Target Scheduler plugin can still run these rows.".into());
            }
            // A rig on another PSF Guard: its rows land here first, then go
            // to the peer by Sync once Apply has committed them.
            let push = planned_push(profile.as_ref(), &known_peers, &mut warnings);
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
                let existing_link = {
                    // Every page: a rig with more linked projects than one
                    // page holds must still find this one, or it would make
                    // a second project.
                    let mut after = None;
                    loop {
                        let page = store.catalog_project_mappings(catalog.identity.id, after, 256)?;
                        if let Some(mapping) = page.items.iter().find(|m| m.project_id == id) {
                            break Some(mapping.source_project_guid);
                        }
                        match page.next_after {
                            Some(next) => after = Some(next),
                            None => break None,
                        }
                    }
                };
                let outcome = write_rig_inner(
                    &tx,
                    &Inputs {
                        project: &project,
                        framing: &framing,
                        plan: &plan,
                        panels: &rig_panels,
                        position_angle_degrees: layout.position_angle_degrees,
                        contributions,
                        reactivate: previous.as_ref().is_some_and(|p| p.inactive_rigs.iter().any(|r| r.rig_id == *rig_id)),
                        rig_id: *rig_id,
                        catalog: catalog.identity,
                        existing_link,
                        instance: service.instance_id,
                        now,
                        scheduling: &scheduling,
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
                    push: None,
                });
                carried.extend(previous_rig(*rig_id));
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
                push,
            });
            pending.push((*rig_id, connection, Some(outcome.record), outcome.created_project));
        }
        // A rig with every contribution off: its Target Scheduler project
        // goes Inactive, so the scheduler stops taking it; its rows, frames
        // and grades stay. A rig never given a project is not listed.
        let mut inactive: Vec<InactiveRig> = Vec::new();
        for rig_id in &off_rigs {
            let was = previous.as_ref().and_then(|p| {
                p.rigs
                    .iter()
                    .find(|r| r.rig_id == *rig_id)
                    .map(|r| InactiveRig { rig_id: r.rig_id, catalog_id: r.catalog_id, project_guid: r.project_guid })
                    .or_else(|| p.inactive_rigs.iter().find(|r| r.rig_id == *rig_id).cloned())
            });
            let Some(was) = was else {
                continue;
            };
            let already = previous.as_ref().is_some_and(|p| p.inactive_rigs.iter().any(|r| r.rig_id == *rig_id));
            let (Some(catalog), Some(rig)) = (rig_catalogs.get(rig_id), store.rig(*rig_id)?) else {
                // Its database is gone from this server; keep what we know,
                // so a later activation can still finish turning it off.
                if already {
                    inactive.push(was);
                } else {
                    carried.extend(previous_rig(*rig_id));
                }
                continue;
            };
            let profile = store.rig_profile(*rig_id)?;
            let mut warnings = Vec::new();
            let push = planned_push(profile.as_ref(), &known_peers, &mut warnings);
            let mut connection = super::super::database_context::open_scheduler_connection_with_flags(
                FilePath::new(&catalog.context.database_path),
                OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
            .map_err(StoreError::from)?;
            connection.busy_timeout(Duration::from_secs(2))?;
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let project_guid = was.project_guid.to_string();
            let mut changes = Vec::new();
            if let Some((row_id, name)) = project_row(&tx, &project_guid)? {
                    let state: i64 = tx.query_row("SELECT IFNULL(state, 0) FROM project WHERE Id=?1", [row_id], |row| row.get(0))?;
                    // Draft or Active goes Inactive. One already Inactive
                    // or Closed by hand is the operator's and stays theirs.
                    if state == 0 || state == 1 {
                        tx.execute("UPDATE project SET state=2 WHERE Id=?1", [row_id])?;
                        changes.push(Change {
                            kind: "project",
                            action: "disable",
                            name,
                            detail: "Inactive in Target Scheduler while this rig is off in the plan".into(),
                        });
                        inactive.push(was);
                    } else if already && state == 2 {
                        inactive.push(was);
                    }
            }
            if changes.is_empty() {
                tx.rollback()?;
                continue;
            }
            if applying {
                tx.commit_pending()?;
            } else {
                tx.rollback()?;
            }
            reports.push(RigReport {
                rig,
                catalog_slug: Some(catalog.context.id.clone()),
                catalog_name: catalog.context.name.clone(),
                profile_id: None,
                changes,
                warnings,
                applied: false,
                push,
            });
            pending.push((*rig_id, connection, None, false));
        }
        let mut warnings = coverage_warnings;
        if pending.iter().all(|(_, _, record, _)| record.is_none()) {
            warnings.push("No rig database can take this plan yet.".into());
        }
        let bytes = serde_json::to_vec(&(
            "activation-v1",
            id,
            framing.revision,
            plan.revision,
            reports
                .iter()
                .map(|r| {
                    (
                        r.rig.id,
                        &r.catalog_slug,
                        &r.profile_id,
                        &r.changes,
                        &r.warnings,
                        r.push.as_ref().map(|p| &p.peer_id),
                    )
                })
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
                let Some(record) = record else {
                    continue;
                };
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
            for record in carried {
                if !rigs.iter().any(|r: &ActivatedRig| r.rig_id == record.rig_id) {
                    rigs.push(record);
                }
            }
            let recorded = store.record_activation(&Activation {
                project_id: id,
                revision: 0,
                framing_revision: framing.revision,
                plan_revision: plan.revision,
                coordinator_instance_id: service.instance_id,
                applied_at_ms: now,
                rigs,
                inactive_rigs: inactive,
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
    .await?;
    if report.applied {
        // The local rows are committed; now each remote rig's peer. A push
        // that fails leaves the activation standing and says so in its row.
        for rig in report.rigs.iter_mut().filter(|rig| rig.applied) {
            let (Some(push), Some(slug)) = (rig.push.as_mut(), rig.catalog_slug.as_deref()) else {
                continue;
            };
            let (Some(peer), Some(context)) = (
                peers.iter().find(|peer| peer.id == push.peer_id),
                state.get_database(slug),
            ) else {
                continue;
            };
            *push = push_planning(&state, &context, peer).await;
        }
    }
    Ok(Json(ApiResponse::success(report)))
}

/// The Sync peer a remote rig's rows go to after Apply, if its profile
/// names one that is still registered.
fn planned_push(
    profile: Option<&psf_guard_director_meta::profile::RigProfile>,
    known_peers: &[PeerEntry],
    warnings: &mut Vec<String>,
) -> Option<Push> {
    let peer_id = profile.and_then(|p| p.peer_id.as_deref())?;
    match known_peers.iter().find(|peer| peer.id == peer_id) {
        Some(peer) => Some(Push::planned(peer)),
        None => {
            warnings.push(format!(
                "Its peer '{peer_id}' is no longer registered; the plan stays on this server until Setup names another."
            ));
            None
        }
    }
}

/// Send a rig database's planning rows to the peer that holds the rig's real
/// database: a Sync planning push, reviewed and applied on the peer. Captures
/// and grades never travel this way.
async fn push_planning(state: &AppState, context: &DatabaseContext, peer: &PeerEntry) -> Push {
    let mut push = Push::planned(peer);
    // The same lock a local Sync apply takes, so a pull cannot rewrite the
    // rows while the bundle is being read.
    let _guard = state.sync_apply_lock.lock().await;
    match sync_remote(RemoteSyncOptions {
        direction: RemoteDirection::PushPlanning,
        local_path: PathBuf::from(&context.database_path),
        local_id: context.id.clone(),
        peer_url: peer.base_url.clone(),
        peer_token: peer.token.clone(),
        peer_catalog: peer.catalog_id.clone(),
        reviewed_only: true,
        dry_run: false,
        with_image_data: false,
    })
    .await
    {
        Ok(outcome) => {
            push.applied = outcome.applied;
            push.summary = outcome.summary;
        }
        Err(error) => {
            tracing::warn!(peer = %peer.id, catalog = %context.id, error = %format!("{error:#}"), "Director planning push failed");
            push.error = Some(format!("{error:#}"));
        }
    }
    push
}

/// Push the last activation's rows again to every remote rig's peer, for a
/// peer that was unreachable at Apply or has since been repaired.
pub(super) async fn push(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<Json<ApiResponse<PushReport>>, ActivationError> {
    let service = writable(&state)?;
    let catalogs: Vec<Arc<DatabaseContext>> = state
        .databases
        .read()
        .map_err(|_| Error::Internal)?
        .values()
        .cloned()
        .collect();
    let peers = registered_peers(&state);
    let (project, revision, targets, mut warnings) = service
        .clone()
        .with_reader(move |store| {
            let project = store.project(id)?.ok_or(Error::Missing)?;
            let activation = store
                .activation(id)?
                .ok_or(ActivationError::NotReady("Apply an activation first."))?;
            let mut targets: Vec<(NamedIdentity, Arc<DatabaseContext>, String)> = Vec::new();
            let found = identified_catalogs(&catalogs, service.instance_id);
            let mut warnings = found.duplicates.clone();
            // Every rig the activation reached, including those it set
            // Inactive, which a peer that was offline still needs to hear.
            let reached = activation
                .rigs
                .iter()
                .map(|r| (r.rig_id, r.catalog_id))
                .chain(
                    activation
                        .inactive_rigs
                        .iter()
                        .map(|r| (r.rig_id, r.catalog_id)),
                );
            for (rig_id, catalog_id) in reached {
                let rig = store.rig(rig_id)?.ok_or(Error::Missing)?;
                let Some(peer_id) = store
                    .rig_profile(rig_id)?
                    .and_then(|profile| profile.peer_id)
                else {
                    continue;
                };
                match found.get(catalog_id) {
                    Some(context) => targets.push((rig, context.clone(), peer_id)),
                    None => warnings.push(format!(
                        "{}: its database is no longer registered on this server.",
                        rig.name
                    )),
                }
            }
            Ok::<_, ActivationError>((project, activation.revision, targets, warnings))
        })
        .await?;
    let mut rigs = Vec::with_capacity(targets.len());
    for (rig, context, peer_id) in targets {
        let push = match peers.iter().find(|peer| peer.id == peer_id) {
            Some(peer) => push_planning(&state, &context, peer).await,
            None => Push {
                peer_id: peer_id.clone(),
                peer_name: peer_id.clone(),
                applied: false,
                summary: BTreeMap::new(),
                error: Some(
                    "This peer is no longer registered; name another under Rigs, Setup.".into(),
                ),
            },
        };
        rigs.push(PushedRig {
            rig,
            catalog_name: context.name.clone(),
            push,
        });
    }
    if rigs.is_empty() && warnings.is_empty() {
        warnings.push(
            "No rig in this activation pushes to a remote site; name a peer under Rigs, Setup."
                .into(),
        );
    }
    Ok(Json(ApiResponse::success(PushReport {
        project,
        activation_revision: revision,
        rigs,
        warnings,
    })))
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

struct Inputs<'a> {
    project: &'a NamedIdentity,
    framing: &'a FramingDraft,
    plan: &'a PlanDraft,
    /// This rig's panels, from its own framing or the shared one.
    panels: &'a [Panel],
    position_angle_degrees: f64,
    contributions: &'a [&'a Contribution],
    /// The last activation set this rig's project Inactive when the rig was
    /// turned off; now that it is on, the project goes Active again.
    reactivate: bool,
    rig_id: Uuid,
    catalog: CatalogIdentity,
    existing_link: Option<Uuid>,
    instance: Uuid,
    now: u64,
    /// Target Scheduler scheduling limits for this rig and project.
    scheduling: &'a ResolvedScheduling,
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
    ensure_director_tables(tx)?;
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
    // A row made here takes every resolved limit; an existing one only those
    // someone set, so a value edited by hand in Target Scheduler stays.
    let mut fresh_row = false;
    let project_guid = match owned {
        Some(guid) => {
            if project_row(tx, &guid)?.is_some() {
                let state: i64 = tx.query_row(
                    "SELECT IFNULL(state, 0) FROM project WHERE guid=?1",
                    [&guid],
                    |row| row.get(0),
                )?;
                let back_on = inputs.reactivate && state == 2;
                tx.execute(
                    "UPDATE project SET isMosaic=?2, state=CASE WHEN state=0 OR (?3 AND state=2) THEN 1 ELSE state END WHERE guid=?1",
                    params![guid, i32::from(mosaic), back_on],
                )?;
                changes.push(Change {
                    kind: "project",
                    action: if back_on { "update" } else { "unchanged" },
                    name: project_name.clone(),
                    detail: if back_on {
                        "Active again in Target Scheduler: this rig is on in the plan".into()
                    } else {
                        "Director's project from the last activation".into()
                    },
                });
                guid
            } else {
                fresh_row = true;
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
                // Director plans one target or one mosaic per project. A
                // project with more targets than this rig's panels would have
                // some left unplanned while its exposure plans are taken over,
                // so this rig is left to Target Scheduler until they agree.
                if let Some((row_id, name)) = project_row(tx, &guid)? {
                    let targets: i64 = tx.query_row(
                        "SELECT COUNT(*) FROM target WHERE projectid=?1",
                        [row_id],
                        |row| row.get(0),
                    )?;
                    if targets > inputs.panels.len() as i64 {
                        return Err(RigError::Skip(format!(
                            "{name} has {targets} targets in Target Scheduler and this plan frames {} here. Director plans one target or one mosaic per project for now, so this rig is left to Target Scheduler.",
                            inputs.panels.len()
                        )));
                    }
                }
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
                fresh_row = true;
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
    write_scheduling(
        tx,
        &project_guid,
        inputs.scheduling,
        fresh_row,
        &project_name,
        &mut changes,
    )?;
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

    // Targets: one per panel this rig owns, matched by panel id. A
    // contribution with no panel list covers every panel.
    let owned_panels: std::collections::BTreeSet<&str> = inputs
        .contributions
        .iter()
        .flat_map(|c| {
            if c.panel_ids.is_empty() {
                inputs
                    .panels
                    .iter()
                    .map(|p| p.footprint.id.as_str())
                    .collect::<Vec<_>>()
            } else {
                c.panel_ids.iter().map(String::as_str).collect()
            }
        })
        .collect();
    let mut targets = Vec::new();
    for panel in inputs
        .panels
        .iter()
        .filter(|p| owned_panels.contains(p.footprint.id.as_str()))
    {
        let name = if mosaic {
            format!("{target_base} {}", panel.footprint.id)
        } else {
            target_base.clone()
        };
        let ra_hours = panel.footprint.center.ra_degrees / 15.0;
        let dec = panel.footprint.center.dec_degrees;
        let rotation = inputs.position_angle_degrees;
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
        // The name Target Scheduler will show for this target, so the plan
        // rows below say which target they land on.
        let mut shown = name.clone();
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
                            name: name0.clone(),
                            detail,
                        });
                        shown = name0;
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
            // Nothing owned yet: a target already in this project (imported
            // from Target Scheduler, or made by hand) is taken over rather
            // than doubled.
            None => match adoptable_target(tx, project_row_id, &name, ra_hours, dec, !mosaic)? {
                Some(found) => {
                    let same = (found.ra_hours - ra_hours).abs() < 1e-7
                        && (found.dec - dec).abs() < 1e-6
                        && (found.rotation - rotation).abs() < 1e-3;
                    if !same {
                        tx.execute(
                            "UPDATE target SET ra=?2, dec=?3, rotation=?4 WHERE guid=?1",
                            params![found.guid, ra_hours, dec, rotation],
                        )?;
                    }
                    changes.push(Change {
                        kind: "target",
                        action: if same { "unchanged" } else { "adopt" },
                        name: found.name.clone(),
                        detail: if same {
                            format!("already in Target Scheduler at {detail}")
                        } else {
                            format!("takes over the existing target, moving it to {detail}")
                        },
                    });
                    shown = found.name;
                    found.guid
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
            },
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
        targets.push((panel.footprint.id.clone(), target_guid, shown));
    }

    // Exposure plans: one per contribution and target.
    let mut plans = Vec::new();
    let mut reported_templates = std::collections::BTreeSet::new();
    // Plans taken over in this activation, so two contributions never take
    // the same one.
    let mut claimed = std::collections::BTreeSet::new();
    // Every (contribution, target) this activation plans; any other plan
    // Director gave this project is turned off below.
    let mut planned: std::collections::BTreeSet<(String, String)> =
        std::collections::BTreeSet::new();
    // Contributions the plan still holds, on or off. A plan owned by any
    // other (a rig dropped and added back, an objective made anew) is free
    // to be taken over again, frames and all, instead of doubled.
    let held: std::collections::BTreeSet<String> = inputs
        .plan
        .contributions
        .iter()
        .map(|c| c.id.to_string())
        .collect();
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
        let template = resolve_template(tx, &profile_id, contribution)?;
        let template_id = template.id;
        if template.created && reported_templates.insert(template_id) {
            changes.push(Change {
                kind: "template",
                action: "create",
                name: format!("#{} {}", template.id, template.name),
                detail: format!(
                    "{}: no template in this database has these settings",
                    contribution.template.filter_name
                ),
            });
        }
        for (panel_id, target_guid, target_name) in targets.iter().filter(|(panel_id, _, _)| {
            contribution.panel_ids.is_empty() || contribution.panel_ids.contains(panel_id)
        }) {
            let _ = panel_id;
            let target_row: i64 = tx.query_row(
                "SELECT Id FROM target WHERE guid=?1",
                [target_guid],
                |row| row.get(0),
            )?;
            let name = format!(
                "{target_name} · {} · {} s",
                template.name, contribution.exposure_seconds
            );
            let detail = format!(
                "{frames} frames, template #{} {}",
                template.id, template.name
            );
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
                // Nothing owned yet: the rig's own plan for the same work on
                // this target (imported from Target Scheduler, or made by
                // hand) is taken over rather than doubled.
                None => match adoptable_plan(
                    tx,
                    target_row,
                    template_id,
                    contribution.exposure_seconds,
                    &claimed,
                    &held,
                )? {
                    Some(found) => {
                        claimed.insert(found.row_id);
                        let mut notes = Vec::new();
                        if found.desired != i64::from(frames) {
                            notes.push(format!("desired {} → {frames}", found.desired));
                        }
                        if found.template_id != template_id {
                            notes.push(format!(
                                "template #{} → #{} {} (same settings)",
                                found.template_id, template.id, template.name
                            ));
                        }
                        if !found.enabled {
                            notes.push("turned on".to_string());
                        }
                        // A row left at the template's default exposure is
                        // given the length explicitly; that is a write too.
                        let same = notes.is_empty() && found.explicit_exposure;
                        if !same {
                            tx.execute(
                                "UPDATE exposureplan SET exposure=?2, desired=?3, exposureTemplateId=?4, enabled=1 WHERE Id=?1",
                                params![found.row_id, contribution.exposure_seconds, i64::from(frames), template_id],
                            )?;
                        }
                        changes.push(Change {
                            kind: "plan",
                            action: if same { "unchanged" } else { "adopt" },
                            name: name.clone(),
                            detail: if same {
                                format!(
                                    "already in Target Scheduler as plan #{} ({} of {} frames taken)",
                                    found.row_id, found.acquired, found.desired
                                )
                            } else {
                                format!(
                                    "takes over plan #{} ({} of {} frames taken){}",
                                    found.row_id,
                                    found.acquired,
                                    found.desired,
                                    if notes.is_empty() {
                                        String::new()
                                    } else {
                                        format!("; {}", notes.join(", "))
                                    }
                                )
                            },
                        });
                        found.guid
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
                            detail: format!(
                                "{detail}; no plan on this target takes {} s with these settings",
                                contribution.exposure_seconds
                            ),
                        });
                        guid
                    }
                },
            };
            tx.execute(
                "INSERT INTO psf_guard_director_plan(exposureplan_guid,target_guid,contribution_id,objective_id,bandpass_id,purpose,required_frames,plan_revision)
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8)
                 ON CONFLICT(exposureplan_guid) DO UPDATE SET contribution_id=excluded.contribution_id, objective_id=excluded.objective_id, required_frames=excluded.required_frames, plan_revision=excluded.plan_revision, bandpass_id=excluded.bandpass_id, purpose=excluded.purpose",
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
            planned.insert((contribution.id.to_string(), target_guid.clone()));
            plans.push(ActivatedPlan {
                contribution_id: contribution.id,
                objective_id: objective.id,
                target_guid: Uuid::parse_str(target_guid).map_err(|_| Error::Internal)?,
                exposureplan_guid: Uuid::parse_str(&plan_guid).map_err(|_| Error::Internal)?,
                required_frames: frames,
            });
        }
    }
    // A plan Director gave this project that nothing plans now (its rig or
    // objective turned off or removed, its band changed, its panel gone or
    // no longer in the contribution's list) is turned off, so Target
    // Scheduler stops taking it. Its frames and grades stay.
    {
        let mut statement = tx.prepare(
            "SELECT d.contribution_id, d.target_guid, e.Id, e.exposure, IFNULL(e.acquired, 0), IFNULL(e.desired, 0), t.name, tg.name
             FROM psf_guard_director_plan d
             JOIN psf_guard_director_target dt ON dt.target_guid = d.target_guid
             JOIN exposureplan e ON e.guid = d.exposureplan_guid
             LEFT JOIN target tg ON tg.guid = d.target_guid
             LEFT JOIN exposuretemplate t ON t.Id = e.exposureTemplateId
             WHERE dt.project_guid = ?1 AND COALESCE(e.enabled, 1) = 1
             ORDER BY e.Id",
        )?;
        type Owned = (
            String,
            String,
            i64,
            Option<f64>,
            i64,
            i64,
            Option<String>,
            Option<String>,
        );
        let owned: Vec<Owned> = statement
            .query_map([&project_guid], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                ))
            })?
            .collect::<Result<_, _>>()?;
        for (
            contribution_id,
            target_guid,
            row_id,
            exposure,
            acquired,
            desired,
            template_name,
            target_name,
        ) in owned
        {
            if planned.contains(&(contribution_id, target_guid)) {
                continue;
            }
            tx.execute("UPDATE exposureplan SET enabled=0 WHERE Id=?1", [row_id])?;
            changes.push(Change {
                kind: "plan",
                action: "disable",
                name: format!(
                    "{} · {} · {} s",
                    target_name.unwrap_or_default(),
                    template_name.unwrap_or_default(),
                    exposure.unwrap_or(0.0)
                ),
                detail: format!(
                    "plan #{row_id} ({acquired} of {desired} frames taken) is no longer in this plan; Target Scheduler stops taking it"
                ),
            });
        }
    }
    // The rig's own plans on these targets that no contribution took stay
    // as they are in Target Scheduler. Say so, so the preview accounts for
    // every plan on the target.
    for (_, target_guid, target_name) in &targets {
        let mut statement = tx.prepare(
            "SELECT e.Id, e.exposure, IFNULL(e.acquired, 0), IFNULL(e.desired, 0), t.name, t.defaultexposure
             FROM exposureplan e
             JOIN target tg ON tg.Id = e.targetid
             LEFT JOIN exposuretemplate t ON t.Id = e.exposureTemplateId
             WHERE tg.guid = ?1 AND COALESCE(e.enabled, 1) = 1
               AND (e.guid IS NULL OR e.guid NOT IN (SELECT exposureplan_guid FROM psf_guard_director_plan))
             ORDER BY e.Id",
        )?;
        type Unclaimed = (i64, Option<f64>, i64, i64, Option<String>, Option<f64>);
        let unclaimed: Vec<Unclaimed> = statement
            .query_map([target_guid], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            })?
            .collect::<Result<_, _>>()?;
        for (row_id, exposure, acquired, desired, template_name, default_exposure) in unclaimed {
            let exposure = exposure
                .filter(|e| *e > 0.0)
                .or(default_exposure)
                .unwrap_or(0.0);
            changes.push(Change {
                kind: "plan",
                action: "keep",
                name: format!(
                    "{target_name} · {} · {exposure} s",
                    template_name.unwrap_or_default()
                ),
                detail: format!(
                    "plan #{row_id} ({acquired} of {desired} frames taken) is not part of this plan; left as it is"
                ),
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
    match contribution.goal_for(objective) {
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

/// One Target Scheduler scheduling limit: its plan field, its project
/// column, how it reads in the report, and the resolved value.
struct Limit {
    field: &'static str,
    column: &'static str,
    label: &'static str,
    value: f64,
}

fn limits(values: &SchedulingValues) -> [Limit; 9] {
    let flag = |v: bool| if v { 1.0 } else { 0.0 };
    [
        Limit {
            field: "minimum_time_minutes",
            column: "minimumtime",
            label: "minimum time",
            value: values.minimum_time_minutes.into(),
        },
        Limit {
            field: "minimum_altitude_degrees",
            column: "minimumaltitude",
            label: "minimum altitude",
            value: values.minimum_altitude_degrees,
        },
        Limit {
            field: "maximum_altitude_degrees",
            column: "maximumAltitude",
            label: "maximum altitude",
            value: values.maximum_altitude_degrees,
        },
        Limit {
            field: "use_custom_horizon",
            column: "usecustomhorizon",
            label: "custom horizon",
            value: flag(values.use_custom_horizon),
        },
        Limit {
            field: "horizon_offset_degrees",
            column: "horizonoffset",
            label: "horizon offset",
            value: values.horizon_offset_degrees,
        },
        Limit {
            field: "meridian_window_minutes",
            column: "meridianwindow",
            label: "meridian window",
            value: values.meridian_window_minutes.into(),
        },
        Limit {
            field: "filter_switch_frequency",
            column: "filterswitchfrequency",
            label: "filter switch",
            value: values.filter_switch_frequency.into(),
        },
        Limit {
            field: "dither_every",
            column: "ditherevery",
            label: "dither",
            value: values.dither_every.into(),
        },
        Limit {
            field: "smart_exposure_order",
            column: "smartexposureorder",
            label: "smart exposure order",
            value: flag(values.smart_exposure_order),
        },
    ]
}

/// A limit's value as the activation report shows it.
fn describe(field: &str, value: f64) -> String {
    let whole = value.round() as i64;
    match field {
        "minimum_time_minutes" => format!("{whole} min"),
        "maximum_altitude_degrees" if value == 0.0 => "none".into(),
        "minimum_altitude_degrees" | "maximum_altitude_degrees" | "horizon_offset_degrees" => {
            format!("{value}°")
        }
        "meridian_window_minutes" if whole == 0 => "off".into(),
        "meridian_window_minutes" => format!("{whole} min"),
        "filter_switch_frequency" if whole == 0 => "automatic".into(),
        "dither_every" if whole == 0 => "per template".into(),
        "filter_switch_frequency" | "dither_every" => format!("every {whole}"),
        _ => {
            if value != 0.0 {
                "on".into()
            } else {
                "off".into()
            }
        }
    }
}

/// Write the plan's scheduling limits into the Target Scheduler project.
/// A project made by this activation takes every resolved limit; an existing
/// one only the limits someone set at a scope, so a value edited by hand in
/// Target Scheduler is not reset to a default nobody chose. Columns an older
/// Target Scheduler schema lacks are skipped.
fn write_scheduling(
    tx: &Connection,
    project_guid: &str,
    scheduling: &ResolvedScheduling,
    fresh_row: bool,
    project_name: &str,
    changes: &mut Vec<Change>,
) -> Result<(), RigError> {
    let present: std::collections::BTreeSet<String> = {
        let mut statement = tx.prepare("SELECT lower(name) FROM pragma_table_info('project')")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let mut notes = Vec::new();
    for limit in limits(&scheduling.values) {
        if !present.contains(&limit.column.to_ascii_lowercase())
            || !(fresh_row || scheduling.sources.contains_key(limit.field))
        {
            continue;
        }
        let current: Option<f64> = tx.query_row(
            &format!("SELECT \"{}\" FROM project WHERE guid=?1", limit.column),
            [project_guid],
            |row| row.get(0),
        )?;
        if current.is_some_and(|now| (now - limit.value).abs() < 1e-9) {
            continue;
        }
        tx.execute(
            &format!("UPDATE project SET \"{}\"=?2 WHERE guid=?1", limit.column),
            params![project_guid, limit.value],
        )?;
        let was = current.map_or_else(|| "unset".to_string(), |now| describe(limit.field, now));
        notes.push(format!(
            "{} {was} → {}",
            limit.label,
            describe(limit.field, limit.value)
        ));
    }
    if !notes.is_empty() {
        changes.push(Change {
            kind: "project",
            action: if fresh_row { "create" } else { "update" },
            name: format!("{project_name} · scheduling limits"),
            detail: notes.join(", "),
        });
    }
    Ok(())
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

/// A target row of this project that no panel owns yet.
struct Adoptable {
    guid: String,
    name: String,
    ra_hours: f64,
    dec: f64,
    rotation: f64,
}

/// The existing target a panel should take over, if any: the one with the
/// panel's name, else the one at the panel's place (within a quarter of an
/// arcminute, an import's round trip), else, for a single-panel framing,
/// the project's only unowned target. Ownership is by GUID, so a match
/// without one refuses the rig.
fn adoptable_target(
    tx: &Connection,
    project_row_id: i64,
    name: &str,
    ra_hours: f64,
    dec: f64,
    single_panel: bool,
) -> Result<Option<Adoptable>, RigError> {
    let mut statement = tx.prepare(
        "SELECT Id, guid, name, ra, dec, rotation FROM target
         WHERE projectid=?1
           AND (guid IS NULL OR guid NOT IN (SELECT target_guid FROM psf_guard_director_target))
         ORDER BY Id",
    )?;
    let rows: Vec<(i64, Option<String>, String, f64, f64, f64)> = statement
        .query_map([project_row_id], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                row.get::<_, Option<f64>>(3)?.unwrap_or(f64::NAN),
                row.get::<_, Option<f64>>(4)?.unwrap_or(f64::NAN),
                row.get::<_, Option<f64>>(5)?.unwrap_or(0.0),
            ))
        })?
        .collect::<Result<_, _>>()?;
    let wanted = name.trim().to_lowercase();
    let by_name = rows
        .iter()
        .position(|row| row.2.trim().to_lowercase() == wanted);
    let by_place = rows.iter().position(|row| {
        (row.3 - ra_hours).abs() * 15.0 * 60.0 < 0.25 && (row.4 - dec).abs() * 60.0 < 0.25
    });
    let only = (single_panel && rows.len() == 1).then_some(0);
    let Some(index) = by_name.or(by_place).or(only) else {
        return Ok(None);
    };
    let (row_id, guid, found_name, found_ra, found_dec, found_rotation) = rows[index].clone();
    let guid = match guid {
        Some(guid) if !guid.is_empty() => guid,
        _ => return Err(missing_guid(&format!("Target #{row_id} {found_name}"))),
    };
    Ok(Some(Adoptable {
        guid,
        name: found_name,
        ra_hours: found_ra,
        dec: found_dec,
        rotation: found_rotation,
    }))
}

/// A row Director would take over has no GUID. Minting one here would give a
/// sync copy GUIDs the rig's own database lacks, so the rig waits for Fill in
/// GUIDs instead, which also keeps Director from making a twin of the row.
fn missing_guid(what: &str) -> RigError {
    RigError::Skip(format!(
        "{what} has no GUID yet. Fill in GUIDs for this database under Settings, Databases, then activate again."
    ))
}

/// An exposure plan already on a target that a contribution takes over.
struct AdoptablePlan {
    row_id: i64,
    guid: String,
    desired: i64,
    acquired: i64,
    template_id: i64,
    enabled: bool,
    /// The row names its exposure, not the template's default.
    explicit_exposure: bool,
}

/// The target's own exposure plan for the work a contribution asks for, if
/// one exists that no contribution the plan still holds owns (a plan left
/// by a rig dropped and added back is taken back, frames and all): on the
/// resolved template, else on
/// one with the same filter, gain, offset, binning and readout mode, and at
/// the same exposure length either way. Another length is other work (Ha at
/// 600 s does not replace Ha at 300 s), so it is never taken over. A plan
/// left at Target Scheduler's "template default" exposure counts at the
/// template's default. Ownership is by GUID, so a match without one refuses
/// the rig.
fn adoptable_plan(
    tx: &Connection,
    target_row: i64,
    template_id: i64,
    exposure_seconds: f64,
    claimed: &std::collections::BTreeSet<i64>,
    held: &std::collections::BTreeSet<String>,
) -> Result<Option<AdoptablePlan>, RigError> {
    type Settings = (String, i64, i64, i64, i64);
    let settings_of = |id: i64| -> rusqlite::Result<Option<Settings>> {
        tx.query_row(
            "SELECT LOWER(TRIM(IFNULL(filtername, ''))), IFNULL(gain, -1), IFNULL(offset, -1),
                    CASE WHEN IFNULL(bin, 1) < 1 THEN 1 ELSE bin END, IFNULL(readoutmode, -1)
             FROM exposuretemplate WHERE Id = ?1",
            [id],
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
        .optional()
    };
    let wanted = settings_of(template_id)?;
    let mut statement = tx.prepare(
        "SELECT e.Id, e.guid, e.exposure, IFNULL(e.desired, 0), IFNULL(e.acquired, 0),
                e.exposureTemplateId, COALESCE(e.enabled, 1), t.defaultexposure, d.contribution_id
         FROM exposureplan e
         LEFT JOIN exposuretemplate t ON t.Id = e.exposureTemplateId
         LEFT JOIN psf_guard_director_plan d ON d.exposureplan_guid = e.guid
         WHERE e.targetid = ?1
         ORDER BY e.Id",
    )?;
    type Row = (
        i64,
        Option<String>,
        Option<f64>,
        i64,
        i64,
        i64,
        i64,
        Option<f64>,
        Option<String>,
    );
    let rows: Vec<Row> = statement
        .query_map([target_row], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
                row.get(8)?,
            ))
        })?
        .collect::<Result<_, _>>()?;
    let same_length = |row: &Row| {
        let exposure = match row.2 {
            Some(exposure) if exposure > 0.0 => exposure,
            _ => row.7.unwrap_or(f64::NAN),
        };
        (exposure - exposure_seconds).abs() < 1e-6
    };
    let candidates: Vec<&Row> = rows
        .iter()
        // A plan no Director contribution owns, or one owned by a
        // contribution the plan no longer holds.
        .filter(|row| row.8.as_ref().is_none_or(|owner| !held.contains(owner)))
        .filter(|row| !claimed.contains(&row.0) && same_length(row))
        .collect();
    let mut found = candidates.iter().find(|row| row.5 == template_id).copied();
    if found.is_none() && wanted.is_some() {
        for row in &candidates {
            if settings_of(row.5)? == wanted {
                found = Some(row);
                break;
            }
        }
    }
    let Some(row) = found else {
        return Ok(None);
    };
    let guid = match &row.1 {
        Some(guid) if !guid.is_empty() => guid.clone(),
        _ => return Err(missing_guid(&format!("Exposure plan #{}", row.0))),
    };
    Ok(Some(AdoptablePlan {
        row_id: row.0,
        guid,
        desired: row.3,
        acquired: row.4,
        template_id: row.5,
        enabled: row.6 != 0,
        explicit_exposure: row.2.is_some_and(|exposure| exposure > 0.0),
    }))
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

/// The template a contribution resolves to, and whether activation made it.
struct ResolvedTemplate {
    id: i64,
    name: String,
    created: bool,
}

/// The chosen template by id or GUID when it still exists with the same
/// filter, else the profile's template with the same settings, else a new one.
fn resolve_template(
    tx: &Connection,
    profile_id: &str,
    contribution: &Contribution,
) -> Result<ResolvedTemplate, RigError> {
    let chosen = |id: i64| -> Result<ResolvedTemplate, RigError> {
        let name: String = tx.query_row(
            "SELECT IFNULL(name, '') FROM exposuretemplate WHERE Id=?1",
            [id],
            |row| row.get(0),
        )?;
        Ok(ResolvedTemplate {
            id,
            name,
            created: false,
        })
    };
    let choice = &contribution.template;
    let moon_matches = |id| -> Result<bool, RigError> {
        Ok(match &choice.moon {
            None => true,
            Some(wanted) => super::plan::read_moon_policy(tx, id)? == *wanted,
        })
    };
    let mut guid_used = false;
    if let Some(id) = choice.template_id {
        let found: Option<String> = tx
            .query_row(
                "SELECT filtername FROM exposuretemplate WHERE Id=?1",
                [id],
                |row| row.get(0),
            )
            .optional()?;
        if found.is_some_and(|filter| {
            filter
                .trim()
                .eq_ignore_ascii_case(choice.filter_name.trim())
        }) && moon_matches(id)?
        {
            return chosen(id);
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
        guid_used = found.is_some();
        if let Some((id, filter)) = found
            && filter.eq_ignore_ascii_case(&choice.filter_name)
            && moon_matches(id)?
        {
            return chosen(id);
        }
    }
    let gain = choice.gain.unwrap_or(-1);
    let offset = choice.offset.unwrap_or(-1);
    let bin = choice.bin.unwrap_or(1);
    let readout = choice.readout_mode.unwrap_or(-1);
    let existing: Vec<i64> = tx
        .prepare(
            "SELECT Id FROM exposuretemplate
             WHERE profileId = ?1 AND LOWER(TRIM(filtername)) = LOWER(TRIM(?2))
               AND IFNULL(gain, -1) = ?3 AND IFNULL(offset, -1) = ?4
               AND IFNULL(bin, 1) = ?5 AND IFNULL(readoutmode, -1) = ?6
             ORDER BY Id LIMIT 512",
        )?
        .query_map(
            params![profile_id, choice.filter_name, gain, offset, bin, readout],
            |row| row.get(0),
        )?
        .collect::<Result<_, _>>()?;
    for id in existing {
        if moon_matches(id)? {
            return chosen(id);
        }
    }
    let moon = choice.moon.clone().unwrap_or_default();
    tx.execute(
        "INSERT INTO exposuretemplate (
            profileId, name, filtername, gain, offset, bin, readoutmode,
            twilightlevel, moonavoidanceenabled, moonavoidanceseparation,
            moonavoidancewidth, maximumhumidity, defaultexposure,
            moonrelaxscale, moonrelaxmaxaltitude, moonrelaxminaltitude,
            moondownenabled, ditherevery, minutesOffset, guid
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, ?10, ?11, ?12, 0, ?8, ?13, ?14, ?15, ?16, -1, 0, ?9)",
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
            // A library template keeps its own GUID in every rig database,
            // so a second activation, and Sync, know it for the same one.
            choice
                .template_guid
                .filter(|_| !guid_used)
                .map(|guid| guid.to_string())
                .unwrap_or_else(new_guid),
            i64::from(moon.enabled), moon.separation_degrees, moon.width_days,
            moon.relax_degrees_per_degree, moon.relax_max_altitude_degrees,
            moon.relax_min_altitude_degrees, i64::from(moon.moon_down),
        ],
    )?;
    Ok(ResolvedTemplate {
        id: tx.last_insert_rowid(),
        name: if choice.name.is_empty() {
            choice.filter_name.clone()
        } else {
            choice.name.clone()
        },
        created: true,
    })
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

#[cfg(test)]
mod goal_tests {
    use super::*;
    use psf_guard_director_meta::plan::TemplateChoice;

    #[test]
    fn a_rigs_own_goal_sets_the_frames_it_is_asked_for() {
        let objective = Objective {
            id: Uuid::new_v4(),
            bandpass_id: "h_alpha".into(),
            purpose: "faint_detail".into(),
            goal: Goal::Hours { value: 6.0 },
            priority: 1,
        };
        let mut contribution = Contribution {
            id: Uuid::new_v4(),
            objective_id: objective.id,
            rig_id: Uuid::new_v4(),
            template: TemplateChoice {
                template_guid: None,
                template_id: Some(1),
                name: "Ha 300".into(),
                filter_name: "Ha".into(),
                gain: None,
                offset: None,
                bin: None,
                readout_mode: None,
                moon: None,
            },
            exposure_seconds: 300.0,
            panel_ids: vec![],
            enabled: true,
            goal: None,
        };
        // 6 h at 300 s.
        assert_eq!(required_frames(&objective, &contribution), Some(72));
        // A slow rig set to 24 h of its own.
        contribution.goal = Some(Goal::Hours { value: 24.0 });
        assert_eq!(required_frames(&objective, &contribution), Some(288));
        contribution.goal = Some(Goal::Frames { value: 50 });
        assert_eq!(required_frames(&objective, &contribution), Some(50));
    }
}
