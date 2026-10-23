use super::*;
use crate::commands::sync::{sync_arrivals, sync_pull, PullOptions};
use std::collections::HashSet;

const NOW: i64 = 1_790_000_000;

/// Two projects with the rows Target Scheduler and PSF Guard keep for each:
/// M31 (Id 1) with a target, an exposure plan, three frames and every side
/// record; M42 (Id 2) with one of each, which deleting M31 must leave.
fn catalog() -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    crate::ts_schema::apply_schema(&conn).unwrap();
    conn.execute_batch(
        r#"INSERT INTO project (Id, profileId, name, state, guid) VALUES
             (1, 'p', 'M31', 3, 'pg-m31'), (2, 'p', 'M42', 1, 'pg-m42');
           INSERT INTO target (Id, name, active, epochcode, projectid, guid) VALUES
             (10, 'M31', 1, 0, 1, 'tg-m31'), (20, 'M42', 1, 0, 2, 'tg-m42');
           INSERT INTO exposuretemplate (Id, profileId, name, filtername, guid) VALUES (1, 'p', 'L', 'L', 'xg');
           INSERT INTO exposureplan (Id, profileId, exposure, desired, acquired, accepted, targetid, exposureTemplateId, guid) VALUES
             (100, 'p', 300, 10, 3, 1, 10, 1, 'eg-m31'), (200, 'p', 300, 10, 1, 1, 20, 1, 'eg-m42');
           INSERT INTO ruleweight (Id, name, weight, projectid) VALUES (1, 'Priority', 0.5, 1), (2, 'Priority', 0.5, 2);
           INSERT INTO overrideexposureorderitem (Id, targetid, "order", action, referenceIdx) VALUES (1, 10, 0, 0, 0), (2, 20, 0, 0, 0);
           INSERT INTO filtercadenceitem (Id, targetid, "order", next, action, referenceIdx) VALUES (1, 10, 0, 1, 0, 0), (2, 20, 0, 1, 0, 0);
           INSERT INTO flathistory (Id, targetId, profileId) VALUES (1, 10, 'p'), (2, 20, 'p');
           INSERT INTO acquiredimage
             (Id, projectId, targetId, acquireddate, filtername, gradingStatus, metadata, guid, exposureId, profileId) VALUES
             (1, 1, 10, 1759300000, 'L', 1, '{"FileName":"C:\\N\\M31_L_001.fits"}', 'g1', 100, 'p'),
             (2, 1, 10, 1759300300, 'L', 2, '{"FileName":"M31_L_002.FITS"}', 'g2', 100, 'p'),
             (3, 1, 10, 1759300600, 'L', 0, '{"FileName":"M31_L_003.fits"}', 'g3', 100, 'p'),
             (4, 2, 20, 1759300900, 'L', 1, '{"FileName":"M42_L_001.fits"}', 'g4', 200, 'p');
           INSERT INTO imagedata (tag, imagedata, acquiredimageid) VALUES ('thumb', X'01', 1), ('thumb', X'02', 4);
           CREATE TABLE psf_guard_rejected_at (acquired_image_guid TEXT PRIMARY KEY, rejected_at INTEGER);
           INSERT INTO psf_guard_rejected_at VALUES ('g2', 1), ('g4', 1);
           CREATE TABLE psf_guard_flat_light_angle (light_id INTEGER PRIMARY KEY, rig_uuid TEXT);
           INSERT INTO psf_guard_flat_light_angle VALUES (1, 'r'), (4, 'r');
           CREATE TABLE psf_guard_archive (acquired_image_guid TEXT PRIMARY KEY, acquired_image_id INTEGER);
           INSERT INTO psf_guard_archive VALUES ('g2', 2), ('g4', 4);
           CREATE TABLE psf_guard_ts_sync_grade (guid TEXT PRIMARY KEY NOT NULL,
             local_grade INTEGER NOT NULL, local_reason TEXT, source_grade INTEGER NOT NULL, source_reason TEXT);
           INSERT INTO psf_guard_ts_sync_grade VALUES ('g1', 1, NULL, 1, NULL), ('g4', 1, NULL, 1, NULL);
           CREATE TABLE psf_guard_ts_sync_pending (guid TEXT PRIMARY KEY NOT NULL, grade INTEGER NOT NULL, reason TEXT);
           INSERT INTO psf_guard_ts_sync_pending VALUES ('g2', 2, NULL), ('g4', 1, NULL);
           CREATE TABLE psf_guard_flat_light_choice (light_guid TEXT);
           INSERT INTO psf_guard_flat_light_choice VALUES ('g1'), ('g4');
           CREATE TABLE psf_guard_remote_image_file (acquiredimage_id INTEGER PRIMARY KEY);
           INSERT INTO psf_guard_remote_image_file VALUES (1), (4);
           CREATE TABLE psf_guard_director_project (project_guid TEXT PRIMARY KEY, global_project_id TEXT);
           INSERT INTO psf_guard_director_project VALUES ('pg-m31', 'plan-a'), ('pg-m42', 'plan-b');
           CREATE TABLE psf_guard_director_target (target_guid TEXT PRIMARY KEY, project_guid TEXT);
           INSERT INTO psf_guard_director_target VALUES ('tg-m31', 'pg-m31'), ('tg-gone', 'PG-M31'), ('tg-m42', 'pg-m42');
           CREATE TABLE psf_guard_director_plan (exposureplan_guid TEXT PRIMARY KEY, target_guid TEXT);
           INSERT INTO psf_guard_director_plan VALUES ('eg-m31', 'tg-m31'), ('eg-m42', 'tg-m42');
           CREATE TABLE psf_guard_collaboration_plan (exposureplan_guid TEXT, target_guid TEXT);
           INSERT INTO psf_guard_collaboration_plan VALUES ('eg-m31', 'tg-m31'), ('eg-m42', 'tg-m42');
           CREATE TABLE psf_guard_scheduler_flat_history (record_id TEXT PRIMARY KEY, target_guid TEXT);
           INSERT INTO psf_guard_scheduler_flat_history VALUES ('r1', 'tg-m31'), ('r2', 'tg-m42');
           CREATE TABLE psf_guard_project_processing (project_key TEXT PRIMARY KEY, split_exposure_groups INTEGER);
           INSERT INTO psf_guard_project_processing VALUES ('guid:pg-m31', 1), ('guid:pg-m42', 1);"#,
    )
    .unwrap();
    crate::frame_derivatives::ensure_schema(&conn).unwrap();
    conn.execute_batch(
        "INSERT INTO psf_guard_frame_derivative
           (derivative_uuid, acquired_image_guid, kind, steps, file_name, producer, evidence, created_at, updated_at)
         VALUES ('d1', 'g1', 'calibrated', 'c', 'M31_L_001_c.xisf', 'pixinsight', 'name', 1, 1),
                ('d4', 'g4', 'calibrated', 'c', 'M42_L_001_c.xisf', 'pixinsight', 'name', 1, 1);",
    )
    .unwrap();
    // Rejects removed from each project before, kept for their trash.
    crate::commands::reject_removal::ensure_schema(&conn).unwrap();
    conn.execute_batch(
        "INSERT INTO psf_guard_removed_image
           (acquired_image_guid, acquired_image_id, project_id, target_id, exposure_id, batch_id,
            removed_at, trash_until, rows_json, files_json) VALUES
           ('r-m31', 90, 1, 10, 100, 'b', 1, 2, '{}', '[]'),
           ('r-m42', 91, 2, 20, 200, 'b', 1, 2, '{}', '[]');",
    )
    .unwrap();
    conn
}

