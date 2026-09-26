use psf_guard_director_core::preparation::{Estimates, Next, Operation};
use psf_guard_director_core::program::*;
use psf_guard_director_core::{Decision, Request, CONTRACT_VERSION, MAX_REQUEST_BYTES};
use serde_json::{json, Value};

fn request() -> Request {
    serde_json::from_value(json!({
        "contract_version": CONTRACT_VERSION,
        "assignment": {
            "id":"allocation", "revision":9007199254740993_u64, "rig_id":"rig", "configuration_id":"config",
            "valid_from_ms":1000, "expires_at_ms":100000,
            "goals":[{"id":"goal","priority":1,"requested":3,"accepted":0,"pending":0,"attempts_remaining":5,
            "exposure_ms":1000,"overhead_ms":0,"eligible_windows":[{"start_ms":1000,"end_ms":100000}],"transits":null}]
        },
        "state":{"rig_id":"rig","configuration_id":"config","now_ms":2000,"conditions_valid_until_ms":90000,
            "safety":"safe","at_boundary":true,"operator_stop":false,"meridian_exclusion":{"before_ms":0,"after_ms":0}}
    })).unwrap()
}

fn program() -> Program {
    Program {
        schema_version: PROGRAM_VERSION,
        assignment: request().assignment,
        configuration: Configuration {
            rig_id: "rig".into(),
            id: "config".into(),
            camera_id: "camera".into(),
            filter_wheel_id: Some("wheel".into()),
            filters: vec![Filter {
                id: "L".into(),
                position: Some(2),
            }],
            binning_modes: vec![Binning { x: 1, y: 1 }, Binning { x: 2, y: 2 }],
            readout_modes: vec![0, 2],
            gain: Control::Range {
                minimum: 0,
                maximum: 100,
            },
            offset: Control::Values {
                values: vec![0, 20],
            },
            exposure_min_ms: 1,
            exposure_max_ms: 1000000,
            enable_slew_center: true,
            dither_every: 3,
        },
        targets: vec![Target {
            id: "target".into(),
            name: "M42".into(),
            icrs_ra_mas: 83 * MAS_PER_DEGREE,
            icrs_dec_mas: -5 * MAS_PER_DEGREE as i32,
            position_angle_mas: Some(90 * MAS_PER_DEGREE),
        }],
        recipes: vec![Recipe {
            id: "recipe".into(),
            exposure_ms: 1000,
            filter_id: "L".into(),
            binning: Binning { x: 1, y: 1 },
            gain: Some(40),
            offset: Some(20),
            readout_mode: 2,
            dither_override: Some(0),
        }],
        bindings: vec![Binding {
            goal_id: "goal".into(),
            target_id: "target".into(),
            recipe_id: "recipe".into(),
        }],
    }
}

fn local() -> LocalState {
    LocalState {
        configuration: program().configuration,
        previous_pointing: None,
        mount_parked: true,
        rotator_connected: true,
        filter_exposures_since_dither: 5,
    }
}

fn bound(program: Program) -> Result<BoundProgram, Error> {
    BoundProgram::new(program, &request().state)
}

#[test]
fn exact_roundtrip_resolves_recipe_without_a_second_scheduler() {
    let program = program();
    let bytes = serde_json::to_vec(&program).unwrap();
    let bound = BoundProgram::from_json(&bytes, &request().state).unwrap();
    assert_eq!(bound.snapshot(), &program);
    let resolved = bound.resolve("goal").unwrap();
    assert_eq!(resolved.target.icrs_ra_mas, 298800000);
    assert_eq!(resolved.recipe.exposure_ms, resolved.goal.exposure_ms);
    assert_eq!(resolved.configuration.filters[0].position, Some(2));
    assert_eq!(resolved.recipe.gain, Some(40));
    assert_eq!(resolved.recipe.readout_mode, 2);
    assert_eq!(bound.snapshot().assignment.revision, 9007199254740993);
    assert!(matches!(bound.resolve("missing"), Err(Error::UnknownGoal)));
    let mut preparation = bound
        .preparation(
            "prep".into(),
            &request(),
            "goal",
            local(),
            Estimates::default(),
        )
        .unwrap();
    let Next::Run(command) = preparation.next(&request()).unwrap() else {
        panic!()
    };
    assert_eq!(command.target_id, "target");
    assert_eq!(command.recipe_id, "recipe");
    assert_eq!(command.operation, Operation::Unpark);
    assert_eq!(preparation.context().filter_id, "L");
    assert_eq!(preparation.context().readout_mode, 2);
    assert_eq!(preparation.context().dither_override, Some(0));
}

