//! One exposure catalogued twice: the import, and the repair. Written in
//! the style of `a_stale_twin_of_a_moved_file_does_not_double_a_master`.

use super::super::*;
use super::{duplicate_snapshot, plan_repair, DuplicateReport, FileAccess, RepairError};
use crate::commands::import::headers::FrameMeta;

fn dark_at(path: &Path, at: i64) -> FrameMeta {
    FrameMeta {
        path: path.to_path_buf(),
        readable: true,
        image_type: Some("DARK".into()),
        telescope: Some("Scope".into()),
        camera: Some("Camera".into()),
        width: Some(3000),
        height: Some(2000),
        channels: Some(1),
        binning_x: Some(1),
        binning_y: Some(1),
        gain: Some(100),
        offset: Some(20),
        exposure_s: Some(60.0),
        camera_temp: Some(-10.0),
        timestamp: Some(at),
        ..Default::default()
    }
}

fn import_now(conn: &mut Connection, frames: &[FrameMeta]) -> CalibrationImportOutcome {
    let tx = conn.transaction().unwrap();
    let outcome = import_calibration_frames(&tx, frames, Some("profile")).unwrap();
    tx.commit().unwrap();
    outcome
}

/// Every row, oldest first, as (uuid, path).
fn recorded_rows(conn: &Connection) -> Vec<(String, String)> {
    let mut statement = conn
        .prepare("SELECT frame_uuid, source_path FROM psf_guard_calibration_frame ORDER BY id")
        .unwrap();
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
}

fn uuid_at(conn: &Connection, path: &Path) -> String {
    conn.query_row(
        "SELECT frame_uuid FROM psf_guard_calibration_frame WHERE source_path = ?1",
        [canonical_text(path)],
        |row| row.get(0),
    )
    .unwrap()
}

/// A second row for `uuid`'s frame at `path`, as an older build recorded
/// every copy it met.
fn record_twin_row(conn: &Connection, uuid: &str, twin_uuid: &str, path: &Path) {
    let columns = table_column_names(conn, "psf_guard_calibration_frame")
        .unwrap()
        .into_iter()
        .filter(|name| !["id", "frame_uuid", "source_path"].contains(&name.as_str()))
        .collect::<Vec<_>>()
        .join(", ");
    conn.execute(
        &format!(
            "INSERT INTO psf_guard_calibration_frame (frame_uuid, source_path, {columns})
             SELECT ?1, ?2, {columns} FROM psf_guard_calibration_frame WHERE frame_uuid = ?3"
        ),
        params![twin_uuid, canonical_text(path), uuid],
    )
    .unwrap();
    if path.is_file() {
        let (fingerprint, size, mtime) = file_fingerprint(path);
        conn.execute(
            "UPDATE psf_guard_calibration_frame
             SET source_fingerprint = ?1, file_size = ?2, file_mtime_ns = ?3
             WHERE frame_uuid = ?4",
            params![fingerprint, size, mtime, twin_uuid],
        )
        .unwrap();
    }
}

fn duplicate_plan(conn: &Connection) -> DuplicateReport {
    plan_repair(&duplicate_snapshot(conn).unwrap(), &FileAccess::disk())
}

fn twin_paths(conn: &Connection) -> Vec<String> {
    let mut statement = conn
        .prepare("SELECT source_path FROM psf_guard_calibration_twin ORDER BY source_path")
        .unwrap();
    statement
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
}

const TWIN_NAME: &str = "2026-10-09_20-01-02_HA_-8.00_60.00s_0001.fits";
const EXPOSURE: &[u8] = b"one dark exposure, its header and pixels";

/// The same exposure in a drop folder and in a library folder.
fn two_copies(temp: &tempfile::TempDir) -> (PathBuf, PathBuf) {
    let drop = temp.path().join("drop");
    let library = temp.path().join("library");
    std::fs::create_dir_all(&drop).unwrap();
    std::fs::create_dir_all(&library).unwrap();
    std::fs::write(drop.join(TWIN_NAME), EXPOSURE).unwrap();
    std::fs::copy(drop.join(TWIN_NAME), library.join(TWIN_NAME)).unwrap();
    (drop.join(TWIN_NAME), library.join(TWIN_NAME))
}

