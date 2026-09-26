use psf_guard_director_core::{
    geometry::*,
    preparation::{Completion, Estimates, Next, Outcome},
    program::*,
    visibility::*,
    windows::*,
    Decision, Safety, State,
};
use psf_guard_director_ledger::{Error, Evidence, Ledger, Reservation};
use rusqlite::Connection;
use std::{
    path::Path,
    sync::{Arc, Barrier},
};
use tempfile::TempDir;

const START: u64 = 1_790_409_600_000;

#[path = "geometry/capture_dispatch.rs"]
mod capture_dispatch;

#[derive(Clone)]
struct Fixture {
    program: Program,
    state: State,
    constraints: Constraints,
}

impl Fixture {
    fn new() -> Self {
        let mut program: Program = serde_json::from_str(include_str!(
            "../../director-core/tests/fixtures/execution-program.json"
        ))
        .unwrap();
        program.assignment.valid_from_ms = START;
        program.assignment.expires_at_ms = START + 60_000;
        let goal = &mut program.assignment.goals[0];
        goal.exposure_ms = 5_000;
        goal.overhead_ms = 1_000;
        goal.eligible_windows = vec![Interval {
            start_ms: START,
            end_ms: START + 60_000,
        }];
        program.recipes[0].exposure_ms = 5_000;
        let state = State {
            rig_id: "rig-1".into(),
            configuration_id: "config-1".into(),
            now_ms: START,
            conditions_valid_until_ms: START + 120_000,
            safety: Safety::Safe,
            at_boundary: true,
            operator_stop: false,
            meridian_exclusion: MeridianExclusion {
                before_ms: 0,
                after_ms: 0,
            },
        };
        let mut constraints = Constraints {
            schema_version: CONSTRAINTS_VERSION,
            rig: RigConstraints {
                rig_id: "rig-1".into(),
                configuration_id: "config-1".into(),
                revision: 1,
                site: Site {
                    latitude_degrees: 35.0,
                    longitude_degrees: -120.0,
                    elevation_meters: 1000.0,
                },
                orientation: EarthOrientation {
                    ut1_minus_utc_seconds: 0.0,
                    polar_motion_x_radians: 0.0,
                    polar_motion_y_radians: 0.0,
                    valid_from_ms: START,
                    valid_until_ms: START + 120_001,
                },
                horizon: Horizon::FixedMinimum {},
                minimum_altitude_degrees: -89.0,
                maximum_altitude_degrees: 89.0,
                meridian_exclusion: state.meridian_exclusion,
            },
            goals: vec![GoalLimits {
                goal_id: "short-ha".into(),
                minimum_altitude_degrees: -89.0,
                maximum_altitude_degrees: 89.0,
                horizon_offset_degrees: 0.0,
            }],
        };
        let az = observe(
            IcrsPosition {
                ra_degrees: 83.0,
                dec_degrees: -5.0,
            },
            constraints.rig.site,
            constraints.rig.orientation,
            START + 21_777,
        )
        .unwrap()
        .azimuth_degrees;
        constraints.rig.horizon = Horizon::Custom {
            points: [
                (0.0, -89.0),
                (az.next_down(), -89.0),
                (az, 89.0),
                (az.next_up(), -89.0),
                (360.0, -89.0),
            ]
            .into_iter()
            .map(|(azimuth_degrees, altitude_degrees)| HorizonPoint {
                azimuth_degrees,
                altitude_degrees,
            })
            .collect(),
        };
        Self {
            program,
            state,
            constraints,
        }
    }
    fn local(&self) -> LocalState {
        LocalState {
            configuration: self.program.configuration.clone(),
            previous_pointing: None,
            mount_parked: false,
            rotator_connected: false,
            filter_exposures_since_dither: 0,
        }
    }
    fn open(&self, path: &Path) -> Ledger {
        Ledger::open_geometry(
            path,
            self.program.clone(),
            self.constraints.clone(),
            self.state.clone(),
        )
        .unwrap()
    }
    fn begin(&self, ledger: &mut Ledger) {
        assert!(
            ledger
                .begin_geometry_preparation(
                    "prep",
                    "short-ha",
                    self.local(),
                    Estimates::default(),
                    self.state.clone(),
                    &self.constraints
                )
                .unwrap()
                .created
        );
    }
    fn next(&self, ledger: &mut Ledger) -> Next {
        ledger
            .advance_geometry_preparation(
                "prep",
                self.state.clone(),
                &self.program.configuration,
                &self.constraints,
            )
            .unwrap()
    }
    fn finish(&self, ledger: &mut Ledger, ordinal: u32) {
        ledger
            .complete_preparation(Completion {
                preparation_id: "prep".into(),
                ordinal,
                ended_at_ms: self.state.now_ms,
                elapsed_ms: 1,
                outcome: Outcome::Succeeded,
            })
            .unwrap();
    }
    fn ready(&self, ledger: &mut Ledger) {
        loop {
            match self.next(ledger) {
                Next::Run(command) => self.finish(ledger, command.ordinal),
                Next::ReadyToReserve { .. } => break,
                other => panic!("{other:?}"),
            }
        }
    }
    fn reserve(&self, ledger: &mut Ledger) -> Result<Reservation, Error> {
        ledger.reserve_geometry_prepared(
            "prep",
            "capture",
            self.state.clone(),
            &self.program.configuration,
            &self.constraints,
        )
    }
    fn first_end(&self) -> u64 {
        BoundGeometry::new(
            BoundProgram::new(self.program.clone(), &self.state).unwrap(),
            self.constraints.clone(),
            &self.state,
        )
        .unwrap()
        .windows("short-ha")
        .unwrap()[0]
            .end_ms
    }
}

