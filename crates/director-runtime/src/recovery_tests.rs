use super::{recovery::*, storage_tests, tests::*, *};
use psf_guard_director_core::{recovery as core, Safety, State};
use psf_guard_director_ledger::recovery as ledger;
use serde_json::{json, value::to_raw_value};
use tempfile::TempDir;
use tokio::io::{duplex, DuplexStream};

fn open() -> Operation {
    Operation::Open {
        identity: core::Identity {
            rig_id: "rig-1".into(),
            configuration_id: "config-1".into(),
            night_id: "night-1".into(),
            starts_at_ms: 1000,
            ends_at_ms: 100000,
        },
        policy: core::Policy {
            revision: 1,
            quality_mode: core::QualityMode::Pause,
            bad_samples: 2,
            good_probes: 2,
            cooldown_ms: 100,
            maximum_hold_ms: 1000,
            maximum_probes: 3,
            operation_timeout_ms: 50,
            evidence_max_age_ms: 100,
            latest_resume_ms: 99000,
            maximum_consecutive_failures: 2,
            maximum_total_failures: 3,
            park_on_stop: true,
            weather: None,
        },
        now_ms: 10000,
    }
}
fn apply(revision: u64, now: u64, event: core::Event) -> Operation {
    Operation::Apply {
        request: ledger::Request {
            night_id: "night-1".into(),
            configuration_id: "config-1".into(),
            event_id: format!("event-{revision}"),
            expected_revision: revision,
            now_ms: now,
            conditions: core::Conditions {
                safety: Safety::Safe,
                motion: core::Motion::Permitted,
            },
            event,
        },
    }
}
fn poor(now: u64) -> core::Event {
    core::Event::Quality {
        sample: core::QualitySample {
            rig_id: "rig-1".into(),
            configuration_id: "config-1".into(),
            capture_id: format!("capture-{now}"),
            observed_at_ms: now,
            context: core::QualityContext {
                target_id: "target".into(),
                filter_id: "L".into(),
                exposure_ms: 1000,
                bin_x: 1,
                bin_y: 1,
                reference_id: "reference".into(),
                source: "pixels".into(),
                algorithm_revision: "1".into(),
            },
            verdict: core::Verdict::CorroboratedPoor,
        },
    }
}
async fn exchange(client: &mut DuplexStream, id: u64, operation: Operation) -> recovery::Reply {
    send(
        client,
        &command(
            id,
            Command::Recovery {
                operation: to_raw_value(&recovery::Request {
                    recovery_version: recovery::CONTRACT_VERSION,
                    operation,
                })
                .unwrap(),
            },
        ),
    )
    .await;
    let reply = receive_reply(client).await;
    assert_eq!(reply.request_id, id);
    let ResultMessage::Recovery { response } = reply.payload else {
        panic!("expected recovery")
    };
    response
}
async fn start(
    dir: &TempDir,
) -> (
    DuplexStream,
    tokio::task::JoinHandle<Result<(), ProtocolError>>,
) {
    let (mut client, server) = duplex(MAX_FRAME_BYTES);
    let storage = recovery::Storage::acquire(dir.path()).unwrap();
    let task = tokio::spawn(serve_with_recovery(server, None, Some(storage)));
    handshake(&mut client).await;
    (client, task)
}

