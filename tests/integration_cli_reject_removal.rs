//! The reject-removal commands, run as the built binary against a registry:
//! preview, apply, list, restore, and emptying the trash.

use std::{
    path::Path,
    process::{Command, Output},
};

use rusqlite::Connection;

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_psf-guard"))
        .args(args)
        .output()
        .expect("running psf-guard")
}

fn run_ok(args: &[&str]) -> String {
    let output = run(args);
    assert!(
        output.status.success(),
        "psf-guard {args:?} failed ({})\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn count(path: &Path, sql: &str) -> i64 {
    Connection::open(path)
        .unwrap()
        .query_row(sql, [], |row| row.get(0))
        .unwrap()
}

#[test]
fn rejects_are_previewed_removed_listed_and_restored_from_the_command_line() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path();
    let light = root.join("images/M42/2026-10-01/LIGHT");
    std::fs::create_dir_all(&light).unwrap();
    std::fs::write(light.join("M42_Ha_001.fits"), vec![1u8; 2048]).unwrap();
    std::fs::write(light.join("M42_Ha_002.fits"), vec![2u8; 2048]).unwrap();
    let database = root.join("scheduler.sqlite");
    {
        let connection = Connection::open(&database).unwrap();
        psf_guard::ts_schema::apply_schema(&connection).unwrap();
        connection
            .execute_batch(
                r#"INSERT INTO project (Id, profileId, name, guid) VALUES (1, 'p', 'M42', 'pg');
                 INSERT INTO target (Id, name, active, epochcode, projectId, guid) VALUES (1, 'M42', 1, 0, 1, 'tg');
                 INSERT INTO acquiredimage (Id, projectId, targetId, acquireddate, filtername, gradingStatus, metadata, guid) VALUES
                    (1, 1, 1, 1759300000, 'Ha', 2, '{"FileName":"M42_Ha_001.fits"}', 'bad-one'),
                    (2, 1, 1, 1759300300, 'Ha', 1, '{"FileName":"M42_Ha_002.fits"}', 'good-one');"#,
            )
            .unwrap();
    }
    let registry = root.join("registry.json");
    std::fs::write(
        &registry,
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": 2,
            "databases": [{
                "id": "rig",
                "name": "Rig",
                "db_path": database,
                "image_dirs": [root.join("images")],
            }],
        }))
        .unwrap(),
    )
    .unwrap();
    let registry = registry.display().to_string();
    let base = ["--db", "rig", "--registry", registry.as_str()];

    // First sight dates the reject now, so with the default grace period it waits.
    let waiting = run_ok(&[&["remove-rejects"][..], &base].concat());
    assert!(waiting.contains("0 frame(s) to remove"), "{waiting}");
    assert!(
        waiting.contains("1 rejected less than 7 day(s) ago"),
        "{waiting}"
    );

    // With no grace period it qualifies; a preview changes nothing.
    let preview = run_ok(&[&["remove-rejects", "--days", "0"][..], &base].concat());
    assert!(
        preview.contains("REMOVE  #1 M42 M42_Ha_001.fits"),
        "{preview}"
    );
    assert!(preview.contains("Preview only"), "{preview}");
    assert_eq!(count(&database, "SELECT COUNT(*) FROM acquiredimage"), 2);

    let applied = run_ok(&[&["remove-rejects", "--days", "0", "--apply"][..], &base].concat());
    assert!(applied.contains("Removed 1 frame(s)"), "{applied}");
    assert_eq!(count(&database, "SELECT COUNT(*) FROM acquiredimage"), 1);
    assert!(!light.join("M42_Ha_001.fits").exists());
    assert!(light.join("M42_Ha_002.fits").exists());

    let listed = run_ok(&[&["list-removed"][..], &base].concat());
    let batch = listed.split_whitespace().next().unwrap().to_string();
    assert!(
        listed.contains("1 frame(s)") && listed.contains("in the trash until"),
        "{listed}"
    );

    // Nothing emptied yet, so nothing to purge.
    let purged = run_ok(&[&["purge-removed"][..], &base].concat());
    assert!(purged.contains("Purged the saved rows of 0"), "{purged}");

    // Not yet past its retention: emptying the trash keeps it.
    let emptied = run_ok(&[&["empty-reject-trash"][..], &base].concat());
    assert!(emptied.contains("Deleted 0 file(s)"), "{emptied}");

    let restored = run_ok(&[&["restore-removed", "--batch", batch.as_str()][..], &base].concat());
    assert!(restored.contains("Restored 1 frame(s)"), "{restored}");
    assert!(light.join("M42_Ha_001.fits").is_file());
    assert_eq!(
        count(
            &database,
            "SELECT COUNT(*) FROM acquiredimage WHERE guid = 'bad-one' AND gradingStatus = 2"
        ),
        1
    );

    // Restore needs a batch or a GUID.
    let refused = run(&[&["restore-removed"][..], &base].concat());
    assert!(!refused.status.success());
}
