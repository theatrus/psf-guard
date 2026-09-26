use super::*;
use psf_guard_director_core::preparation::Command as NativeCommand;

fn pending(f: &Fixture, command: NativeCommand) -> Operation {
    Operation::CheckGeometryPendingDispatch {
        command: Box::new(command),
        configuration: Box::new(f.program().configuration),
        constraints: Box::new(f.constraints.clone()),
        state: f.state.clone(),
    }
}

fn capture(f: &Fixture) -> Operation {
    Operation::CheckGeometryCaptureDispatch {
        preparation_id: "prep".into(),
        capture_id: "capture".into(),
        configuration: Box::new(f.program().configuration),
        constraints: Box::new(f.constraints.clone()),
        state: f.state.clone(),
    }
}

fn issue(f: &Fixture, storage: &mut Storage) -> NativeCommand {
    assert!(matches!(
        storage.handle(f.open(), "rig-1").unwrap(),
        StorageReply::GeometryOpened { .. }
    ));
    storage.handle(f.begin(), "rig-1").unwrap();
    let StorageReply::PreparationAdvanced {
        next: Next::Run(command),
    } = storage.handle(f.advance(), "rig-1").unwrap()
    else {
        panic!()
    };
    command
}

#[test]
fn strict_dispatch_commands_require_all_current_inputs() {
    let f = Fixture::new();
    let dir = TempDir::new().unwrap();
    let mut storage = Storage::acquire(dir.path()).unwrap();
    let command = issue(&f, &mut storage);
    for operation in [pending(&f, command), capture(&f)] {
        let value = serde_json::to_value(&operation).unwrap();
        for field in ["configuration", "constraints", "state"] {
            let mut missing = value.clone();
            missing.as_object_mut().unwrap().remove(field);
            assert!(serde_json::from_value::<Operation>(missing).is_err());
        }
        let mut extra = value.clone();
        extra["dispatch_permit"] = json!(true);
        assert!(serde_json::from_value::<Operation>(extra).is_err());
        let raw = serde_json::to_string(&value).unwrap().replace(
            "\"revision\":9007199254740993",
            "\"revision\":1,\"revision\":9007199254740993",
        );
        assert!(serde_json::from_str::<Operation>(&raw).is_err());
        let encoded = serde_json::to_vec(&operation).unwrap();
        assert!(encoded.len() < psf_guard_director_core::MAX_REQUEST_BYTES);
    }
}

#[test]
fn dispatch_rig_mismatch_is_rejected_before_database_creation() {
    let f = Fixture::new();
    let source = TempDir::new().unwrap();
    let command = issue(&f, &mut Storage::acquire(source.path()).unwrap());
    for operation in [pending(&f, command), capture(&f)] {
        for field in ["configuration", "constraints", "state"] {
            let mut value = serde_json::to_value(&operation).unwrap();
            if field == "constraints" {
                value[field]["rig"]["rig_id"] = json!("other");
            } else {
                value[field]["rig_id"] = json!("other");
            }
            let dir = TempDir::new().unwrap();
            let mut storage = Storage::acquire(dir.path()).unwrap();
            assert!(matches!(
                storage.handle(serde_json::from_value(value).unwrap(), "rig-1"),
                Err(ProtocolError::WrongRig)
            ));
            assert!(!dir.path().join("execution.sqlite").exists());
        }
    }
}

#[test]
fn stale_command_and_capture_link_are_scoped_errors_not_authority() {
    let f = Fixture::new();
    let dir = TempDir::new().unwrap();
    let mut storage = Storage::acquire(dir.path()).unwrap();
    let command = issue(&f, &mut storage);
    let mut wrong = command.clone();
    wrong.ordinal += 1;
    assert!(matches!(
        storage.handle(pending(&f, wrong), "rig-1").unwrap(),
        StorageReply::Error { .. }
    ));
    assert!(matches!(
        storage.handle(capture(&f), "rig-1").unwrap(),
        StorageReply::Error { .. }
    ));
    assert!(matches!(
        storage
            .handle(pending(&f, command.clone()), "rig-1")
            .unwrap(),
        StorageReply::DispatchChecked {
            decision: Decision::Acquire { .. }
        }
    ));
    assert!(matches!(
        storage.handle(f.advance(), "rig-1").unwrap(),
        StorageReply::PreparationAdvanced {
            next: Next::InFlight { .. }
        }
    ));
    storage
        .handle(f.complete(command.ordinal), "rig-1")
        .unwrap();
    f.ready(&mut storage);
    storage.handle(f.reserve(), "rig-1").unwrap();
    assert!(matches!(
        storage.handle(pending(&f, command), "rig-1").unwrap(),
        StorageReply::Error { .. }
    ));
    assert!(matches!(
        storage.handle(capture(&f), "rig-1").unwrap(),
        StorageReply::DispatchChecked {
            decision: Decision::Acquire { .. }
        }
    ));
    storage
        .handle(
            Operation::Record {
                capture_id: "capture".into(),
                evidence: psf_guard_director_ledger::Evidence::Uncertain {
                    reason: "lost_receipt".into(),
                },
            },
            "rig-1",
        )
        .unwrap();
    assert!(matches!(
        storage.handle(capture(&f), "rig-1").unwrap(),
        StorageReply::Error {
            code: StorageError::ConflictingEvidence
        }
    ));
}

