use psf_guard_director_core::{recovery::*, Safety};

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
        weather: None,
    }
}
fn safe() -> Conditions {
    Conditions {
        safety: Safety::Safe,
        motion: Motion::Permitted,
    }
}
fn initial() -> Snapshot {
    Snapshot::new(identity(), policy(), 1000).unwrap()
}

fn weather() -> Snapshot {
    let mut p = policy();
    p.weather = Some(WeatherPolicy {
        stable_safe_ms: 200,
        maximum_hold_ms: 2000,
        maximum_interruptions: 2,
    });
    Snapshot::new(identity(), p, 1000).unwrap()
}

#[test]
fn weather_resume_requires_continuous_clear_evidence_and_explicit_settled_resume() {
    let closed = Conditions {
        safety: Safety::Unsafe,
        motion: Motion::Prohibited,
    };
    let held = weather().apply(1001, closed, &Event::Tick {}).unwrap();
    held.validate().unwrap();
    assert!(matches!(
        held.phase,
        Phase::WeatherHolding {
            cause: Cause::Enclosure {},
            ..
        }
    ));
    let mut state = step(&held, 1010, Event::Tick {});
    assert_eq!(
        state.apply(1100, safe(), &Event::ResumeWeather {}),
        Err(Error::WrongPhase)
    );
    state = step(&state, 1110, Event::Tick {});
    // Unsafe/unknown evidence resets the stability clock, without refilling budgets.
    state = state.apply(1111, closed, &Event::Tick {}).unwrap();
    state = step(&state, 1120, Event::Tick {});
    state = step(&state, 1220, Event::Tick {});
    state = step(&state, 1320, Event::Tick {});
    assert!(matches!(state.phase, Phase::WeatherHolding { .. }));
    state = step(&state, 1320, Event::ResumeWeather {});
    assert!(matches!(state.phase, Phase::Acquiring {}));
    assert_eq!(state.weather_interruptions, 1);
    assert_eq!(state.weather_hold_ms, 319);
    assert_eq!(state.probes_spent, 0);
}

#[test]
fn weather_restart_gap_and_interruption_reset_stability_and_terminal_stops_stay_terminal() {
    let mut state = step(
        &weather(),
        1001,
        Event::WeatherInterrupted { enclosure: false },
    );
    state = step(&state, 1010, Event::Tick {});
    state = step(&state, 1211, Event::Tick {}); // Monitoring gap cannot count as clear.
    assert_eq!(
        state.apply(1211, safe(), &Event::ResumeWeather {}),
        Err(Error::WrongPhase)
    );
    state = step(&state, 1212, Event::WeatherInterrupted { enclosure: true });
    assert!(matches!(
        state.phase,
        Phase::WeatherHolding {
            stable_since_ms: None,
            ..
        }
    ));
    let stopped = step(&state, 1213, Event::StopNight {});
    assert_eq!(
        stopped.apply(1220, safe(), &Event::ResumeWeather {}),
        Err(Error::WrongPhase)
    );
    let ended = step(&state, 10000, Event::Tick {});
    assert!(matches!(
        ended.phase,
        Phase::Stopping {
            cause: Cause::NightEnded {},
            ..
        }
    ));
    let expired = step(&state, 4000, Event::Tick {});
    assert!(matches!(
        expired.phase,
        Phase::Stopping {
            cause: Cause::HoldExpired {},
            ..
        }
    ));
}

