use super::{storage::*, tests::*, *};
use psf_guard_director_ledger::{Evidence, Reservation};
use serde_json::{json, value::to_raw_value};
use tempfile::TempDir;
use tokio::io::{duplex, DuplexStream};

fn fixture() -> Request {
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../director-core/tests/fixtures/decisions.json"
    ))
    .unwrap();
    let mut request: Request = serde_json::from_value(fixture["base"].clone()).unwrap();
    request.assignment.revision = 9_007_199_254_740_993;
    request.assignment.goals.truncate(1);
    request.assignment.goals[0].requested = 1;
    request.assignment.goals[0].accepted = 0;
    request.assignment.goals[0].pending = 0;
    request.assignment.goals[0].attempts_remaining = 2;
    request
}

fn ledger_command(id: u64, operation: Operation) -> Message {
    command(
        id,
        Command::Ledger {
            operation: to_raw_value(&operation).unwrap(),
        },
    )
}

async fn exchange(client: &mut DuplexStream, id: u64, operation: Operation) -> StorageReply {
    send(client, &ledger_command(id, operation)).await;
    let reply = receive_reply(client).await;
    assert_eq!(reply.request_id, id);
    let ResultMessage::Ledger { response } = reply.payload else {
        panic!("expected storage reply");
    };
    response
}

async fn stop(client: &mut DuplexStream, id: u64) {
    send(client, &command(id, Command::Shutdown)).await;
    assert!(matches!(
        receive_reply(client).await.payload,
        ResultMessage::Stopped
    ));
    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn storage_is_opt_in_and_open_is_required() {
    for enabled in [false, true] {
        let dir = TempDir::new().unwrap();
        let (mut client, server) = duplex(MAX_FRAME_BYTES);
        let storage = enabled.then(|| Storage::acquire(dir.path()).unwrap());
        let task = tokio::spawn(serve_with_storage(server, storage));
        handshake(&mut client).await;
        let result = exchange(
            &mut client,
            1,
            Operation::Attempt {
                capture_id: "capture-1".into(),
            },
        )
        .await;
        assert!(
            matches!(result, StorageReply::Error { code } if code == if enabled { StorageError::NotOpen } else { StorageError::Disabled })
        );
        assert!(!dir.path().join("execution.sqlite").exists());
        stop(&mut client, 2).await;
        assert_eq!(task.await.unwrap(), Ok(()));
    }
}

#[tokio::test]
async fn ledger_round_trip_preserves_exact_ids_pending_progress_and_events() {
    let dir = TempDir::new().unwrap();
    let (mut client, server) = duplex(MAX_FRAME_BYTES);
    let task = tokio::spawn(serve_with_storage(
        server,
        Some(Storage::acquire(dir.path()).unwrap()),
    ));
    handshake(&mut client).await;
    let StorageReply::Opened { info } =
        exchange(&mut client, 1, Operation::Open { request: fixture() }).await
    else {
        panic!("open");
    };
    assert_eq!(info.assignment_revision, 9_007_199_254_740_993);
    assert!(matches!(
        exchange(
            &mut client,
            2,
            Operation::Reserve {
                capture_id: "capture-1".into(),
                state: fixture().state
            }
        )
        .await,
        StorageReply::Reserved {
            outcome: Reservation::Created(_)
        }
    ));
    assert!(matches!(
        exchange(
            &mut client,
            3,
            Operation::Record {
                capture_id: "capture-1".into(),
                evidence: Evidence::Saved {
                    image_id: "image-1".into(),
                    elapsed_ms: 1234
                }
            }
        )
        .await,
        StorageReply::Recorded { .. }
    ));
    assert!(matches!(
        exchange(
            &mut client,
            4,
            Operation::Reserve {
                capture_id: "capture-2".into(),
                state: fixture().state
            }
        )
        .await,
        StorageReply::Reserved {
            outcome: Reservation::Decision(psf_guard_director_core::Decision::Wait { .. })
        }
    ));
    let StorageReply::Events {
        events,
        next_cursor,
    } = exchange(
        &mut client,
        5,
        Operation::Events {
            after: 0,
            limit: 64,
        },
    )
    .await
    else {
        panic!("events");
    };
    assert_eq!(next_cursor, 2);
    assert_eq!(events.len(), 2);
    assert!(events.iter().all(|event| event.ledger_id == info.ledger_id));
    let StorageReply::Events {
        events,
        next_cursor,
    } = exchange(&mut client, 6, Operation::Events { after: 2, limit: 1 }).await
    else {
        panic!("events");
    };
    assert!(events.is_empty());
    assert_eq!(next_cursor, 2);
    stop(&mut client, 7).await;
    assert_eq!(task.await.unwrap(), Ok(()));
}

#[tokio::test]
async fn lost_reply_keeps_reservation_and_restart_returns_evidence_not_new_work() {
    let dir = TempDir::new().unwrap();
    let (mut client, server) = duplex(MAX_FRAME_BYTES);
    let task = tokio::spawn(serve_with_storage(
        server,
        Some(Storage::acquire(dir.path()).unwrap()),
    ));
    handshake(&mut client).await;
    exchange(&mut client, 1, Operation::Open { request: fixture() }).await;
    send(
        &mut client,
        &ledger_command(
            2,
            Operation::Reserve {
                capture_id: "capture-1".into(),
                state: fixture().state,
            },
        ),
    )
    .await;
    // The complete request is buffered; the reply now has no reader.
    drop(client);
    assert_eq!(task.await.unwrap(), Err(ProtocolError::Transport));
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
            Operation::Reserve {
                capture_id: "capture-1".into(),
                state: fixture().state
            }
        )
        .await,
        StorageReply::Reserved {
            outcome: Reservation::Existing(_)
        }
    ));
    assert!(matches!(
        exchange(
            &mut client,
            3,
            Operation::Reserve {
                capture_id: "capture-2".into(),
                state: fixture().state
            }
        )
        .await,
        StorageReply::Reserved {
            outcome: Reservation::RecoveryRequired(_)
        }
    ));
    stop(&mut client, 4).await;
    assert_eq!(task.await.unwrap(), Ok(()));
}