#[test]
fn geometry_binding_progress_and_pending_commands_survive_reopen() {
    let f = Fixture::new();
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("execution.sqlite");
    let mut ledger = f.open(&path);
    let identity = ledger.info();
    f.begin(&mut ledger);
    let Next::Run(command) = f.next(&mut ledger) else {
        panic!()
    };
    drop(ledger);
    let mut ledger = f.open(&path);
    assert_eq!(ledger.info(), identity);
    assert_eq!(
        ledger
            .active_preparation()
            .unwrap()
            .unwrap()
            .pending
            .as_ref(),
        Some(&command)
    );
    assert_eq!(
        f.next(&mut ledger),
        Next::InFlight {
            ordinal: command.ordinal
        }
    );
    assert!(
        !ledger
            .begin_geometry_preparation(
                "prep",
                "short-ha",
                f.local(),
                Estimates::default(),
                f.state.clone(),
                &f.constraints
            )
            .unwrap()
            .created
    );
    f.finish(&mut ledger, command.ordinal);
    f.ready(&mut ledger);
    assert!(matches!(
        f.reserve(&mut ledger).unwrap(),
        Reservation::Created(_)
    ));
    drop(ledger);
    let mut ledger = f.open(&path);
    assert!(matches!(
        f.reserve(&mut ledger).unwrap(),
        Reservation::Existing(_)
    ));
    assert_eq!(
        ledger.capture_binding("capture").unwrap().unwrap().recipe,
        f.program.recipes[0]
    );
    ledger
        .record(
            "capture",
            Evidence::Saved {
                image_id: "image".into(),
                elapsed_ms: 5000,
            },
        )
        .unwrap();
    assert!(!matches!(
        ledger
            .evaluate_geometry(f.state.clone(), &f.constraints)
            .unwrap(),
        Decision::Acquire { .. }
    ));
    assert_eq!(ledger.events_after(0, 256).unwrap().len(), 2);
}

