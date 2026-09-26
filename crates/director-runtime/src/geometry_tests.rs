use super::{storage::*, storage_tests::*, tests::*, *};
use psf_guard_director_core::{
    geometry::Constraints,
    preparation::{Completion, Estimates, Next, Outcome},
    program::{LocalState, Program},
    visibility::{observe, Horizon, HorizonPoint, IcrsPosition},
    windows::Interval,
    Decision, Safety, State,
};
use psf_guard_director_ledger::Reservation;
use serde_json::json;
use tempfile::TempDir;
use tokio::io::duplex;

#[path = "geometry_tests/dispatch.rs"]
mod dispatch;

#[derive(Deserialize)]
struct Fixture {
    state: State,
    constraints: Constraints,
}

impl Fixture {
    fn new() -> Self {
        serde_json::from_str(include_str!("../tests/fixtures/geometry.json")).unwrap()
    }
    fn program(&self) -> Program {
        let mut program: Program = serde_json::from_str(include_str!(
            "../../director-core/tests/fixtures/execution-program.json"
        ))
        .unwrap();
        let start = self.constraints.rig.orientation.valid_from_ms;
        program.assignment.valid_from_ms = start;
        program.assignment.expires_at_ms = start + 60_000;
        program.assignment.goals[0].eligible_windows = vec![Interval {
            start_ms: start,
            end_ms: start + 60_000,
        }];
        program.assignment.goals[0].exposure_ms = 5_000;
        program.assignment.goals[0].overhead_ms = 1_000;
        program.recipes[0].exposure_ms = 5_000;
        program
    }
    fn open(&self) -> Operation {
        Operation::OpenGeometry {
            program: Box::new(self.program()),
            constraints: Box::new(self.constraints.clone()),
            state: self.state.clone(),
        }
    }
    fn begin(&self) -> Operation {
        Operation::BeginGeometryPreparation {
            preparation_id: "prep".into(),
            goal_id: "short-ha".into(),
            local: Box::new(LocalState {
                configuration: self.program().configuration,
                previous_pointing: None,
                mount_parked: false,
                rotator_connected: false,
                filter_exposures_since_dither: 0,
            }),
            estimates: Estimates::default(),
            constraints: Box::new(self.constraints.clone()),
            state: self.state.clone(),
        }
    }
    fn advance(&self) -> Operation {
        Operation::AdvanceGeometryPreparation {
            preparation_id: "prep".into(),
            configuration: Box::new(self.program().configuration),
            constraints: Box::new(self.constraints.clone()),
            state: self.state.clone(),
        }
    }
    fn reserve(&self) -> Operation {
        Operation::ReserveGeometryPrepared {
            preparation_id: "prep".into(),
            capture_id: "capture".into(),
            configuration: Box::new(self.program().configuration),
            constraints: Box::new(self.constraints.clone()),
            state: self.state.clone(),
        }
    }
    fn evaluate(&self) -> Operation {
        Operation::EvaluateGeometry {
            constraints: Box::new(self.constraints.clone()),
            state: self.state.clone(),
        }
    }
    fn complete(&self, ordinal: u32) -> Operation {
        Operation::CompletePreparation {
            completion: Completion {
                preparation_id: "prep".into(),
                ordinal,
                ended_at_ms: self.state.now_ms,
                elapsed_ms: 1,
                outcome: Outcome::Succeeded,
            },
        }
    }
    fn ready(&self, storage: &mut Storage) {
        for _ in 0..10 {
            match storage.handle(self.advance(), "rig-1").unwrap() {
                StorageReply::PreparationAdvanced {
                    next: Next::Run(command),
                } => {
                    assert!(matches!(
                        storage
                            .handle(self.complete(command.ordinal), "rig-1")
                            .unwrap(),
                        StorageReply::PreparationRecorded { .. }
                    ));
                }
                StorageReply::PreparationAdvanced {
                    next: Next::ReadyToReserve { .. },
                } => return,
                other => panic!("{other:?}"),
            }
        }
        panic!("preparation did not finish")
    }
}