#[test]
fn both_copies_present_at_import_make_one_row() {
    // The filer has copied the frame into the library and not yet deleted
    // the original: the scan meets both.
    let temp = tempfile::tempdir().unwrap();
    let (drop, library) = two_copies(&temp);
    let mut conn = Connection::open_in_memory().unwrap();
    let both = [dark_at(&drop, 1_000), dark_at(&library, 1_000)];
    let outcome = import_now(&mut conn, &both);
    assert_eq!((outcome.imported, outcome.twins), (1, 1), "{outcome:?}");
    assert_eq!(recorded_rows(&conn).len(), 1, "one exposure, one row");
    assert_eq!(twin_paths(&conn), vec![canonical_text(&library)]);

    // A later scan drops the copy before reading its header, and a run that
    // reads it anyway changes nothing.
    let known = known_calibration_fingerprints(&conn).unwrap();
    let (path, fingerprint) = source_fingerprint(&library);
    assert_eq!(known.get(&path), Some(&fingerprint));
    let again = import_now(&mut conn, &both);
    assert_eq!(
        (
            again.imported,
            again.updated,
            again.skipped_existing,
            again.twins
        ),
        (0, 0, 2, 1),
        "{again:?}"
    );
    assert_eq!(recorded_rows(&conn).len(), 1);
}

