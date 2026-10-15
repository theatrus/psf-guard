//! The part of the library one project's lights use: the Calibration page's
//! project scope.
//!
//! Each light is matched the way a stack matches it ([`LibrarySnapshot::select`]),
//! and then reduced the way a stack builds its masters: for each kind, the
//! coherent set of raw frames one master takes (`coherent_master_subset`,
//! for darks the nearest complete session), or the external master the
//! policy adopts instead. So the scope follows every rule a stack follows:
//! the rig, filter, exposure and settings, the dark reach, validity marks,
//! the frames kept out, and flat grouping's choices (flats from another
//! angle, corrected flats, the side of a change). Matching alone has no age
//! limit for flats, so a year-old session at the light's filter matches;
//! only the set a master is built from is in scope. The scope holds:
//!
//! - the browser's sessions that hold a frame some light's masters use,
//!   with, for corrected flats, the two sessions the change's map is made
//!   from; flats held back from the far side of a change are not used, so
//!   they do not count;
//! - the recorded masters whose every source frame sits in the set one light
//!   would build a master of that kind from, including one a flat-set check
//!   trimmed;
//! - the calibration rigs those sessions and masters belong to. A project
//!   shot with a second camera, or at another binning, spans two.

use super::browse::{self, LibraryFrames};
use super::{
    coherent_master_subset, CalibrationFrame, CalibrationKind, CalibrationSelection,
    ExternalMasterPolicy, LibrarySnapshot, MIN_MASTER_FRAMES,
};
use crate::commands::import::headers::FrameMeta;
use anyhow::Result;
use rusqlite::types::ValueRef;
use rusqlite::Connection;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

/// What a project's lights take from the library, before it is named in
/// the browser's terms.
#[derive(Debug, Clone, Default)]
pub struct LightUse {
    /// Frames some light's masters use, by UUID.
    pub frames: HashSet<String>,
    /// For each kind, the distinct sets of raw frames one light's master
    /// would be built from.
    master_sets: HashMap<CalibrationKind, Vec<BTreeSet<String>>>,
    /// Lights matched.
    pub lights: usize,
}

impl LightUse {
    /// Match each light against the library, and keep what its masters use.
    pub(crate) fn of<'a>(
        snapshot: &LibrarySnapshot,
        lights: impl IntoIterator<Item = &'a FrameMeta>,
    ) -> Self {
        let mut used = Self::default();
        let mut seen: HashSet<(CalibrationKind, BTreeSet<String>)> = HashSet::new();
        for light in lights {
            used.lights += 1;
            used.add(snapshot, &snapshot.select(light), &mut seen);
        }
        used
    }

    fn add(
        &mut self,
        snapshot: &LibrarySnapshot,
        selected: &CalibrationSelection,
        seen: &mut HashSet<(CalibrationKind, BTreeSet<String>)>,
    ) {
        let held_back: HashSet<&str> = selected.flat_held_back.iter().map(String::as_str).collect();
        let rules = &selected.rules;
        for (kind, frames) in [
            (CalibrationKind::Bias, &selected.bias),
            (CalibrationKind::Dark, &selected.dark),
            (CalibrationKind::DarkFlat, &selected.dark_flat),
            (CalibrationKind::Flat, &selected.flat),
        ] {
            // As a stack builds: a flat from the far side of a change is
            // never used, and an external master is used as it is.
            let (externals, raw): (Vec<&CalibrationFrame>, Vec<&CalibrationFrame>) = frames
                .iter()
                .filter(|frame| {
                    kind != CalibrationKind::Flat || !held_back.contains(frame.frame_uuid.as_str())
                })
                .partition(|frame| frame.is_master);
            let raw: Vec<CalibrationFrame> = raw.into_iter().cloned().collect();
            let subset = coherent_master_subset(kind, &raw, rules);
            let adopted = match rules.external_masters {
                ExternalMasterPolicy::Prefer => externals.first(),
                ExternalMasterPolicy::Fallback if subset.len() < MIN_MASTER_FRAMES => {
                    externals.first()
                }
                _ => None,
            };
            if let Some(master) = adopted {
                self.frames.insert(master.frame_uuid.clone());
                continue;
            }
            let set: BTreeSet<String> = subset
                .iter()
                .map(|frame| frame.frame_uuid.clone())
                .collect();
            self.frames.extend(set.iter().cloned());
            if !set.is_empty() && seen.insert((kind, set.clone())) {
                self.master_sets.entry(kind).or_default().push(set);
            }
        }
        // Corrected flats are carried across a change by a map made from a
        // session either side of it, which may be at another filter or angle.
        if let Some(correction) = selected.flat_basis.correction() {
            let book = snapshot.flat_book();
            for key in [&correction.before_response, &correction.after_response] {
                if let Some(index) = book.session_by_response(key) {
                    self.frames
                        .extend(book.sessions[index].row.frame_uuids.iter().cloned());
                }
            }
        }
    }

    /// Whether a recorded master of `kind` built from `sources` is one of
    /// these lights' masters: every source sits in the frames one light
    /// would build that master from.
    pub fn uses_master(&self, kind: CalibrationKind, sources: &[String]) -> bool {
        !sources.is_empty()
            && self.master_sets.get(&kind).is_some_and(|sets| {
                sets.iter()
                    .any(|set| sources.iter().all(|uuid| set.contains(uuid)))
            })
    }
}

