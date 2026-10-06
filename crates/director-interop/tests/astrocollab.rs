use psf_guard_director_interop::{astrocollab::*, Error};
use serde_json::{json, Value};

const STARFRONT: &str = include_str!("fixtures/starfront-tonight.json");
const REFERENCE: &str = include_str!("fixtures/reference-tonight.json");
const NIGHT: &str = "2026-10-05";

fn source() -> Source {
    Source::new("https://collab.example/community", "000000000001", false).unwrap()
}

fn fixture() -> Value {
    serde_json::from_str(STARFRONT).unwrap()
}

fn read(value: &Value) -> Result<Tonight, Error> {
    decode_tonight(&serde_json::to_vec(value).unwrap(), &source(), NIGHT)
}

fn changed(edit: impl FnOnce(&mut Value)) -> Value {
    let mut v = fixture();
    edit(&mut v["tasks"][0]);
    v["task"] = v["tasks"][0].clone();
    v
}

#[test]
fn both_servers_normalize_to_the_same_nightly_visits_not_season_hours() {
    let a = decode_tonight(STARFRONT.as_bytes(), &source(), NIGHT).unwrap();
    let raw: Value = serde_json::from_str(REFERENCE).unwrap();
    let b_source = Source::new(
        "https://reference.example",
        raw["task"]["agent"].as_str().unwrap(),
        false,
    )
    .unwrap();
    let b = decode_tonight(REFERENCE.as_bytes(), &b_source, NIGHT).unwrap();
    for nightly in [&a, &b] {
        assert_eq!(nightly.schema_version, SCHEMA_VERSION);
        assert_eq!(nightly.shares.len(), 1);
        let task = &nightly.shares[0];
        assert!(task.review_reasons.is_empty());
        assert_eq!(task.panel_order, vec![0, 1, 2, 3, 4, 5]);
        assert_eq!(task.demands.len(), 6);
        for (index, demand) in task.demands.iter().enumerate() {
            assert_eq!(demand.panel_index, index as u32);
            assert_eq!(demand.filter, "O");
            assert_eq!(demand.exposure_ms, 300_000);
            assert_eq!(demand.requested_frames, 11);
        }
        assert_eq!(
            task.requirements
                .as_ref()
                .unwrap()
                .min_moon_separation_degrees,
            Some(30.0)
        );
        assert_eq!(task.geometry_digest.len(), 64);
    }
    assert_eq!(a.shares[0].demands, b.shares[0].demands);
    // Neither source chooses rank, grants attempts, or returns an executable program.
    let output = serde_json::to_value(a).unwrap();
    for forbidden in [
        "attempts_remaining",
        "accepted",
        "allocation_id",
        "expires_at_ms",
        "token",
    ] {
        assert!(!output.to_string().contains(&format!("\"{forbidden}\":")));
    }
}

#[test]
fn source_scopes_host_base_path_and_agent_and_rejects_credentials() {
    assert_eq!(source().base_url(), "https://collab.example/community/");
    assert_eq!(source().agent_id(), "000000000001");
    for uri in [
        "http://remote.example",
        "file:///tmp/a",
        "https://u:p@collab.example",
        "https://collab.example?token=secret",
        "https://collab.example/#secret",
    ] {
        assert_eq!(
            Source::new(uri, "000000000001", true),
            Err(Error::InvalidSource)
        );
    }
    assert_eq!(
        Source::new("http://127.0.0.1:8800", "000000000001", false),
        Err(Error::InvalidSource)
    );
    for uri in [
        "http://127.0.0.1:8800",
        "http://[::1]:8800",
        "http://localhost:8800",
    ] {
        assert!(Source::new(uri, "000000000001", true).is_ok());
    }
    assert_ne!(
        source(),
        Source::new("https://collab.example/other", "000000000001", false).unwrap()
    );
    assert_ne!(
        source(),
        Source::new("https://collab.example/community", "000000000002", false).unwrap()
    );
}

#[test]
fn invalid_external_ids_and_other_agents_are_not_adoptable() {
    for id in [
        "",
        "00000000000A",
        "abcdefgh1234",
        "00000000-0000-4000-8000-000000000001",
    ] {
        assert!(Source::new("https://collab.example", id, false).is_err());
        assert!(read(&changed(|t| t["id"] = json!(id))).is_err());
    }
    assert_eq!(
        read(&changed(|t| t["agent"] = json!("000000000002"))),
        Err(Error::InvalidIdentity)
    );
}

