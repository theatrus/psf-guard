use psf_guard_director_meta::MetaStore;
use uuid::Uuid;

/// A store file in a folder that goes when the returned guard drops.
fn scratch() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("meta.sqlite");
    (dir, path)
}

#[test]
fn a_reader_sees_commits_at_once_and_cannot_write() {
    let (_dir, path) = scratch();
    let mut writer = MetaStore::create(&path).unwrap();
    let reader = MetaStore::open_reader(&path).unwrap();
    assert_eq!(reader.instance_id(), writer.instance_id());

    let id = Uuid::new_v4();
    writer.create_project(id, "M31").unwrap();
    // No reopen, no retry: the pooled reader sees the committed row.
    assert_eq!(reader.project(id).unwrap().unwrap().name, "M31");

    // SQLite refuses the write on the reader, whatever the caller asks for.
    let refused = MetaStore::open_reader(&path)
        .unwrap()
        .create_project(Uuid::new_v4(), "M33");
    assert!(refused.is_err(), "a reader must not be able to write");
    assert!(writer.project(id).unwrap().is_some());
}

#[test]
fn a_reader_needs_a_store_at_the_current_schema() {
    let (_dir, path) = scratch();
    assert!(MetaStore::open_reader(&path).is_err(), "no file, no reader");
    let _writer = MetaStore::create(&path).unwrap();
    assert!(MetaStore::open_reader(&path).is_ok());
}
