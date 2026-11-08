//! Changes to what Target Scheduler reads for a rig it drives: a project's
//! priority and state, and whether a target is active. Night plans preview
//! them in the Target Scheduler emulation. Saving writes the changed
//! columns, and only those, of the rig's N.I.N.A. profile, to the rig's
//! database and, for a catalog kept in step with Target Scheduler, to
//! Target Scheduler's own file as well. Each row is written only while it
//! still holds what the person saw. Unlike a Director override, a change
//! lasts until it is changed back.

use super::*;
use psf_guard_director_meta::SYNC_PLACED_PROFILE as WAITING;
use psf_guard_ts_planner::snapshot::{ProjectPriority, ProjectState, Snapshot};
use rusqlite::{params, OpenFlags, OptionalExtension, Transaction, TransactionBehavior};

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/rigs/{rig}/target-scheduler", axum::routing::put(save))
        .layer(DefaultBodyLimit::max(256 * 1024))
}

/// Most rows one change may touch.
const MAX_ROWS: usize = 512;

/// Changes to a rig's Target Scheduler rows.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TsChanges {
    #[serde(default)]
    pub(super) projects: Vec<ProjectChange>,
    #[serde(default)]
    pub(super) targets: Vec<TargetChange>,
}

/// A project's new priority, state or both.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ProjectChange {
    pub(super) project_id: i64,
    #[serde(default)]
    pub(super) priority: Option<ProjectPriority>,
    #[serde(default)]
    pub(super) state: Option<ProjectState>,
    /// What the row held when the person read it. Saving refuses a row that
    /// changed since; a preview does without it.
    #[serde(default)]
    pub(super) was: Option<ProjectWas>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ProjectWas {
    priority: ProjectPriority,
    state: ProjectState,
}

/// Whether a target is active.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TargetChange {
    pub(super) target_id: i64,
    pub(super) active: bool,
    /// What the row held when the person read it; see [`ProjectChange::was`].
    #[serde(default)]
    pub(super) was_active: Option<bool>,
}

impl TsChanges {
    fn rows(&self) -> usize {
        self.projects.len() + self.targets.len()
    }

    /// The snapshot as Target Scheduler would read it with these changes.
    /// A change to a row the snapshot lacks is left out; each such row is
    /// named in what this returns.
    pub(super) fn apply(&self, snapshot: &mut Snapshot) -> Vec<String> {
        let mut missing = Vec::new();
        for change in &self.projects {
            let Some(project) = snapshot
                .projects
                .iter_mut()
                .find(|project| project.id == change.project_id)
            else {
                missing.push(format!(
                    "Project {} is no longer in the rig's database.",
                    change.project_id
                ));
                continue;
            };
            if let Some(priority) = change.priority {
                project.priority = priority;
            }
            if let Some(state) = change.state {
                project.state = state;
            }
        }
        for change in &self.targets {
            let Some(target) = snapshot
                .projects
                .iter_mut()
                .flat_map(|project| project.targets.iter_mut())
                .find(|target| target.id == change.target_id)
            else {
                missing.push(format!(
                    "Target {} is no longer in the rig's database.",
                    change.target_id
                ));
                continue;
            };
            target.active = change.active;
        }
        missing
    }
}

/// A project of the rig's database as Target Scheduler reads it, for the
/// night's controls.
#[derive(Serialize)]
pub(super) struct TsProjectView {
    project_id: i64,
    name: String,
    state: ProjectState,
    priority: ProjectPriority,
    targets: Vec<TsTargetView>,
}

#[derive(Serialize)]
pub(super) struct TsTargetView {
    target_id: i64,
    name: String,
    active: bool,
}

/// Every project of the snapshot, in any state, and its targets.
pub(super) fn views(snapshot: &Snapshot) -> Vec<TsProjectView> {
    snapshot
        .projects
        .iter()
        .map(|project| TsProjectView {
            project_id: project.id,
            name: project.name.clone(),
            state: project.state,
            priority: project.priority,
            targets: project
                .targets
                .iter()
                .map(|target| TsTargetView {
                    target_id: target.id,
                    name: target.name.clone(),
                    active: target.active,
                })
                .collect(),
        })
        .collect()
}

