//! A person's splits, joins and choices for lights, over what the
//! measurements make: applied, in conflict with a camera-side change,
//! orphaned, in the cache keys, and carried by a sync.

use super::model::{HandEdge, Placement, Side};
use super::overrides::{
    GroupChoice, GroupOverrideRow, LightChoice, LightChoiceRow, OverrideStatus, GROUP_CHOICE_KEY,
};
use super::store::ResponseRow;
use super::{override_store, FlatBasis};
use crate::calibration::test_frames::write_frame_with;
use crate::calibration::{import_calibration_frames, select_for_light, CalibrationFrame};
use crate::commands::import::headers::{read_frame_meta, FrameMeta};
use rusqlite::Connection;
use std::path::Path;

const SIZE: (usize, usize) = (32, 24);
/// Flats shot on these mornings; the change lands between 09-12 and 09-20.
const NIGHTS: [&str; 3] = ["2026-09-10", "2026-09-12", "2026-09-20"];
/// 2026-09-15T00:00:00Z: inside the window between the last two mornings.
const CHANGE_AT: i64 = 1_789_430_400;

fn settings_lock() -> std::sync::MutexGuard<'static, ()> {
    crate::calibration::tests::POLICY_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn flats(dir: &Path, filter: &str, night: &str) -> Vec<FrameMeta> {
    (0..3)
        .map(|index| {
            let path = dir.join(format!("flat-{filter}-{night}-{index}.fits"));
            let captured = format!("{night}T14:0{index}:00");
            write_frame_with(
                &path,
                "FLAT",
                SIZE,
                &captured,
                &["ROTATANG= 0.0"],
                |_, _| 30_000.0,
            );
            let mut meta = read_frame_meta(&path);
            meta.filter = Some(filter.to_string());
            meta.rotator_position = Some(0.0);
            meta
        })
        .collect()
}

/// A catalog of Ha (and, with `oiii`, OIII) flats on [`NIGHTS`], measured
/// with `changed` filters differing across [`CHANGE_AT`] by 2% dust, all
/// at one shadow; nothing else differs.
fn catalog(dir: &Path, oiii: bool, changed: &'static [&'static str]) -> Connection {
    let mut frames = Vec::new();
    for night in NIGHTS {
        frames.extend(flats(dir, "Ha", night));
        if oiii {
            frames.extend(flats(dir, "OIII", night));
        }
    }
    let path = dir.join("catalog.sqlite");
    let mut conn = Connection::open(&path).unwrap();
    crate::ts_schema::apply_schema(&conn).unwrap();
    let tx = conn.transaction().unwrap();
    import_calibration_frames(&tx, &frames, Some("profile")).unwrap();
    tx.commit().unwrap();
    super::testing::measure_as(&conn, |a: &ResponseRow, b: &ResponseRow| {
        let after = |row: &ResponseRow| row.captured_first.unwrap() > CHANGE_AT;
        let filter = a.filter.as_deref().unwrap_or("");
        if after(a) != after(b) && changed.iter().any(|f| f.eq_ignore_ascii_case(filter)) {
            0.02
        } else {
            0.0
        }
    });
    conn
}

/// A Ha light shot at `captured`, catalogued with `guid`.
fn light(dir: &Path, conn: &Connection, id: i64, captured: &str, guid: &str) -> FrameMeta {
    let name = format!("light-{id}.fits");
    let path = dir.join(&name);
    write_frame_with(
        &path,
        "LIGHT",
        SIZE,
        captured,
        &["ROTATANG= 0.0"],
        |_, _| 1_000.0,
    );
    let mut meta = read_frame_meta(&path);
    meta.filter = Some("Ha".into());
    meta.rotator_position = Some(0.0);
    conn.execute(
        "INSERT INTO acquiredimage (Id, projectId, targetId, acquireddate, filtername,
             gradingStatus, metadata, profileId, guid)
         VALUES (?1, 1, 1, ?2, 'Ha', 0, ?3, 'profile', ?4)",
        rusqlite::params![
            id,
            meta.timestamp.unwrap(),
            format!(r#"{{"FileName": "C:\\Lights\\{name}"}}"#),
            guid
        ],
    )
    .unwrap();
    meta
}

fn nights(frames: &[CalibrationFrame]) -> Vec<String> {
    let mut nights: Vec<String> = frames
        .iter()
        .map(|frame| {
            let name = frame.source_path.file_stem().unwrap().to_string_lossy();
            name.split('-')
                .skip(2)
                .take(3)
                .collect::<Vec<_>>()
                .join("-")
        })
        .collect();
    nights.sort();
    nights.dedup();
    nights
}

/// The Ha family's session shot on `night`: its first flat and session key.
fn session(conn: &Connection, filter: &str, night: &str) -> (String, String) {
    let book = super::load_book(conn).unwrap();
    let session = book
        .sessions
        .iter()
        .find(|s| s.row.night == night && s.row.filter.as_deref() == Some(filter))
        .unwrap_or_else(|| panic!("no {filter} session on {night}"));
    (
        session.row.frame_uuids[0].clone(),
        session.row.session_key.clone(),
    )
}

fn family_of(conn: &Connection, filter: &str) -> (String, String) {
    let book = super::load_book(conn).unwrap();
    let family = book
        .families
        .iter()
        .find(|f| f.filter.as_deref() == Some(filter))
        .unwrap();
    (family.key.clone(), family.rig_uuid.clone())
}