#[test]
fn health_retains_absent_features_for_signin_discovery() {
    let health = decode_health(include_bytes!("fixtures/starfront-health.json")).unwrap();
    assert_eq!(health.protocol, 1);
    assert!(health.features.is_none());
    assert_eq!(health.server_time_ms, 1_791_171_001_000);
    let body = json!({"ok":true,"protocol":1,"version":"test","time":1791171001.125,"features":["pairing","future"]});
    let health = decode_health(&serde_json::to_vec(&body).unwrap()).unwrap();
    assert_eq!(health.server_time_ms, 1_791_171_001_125);
    assert!(health.features.unwrap().contains("future"));
}

#[test]
fn profile_keeps_physical_units_colour_and_unknown_rotation_distinct() {
    let mut v: Value = serde_json::from_str(include_str!("fixtures/starfront-hello.json")).unwrap();
    let profile = decode_hello_profile(&serde_json::to_vec(&v).unwrap()).unwrap();
    assert_eq!(profile.focal_length_mm, Some(530.0));
    assert_eq!(profile.typical_hfr_arcsec, Some(2.4));
    assert_eq!(profile.typical_guide_rms_arcsec, Some(0.62));
    assert_eq!(profile.rotation, Rotation::Adjustable);
    assert_eq!(profile.window_from.as_deref(), Some("21:30"));
    assert_eq!(profile.window_to.as_deref(), Some("04:30"));
    assert_eq!(
        profile.reported_scale_arcsec_per_pixel,
        Some(1.463313962264151)
    );
    assert_eq!(
        profile.reported_field_degrees,
        Some([2.53966267672956, 1.6974441962264148])
    );
    assert_eq!(profile.exposures_ms["H"], Some(300_000));
    v["profile"].as_object_mut().unwrap().remove("rotation");
    assert_eq!(
        decode_hello_profile(&serde_json::to_vec(&v).unwrap())
            .unwrap()
            .rotation,
        Rotation::Unknown
    );
    v["profile"]["rotation"] = json!(0);
    assert_eq!(
        decode_hello_profile(&serde_json::to_vec(&v).unwrap())
            .unwrap()
            .rotation,
        Rotation::Fixed {
            position_angle_mas: 0
        }
    );
    v["profile"]["rotation"] = json!("unknown");
    v["profile"]["typicalHfr"] = json!("");
    v["profile"]["typicalGuideRms"] = json!(0);
    v["profile"]["colour"] = json!(true);
    v["profile"]["filters"] = json!({"RGB":null,"Dual band":7});
    let profile = decode_hello_profile(&serde_json::to_vec(&v).unwrap()).unwrap();
    assert_eq!(profile.rotation, Rotation::Unknown);
    assert_eq!(profile.typical_hfr_arcsec, None);
    assert_eq!(profile.typical_guide_rms_arcsec, Some(0.0));
    assert!(profile.colour);
    assert!(profile.filters_nm.contains_key("RGB"));
    assert!(profile.filters_nm.contains_key("Dual band"));
    assert!(!profile.filters_nm.contains_key("L"));
}

#[test]
fn projects_preserve_combined_depth_without_generating_local_frame_quotas() {
    let catalog = decode_projects(
        include_bytes!("fixtures/starfront-projects.json"),
        &source(),
    )
    .unwrap();
    assert_eq!(catalog.projects.len(), 2);
    assert_eq!(catalog.projects[0].kind, Kind::Single);
    assert_eq!(catalog.projects[0].goals_hours["L"], Some(20.0));
    assert_eq!(catalog.projects[0].compatible, Some(false));
    assert_eq!(catalog.projects[0].compatibility_certain, Some(false));
    assert_eq!(catalog.projects[1].goals_hours["H"], Some(10.0));
    assert_eq!(catalog.source, source());
}

#[test]
fn canonical_filters_follow_wire_rules_not_core_aliases() {
    for (input, output) in [
        ("Ha 3nm", "H"),
        ("OIII-6.5nm", "O"),
        ("SII (3 nm)", "S"),
        ("UV/IR cut", "L"),
        ("Red", "R"),
        ("Green", "G"),
        ("Blue", "B"),
        ("luminance", "L"),
        ("RGB", "RGB"),
        ("OSC", "OSC"),
        ("Dual band", "Dual band"),
        ("L-Pro", "L-Pro"),
    ] {
        assert_eq!(fold_filter(input).unwrap(), output);
    }
    assert!(fold_filter("").is_err());
    assert!(fold_filter("Ha\n3nm").is_err());
}

