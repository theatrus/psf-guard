use super::*;

fn catalog() -> (tempfile::TempDir, Connection) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.sqlite");
    crate::ts_schema::create_fresh_db(&path).unwrap();
    let conn = Connection::open(path).unwrap();
    conn.execute_batch("INSERT INTO project(Id,profileId,name,guid) VALUES(1,'p','M42','project-guid');
        INSERT INTO target(Id,name,active,epochcode,projectid,guid) VALUES(1,'M42',1,0,1,'target-guid');
        INSERT INTO exposuretemplate(Id,profileId,name,filtername,guid) VALUES(1,'p','Ha','Ha','template-guid');
        INSERT INTO exposureplan(Id,profileId,exposure,desired,acquired,accepted,targetid,exposureTemplateId,guid)
            VALUES(1,'p',300,10,1,0,1,1,'plan-guid');
        INSERT INTO acquiredimage(Id,projectId,targetId,acquireddate,filtername,gradingStatus,metadata,exposureId,guid)
            VALUES(7,1,1,1000,'Ha',0,'{}',1,'image-guid');").unwrap();
    (dir, conn)
}

fn grade(conn: &Connection, status: i32, reason: Option<&str>) {
    conn.execute(
        "UPDATE acquiredimage SET gradingStatus=?1,rejectreason=?2 WHERE guid='image-guid'",
        params![status, reason],
    )
    .unwrap();
}