fn group_choice(
    conn: &Connection,
    filter: &str,
    choice: Option<GroupChoice>,
    after: &str,
    before: Option<&str>,
    at: i64,
) {
    let (anchor_frame, session_key) = session(conn, filter, after);
    let before_night = before;
    let before = before.map(|night| session(conn, filter, night));
    let (family_key, rig_uuid) = family_of(conn, filter);
    override_store::set_group_override(
        conn,
        &GroupOverrideRow {
            anchor_frame,
            session_key,
            family_key,
            rig_uuid,
            choice,
            before_frame: before.as_ref().map(|b| b.0.clone()),
            before_session_key: before.map(|b| b.1),
            label: format!("{filter} {after}"),
            chosen_at: at,
            night: Some(after.to_string()),
            before_night: before_night.map(str::to_string),
            rotation: Some(0.0),
            before_rotation: before_night.map(|_| 0.0),
        },
    )
    .unwrap();
}

fn ha_groups(conn: &Connection) -> Vec<Vec<String>> {
    let book = super::load_book(conn).unwrap();
    let family = book
        .families
        .iter()
        .find(|f| f.filter.as_deref() == Some("Ha"))
        .unwrap();
    family
        .groups
        .iter()
        .map(|group| {
            group
                .iter()
                .map(|s| book.sessions[*s].row.night.clone())
                .collect()
        })
        .collect()
}

fn side_choice(
    light_guid: &str,
    event: &super::model::Event,
    side: Option<LightChoice>,
    at: i64,
) -> LightChoiceRow {
    LightChoiceRow {
        light_guid: light_guid.into(),
        change_key: event.key.clone(),
        rig_uuid: event.rig_uuid.clone(),
        change_start: Some(event.start),
        change_end: Some(event.end),
        choice: side,
        group_frame: None,
        group_session_key: None,
        label: "a side".into(),
        chosen_at: at,
    }
}

#[test]
fn a_split_by_hand_starts_a_group_where_the_nights_measure_alike() {
    let _settings = settings_lock();
    let temp = tempfile::tempdir().unwrap();
    let conn = catalog(temp.path(), false, &[]);
    assert_eq!(ha_groups(&conn), [NIGHTS.to_vec()], "alike: one group");
    assert!(super::load_book(&conn).unwrap().events.is_empty());

    group_choice(&conn, "Ha", Some(GroupChoice::Split), NIGHTS[2], None, 100);
    assert_eq!(
        ha_groups(&conn),
        [vec![NIGHTS[0], NIGHTS[1]], vec![NIGHTS[2]]]
    );
    let book = super::load_book(&conn).unwrap();
    let state = &book.overrides[0];
    assert_eq!(state.status, OverrideStatus::Applied { agrees: false });
    let edge = &book.boundaries[state.boundaries[0]];
    assert_eq!(edge.by_hand, Some(HandEdge::Split));
    assert_eq!(
        edge.dust,
        Some(0.0),
        "the measured numbers stay on the edge"
    );
    // The split makes a change of its filter: a light after it takes the
    // flats after it.
    assert_eq!(book.events.len(), 1);
    assert!(!book.events[0].camera_side);
    let after = light(temp.path(), &conn, 1, "2026-09-21T04:00:00", "g-1");
    // Its own side's flats lead; the far side's are held back from its master.
    let selected = select_for_light(&conn, &after).unwrap();
    assert_eq!(nights(&selected.flat[..3]), [NIGHTS[2]]);
    assert_eq!(
        selected.flat_held_back.len(),
        6,
        "{:?}",
        selected.flat_basis
    );

    // Reset: the measurements group again.
    group_choice(&conn, "Ha", None, NIGHTS[2], None, 200);
    assert_eq!(ha_groups(&conn), [NIGHTS.to_vec()]);
    assert!(super::load_book(&conn).unwrap().overrides.is_empty());
}

#[test]
fn a_join_by_hand_merges_two_groups_that_measure_apart() {
    let _settings = settings_lock();
    let temp = tempfile::tempdir().unwrap();
    let conn = catalog(temp.path(), false, &["Ha"]);
    assert_eq!(
        ha_groups(&conn),
        [vec![NIGHTS[0], NIGHTS[1]], vec![NIGHTS[2]]]
    );
    let inside = light(temp.path(), &conn, 1, "2026-09-16T04:00:00", "g-1");
    assert!(matches!(
        select_for_light(&conn, &inside).unwrap().flat_basis,
        FlatBasis::Undetermined { .. }
    ));

    group_choice(
        &conn,
        "Ha",
        Some(GroupChoice::Join),
        NIGHTS[2],
        Some(NIGHTS[1]),
        100,
    );
    assert_eq!(ha_groups(&conn), [NIGHTS.to_vec()]);
    let book = super::load_book(&conn).unwrap();
    assert_eq!(
        book.overrides[0].status,
        OverrideStatus::Applied { agrees: false }
    );
    assert_eq!(
        book.overrides[0].conflict, None,
        "one filter: nothing to stand against"
    );
    let family = book
        .families
        .iter()
        .find(|f| f.filter.as_deref() == Some("Ha"))
        .unwrap();
    assert_eq!(family.joined.len(), 1, "the measured split is kept, joined");
    assert!((book.boundaries[family.joined[0]].dust.unwrap() - 0.02).abs() < 1e-6);
    assert!(
        book.events.is_empty(),
        "a joined split makes no change alone"
    );
    // The nights pool without a mixed-flats warning: a person said so.
    assert!(book.measured_alike(&family.sessions));
    let selected = select_for_light(&conn, &inside).unwrap();
    assert_eq!(selected.flat_basis, FlatBasis::Nearest);
}

