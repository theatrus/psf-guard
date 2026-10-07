//! The Director page's plan list: every global project with where it is
//! linked, and how far its framing, plan and activation have come.
//!
//! The list also takes in what is new, without an operator step: a database
//! seen for the first time becomes a rig, a project a plan, and its Target
//! Scheduler rows that plan's first drafts. Every database is read on a
//! read-only connection with no gate held, and the list is built on a pooled
//! reader. Only something new takes the writer, briefly, and a database is
//! opened for writing only to add its missing identity row.

use super::catalog_discovery::DiscoveryError;
use super::import_drafts::{self, Drafts, Missing};
use super::rig_profile;
use super::*;
use crate::catalog_identity;
use crate::server::database_context::DatabaseContext;
use psf_guard_director_core::{
    framing::{FramingRequest, Mosaic, PanelSize},
    visibility::IcrsPosition,
};
use psf_guard_director_meta::{catalog::SourceProject, profile::RigProfile, MAX_PLANS};
use rusqlite::{Connection, OpenFlags, TransactionBehavior};
use std::{
    collections::{BTreeMap, HashMap},
    time::Duration,
};

/// The most database links one listing carries.
const MAX_LINKS: usize = 4096;

/// One target of a linked project and its frames: what the exposure plans
/// ask for, have, and have accepted. A mosaic has one per panel.
#[derive(Clone, Serialize)]
struct TargetProgress {
    name: String,
    desired: i64,
    acquired: i64,
    accepted: i64,
    /// Frames graded rejected in `acquiredimage`; the rest of `acquired`
    /// minus `accepted` is still pending.
    rejected: i64,
    /// Where Target Scheduler points this target, in ICRS degrees, when the
    /// row has coordinates.
    center: Option<IcrsPosition>,
    rotation_degrees: Option<f64>,
}

#[derive(Serialize)]
struct PlanLink {
    catalog_slug: String,
    catalog_name: String,
    rig: NamedIdentity,
    source_project_guid: Uuid,
    /// The row and name in that database, when the project still exists there.
    source_row_id: Option<i64>,
    source_name: Option<String>,
    /// Target Scheduler's project state there: 0 draft, 1 active, 2
    /// inactive, 3 closed; None when the row is gone.
    source_state: Option<i32>,
    /// First and last capture of the project's frames there, Unix seconds.
    earliest_capture_s: Option<i64>,
    latest_capture_s: Option<i64>,
    /// The project's targets in that database, in row order.
    targets: Vec<TargetProgress>,
    /// The database could not be read for this listing and none was read
    /// before, so its row, state and targets are unknown, not gone. The
    /// listing's warnings say why.
    source_unread: bool,
}

/// What the Library shows for a project and the plan list shows per rig:
/// its Target Scheduler state and when its frames were taken.
#[derive(Clone, Copy, Default)]
struct ProjectFacts {
    state: Option<i32>,
    earliest_capture_s: Option<i64>,
    latest_capture_s: Option<i64>,
}

/// Whether a row failed on one odd cell, which skips the row, rather than
/// on the database, which fails the read.
fn is_cell_error(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::FromSqlConversionFailure(..)
            | rusqlite::Error::InvalidColumnType(..)
            | rusqlite::Error::IntegralValueOutOfRange(..)
    )
}

fn is_busy(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(code, _)
            if matches!(code.code, rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked)
    )
}

/// The rows a query answers, skipping any with an odd cell. A query the
/// schema cannot run (an older Target Scheduler without the columns)
/// answers none; any other failure is an error.
fn rows_of<T>(
    connection: &Connection,
    sql: &str,
    map: impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
) -> rusqlite::Result<Vec<T>> {
    let Ok(mut statement) = connection.prepare(sql) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for row in statement.query_map([], map)? {
        match row {
            Ok(value) => out.push(value),
            Err(error) if is_cell_error(&error) => {}
            Err(error) => return Err(error),
        }
    }
    Ok(out)
}

/// Per project row, its state and capture dates.
fn project_facts(connection: &Connection) -> rusqlite::Result<BTreeMap<i64, ProjectFacts>> {
    let mut facts: BTreeMap<i64, ProjectFacts> = BTreeMap::new();
    for (id, state) in rows_of(connection, "SELECT Id, state FROM project", |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, Option<i32>>(1)?))
    })? {
        facts.entry(id).or_default().state = state;
    }
    for (id, earliest, latest) in rows_of(
        connection,
        "SELECT t.projectid, MIN(a.acquireddate), MAX(a.acquireddate)
         FROM acquiredimage a JOIN target t ON a.targetId = t.Id
         WHERE a.acquireddate IS NOT NULL GROUP BY t.projectid",
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, Option<i64>>(1)?,
                row.get::<_, Option<i64>>(2)?,
            ))
        },
    )? {
        let entry = facts.entry(id).or_default();
        entry.earliest_capture_s = earliest;
        entry.latest_capture_s = latest;
    }
    Ok(facts)
}

/// Frames across every linked database, so the list can say how far a plan
/// has come without opening it.
#[derive(Serialize)]
struct Progress {
    desired: i64,
    acquired: i64,
    accepted: i64,
    rejected: i64,
    targets: u32,
}

/// Enough of the framing for the plan list to draw a thumbnail: the
/// survey view at the center, with the panel rectangles over it.
#[derive(Serialize)]
struct FramingSummary {
    /// `draft`: a framing saved in Director. `catalog`: no draft yet, so the
    /// first linked database's target stands in, with the rig's field as
    /// the panel when its optics are known.
    source: &'static str,
    revision: u64,
    target_name: String,
    panels: u32,
    panel_rig_id: Option<Uuid>,
    center: IcrsPosition,
    position_angle_degrees: f64,
    panel: Option<PanelSize>,
    mosaic: Mosaic,
    survey_id: String,
    /// The whole mosaic along the camera axes, once a panel size is known.
    extent: Option<PanelSize>,
}

