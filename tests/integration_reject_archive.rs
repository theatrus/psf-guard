//! End-to-end test for the out-of-tree reject archive (A7).
//!
//! Builds a tempdir with a NINA-style layout, a SQLite DB containing the
//! Target Scheduler tables (including the v22 `guid` column required by
//! the archive feature), rejects three images, then exercises the
//! `move_rejects` orchestrator. Verifies the move plan, the on-disk
//! result, the `psf_guard_archive` row, the manifest file, and
//! idempotent re-runs.

use std::fs;
use std::path::PathBuf;

use psf_guard::commands::reject_archive::{
    ensure_archive_schema, move_rejects, require_target_scheduler_guid, resolve_config,
    MoveRejectsOptions,
};
use rusqlite::{params, Connection};
use tempfile::tempdir;

struct Fixture {
    _tmp: tempfile::TempDir,
    image_dir: PathBuf,
    db_path: PathBuf,
    // Per-image absolute source paths so the test can assert their move/non-move.
    img1_src: PathBuf, // rejected, has sidecars
    img2_src: PathBuf, // rejected, no sidecars
    img3_src: PathBuf, // accepted, must NOT move
}

/// Build a self-contained NINA-style layout:
///
/// ```
/// images/
///   M31/
///     2026-04-16/
///       LIGHT/
///         img_0028.fits          (rejected) + .xisf + .json sidecars
///         img_0029.fits          (rejected, no sidecars)
///         img_0030.fits          (accepted)
///   M42/
///     2026-04-17/
///       LIGHT/
///         (calibration master that must not be touched)
///         Bias_master.fits
/// ```
fn build_fixture() -> Fixture {
    let tmp = tempdir().unwrap();
    let root = tmp.path();
    let image_dir = root.join("images");
    let m31_light = image_dir.join("M31").join("2026-04-16").join("LIGHT");
    let m42_light = image_dir.join("M42").join("2026-04-17").join("LIGHT");
    fs::create_dir_all(&m31_light).unwrap();
    fs::create_dir_all(&m42_light).unwrap();

    let img1_src = m31_light.join("img_0028.fits");
    fs::write(&img1_src, b"primary 28").unwrap();
    fs::write(m31_light.join("img_0028.xisf"), b"xisf 28").unwrap();
    fs::write(m31_light.join("img_0028.json"), b"json 28").unwrap();
    // Non-matching extension; should NOT move.
    fs::write(m31_light.join("img_0028.log"), b"log 28").unwrap();

    let img2_src = m31_light.join("img_0029.fits");
    fs::write(&img2_src, b"primary 29").unwrap();

    let img3_src = m31_light.join("img_0030.fits");
    fs::write(&img3_src, b"primary 30 accepted").unwrap();

    // Calibration master in a different folder — same .fits ext but
    // different stem from any LIGHT, never touched.
    fs::write(m42_light.join("Bias_master.fits"), b"bias").unwrap();

    let db_path = root.join("scheduler.sqlite");
    let conn = Connection::open(&db_path).unwrap();
    conn.execute_batch(
        r#"
        CREATE TABLE project (
            Id INTEGER PRIMARY KEY,
            profileId TEXT,
            name TEXT NOT NULL,
            description TEXT
        );
        CREATE TABLE target (
            Id INTEGER PRIMARY KEY,
            projectId INTEGER NOT NULL,
            name TEXT NOT NULL,
            active INTEGER NOT NULL DEFAULT 1,
            ra REAL,
            dec REAL
        );
        CREATE TABLE acquiredimage (
            Id INTEGER PRIMARY KEY,
            projectId INTEGER NOT NULL,
            targetId INTEGER NOT NULL,
            acquireddate INTEGER,
            filtername TEXT NOT NULL,
            gradingStatus INTEGER NOT NULL DEFAULT 0,
            metadata TEXT NOT NULL DEFAULT '{}',
            rejectreason TEXT,
            profileId TEXT,
            guid TEXT
        );

        INSERT INTO project (Id, profileId, name) VALUES (1, 'default', 'M31');
        INSERT INTO target (Id, projectId, name) VALUES (1, 1, 'M31');
    "#,
    )
    .unwrap();

    // 1700000000 = 2023-11-14 UTC; date doesn't matter for the archive
    // logic — we go through directory_tree fallback. The metadata FileName
    // is what matters for source-on-disk discovery.
    let insert = "INSERT INTO acquiredimage
        (Id, projectId, targetId, acquireddate, filtername, gradingStatus, metadata, guid)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?)";
    conn.execute(
        insert,
        params![
            1,
            1,
            1,
            1700000000i64,
            "L",
            2, // Rejected
            r#"{"FileName": "img_0028.fits"}"#,
            "guid-img-28",
        ],
    )
    .unwrap();
    conn.execute(
        insert,
        params![
            2,
            1,
            1,
            1700000100i64,
            "L",
            2,
            r#"{"FileName": "img_0029.fits"}"#,
            "guid-img-29",
        ],
    )
    .unwrap();
    conn.execute(
        insert,
        params![
            3,
            1,
            1,
            1700000200i64,
            "L",
            1, // Accepted — must NOT move
            r#"{"FileName": "img_0030.fits"}"#,
            "guid-img-30",
        ],
    )
    .unwrap();

    Fixture {
        _tmp: tmp,
        image_dir,
        db_path,
        img1_src,
        img2_src,
        img3_src,
    }
}

fn open_conn(path: &std::path::Path) -> Connection {
    Connection::open(path).unwrap()
}