/// A calibration rig in a project's scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ScopeRig {
    pub rig_uuid: String,
    pub name: Option<String>,
}

/// The library as one project's lights use it.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ProjectScope {
    pub project_id: i32,
    /// Lights matched against the library.
    pub lights: usize,
    /// Lights left out: rejected lights never count; these are the ones
    /// whose file was not found or whose header could not be read.
    pub unread: usize,
    /// The browser's sessions holding frames the lights use.
    pub sessions: BTreeSet<String>,
    /// Frames in those sessions.
    pub frames: usize,
    /// The recorded masters the lights calibrate with.
    pub masters: BTreeSet<String>,
    /// The calibration rigs of those sessions and masters, by name.
    pub rigs: Vec<ScopeRig>,
    /// Lights whose headers are still being read; the scope grows as they
    /// arrive.
    pub pending: usize,
    /// What the scope was worked out from ([`scope_revision`]), so a view
    /// can ask its lists again when it changes.
    pub revision: String,
    /// Worked out before the latest change; a newer one is on its way.
    pub refreshing: bool,
}

impl ProjectScope {
    /// Name what the lights use in the browser's terms.
    pub fn of(
        project_id: i32,
        used: &LightUse,
        unread: usize,
        library: &LibraryFrames,
        masters: &[browse::RecordedLibraryMaster],
    ) -> Self {
        let mut rigs: BTreeMap<String, Option<String>> = BTreeMap::new();
        let sessions: BTreeSet<String> = library
            .frames
            .iter()
            .filter(|row| used.frames.contains(&row.frame.frame_uuid))
            .map(|row| {
                rigs.entry(row.frame.rig_uuid.clone())
                    .or_insert_with(|| library.rig_name(&row.frame.rig_uuid));
                row.session.clone()
            })
            .collect();
        let frames = library
            .frames
            .iter()
            .filter(|row| sessions.contains(&row.session))
            .count();
        let masters: BTreeSet<String> = masters
            .iter()
            .filter(|master| used.uses_master(master.kind, &master.source_uuids))
            .map(|master| {
                rigs.entry(master.rig_uuid.clone())
                    .or_insert_with(|| master.rig_name.clone());
                master.master_uuid.clone()
            })
            .collect();
        Self {
            project_id,
            lights: used.lights,
            unread,
            sessions,
            frames,
            masters,
            rigs: rigs
                .into_iter()
                .map(|(rig_uuid, name)| ScopeRig { rig_uuid, name })
                .collect(),
            ..Self::default()
        }
    }
}

/// The scope of a project whose lights' headers are `lights`, read from
/// one catalog connection.
pub fn project_scope(
    conn: &Connection,
    project_id: i32,
    lights: &[FrameMeta],
    unread: usize,
) -> Result<ProjectScope> {
    let snapshot = LibrarySnapshot::load(conn)?;
    let used = LightUse::of(&snapshot, lights);
    let library = browse::library_frames(conn)?;
    let masters = browse::recorded_masters(conn)?;
    Ok(ProjectScope::of(
        project_id, &used, unread, &library, &masters,
    ))
}

/// Fold every row of `sql` into `hasher`.
fn digest_query(
    conn: &Connection,
    sql: &str,
    params: impl rusqlite::Params,
    hasher: &mut Sha256,
) -> Result<()> {
    let mut statement = conn.prepare(sql)?;
    let columns = statement.column_count();
    let mut rows = statement.query(params)?;
    while let Some(row) = rows.next()? {
        for index in 0..columns {
            match row.get_ref(index)? {
                ValueRef::Null => hasher.update(b"\x00"),
                ValueRef::Integer(value) => hasher.update(value.to_le_bytes()),
                ValueRef::Real(value) => hasher.update(value.to_le_bytes()),
                ValueRef::Text(bytes) | ValueRef::Blob(bytes) => hasher.update(bytes),
            }
            hasher.update(b"\x1f");
        }
        hasher.update(b"\x1e");
    }
    hasher.update(b"\x1d");
    Ok(())
}

