use psf_guard_director_core::{recovery::*, Safety};
use psf_guard_director_ledger::recovery::{Error as StoreError, *};
use rusqlite::Connection;
use tempfile::TempDir;

fn identity() -> Identity {
    Identity {
        rig_id: "rig".into(),
        configuration_id: "config".into(),
        night_id: "night".into(),
        starts_at_ms: 1000,
        ends_at_ms: 10000,
    }
}
fn policy() -> Policy {
    Policy {
        revision: 1,
        quality_mode: QualityMode::Pause,
        bad_samples: 2,
        good_probes: 2,
        cooldown_ms: 100,
        maximum_hold_ms: 1000,
        maximum_probes: 3,
        operation_timeout_ms: 50,
        evidence_max_age_ms: 100,
        latest_resume_ms: 9000,
        maximum_consecutive_failures: 2,
        maximum_total_failures: 3,
        park_on_stop: true,
    }
}
fn request(revision: u64, now: u64, event: Event) -> Request {
    Request {
        night_id: "night".into(),
        configuration_id: "config".into(),
        event_id: format!("event-{revision}"),
        expected_revision: revision,
        now_ms: now,
        conditions: Conditions {
            safety: Safety::Safe,
            motion: Motion::Permitted,
        },
        event,
    }
}
fn quality(now: u64) -> Event {
    Event::Quality {
        sample: QualitySample {
            rig_id: "rig".into(),
            configuration_id: "config".into(),
            capture_id: format!("capture-{now}"),
            observed_at_ms: now,
            context: QualityContext {
                target_id: "target".into(),
                filter_id: "L".into(),
                exposure_ms: 1000,
                bin_x: 1,
                bin_y: 1,
                reference_id: "reference".into(),
                source: "pixels".into(),
                algorithm_revision: "1".into(),
            },
            verdict: Verdict::CorroboratedPoor,
        },
    }
}
fn open(temp: &TempDir) -> SessionStore {
    SessionStore::open(&temp.path().join("recovery.sqlite"), "rig").unwrap()
}
fn setup() -> (TempDir, SessionStore) {
    let temp = TempDir::new().unwrap();
    let mut store = open(&temp);
    assert!(
        store
            .begin_night(identity(), policy(), 1000)
            .unwrap()
            .newly_applied
    );
    (temp, store)
}
fn hold(store: &mut SessionStore) {
    store.apply(&request(0, 1001, quality(1001))).unwrap();
    store.apply(&request(1, 1002, quality(1002))).unwrap();
}

#[test]
fn restart_preserves_hold_budgets_and_journal() {
    let (temp, mut store) = setup();
    hold(&mut store);
    store
        .apply(&request(
            2,
            1102,
            Event::BeginRecovery {
                attempt_id: "probe".into(),
            },
        ))
        .unwrap();
    let before = store.current().unwrap().unwrap();
    drop(store);
    let mut store = open(&temp);
    assert_eq!(store.current().unwrap().unwrap(), before);
    assert_eq!(before.snapshot.probes_spent, 1);
    assert_eq!(before.snapshot.total_hold_ms, 100);
    let restored = store.begin_night(identity(), policy(), 1103).unwrap();
    assert!(!restored.newly_applied);
    assert_eq!(restored.record, before);
    assert!(store
        .apply(&request(
            3,
            1103,
            Event::BeginRecovery {
                attempt_id: "another".into()
            }
        ))
        .is_err());
    let result = store.apply(&request(3, 1152, Event::Tick {})).unwrap();
    assert!(matches!(
        result.record.snapshot.phase,
        Phase::Stopping {
            cause: Cause::RecoveryUncertain {},
            ..
        }
    ));
    assert_eq!(store.events("night", 0, 256).unwrap().len(), 4);
}

#[test]
fn exact_retries_return_current_state_without_dispatch_or_extra_events() {
    let (_temp, mut store) = setup();
    let first = request(0, 1001, quality(1001));
    assert!(store.apply(&first).unwrap().newly_applied);
    let current = store
        .apply(&request(1, 1002, quality(1002)))
        .unwrap()
        .record;
    let replay = store.apply(&first).unwrap();
    assert!(!replay.newly_applied);
    assert_eq!(replay.record, current); // Never rewind to the first receipt's state.
    let mut altered = first;
    altered.now_ms += 1;
    assert!(matches!(store.apply(&altered), Err(StoreError::Conflict)));
    assert_eq!(store.events("night", 0, 256).unwrap().len(), 2);
}

