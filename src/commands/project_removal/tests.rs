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
    let arrivals = sync_arrivals(&source, &tx, &HashSet::new()).unwrap();
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

/// A project without frames leaves nothing to keep out: no sync or import
/// skips it, and Deleted projects does not list it. Restore still sees its
/// deletion.
#[test]
fn deleting_a_project_without_frames_lists_nothing() {
    let conn = catalog();
    conn.execute(
        "INSERT INTO project (Id, profileId, name, guid) VALUES (3, 'p', 'Empty', 'pg-empty')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO target (Id, name, active, epochcode, projectid, guid) VALUES (30, 'Empty', 1, 0, 3, 'tg-empty')",
        [],
    )
    .unwrap();
    assert!(matches!(delete(&conn, 3), Outcome::Removed { .. }));
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM project WHERE Id = 3"), 0);
    assert_eq!(count(&conn, "SELECT COUNT(*) FROM target WHERE Id = 30"), 0);
    assert!(deletions(&conn).unwrap().is_empty());
    assert!(removed_project_guids(&conn).unwrap().is_empty());
    conn.execute(
        "INSERT INTO project (Id, profileId, name, guid) VALUES (3, 'p', 'Reused', 'pg-new')",
        [],
    )
    .unwrap();
    assert!(project_removed_since(&conn, 3, NOW - 10).unwrap());
}