#[test]
fn weather_policy_is_opt_in_and_legacy_serialization_stays_unchanged() {
    let state = initial();
    let json = serde_json::to_value(&state).unwrap();
    assert!(json.get("weather_hold_ms").is_none());
    assert!(json["policy"].get("weather").is_none());
    let stopped = state
        .apply(
            1001,
            Conditions {
                safety: Safety::Unsafe,
                motion: Motion::Permitted,
            },
            &Event::Tick {},
        )
        .unwrap();
    assert!(matches!(stopped.phase, Phase::Stopping { .. }));
}
fn sample(now: u64, verdict: Verdict) -> QualitySample {
    QualitySample {
        rig_id: "rig".into(),
        configuration_id: "config".into(),
        capture_id: format!("capture-{now}"),
        observed_at_ms: now,
        context: QualityContext {
            target_id: "target".into(),
            filter_id: "L".into(),
            exposure_ms: 10000,
            bin_x: 1,
            bin_y: 1,
            reference_id: "known-good".into(),
            source: "pixels".into(),
            algorithm_revision: "1".into(),
        },
        verdict,
    }
}
fn step(state: &Snapshot, now: u64, event: Event) -> Snapshot {
    let next = state.apply(now, safe(), &event).unwrap();
    next.validate().unwrap();
    next
}
fn quality(state: &Snapshot, now: u64, verdict: Verdict) -> Snapshot {
    step(
        state,
        now,
        Event::Quality {
            sample: sample(now, verdict),
        },
    )
}
fn held() -> Snapshot {
    quality(
        &quality(&initial(), 1001, Verdict::CorroboratedPoor),
        1002,
        Verdict::CorroboratedPoor,
    )
}
fn begin(state: &Snapshot, now: u64) -> Snapshot {
    step(
        state,
        now,
        Event::BeginRecovery {
            attempt_id: format!("probe-{now}"),
        },
    )
}
fn complete(state: &Snapshot, now: u64, verdict: Verdict) -> Snapshot {
    let Phase::Recovering { attempt_id, .. } = &state.phase else {
        panic!("not recovering")
    };
    step(
        state,
        now,
        Event::RecoveryCompleted {
            attempt_id: attempt_id.clone(),
            result: RecoveryResult::Quality {
                sample: Box::new(sample(now, verdict)),
            },
        },
    )
}
fn failure(now: u64, operation: Operation, uncertain: bool) -> Event {
    Event::Failure {
        failure: Failure {
            attempt_id: format!("operation-{now}"),
            operation,
            device_id: "device".into(),
            target_id: format!("target-{now}"),
            uncertain,
        },
    }
}

#[test]
fn quality_requires_consecutive_compatible_corroborated_evidence() {
    let first = quality(&initial(), 1001, Verdict::CorroboratedPoor);
    assert_eq!(first.phase, Phase::Acquiring {});
    let unknown = quality(&first, 1002, Verdict::Unknown);
    assert_eq!(unknown.consecutive_bad, 0);
    let first = quality(&unknown, 1003, Verdict::CorroboratedPoor);
    let mut other = sample(1004, Verdict::CorroboratedPoor);
    other.context.filter_id = "Ha".into();
    let changed = step(&first, 1004, Event::Quality { sample: other });
    assert_eq!(changed.consecutive_bad, 1);
    assert_eq!(changed.phase, Phase::Acquiring {});
    assert!(matches!(held().phase, Phase::Holding { .. }));
}

#[test]
fn disabled_and_monitor_only_never_hold() {
    for mode in [QualityMode::Disabled, QualityMode::MonitorOnly] {
        let mut p = policy();
        p.quality_mode = mode;
        let mut state = Snapshot::new(identity(), p, 1000).unwrap();
        for now in 1001..1010 {
            state = quality(&state, now, Verdict::CorroboratedPoor);
        }
        assert_eq!(state.phase, Phase::Acquiring {});
    }
    let mut p = policy();
    p.quality_mode = QualityMode::ParkAndStop;
    let state = Snapshot::new(identity(), p, 1000).unwrap();
    let state = quality(
        &quality(&state, 1001, Verdict::CorroboratedPoor),
        1002,
        Verdict::CorroboratedPoor,
    );
    assert!(matches!(
        state.phase,
        Phase::Stopping {
            cause: Cause::Quality { .. },
            ..
        }
    ));
}