#[test]
fn source_mutation_cannot_change_a_validated_program() {
    let mut original = program();
    let bound = bound(original.clone()).unwrap();
    original.recipes[0].gain = Some(99);
    original.targets[0].icrs_ra_mas = 0;
    assert_eq!(bound.resolve("goal").unwrap().recipe.gain, Some(40));
    assert_eq!(bound.resolve("goal").unwrap().target.icrs_ra_mas, 298800000);
}

#[test]
fn long_and_short_goals_keep_distinct_recipe_identity() {
    let mut program = program();
    let mut long = program.assignment.goals[0].clone();
    long.id = "long".into();
    long.exposure_ms = 60000;
    program.assignment.goals.push(long);
    let mut recipe = program.recipes[0].clone();
    recipe.id = "long-recipe".into();
    recipe.exposure_ms = 60000;
    program.recipes.push(recipe);
    program.bindings.push(Binding {
        goal_id: "long".into(),
        target_id: "target".into(),
        recipe_id: "long-recipe".into(),
    });
    let bound = bound(program).unwrap();
    assert_eq!(bound.resolve("goal").unwrap().recipe.exposure_ms, 1000);
    assert_eq!(bound.resolve("long").unwrap().recipe.exposure_ms, 60000);
}

#[test]
fn complete_binding_is_required_without_name_or_coordinate_matching() {
    for fault in 0..8 {
        let mut program = program();
        match fault {
            0 => program.bindings.clear(),
            1 => program.bindings[0].goal_id = "other".into(),
            2 => program.bindings[0].target_id = "M42".into(),
            3 => program.bindings[0].recipe_id = "L".into(),
            4 => program.recipes[0].exposure_ms = 2000,
            5 => program.bindings.push(program.bindings[0].clone()),
            6 => program.targets.push(program.targets[0].clone()),
            _ => program.recipes.push(program.recipes[0].clone()),
        }
        assert!(bound(program).is_err(), "fault {fault}");
    }
}

#[test]
fn duplicate_ids_and_unused_objects_are_rejected_even_when_counts_fit() {
    for fault in 0..4 {
        let mut program = program();
        let mut goal = program.assignment.goals[0].clone();
        goal.id = "second".into();
        program.assignment.goals.push(goal);
        program.bindings.push(Binding {
            goal_id: "second".into(),
            target_id: "target".into(),
            recipe_id: "recipe".into(),
        });
        match fault {
            0 => program.targets.push(program.targets[0].clone()),
            1 => program.recipes.push(program.recipes[0].clone()),
            2 => {
                let mut target = program.targets[0].clone();
                target.id = "unused".into();
                program.targets.push(target);
            }
            _ => {
                let mut recipe = program.recipes[0].clone();
                recipe.id = "unused".into();
                program.recipes.push(recipe);
            }
        }
        assert!(bound(program).is_err(), "fault {fault}");
    }
}

#[test]
fn coordinate_bounds_and_display_name_are_explicit() {
    for fault in 0..6 {
        let mut program = program();
        match fault {
            0 => program.targets[0].icrs_ra_mas = 360 * MAS_PER_DEGREE,
            1 => program.targets[0].icrs_dec_mas = 90 * MAS_PER_DEGREE as i32 + 1,
            2 => program.targets[0].icrs_dec_mas = -90 * MAS_PER_DEGREE as i32 - 1,
            3 => program.targets[0].position_angle_mas = Some(360 * MAS_PER_DEGREE),
            4 => program.targets[0].name = "   ".into(),
            _ => program.targets[0].name = "M42\ncommand".into(),
        }
        assert!(
            matches!(bound(program), Err(Error::InvalidTarget)),
            "fault {fault}"
        );
    }
    let mut poles = program();
    poles.targets[0].icrs_ra_mas = 0;
    poles.targets[0].icrs_dec_mas = -90 * MAS_PER_DEGREE as i32;
    assert!(bound(poles).is_ok());
}

#[test]
fn unsupported_settings_never_fall_back_to_current_device_state() {
    for fault in 0..9 {
        let mut program = program();
        match fault {
            0 => program.recipes[0].gain = None,
            1 => program.recipes[0].gain = Some(101),
            2 => program.recipes[0].offset = Some(10),
            3 => program.recipes[0].offset = None,
            4 => program.recipes[0].readout_mode = 1,
            5 => program.recipes[0].binning = Binning { x: 1, y: 2 },
            6 => program.recipes[0].filter_id = "filter-name".into(),
            7 => program.recipes[0].exposure_ms = 0,
            _ => program.recipes[0].exposure_ms = 1000001,
        }
        assert!(
            matches!(bound(program), Err(Error::InvalidRecipe)),
            "fault {fault}"
        );
    }
    let mut unsupported = program();
    unsupported.configuration.gain = Control::Unsupported;
    assert!(bound(unsupported.clone()).is_err());
    unsupported.recipes[0].gain = None;
    assert!(bound(unsupported).is_ok());
}