#[test]
fn a_join_across_a_camera_side_change_shows_the_conflict_and_drops_neither() {
    let _settings = settings_lock();
    let temp = tempfile::tempdir().unwrap();
    let conn = catalog(temp.path(), true, &["Ha", "OIII"]);
    let book = super::load_book(&conn).unwrap();
    assert_eq!(book.events.len(), 1);
    assert!(book.events[0].camera_side, "seen in Ha and OIII");

    group_choice(
        &conn,
        "Ha",
        Some(GroupChoice::Join),
        NIGHTS[2],
        Some(NIGHTS[1]),
        100,
    );
    let book = super::load_book(&conn).unwrap();
    // The join holds: Ha's flats are one group.
    assert_eq!(ha_groups(&conn), [NIGHTS.to_vec()]);
    // And so does the change: still camera side, still applying to Ha.
    assert_eq!(book.events.len(), 1);
    let event = &book.events[0];
    assert!(event.camera_side);
    assert!(event.applies_to(Some("Ha")));
    assert_eq!(
        book.overrides[0].conflict.as_deref(),
        Some(event.key.as_str())
    );
    // A Ha light shot during it is still flagged: the change decides.
    let inside = light(temp.path(), &conn, 1, "2026-09-16T04:00:00", "g-1");
    assert!(select_for_light(&conn, &inside).unwrap().flat_basis.flags());
}

#[test]
fn a_side_chosen_by_hand_wins_over_time_session_and_pixels() {
    let _settings = settings_lock();
    let temp = tempfile::tempdir().unwrap();
    let conn = catalog(temp.path(), false, &["Ha"]);
    let inside = light(temp.path(), &conn, 1, "2026-09-16T04:00:00", "g-1");
    let before = light(temp.path(), &conn, 2, "2026-09-11T04:00:00", "g-2");
    let event = super::load_book(&conn).unwrap().events[0].clone();

    override_store::set_light_choices(
        &conn,
        &[
            side_choice("g-1", &event, Some(LightChoice::After), 100),
            // Over its time: shot before the change, put after it.
            side_choice("g-2", &event, Some(LightChoice::After), 100),
        ],
    )
    .unwrap();
    for light in [&inside, &before] {
        let selected = select_for_light(&conn, light).unwrap();
        assert_eq!(nights(&selected.flat), [NIGHTS[2]]);
        match &selected.flat_basis {
            FlatBasis::SameSide { placement, .. } => {
                assert_eq!(*placement, Placement::Person { side: Side::After })
            }
            other => panic!("{other:?}"),
        }
        assert!(!selected.flat_basis.flags());
        assert_eq!(selected.flat_choices.len(), 1);
        assert_eq!(selected.flat_choices[0].orphaned, None);
        // An export (and so WBPP) takes the flats a stack takes.
        let delivered: std::collections::BTreeSet<String> =
            crate::calibration::delivery_selection(selected)
                .flat
                .iter()
                .map(|f| f.frame_uuid.clone())
                .collect();
        let export = crate::calibration::export_destinations(
            &conn,
            light,
            "Target",
            None,
            crate::commands::export::ExportLayout::Standard,
            &crate::calibration::Nights::load(&conn),
            false,
        )
        .unwrap();
        let exported: std::collections::BTreeSet<String> = export
            .items
            .iter()
            .filter(|(kind, _, _)| *kind == crate::calibration::CalibrationKind::Flat)
            .map(|(_, f, _)| f.frame_uuid.clone())
            .collect();
        assert_eq!(delivered, exported);
    }

    // Reset: the light inside the window is flagged again.
    override_store::set_light_choices(&conn, &[side_choice("g-1", &event, None, 200)]).unwrap();
    let selected = select_for_light(&conn, &inside).unwrap();
    assert!(selected.flat_basis.flags());
    assert!(selected.flat_choices.is_empty());
}

#[test]
fn a_group_chosen_by_hand_gives_its_flats_over_every_change() {
    let _settings = settings_lock();
    let temp = tempfile::tempdir().unwrap();
    let conn = catalog(temp.path(), false, &["Ha"]);
    let after = light(temp.path(), &conn, 1, "2026-09-21T04:00:00", "g-1");

    let (frame, session_key) = session(&conn, "Ha", NIGHTS[1]);
    let (_, rig) = family_of(&conn, "Ha");
    override_store::set_light_choices(
        &conn,
        &[LightChoiceRow {
            light_guid: "g-1".into(),
            change_key: GROUP_CHOICE_KEY.into(),
            rig_uuid: rig,
            change_start: None,
            change_end: None,
            choice: Some(LightChoice::Group),
            group_frame: Some(frame),
            group_session_key: Some(session_key),
            label: "the first group".into(),
            chosen_at: 100,
        }],
    )
    .unwrap();
    let selected = select_for_light(&conn, &after).unwrap();
    assert!(nights(&selected.flat).iter().all(|n| n != NIGHTS[2]));
    assert!(matches!(selected.flat_basis, FlatBasis::Group { .. }));
    assert_eq!(
        selected.flat_family.as_ref().map(|f| f.basis.clone()),
        Some(super::lights::FamilyBasis::Person)
    );
}

