use super::{storage::*, storage_tests::*, tests::*, *};
use psf_guard_director_core::preparation::{Completion, Estimates, Next, Outcome};
use psf_guard_director_core::program::{LocalState, PointingContext, Program};
use psf_guard_director_core::Decision;
use psf_guard_director_ledger::{Evidence, Reservation};
use serde_json::json;
use tempfile::TempDir;
use tokio::io::duplex;

fn program() -> Program {
    serde_json::from_str(include_str!(
        "../../director-core/tests/fixtures/execution-program.json"
    ))
    .unwrap()
}

fn open() -> Operation {
    Operation::OpenProgram {
        program: Box::new(program()),
        state: fixture().state,
    }
}

fn begin() -> Operation {
    let program = program();
    Operation::BeginProgramPreparation {
        preparation_id: "prep".into(),
        goal_id: "short-ha".into(),
        local: Box::new(LocalState {
            previous_pointing: Some(PointingContext {
                configuration_id: program.configuration.id.clone(),
                target: program.targets[0].clone(),
            }),
            configuration: program.configuration,
            mount_parked: false,
            rotator_connected: false,
            filter_exposures_since_dither: 0,
        }),
        estimates: Estimates::default(),
        state: fixture().state,
    }
}

fn advance() -> Operation {
    Operation::AdvanceProgramPreparation {
        preparation_id: "prep".into(),
        configuration: Box::new(program().configuration),
        state: fixture().state,
    }
}

fn reserve() -> Operation {
    Operation::ReserveProgramPrepared {
        preparation_id: "prep".into(),
        capture_id: "capture".into(),
        configuration: Box::new(program().configuration),
        state: fixture().state,
    }
}

#[tokio::test]
async fn bound_program_round_trip_preserves_recipe_and_pending_progress() {
    let dir = TempDir::new().unwrap();
    let (mut client, server) = duplex(MAX_FRAME_BYTES);
    let task = tokio::spawn(serve_with_storage(
        server,
        Some(Storage::acquire(dir.path()).unwrap()),
    ));
    handshake(&mut client).await;
    let StorageReply::ProgramOpened {
        info,
        program_version,
    } = exchange(&mut client, 1, open()).await
    else {
        panic!()
    };
    assert_eq!(program_version, 1);
    assert_eq!(info.assignment_revision, 9_007_199_254_740_993);
    assert!(matches!(
        exchange(&mut client, 2, begin()).await,
        StorageReply::PreparationStarted { created: true, .. }
    ));
    for ordinal in 1..=2 {
        let StorageReply::PreparationAdvanced {
            next: Next::Run(command),
        } = exchange(&mut client, ordinal * 2 + 1, advance()).await
        else {
            panic!()
        };
        assert_eq!(command.ordinal as u64, ordinal);
        let StorageReply::PreparationRecorded { record } = exchange(
            &mut client,
            ordinal * 2 + 2,
            Operation::CompletePreparation {
                completion: Completion {
                    preparation_id: "prep".into(),
                    ordinal: command.ordinal,
                    ended_at_ms: fixture().state.now_ms,
                    elapsed_ms: u64::MAX,
                    outcome: Outcome::Succeeded,
                },
            },
        )
        .await
        else {
            panic!()
        };
        assert_eq!(
            record.observations.last().unwrap().completion.elapsed_ms,
            u64::MAX
        );
    }
    assert!(matches!(
        exchange(&mut client, 7, reserve()).await,
        StorageReply::Reserved {
            outcome: Reservation::Created(_)
        }
    ));
    let StorageReply::CaptureBindingFound {
        binding: Some(binding),
    } = exchange(
        &mut client,
        8,
        Operation::CaptureBinding {
            capture_id: "capture".into(),
        },
    )
    .await
    else {
        panic!()
    };
    assert_eq!(binding.ledger, info);
    assert_eq!(binding.target, program().targets[0]);
    assert_eq!(binding.recipe, program().recipes[0]);
    assert_eq!(binding.configuration, program().configuration);
    assert!(matches!(
        exchange(
            &mut client,
            9,
            Operation::CaptureBinding {
                capture_id: "unknown".into()
            }
        )
        .await,
        StorageReply::CaptureBindingFound { binding: None }
    ));
    exchange(
        &mut client,
        10,
        Operation::Record {
            capture_id: "capture".into(),
            evidence: Evidence::Saved {
                image_id: "image".into(),
                elapsed_ms: 9007199254740993,
            },
        },
    )
    .await;
    assert!(
        matches!(exchange(&mut client, 11, Operation::Evaluate { state: fixture().state }).await, StorageReply::Evaluated { decision: Decision::Wait { reason } } if reason == "pending_assessment")
    );
    stop(&mut client, 12).await;
    assert_eq!(task.await.unwrap(), Ok(()));
}

