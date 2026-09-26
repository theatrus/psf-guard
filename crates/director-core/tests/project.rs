use psf_guard_director_core::{program::*, project::*, MAX_REQUEST_BYTES};

fn configuration(rig: &str) -> Configuration {
    Configuration {
        rig_id: rig.into(),
        id: format!("nina-{rig}"),
        camera_id: format!("camera-{rig}"),
        filter_wheel_id: None,
        filters: vec![Filter {
            id: "native-L".into(),
            position: None,
        }],
        binning_modes: vec![Binning { x: 1, y: 1 }],
        readout_modes: vec![0],
        gain: Control::Unsupported {},
        offset: Control::Unsupported {},
        exposure_min_ms: 100,
        exposure_max_ms: 600_000,
        enable_slew_center: true,
        dither_every: 3,
    }
}

fn target(id: &str) -> Target {
    Target {
        id: id.into(),
        name: "M42".into(),
        icrs_ra_mas: 83 * MAS_PER_DEGREE,
        icrs_dec_mas: -5 * MAS_PER_DEGREE as i32,
        position_angle_mas: None,
    }
}

fn project() -> Project {
    let mut objectives = vec![];
    let mut contributions = vec![];
    for (purpose, duration, frames) in [
        ("faint_detail", 300_000, 60),
        ("unsaturated_stars", 1000, 20),
    ] {
        objectives.push(Objective {
            id: purpose.into(),
            purpose: purpose.into(),
            bandpass_id: "luminance".into(),
            target: target("M42-center"),
            priority: 1,
        });
        for rig in ["wide", "narrow"] {
            let mut framing = target(&format!("{rig}-panel"));
            if rig == "narrow" {
                framing.icrs_ra_mas += MAS_PER_DEGREE;
            }
            contributions.push(Contribution {
                id: format!("{rig}-{purpose}"),
                objective_id: purpose.into(),
                rig_id: rig.into(),
                setup_id: format!("setup-{rig}"),
                configuration_id: configuration(rig).id,
                framing,
                recipe: Recipe {
                    id: purpose.into(),
                    exposure_ms: duration,
                    filter_id: "native-L".into(),
                    binning: Binning { x: 1, y: 1 },
                    gain: None,
                    offset: None,
                    readout_mode: 0,
                    dither_override: None,
                },
                required_accepted_frames: frames,
            });
        }
    }
    Project {
        schema_version: PROJECT_VERSION,
        project_id: "global-M42".into(),
        id: "intent-1".into(),
        objectives,
        contributions,
    }
}

#[test]
fn two_rigs_and_short_long_intents_roundtrip_without_coalescing() {
    let source = project();
    let bound = BoundProject::from_json(&serde_json::to_vec(&source).unwrap()).unwrap();
    assert_eq!(bound.snapshot(), &source);
    assert_eq!(bound.snapshot().objectives.len(), 2);
    assert_eq!(bound.snapshot().contributions.len(), 4);
    let long = bound
        .resolve("wide-faint_detail", "setup-wide", &configuration("wide"))
        .unwrap();
    let short = bound
        .resolve(
            "wide-unsaturated_stars",
            "setup-wide",
            &configuration("wide"),
        )
        .unwrap();
    let narrow = bound
        .resolve(
            "narrow-faint_detail",
            "setup-narrow",
            &configuration("narrow"),
        )
        .unwrap();
    assert_eq!(long.objective.bandpass_id, short.objective.bandpass_id);
    assert_ne!(long.objective.id, short.objective.id);
    assert_eq!(long.contribution.recipe.exposure_ms, 300_000);
    assert_eq!(short.contribution.recipe.exposure_ms, 1000);
    assert_eq!(short.contribution.required_accepted_frames, 20);
    assert_ne!(long.contribution.id, narrow.contribution.id);
    assert_ne!(long.contribution.framing, narrow.contribution.framing);
}

#[test]
fn capability_mapping_and_setup_identity_are_explicit() {
    let bound = BoundProject::new(project()).unwrap();
    assert!(matches!(
        bound.resolve("wide-faint_detail", "setup-narrow", &configuration("wide")),
        Err(psf_guard_director_core::project::Error::SetupMismatch)
    ));
    assert!(matches!(
        bound.resolve("wide-faint_detail", "setup-wide", &configuration("narrow")),
        Err(psf_guard_director_core::project::Error::SetupMismatch)
    ));
    assert!(bound
        .resolve("M42", "setup-wide", &configuration("wide"))
        .is_err());
    for fault in 0..6 {
        let mut config = configuration("wide");
        match fault {
            0 => config.id = "changed".into(),
            1 => config.filters[0].id = "L".into(),
            2 => config.exposure_max_ms = 2000,
            3 => config.binning_modes[0].x = 2,
            4 => config.readout_modes[0] = 1,
            _ => {
                config.gain = Control::Range {
                    minimum: 0,
                    maximum: 100,
                }
            }
        }
        assert!(
            bound
                .resolve("wide-faint_detail", "setup-wide", &config)
                .is_err(),
            "fault {fault}"
        );
    }
}

#[test]
fn source_mutation_cannot_change_bound_intent() {
    let mut source = project();
    let bound = BoundProject::new(source.clone()).unwrap();
    source.contributions[0].recipe.exposure_ms = 1000;
    source.objectives[0].priority = 99;
    assert_eq!(
        bound.snapshot().contributions[0].recipe.exposure_ms,
        300_000
    );
    assert_eq!(bound.snapshot().objectives[0].priority, 1);
}

