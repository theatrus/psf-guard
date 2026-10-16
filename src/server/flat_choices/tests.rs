//! Flat group choices through the server: the gate, and what a write does.

use super::*;
use crate::calibration::flat_groups::overrides::{GroupChoice, GroupOverrideRow};

/// A catalog on disk with one light and one recorded split, and a state
/// whose database points at it.
fn fixture(dir: &std::path::Path) -> (Arc<AppState>, Arc<DatabaseContext>) {
    let path = dir.join("catalog.sqlite");
    let conn = Connection::open(&path).unwrap();
    crate::ts_schema::apply_schema(&conn).unwrap();
    crate::calibration::ensure_schema(&conn).unwrap();
    crate::calibration::flat_groups::store::add_tables(&conn).unwrap();
    conn.execute(
        "INSERT INTO acquiredimage (Id, projectId, targetId, acquireddate, filtername,
             gradingStatus, metadata, profileId, guid)
         VALUES (7, 1, 1, 1000, 'Ha', 0, '{\"FileName\": \"light-7.fits\"}', 'p', 'guid-7')",
        [],
    )
    .unwrap();
    override_store::set_group_override(
        &conn,
        &GroupOverrideRow {
            anchor_frame: "flat-1".into(),
            session_key: "session-1".into(),
            family_key: "family".into(),
            rig_uuid: "rig".into(),
            choice: Some(GroupChoice::Split),
            before_frame: None,
            before_session_key: None,
            label: "Ha, 2026-09-12".into(),
            chosen_at: 10,
            night: Some("2026-09-12".into()),
            before_night: None,
            rotation: Some(0.0),
            before_rotation: None,
        },
    )
    .unwrap();
    let state = Arc::new(AppState::new_for_test(
        Connection::open_in_memory().unwrap(),
    ));
    let mut context = (*state.all_databases().remove(0)).clone();
    context.database_path = path.to_string_lossy().into_owned();
    (state, Arc::new(context))
}

#[tokio::test]
async fn every_choice_waits_for_the_database_management_gate() {
    let temp = tempfile::tempdir().unwrap();
    let (state, context) = fixture(temp.path());
    let ctx = || DbContext(Arc::clone(&context));
    let reset_split = || GroupOverrideRequest {
        choice: "reset".into(),
        after_response: None,
        before_response: None,
        anchor_frame: Some("flat-1".into()),
    };
    let reset_light = || LightChoiceRequest {
        image_ids: vec![7],
        whole_session: false,
        choice: "reset".into(),
        change_key: None,
        group_response: None,
    };
    assert!(matches!(
        put_group_override(State(Arc::clone(&state)), ctx(), Json(reset_split())).await,
        Err(AppError::Forbidden(_))
    ));
    assert!(matches!(
        put_light_choices(State(Arc::clone(&state)), ctx(), Json(reset_light())).await,
        Err(AppError::Forbidden(_))
    ));
    // Reading needs no gate.
    let Json(options) = post_light_choice_options(
        ctx(),
        Json(LightChoiceOptionsRequest { image_ids: vec![7] }),
    )
    .await
    .unwrap();
    assert_eq!(options.data.unwrap().without_guid, 0);
    let read = Connection::open(&context.database_path).unwrap();
    assert_eq!(
        override_store::load_group_overrides(&read).unwrap()[0].choice,
        Some(GroupChoice::Split),
        "a refused write changes nothing"
    );

    state.set_allow_database_management(true);
    let Json(reset) = put_group_override(State(Arc::clone(&state)), ctx(), Json(reset_split()))
        .await
        .unwrap();
    assert_eq!(reset.data.unwrap().choice, None);
    let rows = override_store::load_group_overrides(&read).unwrap();
    assert_eq!(rows[0].choice, None, "kept, reset, with when");
    assert!(rows[0].chosen_at > 10);
    // The rig's lights are asked again; no flat is measured again.
    let path = &context.database_path;
    assert_eq!(crate::server::flat_measure::status(path).rigs, ["rig"]);
    crate::server::flat_measure::cancel(path);

    let Json(lights) = put_light_choices(State(Arc::clone(&state)), ctx(), Json(reset_light()))
        .await
        .unwrap();
    assert_eq!(lights.data.unwrap().lights, 1);
    // A side needs a change the book knows.
    let side = LightChoiceRequest {
        choice: "after".into(),
        change_key: Some("no such change".into()),
        ..reset_light()
    };
    assert!(matches!(
        put_light_choices(State(Arc::clone(&state)), ctx(), Json(side)).await,
        Err(AppError::NotFound)
    ));
}

