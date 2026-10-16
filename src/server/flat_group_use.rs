//! What each flat group feeds: the recorded flat masters built from it,
//! and the lights whose flat master it is, counted by how their flats were
//! chosen. The Calibration page's group detail reads it.
//!
//! Each light is matched the way a stack matches it
//! ([`crate::calibration::project_scope::master_frames`]), from the headers
//! the calibration health pass keeps: no request reads a header, and a
//! light the pass has not reached is counted as pending. Groups come from
//! the flat-groups view itself ([`super::flat_groups::build_view`]), joined
//! through the Calibration page's sessions, so this follows however the
//! groups are formed.
//!
//! Matching runs on a read-only connection of its own, off the async
//! workers, and the result is kept in memory under a revision made of each
//! project's health key and kept headers. When the revision moves on, the
//! last result is served, marked `refreshing`, while one background refresh
//! works out the next.

use crate::calibration::browse::{self, LibraryFrames, RecordedLibraryMaster};
use crate::calibration::project_scope::{master_frames, MasterFrames};
use crate::calibration::{CalibrationKind, LibrarySnapshot};
use crate::commands::import::headers::FrameMeta;
use crate::server::api::ApiResponse;
use crate::server::database_context::DatabaseContext;
use crate::server::extract::DbContext;
use crate::server::flat_groups::FlatGroupsView;
use crate::server::handlers::AppError;
use axum::Json;
use rusqlite::{Connection, OpenFlags};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex};

/// Lights by how their flats were chosen.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct LightCounts {
    pub total: usize,
    /// The nearest flats, or its own side of each change.
    pub own: usize,
    /// None at its angle on its side: this group's flats at another angle.
    pub other_angle: usize,
    /// Carried across a change by the change's map.
    pub corrected: usize,
    /// Shot during a change, side unknown: the nearest flats, flagged.
    pub undetermined: usize,
    /// Every flat from the other side of a change, flagged.
    pub no_same_side: usize,
}

impl LightCounts {
    fn add(&mut self, basis: &str) {
        self.total += 1;
        match basis {
            "other_angle" => self.other_angle += 1,
            "corrected" => self.corrected += 1,
            "undetermined" => self.undetermined += 1,
            "no_same_side" => self.no_same_side += 1,
            _ => self.own += 1,
        }
    }
}

/// One project's lights that take a group's flats.
#[derive(Debug, Clone, Serialize)]
pub struct ProjectLights {
    pub project_id: i32,
    pub name: Option<String>,
    pub lights: LightCounts,
    /// The lights' filters, as Images names them.
    pub filters: Vec<String>,
    /// Unix seconds: the first and last of those lights.
    pub first: Option<i64>,
    pub last: Option<i64>,
}