#[tokio::test]
async fn quality_classification_is_scoped_read_only_and_not_a_dispatch_permit() {
    use psf_guard_director_core::quality as q;
    let dir = TempDir::new().unwrap();
    let (mut client, task) = start(&dir).await;
    let reference_frame = q::Frame {
        capture_id: "reference-frame".into(),
        observed_at_ms: 1000,
        context: q::Context {
            rig_id: "rig-1".into(),
            configuration_id: "config-1".into(),
            target_id: "target".into(),
            recipe_fingerprint: "recipe".into(),
            analysis_fingerprint: "nina-fast".into(),
            width: 100,
            height: 100,
        },
        metrics: q::Metrics {
            stars: Some(100),
            hfr_pixels: Some(2.0),
            background_adu: Some(1000.0),
            eccentricity: Some(0.4),
        },
    };
    let reference = q::Reference {
        id: "reference".into(),
        approved: true,
        initial_group: vec![],
        frame: reference_frame.clone(),
    };
    let mut frame = q::Frame {
        capture_id: "new-frame".into(),
        observed_at_ms: 2000,
        ..reference_frame
    };
    frame.metrics.stars = Some(30);
    frame.metrics.background_adu = Some(2000.0);
    let reply = exchange(
        &mut client,
        1,
        Operation::ClassifyQuality {
            policy: q::Policy::default(),
            reference: Box::new(reference.clone()),
            frame: Box::new(frame.clone()),
            now_ms: 2000,
        },
    )
    .await;
    assert!(matches!(
        reply,
        recovery::Reply::QualityClassified {
            assessment: q::Assessment {
                verdict: core::Verdict::CorroboratedPoor,
                ..
            }
        }
    ));
    assert!(!dir.path().join("recovery.sqlite").exists());
    let group: Vec<_> = (0..5)
        .map(|i| q::Frame {
            capture_id: format!("initial-{i}"),
            observed_at_ms: 1000 + i * 100,
            ..reference.frame.clone()
        })
        .collect();
    let built = exchange(
        &mut client,
        2,
        Operation::BuildQualityReference {
            policy: q::Policy::default(),
            id: "initial-reference".into(),
            frames: group.clone(),
        },
    )
    .await;
    let recovery::Reply::QualityReferenceBuilt { reference: initial } = built else {
        panic!("expected initial reference")
    };
    assert!(!initial.approved);
    assert_eq!(initial.initial_group, group);
    let result = exchange(
        &mut client,
        3,
        Operation::ClassifyQuality {
            policy: q::Policy::default(),
            reference: initial,
            frame: Box::new(frame.clone()),
            now_ms: 2000,
        },
    )
    .await;
    assert!(matches!(
        result,
        recovery::Reply::QualityClassified {
            assessment: q::Assessment {
                verdict: core::Verdict::CorroboratedPoor,
                reference_quality_unknown: true,
                ..
            }
        }
    ));
    assert!(!dir.path().join("recovery.sqlite").exists());
    frame.context.rig_id = "another-rig".into();
    let reply = exchange(
        &mut client,
        4,
        Operation::ClassifyQuality {
            policy: q::Policy::default(),
            reference: Box::new(reference),
            frame: Box::new(frame),
            now_ms: 2000,
        },
    )
    .await;
    assert!(matches!(
        reply,
        recovery::Reply::Error {
            code: recovery::Error::WrongScope
        }
    ));
    assert!(!dir.path().join("recovery.sqlite").exists());
    drop(client);
    task.await.unwrap().unwrap();
}

#[tokio::test]
async fn restart_review_cannot_mutate_the_journal_or_clear_a_stop() {
    use core::readmission as r;
    let dir = TempDir::new().unwrap();
    let (mut client, task) = start(&dir).await;
    exchange(&mut client, 1, open()).await;
    let input = r::Review {
        rig_id: "rig-1".into(),
        configuration_id: "config-1".into(),
        night_id: "night-1".into(),
        now_ms: 10001,
        operator_requested: true,
        boundary: r::Boundary::Settled,
        camera_idle: true,
        mount_stopped: true,
        guider_stopped: true,
        safety: Safety::Safe,
        motion: core::Motion::Permitted,
    };
    let reply = exchange(
        &mut client,
        2,
        Operation::ReviewRestart {
            input: input.clone(),
        },
    )
    .await;
    assert!(
        matches!(reply, recovery::Reply::RestartReviewed { advice: r::Advice::RequestFreshAuthority, record } if record.revision == 0)
    );
    exchange(&mut client, 3, apply(0, 10001, core::Event::StopNight {})).await;
    let reply = exchange(&mut client, 4, Operation::ReviewRestart { input }).await;
    assert!(
        matches!(reply, recovery::Reply::RestartReviewed { advice: r::Advice::TerminalStop, record } if record.revision == 1)
    );
    drop(client);
    task.await.unwrap().unwrap();
}