#[test]
fn a_choice_whose_change_or_flats_are_gone_is_kept_and_shown_not_applied() {
    let _settings = settings_lock();
    let temp = tempfile::tempdir().unwrap();
    let conn = catalog(temp.path(), false, &["Ha"]);
    let inside = light(temp.path(), &conn, 1, "2026-09-16T04:00:00", "g-1");
    let event = super::load_book(&conn).unwrap().events[0].clone();
    // A change whose window narrowed is found again by its window.
    let mut moved = side_choice("g-1", &event, Some(LightChoice::After), 100);
    moved.change_key = "renamed".into();
    override_store::set_light_choices(&conn, &[moved]).unwrap();
    let selected = select_for_light(&conn, &inside).unwrap();
    assert_eq!(nights(&selected.flat), [NIGHTS[2]]);

    // One for a change gone altogether stays, orphaned, and does nothing.
    let mut gone = side_choice("g-1", &event, Some(LightChoice::Before), 200);
    gone.change_key = "gone".into();
    gone.change_start = Some(1);
    gone.change_end = Some(2);
    override_store::set_light_choices(&conn, &[gone]).unwrap();
    let selected = select_for_light(&conn, &inside).unwrap();
    assert_eq!(
        nights(&selected.flat),
        [NIGHTS[2]],
        "the other choice still holds"
    );
    let orphaned: Vec<_> = selected
        .flat_choices
        .iter()
        .filter(|note| note.orphaned.is_some())
        .collect();
    assert_eq!(orphaned.len(), 1);
    assert_eq!(orphaned[0].change_key.as_deref(), Some("gone"));

    // A split at flats the catalog no longer holds: orphaned, shown.
    override_store::set_group_override(
        &conn,
        &GroupOverrideRow {
            anchor_frame: "forgotten".into(),
            session_key: "forgotten".into(),
            family_key: "old".into(),
            rig_uuid: event.rig_uuid.clone(),
            choice: Some(GroupChoice::Split),
            before_frame: None,
            before_session_key: None,
            label: "Ha, 2026-08-01".into(),
            chosen_at: 300,
            night: Some("2026-08-01".into()),
            before_night: None,
            rotation: Some(0.0),
            before_rotation: None,
        },
    )
    .unwrap();
    let book = super::load_book(&conn).unwrap();
    assert!(matches!(
        book.overrides[0].status,
        OverrideStatus::Orphaned { .. }
    ));
    let view = crate::server::flat_groups::build_view(&conn).unwrap();
    assert_eq!(view.rigs[0].orphaned.len(), 1);
}

#[test]
fn choices_are_in_every_cache_key_and_stacks_change_only_with_their_flats() {
    let _settings = settings_lock();
    let temp = tempfile::tempdir().unwrap();
    let conn = catalog(temp.path(), false, &["Ha"]);
    let after = light(temp.path(), &conn, 1, "2026-09-21T04:00:00", "g-1");
    let event = super::load_book(&conn).unwrap().events[0].clone();
    let book = super::load_book(&conn).unwrap();
    let health = crate::calibration::health::library_digest(&conn).unwrap();
    let scope = crate::calibration::project_scope::catalog_revision(&conn).unwrap();
    let stack = |conn: &Connection| {
        crate::calibration::selection_hash(&select_for_light(conn, &after).unwrap(), false)
    };
    let first = stack(&conn);

    // Its time already puts it after: the choice changes no flat.
    override_store::set_light_choices(
        &conn,
        &[side_choice("g-1", &event, Some(LightChoice::After), 100)],
    )
    .unwrap();
    assert!(!std::sync::Arc::ptr_eq(
        &book,
        &super::load_book(&conn).unwrap()
    ));
    assert_ne!(
        health,
        crate::calibration::health::library_digest(&conn).unwrap()
    );
    assert_ne!(
        scope,
        crate::calibration::project_scope::catalog_revision(&conn).unwrap()
    );
    assert_eq!(stack(&conn), first, "same flats, same stack");

    // Put before: other flats, another stack.
    override_store::set_light_choices(
        &conn,
        &[side_choice("g-1", &event, Some(LightChoice::Before), 200)],
    )
    .unwrap();
    assert_ne!(stack(&conn), first);
}

#[test]
fn choices_travel_with_a_sync_and_the_newer_wins() {
    let _settings = settings_lock();
    let temp = tempfile::tempdir().unwrap();
    let source = catalog(temp.path(), false, &["Ha"]);
    light(temp.path(), &source, 1, "2026-09-16T04:00:00", "g-1");
    let event = super::load_book(&source).unwrap().events[0].clone();
    group_choice(
        &source,
        "Ha",
        Some(GroupChoice::Join),
        NIGHTS[2],
        Some(NIGHTS[1]),
        100,
    );
    override_store::set_light_choices(
        &source,
        &[side_choice("g-1", &event, Some(LightChoice::After), 100)],
    )
    .unwrap();

    // The destination: a copy of the catalog without the choices.
    let copy = temp.path().join("copy.sqlite");
    std::fs::copy(source.path().unwrap(), &copy).unwrap();
    let mut destination = Connection::open(&copy).unwrap();
    destination
        .execute_batch(
            "DELETE FROM psf_guard_flat_group_override; DELETE FROM psf_guard_flat_light_choice;",
        )
        .unwrap();
    let sync = |destination: &mut Connection| {
        let tx = destination.transaction().unwrap();
        let outcome = crate::calibration::sync_library(&source, &tx).unwrap();
        tx.commit().unwrap();
        outcome.choices
    };
    let choices = sync(&mut destination);
    assert_eq!((choices.flat_groups, choices.flat_lights), (1, 1));
    assert_eq!(
        override_store::load_group_overrides(&destination)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        override_store::light_choices_of(&destination, &["g-1".into()]).unwrap()[0].choice,
        Some(LightChoice::After)
    );
    assert_eq!(
        sync(&mut destination).total(),
        0,
        "nothing new the second time"
    );

    // A reset made later on the destination survives the older choice.
    override_store::set_light_choices(&destination, &[side_choice("g-1", &event, None, 500)])
        .unwrap();
    sync(&mut destination);
    assert_eq!(
        override_store::light_choices_of(&destination, &["g-1".into()]).unwrap()[0].choice,
        None
    );
    // A newer choice on the source comes across.
    override_store::set_light_choices(
        &source,
        &[side_choice("g-1", &event, Some(LightChoice::Before), 900)],
    )
    .unwrap();
    sync(&mut destination);
    assert_eq!(
        override_store::light_choices_of(&destination, &["g-1".into()]).unwrap()[0].choice,
        Some(LightChoice::Before)
    );
}