#[test]
fn legacy_calls_and_reopening_cannot_bypass_geometry() {
    let f = Fixture::new();
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("execution.sqlite");
    let mut ledger = f.open(&path);
    assert!(matches!(
        ledger.evaluate(f.state.clone()),
        Err(Error::ConflictingEvidence)
    ));
    assert!(matches!(
        ledger.reserve("capture", f.state.clone()),
        Err(Error::ConflictingEvidence)
    ));
    assert!(matches!(
        ledger.begin_program_preparation(
            "prep",
            "short-ha",
            f.local(),
            Estimates::default(),
            f.state.clone()
        ),
        Err(Error::ConflictingEvidence)
    ));
    f.begin(&mut ledger);
    assert!(matches!(
        ledger.advance_program_preparation("prep", f.state.clone(), &f.program.configuration),
        Err(Error::ConflictingEvidence)
    ));
    assert!(matches!(
        ledger.advance_preparation("prep", f.state.clone()),
        Err(Error::ConflictingEvidence)
    ));
    f.ready(&mut ledger);
    assert!(matches!(
        ledger.reserve_program_prepared(
            "prep",
            "capture",
            f.state.clone(),
            &f.program.configuration
        ),
        Err(Error::ConflictingEvidence)
    ));
    assert!(matches!(
        ledger.reserve_prepared("prep", "capture", f.state.clone()),
        Err(Error::ConflictingEvidence)
    ));
    drop(ledger);
    assert!(matches!(
        Ledger::open_program(&path, f.program.clone(), f.state.clone()),
        Err(Error::AssignmentMismatch)
    ));
    assert!(Ledger::open(&path, f.program.assignment.clone(), f.state.clone()).is_err());
    let mut changed = f.clone();
    changed.constraints.rig.site.longitude_degrees += 0.001;
    assert!(matches!(
        Ledger::open_geometry(&path, changed.program, changed.constraints, changed.state),
        Err(Error::AssignmentMismatch)
    ));
    assert!(f.open(&path).attempt("capture").unwrap().is_none());
}

#[test]
fn legacy_program_migration_preserves_evidence_without_adopting_geometry() {
    let f = Fixture::new();
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("execution.sqlite");
    let mut legacy = Ledger::open_program(&path, f.program.clone(), f.state.clone()).unwrap();
    legacy
        .begin_program_preparation(
            "prep",
            "short-ha",
            f.local(),
            Estimates::default(),
            f.state.clone(),
        )
        .unwrap();
    legacy
        .advance_program_preparation("prep", f.state.clone(), &f.program.configuration)
        .unwrap();
    let identity = legacy.info();
    let record = legacy.active_preparation().unwrap();
    let events = legacy.preparation_events_after(0, 256).unwrap();
    drop(legacy);
    let db = Connection::open(&path).unwrap();
    db.execute_batch("DROP TABLE execution_geometry; ALTER TABLE allocation DROP COLUMN geometry_required; PRAGMA user_version=3;").unwrap();
    assert!(matches!(
        Ledger::open_geometry(
            &path,
            f.program.clone(),
            f.constraints.clone(),
            f.state.clone()
        ),
        Err(Error::AssignmentMismatch)
    ));
    assert_eq!(
        db.pragma_query_value(None, "user_version", |r| r.get::<_, i32>(0))
            .unwrap(),
        3
    );
    let mut legacy = Ledger::open_program(&path, f.program.clone(), f.state.clone()).unwrap();
    assert_eq!(legacy.info(), identity);
    assert_eq!(legacy.active_preparation().unwrap(), record);
    assert_eq!(legacy.preparation_events_after(0, 256).unwrap(), events);
    assert_eq!(
        db.pragma_query_value(None, "user_version", |r| r.get::<_, i32>(0))
            .unwrap(),
        4
    );
    assert!(matches!(
        legacy.evaluate_geometry(f.state, &f.constraints),
        Err(Error::ConflictingEvidence)
    ));
}