/// What a project's scope is worked out from, as a digest: one changes
/// whenever the scope could.
///
/// - each frame's identity, file, validity mark and whether it is kept
///   out: a grade counts only where it keeps a frame out, so grading the
///   library in the background leaves it alone;
/// - flat grouping's measurements, which choose a light's flats;
/// - the recorded masters, by count and newest;
/// - the settings and rules that match a light, with the night boundary;
/// - this project's lights, and no other project's.
pub fn scope_revision(conn: &Connection, project_id: i32) -> Result<String> {
    let mut hasher = Sha256::new();
    if super::schema_exists(conn) {
        let kept = if super::quality::table_exists(conn) {
            "CASE WHEN q.override_choice = 'use' THEN 0
                  WHEN q.override_choice = 'exclude' THEN 1
                  WHEN q.grade = 'poor' THEN 1
                  WHEN q.grade IS NOT NULL THEN 0
                  ELSE f.stray_light IS NOT NULL END
             FROM psf_guard_calibration_frame f
             LEFT JOIN psf_guard_calibration_frame_quality q ON q.frame_uuid = f.frame_uuid"
        } else {
            "f.stray_light IS NOT NULL FROM psf_guard_calibration_frame f"
        };
        digest_query(
            conn,
            &format!(
                "SELECT f.frame_uuid, f.source_fingerprint, f.source_path, f.valid_direction,
                        f.is_master, {kept}
                 ORDER BY f.frame_uuid"
            ),
            [],
            &mut hasher,
        )?;
        digest_query(
            conn,
            "SELECT COUNT(*), MAX(created_at) FROM psf_guard_calibration_master",
            [],
            &mut hasher,
        )?;
    }
    super::flat_groups::store::digest_rows(conn, &mut hasher)?;
    hasher.update(super::health::settings_digest(
        super::catalog_night_boundary(conn),
    ));
    hasher.update(b"\x1d");
    // One project's lights, as health reads them: rejected or not, when,
    // with what filter and file.
    digest_query(
        conn,
        "SELECT Id, gradingStatus = 2, acquireddate, filtername,
                CASE WHEN json_valid(metadata) THEN json_extract(metadata, '$.FileName') END
         FROM acquiredimage WHERE projectId = ?1 ORDER BY Id",
        [project_id],
        &mut hasher,
    )?;
    Ok(hasher.finalize()[..12]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calibration::import_calibration_frames;
    use crate::calibration::test_frames::{noise, write_frame};
    use crate::commands::import::headers::read_frame_meta;
    use std::path::{Path, PathBuf};

    /// Two sizes of one camera are two calibration rigs, as after a camera
    /// swap or a change of binning.
    const RIG_A: (usize, usize) = (64, 64);
    const RIG_B: (usize, usize) = (80, 80);
    const RIG_C: (usize, usize) = (96, 96);

    struct Library {
        temp: tempfile::TempDir,
        conn: Connection,
    }

    impl Library {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            let conn = Connection::open(temp.path().join("catalog.sqlite")).unwrap();
            Self { temp, conn }
        }

        fn path(&self, name: &str) -> PathBuf {
            self.temp.path().join(name)
        }

        /// Three frames named `name-0..2`, a minute apart from `captured`.
        fn frames(
            &self,
            name: &str,
            kind: &str,
            size: (usize, usize),
            captured: &str,
            filter: &str,
        ) {
            let metas: Vec<FrameMeta> = (0..3)
                .map(|index| {
                    let path = self.path(&format!("{name}-{index}.fits"));
                    let at = format!("{captured}:0{index}");
                    write_frame(&path, kind, size, &at, |x, y| {
                        20_000.0 + 5.0 * noise(x, y, index)
                    });
                    if filter != "Ha" {
                        retitle(&path, filter);
                    }
                    read_frame_meta(&path)
                })
                .collect();
            let tx = self.conn.unchecked_transaction().unwrap();
            import_calibration_frames(&tx, &metas, Some("profile")).unwrap();
            tx.commit().unwrap();
        }

        fn light(&self, name: &str, size: (usize, usize), captured: &str) -> FrameMeta {
            let path = self.path(&format!("{name}.fits"));
            write_frame(&path, "LIGHT", size, captured, |_, _| 1_000.0);
            read_frame_meta(&path)
        }

        /// The frame UUIDs and browser sessions of the frames named `name`.
        fn of(&self, name: &str) -> (Vec<String>, BTreeSet<String>) {
            let library = browse::library_frames(&self.conn).unwrap();
            let prefix = format!("{name}-");
            let rows: Vec<_> = library
                .frames
                .iter()
                .filter(|row| {
                    Path::new(&row.frame.source_path)
                        .file_name()
                        .unwrap()
                        .to_string_lossy()
                        .starts_with(&prefix)
                })
                .collect();
            assert_eq!(rows.len(), 3, "{name}");
            (
                rows.iter()
                    .map(|row| row.frame.frame_uuid.clone())
                    .collect(),
                rows.iter().map(|row| row.session.clone()).collect(),
            )
        }

        fn rig_of(&self, name: &str) -> String {
            let uuid = &self.of(name).0[0];
            self.conn
                .query_row(
                    "SELECT rig_uuid FROM psf_guard_calibration_frame WHERE frame_uuid = ?1",
                    [uuid],
                    |row| row.get(0),
                )
                .unwrap()
        }

        fn master(&self, uuid: &str, kind: &str, rig: &str, sources: &[String]) {
            self.conn
                .execute(
                    "INSERT INTO psf_guard_calibration_master (
                        master_uuid, rig_uuid, kind, cache_path, source_set_hash, source_count,
                        source_frame_uuids, created_at, seiza_version, cache_version,
                        statistics_json
                     ) VALUES (?1, ?2, ?3, ?4, ?1, ?5, ?6, 0, 'test', 1, '{}')",
                    rusqlite::params![
                        uuid,
                        rig,
                        kind,
                        format!("/masters/{uuid}.fits"),
                        sources.len() as i64,
                        serde_json::to_string(sources).unwrap(),
                    ],
                )
                .unwrap();
        }
    }

    /// Write another two-letter filter over the frame's `Ha`.
    fn retitle(path: &Path, filter: &str) {
        assert_eq!(filter.len(), 2);
        let mut bytes = std::fs::read(path).unwrap();
        let from = b"FILTER  = 'Ha'";
        let at = bytes
            .windows(from.len())
            .position(|window| window == from)
            .unwrap();
        bytes[at + 11..at + 13].copy_from_slice(filter.as_bytes());
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn a_project_on_two_rigs_holds_both_rigs_sessions_and_masters_and_nothing_else() {
        let _settings = crate::calibration::tests::POLICY_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let library = Library::new();
        library.frames("a-flat-ha", "FLAT", RIG_A, "2026-09-10T14:00", "Ha");
        library.frames("a-flat-o3", "FLAT", RIG_A, "2026-09-10T14:10", "O3");
        library.frames("a-dark", "DARK", RIG_A, "2026-09-10T15:00", "Ha");
        library.frames("b-flat-ha", "FLAT", RIG_B, "2026-09-10T14:20", "Ha");
        library.frames("b-dark", "DARK", RIG_B, "2026-09-10T15:10", "Ha");
        library.frames("c-dark", "DARK", RIG_C, "2026-09-10T15:20", "Ha");
        let lights = [
            library.light("light-a", RIG_A, "2026-09-11T04:00:00"),
            library.light("light-b", RIG_B, "2026-09-11T05:00:00"),
        ];

        let (a_flats, _) = library.of("a-flat-ha");
        let (a_darks, _) = library.of("a-dark");
        let (o3_flats, _) = library.of("a-flat-o3");
        let (c_darks, _) = library.of("c-dark");
        let (rig_a, rig_b) = (library.rig_of("a-dark"), library.rig_of("b-dark"));
        assert_ne!(rig_a, rig_b);
        library.master("m-a-dark", "dark", &rig_a, &a_darks);
        // A flat-set check that set one flat aside still built this one.
        library.master("m-a-flat", "flat", &rig_a, &a_flats[..2]);
        library.master("m-a-o3", "flat", &rig_a, &o3_flats);
        library.master("m-c-dark", "dark", &library.rig_of("c-dark"), &c_darks);
        // A master mixing frames no one light takes together.
        let mixed = [a_darks[0].clone(), c_darks[0].clone()];
        library.master("m-mixed", "dark", &rig_a, &mixed);

        let scope = project_scope(&library.conn, 7, &lights, 1).unwrap();
        let expected: BTreeSet<String> = ["a-flat-ha", "a-dark", "b-flat-ha", "b-dark"]
            .iter()
            .flat_map(|name| library.of(name).1)
            .collect();
        assert_eq!(scope.sessions, expected);
        assert_eq!(scope.frames, 12);
        assert_eq!(
            scope.masters,
            BTreeSet::from(["m-a-dark".to_string(), "m-a-flat".to_string()])
        );
        let mut rigs: Vec<String> = scope.rigs.iter().map(|rig| rig.rig_uuid.clone()).collect();
        rigs.sort();
        let mut both = vec![rig_a, rig_b];
        both.sort();
        assert_eq!(rigs, both, "both rigs, and not the third");
        assert_eq!((scope.project_id, scope.lights, scope.unread), (7, 2, 1));

        // The scope narrows the browser's lists to exactly its sessions.
        let frames = browse::library_frames(&library.conn).unwrap();
        let filter = browse::FrameFilter {
            within: Some(std::sync::Arc::new(scope.sessions.clone())),
            ..Default::default()
        };
        let page = frames.sessions_page(&filter, |_| true, 0, None);
        assert_eq!(page.total, 4);
        assert_eq!(page.frames, 12);
    }

    #[test]
    fn a_flat_session_a_year_older_matches_but_is_not_in_scope() {
        let _settings = crate::calibration::tests::POLICY_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let library = Library::new();
        // Matching has no age limit for flats: both sessions match the
        // light, but its flat master takes only the one beside it.
        library.frames("old-flat", "FLAT", RIG_A, "2025-09-10T14:00", "Ha");
        library.frames("new-flat", "FLAT", RIG_A, "2026-09-10T14:00", "Ha");
        library.frames("dark", "DARK", RIG_A, "2026-09-10T15:00", "Ha");
        let light = library.light("light", RIG_A, "2026-09-11T04:00:00");
        let selected = LibrarySnapshot::load(&library.conn).unwrap().select(&light);
        assert_eq!(selected.flat.len(), 6, "both sessions match");

        let rig = library.rig_of("dark");
        let (old, _) = library.of("old-flat");
        let (new, _) = library.of("new-flat");
        library.master("m-old", "flat", &rig, &old);
        library.master("m-new", "flat", &rig, &new);

        let scope = project_scope(&library.conn, 1, &[light], 0).unwrap();
        let expected: BTreeSet<String> = ["new-flat", "dark"]
            .iter()
            .flat_map(|name| library.of(name).1)
            .collect();
        assert_eq!(scope.sessions, expected);
        assert_eq!(scope.masters, BTreeSet::from(["m-new".to_string()]));
    }

    #[test]
    fn the_revision_moves_only_with_what_can_change_the_scope() {
        let library = Library::new();
        crate::ts_schema::apply_schema(&library.conn).unwrap();
        library.frames("dark", "DARK", RIG_A, "2026-09-10T15:00", "Ha");
        // Another project's light; where nights split follows every light.
        library
            .conn
            .execute(
                "INSERT INTO acquiredimage (Id, projectId, targetId, acquireddate, filtername,
                     gradingStatus, metadata, profileId)
                 VALUES (1, 2, 2, 1757563200, 'Ha', 0, '{}', 'p')",
                [],
            )
            .unwrap();
        let revision = || scope_revision(&library.conn, 1).unwrap();
        let start = revision();
        let (darks, _) = library.of("dark");
        let grade = |uuid: &str, grade: &str| {
            library
                .conn
                .execute(
                    "INSERT INTO psf_guard_calibration_frame_quality
                        (frame_uuid, grade, algorithm_version, measured_at, updated_at)
                     VALUES (?1, ?2, 1, 0, 0)
                     ON CONFLICT(frame_uuid) DO UPDATE SET grade = excluded.grade",
                    rusqlite::params![uuid, grade],
                )
                .unwrap()
        };
        // Grading a frame OK, as the background pass does, keeps no frame
        // out: the scope stays.
        grade(&darks[0], "ok");
        assert_eq!(revision(), start);
        // A poor grade keeps one out.
        grade(&darks[0], "poor");
        let poor = revision();
        assert_ne!(poor, start);
        // Another project's lights are not this one's.
        library
            .conn
            .execute(
                "UPDATE acquiredimage SET filtername = 'OIII', gradingStatus = 2",
                [],
            )
            .unwrap();
        assert_eq!(revision(), poor);
        // A master recorded since counts.
        library.master("m", "dark", &library.rig_of("dark"), &darks);
        assert_ne!(revision(), poor);
    }

    #[test]
    fn a_project_without_lights_uses_nothing() {
        let library = Library::new();
        library.frames("a-dark", "DARK", RIG_A, "2026-09-10T15:00", "Ha");
        let scope = project_scope(&library.conn, 1, &[], 0).unwrap();
        assert!(scope.sessions.is_empty() && scope.masters.is_empty() && scope.rigs.is_empty());
    }
}