fn count(conn: &Connection, sql: &str) -> i64 {
    conn.query_row(sql, [], |row| row.get(0)).unwrap()
}

fn delete(conn: &Connection, project: i64) -> Outcome {
    let digest = plan(conn, project, &BTreeSet::new())
        .unwrap()
        .unwrap()
        .digest;
    remove(conn, project, &digest, &BTreeSet::new(), NOW).unwrap()
}

#[test]
fn the_plan_counts_what_goes() {
    let conn = catalog();
    let plan = plan(&conn, 1, &BTreeSet::new()).unwrap().unwrap();
    assert_eq!(plan.name, "M31");
    assert_eq!(plan.guid.as_deref(), Some("pg-m31"));
    assert!(plan.unique_guid);
    assert_eq!(plan.state, Some(3));
    assert_eq!((plan.targets, plan.exposure_plans), (1, 1));
    assert_eq!(
        plan.frames,
        FrameCounts {
            total: 3,
            accepted: 1,
            rejected: 1,
            pending: 1
        }
    );
    assert_eq!(plan.protected_frames, 0);
    assert!(super::plan(&conn, 99, &BTreeSet::new()).unwrap().is_none());
}

#[test]
fn deleting_a_project_deletes_its_rows_and_leaves_the_others() {
    let conn = catalog();
    let Outcome::Removed { frames, .. } = delete(&conn, 1) else {
        panic!("not deleted");
    };
    assert_eq!(
        frames
            .iter()
            .map(|frame| frame.image_id)
            .collect::<Vec<_>>(),
        [1, 2, 3]
    );
    for (table, column, gone, kept) in [
        ("project", "Id", 1, 2),
        ("target", "projectid", 1, 2),
        ("exposureplan", "targetid", 10, 20),
        ("ruleweight", "projectid", 1, 2),
        ("overrideexposureorderitem", "targetid", 10, 20),
        ("filtercadenceitem", "targetid", 10, 20),
        ("flathistory", "targetId", 10, 20),
        ("acquiredimage", "projectId", 1, 2),
        ("imagedata", "acquiredimageid", 1, 4),
        ("psf_guard_flat_light_angle", "light_id", 1, 4),
        ("psf_guard_remote_image_file", "acquiredimage_id", 1, 4),
    ] {
        assert_eq!(
            count(
                &conn,
                &format!("SELECT COUNT(*) FROM {table} WHERE {column} = {gone}")
            ),
            0,
            "{table} still has the deleted project's rows"
        );
        assert_eq!(
            count(
                &conn,
                &format!("SELECT COUNT(*) FROM {table} WHERE {column} = {kept}")
            ),
            1,
            "{table} lost the other project's rows"
        );
    }
    for (table, column, gone, kept) in [
        ("psf_guard_rejected_at", "acquired_image_guid", "g2", "g4"),
        (
            "psf_guard_frame_derivative",
            "acquired_image_guid",
            "g1",
            "g4",
        ),
        ("psf_guard_archive", "acquired_image_guid", "g2", "g4"),
        ("psf_guard_ts_sync_grade", "guid", "g1", "g4"),
        ("psf_guard_ts_sync_pending", "guid", "g2", "g4"),
        ("psf_guard_flat_light_choice", "light_guid", "g1", "g4"),
        (
            "psf_guard_director_project",
            "project_guid",
            "pg-m31",
            "pg-m42",
        ),
        (
            "psf_guard_director_target",
            "lower(project_guid)",
            "pg-m31",
            "pg-m42",
        ),
        ("psf_guard_director_plan", "target_guid", "tg-m31", "tg-m42"),
        (
            "psf_guard_collaboration_plan",
            "target_guid",
            "tg-m31",
            "tg-m42",
        ),
        (
            "psf_guard_scheduler_flat_history",
            "target_guid",
            "tg-m31",
            "tg-m42",
        ),
        (
            "psf_guard_project_processing",
            "project_key",
            "guid:pg-m31",
            "guid:pg-m42",
        ),
    ] {
        assert_eq!(
            count(
                &conn,
                &format!("SELECT COUNT(*) FROM {table} WHERE {column} = '{gone}'")
            ),
            0,
            "{table} still has the deleted project's rows"
        );
        assert_eq!(
            count(
                &conn,
                &format!("SELECT COUNT(*) FROM {table} WHERE {column} = '{kept}'")
            ),
            1,
            "{table} lost the other project's rows"
        );
    }
    // The exposure template belongs to the profile, not the project.
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM exposuretemplate"), 1);
    // A removed reject keeps its tombstone, but no longer counts toward
    // the target or exposure plan Ids.
    assert_eq!(
        count(&conn, "SELECT COUNT(*) FROM psf_guard_removed_image WHERE target_id IS NULL AND exposure_id IS NULL"),
        1
    );
    assert_eq!(
        count(&conn, "SELECT COUNT(*) FROM psf_guard_removed_image WHERE acquired_image_guid = 'r-m42' AND target_id = 20"),
        1
    );
    assert_eq!(
        removed_project_guids(&conn).unwrap(),
        BTreeSet::from(["pg-m31".to_string()])
    );
    let mut names = removed_file_names(&conn).unwrap();
    names.sort();
    assert_eq!(
        names,
        [
            "m31_l_001.fits",
            "m31_l_001_c.xisf",
            "m31_l_002.fits",
            "m31_l_003.fits"
        ]
    );
    // Import and sync read the frame tombstones with the reject removal's.
    assert!(crate::commands::reject_removal::removed_file_names(&conn)
        .unwrap()
        .contains("m31_l_002.fits"));
    assert!(crate::commands::reject_removal::removed_guids(&conn)
        .unwrap()
        .contains("g3"));
}

#[test]
fn frames_on_the_projects_targets_go_with_it() {
    let conn = catalog();
    // A frame recorded under M42 but on M31's target.
    conn.execute(
        "INSERT INTO acquiredimage (Id, projectId, targetId, acquireddate, filtername, gradingStatus, metadata, guid)
         VALUES (5, 2, 10, 1759301000, 'L', 0, '{}', 'g5')",
        [],
    )
    .unwrap();
    // And a reject removed from M42 off M31's target.
    conn.execute(
        "INSERT INTO psf_guard_removed_image
           (acquired_image_guid, acquired_image_id, project_id, target_id, exposure_id, batch_id,
            removed_at, trash_until, rows_json, files_json)
         VALUES ('r-x', 92, 2, 10, 100, 'b', 1, 2, '{}', '[]')",
        [],
    )
    .unwrap();
    assert_eq!(
        plan(&conn, 1, &BTreeSet::new())
            .unwrap()
            .unwrap()
            .frames
            .total,
        4
    );
    let Outcome::Removed { frames, .. } = delete(&conn, 1) else {
        panic!("not deleted");
    };
    // Its cache files are keyed by the project it was recorded under.
    assert_eq!(
        frames
            .iter()
            .find(|frame| frame.image_id == 5)
            .map(|frame| frame.project_id),
        Some(2)
    );
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM acquiredimage WHERE targetId = 10"
        ),
        0
    );
    assert_eq!(
        count(&conn, "SELECT COUNT(*) FROM acquiredimage WHERE Id = 4"),
        1
    );
    assert_eq!(
        count(&conn, "SELECT COUNT(*) FROM psf_guard_removed_image WHERE acquired_image_guid = 'r-x' AND target_id IS NULL"),
        1
    );
}