#[test]
fn the_row_moves_to_the_surviving_copy_when_the_original_goes() {
    let temp = tempfile::tempdir().unwrap();
    let (drop, library) = two_copies(&temp);
    let mut conn = Connection::open_in_memory().unwrap();
    import_now(
        &mut conn,
        &[dark_at(&drop, 1_000), dark_at(&library, 1_000)],
    );
    let uuid = uuid_at(&conn, &drop);
    set_frames_validity(
        &conn,
        std::slice::from_ref(&uuid),
        Some(ValidDirection::Forward),
    )
    .unwrap();

    std::fs::remove_file(&drop).unwrap();
    // The copy is no longer skipped unread: the row has to learn it.
    let known = known_calibration_fingerprints(&conn).unwrap();
    assert!(!known.contains_key(&source_fingerprint(&library).0));
    let outcome = import_now(&mut conn, &[dark_at(&library, 1_000)]);
    assert_eq!((outcome.imported, outcome.updated), (0, 1), "{outcome:?}");
    assert_eq!(
        recorded_rows(&conn),
        vec![(uuid.clone(), canonical_text(&library))],
        "the same row, at the surviving file"
    );
    assert!(twin_paths(&conn).is_empty(), "no longer a twin of itself");
    let validity: Option<String> = conn
        .query_row(
            "SELECT valid_direction FROM psf_guard_calibration_frame",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(validity.as_deref(), Some("forward"), "and kept its mark");
}

#[test]
fn a_different_frame_that_shares_a_name_stays_its_own_row() {
    let temp = tempfile::tempdir().unwrap();
    let folder = |name: &str| {
        let path = temp.path().join(name);
        std::fs::create_dir_all(&path).unwrap();
        path.join(TWIN_NAME)
    };
    let first = folder("first");
    let longer = folder("longer");
    let other_bytes = folder("other-bytes");
    let later = folder("later");
    std::fs::write(&first, b"exposure one").unwrap();
    // Same name and second, a different size.
    std::fs::write(&longer, b"exposure one, and more").unwrap();
    // Same name, second and size, different pixels.
    std::fs::write(&other_bytes, b"exposure two").unwrap();
    // The same bytes, shot at another time.
    std::fs::write(&later, b"exposure one").unwrap();
    let mut conn = Connection::open_in_memory().unwrap();
    let outcome = import_now(
        &mut conn,
        &[
            dark_at(&first, 1_000),
            dark_at(&longer, 1_000),
            dark_at(&other_bytes, 1_000),
            dark_at(&later, 2_000),
        ],
    );
    assert_eq!((outcome.imported, outcome.twins), (4, 0), "{outcome:?}");
    assert!(
        duplicate_plan(&conn).is_empty(),
        "and the repair finds no twins among them"
    );

    // Nor does a frame of another size take over a row whose file has gone.
    // (Once a file is gone its bytes cannot be compared, so a frame of the
    // same name, second, rig and size would: N.I.N.A. names a frame by its
    // time and number, so only a copy shares all four.)
    std::fs::remove_file(&first).unwrap();
    let elsewhere = folder("elsewhere");
    std::fs::write(&elsewhere, b"a different exposure entirely").unwrap();
    let outcome = import_now(&mut conn, &[dark_at(&elsewhere, 1_000)]);
    assert_eq!((outcome.imported, outcome.updated), (1, 0), "{outcome:?}");
    assert_eq!(recorded_rows(&conn).len(), 5);
}

#[test]
fn repair_merges_a_missing_row_into_its_live_twin() {
    // An older build recorded the drop-zone copy, then the library copy; the
    // filer has since deleted the drop-zone copy.
    let temp = tempfile::tempdir().unwrap();
    let (drop, library) = two_copies(&temp);
    let mut conn = Connection::open_in_memory().unwrap();
    import_now(&mut conn, &[dark_at(&drop, 1_000)]);
    let oldest = uuid_at(&conn, &drop);
    record_twin_row(&conn, &oldest, "library-row", &library);
    // A person's choice and a validity mark were made on the newer row.
    set_overrides(
        &conn,
        &["library-row".into()],
        Some(QualityOverride::Exclude),
    )
    .unwrap();
    set_frames_validity(
        &conn,
        &["library-row".into()],
        Some(ValidDirection::Forward),
    )
    .unwrap();
    std::fs::remove_file(&drop).unwrap();

    let report = duplicate_plan(&conn);
    assert_eq!(report.groups.len(), 1);
    assert_eq!(
        (
            report.rows_removed,
            report.missing_rows,
            report.physical_rows,
            report.repointed
        ),
        (1, 1, 0, 1)
    );
    let group = &report.groups[0];
    assert_eq!(group.keep, oldest, "the oldest row stays");
    assert_eq!(group.path, canonical_text(&library));
    assert!(group.repointed && !group.physical);

    let outcome = super::apply_repair(&mut conn, &report).unwrap();
    assert_eq!((outcome.rows_removed, outcome.repointed), (1, 1));
    assert_eq!(
        recorded_rows(&conn),
        vec![(oldest.clone(), canonical_text(&library))]
    );
    let (choice, validity): (Option<String>, Option<String>) = conn
        .query_row(
            "SELECT q.override_choice, f.valid_direction
             FROM psf_guard_calibration_frame f
             LEFT JOIN psf_guard_calibration_frame_quality q USING (frame_uuid)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        choice.as_deref(),
        Some("exclude"),
        "the choice carried over"
    );
    assert_eq!(validity.as_deref(), Some("forward"), "and the mark");
    assert!(library.is_file(), "no file is touched");

    // A second run finds nothing to do.
    let again = duplicate_plan(&conn);
    assert!(again.is_empty());
    let outcome = super::apply_repair(&mut conn, &again).unwrap();
    assert_eq!(outcome.rows_removed, 0);
    assert_eq!(recorded_rows(&conn).len(), 1);
}

#[test]
fn repair_keeps_one_row_for_two_copies_on_disk() {
    let temp = tempfile::tempdir().unwrap();
    let (drop, library) = two_copies(&temp);
    let mut conn = Connection::open_in_memory().unwrap();
    import_now(&mut conn, &[dark_at(&library, 1_000)]);
    let oldest = uuid_at(&conn, &library);
    record_twin_row(&conn, &oldest, "second-copy", &drop);

    let report = duplicate_plan(&conn);
    assert_eq!(
        (
            report.rows_removed,
            report.missing_rows,
            report.physical_rows,
            report.repointed
        ),
        (1, 0, 1, 0)
    );
    assert!(report.groups[0].physical);
    super::apply_repair(&mut conn, &report).unwrap();
    assert_eq!(
        recorded_rows(&conn),
        vec![(oldest.clone(), canonical_text(&library))]
    );
    assert!(drop.is_file() && library.is_file(), "both files stay");
    // The second copy is remembered, so a scan skips it.
    assert_eq!(twin_paths(&conn), vec![canonical_text(&drop)]);
    let outcome = import_now(
        &mut conn,
        &[dark_at(&drop, 1_000), dark_at(&library, 1_000)],
    );
    assert_eq!((outcome.imported, outcome.twins), (0, 1), "{outcome:?}");
    assert!(duplicate_plan(&conn).is_empty());
}

fn insert_master(conn: &Connection, uuid: &str, sources: &[&str], dark: Option<&str>) {
    conn.execute(
        "INSERT INTO psf_guard_calibration_master (
            master_uuid, rig_uuid, kind, cache_path, source_set_hash, source_count,
            source_frame_uuids, created_at, seiza_version, cache_version,
            dark_master_uuid, statistics_json
         ) VALUES (?1, 'rig', 'dark', ?2, ?1, ?3, ?4, 0, 'test', 1, ?5, '{}')",
        params![
            uuid,
            format!("/masters/{uuid}.fits"),
            sources.len() as i64,
            serde_json::to_string(sources).unwrap(),
            dark
        ],
    )
    .unwrap();
}