/// A recorded flat master with sources in a group.
#[derive(Debug, Clone, Serialize)]
pub struct GroupMaster {
    pub id: String,
    pub label: String,
    pub created_at: i64,
    pub sources: usize,
    /// Of its sources, how many are this group's.
    pub in_group: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct GroupUse {
    /// The group's key, as the flat-groups view gives it.
    pub key: String,
    pub masters: Vec<GroupMaster>,
    pub lights: LightCounts,
    pub projects: Vec<ProjectLights>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct FlatGroupUseView {
    /// Groups something uses; a group not listed feeds nothing.
    pub groups: Vec<GroupUse>,
    /// Lights matched.
    pub lights: usize,
    /// Lights the calibration check has not read yet.
    pub pending: usize,
    /// Lights whose file was not found or whose header could not be read.
    pub unread: usize,
    /// Worked out before the latest change; a newer one is on its way.
    pub refreshing: bool,
}

/// One light to count: its project, filter and header.
pub(crate) struct CountedLight<'a> {
    pub project: i32,
    pub filter: &'a str,
    pub meta: &'a FrameMeta,
}

/// Each group's masters and lights. Frames join groups through the
/// browser's sessions the view names.
pub(crate) fn tally<'a>(
    view: &FlatGroupsView,
    library: &LibraryFrames,
    snapshot: &LibrarySnapshot,
    masters: &[RecordedLibraryMaster],
    names: &HashMap<i32, String>,
    lights: impl IntoIterator<Item = CountedLight<'a>>,
) -> (Vec<GroupUse>, usize) {
    let mut group_of_session: HashMap<&str, &str> = HashMap::new();
    for rig in &view.rigs {
        for family in &rig.families {
            for group in &family.groups {
                for session in &group.sessions {
                    for id in &session.sessions {
                        group_of_session.insert(id.as_str(), group.key.as_str());
                    }
                }
            }
        }
    }
    let group_of_frame: HashMap<&str, &str> = library
        .frames
        .iter()
        .filter(|row| row.frame.kind == CalibrationKind::Flat)
        .filter_map(|row| {
            group_of_session
                .get(row.session.as_str())
                .map(|group| (row.frame.frame_uuid.as_str(), *group))
        })
        .collect();
    let groups_of = |uuids: &mut dyn Iterator<Item = &String>| -> BTreeMap<&str, usize> {
        let mut out: BTreeMap<&str, usize> = BTreeMap::new();
        for uuid in uuids {
            if let Some(group) = group_of_frame.get(uuid.as_str()) {
                *out.entry(*group).or_default() += 1;
            }
        }
        out
    };

    #[derive(Default)]
    struct Use {
        masters: Vec<GroupMaster>,
        lights: LightCounts,
        projects: BTreeMap<i32, ProjectLights>,
    }
    let mut uses: BTreeMap<String, Use> = BTreeMap::new();
    for master in masters
        .iter()
        .filter(|master| master.kind == CalibrationKind::Flat)
    {
        for (group, in_group) in groups_of(&mut master.source_uuids.iter()) {
            uses.entry(group.to_string())
                .or_default()
                .masters
                .push(GroupMaster {
                    id: master.master_uuid.clone(),
                    label: master
                        .cache_path
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    created_at: master.created_at,
                    sources: master.source_count,
                    in_group,
                });
        }
    }
    let mut matched = 0;
    for light in lights {
        matched += 1;
        let selected = snapshot.select(light.meta);
        let MasterFrames::Raw(flats) = master_frames(&selected, CalibrationKind::Flat) else {
            continue;
        };
        let basis = selected.flat_basis.name();
        for group in groups_of(&mut flats.iter()).into_keys() {
            let entry = uses.entry(group.to_string()).or_default();
            entry.lights.add(basis);
            let project = entry
                .projects
                .entry(light.project)
                .or_insert_with(|| ProjectLights {
                    project_id: light.project,
                    name: names.get(&light.project).cloned(),
                    lights: LightCounts::default(),
                    filters: Vec::new(),
                    first: None,
                    last: None,
                });
            project.lights.add(basis);
            if !light.filter.is_empty() && !project.filters.iter().any(|f| f == light.filter) {
                project.filters.push(light.filter.to_string());
                project.filters.sort();
            }
            if let Some(at) = light.meta.timestamp {
                project.first = Some(project.first.map_or(at, |first| first.min(at)));
                project.last = Some(project.last.map_or(at, |last| last.max(at)));
            }
        }
    }
    let groups = uses
        .into_iter()
        .map(|(key, entry)| GroupUse {
            key,
            masters: entry.masters,
            lights: entry.lights,
            projects: entry.projects.into_values().collect(),
        })
        .collect();
    (groups, matched)
}

fn read_only(path: &str) -> anyhow::Result<Connection> {
    Ok(
        crate::server::database_context::open_scheduler_connection_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
        )?,
    )
}