#[test]
fn a_frame_captured_after_the_preview_stops_the_deletion() {
    let conn = catalog();
    let digest = plan(&conn, 1, &BTreeSet::new()).unwrap().unwrap().digest;
    conn.execute(
        "INSERT INTO acquiredimage (Id, projectId, targetId, acquireddate, filtername, gradingStatus, metadata, guid)
         VALUES (5, 1, 10, 1759301000, 'L', 0, '{}', 'g5')",
        [],
    )
    .unwrap();
    let Outcome::Changed(fresh) = remove(&conn, 1, &digest, &BTreeSet::new(), NOW).unwrap() else {
        panic!("deleted a changed project");
    };
    assert_eq!(fresh.frames.total, 4);
    assert_ne!(fresh.digest, digest);
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM acquiredimage WHERE projectId = 1"
        ),
        4
    );
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM project"), 2);
}

#[test]
fn frames_reported_to_a_collaboration_keep_their_project() {
    let conn = catalog();
    let protected = BTreeSet::from(["g2".to_string()]);
    let plan = plan(&conn, 1, &protected).unwrap().unwrap();
    assert_eq!(plan.protected_frames, 1);
    assert!(matches!(
        remove(&conn, 1, &plan.digest, &protected, NOW).unwrap(),
        Outcome::Protected(_)
    ));
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM acquiredimage WHERE projectId = 1"
        ),
        3
    );
}

#[test]
fn a_missing_project_deletes_nothing() {
    let conn = catalog();
    assert_eq!(
        remove(&conn, 99, "digest", &BTreeSet::new(), NOW).unwrap(),
        Outcome::Missing
    );
}

/// A damaged catalog with two projects under one GUID: the GUID names
/// neither, so it blocks no sync and the twin keeps its Planning row.
#[test]
fn a_duplicate_guid_is_not_tombstoned() {
    let conn = catalog();
    conn.execute(
        "INSERT INTO project (Id, profileId, name, guid) VALUES (3, 'p', 'M31 twin', 'PG-M31')",
        [],
    )
    .unwrap();
    assert!(
        !plan(&conn, 1, &BTreeSet::new())
            .unwrap()
            .unwrap()
            .unique_guid
    );
    delete(&conn, 1);
    assert!(removed_project_guids(&conn).unwrap().is_empty());
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM psf_guard_director_project WHERE project_guid = 'pg-m31'"
        ),
        1
    );
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM psf_guard_removed_project WHERE project_id = 1"
        ),
        1
    );
}

/// Target Scheduler can give a deleted project's Id to the next project. A
/// reject removed from that new project is not one of the deleted
/// project's frames.
#[test]
fn a_deletion_counts_only_for_frames_removed_before_it() {
    let conn = catalog();
    delete(&conn, 1);
    assert!(
        project_removed_since(&conn, 1, NOW - 10).unwrap(),
        "the row is gone"
    );
    conn.execute(
        "INSERT INTO project (Id, profileId, name, guid) VALUES (1, 'p', 'New', 'pg-new')",
        [],
    )
    .unwrap();
    assert!(project_removed_since(&conn, 1, NOW - 10).unwrap());
    assert!(project_removed_since(&conn, 1, NOW).unwrap());
    assert!(!project_removed_since(&conn, 1, NOW + 10).unwrap());
    assert!(!project_removed_since(&conn, 2, NOW - 10).unwrap());
}

/// The Target Scheduler database a catalog pulls from still has the
/// project. Neither a full pull nor the automatic one brings it back, and
/// neither lists its rows as skipped; the other project keeps arriving.
#[test]
fn a_sync_does_not_bring_a_deleted_project_back() {
    let source = catalog();
    source
        .execute(
            "INSERT INTO acquiredimage (Id, projectId, targetId, acquireddate, filtername, gradingStatus, metadata, guid, profileId)
             VALUES (6, 2, 20, 1759302000, 'L', 0, '{\"FileName\":\"M42_L_002.fits\"}', 'g6', 'p')",
            [],
        )
        .unwrap();
    let options = PullOptions {
        dry_run: false,
        with_image_data: false,
        project_filter: None,
        deletions: Default::default(),
    };
    let left_out = |local: &Connection| {
        assert_eq!(
            count(local, "SELECT COUNT(*) FROM project WHERE guid = 'pg-m31'"),
            0
        );
        assert_eq!(
            count(local, "SELECT COUNT(*) FROM target WHERE guid = 'tg-m31'"),
            0
        );
        assert_eq!(
            count(
                local,
                "SELECT COUNT(*) FROM exposureplan WHERE guid = 'eg-m31'"
            ),
            0
        );
        assert_eq!(
            count(
                local,
                "SELECT COUNT(*) FROM acquiredimage WHERE guid IN ('g1','g2','g3')"
            ),
            0
        );
        assert_eq!(
            count(
                local,
                "SELECT COUNT(*) FROM acquiredimage WHERE guid = 'g6'"
            ),
            1
        );
    };

    let local = catalog();
    delete(&local, 1);
    let summary = sync_pull(&source, &local, &options).unwrap();
    left_out(&local);
    assert_eq!(
        (
            summary.project.skipped,
            summary.target.skipped,
            summary.exposureplan.skipped,
            summary.acquiredimage.skipped
        ),
        (0, 0, 0, 0),
        "{:?}",
        summary.changes
    );
    // One line says the project was left out; none lists its rows.
    assert_eq!(
        summary
            .changes
            .iter()
            .filter(|line| line.contains("M31") || line.contains("m31") || line.contains("g1"))
            .collect::<Vec<_>>(),
        ["leave out project \"M31\": deleted in this catalog (Allow again in the Library lets it back)"],
    );

    let local = catalog();
    delete(&local, 1);
    let tx = Transaction::new_unchecked(&local, TransactionBehavior::Immediate).unwrap();
    let arrivals = sync_arrivals(&source, &tx, &HashSet::new(), &Default::default()).unwrap();
    tx.commit().unwrap();
    left_out(&local);
    assert_eq!(arrivals.acquiredimage.skipped, 0, "{:?}", arrivals.changes);
    assert!(
        arrivals.changes.iter().all(|line| !line.contains("M31")),
        "{:?}",
        arrivals.changes
    );
}