#[test]
fn sample_scope_age_order_and_context_are_checked() {
    let state = quality(&initial(), 1001, Verdict::ConfirmedGood);
    for mut bad in [
        sample(1001, Verdict::ConfirmedGood),
        sample(1300, Verdict::ConfirmedGood),
        sample(999, Verdict::ConfirmedGood),
    ] {
        assert!(state
            .apply(
                1200,
                safe(),
                &Event::Quality {
                    sample: bad.clone()
                }
            )
            .is_err());
        bad.observed_at_ms = 1200;
        bad.rig_id = "other".into();
        assert!(state
            .apply(1200, safe(), &Event::Quality { sample: bad })
            .is_err());
    }
    let mut bad = sample(1200, Verdict::ConfirmedGood);
    bad.context.exposure_ms = 0;
    assert_eq!(
        state.apply(1200, safe(), &Event::Quality { sample: bad }),
        Err(Error::InvalidInput)
    );
}

#[test]
fn cooldown_and_tick_never_grant_recovery_or_resume() {
    let state = held();
    let event = Event::BeginRecovery {
        attempt_id: "probe".into(),
    };
    assert_eq!(state.apply(1101, safe(), &event), Err(Error::WrongPhase));
    assert_eq!(
        state.apply(
            1102,
            Conditions {
                motion: Motion::Unknown,
                ..safe()
            },
            &event
        ),
        Err(Error::WrongPhase)
    );
    assert!(matches!(
        step(&state, 1102, Event::Tick {}).phase,
        Phase::Holding { .. }
    ));
}

#[test]
fn recovery_requires_independent_good_probes_with_frozen_reference() {
    let state = begin(&held(), 1102);
    let mut changed = sample(1103, Verdict::ConfirmedGood);
    changed.context.reference_id = "drifting".into();
    assert_eq!(
        state.apply(
            1103,
            safe(),
            &Event::RecoveryCompleted {
                attempt_id: "probe-1102".into(),
                result: RecoveryResult::Quality {
                    sample: Box::new(changed)
                },
            }
        ),
        Err(Error::ChangedReference)
    );
    let state = step(&state, 1104, Event::Tick {});
    // Delivery after a tick is fine: acquisition began after the attempt, not after the tick.
    let state = step(
        &state,
        1105,
        Event::RecoveryCompleted {
            attempt_id: "probe-1102".into(),
            result: RecoveryResult::Quality {
                sample: Box::new(sample(1103, Verdict::ConfirmedGood)),
            },
        },
    );
    assert!(matches!(state.phase, Phase::Holding { .. }));
    let state = complete(&begin(&state, 1205), 1206, Verdict::ConfirmedGood);
    assert_eq!(state.phase, Phase::Acquiring {});
    assert_eq!(state.probes_spent, 2);
    assert_eq!(state.total_hold_ms, 204);
}

#[test]
fn unknown_probe_resets_hysteresis_and_exhaustion_stops() {
    let state = complete(&begin(&held(), 1102), 1103, Verdict::ConfirmedGood);
    let state = complete(&begin(&state, 1203), 1204, Verdict::Unknown);
    let Phase::Holding { hold } = &state.phase else {
        panic!()
    };
    assert_eq!(hold.good_probes, 0);
    let state = complete(&begin(&state, 1304), 1305, Verdict::ConfirmedGood);
    assert!(matches!(
        state.phase,
        Phase::Stopping {
            cause: Cause::ProbeBudget {},
            ..
        }
    ));
}

#[test]
fn hold_and_probe_budgets_survive_success_and_new_incidents() {
    let state = complete(&begin(&held(), 1102), 1103, Verdict::ConfirmedGood);
    let state = complete(&begin(&state, 1203), 1204, Verdict::ConfirmedGood);
    let state = quality(
        &quality(&state, 1300, Verdict::CorroboratedPoor),
        1301,
        Verdict::CorroboratedPoor,
    );
    let Phase::Holding { hold } = &state.phase else {
        panic!()
    };
    assert_eq!(hold.expires_at_ms, 2099); // Only 798 ms remain across the night.
    let state = complete(&begin(&state, 1401), 1402, Verdict::ConfirmedGood);
    assert!(matches!(
        state.phase,
        Phase::Stopping {
            cause: Cause::ProbeBudget {},
            ..
        }
    ));
}