/// What a result is worked out from: each project's health key, which
/// covers the library, its grades, flat grouping's measurements, the
/// settings and the project's lights, and when its headers were kept.
fn revision(conn: &Connection, ctx: &DatabaseContext) -> anyhow::Result<String> {
    let mut hasher = Sha256::new();
    for (project, key) in crate::calibration::health_keys(conn)? {
        let stamp = crate::server::calibration_health::headers_stamp(&ctx.cache_dir_path, project);
        hasher.update(format!("{project}\x1f{key}\x1f{stamp}\x1e"));
    }
    Ok(hasher.finalize()[..12]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn work_out(ctx: &DatabaseContext) -> anyhow::Result<FlatGroupUseView> {
    let conn = read_only(&ctx.database_path)?;
    let view = crate::server::flat_groups::build_view(&conn)?;
    if !view.available {
        return Ok(FlatGroupUseView::default());
    }
    let library = browse::library_frames(&conn)?;
    let snapshot = LibrarySnapshot::load(&conn)?;
    let masters = browse::recorded_masters(&conn)?;
    let names = crate::server::calibration_browser::project_names(&conn);
    let tree = ctx.get_directory_tree()?;
    let boundary = crate::calibration::catalog_night_boundary(&conn);
    let mut pending = 0;
    let mut unread = 0;
    let mut kept: Vec<(i32, String, FrameMeta)> = Vec::new();
    for project in crate::calibration::health_keys(&conn)?.into_keys() {
        let (lights, missing) =
            crate::calibration::project_lights(&conn, project, &tree, boundary)?;
        unread += missing;
        let headers = crate::server::calibration_health::kept_headers(&ctx.cache_dir_path, project);
        for light in lights {
            match headers.get(&light.image_id) {
                Some(header) if header.path == light.path && header.meta.readable => {
                    kept.push((project, light.filter, header.meta.clone()));
                }
                Some(header) if header.path == light.path => unread += 1,
                _ => pending += 1,
            }
        }
    }
    let (groups, lights) = tally(
        &view,
        &library,
        &snapshot,
        &masters,
        &names,
        kept.iter().map(|(project, filter, meta)| CountedLight {
            project: *project,
            filter,
            meta,
        }),
    );
    Ok(FlatGroupUseView {
        groups,
        lights,
        pending,
        unread,
        refreshing: false,
    })
}

/// Results by catalog path, with their revision.
static KEPT: LazyLock<Mutex<HashMap<String, Kept>>> = LazyLock::new(Mutex::default);

/// A kept result, with the revision it was worked out at.
type Kept = (String, Arc<FlatGroupUseView>);
/// Catalogs being refreshed in the background.
static REFRESHING: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Mutex::default);
/// One lock per catalog, so requests at once share one working out.
static WORKING: LazyLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> = LazyLock::new(Mutex::default);

fn kept(path: &str) -> Option<Kept> {
    KEPT.lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(path)
        .cloned()
}

/// Work the result out at `revision` and keep it, unless another request
/// did while this one waited. Blocking.
fn work_out_kept(ctx: &DatabaseContext, revision: String) -> anyhow::Result<Arc<FlatGroupUseView>> {
    let lock = {
        let mut locks = WORKING.lock().unwrap_or_else(|p| p.into_inner());
        locks.retain(|_, lock| Arc::strong_count(lock) > 1);
        Arc::clone(locks.entry(ctx.database_path.clone()).or_default())
    };
    let _working = lock.lock().unwrap_or_else(|p| p.into_inner());
    if let Some((kept_revision, view)) = kept(&ctx.database_path)
        && kept_revision == revision
    {
        return Ok(view);
    }
    let view = Arc::new(work_out(ctx)?);
    KEPT.lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(ctx.database_path.clone(), (revision, Arc::clone(&view)));
    Ok(view)
}

fn start_refresh(ctx: &Arc<DatabaseContext>) {
    let path = ctx.database_path.clone();
    if !REFRESHING
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(path.clone())
    {
        return;
    }
    let ctx = Arc::clone(ctx);
    let spawned = std::thread::Builder::new()
        .name("flat-group-use".into())
        .spawn({
            let path = path.clone();
            move || {
                let outcome = read_only(&ctx.database_path)
                    .and_then(|conn| revision(&conn, &ctx))
                    .and_then(|revision| work_out_kept(&ctx, revision));
                if let Err(error) = outcome {
                    tracing::warn!("Could not work out what the flat groups feed: {error:#}");
                }
                REFRESHING
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .remove(&path);
            }
        });
    if spawned.is_err() {
        REFRESHING
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&path);
    }
}

/// The kept result when current, the last one while a refresh runs, or
/// worked out now when there is none. Blocking.
fn use_of(ctx: &Arc<DatabaseContext>) -> anyhow::Result<Arc<FlatGroupUseView>> {
    let revision = revision(&read_only(&ctx.database_path)?, ctx)?;
    match kept(&ctx.database_path) {
        Some((kept_revision, view)) if kept_revision == revision => Ok(view),
        Some((_, view)) => {
            start_refresh(ctx);
            let mut stale = (*view).clone();
            stale.refreshing = true;
            Ok(Arc::new(stale))
        }
        None => work_out_kept(ctx, revision),
    }
}