#[test]
fn aliases_cannot_hide_conflicting_bandpasses_or_exposures() {
    let mut body = json!({"profile":{"filters":{"Ha":3,"H":7}}});
    assert_eq!(
        decode_hello_profile(&serde_json::to_vec(&body).unwrap()),
        Err(Error::AmbiguousFilter)
    );
    body["profile"] = json!({"exposures":{"Ha":300,"H":120}});
    assert_eq!(
        decode_hello_profile(&serde_json::to_vec(&body).unwrap()),
        Err(Error::AmbiguousFilter)
    );
    let v = changed(|t| t["filters"][1]["filter"] = json!("Ha"));
    assert_eq!(read(&v), Err(Error::AmbiguousFilter));
}

#[test]
fn unaccepted_inactive_and_unknown_states_never_emit_panel_demand() {
    for state in ["offered", "declined", "complete", "superseded", "future"] {
        let result = read(&changed(|t| t["state"] = json!(state))).unwrap();
        assert!(result.shares[0].demands.is_empty());
        assert!(result.shares[0]
            .review_reasons
            .contains(&ReviewReason::NotAccepted));
    }
    let result = read(&changed(|t| {
        t.as_object_mut().unwrap().remove("state");
    }))
    .unwrap();
    assert_eq!(result.shares[0].state, State::Offered);
    assert!(result.shares[0].demands.is_empty());
}

#[test]
fn stale_or_unnamed_deals_are_review_only() {
    for night in [json!("2026-10-04"), json!(""), Value::Null] {
        let result = read(&changed(|t| t["assignedNight"] = night)).unwrap();
        assert!(result.shares[0].demands.is_empty());
        assert!(result.shares[0]
            .review_reasons
            .contains(&ReviewReason::WrongNight));
    }
    for night in ["2026-02-30", "2026-2-05", "2026-10-05Z", ""] {
        assert_eq!(
            decode_tonight(STARFRONT.as_bytes(), &source(), night),
            Err(Error::InvalidNight)
        );
    }
}

#[test]
fn empty_share_does_not_expand_to_all_cells_or_long_term_goal() {
    let result = read(&changed(|t| t["share"] = json!([]))).unwrap();
    assert!(result.shares[0].demands.is_empty());
    assert!(result.shares[0]
        .review_reasons
        .contains(&ReviewReason::EmptyShare));
    let v = changed(|t| {
        t.as_object_mut().unwrap().remove("visit");
    });
    let result = read(&v).unwrap();
    assert!(result.shares[0].demands.is_empty());
    assert!(result.shares[0]
        .review_reasons
        .contains(&ReviewReason::MissingVisit));
}

#[test]
fn missing_unreadable_or_violated_requirements_require_review() {
    let mut missing = fixture();
    missing
        .as_object_mut()
        .unwrap()
        .remove("requirementsByProject");
    missing.as_object_mut().unwrap().remove("requirements");
    let task = read(&missing).unwrap().shares.remove(0);
    assert_eq!(task.requirements, None);
    assert!(task.demands.is_empty());
    for (key, value, reason) in [
        (
            "minAltitude",
            json!("unknown"),
            ReviewReason::UnresolvedRequirements,
        ),
        (
            "minFramesPerVisit",
            Value::Null,
            ReviewReason::UnresolvedRequirements,
        ),
        (
            "minFramesPerVisit",
            json!(12),
            ReviewReason::BelowMinimumVisit,
        ),
        (
            "maxExposure",
            json!(120),
            ReviewReason::ExposureOutsideRequirements,
        ),
        (
            "filters",
            json!({"H":7}),
            ReviewReason::FilterOutsideRequirements,
        ),
    ] {
        let mut v = fixture();
        v["requirementsByProject"]["000000000002"][key] = value;
        let task = read(&v).unwrap().shares.remove(0);
        assert!(task.demands.is_empty(), "{key}");
        assert!(task.review_reasons.contains(&reason), "{key}");
    }
    let mut v = fixture();
    v["requirementsByProject"]["000000000002"]["maxMoonIllumination"] = json!(2);
    assert_eq!(read(&v), Err(Error::InvalidValue));
}