#[test]
fn deadlines_stop_without_an_unbounded_retry() {
    let state = held();
    assert!(matches!(
        step(&state, 2002, Event::Tick {}).phase,
        Phase::Stopping {
            cause: Cause::HoldExpired {},
            ..
        }
    ));
    let state = begin(&state, 1102);
    assert!(matches!(
        step(&state, 1152, Event::Tick {}).phase,
        Phase::Stopping {
            cause: Cause::RecoveryUncertain {},
            ..
        }
    ));
    assert!(matches!(
        step(&initial(), 10000, Event::Tick {}).phase,
        Phase::Stopping {
            cause: Cause::NightEnded {},
            ..
        }
    ));
}

#[test]
fn clean_equipment_recovery_does_not_reset_nightly_failure_budget() {
    let mut state = initial();
    for (index, now) in [1001, 1200, 1400].into_iter().enumerate() {
        state = step(&state, now, failure(now, Operation::Guide, false));
        assert_eq!(state.total_failures, index as u32 + 1);
        if index == 2 {
            break;
        }
        state = begin(&state, now + 100);
        state = step(
            &state,
            now + 101,
            Event::RecoveryCompleted {
                attempt_id: format!("probe-{}", now + 100),
                result: RecoveryResult::EquipmentVerified {},
            },
        );
        assert_eq!(state.phase, Phase::Acquiring {});
        assert_eq!(state.failures[0].consecutive, 0);
    }
    assert_eq!(state.failures.len(), 1); // Different targets do not erase device history.
    assert!(matches!(
        state.phase,
        Phase::Stopping {
            cause: Cause::FailureBudget {},
            ..
        }
    ));
}

#[test]
fn uncertain_and_motion_failures_stop_instead_of_retrying() {
    for (op, uncertain) in [
        (Operation::Slew, false),
        (Operation::Capture, false),
        (Operation::Guide, true),
    ] {
        assert!(matches!(
            step(&initial(), 1001, failure(1001, op, uncertain)).phase,
            Phase::Stopping { .. }
        ));
    }
    let state = step(&initial(), 1001, failure(1001, Operation::Guide, false));
    let state = step(&state, 1002, failure(1002, Operation::Guide, false));
    assert!(matches!(
        state.phase,
        Phase::Stopping {
            cause: Cause::FailureBudget {},
            ..
        }
    ));
}

#[test]
fn safety_and_roof_preempt_quality_and_never_clear_the_latch() {
    for state in [initial(), held(), begin(&held(), 1102)] {
        for safety in [Safety::Unsafe, Safety::Unknown] {
            let stopped = state
                .apply(
                    1103,
                    Conditions {
                        safety,
                        motion: Motion::Unknown,
                    },
                    &Event::Tick {},
                )
                .unwrap();
            assert!(matches!(
                stopped.phase,
                Phase::Stopped {
                    cause: Cause::Safety {},
                    shutdown: Shutdown::MotionBlocked
                }
            ));
            assert_eq!(step(&stopped, 1200, Event::Tick {}).phase, stopped.phase);
        }
        let stopped = state
            .apply(
                1103,
                Conditions {
                    motion: Motion::Prohibited,
                    ..safe()
                },
                &Event::Tick {},
            )
            .unwrap();
        assert!(matches!(
            stopped.phase,
            Phase::Stopped {
                cause: Cause::Enclosure {},
                ..
            }
        ));
    }
}

