use super::*;

const TS_PROFILE: &str = "ts-profile";
const IMPORT_PROFILE: &str = "import-profile";
const NAME: &str =
    "M 45 RGB_LIGHT_Luminance_30.00s_2026-10-04_03-13-11_Session_2026-10-03_0052.fits";
const CAPTURED: &str = "2026-10-04T02:13:11";
const ACQUIRED: i64 = 1_791_080_000;

/// A catalog holding Target Scheduler's record of an exposure under one
/// profile, and an import of the capture folder under another, as a merge
/// of the two catalogs leaves it.
struct Catalog {
    _dir: tempfile::TempDir,
    path: std::path::PathBuf,
    conn: Connection,
}

/// One light to insert.
struct Row<'a> {
    project: i64,
    target: i64,
    plan: i64,
    profile: &'a str,
    guid: &'a str,
    file_name: String,
    captured: &'a str,
    acquired: i64,
    filter: &'a str,
    grade: i64,
    nina: bool,
}

impl Catalog {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("catalog.sqlite");
        let conn = Connection::open(&path).unwrap();
        crate::ts_schema::apply_schema(&conn).unwrap();
        conn.execute(
            "INSERT INTO exposuretemplate (Id, profileId, name, filtername, gain, offset, bin,
                 readoutmode, twilightlevel, moonavoidanceenabled, moonavoidanceseparation,
                 moonavoidancewidth, maximumhumidity)
             VALUES (1, 'ts-profile', 'Luminance', 'Luminance', -1, -1, 1, -1, 0, 0, 0, 0, 0)",
            [],
        )
        .unwrap();
        Self {
            _dir: dir,
            path,
            conn,
        }
    }

    fn project(&self, profile: &str, name: &str, imported: bool, state: i64) -> i64 {
        let description = if imported {
            format!("{IMPORTED_PROJECT_MARKER} (1 frames)")
        } else {
            "LRGB".to_string()
        };
        self.conn
            .execute(
                "INSERT INTO project (profileId, name, description, state, priority, createdate,
                     minimumtime, minimumaltitude, maximumAltitude, usecustomhorizon,
                     horizonoffset, meridianwindow, filterswitchfrequency, ditherevery,
                     enablegrader, isMosaic, flatsHandling, smartexposureorder, guid)
                 VALUES (?1, ?2, ?3, ?4, 1, 0, 30, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0,
                     'project-' || ?2)",
                params![profile, name, description, state],
            )
            .unwrap();
        self.conn.last_insert_rowid()
    }

    fn target(&self, project: i64, name: &str) -> i64 {
        self.conn
            .execute(
                "INSERT INTO target (name, active, ra, dec, epochcode, rotation, roi, projectid,
                     guid)
                 VALUES (?1, 1, 3.8, 24.1, 2, 0, 100, ?2, 'target-' || ?1)",
                params![name, project],
            )
            .unwrap();
        self.conn.last_insert_rowid()
    }

    /// An exposure plan with these counts.
    fn plan(&self, target: i64, profile: &str, acquired: i64, accepted: i64) -> i64 {
        self.conn
            .execute(
                "INSERT INTO exposureplan (profileId, exposure, desired, acquired, accepted,
                     targetid, exposureTemplateId, enabled, guid)
                 VALUES (?1, 30, 100, ?2, ?3, ?4, 1, 1, 'plan-' || ?4 || '-' || ?1)",
                params![profile, acquired, accepted, target],
            )
            .unwrap();
        self.conn.last_insert_rowid()
    }

    fn light(&self, row: Row<'_>) -> i64 {
        let mut metadata = serde_json::json!({
            "FileName": row.file_name,
            "FilterName": row.filter,
            "ExposureStartTime": row.captured,
            "ExposureDuration": 30.0,
        });
        if row.nina {
            metadata["GuidingRMS"] = serde_json::json!(0.48);
            metadata["SessionId"] = serde_json::json!(105);
        }
        self.conn
            .execute(
                "INSERT INTO acquiredimage (projectId, targetId, acquireddate, filtername,
                     gradingStatus, metadata, profileId, exposureId, guid, rejectreason)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    row.project,
                    row.target,
                    row.acquired,
                    row.filter,
                    row.grade,
                    metadata.to_string(),
                    row.profile,
                    row.plan,
                    row.guid,
                    (row.grade == 2).then_some("Stars")
                ],
            )
            .unwrap();
        self.conn.last_insert_rowid()
    }

    fn count(&self, sql: &str) -> i64 {
        self.conn.query_row(sql, [], |row| row.get(0)).unwrap()
    }

    fn counts(&self, plan: i64) -> (i64, i64) {
        self.conn
            .query_row(
                "SELECT acquired, accepted FROM exposureplan WHERE Id = ?1",
                [plan],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap()
    }

    fn grade(&self, guid: &str) -> (i64, Option<String>) {
        self.conn
            .query_row(
                "SELECT gradingStatus, rejectreason FROM acquiredimage WHERE guid = ?1",
                [guid],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap()
    }
}

/// Target Scheduler's light and plan, and an import of the same file with
/// its own project, target and plan.
struct Pair {
    catalog: Catalog,
    ts_plan: i64,
    import_project: i64,
    import_target: i64,
    import_plan: i64,
}

fn pair(ts_grade: i64, import_grade: i64) -> Pair {
    let catalog = Catalog::new();
    let ts_project = catalog.project(TS_PROFILE, "M45", false, 3);
    let ts_target = catalog.target(ts_project, "M45");
    let ts_plan = catalog.plan(ts_target, TS_PROFILE, 1, i64::from(ts_grade == 1));
    catalog.light(Row {
        project: ts_project,
        target: ts_target,
        plan: ts_plan,
        profile: TS_PROFILE,
        guid: "ts-light",
        file_name: format!(r"C:\Users\astro\Documents\N.I.N.A\Session_2026-10-03\{NAME}"),
        captured: "2026-10-04T02:13:11.4882859Z",
        acquired: ACQUIRED,
        filter: "Luminance",
        grade: ts_grade,
        nina: true,
    });
    let import_project = catalog.project(IMPORT_PROFILE, "M 45 RGB", true, 0);
    let import_target = catalog.target(import_project, "M 45 RGB");
    let import_plan = catalog.plan(
        import_target,
        IMPORT_PROFILE,
        1,
        i64::from(import_grade == 1),
    );
    catalog.light(Row {
        project: import_project,
        target: import_target,
        plan: import_plan,
        profile: IMPORT_PROFILE,
        guid: "import-light",
        file_name: format!("/capture/Session_2026-10-05/M 45 RGB/{NAME}"),
        captured: &format!("{CAPTURED}Z"),
        acquired: ACQUIRED,
        filter: "Luminance",
        grade: import_grade,
        nina: false,
    });
    Pair {
        catalog,
        ts_plan,
        import_project,
        import_target,
        import_plan,
    }
}

fn keepers() -> Keepers {
    Keepers::default()
}

#[test]
fn an_imported_copy_folds_into_target_schedulers_light() {
    let mut pair = pair(1, 0);
    let plan = super::plan(&pair.catalog.conn, &keepers()).unwrap();
    assert!(plan.held.is_empty(), "{plan:?}");
    let [group] = plan.groups.as_slice() else {
        panic!("{plan:?}");
    };
    assert_eq!(group.kept.guid, "ts-light");
    assert_eq!(
        group
            .copies
            .iter()
            .map(|copy| copy.guid.as_str())
            .collect::<Vec<_>>(),
        ["import-light"]
    );
    assert!(!group.takes_grade);
    // The import's project is left with no lights, so it goes too.
    let [project] = plan.projects.as_slice() else {
        panic!("{plan:?}");
    };
    assert_eq!(
        (
            project.id,
            project.name.as_str(),
            project.targets.as_slice()
        ),
        (
            pair.import_project,
            "M 45 RGB",
            ["M 45 RGB".to_string()].as_slice()
        )
    );
    assert_eq!(project.exposure_plans, 1);
    assert!(plan.targets.is_empty());

    let before: (i64, i64, i64, String, String) = pair
        .catalog
        .conn
        .query_row(
            "SELECT projectId, targetId, exposureId, profileId, metadata FROM acquiredimage
             WHERE guid = 'ts-light'",
            [],
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
        .unwrap();
    let FoldOutcome::Folded(done) =
        fold(&mut pair.catalog.conn, Some(&plan.digest), &keepers()).unwrap()
    else {
        panic!("the catalog did not change");
    };
    assert_eq!(done.groups.len(), 1);

    let catalog = &pair.catalog;
    assert_eq!(catalog.count("SELECT COUNT(*) FROM acquiredimage"), 1);
    let after: (i64, i64, i64, String, String) = catalog
        .conn
        .query_row(
            "SELECT projectId, targetId, exposureId, profileId, metadata FROM acquiredimage
             WHERE guid = 'ts-light'",
            [],
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
        .unwrap();
    assert_eq!(after, before, "Target Scheduler's light stays as it was");
    assert_eq!(catalog.counts(pair.ts_plan), (1, 1));
    assert_eq!(
        catalog.count(&format!(
            "SELECT COUNT(*) FROM project WHERE Id = {}",
            pair.import_project
        )),
        0
    );
    assert_eq!(
        catalog.count(&format!(
            "SELECT COUNT(*) FROM target WHERE Id = {}",
            pair.import_target
        )),
        0
    );
    assert_eq!(
        catalog.count(&format!(
            "SELECT COUNT(*) FROM exposureplan WHERE Id = {}",
            pair.import_plan
        )),
        0
    );
    assert_eq!(
        catalog.count(&format!(
            "SELECT COUNT(*) FROM ruleweight WHERE projectid = {}",
            pair.import_project
        )),
        0
    );
    // A sync brings back neither the copy nor the project.
    assert_eq!(
        catalog.count(
            "SELECT COUNT(*) FROM psf_guard_merged_light
             WHERE removed_guid = 'import-light' AND kept_guid = 'ts-light'"
        ),
        1
    );
    assert_eq!(
        catalog.count(
            "SELECT COUNT(*) FROM psf_guard_removed_project WHERE project_guid = 'project-M 45 RGB'"
        ),
        1
    );
    assert_eq!(
        catalog
            .conn
            .query_row("PRAGMA integrity_check", [], |row| row.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    // Nothing is left to do.
    let again = super::plan(&catalog.conn, &keepers()).unwrap();
    assert!(again.is_empty() && again.held.is_empty(), "{again:?}");
}

#[test]
fn target_schedulers_light_takes_the_copys_grade_when_it_has_none() {
    let mut accepted = pair(0, 1);
    let plan = super::plan(&accepted.catalog.conn, &keepers()).unwrap();
    assert!(plan.groups[0].takes_grade);
    fold(&mut accepted.catalog.conn, Some(&plan.digest), &keepers()).unwrap();
    assert_eq!(accepted.catalog.grade("ts-light"), (1, None));
    // Its plan counts one more accepted frame and the same acquired.
    assert_eq!(accepted.catalog.counts(accepted.ts_plan), (1, 1));

    let mut rejected = pair(0, 2);
    crate::db::record_rejection_times_at(&rejected.catalog.conn, 1_790_000_000).unwrap();
    let plan = super::plan(&rejected.catalog.conn, &keepers()).unwrap();
    fold(&mut rejected.catalog.conn, Some(&plan.digest), &keepers()).unwrap();
    assert_eq!(
        rejected.catalog.grade("ts-light"),
        (2, Some("Stars".into()))
    );
    assert_eq!(rejected.catalog.counts(rejected.ts_plan), (1, 0));
    // The reject keeps the day it was rejected.
    assert_eq!(
        rejected.catalog.count(
            "SELECT rejected_at FROM psf_guard_rejected_at WHERE acquired_image_guid = 'ts-light'"
        ),
        1_790_000_000
    );

    // A Pending copy leaves Target Scheduler's grade alone.
    let mut pending = pair(2, 0);
    let plan = super::plan(&pending.catalog.conn, &keepers()).unwrap();
    assert!(!plan.groups[0].takes_grade);
    fold(&mut pending.catalog.conn, Some(&plan.digest), &keepers()).unwrap();
    assert_eq!(pending.catalog.grade("ts-light"), (2, Some("Stars".into())));
}

#[test]
fn lights_with_different_grades_stay() {
    for (ts, import) in [(1, 2), (2, 1)] {
        let mut pair = pair(ts, import);
        let plan = super::plan(&pair.catalog.conn, &keepers()).unwrap();
        assert!(plan.groups.is_empty(), "{plan:?}");
        let [held] = plan.held.as_slice() else {
            panic!("{plan:?}");
        };
        assert_eq!(
            held.reason,
            "the lights have different grades; grade them to match"
        );
        // The import keeps its light, so its project stays.
        assert!(plan.projects.is_empty() && plan.targets.is_empty());
        assert!(plan.is_empty());
        let FoldOutcome::Folded(_) =
            fold(&mut pair.catalog.conn, Some(&plan.digest), &keepers()).unwrap()
        else {
            panic!("the catalog did not change");
        };
        assert_eq!(pair.catalog.count("SELECT COUNT(*) FROM acquiredimage"), 2);
    }
}

#[test]
fn only_the_same_exposure_under_the_same_name_is_a_copy() {
    let other = |captured: &str, acquired: i64, filter: &str, name: &str| {
        let pair = pair(1, 0);
        pair.catalog
            .conn
            .execute(
                "UPDATE acquiredimage SET acquireddate = ?1, filtername = ?2,
                     metadata = json_set(metadata, '$.ExposureStartTime', ?3,
                         '$.FilterName', ?2, '$.FileName', ?4)
                 WHERE guid = 'import-light'",
                params![acquired, filter, captured, name],
            )
            .unwrap();
        super::plan(&pair.catalog.conn, &keepers()).unwrap()
    };
    let same = format!("/capture/{NAME}");
    // Another second, another filter or another name is another frame.
    for plan in [
        other("2026-10-04T02:13:41Z", ACQUIRED + 30, "Luminance", &same),
        other(&format!("{CAPTURED}Z"), ACQUIRED, "Red", &same),
        other(
            &format!("{CAPTURED}Z"),
            ACQUIRED,
            "Luminance",
            "/capture/M 45 RGB_LIGHT_Luminance_30.00s_2026-10-04_03-13-11_0053.fits",
        ),
    ] {
        assert!(plan.groups.is_empty() && plan.held.is_empty(), "{plan:?}");
    }

    // Two imports of the file, with no record from Target Scheduler, are
    // not this repair's.
    let pair = pair(0, 0);
    pair.catalog
        .conn
        .execute(
            "UPDATE acquiredimage SET metadata = json_remove(metadata, '$.GuidingRMS')
             WHERE guid = 'ts-light'",
            [],
        )
        .unwrap();
    let plan = super::plan(&pair.catalog.conn, &keepers()).unwrap();
    assert!(plan.groups.is_empty() && plan.held.is_empty(), "{plan:?}");
}

#[test]
fn a_second_record_from_target_scheduler_or_a_copy_that_must_stay_holds_the_group() {
    let pair = pair(1, 0);
    let catalog = &pair.catalog;
    let other_project = catalog.project("old-profile", "M45 again", false, 3);
    let other_target = catalog.target(other_project, "M45 again");
    let other_plan = catalog.plan(other_target, "old-profile", 1, 1);
    catalog.light(Row {
        project: other_project,
        target: other_target,
        plan: other_plan,
        profile: "old-profile",
        guid: "old-ts-light",
        file_name: format!(r"D:\NINA\{NAME}"),
        captured: "2026-10-04T02:13:11.4882859Z",
        acquired: ACQUIRED,
        filter: "Luminance",
        grade: 1,
        nina: true,
    });
    let plan = super::plan(&catalog.conn, &keepers()).unwrap();
    assert!(plan.groups.is_empty(), "{plan:?}");
    assert_eq!(
        plan.held[0].reason,
        "Target Scheduler recorded this exposure more than once"
    );
    // Neither project is the import's to remove.
    assert!(plan.projects.is_empty());

    let pair = self::pair(1, 0);
    let reported = Keepers {
        reported: BTreeSet::from(["import-light".to_string()]),
        ..Keepers::default()
    };
    let plan = super::plan(&pair.catalog.conn, &reported).unwrap();
    assert!(plan.groups.is_empty(), "{plan:?}");
    assert_eq!(
        plan.held[0].reason,
        "an imported copy must stay: it is in a collaboration report"
    );
}

#[test]
fn several_imports_of_the_file_all_fold_in() {
    let mut pair = pair(0, 0);
    let catalog = &pair.catalog;
    let second_project = catalog.project("third-profile", "M 45 again", true, 0);
    let second_target = catalog.target(second_project, "M 45 again");
    let second_plan = catalog.plan(second_target, "third-profile", 1, 1);
    catalog.light(Row {
        project: second_project,
        target: second_target,
        plan: second_plan,
        profile: "third-profile",
        guid: "second-import",
        file_name: format!("/mnt/archive/{NAME}"),
        captured: &format!("{CAPTURED}Z"),
        acquired: ACQUIRED + 1,
        filter: "luminance",
        grade: 1,
        nina: false,
    });
    let plan = super::plan(&catalog.conn, &keepers()).unwrap();
    let [group] = plan.groups.as_slice() else {
        panic!("{plan:?}");
    };
    assert_eq!(group.copies.len(), 2);
    assert!(group.grade_from_copy(&group.copies[1]));
    assert_eq!(plan.projects.len(), 2);
    fold(&mut pair.catalog.conn, Some(&plan.digest), &keepers()).unwrap();
    assert_eq!(pair.catalog.count("SELECT COUNT(*) FROM acquiredimage"), 1);
    assert_eq!(pair.catalog.grade("ts-light"), (1, None));
    assert_eq!(pair.catalog.counts(pair.ts_plan), (1, 1));
    assert_eq!(pair.catalog.count("SELECT COUNT(*) FROM project"), 1);
}

#[test]
fn only_empty_targets_an_import_made_and_nothing_uses_are_deleted() {
    let pair = pair(1, 0);
    let catalog = &pair.catalog;
    // The import's project keeps a light of its own in a second target, so
    // only the emptied target goes.
    let kept_target = catalog.target(pair.import_project, "M 45 RGB Panel 2");
    let kept_plan = catalog.plan(kept_target, IMPORT_PROFILE, 1, 0);
    catalog.light(Row {
        project: pair.import_project,
        target: kept_target,
        plan: kept_plan,
        profile: IMPORT_PROFILE,
        guid: "import-only",
        file_name: "/capture/M 45 RGB_LIGHT_Luminance_0099.fits".into(),
        captured: "2026-10-05T02:13:11Z",
        acquired: ACQUIRED + 86_400,
        filter: "Luminance",
        grade: 0,
        nina: false,
    });
    // A project Target Scheduler made, with no lights, is not the import's.
    let ts_empty = catalog.project(TS_PROFILE, "Pleiades wide", false, 0);
    catalog.target(ts_empty, "Pleiades wide");
    // Nor is an empty one an import made that no merge took a light from:
    // a sync may have brought only its plan.
    let never = catalog.project(IMPORT_PROFILE, "M 94", true, 0);
    let never_target = catalog.target(never, "M 94");
    catalog.plan(never_target, IMPORT_PROFILE, 0, 0);
    let plan = super::plan(&catalog.conn, &keepers()).unwrap();
    assert!(plan.projects.is_empty(), "{plan:?}");
    let [target] = plan.targets.as_slice() else {
        panic!("{plan:?}");
    };
    assert_eq!(
        (target.id, target.project_id, target.exposure_plans),
        (pair.import_target, pair.import_project, 1)
    );

    // Planning's record of the target keeps it.
    catalog
        .conn
        .execute_batch(
            "CREATE TABLE psf_guard_director_target (target_guid TEXT PRIMARY KEY, project_guid TEXT);
             INSERT INTO psf_guard_director_target VALUES ('target-M 45 RGB', 'project-M 45 RGB');",
        )
        .unwrap();
    let plan = super::plan(&catalog.conn, &keepers()).unwrap();
    assert!(
        plan.targets.is_empty() && plan.projects.is_empty(),
        "{plan:?}"
    );
    assert_eq!(plan.groups.len(), 1, "the copy still goes");
}

#[test]
fn a_target_an_earlier_merge_emptied_goes_too() {
    // Merge renamed lights took the import's light, leaving its project
    // empty; this repair finds no copy left, but deletes the project.
    let mut pair = pair(1, 0);
    let catalog = &pair.catalog;
    let ts_light = catalog.count("SELECT Id FROM acquiredimage WHERE guid = 'ts-light'");
    let all = crate::renamed_lights::lights(&catalog.conn).unwrap();
    let kept = all.iter().find(|light| light.id == ts_light).unwrap();
    let copy = all.iter().find(|light| light.id != ts_light).unwrap();
    let removal = LightRemoval::new(&catalog.conn).unwrap();
    removal.remove(&catalog.conn, kept, copy, 1).unwrap();
    assert_eq!(
        catalog
            .conn
            .query_row(
                "SELECT target_guid FROM psf_guard_merged_light",
                [],
                |row| row.get::<_, String>(0)
            )
            .unwrap(),
        "target-M 45 RGB"
    );
    let plan = super::plan(&catalog.conn, &keepers()).unwrap();
    assert!(plan.groups.is_empty());
    assert_eq!(
        plan.projects
            .iter()
            .map(|project| project.id)
            .collect::<Vec<_>>(),
        [pair.import_project]
    );
    fold(&mut pair.catalog.conn, Some(&plan.digest), &keepers()).unwrap();
    assert_eq!(pair.catalog.count("SELECT COUNT(*) FROM project"), 1);
}

#[test]
fn a_planned_or_active_imported_project_stays() {
    let pair = pair(1, 0);
    let planned = Keepers {
        planned_projects: BTreeSet::from(["project-m 45 rgb".to_string()]),
        ..Keepers::default()
    };
    let plan = super::plan(&pair.catalog.conn, &planned).unwrap();
    assert_eq!(plan.groups.len(), 1);
    assert!(plan.projects.is_empty() && plan.targets.is_empty());
    assert_eq!(plan.kept_projects[0].reason, "a Director plan uses it");

    pair.catalog
        .conn
        .execute(
            "UPDATE project SET state = 1 WHERE Id = ?1",
            [pair.import_project],
        )
        .unwrap();
    let plan = super::plan(&pair.catalog.conn, &keepers()).unwrap();
    assert!(plan.projects.is_empty() && plan.targets.is_empty());
    assert_eq!(
        plan.kept_projects[0].reason,
        "it is active in Target Scheduler"
    );
}

#[test]
fn a_fold_refuses_a_catalog_that_changed_since_the_preview() {
    let mut pair = pair(1, 0);
    let plan = super::plan(&pair.catalog.conn, &keepers()).unwrap();
    pair.catalog
        .conn
        .execute(
            "UPDATE acquiredimage SET gradingStatus = 2 WHERE guid = 'import-light'",
            [],
        )
        .unwrap();
    let FoldOutcome::Changed(preview) =
        fold(&mut pair.catalog.conn, Some(&plan.digest), &keepers()).unwrap()
    else {
        panic!("the catalog changed");
    };
    assert_ne!(preview.digest, plan.digest);
    assert_eq!(preview.held.len(), 1);
    assert_eq!(pair.catalog.count("SELECT COUNT(*) FROM acquiredimage"), 2);
    assert_eq!(pair.catalog.count("SELECT COUNT(*) FROM project"), 2);

    // A light imported since the preview keeps the project it is in, which
    // the preview deletes: that counts as a change too.
    let pair = self::pair(2, 0);
    let plan = super::plan(&pair.catalog.conn, &keepers()).unwrap();
    assert_eq!(plan.projects.len(), 1);
    pair.catalog.light(Row {
        project: pair.import_project,
        target: pair.import_target,
        plan: pair.import_plan,
        profile: IMPORT_PROFILE,
        guid: "new-import",
        file_name: "/capture/M 45 RGB_LIGHT_Luminance_0099.fits".into(),
        captured: "2026-10-05T02:13:11Z",
        acquired: ACQUIRED + 86_400,
        filter: "Luminance",
        grade: 0,
        nina: false,
    });
    let outcome = repair_database(&pair.catalog.path, Some(&plan.digest), &keepers()).unwrap();
    assert!(matches!(outcome.outcome, FoldOutcome::Changed(_)));
    assert!(outcome.backup.is_none());
}

#[test]
fn the_repair_copies_the_database_first_and_only_when_it_changes_something() {
    let pair = pair(1, 0);
    let plan = super::plan(&pair.catalog.conn, &keepers()).unwrap();
    let repaired = repair_database(&pair.catalog.path, Some(&plan.digest), &keepers()).unwrap();
    let backup = repaired.backup.expect("a copy before the change");
    let copy = Connection::open(&backup).unwrap();
    assert_eq!(
        copy.query_row("SELECT COUNT(*) FROM acquiredimage", [], |row| row
            .get::<_, i64>(0))
            .unwrap(),
        2
    );
    let FoldOutcome::Folded(done) = repaired.outcome else {
        panic!("the catalog did not change");
    };
    assert_eq!((done.groups.len(), done.projects.len()), (1, 1));

    let again = repair_database(&pair.catalog.path, None, &keepers()).unwrap();
    assert!(again.backup.is_none());
    let FoldOutcome::Folded(nothing) = again.outcome else {
        panic!("nothing changed");
    };
    assert!(nothing.is_empty());
}