#[tokio::test]
async fn weather_resume_ipc_cannot_bypass_missing_execution_evidence() {
    let dir = TempDir::new().unwrap();
    let (mut client, task) = start(&dir).await;
    let mut begin = open();
    let Operation::Open { policy, .. } = &mut begin else {
        unreachable!()
    };
    policy.weather = Some(core::WeatherPolicy {
        stable_safe_ms: 100,
        maximum_hold_ms: 1000,
        maximum_interruptions: 2,
    });
    exchange(&mut client, 1, begin).await;
    exchange(
        &mut client,
        2,
        apply(
            0,
            10001,
            core::Event::WeatherInterrupted { enclosure: true },
        ),
    )
    .await;
    exchange(&mut client, 3, apply(1, 10002, core::Event::Tick {})).await;
    exchange(&mut client, 4, apply(2, 10102, core::Event::Tick {})).await;
    assert!(matches!(
        exchange(
            &mut client,
            5,
            apply(3, 10102, core::Event::ResumeWeather {})
        )
        .await,
        recovery::Reply::Error {
            code: recovery::Error::AcquisitionBlocked
        }
    ));
    let recovery::Reply::Current {
        record: Some(record),
    } = exchange(&mut client, 6, Operation::Current {}).await
    else {
        panic!("missing hold")
    };
    assert!(matches!(
        record.snapshot.phase,
        core::Phase::WeatherHolding { .. }
    ));
    storage_tests::stop(&mut client, 7).await;
    assert_eq!(task.await.unwrap(), Ok(()));
}

#[tokio::test]
async fn disabled_and_wrong_version_do_not_open_database() {
    let (mut client, server) = duplex(MAX_FRAME_BYTES);
    let task = tokio::spawn(serve(server));
    handshake(&mut client).await;
    assert!(matches!(
        exchange(&mut client, 1, open()).await,
        recovery::Reply::Error {
            code: recovery::Error::Disabled
        }
    ));
    storage_tests::stop(&mut client, 2).await;
    assert_eq!(task.await.unwrap(), Ok(()));
    let dir = TempDir::new().unwrap();
    let (mut client, task) = start(&dir).await;
    send(
        &mut client,
        &command(
            1,
            Command::Recovery {
                operation: to_raw_value(&recovery::Request {
                    recovery_version: recovery::CONTRACT_VERSION + 1,
                    operation: open(),
                })
                .unwrap(),
            },
        ),
    )
    .await;
    assert!(matches!(
        receive_reply(&mut client).await.payload,
        ResultMessage::Recovery {
            response: recovery::Reply::Error {
                code: recovery::Error::UnsupportedVersion
            }
        }
    ));
    assert!(!dir.path().join("recovery.sqlite").exists());
    storage_tests::stop(&mut client, 2).await;
    assert_eq!(task.await.unwrap(), Ok(()));
}

#[tokio::test]
async fn cross_rig_input_is_rejected_before_storage_and_nested_scope_is_checked() {
    let dir = TempDir::new().unwrap();
    let (mut client, task) = start(&dir).await;
    let mut wrong = open();
    let Operation::Open { identity, .. } = &mut wrong else {
        unreachable!()
    };
    identity.rig_id = "other".into();
    assert!(matches!(
        exchange(&mut client, 1, wrong).await,
        recovery::Reply::Error {
            code: recovery::Error::WrongScope
        }
    ));
    assert!(!dir.path().join("recovery.sqlite").exists());
    let mut wrong = apply(0, 10001, poor(10001));
    let Operation::Apply { request } = &mut wrong else {
        unreachable!()
    };
    let core::Event::Quality { sample } = &mut request.event else {
        unreachable!()
    };
    sample.rig_id = "other".into();
    assert!(matches!(
        exchange(&mut client, 2, wrong).await,
        recovery::Reply::Error {
            code: recovery::Error::WrongScope
        }
    ));
    assert!(!dir.path().join("recovery.sqlite").exists());
    storage_tests::stop(&mut client, 3).await;
    assert_eq!(task.await.unwrap(), Ok(()));
}