/// `GET /api/db/{db}/calibrations/flat-groups/use` — each group's flat
/// masters and the lights that take its flats, by project and by how the
/// flats were chosen.
pub async fn get_flat_group_use(
    ctx: DbContext,
) -> Result<Json<ApiResponse<FlatGroupUseView>>, AppError> {
    let ctx = Arc::clone(&ctx.0);
    let view = tokio::task::spawn_blocking(move || use_of(&ctx))
        .await
        .map_err(|error| AppError::InternalError(format!("flat group use task failed: {error}")))?
        .map_err(|error| {
            AppError::InternalError(format!("working out what the flat groups feed: {error:#}"))
        })?;
    Ok(Json(ApiResponse::success((*view).clone())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calibration::test_frames::write_frame;
    use crate::calibration::tests::POLICY_LOCK;

    #[test]
    fn each_group_counts_the_lights_that_take_its_flats_and_the_masters_built_from_it() {
        let _lock = POLICY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let temp = tempfile::tempdir().unwrap();
        // 09-10 and 09-14 split at new dust.
        let conn = crate::server::flat_groups::tests::catalog_of(
            temp.path(),
            &["2026-09-10T14:0", "2026-09-14T14:0"],
            |a, b| if a.night == b.night { 0.0 } else { 0.02 },
        );
        let light = |name: &str, at: &str| {
            let path = temp.path().join(format!("{name}.fits"));
            write_frame(&path, "LIGHT", (32, 32), at, |_, _| 1_000.0);
            crate::commands::import::headers::read_frame_meta(&path)
        };
        // Before the change, after it, and during it with nothing to place it.
        let before = light("before", "2026-09-09T03:00:00");
        let after = light("after", "2026-09-16T03:00:00");
        let during = light("during", "2026-09-12T03:00:00");

        let view = crate::server::flat_groups::build_view(&conn).unwrap();
        let family = &view.rigs[0].families[0];
        let (newer, older) = (family.groups[0].key.clone(), family.groups[1].key.clone());
        let library = browse::library_frames(&conn).unwrap();
        let snapshot = LibrarySnapshot::load(&conn).unwrap();
        // A master of the older night's flats.
        let sources: Vec<String> = library
            .frames
            .iter()
            .filter(|row| row.night.as_deref() == Some("2026-09-10"))
            .map(|row| row.frame.frame_uuid.clone())
            .collect();
        let master = RecordedLibraryMaster {
            master_uuid: "m1".into(),
            kind: CalibrationKind::Flat,
            rig_uuid: library.frames[0].frame.rig_uuid.clone(),
            rig_name: None,
            cache_path: "/cache/flat-m1.fits".into(),
            created_at: 1,
            source_count: sources.len(),
            exposure_s: None,
            filter: None,
            camera_temp: None,
            sources: None,
            statistics: serde_json::Value::Null,
            source_uuids: sources,
        };
        let names = HashMap::from([(7, "M 44".to_string())]);
        let (groups, matched) = tally(
            &view,
            &library,
            &snapshot,
            &[master],
            &names,
            [&before, &after, &during].map(|meta| CountedLight {
                project: 7,
                filter: "Ha",
                meta,
            }),
        );
        assert_eq!(matched, 3);
        let of = |key: &str| groups.iter().find(|group| group.key == key).unwrap();
        let older = of(&older);
        assert_eq!(
            older.lights,
            LightCounts {
                total: 2,
                own: 1,
                undetermined: 1,
                ..Default::default()
            },
            "the light before, and the one during the change keeps the nearest"
        );
        assert_eq!(older.masters.len(), 1);
        assert_eq!(older.masters[0].in_group, 3);
        assert_eq!(older.projects.len(), 1);
        let project = &older.projects[0];
        assert_eq!(project.name.as_deref(), Some("M 44"));
        assert_eq!(project.filters, vec!["Ha"]);
        assert_eq!(
            (project.first, project.last),
            (before.timestamp, during.timestamp)
        );
        let newer = of(&newer);
        assert_eq!((newer.lights.total, newer.lights.own), (1, 1));
        assert!(newer.masters.is_empty());
    }
}