#[test]
fn imports_new_captures_and_writes_only_local_grade_edits() {
    let (_local_dir, mut local) = catalog();
    let (_source_dir, mut source) = catalog();
    let (_snapshot_dir, snapshot) = catalog();
    initialize(&local).unwrap();
    grade(&local, 1, None);
    snapshot.execute_batch("INSERT INTO acquiredimage(Id,projectId,targetId,acquireddate,filtername,gradingStatus,metadata,guid)
        VALUES(8,1,1,2000,'Ha',0,'{}','new-image-guid')").unwrap();
    let report = synchronize(&mut local, &snapshot, Some(&mut source)).unwrap();
    assert_eq!(report.inserted, 1);
    assert_eq!(report.grades_written, 1);
    assert_eq!(grades(&source).unwrap()["image-guid"].status, 1);
    assert_eq!(
        source
            .query_row("SELECT accepted FROM exposureplan WHERE Id=1", [], |r| r
                .get::<_, i32>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        local
            .query_row(
                "SELECT Id FROM acquiredimage WHERE guid='image-guid'",
                [],
                |r| r.get::<_, i32>(0)
            )
            .unwrap(),
        7
    );
    assert_eq!(
        source
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE name LIKE 'psf_guard_ts_sync_%'",
                [],
                |r| r.get::<_, i32>(0)
            )
            .unwrap(),
        0
    );
}

#[test]
fn conflicting_external_grades_are_not_overwritten() {
    let (_local_dir, mut local) = catalog();
    let (_source_dir, mut source) = catalog();
    let (_snapshot_dir, snapshot) = catalog();
    initialize(&local).unwrap();
    grade(&local, 1, None);
    grade(&snapshot, 2, Some("cloud"));
    grade(&source, 2, Some("cloud"));
    let report = synchronize(&mut local, &snapshot, Some(&mut source)).unwrap();
    assert_eq!(report.conflicts, 1);
    assert_eq!(report.grades_written, 0);
    assert_eq!(grades(&local).unwrap()["image-guid"].status, 1);
    assert_eq!(grades(&source).unwrap()["image-guid"].status, 2);
}

#[test]
fn a_ts_edit_after_the_snapshot_is_checked_in_the_write_transaction() {
    let (_local_dir, mut local) = catalog();
    let (_source_dir, mut source) = catalog();
    let (_snapshot_dir, snapshot) = catalog();
    initialize(&local).unwrap();
    grade(&local, 1, None);
    grade(&source, 2, Some("changed after snapshot"));
    let report = synchronize(&mut local, &snapshot, Some(&mut source)).unwrap();
    assert_eq!(report.conflicts, 1);
    assert_eq!(grades(&source).unwrap()["image-guid"].status, 2);
}

#[test]
fn a_committed_external_write_can_be_recovered_before_a_later_local_edit() {
    let (_local_dir, mut local) = catalog();
    let (_source_dir, mut source) = catalog();
    let (_snapshot_dir, snapshot) = catalog();
    initialize(&local).unwrap();
    // The previous run persisted a Pending -> Accepted export. TS committed,
    // but the host stopped before acknowledging it. The user then rejected it.
    local
        .execute(
            "INSERT INTO psf_guard_ts_sync_pending VALUES('image-guid',1,NULL)",
            [],
        )
        .unwrap();
    grade(&source, 1, None);
    grade(&snapshot, 1, None);
    grade(&local, 2, Some("reviewed again"));
    let report = synchronize(&mut local, &snapshot, Some(&mut source)).unwrap();
    assert_eq!(report.conflicts, 0);
    assert_eq!(report.grades_written, 1);
    assert_eq!(grades(&source).unwrap()["image-guid"].status, 2);
    assert_eq!(
        local
            .query_row("SELECT count(*) FROM psf_guard_ts_sync_pending", [], |r| {
                r.get::<_, i32>(0)
            })
            .unwrap(),
        0
    );
}

#[test]
fn resolving_a_conflict_clears_an_obsolete_pending_write() {
    let (_local_dir, mut local) = catalog();
    let (_snapshot_dir, snapshot) = catalog();
    initialize(&local).unwrap();
    local
        .execute(
            "INSERT INTO psf_guard_ts_sync_pending VALUES('image-guid',1,NULL)",
            [],
        )
        .unwrap();
    grade(&local, 2, Some("manual resolution"));
    grade(&snapshot, 2, Some("manual resolution"));
    synchronize(&mut local, &snapshot, None).unwrap();
    assert_eq!(
        local
            .query_row("SELECT count(*) FROM psf_guard_ts_sync_pending", [], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap(),
        0
    );
}

#[test]
fn disabling_writeback_preserves_edits_for_a_later_run() {
    let (_local_dir, mut local) = catalog();
    let (_snapshot_dir, snapshot) = catalog();
    initialize(&local).unwrap();
    grade(&local, 1, None);
    let report = synchronize(&mut local, &snapshot, None).unwrap();
    assert_eq!(report.grades_written, 0);
    let (_source_dir, mut source) = catalog();
    let report = synchronize(&mut local, &snapshot, Some(&mut source)).unwrap();
    assert_eq!(report.grades_written, 1);
}

#[test]
fn an_external_grade_is_not_mistaken_for_a_local_edit() {
    let (_local_dir, mut local) = catalog();
    let (_source_dir, mut source) = catalog();
    let (_snapshot_dir, snapshot) = catalog();
    initialize(&local).unwrap();
    grade(&snapshot, 2, Some("NINA"));
    grade(&source, 2, Some("NINA"));
    let first = synchronize(&mut local, &snapshot, Some(&mut source)).unwrap();
    assert_eq!(first.grades_written, 0);
    let second = synchronize(&mut local, &snapshot, Some(&mut source)).unwrap();
    assert_eq!(second.grades_written, 0);
    assert_eq!(grades(&local).unwrap()["image-guid"].status, 2);
}

#[test]
fn automatic_capture_import_preserves_local_planning_and_updates_acquisition_progress() {
    let (_local_dir, mut local) = catalog();
    let (_snapshot_dir, snapshot) = catalog();
    initialize(&local).unwrap();
    local
        .execute_batch(
            "UPDATE project SET name='My planning edits'; UPDATE exposureplan SET desired=50",
        )
        .unwrap();
    snapshot
        .execute_batch("UPDATE exposureplan SET acquired=2")
        .unwrap();
    synchronize(&mut local, &snapshot, None).unwrap();
    assert_eq!(
        local
            .query_row("SELECT name FROM project", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "My planning edits"
    );
    assert_eq!(
        local
            .query_row("SELECT desired,acquired FROM exposureplan", [], |r| Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?
            )))
            .unwrap(),
        (50, 2)
    );
}

#[test]
fn resetting_a_reviewed_grade_to_pending_is_not_filled_back_from_ts() {
    let (_local_dir, mut local) = catalog();
    let (_source_dir, mut source) = catalog();
    let (_snapshot_dir, snapshot) = catalog();
    grade(&local, 1, None);
    grade(&source, 1, None);
    grade(&snapshot, 1, None);
    initialize(&local).unwrap();
    grade(&local, 0, None);
    let report = synchronize(&mut local, &snapshot, Some(&mut source)).unwrap();
    assert_eq!(report.grades_written, 1);
    assert_eq!(grades(&local).unwrap()["image-guid"].status, 0);
    assert_eq!(grades(&source).unwrap()["image-guid"].status, 0);
}

#[test]
fn a_disabled_pending_reset_and_idle_sync_do_not_rewrite_the_catalog() {
    let (_local_dir, mut local) = catalog();
    let (_snapshot_dir, snapshot) = catalog();
    grade(&local, 1, None);
    grade(&snapshot, 1, None);
    initialize(&local).unwrap();
    grade(&local, 0, None);
    synchronize(&mut local, &snapshot, None).unwrap();
    let changes = local.total_changes();
    let report = synchronize(&mut local, &snapshot, None).unwrap();
    assert_eq!(report.updated, 0);
    assert_eq!(local.total_changes(), changes);
    assert_eq!(grades(&local).unwrap()["image-guid"].status, 0);
}

#[test]
fn capture_refresh_keeps_pixel_measurements_but_accepts_ts_metrics_and_file_changes() {
    let (_local_dir, mut local) = catalog();
    let (_snapshot_dir, snapshot) = catalog();
    initialize(&local).unwrap();
    let source = serde_json::json!({"FileName":"frame.fits","HFR":2.5});
    snapshot
        .execute("UPDATE acquiredimage SET metadata=?1", [source.to_string()])
        .unwrap();
    let measured = serde_json::json!({"FileName":"frame.fits","HFR":2.5,"DetectedStars":100,
        "PsfGuardQualityFields":["DetectedStars"],"PsfGuardQualitySource":"file:current"});
    local
        .execute(
            "UPDATE acquiredimage SET metadata=?1",
            [measured.to_string()],
        )
        .unwrap();
    synchronize(&mut local, &snapshot, None).unwrap();
    let changes = local.total_changes();
    synchronize(&mut local, &snapshot, None).unwrap();
    assert_eq!(
        local.total_changes(),
        changes,
        "idle refresh must preserve the measured metadata"
    );
    let read = |conn: &Connection| -> serde_json::Value {
        let json: String = conn
            .query_row("SELECT metadata FROM acquiredimage", [], |r| r.get(0))
            .unwrap();
        serde_json::from_str(&json).unwrap()
    };
    assert_eq!(read(&local), measured);
    snapshot
        .execute(
            "UPDATE acquiredimage SET metadata=?1",
            [
                serde_json::json!({"FileName":"frame.fits","HFR":3.5,"DetectedStars":200})
                    .to_string(),
            ],
        )
        .unwrap();
    synchronize(&mut local, &snapshot, None).unwrap();
    assert_eq!(read(&local)["DetectedStars"], 200);
    assert_eq!(read(&local)["HFR"], 3.5);
    assert_eq!(read(&local)["PsfGuardQualityFields"], serde_json::json!([]));
    snapshot
        .execute(
            "UPDATE acquiredimage SET metadata=?1",
            [serde_json::json!({"FileName":"replacement.fits"}).to_string()],
        )
        .unwrap();
    let report = synchronize(&mut local, &snapshot, None).unwrap();
    assert_eq!(report.arrivals.len(), 1);
    assert!(read(&local).get("PsfGuardQualitySource").is_none());
}

#[test]
fn an_adopted_capture_retains_its_local_review_for_writeback() {
    let (_local_dir, mut local) = catalog();
    let (_snapshot_dir, snapshot) = catalog();
    let (_source_dir, mut source) = catalog();
    initialize(&local).unwrap();
    local.execute_batch("UPDATE acquiredimage SET guid='psf-minted',gradingStatus=1,metadata='{\"FileName\":\"frame.fits\",\"DetectedStars\":100,\"PsfGuardQualityFields\":[\"DetectedStars\"],\"PsfGuardQualitySource\":\"file:current\"}'").unwrap();
    snapshot
        .execute_batch("UPDATE acquiredimage SET metadata='{\"FileName\":\"frame.fits\"}'")
        .unwrap();
    synchronize(&mut local, &snapshot, Some(&mut source)).unwrap();
    assert_eq!(grades(&local).unwrap()["image-guid"].status, 1);
    let metadata: String = local
        .query_row("SELECT metadata FROM acquiredimage", [], |r| r.get(0))
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&metadata).unwrap()["DetectedStars"],
        100
    );
    let report = synchronize(&mut local, &snapshot, Some(&mut source)).unwrap();
    assert_eq!(report.grades_written, 1);
    assert_eq!(grades(&source).unwrap()["image-guid"].status, 1);
}