#[test]
fn owner_lock_is_exclusive_and_released_without_deleting_lock_file() {
    let dir = TempDir::new().unwrap();
    let first = Storage::acquire(dir.path()).unwrap();
    assert!(matches!(
        Storage::acquire(dir.path()),
        Err(StorageError::Busy)
    ));
    drop(first);
    let second = Storage::acquire(dir.path()).unwrap();
    assert!(dir.path().join("director-runtime.lock").is_file());
    drop(second);
    assert!(matches!(
        Storage::acquire(std::path::Path::new("relative")),
        Err(StorageError::InvalidDirectory)
    ));
}

#[tokio::test(start_paused = true)]
async fn startup_waits_for_owner_release_without_stealing_lock() {
    let dir = TempDir::new().unwrap();
    let owner = Storage::acquire(dir.path()).unwrap();
    let started = tokio::time::Instant::now();
    let release = async {
        tokio::time::sleep(Duration::from_millis(100)).await;
        drop(owner);
    };
    let (acquired, ()) = tokio::join!(Storage::acquire_for_startup(dir.path()), release);
    let acquired = acquired.unwrap();
    assert!(started.elapsed() >= Duration::from_millis(100));
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(matches!(
        Storage::acquire(dir.path()),
        Err(StorageError::Busy)
    ));
    drop(acquired);
}

#[tokio::test(start_paused = true)]
async fn startup_contention_times_out_and_preserves_live_owner() {
    let dir = TempDir::new().unwrap();
    let owner = Storage::acquire(dir.path()).unwrap();
    let started = tokio::time::Instant::now();
    assert!(matches!(
        Storage::acquire_for_startup(dir.path()).await,
        Err(StorageError::Busy)
    ));
    assert_eq!(started.elapsed(), Duration::from_secs(2));
    assert!(matches!(
        Storage::acquire(dir.path()),
        Err(StorageError::Busy)
    ));
    drop(owner);
    drop(Storage::acquire(dir.path()).unwrap());
}