#[test]
fn slow_operations_and_late_reservation_cannot_cross_horizon_gap() {
    for late_reserve in [false, true] {
        let mut f = Fixture::new();
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("execution.sqlite");
        let mut ledger = f.open(&path);
        f.begin(&mut ledger);
        if late_reserve {
            f.ready(&mut ledger);
        } else {
            let Next::Run(command) = f.next(&mut ledger) else {
                panic!()
            };
            f.state.now_ms = f.first_end() - 4_999;
            f.finish(&mut ledger, command.ordinal);
        }
        f.state.now_ms = f.first_end() - 4_999;
        if late_reserve {
            assert!(matches!(
                f.reserve(&mut ledger).unwrap(),
                Reservation::Decision(Decision::Wait { .. })
            ));
        } else {
            assert!(matches!(
                f.next(&mut ledger),
                Next::Decision(Decision::Wait { .. })
            ));
        }
        assert!(ledger.attempt("capture").unwrap().is_none());
        drop(ledger);
        let mut ledger = f.open(&path);
        assert!(matches!(
            ledger.active_preparation().unwrap().unwrap().halted,
            Some(Decision::Wait { .. })
        ));
    }
}

#[test]
fn changed_constraints_latch_durably_and_safety_still_wins() {
    for unsafe_now in [false, true] {
        let f = Fixture::new();
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("execution.sqlite");
        let mut ledger = f.open(&path);
        f.begin(&mut ledger);
        let Next::Run(command) = f.next(&mut ledger) else {
            panic!()
        };
        let mut changed = f.clone();
        changed.constraints.rig.site.elevation_meters += 1.0;
        if unsafe_now {
            changed.state.safety = Safety::Unsafe;
        }
        let next = changed.next(&mut ledger);
        if unsafe_now {
            assert!(matches!(next, Next::Decision(Decision::Stop { .. })));
        } else {
            assert_eq!(
                next,
                Next::InFlight {
                    ordinal: command.ordinal
                }
            );
        }
        drop(ledger);
        let mut ledger = f.open(&path);
        f.finish(&mut ledger, command.ordinal);
        assert!(matches!(
            f.next(&mut ledger),
            Next::Decision(Decision::CheckIn { .. } | Decision::Stop { .. })
        ));
        assert!(f.reserve(&mut ledger).is_err());
    }
}

#[test]
fn final_reservation_requires_current_constraints_and_equipment() {
    for equipment in [false, true] {
        let f = Fixture::new();
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("execution.sqlite");
        let mut ledger = f.open(&path);
        f.begin(&mut ledger);
        f.ready(&mut ledger);
        let mut changed = f.clone();
        if equipment {
            changed.program.configuration.camera_id = "other".into();
        } else {
            changed.constraints.goals[0].horizon_offset_degrees += 1.0;
        }
        if equipment {
            assert!(matches!(
                changed.reserve(&mut ledger),
                Err(Error::AssignmentMismatch)
            ));
        } else {
            assert!(matches!(
                changed.reserve(&mut ledger).unwrap(),
                Reservation::Decision(Decision::CheckIn { .. })
            ));
        }
        assert!(ledger.attempt("capture").unwrap().is_none());
    }
}

#[test]
fn missing_or_corrupt_geometry_and_checkpoint_metadata_never_downgrades() {
    for sql in [
        "DELETE FROM execution_geometry",
        "UPDATE execution_geometry SET payload='{}'",
        "UPDATE allocation SET geometry_required=0",
    ] {
        let f = Fixture::new();
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("execution.sqlite");
        drop(f.open(&path));
        Connection::open(&path).unwrap().execute_batch(sql).unwrap();
        assert!(matches!(
            Ledger::open_geometry(
                &path,
                f.program.clone(),
                f.constraints.clone(),
                f.state.clone()
            ),
            Err(Error::CorruptLedger)
        ));
        assert!(Ledger::open_program(&path, f.program.clone(), f.state.clone()).is_err());
    }
    let f = Fixture::new();
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("execution.sqlite");
    let mut ledger = f.open(&path);
    f.begin(&mut ledger);
    f.next(&mut ledger);
    Connection::open(&path)
        .unwrap()
        .execute("UPDATE preparation SET checkpoint=?1", [b"{}".as_slice()])
        .unwrap();
    assert!(matches!(
        ledger.active_preparation(),
        Err(Error::CorruptLedger)
    ));
    assert!(ledger
        .advance_geometry_preparation("prep", f.state, &f.program.configuration, &f.constraints)
        .is_err());
}