#[tokio::test]
async fn bound_program_reopen_refuses_downgrade_and_recovers_issued_work() {
    let dir = TempDir::new().unwrap();
    let (mut client, server) = duplex(MAX_FRAME_BYTES);
    let task = tokio::spawn(serve_with_storage(
        server,
        Some(Storage::acquire(dir.path()).unwrap()),
    ));
    handshake(&mut client).await;
    exchange(&mut client, 1, open()).await;
    exchange(&mut client, 2, begin()).await;
    assert!(matches!(
        exchange(&mut client, 3, advance()).await,
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
        exchange(&mut client, 1, Operation::Open { request: fixture() }).await,
        StorageReply::Error {
            code: StorageError::AssignmentMismatch
        }
    ));
    let mut changed = program();
    changed.recipes[0].gain = Some(41);
    assert!(matches!(
        exchange(
            &mut client,
            2,
            Operation::OpenProgram {
                program: Box::new(changed),
                state: fixture().state
            }
        )
        .await,
        StorageReply::Error {
            code: StorageError::AssignmentMismatch
        }
    ));
    assert!(matches!(
        exchange(&mut client, 3, open()).await,
        StorageReply::ProgramOpened { .. }
    ));
    assert!(
        matches!(exchange(&mut client, 4, Operation::ActivePreparation {}).await, StorageReply::PreparationFound { record: Some(record) } if record.pending.is_some())
    );
    assert!(matches!(
        exchange(&mut client, 5, advance()).await,
        StorageReply::PreparationAdvanced {
            next: Next::InFlight { ordinal: 1 }
        }
    ));
    assert!(matches!(
        exchange(
            &mut client,
            6,
            Operation::AdvancePreparation {
                preparation_id: "prep".into(),
                state: fixture().state
            }
        )
        .await,
        StorageReply::Error {
            code: StorageError::ConflictingEvidence
        }
    ));
    assert!(matches!(
        exchange(
            &mut client,
            7,
            Operation::Reserve {
                capture_id: "bypass".into(),
                state: fixture().state
            }
        )
        .await,
        StorageReply::Error {
            code: StorageError::ConflictingEvidence
        }
    ));
    stop(&mut client, 8).await;
    assert_eq!(task.await.unwrap(), Ok(()));
}

#[test]
fn invalid_program_or_configuration_is_a_scoped_error_without_new_work() {
    let dir = TempDir::new().unwrap();
    let mut storage = Storage::acquire(dir.path()).unwrap();
    let mut invalid = program();
    invalid.recipes[0].gain = Some(101);
    assert!(matches!(
        storage
            .handle(
                Operation::OpenProgram {
                    program: Box::new(invalid),
                    state: fixture().state
                },
                "rig-1"
            )
            .unwrap(),
        StorageReply::Error {
            code: StorageError::InvalidProgram
        }
    ));
    assert!(!dir.path().join("execution.sqlite").exists());
    assert!(matches!(
        storage.handle(open(), "rig-1").unwrap(),
        StorageReply::ProgramOpened { .. }
    ));
    assert!(matches!(
        storage.handle(open(), "rig-1").unwrap(),
        StorageReply::Error {
            code: StorageError::AlreadyOpen
        }
    ));
    let Operation::BeginProgramPreparation {
        mut local,
        estimates,
        state,
        ..
    } = begin()
    else {
        panic!()
    };
    local.configuration.filters[0].position = Some(3);
    assert!(matches!(
        storage
            .handle(
                Operation::BeginProgramPreparation {
                    preparation_id: "prep".into(),
                    goal_id: "short-ha".into(),
                    local,
                    estimates,
                    state
                },
                "rig-1"
            )
            .unwrap(),
        StorageReply::Error {
            code: StorageError::AssignmentMismatch
        }
    ));
    storage.handle(begin(), "rig-1").unwrap();
    for operation in [advance(), reserve()] {
        let mut json = serde_json::to_value(operation).unwrap();
        json["configuration"]["camera_id"] = json!("replacement");
        assert!(matches!(
            storage
                .handle(serde_json::from_value(json).unwrap(), "rig-1")
                .unwrap(),
            StorageReply::Error {
                code: StorageError::AssignmentMismatch
            }
        ));
    }
    assert!(
        matches!(storage.handle(Operation::ActivePreparation {}, "rig-1").unwrap(), StorageReply::PreparationFound { record: Some(record) } if record.pending.is_none())
    );
}