#[test]
fn duplicate_missing_and_unused_references_are_rejected() {
    for fault in 0..9 {
        let mut value = project();
        match fault {
            0 => value.objectives.push(value.objectives[0].clone()),
            1 => value.contributions.push(value.contributions[0].clone()),
            2 => value.contributions[0].objective_id = "M42".into(),
            3 => value
                .contributions
                .retain(|c| c.objective_id != "faint_detail"),
            4 => value.objectives.clear(),
            5 => value.contributions.clear(),
            6 => value.project_id.clear(),
            7 => value.id.clear(),
            _ => value.schema_version += 1,
        }
        assert!(BoundProject::new(value).is_err(), "fault {fault}");
    }
}

#[test]
fn same_ids_cannot_name_different_framing_or_recipes() {
    let mut value = project();
    value.contributions[0].framing.icrs_ra_mas += 1;
    assert!(matches!(
        BoundProject::new(value),
        Err(psf_guard_director_core::project::Error::ConflictingTarget)
    ));
    let mut value = project();
    value.contributions[2].recipe.id = value.contributions[0].recipe.id.clone();
    assert!(matches!(
        BoundProject::new(value),
        Err(psf_guard_director_core::project::Error::ConflictingRecipe)
    ));
    // Native recipe IDs are configuration-scoped, not globally interchangeable.
    let mut value = project();
    value.contributions[1].recipe.exposure_ms = 60_000;
    assert!(BoundProject::new(value).is_ok());
}

#[test]
fn invalid_science_inputs_and_unbounded_payloads_are_refused() {
    for fault in 0..10 {
        let mut value = project();
        match fault {
            0 => value.objectives[0].purpose = " ".into(),
            1 => value.objectives[0].target.icrs_ra_mas = 360 * MAS_PER_DEGREE,
            2 => value.contributions[0].required_accepted_frames = 0,
            3 => value.contributions[0].recipe.binning.x = 0,
            4 => value.contributions[0].recipe.exposure_ms = 0,
            5 => value.contributions[0].recipe.readout_mode = -1,
            6 => value.contributions[0].recipe.gain = Some(-1),
            7 => value.contributions[0].recipe.filter_id.clear(),
            8 => value.contributions[0].setup_id.clear(),
            _ => value.contributions[0].framing.position_angle_mas = Some(360 * MAS_PER_DEGREE),
        }
        assert!(BoundProject::new(value).is_err(), "fault {fault}");
    }
    assert!(matches!(
        BoundProject::from_json(&vec![b' '; MAX_REQUEST_BYTES + 1]),
        Err(psf_guard_director_core::project::Error::TooLarge)
    ));
    let mut value = serde_json::to_value(project()).unwrap();
    value["authorize_acquisition"] = true.into();
    assert!(BoundProject::from_json(&serde_json::to_vec(&value).unwrap()).is_err());
}

#[test]
fn ordering_and_duplicate_display_names_do_not_choose_identity() {
    let original = BoundProject::new(project()).unwrap();
    let mut reordered = project();
    reordered.objectives.reverse();
    reordered.contributions.reverse();
    let reordered = BoundProject::new(reordered).unwrap();
    for contribution in &original.snapshot().contributions {
        let a = original
            .resolve(
                &contribution.id,
                &contribution.setup_id,
                &configuration(&contribution.rig_id),
            )
            .unwrap();
        let b = reordered
            .resolve(
                &contribution.id,
                &contribution.setup_id,
                &configuration(&contribution.rig_id),
            )
            .unwrap();
        assert_eq!(a.objective, b.objective);
        assert_eq!(a.contribution, b.contribution);
        assert_eq!(a.objective.target.name, "M42");
    }
}

#[test]
fn immutable_setup_and_bandpass_mapping_cannot_contradict_themselves() {
    let mut value = project();
    value.contributions[1].setup_id = value.contributions[0].setup_id.clone();
    assert!(matches!(
        BoundProject::new(value),
        Err(psf_guard_director_core::project::Error::ConflictingSetup)
    ));
    let mut value = project();
    value.objectives[1].bandpass_id = "hydrogen-alpha".into();
    assert!(matches!(
        BoundProject::new(value),
        Err(psf_guard_director_core::project::Error::ConflictingBandpass)
    ));
    let mut value = project();
    // Geometry can change without changing the native equipment fingerprint.
    value.contributions[2].setup_id = "wide-new-horizon".into();
    assert!(BoundProject::new(value).is_ok());
}

#[test]
fn counts_and_nested_wire_fields_are_bounded_and_strict() {
    let mut value = project();
    let template = value.contributions[0].clone();
    for n in 0..253 {
        let mut contribution = template.clone();
        contribution.id = format!("extra-{n}");
        value.contributions.push(contribution);
    }
    assert!(matches!(
        BoundProject::new(value),
        Err(psf_guard_director_core::project::Error::InvalidProject)
    ));
    let mut value = project();
    value.objectives[0].purpose = "x".repeat(MAX_REQUEST_BYTES);
    assert!(matches!(
        BoundProject::new(value),
        Err(psf_guard_director_core::project::Error::TooLarge)
    ));
    for fault in 0..3 {
        let mut value = serde_json::to_value(project()).unwrap();
        match fault {
            0 => {
                value["contributions"][0]["accepted"] = 100.into();
            }
            1 => {
                value["contributions"][0]["recipe"]
                    .as_object_mut()
                    .unwrap()
                    .remove("gain");
            }
            _ => {
                value["objectives"][0]["target"]["icrs_dec_mas"] =
                    (-91_i64 * i64::from(MAS_PER_DEGREE)).into();
            }
        }
        assert!(
            BoundProject::from_json(&serde_json::to_vec(&value).unwrap()).is_err(),
            "fault {fault}"
        );
    }
}