#[test]
fn a_capture_received_by_manual_sync_tracks_reviews_before_the_first_automatic_run() {
    for ts_changed in [false, true] {
        let (_local_dir, mut local) = catalog();
        let (_snapshot_dir, snapshot) = catalog();
        let (_source_dir, mut source) = catalog();
        initialize(&local).unwrap();
        for conn in [&snapshot, &source] {
            conn.execute_batch("INSERT INTO acquiredimage(Id,projectId,targetId,acquireddate,filtername,gradingStatus,metadata,guid)
                VALUES(8,1,1,2000,'Ha',0,'{}','manual-arrival')").unwrap();
        }
        let merged = crate::commands::sync::sync_pull(
            &snapshot,
            &local,
            &crate::commands::sync::PullOptions {
                dry_run: false,
                with_image_data: false,
                project_filter: None,
                deletions: Default::default(),
            },
        )
        .unwrap();
        let id: i64 = local
            .query_row(
                "SELECT Id FROM acquiredimage WHERE guid='manual-arrival'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert!(merged
            .changed_acquiredimages
            .iter()
            .any(|image| image.id == id));
        local
            .execute(
                "UPDATE acquiredimage SET gradingStatus=1 WHERE guid='manual-arrival'",
                [],
            )
            .unwrap();
        if ts_changed {
            for conn in [&snapshot, &source] {
                conn.execute("UPDATE acquiredimage SET gradingStatus=2,rejectreason='TS review' WHERE guid='manual-arrival'", []).unwrap();
            }
        }
        let report = synchronize(&mut local, &snapshot, Some(&mut source)).unwrap();
        assert_eq!(report.grades_written, usize::from(!ts_changed));
        assert_eq!(report.conflicts, usize::from(ts_changed));
        assert_eq!(
            grades(&source).unwrap()["manual-arrival"].status,
            if ts_changed { 2 } else { 1 }
        );
        assert_eq!(grades(&local).unwrap()["manual-arrival"].status, 1);
    }
}