#[tokio::test]
async fn issued_probe_is_exact_and_lost_reply_never_reissues_after_restart() {
    let dir = TempDir::new().unwrap();
    let (mut client, task) = start(&dir).await;
    assert!(matches!(
        exchange(&mut client, 1, open()).await,
        recovery::Reply::Opened { created: true, .. }
    ));
    for (id, revision, now) in [(2, 0, 10001), (3, 1, 10002)] {
        assert!(matches!(
            exchange(&mut client, id, apply(revision, now, poor(now))).await,
            recovery::Reply::Applied {
                newly_applied: true,
                issued: None,
                ..
            }
        ));
    }
    let probe = || {
        apply(
            2,
            10102,
            core::Event::BeginRecovery {
                attempt_id: "probe-1".into(),
            },
        )
    };
    let recovery::Reply::Applied {
        issued: Some(Issued::Probe {
            attempt_id,
            deadline_ms,
        }),
        record,
        ..
    } = exchange(&mut client, 4, probe()).await
    else {
        panic!("probe must be issued once")
    };
    assert_eq!(attempt_id, "probe-1");
    assert_eq!(deadline_ms, 10152);
    assert_eq!(record.snapshot.probes_spent, 1);
    drop(client);
    assert_eq!(task.await.unwrap(), Ok(()));
    let (mut client, task) = start(&dir).await;
    assert!(matches!(
        exchange(&mut client, 1, probe()).await,
        recovery::Reply::Applied {
            newly_applied: false,
            issued: None,
            ..
        }
    ));
    let recovery::Reply::Current {
        record: Some(record),
    } = exchange(&mut client, 2, Operation::Current {}).await
    else {
        panic!()
    };
    assert_eq!(record.snapshot.probes_spent, 1);
    assert!(matches!(
        exchange(
            &mut client,
            3,
            apply(
                3,
                10103,
                core::Event::BeginRecovery {
                    attempt_id: "new-id".into()
                }
            )
        )
        .await,
        recovery::Reply::Error {
            code: recovery::Error::WrongPhase
        }
    ));
    let recovery::Reply::Applied {
        issued: None,
        record,
        ..
    } = exchange(&mut client, 4, apply(3, 10152, core::Event::Tick {})).await
    else {
        panic!()
    };
    assert!(matches!(
        record.snapshot.phase,
        core::Phase::Stopping { .. }
    ));
    assert!(matches!(
        exchange(
            &mut client,
            5,
            apply(
                4,
                10153,
                core::Event::BeginPark {
                    attempt_id: "park".into()
                }
            )
        )
        .await,
        recovery::Reply::Applied {
            issued: Some(Issued::Park { .. }),
            ..
        }
    ));
    assert!(matches!(
        exchange(
            &mut client,
            6,
            apply(
                4,
                10153,
                core::Event::BeginPark {
                    attempt_id: "park".into()
                }
            )
        )
        .await,
        recovery::Reply::Applied {
            newly_applied: false,
            issued: None,
            ..
        }
    ));
    storage_tests::stop(&mut client, 7).await;
    assert_eq!(task.await.unwrap(), Ok(()));
}

#[tokio::test]
async fn unsafe_preemption_never_turns_committed_input_into_a_probe() {
    let dir = TempDir::new().unwrap();
    let (mut client, task) = start(&dir).await;
    exchange(&mut client, 1, open()).await;
    exchange(&mut client, 2, apply(0, 10001, poor(10001))).await;
    exchange(&mut client, 3, apply(1, 10002, poor(10002))).await;
    let mut probe = apply(
        2,
        10102,
        core::Event::BeginRecovery {
            attempt_id: "probe".into(),
        },
    );
    let Operation::Apply { request } = &mut probe else {
        unreachable!()
    };
    request.conditions.safety = Safety::Unsafe;
    request.conditions.motion = core::Motion::Unknown;
    let recovery::Reply::Applied {
        newly_applied: true,
        issued: None,
        record,
    } = exchange(&mut client, 4, probe).await
    else {
        panic!()
    };
    assert!(matches!(record.snapshot.phase, core::Phase::Stopped { .. }));
    assert_eq!(record.snapshot.probes_spent, 0);
    storage_tests::stop(&mut client, 5).await;
    assert_eq!(task.await.unwrap(), Ok(()));
}