#[test]
fn all_program_operations_and_nested_snapshots_are_rig_scoped() {
    for operation in [open(), begin(), advance(), reserve()] {
        let value = serde_json::to_value(operation).unwrap();
        let paths = match value["action"].as_str().unwrap() {
            "open_program" => vec![
                "/program/assignment/rig_id",
                "/program/configuration/rig_id",
                "/state/rig_id",
            ],
            "begin_program_preparation" => vec!["/local/configuration/rig_id", "/state/rig_id"],
            _ => vec!["/configuration/rig_id", "/state/rig_id"],
        };
        for path in paths {
            let dir = TempDir::new().unwrap();
            let mut storage = Storage::acquire(dir.path()).unwrap();
            let mut changed = value.clone();
            *changed.pointer_mut(path).unwrap() = json!("other-rig");
            assert!(matches!(
                storage.handle(serde_json::from_value(changed).unwrap(), "rig-1"),
                Err(ProtocolError::WrongRig)
            ));
            assert!(!dir.path().join("execution.sqlite").exists());
        }
    }
    let dir = TempDir::new().unwrap();
    let mut storage = Storage::acquire(dir.path()).unwrap();
    storage.handle(open(), "rig-1").unwrap();
    assert!(matches!(
        storage.handle(
            Operation::CaptureBinding {
                capture_id: "capture".into()
            },
            "other-rig"
        ),
        Err(ProtocolError::WrongRig)
    ));
}

#[test]
fn program_wire_rejects_unknown_duplicate_and_missing_nullable_fields() {
    let encoded = serde_json::to_string(&open()).unwrap();
    assert!(serde_json::from_str::<Operation>(
        &encoded.replace("\"gain\":40", "\"gain\":40,\"gain\":41")
    )
    .is_err());
    for path in [
        "/program/targets/0",
        "/program/recipes/0",
        "/program/configuration",
        "/program/configuration/gain",
        "/program/configuration/offset",
        "/program/configuration/filters/0",
        "/program/recipes/0/binning",
        "/program/bindings/0",
    ] {
        let mut value = serde_json::to_value(open()).unwrap();
        value
            .pointer_mut(path)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("unknown".into(), json!(true));
        assert!(
            serde_json::from_value::<Operation>(value).is_err(),
            "{path}"
        );
    }
    for (mut value, path, field) in [
        (
            serde_json::to_value(open()).unwrap(),
            "/program/recipes/0",
            "offset",
        ),
        (
            serde_json::to_value(open()).unwrap(),
            "/program/targets/0",
            "position_angle_mas",
        ),
        (
            serde_json::to_value(begin()).unwrap(),
            "/local",
            "previous_pointing",
        ),
    ] {
        value
            .pointer_mut(path)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert!(serde_json::from_value::<Operation>(value).is_err());
    }
    assert!(
        serde_json::from_value::<StorageReply>(json!({"status":"capture_binding_found"})).is_err()
    );
    let mut value = serde_json::to_value(begin()).unwrap();
    value["local"]["previous_pointing"]["unknown"] = json!(true);
    assert!(serde_json::from_value::<Operation>(value).is_err());
}

#[tokio::test]
async fn ipc_four_cannot_open_program_storage() {
    let dir = TempDir::new().unwrap();
    let (mut client, server) = duplex(MAX_FRAME_BYTES);
    let task = tokio::spawn(serve_with_storage(
        server,
        Some(Storage::acquire(dir.path()).unwrap()),
    ));
    let mut hello = command(
        0,
        Command::Hello {
            runtime_version: "0.3.0".into(),
            engine_version: ENGINE_VERSION.into(),
            contract_version: CONTRACT_VERSION,
            rig_id: "rig-1".into(),
        },
    );
    hello.protocol_version = 4;
    send(&mut client, &hello).await;
    assert_eq!(task.await.unwrap(), Err(ProtocolError::VersionMismatch));
    assert!(!dir.path().join("execution.sqlite").exists());
}