fn master_uuids(conn: &Connection) -> Vec<String> {
    let mut statement = conn
        .prepare("SELECT master_uuid FROM psf_guard_calibration_master ORDER BY master_uuid")
        .unwrap();
    statement
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap()
}

#[test]
fn repair_rebuilds_every_master_built_from_a_merged_row() {
    let temp = tempfile::tempdir().unwrap();
    let mut conn = Connection::open_in_memory().unwrap();
    // X: the library copy was recorded first and the newer drop-zone row's
    // file is gone. Y: two copies on disk. Z: one row, one file.
    let library = temp.path().join("library");
    let other = temp.path().join("other");
    std::fs::create_dir_all(&library).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    let x = library.join("x.fits");
    let y = library.join("y.fits");
    let z = library.join("z.fits");
    std::fs::write(&x, b"exposure x").unwrap();
    std::fs::write(&y, b"exposure y").unwrap();
    std::fs::write(&z, b"exposure z").unwrap();
    std::fs::copy(&y, other.join("y.fits")).unwrap();
    import_now(
        &mut conn,
        &[dark_at(&x, 1_000), dark_at(&y, 2_000), dark_at(&z, 3_000)],
    );
    let (ux, uy, uz) = (uuid_at(&conn, &x), uuid_at(&conn, &y), uuid_at(&conn, &z));
    record_twin_row(
        &conn,
        &ux,
        "x-dropped",
        &temp.path().join("drop").join("x.fits"),
    );
    record_twin_row(&conn, &uy, "y-second", &other.join("y.fits"));
    // Built from both rows of X: one exposure averaged twice.
    insert_master(&conn, "m-doubled", &[&ux, "x-dropped"], None);
    // Built on that master.
    insert_master(&conn, "m-on-doubled", &[&uz], Some("m-doubled"));
    insert_master(&conn, "m-second-y", &["y-second", &uz], None);
    insert_master(&conn, "m-kept-rows", &[&ux, &uy, &uz], None);

    let report = duplicate_plan(&conn);
    assert_eq!(report.groups.len(), 2);
    assert_eq!(
        (
            report.rows_removed,
            report.missing_rows,
            report.physical_rows,
            report.repointed
        ),
        (2, 1, 1, 0)
    );
    let mut invalidated = report
        .masters
        .iter()
        .map(|master| (master.master_uuid.as_str(), master.direct, master.doubled))
        .collect::<Vec<_>>();
    invalidated.sort();
    assert_eq!(
        invalidated,
        vec![
            ("m-doubled", true, true),
            ("m-on-doubled", false, false),
            ("m-second-y", true, false)
        ]
    );
    let outcome = super::apply_repair(&mut conn, &report).unwrap();
    assert_eq!(outcome.masters_removed, 3);
    assert_eq!(master_uuids(&conn), vec!["m-kept-rows"]);
    assert_eq!(
        recorded_rows(&conn)
            .into_iter()
            .map(|(uuid, _)| uuid)
            .collect::<Vec<_>>(),
        vec![ux, uy, uz],
        "the oldest row of each exposure stays"
    );
}