#[tokio::test]
async fn recovery_latch_blocks_acquisition_but_not_readback_or_completion() {
    let dir = TempDir::new().unwrap();
    let execution = TempDir::new().unwrap();
    let (mut client, server) = duplex(MAX_FRAME_BYTES);
    let task = tokio::spawn(serve_with_recovery(
        server,
        Some(storage::Storage::acquire(execution.path()).unwrap()),
        Some(recovery::Storage::acquire(dir.path()).unwrap()),
    ));
    handshake(&mut client).await;
    assert!(matches!(
        storage_tests::exchange(
            &mut client,
            1,
            storage::Operation::Open {
                request: storage_tests::fixture()
            }
        )
        .await,
        storage::StorageReply::Opened { .. }
    ));
    let reserve = || storage::Operation::Reserve {
        capture_id: "capture".into(),
        state: storage_tests::fixture().state,
    };
    assert!(matches!(
        storage_tests::exchange(&mut client, 2, reserve()).await,
        storage::StorageReply::RecoveryBlocked {
            code: recovery::Error::NotAdmitted
        }
    ));
    exchange(&mut client, 3, open()).await;
    exchange(&mut client, 4, apply(0, 10000, core::Event::Tick {})).await;
    assert!(matches!(
        storage_tests::exchange(&mut client, 5, reserve()).await,
        storage::StorageReply::Reserved { .. }
    ));
    exchange(&mut client, 6, apply(1, 10000, core::Event::StopNight {})).await;
    assert!(matches!(
        storage_tests::exchange(&mut client, 7, reserve()).await,
        storage::StorageReply::RecoveryBlocked {
            code: recovery::Error::AcquisitionBlocked
        }
    ));
    assert!(matches!(
        storage_tests::exchange(
            &mut client,
            8,
            storage::Operation::Attempt {
                capture_id: "capture".into()
            }
        )
        .await,
        storage::StorageReply::Found { attempt: Some(_) }
    ));
    assert!(matches!(
        storage_tests::exchange(
            &mut client,
            9,
            storage::Operation::Record {
                capture_id: "capture".into(),
                evidence: psf_guard_director_ledger::Evidence::Uncertain {
                    reason: "interrupted".into()
                }
            }
        )
        .await,
        storage::StorageReply::Recorded { .. }
    ));
    storage_tests::stop(&mut client, 10).await;
    assert_eq!(task.await.unwrap(), Ok(()));
    // A fresh allocation directory cannot bypass the independent per-rig latch.
    let successor = TempDir::new().unwrap();
    let (mut client, server) = duplex(MAX_FRAME_BYTES);
    let task = tokio::spawn(serve_with_recovery(
        server,
        Some(storage::Storage::acquire(successor.path()).unwrap()),
        Some(recovery::Storage::acquire(dir.path()).unwrap()),
    ));
    handshake(&mut client).await;
    storage_tests::exchange(
        &mut client,
        1,
        storage::Operation::Open {
            request: storage_tests::fixture(),
        },
    )
    .await;
    assert!(matches!(
        storage_tests::exchange(&mut client, 2, reserve()).await,
        storage::StorageReply::RecoveryBlocked {
            code: recovery::Error::AcquisitionBlocked
        }
    ));
    storage_tests::stop(&mut client, 3).await;
    assert_eq!(task.await.unwrap(), Ok(()));
}

#[tokio::test]
async fn recovery_refuses_stateless_evaluation_bypass() {
    let dir = TempDir::new().unwrap();
    let (mut client, task) = start(&dir).await;
    send(
        &mut client,
        &command(
            1,
            Command::Evaluate {
                request: to_raw_value(&storage_tests::fixture()).unwrap(),
            },
        ),
    )
    .await;
    assert_eq!(task.await.unwrap(), Err(ProtocolError::UntrackedEvaluation));
}

#[test]
fn recovery_contract_rejects_paths_unknown_fields_and_missing_versions() {
    for value in [
        json!({"operation":{"action":"current"}}),
        json!({"recovery_version":1,"operation":{"action":"current","path":"elsewhere"}}),
        json!({"recovery_version":1,"operation":{"action":"apply","request":{}}}),
    ] {
        assert!(serde_json::from_value::<recovery::Request>(value).is_err());
    }
    assert!(serde_json::from_value::<Command>(
        json!({"type":"recovery","operation":{},"path":"elsewhere"})
    )
    .is_err());
    assert!(serde_json::from_value::<recovery::Reply>(
        json!({"status":"applied","newly_applied":false,"record":null})
    )
    .is_err());
}