/// Where saving the night's Target Scheduler changes writes, or why it
/// cannot.
#[derive(Serialize)]
pub(super) struct TsSaveView {
    /// The files written, the rig's database first.
    files: Vec<String>,
    refused: Option<String>,
}

impl TsSaveView {
    pub(super) fn of(
        context: &DatabaseContext,
        peer: Option<&str>,
        database_management: bool,
    ) -> Self {
        let refused = match peer {
            Some(peer) => Some(not_here(peer)),
            None if !database_management => Some(NO_DATABASE_MANAGEMENT.into()),
            None => None,
        };
        if let Some(refused) = refused {
            return Self {
                files: Vec::new(),
                refused: Some(refused),
            };
        }
        let mut files = vec![context.database_path.clone()];
        if let Some(sync) = &context.ts_sync {
            files.push(sync.source_path.clone());
        }
        Self {
            files,
            refused: None,
        }
    }
}

/// Why a server that may not write into rig databases offers no changes.
const NO_DATABASE_MANAGEMENT: &str =
    "Changing Target Scheduler settings needs database management on this server.";

fn not_here(peer: &str) -> String {
    format!(
        "This rig runs on another PSF Guard ({peer}): change its Target Scheduler settings there."
    )
}

#[derive(Serialize)]
struct Saved {
    files: Vec<String>,
    projects: usize,
    targets: usize,
}

async fn save(
    State(state): State<Arc<AppState>>,
    Path(rig): Path<Uuid>,
    Json(changes): Json<TsChanges>,
) -> Result<Json<ApiResponse<Saved>>, Error> {
    if changes.rows() == 0 || changes.rows() > MAX_ROWS {
        return Err(Error::Refused(format!(
            "Send from 1 to {MAX_ROWS} changed rows."
        )));
    }
    if changes.projects.iter().any(|change| change.was.is_none())
        || changes
            .targets
            .iter()
            .any(|change| change.was_active.is_none())
    {
        return Err(Error::Refused(
            "Each change needs what the row held when it was read.".into(),
        ));
    }
    let service = writable(&state)?;
    let catalogs: Vec<Arc<DatabaseContext>> = state
        .databases
        .read()
        .map_err(|_| Error::Internal)?
        .values()
        .cloned()
        .collect();
    let instance = service.instance_id;
    let context = service
        .clone()
        .with_reader(move |store| {
            store.rig(rig)?.ok_or(Error::Missing)?;
            let profile = store.rig_profile(rig)?;
            if let Some(peer) = profile.as_ref().and_then(|p| p.peer_id.as_deref()) {
                return Err(Error::Mode(not_here(peer)));
            }
            if night_plans::runtime_of(store, rig, profile.as_ref())?
                == night_plans::Runtime::Director
            {
                return Err(Error::Mode(
                    "Director picks this rig's targets, so its Target Scheduler settings have no effect. Use Edit night instead.".into(),
                ));
            }
            let found = identified_catalogs(&catalogs, instance);
            for (identity, context) in found.iter() {
                if store
                    .catalog_rig(identity.id)?
                    .is_some_and(|binding| binding.rig.id == rig)
                {
                    return Ok(Arc::clone(context));
                }
            }
            Err(Error::Missing)
        })
        .await?;
    // The rig-database gate, as activation and Apply take it, so no two of
    // them write one database at once.
    let _permit = admit(&service.discovery_admission).await?;
    let saved = blocking(move || write(&context, &changes)).await??;
    Ok(Json(ApiResponse::success(saved)))
}

/// Which column finds a row: its id in the rig's database, its GUID in
/// Target Scheduler's file, where ids may differ.
enum Row<'a> {
    Id(i64),
    Guid(&'a str),
}