/// Allowing a deleted project back lets the next sync add it again, and
/// its files' next import. Restore still refuses rejects removed before the
/// deletion, whatever project now has the Id.
#[test]
fn a_deleted_project_can_be_allowed_back() {
    let source = catalog();
    let local = catalog();
    delete(&local, 1);
    let listed = deletions(&local).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!((listed[0].name.as_str(), listed[0].frames), ("M31", 3));
    assert!(allow_back(&local, listed[0].id, NOW + 5).unwrap());
    assert!(
        !allow_back(&local, listed[0].id, NOW + 6).unwrap(),
        "once is enough"
    );
    assert!(deletions(&local).unwrap().is_empty());
    assert!(removed_project_guids(&local).unwrap().is_empty());
    assert!(removed_file_names(&local).unwrap().is_empty());
    assert!(project_removed_since(&local, 1, NOW - 10).unwrap());

    let options = PullOptions {
        dry_run: false,
        with_image_data: false,
        project_filter: None,
        deletions: Default::default(),
    };
    sync_pull(&source, &local, &options).unwrap();
    assert_eq!(
        count(&local, "SELECT COUNT(*) FROM project WHERE guid = 'pg-m31'"),
        1
    );
    assert_eq!(
        count(
            &local,
            "SELECT COUNT(*) FROM acquiredimage WHERE guid IN ('g1','g2','g3')"
        ),
        3
    );
}

/// Add a project without frames: Empty (Id 3), with a target and an
/// exposure plan.
fn add_empty_project(conn: &Connection) {
    conn.execute_batch(
        "INSERT INTO project (Id, profileId, name, state, guid) VALUES (3, 'p', 'Empty', 1, 'pg-empty');
         INSERT INTO target (Id, name, active, epochcode, projectid, guid) VALUES (30, 'Empty', 1, 0, 3, 'tg-empty');
         INSERT INTO exposureplan (Id, profileId, exposure, desired, acquired, accepted, targetid, exposureTemplateId, guid)
           VALUES (300, 'p', 300, 10, 0, 0, 30, 1, 'eg-empty');",
    )
    .unwrap();
}

fn pull_options(dry_run: bool) -> PullOptions {
    PullOptions {
        dry_run,
        with_image_data: false,
        project_filter: None,
        deletions: Default::default(),
    }
}

fn has_table(conn: &Connection, table: &str) -> bool {
    count(
        conn,
        &format!("SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = '{table}'"),
    ) > 0
}

/// A project without frames stays deleted: no sync brings it back. Deleted
/// projects lists it only once a sync has left it out, and Allow again
/// works for it as for any other. Restore still sees its deletion.
#[test]
fn an_unshot_project_stays_deleted_and_is_listed_once_a_sync_leaves_it_out() {
    let source = catalog();
    add_empty_project(&source);
    let local = catalog();
    add_empty_project(&local);
    assert!(matches!(delete(&local, 3), Outcome::Removed { .. }));
    assert_eq!(
        count(&local, "SELECT COUNT(*) FROM project WHERE Id = 3"),
        0
    );
    assert_eq!(
        count(&local, "SELECT COUNT(*) FROM target WHERE Id = 30"),
        0
    );
    assert!(removed_project_guids(&local).unwrap().contains("pg-empty"));
    assert!(deletions(&local).unwrap().is_empty(), "not listed yet");

    // A dry run leaves it unlisted.
    sync_pull(&source, &local, &pull_options(true)).unwrap();
    assert!(deletions(&local).unwrap().is_empty());

    let summary = sync_pull(&source, &local, &pull_options(false)).unwrap();
    assert_eq!(
        count(
            &local,
            "SELECT COUNT(*) FROM project WHERE guid = 'pg-empty'"
        ),
        0,
        "{:?}",
        summary.changes
    );
    let listed = deletions(&local).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!((listed[0].name.as_str(), listed[0].frames), ("Empty", 0));

    assert!(allow_back(&local, listed[0].id, NOW + 5).unwrap());
    assert!(deletions(&local).unwrap().is_empty());
    sync_pull(&source, &local, &pull_options(false)).unwrap();
    assert_eq!(
        count(
            &local,
            "SELECT COUNT(*) FROM project WHERE guid = 'pg-empty'"
        ),
        1
    );
    // The dated row stays for Restore in reject removal.
    assert_eq!(
        count(
            &local,
            "SELECT COUNT(*) FROM psf_guard_removed_project WHERE project_id = 3 AND removed_at = 1790000000"
        ),
        1
    );
}

/// A table from before syncs recorded leaving a project out gains the
/// column, and lists deletions with frames meanwhile.
#[test]
fn an_older_tombstone_table_gains_the_new_columns() {
    let conn = catalog();
    // The tables as the first release of project removal made them.
    conn.execute_batch(
        "CREATE TABLE psf_guard_removed_project (id INTEGER PRIMARY KEY, project_id INTEGER NOT NULL,
           project_guid TEXT, name TEXT NOT NULL, removed_at INTEGER NOT NULL, frames INTEGER NOT NULL,
           allowed_back_at INTEGER);
         CREATE TABLE psf_guard_removed_project_frame (removal_id INTEGER NOT NULL,
           acquired_image_id INTEGER NOT NULL, acquired_image_guid TEXT, project_id INTEGER NOT NULL,
           project_guid TEXT, file_name TEXT, removed_at INTEGER NOT NULL);",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO psf_guard_removed_project (project_id, project_guid, name, removed_at, frames)
         VALUES (5, 'pg-old', 'Old', 1, 2)",
        [],
    )
    .unwrap();
    assert_eq!(deletions(&conn).unwrap().len(), 1);
    assert!(deleted_project_frames_named(&conn, "a.fits")
        .unwrap()
        .is_empty());
    ensure_schema(&conn).unwrap();
    for (table, column) in [
        ("psf_guard_removed_project", "left_out_at"),
        ("psf_guard_removed_project", "origin"),
        ("psf_guard_removed_project", "left_out_source_frames"),
        ("psf_guard_removed_project_frame", "acquired_at"),
    ] {
        assert!(has_column(&conn, table, column), "{table}.{column}");
    }
    assert_eq!(
        count(
            &conn,
            "SELECT COUNT(*) FROM psf_guard_removed_project WHERE origin = 'local'"
        ),
        1
    );
    assert_eq!(deletions(&conn).unwrap().len(), 1);
}

/// A pull leaves out the frames a rig still captures for a deleted project,
/// and records their files so an upload or import of them is skipped. A
/// second pull adds nothing twice; a dry run adds nothing.
#[test]
fn a_pull_records_the_files_of_frames_it_leaves_out() {
    let source = catalog();
    source
        .execute(
            "INSERT INTO acquiredimage (Id, projectId, targetId, acquireddate, filtername, gradingStatus, metadata, guid, profileId)
             VALUES (7, 1, 10, 1759302000, 'L', 0, '{\"FileName\":\"C:\\\\N\\\\M31_L_007.FITS\"}', 'g7', 'p')",
            [],
        )
        .unwrap();
    let local = catalog();
    delete(&local, 1);
    let frames = || {
        count(
            &local,
            "SELECT COUNT(*) FROM psf_guard_removed_project_frame",
        )
    };
    let before = frames();

    sync_pull(&source, &local, &pull_options(true)).unwrap();
    assert_eq!(frames(), before, "a dry run records nothing");

    sync_pull(&source, &local, &pull_options(false)).unwrap();
    assert_eq!(frames(), before + 1);
    assert!(removed_file_names(&local)
        .unwrap()
        .contains(&"m31_l_007.fits".to_string()));
    assert_eq!(
        count(
            &local,
            "SELECT COUNT(*) FROM acquiredimage WHERE guid = 'g7'"
        ),
        0
    );
    assert_eq!(
        deleted_project_frames_named(&local, "M31_L_007.fits").unwrap(),
        [("M31".to_string(), Some(1759302000))]
    );

    sync_pull(&source, &local, &pull_options(false)).unwrap();
    assert_eq!(frames(), before + 1, "already recorded");

    // Allow again clears them with the rest.
    let listed = deletions(&local).unwrap();
    assert!(allow_back(&local, listed[0].id, NOW + 5).unwrap());
    assert!(deleted_project_frames_named(&local, "m31_l_007.fits")
        .unwrap()
        .is_empty());
}