#[tokio::test]
async fn geometry_wire_recovery_never_replays_issued_operations_or_reservations() {
    let f = Fixture::new();
    let dir = TempDir::new().unwrap();
    let (mut client, server) = duplex(MAX_FRAME_BYTES);
    let task = tokio::spawn(serve_with_storage(
        server,
        Some(Storage::acquire(dir.path()).unwrap()),
    ));
    handshake(&mut client).await;
    let StorageReply::GeometryOpened {
        info,
        program_version: 1,
        constraints_version: 1,
    } = exchange(&mut client, 1, f.open()).await
    else {
        panic!()
    };
    assert!(matches!(
        exchange(&mut client, 2, f.evaluate()).await,
        StorageReply::Evaluated {
            decision: Decision::Acquire { .. }
        }
    ));
    assert!(matches!(
        exchange(&mut client, 3, f.begin()).await,
        StorageReply::PreparationStarted { created: true, .. }
    ));
    assert!(matches!(
        exchange(&mut client, 4, f.advance()).await,
        StorageReply::PreparationAdvanced { next: Next::Run(_) }
    ));
    drop(client);
    assert_eq!(task.await.unwrap(), Ok(()));

    let (mut client, server) = duplex(MAX_FRAME_BYTES);
    let task = tokio::spawn(serve_with_storage(
        server,
        Some(Storage::acquire(dir.path()).unwrap()),
    ));
    handshake(&mut client).await;
    assert!(matches!(
        exchange(
            &mut client,
            1,
            Operation::OpenProgram {
                program: Box::new(f.program()),
                state: f.state.clone()
            }
        )
        .await,
        StorageReply::Error {
            code: StorageError::AssignmentMismatch
        }
    ));
    let StorageReply::GeometryOpened {
        info: recovered, ..
    } = exchange(&mut client, 2, f.open()).await
    else {
        panic!()
    };
    assert_eq!(info, recovered);
    assert!(matches!(
        exchange(&mut client, 3, f.begin()).await,
        StorageReply::PreparationStarted { created: false, .. }
    ));
    assert!(matches!(
        exchange(&mut client, 4, f.advance()).await,
        StorageReply::PreparationAdvanced {
            next: Next::InFlight { ordinal: 1 }
        }
    ));
    assert!(matches!(
        exchange(&mut client, 5, f.complete(1)).await,
        StorageReply::PreparationRecorded { .. }
    ));
    let mut id = 6;
    loop {
        let reply = exchange(&mut client, id, f.advance()).await;
        id += 1;
        match reply {
            StorageReply::PreparationAdvanced {
                next: Next::Run(command),
            } => {
                assert!(matches!(
                    exchange(&mut client, id, f.complete(command.ordinal)).await,
                    StorageReply::PreparationRecorded { .. }
                ));
                id += 1;
            }
            StorageReply::PreparationAdvanced {
                next: Next::ReadyToReserve { .. },
            } => break,
            other => panic!("{other:?}"),
        }
    }
    assert!(matches!(
        exchange(&mut client, id, f.reserve()).await,
        StorageReply::Reserved {
            outcome: Reservation::Created(_)
        }
    ));
    drop(client);
    assert_eq!(task.await.unwrap(), Ok(()));

    let (mut client, server) = duplex(MAX_FRAME_BYTES);
    let task = tokio::spawn(serve_with_storage(
        server,
        Some(Storage::acquire(dir.path()).unwrap()),
    ));
    handshake(&mut client).await;
    exchange(&mut client, 1, f.open()).await;
    assert!(matches!(
        exchange(&mut client, 2, f.reserve()).await,
        StorageReply::Reserved {
            outcome: Reservation::Existing(_)
        }
    ));
    let StorageReply::CaptureBindingFound {
        binding: Some(binding),
    } = exchange(
        &mut client,
        3,
        Operation::CaptureBinding {
            capture_id: "capture".into(),
        },
    )
    .await
    else {
        panic!()
    };
    assert_eq!(binding.recipe, f.program().recipes[0]);
    stop(&mut client, 4).await;
    assert_eq!(task.await.unwrap(), Ok(()));
}