/// #878's C925 check, on a copy measured by the flat pass: a split, a join
/// against a camera-side change, and a light's side. Run by hand with
/// `PSF_C925_COPY=<copy of the catalog> cargo test --lib c925_split_join_side
/// -- --ignored --nocapture`; it writes choices into that copy.
#[test]
#[ignore]
fn c925_split_join_side() {
    let path = std::env::var("PSF_C925_COPY").expect("PSF_C925_COPY");
    let conn = Connection::open(path).unwrap();
    crate::calibration::flat_groups::override_store::add_tables(&conn).unwrap();
    let book = crate::calibration::flat_groups::load_book(&conn).unwrap();
    let key_of = |filter: &str, night: &str, low: f64, high: f64| {
        book.sessions
            .iter()
            .find(|s| {
                s.row
                    .filter
                    .as_deref()
                    .is_some_and(|f| f.eq_ignore_ascii_case(filter))
                    && s.row.night == night
                    && s.row.rotation.is_some_and(|r| (low..=high).contains(&r))
            })
            .map(|s| s.row.response_key.clone())
            .unwrap_or_else(|| panic!("no {filter} session on {night}"))
    };
    let request = |choice: &str, after: String, before: Option<String>| GroupOverrideRequest {
        choice: choice.into(),
        after_response: Some(after),
        before_response: before,
        anchor_frame: None,
    };
    let split = request("split", key_of("SII", "2026-09-28", 81.0, 83.0), None);
    let join = request(
        "join",
        key_of("SII", "2026-08-15", 101.0, 105.0),
        Some(key_of("SII", "2026-08-10", 101.0, 105.0)),
    );
    for request in [split, join] {
        let (row, _) = group_override_row(&conn, &request).unwrap();
        crate::calibration::flat_groups::override_store::set_group_override(&conn, &row).unwrap();
    }
    let options = light_choice_options(&conn, &[5961, 5982, 5983]).unwrap();
    for change in &options.changes {
        println!(
            "option {} {:?} lights {:?}",
            change.what, change.filters, change.image_ids
        );
    }
    let change = options
        .changes
        .iter()
        .find(|change| change.image_ids.contains(&5982))
        .expect("a change 5982 was shot during");
    let set = apply_light_choices(
        &conn,
        &LightChoiceRequest {
            image_ids: vec![5982],
            whole_session: false,
            choice: "before".into(),
            change_key: Some(change.key.clone()),
            group_response: None,
        },
        None,
    )
    .unwrap();
    println!("side for 5982: {set:?}");
    let book = crate::calibration::flat_groups::load_book(&conn).unwrap();
    for state in &book.overrides {
        println!(
            "override {:?} {} -> {:?} edges {:?} conflict {:?}",
            state.row.choice, state.row.label, state.status, state.boundaries, state.conflict
        );
    }
    for family in book
        .families
        .iter()
        .filter(|f| f.filter.as_deref() == Some("SII"))
    {
        let groups: Vec<Vec<&str>> = family
            .groups
            .iter()
            .map(|g| {
                g.iter()
                    .map(|s| book.sessions[*s].row.night.as_str())
                    .collect()
            })
            .collect();
        println!("SII {:?}: {groups:?}", family.rotation);
    }
    for event in &book.events {
        println!(
            "event {} {}..{} by hand {} camera side {} filters {:?}",
            crate::calibration::flat_groups::describe_what(event),
            event.start,
            event.end,
            event.by_hand,
            event.camera_side,
            event.filters
        );
    }
}