impl Row<'_> {
    fn clause(&self) -> &'static str {
        match self {
            Self::Id(_) => "Id",
            Self::Guid(_) => "guid",
        }
    }

    fn value(&self) -> rusqlite::types::Value {
        match self {
            Self::Id(id) => (*id).into(),
            Self::Guid(guid) => (*guid).to_owned().into(),
        }
    }
}

fn store_error(error: rusqlite::Error) -> Error {
    StoreError::Sqlite(error).into()
}

/// A failure in Target Scheduler's file, which N.I.N.A. may hold open.
fn source_error(error: rusqlite::Error) -> Error {
    if let rusqlite::Error::SqliteFailure(code, _) = &error
        && matches!(
            code.code,
            rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
        )
    {
        return Error::Unavailable(
            "Target Scheduler's database is in use, probably by N.I.N.A. Try again in a moment."
                .into(),
        );
    }
    Error::Refused(format!(
        "Target Scheduler's database could not be written: {error}"
    ))
}

/// A GUID that names a row. Rows from before Target Scheduler's GUIDs hold
/// an empty one, which would match every such row.
fn named(guid: Option<String>, name: &str) -> Result<String, Error> {
    guid.filter(|guid| !guid.trim().is_empty())
        .ok_or_else(|| missing_guid(name))
}