#[test]
fn separate_runtime_owners_cannot_share_the_recovery_directory() {
    let dir = TempDir::new().unwrap();
    let first = recovery::Storage::acquire(dir.path()).unwrap();
    assert!(matches!(
        recovery::Storage::acquire(dir.path()),
        Err(recovery::Error::Busy)
    ));
    drop(first);
    assert!(recovery::Storage::acquire(dir.path()).is_ok());
    assert!(matches!(
        recovery::Storage::acquire(std::path::Path::new("relative")),
        Err(recovery::Error::InvalidDirectory)
    ));
}

#[tokio::test]
async fn journal_pages_are_bounded_and_preserve_cursor() {
    let dir = TempDir::new().unwrap();
    let (mut client, task) = start(&dir).await;
    exchange(&mut client, 1, open()).await;
    exchange(&mut client, 2, apply(0, 10001, poor(10001))).await;
    exchange(&mut client, 3, apply(1, 10002, poor(10002))).await;
    let recovery::Reply::Events {
        events,
        next_cursor,
    } = exchange(
        &mut client,
        4,
        Operation::Events {
            night_id: "night-1".into(),
            after: 0,
            limit: 1,
        },
    )
    .await
    else {
        panic!()
    };
    assert_eq!(events.len(), 1);
    assert_eq!(next_cursor, 1);
    let recovery::Reply::Events {
        events,
        next_cursor,
    } = exchange(
        &mut client,
        5,
        Operation::Events {
            night_id: "night-1".into(),
            after: next_cursor,
            limit: 1,
        },
    )
    .await
    else {
        panic!()
    };
    assert_eq!(events.len(), 1);
    assert_eq!(next_cursor, 2);
    assert!(matches!(
        exchange(
            &mut client,
            6,
            Operation::Events {
                night_id: "night-1".into(),
                after: 0,
                limit: 17
            }
        )
        .await,
        recovery::Reply::Error {
            code: recovery::Error::InvalidInput
        }
    ));
    storage_tests::stop(&mut client, 7).await;
    assert_eq!(task.await.unwrap(), Ok(()));
}

