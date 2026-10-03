use psf_guard_director_core::{priority::*, windows::Interval, Decision, Request, Safety};

fn source(scope: Scope) -> Source {
    Source {
        scope,
        id: format!("{scope:?}"),
        revision: 1,
    }
}

#[test]
fn resolved_policy_roundtrip_rejects_forged_values_and_provenance() {
    let policy = resolved(Policy::preset(Preset::Balanced));
    let encoded = serde_json::to_value(&policy).unwrap();
    assert_eq!(
        serde_json::from_value::<ResolvedPolicy>(encoded.clone()).unwrap(),
        policy
    );
    for field in ["policy", "provenance", "schema_version"] {
        let mut changed = encoded.clone();
        match field {
            "policy" => changed[field]["importance"] = 99.into(),
            "provenance" => changed[field]["importance"]["revision"] = 99.into(),
            _ => changed[field] = 99.into(),
        }
        assert!(serde_json::from_value::<ResolvedPolicy>(changed).is_err());
    }
}
fn resolved(policy: Policy) -> ResolvedPolicy {
    resolve(policy, source(Scope::Global), &[]).unwrap()
}
fn only(factor: Factor) -> Policy {
    let mut policy = Policy::preset(Preset::Balanced);
    for (f, w) in &mut policy.weights {
        *w = if *f == factor { 100 } else { 0 };
    }
    policy.minimum_dwell_ms = 0;
    policy.switch_margin = 0;
    policy
}
fn fixture() -> (Request, Vec<Candidate>) {
    let json: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/decisions.json")).unwrap();
    let request: Request = serde_json::from_value(json["base"].clone()).unwrap();
    let candidates = request
        .assignment
        .goals
        .iter()
        .map(|g| Candidate {
            goal_id: g.id.clone(),
            target_id: g.id.clone(),
            policy: resolved(Policy::preset(Preset::Balanced)),
            altitude: Some(5000),
            moon_opportunity: Some(5000),
        })
        .collect();
    (request, candidates)
}
fn winner(result: &Ranking) -> &str {
    match &result.decision {
        Decision::Acquire { goal_id, .. } => goal_id,
        other => panic!("{other:?}"),
    }
}

#[test]
fn hierarchy_preserves_explicit_zero_and_field_sources() {
    let global = Policy::preset(Preset::Balanced);
    let layers = [
        Layer {
            source: source(Scope::Site),
            overrides: Overrides {
                weights: [(Factor::MoonOpportunity, 75)].into(),
                ..Default::default()
            },
        },
        Layer {
            source: source(Scope::Rig),
            overrides: Overrides {
                minimum_dwell_ms: Some(0),
                ..Default::default()
            },
        },
        Layer {
            source: source(Scope::Project),
            overrides: Overrides {
                importance: Some(0),
                weights: [(Factor::Altitude, 0)].into(),
                ..Default::default()
            },
        },
    ];
    let result = resolve(global.clone(), source(Scope::Global), &layers).unwrap();
    assert_eq!(result.policy().importance, 0);
    assert_eq!(result.policy().minimum_dwell_ms, 0);
    assert_eq!(result.policy().weights[&Factor::MoonOpportunity], 75);
    assert_eq!(result.policy().weights[&Factor::Altitude], 0);
    assert_eq!(
        result.provenance().weights[&Factor::Efficiency].scope,
        Scope::Global
    );
    assert_eq!(
        result.provenance().weights[&Factor::MoonOpportunity].scope,
        Scope::Site
    );
    assert_eq!(result.provenance().importance.scope, Scope::Project);
    let inherited = resolve(global.clone(), source(Scope::Global), &layers[..2]).unwrap();
    assert_eq!(inherited.policy().importance, global.importance);
    assert_eq!(
        inherited.policy().weights[&Factor::Altitude],
        global.weights[&Factor::Altitude]
    );
    let encoded = serde_json::to_value(result).unwrap();
    assert_eq!(encoded["schema_version"], VERSION);
    assert_eq!(encoded["layers"][2]["overrides"]["importance"], 0);
    assert_eq!(encoded["global"]["importance"], 50);
    assert_eq!(encoded["global_source"]["revision"], 1);
}