#[test]
fn a_split_by_hand_places_lights_by_time_at_the_start_of_its_later_night() {
    let _settings = settings_lock();
    let temp = tempfile::tempdir().unwrap();
    let conn = catalog(temp.path(), false, &[]);
    let between = light(temp.path(), &conn, 1, "2026-09-16T04:00:00", "g-1");
    group_choice(&conn, "Ha", Some(GroupChoice::Split), NIGHTS[2], None, 100);
    let book = super::load_book(&conn).unwrap();
    let event = &book.events[0];
    assert!(event.by_hand);
    assert!(!event.within_flat_run, "a moment, not a narrow window");
    // No window: the split falls where the night of its later flats begins,
    // at the rig's own night boundary.
    assert_eq!(event.start, event.end);
    let (frame, _) = session(&conn, "Ha", NIGHTS[2]);
    let later = book.session_of_frame(&frame).unwrap();
    assert_eq!(event.start, book.night_start(later));
    assert!(event.start <= book.sessions[later].first);
    assert!(event.start > book.sessions[later].first - 86_400);
    let local = crate::calibration::Nights::load(&conn);
    let rig = &book.sessions[later].row.rig_uuid;
    let night = local.night_of_frame(rig, &frame, book.sessions[later].first);
    assert_eq!(local.night_of_frame(rig, &frame, event.start), night);
    assert_ne!(local.night_of_frame(rig, &frame, event.start - 1), night);
    // No light is read for it.
    assert!(book.sessions_in_windows().is_empty());
    // A light of an earlier night is before it, by its time, and flagged
    // for nothing.
    let selected = select_for_light(&conn, &between).unwrap();
    assert!(!selected.flat_basis.flags(), "{:?}", selected.flat_basis);
    assert!(!nights(&selected.flat[..1]).contains(&NIGHTS[2].to_string()));
}

#[test]
fn a_split_by_hand_never_carries_an_alike_range() {
    let _settings = settings_lock();
    let temp = tempfile::tempdir().unwrap();
    let conn = catalog(temp.path(), true, &["Ha", "OIII"]);
    let limits = |book: &super::model::Book| book.limits(&book.events[0].rig_uuid).unwrap();
    let ha_edge = |book: &super::model::Book| {
        book.families
            .iter()
            .find(|f| f.filter.as_deref() == Some("Ha"))
            .unwrap()
            .boundaries[0]
    };
    let book = super::load_book(&conn).unwrap();
    assert!(
        book.carries_angle_term(ha_edge(&book), &limits(&book)),
        "camera-side dust carries a range"
    );
    // The same edge, split by hand as well: the person's word carries none.
    group_choice(&conn, "Ha", Some(GroupChoice::Split), NIGHTS[2], None, 100);
    let book = super::load_book(&conn).unwrap();
    assert_eq!(
        book.overrides[0].status,
        OverrideStatus::Applied { agrees: true }
    );
    assert!(!book.carries_angle_term(ha_edge(&book), &limits(&book)));
}

// Lights that share a file name, flats a repair merged, and bulk sides.

/// A Ha light written as `dir/name`, catalogued with `id` and `guid`.
fn light_named(
    dir: &Path,
    conn: &Connection,
    id: i64,
    name: &str,
    captured: &str,
    guid: &str,
) -> FrameMeta {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(name);
    write_frame_with(
        &path,
        "LIGHT",
        SIZE,
        captured,
        &["ROTATANG= 0.0"],
        |_, _| 1_000.0,
    );
    let mut meta = read_frame_meta(&path);
    meta.filter = Some("Ha".into());
    meta.rotator_position = Some(0.0);
    conn.execute(
        "INSERT INTO acquiredimage (Id, projectId, targetId, acquireddate, filtername,
             gradingStatus, metadata, profileId, guid)
         VALUES (?1, 1, 1, ?2, 'Ha', 0, ?3, 'profile', ?4)",
        rusqlite::params![
            id,
            meta.timestamp.unwrap(),
            format!(r#"{{"FileName": "C:\\Lights\\{id}\\{name}"}}"#),
            guid
        ],
    )
    .unwrap();
    meta
}