#[test]
fn a_repair_previewed_before_the_catalog_changed_is_refused() {
    let temp = tempfile::tempdir().unwrap();
    let (drop, library) = two_copies(&temp);
    let mut conn = Connection::open_in_memory().unwrap();
    import_now(&mut conn, &[dark_at(&library, 1_000)]);
    let oldest = uuid_at(&conn, &library);
    record_twin_row(&conn, &oldest, "second-copy", &drop);
    let report = duplicate_plan(&conn);
    assert!(!report.is_empty());

    set_frames_validity(
        &conn,
        std::slice::from_ref(&oldest),
        Some(ValidDirection::Backward),
    )
    .unwrap();
    let refused = super::apply_repair(&mut conn, &report).unwrap_err();
    assert!(matches!(refused, RepairError::Stale), "{refused}");
    assert_eq!(recorded_rows(&conn).len(), 2, "nothing applied");

    // A file that goes between preview and apply changes the plan, and so
    // its token.
    let fresh = duplicate_plan(&conn);
    std::fs::remove_file(&drop).unwrap();
    let after = duplicate_plan(&conn);
    assert_ne!(fresh.token, after.token);
    assert_eq!(after.missing_rows, 1);
}

#[test]
fn rows_whose_files_are_all_gone_or_differ_are_left_alone() {
    let temp = tempfile::tempdir().unwrap();
    let first = temp.path().join("a").join(TWIN_NAME);
    let second = temp.path().join("b").join(TWIN_NAME);
    std::fs::create_dir_all(first.parent().unwrap()).unwrap();
    std::fs::create_dir_all(second.parent().unwrap()).unwrap();
    std::fs::write(&first, b"exposure one").unwrap();
    std::fs::write(&second, b"exposure two").unwrap();
    let mut conn = Connection::open_in_memory().unwrap();
    // Recorded as an older build would have: one row per file.
    import_now(&mut conn, &[dark_at(&first, 1_000)]);
    let uuid = uuid_at(&conn, &first);
    record_twin_row(&conn, &uuid, "same-name", &second);
    assert!(
        duplicate_plan(&conn).is_empty(),
        "same name, second and size, other pixels"
    );

    // With no file readable — a disk that is not mounted — nothing merges
    // either.
    record_twin_row(&conn, &uuid, "copy", &temp.path().join("c").join(TWIN_NAME));
    let snapshot = duplicate_snapshot(&conn).unwrap();
    assert!(plan_repair(&snapshot, &FileAccess::resolving(|_| None)).is_empty());

    // A catalog copied from another host reads its files through a remapped
    // mount.
    let copied = Connection::open_in_memory().unwrap();
    ensure_schema(&copied).unwrap();
    let host = Path::new("/mnt/host-share");
    for (id, uuid, folder) in [(1, "kept", "library"), (2, "dropped", "drop")] {
        copied
            .execute(
                "INSERT INTO psf_guard_calibration_frame
                    (id, frame_uuid, rig_uuid, kind, source_path, source_fingerprint,
                     captured_at, file_size, added_at, updated_at)
                 VALUES (?1, ?2, 'rig', 'dark', ?3, 'f', 1000, ?4, 0, 0)",
                params![
                    id,
                    uuid,
                    host.join(folder).join(TWIN_NAME).to_string_lossy(),
                    EXPOSURE.len() as i64
                ],
            )
            .unwrap();
    }
    let local = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(local.path().join("library")).unwrap();
    std::fs::write(local.path().join("library").join(TWIN_NAME), EXPOSURE).unwrap();
    let snapshot = duplicate_snapshot(&copied).unwrap();
    assert!(plan_repair(&snapshot, &FileAccess::disk()).is_empty());
    let remapped = plan_repair(
        &snapshot,
        &FileAccess::remapped(host.to_path_buf(), local.path().to_path_buf()),
    );
    assert_eq!((remapped.groups.len(), remapped.missing_rows), (1, 1));
    assert_eq!(remapped.groups[0].keep, "kept");
}