#[test]
fn inheritance_rejects_bad_order_revision_identity_and_values() {
    let global = Policy::preset(Preset::Balanced);
    for scopes in [
        vec![Scope::Rig, Scope::Site],
        vec![Scope::Project, Scope::Project],
        vec![Scope::Global],
    ] {
        let layers: Vec<_> = scopes
            .into_iter()
            .map(|s| Layer {
                source: source(s),
                overrides: Overrides::default(),
            })
            .collect();
        assert_eq!(
            resolve(global.clone(), source(Scope::Global), &layers),
            Err(Error::InvalidHierarchy)
        );
    }
    for invalid in [
        Source {
            revision: 0,
            ..source(Scope::Global)
        },
        Source {
            id: "".into(),
            ..source(Scope::Global)
        },
        source(Scope::Site),
    ] {
        assert_eq!(
            resolve(global.clone(), invalid, &[]),
            Err(Error::InvalidHierarchy)
        );
    }
    for overrides in [
        Overrides {
            importance: Some(101),
            ..Default::default()
        },
        Overrides {
            switch_margin: Some(10001),
            ..Default::default()
        },
        Overrides {
            minimum_dwell_ms: Some(86_400_001),
            ..Default::default()
        },
        Overrides {
            weights: [(Factor::Altitude, 1001)].into(),
            ..Default::default()
        },
    ] {
        assert_eq!(
            resolve(
                global.clone(),
                source(Scope::Global),
                &[Layer {
                    source: source(Scope::Rig),
                    overrides
                }]
            ),
            Err(Error::InvalidPolicy)
        );
    }
    let mut empty = global.clone();
    empty.weights.clear();
    assert_eq!(empty.validate(), Err(Error::InvalidPolicy));
    let mut zero = global;
    for w in zero.weights.values_mut() {
        *w = 0;
    }
    assert_eq!(zero.validate(), Err(Error::InvalidPolicy));
}