/// A light's choice is found by file name and filter, then the nearest
/// time among lights that HAVE choices. A light with no choice that shares
/// its file name (two targets both writing `Ha_0001.fits`) takes the other
/// light's side, though it was shot on another night.
#[test]
fn a_light_without_a_choice_does_not_take_another_lights_choice_by_file_name() {
    let _settings = settings_lock();
    let temp = tempfile::tempdir().unwrap();
    let conn = catalog(temp.path(), false, &["Ha"]);
    let a = temp.path().join("a");
    let b = temp.path().join("b");
    let chosen = light_named(&a, &conn, 1, "Ha_0001.fits", "2026-09-16T04:00:00", "g-1");
    // Shot well before the change, with no choice of its own.
    let other = light_named(&b, &conn, 2, "Ha_0001.fits", "2026-09-11T04:00:00", "g-2");
    let event = super::load_book(&conn).unwrap().events[0].clone();
    override_store::set_light_choices(
        &conn,
        &[side_choice("g-1", &event, Some(LightChoice::After), 100)],
    )
    .unwrap();
    assert!(matches!(
        select_for_light(&conn, &chosen).unwrap().flat_basis,
        FlatBasis::SameSide {
            placement: Placement::Person { .. },
            ..
        }
    ));
    let selected = select_for_light(&conn, &other).unwrap();
    assert!(
        selected.flat_choices.is_empty(),
        "light 2 took light 1's choice: {:?}",
        selected.flat_choices
    );
    assert!(!nights(&selected.flat[..1]).contains(&NIGHTS[2].to_string()));
}

/// A split anchored on a flat the destination's duplicate repair merged
/// into its twin (#877) never comes across: the sync looks the anchor up
/// in `psf_guard_calibration_frame` without the merge map that the frame
/// and quality syncs translate through.
#[test]
fn a_split_on_a_merged_flat_still_syncs_to_its_keeper() {
    let _settings = settings_lock();
    let temp = tempfile::tempdir().unwrap();
    let source = catalog(temp.path(), false, &[]);
    group_choice(
        &source,
        "Ha",
        Some(GroupChoice::Split),
        NIGHTS[2],
        None,
        100,
    );
    let (anchor, _) = session(&source, "Ha", NIGHTS[2]);
    let book = super::load_book(&source).unwrap();
    let session = book.session_of_frame(&anchor).unwrap();
    let keeper = book.sessions[session].row.frame_uuids[1].clone();

    let copy = temp.path().join("copy.sqlite");
    std::fs::copy(source.path().unwrap(), &copy).unwrap();
    let mut destination = Connection::open(&copy).unwrap();
    destination
        .execute_batch("DELETE FROM psf_guard_flat_group_override;")
        .unwrap();
    // The destination's repair merged the anchor into its twin.
    destination
        .execute(
            "DELETE FROM psf_guard_calibration_frame WHERE frame_uuid = ?1",
            [&anchor],
        )
        .unwrap();
    destination
        .execute(
            "INSERT INTO psf_guard_calibration_merged (merged_uuid, keep_uuid, merged_at)
             VALUES (?1, ?2, 1)",
            [&anchor, &keeper],
        )
        .unwrap();
    let tx = destination.transaction().unwrap();
    let outcome = crate::calibration::sync_library(&source, &tx).unwrap();
    tx.commit().unwrap();
    assert_eq!(outcome.choices.flat_groups, 1, "the split was dropped");
}

/// Images' bulk "Flat side…" sends every selected light with the change's
/// key, and the route writes a side for each, so a light shot days before
/// the change is put after it, over its own time.
#[test]
fn the_bulk_side_moves_only_lights_shot_during_the_change() {
    let _settings = settings_lock();
    let temp = tempfile::tempdir().unwrap();
    let conn = catalog(temp.path(), false, &["Ha"]);
    light(temp.path(), &conn, 1, "2026-09-16T04:00:00", "g-1");
    let before = light(temp.path(), &conn, 2, "2026-09-11T04:00:00", "g-2");
    let event = super::load_book(&conn).unwrap().events[0].clone();
    let options = crate::server::flat_choices::light_choice_options(&conn, &[1, 2]).unwrap();
    assert_eq!(options.changes.len(), 1);
    assert_eq!(options.changes[0].lights, 1, "one light shot during it");
    // What FlatSideDialog sends on "After".
    crate::server::flat_choices::apply_light_choices(
        &conn,
        &crate::server::flat_choices::LightChoiceRequest {
            image_ids: vec![1, 2],
            whole_session: false,
            choice: "after".into(),
            change_key: Some(event.key.clone()),
            group_response: None,
        },
        None,
    )
    .unwrap();
    let selected = select_for_light(&conn, &before).unwrap();
    assert!(
        !nights(&selected.flat[..1]).contains(&NIGHTS[2].to_string()),
        "a light shot before the change now takes the flats after it: {:?}",
        selected.flat_basis
    );
}

/// A duplicate repair here that folds a split's anchor flat into its twin
/// moves the split to the kept flat, so it is still found.
#[test]
fn a_repair_moves_a_split_to_the_flat_it_kept() {
    let _settings = settings_lock();
    let temp = tempfile::tempdir().unwrap();
    let conn = catalog(temp.path(), false, &[]);
    group_choice(&conn, "Ha", Some(GroupChoice::Split), NIGHTS[2], None, 100);
    let (anchor, _) = session(&conn, "Ha", NIGHTS[2]);
    override_store::repoint_frames(&conn, std::slice::from_ref(&anchor), "kept-twin").unwrap();
    let rows = override_store::load_group_overrides(&conn).unwrap();
    assert_eq!(rows[0].anchor_frame, "kept-twin");
}