#[derive(Serialize)]
struct PlanSummary {
    revision: u64,
    objectives: u32,
    rigs: u32,
}

#[derive(Serialize)]
struct ActivationSummary {
    revision: u64,
    applied_at_ms: u64,
    rigs: u32,
}

#[derive(Serialize)]
pub(super) struct PlanRow {
    project: NamedIdentity,
    links: Vec<PlanLink>,
    /// `None` until some linked database holds a target for the project.
    progress: Option<Progress>,
    framing: Option<FramingSummary>,
    plan: Option<PlanSummary>,
    activation: Option<ActivationSummary>,
}

/// Every target's frame counts, grouped by project row.
fn target_progress(
    connection: &Connection,
) -> rusqlite::Result<BTreeMap<i64, Vec<TargetProgress>>> {
    let mut by_project: BTreeMap<i64, Vec<TargetProgress>> = BTreeMap::new();
    // `rotation` arrived with a later Target Scheduler schema.
    let rotation = if import_drafts::has_column(connection, "target", "rotation") {
        "t.rotation"
    } else {
        "NULL"
    };
    // Rejected frames live in `acquiredimage`; a pre-TS5 file names the column
    // `accepted` and then reports none.
    let rejected: BTreeMap<i64, i64> = rows_of(
        connection,
        "SELECT targetId, COUNT(*) FROM acquiredimage WHERE gradingStatus = 2 GROUP BY targetId",
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
    )?
    .into_iter()
    .collect();
    let rows = rows_of(
        connection,
        &format!(
            "SELECT t.projectid, t.name, COALESCE(SUM(e.desired), 0), COALESCE(SUM(e.acquired), 0), COALESCE(SUM(e.accepted), 0),
                    t.ra, t.dec, {rotation}, t.Id
             FROM target t LEFT JOIN exposureplan e ON e.targetid = t.Id
             GROUP BY t.Id ORDER BY t.Id"
        ),
        |row| {
            let ra_hours: Option<f64> = row.get(5)?;
            let dec: Option<f64> = row.get(6)?;
            let center = match (ra_hours, dec) {
                (Some(ra_hours), Some(dec_degrees)) => {
                    crate::astrometry::target_scheduler_coordinates(ra_hours, dec_degrees).map(
                        |(ra_degrees, dec_degrees)| IcrsPosition {
                            ra_degrees,
                            dec_degrees,
                        },
                    )
                }
                _ => None,
            };
            Ok((
                row.get::<_, i64>(0)?,
                TargetProgress {
                    name: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    desired: row.get(2)?,
                    acquired: row.get(3)?,
                    accepted: row.get(4)?,
                    rejected: rejected.get(&row.get::<_, i64>(8)?).copied().unwrap_or(0),
                    center,
                    rotation_degrees: row.get::<_, Option<f64>>(7)?.filter(|r| r.is_finite()),
                },
            ))
        },
    )?;
    for (project, target) in rows {
        by_project.entry(project).or_default().push(target);
    }
    Ok(by_project)
}

#[derive(Serialize)]
pub(super) struct PlanList {
    rows: Vec<PlanRow>,
    /// Databases and records the list could not read or take in, and why.
    warnings: Vec<String>,
}

/// What earlier listings learned, kept while the server runs.
#[derive(Default)]
pub(super) struct Memo {
    /// Per database file, its identity and what it held at the last read
    /// that worked: what the list shows while the file is busy.
    pub(super) reads: HashMap<String, (CatalogIdentity, Arc<Contents>)>,
    /// Per rig, the newest frame its headers were last searched at, so the
    /// frames of a rig whose headers name no optics are not searched on
    /// every listing, only once more frames arrive.
    header_searches: HashMap<Uuid, Option<i64>>,
    /// Where the next listing tells a test it reached its header search,
    /// and waits for the test to let it go on.
    #[cfg(test)]
    pub(super) hold: Option<(
        tokio::sync::oneshot::Sender<()>,
        std::sync::mpsc::Receiver<()>,
    )>,
}

/// A source project with a usable GUID.
struct SourceRow {
    guid: Uuid,
    row: i64,
    name: Option<String>,
    profile: String,
}

/// What one database holds that the list needs.
pub(super) struct Contents {
    /// In row order.
    rows: Vec<SourceRow>,
    targets: BTreeMap<i64, Vec<TargetProgress>>,
    facts: BTreeMap<i64, ProjectFacts>,
    /// The newest frame's row.
    newest_frame: Option<i64>,
}

/// One registered database as this listing sees it.
struct Source {
    catalog: Arc<DatabaseContext>,
    identity: CatalogIdentity,
    /// Read now and carrying no identity table: `identity` is the derived
    /// one, written into the file once a managing server binds it.
    unwritten: bool,
    /// What the file holds: read now when `fresh`, else from the last read
    /// that worked, or `None` when there was none.
    contents: Option<Arc<Contents>>,
    /// Only a database read now is taken in.
    fresh: bool,
}

/// One database as this listing read it.
enum Read {
    /// Its identity row and contents, from one snapshot.
    Done {
        saved: Option<CatalogIdentity>,
        contents: Contents,
    },
    /// It holds nothing planning can take in, such as no project table.
    Unusable {
        saved: Option<CatalogIdentity>,
        cause: &'static str,
    },
    /// Busy or not readable now, so it may read on the next listing.
    /// `identity` is the one it is planned under, when that much was read.
    Failed {
        identity: Option<CatalogIdentity>,
        cause: &'static str,
    },
}