#[test]
fn every_geometry_command_rejects_cross_rig_fields_before_storage() {
    let f = Fixture::new();
    for operation in [f.open(), f.evaluate(), f.begin(), f.advance(), f.reserve()] {
        let value = serde_json::to_value(operation).unwrap();
        for path in [
            "/state/rig_id",
            "/constraints/rig/rig_id",
            "/program/assignment/rig_id",
            "/program/configuration/rig_id",
            "/local/configuration/rig_id",
            "/configuration/rig_id",
        ] {
            let mut changed = value.clone();
            let Some(field) = changed.pointer_mut(path) else {
                continue;
            };
            *field = json!("other-rig");
            let dir = TempDir::new().unwrap();
            let mut storage = Storage::acquire(dir.path()).unwrap();
            assert_eq!(
                storage
                    .handle(serde_json::from_value(changed).unwrap(), "rig-1")
                    .unwrap_err(),
                ProtocolError::WrongRig,
                "{path}"
            );
            assert!(!dir.path().join("execution.sqlite").exists());
        }
    }
}

#[test]
fn geometry_commands_require_full_strict_constraints_and_remain_bounded() {
    let f = Fixture::new();
    for operation in [f.open(), f.evaluate(), f.begin(), f.advance(), f.reserve()] {
        let value = serde_json::to_value(operation).unwrap();
        let mut missing = value.clone();
        missing.as_object_mut().unwrap().remove("constraints");
        assert!(serde_json::from_value::<Operation>(missing).is_err());
        let mut unknown = value.clone();
        unknown["constraints"]["rig"]["computed_windows"] = json!([]);
        assert!(serde_json::from_value::<Operation>(unknown).is_err());
        let raw = serde_json::to_string(&value).unwrap().replace(
            "\"revision\":9007199254740993",
            "\"revision\":1,\"revision\":9007199254740993",
        );
        assert!(serde_json::from_str::<Operation>(&raw).is_err());
        assert!(
            serde_json::to_vec(&value).unwrap().len() < psf_guard_director_core::MAX_REQUEST_BYTES
        );
    }
}

#[test]
fn geometry_ledger_refuses_legacy_paths_and_latches_changed_constraints() {
    let f = Fixture::new();
    let dir = TempDir::new().unwrap();
    let mut storage = Storage::acquire(dir.path()).unwrap();
    storage.handle(f.open(), "rig-1").unwrap();
    storage.handle(f.begin(), "rig-1").unwrap();
    for operation in [
        Operation::Evaluate {
            state: f.state.clone(),
        },
        Operation::AdvanceProgramPreparation {
            preparation_id: "prep".into(),
            configuration: Box::new(f.program().configuration),
            state: f.state.clone(),
        },
        Operation::ReserveProgramPrepared {
            preparation_id: "prep".into(),
            capture_id: "bypass".into(),
            configuration: Box::new(f.program().configuration),
            state: f.state.clone(),
        },
        Operation::Reserve {
            capture_id: "bypass".into(),
            state: f.state.clone(),
        },
    ] {
        assert!(matches!(
            storage.handle(operation, "rig-1").unwrap(),
            StorageReply::Error {
                code: StorageError::ConflictingEvidence
            }
        ));
    }
    assert!(matches!(
        storage.handle(f.advance(), "rig-1").unwrap(),
        StorageReply::PreparationAdvanced { next: Next::Run(_) }
    ));
    let mut changed = Fixture::new();
    changed.constraints.rig.site.latitude_degrees += 1.0;
    assert!(matches!(
        storage.handle(changed.advance(), "rig-1").unwrap(),
        StorageReply::PreparationAdvanced {
            next: Next::InFlight { .. }
        }
    ));
    storage.handle(f.complete(1), "rig-1").unwrap();
    assert!(
        matches!(storage.handle(f.advance(), "rig-1").unwrap(), StorageReply::PreparationAdvanced { next: Next::Decision(Decision::CheckIn { reason }) } if reason == "observing_constraints_changed")
    );
}