#[test]
fn both_checks_reject_slow_hooks_using_geometry_not_host_window_claims() {
    for reserved in [false, true] {
        let mut f = Fixture::new();
        let az = observe(
            IcrsPosition {
                ra_degrees: 83.0,
                dec_degrees: -5.0,
            },
            f.constraints.rig.site,
            f.constraints.rig.orientation,
            f.state.now_ms + 21_777,
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
        let command = issue(&f, &mut storage);
        if reserved {
            storage
                .handle(f.complete(command.ordinal), "rig-1")
                .unwrap();
            f.ready(&mut storage);
            storage.handle(f.reserve(), "rig-1").unwrap();
        }
        f.state.now_ms += 18_000;
        let operation = if reserved {
            capture(&f)
        } else {
            pending(&f, command)
        };
        assert!(matches!(
            storage.handle(operation, "rig-1").unwrap(),
            StorageReply::DispatchChecked {
                decision: Decision::Wait { .. }
            }
        ));
    }
}

#[tokio::test]
async fn wire_dispatch_refusals_survive_reconnect_without_new_commands_or_captures() {
    for reserved in [false, true] {
        let f = Fixture::new();
        let dir = TempDir::new().unwrap();
        let mut storage = Storage::acquire(dir.path()).unwrap();
        let command = issue(&f, &mut storage);
        if reserved {
            storage
                .handle(f.complete(command.ordinal), "rig-1")
                .unwrap();
            f.ready(&mut storage);
            storage.handle(f.reserve(), "rig-1").unwrap();
        }
        let (mut client, server) = duplex(MAX_FRAME_BYTES);
        let task = tokio::spawn(serve_with_storage(server, Some(storage)));
        handshake(&mut client).await;
        let operation = if reserved {
            capture(&f)
        } else {
            pending(&f, command.clone())
        };
        assert!(matches!(
            exchange(&mut client, 1, operation).await,
            StorageReply::DispatchChecked {
                decision: Decision::Acquire { .. }
            }
        ));
        let mut changed = Fixture::new();
        changed.constraints.rig.site.longitude_degrees += 1.0;
        let operation = if reserved {
            capture(&changed)
        } else {
            pending(&changed, command.clone())
        };
        assert!(matches!(
            exchange(&mut client, 2, operation).await,
            StorageReply::DispatchChecked {
                decision: Decision::CheckIn { .. }
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
        let operation = if reserved {
            capture(&f)
        } else {
            pending(&f, command)
        };
        assert!(matches!(
            exchange(&mut client, 2, operation).await,
            StorageReply::DispatchChecked {
                decision: Decision::CheckIn { .. }
            }
        ));
        let evidence = if reserved { f.reserve() } else { f.advance() };
        assert!(matches!(
            exchange(&mut client, 3, evidence).await,
            StorageReply::Reserved {
                outcome: Reservation::Existing(_)
            } | StorageReply::PreparationAdvanced {
                next: Next::InFlight { .. }
            }
        ));
        changed.state.safety = Safety::Unsafe;
        let operation = if reserved {
            capture(&changed)
        } else {
            let StorageReply::PreparationFound {
                record: Some(record),
            } = exchange(
                &mut client,
                4,
                Operation::Preparation {
                    preparation_id: "prep".into(),
                },
            )
            .await
            else {
                panic!()
            };
            pending(&changed, record.pending.unwrap())
        };
        let id = if reserved { 4 } else { 5 };
        assert!(matches!(
            exchange(&mut client, id, operation).await,
            StorageReply::DispatchChecked {
                decision: Decision::Stop { .. }
            }
        ));
        drop(client);
        assert_eq!(task.await.unwrap(), Ok(()));
    }
}
