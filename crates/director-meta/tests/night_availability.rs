use psf_guard_director_core::night::NightDate;
use psf_guard_director_meta::{
    night_availability::{Availability, MarkEdit, MarkScope},
    MetaStore, Uuid,
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tempfile::TempDir;

/// A save commits between the reads of a scope's revision and its marks
/// unless they share a snapshot; the marks would then look newer than their
/// scope, which reads as a corrupt database. Night plans read marks while a
/// person saves them, so a reader must never see that.
#[test]
fn marks_read_while_they_are_saved_are_never_newer_than_their_scope() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("meta.sqlite");
    let mut writer = MetaStore::create(&path).unwrap();
    let rig = writer.create_rig(Uuid::new_v4(), "Rig").unwrap().id;
    let first: NightDate = "2026-09-24".parse().unwrap();
    let last: NightDate = "2026-12-31".parse().unwrap();
    let done = Arc::new(AtomicBool::new(false));
    let reading = {
        let path = path.clone();
        let done = Arc::clone(&done);
        std::thread::spawn(move || {
            let reader = MetaStore::open_reader(&path).unwrap();
            let mut reads = 0;
            while !done.load(Ordering::Acquire) || reads < 100 {
                let marks = reader
                    .night_marks(MarkScope::Rig, rig, first, last)
                    .unwrap_or_else(|error| panic!("read {reads} failed: {error:?}"));
                assert!(marks
                    .marks
                    .iter()
                    .all(|mark| mark.revision <= marks.revision));
                reads += 1;
            }
            reads
        })
    };
    let mut date = first;
    let mut revision = 0;
    for _ in 0..300 {
        let saved = writer
            .save_night_marks(
                MarkScope::Rig,
                rig,
                revision,
                &[MarkEdit {
                    date,
                    state: Availability::Unavailable,
                    windows: Vec::new(),
                    note: String::new(),
                }],
                1_000,
            )
            .unwrap();
        revision = saved.revision;
        date = date.next().unwrap();
    }
    done.store(true, Ordering::Release);
    assert!(reading.join().unwrap() >= 100);
}