#[test]
fn final_reservation_rechecks_geometry_safety_and_constraints() {
    for fault in 0..4 {
        let mut f = Fixture::new();
        let start = f.state.now_ms;
        let az = observe(
            IcrsPosition {
                ra_degrees: 83.0,
                dec_degrees: -5.0,
            },
            f.constraints.rig.site,
            f.constraints.rig.orientation,
            start + 21_777,
        )
        .unwrap()
        .azimuth_degrees;
        f.constraints.rig.horizon = Horizon::Custom {
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
        let dir = TempDir::new().unwrap();
        let mut storage = Storage::acquire(dir.path()).unwrap();
        assert!(matches!(
            storage.handle(f.open(), "rig-1").unwrap(),
            StorageReply::GeometryOpened { .. }
        ));
        storage.handle(f.begin(), "rig-1").unwrap();
        f.ready(&mut storage);
        match fault {
            0 => f.state.now_ms += 18_000,
            1 => f.constraints.rig.minimum_altitude_degrees += 1.0,
            2 => f.state.safety = Safety::Unsafe,
            _ => f.state.conditions_valid_until_ms = f.state.now_ms,
        }
        assert!(
            matches!(
                storage.handle(f.reserve(), "rig-1").unwrap(),
                StorageReply::Reserved {
                    outcome: Reservation::Decision(_)
                }
            ),
            "fault {fault}"
        );
        assert!(matches!(
            storage
                .handle(Operation::UnresolvedAttempt {}, "rig-1")
                .unwrap(),
            StorageReply::Found { attempt: None }
        ));
    }
}

#[tokio::test]
async fn previous_protocol_is_rejected_before_ledger_creation() {
    let dir = TempDir::new().unwrap();
    let (mut client, server) = duplex(MAX_FRAME_BYTES);
    let task = tokio::spawn(serve_with_storage(
        server,
        Some(Storage::acquire(dir.path()).unwrap()),
    ));
    let mut old = command(
        0,
        Command::Hello {
            runtime_version: RUNTIME_VERSION.into(),
            engine_version: ENGINE_VERSION.into(),
            contract_version: CONTRACT_VERSION,
            rig_id: "rig-1".into(),
        },
    );
    old.protocol_version = PROTOCOL_VERSION - 1;
    send(&mut client, &old).await;
    assert_eq!(task.await.unwrap(), Err(ProtocolError::VersionMismatch));
    assert!(!dir.path().join("execution.sqlite").exists());
}

#[test]
fn invalid_geometry_open_does_not_create_or_poison_storage() {
    for fault in 0..4 {
        let dir = TempDir::new().unwrap();
        let mut storage = Storage::acquire(dir.path()).unwrap();
        let mut f = Fixture::new();
        match fault {
            0 => f.constraints.schema_version += 1,
            1 => f.constraints.rig.site.latitude_degrees = 91.0,
            2 => f.constraints.rig.orientation.valid_until_ms = f.state.now_ms,
            _ => f.constraints.goals.clear(),
        }
        assert!(matches!(
            storage.handle(f.open(), "rig-1").unwrap(),
            StorageReply::Error {
                code: StorageError::InvalidSnapshot
            }
        ));
        assert!(!dir.path().join("execution.sqlite").exists());
        assert!(matches!(
            storage.handle(Fixture::new().open(), "rig-1").unwrap(),
            StorageReply::GeometryOpened { .. }
        ));
        assert!(matches!(
            storage.handle(Fixture::new().open(), "rig-1").unwrap(),
            StorageReply::Error {
                code: StorageError::AlreadyOpen
            }
        ));
    }
}

#[tokio::test]
async fn geometry_wire_requires_storage_and_an_explicit_open() {
    for enabled in [false, true] {
        let dir = TempDir::new().unwrap();
        let f = Fixture::new();
        let (mut client, server) = duplex(MAX_FRAME_BYTES);
        let task = tokio::spawn(serve_with_storage(
            server,
            enabled.then(|| Storage::acquire(dir.path()).unwrap()),
        ));
        handshake(&mut client).await;
        for (i, operation) in [f.evaluate(), f.begin(), f.advance(), f.reserve()]
            .into_iter()
            .enumerate()
        {
            let StorageReply::Error { code } = exchange(&mut client, i as u64 + 1, operation).await
            else {
                panic!()
            };
            assert_eq!(
                code,
                if enabled {
                    StorageError::NotOpen
                } else {
                    StorageError::Disabled
                }
            );
        }
        assert!(!dir.path().join("execution.sqlite").exists());
        stop(&mut client, 5).await;
        assert_eq!(task.await.unwrap(), Ok(()));
    }
}
