use psf_guard_director_meta::{CatalogIdentity, Error, MetaStore, Uuid};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tempfile::TempDir;

fn bind(store: &mut MetaStore) -> Uuid {
    let catalog = CatalogIdentity {
        id: Uuid::new_v4(),
        origin_instance_id: store.instance_id(),
    };
    store
        .bind_catalog_rig_after(catalog, "Rig", true, || Ok(()))
        .unwrap();
    catalog.id
}

/// Reads `catalog_rig` on a snapshot while the writer binds and removes the
/// catalog's rig over and over. Answers the first error, if any.
fn read_while_rigs_come_and_go() -> Option<Error> {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("meta.sqlite");
    let mut writer = MetaStore::create(&path).unwrap();
    let catalog = bind(&mut writer);
    let done = Arc::new(AtomicBool::new(false));
    let reading = {
        let done = Arc::clone(&done);
        std::thread::spawn(move || {
            let reader = MetaStore::open_reader(&path).unwrap();
            while !done.load(Ordering::Acquire) {
                let read = reader.snapshot().and_then(|taken| {
                    let read = reader.catalog_rig(catalog);
                    taken.end()?;
                    read
                });
                if let Err(error) = read {
                    return Some(error);
                }
            }
            None
        })
    };
    for _ in 0..150 {
        writer.remove_rig(catalog).unwrap();
        writer
            .bind_catalog_rig_after(
                CatalogIdentity {
                    id: catalog,
                    origin_instance_id: writer.instance_id(),
                },
                "Rig",
                false,
                || Ok(()),
            )
            .unwrap();
    }
    done.store(true, Ordering::Release);
    reading.join().unwrap()
}

/// A rig removed between a read of its binding and a read of the rig
/// looked like a corrupt database. On one snapshot the read sees the rig
/// either bound or gone.
#[test]
fn a_rig_read_on_one_snapshot_never_sees_its_removal_half_done() {
    // Without the snapshot this read failed with CorruptDatabase in every
    // run (checked by hand; a race is no assertion to keep in CI).
    assert!(read_while_rigs_come_and_go().is_none());
}

#[test]
fn snapshots_nest_and_one_dropped_early_leaves_the_reader_current() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("meta.sqlite");
    let mut writer = MetaStore::create(&path).unwrap();
    let reader = MetaStore::open_reader(&path).unwrap();
    let first = bind(&mut writer);

    let outer = reader.snapshot().unwrap();
    assert!(reader.catalog_rig(first).unwrap().is_some());
    // A method that takes its own snapshot runs inside this one.
    assert!(reader.project_intent(Uuid::new_v4()).unwrap().is_none());
    let second = bind(&mut writer);
    assert!(
        reader.catalog_rig(second).unwrap().is_none(),
        "the snapshot predates the second rig"
    );
    assert!(!reader.is_idle());
    // Dropped without end, as after an error: the next read is current.
    drop(outer);
    assert!(reader.is_idle());
    assert!(reader.catalog_rig(second).unwrap().is_some());
}