#[test]
fn a_new_event_id_cannot_double_count_an_image_or_failure() {
    let (_temp, mut store) = setup();
    store.apply(&request(0, 1001, quality(1001))).unwrap();
    let Event::Quality { mut sample } = quality(1001) else {
        unreachable!()
    };
    sample.observed_at_ms = 1002;
    assert!(matches!(
        store.apply(&request(1, 1002, Event::Quality { sample })),
        Err(StoreError::Conflict)
    ));
    assert_eq!(store.current().unwrap().unwrap().revision, 1);
    // The rolled-back request ID is still available for genuinely new evidence.
    store.apply(&request(1, 1002, quality(1002))).unwrap();

    let (_temp, mut store) = setup();
    let failure = Event::Failure {
        failure: Failure {
            attempt_id: "failed-guide".into(),
            operation: Operation::Guide,
            device_id: "guider".into(),
            target_id: "target".into(),
            uncertain: false,
        },
    };
    store.apply(&request(0, 1001, failure.clone())).unwrap();
    assert!(matches!(
        store.apply(&request(1, 1002, failure)),
        Err(StoreError::Conflict)
    ));
    assert_eq!(store.current().unwrap().unwrap().snapshot.total_failures, 1);
}

#[test]
fn competing_writers_and_wrong_configuration_cannot_replace_current_state() {
    let (temp, mut one) = setup();
    let mut two = open(&temp);
    one.apply(&request(0, 1001, quality(1001))).unwrap();
    let mut competing = request(0, 1002, quality(1002));
    competing.event_id = "competing".into();
    assert!(matches!(two.apply(&competing), Err(StoreError::Conflict)));
    let mut wrong = request(1, 1002, Event::Tick {});
    wrong.configuration_id = "different".into();
    assert!(matches!(two.apply(&wrong), Err(StoreError::WrongScope)));
    assert_eq!(one.current().unwrap(), two.current().unwrap());
}

#[test]
fn park_dispatch_is_not_replayed_after_restart_and_stop_cannot_be_cleared() {
    let (temp, mut store) = setup();
    store.apply(&request(0, 1001, Event::StopNight {})).unwrap();
    let park = request(
        1,
        1002,
        Event::BeginPark {
            attempt_id: "park".into(),
        },
    );
    store.apply(&park).unwrap();
    drop(store);
    let mut store = open(&temp);
    assert!(!store.apply(&park).unwrap().newly_applied);
    assert!(store
        .apply(&request(
            2,
            1003,
            Event::BeginPark {
                attempt_id: "new-park".into()
            }
        ))
        .is_err());
    let stopped = store
        .apply(&request(2, 1051, Event::Tick {}))
        .unwrap()
        .record;
    assert!(matches!(
        stopped.snapshot.phase,
        Phase::Stopped {
            shutdown: Shutdown::ParkUncertain,
            ..
        }
    ));
    assert_eq!(
        store
            .apply(&request(3, 1100, Event::Tick {}))
            .unwrap()
            .record
            .snapshot
            .phase,
        stopped.snapshot.phase
    );
    // Midnight/end-of-night does not silently clear a persisted latch on reopen.
    assert!(
        !store
            .begin_night(identity(), policy(), 20000)
            .unwrap()
            .newly_applied
    );
    let mut changed_policy = policy();
    changed_policy.revision += 1;
    assert!(matches!(
        store.begin_night(identity(), changed_policy, 1100),
        Err(StoreError::Conflict)
    ));
    let mut renamed = identity();
    renamed.night_id = "new-workload".into();
    assert!(matches!(
        store.begin_night(renamed, policy(), 1100),
        Err(StoreError::Conflict)
    ));
}

#[test]
fn new_night_needs_a_finished_nonoverlapping_session_and_retains_old_journal() {
    let (_temp, mut store) = setup();
    let mut next = identity();
    next.night_id = "next-night".into();
    next.starts_at_ms = 10000;
    next.ends_at_ms = 20000;
    let mut next_policy = policy();
    next_policy.latest_resume_ms = 19000;
    assert!(matches!(
        store.begin_night(next.clone(), next_policy.clone(), 10000),
        Err(StoreError::Conflict)
    ));
    let mut stop = request(0, 1001, Event::StopNight {});
    stop.conditions.motion = Motion::Prohibited;
    store.apply(&stop).unwrap();
    assert!(
        store
            .begin_night(next, next_policy, 10000)
            .unwrap()
            .newly_applied
    );
    assert!(matches!(store.apply(&stop), Err(StoreError::WrongScope)));
    assert_eq!(store.events("night", 0, 10).unwrap().len(), 1);
    assert_eq!(store.current().unwrap().unwrap().snapshot.probes_spent, 0);
}

#[test]
fn journal_is_ordered_and_paginated_without_issuing_operations() {
    let (_temp, mut store) = setup();
    hold(&mut store);
    let first = store.events("night", 0, 1).unwrap();
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].revision, 1);
    let second = store.events("night", first[0].revision, 1).unwrap();
    assert_eq!(second[0].revision, 2);
    assert!(store.events("night", 2, 1).unwrap().is_empty());
    assert!(store.events("night", 0, 257).is_err());
    assert!(store.events("night", u64::MAX, 1).is_err());
}