#[test]
fn ambiguous_or_invalid_capabilities_are_rejected() {
    for fault in 0..14 {
        let mut program = program();
        let config = &mut program.configuration;
        match fault {
            0 => config.binning_modes.push(config.binning_modes[0]),
            1 => config.binning_modes[0].x = 0,
            2 => config.readout_modes.push(0),
            3 => config.readout_modes.clear(),
            4 => config.readout_modes[0] = -1,
            5 => config.filters[0].position = None,
            6 => config.filters[0].position = Some(-1),
            7 => config.filters.push(Filter {
                id: "other".into(),
                position: Some(2),
            }),
            8 => config.filters.push(Filter {
                id: "L".into(),
                position: Some(3),
            }),
            9 => config.exposure_min_ms = 0,
            10 => {
                config.gain = Control::Range {
                    minimum: 100,
                    maximum: 0,
                }
            }
            11 => config.gain = Control::Values { values: vec![1, 1] },
            12 => config.gain = Control::Values { values: vec![-1] },
            _ => config.gain = Control::Values { values: vec![] },
        }
        assert!(
            matches!(bound(program), Err(Error::InvalidConfiguration)),
            "fault {fault}"
        );
    }
}

#[test]
fn fixed_filter_is_explicit_and_cannot_have_multiple_choices() {
    let mut program = program();
    program.configuration.filter_wheel_id = None;
    program.configuration.filters[0].position = None;
    assert!(bound(program.clone()).is_ok());
    program.configuration.filters.push(Filter {
        id: "R".into(),
        position: None,
    });
    assert!(matches!(bound(program), Err(Error::InvalidConfiguration)));
}

#[test]
fn rig_and_configuration_must_match_allocation_and_local_state() {
    for fault in 0..5 {
        let mut program = program();
        let mut state = request().state;
        match fault {
            0 => program.configuration.rig_id = "other".into(),
            1 => program.configuration.id = "other".into(),
            2 => state.rig_id = "other".into(),
            3 => state.configuration_id = "other".into(),
            _ => program.assignment.configuration_id = "other".into(),
        }
        assert!(matches!(
            BoundProgram::new(program, &state),
            Err(Error::ConfigurationMismatch)
        ));
    }
}

#[test]
fn optional_fields_are_required_and_unknown_or_duplicate_fields_fail() {
    for path in [
        "/targets/0/position_angle_mas",
        "/recipes/0/gain",
        "/recipes/0/offset",
        "/recipes/0/dither_override",
        "/configuration/filter_wheel_id",
        "/configuration/filters/0/position",
    ] {
        let mut value = serde_json::to_value(program()).unwrap();
        let (parent, field) = path.rsplit_once('/').unwrap();
        value
            .pointer_mut(parent)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .remove(field);
        assert!(
            matches!(
                BoundProgram::from_json(&serde_json::to_vec(&value).unwrap(), &request().state),
                Err(Error::InvalidJson)
            ),
            "{path}"
        );
    }
    let mut value = serde_json::to_value(program()).unwrap();
    value["recipes"][0]["extra"] = Value::Bool(true);
    assert!(matches!(
        BoundProgram::from_json(&serde_json::to_vec(&value).unwrap(), &request().state),
        Err(Error::InvalidJson)
    ));
    let json = serde_json::to_string(&program()).unwrap().replacen(
        "\"schema_version\":1",
        "\"schema_version\":1,\"schema_version\":1",
        1,
    );
    assert!(matches!(
        BoundProgram::from_json(json.as_bytes(), &request().state),
        Err(Error::InvalidJson)
    ));
}

#[test]
fn version_size_and_unknown_goals_fail_closed() {
    let mut program = program();
    program.schema_version += 1;
    assert!(matches!(bound(program), Err(Error::UnsupportedVersion)));
    assert!(matches!(
        BoundProgram::from_json(&vec![b' '; MAX_REQUEST_BYTES + 1], &request().state),
        Err(Error::TooLarge)
    ));
    let mut program = self::program();
    program.targets[0].name = "x".repeat(MAX_REQUEST_BYTES);
    assert!(matches!(bound(program), Err(Error::TooLarge)));
}