#[test]
fn lost_enclosure_clearance_cannot_resume_from_a_successful_probe() {
    let state = begin(&held(), 1102);
    let stopped = state
        .apply(
            1103,
            Conditions {
                motion: Motion::Unknown,
                ..safe()
            },
            &Event::RecoveryCompleted {
                attempt_id: "probe-1102".into(),
                result: RecoveryResult::Quality {
                    sample: Box::new(sample(1103, Verdict::ConfirmedGood)),
                },
            },
        )
        .unwrap();
    assert!(matches!(
        stopped.phase,
        Phase::Stopped {
            cause: Cause::Enclosure {},
            shutdown: Shutdown::MotionBlocked
        }
    ));
    assert_eq!(step(&stopped, 1104, Event::Tick {}).phase, stopped.phase);
    let mut contradictory = policy();
    contradictory.quality_mode = QualityMode::ParkAndStop;
    contradictory.park_on_stop = false;
    assert_eq!(
        Snapshot::new(identity(), contradictory, 1000),
        Err(Error::InvalidInput)
    );
}

#[test]
fn park_is_single_attempt_and_reports_failure_or_uncertainty_truthfully() {
    let stop = step(&initial(), 1001, Event::StopNight {});
    let park = step(
        &stop,
        1002,
        Event::BeginPark {
            attempt_id: "park".into(),
        },
    );
    assert_eq!(
        park.apply(
            1003,
            safe(),
            &Event::BeginPark {
                attempt_id: "again".into()
            }
        ),
        Err(Error::WrongPhase)
    );
    for (result, expected) in [
        (ParkResult::Parked, Shutdown::Parked),
        (ParkResult::Failed, Shutdown::ParkFailed),
        (ParkResult::Uncertain, Shutdown::ParkUncertain),
    ] {
        let state = step(
            &park,
            1003,
            Event::ParkCompleted {
                attempt_id: "park".into(),
                result,
            },
        );
        assert!(matches!(state.phase, Phase::Stopped { shutdown, .. } if shutdown == expected));
        assert_eq!(
            step(
                &state,
                1004,
                Event::BeginPark {
                    attempt_id: "retry".into()
                }
            )
            .phase,
            state.phase
        );
    }
    let lost = park
        .apply(
            1003,
            Conditions {
                motion: Motion::Prohibited,
                ..safe()
            },
            &Event::Tick {},
        )
        .unwrap();
    assert!(matches!(
        lost.phase,
        Phase::Stopped {
            shutdown: Shutdown::ParkUncertain,
            ..
        }
    ));
    assert!(matches!(
        step(&park, 1051, Event::Tick {}).phase,
        Phase::Stopped {
            shutdown: Shutdown::ParkUncertain,
            ..
        }
    ));
}

#[test]
fn invalid_state_policy_clock_and_wire_fields_fail_closed() {
    let state = initial();
    assert_eq!(
        state.apply(999, safe(), &Event::Tick {}),
        Err(Error::ClockReversed)
    );
    let mut broken = state.clone();
    broken.probes_spent = 1000;
    assert_eq!(
        broken.apply(1001, safe(), &Event::Tick {}),
        Err(Error::InvalidInput)
    );
    let mut p = policy();
    p.good_probes = p.maximum_probes + 1;
    assert_eq!(Snapshot::new(identity(), p, 1000), Err(Error::InvalidInput));
    assert!(serde_json::from_str::<Event>(r#"{"event":"tick","resume":true}"#).is_err());
    assert!(serde_json::from_str::<Phase>(r#"{"state":"acquiring","resume":true}"#).is_err());
    assert!(serde_json::from_str::<Cause>(r#"{"kind":"safety","clear":true}"#).is_err());
    assert!(serde_json::from_str::<RecoveryResult>(
        r#"{"outcome":"equipment_verified","clear":true}"#
    )
    .is_err());
    let json = serde_json::to_string(&held()).unwrap();
    assert_eq!(serde_json::from_str::<Snapshot>(&json).unwrap(), held());
}