/// Read one file's identity and contents in one snapshot, so the rows, their
/// targets and their frames agree.
fn read_contents(instance: Uuid, catalog: &DatabaseContext, notes: &mut Vec<String>) -> Read {
    let failed = |identity, cause| Read::Failed { identity, cause };
    let Ok(mut connection) = super::super::database_context::open_scheduler_connection_with_flags(
        FilePath::new(&catalog.database_path),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) else {
        return failed(None, "could not be opened");
    };
    if connection.busy_timeout(Duration::from_secs(1)).is_err() {
        return failed(None, "could not be opened");
    }
    let Ok(tx) = connection.transaction() else {
        return failed(None, "could not be opened");
    };
    let saved = match catalog_identity::read(&tx) {
        Ok(saved) => saved,
        Err(catalog_identity::Error::Sqlite(error)) if is_busy(&error) => {
            return failed(None, "is busy")
        }
        Err(catalog_identity::Error::Sqlite(_)) => return failed(None, "could not be read"),
        Err(_) => return failed(None, "has an identity table PSF Guard cannot read"),
    };
    let identity =
        saved.unwrap_or_else(|| super::derived_identity(instance, &catalog.database_path));
    let evidence = match catalog_discovery::read_evidence(&tx) {
        Ok(evidence) => evidence,
        Err(DiscoveryError::UnsupportedSchema) => {
            return Read::Unusable {
                saved,
                cause: "has no Target Scheduler project table",
            }
        }
        Err(DiscoveryError::TooManyProjects) => {
            return Read::Unusable {
                saved,
                cause: "holds more than 4096 projects, more than planning reads, so none of them are taken in or counted",
            }
        }
        Err(DiscoveryError::Api(Error::Busy)) => return failed(Some(identity), "is busy"),
        Err(_) => return failed(Some(identity), "could not be read"),
    };
    let rows = evidence
        .identified_projects()
        .map(|(guid, row, name, profile)| SourceRow {
            guid,
            row,
            name: name.map(str::to_owned),
            profile: profile.to_owned(),
        })
        .collect();
    let (targets, facts) = match (target_progress(&tx), project_facts(&tx)) {
        (Ok(targets), Ok(facts)) => (targets, facts),
        (Err(error), _) | (_, Err(error)) => {
            tracing::warn!(%error, catalog = %catalog.name, "Plan list could not count frames");
            notes.push(format!(
                "{}: its targets and frames could not be read, so its plans show no progress",
                catalog.name
            ));
            Default::default()
        }
    };
    let newest_frame = tx
        .query_row("SELECT MAX(Id) FROM acquiredimage", [], |row| {
            row.get::<_, Option<i64>>(0)
        })
        .unwrap_or(None);
    Read::Done {
        saved,
        contents: Contents {
            rows,
            targets,
            facts,
            newest_frame,
        },
    }
}

/// Read every registered database, with no gate held, in slug order so the
/// choice among copies is stable. A busy file is shown as the last listing
/// read it, and a hand-made copy of another file is named and left out.
fn read_sources(
    service: &Service,
    catalogs: &[Arc<DatabaseContext>],
    warnings: &mut Vec<String>,
) -> Vec<Source> {
    let mut sorted: Vec<&Arc<DatabaseContext>> = catalogs.iter().collect();
    sorted.sort_by(|a, b| a.id.cmp(&b.id));
    let mut sources: Vec<Source> = Vec::new();
    for catalog in sorted {
        let derived = || super::derived_identity(service.instance_id, &catalog.database_path);
        let path = &catalog.database_path;
        let source = match read_contents(service.instance_id, catalog, warnings) {
            Read::Done { saved, contents } => {
                let identity = saved.unwrap_or_else(derived);
                let contents = Arc::new(contents);
                if let Ok(mut memo) = service.plans_memo.lock() {
                    memo.reads
                        .insert(path.clone(), (identity, contents.clone()));
                }
                Source {
                    catalog: catalog.clone(),
                    identity,
                    unwritten: saved.is_none(),
                    contents: Some(contents),
                    fresh: true,
                }
            }
            Read::Unusable { saved, cause } => {
                warnings.push(format!("{}: {cause}", catalog.name));
                Source {
                    catalog: catalog.clone(),
                    identity: saved.unwrap_or_else(derived),
                    unwritten: false,
                    contents: None,
                    fresh: false,
                }
            }
            Read::Failed { identity, cause } => {
                // The last read stands in only for the same catalog.
                let last = service
                    .plans_memo
                    .lock()
                    .ok()
                    .and_then(|memo| memo.reads.get(path).cloned())
                    .filter(|(seen, _)| identity.is_none_or(|identity| identity == *seen));
                warnings.push(match &last {
                    Some(_) => format!(
                        "{}: {cause}, so the list shows what it held at the last listing",
                        catalog.name
                    ),
                    None => format!(
                        "{}: {cause}, so its plans show no rows or progress until it can be read",
                        catalog.name
                    ),
                });
                let (identity, contents) = match last {
                    Some((identity, contents)) => (identity, Some(contents)),
                    None => (identity.unwrap_or_else(derived), None),
                };
                Source {
                    catalog: catalog.clone(),
                    identity,
                    unwritten: false,
                    contents,
                    fresh: false,
                }
            }
        };
        if let Some(first) = sources
            .iter()
            .find(|seen| seen.identity.id == source.identity.id)
        {
            warnings.push(super::copy_warning(&catalog.name, &first.catalog.name));
            continue;
        }
        sources.push(source);
    }
    // A database taken out of the registry is not shown again.
    if let Ok(mut memo) = service.plans_memo.lock() {
        memo.reads.retain(|path, _| {
            catalogs
                .iter()
                .any(|catalog| &catalog.database_path == path)
        });
    }
    sources
}