#[test]
fn preparation_uses_core_selection_and_fresh_boundaries() {
    let bound = bound(program()).unwrap();
    let mut request = request();
    request.state.operator_stop = true;
    assert!(matches!(
        bound.preparation(
            "prep".into(),
            &request,
            "goal",
            local(),
            Estimates::default()
        ),
        Err(Error::Preparation(_))
    ));
    request.state.operator_stop = false;
    let mut preparation = bound
        .preparation(
            "prep".into(),
            &request,
            "goal",
            local(),
            Estimates::default(),
        )
        .unwrap();
    request.state.operator_stop = true;
    assert!(matches!(
        preparation.next(&request).unwrap(),
        Next::Decision(Decision::Stop { .. })
    ));
}

#[test]
fn rotation_is_explicit_not_implied_by_connected_equipment() {
    let mut program = program();
    let mut local = local();
    local.rotator_connected = false;
    assert!(matches!(
        bound(program.clone()).unwrap().preparation(
            "prep".into(),
            &request(),
            "goal",
            local.clone(),
            Estimates::default()
        ),
        Err(Error::RotationUnavailable)
    ));
    program.targets[0].position_angle_mas = None;
    local.mount_parked = false;
    local.rotator_connected = true;
    let mut preparation = bound(program)
        .unwrap()
        .preparation(
            "prep".into(),
            &request(),
            "goal",
            local,
            Estimates::default(),
        )
        .unwrap();
    assert!(
        matches!(preparation.next(&request()).unwrap(), Next::Run(command) if command.operation == Operation::Center { rotate:false })
    );
}

#[test]
fn progress_projection_cannot_change_recipe_duration_windows_or_expand_budget() {
    let bound = bound(program()).unwrap();
    for fault in 0..5 {
        let mut request = request();
        match fault {
            0 => request.assignment.goals[0].exposure_ms = 2,
            1 => request.assignment.goals[0].attempts_remaining += 1,
            2 => request.assignment.goals[0].eligible_windows[0].start_ms = 0,
            3 => request.assignment.revision += 1,
            _ => request.assignment.goals[0].id = "other".into(),
        }
        assert!(
            matches!(
                bound.preparation(
                    "prep".into(),
                    &request,
                    "goal",
                    local(),
                    Estimates::default()
                ),
                Err(Error::ConfigurationMismatch)
            ),
            "fault {fault}"
        );
    }
    let mut request = request();
    request.assignment.goals[0].pending = 1;
    request.assignment.goals[0].attempts_remaining -= 1;
    assert!(bound
        .preparation(
            "prep".into(),
            &request,
            "goal",
            local(),
            Estimates::default()
        )
        .is_ok());
}

#[test]
fn same_target_id_does_not_reuse_stale_framing_or_configuration() {
    for fault in 0..4 {
        let program = program();
        let mut previous = PointingContext {
            configuration_id: "config".into(),
            target: program.targets[0].clone(),
        };
        match fault {
            0 => previous.target.icrs_ra_mas += 1,
            1 => previous.target.position_angle_mas = Some(0),
            2 => previous.configuration_id = "previous-config".into(),
            _ => {}
        }
        let mut local = local();
        local.previous_pointing = Some(previous);
        local.mount_parked = false;
        let mut preparation = bound(program)
            .unwrap()
            .preparation(
                "prep".into(),
                &request(),
                "goal",
                local,
                Estimates::default(),
            )
            .unwrap();
        let Next::Run(command) = preparation.next(&request()).unwrap() else {
            panic!()
        };
        if fault < 3 {
            assert_eq!(command.operation, Operation::Center { rotate: true });
        } else {
            assert_eq!(
                command.operation,
                Operation::SwitchFilter {
                    filter_id: "L".into()
                }
            );
        }
    }
}

#[test]
fn matching_configuration_id_cannot_hide_changed_equipment() {
    let bound = bound(program()).unwrap();
    for fault in 0..4 {
        let mut local = local();
        match fault {
            0 => local.configuration.camera_id = "replacement-camera".into(),
            1 => local.configuration.filters[0].position = Some(1),
            2 => local.configuration.readout_modes = vec![0],
            _ => local.configuration.enable_slew_center = false,
        }
        assert!(matches!(
            bound.preparation(
                "prep".into(),
                &request(),
                "goal",
                local,
                Estimates::default()
            ),
            Err(Error::ConfigurationMismatch)
        ));
    }
}
