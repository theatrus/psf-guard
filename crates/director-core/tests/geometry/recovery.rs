use super::*;

#[test]
fn checkpoint_preserves_adjacent_horizon_vertices_exactly() {
    let (program, request, mut constraints) = fixture();
    let local = local(&program);
    let mut seed = 0x41c6_4e6d_1234_5678_u64;
    for sample in 0..128 {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let az = 30.0 + 30.0 * ((seed >> 12) as f64 / (1_u64 << 52) as f64);
        constraints.rig.horizon = Horizon::Custom {
            points: [0.0, az.next_down(), az, az.next_up(), 360.0]
                .into_iter()
                .map(|azimuth_degrees| HorizonPoint {
                    azimuth_degrees,
                    altitude_degrees: -89.0,
                })
                .collect(),
        };
        let bound = compile(program.clone(), &request, constraints.clone()).unwrap();
        let prep = bound
            .preparation(
                "prep".into(),
                &request,
                &constraints,
                "goal",
                local.clone(),
                Estimates::default(),
            )
            .unwrap();
        assert!(
            bound
                .restore_preparation(&prep.checkpoint().unwrap())
                .is_ok(),
            "sample {sample}, az {az:?}"
        );
    }
}

#[test]
fn every_boundary_recovers_against_recompiled_geometry_without_reissuing_work() {
    let (program, mut request, constraints) = preparation_fixture();
    let local = local(&program);
    let bound = compile(program.clone(), &request, constraints.clone()).unwrap();
    let rebuilt = compile(program, &request, constraints.clone()).unwrap();
    let mut prep = bound
        .preparation(
            "prep".into(),
            &request,
            &constraints,
            "goal",
            local,
            Estimates::default(),
        )
        .unwrap();
    loop {
        let bytes = prep.checkpoint().unwrap();
        prep = rebuilt.restore_preparation(&bytes).unwrap();
        assert_eq!(prep.checkpoint().unwrap(), bytes);
        match prep.next(&request, &constraints).unwrap() {
            Next::Run(command) => {
                prep = rebuilt
                    .restore_preparation(&prep.checkpoint().unwrap())
                    .unwrap();
                assert_eq!(
                    prep.next(&request, &constraints).unwrap(),
                    Next::InFlight {
                        ordinal: command.ordinal
                    }
                );
                let completion = receipt(&command, request.state.now_ms);
                prep.complete(completion.clone()).unwrap();
                prep = rebuilt
                    .restore_preparation(&prep.checkpoint().unwrap())
                    .unwrap();
                prep.complete(completion).unwrap();
            }
            Next::ReadyToReserve { .. } => break,
            next => panic!("{next:?}"),
        }
    }
    assert_eq!(prep.observations().len(), 4);
    prep = rebuilt
        .restore_preparation(&prep.checkpoint().unwrap())
        .unwrap();
    request.state.now_ms = rebuilt.windows("goal").unwrap()[0].end_ms - 4_999;
    assert!(matches!(
        prep.next(&request, &constraints),
        Ok(Next::Decision(Decision::Wait { .. }))
    ));
}

#[test]
fn recovered_constraint_halt_or_safety_stop_keeps_pending_receipt_and_late_clock() {
    for stop in [false, true] {
        let (program, mut request, constraints) = fixture();
        let local = local(&program);
        let bound = compile(program, &request, constraints.clone()).unwrap();
        let mut prep = bound
            .preparation(
                "prep".into(),
                &request,
                &constraints,
                "goal",
                local,
                Estimates::default(),
            )
            .unwrap();
        let Next::Run(command) = prep.next(&request, &constraints).unwrap() else {
            panic!()
        };
        let mut changed = constraints.clone();
        changed.rig.site.longitude_degrees += 0.001;
        request.state.now_ms += 100;
        request.state.operator_stop = stop;
        let before = prep.next(&request, &changed).unwrap();
        prep = bound
            .restore_preparation(&prep.checkpoint().unwrap())
            .unwrap();
        assert_eq!(prep.next(&request, &changed).unwrap(), before);
        assert_eq!(prep.pending(), Some(&command));
        prep.complete(receipt(&command, request.state.now_ms - 50))
            .unwrap();
        prep = bound
            .restore_preparation(&prep.checkpoint().unwrap())
            .unwrap();
        request.state.operator_stop = false;
        let next = prep.next(&request, &constraints).unwrap();
        if stop {
            assert!(matches!(next, Next::Decision(Decision::Stop { .. })));
        } else {
            assert_eq!(
                next,
                Next::Decision(Decision::CheckIn {
                    reason: "observing_constraints_changed".into()
                })
            );
        }
        request.state.now_ms -= 1;
        assert_eq!(
            prep.next(&request, &constraints),
            Err(Error::Preparation(
                psf_guard_director_core::preparation::Error::ClockRegression
            ))
        );
    }
}

#[test]
fn complete_binding_is_required_even_when_ids_and_computed_windows_are_unchanged() {
    let (program, request, constraints) = fixture();
    let local = local(&program);
    let bound = compile(program.clone(), &request, constraints.clone()).unwrap();
    let prep = bound
        .preparation(
            "prep".into(),
            &request,
            &constraints,
            "goal",
            local,
            Estimates::default(),
        )
        .unwrap();
    let bytes = prep.checkpoint().unwrap();
    for fault in 0..7 {
        let mut program = program.clone();
        let mut constraints = constraints.clone();
        match fault {
            0 => constraints.rig.site.elevation_meters += 1.0,
            1 => constraints.rig.orientation.ut1_minus_utc_seconds += 0.001,
            2 => constraints.goals[0].horizon_offset_degrees += 0.001,
            3 => program.targets[0].icrs_ra_mas += 1,
            4 => program.configuration.camera_id = "replacement-camera".into(),
            5 => program.assignment.goals[0].attempts_remaining -= 1,
            _ => constraints.rig.revision += 1,
        }
        let changed = compile(program, &request, constraints).unwrap();
        assert!(
            matches!(
                changed.restore_preparation(&bytes),
                Err(Error::InvalidCheckpoint)
            ),
            "fault {fault}"
        );
    }
}

