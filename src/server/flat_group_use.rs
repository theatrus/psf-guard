//! What each flat group feeds: the recorded flat masters built from it,
//! and the lights whose flat master it is, counted by how their flats were
//! chosen. The Calibration page's group detail, and the Images filter for
//! one group's lights, read it.
//!
//! Each light is matched the way a stack matches it
//! ([`crate::calibration::project_scope::master_frames`]), from the headers
//! the calibration health pass keeps: no request reads a header, and a
//! light the pass has not reached is counted as pending. Flats join groups
//! through the measured sessions' own frames
//! ([`super::flat_groups::build_view_with_frames`]), so this follows
//! however the groups are formed.
//!
//! No request works anything out. The answer is kept in memory under a
//! revision of what it depends on ([`revision`]); a request that finds none,
//! or an older one, starts one background refresh and is answered with
//! what is kept, marked `refreshing` (with `computed` false when nothing is
//! kept yet), and the page asks again. The refresh reads its own read-only
//! connection and matches lights on a pool sized at background priority
//! ([`crate::concurrency`]), waiting while interactive work runs.

use crate::calibration::browse::{self, RecordedLibraryMaster};
use crate::calibration::flat_groups::FlatBasis;
use crate::calibration::project_scope::{master_frames, MasterFrames};
use crate::calibration::{CalibrationKind, LibrarySnapshot};
use crate::commands::import::headers::FrameMeta;
use crate::concurrency::{ComputePool, Priority};
use crate::server::api::ApiResponse;
use crate::server::database_context::DatabaseContext;
use crate::server::extract::DbContext;
use crate::server::flat_groups::FrameGroups;
use crate::server::handlers::AppError;
use crate::server::state::AppState;
use axum::extract::{Query, State};
use axum::Json;
use rayon::prelude::*;
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

/// How often a refresh looks again while interactive work runs.
const YIELD_POLL: Duration = Duration::from_millis(250);

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
    fn add(&mut self, basis: &FlatBasis) {
        self.total += 1;
        match basis {
            FlatBasis::Nearest | FlatBasis::SameSide { .. } => self.own += 1,
            FlatBasis::OtherAngle { .. } => self.other_angle += 1,
            FlatBasis::Corrected { .. } => self.corrected += 1,
            FlatBasis::Undetermined { .. } => self.undetermined += 1,
            FlatBasis::NoSameSide { .. } => self.no_same_side += 1,
        }
    }
}

/// Whether a light is flagged for its flats.
fn flagged(basis: &FlatBasis) -> bool {
    match basis {
        FlatBasis::Undetermined { .. } | FlatBasis::NoSameSide { .. } => true,
        FlatBasis::Nearest
        | FlatBasis::SameSide { .. }
        | FlatBasis::OtherAngle { .. }
        | FlatBasis::Corrected { .. } => false,
    }
}

/// One project's lights that take a group's flats.
#[derive(Debug, Clone, Serialize)]
pub struct ProjectLights {
    pub project_id: i32,
    pub name: Option<String>,
    pub lights: LightCounts,
    /// The lights, by id: sent only when one group is asked for, for the
    /// Images filter.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub image_ids: Vec<i32>,
    /// Of them, those flagged for their flats (undetermined, or every flat
    /// from the other side of a change). Sent as `image_ids` is.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub flagged_ids: Vec<i32>,
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
    /// False until the first answer is worked out: nothing below is known.
    pub computed: bool,
    /// Groups something uses; a group not listed feeds nothing.
    pub groups: Vec<GroupUse>,
    /// Lights matched.
    pub lights: usize,
    /// Lights the calibration check has not read yet.
    pub pending: usize,
    /// Lights whose file was not found or whose header could not be read.
    pub unread: usize,
    /// A newer answer is being worked out; this one is the last kept.
    pub refreshing: bool,
}

/// One light, matched against the library.
pub(crate) struct MatchedLight {
    pub project: i32,
    pub image_id: i32,
    /// What its flat master is made from.
    pub flats: MasterFrames,
    pub basis: FlatBasis,
}

/// Match one light the way a stack does.
pub(crate) fn match_light(
    snapshot: &LibrarySnapshot,
    project: i32,
    image_id: i32,
    meta: &FrameMeta,
) -> MatchedLight {
    let selected = snapshot.select(meta);
    MatchedLight {
        project,
        image_id,
        flats: master_frames(&selected, CalibrationKind::Flat),
        basis: selected.flat_basis,
    }
}

