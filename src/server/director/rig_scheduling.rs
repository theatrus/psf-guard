//! A rig's Target Scheduler scheduling limits, written into every project in
//! its database when the operator applies them, including projects made in
//! N.I.N.A. Each project takes what an activation would write: the limits
//! set for every plan, for the rig's site and for the rig, and for a project
//! a plan holds, that plan's own. The rig's value replaces one changed by
//! hand in Target Scheduler; a limit no scope sets stays as each project has
//! it. The preview lists what differs, and Apply writes exactly that or
//! refuses when the database or the limits changed in between.

use super::activation::{self, ActivationError, Push};
use super::*;
use crate::{db_registry::PeerEntry, server::peers::registered_peers};
use psf_guard_director_core::priority::Scope;
use psf_guard_director_meta::preferences::ResolvedScheduling;
use rusqlite::{Connection, OpenFlags};
use std::collections::BTreeSet;

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/rigs/{rig}/scheduling", get(preview))
        .route("/rigs/{rig}/scheduling/apply", axum::routing::post(apply))
}

#[derive(Serialize)]
struct LimitChange {
    label: &'static str,
    was: String,
    now: String,
}

#[derive(Serialize)]
struct ProjectChange {
    name: String,
    /// Target Scheduler's state: 0 draft, 1 active, 2 inactive, 3 closed.
    state: i64,
    /// The plan that sets some of this project's limits itself.
    plan: Option<NamedIdentity>,
    changes: Vec<LimitChange>,
}