#[test]
fn legacy_single_task_and_first_project_requirements_are_supported() {
    let mut v = fixture();
    v.as_object_mut().unwrap().remove("tasks");
    v.as_object_mut().unwrap().remove("requirementsByProject");
    assert_eq!(read(&v).unwrap(), read(&fixture()).unwrap());
    v["tasks"] = json!([]);
    assert_eq!(read(&v), Err(Error::AmbiguousTask));
}

#[test]
fn inconsistent_alias_duplicate_tasks_and_invalid_panels_are_refused() {
    let mut v = fixture();
    v["task"] = Value::Null;
    assert_eq!(read(&v), Err(Error::AmbiguousTask));
    let mut v = fixture();
    v["task"]["cells"][0]["ra"] = json!(11);
    assert_eq!(read(&v), Err(Error::AmbiguousTask));
    let mut v = fixture();
    let task = v["task"].clone();
    v["tasks"].as_array_mut().unwrap().push(task);
    assert_eq!(read(&v), Err(Error::AmbiguousTask));
    for panels in [json!([0, 0]), json!([9]), json!([-1]), json!([0.5])] {
        assert!(read(&changed(|t| t["share"] = panels)).is_err());
    }
    assert!(read(&changed(|t| t["cells"][1]["column"] = json!(0))).is_err());
}

#[test]
fn profile_clock_windows_and_field_dimensions_are_not_silently_erased() {
    let mut v: Value = serde_json::from_str(include_str!("fixtures/starfront-hello.json")).unwrap();
    for clock in ["24:00", "9:030", "unknown"] {
        v["profile"]["windowFrom"] = json!(clock);
        assert_eq!(
            decode_hello_profile(&serde_json::to_vec(&v).unwrap()),
            Err(Error::InvalidValue)
        );
    }
    v["profile"]["windowFrom"] = json!("");
    assert_eq!(
        decode_hello_profile(&serde_json::to_vec(&v).unwrap())
            .unwrap()
            .window_from,
        None
    );
    v["profile"]["field"] = json!([10, 20, 30]);
    assert!(decode_hello_profile(&serde_json::to_vec(&v).unwrap()).is_err());
    v["profile"]["field"] = json!(["unknown", 20]);
    assert_eq!(
        decode_hello_profile(&serde_json::to_vec(&v).unwrap())
            .unwrap()
            .reported_field_degrees,
        None
    );
}

#[test]
fn total_normalized_demands_are_bounded_across_shares() {
    let mut v = fixture();
    v["task"]["share"] = json!([0, 1, 2, 3, 4, 5, 6, 7, 8]);
    v["tasks"] = Value::Array(
        (0..MAX_TASKS)
            .map(|index| {
                let mut task = v["task"].clone();
                task["id"] = json!(format!("{:012x}", index + 4));
                task
            })
            .collect(),
    );
    assert_eq!(read(&v), Err(Error::LimitExceeded));
}

#[test]
fn multi_project_requirements_do_not_fall_back_to_another_project() {
    let mut v = fixture();
    let mut second = v["task"].clone();
    second["id"] = json!("000000000006");
    second["project"] = json!("000000000003");
    v["tasks"].as_array_mut().unwrap().push(second);
    let result = read(&v).unwrap();
    assert_eq!(result.shares[0].demands.len(), 6);
    assert!(result.shares[1].demands.is_empty());
    assert!(result.shares[1]
        .review_reasons
        .contains(&ReviewReason::MissingRequirements));
}

#[test]
fn degree_coordinates_are_not_ra_hours_and_are_bounded_at_wrap_and_poles() {
    let v = changed(|t| {
        t["region"]["ra"] = json!(15);
        t["region"]["dec"] = json!(-90);
        t["region"]["rotation"] = json!(-5);
    });
    let task = read(&v).unwrap().shares.remove(0);
    assert_eq!(task.region.icrs_ra_mas, 54_000_000);
    assert_eq!(task.region.icrs_dec_mas, -324_000_000);
    assert_eq!(task.region.position_angle_mas, 1_278_000_000);
    let v = changed(|t| t["region"]["ra"] = json!(359.99999999));
    assert_eq!(read(&v).unwrap().shares[0].region.icrs_ra_mas, 0);
    for (key, value) in [
        ("ra", json!(360)),
        ("ra", json!(-0.1)),
        ("dec", json!(90.1)),
        ("width", json!(0)),
        ("height", json!(0.000000001)),
        ("rotation", json!("unknown")),
    ] {
        assert!(
            read(&changed(|t| t["region"][key] = value)).is_err(),
            "{key}"
        );
    }
}