/// Each group's masters and lights, from the group each measured flat is
/// in. A light whose master takes flats of two groups counts in both; one
/// calibrated with a master from other software counts in none.
pub(crate) fn tally(
    frame_groups: &FrameGroups,
    masters: &[RecordedLibraryMaster],
    names: &HashMap<i32, String>,
    lights: impl IntoIterator<Item = MatchedLight>,
) -> (Vec<GroupUse>, usize) {
    let groups_of = |uuids: &mut dyn Iterator<Item = &String>| -> BTreeMap<&str, usize> {
        let mut out: BTreeMap<&str, usize> = BTreeMap::new();
        for uuid in uuids {
            if let Some(group) = frame_groups.get(uuid) {
                *out.entry(group.as_str()).or_default() += 1;
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
        let MasterFrames::Raw(flats) = &light.flats else {
            continue;
        };
        for group in groups_of(&mut flats.iter()).into_keys() {
            let entry = uses.entry(group.to_string()).or_default();
            entry.lights.add(&light.basis);
            let project = entry
                .projects
                .entry(light.project)
                .or_insert_with(|| ProjectLights {
                    project_id: light.project,
                    name: names.get(&light.project).cloned(),
                    lights: LightCounts::default(),
                    image_ids: Vec::new(),
                    flagged_ids: Vec::new(),
                });
            project.lights.add(&light.basis);
            project.image_ids.push(light.image_id);
            if flagged(&light.basis) {
                project.flagged_ids.push(light.image_id);
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

/// The catalog's projects, by id; none when it has no project table.
fn project_ids(conn: &Connection) -> Vec<i32> {
    conn.prepare("SELECT Id FROM project ORDER BY Id")
        .and_then(|mut statement| {
            statement
                .query_map([], |row| row.get(0))?
                .collect::<rusqlite::Result<Vec<i32>>>()
        })
        .unwrap_or_default()
}

/// What an answer is worked out from: the library as matching reads it
/// (each frame's identity, file, validity mark and whether it is kept out;
/// the recorded masters; flat grouping's measurements; the settings),
/// every project's lights and names
/// ([`crate::calibration::project_scope::catalog_revision`]), and when each
/// project's headers were kept. Grading the library in the background
/// moves it only where a grade keeps a frame out or lets one back in.
fn revision(conn: &Connection, ctx: &DatabaseContext) -> anyhow::Result<String> {
    let mut hasher = Sha256::new();
    hasher.update(crate::calibration::project_scope::catalog_revision(conn)?);
    for project in project_ids(conn) {
        let stamp = crate::server::calibration_health::headers_stamp(&ctx.cache_dir_path, project);
        hasher.update(format!("\x1e{project}\x1f{stamp}"));
    }
    Ok(hasher.finalize()[..12]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// Work the answer out. Blocking; for the background refresh only.
fn work_out(state: &AppState, ctx: &DatabaseContext) -> anyhow::Result<FlatGroupUseView> {
    let conn = read_only(&ctx.database_path)?;
    let (view, frame_groups) = crate::server::flat_groups::build_view_with_frames(&conn)?;
    if !view.available {
        return Ok(FlatGroupUseView {
            computed: true,
            ..FlatGroupUseView::default()
        });
    }
    let snapshot = LibrarySnapshot::load(&conn)?;
    let masters = browse::recorded_masters(&conn)?;
    let names = crate::server::calibration_browser::project_names(&conn);
    let tree = ctx.get_directory_tree()?;
    let boundary = crate::calibration::catalog_night_boundary(&conn);
    let mut pending = 0;
    let mut unread = 0;
    let mut kept: Vec<(i32, i32, FrameMeta)> = Vec::new();
    for project in project_ids(&conn) {
        let (lights, missing) =
            crate::calibration::project_lights(&conn, project, &tree, boundary)?;
        unread += missing;
        let headers = crate::server::calibration_health::kept_headers(&ctx.cache_dir_path, project);
        for light in lights {
            match headers.get(&light.image_id) {
                Some(header) if header.path == light.path && header.meta.readable => {
                    kept.push((project, light.image_id, header.meta.clone()));
                }
                Some(header) if header.path == light.path => unread += 1,
                _ => pending += 1,
            }
        }
    }
    drop(conn);
    let budget =
        crate::concurrency::plan_workers(None, &state.worker_policy(), Priority::Background, None);
    let lease = state.lease_workers(Priority::Background, budget.workers);
    let pool = ComputePool::take("flat-group-use", lease.workers)?;
    let matched: Vec<MatchedLight> = pool.install(|| {
        kept.par_iter()
            .map(|(project, image_id, meta)| {
                while state.interactive_job_active() && !state.shutdown.is_stopping() {
                    std::thread::sleep(YIELD_POLL);
                }
                anyhow::ensure!(!state.shutdown.is_stopping(), "the server is stopping");
                Ok(match_light(&snapshot, *project, *image_id, meta))
            })
            .collect::<anyhow::Result<_>>()
    })?;
    let (groups, lights) = tally(&frame_groups, &masters, &names, matched);
    Ok(FlatGroupUseView {
        computed: true,
        groups,
        lights,
        pending,
        unread,
        refreshing: false,
    })
}

/// A kept answer, with the revision it was worked out at.
type Kept = (String, Arc<FlatGroupUseView>);

/// Answers by catalog path.
static KEPT: LazyLock<Mutex<HashMap<String, Kept>>> = LazyLock::new(Mutex::default);
/// Catalogs being refreshed in the background.
static REFRESHING: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Mutex::default);

fn kept(path: &str) -> Option<Kept> {
    KEPT.lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(path)
        .cloned()
}

/// Work out the next answer in the background, unless a refresh runs.
fn start_refresh(state: &Arc<AppState>, ctx: &Arc<DatabaseContext>) {
    let path = ctx.database_path.clone();
    if state.shutdown.is_stopping()
        || !REFRESHING
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(path.clone())
    {
        return;
    }
    let (state, ctx) = (Arc::clone(state), Arc::clone(ctx));
    let work = state
        .shutdown
        .hold(format!("flat group use ({})", ctx.database_path));
    let spawned = std::thread::Builder::new()
        .name("flat-group-use".into())
        .spawn({
            let path = path.clone();
            move || {
                let _work = work;
                let outcome = read_only(&ctx.database_path)
                    .and_then(|conn| revision(&conn, &ctx))
                    .and_then(|revision| Ok((revision, work_out(&state, &ctx)?)));
                match outcome {
                    Ok((revision, view)) => {
                        KEPT.lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .insert(path.clone(), (revision, Arc::new(view)));
                    }
                    Err(error) => {
                        tracing::info!("Could not work out what the flat groups feed: {error:#}");
                    }
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

/// The kept answer when current; otherwise the last one, or a pending
/// one, marked `refreshing` while a refresh works out the next. Blocking,
/// but never long: it reads only what the revision needs.
fn use_of(state: &Arc<AppState>, ctx: &Arc<DatabaseContext>) -> anyhow::Result<FlatGroupUseView> {
    let revision = revision(&read_only(&ctx.database_path)?, ctx)?;
    match kept(&ctx.database_path) {
        Some((kept_revision, view)) if kept_revision == revision => Ok((*view).clone()),
        kept => {
            start_refresh(state, ctx);
            let mut stale = kept.map(|(_, view)| (*view).clone()).unwrap_or_default();
            stale.refreshing = true;
            Ok(stale)
        }
    }
}

#[derive(Debug, Default, Deserialize)]
pub struct UseQuery {
    /// Send the lights of this group, by id.
    pub group: Option<String>,
}

/// Leave the lights' ids only on `group`'s projects.
fn keep_ids_of(view: &mut FlatGroupUseView, group: Option<&str>) {
    for entry in &mut view.groups {
        if Some(entry.key.as_str()) == group {
            continue;
        }
        for project in &mut entry.projects {
            project.image_ids = Vec::new();
            project.flagged_ids = Vec::new();
        }
    }
}

/// `GET /api/db/{db}/calibrations/flat-groups/use?group=` — each group's
/// flat masters and the lights that take its flats, by project and by how
/// the flats were chosen; with `group`, that group's lights by id.
pub async fn get_flat_group_use(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Query(query): Query<UseQuery>,
) -> Result<Json<ApiResponse<FlatGroupUseView>>, AppError> {
    let ctx = Arc::clone(&ctx.0);
    let mut view = tokio::task::spawn_blocking(move || use_of(&state, &ctx))
        .await
        .map_err(|error| AppError::InternalError(format!("flat group use task failed: {error}")))?
        .map_err(|error| {
            AppError::InternalError(format!("working out what the flat groups feed: {error:#}"))
        })?;
    keep_ids_of(&mut view, query.group.as_deref());
    Ok(Json(ApiResponse::success(view)))
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

        let (view, frame_groups) =
            crate::server::flat_groups::build_view_with_frames(&conn).unwrap();
        let family = &view.rigs[0].families[0];
        let (newer, older) = (family.groups[0].key.clone(), family.groups[1].key.clone());
        let library = browse::library_frames(&conn).unwrap();
        let snapshot = LibrarySnapshot::load(&conn).unwrap();
        let night = |night: &str| -> Vec<String> {
            library
                .frames
                .iter()
                .filter(|row| row.night.as_deref() == Some(night))
                .map(|row| row.frame.frame_uuid.clone())
                .collect()
        };
        // A master of the older night's flats.
        let sources = night("2026-09-10");
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
        let mut lights: Vec<MatchedLight> = [(1, &before), (2, &after), (3, &during)]
            .into_iter()
            .map(|(id, meta)| match_light(&snapshot, 7, id, meta))
            .collect();
        // A master that takes both nights' flats counts in both groups; one
        // from other software counts in none.
        let both: std::collections::BTreeSet<String> = night("2026-09-10")
            .into_iter()
            .chain(night("2026-09-14"))
            .collect();
        lights.push(MatchedLight {
            project: 8,
            image_id: 4,
            flats: MasterFrames::Raw(both),
            basis: FlatBasis::Nearest,
        });
        lights.push(MatchedLight {
            project: 8,
            image_id: 5,
            flats: MasterFrames::External("ext".into()),
            basis: FlatBasis::Nearest,
        });
        let (groups, matched) = tally(&frame_groups, &[master], &names, lights);
        assert_eq!(matched, 5);
        let of = |key: &str| groups.iter().find(|group| group.key == key).unwrap();
        let older = of(&older);
        assert_eq!(
            older.lights,
            LightCounts {
                total: 3,
                own: 2,
                undetermined: 1,
                ..Default::default()
            },
            "the light before, the one during the change, which keeps the nearest, and the \
             one whose master spans both"
        );
        assert_eq!(older.masters.len(), 1);
        assert_eq!(older.masters[0].in_group, 3);
        assert_eq!(older.projects.len(), 2);
        let project = &older.projects[0];
        assert_eq!(project.name.as_deref(), Some("M 44"));
        assert_eq!(project.image_ids, vec![1, 3]);
        assert_eq!(project.flagged_ids, vec![3], "the undetermined light");
        let newer = of(&newer);
        assert_eq!((newer.lights.total, newer.lights.own), (2, 2));
        assert_eq!(newer.projects[1].image_ids, vec![4]);
        assert!(newer.masters.is_empty());
    }

    /// A flat held back from the far side of a change is never in a
    /// light's master, and a master from other software the policy prefers
    /// stands in for the raw flats.
    #[test]
    fn a_lights_flat_master_leaves_out_held_back_flats_and_takes_an_adopted_master() {
        let _lock = POLICY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let conn = crate::server::flat_groups::tests::catalog_of(
            temp.path(),
            &["2026-09-10T14:0", "2026-09-14T14:0"],
            |a, b| if a.night == b.night { 0.0 } else { 0.02 },
        );
        let snapshot = LibrarySnapshot::load(&conn).unwrap();
        let mut flats: Vec<crate::calibration::CalibrationFrame> = snapshot
            .frames()
            .iter()
            .filter(|frame| frame.kind == CalibrationKind::Flat)
            .cloned()
            .collect();
        flats.sort_by_key(|frame| frame.captured_at);
        assert_eq!(flats.len(), 6);
        let (early, late) = flats.split_at(3);
        let held: Vec<String> = early.iter().map(|frame| frame.frame_uuid.clone()).collect();
        let selected = crate::calibration::CalibrationSelection {
            flat: flats.clone(),
            flat_held_back: held.clone(),
            ..Default::default()
        };
        let MasterFrames::Raw(used) = master_frames(&selected, CalibrationKind::Flat) else {
            panic!("raw flats");
        };
        assert!(!used.is_empty());
        assert!(used.iter().all(|uuid| !held.contains(uuid)), "{used:?}");
        assert!(used
            .iter()
            .all(|uuid| late.iter().any(|frame| &frame.frame_uuid == uuid)));

        let mut external = late[0].clone();
        external.frame_uuid = "external".into();
        external.is_master = true;
        let mut rules = crate::calibration::Rules::default();
        rules.external_masters = crate::calibration::ExternalMasterPolicy::Prefer;
        let selected = crate::calibration::CalibrationSelection {
            flat: [flats.clone(), vec![external]].concat(),
            rules: Arc::new(rules),
            ..Default::default()
        };
        assert_eq!(
            master_frames(&selected, CalibrationKind::Flat),
            MasterFrames::External("external".into())
        );
    }

    /// Just the Target Scheduler tables the health keys read.
    const MINIMAL_TS: &str = "
        CREATE TABLE project (Id INTEGER PRIMARY KEY, profileId TEXT, name TEXT, guid TEXT);
        CREATE TABLE target (Id INTEGER PRIMARY KEY, name TEXT, active INTEGER,
            epochcode INTEGER, projectId INTEGER, guid TEXT);
        CREATE TABLE acquiredimage (Id INTEGER PRIMARY KEY, projectId INTEGER, targetId INTEGER,
            acquireddate INTEGER, filtername TEXT, gradingStatus INTEGER, metadata TEXT,
            profileId TEXT);";

    /// Review repro: a flat master recorded after a result was kept must
    /// move the revision, or "What it feeds" never lists it.
    #[test]
    fn review_recording_a_flat_master_moves_the_revision() {
        let _lock = POLICY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let conn = crate::server::flat_groups::tests::catalog_of(
            temp.path(),
            &["2026-09-10T14:0", "2026-09-14T14:0"],
            |a, b| if a.night == b.night { 0.0 } else { 0.02 },
        );
        conn.execute_batch(MINIMAL_TS).unwrap();
        conn.execute_batch(
            "INSERT INTO project (Id, profileId, name, guid) VALUES (1, 'p', 'M42', 'pg');
             INSERT INTO target (Id, name, active, epochcode, projectId, guid)
                 VALUES (1, 'M42', 1, 0, 1, 'tg');
             INSERT INTO acquiredimage (Id, projectId, targetId, acquireddate, filtername,
                 gradingStatus, metadata, profileId)
                 VALUES (1, 1, 1, 1789000000, 'Ha', 0, '{\"FileName\":\"l.fits\"}', 'p');",
        )
        .unwrap();
        let ctx = DatabaseContext::new_for_test(Connection::open_in_memory().unwrap());
        let before = revision(&conn, &ctx).unwrap();
        conn.execute(
            "INSERT INTO psf_guard_calibration_master (master_uuid, rig_uuid, kind, cache_path,
                 source_set_hash, source_count, source_frame_uuids, created_at, seiza_version,
                 cache_version, statistics_json)
             VALUES ('m1', 'r', 'flat', '/cache/m1.fits', 'h', 3, '[]', 5, 'x', 1, '{}')",
            [],
        )
        .unwrap();
        assert_ne!(
            revision(&conn, &ctx).unwrap(),
            before,
            "a new flat master leaves the kept result current"
        );
    }

    /// Review repro: a catalog with no projects has a constant revision, so
    /// a regroup never refreshes the kept masters' group keys.
    #[test]
    fn review_a_regroup_moves_the_revision_without_projects() {
        let _lock = POLICY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let conn = crate::server::flat_groups::tests::catalog_of(
            temp.path(),
            &["2026-09-10T14:0", "2026-09-14T14:0"],
            |a, b| if a.night == b.night { 0.0 } else { 0.02 },
        );
        conn.execute_batch(MINIMAL_TS).unwrap();
        let ctx = DatabaseContext::new_for_test(Connection::open_in_memory().unwrap());
        let before = revision(&conn, &ctx).unwrap();
        // Measured again, now alike: one group instead of two.
        crate::calibration::flat_groups::testing::measure_as(&conn, |_, _| 0.0);
        assert_ne!(revision(&conn, &ctx).unwrap(), before);
    }

    /// Grading the library again in the background touches each frame's
    /// row and measurements, but keeps out no other frame: the answer stays.
    #[test]
    fn grading_that_keeps_out_nothing_new_leaves_the_revision() {
        let _lock = POLICY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let conn = crate::server::flat_groups::tests::catalog_of(
            temp.path(),
            &["2026-09-10T14:0", "2026-09-14T14:0"],
            |a, b| if a.night == b.night { 0.0 } else { 0.02 },
        );
        conn.execute_batch(MINIMAL_TS).unwrap();
        let ctx = DatabaseContext::new_for_test(Connection::open_in_memory().unwrap());
        let quality = |grade: &str, at: i64| {
            conn.execute(
                "INSERT OR REPLACE INTO psf_guard_calibration_frame_quality
                     (frame_uuid, grade, findings_json, measured_at, graded_at, updated_at)
                 SELECT frame_uuid, ?1, ?2, ?3, ?3, ?3 FROM psf_guard_calibration_frame",
                rusqlite::params![grade, format!("[{at}]"), at],
            )
            .unwrap();
        };
        quality("ok", 1);
        let before = revision(&conn, &ctx).unwrap();
        conn.execute(
            "UPDATE psf_guard_calibration_frame SET updated_at = updated_at + 1",
            [],
        )
        .unwrap();
        quality("suspect", 2);
        assert_eq!(revision(&conn, &ctx).unwrap(), before);
        // A grade that keeps flats out does move it.
        quality("poor", 3);
        assert_ne!(revision(&conn, &ctx).unwrap(), before);
    }
}