/// A planning push into a rig's Target Scheduler database deletes a
/// project the source catalog deleted when nothing was captured for it
/// there, and writes no PSF Guard table. A project with frames there stays,
/// and the preview says why.
#[test]
fn a_planning_push_deletes_a_project_the_source_deleted_when_the_rig_has_no_frames_for_it() {
    use crate::commands::sync::{sync_planning, PlanningOptions};

    let source = catalog();
    add_empty_project(&source);
    delete(&source, 3);
    delete(&source, 1);

    let rig = Connection::open_in_memory().unwrap();
    crate::ts_schema::apply_schema(&rig).unwrap();
    rig.execute_batch(
        r#"INSERT INTO project (Id, profileId, name, state, guid) VALUES
             (1, 'p', 'M31', 1, 'pg-m31'), (2, 'p', 'M42', 1, 'pg-m42');
           INSERT INTO target (Id, name, active, epochcode, projectid, guid) VALUES
             (10, 'M31', 1, 0, 1, 'tg-m31'), (20, 'M42', 1, 0, 2, 'tg-m42');
           INSERT INTO exposuretemplate (Id, profileId, name, filtername, guid) VALUES (1, 'p', 'L', 'L', 'xg');
           INSERT INTO acquiredimage (Id, projectId, targetId, acquireddate, filtername, gradingStatus, metadata, guid, profileId)
             VALUES (1, 1, 10, 1759300000, 'L', 0, '{}', 'g1', 'p');"#,
    )
    .unwrap();
    add_empty_project(&rig);

    let options = |dry_run| PlanningOptions {
        dry_run,
        project_filter: None,
        destination: crate::commands::sync::SyncDestination::Scheduler,
        deletions: Default::default(),
    };
    let preview = sync_planning(&source, &rig, &options(true)).unwrap();
    assert_eq!(preview.project.deleted, 1);
    assert_eq!(
        count(&rig, "SELECT COUNT(*) FROM project WHERE guid = 'pg-empty'"),
        1,
        "a dry run deletes nothing"
    );

    let summary = sync_planning(&source, &rig, &options(false)).unwrap();
    assert_eq!(
        (
            summary.project.deleted,
            summary.target.deleted,
            summary.exposureplan.deleted
        ),
        (1, 1, 1)
    );
    assert!(summary
        .changes
        .contains(&"delete project \"Empty\": deleted in the source catalog".to_string()));
    assert!(
        summary.changes.contains(
            &"keep project \"M31\": deleted in the source catalog, but it has 1 exposure here"
                .to_string()
        ),
        "{:?}",
        summary.changes
    );
    assert_eq!(
        count(&rig, "SELECT COUNT(*) FROM project WHERE guid = 'pg-empty'"),
        0
    );
    assert_eq!(count(&rig, "SELECT COUNT(*) FROM target WHERE Id = 30"), 0);
    assert_eq!(
        count(&rig, "SELECT COUNT(*) FROM exposureplan WHERE Id = 300"),
        0
    );
    assert_eq!(
        count(&rig, "SELECT COUNT(*) FROM project WHERE guid = 'pg-m31'"),
        1
    );
    assert!(
        !has_table(&rig, "psf_guard_removed_project"),
        "a Target Scheduler database gets no tombstone"
    );
}

/// A pull from a catalog that deleted a project deletes the destination's
/// copy when nothing was captured for it there, and records the deletion
/// so a later sync from a rig does not bring it back. A removed reject that
/// Restore can bring back keeps the project.
#[test]
fn a_pull_deletes_a_project_the_source_catalog_deleted() {
    let source = catalog();
    add_empty_project(&source);
    delete(&source, 3);

    let local = catalog();
    add_empty_project(&local);
    let summary = sync_pull(&source, &local, &pull_options(true)).unwrap();
    assert_eq!(summary.project.deleted, 1);
    assert_eq!(
        count(
            &local,
            "SELECT COUNT(*) FROM project WHERE guid = 'pg-empty'"
        ),
        1,
        "a dry run deletes nothing"
    );
    assert!(removed_project_guids(&local).unwrap().is_empty());

    let summary = sync_pull(&source, &local, &pull_options(false)).unwrap();
    assert_eq!(summary.project.deleted, 1, "{:?}", summary.changes);
    assert_eq!(
        count(
            &local,
            "SELECT COUNT(*) FROM project WHERE guid = 'pg-empty'"
        ),
        0
    );
    assert!(removed_project_guids(&local).unwrap().contains("pg-empty"));
    assert!(deletions(&local).unwrap().is_empty(), "hidden");

    // The rig still has the project: the next pull leaves it out, and from
    // then on Deleted projects lists it.
    let rig = catalog();
    add_empty_project(&rig);
    sync_pull(&rig, &local, &pull_options(false)).unwrap();
    assert_eq!(
        count(
            &local,
            "SELECT COUNT(*) FROM project WHERE guid = 'pg-empty'"
        ),
        0
    );
    assert_eq!(deletions(&local).unwrap().len(), 1);

    // A removed reject Restore can bring back keeps the project.
    let local = catalog();
    add_empty_project(&local);
    local
        .execute(
            "INSERT INTO psf_guard_removed_image
               (acquired_image_guid, acquired_image_id, project_id, target_id, batch_id,
                removed_at, trash_until, rows_json, files_json)
             VALUES ('r-empty', 95, 3, 30, 'b', 1, 2, '{}', '[]')",
            [],
        )
        .unwrap();
    let summary = sync_pull(&source, &local, &pull_options(false)).unwrap();
    assert_eq!(summary.project.deleted, 0);
    assert!(summary.changes.contains(
        &"keep project \"Empty\": deleted in the source catalog, but it has 1 removed exposure here that Restore can bring back".to_string()
    ), "{:?}", summary.changes);
    assert_eq!(
        count(&local, "SELECT COUNT(*) FROM project WHERE Id = 3"),
        1
    );
}

fn deletion_row(conn: &Connection) -> (Option<String>, Option<i64>, String) {
    conn.query_row(
        "SELECT project_guid, allowed_back_at, origin FROM psf_guard_removed_project
         WHERE lower(project_guid) = 'pg-empty' ORDER BY id DESC LIMIT 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )
    .unwrap()
}