#[tokio::test]
async fn acquisition_needs_fresh_motion_evidence_and_cannot_outlive_the_night() {
    let dir = TempDir::new().unwrap();
    let mut storage = Some(recovery::Storage::acquire(dir.path()).unwrap());
    let mut wrong_rig = storage_tests::fixture().state;
    wrong_rig.rig_id = "other".into();
    let mut wrong = storage::Operation::Reserve {
        capture_id: "wrong".into(),
        state: wrong_rig,
    };
    assert_eq!(
        recovery::gate(&mut storage, &mut wrong, "rig-1")
            .await
            .unwrap(),
        Err(recovery::Error::WrongScope)
    );
    assert!(!dir.path().join("recovery.sqlite").exists());
    let req = |operation| recovery::Request {
        recovery_version: recovery::CONTRACT_VERSION,
        operation,
    };
    recovery::execute(&mut storage, req(open()), "rig-1")
        .await
        .unwrap();
    let operation = |state| storage::Operation::Reserve {
        capture_id: "capture".into(),
        state,
    };
    let original = storage_tests::fixture().state;
    assert_eq!(
        recovery::gate(&mut storage, &mut operation(original.clone()), "rig-1")
            .await
            .unwrap(),
        Err(recovery::Error::AcquisitionBlocked)
    );
    recovery::execute(
        &mut storage,
        req(apply(0, 10000, core::Event::Tick {})),
        "rig-1",
    )
    .await
    .unwrap();
    let mut valid = operation(original.clone());
    assert_eq!(
        recovery::gate(&mut storage, &mut valid, "rig-1")
            .await
            .unwrap(),
        Ok(())
    );
    assert_eq!(
        valid
            .acquisition_state_mut()
            .unwrap()
            .conditions_valid_until_ms,
        100000
    );
    assert_eq!(
        valid
            .acquisition_state_mut()
            .unwrap()
            .completion_deadline_ms,
        Some(100000)
    );
    let mut earlier = operation(State {
        completion_deadline_ms: Some(50000),
        ..original.clone()
    });
    assert_eq!(
        recovery::gate(&mut storage, &mut earlier, "rig-1")
            .await
            .unwrap(),
        Ok(())
    );
    assert_eq!(
        earlier
            .acquisition_state_mut()
            .unwrap()
            .completion_deadline_ms,
        Some(50000)
    );
    let mut request = storage_tests::fixture();
    request.assignment.goals.truncate(1);
    request.state = valid.acquisition_state_mut().unwrap().clone();
    request.assignment.goals[0].exposure_ms = 90001;
    request.assignment.goals[0].overhead_ms = 0;
    request.assignment.goals[0].eligible_windows[0].end_ms = 200000;
    assert!(!matches!(
        psf_guard_director_core::evaluate(&request).unwrap(),
        psf_guard_director_core::Decision::Acquire { .. }
    ));
    for (state, code) in [
        (
            State {
                configuration_id: "other".into(),
                ..original.clone()
            },
            recovery::Error::WrongScope,
        ),
        (
            State {
                now_ms: 9999,
                ..original.clone()
            },
            recovery::Error::ClockReversed,
        ),
        (
            State {
                now_ms: 10101,
                ..original.clone()
            },
            recovery::Error::AcquisitionBlocked,
        ),
    ] {
        assert_eq!(
            recovery::gate(&mut storage, &mut operation(state), "rig-1")
                .await
                .unwrap(),
            Err(code)
        );
    }
    let mut unknown = apply(1, 10000, core::Event::Tick {});
    let Operation::Apply { request } = &mut unknown else {
        unreachable!()
    };
    request.conditions.motion = core::Motion::Unknown;
    recovery::execute(&mut storage, req(unknown), "rig-1")
        .await
        .unwrap();
    assert_eq!(
        recovery::gate(&mut storage, &mut operation(original), "rig-1")
            .await
            .unwrap(),
        Err(recovery::Error::AcquisitionBlocked)
    );
}

#[tokio::test]
async fn large_persisted_evidence_pages_fit_frames_without_losing_events() {
    let dir = TempDir::new().unwrap();
    let mut store =
        ledger::SessionStore::open(&dir.path().join("recovery.sqlite"), "rig-1").unwrap();
    let Operation::Open {
        identity,
        policy,
        now_ms,
    } = open()
    else {
        unreachable!()
    };
    store.begin_night(identity, policy, now_ms).unwrap();
    let Operation::Apply { mut request } = apply(0, 10000, core::Event::StopNight {}) else {
        unreachable!()
    };
    request.conditions.motion = core::Motion::Prohibited;
    store.apply(&request).unwrap();
    // The store can retain unexecuted terminal input up to its bounded record
    // limit. Such evidence must not make the IPC journal unreadable.
    for revision in 1..6 {
        let Operation::Apply { request } = apply(
            revision,
            10000,
            core::Event::Failure {
                failure: core::Failure {
                    attempt_id: format!("failure-{revision}"),
                    operation: core::Operation::Guide,
                    device_id: "\"".repeat(60000),
                    target_id: "target".into(),
                    uncertain: true,
                },
            },
        ) else {
            unreachable!()
        };
        store.apply(&request).unwrap();
    }
    drop(store);
    let (mut client, task) = start(&dir).await;
    let mut after = 0;
    let mut seen = Vec::new();
    let mut id = 1;
    while after < 6 {
        let recovery::Reply::Events {
            events,
            next_cursor,
        } = exchange(
            &mut client,
            id,
            Operation::Events {
                night_id: "night-1".into(),
                after,
                limit: 16,
            },
        )
        .await
        else {
            panic!()
        };
        assert!(next_cursor > after);
        seen.extend(events.iter().map(|event| event.revision));
        after = next_cursor;
        id += 1;
    }
    assert_eq!(seen, vec![1, 2, 3, 4, 5, 6]);
    assert!(id > 2);
    storage_tests::stop(&mut client, id).await;
    assert_eq!(task.await.unwrap(), Ok(()));
}