#[test]
fn event_failure_rolls_back_issue_receipt_and_reservation_with_checkpoint() {
    let f = Fixture::new();
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("execution.sqlite");
    let mut ledger = f.open(&path);
    f.begin(&mut ledger);
    let db = Connection::open(&path).unwrap();
    let fail = || {
        db.execute_batch("CREATE TRIGGER fail_event BEFORE INSERT ON preparation_event BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap()
    };
    let recover = || db.execute_batch("DROP TRIGGER fail_event").unwrap();
    fail();
    assert!(matches!(
        ledger.advance_geometry_preparation(
            "prep",
            f.state.clone(),
            &f.program.configuration,
            &f.constraints
        ),
        Err(Error::Sqlite(_))
    ));
    assert!(ledger
        .active_preparation()
        .unwrap()
        .unwrap()
        .pending
        .is_none());
    recover();
    let Next::Run(command) = f.next(&mut ledger) else {
        panic!()
    };
    let completion = Completion {
        preparation_id: "prep".into(),
        ordinal: command.ordinal,
        ended_at_ms: START,
        elapsed_ms: 1,
        outcome: Outcome::Succeeded,
    };
    fail();
    assert!(matches!(
        ledger.complete_preparation(completion.clone()),
        Err(Error::Sqlite(_))
    ));
    assert!(ledger
        .active_preparation()
        .unwrap()
        .unwrap()
        .observations
        .is_empty());
    recover();
    ledger.complete_preparation(completion).unwrap();
    f.ready(&mut ledger);
    fail();
    assert!(matches!(f.reserve(&mut ledger), Err(Error::Sqlite(_))));
    assert!(ledger.attempt("capture").unwrap().is_none());
    assert!(ledger.events_after(0, 256).unwrap().is_empty());
    recover();
    assert!(matches!(
        f.reserve(&mut ledger).unwrap(),
        Reservation::Created(_)
    ));
}

#[test]
fn concurrent_handles_issue_once_and_reserve_once() {
    let f = Fixture::new();
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("execution.sqlite");
    let mut ledger = f.open(&path);
    f.begin(&mut ledger);
    let barrier = Arc::new(Barrier::new(2));
    let threads: Vec<_> = [ledger, f.open(&path)]
        .into_iter()
        .map(|mut ledger| {
            let f = f.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                f.next(&mut ledger)
            })
        })
        .collect();
    let results: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert_eq!(
        results.iter().filter(|r| matches!(r, Next::Run(_))).count(),
        1
    );
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, Next::InFlight { .. }))
            .count(),
        1
    );
    let mut ledger = f.open(&path);
    let pending = ledger
        .active_preparation()
        .unwrap()
        .unwrap()
        .pending
        .unwrap();
    f.finish(&mut ledger, pending.ordinal);
    f.ready(&mut ledger);
    let barrier = Arc::new(Barrier::new(2));
    let threads: Vec<_> = [ledger, f.open(&path)]
        .into_iter()
        .map(|mut ledger| {
            let f = f.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                f.reserve(&mut ledger).unwrap()
            })
        })
        .collect();
    let results: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, Reservation::Created(_)))
            .count(),
        1
    );
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, Reservation::Existing(_)))
            .count(),
        1
    );
}