/// Allow again in one catalog is not undone by a sync with a catalog that
/// deleted the project after the first deletion, and it carries to that
/// catalog. A deletes Empty; B pulls from A and deletes it too; A allows it
/// back and a rig brings it back to A. A pulls from B, B pushes planning to
/// A: Empty stays in A. B pulls from A: B's deletion is allowed back, and
/// Empty comes back to B.
#[test]
fn allow_again_survives_a_round_trip_between_catalogs() {
    use crate::commands::sync::{sync_planning, PlanningOptions, SyncDestination};

    let empty_in =
        |conn: &Connection| count(conn, "SELECT COUNT(*) FROM project WHERE guid = 'pg-empty'");
    let rig = catalog();
    add_empty_project(&rig);
    let a = catalog();
    add_empty_project(&a);
    let b = catalog();
    add_empty_project(&b);

    delete(&a, 3);
    assert_eq!(
        deletion_row(&a),
        (Some("pg-empty".into()), None, "local".into())
    );
    sync_pull(&a, &b, &pull_options(false)).unwrap();
    assert_eq!(empty_in(&b), 0);
    assert_eq!(
        deletion_row(&b),
        (Some("pg-empty".into()), None, "sync".into())
    );

    let id = count(&a, "SELECT id FROM psf_guard_removed_project");
    let allowed_at = NOW + 100;
    assert!(allow_back(&a, id, allowed_at).unwrap());
    // Allow again keeps the GUID, so syncs can pass the choice on.
    assert_eq!(
        deletion_row(&a),
        (Some("pg-empty".into()), Some(allowed_at), "local".into())
    );
    assert!(removed_project_guids(&a).unwrap().is_empty());
    sync_pull(&rig, &a, &pull_options(false)).unwrap();
    assert_eq!(empty_in(&a), 1, "the rig brings it back");

    let summary = sync_pull(&b, &a, &pull_options(false)).unwrap();
    assert_eq!(empty_in(&a), 1, "{:?}", summary.changes);
    assert!(
        summary.changes.contains(
            &"keep project \"Empty\": deleted in the source catalog, but it was allowed back here after that"
                .to_string()
        ),
        "{:?}",
        summary.changes
    );
    let pushed = sync_planning(
        &b,
        &a,
        &PlanningOptions {
            dry_run: false,
            project_filter: None,
            destination: SyncDestination::Catalog,
            deletions: Default::default(),
        },
    )
    .unwrap();
    assert_eq!(empty_in(&a), 1, "{:?}", pushed.changes);

    let summary = sync_pull(&a, &b, &pull_options(false)).unwrap();
    assert!(
        summary.changes.contains(
            &"allow project \"Empty\" back: allowed back in the source catalog".to_string()
        ),
        "{:?}",
        summary.changes
    );
    assert_eq!(
        deletion_row(&b),
        (Some("pg-empty".into()), Some(allowed_at), "sync".into())
    );
    assert_eq!(empty_in(&b), 1, "back in B with the same pull");
}

/// A deletion made here after the source allowed the project back stands.
#[test]
fn a_later_deletion_here_outlasts_an_older_allow_again() {
    let source = catalog();
    add_empty_project(&source);
    delete(&source, 3);
    let id = count(&source, "SELECT id FROM psf_guard_removed_project");
    assert!(allow_back(&source, id, NOW + 10).unwrap());

    let local = catalog();
    add_empty_project(&local);
    let digest = plan(&local, 3, &BTreeSet::new()).unwrap().unwrap().digest;
    remove(&local, 3, &digest, &BTreeSet::new(), NOW + 20).unwrap();
    sync_pull(&source, &local, &pull_options(false)).unwrap();
    assert_eq!(deletion_row(&local).1, None);
}

/// A project a collaboration's plan holds, or whose removed reject was
/// reported to a collaboration, stays when its source deletes it.
#[test]
fn a_collaboration_keeps_a_project_its_source_deleted() {
    let source = catalog();
    add_empty_project(&source);
    delete(&source, 3);
    let kept = "keep project \"Empty\": deleted in the source catalog, but it belongs to a collaboration here".to_string();

    let local = catalog();
    add_empty_project(&local);
    let mut options = pull_options(false);
    options
        .deletions
        .protected_projects
        .insert("pg-empty".into());
    let summary = sync_pull(&source, &local, &options).unwrap();
    assert!(summary.changes.contains(&kept), "{:?}", summary.changes);
    assert_eq!(
        count(&local, "SELECT COUNT(*) FROM project WHERE Id = 3"),
        1
    );

    // A reported reject whose files are gone, so Restore cannot bring it back.
    local
        .execute(
            "INSERT INTO psf_guard_removed_image
               (acquired_image_guid, acquired_image_id, project_id, target_id, batch_id,
                removed_at, trash_until, files_deleted_at, rows_json, files_json)
             VALUES ('reported', 95, 3, 30, 'b', 1, 2, 3, '{}', '[]')",
            [],
        )
        .unwrap();
    let mut options = pull_options(false);
    options.deletions.protected_frames.insert("reported".into());
    let summary = sync_pull(&source, &local, &options).unwrap();
    assert!(summary.changes.contains(&kept), "{:?}", summary.changes);
    assert_eq!(
        count(&local, "SELECT COUNT(*) FROM project WHERE Id = 3"),
        1
    );
}

/// An apply that reads its source again deletes only what its preview
/// listed.
#[test]
fn an_apply_deletes_only_what_its_preview_listed() {
    let source = catalog();
    add_empty_project(&source);
    delete(&source, 3);
    let local = catalog();
    add_empty_project(&local);

    let preview = sync_pull(&source, &local, &pull_options(true)).unwrap();
    assert_eq!(preview.deleted_project_guids, ["pg-empty"]);

    let mut options = pull_options(false);
    options.deletions.previewed = Some(HashSet::new());
    let summary = sync_pull(&source, &local, &options).unwrap();
    assert!(summary.changes.contains(
        &"keep project \"Empty\": deleted in the source catalog, but it was not in the preview; preview again".to_string()
    ), "{:?}", summary.changes);
    assert_eq!(
        count(&local, "SELECT COUNT(*) FROM project WHERE Id = 3"),
        1
    );

    options.deletions.previewed = Some(preview.deleted_project_guids.into_iter().collect());
    let summary = sync_pull(&source, &local, &options).unwrap();
    assert_eq!(summary.deleted_project_guids, ["pg-empty"]);
    assert_eq!(
        count(&local, "SELECT COUNT(*) FROM project WHERE Id = 3"),
        0
    );
}