#[test]
fn geometry_fingerprint_changes_for_retile_not_version_or_unknown_fields() {
    let original = read(&fixture()).unwrap().shares.remove(0);
    let v = changed(|t| {
        t["version"] = json!(3);
        t["future"] = json!({"enabled":true});
        t["share"] = json!([5, 4, 3]);
    });
    assert_eq!(
        read(&v).unwrap().shares[0].geometry_digest,
        original.geometry_digest
    );
    let v = changed(|t| t["cells"][0]["rotation"] = json!(40));
    let changed = read(&v).unwrap().shares.remove(0);
    assert_eq!(changed.task_id, original.task_id);
    assert_eq!(changed.version, original.version);
    assert_ne!(changed.geometry_digest, original.geometry_digest);
}

#[test]
fn unknown_fields_are_ignored_but_duplicate_json_keys_are_not() {
    let mut v = changed(|t| t["future"] = json!({"someday":[1,2,3]}));
    v["newFeature"] = json!(true);
    assert_eq!(read(&v).unwrap(), read(&fixture()).unwrap());
    let bytes = br#"{"protocol":1,"protocol":2,"task":null,"tasks":[]}"#;
    assert_eq!(
        decode_tonight(bytes, &source(), NIGHT),
        Err(Error::InvalidJson)
    );
    assert_eq!(
        decode_health(br#"{"ok":true,"protocol":1,"version":"x","time":0,"future":{"x":1,"x":2}}"#),
        Err(Error::InvalidJson)
    );
}

#[test]
fn unknown_numeric_limits_never_masquerade_as_no_limit() {
    for value in [
        json!("NaN"),
        json!("Infinity"),
        json!(""),
        json!(true),
        json!([]),
    ] {
        let mut v = fixture();
        v["requirementsByProject"]["000000000002"]["maxHfr"] = value;
        let task = read(&v).unwrap().shares.remove(0);
        assert!(task.demands.is_empty());
        assert!(task
            .requirements
            .unwrap()
            .unresolved_fields
            .contains(&"maxHfr".into()));
    }
    let mut v = fixture();
    v["requirementsByProject"]["000000000002"]["minAltitude"] = json!("30");
    assert_eq!(read(&v).unwrap(), read(&fixture()).unwrap());
}

#[test]
fn time_exposure_and_frame_conversions_do_not_round_away_constraints() {
    for value in [
        json!(300.0001),
        json!(-1),
        json!(1e20),
        json!("unknown"),
        json!(0),
    ] {
        assert!(read(&changed(|t| t["filters"][1]["exposure"] = value)).is_err());
    }
    for value in [json!(11.5), json!(0), json!(-1), json!(4_294_967_296u64)] {
        assert!(read(&changed(|t| t["visit"]["frames"]["O"] = value)).is_err());
    }
    let mut v = fixture();
    v["protocol"] = json!(2);
    assert_eq!(read(&v), Err(Error::UnsupportedProtocol));
}

#[test]
fn structural_size_and_depth_limits_are_enforced_without_echoing_inputs() {
    assert_eq!(
        decode_tonight(&vec![b' '; MAX_BODY_BYTES + 1], &source(), NIGHT),
        Err(Error::TooLarge)
    );
    let v = changed(|t| {
        let cell = t["cells"][0].clone();
        t["cells"] = Value::Array(vec![cell; MAX_CELLS + 1]);
    });
    assert_eq!(read(&v), Err(Error::LimitExceeded));
    let mut nested = json!(true);
    for _ in 0..40 {
        nested = json!({"next":nested});
    }
    let mut v = fixture();
    v["future"] = nested;
    assert_eq!(read(&v), Err(Error::LimitExceeded));
    let error = decode_health(br#"{"token":"super-secret"}"#).unwrap_err();
    assert!(!error.to_string().contains("super-secret"));
    assert!(!format!("{error:?}").contains("token"));
}

#[test]
fn single_targets_and_future_kinds_are_never_implicitly_retiled() {
    let v = changed(|t| {
        t["kind"] = json!("single");
        t["cells"] = json!([t["cells"][4].clone()]);
        t["share"] = json!([0]);
    });
    assert_eq!(read(&v).unwrap().shares[0].demands.len(), 1);
    let result = read(&changed(|t| t["kind"] = json!("future"))).unwrap();
    assert!(result.shares[0].demands.is_empty());
    assert!(result.shares[0]
        .review_reasons
        .contains(&ReviewReason::UnknownKind));
}