#[test]
fn geometry_process_exit_helper() {
    let Some(path) = std::env::var_os("DIRECTOR_GEOMETRY_CRASH_DB") else {
        return;
    };
    let f = Fixture::new();
    let mut ledger = f.open(Path::new(&path));
    f.begin(&mut ledger);
    if std::env::var("DIRECTOR_GEOMETRY_CRASH_PHASE").unwrap() == "issued" {
        f.next(&mut ledger);
    } else {
        f.ready(&mut ledger);
        f.reserve(&mut ledger).unwrap();
    }
    std::process::exit(82);
}

#[test]
fn meridian_geometry_blocks_reservation_despite_empty_claimed_transits() {
    let mut f = Fixture::new();
    let transit = START + 30_000;
    let mut position = IcrsPosition {
        ra_degrees: 83.0,
        dec_degrees: -5.0,
    };
    for _ in 0..8 {
        let ha = observe(
            position,
            f.constraints.rig.site,
            f.constraints.rig.orientation,
            transit,
        )
        .unwrap()
        .hour_angle_degrees;
        position.ra_degrees = (position.ra_degrees + ha).rem_euclid(360.0);
    }
    f.program.targets[0].icrs_ra_mas =
        (position.ra_degrees * f64::from(MAS_PER_DEGREE)).round() as u32;
    f.constraints.rig.horizon = Horizon::FixedMinimum {};
    f.constraints.rig.orientation.valid_from_ms = START - 3_000;
    f.constraints.rig.meridian_exclusion = MeridianExclusion {
        before_ms: 1_000,
        after_ms: 2_000,
    };
    f.state.meridian_exclusion = f.constraints.rig.meridian_exclusion;
    f.program.assignment.goals[0].transits = Some(TransitCoverage {
        searched: Interval {
            start_ms: START - 2_000,
            end_ms: START + 61_000,
        },
        transits_ms: vec![],
    });
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("execution.sqlite");
    let mut ledger = f.open(&path);
    f.begin(&mut ledger);
    f.ready(&mut ledger);
    f.state.now_ms = f.first_end() - 4_999;
    assert!(matches!(
        f.reserve(&mut ledger).unwrap(),
        Reservation::Decision(Decision::Wait { .. })
    ));
    assert!(ledger.attempt("capture").unwrap().is_none());
}

#[test]
fn safety_and_expired_conditions_block_final_reservation_after_reopen() {
    for unsafe_now in [false, true] {
        let mut f = Fixture::new();
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("execution.sqlite");
        let mut ledger = f.open(&path);
        f.begin(&mut ledger);
        f.ready(&mut ledger);
        drop(ledger);
        let mut ledger = f.open(&path);
        if unsafe_now {
            f.state.safety = Safety::Unknown;
        } else {
            f.state.conditions_valid_until_ms = f.state.now_ms;
        }
        let result = f.reserve(&mut ledger).unwrap();
        if unsafe_now {
            assert!(matches!(
                result,
                Reservation::Decision(Decision::Stop { .. })
            ));
        } else {
            assert!(matches!(
                result,
                Reservation::Decision(Decision::CheckIn { .. })
            ));
        }
        assert!(ledger.attempt("capture").unwrap().is_none());
    }
}

#[test]
fn abrupt_process_exit_cannot_replay_geometry_preparation_or_capture() {
    for phase in ["issued", "captured"] {
        let f = Fixture::new();
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("execution.sqlite");
        let status = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "geometry_process_exit_helper"])
            .env("DIRECTOR_GEOMETRY_CRASH_DB", &path)
            .env("DIRECTOR_GEOMETRY_CRASH_PHASE", phase)
            .status()
            .unwrap();
        assert_eq!(status.code(), Some(82));
        let mut ledger = f.open(&path);
        if phase == "issued" {
            assert!(matches!(f.next(&mut ledger), Next::InFlight { .. }));
        } else {
            assert!(matches!(
                f.reserve(&mut ledger).unwrap(),
                Reservation::Existing(_)
            ));
        }
    }
}