#[test]
fn presets_and_ts_import_are_explicit_not_legacy_ordinals() {
    for preset in [
        Preset::Balanced,
        Preset::FinishGoals,
        Preset::BestConditions,
    ] {
        let p = Policy::preset(preset);
        p.validate().unwrap();
        assert_eq!(p.weights.values().sum::<u16>(), 100);
    }
    assert_eq!(
        [
            ts_importance(Some(0)),
            ts_importance(Some(1)),
            ts_importance(Some(2))
        ],
        [25, 50, 75]
    );
    for input in [None, Some(-1), Some(3), Some(i64::MAX)] {
        assert_eq!(ts_importance(input), 50);
    }
    assert!(serde_json::from_str::<Overrides>(r#"{"weights":{"mystery":1}}"#).is_err());
    assert!(
        serde_json::from_str::<Overrides>(r#"{"weights":{"altitude":1,"altitude":2}}"#).is_err()
    );
}

#[test]
fn each_preference_can_drive_selection_without_changing_legacy_priority() {
    for (factor, expected) in [
        (Factor::Importance, "long-ha"),
        (Factor::WindowUrgency, "short-ha"),
        (Factor::Altitude, "long-ha"),
        (Factor::MoonOpportunity, "long-ha"),
        (Factor::Completion, "short-ha"),
        (Factor::Efficiency, "long-ha"),
        (Factor::Continuity, "short-ha"),
    ] {
        let (request, mut candidates) = fixture();
        for candidate in &mut candidates {
            candidate.policy = resolved(only(factor));
        }
        let mut important = only(factor);
        important.importance = 51;
        candidates[1].policy = resolved(important);
        candidates[1].altitude = Some(9000);
        candidates[1].moon_opportunity = Some(9000);
        let active = ActiveGoal {
            goal_id: "short-ha".into(),
            selected_at_ms: request.state.now_ms,
        };
        let result = preview(&request, &candidates, Some(&active)).unwrap();
        assert_eq!(winner(&result), expected, "{factor:?}");
        assert!(
            matches!(psf_guard_director_core::evaluate(&request).unwrap(), Decision::Acquire { goal_id, .. } if goal_id == "short-ha")
        );
        for score in result.candidates {
            let numerator: u64 = score.contributions.values().map(|c| c.weighted_value).sum();
            let denominator: u64 = score
                .contributions
                .values()
                .map(|c| u64::from(c.weight))
                .sum();
            assert_eq!(u64::from(score.total), numerator / denominator);
        }
    }
}

#[test]
fn pending_credit_is_not_accepted_completion_and_exhausted_work_is_excluded() {
    let (mut request, mut candidates) = fixture();
    for c in &mut candidates {
        c.policy = resolved(only(Factor::Completion));
    }
    request.assignment.goals[1].pending = 3;
    let result = preview(&request, &candidates, None).unwrap();
    assert_eq!(winner(&result), "short-ha");
    assert_eq!(
        result.candidates[1].contributions[&Factor::Completion].value,
        0
    );
    request.assignment.goals[0].attempts_remaining = 0;
    assert_eq!(
        winner(&preview(&request, &candidates, None).unwrap()),
        "long-ha"
    );
    request.assignment.goals[1].pending = 4;
    assert!(preview(&request, &candidates, None)
        .unwrap()
        .candidates
        .is_empty());
}

#[test]
fn missing_optional_evidence_is_neutral_and_visible_not_perfect() {
    let (request, mut candidates) = fixture();
    for c in &mut candidates {
        c.policy = resolved(only(Factor::Altitude));
    }
    candidates[0].altitude = None;
    candidates[1].altitude = Some(5001);
    let result = preview(&request, &candidates, None).unwrap();
    assert_eq!(winner(&result), "long-ha");
    let missing = &result.candidates[1].contributions[&Factor::Altitude];
    assert_eq!(missing.value, 5000);
    assert!(missing.missing);
}

#[test]
fn ties_and_order_are_stable_and_weight_scale_does_not_matter() {
    let (mut request, mut candidates) = fixture();
    for c in &mut candidates {
        c.policy = resolved(only(Factor::Importance));
    }
    let original = preview(&request, &candidates, None).unwrap();
    assert_eq!(winner(&original), "long-ha");
    request.assignment.goals.reverse();
    candidates.reverse();
    let mut scaled = only(Factor::Importance);
    scaled.weights.insert(Factor::Importance, 1000);
    for c in &mut candidates {
        c.policy = resolved(scaled.clone());
    }
    let other = preview(&request, &candidates, None).unwrap();
    assert_eq!(other.decision, original.decision);
    assert_eq!(other.candidates[0].total, original.candidates[0].total);
}

#[test]
fn dwell_and_margin_reduce_switching_but_never_hold_ineligible_work() {
    let (mut request, mut candidates) = fixture();
    let mut p = only(Factor::Importance);
    p.minimum_dwell_ms = 5000;
    p.switch_margin = 200;
    candidates[0].policy = resolved(p.clone());
    p.importance = 53;
    candidates[1].policy = resolved(p);
    let mut active = ActiveGoal {
        goal_id: "short-ha".into(),
        selected_at_ms: 9000,
    };
    assert!(
        preview(&request, &candidates, Some(&active))
            .unwrap()
            .retained_active
    );
    active.selected_at_ms = 5000;
    assert_eq!(
        winner(&preview(&request, &candidates, Some(&active)).unwrap()),
        "long-ha"
    );
    let mut p = candidates[1].policy.policy().clone();
    p.importance = 52;
    candidates[1].policy = resolved(p);
    assert!(
        preview(&request, &candidates, Some(&active))
            .unwrap()
            .retained_active
    );
    request.assignment.goals[0].eligible_windows = vec![Interval {
        start_ms: 1000,
        end_ms: 20000,
    }];
    let result = preview(&request, &candidates, Some(&active)).unwrap();
    assert_eq!(winner(&result), "long-ha");
    assert!(!result.retained_active);
}

#[test]
fn hard_stops_and_boundaries_win_even_with_missing_preferences() {
    let (mut request, _) = fixture();
    request.state.safety = Safety::Unsafe;
    assert!(matches!(
        preview(&request, &[], None).unwrap().decision,
        Decision::Stop { .. }
    ));
    request.state.safety = Safety::Safe;
    request.state.at_boundary = false;
    assert!(matches!(
        preview(&request, &[], None).unwrap().decision,
        Decision::Continue { .. }
    ));
    request.state.at_boundary = true;
    request.state.now_ms = request.assignment.expires_at_ms;
    assert!(matches!(
        preview(&request, &[], None).unwrap().decision,
        Decision::CheckIn { .. }
    ));
}

#[test]
fn reject_mismatched_duplicate_candidates_bad_scores_and_future_active_time() {
    let (request, candidates) = fixture();
    for mutation in 0..5 {
        let mut bad = candidates.clone();
        match mutation {
            0 => {
                bad.pop();
            }
            1 => bad[1].goal_id = bad[0].goal_id.clone(),
            2 => bad[0].altitude = Some(10001),
            3 => bad[0].moon_opportunity = Some(10001),
            _ => bad[0].target_id.clear(),
        }
        assert_eq!(preview(&request, &bad, None), Err(Error::InvalidCandidates));
    }
    for (id, time) in [("unknown", 1000), ("short-ha", 0), ("short-ha", 10001)] {
        assert_eq!(
            preview(
                &request,
                &candidates,
                Some(&ActiveGoal {
                    goal_id: id.into(),
                    selected_at_ms: time
                })
            ),
            Err(Error::InvalidActiveGoal)
        );
    }
}

#[test]
fn changing_parent_defaults_reaches_only_inherited_fields() {
    let first = Policy::preset(Preset::Balanced);
    let project = Layer {
        source: source(Scope::Project),
        overrides: Overrides {
            weights: [(Factor::Altitude, 0)].into(),
            ..Default::default()
        },
    };
    let a = resolve(
        first.clone(),
        source(Scope::Global),
        std::slice::from_ref(&project),
    )
    .unwrap();
    let mut second = first;
    second.weights.insert(Factor::Altitude, 55);
    second.weights.insert(Factor::Efficiency, 30);
    let b = resolve(
        second,
        Source {
            revision: 2,
            ..source(Scope::Global)
        },
        &[project],
    )
    .unwrap();
    assert_eq!(
        a.policy().weights[&Factor::Altitude],
        b.policy().weights[&Factor::Altitude]
    );
    assert_ne!(
        a.policy().weights[&Factor::Efficiency],
        b.policy().weights[&Factor::Efficiency]
    );
    assert_eq!(b.provenance().weights[&Factor::Efficiency].revision, 2);
    assert_eq!(
        b.provenance().weights[&Factor::Altitude].scope,
        Scope::Project
    );
}

#[test]
fn slow_overhead_and_exact_window_end_are_shared_eligibility_not_scoring_choices() {
    let (mut request, mut candidates) = fixture();
    for c in &mut candidates {
        c.policy = resolved(only(Factor::Importance));
    }
    let mut preferred = only(Factor::Importance);
    preferred.importance = 100;
    candidates[0].policy = resolved(preferred);
    request.state.now_ms = 60_000;
    assert_eq!(
        winner(&preview(&request, &candidates, None).unwrap()),
        "short-ha"
    );
    request.assignment.goals[0].overhead_ms += 1;
    assert_eq!(
        winner(&preview(&request, &candidates, None).unwrap()),
        "long-ha"
    );
    request.state.now_ms = u64::MAX - 1;
    request.assignment.expires_at_ms = u64::MAX;
    request.state.conditions_valid_until_ms = u64::MAX;
    for g in &mut request.assignment.goals {
        g.eligible_windows[0].end_ms = u64::MAX;
    }
    assert!(matches!(
        preview(&request, &candidates, None).unwrap().decision,
        Decision::CheckIn { .. }
    ));
}