#[derive(Serialize)]
pub(super) struct Report {
    rig: NamedIdentity,
    catalog_slug: String,
    catalog_name: String,
    /// Projects whose limits differ, each with what changes.
    projects: Vec<ProjectChange>,
    /// Projects that already have every limit.
    matching: usize,
    /// Limits no scope sets for the rig, which no project is given.
    unset: Vec<&'static str>,
    digest: String,
    applied: bool,
    /// Set when the rig's real database is on a Sync peer.
    push: Option<Push>,
    warnings: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Apply {
    digest: String,
}

async fn preview(
    State(state): State<Arc<AppState>>,
    Path(rig): Path<Uuid>,
) -> Result<Json<ApiResponse<Report>>, ActivationError> {
    run(state, rig, None).await
}

async fn apply(
    State(state): State<Arc<AppState>>,
    Path(rig): Path<Uuid>,
    Json(request): Json<Apply>,
) -> Result<Json<ApiResponse<Report>>, ActivationError> {
    if request.digest.len() != 64
        || !request
            .digest
            .bytes()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    {
        return Err(Error::Invalid.into());
    }
    run(state, rig, Some(request.digest)).await
}

/// What one Target Scheduler project linked to a plan resolves to.
struct Follows {
    plan: Option<NamedIdentity>,
    scheduling: ResolvedScheduling,
}

/// The writes one read of the rig database calls for.
struct Planned {
    projects: Vec<ProjectChange>,
    matching: usize,
    /// Project row id, column, value.
    writes: Vec<(i64, &'static str, f64)>,
    digest: String,
}

async fn run(
    state: Arc<AppState>,
    rig: Uuid,
    expected: Option<String>,
) -> Result<Json<ApiResponse<Report>>, ActivationError> {
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
    // Apply holds the rig-database gate, as activation does, so the two
    // never write one database at once.
    let _permit = match expected {
        Some(_) => Some(admit(&service.discovery_admission).await?),
        None => None,
    };
    let peers = registered_peers(&state);
    let instance = service.instance_id;
    let found = service
        .clone()
        .with_reader(move |store| -> Result<_, ActivationError> {
            let identity = store.rig(rig)?.ok_or(Error::Missing)?;
            let mut bound = None;
            for (catalog, context) in identified_catalogs(&catalogs, instance).iter() {
                if store
                    .catalog_rig(catalog.id)?
                    .is_some_and(|binding| binding.rig.id == rig)
                {
                    bound = Some((catalog.id, context.clone()));
                    break;
                }
            }
            let (catalog, context) = bound.ok_or(ActivationError::NotReady(
                "This rig has no registered database on this server.",
            ))?;
            let defaults = store.effective_observing_preferences(rig, None)?.scheduling;
            let mut warnings = Vec::new();
            let mut follows = BTreeMap::new();
            // A plan whose assignment could not be read keeps its project
            // out of the writes altogether.
            let mut skipped = BTreeSet::new();
            let mut after = None;
            loop {
                let page = store.catalog_project_mappings(catalog, after, 256)?;
                for mapping in &page.items {
                    let project = mapping.project_id;
                    let mut scheduling = store
                        .effective_observing_preferences(rig, Some(project))?
                        .scheduling;
                    // A collaboration assignment's altitude floor, as
                    // activation applies it, so the two never undo each other.
                    if store.collaboration_requires_admission(project)? {
                        let floor = collaboration_activation::current(
                            store, project, instance, &catalogs,
                        )
                        .and_then(|visits| collaboration_activation::admit(store, project, &visits))
                        .map(|admitted| {
                            admitted
                                .get(&rig)
                                .and_then(|a| a.import.plan.share().requirements.as_ref())
                                .and_then(|r| r.min_altitude_degrees)
                        });
                        match floor {
                            Ok(Some(minimum)) => {
                                scheduling.values.minimum_altitude_degrees =
                                    scheduling.values.minimum_altitude_degrees.max(minimum);
                            }
                            Ok(None) => {}
                            Err(_) => {
                                let name = store.project(project)?.map_or_else(String::new, |p| p.name);
                                warnings.push(format!(
                                    "Plan {name}: its collaboration assignment could not be read, so its project is left as it is; activation sets its limits."
                                ));
                                skipped.insert(mapping.source_project_guid);
                                continue;
                            }
                        }
                    }
                    let own = scheduling
                        .sources
                        .values()
                        .any(|source| source.scope == Scope::Project);
                    let plan = if own { store.project(project)? } else { None };
                    follows.insert(mapping.source_project_guid, Follows { plan, scheduling });
                }
                match page.next_after {
                    Some(next) => after = Some(next),
                    None => break,
                }
            }
            let profile = store.rig_profile(rig)?;
            let peer: Option<PeerEntry> = profile
                .as_ref()
                .and_then(|p| p.peer_id.as_deref())
                .and_then(|id| peers.iter().find(|peer| peer.id == id).cloned());
            let push = activation::planned_push(profile.as_ref(), &peers, &mut warnings);
            Ok((identity, context, defaults, follows, skipped, peer, push, warnings))
        })
        .await?;
    let (identity, context, defaults, follows, skipped, peer, mut push, mut warnings) = found;
    let unset = activation::limits(&defaults.values)
        .into_iter()
        .filter(|limit| !defaults.sources.contains_key(limit.field))
        .map(|limit| limit.label)
        .collect();
    let path = context.database_path.clone();
    let applying = expected.clone();
    // The rig database is read, and written, with nothing of the store held.
    let outcome = tokio::task::spawn_blocking(
        move || -> Result<Result<(Planned, bool), String>, ActivationError> {
            match applying {
                None => {
                    let connection = match read_only(&path) {
                        Ok(connection) => connection,
                        Err(message) => return Ok(Err(message)),
                    };
                    Ok(Ok((
                        plan(&connection, &defaults, &follows, &skipped)?,
                        false,
                    )))
                }
                Some(expected) => {
                    let mut connection = match activation::open_rig(&path) {
                        Ok(connection) => connection,
                        Err(message) => return Ok(Err(message)),
                    };
                    let tx = match activation::begin_rig(&mut connection) {
                        Ok(tx) => tx,
                        Err(message) => return Ok(Err(message)),
                    };
                    let planned = plan(&tx, &defaults, &follows, &skipped)?;
                    if planned.digest != expected {
                        return Err(Error::Conflict.into());
                    }
                    for (id, column, value) in &planned.writes {
                        tx.execute(
                            &format!("UPDATE project SET \"{column}\"=?2 WHERE Id=?1"),
                            rusqlite::params![id, value],
                        )?;
                    }
                    tx.commit()?;
                    Ok(Ok((planned, true)))
                }
            }
        },
    )
    .await
    .map_err(|_| Error::Internal)??;
    let (planned, applied) = match outcome {
        Ok(done) => done,
        Err(message) => {
            warnings.push(message);
            (
                Planned {
                    projects: vec![],
                    matching: 0,
                    writes: vec![],
                    digest: String::new(),
                },
                false,
            )
        }
    };
    // A remote rig's real database takes the rows by Sync, as after an
    // activation.
    if applied
        && !planned.writes.is_empty()
        && let Some(peer) = peer
    {
        push = Some(activation::push_planning(&state, &context, &peer).await);
    }
    Ok(Json(ApiResponse::success(Report {
        rig: identity,
        catalog_slug: context.id.clone(),
        catalog_name: context.name.clone(),
        projects: planned.projects,
        matching: planned.matching,
        unset,
        digest: planned.digest,
        applied,
        push,
        warnings,
    })))
}

fn read_only(path: &str) -> Result<Connection, String> {
    let connection = crate::server::database_context::open_scheduler_connection_with_flags(
        std::path::Path::new(path),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| format!("Its database could not be read: {error}."))?;
    connection
        .busy_timeout(std::time::Duration::from_secs(2))
        .map_err(|error| format!("Its database could not be read: {error}."))?;
    Ok(connection)
}

/// Every project's limits against what it resolves to: a plan's project its
/// plan's, every other project the rig's. Only limits some scope sets are
/// compared, and only columns this Target Scheduler schema has.
fn plan(
    connection: &Connection,
    defaults: &ResolvedScheduling,
    follows: &BTreeMap<Uuid, Follows>,
    skipped: &BTreeSet<Uuid>,
) -> rusqlite::Result<Planned> {
    let present: BTreeSet<String> = {
        let mut statement =
            connection.prepare("SELECT lower(name) FROM pragma_table_info('project')")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<rusqlite::Result<_>>()?
    };
    let columns: Vec<&'static str> = activation::limits(&defaults.values)
        .iter()
        .map(|limit| limit.column)
        .filter(|column| present.contains(&column.to_ascii_lowercase()))
        .collect();
    let guid = if present.contains("guid") {
        "guid"
    } else {
        "NULL"
    };
    let selected = columns
        .iter()
        .map(|column| format!("\"{column}\""))
        .collect::<Vec<_>>()
        .join(", ");
    let mut statement = connection.prepare(&format!(
        "SELECT Id, {guid}, IFNULL(name, ''), IFNULL(state, 0){}{selected}
         FROM project ORDER BY name COLLATE NOCASE, Id",
        if columns.is_empty() { "" } else { ", " }
    ))?;
    let mut rows = statement.query([])?;
    let mut planned = Planned {
        projects: vec![],
        matching: 0,
        writes: vec![],
        digest: String::new(),
    };
    let mut lines = Vec::new();
    while let Some(row) = rows.next()? {
        let id: i64 = row.get(0)?;
        let guid: Option<String> = row.get(1)?;
        let name: String = row.get(2)?;
        let state: i64 = row.get(3)?;
        let key = guid.as_deref().and_then(|g| Uuid::parse_str(g.trim()).ok());
        if key.is_some_and(|key| skipped.contains(&key)) {
            continue;
        }
        let linked = key.and_then(|key| follows.get(&key));
        let scheduling = linked.map_or(defaults, |f| &f.scheduling);
        let mut changes = Vec::new();
        for limit in activation::limits(&scheduling.values) {
            let Some(index) = columns.iter().position(|c| *c == limit.column) else {
                continue;
            };
            if !scheduling.sources.contains_key(limit.field) {
                continue;
            }
            let current: Option<f64> = row.get(4 + index)?;
            if current.is_some_and(|now| (now - limit.value).abs() < 1e-9) {
                continue;
            }
            lines.push(format!(
                "{id}\t{}\t{name}\t{}\t{current:?}\t{}",
                guid.as_deref().unwrap_or(""),
                limit.column,
                limit.value
            ));
            planned.writes.push((id, limit.column, limit.value));
            changes.push(LimitChange {
                label: limit.label,
                was: current.map_or_else(
                    || "unset".into(),
                    |now| activation::describe(limit.field, now),
                ),
                now: activation::describe(limit.field, limit.value),
            });
        }
        if changes.is_empty() {
            planned.matching += 1;
        } else {
            planned.projects.push(ProjectChange {
                name,
                state,
                plan: linked.and_then(|f| f.plan.clone()),
                changes,
            });
        }
    }
    planned.digest = catalog_discovery::digest(lines.join("\n").as_bytes());
    Ok(planned)
}