/// Keeping out the first flat of a split's session renames the session
/// (its key hashes its first flat in use): the split is found again by its
/// rig, family and night, not orphaned.
#[test]
fn keeping_out_a_splits_first_flat_does_not_orphan_it() {
    let _settings = settings_lock();
    let temp = tempfile::tempdir().unwrap();
    let conn = catalog(temp.path(), false, &[]);
    group_choice(&conn, "Ha", Some(GroupChoice::Split), NIGHTS[2], None, 100);
    let (anchor, old_key) = session(&conn, "Ha", NIGHTS[2]);
    crate::calibration::set_overrides(
        &conn,
        std::slice::from_ref(&anchor),
        Some(crate::calibration::QualityOverride::Exclude),
    )
    .unwrap();
    // The pass measures the session again, without that flat.
    conn.execute_batch(
        "DELETE FROM psf_guard_flat_response; DELETE FROM psf_guard_flat_comparison;",
    )
    .unwrap();
    super::testing::measure_as(&conn, |_, _| 0.0);
    let (_, new_key) = session(&conn, "Ha", NIGHTS[2]);
    assert_ne!(new_key, old_key, "the session is named anew");
    let book = super::load_book(&conn).unwrap();
    assert_eq!(
        book.overrides[0].status,
        OverrideStatus::Applied { agrees: false },
        "{:?}",
        book.overrides[0].status
    );
    assert_eq!(
        ha_groups(&conn),
        [vec![NIGHTS[0], NIGHTS[1]], vec![NIGHTS[2]]]
    );
}

/// A side for one light alone needs a change of its rig and filter; a light
/// of another filter is refused, and nothing is written.
#[test]
fn a_side_for_a_light_the_change_does_not_reach_is_refused() {
    let _settings = settings_lock();
    let temp = tempfile::tempdir().unwrap();
    let conn = catalog(temp.path(), false, &["Ha"]);
    conn.execute(
        "INSERT INTO acquiredimage (Id, projectId, targetId, acquireddate, filtername,
             gradingStatus, metadata, profileId, guid)
         VALUES (5, 1, 1, ?1, 'OIII', 0, '{\"FileName\": \"oiii.fits\"}', 'profile', 'g-5')",
        [CHANGE_AT],
    )
    .unwrap();
    let event = super::load_book(&conn).unwrap().events[0].clone();
    let refused = crate::server::flat_choices::apply_light_choices(
        &conn,
        &crate::server::flat_choices::LightChoiceRequest {
            image_ids: vec![5],
            whole_session: false,
            choice: "before".into(),
            change_key: Some(event.key.clone()),
            group_response: None,
        },
        None,
    );
    assert!(
        matches!(
            refused,
            Err(crate::server::handlers::AppError::BadRequest(_))
        ),
        "{refused:?}"
    );
    assert!(override_store::light_choices_of(&conn, &["g-5".into()])
        .unwrap()
        .is_empty());
}

// A split's night does not reach another angle, and one night's two sessions.

fn angled_flats(dir: &Path, night: &str, angle: f64, hour: u32) -> Vec<FrameMeta> {
    (0..3)
        .map(|index| {
            let path = dir.join(format!("flat-Ha-{night}-a{angle}-{index}.fits"));
            let captured = format!("{night}T{hour:02}:0{index}:00");
            write_frame_with(
                &path,
                "FLAT",
                SIZE,
                &captured,
                &[&format!("ROTATANG= {angle:.1}")],
                |_, _| 30_000.0,
            );
            let mut meta = read_frame_meta(&path);
            meta.filter = Some("Ha".to_string());
            meta.rotator_position = Some(angle);
            meta
        })
        .collect()
}

fn angled_catalog(dir: &Path, frames: &[FrameMeta]) -> Connection {
    let mut conn = Connection::open(dir.join("catalog.sqlite")).unwrap();
    crate::ts_schema::apply_schema(&conn).unwrap();
    let tx = conn.transaction().unwrap();
    import_calibration_frames(&tx, frames, Some("profile")).unwrap();
    tx.commit().unwrap();
    conn
}

fn split_at(conn: &Connection, session: usize) -> Vec<String> {
    let book = super::load_book(conn).unwrap();
    let family = book
        .families
        .iter()
        .position(|f| f.sessions.contains(&session))
        .unwrap();
    let row = &book.sessions[session].row;
    override_store::set_group_override(
        conn,
        &GroupOverrideRow {
            anchor_frame: row.frame_uuids[0].clone(),
            session_key: row.session_key.clone(),
            family_key: book.families[family].key.clone(),
            rig_uuid: row.rig_uuid.clone(),
            choice: Some(GroupChoice::Split),
            before_frame: None,
            before_session_key: None,
            label: "split".into(),
            chosen_at: 100,
            night: Some(row.night.clone()),
            before_night: None,
            rotation: row.rotation,
            before_rotation: None,
        },
    )
    .unwrap();
    row.frame_uuids.clone()
}