fn run_archive(
    fixture: &Fixture,
    dry_run: bool,
) -> psf_guard::commands::reject_archive::MoveRejectsSummary {
    let conn = open_conn(&fixture.db_path);
    require_target_scheduler_guid(&conn).unwrap();
    ensure_archive_schema(&conn).unwrap();

    let config = resolve_config(None, None, None, None).unwrap();
    let options = MoveRejectsOptions {
        config,
        project_filter: None,
        target_filter: None,
        dry_run,
        source_db_slug: "test-rig".into(),
        verbose: false,
    };
    let dirs = vec![fixture.image_dir.to_string_lossy().into_owned()];
    move_rejects(&conn, &dirs, &options).unwrap()
}

#[test]
fn dry_run_leaves_filesystem_and_db_alone() {
    let fixture = build_fixture();
    let summary = run_archive(&fixture, true);

    assert_eq!(summary.planned, 2, "two rejected rows expected");
    assert_eq!(summary.archived, 0);
    assert_eq!(summary.already_archived, 0);
    assert_eq!(summary.errors, 0);

    // Files still at their source paths.
    assert!(fixture.img1_src.exists());
    assert!(fixture.img2_src.exists());
    assert!(fixture.img3_src.exists());

    // No archive table rows were inserted.
    let conn = open_conn(&fixture.db_path);
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM psf_guard_archive", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn live_run_moves_primary_plus_matching_sidecars_only() {
    let fixture = build_fixture();
    let summary = run_archive(&fixture, false);

    assert_eq!(summary.archived, 2);
    assert_eq!(summary.errors, 0);

    // Primary should have moved to <image_dir>/M31/REJECT/2026-04-16/LIGHT/.
    // (Date string in get_possible_paths comes from acquireddate; but the
    // directory_tree fallback works regardless. We just check the new
    // location matches the depth-1 rule against image_dir.)
    let archive_dir = fixture
        .image_dir
        .join("M31")
        .join("REJECT")
        .join("2026-04-16")
        .join("LIGHT");
    assert!(
        archive_dir.join("img_0028.fits").exists(),
        "primary 28 should have moved"
    );
    assert!(
        archive_dir.join("img_0028.xisf").exists(),
        "matching .xisf sidecar should have moved"
    );
    assert!(
        archive_dir.join("img_0028.json").exists(),
        "matching .json sidecar should have moved"
    );
    assert!(
        !archive_dir.join("img_0028.log").exists(),
        "non-matching extension should NOT have moved"
    );

    // The .log file stays where the primary was.
    let orig_dir = fixture.img1_src.parent().unwrap();
    assert!(orig_dir.join("img_0028.log").exists());
    // The accepted image stays put.
    assert!(fixture.img3_src.exists());

    // Archive table records both moves keyed on guid.
    let conn = open_conn(&fixture.db_path);
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM psf_guard_archive", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 2);

    let (orig, arch, segment, depth, sidecar_json): (String, String, String, i64, String) = conn
        .query_row(
            "SELECT original_path, archive_path, segment_name, archive_depth, sidecar_files
             FROM psf_guard_archive WHERE acquired_image_guid = 'guid-img-28'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap();
    assert!(orig.contains("img_0028.fits"));
    assert!(arch.ends_with(
        std::path::PathBuf::from("REJECT")
            .join("2026-04-16")
            .join("LIGHT")
            .join("img_0028.fits")
            .to_string_lossy()
            .as_ref()
    ));
    assert_eq!(segment, "REJECT");
    assert_eq!(depth, 1);
    let sidecars: Vec<String> = serde_json::from_str(&sidecar_json).unwrap();
    let mut sidecars_sorted = sidecars.clone();
    sidecars_sorted.sort();
    assert_eq!(sidecars_sorted, vec!["img_0028.json", "img_0028.xisf"]);

    // Manifest file appended.
    let manifest = archive_dir.join(".psf-guard-manifest.json");
    assert!(manifest.exists());
    let body: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&manifest).unwrap()).unwrap();
    let moves = body["moves"].as_array().unwrap();
    assert_eq!(moves.len(), 2, "manifest should have both moves");
}

#[test]
fn re_run_is_idempotent_and_reports_already_archived() {
    let fixture = build_fixture();
    let _first = run_archive(&fixture, false);
    let second = run_archive(&fixture, false);

    assert_eq!(second.archived, 0, "second run should archive nothing");
    assert_eq!(
        second.already_archived, 2,
        "both prior moves should be counted as already_archived"
    );
    assert_eq!(second.errors, 0);
}

#[test]
fn legacy_db_without_guid_column_is_refused() {
    // Same layout as build_fixture, but drop the `guid` column on
    // acquiredimage. The schema guard should refuse the run with a
    // clear message.
    let tmp = tempdir().unwrap();
    let db_path = tmp.path().join("legacy.sqlite");
    let conn = Connection::open(&db_path).unwrap();
    conn.execute_batch(
        "CREATE TABLE acquiredimage (
            Id INTEGER PRIMARY KEY,
            projectId INTEGER NOT NULL,
            targetId INTEGER NOT NULL,
            gradingStatus INTEGER NOT NULL DEFAULT 0,
            metadata TEXT NOT NULL DEFAULT '{}'
        );",
    )
    .unwrap();

    let err = require_target_scheduler_guid(&conn).unwrap_err();
    let msg = format!("{err}");
    assert!(msg.contains("guid"), "msg should mention guid: {msg}");
    assert!(
        msg.contains("filter-rejected"),
        "msg should point at the legacy command: {msg}"
    );
}