#[test]
fn storage_refuses_foreign_databases_wrong_rigs_and_unknown_schema() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("ts.sqlite");
    let db = Connection::open(&path).unwrap();
    db.execute_batch("CREATE TABLE project(id INTEGER)")
        .unwrap();
    drop(db);
    assert!(matches!(
        SessionStore::open(&path, "rig"),
        Err(StoreError::ForeignDatabase)
    ));
    assert!(matches!(
        SessionStore::open(std::path::Path::new("relative.sqlite"), "rig"),
        Err(StoreError::InvalidInput)
    ));
    let (temp, store) = setup();
    drop(store);
    assert!(matches!(
        SessionStore::open(&temp.path().join("recovery.sqlite"), "other"),
        Err(StoreError::WrongScope)
    ));
    let db = Connection::open(temp.path().join("recovery.sqlite")).unwrap();
    db.pragma_update(None, "user_version", 999).unwrap();
    drop(db);
    assert!(matches!(
        SessionStore::open(&temp.path().join("recovery.sqlite"), "rig"),
        Err(StoreError::UnsupportedSchema)
    ));
}

#[test]
fn corrupt_snapshot_and_negative_revision_fail_closed() {
    for sql in [
        "UPDATE night SET payload='{}'",
        "UPDATE night SET revision=-1",
    ] {
        let (temp, store) = setup();
        drop(store);
        let db = Connection::open(temp.path().join("recovery.sqlite")).unwrap();
        db.execute_batch(sql).unwrap();
        drop(db);
        let store = open(&temp);
        assert!(matches!(store.current(), Err(StoreError::Corrupt)));
    }
}

#[test]
fn native_attempt_ids_cannot_be_reused_for_a_later_probe() {
    let (_temp, mut store) = setup();
    hold(&mut store);
    store
        .apply(&request(
            2,
            1102,
            Event::BeginRecovery {
                attempt_id: "probe".into(),
            },
        ))
        .unwrap();
    store
        .apply(&request(
            3,
            1103,
            Event::RecoveryCompleted {
                attempt_id: "probe".into(),
                result: RecoveryResult::Failed {},
            },
        ))
        .unwrap();
    assert!(matches!(
        store.apply(&request(
            4,
            1203,
            Event::BeginRecovery {
                attempt_id: "probe".into()
            }
        )),
        Err(StoreError::Conflict)
    ));
    assert_eq!(store.current().unwrap().unwrap().snapshot.probes_spent, 1);
    store
        .apply(&request(
            4,
            1203,
            Event::BeginRecovery {
                attempt_id: "probe-2".into(),
            },
        ))
        .unwrap();
    assert_eq!(store.current().unwrap().unwrap().snapshot.probes_spent, 2);
}

#[test]
fn failed_commit_rolls_back_snapshot_evidence_and_request_identity() {
    let (temp, mut store) = setup();
    let db = Connection::open(temp.path().join("recovery.sqlite")).unwrap();
    db.execute_batch("CREATE TRIGGER fail_event BEFORE INSERT ON event BEGIN SELECT RAISE(ABORT,'injected failure'); END").unwrap();
    let first = request(0, 1001, quality(1001));
    let error = store.apply(&first).unwrap_err();
    assert!(std::error::Error::source(&error).is_some());
    assert_eq!(store.current().unwrap().unwrap().revision, 0);
    assert!(store.events("night", 0, 10).unwrap().is_empty());
    db.execute_batch("DROP TRIGGER fail_event").unwrap();
    assert!(store.apply(&first).unwrap().newly_applied);
    assert_eq!(
        store.current().unwrap().unwrap().snapshot.consecutive_bad,
        1
    );
}

#[test]
fn newly_committed_input_is_not_a_native_dispatch_permit() {
    let (_temp, mut store) = setup();
    hold(&mut store);
    let mut probe = request(
        2,
        1102,
        Event::BeginRecovery {
            attempt_id: "probe".into(),
        },
    );
    probe.conditions.safety = Safety::Unsafe;
    probe.conditions.motion = Motion::Unknown;
    let applied = store.apply(&probe).unwrap();
    assert!(applied.newly_applied);
    assert_eq!(applied.record.snapshot.probes_spent, 0);
    assert!(matches!(
        applied.record.snapshot.phase,
        Phase::Stopped {
            cause: Cause::Safety {},
            shutdown: Shutdown::MotionBlocked
        }
    ));
    assert!(!store.apply(&probe).unwrap().newly_applied);
}
