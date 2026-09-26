use super::{storage::*, storage_tests::*, tests::*, *};
use psf_guard_director_core::preparation::{
    Command as NativeCommand, Completion, Context, Estimates, Next, Outcome,
};
use psf_guard_director_core::{Decision, Safety};
use psf_guard_director_ledger::preparation::{Event, EventKind, Lifecycle};
use psf_guard_director_ledger::{Evidence, Reservation};
use serde_json::json;
use tempfile::TempDir;
use tokio::io::duplex;

fn context() -> Context {
    Context {
        goal_id: fixture().assignment.goals[0].id.clone(),
        target_id: "target-1".into(),
        recipe_id: "recipe-1".into(),
        previous_target_id: Some("target-1".into()),
        filter_id: "ha".into(),
        readout_mode: 1,
        mount_parked: false,
        rotator_connected: false,
        enable_slew_center: true,
        dither_every: 0,
        dither_override: None,
        filter_exposures_since_dither: 0,
    }
}

fn begin() -> Operation {
    Operation::BeginPreparation {
        preparation_id: "prep-1".into(),
        context: context(),
        estimates: Estimates::default(),
        state: fixture().state,
    }
}

fn advance() -> Operation {
    Operation::AdvancePreparation {
        preparation_id: "prep-1".into(),
        state: fixture().state,
    }
}

fn complete(command: &NativeCommand) -> Operation {
    Operation::CompletePreparation {
        completion: Completion {
            preparation_id: command.preparation_id.clone(),
            ordinal: command.ordinal,
            ended_at_ms: fixture().state.now_ms,
            elapsed_ms: u64::MAX,
            outcome: Outcome::Succeeded,
        },
    }
}

#[tokio::test]
async fn preparation_round_trip_selects_records_and_reserves_with_durable_progress() {
    let dir = TempDir::new().unwrap();
    let (mut client, server) = duplex(MAX_FRAME_BYTES);
    let task = tokio::spawn(serve_with_storage(
        server,
        Some(Storage::acquire(dir.path()).unwrap()),
    ));
    handshake(&mut client).await;
    exchange(&mut client, 1, Operation::Open { request: fixture() }).await;
    assert!(matches!(
        exchange(
            &mut client,
            2,
            Operation::Evaluate {
                state: fixture().state
            }
        )
        .await,
        StorageReply::Evaluated {
            decision: Decision::Acquire { .. }
        }
    ));
    assert!(matches!(
        exchange(&mut client, 3, Operation::UnresolvedAttempt {}).await,
        StorageReply::Found { attempt: None }
    ));
    assert!(matches!(
        exchange(&mut client, 4, Operation::ActivePreparation {}).await,
        StorageReply::PreparationFound { record: None }
    ));
    assert!(matches!(
        exchange(&mut client, 5, begin()).await,
        StorageReply::PreparationStarted { created: true, .. }
    ));
    assert!(matches!(
        exchange(&mut client, 6, begin()).await,
        StorageReply::PreparationStarted { created: false, .. }
    ));
    assert!(
        matches!(exchange(&mut client, 7, Operation::Evaluate { state: fixture().state }).await,
        StorageReply::Evaluated { decision: Decision::CheckIn { reason } } if reason == "preparation_active")
    );
    let mut id = 8;
    for ordinal in 1..=2 {
        let StorageReply::PreparationAdvanced {
            next: Next::Run(command),
        } = exchange(&mut client, id, advance()).await
        else {
            panic!("native command expected");
        };
        assert_eq!(command.ordinal, ordinal);
        assert!(matches!(exchange(&mut client, id + 1, advance()).await,
            StorageReply::PreparationAdvanced { next: Next::InFlight { ordinal: actual } } if actual == ordinal));
        exchange(&mut client, id + 2, complete(&command)).await;
        let StorageReply::PreparationRecorded { record } =
            exchange(&mut client, id + 3, complete(&command)).await
        else {
            panic!("receipt expected");
        };
        assert_eq!(record.observations.len(), ordinal as usize);
        assert_eq!(
            record.observations.last().unwrap().completion.elapsed_ms,
            u64::MAX
        );
        id += 4;
    }
    assert!(matches!(
        exchange(&mut client, id, advance()).await,
        StorageReply::PreparationAdvanced {
            next: Next::ReadyToReserve { .. }
        }
    ));
    assert!(matches!(
        exchange(
            &mut client,
            id + 1,
            Operation::ReservePrepared {
                preparation_id: "prep-1".into(),
                capture_id: "capture-1".into(),
                state: fixture().state,
            }
        )
        .await,
        StorageReply::Reserved {
            outcome: Reservation::Created(_)
        }
    ));
    assert!(matches!(
        exchange(&mut client, id + 2, Operation::ActivePreparation {}).await,
        StorageReply::PreparationFound { record: None }
    ));
    assert!(
        matches!(exchange(&mut client, id + 3, Operation::UnresolvedAttempt {}).await,
        StorageReply::Found { attempt: Some(attempt) } if attempt.capture_id == "capture-1")
    );
    assert!(
        matches!(exchange(&mut client, id + 4, Operation::Evaluate { state: fixture().state }).await,
        StorageReply::Evaluated { decision: Decision::CheckIn { reason } } if reason == "capture_recovery_required")
    );
    let StorageReply::PreparationEvents {
        events,
        next_cursor,
    } = exchange(
        &mut client,
        id + 5,
        Operation::PreparationEvents {
            after: 0,
            limit: MAX_PREPARATION_EVENT_PAGE,
        },
    )
    .await
    else {
        panic!("events expected");
    };
    assert_eq!(events.len(), 6);
    assert_eq!(next_cursor, 6);
    assert_eq!(events[0].assignment_revision, 9_007_199_254_740_993);
    assert!(
        matches!(exchange(&mut client, id + 6, Operation::Events { after: 0, limit: 64 }).await,
        StorageReply::Events { events, next_cursor: 1 } if events.len() == 1)
    );
    exchange(
        &mut client,
        id + 7,
        Operation::Record {
            capture_id: "capture-1".into(),
            evidence: Evidence::Saved {
                image_id: "image-1".into(),
                elapsed_ms: 1,
            },
        },
    )
    .await;
    assert!(
        matches!(exchange(&mut client, id + 8, Operation::Evaluate { state: fixture().state }).await,
        StorageReply::Evaluated { decision: Decision::Wait { reason } } if reason == "pending_assessment")
    );
    assert!(matches!(
        exchange(&mut client, id + 9, Operation::UnresolvedAttempt {}).await,
        StorageReply::Found { attempt: None }
    ));
    stop(&mut client, id + 10).await;
    assert_eq!(task.await.unwrap(), Ok(()));
}