/// A plan, new or not, whose drafts are still to be taken from its
/// Target Scheduler rows.
struct Import {
    source: usize,
    rig: Uuid,
    guid: Uuid,
    /// The plan; for a new one, the id proposed for it.
    project: Uuid,
    new_plan: bool,
    row: i64,
    label: String,
    missing: Missing,
    /// A plan an import made and nobody has edited since: read again, it
    /// takes in what changed in Target Scheduler.
    follow: Option<import_drafts::Follow>,
}

/// What a listing has to record, found on a pooled reader.
#[derive(Default)]
struct Work {
    /// Sources read now with no rig yet.
    unbound: Vec<usize>,
    /// Bound sources whose file still lacks its identity table.
    unwritten: Vec<usize>,
    /// Rigs with no optics yet, and the profile each has.
    optics: Vec<(usize, Uuid, Option<RigProfile>)>,
    /// Per source, its rig's field when the rig's optics are known.
    panels: HashMap<usize, Option<PanelSize>>,
    /// Per source and rig, projects with no plan there yet.
    plans: Vec<(usize, Uuid, Vec<SourceProject>)>,
    imports: Vec<Import>,
    warnings: Vec<String>,
}

fn field_of(profile: Option<&RigProfile>) -> Option<PanelSize> {
    profile
        .and_then(|profile| profile.optics.as_ref())
        .and_then(|optics| optics.value.field_of_view().ok())
        .map(|fov| PanelSize {
            width_degrees: fov.width_degrees,
            height_degrees: fov.height_degrees,
        })
}

/// The drafts a plan has none of. One that cannot be read is left alone;
/// the list names it.
fn missing_drafts(store: &MetaStore, project: Uuid) -> Option<Missing> {
    let missing = Missing {
        framing: matches!(store.framing_draft(project), Ok(None)),
        plan: matches!(store.plan_draft(project), Ok(None)),
    };
    (missing.framing || missing.plan).then_some(missing)
}

/// A plan's name from its project's: the project name, or "Project".
fn plan_name(name: Option<&str>) -> String {
    let label = name.unwrap_or("Project").trim();
    if label.is_empty() { "Project" } else { label }.to_owned()
}

fn find_work(store: &MetaStore, sources: &[Source], management: bool) -> Work {
    let mut work = Work::default();
    for (index, source) in sources.iter().enumerate() {
        let Some(contents) = source.contents.as_ref().filter(|_| source.fresh) else {
            continue;
        };
        let name = &source.catalog.name;
        let catalog = source.identity.id;
        let binding = match store.catalog_rig(catalog) {
            Ok(Some(binding)) => binding,
            Ok(None) => {
                work.unbound.push(index);
                continue;
            }
            Err(error) => {
                work.warnings
                    .push(format!("{name}: its rig could not be read ({error})"));
                continue;
            }
        };
        if source.unwritten && management {
            work.unwritten.push(index);
        }
        let rig = binding.rig.id;
        // A rig with no optics yet takes them from its own frames, so the
        // framing view can draw its rectangle at once; drafts wait for them.
        let profile = match store.rig_profile(rig) {
            Ok(profile) => Some(profile),
            Err(error) => {
                work.warnings.push(format!(
                    "{name}: its rig profile could not be read ({error})"
                ));
                None
            }
        };
        if let Some(profile) = &profile {
            work.panels.insert(index, field_of(profile.as_ref()));
            if profile.as_ref().is_none_or(|p| p.optics.is_none()) {
                work.optics.push((index, rig, profile.clone()));
            }
        }
        let mut new = Vec::new();
        for row in &contents.rows {
            match store.linked_project(catalog, row.guid) {
                Ok(Some(_)) => continue,
                Ok(None) => {}
                Err(error) => {
                    work.warnings.push(format!(
                        "{name}: its project links could not be read ({error})"
                    ));
                    break;
                }
            }
            let label = plan_name(row.name.as_deref());
            let (project, missing) = match store.project_for_source_guid(row.guid) {
                Ok(Some(existing)) => (existing, missing_drafts(store, existing)),
                Ok(None) => (
                    Uuid::new_v4(),
                    Some(Missing {
                        framing: true,
                        plan: true,
                    }),
                ),
                Err(error) => {
                    work.warnings.push(format!(
                        "{name}: its project links could not be read ({error})"
                    ));
                    break;
                }
            };
            new.push(SourceProject {
                source_project_guid: row.guid,
                source_profile_id: row.profile.clone(),
                proposed_project_id: project,
                name: label.clone(),
            });
            if let Some(missing) = missing.filter(|_| profile.is_some()) {
                work.imports.push(Import {
                    source: index,
                    rig,
                    guid: row.guid,
                    project,
                    new_plan: true,
                    row: row.row,
                    label,
                    missing,
                    follow: None,
                });
            }
        }
        if !new.is_empty() {
            work.plans.push((index, rig, new));
        }
        if profile.is_none() {
            continue;
        }
        // What Target Scheduler already holds for a linked project is its
        // plan: take it in as drafts the first time, and again after a
        // change there until someone edits or activates the plan.
        let rows: HashMap<Uuid, &SourceRow> = contents.rows.iter().map(|r| (r.guid, r)).collect();
        let mut after = None;
        loop {
            let page = match store.catalog_project_mappings(catalog, after, 256) {
                Ok(page) => page,
                Err(_) => break, // the list names it
            };
            for mapping in &page.items {
                let Some(row) = rows.get(&mapping.source_project_guid) else {
                    continue;
                };
                let (missing, follow) = match missing_drafts(store, mapping.project_id) {
                    Some(missing) => (missing, None),
                    None => match import_drafts::following(
                        store,
                        mapping.project_id,
                        catalog,
                        mapping.source_project_guid,
                    ) {
                        Some(follow) => (
                            Missing {
                                framing: true,
                                plan: true,
                            },
                            Some(follow),
                        ),
                        None => continue,
                    },
                };
                work.imports.push(Import {
                    source: index,
                    rig,
                    guid: row.guid,
                    project: mapping.project_id,
                    new_plan: false,
                    row: row.row,
                    label: plan_name(row.name.as_deref()),
                    missing,
                    follow,
                });
            }
            match page.next_after {
                Some(next) => after = Some(next),
                None => break,
            }
        }
    }
    work
}

