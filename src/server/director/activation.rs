//! Push a project's framing and plan into each participating rig database:
//! one Target Scheduler project, a target per panel, an exposure plan per rig
//! objective. Preview performs the same writes and rolls them back, so what
//! the operator reviews is what Apply commits. Captures and grades are never
//! touched; removed panels leave their targets behind.

use super::framing::{cover_warnings, rig_field, ShootingRig};
use super::*;
use crate::{
    commands::sync::{sync_remote, RemoteDirection, RemoteSyncOptions},
    db_registry::PeerEntry,
    server::peers::registered_peers,
    ts_schema::new_guid,
};
use psf_guard_director_core::{bandpass::frames_for_hours, framing::Panel};
use psf_guard_director_meta::{
    activation::{
        ActivatedPlan, ActivatedRig, ActivatedTarget, Activation, InactiveRig, RemotePush,
    },
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
    time::{SystemTime, UNIX_EPOCH},
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
pub(super) struct Push {
    pub(super) peer_id: String,
    pub(super) peer_name: String,
    /// True once the peer applied the planning rows.
    pub(super) applied: bool,
    pub(super) summary: BTreeMap<String, i64>,
    pub(super) error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) ts_file: Option<file_delivery::Preview>,
    #[serde(skip)]
    pub(super) ts_sync: Option<psf_guard_director_meta::activation::TsSyncDelivery>,
}

impl Push {
    fn planned(peer: &PeerEntry) -> Self {
        Self {
            peer_id: peer.id.clone(),
            peer_name: peer.name.clone(),
            applied: false,
            summary: BTreeMap::new(),
            error: None,
            ts_file: None,
            ts_sync: None,
        }
    }
}

/// An exposure plan Target Scheduler shoots after the activation, as the
/// preview and the result list it.
#[derive(Clone, Serialize)]
struct Shot {
    /// Its row in Target Scheduler.
    plan: i64,
    target: String,
    template: String,
    filter: String,
    gain: Option<i64>,
    offset: Option<i64>,
    exposure_seconds: f64,
    desired: i64,
    acquired: i64,
    /// The objectives it serves; more than one when their frames are summed.
    objectives: Vec<Uuid>,
    /// It was off in Target Scheduler and comes back on.
    turned_on: bool,
}

#[derive(Serialize)]
struct RigReport {
    rig: NamedIdentity,
    catalog_slug: Option<String>,
    catalog_name: String,
    profile_id: Option<String>,
    changes: Vec<Change>,
    /// What Target Scheduler shoots in this rig's project afterwards.
    shoots: Vec<Shot>,
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
    #[serde(default)]
    collaboration: Vec<collaboration_activation::Visit>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Preview {
    #[serde(default)]
    collaboration: Vec<collaboration_activation::Visit>,
}

#[derive(Serialize)]
pub(super) struct Last {
    activation: Option<Activation>,
}

pub(super) enum ActivationError {
    Api(Error),
    NotReady(&'static str),
    /// Apply ran on the rig databases themselves and reached another digest
    /// than the preview's: nothing was written. The report is what Apply
    /// found, so the caller can say what changed.
    Changed(Box<Report>),
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
            Self::Changed(report) => (
                StatusCode::CONFLICT,
                Json(ApiResponse {
                    success: false,
                    data: Some(*report),
                    error: Some(
                        "The plan or a rig database changed since the preview; nothing was activated. Preview again."
                            .into(),
                    ),
                    status: None,
                }),
            )
                .into_response(),
        }
    }
}