#[tokio::test]
async fn lost_reply_can_discover_pending_preparation_after_new_session() {
    let dir = TempDir::new().unwrap();
    let (mut client, server) = duplex(MAX_FRAME_BYTES);
    let task = tokio::spawn(serve_with_storage(
        server,
        Some(Storage::acquire(dir.path()).unwrap()),
    ));
    handshake(&mut client).await;
    exchange(&mut client, 1, Operation::Open { request: fixture() }).await;
    exchange(&mut client, 2, begin()).await;
    send(&mut client, &ledger_command(3, advance())).await;
    // Observe committed storage, not a sleep or a consumed command reply.
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let db = rusqlite::Connection::open(dir.path().join("execution.sqlite")).unwrap();
            let count: i64 = db
                .query_row("SELECT count(*) FROM preparation_event", [], |r| r.get(0))
                .unwrap();
            if count == 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    drop(client);
    assert!(matches!(
        task.await.unwrap(),
        Ok(()) | Err(ProtocolError::Transport)
    ));
    let (mut client, server) = duplex(MAX_FRAME_BYTES);
    let task = tokio::spawn(serve_with_storage(
        server,
        Some(Storage::acquire(dir.path()).unwrap()),
    ));
    handshake(&mut client).await;
    exchange(&mut client, 1, Operation::Open { request: fixture() }).await;
    let StorageReply::PreparationFound {
        record: Some(record),
    } = exchange(&mut client, 2, Operation::ActivePreparation {}).await
    else {
        panic!("lost preparation should be discoverable");
    };
    assert_eq!(record.preparation_id, "prep-1");
    assert_eq!(record.pending.unwrap().ordinal, 1);
    assert!(matches!(
        exchange(&mut client, 3, advance()).await,
        StorageReply::PreparationAdvanced {
            next: Next::InFlight { ordinal: 1 }
        }
    ));
    assert!(matches!(
        exchange(
            &mut client,
            4,
            Operation::ClosePreparation {
                preparation_id: "prep-1".into()
            }
        )
        .await,
        StorageReply::Error {
            code: StorageError::ConflictingEvidence
        }
    ));
    stop(&mut client, 5).await;
    assert_eq!(task.await.unwrap(), Ok(()));
}