/// An arrival sync reads a left-out project's frames again only when the
/// source's frames for it changed.
#[test]
fn left_out_frames_are_read_again_only_when_they_change() {
    let source = catalog();
    let local = catalog();
    delete(&local, 1);
    let frames = || {
        count(
            &local,
            "SELECT COUNT(*) FROM psf_guard_removed_project_frame",
        )
    };
    let signature = || {
        local
            .query_row(
                "SELECT left_out_source_frames FROM psf_guard_removed_project",
                [],
                |row| row.get::<_, Option<String>>(0),
            )
            .unwrap()
    };
    sync_pull(&source, &local, &pull_options(false)).unwrap();
    let first = signature().unwrap();
    assert!(first.starts_with("3:"), "{first}");
    let before = frames();
    // Not read again: a frame tombstone deleted by hand stays deleted.
    local
        .execute(
            "DELETE FROM psf_guard_removed_project_frame WHERE file_name = 'm31_l_003.fits'",
            [],
        )
        .unwrap();
    sync_pull(&source, &local, &pull_options(false)).unwrap();
    assert_eq!(frames(), before - 1);
    // A new frame at the source: read again, and the missing name returns.
    source
        .execute(
            "INSERT INTO acquiredimage (Id, projectId, targetId, acquireddate, filtername, gradingStatus, metadata, guid, profileId)
             VALUES (8, 1, 10, 1759303000, 'L', 0, '{\"FileName\":\"M31_L_008.fits\"}', 'g8', 'p')",
            [],
        )
        .unwrap();
    sync_pull(&source, &local, &pull_options(false)).unwrap();
    assert!(signature().unwrap().starts_with("4:"));
    assert_eq!(frames(), before + 1);

    // Another source with as many frames, but other ones, is read too.
    let other = catalog();
    other
        .execute_batch(
            "DELETE FROM imagedata WHERE acquiredimageid IN (SELECT Id FROM acquiredimage WHERE projectId = 1);
             DELETE FROM acquiredimage WHERE projectId = 1;
             INSERT INTO acquiredimage (Id, projectId, targetId, acquireddate, filtername, gradingStatus, metadata, guid, profileId) VALUES
               (1, 1, 10, 1759304000, 'L', 0, '{\"FileName\":\"M31_R_001.fits\"}', 'r1', 'p'),
               (2, 1, 10, 1759304300, 'L', 0, '{\"FileName\":\"M31_R_002.fits\"}', 'r2', 'p'),
               (3, 1, 10, 1759304600, 'L', 0, '{\"FileName\":\"M31_R_003.fits\"}', 'r3', 'p'),
               (8, 1, 10, 1759304900, 'L', 0, '{\"FileName\":\"M31_R_008.fits\"}', 'r8', 'p');",
        )
        .unwrap();
    sync_pull(&other, &local, &pull_options(false)).unwrap();
    assert!(removed_file_names(&local)
        .unwrap()
        .contains(&"m31_r_008.fits".to_string()));
}