#[tokio::test(start_paused = true)]
async fn startup_does_not_retry_invalid_directory_or_non_contention_io() {
    let dir = TempDir::new().unwrap();
    std::fs::create_dir(dir.path().join("director-runtime.lock")).unwrap();
    let started = tokio::time::Instant::now();
    assert!(matches!(
        Storage::acquire_for_startup(std::path::Path::new("relative")).await,
        Err(StorageError::InvalidDirectory)
    ));
    assert!(matches!(
        Storage::acquire_for_startup(dir.path()).await,
        Err(StorageError::Unavailable)
    ));
    assert_eq!(started.elapsed(), Duration::ZERO);
}

#[tokio::test(start_paused = true)]
async fn cancelled_startup_does_not_acquire_ownership_later() {
    let dir = TempDir::new().unwrap();
    let owner = Storage::acquire(dir.path()).unwrap();
    assert!(tokio::time::timeout(
        Duration::from_millis(50),
        Storage::acquire_for_startup(dir.path())
    )
    .await
    .is_err());
    drop(owner);
    tokio::time::sleep(Duration::from_secs(3)).await;
    drop(Storage::acquire(dir.path()).unwrap());
}

#[cfg(windows)]
#[tokio::test(start_paused = true)]
async fn startup_retries_windows_sharing_violation_until_handle_closes() {
    use std::os::windows::fs::OpenOptionsExt;
    let dir = TempDir::new().unwrap();
    let handle = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .share_mode(0)
        .open(dir.path().join("director-runtime.lock"))
        .unwrap();
    assert!(matches!(
        Storage::acquire(dir.path()),
        Err(StorageError::Busy)
    ));
    let release = async {
        tokio::time::sleep(Duration::from_millis(100)).await;
        drop(handle);
    };
    let (acquired, ()) = tokio::join!(Storage::acquire_for_startup(dir.path()), release);
    drop(acquired.unwrap());
}