#[test]
fn new_operations_keep_rig_scope_and_local_storage_ownership() {
    let dir = TempDir::new().unwrap();
    let mut storage = Storage::acquire(dir.path()).unwrap();
    storage
        .handle(Operation::Open { request: fixture() }, "rig-1")
        .unwrap();
    for operation in [
        Operation::ActivePreparation {},
        Operation::UnresolvedAttempt {},
    ] {
        assert_eq!(
            storage.handle(operation, "foreign-rig").unwrap_err(),
            ProtocolError::WrongRig
        );
    }
    let mut wrong = fixture().state;
    wrong.rig_id = "foreign-rig".into();
    for operation in [
        Operation::Evaluate {
            state: wrong.clone(),
        },
        Operation::BeginPreparation {
            preparation_id: "prep-1".into(),
            context: context(),
            estimates: Estimates::default(),
            state: wrong.clone(),
        },
        Operation::AdvancePreparation {
            preparation_id: "prep-1".into(),
            state: wrong.clone(),
        },
        Operation::ReservePrepared {
            preparation_id: "prep-1".into(),
            capture_id: "capture-1".into(),
            state: wrong,
        },
    ] {
        assert_eq!(
            storage.handle(operation, "rig-1").unwrap_err(),
            ProtocolError::WrongRig
        );
    }
    assert!(matches!(
        storage
            .handle(Operation::ActivePreparation {}, "rig-1")
            .unwrap(),
        StorageReply::PreparationFound { record: None }
    ));
}

#[test]
fn scoped_preparation_errors_and_page_limits_do_not_destroy_session_state() {
    let dir = TempDir::new().unwrap();
    let mut storage = Storage::acquire(dir.path()).unwrap();
    storage
        .handle(Operation::Open { request: fixture() }, "rig-1")
        .unwrap();
    let mut not_selected = begin();
    if let Operation::BeginPreparation { state, .. } = &mut not_selected {
        state.safety = Safety::Unknown;
    }
    assert!(matches!(
        storage.handle(not_selected, "rig-1").unwrap(),
        StorageReply::Error {
            code: StorageError::PreparationNotSelected
        }
    ));
    storage.handle(begin(), "rig-1").unwrap();
    let StorageReply::PreparationAdvanced {
        next: Next::Run(command),
    } = storage.handle(advance(), "rig-1").unwrap()
    else {
        panic!("command");
    };
    let mut invalid = complete(&command);
    if let Operation::CompletePreparation { completion } = &mut invalid {
        completion.ordinal = 2;
    }
    assert!(matches!(
        storage.handle(invalid, "rig-1").unwrap(),
        StorageReply::Error {
            code: StorageError::InvalidCompletion
        }
    ));
    let mut past = advance();
    if let Operation::AdvancePreparation { state, .. } = &mut past {
        state.now_ms -= 1;
    }
    assert!(matches!(
        storage.handle(past, "rig-1").unwrap(),
        StorageReply::Error {
            code: StorageError::ClockRegression
        }
    ));
    for limit in [0, MAX_PREPARATION_EVENT_PAGE + 1] {
        assert!(matches!(
            storage
                .handle(Operation::PreparationEvents { after: 0, limit }, "rig-1")
                .unwrap(),
            StorageReply::Error {
                code: StorageError::InvalidInput
            }
        ));
    }
    storage.handle(complete(&command), "rig-1").unwrap();
    let mut conflict = complete(&command);
    if let Operation::CompletePreparation { completion } = &mut conflict {
        completion.elapsed_ms -= 1;
    }
    assert!(matches!(
        storage.handle(conflict, "rig-1").unwrap(),
        StorageReply::Error {
            code: StorageError::ConflictingEvidence
        }
    ));
    assert!(
        matches!(storage.handle(Operation::ClosePreparation { preparation_id: "prep-1".into() }, "rig-1").unwrap(),
        StorageReply::PreparationClosed { record } if record.lifecycle == Lifecycle::Closed)
    );
    assert!(matches!(
        storage
            .handle(Operation::ActivePreparation {}, "rig-1")
            .unwrap(),
        StorageReply::PreparationFound { record: None }
    ));
}

#[test]
fn preparation_commands_reject_missing_nullable_fields_unknown_fields_and_duplicate_ids() {
    for field in ["previous_target_id", "dither_override"] {
        let mut value = serde_json::to_value(begin()).unwrap();
        value["context"].as_object_mut().unwrap().remove(field);
        assert!(serde_json::from_value::<Operation>(value).is_err());
    }
    let mut value = serde_json::to_value(begin()).unwrap();
    value["context"]["allow_unsafe"] = json!(true);
    assert!(serde_json::from_value::<Operation>(value).is_err());
    for raw in [
        r#"{"action":"active_preparation","path":"somewhere"}"#,
        r#"{"action":"unresolved_attempt","extra":true}"#,
        r#"{"action":"preparation","preparation_id":"first","preparation_id":"second"}"#,
    ] {
        assert!(serde_json::from_str::<Operation>(raw).is_err());
    }
    for raw in [r#"{"status":"preparation_found"}"#, r#"{"status":"found"}"#] {
        assert!(serde_json::from_str::<StorageReply>(raw).is_err());
    }
}