#[test]
fn changed_inner_windows_or_context_cannot_replace_recompiled_evidence() {
    let (program, request, constraints) = preparation_fixture();
    let local = local(&program);
    let bound = compile(program, &request, constraints.clone()).unwrap();
    let prep = bound
        .preparation(
            "prep".into(),
            &request,
            &constraints,
            "goal",
            local,
            Estimates::default(),
        )
        .unwrap();
    let saved: serde_json::Value = serde_json::from_slice(&prep.checkpoint().unwrap()).unwrap();
    for fault in 0..7 {
        let mut outer = saved.clone();
        let mut inner: serde_json::Value =
            serde_json::from_str(outer["preparation"].as_str().unwrap()).unwrap();
        match fault {
            0 => {
                inner["initial"]["assignment"]["goals"][0]["eligible_windows"] =
                    json!([interval(START, START + 60_000)])
            }
            1 => inner["context"]["filter_id"] = json!("other"),
            2 => inner["context"]["target_id"] = json!("other"),
            3 => inner["initial"]["state"]["conditions_valid_until_ms"] = json!(START + 100_000),
            4 => inner["initial"]["assignment"]["goals"][0]["attempts_remaining"] = json!(1),
            5 => outer["initial"]["assignment"]["goals"][0]["exposure_ms"] = json!(1000),
            _ => outer["local"]["configuration"]["camera_id"] = json!("other"),
        }
        outer["preparation"] = json!(serde_json::to_string(&inner).unwrap());
        assert!(
            matches!(
                bound.restore_preparation(&serde_json::to_vec(&outer).unwrap()),
                Err(Error::InvalidCheckpoint)
            ),
            "fault {fault}"
        );
    }
}

#[test]
fn failed_and_uncertain_completion_survive_recovery_without_new_commands() {
    let (program, request, constraints) = fixture();
    let local = local(&program);
    let bound = compile(program, &request, constraints.clone()).unwrap();
    for outcome in [
        Outcome::Failed {
            reason: "failed".into(),
        },
        Outcome::Uncertain {
            reason: "unknown".into(),
        },
    ] {
        let mut prep = bound
            .preparation(
                "prep".into(),
                &request,
                &constraints,
                "goal",
                local.clone(),
                Estimates::default(),
            )
            .unwrap();
        let Next::Run(command) = prep.next(&request, &constraints).unwrap() else {
            panic!()
        };
        let mut completion = receipt(&command, request.state.now_ms);
        completion.outcome = outcome;
        prep.complete(completion.clone()).unwrap();
        prep = bound
            .restore_preparation(&prep.checkpoint().unwrap())
            .unwrap();
        assert_eq!(prep.observations()[0].completion, completion);
        assert!(matches!(
            prep.next(&request, &constraints),
            Ok(Next::Decision(Decision::CheckIn { .. }))
        ));
        // An inner snapshot cannot erase its own failed evidence.
        let mut outer: serde_json::Value =
            serde_json::from_slice(&prep.checkpoint().unwrap()).unwrap();
        let mut inner: serde_json::Value =
            serde_json::from_str(outer["preparation"].as_str().unwrap()).unwrap();
        inner["halted"] = serde_json::Value::Null;
        outer["preparation"] = json!(serde_json::to_string(&inner).unwrap());
        assert!(matches!(
            bound.restore_preparation(&serde_json::to_vec(&outer).unwrap()),
            Err(Error::InvalidCheckpoint)
        ));
    }
}

#[test]
fn geometry_checkpoints_are_strict_bounded_versioned_and_not_legacy_preparation() {
    let (program, request, constraints) = fixture();
    let local = local(&program);
    let bound = compile(program, &request, constraints.clone()).unwrap();
    let prep = bound
        .preparation(
            "prep".into(),
            &request,
            &constraints,
            "goal",
            local,
            Estimates::default(),
        )
        .unwrap();
    let bytes = prep.checkpoint().unwrap();
    assert!(psf_guard_director_core::preparation::Preparation::restore(&bytes).is_err());
    let saved: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    for field in saved.as_object().unwrap().keys() {
        let mut changed = saved.clone();
        changed.as_object_mut().unwrap().remove(field);
        assert!(
            matches!(
                bound.restore_preparation(&serde_json::to_vec(&changed).unwrap()),
                Err(Error::InvalidCheckpoint)
            ),
            "missing {field}"
        );
    }
    for fault in 0..4 {
        let mut changed = saved.clone();
        match fault {
            0 => changed["format_version"] = json!(99),
            1 => changed["engine_version"] = json!("future"),
            2 => changed["unknown"] = json!(true),
            _ => {
                changed["preparation"] = json!(format!(
                    "{{\"format_version\":1,{}",
                    &saved["preparation"].as_str().unwrap()[1..]
                ))
            }
        }
        assert!(matches!(
            bound.restore_preparation(&serde_json::to_vec(&changed).unwrap()),
            Err(Error::InvalidCheckpoint)
        ));
    }
    for invalid in [
        vec![b' '; 8 * MAX_REQUEST_BYTES + 1],
        vec![0xff],
        format!(
            "{{\"format_version\":1,{}",
            &std::str::from_utf8(&bytes).unwrap()[1..]
        )
        .into_bytes(),
    ] {
        assert!(matches!(
            bound.restore_preparation(&invalid),
            Err(Error::InvalidCheckpoint)
        ));
    }
}