// --- Review fixes ------------------------------------------------------------

/// A frame whose header gives no capture time, as many masters' do.
fn undated(path: &Path, gain: i64) -> FrameMeta {
    let mut frame = dark_at(path, 0);
    frame.timestamp = None;
    frame.gain = Some(gain);
    frame
}

#[test]
fn an_undated_missing_frame_is_never_taken_for_another_on_its_record() {
    // Two masters of one name in two folders, shot at different gains, of
    // one size. The gain-100 one is deleted.
    let temp = tempfile::tempdir().unwrap();
    let gain100 = temp.path().join("gain100").join("MasterDark_60s.fits");
    let gain200 = temp.path().join("gain200").join("MasterDark_60s.fits");
    std::fs::create_dir_all(gain100.parent().unwrap()).unwrap();
    std::fs::create_dir_all(gain200.parent().unwrap()).unwrap();
    std::fs::write(&gain100, b"master dark at gain 100").unwrap();
    std::fs::write(&gain200, b"master dark at gain 200").unwrap();
    let mut conn = Connection::open_in_memory().unwrap();
    let outcome = import_now(&mut conn, &[undated(&gain100, 100), undated(&gain200, 200)]);
    assert_eq!(outcome.imported, 2, "{outcome:?}");
    let gain100_row = uuid_at(&conn, &gain100);
    std::fs::remove_file(&gain100).unwrap();
    assert!(duplicate_plan(&conn).is_empty(), "different settings");

    // Even with the same settings, an undated row whose file is gone is not
    // merged into a live file of its name: there is nothing to compare.
    conn.execute(
        "UPDATE psf_guard_calibration_frame SET gain = 200 WHERE frame_uuid = ?1",
        [&gain100_row],
    )
    .unwrap();
    assert!(duplicate_plan(&conn).is_empty(), "undated, one file gone");

    // Nor does the import move it onto a new undated file of its name.
    let elsewhere = temp.path().join("elsewhere").join("MasterDark_60s.fits");
    std::fs::create_dir_all(elsewhere.parent().unwrap()).unwrap();
    std::fs::write(&elsewhere, b"master dark at gain 100").unwrap();
    let outcome = import_now(&mut conn, &[undated(&elsewhere, 200)]);
    assert_eq!((outcome.imported, outcome.updated), (1, 0), "{outcome:?}");
}

#[test]
fn a_dated_twin_must_agree_on_its_capture_settings() {
    let temp = tempfile::tempdir().unwrap();
    let (drop, library) = two_copies(&temp);
    let mut conn = Connection::open_in_memory().unwrap();
    import_now(&mut conn, &[dark_at(&drop, 1_000)]);
    let oldest = uuid_at(&conn, &drop);
    record_twin_row(&conn, &oldest, "library-row", &library);
    std::fs::remove_file(&drop).unwrap();
    for (column, value) in [
        ("gain", "200"),
        ("offset", "50"),
        ("exposure_s", "120.0"),
        ("camera_temp", "0.0"),
        ("filter_name", "'OIII'"),
        ("binning_x", "2"),
        ("width", "1500"),
    ] {
        conn.execute(
            &format!(
                "UPDATE psf_guard_calibration_frame SET {column} = {value}
                 WHERE frame_uuid = 'library-row'"
            ),
            [],
        )
        .unwrap();
        assert!(duplicate_plan(&conn).is_empty(), "{column} differs");
        record_rows_reset(&conn, &oldest);
    }
    assert_eq!(duplicate_plan(&conn).groups.len(), 1, "and agrees again");
}

/// Put the library row's settings back to the oldest row's.
fn record_rows_reset(conn: &Connection, oldest: &str) {
    conn.execute(
        "UPDATE psf_guard_calibration_frame SET
            (gain, offset, exposure_s, camera_temp, filter_name, binning_x, width) =
            (SELECT gain, offset, exposure_s, camera_temp, filter_name, binning_x, width
             FROM psf_guard_calibration_frame WHERE frame_uuid = ?1)
         WHERE frame_uuid = 'library-row'",
        [oldest],
    )
    .unwrap();
}