#[test]
fn read_only_planning_preserves_journal_and_safety_overrides_recovery_waits() {
    let dir = TempDir::new().unwrap();
    let mut storage = Storage::acquire(dir.path()).unwrap();
    assert!(matches!(
        storage
            .handle(Operation::ActivePreparation {}, "rig-1")
            .unwrap(),
        StorageReply::Error {
            code: StorageError::NotOpen
        }
    ));
    storage
        .handle(Operation::Open { request: fixture() }, "rig-1")
        .unwrap();
    storage.handle(begin(), "rig-1").unwrap();
    let db = rusqlite::Connection::open(dir.path().join("execution.sqlite")).unwrap();
    let before: Vec<u8> = db
        .query_row("SELECT checkpoint FROM preparation", [], |r| r.get(0))
        .unwrap();
    let mut state = fixture().state;
    state.now_ms += 100;
    state.safety = Safety::Unsafe;
    assert!(matches!(
        storage
            .handle(Operation::Evaluate { state }, "rig-1")
            .unwrap(),
        StorageReply::Evaluated {
            decision: Decision::Stop { .. }
        }
    ));
    storage
        .handle(Operation::ActivePreparation {}, "rig-1")
        .unwrap();
    let after: Vec<u8> = db
        .query_row("SELECT checkpoint FROM preparation", [], |r| r.get(0))
        .unwrap();
    assert_eq!(before, after);
    assert_eq!(
        db.query_row("SELECT count(*) FROM preparation_event", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        1
    );
    assert_eq!(
        db.query_row("SELECT count(*) FROM event", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn ipc_three_cannot_open_the_new_runtime_ledger() {
    let dir = TempDir::new().unwrap();
    let (mut client, server) = duplex(MAX_FRAME_BYTES);
    let task = tokio::spawn(serve_with_storage(
        server,
        Some(Storage::acquire(dir.path()).unwrap()),
    ));
    let mut hello = command(
        0,
        Command::Hello {
            runtime_version: "0.2.1".into(),
            engine_version: ENGINE_VERSION.into(),
            contract_version: CONTRACT_VERSION,
            rig_id: "rig-1".into(),
        },
    );
    hello.protocol_version = 3;
    send(&mut client, &hello).await;
    assert_eq!(task.await.unwrap(), Err(ProtocolError::VersionMismatch));
    assert!(!dir.path().join("execution.sqlite").exists());
}

#[test]
fn maximum_preparation_page_fits_the_unchanged_frame_limit() {
    let long = "\"".repeat(128);
    let mut context = context();
    context.goal_id = long.clone();
    context.target_id = long.clone();
    context.recipe_id = long.clone();
    context.previous_target_id = Some(long.clone());
    context.filter_id = long.clone();
    let event = Event {
        schema_version: 1,
        ledger_id: long.clone(),
        sequence: i64::MAX as u64,
        contract_version: CONTRACT_VERSION,
        engine_version: ENGINE_VERSION.into(),
        assignment_id: long.clone(),
        assignment_revision: u64::MAX,
        rig_id: long.clone(),
        configuration_id: long.clone(),
        preparation_id: long.clone(),
        event: EventKind::Started {
            context,
            estimates: Estimates {
                unpark_ms: u64::MAX,
                center_ms: u64::MAX,
                before_target_ms: u64::MAX,
                dither_ms: u64::MAX,
                filter_ms: u64::MAX,
                readout_ms: u64::MAX,
                capture_overhead_ms: u64::MAX,
            },
        },
    };
    let completed = EventKind::Completed {
        observation: psf_guard_director_core::preparation::Observation {
            command: NativeCommand {
                preparation_id: long.clone(),
                ordinal: u32::MAX,
                goal_id: long.clone(),
                target_id: long.clone(),
                recipe_id: long.clone(),
                operation: psf_guard_director_core::preparation::Operation::SwitchFilter {
                    filter_id: long.clone(),
                },
            },
            issued_at_ms: u64::MAX,
            completion: Completion {
                preparation_id: long.clone(),
                ordinal: u32::MAX,
                ended_at_ms: u64::MAX,
                elapsed_ms: u64::MAX,
                outcome: Outcome::Uncertain { reason: long },
            },
        },
    };
    for kind in [event.event.clone(), completed] {
        let mut event = event.clone();
        event.event = kind;
        let reply = Reply {
            protocol_version: PROTOCOL_VERSION,
            session_id: "f".repeat(32),
            request_id: u64::MAX,
            payload: ResultMessage::Ledger {
                response: StorageReply::PreparationEvents {
                    events: vec![event; MAX_PREPARATION_EVENT_PAGE],
                    next_cursor: u64::MAX,
                },
            },
        };
        assert!(serde_json::to_vec(&reply).unwrap().len() <= MAX_FRAME_BYTES);
    }
}