/// Three catalogs: every deletion a sync records carries the date the
/// person deleted the project, so a third catalog compares Allow again
/// with that date and not with when it synced. A deletes P at t1; C pulls
/// from A; A allows P back at t3 and a rig brings P back to A. B pulls
/// from C, then from A, and A pulls from B: P stays in A, and B's deletion
/// ends allowed back.
#[test]
fn allow_again_holds_across_three_catalogs() {
    let t1 = NOW;
    let t3 = NOW + 300;
    let empty_in =
        |conn: &Connection| count(conn, "SELECT COUNT(*) FROM project WHERE guid = 'pg-empty'");
    let rig = catalog();
    add_empty_project(&rig);
    let [a, b, c] = [catalog(), catalog(), catalog()];
    for catalog in [&a, &b, &c] {
        add_empty_project(catalog);
    }

    let digest = plan(&a, 3, &BTreeSet::new()).unwrap().unwrap().digest;
    remove(&a, 3, &digest, &BTreeSet::new(), t1).unwrap();
    sync_pull(&a, &c, &pull_options(false)).unwrap();
    assert_eq!(empty_in(&c), 0);
    let dated = |conn: &Connection| -> (i64, Option<i64>) {
        conn.query_row(
            "SELECT removed_at, allowed_back_at FROM psf_guard_removed_project
             WHERE project_guid = 'pg-empty' ORDER BY id DESC LIMIT 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap()
    };
    assert_eq!(dated(&c), (t1, None), "dated when A deleted it");

    let id = count(&a, "SELECT id FROM psf_guard_removed_project");
    assert!(allow_back(&a, id, t3).unwrap());
    sync_pull(&rig, &a, &pull_options(false)).unwrap();
    assert_eq!(empty_in(&a), 1);

    sync_pull(&c, &b, &pull_options(false)).unwrap();
    assert_eq!(empty_in(&b), 0);
    assert_eq!(dated(&b), (t1, None));
    sync_pull(&a, &b, &pull_options(false)).unwrap();
    assert_eq!(dated(&b), (t1, Some(t3)), "B follows A's Allow again");
    assert_eq!(empty_in(&b), 1);
    let summary = sync_pull(&b, &a, &pull_options(false)).unwrap();
    assert_eq!(empty_in(&a), 1, "{:?}", summary.changes);
}

/// Another catalog's Allow again does not undo a deletion a person made in
/// this catalog, however it is dated; only Allow again here does.
#[test]
fn a_deletion_made_here_waits_for_allow_again_here() {
    let source = catalog();
    add_empty_project(&source);
    delete(&source, 3);
    let id = count(&source, "SELECT id FROM psf_guard_removed_project");
    assert!(allow_back(&source, id, NOW + 500).unwrap());

    let local = catalog();
    add_empty_project(&local);
    let digest = plan(&local, 3, &BTreeSet::new()).unwrap().unwrap().digest;
    remove(&local, 3, &digest, &BTreeSet::new(), NOW + 10).unwrap();
    sync_pull(&source, &local, &pull_options(false)).unwrap();
    assert_eq!(
        deletion_row(&local),
        (Some("pg-empty".into()), None, "local".into())
    );
    assert_eq!(
        count(
            &local,
            "SELECT COUNT(*) FROM project WHERE guid = 'pg-empty'"
        ),
        0
    );
}

/// A target a person moved here from a project, before deleting that
/// project without frames, keeps arriving: the Target Scheduler source
/// still lists it under the old project. Its new frames arrive under its
/// project here, and none of them is recorded as removed.
#[test]
fn a_target_moved_out_of_a_deleted_project_keeps_arriving() {
    let source = catalog();
    source
        .execute(
            "INSERT INTO acquiredimage (Id, projectId, targetId, acquireddate, filtername, gradingStatus, metadata, guid, exposureId, profileId)
             VALUES (9, 1, 10, 1759309000, 'L', 0, '{\"FileName\":\"M31_L_009.fits\"}', 'g9', 100, 'p')",
            [],
        )
        .unwrap();
    let local = catalog();
    // Move M31's target, with its frames, to M42; then delete M31, empty.
    local
        .execute_batch(
            "UPDATE target SET projectid = 2 WHERE Id = 10;
             UPDATE acquiredimage SET projectId = 2 WHERE targetId = 10;",
        )
        .unwrap();
    let removal = plan(&local, 1, &BTreeSet::new()).unwrap().unwrap();
    assert_eq!((removal.targets, removal.frames.total), (0, 0));
    delete(&local, 1);

    let summary = sync_pull(&source, &local, &pull_options(false)).unwrap();
    assert_eq!(
        count(&local, "SELECT COUNT(*) FROM project WHERE guid = 'pg-m31'"),
        0,
        "the deleted project stays deleted"
    );
    assert_eq!(
        count(
            &local,
            "SELECT COUNT(*) FROM acquiredimage WHERE guid = 'g9' AND projectId = 2 AND targetId = 10"
        ),
        1,
        "{:?}",
        summary.changes
    );
    assert_eq!(
        count(
            &local,
            "SELECT COUNT(*) FROM target WHERE Id = 10 AND projectid = 2"
        ),
        1
    );
    assert_eq!(
        summary.target.skipped + summary.acquiredimage.skipped,
        0,
        "{:?}",
        summary.changes
    );
    assert!(deleted_project_frames_named(&local, "m31_l_009.fits")
        .unwrap()
        .is_empty());
    assert!(!removed_file_names(&local)
        .unwrap()
        .contains(&"m31_l_009.fits".to_string()));
}

/// When the server cannot check Planning, a sync holds its deletions for a
/// later run and still brings everything else.
#[test]
fn held_deletions_wait_for_a_later_sync() {
    let source = catalog();
    add_empty_project(&source);
    delete(&source, 3);
    let local = catalog();
    add_empty_project(&local);
    let mut options = pull_options(false);
    options.deletions.held = Some("Planning could not be read".into());
    let summary = sync_pull(&source, &local, &options).unwrap();
    assert!(summary.changes.contains(
        &"keep project \"Empty\": deleted in the source catalog, but Planning could not be read"
            .to_string()
    ), "{:?}", summary.changes);
    assert_eq!(
        count(&local, "SELECT COUNT(*) FROM project WHERE Id = 3"),
        1
    );
    let summary = sync_pull(&source, &local, &pull_options(false)).unwrap();
    assert_eq!(summary.project.deleted, 1);
}

/// Give Empty's target the rows that refer to it: order and cadence items,
/// flat history, and Planning's records keyed by its GUID.
fn add_target_side_rows(conn: &Connection) {
    conn.execute_batch(
        r#"INSERT INTO overrideexposureorderitem (Id, targetid, "order", action, referenceIdx) VALUES (3, 30, 0, 0, 0);
           INSERT INTO filtercadenceitem (Id, targetid, "order", next, action, referenceIdx) VALUES (3, 30, 0, 1, 0, 0);
           INSERT INTO flathistory (Id, targetId, profileId) VALUES (3, 30, 'p');
           INSERT INTO psf_guard_director_target VALUES ('tg-empty', 'pg-empty');
           INSERT INTO psf_guard_director_plan VALUES ('eg-empty', 'tg-empty');
           INSERT INTO psf_guard_scheduler_flat_history VALUES ('r3', 'tg-empty');"#,
    )
    .unwrap();
}

fn assert_target_kept(conn: &Connection, changes: &[String]) {
    assert_eq!(
        count(
            conn,
            "SELECT COUNT(*) FROM target WHERE Id = 30 AND guid = 'tg-empty' AND projectid = 2"
        ),
        1,
        "{changes:?}"
    );
    assert_eq!(
        count(conn, "SELECT COUNT(*) FROM project WHERE guid = 'pg-empty'"),
        0,
        "{changes:?}"
    );
    for (table, column) in [
        ("overrideexposureorderitem", "targetid = 30"),
        ("filtercadenceitem", "targetid = 30"),
        ("flathistory", "targetId = 30"),
        ("exposureplan", "targetid = 30"),
        ("psf_guard_director_target", "target_guid = 'tg-empty'"),
        ("psf_guard_director_plan", "target_guid = 'tg-empty'"),
        (
            "psf_guard_scheduler_flat_history",
            "target_guid = 'tg-empty'",
        ),
    ] {
        assert_eq!(
            count(
                conn,
                &format!("SELECT COUNT(*) FROM {table} WHERE {column}")
            ),
            1,
            "{table} lost the moved target's row"
        );
    }
}

/// A source catalog that moved a target out of a project and then deleted
/// the project: a sync re-parents the target here, with its Id and every
/// row that refers to it, before it deletes the project.
#[test]
fn a_target_moved_out_of_a_project_the_source_deleted_keeps_its_id_and_rows() {
    use crate::commands::sync::{sync_planning, PlanningOptions, SyncDestination};

    let source = catalog();
    add_empty_project(&source);
    source
        .execute("UPDATE target SET projectid = 2 WHERE Id = 30", [])
        .unwrap();
    delete(&source, 3);

    let local = catalog();
    add_empty_project(&local);
    add_target_side_rows(&local);
    let summary = sync_pull(&source, &local, &pull_options(false)).unwrap();
    assert_eq!(summary.project.deleted, 1, "{:?}", summary.changes);
    assert_target_kept(&local, &summary.changes);

    let rig = catalog();
    add_empty_project(&rig);
    add_target_side_rows(&rig);
    let pushed = sync_planning(
        &source,
        &rig,
        &PlanningOptions {
            dry_run: false,
            project_filter: None,
            destination: SyncDestination::Scheduler,
            deletions: Default::default(),
        },
    )
    .unwrap();
    assert_eq!(pushed.project.deleted, 1, "{:?}", pushed.changes);
    assert_target_kept(&rig, &pushed.changes);
}

/// When a target the source has under another project cannot move here,
/// the project it is under stays.
#[test]
fn a_project_stays_while_a_target_cannot_move_out_of_it() {
    let source = catalog();
    add_empty_project(&source);
    source
        .execute("UPDATE target SET projectid = 2 WHERE Id = 30", [])
        .unwrap();
    delete(&source, 3);
    let local = catalog();
    add_empty_project(&local);
    // A filter that leaves out the target's new project.
    let mut options = pull_options(false);
    options.project_filter = Some("Empty".into());
    let summary = sync_pull(&source, &local, &options).unwrap();
    assert!(summary.changes.contains(
        &"keep project \"Empty\": deleted in the source catalog, but its target \"Empty\" could not move to its project in the source".to_string()
    ), "{:?}", summary.changes);
    assert_eq!(
        count(&local, "SELECT COUNT(*) FROM project WHERE Id = 3"),
        1
    );
}

/// After a planning sync the rig has the target under its new project, but
/// its earlier frames keep the old project's Id. They arrive with their
/// target, under its project here, and none is recorded as removed.
#[test]
fn frames_follow_their_target_out_of_a_deleted_project() {
    let rig = catalog();
    rig.execute_batch(
        "UPDATE target SET projectid = 2 WHERE Id = 10;
         INSERT INTO acquiredimage (Id, projectId, targetId, acquireddate, filtername, gradingStatus, metadata, guid, exposureId, profileId)
           VALUES (9, 1, 10, 1759309000, 'L', 0, '{\"FileName\":\"M31_L_009.fits\"}', 'g9', 100, 'p');",
    )
    .unwrap();
    let local = catalog();
    local
        .execute_batch(
            "UPDATE target SET projectid = 2 WHERE Id = 10;
             UPDATE acquiredimage SET projectId = 2 WHERE targetId = 10;",
        )
        .unwrap();
    delete(&local, 1);

    let summary = sync_pull(&rig, &local, &pull_options(false)).unwrap();
    assert_eq!(
        count(
            &local,
            "SELECT COUNT(*) FROM acquiredimage WHERE guid IN ('g1','g2','g3','g9') AND projectId = 2 AND targetId = 10"
        ),
        4,
        "{:?}",
        summary.changes
    );
    assert_eq!(summary.acquiredimage.skipped, 0, "{:?}", summary.changes);
    assert_eq!(
        count(
            &local,
            "SELECT COUNT(*) FROM psf_guard_removed_project_frame"
        ),
        0
    );
    for name in ["m31_l_001.fits", "m31_l_009.fits"] {
        assert!(deleted_project_frames_named(&local, name)
            .unwrap()
            .is_empty());
    }
}