/// Write the changes to the rig's database and, when it is kept in step
/// with Target Scheduler, to Target Scheduler's file, all or nothing in
/// each, and only rows of the rig's N.I.N.A. profile. The rig's database
/// commits first: it is where PSF Guard keeps planning, and Send planning
/// carries it to Target Scheduler, so a failure between the two commits
/// leaves the change where the next send finds it.
fn write(context: &DatabaseContext, changes: &TsChanges) -> Result<Saved, Error> {
    let mut source = match &context.ts_sync {
        Some(sync) => Some(
            super::super::database_context::open_scheduler_connection_with_flags(
                &sync.source_path,
                OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
            .map_err(|error| {
                Error::Refused(format!(
                    "Target Scheduler's database could not be opened for writing: {error}"
                ))
            })?,
        ),
        None => None,
    };
    // Immediate: each reads before it writes, and such a transaction cannot
    // wait for another connection's write lock. Target Scheduler's first,
    // so a wait for N.I.N.A. does not hold the catalog's connection.
    let source_tx = source
        .as_mut()
        .map(|connection| connection.transaction_with_behavior(TransactionBehavior::Immediate))
        .transpose()
        .map_err(source_error)?;
    let connection = context.db();
    let mut catalog = connection.lock().map_err(|_| Error::Internal)?;
    let catalog_tx = catalog
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(store_error)?;
    let reported = super::activation::reported_profile(&context.id);
    let profile = super::activation::resolve_profile(&catalog_tx, reported.as_deref())
        .map_err(store_error)?
        .ok_or_else(|| Error::Changed("The rig's database has no N.I.N.A. profile.".into()))?;
    for change in &changes.projects {
        let (guid, name): (Option<String>, String) = catalog_tx
            .query_row(
                &format!("SELECT guid, name FROM project WHERE Id = ?1 AND profileId IN (?2,'{WAITING}')"),
                params![change.project_id, profile],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(store_error)?
            .ok_or_else(|| {
                Error::Changed(format!(
                    "Project {} is no longer in the rig's database.",
                    change.project_id
                ))
            })?;
        let was = change.was.ok_or(Error::Invalid)?;
        update_project(
            &catalog_tx,
            Row::Id(change.project_id),
            &profile,
            change,
            was,
        )
        .map_err(store_error)?
        .ok_or_else(|| changed(&name, Where::Catalog))?;
        if let Some(tx) = &source_tx {
            let guid = named(guid, &name)?;
            update_project(tx, Row::Guid(&guid), &profile, change, was)
                .map_err(source_error)?
                .ok_or_else(|| changed(&name, Where::Source))?;
        }
    }
    for change in &changes.targets {
        let (guid, name): (Option<String>, String) = catalog_tx
            .query_row(
                &format!(
                    "SELECT t.guid, t.name FROM target t JOIN project p ON p.Id = t.projectid
                 WHERE t.Id = ?1 AND p.profileId IN (?2,'{WAITING}')"
                ),
                params![change.target_id, profile],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(store_error)?
            .ok_or_else(|| {
                Error::Changed(format!(
                    "Target {} is no longer in the rig's database.",
                    change.target_id
                ))
            })?;
        let was = change.was_active.ok_or(Error::Invalid)?;
        update_target(
            &catalog_tx,
            Row::Id(change.target_id),
            &profile,
            change.active,
            was,
        )
        .map_err(store_error)?
        .ok_or_else(|| changed(&name, Where::Catalog))?;
        if let Some(tx) = &source_tx {
            let guid = named(guid, &name)?;
            update_target(tx, Row::Guid(&guid), &profile, change.active, was)
                .map_err(source_error)?
                .ok_or_else(|| changed(&name, Where::Source))?;
        }
    }
    catalog_tx.commit().map_err(store_error)?;
    drop(catalog);
    let mut files = vec![context.database_path.clone()];
    if let (Some(tx), Some(sync)) = (source_tx, &context.ts_sync) {
        tx.commit().map_err(|error| {
            tracing::warn!(%error, source = %sync.source_path, "Target Scheduler's database refused a commit the rig's database took");
            Error::Unavailable(format!(
                "The changes were saved in the rig's database but not in Target Scheduler's ({error}). To copy them to Target Scheduler, use Send planning in the rig's Sync settings."
            ))
        })?;
        files.push(sync.source_path.clone());
    }
    Ok(Saved {
        files,
        projects: changes.projects.len(),
        targets: changes.targets.len(),
    })
}

fn missing_guid(name: &str) -> Error {
    Error::Refused(format!(
        "{name} has no GUID, so it cannot be found in Target Scheduler's database. Use Fill in GUIDs in the rig's Database & files settings first."
    ))
}

/// Set a project's priority and state where the row still holds what was
/// read. `None` when no row of the profile does.
fn update_project(
    tx: &Transaction<'_>,
    row: Row<'_>,
    profile: &str,
    change: &ProjectChange,
    was: ProjectWas,
) -> rusqlite::Result<Option<()>> {
    let priority = change.priority.unwrap_or(was.priority);
    let state = change.state.unwrap_or(was.state);
    let written = tx.execute(
        &format!(
            "UPDATE project SET priority = ?1, state = ?2
             WHERE {} = ?3 AND profileId IN (?4,'{WAITING}') AND priority = ?5 AND state = ?6",
            row.clause()
        ),
        params![
            priority.to_column(),
            state.to_column(),
            row.value(),
            profile,
            was.priority.to_column(),
            was.state.to_column()
        ],
    )?;
    Ok((written == 1).then_some(()))
}

/// Set whether a target is active where the row still holds what was read.
/// `None` when no row of the profile does.
fn update_target(
    tx: &Transaction<'_>,
    row: Row<'_>,
    profile: &str,
    active: bool,
    was: bool,
) -> rusqlite::Result<Option<()>> {
    let written = tx.execute(
        &format!(
            "UPDATE target SET active = ?1
             WHERE {} = ?2 AND active = ?3 AND projectid IN (SELECT Id FROM project WHERE profileId IN (?4,'{WAITING}'))",
            row.clause()
        ),
        params![active, row.value(), was, profile],
    )?;
    Ok((written == 1).then_some(()))
}

/// Which file a refused row is in.
enum Where {
    Catalog,
    Source,
}

fn changed(name: &str, place: Where) -> Error {
    Error::Changed(match place {
        Where::Catalog => format!(
            "{name} changed in the rig's database. Make your changes again."
        ),
        Where::Source => format!(
            "{name} differs between Target Scheduler's database and the rig's, or is missing from Target Scheduler's. In the rig's Sync settings, use Merge databases to keep Target Scheduler's settings, or Send planning to keep the rig's. Then make your changes again."
        ),
    })
}