/// Make each new database a rig, under the rig-database gate, then give each
/// file the identity it is planned under. The gate and the writer are each
/// held only for their own step.
async fn bind(
    service: &Arc<Service>,
    sources: &Arc<Vec<Source>>,
    unbound: Vec<usize>,
    unwritten: Vec<usize>,
    management: bool,
    warnings: &mut Vec<String>,
) -> Result<(), Error> {
    let later = |indices: &[usize]| {
        let names: Vec<&str> = indices
            .iter()
            .map(|i| sources[*i].catalog.name.as_str())
            .collect();
        format!(
            "{}: busy, so planning takes {} in on a later listing",
            names.join(", "),
            if names.len() == 1 { "it" } else { "them" }
        )
    };
    let permit = match admit(&service.discovery_admission).await {
        Ok(permit) => permit,
        Err(Error::Busy) => {
            warnings.push(later(&[unbound.as_slice(), unwritten.as_slice()].concat()));
            return Ok(());
        }
        Err(error) => return Err(error),
    };
    let mut written = unwritten;
    if !unbound.is_empty() {
        let shared = sources.clone();
        let waiting = unbound.clone();
        match service
            .clone()
            .with_writer(move |store| {
                let mut bound = Vec::new();
                let mut warnings = Vec::new();
                for index in unbound {
                    let source = &shared[index];
                    // Another listing may have bound it since this one looked.
                    let done = matches!(store.catalog_rig(source.identity.id), Ok(Some(_)))
                        || store
                            .bind_catalog_rig_after(
                                source.identity,
                                &source.catalog.name,
                                source.unwritten,
                                || Ok(()),
                            )
                            .map_err(|error| {
                                warnings.push(format!(
                                    "{}: could not be bound to a rig ({error})",
                                    source.catalog.name
                                ))
                            })
                            .is_ok();
                    if done {
                        bound.push(index);
                    }
                }
                Ok::<_, Error>((bound, warnings))
            })
            .await
        {
            Ok((bound, notes)) => {
                warnings.extend(notes);
                written.extend(bound.into_iter().filter(|i| sources[*i].unwritten));
            }
            Err(Error::Busy) => warnings.push(later(&waiting)),
            Err(error) => return Err(error),
        }
    }
    // Without database management the file is only read: it stays planned
    // under its derived identity until a managing server lists it.
    if !management || written.is_empty() {
        return Ok(());
    }
    let shared = sources.clone();
    let notes = blocking(move || {
        let _permit = permit;
        written
            .into_iter()
            .filter_map(|index| {
                let source = &shared[index];
                write_identity(&source.catalog, source.identity).err()
            })
            .collect::<Vec<_>>()
    })
    .await?;
    warnings.extend(notes);
    Ok(())
}

/// Write the identity a database is planned under into its file, in one
/// short transaction committed at once, so N.I.N.A. never waits behind it.
fn write_identity(catalog: &DatabaseContext, identity: CatalogIdentity) -> Result<(), String> {
    let failed = |busy: bool| {
        if busy {
            format!(
                "{}: busy, so its identity is written on a later listing",
                catalog.name
            )
        } else {
            format!("{}: its identity could not be written", catalog.name)
        }
    };
    let mut connection = super::super::database_context::open_scheduler_connection_with_flags(
        FilePath::new(&catalog.database_path),
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| failed(false))?;
    connection
        .busy_timeout(Duration::from_secs(2))
        .map_err(|_| failed(false))?;
    let mut tx = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| failed(is_busy(&error)))?;
    catalog_identity::adopt(&mut tx, identity).map_err(|error| {
        failed(matches!(&error, catalog_identity::Error::Sqlite(error) if is_busy(error)))
    })?;
    tx.commit().map_err(|error| failed(is_busy(&error)))
}

/// What a listing found in frame headers and Target Scheduler rows, ready
/// for the writer.
#[derive(Default)]
struct Prepared {
    /// Per rig: the profile with header optics and site, and the revision it
    /// replaces.
    profiles: Vec<(Uuid, RigProfile, u64)>,
    /// Header searches that found something, recorded once it is saved.
    searched: Vec<(Uuid, Option<i64>)>,
    plans: Vec<(usize, Uuid, Vec<SourceProject>)>,
    drafts: Vec<(Import, Drafts)>,
    warnings: Vec<String>,
}

impl Prepared {
    fn writes(&self) -> bool {
        !self.profiles.is_empty() || !self.plans.is_empty() || !self.drafts.is_empty()
    }
}