#[test]
fn storage_operations_preserve_strict_nested_decoding() {
    let good = serde_json::to_string(&Operation::Open { request: fixture() }).unwrap();
    for bad in [
        good.replace("\"revision\":9007199254740993", "\"revision\":1,\"revision\":9007199254740993"),
        good.replace("\"action\":\"open\"", "\"action\":\"open\",\"action\":\"events\""),
        good.replace("\"contract_version\":2", "\"contract_version\":2,\"extra\":true"),
        r#"{"action":"events","after":0,"limit":1,"path":"elsewhere"}"#.into(),
        r#"{"action":"record","capture_id":"x","evidence":{"state":"saved","image_id":"a","image_id":"b","elapsed_ms":1}}"#.into(),
    ] {
        assert!(serde_json::from_str::<Operation>(&bad).is_err(), "{bad}");
    }
    assert!(
        serde_json::from_str::<Command>(r#"{"type":"ledger","operation":{},"operation":{}}"#)
            .is_err()
    );
}

#[tokio::test]
async fn wrong_rig_never_opens_or_mutates_ledger() {
    for action in ["open", "reserve"] {
        let dir = TempDir::new().unwrap();
        let (mut client, server) = duplex(MAX_FRAME_BYTES);
        let task = tokio::spawn(serve_with_storage(
            server,
            Some(Storage::acquire(dir.path()).unwrap()),
        ));
        handshake(&mut client).await;
        let mut r = fixture();
        let operation = if action == "open" {
            r.assignment.rig_id = "other-rig".into();
            Operation::Open { request: r }
        } else {
            r.state.rig_id = "other-rig".into();
            Operation::Reserve {
                capture_id: "capture-1".into(),
                state: r.state,
            }
        };
        send(&mut client, &ledger_command(1, operation)).await;
        assert_eq!(task.await.unwrap(), Err(ProtocolError::WrongRig));
        assert!(!dir.path().join("execution.sqlite").exists());
    }
}

#[tokio::test]
async fn rejected_operation_does_not_poison_session_or_replace_active_ledger() {
    let dir = TempDir::new().unwrap();
    let (mut client, server) = duplex(MAX_FRAME_BYTES);
    let task = tokio::spawn(serve_with_storage(
        server,
        Some(Storage::acquire(dir.path()).unwrap()),
    ));
    handshake(&mut client).await;
    let mut bad = fixture();
    bad.contract_version = 1;
    assert!(matches!(
        exchange(&mut client, 1, Operation::Open { request: bad }).await,
        StorageReply::Error {
            code: StorageError::InvalidSnapshot
        }
    ));
    exchange(&mut client, 2, Operation::Open { request: fixture() }).await;
    let mut other = fixture();
    other.assignment.revision += 1;
    assert!(matches!(
        exchange(&mut client, 3, Operation::Open { request: other }).await,
        StorageReply::Error {
            code: StorageError::AlreadyOpen
        }
    ));
    assert!(matches!(
        exchange(
            &mut client,
            4,
            Operation::Events {
                after: 0,
                limit: 65
            }
        )
        .await,
        StorageReply::Error {
            code: StorageError::InvalidInput
        }
    ));
    assert!(matches!(
        exchange(
            &mut client,
            5,
            Operation::Attempt {
                capture_id: "missing".into()
            }
        )
        .await,
        StorageReply::Found { attempt: None }
    ));
    stop(&mut client, 6).await;
    assert_eq!(task.await.unwrap(), Ok(()));
}

#[tokio::test]
async fn malformed_storage_operation_closes_session_without_file_creation() {
    let dir = TempDir::new().unwrap();
    let (mut client, server) = duplex(MAX_FRAME_BYTES);
    let task = tokio::spawn(serve_with_storage(
        server,
        Some(Storage::acquire(dir.path()).unwrap()),
    ));
    handshake(&mut client).await;
    send(
        &mut client,
        &command(
            1,
            Command::Ledger {
                operation: to_raw_value(&json!({"action":"open","request":{}})).unwrap(),
            },
        ),
    )
    .await;
    assert_eq!(task.await.unwrap(), Err(ProtocolError::InvalidMessage));
    assert!(!dir.path().join("execution.sqlite").exists());
}

#[tokio::test]
async fn valid_but_oversized_storage_operation_is_bounded_before_decoding() {
    let (mut client, server) = duplex(MAX_FRAME_BYTES);
    let task = tokio::spawn(serve(server));
    handshake(&mut client).await;
    let raw = format!(
        "{{\"action\":\"events\",\"after\":0,\"limit\":1{}}}",
        " ".repeat(psf_guard_director_core::MAX_REQUEST_BYTES)
    );
    send(
        &mut client,
        &command(
            1,
            Command::Ledger {
                operation: serde_json::value::RawValue::from_string(raw).unwrap(),
            },
        ),
    )
    .await;
    assert!(matches!(
        receive_reply(&mut client).await.payload,
        ResultMessage::Ledger {
            response: StorageReply::Error {
                code: StorageError::InvalidInput
            }
        }
    ));
    stop(&mut client, 2).await;
    assert_eq!(task.await.unwrap(), Ok(()));
}

#[tokio::test]
async fn stateless_evaluation_cannot_bypass_ledger_progress() {
    let dir = TempDir::new().unwrap();
    let (mut client, server) = duplex(MAX_FRAME_BYTES);
    let task = tokio::spawn(serve_with_storage(
        server,
        Some(Storage::acquire(dir.path()).unwrap()),
    ));
    handshake(&mut client).await;
    exchange(&mut client, 1, Operation::Open { request: fixture() }).await;
    send(
        &mut client,
        &command(
            2,
            Command::Evaluate {
                request: to_raw_value(&fixture()).unwrap(),
            },
        ),
    )
    .await;
    assert_eq!(task.await.unwrap(), Err(ProtocolError::UntrackedEvaluation));
}

#[tokio::test]
async fn version_one_handshake_is_rejected_before_opening_a_ledger() {
    let dir = TempDir::new().unwrap();
    let (mut client, server) = duplex(MAX_FRAME_BYTES);
    let task = tokio::spawn(serve_with_storage(
        server,
        Some(Storage::acquire(dir.path()).unwrap()),
    ));
    let mut old = command(
        0,
        Command::Hello {
            runtime_version: "0.1.0".into(),
            engine_version: ENGINE_VERSION.into(),
            contract_version: CONTRACT_VERSION,
            rig_id: "rig-1".into(),
        },
    );
    old.protocol_version = 1;
    send(&mut client, &old).await;
    assert_eq!(task.await.unwrap(), Err(ProtocolError::VersionMismatch));
    assert!(!dir.path().join("execution.sqlite").exists());
}

#[test]
fn full_event_page_with_maximum_escaped_identifiers_fits_one_frame() {
    let dir = TempDir::new().unwrap();
    let mut storage = Storage::acquire(dir.path()).unwrap();
    let long = "\\".repeat(128);
    let mut r = fixture();
    r.assignment.id = long.clone();
    r.assignment.rig_id = long.clone();
    r.assignment.configuration_id = long.clone();
    r.assignment.goals[0].id = long.clone();
    r.assignment.goals[0].attempts_remaining = 32;
    r.state.rig_id = long.clone();
    r.state.configuration_id = long.clone();
    let state = r.state.clone();
    assert!(matches!(
        storage
            .handle(Operation::Open { request: r }, &long)
            .unwrap(),
        StorageReply::Opened { .. }
    ));
    for index in 0..32 {
        let capture_id = format!("{index:02}{}", "\"".repeat(126));
        assert!(matches!(
            storage
                .handle(
                    Operation::Reserve {
                        capture_id: capture_id.clone(),
                        state: state.clone()
                    },
                    &long
                )
                .unwrap(),
            StorageReply::Reserved {
                outcome: Reservation::Created(_)
            }
        ));
        assert!(matches!(
            storage
                .handle(
                    Operation::Record {
                        capture_id,
                        evidence: Evidence::Failed {
                            reason: long.clone()
                        }
                    },
                    &long
                )
                .unwrap(),
            StorageReply::Recorded { .. }
        ));
    }
    let response = storage
        .handle(
            Operation::Events {
                after: 0,
                limit: 64,
            },
            &long,
        )
        .unwrap();
    assert!(matches!(&response, StorageReply::Events { events, .. } if events.len() == 64));
    let reply = Reply {
        protocol_version: PROTOCOL_VERSION,
        session_id: "f".repeat(32),
        request_id: u64::MAX,
        payload: ResultMessage::Ledger { response },
    };
    assert!(serde_json::to_vec(&reply).unwrap().len() <= MAX_FRAME_BYTES);
}

#[tokio::test]
async fn busy_database_returns_scoped_error_and_retry_cannot_duplicate_work() {
    let dir = TempDir::new().unwrap();
    let (mut client, server) = duplex(MAX_FRAME_BYTES);
    let task = tokio::spawn(serve_with_storage(
        server,
        Some(Storage::acquire(dir.path()).unwrap()),
    ));
    handshake(&mut client).await;
    exchange(&mut client, 1, Operation::Open { request: fixture() }).await;
    let connection = rusqlite::Connection::open(dir.path().join("execution.sqlite")).unwrap();
    connection.execute_batch("BEGIN IMMEDIATE").unwrap();
    send(
        &mut client,
        &ledger_command(
            2,
            Operation::Reserve {
                capture_id: "capture-1".into(),
                state: fixture().state,
            },
        ),
    )
    .await;
    // This deliberately exhausts SQLite's two-second busy timeout. Allow for
    // runner scheduling delays; this test asserts the scoped error and retry
    // identity, not wall-clock latency. Production deadlines remain unchanged.
    let frame = read_frame(&mut client, Duration::from_secs(30))
        .await
        .unwrap()
        .unwrap();
    let reply: Reply = serde_json::from_slice(&frame).unwrap();
    assert!(matches!(
        reply.payload,
        ResultMessage::Ledger {
            response: StorageReply::Error {
                code: StorageError::Busy
            }
        }
    ));
    connection.execute_batch("ROLLBACK").unwrap();
    assert!(matches!(
        exchange(
            &mut client,
            3,
            Operation::Reserve {
                capture_id: "capture-1".into(),
                state: fixture().state
            }
        )
        .await,
        StorageReply::Reserved {
            outcome: Reservation::Created(_)
        }
    ));
    assert!(matches!(
        exchange(
            &mut client,
            4,
            Operation::Reserve {
                capture_id: "capture-1".into(),
                state: fixture().state
            }
        )
        .await,
        StorageReply::Reserved {
            outcome: Reservation::Existing(_)
        }
    ));
    stop(&mut client, 5).await;
    assert_eq!(task.await.unwrap(), Ok(()));
}