pub(super) async fn last(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<Json<ApiResponse<Last>>, Error> {
    let peers = registered_peers(&state);
    let catalogs = state.all_databases();
    let instance = enabled(&state)?.instance_id;
    let activation = enabled(&state)?
        .query(move |store| {
            store.project(id)?.ok_or(StoreError::NotFound)?;
            let Some(mut activation) = store.activation(id)? else {
                return Ok(None);
            };
            // A push to a peer since removed from the registry, or one the
            // rig no longer sends to, is moot: the rig's next activation goes
            // where Setup points. The others carry the peer's name now.
            let found = identified_catalogs(&catalogs, instance);
            let mut pushes = Vec::new();
            for mut push in std::mem::take(&mut activation.pushes) {
                if let Some(delivery) = &push.ts_sync {
                    if let Some(context) = remote_push::catalog_of(&activation, push.rig_id)
                        .and_then(|id| found.get(id))
                        && push.peer_id == format!("ts:{}", context.id)
                        && file_delivery::is_current(context, delivery)
                    {
                        push.peer_name = format!("Target Scheduler ({})", context.name);
                        pushes.push(push);
                    }
                    continue;
                }
                let Some(peer) = peers.iter().find(|peer| peer.id == push.peer_id) else {
                    continue;
                };
                if remote_push::sends_to(store, push.rig_id, &peer.id)? {
                    push.peer_name = peer.name.clone();
                    pushes.push(push);
                }
            }
            activation.pushes = pushes;
            Ok(Some(activation))
        })
        .await?;
    Ok(Json(ApiResponse::success(Last { activation })))
}

pub(super) async fn preview(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    request: Option<Json<Preview>>,
) -> Result<Json<ApiResponse<Report>>, ActivationError> {
    let request = request.map(|Json(request)| request).unwrap_or_default();
    // On a copy of each rig's planning tables, as the check runs: the
    // dialog previews by itself whenever its inputs change, so a preview
    // must not take the rig databases' turn or N.I.N.A.'s write lock. The
    // copy holds every table an activation reads, so Apply, run on the
    // database itself, reaches the same digest unless a rig changed since.
    execute(state, id, None, true, false, request.collaboration).await
}

/// What an activation would change, worked out on a copy of each rig
/// database's planning tables. The page asks for an activation only while a
/// rig lacks the saved plan: a plan taken in from Target Scheduler, or rows
/// that came by Sync, already match. The copy never takes N.I.N.A.'s lock.
/// A collaboration plan is checked with the assignments it last took.
pub(super) async fn check(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<Json<ApiResponse<Report>>, ActivationError> {
    execute(state, id, None, true, true, vec![]).await
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
    execute(
        state,
        id,
        Some(request.preview_digest),
        false,
        false,
        request.collaboration,
    )
    .await
}

/// Two panel sizes the same to a millionth of a degree.
fn same_size(
    a: psf_guard_director_core::framing::PanelSize,
    b: psf_guard_director_core::framing::PanelSize,
) -> bool {
    (a.width_degrees - b.width_degrees).abs() <= 1e-6
        && (a.height_degrees - b.height_degrees).abs() <= 1e-6
}

pub(super) fn now_ms() -> u64 {
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
    on_copy: bool,
    checking: bool,
    visits: Vec<collaboration_activation::Visit>,
) -> Result<Json<ApiResponse<Report>>, ActivationError> {
    execute_guarded(state, id, expected, on_copy, checking, visits, None).await
}

/// Background consent is limited to one rig/catalog. Never apply another
/// rig's edits through a project shared with that rig.
pub(super) async fn background_apply(
    state: Arc<AppState>,
    id: Uuid,
    rig: Uuid,
    catalog: Uuid,
    visit: collaboration_activation::Visit,
) -> Result<bool, ActivationError> {
    let service = writable(&state)?;
    let catalogs = state.all_databases();
    let instance = service.instance_id;
    let import_id = visit.import_id;
    let (current, recorded) = service
        .with_reader(move |store| {
            let current = collaboration_activation::current(store, id, instance, &catalogs)?;
            let imported = store
                .collaboration_import(import_id)?
                .ok_or(Error::Missing)?;
            let recorded = store.activation(id)?.is_some_and(|a| {
                a.applied_at_ms >= imported.imported_at_ms
                    && a.rigs
                        .iter()
                        .any(|r| r.rig_id == rig && r.catalog_id == catalog)
            });
            Ok::<_, ActivationError>((current, recorded))
        })
        .await?;
    // Rig commits and the metadata record are separate. A retry must repair a
    // missing record before it considers an already-written assignment complete.
    if recorded
        && current
            .iter()
            .any(|v| v.import_id == visit.import_id && v.source_digest == visit.source_digest)
    {
        return Ok(false);
    }
    let Json(preview) = execute_guarded(
        state.clone(),
        id,
        None,
        true,
        false,
        vec![visit.clone()],
        Some((rig, catalog)),
    )
    .await?;
    let preview = preview.data.ok_or(Error::Internal)?;
    let Json(applied) = execute_guarded(
        state,
        id,
        Some(preview.preview_digest),
        false,
        false,
        vec![visit],
        Some((rig, catalog)),
    )
    .await?;
    let applied = applied.data.ok_or(Error::Internal)?;
    if applied.rigs.len() == 1
        && applied.rigs[0].applied
        && applied.rigs[0].push.as_ref().is_none_or(|p| p.applied)
    {
        Ok(true)
    } else {
        Err(ActivationError::NotReady(
            "Activation could not reach this rig; preview the activation and try again.",
        ))
    }
}

async fn execute_guarded(
    state: Arc<AppState>,
    id: Uuid,
    expected: Option<String>,
    on_copy: bool,
    checking: bool,
    visits: Vec<collaboration_activation::Visit>,
    guard: Option<(Uuid, Uuid)>,
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
    // A check or a preview runs on an in-memory copy of each rig's planning
    // tables, taken in one read transaction, and writes nothing, so it skips
    // the turn at the rig databases that discovery, adoption and activations
    // queue for: opening a plan or its activation dialog never waits behind them,
    // nor they behind it. The copy cannot hold half of an activation, which
    // commits each rig database in one transaction; the pooled reader below
    // bounds how many run. Applies write, so they still take the turn.
    let catalog_permit = if on_copy && expected.is_none() {
        None
    } else {
        Some(admit(&service.discovery_admission).await?)
    };
    let sync_guard = if expected.is_some() {
        Some(state.sync_apply_lock.lock().await)
    } else {
        None
    };
    let peers = registered_peers(&state);
    let known_peers = peers.clone();
    let delivery_state = state.clone();
    // Everything up to the record runs on a pooled reader: the store's one
    // writer is taken only to record the result, so check-ins, program pulls
    // and saves never wait behind rig databases. The rig-database gate held
    // throughout keeps two activations apart.
    let recorder = service.clone();
    let (mut report, to_record, mut staged) = service.clone().with_reader(move |store| {
        let mut staged = file_delivery::Staged::new(delivery_state.clone());
        let _catalog_permit = catalog_permit;
        let applying = expected.is_some();
        let project = store.project(id)?.ok_or(Error::Missing)?;
        let is_collaboration = store.collaboration_requires_admission(id)?;
        // The page's status check reads the assignments the plan last took.
        let visits = if is_collaboration && checking && visits.is_empty() {
            collaboration_activation::current(store, id, service.instance_id, &catalogs)?
        } else { visits };
        if is_collaboration && visits.is_empty() {
            return Err(ActivationError::NotReady(
                "This collaboration import is an inactive draft. Choose its assignments for activation first.",
            ));
        }
        if !is_collaboration && !visits.is_empty() { return Err(Error::Invalid.into()); }
        if visits.len() > 32 { return Err(Error::Invalid.into()); }
        let admitted = collaboration_activation::admit(store, id, &visits)?;
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
        let panels = match framing.shared_request() {
            Some(request) => request.preview().map_err(|_| Error::Invalid)?.panels,
            None => vec![],
        };
        let mut by_rig: BTreeMap<Uuid, Vec<&Contribution>> = BTreeMap::new();
        // A plan saved before duplicates were refused may hold two
        // contributions for one rig and objective; the first counts, so the
        // work is never planned twice.
        let mut seen = std::collections::BTreeSet::new();
        for contribution in plan.contributions.iter().filter(|c| c.enabled) {
            if seen.insert((contribution.rig_id, contribution.objective_id)) {
                by_rig.entry(contribution.rig_id).or_default().push(contribution);
            }
        }
        // The last activation: which rigs had rows, and which projects it
        // set Inactive when their rig was turned off.
        let previous = store.activation(id)?;
        if let Some((rig, _)) = guard
            && (by_rig.len() != 1 || !by_rig.contains_key(&rig)
                || plan.contributions.iter().any(|c| c.rig_id != rig)
                || previous.as_ref().is_some_and(|a| a.rigs.iter().any(|r| r.rig_id != rig) || a.inactive_rigs.iter().any(|r| r.rig_id != rig)))
        {
            return Err(ActivationError::NotReady("Configure this rig's recipes in Planning; shared multi-rig plans need a manual activation."));
        }
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
        if is_collaboration && (by_rig.len() != admitted.len() || by_rig.keys().any(|rig| !admitted.contains_key(rig))) {
            return Err(ActivationError::NotReady("Select one collaboration assignment for every enabled rig, and no other rigs."));
        }
        for (rig, contributions) in &by_rig {
            if let Some(a) = admitted.get(rig) { a.validate_recipes(contributions)?; }
        }
        // Every objective should have some rig on every shared panel; say
        // where not. A rig framed on its own covers its own grid.
        let mut coverage_warnings: Vec<String> = Vec::new();
        // The rigs that get panels from a framing, for the layout's warnings.
        let mut shooting: Vec<ShootingRig> = Vec::new();
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
        if let Some((rig, catalog)) = guard
            && rig_catalogs.get(&rig).is_none_or(|c| c.identity.id != catalog)
        {
            return Err(ActivationError::NotReady("The approved rig database changed; review background settings."));
        }
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
                    changes: vec![], shoots: vec![],
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
            let field = rig_field(profile.as_ref());
            let Some(layout) = framing.layout_for(*rig_id, field) else {
                reports.push(RigReport {
                    rig,
                    catalog_slug: Some(catalog.context.id.clone()),
                    catalog_name: catalog.context.name.clone(),
                    profile_id: None,
                    changes: vec![], shoots: vec![],
                    warnings: vec![
                        "No panel size for this rig: give its framing a size in Framing, or set its optics under Rigs, Setup.".into(),
                    ],
                    applied: false,
                    push: None,
                });
                carried.extend(previous_rig(*rig_id));
                continue;
            };
            let assignment = admitted.get(rig_id);
            let rig_panels = if let Some(assignment) = assignment {
                warnings.push("Uses the assignment's exact panels, exposures and frame goals, not the editable framing grid. Target Scheduler runs normally, including later nights and grading retries; there is no nightly deadline. Reports retain the task and use the actual observing night. Remote quality and Moon requirements determine contribution credit, not TS scheduling.".into());
                assignment.panels.clone()
            } else if layout.own {
                // A separate framing sized from the rig's own field is only
                // laid out here; one the core refuses (too wide a mosaic)
                // is this rig's warning, not a refused request.
                match layout.request().preview() {
                    // Its layout's issues are said with the coverage.
                    Ok(preview) => preview.panels,
                    Err(error) => {
                        reports.push(RigReport {
                            rig,
                            catalog_slug: Some(catalog.context.id.clone()),
                            catalog_name: catalog.context.name.clone(),
                            profile_id: None,
                            changes: vec![], shoots: vec![],
                            warnings: vec![format!(
                                "Its separate framing cannot be laid out ({error:?}); make its grid smaller or its panels narrower in Framing."
                            )],
                            applied: false,
                            push: None,
                        });
                        carried.extend(previous_rig(*rig_id));
                        continue;
                    }
                }
            } else {
                // The shared panels are sized for one field; a rig with
                // another shoots them at that place, not as they were drawn.
                if layout.layout.panels.len() > 1
                    && let Some(own) = field
                    && !same_size(own, layout.panel)
                {
                    warnings.push(format!(
                        "Its field is {:.2}° × {:.2}°, the shared panels {:.2}° × {:.2}°: frame it separately, or size the shared mosaic by it.",
                        own.width_degrees, own.height_degrees, layout.panel.width_degrees, layout.panel.height_degrees
                    ));
                }
                panels.clone()
            };
            if assignment.is_none() {
                shooting.push(ShootingRig {
                    id: *rig_id,
                    name: catalog.context.name.clone(),
                    field,
                });
            }
            if profile.as_ref().is_none_or(|p| p.configuration.is_none()) {
                warnings.push("The N.I.N.A. plugin has not reported this rig's camera yet; the Target Scheduler plugin can still run these rows.".into());
            }
            // A rig on another PSF Guard: its rows land here first, then go
            // to the peer by Sync once Apply has committed them.
            let mut push = if guard.is_none() {
                planned_push(profile.as_ref(), &known_peers, &mut warnings)
            } else {
                None
            };
            // A preview performs the same writes and rolls them back, so the
            // connection is read-write either way; nothing lands without Apply.
            let mut connection = match open_rig_for(&catalog.context.database_path, on_copy, !checking) {
                Ok(connection) => connection,
                Err(reason) => {
                    if guard.is_some() {
                        tracing::warn!(%reason, "Automatic activation could not open its rig database");
                        return Err(Error::Internal.into());
                    }
                    warnings.push(reason);
                    reports.push(RigReport {
                        rig,
                        catalog_slug: Some(catalog.context.id.clone()),
                        catalog_name: catalog.context.name.clone(),
                        profile_id: None,
                        changes: vec![], shoots: vec![],
                        warnings,
                        applied: false,
                        push: None,
                    });
                    carried.extend(previous_rig(*rig_id));
                    continue;
                }
            };
            if guard.is_some() {
                // A background pass yields quickly to NINA and retries its saved deal.
                connection.busy_timeout(std::time::Duration::from_secs(2))?;
            }
            let outcome = {
                let tx = match begin_rig(&mut connection) {
                    Ok(tx) => tx,
                    Err(reason) => {
                        if guard.is_some() {
                            tracing::warn!(%reason, "Automatic activation could not lock its rig database");
                            return Err(Error::Busy.into());
                        }
                        warnings.push(reason);
                        reports.push(RigReport {
                            rig,
                            catalog_slug: Some(catalog.context.id.clone()),
                            catalog_name: catalog.context.name.clone(),
                            profile_id: None,
                            changes: vec![], shoots: vec![],
                            warnings,
                            applied: false,
                            push: None,
                        });
                        carried.extend(previous_rig(*rig_id));
                        continue;
                    }
                };
                // The project this plan is linked to in this database, and
                // those linked to other plans: a detach or an attach moves
                // a link, and the rig database's own record of who owns a
                // project follows the link, not the other way round.
                let (existing_link, foreign) = {
                    // Every page: a rig with more linked projects than one
                    // page holds must still find this one, or it would make
                    // a second project.
                    let mut after = None;
                    let mut mine = None;
                    let mut foreign = std::collections::BTreeSet::new();
                    loop {
                        let page = store.catalog_project_mappings(catalog.identity.id, after, 256)?;
                        for mapping in &page.items {
                            if mapping.project_id == id {
                                mine.get_or_insert(mapping.source_project_guid);
                            } else {
                                foreign.insert(mapping.source_project_guid.to_string());
                            }
                        }
                        match page.next_after {
                            Some(next) => after = Some(next),
                            None => break (mine, foreign),
                        }
                    }
                };
                let before = if guard.is_none() && catalog.context.ts_sync.is_some() {
                    Some(file_delivery::local_fingerprint(&tx).map_err(|_| Error::Internal)?)
                } else { None };
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
                        foreign: &foreign,
                        instance: service.instance_id,
                        now,
                        scheduling: &scheduling,
                        collaboration: assignment,
                    },
                );
                match outcome {
                    Ok(mut outcome) => {
                        if let Some(before) = before {
                            let collaboration_minimum = assignment
                                .and_then(|a| a.import.plan.share().requirements.as_ref())
                                .and_then(|r| r.min_altitude_degrees)
                                .filter(|minimum| *minimum > 0.0);
                            push = file_delivery::prepare(&catalog.context, &tx, before, &outcome.record.project_guid.to_string(), file_delivery::Intent {
                                active: true,
                                reactivate: previous.as_ref().is_some_and(|p| p.inactive_rigs.iter().any(|r| r.rig_id == *rig_id)),
                                scheduling: limits(&scheduling.values).into_iter().filter(|limit| scheduling.sources.contains_key(limit.field)).map(|limit| limit.column.into()).collect(),
                                minimum_altitude: collaboration_minimum,
                            }, &outcome.shoots.iter().map(|s| (s.plan, s.exposure_seconds)).collect::<Vec<_>>());
                            if let Some(push) = &push { for shot in &mut outcome.shoots {
                                if let Some((name,filter,gain,offset)) = file_delivery::shot_template(push, shot.plan) {
                                    shot.template=name;shot.filter=filter;shot.gain=gain;shot.offset=offset;
                                }
                            }}
                        }
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
                    changes: vec![], shoots: vec![],
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
                shoots: outcome.shoots,
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
            let mut push = if guard.is_none() {
                planned_push(profile.as_ref(), &known_peers, &mut warnings)
            } else {
                None
            };
            // A database that cannot be opened or locked now keeps what the
            // last activation knew, so a later one can finish the job.
            let mut connection = match open_rig_for(&catalog.context.database_path, on_copy, !checking) {
                Ok(connection) => connection,
                Err(reason) => {
                    warnings.push(reason);
                    reports.push(RigReport { rig, catalog_slug: Some(catalog.context.id.clone()), catalog_name: catalog.context.name.clone(), profile_id: None, changes: vec![], shoots: vec![], warnings, applied: false, push: None });
                    if already { inactive.push(was); } else { carried.extend(previous_rig(*rig_id)); }
                    continue;
                }
            };
            let tx = match begin_rig(&mut connection) {
                Ok(tx) => tx,
                Err(reason) => {
                    warnings.push(reason);
                    reports.push(RigReport { rig, catalog_slug: Some(catalog.context.id.clone()), catalog_name: catalog.context.name.clone(), profile_id: None, changes: vec![], shoots: vec![], warnings, applied: false, push: None });
                    if already { inactive.push(was); } else { carried.extend(previous_rig(*rig_id)); }
                    continue;
                }
            };
            let before = if guard.is_none() && catalog.context.ts_sync.is_some() {
                Some(file_delivery::local_fingerprint(&tx).map_err(|_| Error::Internal)?)
            } else { None };
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
            if changes.is_empty() && before.is_none() {
                tx.rollback()?;
                continue;
            }
            if let Some(before) = before { push = file_delivery::prepare(&catalog.context, &tx, before, &project_guid, file_delivery::Intent { active: false, reactivate: false, scheduling: vec![], minimum_altitude: None }, &[]); }
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
                shoots: vec![],
                warnings,
                applied: false,
                push,
            });
            pending.push((*rig_id, connection, None, false));
        }
        // A layout short of its minimum overlap is the operator's call, not
        // a refusal: it is said first, read across the shared framing and
        // the own framing of every rig given panels, as one cover.
        let mut warnings = cover_warnings(&framing, &shooting);
        warnings.extend(coverage_warnings);
        if pending.iter().all(|(_, _, record, _)| record.is_none()) {
            warnings.push("No rig database can take this plan yet.".into());
        }
        // The digest covers what Apply will write, not the frame counts the
        // preview quotes (a frame saved between the two would refuse Apply
        // for nothing), and each rig's profile revision, since its field
        // sizes panels below what the text shows.
        let mut profile_revisions = Vec::new();
        for r in &reports {
            profile_revisions.push(store.rig_profile(r.rig.id)?.map(|p| p.revision));
        }
        let bytes = serde_json::to_vec(&(
            "activation-v2",
            &visits,
            id,
            framing.revision,
            plan.revision,
            reports
                .iter()
                .zip(&profile_revisions)
                .map(|(r, profile_revision)| {
                    (
                        r.rig.id,
                        &r.catalog_slug,
                        &r.profile_id,
                        r.changes
                            .iter()
                            .map(|c| (c.kind, c.action, &c.name, without_frame_counts(&c.detail)))
                            .collect::<Vec<_>>(),
                        &r.warnings,
                        r.push.as_ref().map(|p| (&p.peer_id, p.ts_file.as_ref().map(|file| (&file.path, &file.local_fingerprint, &file.destination_fingerprint)))),
                        profile_revision,
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
        // Apply holds every rig database's write lock from its first read
        // to its commit, so what it compares here is what it would write.
        // A different digest writes nothing and says what Apply found.
        if expected.as_ref().is_some_and(|value| value != &digest) {
            return Err(ActivationError::Changed(Box::new(Report {
                project,
                framing_revision: framing.revision,
                plan_revision: plan.revision,
                panels: panels.len(),
                rigs: reports,
                warnings,
                preview_digest: digest,
                applied: false,
                activation_revision: None,
            })));
        }
        let mut to_record = None;
        if applying {
            for (rig_id, _, _, _) in &pending {
                if let Some(report) = reports.iter_mut().find(|r| r.rig.id == *rig_id)
                    && let (Some(push), Some(slug)) = (&mut report.push, &report.catalog_slug) {
                    file_delivery::stage(&delivery_state, slug, push).map_err(|error| {
                        tracing::error!(?error, "Could not stage TS activation delivery");
                        Error::Internal
                    })?;
                    staged.track(slug, push);
                }
            }
            let previously_inactive = |rig: Uuid| previous.as_ref().is_some_and(|p| p.inactive_rigs.iter().any(|r| r.rig_id == rig));
            let record_of = |rigs: Vec<ActivatedRig>, inactive: Vec<InactiveRig>, carried: &[ActivatedRig]| {
                let mut all = rigs;
                for record in carried {
                    if !all.iter().any(|r| r.rig_id == record.rig_id) {
                        all.push(record.clone());
                    }
                }
                Activation {
                    project_id: id,
                    revision: 0,
                    framing_revision: framing.revision,
                    plan_revision: plan.revision,
                    coordinator_instance_id: service.instance_id,
                    applied_at_ms: now,
                    rigs: all,
                    inactive_rigs: inactive,
                    pushes: vec![],
                }
            };
            // The remote rigs this apply writes, and their peers. A
            // background apply sends nothing, so its rigs keep how their
            // last push went.
            let remote: BTreeMap<Uuid, (String, String, Option<psf_guard_director_meta::activation::TsSyncDelivery>)> = reports
                .iter()
                .filter_map(|r| r.push.as_ref().map(|p| (r.rig.id, (p.peer_id.clone(), p.peer_name.clone(), p.ts_sync.clone()))))
                .collect();
            let mut written: std::collections::BTreeSet<Uuid> = if guard.is_none() {
                pending.iter().map(|(rig, ..)| *rig).collect()
            } else {
                Default::default()
            };
            // The record is built and checked before any rig commits, so one
            // the store would refuse never leaves rows written but unrecorded.
            let mut whole = record_of(
                pending.iter().filter_map(|(_, _, record, _)| record.clone()).collect(),
                inactive.clone(),
                &carried,
            );
            whole.pushes = pushes_of(&whole, &written, &remote, previous.as_ref());
            if whole.validate().is_err() {
                return Err(ActivationError::NotReady(
                    "This activation is too large to record; nothing was written. Split the plan or turn some rigs off.",
                ));
            }
            // Each rig database commits on its own. One that fails is said
            // so and keeps what the last activation knew of it; the others
            // stand and are recorded.
            let mut rigs = Vec::new();
            let mut links = Vec::new();
            let mut failed: std::collections::BTreeSet<Uuid> = std::collections::BTreeSet::new();
            for (rig_id, connection, record, created_project) in pending {
                if let Err(error) = connection.execute_batch("COMMIT") {
                    tracing::error!(%error, rig = %rig_id, "Director activation commit failed");
                    let _ = connection.execute_batch("ROLLBACK");
                    if guard.is_some() {
                        return Err(error.into());
                    }
                    if let Some(report) = reports.iter_mut().find(|r| r.rig.id == rig_id) {
                        report.warnings.push(format!("Nothing was written to this database: its commit failed ({error})."));
                    }
                    failed.insert(rig_id);
                    carried.extend(previous_rig(rig_id));
                    continue;
                }
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
            if !failed.is_empty() {
                let names: Vec<&str> = reports.iter().filter(|r| failed.contains(&r.rig.id)).map(|r| r.catalog_name.as_str()).collect();
                warnings.push(format!("Not applied to {}; the other rigs were. Activate the plan again to finish.", names.join(", ")));
            }
            let inactive: Vec<InactiveRig> = inactive
                .into_iter()
                .filter(|r| !failed.contains(&r.rig_id) || previously_inactive(r.rig_id))
                .collect();
            written.retain(|rig| !failed.contains(rig));
            let mut record = record_of(rigs, inactive, &carried);
            record.pushes = pushes_of(&record, &written, &remote, previous.as_ref());
            to_record = Some((record, links));
        }
        Ok::<_, ActivationError>((Report {
            project,
            framing_revision: framing.revision,
            plan_revision: plan.revision,
            panels: panels.len(),
            rigs: reports,
            warnings,
            preview_digest: digest,
            applied: applying,
            activation_revision: None,
        }, to_record, staged))
    })
    .await?;
    if let Some((record, links)) = to_record {
        let approved: Vec<String> = record
            .pushes
            .iter()
            .filter_map(|push| push.ts_sync.as_ref()?.preview_id.clone())
            .collect();
        let names: Vec<(Uuid, String)> = report
            .rigs
            .iter()
            .map(|r| (r.rig.id, r.catalog_name.clone()))
            .collect();
        let (revision, link_warnings) = recorder
            .clone()
            .with_writer(move |store| {
                let recorded = store.record_activation(&record).map_err(|error| {
                    tracing::error!(error = ?error, "Applied activation could not be recorded");
                    if guard.is_some() {
                        return ActivationError::from(error);
                    }
                    ActivationError::NotReady(
                        "The rig databases were written but the activation could not be recorded; activate the plan again.",
                    )
                })?;
                let mut warnings = Vec::new();
                for link in links {
                    if let Err(error) = store.link_catalog_project(&link) {
                        tracing::warn!(error = ?error, "Activated project could not be linked in meta");
                        warnings.push(format!(
                            "The new project in {} was written but could not be linked under Project planning links; link it there by hand.",
                            names
                                .iter()
                                .find(|(rig, _)| *rig == link.rig_id)
                                .map(|(_, name)| name.as_str())
                                .unwrap_or("its database")
                        ));
                    }
                }
                Ok::<_, ActivationError>((recorded.revision, warnings))
            })
            .await?;
        report.activation_revision = Some(revision);
        report.warnings.extend(link_warnings);
        staged.recorded(&approved);
    }
    drop(staged);
    drop(sync_guard);
    if let (true, Some(revision)) = (report.applied, report.activation_revision) {
        // The local rows are committed and the record says each remote rig
        // waits; now each rig's peer, and the record says how it went. A
        // push that fails leaves the activation standing and says so in its
        // row.
        let mut answered = std::collections::BTreeSet::new();
        for rig in report.rigs.iter_mut().filter(|rig| rig.applied) {
            let (Some(push), Some(slug)) = (rig.push.as_mut(), rig.catalog_slug.as_deref()) else {
                continue;
            };
            if let Some(delivery) = push.ts_sync.clone() {
                if let Some(context) = state.get_database(slug) {
                    *push = file_delivery::send(
                        &state,
                        &recorder,
                        &context,
                        remote_push::Waiting {
                            project: id,
                            revision,
                            rig: rig.rig.id,
                        },
                        delivery,
                    )
                    .await;
                }
                continue;
            }
            let (Some(peer), Some(context)) = (
                peers.iter().find(|peer| peer.id == push.peer_id),
                state.get_database(slug),
            ) else {
                continue;
            };
            let waiting = remote_push::Waiting {
                project: id,
                revision,
                rig: rig.rig.id,
            };
            *push =
                remote_push::push_catalog(&state, &recorder, &context, peer, vec![waiting]).await;
            if push.applied {
                answered.insert(peer.id.clone());
            }
        }
        // The peers answered: anything else waiting for them goes too.
        for peer in answered {
            remote_push::peer_reached(&state, &peer);
        }
    }
    Ok(Json(ApiResponse::success(report)))
}

/// Open a rig database for an activation, read-write (a preview makes the
/// same writes and rolls them back), with the server's shared lock wait:
/// N.I.N.A. and a directory refresh can hold the file for tens of seconds.
/// A failure is the rig's, said in words, not the request's.
pub(super) fn open_rig(path: &str) -> Result<Connection, String> {
    let connection = super::super::database_context::open_scheduler_connection_with_flags(
        FilePath::new(path),
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| format!("Its database could not be opened for writing: {error}."))?;
    super::super::database_context::configure_scheduler_busy_timeout(&connection)
        .map_err(|error| format!("Its database could not be opened for writing: {error}."))?;
    Ok(connection)
}

/// A rig database for an activation, or for a check or a preview a copy of
/// its planning tables. A preview first opens the file for writing, as
/// Apply will, without taking its lock: a database Apply could not open is
/// said in the preview, so the two agree and Activate is not refused for it.
fn open_rig_for(path: &str, on_copy: bool, probe_write: bool) -> Result<Connection, String> {
    if !on_copy {
        return open_rig(path);
    }
    if probe_write {
        let connection = open_rig(path)?;
        // SQLite opens a write-protected file read-only without a word; Apply
        // then fails at its first write, and says so in these words.
        if connection.is_readonly(rusqlite::MAIN_DB).unwrap_or(false) {
            let refused = rusqlite::Error::SqliteFailure(
                rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_READONLY),
                Some("attempt to write a readonly database".into()),
            );
            let RigError::Skip(reason) = RigError::from(refused) else {
                unreachable!("a SQLite error always skips the rig");
            };
            return Err(reason);
        }
    }
    planning_copy(path)
}

/// Each remote rig this apply writes waits for its push, which runs once
/// the record is kept; a rig it does not write keeps how its last push
/// went. A rig written here with no peer has nothing to wait for.
fn pushes_of(
    record: &Activation,
    written: &std::collections::BTreeSet<Uuid>,
    remote: &BTreeMap<
        Uuid,
        (
            String,
            String,
            Option<psf_guard_director_meta::activation::TsSyncDelivery>,
        ),
    >,
    previous: Option<&Activation>,
) -> Vec<RemotePush> {
    let mut seen = std::collections::BTreeSet::new();
    record
        .rigs
        .iter()
        .map(|r| r.rig_id)
        .chain(record.inactive_rigs.iter().map(|r| r.rig_id))
        .filter(|rig| seen.insert(*rig))
        .filter_map(|rig| {
            if written.contains(&rig) {
                remote
                    .get(&rig)
                    .map(|(peer_id, peer_name, ts_sync)| RemotePush {
                        rig_id: rig,
                        ts_sync: ts_sync.clone(),
                        peer_id: peer_id.clone(),
                        peer_name: peer_name
                            .chars()
                            .take(psf_guard_director_meta::activation::MAX_PEER_NAME_CHARS)
                            .collect(),
                        applied: false,
                        attempted_at_ms: None,
                        error: None,
                    })
            } else {
                previous
                    .and_then(|p| p.pushes.iter().find(|push| push.rig_id == rig))
                    .cloned()
            }
        })
        .collect()
}

/// Every table an activation reads or writes in a rig database.
const PLANNING_TABLES: [&str; 10] = [
    "project",
    "target",
    "exposuretemplate",
    "exposureplan",
    "ruleweight",
    "profilepreference",
    "psf_guard_director_project",
    "psf_guard_director_target",
    "psf_guard_director_plan",
    collaboration_activation::TABLE,
];

/// A rig database's planning tables, schema and rows, copied into memory in
/// one read: an activation run on the copy shows what it would change and
/// never takes the file's write lock. These tables are small; frames and
/// their metadata stay behind.
pub(super) fn planning_copy(path: &str) -> Result<Connection, String> {
    let read_failed = |error: rusqlite::Error| format!("Its database could not be read: {error}.");
    let source = super::super::database_context::open_scheduler_connection_with_flags(
        FilePath::new(path),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(read_failed)?;
    let read = source.unchecked_transaction().map_err(read_failed)?;
    planning_copy_from(&read)
}

pub(super) fn planning_copy_from(read: &Connection) -> Result<Connection, String> {
    let read_failed = |error: rusqlite::Error| format!("Its database could not be read: {error}.");
    let copy = Connection::open_in_memory().map_err(read_failed)?;
    for table in PLANNING_TABLES {
        let schema: Vec<(String, String)> = read
            .prepare(
                "SELECT tbl_name, sql FROM sqlite_master
                 WHERE lower(tbl_name)=?1 AND type IN ('table', 'index') AND sql IS NOT NULL
                 ORDER BY type='index'",
            )
            .and_then(|mut statement| {
                statement
                    .query_map([table], |row| Ok((row.get(0)?, row.get(1)?)))?
                    .collect()
            })
            .map_err(read_failed)?;
        let Some((name, _)) = schema.first() else {
            continue;
        };
        for (_, sql) in &schema {
            copy.execute_batch(sql).map_err(read_failed)?;
        }
        let mut rows = read
            .prepare(&format!("SELECT * FROM \"{name}\""))
            .map_err(read_failed)?;
        let columns = rows.column_count();
        let mut insert = copy
            .prepare(&format!(
                "INSERT INTO \"{name}\" VALUES ({})",
                vec!["?"; columns].join(",")
            ))
            .map_err(read_failed)?;
        let mut cursor = rows.query([]).map_err(read_failed)?;
        while let Some(row) = cursor.next().map_err(read_failed)? {
            let values = (0..columns)
                .map(|i| row.get::<_, rusqlite::types::Value>(i))
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(read_failed)?;
            insert
                .execute(rusqlite::params_from_iter(values))
                .map_err(read_failed)?;
        }
    }
    Ok(copy)
}

/// Take the rig database's write lock for an activation.
pub(super) fn begin_rig(connection: &mut Connection) -> Result<rusqlite::Transaction<'_>, String> {
    connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| match error.sqlite_error_code() {
            Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked) => {
                "Its database stayed locked (N.I.N.A. or a refresh may be writing it); try again in a moment.".to_string()
            }
            _ => format!("Its database could not be locked for writing: {error}."),
        })
}

/// The Sync peer a remote rig's rows go to after Apply, if its profile
/// names one that is still registered.
pub(super) fn planned_push(
    profile: Option<&psf_guard_director_meta::profile::RigProfile>,
    known_peers: &[PeerEntry],
    warnings: &mut Vec<String>,
) -> Option<Push> {
    let peer_id = profile.and_then(|p| p.peer_id.as_deref())?;
    match known_peers.iter().find(|peer| peer.id == peer_id) {
        Some(peer) => Some(Push::planned(peer)),
        None => {
            warnings.push(format!(
                "Its peer '{peer_id}' is no longer registered; the plan stays on this server until Setup picks another."
            ));
            None
        }
    }
}

/// Send a rig database's planning rows to the peer that holds the rig's real
/// database: a Sync planning push, reviewed and applied on the peer. Captures
/// and grades never travel this way. The caller holds the server's Sync
/// apply lock; [`remote_push::push_catalog`] takes it and records the result.
pub(super) async fn send_planning(context: &DatabaseContext, peer: &PeerEntry) -> Push {
    let mut push = Push::planned(peer);
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
        stop: crate::concurrency::Stop::new(),
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

/// Push the last activation's rows again to each remote rig's peer that has
/// not had them, for a peer that was unreachable at Apply or has since been
/// repaired; to every remote rig's peer when each has them already, or the
/// record predates keeping pushes.
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
    let registered: std::collections::BTreeSet<String> =
        peers.iter().map(|peer| peer.id.clone()).collect();
    let instance = service.instance_id;
    let (project, revision, targets, mut warnings) = service
        .clone()
        .with_reader(move |store| {
            let project = store.project(id)?.ok_or(Error::Missing)?;
            let activation = store
                .activation(id)?
                .ok_or(ActivationError::NotReady("Activate the plan first."))?;
            let mut targets: Vec<(
                NamedIdentity,
                Arc<DatabaseContext>,
                String,
                Option<psf_guard_director_meta::activation::TsSyncDelivery>,
            )> = Vec::new();
            let found = identified_catalogs(&catalogs, instance);
            let mut warnings = found.duplicates.clone();
            // The rigs whose site still lacks this activation, as the page
            // shows them.
            let mut unsent = std::collections::BTreeSet::new();
            for push in activation.pushes.iter().filter(|p| !p.applied) {
                if let Some(delivery) = &push.ts_sync {
                    if let Some(context) = remote_push::catalog_of(&activation, push.rig_id)
                        .and_then(|id| found.get(id))
                        && file_delivery::is_current(context, delivery)
                    {
                        unsent.insert(push.rig_id);
                    }
                    continue;
                }
                if registered.contains(&push.peer_id)
                    && remote_push::sends_to(store, push.rig_id, &push.peer_id)?
                {
                    unsent.insert(push.rig_id);
                }
            }
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
                )
                .filter(|(rig, _)| unsent.is_empty() || unsent.contains(rig));
            for (rig_id, catalog_id) in reached {
                let rig = store.rig(rig_id)?.ok_or(Error::Missing)?;
                if let Some(context) = found.get(catalog_id)
                    && let Some(settings) = &context.ts_sync
                {
                    let delivery = activation
                        .pushes
                        .iter()
                        .find(|p| p.rig_id == rig_id)
                        .and_then(|p| p.ts_sync.clone())
                        .unwrap_or(psf_guard_director_meta::activation::TsSyncDelivery {
                            source_path: settings.source_path.clone(),
                            preview_id: None,
                        });
                    targets.push((
                        rig,
                        context.clone(),
                        format!("ts:{}", context.id),
                        Some(delivery),
                    ));
                    continue;
                }
                let Some(peer_id) = store
                    .rig_profile(rig_id)?
                    .and_then(|profile| profile.peer_id)
                else {
                    continue;
                };
                match found.get(catalog_id) {
                    Some(context) => targets.push((rig, context.clone(), peer_id, None)),
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
    let mut answered = std::collections::BTreeSet::new();
    for (rig, context, peer_id, delivery) in targets {
        if let Some(delivery) = delivery {
            let push = file_delivery::send(
                &state,
                &service,
                &context,
                remote_push::Waiting {
                    project: id,
                    revision,
                    rig: rig.id,
                },
                delivery,
            )
            .await;
            rigs.push(PushedRig {
                rig,
                catalog_name: context.name.clone(),
                push,
            });
            continue;
        }
        let push = match peers.iter().find(|peer| peer.id == peer_id) {
            Some(peer) => {
                let waiting = remote_push::Waiting {
                    project: id,
                    revision,
                    rig: rig.id,
                };
                remote_push::push_catalog(&state, &service, &context, peer, vec![waiting]).await
            }
            None => Push {
                peer_id: peer_id.clone(),
                peer_name: peer_id.clone(),
                applied: false,
                summary: BTreeMap::new(),
                error: Some(
                    "This peer is no longer registered; name another under Rigs, Setup.".into(),
                ),
                ts_file: None,
                ts_sync: None,
            },
        };
        if push.applied {
            answered.insert(push.peer_id.clone());
        }
        rigs.push(PushedRig {
            rig,
            catalog_name: context.name.clone(),
            push,
        });
    }
    // The peers answered: anything else waiting for them goes too.
    for peer in answered {
        remote_push::peer_reached(&state, &peer);
    }
    if rigs.is_empty() && warnings.is_empty() {
        warnings.push(
            "No rig in this activation has a TS file connection or remote site. Configure sync before sending planning."
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
    /// This rig's panels, from its own framing or the shared one, each with
    /// the camera angle to shoot it at.
    panels: &'a [Panel],
    /// The framing's angle, which each panel's own angle is told against.
    /// Every rig gets each panel's own angle, rotator or not: planning sets
    /// the rotation, and reaching it belongs to execution.
    position_angle_degrees: f64,
    contributions: &'a [&'a Contribution],
    /// The last activation set this rig's project Inactive when the rig was
    /// turned off; now that it is on, the project goes Active again.
    reactivate: bool,
    rig_id: Uuid,
    catalog: CatalogIdentity,
    existing_link: Option<Uuid>,
    /// Projects in this database linked to other plans; never this plan's,
    /// whatever the rig database's side table still says.
    foreign: &'a std::collections::BTreeSet<String>,
    instance: Uuid,
    now: u64,
    /// Target Scheduler scheduling limits for this rig and project.
    scheduling: &'a ResolvedScheduling,
    collaboration: Option<&'a collaboration_activation::Admitted>,
}

struct Outcome {
    changes: Vec<Change>,
    shoots: Vec<Shot>,
    record: ActivatedRig,
    created_project: bool,
}

/// A rig that cannot take the plan carries the reason the operator sees;
/// anything else is a real failure.
pub(super) enum RigError {
    Skip(String),
    Failed(Error),
}
// Every SQLite error inside a rig's write comes from that rig's database
// (read-only, full, corrupt, locked): the rig is skipped and says why, and
// the other rigs go ahead.
impl From<rusqlite::Error> for RigError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Skip(format!(
            "Its database could not be read or written: {error}."
        ))
    }
}
impl From<Error> for RigError {
    fn from(error: Error) -> Self {
        Self::Failed(error)
    }
}

pub(super) fn has_column(conn: &Connection, table: &str, column: &str) -> rusqlite::Result<bool> {
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
        .optional()?
        .filter(|guid| !inputs.foreign.contains(guid));
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
                let was_mosaic: i64 = tx.query_row(
                    "SELECT IFNULL(isMosaic, 0) FROM project WHERE guid=?1",
                    [&guid],
                    |row| row.get(0),
                )?;
                // One rule on both paths: a Draft goes Active, an Inactive
                // one only when Director set it so; the mosaic flag follows
                // the panels. Any change is reported as one.
                let mut notes = Vec::new();
                if state == 0 {
                    notes.push("Draft → Active");
                }
                if back_on {
                    notes.push("Active again: this rig is on in the plan");
                }
                if (was_mosaic != 0) != mosaic {
                    notes.push(if mosaic {
                        "now a mosaic"
                    } else {
                        "no longer a mosaic"
                    });
                }
                if !notes.is_empty() {
                    tx.execute(
                        "UPDATE project SET isMosaic=?2, state=CASE WHEN state=0 OR (?3 AND state=2) THEN 1 ELSE state END WHERE guid=?1",
                        params![guid, i32::from(mosaic), back_on],
                    )?;
                }
                changes.push(Change {
                    kind: "project",
                    action: if notes.is_empty() {
                        "unchanged"
                    } else {
                        "update"
                    },
                    name: project_name.clone(),
                    detail: if notes.is_empty() {
                        "Director's project from the last activation".into()
                    } else {
                        notes.join("; ")
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
                // Director takes over its planning rows either way; the row
                // itself changes only for a Draft or a new mosaic, so one
                // that already matches is reported unchanged.
                let (state, was_mosaic): (i64, i64) = tx.query_row(
                    "SELECT IFNULL(state, 0), IFNULL(isMosaic, 0) FROM project WHERE guid=?1",
                    [&guid],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )?;
                let mut notes = Vec::new();
                if state == 0 {
                    notes.push("Draft → Active");
                }
                if mosaic && was_mosaic == 0 {
                    notes.push("now a mosaic");
                }
                tx.execute(
                    "UPDATE project SET isMosaic=CASE WHEN ?2=1 THEN 1 ELSE isMosaic END, state=CASE WHEN state=0 THEN 1 ELSE state END WHERE guid=?1",
                    params![guid, i32::from(mosaic)],
                )?;
                changes.push(Change {
                    kind: "project",
                    action: if notes.is_empty() { "unchanged" } else { "update" },
                    name: project_row(tx, &guid)?.map(|(_, name)| name).unwrap_or_default(),
                    detail: if notes.is_empty() {
                        "already in Target Scheduler, linked to this plan; Director takes over its planning rows".into()
                    } else {
                        format!(
                            "linked to this plan; Director takes over its planning rows; {}",
                            notes.join("; ")
                        )
                    },
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
    let mut scheduling = inputs.scheduling.clone();
    if let Some(minimum) = inputs
        .collaboration
        .and_then(|a| a.import.plan.share().requirements.as_ref())
        .and_then(|r| r.min_altitude_degrees)
    {
        scheduling.values.minimum_altitude_degrees =
            scheduling.values.minimum_altitude_degrees.max(minimum);
    }
    write_scheduling(
        tx,
        &project_guid,
        &scheduling,
        fresh_row,
        &project_name,
        &mut changes,
    )?;
    tx.execute(
        "INSERT INTO psf_guard_director_project(project_guid,global_project_id,coordinator_instance_id,activation_revision,applied_at_ms)
         VALUES(?1,?2,?3,?4,?5)
         ON CONFLICT(project_guid) DO UPDATE SET global_project_id=excluded.global_project_id, coordinator_instance_id=excluded.coordinator_instance_id, activation_revision=excluded.activation_revision, applied_at_ms=excluded.applied_at_ms",
        params![
            project_guid,
            inputs.project.id.to_string(),
            inputs.instance.to_string(),
            inputs.plan.revision as i64,
            inputs.now as i64
        ],
    )?;
    let (project_row_id, _) = project_row(tx, &project_guid)?.ok_or(Error::Internal)?;
    if let Some(assignment) = inputs.collaboration {
        let new_assignment = assignment.prepare(tx, &project_guid, inputs.now)?;
        let minimum = assignment
            .import
            .plan
            .share()
            .requirements
            .as_ref()
            .and_then(|r| r.min_altitude_degrees)
            .unwrap_or(0.0);
        let mut limits_changed = false;
        if minimum > 0.0 {
            if !has_column(tx, "project", "minimumaltitude")? {
                return Err(RigError::Skip(
                    "This Target Scheduler schema cannot hold collaboration limits.".into(),
                ));
            }
            limits_changed = tx.execute(
            "UPDATE project SET minimumaltitude=?2 WHERE guid=?1 AND COALESCE(minimumaltitude,0)<?2",
            params![project_guid, minimum],
            )? > 0;
        }
        changes.push(Change { kind:"project",action:if new_assignment || limits_changed {"update"} else {"unchanged"},name:project_name.clone(),detail:format!("Collaboration {} assigned {}, revision {}; ordinary TS scheduling, no nightly deadline",
            assignment.import.plan.share().task_id,assignment.import.plan.night(),assignment.import.plan.share().version) });
    }
    // Rows added to an existing project go under that project's N.I.N.A.
    // profile, not the file's most common one.
    let profile_id: String = tx
        .query_row(
            "SELECT IFNULL(profileId, '') FROM project WHERE Id=?1",
            [project_row_id],
            |row| row.get::<_, String>(0),
        )
        .ok()
        .filter(|profile| !profile.is_empty())
        .unwrap_or(profile_id);

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
    // The panel each of the project's targets stands for, named as the
    // framing taken in from them names its panels (a lone target is `r1c1`,
    // targets on a grid are its cells): a panel drawn where its frames were
    // shot sits a little off its target's coordinates, so its target is
    // found by the panel it stands for, not by place.
    let stands_for: BTreeMap<String, i64> = match Uuid::parse_str(&project_guid) {
        Ok(project) if inputs.collaboration.is_none() => {
            let found = super::mosaic::project_targets(tx, project)?;
            super::mosaic::linked_panel_ids(tx, project, &found)?
                .into_iter()
                .zip(found)
                .map(|(panel_id, (row, _, _))| (panel_id, row))
                .collect()
        }
        _ => BTreeMap::new(),
    };
    let mut targets = Vec::new();
    for panel in inputs
        .panels
        .iter()
        .filter(|p| owned_panels.contains(p.footprint.id.as_str()))
    {
        let name = if let Some(a) = inputs.collaboration {
            let index = a.panel_index(&panel.footprint.id).ok_or(Error::Invalid)?;
            format!("{target_base} panel {index}")
        } else if mosaic {
            format!("{target_base} {}", panel.footprint.id)
        } else {
            target_base.clone()
        };
        let ra_hours = panel.footprint.center.ra_degrees / 15.0;
        let dec = panel.footprint.center.dec_degrees;
        // Each panel's own angle: the framing's plus its turn, plus north's
        // lean at its center, so its up runs where the framing drew it; as
        // shot for a panel measured from its frames.
        let rotation = match inputs.collaboration {
            Some(a) => a
                .rotation(&panel.footprint.id)
                .unwrap_or(inputs.position_angle_degrees),
            None => panel.position_angle_degrees,
        };
        let detail = format!(
            "{} {}, {}",
            format_ra(ra_hours),
            format_dec(dec),
            describe_angle(
                rotation,
                (inputs.collaboration.is_none()).then_some(panel),
                inputs.position_angle_degrees,
            )
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
                // RA, Dec, angle (any may be NULL), name, epoch code, frames taken.
                type Placed = (Option<f64>, Option<f64>, Option<f64>, String, i64, i64);
                let existing: Option<Placed> = tx
                    .query_row(
                        "SELECT t.ra, t.dec, t.rotation, IFNULL(t.name, ''), IFNULL(t.epochcode, 2),
                                (SELECT IFNULL(SUM(acquired), 0) FROM exposureplan WHERE targetid = t.Id)
                         FROM target t WHERE t.guid=?1",
                        [&guid],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
                    )
                    .optional()?;
                match existing {
                    Some((ra0, dec0, rot0, name0, epoch0, frames)) => {
                        let (ra0, dec0, rot0) = (
                            ra0.unwrap_or(f64::NAN),
                            dec0.unwrap_or(f64::NAN),
                            rot0.unwrap_or(f64::NAN),
                        );
                        // A locked panel with frames keeps its target as it
                        // was shot: the place and angle the frames were
                        // taken at, whatever the plan draws now.
                        let kept = panel.locked
                            && frames > 0
                            && epoch0 == 2
                            && ra0.is_finite()
                            && dec0.is_finite()
                            && rot0.is_finite();
                        // Frames shot before panels took north's lean were
                        // shot at the framing's angle; the target keeps it.
                        // A new target, made when the panel moved far, has
                        // no frames and takes the panel's own angle.
                        let unleaned = frames > 0
                            && inputs.collaboration.is_none()
                            && planned_angle(rot0, inputs.position_angle_degrees, panel);
                        let in_place = if unleaned { rot0 } else { rotation };
                        let same = kept
                            || ((ra0 - ra_hours).abs() < 1e-7
                                && (dec0 - dec).abs() < 1e-6
                                && angle_apart(rot0, in_place) < 1e-3
                                && epoch0 == 2);
                        let in_place_detail = if kept {
                            format!(
                                "{} {}, angle {rot0:.1}°, kept where its frames were shot",
                                format_ra(ra0),
                                format_dec(dec0)
                            )
                        } else if unleaned {
                            format!(
                                "{} {}, angle {rot0:.1}° as its frames were shot",
                                format_ra(ra_hours),
                                format_dec(dec)
                            )
                        } else {
                            detail.clone()
                        };
                        // A panel whose place moved by more than a quarter of
                        // its size (a grid grown or shrunk, a new center)
                        // is other sky. Moving a target that has frames
                        // would credit them to the new place, so the new
                        // place gets a new target and the old one keeps its
                        // frames; its plans are turned off below.
                        let moved = separation_degrees(ra0 * 15.0, dec0, ra_hours * 15.0, dec);
                        let far = !moved.is_finite() || moved > 0.25 * panel_side_degrees(panel);
                        // So is a shot panel unlocked by hand and turned:
                        // turning its target would mix two camera angles
                        // under one target.
                        let turn = angle_apart(rot0, in_place);
                        let turned = panel.unlocked_by_hand
                            && (!turn.is_finite() || turn > MAX_TURN_IN_PLACE_DEGREES);
                        if !same && (far || turned) && frames > 0 {
                            let fresh = new_guid();
                            insert_target(
                                tx,
                                &fresh,
                                &name,
                                ra_hours,
                                dec,
                                rotation,
                                project_row_id,
                            )?;
                            tx.execute(
                                "UPDATE psf_guard_director_target SET panel_id = panel_id || '~' || ?2 WHERE target_guid=?1",
                                params![guid, inputs.framing.revision as i64],
                            )?;
                            changes.push(Change {
                                kind: "target",
                                action: "create",
                                name: name.clone(),
                                detail: if far {
                                    format!(
                                        "{detail}; the panel moved {} from its old target, which keeps its {frames} frames and stops being taken",
                                        format_distance(moved)
                                    )
                                } else {
                                    format!(
                                        "{detail}; the panel turned {turn:.1}° from its old target, which keeps its {frames} frames and stops being taken"
                                    )
                                },
                            });
                            fresh
                        } else {
                            if !same {
                                tx.execute(
                                    "UPDATE target SET ra=?2, dec=?3, rotation=?4, epochcode=2, projectid=?5 WHERE guid=?1",
                                    params![guid, ra_hours, dec, in_place, project_row_id],
                                )?;
                            }
                            changes.push(Change {
                                kind: "target",
                                action: if same { "unchanged" } else { "update" },
                                name: name0.clone(),
                                detail: in_place_detail,
                            });
                            shown = name0;
                            guid
                        }
                    }
                    None => {
                        insert_target(tx, &guid, &name, ra_hours, dec, rotation, project_row_id)?;
                        changes.push(Change {
                            kind: "target",
                            action: "create",
                            name: name.clone(),
                            detail,
                        });
                        guid
                    }
                }
            }
            // Nothing owned yet: a target already in this project (imported
            // from Target Scheduler, or made by hand) is taken over rather
            // than doubled.
            None => {
                let found = if inputs.collaboration.is_some() {
                    None
                } else {
                    adoptable_target(
                        tx,
                        project_row_id,
                        &name,
                        stands_for.get(&panel.footprint.id).copied(),
                        PanelPlace {
                            ra_hours,
                            dec,
                            side_degrees: panel_side_degrees(panel),
                        },
                        !mosaic,
                    )?
                };
                // A target with frames sits where they were shot. Taking it
                // over from further than a quarter of the panel would move
                // the frames with it, so it is left as it is and the panel
                // gets a new target, as an owned target that moved far does.
                let left_alone = found.as_ref().and_then(|found| {
                    let kept = panel.locked
                        && found.frames > 0
                        && found.epochcode == 2
                        && found.ra_hours.is_finite()
                        && found.dec.is_finite()
                        && found.rotation.is_finite();
                    let moved =
                        separation_degrees(found.ra_hours * 15.0, found.dec, ra_hours * 15.0, dec);
                    let far = !moved.is_finite() || moved > 0.25 * panel_side_degrees(panel);
                    // A shot panel unlocked by hand and turned leaves it
                    // alone too, unless its frames were shot at the
                    // framing's angle. Any other takeover sets the angle,
                    // as a hand-made target's is often just 0.
                    let unleaned =
                        planned_angle(found.rotation, inputs.position_angle_degrees, panel);
                    let turn = if unleaned {
                        0.0
                    } else {
                        angle_apart(found.rotation, rotation)
                    };
                    let turned = panel.unlocked_by_hand
                        && (!turn.is_finite() || turn > MAX_TURN_IN_PLACE_DEGREES);
                    (!kept && found.frames > 0 && (far || turned))
                        .then(|| (found.name.clone(), found.frames, moved, far, turn))
                });
                let found = if left_alone.is_some() { None } else { found };
                match found {
                    Some(found) => {
                        // Director writes J2000 coordinates and plans the target,
                        // so a JNOW or switched-off target is put right too; the
                        // program only serves J2000 targets that are on.
                        // A locked panel with frames keeps the target where
                        // they were shot.
                        let kept = panel.locked
                            && found.frames > 0
                            && found.epochcode == 2
                            && found.ra_hours.is_finite()
                            && found.dec.is_finite()
                            && found.rotation.is_finite();
                        // Frames shot at the framing's angle keep it.
                        let unleaned = found.frames > 0
                            && planned_angle(found.rotation, inputs.position_angle_degrees, panel);
                        let (ra_hours, dec, rotation) = if kept {
                            (found.ra_hours, found.dec, found.rotation)
                        } else if unleaned {
                            (ra_hours, dec, found.rotation)
                        } else {
                            (ra_hours, dec, rotation)
                        };
                        let detail = if kept {
                            format!(
                                "{} {}, angle {rotation:.1}°, kept where its frames were shot",
                                format_ra(ra_hours),
                                format_dec(dec)
                            )
                        } else if unleaned {
                            format!(
                                "{} {}, angle {rotation:.1}° as its frames were shot",
                                format_ra(ra_hours),
                                format_dec(dec)
                            )
                        } else {
                            detail.clone()
                        };
                        let placed = (found.ra_hours - ra_hours).abs() < 1e-7
                            && (found.dec - dec).abs() < 1e-6
                            && angle_apart(found.rotation, rotation) < 1e-3;
                        let same = placed && found.epochcode == 2 && found.active;
                        if !same {
                            tx.execute(
                            "UPDATE target SET ra=?2, dec=?3, rotation=?4, epochcode=2, active=1 WHERE guid=?1",
                            params![found.guid, ra_hours, dec, rotation],
                        )?;
                        }
                        let mut notes = Vec::new();
                        if found.epochcode != 2 {
                            notes.push("now J2000");
                        }
                        if !found.active {
                            notes.push("turned on");
                        }
                        let notes = if notes.is_empty() {
                            String::new()
                        } else {
                            format!("; {}", notes.join(", "))
                        };
                        changes.push(Change {
                            kind: "target",
                            action: if same { "unchanged" } else { "adopt" },
                            name: found.name.clone(),
                            detail: if same {
                                format!("already in Target Scheduler at {detail}")
                            } else if placed {
                                format!("takes over the existing target at {detail}{notes}")
                            } else {
                                format!(
                                    "takes over the existing target, moving it to {detail}{notes}"
                                )
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
                        detail: match &left_alone {
                            Some((other, frames, moved, true, _)) => format!(
                                "{detail}; the existing target {other} sits {} away with its {frames} frames, so it is left as it is",
                                format_distance(*moved)
                            ),
                            Some((other, frames, _, false, turn)) => format!(
                                "{detail}; the existing target {other} is turned {turn:.1}° from this panel with its {frames} frames, so it is left as it is"
                            ),
                            None => detail,
                        },
                    });
                        guid
                    }
                }
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
        targets.push((panel.footprint.id.clone(), target_guid, shown));
    }

    // Exposure plans: one per target, template settings and exposure length.
    // Contributions that come to the same plan (two objectives on one filter
    // at one length) share it, and it asks for the sum of their frames; each
    // is recorded against it. Any other difference makes a plan of its own.
    struct Part<'c> {
        contribution: &'c Contribution,
        objective: &'c Objective,
        frames: u32,
    }
    struct Group<'c> {
        target_guid: String,
        target_name: String,
        panel_id: String,
        template: ResolvedTemplate,
        exposure_seconds: f64,
        /// What makes two rows one plan; a collaboration's rows are its own.
        key: (String, Option<TemplateSettings>, i64, Option<Uuid>),
        parts: Vec<Part<'c>>,
    }
    let mut groups: Vec<Group> = Vec::new();
    let mut reported_templates = std::collections::BTreeSet::new();
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
        if template.created && reported_templates.insert(template.id) {
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
        let settings = template_settings(tx, template.id)?;
        for (panel_id, target_guid, target_name) in targets.iter().filter(|(panel_id, _, _)| {
            contribution.panel_ids.is_empty() || contribution.panel_ids.contains(panel_id)
        }) {
            let frames = match inputs.collaboration {
                Some(a) => a
                    .frames(panel_id, &contribution.template.filter_name)
                    .map(|(_, frames)| frames)
                    .ok_or(Error::Invalid)?,
                None => frames,
            };
            let key = (
                target_guid.clone(),
                settings
                    .clone()
                    .or(Some((format!("#{}", template.id), 0, 0, 0, 0))),
                (contribution.exposure_seconds * 1000.0).round() as i64,
                inputs.collaboration.map(|_| contribution.id),
            );
            let part = Part {
                contribution,
                objective,
                frames,
            };
            match groups.iter_mut().find(|group| group.key == key) {
                Some(group) => group.parts.push(part),
                None => groups.push(Group {
                    target_guid: target_guid.clone(),
                    target_name: target_name.clone(),
                    panel_id: panel_id.clone(),
                    template: template.clone(),
                    exposure_seconds: contribution.exposure_seconds,
                    key,
                    parts: vec![part],
                }),
            }
        }
    }

    let mut plans = Vec::new();
    let mut shoots = Vec::new();
    // Plans taken over in this activation, so two groups never take the
    // same one.
    let mut claimed = std::collections::BTreeSet::new();
    // Every plan this activation writes; any other plan on the project's
    // targets is turned off below.
    let mut written: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    // Contributions the plan still holds, on or off. A plan owned by any
    // other (a rig dropped and added back, an objective made anew) is free
    // to be taken over again, frames and all, instead of doubled.
    let held: std::collections::BTreeSet<String> = inputs
        .plan
        .contributions
        .iter()
        .map(|c| c.id.to_string())
        .collect();
    // What each target's plans take, by template settings, so a plan left
    // on that takes the same frames at another length says what replaced it.
    let mut taken: std::collections::BTreeMap<String, Vec<(TemplateSettings, String)>> =
        std::collections::BTreeMap::new();
    for group in &groups {
        let template = &group.template;
        let template_id = template.id;
        let target_guid = &group.target_guid;
        let exposure_seconds = group.exposure_seconds;
        let first = &group.parts[0];
        let target_row: i64 = tx.query_row(
            "SELECT Id FROM target WHERE guid=?1",
            [target_guid],
            |row| row.get(0),
        )?;
        let name = format!(
            "{} · {} · {} s",
            group.target_name, template.name, exposure_seconds
        );
        // The plan an earlier activation wrote for any of the group's
        // contributions: one still on first, then the oldest row, so a
        // group that used to be separate plans settles on one of them.
        let mut owned: Option<(String, &Part)> = None;
        let mut best: Option<(bool, i64)> = None;
        for part in &group.parts {
            let found: Option<(String, Option<i64>, Option<i64>)> = tx
                .query_row(
                    "SELECT d.exposureplan_guid, e.Id, COALESCE(e.enabled, 1)
                     FROM psf_guard_director_plan d LEFT JOIN exposureplan e ON e.guid = d.exposureplan_guid
                     WHERE d.target_guid=?1 AND d.contribution_id=?2",
                    params![target_guid, part.contribution.id.to_string()],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()?;
            let Some((guid, row_id, enabled)) = found else {
                continue;
            };
            let rank = (enabled != Some(1), row_id.unwrap_or(i64::MAX));
            if best.is_none_or(|best| rank < best) {
                best = Some(rank);
                owned = Some((guid, part));
            }
        }
        let owner = owned.as_ref().map_or(first, |(_, part)| *part);
        let owned = match (owned.map(|(guid, _)| guid), inputs.collaboration) {
            (None, Some(a)) => {
                a.plan_for(tx, target_guid, &first.contribution.template.filter_name)?
            }
            (owned, _) => owned,
        };
        let frames = group
            .parts
            .iter()
            .fold(0u32, |sum, part| sum.saturating_add(part.frames));
        let frames = match inputs.collaboration {
            Some(a) => a.frame_goal(tx, owned.as_deref(), frames)?,
            None => frames,
        };
        let detail = format!(
            "{frames} frames, template #{} {}{}",
            template.id,
            template.name,
            if group.parts.len() > 1 {
                format!(", summed from {} objectives", group.parts.len())
            } else {
                String::new()
            }
        );
        let mut turned_on = false;
        let plan_guid = match owned {
            Some(guid) => {
                // The length the row runs at: its own, or its template's
                // default when it is left at -1.
                type Existing = (Option<f64>, i64, i64, i64, Option<String>);
                let existing: Option<Existing> = tx
                    .query_row(
                        "SELECT CASE WHEN e.exposure IS NULL OR e.exposure < 0 THEN t.defaultexposure ELSE e.exposure END,
                                e.desired, e.exposureTemplateId, COALESCE(e.enabled,1), t.name
                         FROM exposureplan e LEFT JOIN exposuretemplate t ON t.Id = e.exposureTemplateId
                         WHERE e.guid=?1",
                        [&guid],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
                    )
                    .optional()?;
                match existing {
                    Some((exposure0, desired0, template0, enabled0, template_name0)) => {
                        // What differs, so a check that asks for an
                        // activation says why.
                        let mut notes = Vec::new();
                        match exposure0 {
                            Some(exposure) if (exposure - exposure_seconds).abs() < 1e-6 => {}
                            Some(exposure) => {
                                notes.push(format!("exposure {exposure} s → {exposure_seconds} s"))
                            }
                            None => notes.push(format!("exposure → {exposure_seconds} s")),
                        }
                        if desired0 != i64::from(frames) {
                            notes.push(format!("desired {desired0} → {frames}"));
                        }
                        if template0 != template_id {
                            let was = match template_name0 {
                                Some(name) => format!("#{template0} {name}"),
                                None => format!("#{template0}"),
                            };
                            notes.push(format!(
                                "template {was} → #{} {}",
                                template.id, template.name
                            ));
                        }
                        if enabled0 != 1 {
                            notes.push("turned on".to_string());
                            turned_on = true;
                        }
                        let same = notes.is_empty();
                        if !same {
                            tx.execute(
                                "UPDATE exposureplan SET exposure=?2, desired=?3, exposureTemplateId=?4, enabled=1, targetid=?5 WHERE guid=?1",
                                params![guid, exposure_seconds, i64::from(frames), template_id, target_row],
                            )?;
                        }
                        changes.push(Change {
                            kind: "plan",
                            action: if same { "unchanged" } else { "update" },
                            name: name.clone(),
                            detail: if same {
                                detail
                            } else {
                                format!("{detail}; {}", notes.join(", "))
                            },
                        });
                    }
                    None => {
                        insert_plan(
                            tx,
                            &guid,
                            &profile_id,
                            exposure_seconds,
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
            // Nothing owned yet: the rig's own plan for the same work on this
            // target (imported from Target Scheduler, or made by hand) is
            // taken over rather than doubled.
            None => match adoptable_plan(
                tx,
                target_row,
                template_id,
                exposure_seconds,
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
                        turned_on = true;
                    }
                    // A row at its template's default length (-1) was
                    // matched on that length, so it already runs at the
                    // plan's and stays as Target Scheduler wrote it. One
                    // asking for 0 s is given the length.
                    let same = notes.is_empty() && !found.zero_length;
                    if !same {
                        tx.execute(
                            "UPDATE exposureplan SET exposure=?2, desired=?3, exposureTemplateId=?4, enabled=1 WHERE Id=?1",
                            params![found.row_id, exposure_seconds, i64::from(frames), template_id],
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
                        exposure_seconds,
                        frames,
                        target_row,
                        template_id,
                    )?;
                    changes.push(Change {
                        kind: "plan",
                        action: "create",
                        name: name.clone(),
                        detail: format!(
                            "{detail}; no plan on this target takes {exposure_seconds} s with these settings"
                        ),
                    });
                    guid
                }
            },
        };
        let frames = match inputs.collaboration {
            Some(a) => a.record(
                tx,
                &plan_guid,
                target_guid,
                &group.panel_id,
                first.contribution,
                inputs.now,
            )?,
            None => frames,
        };
        // Director's side table keeps one owner per plan: the contribution
        // whose row found it, so its other rows never clash.
        tx.execute(
            "INSERT INTO psf_guard_director_plan(exposureplan_guid,target_guid,contribution_id,objective_id,bandpass_id,purpose,required_frames,plan_revision)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8)
             ON CONFLICT(exposureplan_guid) DO UPDATE SET contribution_id=excluded.contribution_id, objective_id=excluded.objective_id, required_frames=excluded.required_frames, plan_revision=excluded.plan_revision, bandpass_id=excluded.bandpass_id, purpose=excluded.purpose",
            params![
                plan_guid,
                target_guid,
                owner.contribution.id.to_string(),
                owner.objective.id.to_string(),
                owner.objective.bandpass_id,
                owner.objective.purpose,
                i64::from(frames),
                inputs.plan.revision as i64
            ],
        )?;
        written.insert(plan_guid.clone());
        if let Some(settings) = template_settings(tx, template_id)? {
            taken
                .entry(target_guid.clone())
                .or_default()
                .push((settings, name.clone()));
        }
        let exposureplan_guid = Uuid::parse_str(&plan_guid).map_err(|_| bad_guid(&plan_guid))?;
        // The activation record keeps every contribution against the plan,
        // each with its own frames, so progress and the program find it
        // under each objective.
        for part in &group.parts {
            plans.push(ActivatedPlan {
                contribution_id: part.contribution.id,
                objective_id: part.objective.id,
                target_guid: Uuid::parse_str(target_guid).map_err(|_| bad_guid(target_guid))?,
                exposureplan_guid,
                required_frames: if group.parts.len() == 1 {
                    frames
                } else {
                    part.frames
                },
                intent: Some(psf_guard_director_meta::activation::PlanIntent {
                    bandpass_id: part.objective.bandpass_id.clone(),
                    purpose: part.objective.purpose.clone(),
                    priority: part.objective.priority,
                }),
            });
        }
        let (row_id, desired, acquired, filter, gain, offset): (i64, i64, i64, String, Option<i64>, Option<i64>) = tx.query_row(
            "SELECT e.Id, IFNULL(e.desired, 0), IFNULL(e.acquired, 0), IFNULL(t.filtername, ''), t.gain, t.offset
             FROM exposureplan e LEFT JOIN exposuretemplate t ON t.Id = e.exposureTemplateId WHERE e.guid=?1",
            [&plan_guid],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
        )?;
        shoots.push(Shot {
            plan: row_id,
            target: group.target_name.clone(),
            template: template.name.clone(),
            filter,
            gain: gain.filter(|g| *g >= 0),
            offset: offset.filter(|o| *o >= 0),
            exposure_seconds,
            desired,
            acquired,
            objectives: group.parts.iter().map(|part| part.objective.id).collect(),
            turned_on,
        });
    }
    // Every other plan on the project's targets is turned off, so Target
    // Scheduler takes only what the plan has: one an earlier activation
    // wrote that nothing plans now (its rig or objective turned off or
    // removed, its band changed, its panel gone), one another length
    // replaced, and one the plan has no part for at all. Its frames, counts
    // and grades stay; taking Target Scheduler's values first keeps it.
    {
        let mut statement = tx.prepare(
            "SELECT e.Id, e.guid, e.exposure, IFNULL(e.acquired, 0), IFNULL(e.desired, 0), t.name, t.defaultexposure,
                    e.exposureTemplateId, tg.guid, tg.name
             FROM exposureplan e
             JOIN target tg ON tg.Id = e.targetid
             LEFT JOIN exposuretemplate t ON t.Id = e.exposureTemplateId
             WHERE tg.projectid = ?1 AND COALESCE(e.enabled, 1) = 1
             ORDER BY e.Id",
        )?;
        type Running = (
            i64,
            Option<String>,
            Option<f64>,
            i64,
            i64,
            Option<String>,
            Option<f64>,
            Option<i64>,
            Option<String>,
            Option<String>,
        );
        let running: Vec<Running> = statement
            .query_map([project_row_id], |row| {
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
                    row.get(9)?,
                ))
            })?
            .collect::<Result<_, _>>()?;
        drop(statement);
        for (
            row_id,
            guid,
            exposure,
            acquired,
            desired,
            template_name,
            default_exposure,
            template_id,
            target_guid,
            target_name,
        ) in running
        {
            if guid.as_ref().is_some_and(|guid| written.contains(guid)) {
                continue;
            }
            let exposure = exposure
                .filter(|e| *e > 0.0)
                .or(default_exposure)
                .unwrap_or(0.0);
            let settings = match template_id {
                Some(id) => template_settings(tx, id)?,
                None => None,
            };
            let replacement = settings.and_then(|settings| {
                target_guid
                    .as_ref()
                    .and_then(|target| taken.get(target))
                    .and_then(|plans| {
                        plans
                            .iter()
                            .find(|(taken, _)| *taken == settings)
                            .map(|(_, by)| by.clone())
                    })
            });
            tx.execute("UPDATE exposureplan SET enabled=0 WHERE Id=?1", [row_id])?;
            changes.push(Change {
                kind: "plan",
                action: "disable",
                name: format!(
                    "{} · {} · {exposure} s",
                    target_name.unwrap_or_default(),
                    template_name.unwrap_or_default()
                ),
                detail: match replacement {
                    Some(by) => format!(
                        "plan #{row_id} ({acquired} of {desired} frames taken), replaced by {by}"
                    ),
                    None => format!(
                        "plan #{row_id} ({acquired} of {desired} frames taken), not in the plan"
                    ),
                },
            });
        }
    }

    Ok(Outcome {
        changes,
        shoots,
        record: ActivatedRig {
            rig_id: inputs.rig_id,
            catalog_id: inputs.catalog.id,
            project_guid: Uuid::parse_str(&project_guid).map_err(|_| bad_guid(&project_guid))?,
            profile_id,
            targets: targets
                .into_iter()
                .map(|(panel_id, guid, _)| {
                    Ok(ActivatedTarget {
                        panel_id,
                        target_guid: Uuid::parse_str(&guid).map_err(|_| bad_guid(&guid))?,
                    })
                })
                .collect::<Result<Vec<_>, RigError>>()?,
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
pub(super) struct Limit {
    pub(super) field: &'static str,
    pub(super) column: &'static str,
    pub(super) label: &'static str,
    pub(super) value: f64,
}

pub(super) fn limits(values: &SchedulingValues) -> [Limit; 9] {
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
pub(super) fn describe(field: &str, value: f64) -> String {
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
    epochcode: i64,
    active: bool,
    /// Frames its exposure plans have taken.
    frames: i64,
}

/// Where a panel sits, as Target Scheduler stores it, and its shorter side.
struct PanelPlace {
    ra_hours: f64,
    dec: f64,
    side_degrees: f64,
}

/// The existing target a panel should take over, if any: the one with the
/// panel's name, else the one that stands for the panel (`stands_for`), else
/// the one at the panel's place (within a quarter of an arcminute, an
/// import's round trip), else, for a single-panel framing, the project's
/// only unowned target. The one the panel stands for counts only within a
/// quarter of the panel side. Ownership is by GUID, so a match
/// without one refuses the rig.
fn adoptable_target(
    tx: &Connection,
    project_row_id: i64,
    name: &str,
    stands_for: Option<i64>,
    place: PanelPlace,
    single_panel: bool,
) -> Result<Option<Adoptable>, RigError> {
    let PanelPlace {
        ra_hours,
        dec,
        side_degrees: panel_side,
    } = place;
    let mut statement = tx.prepare(
        "SELECT Id, guid, name, ra, dec, rotation, IFNULL(epochcode, 2), COALESCE(active, 1),
                (SELECT IFNULL(SUM(acquired), 0) FROM exposureplan WHERE targetid = target.Id)
         FROM target
         WHERE projectid=?1
           AND (guid IS NULL OR guid NOT IN (SELECT target_guid FROM psf_guard_director_target))
         ORDER BY Id",
    )?;
    type Row = (i64, Option<String>, String, f64, f64, f64, i64, bool, i64);
    let rows: Vec<Row> = statement
        .query_map([project_row_id], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                row.get::<_, Option<f64>>(3)?.unwrap_or(f64::NAN),
                row.get::<_, Option<f64>>(4)?.unwrap_or(f64::NAN),
                // A missing angle is written, never taken as 0.
                row.get::<_, Option<f64>>(5)?.unwrap_or(f64::NAN),
                row.get(6)?,
                row.get::<_, i64>(7)? != 0,
                row.get(8)?,
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
    // The target the panel stands for, if it is near the panel: a lone
    // target is `r1c1` whatever the layout, and that is no claim on a
    // target a whole panel or a constellation away.
    let by_panel = stands_for.and_then(|id| {
        rows.iter().position(|row| {
            row.0 == id
                && separation_degrees(row.3 * 15.0, row.4, ra_hours * 15.0, dec)
                    <= 0.25 * panel_side
        })
    });
    let Some(index) = by_name.or(by_panel).or(by_place).or(only) else {
        return Ok(None);
    };
    let (row_id, guid, found_name, found_ra, found_dec, found_rotation, epochcode, active, frames) =
        rows[index].clone();
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
        epochcode,
        active,
        frames,
    }))
}

/// A change's detail without the "(N of M frames taken)" it quotes.
fn without_frame_counts(detail: &str) -> String {
    let mut out = String::with_capacity(detail.len());
    let mut rest = detail;
    while let Some(start) = rest.find('(') {
        let Some(end) = rest[start..].find(')') else {
            break;
        };
        let inner = &rest[start..start + end + 1];
        out.push_str(&rest[..start]);
        if !inner.ends_with("frames taken)") {
            out.push_str(inner);
        }
        rest = &rest[start + end + 1..];
    }
    out.push_str(rest);
    out
}

/// A row Director would take over carries a GUID that is not a GUID.
fn bad_guid(value: &str) -> RigError {
    RigError::Skip(format!(
        "A row in its database has the GUID \"{value}\", which is not a GUID; correct it in Target Scheduler."
    ))
}

/// A row Director would take over has no GUID. Minting one here would give a
/// sync copy GUIDs the rig's own database lacks, so the rig waits for Fill in
/// GUIDs instead, which also keeps Director from making a twin of the row.
fn missing_guid(what: &str) -> RigError {
    RigError::Skip(format!(
        "{what} has no GUID yet. Fill in GUIDs for this database under Settings, Databases, then activate the plan again."
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
    /// The row asks for a 0 s exposure; it was matched on its template's length.
    zero_length: bool,
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
/// What makes two templates take the same frames: filter, gain, offset,
/// binning and readout mode. Exposure is the plan's, not the template's.
type TemplateSettings = (String, i64, i64, i64, i64);

fn template_settings(tx: &Connection, id: i64) -> rusqlite::Result<Option<TemplateSettings>> {
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
}

fn adoptable_plan(
    tx: &Connection,
    target_row: i64,
    template_id: i64,
    exposure_seconds: f64,
    claimed: &std::collections::BTreeSet<i64>,
    held: &std::collections::BTreeSet<String>,
) -> Result<Option<AdoptablePlan>, RigError> {
    let settings_of = |id: i64| template_settings(tx, id);
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
        zero_length: row.2 == Some(0.0),
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
#[derive(Clone)]
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
            Some(wanted) => {
                super::plan::read_moon_policy(tx, id).map_err(|error| {
                    RigError::Skip(format!(
                        "Template #{id}'s Moon avoidance settings cannot be read ({error}); correct them in Target Scheduler."
                    ))
                })? == *wanted
            }
        })
    };
    let mut guid_used = false;
    if let Some(guid) = choice.template_guid {
        // A template belongs to one N.I.N.A. profile; the project's own.
        let found: Option<(i64, String, String)> = tx
            .query_row(
                "SELECT Id, IFNULL(filtername, ''), IFNULL(profileId, '') FROM exposuretemplate WHERE guid=?1",
                [guid.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        guid_used = found.is_some();
        if let Some((id, filter, profile)) = found
            && filter.eq_ignore_ascii_case(&choice.filter_name)
            && profile == profile_id
            && moon_matches(id)?
        {
            return chosen(id);
        }
    }
    // A Target Scheduler table has no AUTOINCREMENT, so a deleted template's
    // id can come back on another one. The id is trusted only when the row
    // still has the chosen settings.
    let settings_match = |id: i64| -> Result<bool, RigError> {
        let row: Option<(String, i64, i64, i64, i64, String)> = tx
            .query_row(
                "SELECT IFNULL(filtername, ''), IFNULL(gain, -1), IFNULL(offset, -1),
                        CASE WHEN IFNULL(bin, 1) < 1 THEN 1 ELSE bin END, IFNULL(readoutmode, -1), IFNULL(profileId, '')
                 FROM exposuretemplate WHERE Id=?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
            )
            .optional()?;
        Ok(
            row.is_some_and(|(filter, gain, offset, bin, readout, profile)| {
                filter
                    .trim()
                    .eq_ignore_ascii_case(choice.filter_name.trim())
                    && gain == i64::from(choice.gain.unwrap_or(-1))
                    && offset == i64::from(choice.offset.unwrap_or(-1))
                    && bin == i64::from(choice.bin.unwrap_or(1).max(1))
                    && readout == i64::from(choice.readout_mode.unwrap_or(-1))
                    && profile == profile_id
            }),
        )
    };
    if let Some(id) = choice.template_id
        && settings_match(id)?
        && moon_matches(id)?
    {
        return chosen(id);
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

/// The angle between two sky positions, in degrees.
fn separation_degrees(ra1: f64, dec1: f64, ra2: f64, dec2: f64) -> f64 {
    let (ra1, dec1, ra2, dec2) = (
        ra1.to_radians(),
        dec1.to_radians(),
        ra2.to_radians(),
        dec2.to_radians(),
    );
    let a = ((dec2 - dec1) / 2.0).sin().powi(2)
        + dec1.cos() * dec2.cos() * ((ra2 - ra1) / 2.0).sin().powi(2);
    (2.0 * a.sqrt().min(1.0).asin()).to_degrees()
}

/// A panel's shorter side, in degrees, from its corners.
fn panel_side_degrees(panel: &Panel) -> f64 {
    let c = &panel.footprint.corners;
    let side = |a: &psf_guard_director_core::visibility::IcrsPosition,
                b: &psf_guard_director_core::visibility::IcrsPosition| {
        separation_degrees(a.ra_degrees, a.dec_degrees, b.ra_degrees, b.dec_degrees)
    };
    side(&c[0], &c[1]).min(side(&c[1], &c[2]))
}

fn format_distance(degrees: f64) -> String {
    if !degrees.is_finite() {
        "an unknown distance".into()
    } else if degrees >= 1.0 {
        format!("{degrees:.2}°")
    } else {
        format!("{:.1}′", degrees * 60.0)
    }
}

/// The most a shot panel unlocked by hand turns its target in place at
/// activation; turned further, it gets a new target, as one moved far
/// does, and the old one keeps its frames at their angle.
const MAX_TURN_IN_PLACE_DEGREES: f64 = 1.0;

/// How far apart two angles are, the short way round.
fn angle_apart(a: f64, b: f64) -> f64 {
    let apart = (a - b).rem_euclid(360.0);
    apart.min(360.0 - apart)
}

/// Whether a stored angle is the framing's plus the panel's own turn, with
/// no lean for where the panel sits: what activations wrote before panels
/// took north's lean, and N.I.N.A. writes for every panel of a mosaic.
fn planned_angle(stored: f64, framing: f64, panel: &Panel) -> bool {
    if !stored.is_finite() || panel.measured.is_some() {
        return false;
    }
    let apart = (stored - framing - panel.angle_offset_degrees).rem_euclid(360.0);
    apart.min(360.0 - apart) < 1e-3
}

/// A panel's camera angle for the activation preview, with what it is made
/// of: the framing's angle, the panel's own turn and north's lean at its
/// center; or as shot, for a panel measured from its frames.
fn describe_angle(rotation: f64, panel: Option<&Panel>, framing: f64) -> String {
    let Some(panel) = panel else {
        return format!("angle {rotation:.1}°");
    };
    if let Some(measured) = &panel.measured {
        let from = match measured.source {
            psf_guard_director_core::framing::MeasuredSource::Solved => "solved frames",
            psf_guard_director_core::framing::MeasuredSource::Header => "frame headers",
            psf_guard_director_core::framing::MeasuredSource::Plan => "the target as planned",
        };
        return format!("angle {rotation:.1}° as shot, from {from}");
    }
    let signed = |degrees: f64| {
        let wrapped = (degrees + 180.0).rem_euclid(360.0) - 180.0;
        format!(
            "{}{:.1}°",
            if wrapped < 0.0 { "−" } else { "+" },
            wrapped.abs()
        )
    };
    let turn = panel.angle_offset_degrees;
    let lean = (rotation - framing - turn + 180.0).rem_euclid(360.0) - 180.0;
    let mut parts = vec![format!("framing {framing:.1}°")];
    if turn.abs() >= 0.05 {
        parts.push(format!("turned {}", signed(turn)));
    }
    if lean.abs() >= 0.05 {
        parts.push(format!("north here {}", signed(lean)));
    }
    if parts.len() == 1 {
        format!("angle {rotation:.1}°")
    } else {
        format!("angle {rotation:.1}° ({})", parts.join(", "))
    }
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