/// Header optics for rigs that have none, and drafts for plans that have
/// none, read with no gate held. The directory tree a header search needs
/// can be a full scan the first time.
fn prepare(service: &Service, sources: &[Source], work: Work) -> Prepared {
    let mut prepared = Prepared {
        plans: work.plans,
        ..Default::default()
    };
    let newest = |index: usize| {
        sources[index]
            .contents
            .as_ref()
            .and_then(|c| c.newest_frame)
    };
    // A rig whose headers named no optics is searched again only once
    // more frames arrive.
    let mut optics: HashMap<usize, (Uuid, Option<RigProfile>)> = {
        let memo = service.plans_memo.lock().ok();
        work.optics
            .into_iter()
            .filter(|(index, rig, _)| {
                memo.as_ref()
                    .is_none_or(|memo| memo.header_searches.get(rig) != Some(&newest(*index)))
            })
            .map(|(index, rig, profile)| (index, (rig, profile)))
            .collect()
    };
    let mut indices: Vec<usize> = optics
        .keys()
        .copied()
        .chain(work.imports.iter().map(|import| import.source))
        .collect();
    indices.sort_unstable();
    indices.dedup();
    let mut imports: Vec<Import> = work.imports;
    for index in indices {
        let source = &sources[index];
        let catalog = &source.catalog;
        let connection = super::super::database_context::open_scheduler_connection_with_flags(
            FilePath::new(&catalog.database_path),
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .and_then(|connection| {
            connection.busy_timeout(Duration::from_secs(1))?;
            Ok(connection)
        });
        let Ok(connection) = connection else {
            prepared.warnings.push(format!(
                "{}: could not be opened, so its new plans get their drafts on a later listing",
                catalog.name
            ));
            imports.retain(|import| import.source != index);
            continue;
        };
        #[cfg(test)]
        hold(service);
        let mut panel = work.panels.get(&index).copied().flatten();
        if let Some((rig, profile)) = optics.remove(&index) {
            let newest = newest(index);
            let defaults = rig_profile::header_defaults(catalog, &connection);
            if defaults.optics.is_some() || defaults.site.is_some() {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                let revision = profile.as_ref().map_or(0, |p| p.revision);
                let mut next = profile.unwrap_or_else(|| RigProfile::empty(rig, now));
                if next.optics.is_none() {
                    next.optics = defaults.optics;
                }
                if next.site.is_none() {
                    next.site = defaults.site;
                }
                next.updated_at_ms = now;
                panel = panel.or(field_of(Some(&next)));
                prepared.profiles.push((rig, next, revision));
                prepared.searched.push((rig, newest));
            } else if let Ok(mut memo) = service.plans_memo.lock() {
                memo.header_searches.insert(rig, newest);
            }
        }
        let (mine, rest): (Vec<Import>, Vec<Import>) = std::mem::take(&mut imports)
            .into_iter()
            .partition(|import| import.source == index);
        imports = rest;
        // One read of the templates serves every plan in the database.
        let templates = match mine
            .iter()
            .any(|import| import.missing.plan)
            .then(|| super::plan::read_templates(&connection))
        {
            Some(Ok(templates)) => Some(templates),
            Some(Err(error)) => {
                tracing::warn!(?error, catalog = %catalog.name, "Plan list could not read templates");
                prepared.warnings.push(format!(
                    "{}: its exposure templates could not be read, so its new plans get their plan drafts on a later listing",
                    catalog.name
                ));
                None
            }
            None => None,
        };
        for import in mine {
            match import_drafts::read_drafts(
                &connection,
                templates.as_ref(),
                import.rig,
                import.project,
                import.row,
                &import.label,
                panel,
                import.missing,
            ) {
                Ok(mut drafts) => {
                    // A plan read again was warned about when it came in.
                    let warnings = std::mem::take(&mut drafts.warnings);
                    if import.follow.is_none() {
                        prepared.warnings.extend(
                            warnings
                                .into_iter()
                                .map(|warning| format!("{}: {warning}", catalog.name)),
                        );
                    }
                    if drafts.framing.is_some() || drafts.plan.is_some() {
                        prepared.drafts.push((import, drafts));
                    }
                }
                Err(error) => prepared.warnings.push(format!(
                    "{}: {} could not be imported into Director ({error})",
                    catalog.name, import.label
                )),
            }
        }
    }
    prepared
}

/// A test holds a listing here, with no gate held and no rig database
/// locked, to show what runs beside it.
#[cfg(test)]
fn hold(service: &Service) {
    let hold = service
        .plans_memo
        .lock()
        .ok()
        .and_then(|mut memo| memo.hold.take());
    if let Some((held, release)) = hold {
        let _ = held.send(());
        let _ = release.recv();
    }
}

/// Record what the listing found, in one turn at the writer: header optics,
/// new plans with their links, and first drafts. Answers the rigs whose
/// header optics are now saved, and what could not be recorded.
fn record(
    store: &mut MetaStore,
    sources: &[Source],
    prepared: Prepared,
) -> (Vec<Uuid>, Vec<String>) {
    let mut warnings = Vec::new();
    let mut saved = Vec::new();
    for (rig, profile, revision) in prepared.profiles {
        match store.save_rig_profile(&profile, revision) {
            // An operator's edit since the read wins.
            Ok(_) | Err(StoreError::Conflict) => saved.push(rig),
            Err(error) => {
                tracing::warn!(?error, %rig, "Header optics were not saved to the rig profile")
            }
        }
    }
    let mut resolved: HashMap<(usize, Uuid), Uuid> = HashMap::new();
    for (index, rig, projects) in prepared.plans {
        let source = &sources[index];
        for chunk in projects.chunks(256) {
            match store.adopt_source_projects(source.identity.id, rig, chunk) {
                Ok(ids) => {
                    for (project, id) in chunk.iter().zip(ids) {
                        resolved.insert((index, project.source_project_guid), id);
                    }
                }
                Err(error) => warnings.push(format!(
                    "{}: could not take in its projects ({error})",
                    source.catalog.name
                )),
            }
        }
    }
    for (mut import, drafts) in prepared.drafts {
        let name = &sources[import.source].catalog.name;
        if let Some(follow) = import.follow.take() {
            if let Err(error) = import_drafts::refresh_drafts(store, follow, drafts) {
                warnings.push(format!(
                    "{name}: {} could not take in its Target Scheduler changes ({error})",
                    import.label
                ));
            }
            continue;
        }
        let project = if import.new_plan {
            match resolved.get(&(import.source, import.guid)) {
                Some(project) => *project,
                None => continue,
            }
        } else {
            import.project
        };
        // A plan made whole from its project follows it from now on.
        let source = (import.missing.framing && import.missing.plan)
            .then_some((sources[import.source].identity.id, import.guid));
        match import_drafts::save_drafts(store, project, drafts, source) {
            Ok(imported) if imported.separate_targets > 1 => warnings.push(format!(
                "{name}: {} has {} separate targets; its plan frames the first. Target Scheduler keeps running the others.",
                import.label, imported.separate_targets
            )),
            Ok(_) => {}
            Err(error) => warnings.push(format!(
                "{name}: {} could not be imported into Director ({error})",
                import.label
            )),
        }
    }
    (saved, warnings)
}

/// A framing summary from the first linked target with coordinates: its
/// center and rotation, the rig's field as the panel when the rig profile
/// holds optics, and every target of that database counted as a panel.
fn catalog_framing(
    panels: &HashMap<Uuid, Option<PanelSize>>,
    links: &[PlanLink],
) -> Option<FramingSummary> {
    for link in links {
        let Some(target) = link.targets.iter().find(|t| t.center.is_some()) else {
            continue;
        };
        let center = target.center.expect("filtered on Some");
        let panel = panels.get(&link.rig.id).copied().flatten();
        return Some(FramingSummary {
            source: "catalog",
            revision: 0,
            target_name: target.name.clone(),
            panels: link.targets.len() as u32,
            panel_rig_id: panel.is_some().then_some(link.rig.id),
            center,
            position_angle_degrees: target.rotation_degrees.unwrap_or(0.0).rem_euclid(360.0),
            panel,
            mosaic: Mosaic {
                rows: 1,
                columns: 1,
                overlap_percent: 20,
            },
            survey_id: "dss2_color".to_owned(),
            extent: panel,
        });
    }
    None
}

/// The list itself, from a pooled reader and what the databases held. A
/// record that cannot be read is named in the warnings and left out of its
/// row; it never fails the list.
fn assemble(
    store: &MetaStore,
    sources: &[Source],
    mut warnings: Vec<String>,
) -> Result<PlanList, Error> {
    let mut links: BTreeMap<Uuid, Vec<PlanLink>> = BTreeMap::new();
    let mut panels: HashMap<Uuid, Option<PanelSize>> = HashMap::new();
    let mut total = 0;
    'sources: for source in sources {
        let name = &source.catalog.name;
        let binding = match store.catalog_rig(source.identity.id) {
            Ok(Some(binding)) => binding,
            Ok(None) => continue,
            Err(error) => {
                warnings.push(format!("{name}: its rig could not be read ({error})"));
                continue;
            }
        };
        let rig = binding.rig;
        match store.rig_profile(rig.id) {
            Ok(profile) => {
                panels.insert(rig.id, field_of(profile.as_ref()));
            }
            Err(error) => warnings.push(format!(
                "{name}: its rig profile could not be read ({error})"
            )),
        }
        let rows: HashMap<Uuid, &SourceRow> = source
            .contents
            .iter()
            .flat_map(|contents| contents.rows.iter())
            .map(|row| (row.guid, row))
            .collect();
        let mut after = None;
        loop {
            let page = match store.catalog_project_mappings(source.identity.id, after, 256) {
                Ok(page) => page,
                Err(error) => {
                    warnings.push(format!(
                        "{name}: its project links could not be read ({error})"
                    ));
                    continue 'sources;
                }
            };
            for mapping in &page.items {
                if total == MAX_LINKS {
                    warnings.push(format!(
                        "Only the first {MAX_LINKS} database links are listed; plans in {name} and the databases after it may show without them."
                    ));
                    break 'sources;
                }
                total += 1;
                let row = rows.get(&mapping.source_project_guid);
                let facts = source
                    .contents
                    .as_ref()
                    .zip(row)
                    .and_then(|(contents, row)| contents.facts.get(&row.row));
                links.entry(mapping.project_id).or_default().push(PlanLink {
                    catalog_slug: source.catalog.id.clone(),
                    catalog_name: name.clone(),
                    rig: rig.clone(),
                    source_project_guid: mapping.source_project_guid,
                    source_row_id: row.map(|row| row.row),
                    source_name: row.and_then(|row| row.name.clone()),
                    source_state: facts.and_then(|f| f.state),
                    earliest_capture_s: facts.and_then(|f| f.earliest_capture_s),
                    latest_capture_s: facts.and_then(|f| f.latest_capture_s),
                    targets: source
                        .contents
                        .as_ref()
                        .zip(row)
                        .and_then(|(contents, row)| contents.targets.get(&row.row))
                        .cloned()
                        .unwrap_or_default(),
                    source_unread: source.contents.is_none(),
                });
            }
            match page.next_after {
                Some(next) => after = Some(next),
                None => break,
            }
        }
    }
    let mut projects = Vec::new();
    let mut after = None;
    loop {
        let page = store.projects(after, 256)?;
        projects.extend(page.items);
        match page.next_after {
            Some(next) if projects.len() < MAX_PLANS => after = Some(next),
            Some(_) => {
                warnings.push(format!("Only the first {MAX_PLANS} plans are listed."));
                break;
            }
            None => break,
        }
    }
    projects.truncate(MAX_PLANS);
    // Databases come from a map; name order keeps the first link stable.
    for entries in links.values_mut() {
        entries.sort_by(|a, b| {
            a.catalog_name
                .cmp(&b.catalog_name)
                .then_with(|| a.source_row_id.cmp(&b.source_row_id))
        });
    }
    let mut rows = Vec::with_capacity(projects.len());
    for project in projects {
        let unreadable = |record: &str, error: StoreError| {
            format!(
                "{}: its {record} could not be read, so the list leaves it out ({error})",
                project.name
            )
        };
        let links = links.remove(&project.id).unwrap_or_default();
        let framing = match store.framing_draft(project.id) {
            Ok(Some(draft)) => {
                let preview = draft.panel.and_then(|panel| {
                    FramingRequest {
                        center: draft.center,
                        position_angle_degrees: draft.position_angle_degrees,
                        panel,
                        mosaic: draft.mosaic,
                        overlays: vec![],
                        view: None,
                    }
                    .preview()
                    .ok()
                });
                Some(FramingSummary {
                    source: "draft",
                    revision: draft.revision,
                    target_name: draft.target_name.clone(),
                    panels: preview
                        .as_ref()
                        .map(|p| p.panels.len() as u32)
                        // No panel size yet; the grid still says how many.
                        .unwrap_or(draft.mosaic.rows * draft.mosaic.columns),
                    panel_rig_id: draft.panel_rig_id,
                    center: draft.center,
                    position_angle_degrees: draft.position_angle_degrees,
                    panel: draft.panel,
                    mosaic: draft.mosaic,
                    survey_id: draft.survey_id.clone(),
                    extent: preview.map(|p| p.extent),
                })
            }
            // No draft yet, but Target Scheduler already points somewhere:
            // that is the framing until Director saves one of its own.
            Ok(None) => catalog_framing(&panels, &links),
            Err(error) => {
                warnings.push(unreadable("framing draft", error));
                None
            }
        };
        let plan = match store.plan_draft(project.id) {
            Ok(plan) => plan.map(|plan| PlanSummary {
                revision: plan.revision,
                objectives: plan.objectives.len() as u32,
                rigs: plan
                    .contributions
                    .iter()
                    .filter(|c| c.enabled)
                    .map(|c| c.rig_id)
                    .collect::<std::collections::BTreeSet<_>>()
                    .len() as u32,
            }),
            Err(error) => {
                warnings.push(unreadable("plan draft", error));
                None
            }
        };
        let activation = match store.activation(project.id) {
            Ok(activation) => activation.map(|a| ActivationSummary {
                revision: a.revision,
                applied_at_ms: a.applied_at_ms,
                rigs: a.rigs.len() as u32,
            }),
            Err(error) => {
                warnings.push(unreadable("last activation", error));
                None
            }
        };
        let targets: Vec<&TargetProgress> = links.iter().flat_map(|l| &l.targets).collect();
        let progress = (!targets.is_empty()).then(|| Progress {
            desired: targets.iter().map(|t| t.desired).sum(),
            acquired: targets.iter().map(|t| t.acquired).sum(),
            accepted: targets.iter().map(|t| t.accepted).sum(),
            rejected: targets.iter().map(|t| t.rejected).sum(),
            targets: targets.len() as u32,
        });
        rows.push(PlanRow {
            links,
            progress,
            project,
            framing,
            plan,
            activation,
        });
    }
    Ok(PlanList { rows, warnings })
}