#[test]
fn a_kept_row_that_moves_takes_its_new_files_header_and_its_masters_rebuild() {
    let temp = tempfile::tempdir().unwrap();
    let (drop, library) = two_copies(&temp);
    let mut conn = Connection::open_in_memory().unwrap();
    import_now(&mut conn, &[dark_at(&drop, 1_000)]);
    let oldest = uuid_at(&conn, &drop);
    record_twin_row(&conn, &oldest, "library-row", &library);
    conn.execute(
        "UPDATE psf_guard_calibration_frame
         SET focal_length_mm = 530.0, camera_temp = -10.2, readout_mode_name = 'High Gain'
         WHERE frame_uuid = 'library-row'",
        [],
    )
    .unwrap();
    std::fs::remove_file(&drop).unwrap();
    // Built from the kept row alone: its file changes, so its record would
    // never be found again.
    insert_master(&conn, "m-kept-row", &[&oldest], None);

    let report = duplicate_plan(&conn);
    assert!(report.groups[0].repointed);
    assert_eq!(
        report
            .masters
            .iter()
            .map(|master| (master.master_uuid.as_str(), master.direct))
            .collect::<Vec<_>>(),
        vec![("m-kept-row", true)]
    );
    super::apply_repair(&mut conn, &report).unwrap();
    let (uuid, path, focal, temp_c, mode, fingerprint): (String, String, f64, f64, String, String) =
        conn.query_row(
            "SELECT frame_uuid, source_path, focal_length_mm, camera_temp, readout_mode_name,
                    source_fingerprint
             FROM psf_guard_calibration_frame",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                ))
            },
        )
        .unwrap();
    assert_eq!(uuid, oldest);
    assert_eq!(path, canonical_text(&library));
    assert_eq!((focal, temp_c, mode.as_str()), (530.0, -10.2, "High Gain"));
    assert_eq!(fingerprint, source_fingerprint(&library).1);
    assert!(master_uuids(&conn).is_empty());
}