#[test]
fn final_bound_reservation_rechecks_safety_after_readiness() {
    let dir = TempDir::new().unwrap();
    let mut storage = Storage::acquire(dir.path()).unwrap();
    storage.handle(open(), "rig-1").unwrap();
    storage.handle(begin(), "rig-1").unwrap();
    for _ in 0..2 {
        let StorageReply::PreparationAdvanced {
            next: Next::Run(command),
        } = storage.handle(advance(), "rig-1").unwrap()
        else {
            panic!()
        };
        storage
            .handle(
                Operation::CompletePreparation {
                    completion: Completion {
                        preparation_id: command.preparation_id,
                        ordinal: command.ordinal,
                        ended_at_ms: fixture().state.now_ms,
                        elapsed_ms: 1,
                        outcome: Outcome::Succeeded,
                    },
                },
                "rig-1",
            )
            .unwrap();
    }
    assert!(matches!(
        storage.handle(advance(), "rig-1").unwrap(),
        StorageReply::PreparationAdvanced {
            next: Next::ReadyToReserve { .. }
        }
    ));
    let mut unsafe_reservation = serde_json::to_value(reserve()).unwrap();
    unsafe_reservation["state"]["safety"] = json!("unsafe");
    assert!(matches!(
        storage
            .handle(serde_json::from_value(unsafe_reservation).unwrap(), "rig-1")
            .unwrap(),
        StorageReply::Reserved {
            outcome: Reservation::Decision(Decision::Stop { .. })
        }
    ));
    assert!(matches!(
        storage
            .handle(
                Operation::CaptureBinding {
                    capture_id: "capture".into()
                },
                "rig-1"
            )
            .unwrap(),
        StorageReply::CaptureBindingFound { binding: None }
    ));
    assert!(
        matches!(storage.handle(Operation::Events { after: 0, limit: 64 }, "rig-1").unwrap(), StorageReply::Events { events, next_cursor: 0 } if events.is_empty())
    );
}

#[tokio::test]
async fn bound_storage_cannot_use_stateless_progress() {
    let dir = TempDir::new().unwrap();
    let (mut client, server) = duplex(MAX_FRAME_BYTES);
    let task = tokio::spawn(serve_with_storage(
        server,
        Some(Storage::acquire(dir.path()).unwrap()),
    ));
    handshake(&mut client).await;
    exchange(&mut client, 1, open()).await;
    send(
        &mut client,
        &command(
            2,
            Command::Evaluate {
                request: serde_json::value::to_raw_value(&fixture()).unwrap(),
            },
        ),
    )
    .await;
    assert_eq!(task.await.unwrap(), Err(ProtocolError::UntrackedEvaluation));
}

#[test]
fn largest_configuration_binding_fits_one_reply_frame() {
    use psf_guard_director_core::program::{Binning, Control, Filter};
    let mut program = program();
    let long = "\"".repeat(128);
    program.configuration.camera_id = long.clone();
    program.configuration.filter_wheel_id = Some(long.clone());
    program.configuration.filters = (0..256)
        .map(|i| Filter {
            id: format!("{}{:03}", "\"".repeat(125), i),
            position: Some(i),
        })
        .collect();
    program.configuration.binning_modes = (1..=256).map(|i| Binning { x: i, y: i }).collect();
    program.configuration.readout_modes = (0..256).collect();
    program.configuration.gain = Control::Values {
        values: (0..256).map(|i| i32::MAX - i).collect(),
    };
    program.configuration.offset = program.configuration.gain.clone();
    program.targets[0].id = long.clone();
    program.targets[0].name = "\"".repeat(256);
    program.recipes[0].id = long.clone();
    program.recipes[0].filter_id = program.configuration.filters[0].id.clone();
    program.recipes[0].gain = Some(i32::MAX);
    program.recipes[0].offset = Some(i32::MAX);
    program.bindings[0].target_id = long.clone();
    program.bindings[0].recipe_id = long.clone();
    psf_guard_director_core::program::BoundProgram::new(program.clone(), &fixture().state).unwrap();
    let dir = TempDir::new().unwrap();
    let ledger = psf_guard_director_ledger::Ledger::open_program(
        &dir.path().join("execution.sqlite"),
        program.clone(),
        fixture().state,
    )
    .unwrap();
    let binding = psf_guard_director_ledger::program::CaptureBinding {
        ledger: ledger.info(),
        attempt: psf_guard_director_ledger::Attempt {
            capture_id: long.clone(),
            goal_id: "short-ha".into(),
            reserved_at_ms: u64::MAX,
            evidence: Evidence::Saved {
                image_id: long,
                elapsed_ms: u64::MAX,
            },
        },
        target: program.targets.remove(0),
        recipe: program.recipes.remove(0),
        configuration: program.configuration,
    };
    let reply = Reply {
        protocol_version: PROTOCOL_VERSION,
        session_id: "0123456789abcdef0123456789abcdef".into(),
        request_id: u64::MAX,
        payload: ResultMessage::Ledger {
            response: StorageReply::CaptureBindingFound {
                binding: Some(Box::new(binding)),
            },
        },
    };
    assert!(serde_json::to_vec(&reply).unwrap().len() < MAX_FRAME_BYTES);
}