/// Every registered database is a rig and every Target Scheduler project in
/// it is a plan. Projects that share a GUID across databases, as Sync copies
/// do, become one plan with several rigs; projects that merely share a name
/// stay apart. A database that cannot be read or written, or has no usable
/// GUIDs, is reported, not failed.
///
/// Without database management the file is only read: it is planned under
/// its derived identity, and the identity table is written the first time a
/// managing server lists it, under the same id.
pub(super) async fn list(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<PlanList>>, Error> {
    let service = enabled(&state)?;
    let management = state.database_management_allowed();
    let catalogs: Vec<_> = state
        .databases
        .read()
        .map_err(|_| Error::Internal)?
        .values()
        .cloned()
        .collect();
    let (sources, mut warnings) = blocking({
        let service = service.clone();
        move || {
            let mut warnings = Vec::new();
            let sources = read_sources(&service, &catalogs, &mut warnings);
            (Arc::new(sources), warnings)
        }
    })
    .await?;
    let find = |sources: Arc<Vec<Source>>| {
        service
            .clone()
            .with_reader(move |store| Ok::<_, Error>(find_work(store, &sources, management)))
    };
    let mut work = find(sources.clone()).await?;
    if !work.unbound.is_empty() || !work.unwritten.is_empty() {
        let rebound = !work.unbound.is_empty();
        let (unbound, unwritten) = (
            std::mem::take(&mut work.unbound),
            std::mem::take(&mut work.unwritten),
        );
        bind(
            &service,
            &sources,
            unbound,
            unwritten,
            management,
            &mut warnings,
        )
        .await?;
        if rebound {
            // A new rig's projects become plans in the same listing. One that
            // still has no rig has its warning already.
            work = find(sources.clone()).await?;
        }
    }
    warnings.append(&mut work.warnings);
    if !work.optics.is_empty() || !work.plans.is_empty() || !work.imports.is_empty() {
        let mut prepared = blocking({
            let (service, sources) = (service.clone(), sources.clone());
            move || prepare(&service, &sources, work)
        })
        .await?;
        warnings.append(&mut prepared.warnings);
        if prepared.writes() {
            let searched = std::mem::take(&mut prepared.searched);
            let shared = sources.clone();
            match service
                .clone()
                .with_writer(move |store| Ok::<_, Error>(record(store, &shared, prepared)))
                .await
            {
                Ok((saved, notes)) => {
                    warnings.extend(notes);
                    if let Ok(mut memo) = service.plans_memo.lock() {
                        for (rig, newest) in searched {
                            if saved.contains(&rig) {
                                memo.header_searches.insert(rig, newest);
                            }
                        }
                    }
                }
                Err(Error::Busy) => warnings.push(
                    "Director metadata is busy, so new projects are taken in on a later listing."
                        .to_owned(),
                ),
                Err(error) => return Err(error),
            }
        }
    }
    let list = service
        .clone()
        .with_reader(move |store| assemble(store, &sources, warnings))
        .await?;
    Ok(Json(ApiResponse::success(list)))
}