#[test]
fn a_cleared_choice_survives_a_merge() {
    let temp = tempfile::tempdir().unwrap();
    let (drop, library) = two_copies(&temp);
    let mut conn = Connection::open_in_memory().unwrap();
    import_now(&mut conn, &[dark_at(&library, 1_000)]);
    let oldest = uuid_at(&conn, &library);
    record_twin_row(&conn, &oldest, "second-copy", &drop);
    // An older Keep out on the twin, and a newer clear on the kept row.
    set_overrides(
        &conn,
        &["second-copy".into()],
        Some(QualityOverride::Exclude),
    )
    .unwrap();
    set_overrides(
        &conn,
        std::slice::from_ref(&oldest),
        Some(QualityOverride::Use),
    )
    .unwrap();
    set_overrides(&conn, std::slice::from_ref(&oldest), None).unwrap();
    conn.execute(
        "UPDATE psf_guard_calibration_frame_quality SET override_at = override_at - 100
         WHERE frame_uuid = 'second-copy'",
        [],
    )
    .unwrap();

    let report = duplicate_plan(&conn);
    super::apply_repair(&mut conn, &report).unwrap();
    let (choice, at): (Option<String>, Option<i64>) = conn
        .query_row(
            "SELECT override_choice, override_at FROM psf_guard_calibration_frame_quality
             WHERE frame_uuid = ?1",
            [&oldest],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(choice, None, "the newer clear stands");
    assert!(at.is_some(), "and is dated, so a sync honours it");
}

/// Copy `source`'s calibration library into `destination`, as a pull does.
fn pull(source: &Connection, destination: &mut Connection) -> CalibrationSyncOutcome {
    let tx = destination.transaction().unwrap();
    let outcome = sync_library(source, &tx).unwrap();
    tx.commit().unwrap();
    outcome
}

#[test]
fn a_pull_from_an_unrepaired_catalog_does_not_undo_the_repair() {
    // The telescope recorded the drop-zone copy, then the library copy; the
    // drop-zone copy is gone. The review catalog pulled both, then repaired.
    let temp = tempfile::tempdir().unwrap();
    let (drop, library) = two_copies(&temp);
    let mut telescope = Connection::open_in_memory().unwrap();
    import_now(&mut telescope, &[dark_at(&drop, 1_000)]);
    let oldest = uuid_at(&telescope, &drop);
    record_twin_row(&telescope, &oldest, "library-row", &library);
    std::fs::remove_file(&drop).unwrap();
    let mut review = Connection::open_in_memory().unwrap();
    pull(&telescope, &mut review);
    assert_eq!(recorded_rows(&review).len(), 2);
    let report = duplicate_plan(&review);
    assert!(report.groups[0].repointed);
    super::apply_repair(&mut review, &report).unwrap();
    let repaired = vec![(oldest.clone(), canonical_text(&library))];
    assert_eq!(recorded_rows(&review), repaired);

    // A choice made on the telescope's merged row afterwards still arrives.
    set_overrides(
        &telescope,
        &["library-row".into()],
        Some(QualityOverride::Exclude),
    )
    .unwrap();
    let outcome = pull(&telescope, &mut review);
    assert_eq!(outcome.frames.skipped, 1, "{:?}", outcome.frames);
    assert_eq!(outcome.frames.inserted, 0);
    assert_eq!(recorded_rows(&review), repaired, "nothing came back");
    assert!(duplicate_plan(&review).is_empty());
    let choice: Option<String> = review
        .query_row(
            "SELECT override_choice FROM psf_guard_calibration_frame_quality
             WHERE frame_uuid = ?1",
            [&oldest],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(choice.as_deref(), Some("exclude"));

    // Again: nothing changes. The telescope keeps its own twins until it is
    // repaired itself.
    let again = pull(&telescope, &mut review);
    assert_eq!((again.frames.inserted, again.frames.updated), (0, 0));
    assert_eq!(recorded_rows(&review), repaired);
    assert_eq!(recorded_rows(&telescope).len(), 2);
}

#[test]
fn a_pull_from_a_repaired_catalog_merges_the_same_rows_here() {
    let temp = tempfile::tempdir().unwrap();
    let (drop, library) = two_copies(&temp);
    let mut telescope = Connection::open_in_memory().unwrap();
    import_now(&mut telescope, &[dark_at(&library, 1_000)]);
    let oldest = uuid_at(&telescope, &library);
    record_twin_row(&telescope, &oldest, "second-copy", &drop);
    insert_master(&telescope, "m-both", &[&oldest, "second-copy"], None);
    let mut review = Connection::open_in_memory().unwrap();
    pull(&telescope, &mut review);
    insert_master(&review, "m-both", &[&oldest, "second-copy"], None);
    let report = duplicate_plan(&telescope);
    super::apply_repair(&mut telescope, &report).unwrap();

    let outcome = pull(&telescope, &mut review);
    assert_eq!((outcome.merged_rows, outcome.merged_masters), (1, 1));
    assert_eq!(
        recorded_rows(&review),
        vec![(oldest.clone(), canonical_text(&library))]
    );
    assert!(master_uuids(&review).is_empty());
    assert_eq!(twin_paths(&review), vec![canonical_text(&drop)]);
}

#[test]
fn a_remembered_copy_whose_file_is_gone_is_forgotten() {
    let temp = tempfile::tempdir().unwrap();
    let (drop, library) = two_copies(&temp);
    let mut conn = Connection::open_in_memory().unwrap();
    import_now(
        &mut conn,
        &[dark_at(&library, 1_000), dark_at(&drop, 1_000)],
    );
    assert_eq!(twin_paths(&conn), vec![canonical_text(&drop)]);
    std::fs::remove_file(&drop).unwrap();
    let other = temp.path().join("other.fits");
    std::fs::write(&other, b"another dark").unwrap();
    import_now(&mut conn, &[dark_at(&other, 2_000)]);
    assert!(twin_paths(&conn).is_empty());
}