#[test]
fn the_night_fallback_does_not_move_a_split_to_another_angle() {
    let _settings = settings_lock();
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    let mut frames = Vec::new();
    for night in NIGHTS {
        frames.extend(angled_flats(dir, night, 0.0, 14));
    }
    for night in &NIGHTS[1..] {
        frames.extend(angled_flats(dir, night, 90.0, 15));
    }
    let conn = angled_catalog(dir, &frames);
    let measure = |conn: &Connection| {
        super::testing::measure_as(
            conn,
            |a: &ResponseRow, b: &ResponseRow| {
                if a.rotation != b.rotation {
                    0.05
                } else {
                    0.0
                }
            },
        )
    };
    measure(&conn);
    let book = super::load_book(&conn).unwrap();
    assert_eq!(book.families.len(), 2, "two angles measure apart");
    let anchor_session = book
        .sessions
        .iter()
        .position(|s| s.row.night == NIGHTS[2] && s.row.rotation == Some(0.0))
        .unwrap();
    let zero_frames = split_at(&conn, anchor_session);
    // Keep out the whole 0° session of that night.
    crate::calibration::set_overrides(
        &conn,
        &zero_frames,
        Some(crate::calibration::QualityOverride::Exclude),
    )
    .unwrap();
    conn.execute_batch(
        "DELETE FROM psf_guard_flat_response; DELETE FROM psf_guard_flat_comparison;",
    )
    .unwrap();
    measure(&conn);
    let book = super::load_book(&conn).unwrap();
    let state = &book.overrides[0];
    if let OverrideStatus::Applied { .. } = state.status {
        let edge = &book.boundaries[state.boundaries[0]];
        let after = &book.sessions[edge.after].row;
        panic!(
            "split anchored on the 0° session now applies before the {:?}° session on {}",
            after.rotation, after.night
        );
    }
}

#[test]
fn a_same_night_split_falls_at_the_later_sessions_first_flat() {
    let _settings = settings_lock();
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path();
    let mut frames = Vec::new();
    for night in &NIGHTS[..2] {
        frames.extend(angled_flats(dir, night, 0.0, 14));
    }
    // Two sessions that night: an earlier one recording no angle, alike.
    frames.extend(
        angled_flats(dir, NIGHTS[2], 7.0, 14)
            .into_iter()
            .map(|mut m| {
                m.rotator_position = None;
                m
            }),
    );
    frames.extend(angled_flats(dir, NIGHTS[2], 0.0, 20));
    let conn = angled_catalog(dir, &frames);
    super::testing::measure_as(&conn, |_, _| 0.0);
    let book = super::load_book(&conn).unwrap();
    assert_eq!(book.families.len(), 1, "alike: one family");
    let later = book
        .sessions
        .iter()
        .position(|s| s.row.night == NIGHTS[2] && s.row.rotation == Some(0.0))
        .unwrap();
    let earlier = book
        .sessions
        .iter()
        .position(|s| s.row.night == NIGHTS[2] && s.row.rotation.is_none())
        .unwrap();
    assert_eq!(
        book.sessions[later].row.night,
        book.sessions[earlier].row.night
    );
    split_at(&conn, later);
    let book = super::load_book(&conn).unwrap();
    assert_eq!(book.events.len(), 1, "{:?}", book.overrides);
    let event = &book.events[0];
    assert!(event.by_hand);
    assert_eq!(event.start, book.sessions[later].first);
    assert_eq!(event.end, book.sessions[later].first);
    assert!(event.start > book.sessions[earlier].last);
    assert!(!event.within_flat_run);
}

/// A catalog that records when a light was saved, not when its exposure
/// began, still finds the light's choice: N.I.N.A.'s `ExposureStartTime`,
/// which its header's DATE-OBS records too, is read first.
#[test]
fn a_light_saved_minutes_after_it_began_still_finds_its_choice() {
    let _settings = settings_lock();
    let temp = tempfile::tempdir().unwrap();
    let conn = catalog(temp.path(), false, &["Ha"]);
    let inside = light(temp.path(), &conn, 1, "2026-09-16T04:00:00", "g-1");
    let began = inside.timestamp.unwrap();
    conn.execute(
        "UPDATE acquiredimage SET acquireddate = ?1,
             metadata = json_set(metadata, '$.ExposureStartTime', '2026-09-16T04:00:00Z')
         WHERE Id = 1",
        [began + 305],
    )
    .unwrap();
    let event = super::load_book(&conn).unwrap().events[0].clone();
    override_store::set_light_choices(
        &conn,
        &[side_choice("g-1", &event, Some(LightChoice::After), 100)],
    )
    .unwrap();
    let selected = select_for_light(&conn, &inside).unwrap();
    assert_eq!(
        selected.flat_choices.len(),
        1,
        "{:?}",
        selected.flat_choices
    );
    assert_eq!(nights(&selected.flat), [NIGHTS[2]]);
}

/// One light alone may take a side of a change it was not shot during only
/// when its panel offers that change (its flats were chosen around it).
#[test]
fn one_light_outside_a_change_takes_a_side_only_of_a_change_its_panel_offers() {
    let _settings = settings_lock();
    let temp = tempfile::tempdir().unwrap();
    let conn = catalog(temp.path(), false, &["Ha"]);
    light(temp.path(), &conn, 2, "2026-09-11T04:00:00", "g-2");
    let event = super::load_book(&conn).unwrap().events[0].clone();
    let request = crate::server::flat_choices::LightChoiceRequest {
        image_ids: vec![2],
        whole_session: false,
        choice: "after".into(),
        change_key: Some(event.key.clone()),
        group_response: None,
    };
    let refused = crate::server::flat_choices::apply_light_choices(&conn, &request, None);
    assert!(
        matches!(
            refused,
            Err(crate::server::handlers::AppError::BadRequest(_))
        ),
        "{refused:?}"
    );
    let offered: std::collections::HashSet<String> = [event.key.clone()].into();
    let set =
        crate::server::flat_choices::apply_light_choices(&conn, &request, Some(&offered)).unwrap();
    assert_eq!((set.lights, set.skipped), (1, 0));
}
