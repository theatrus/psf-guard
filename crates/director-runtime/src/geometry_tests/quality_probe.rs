use super::*;
use crate::recovery as wire;
use psf_guard_director_core::recovery as r;

#[tokio::test]
async fn quality_probe_uses_durable_recovery_and_never_reserves_science() {
    let dir = TempDir::new().unwrap();
    let rd = TempDir::new().unwrap();
    let mut storage = Some(Storage::acquire(dir.path()).unwrap());
    let mut recovery = Some(wire::Storage::acquire(rd.path()).unwrap());
    let mut f = Fixture::new();
    let start = f.state.now_ms;
    let program = f.program();
    let recipe = program.recipes[0].clone();
    let goal = program.bindings[0].goal_id.clone();
    assert!(matches!(
        execute(&mut storage, f.open(), "rig-1").await.unwrap(),
        StorageReply::GeometryOpened { .. }
    ));
    let request = |operation| wire::Request {
        recovery_version: wire::CONTRACT_VERSION,
        operation,
    };
    let policy = r::Policy {
        revision: 1,
        quality_mode: r::QualityMode::Pause,
        bad_samples: 1,
        good_probes: 2,
        cooldown_ms: 10,
        maximum_hold_ms: 50_000,
        maximum_probes: 3,
        operation_timeout_ms: 40_000,
        evidence_max_age_ms: 1000,
        latest_resume_ms: start + 59_000,
        maximum_consecutive_failures: 2,
        maximum_total_failures: 3,
        park_on_stop: true,
        weather: None,
    };
    let opened = wire::execute(
        &mut recovery,
        request(wire::Operation::Open {
            identity: r::Identity {
                rig_id: "rig-1".into(),
                configuration_id: program.configuration.id.clone(),
                night_id: "night".into(),
                starts_at_ms: start,
                ends_at_ms: start + 60_000,
            },
            policy,
            now_ms: start,
        }),
        "rig-1",
    )
    .await
    .unwrap();
    assert!(matches!(opened, wire::Reply::Opened { .. }));
    let apply = |revision, now, event| {
        request(wire::Operation::Apply {
            request: psf_guard_director_ledger::recovery::Request {
                night_id: "night".into(),
                configuration_id: program.configuration.id.clone(),
                event_id: format!("event-{revision}"),
                expected_revision: revision,
                now_ms: now,
                conditions: r::Conditions {
                    safety: Safety::Safe,
                    motion: r::Motion::Permitted,
                },
                event,
            },
        })
    };
    let probe = |state: State| Operation::CheckQualityProbe {
        goal_id: goal.clone(),
        attempt_id: "probe".into(),
        recipe: Box::new(recipe.clone()),
        constraints: Box::new(f.constraints.clone()),
        state,
        recovery: None,
    };
    assert!(
        wire::gate(&mut recovery, &mut probe(f.state.clone()), "rig-1")
            .await
            .unwrap()
            .is_err()
    );
    let sample = r::QualitySample {
        rig_id: "rig-1".into(),
        configuration_id: program.configuration.id.clone(),
        capture_id: "poor".into(),
        observed_at_ms: start + 1,
        context: r::QualityContext {
            target_id: program.bindings[0].target_id.clone(),
            filter_id: recipe.filter_id.clone(),
            exposure_ms: recipe.exposure_ms,
            bin_x: recipe.binning.x as u16,
            bin_y: recipe.binning.y as u16,
            reference_id: "reference".into(),
            source: "pixels".into(),
            algorithm_revision: "1".into(),
        },
        verdict: r::Verdict::CorroboratedPoor,
    };
    assert!(matches!(
        wire::execute(
            &mut recovery,
            apply(0, start + 1, r::Event::Quality { sample }),
            "rig-1"
        )
        .await
        .unwrap(),
        wire::Reply::Applied { .. }
    ));
    assert!(matches!(
        wire::execute(
            &mut recovery,
            apply(
                1,
                start + 11,
                r::Event::BeginRecovery {
                    attempt_id: "probe".into()
                }
            ),
            "rig-1"
        )
        .await
        .unwrap(),
        wire::Reply::Applied {
            issued: Some(_),
            ..
        }
    ));
    f.state.now_ms = start + 11;
    let mut check = probe(f.state.clone());
    assert_eq!(
        wire::gate(&mut recovery, &mut check, "rig-1")
            .await
            .unwrap(),
        Ok(())
    );
    assert!(matches!(
        execute(&mut storage, check, "rig-1").await.unwrap(),
        StorageReply::DispatchChecked {
            decision: Decision::Acquire { .. },
            ..
        }
    ));
    assert!(matches!(
        execute(&mut storage, Operation::UnresolvedAttempt {}, "rig-1")
            .await
            .unwrap(),
        StorageReply::Found { attempt: None }
    ));
    // A serialized caller cannot inject an acquiring or recovering snapshot.
    let mut injected = serde_json::to_value(probe(f.state.clone())).unwrap();
    injected["recovery"] = json!({});
    assert!(serde_json::from_value::<Operation>(injected).is_err());
    // Preparation or an unresolved science capture blocks the separate probe.
    assert!(matches!(
        execute(&mut storage, f.begin(), "rig-1").await.unwrap(),
        StorageReply::PreparationStarted { .. }
    ));
    let mut blocked = probe(f.state.clone());
    wire::gate(&mut recovery, &mut blocked, "rig-1")
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        execute(&mut storage, blocked, "rig-1").await.unwrap(),
        StorageReply::Error { .. }
    ));
    f.ready(storage.as_mut().unwrap());
    assert!(matches!(
        execute(&mut storage, f.reserve(), "rig-1").await.unwrap(),
        StorageReply::Reserved { .. }
    ));
    let mut blocked = probe(f.state.clone());
    wire::gate(&mut recovery, &mut blocked, "rig-1")
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        execute(&mut storage, blocked, "rig-1").await.unwrap(),
        StorageReply::Error { .. }
    ));
    // A terminal stop invalidates the original probe, not just future science.
    wire::execute(
        &mut recovery,
        apply(2, start + 12, r::Event::StopNight {}),
        "rig-1",
    )
    .await
    .unwrap();
    assert!(wire::gate(&mut recovery, &mut probe(f.state), "rig-1")
        .await
        .unwrap()
        .is_err());
}
