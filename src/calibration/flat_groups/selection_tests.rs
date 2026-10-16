//! End to end on synthetic frames: flats written as FITS, measured by the
//! pass, grouped, and then chosen for lights on either side of a change.

use super::model::{Placement, Side};
use super::FlatBasis;
use crate::calibration::test_frames::write_frame_with;
use crate::calibration::{
    corrected_flat_master, import_calibration_frames, select_for_light, BuiltMaster,
    CalibrationFrame,
};
use crate::commands::import::headers::{read_frame_meta, FrameMeta};
use rusqlite::Connection;
use std::path::{Path, PathBuf};

const SIZE: (usize, usize) = (240, 160);
/// The shadow that turns up between the two mornings.
const DUST: (f32, f32, f32, f32) = (120.0, 70.0, 16.0, 0.03);
/// A shadow cast ahead of the rotator: it lands here only at 90°, so flats
/// at 0° and 90° measure apart and stay two response families.
const AT_90: (f32, f32, f32, f32) = (50.0, 120.0, 14.0, 0.025);

/// One morning's flats: three frames of one filter at one angle.
struct Session {
    name: &'static str,
    captured: &'static str,
    filter: &'static str,
    /// `None`: the flats record no angle.
    rotation: Option<f64>,
    dusty: bool,
}

fn write_session(dir: &Path, session: &Session, seed: u64) -> Vec<FrameMeta> {
    let mut dust: Vec<(f32, f32, f32, f32)> = Vec::new();
    if session.dusty {
        dust.push(DUST);
    }
    // Ha at 90° sees a reflection no other filter or angle shows: its
    // flats measure apart from Ha at 0°, so the two stay two response
    // families, while OIII at 0° and 90° measure alike and give the rig
    // its angle term.
    if session.filter == "Ha" && session.rotation == Some(90.0) {
        dust.push(AT_90);
    }
    write_session_with(dir, session, seed, &dust)
}

/// [`write_session`] with the shadows given.
fn write_session_with(
    dir: &Path,
    session: &Session,
    seed: u64,
    dust: &[(f32, f32, f32, f32)],
) -> Vec<FrameMeta> {
    (0..3)
        .map(|index| {
            let path = dir.join(format!("{}-{index}.fits", session.name));
            let plane = super::testing::synthetic(
                SIZE.0,
                SIZE.1,
                dust,
                0.0,
                &[],
                0.0005,
                seed * 10 + index as u64,
            );
            // Frames a second apart.
            let captured = format!("{}:0{index}", session.captured);
            let angle = session
                .rotation
                .map(|rotation| format!("ROTATANG= {rotation:20.1}"));
            let cards: Vec<&str> = angle.iter().map(String::as_str).collect();
            write_frame_with(&path, "FLAT", SIZE, &captured, &cards, |x, y| {
                30_000.0 * f64::from(plane.data[y * SIZE.0 + x])
            });
            let mut meta = read_frame_meta(&path);
            meta.filter = Some(session.filter.to_string());
            meta.rotator_position = session.rotation;
            meta
        })
        .collect()
}

/// A catalog of these sessions, measured.
fn measured_library(dir: &Path, sessions: &[Session]) -> (Connection, PathBuf) {
    let frames: Vec<FrameMeta> = sessions
        .iter()
        .enumerate()
        .flat_map(|(seed, session)| write_session(dir, session, seed as u64 + 1))
        .collect();
    measured_frames(dir, &frames)
}

/// A catalog of these flats, measured.
fn measured_frames(dir: &Path, frames: &[FrameMeta]) -> (Connection, PathBuf) {
    let path = dir.join("catalog.sqlite");
    let mut conn = Connection::open(&path).unwrap();
    let tx = conn.transaction().unwrap();
    import_calibration_frames(&tx, frames, Some("profile")).unwrap();
    tx.commit().unwrap();
    super::store::add_tables(&conn).unwrap();
    let responses = dir.join("responses");
    super::register_response_dir(&path, responses.clone());
    let outcome = super::job::run(super::job::Pass {
        conn: &conn,
        dir: &responses,
        scope: super::job::Scope::everything(),
        keep_going: &|| true,
        yield_now: &|| {},
        progress: &mut |_| {},
        goes_first: &|_| false,
        find_light: &|_| None,
        pool: None,
    })
    .unwrap();
    assert_eq!(outcome.unreadable, 0, "{outcome:?}");
    (conn, responses)
}

/// Selection reads the settings in force, which other tests change.
fn settings_lock() -> std::sync::MutexGuard<'static, ()> {
    crate::calibration::tests::POLICY_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn light(dir: &Path, captured: &str, rotation: f64) -> FrameMeta {
    // Windows refuses `:` in file names, so the capture time loses it here.
    let path = dir.join(format!("light-{}.fits", captured.replace(':', "")));
    let angle = format!("ROTATANG= {rotation:20.1}");
    write_frame_with(&path, "LIGHT", SIZE, captured, &[angle.as_str()], |_, _| {
        1_000.0
    });
    let mut meta = read_frame_meta(&path);
    meta.rotator_position = Some(rotation);
    meta
}

fn names(frames: &[CalibrationFrame]) -> Vec<String> {
    let mut names: Vec<String> = frames
        .iter()
        .map(|frame| {
            let name = frame.source_path.file_stem().unwrap().to_string_lossy();
            name.rsplit_once('-').unwrap().0.to_string()
        })
        .collect();
    names.dedup();
    names
}

const BEFORE_HA_0: Session = Session {
    name: "before-ha-0",
    captured: "2026-09-10T14:00",
    filter: "Ha",
    rotation: Some(0.0),
    dusty: false,
};
const BEFORE_OIII_0: Session = Session {
    name: "before-oiii-0",
    captured: "2026-09-10T14:10",
    filter: "OIII",
    rotation: Some(0.0),
    dusty: false,
};
const BEFORE_OIII_90: Session = Session {
    name: "before-oiii-90",
    captured: "2026-09-10T14:20",
    filter: "OIII",
    rotation: Some(90.0),
    dusty: false,
};
const AFTER_HA_0: Session = Session {
    name: "after-ha-0",
    captured: "2026-09-14T14:00",
    filter: "Ha",
    rotation: Some(0.0),
    dusty: true,
};
const AFTER_HA_90: Session = Session {
    name: "after-ha-90",
    // Days later, so no pair at two angles measures the rig.
    captured: "2026-09-17T14:00",
    filter: "Ha",
    rotation: Some(90.0),
    dusty: true,
};
const AFTER_OIII_0: Session = Session {
    name: "after-oiii-0",
    captured: "2026-09-14T14:20",
    filter: "OIII",
    rotation: Some(0.0),
    dusty: true,
};

#[test]
fn a_light_before_new_dust_takes_its_side_at_another_angle() {
    let _settings = settings_lock();
    let temp = tempfile::tempdir().unwrap();
    let (conn, _) = measured_library(
        temp.path(),
        &[
            BEFORE_HA_0,
            BEFORE_OIII_0,
            BEFORE_OIII_90,
            AFTER_HA_0,
            AFTER_HA_90,
            AFTER_OIII_0,
        ],
    );
    let book = super::load_book(&conn).unwrap();
    assert_eq!(book.events.len(), 1, "{:?}", book.events);
    let event = &book.events[0];
    assert!(event.camera_side, "two filters split at once");
    assert!((event.dust - 0.03).abs() < 0.008, "dust {}", event.dust);

    // Ha at 90° was only ever flatted after the dust arrived. The OIII
    // pair from the first morning measures the angle term, so the light
    // takes the first morning's Ha flats at 0°.
    let before = light(temp.path(), "2026-09-09T04:00:00", 90.0);
    let selected = select_for_light(&conn, &before).unwrap();
    assert_eq!(names(&selected.flat), ["before-ha-0"]);
    match &selected.flat_basis {
        FlatBasis::OtherAngle {
            rotation,
            placement,
            angle_term,
            ..
        } => {
            assert_eq!(*rotation, Some(0.0));
            assert_eq!(*placement, Placement::Time { side: Side::Before });
            assert_eq!(angle_term.pairs, 1);
        }
        other => panic!("{other:?}"),
    }
    assert!(!selected.flat_basis.flags());
    let delivery =
        crate::calibration::delivery_selection(select_for_light(&conn, &before).unwrap());
    let export = crate::calibration::export_destinations(
        &conn,
        &before,
        "Target",
        None,
        crate::commands::export::ExportLayout::Standard,
        crate::calibration::catalog_night_boundary(&conn),
        false,
    )
    .unwrap();
    let delivered = delivery
        .flat
        .iter()
        .map(|f| f.frame_uuid.clone())
        .collect::<std::collections::BTreeSet<_>>();
    let exported = export
        .items
        .iter()
        .filter(|(kind, _, _)| *kind == crate::calibration::CalibrationKind::Flat)
        .map(|(_, f, _)| f.frame_uuid.clone())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        delivered, exported,
        "other-angle flat delivery must match export exactly"
    );

    // The stacker holds a light to its flats' angle within 2°; the
    // light's gate is wide enough for these flats.
    let masters = seiza_stacking::CalibrationMasters::from_fits_paths(
        None,
        None,
        Some(selected.flat[0].source_path.as_path()),
        Some(300.0),
    )
    .unwrap();
    let light_frame = crate::image_io::open_linear_frame(&before.path).unwrap();
    assert!(masters.validate_light_frame(&light_frame).is_err());
    masters
        .validate_light_frame_with(&light_frame, &selected.match_tolerances())
        .unwrap();

    // At its own angle the nearest flats are from its side, and the ones
    // after the change lie too far off to join its master: the list is
    // left alone, so its stacks keep their keys.
    let own_angle = light(temp.path(), "2026-09-09T05:00:00", 0.0);
    let selected = select_for_light(&conn, &own_angle).unwrap();
    assert_eq!(names(&selected.flat), ["before-ha-0", "after-ha-0"]);
    assert_eq!(selected.flat_basis, FlatBasis::Nearest);

    // A light after the change keeps its nearest flats and nothing is said.
    let after = light(temp.path(), "2026-09-18T04:00:00", 90.0);
    let selected = select_for_light(&conn, &after).unwrap();
    assert_eq!(names(&selected.flat), ["after-ha-90"]);
    assert_eq!(selected.flat_basis, FlatBasis::Nearest);
}

/// The browser sessions of the flats of `sessions`.
fn browser_sessions(
    conn: &Connection,
    sessions: &[&Session],
) -> std::collections::BTreeSet<String> {
    crate::calibration::browse::library_frames(conn)
        .unwrap()
        .frames
        .into_iter()
        .filter(|row| {
            let name = Path::new(&row.frame.source_path)
                .file_stem()
                .unwrap()
                .to_string_lossy()
                .into_owned();
            sessions
                .iter()
                .any(|session| name.rsplit_once('-').unwrap().0 == session.name)
        })
        .map(|row| row.session)
        .collect()
}

#[test]
fn a_project_scope_holds_the_flats_its_lights_take_from_another_angle() {
    use crate::calibration::project_scope::project_scope;
    let _settings = settings_lock();
    let temp = tempfile::tempdir().unwrap();
    let (conn, _) = measured_library(
        temp.path(),
        &[
            BEFORE_HA_0,
            BEFORE_OIII_0,
            BEFORE_OIII_90,
            AFTER_HA_0,
            AFTER_HA_90,
            AFTER_OIII_0,
        ],
    );
    // A light at 90° before the dust takes the first morning's Ha flats at
    // 0°: that session is in its project's scope, and the Ha flats at 90°
    // after the dust, the nearest at its own angle, are not.
    let before = light(temp.path(), "2026-09-09T04:00:00", 90.0);
    let scope = project_scope(&conn, 1, std::slice::from_ref(&before), 0).unwrap();
    assert_eq!(scope.sessions, browser_sessions(&conn, &[&BEFORE_HA_0]));

    // A light at 0° before the dust keeps the dusty flats in its list only
    // so its stacks keep their keys; they are held back, so not in scope.
    let own_angle = light(temp.path(), "2026-09-09T05:00:00", 0.0);
    let scope = project_scope(&conn, 1, &[before, own_angle], 0).unwrap();
    assert_eq!(scope.sessions, browser_sessions(&conn, &[&BEFORE_HA_0]));

    // A light after the change takes the Ha flats at 90° after it.
    let after = light(temp.path(), "2026-09-18T04:00:00", 90.0);
    let scope = project_scope(&conn, 1, &[after], 0).unwrap();
    assert_eq!(scope.sessions, browser_sessions(&conn, &[&AFTER_HA_90]));
    assert_eq!(scope.rigs.len(), 1);
}

#[test]
fn a_project_scope_holds_the_sessions_a_correction_map_is_made_from() {
    use crate::calibration::project_scope::project_scope;
    let _settings = settings_lock();
    let temp = tempfile::tempdir().unwrap();
    let (conn, _) = measured_library(
        temp.path(),
        &[
            BEFORE_HA_0,
            BEFORE_OIII_0,
            AFTER_HA_0,
            AFTER_HA_90,
            AFTER_OIII_0,
        ],
    );
    let before = light(temp.path(), "2026-09-09T04:00:00", 90.0);
    let selected = select_for_light(&conn, &before).unwrap();
    let correction = selected.flat_basis.correction().expect("corrected").clone();
    let book = super::load_book(&conn).unwrap();
    let source: std::collections::HashSet<String> =
        [&correction.before_response, &correction.after_response]
            .into_iter()
            .flat_map(|key| {
                let index = book.session_by_response(key).expect("measured");
                book.sessions[index].row.frame_uuids.clone()
            })
            .collect();
    let library = crate::calibration::browse::library_frames(&conn).unwrap();
    let mut expected = browser_sessions(&conn, &[&AFTER_HA_90]);
    expected.extend(
        library
            .frames
            .iter()
            .filter(|row| source.contains(&row.frame.frame_uuid))
            .map(|row| row.session.clone()),
    );
    let scope = project_scope(&conn, 1, &[before], 0).unwrap();
    assert!(expected.len() > 1, "the map's sessions are others");
    assert_eq!(scope.sessions, expected);
}

#[test]
fn without_an_angle_term_the_other_side_is_carried_across() {
    let _settings = settings_lock();
    let temp = tempfile::tempdir().unwrap();
    let (conn, _) = measured_library(
        temp.path(),
        &[
            BEFORE_HA_0,
            BEFORE_OIII_0,
            AFTER_HA_0,
            AFTER_HA_90,
            AFTER_OIII_0,
        ],
    );
    let before = light(temp.path(), "2026-09-09T04:00:00", 90.0);
    let selected = select_for_light(&conn, &before).unwrap();
    assert_eq!(names(&selected.flat), ["after-ha-90"]);
    let correction = selected.flat_basis.correction().expect("corrected").clone();
    assert_eq!(correction.light_side, Side::Before);
    assert!(!selected.flat_basis.flags());

    // Carried across, the dusty flat loses its shadow.
    let master = BuiltMaster {
        path: temp.path().join("after-ha-90-0.fits"),
        master_uuid: "m".into(),
        skipped: Vec::new(),
        flat_star_masking: None,
        stability_note: None,
    };
    let root = temp.path().join("masters");
    std::fs::create_dir_all(&root).unwrap();
    let (corrected, _) = corrected_flat_master(&conn, &root, &master, &correction).unwrap();
    // The shadow's depth against the point mirrored through the centre,
    // where the vignette is the same and there is no shadow.
    let dip = |path: &Path| {
        let image = crate::image_io::open_linear_frame(path).unwrap().image;
        let at = |x: usize, y: usize| image.data[y * image.width + x];
        let (x, y) = (DUST.0 as usize, DUST.1 as usize);
        1.0 - at(x, y) / at(x, SIZE.1 - y)
    };
    let (raw, fixed) = (dip(&master.path), dip(&corrected.path));
    assert!(raw > 0.02, "raw dip {raw}");
    assert!(fixed.abs() < 0.006, "corrected dip {fixed}");

    // The corrected master keeps its source's signature, the light's
    // angle, so the stacker takes it with its default tolerances.
    let masters = seiza_stacking::CalibrationMasters::from_fits_paths(
        None,
        None,
        Some(corrected.path.as_path()),
        Some(300.0),
    )
    .unwrap();
    let light_frame = crate::image_io::open_linear_frame(&before.path).unwrap();
    masters.validate_light_frame(&light_frame).unwrap();
}

#[test]
fn one_filter_dust_and_no_angle_term_leaves_the_light_flagged() {
    let _settings = settings_lock();
    let temp = tempfile::tempdir().unwrap();
    let (conn, _) = measured_library(temp.path(), &[BEFORE_HA_0, AFTER_HA_0, AFTER_HA_90]);
    let book = super::load_book(&conn).unwrap();
    assert!(!book.events[0].camera_side);
    let before = light(temp.path(), "2026-09-09T04:00:00", 90.0);
    let selected = select_for_light(&conn, &before).unwrap();
    assert_eq!(names(&selected.flat), ["after-ha-90"]);
    assert!(
        matches!(selected.flat_basis, FlatBasis::NoSameSide { .. }),
        "{:?}",
        selected.flat_basis
    );
    assert!(selected.flat_basis.flags());
}

#[test]
fn grouping_by_time_leaves_selection_as_it_was() {
    let _settings = settings_lock();
    let _rules = crate::calibration::tests::RulesInForce::global(&[(
        "flat_grouping",
        serde_json::json!("time"),
    )]);
    let temp = tempfile::tempdir().unwrap();
    let (conn, _) = measured_library(temp.path(), &[BEFORE_HA_0, AFTER_HA_0, AFTER_HA_90]);
    let before = light(temp.path(), "2026-09-09T04:00:00", 90.0);
    let selected = select_for_light(&conn, &before).unwrap();
    assert_eq!(names(&selected.flat), ["after-ha-90"]);
    assert_eq!(selected.flat_basis, FlatBasis::Nearest);
}

/// One pass over a measured library, counting how often it yielded to
/// interactive work.
fn pass_counting_yields(conn: &Connection, dir: &Path) -> (super::job::PassOutcome, usize) {
    let yields = std::sync::atomic::AtomicUsize::new(0);
    let outcome = super::job::run(super::job::Pass {
        conn,
        dir,
        scope: super::job::Scope::everything(),
        keep_going: &|| true,
        yield_now: &|| {
            yields.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        },
        progress: &mut |_| {},
        goes_first: &|_| false,
        find_light: &|_| None,
        pool: None,
    })
    .unwrap();
    (outcome, yields.into_inner())
}

#[test]
fn comparing_yields_to_interactive_work() {
    let temp = tempfile::tempdir().unwrap();
    let (conn, dir) = measured_library(temp.path(), &[BEFORE_HA_0, AFTER_HA_0]);
    // Every response is kept; only the comparison is to be made again.
    conn.execute("DELETE FROM psf_guard_flat_comparison", [])
        .unwrap();
    let (outcome, yields) = pass_counting_yields(&conn, &dir);
    assert_eq!(outcome.measured, 0);
    assert_eq!(outcome.compared, 1);
    assert!(yields >= 1, "the comparison went straight through");
}

#[test]
fn a_session_measured_while_a_flat_was_away_is_measured_again() {
    let temp = tempfile::tempdir().unwrap();
    let session = Session {
        name: "four",
        captured: "2026-09-10T14:00",
        filter: "Ha",
        rotation: Some(0.0),
        dusty: false,
    };
    let mut frames = write_session(temp.path(), &session, 1);
    // A fourth flat of the same morning.
    let extra = Session {
        name: "four-more",
        captured: "2026-09-10T14:01",
        ..session
    };
    frames.push(write_session(temp.path(), &extra, 2).remove(0));
    let path = temp.path().join("catalog.sqlite");
    let mut conn = Connection::open(&path).unwrap();
    let tx = conn.transaction().unwrap();
    import_calibration_frames(&tx, &frames, Some("profile")).unwrap();
    tx.commit().unwrap();
    super::store::add_tables(&conn).unwrap();
    let dir = temp.path().join("responses");

    // One flat is away, as on a share that dropped: three still measure.
    let away = temp.path().join("four-more-0.fits");
    let hidden = temp.path().join("four-more-0.away");
    std::fs::rename(&away, &hidden).unwrap();
    let (first, _) = pass_counting_yields(&conn, &dir);
    assert_eq!(first.measured, 1);
    let combined =
        |conn: &Connection| -> usize { super::store::load_responses(conn).unwrap()[0].frames };
    assert_eq!(combined(&conn), 3);

    // Back: the session is measured again from all four.
    std::fs::rename(&hidden, &away).unwrap();
    let (second, _) = pass_counting_yields(&conn, &dir);
    assert_eq!(second.measured, 1);
    assert_eq!(combined(&conn), 4);
    let (third, _) = pass_counting_yields(&conn, &dir);
    assert_eq!(third.measured, 0, "complete now: not read again");
}

#[test]
fn a_bias_that_arrives_later_is_taken_out_and_the_nights_compared_again() {
    let temp = tempfile::tempdir().unwrap();
    let (conn, dir) = measured_library(temp.path(), &[BEFORE_HA_0, AFTER_HA_0]);
    let pedestal = |conn: &Connection| -> Vec<Option<f64>> {
        super::store::load_responses(conn)
            .unwrap()
            .iter()
            .map(|row| row.pedestal_adu)
            .collect()
    };
    assert_eq!(pedestal(&conn), [None, None]);

    // Bias frames of the same camera, measured at 500 ADU.
    let metas: Vec<FrameMeta> = (0..2)
        .map(|index| {
            let path = temp.path().join(format!("bias-{index}.fits"));
            write_frame_with(&path, "BIAS", SIZE, "2026-09-10T13:00:0", &[], |_, _| 500.0);
            read_frame_meta(&path)
        })
        .collect();
    {
        let tx = conn.unchecked_transaction().unwrap();
        import_calibration_frames(&tx, &metas, Some("profile")).unwrap();
        tx.commit().unwrap();
    }
    conn.execute(
        "INSERT INTO psf_guard_calibration_frame_quality (frame_uuid, measurement_json, updated_at)
         SELECT frame_uuid, '{\"level\": 500.0}', 0 FROM psf_guard_calibration_frame
         WHERE kind = 'bias'",
        [],
    )
    .unwrap();

    let (outcome, _) = pass_counting_yields(&conn, &dir);
    assert_eq!(outcome.measured, 0, "no flat is read again");
    assert_eq!(
        outcome.compared, 1,
        "the nights are compared with the bias out"
    );
    assert_eq!(pedestal(&conn), [Some(500.0), Some(500.0)]);
}

#[test]
fn a_twin_merged_away_has_its_session_measured_again_from_the_row_kept() {
    let temp = tempfile::tempdir().unwrap();
    let (mut conn, dir) = measured_library(temp.path(), &[BEFORE_HA_0, AFTER_HA_0]);
    // An older import catalogued one flat twice: a second row for a copy
    // of its file elsewhere, which the next pass measures with the rest.
    let copies = temp.path().join("copies");
    std::fs::create_dir_all(&copies).unwrap();
    let copy = copies.join("before-ha-0-0.fits");
    std::fs::copy(temp.path().join("before-ha-0-0.fits"), &copy).unwrap();
    let original: String = conn
        .query_row(
            "SELECT frame_uuid FROM psf_guard_calibration_frame WHERE source_path LIKE '%before-ha-0-0.fits'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let columns = crate::calibration::table_column_names(&conn, "psf_guard_calibration_frame")
        .unwrap()
        .into_iter()
        .filter(|name| !["id", "frame_uuid", "source_path"].contains(&name.as_str()))
        .collect::<Vec<_>>()
        .join(", ");
    conn.execute(
        &format!(
            "INSERT INTO psf_guard_calibration_frame (frame_uuid, source_path, {columns})
             SELECT 'second-row', ?1, {columns} FROM psf_guard_calibration_frame
             WHERE frame_uuid = ?2"
        ),
        rusqlite::params![crate::calibration::canonical_text(&copy), original],
    )
    .unwrap();
    let (outcome, _) = pass_counting_yields(&conn, &dir);
    assert_eq!(outcome.measured, 1, "the session with its second row");
    let holds = |conn: &Connection, uuid: &str| {
        super::store::load_responses(conn)
            .unwrap()
            .iter()
            .any(|row| row.frame_uuids.iter().any(|frame| frame == uuid))
    };
    assert!(holds(&conn, "second-row"));

    // The repair merges the second row into the first.
    let twins = crate::calibration::twins::plan_repair(
        &crate::calibration::twins::duplicate_snapshot(&conn).unwrap(),
        &crate::calibration::twins::FileAccess::disk(),
    );
    assert_eq!(twins.rows_removed, 1, "{twins:?}");
    crate::calibration::twins::apply_repair(&mut conn, &twins).unwrap();
    assert!(
        !holds(&conn, "second-row"),
        "the merged row's response is gone"
    );
    assert!(
        !holds(&conn, &original),
        "so is the one the kept row was measured in"
    );
    assert_eq!(
        super::store::load_comparisons(&conn).unwrap().len(),
        0,
        "and its comparisons"
    );
    let book = super::load_book(&conn).unwrap();
    assert!(book.session_of_frame("second-row").is_none());

    // The next pass measures the session from the rows the library holds.
    let (outcome, _) = pass_counting_yields(&conn, &dir);
    assert_eq!((outcome.measured, outcome.compared), (1, 1));
    assert!(holds(&conn, &original) && !holds(&conn, "second-row"));
}

#[test]
fn forgetting_a_flat_has_its_session_measured_again() {
    let temp = tempfile::tempdir().unwrap();
    let (mut conn, dir) = measured_library(temp.path(), &[BEFORE_HA_0, AFTER_HA_0]);
    let uuid: String = conn
        .query_row(
            "SELECT frame_uuid FROM psf_guard_calibration_frame WHERE source_path LIKE '%after-ha-0-0.fits'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    crate::calibration::forget_frame(&mut conn, &uuid).unwrap();
    assert_eq!(super::store::load_responses(&conn).unwrap().len(), 1);
    assert!(super::store::load_comparisons(&conn).unwrap().is_empty());
    let (outcome, _) = pass_counting_yields(&conn, &dir);
    // Two flats still make a response, as they make a master: the session
    // is measured again from the two left, and compared again.
    assert_eq!((outcome.measured, outcome.compared), (1, 1));
}

/// Three mornings of Ha flats: at 101.5° on either side of a morning whose
/// flats record no angle, as C925's 2026-08-18 flats. `dusty` puts new dust
/// on that morning alone.
fn unknown_angle_library(dir: &Path, dusty: bool) -> Connection {
    let sessions = [
        Session {
            name: "angled-1",
            captured: "2026-08-16T13:00",
            filter: "Ha",
            rotation: Some(101.5),
            dusty: false,
        },
        Session {
            name: "no-angle",
            captured: "2026-08-19T02:20",
            filter: "Ha",
            rotation: None,
            dusty,
        },
        Session {
            name: "angled-2",
            captured: "2026-08-22T12:20",
            filter: "Ha",
            rotation: Some(101.5),
            dusty: false,
        },
    ];
    measured_library(dir, &sessions).0
}

#[test]
fn flats_with_no_angle_serve_the_angle_they_measure_alike_with() {
    let _settings = settings_lock();
    let temp = tempfile::tempdir().unwrap();
    let conn = unknown_angle_library(temp.path(), false);
    let book = super::load_book(&conn).unwrap();
    let uuid: String = conn
        .query_row(
            "SELECT frame_uuid FROM psf_guard_calibration_frame WHERE source_path LIKE '%no-angle-0.fits'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(book.inferred_rotation(&uuid), Some(101.5));

    // A light at 101.5° that night takes them, nearest first.
    let near = light(temp.path(), "2026-08-19T06:00:00", 101.5);
    let selected = select_for_light(&conn, &near).unwrap();
    assert_eq!(names(&selected.flat)[0], "no-angle");

    // A light at 11.1° does not: they were shot at 101.5°. Nothing else
    // matches it, so it has no flats rather than another angle's.
    let far = light(temp.path(), "2026-08-19T06:30:00", 11.1);
    let selected = select_for_light(&conn, &far).unwrap();
    assert!(selected.flat.is_empty(), "{:?}", names(&selected.flat));

    // Grouping by time leaves Seiza's rule alone: no angle matches any.
    let _rules = crate::calibration::tests::RulesInForce::global(&[(
        "flat_grouping",
        serde_json::json!("time"),
    )]);
    let selected = select_for_light(&conn, &far).unwrap();
    assert_eq!(names(&selected.flat), ["no-angle"]);
}

#[test]
fn flats_of_unknown_angle_are_a_last_resort() {
    let _settings = settings_lock();
    let temp = tempfile::tempdir().unwrap();
    let conn = unknown_angle_library(temp.path(), true);
    let book = super::load_book(&conn).unwrap();
    assert!(
        book.sessions
            .iter()
            .all(|session| session.inferred_rotation.is_none()),
        "unlike both neighbours, the morning is at no angle"
    );

    // A light at 101.5° has flats of its angle: the unknown ones go,
    // though they are the nearest.
    let near = light(temp.path(), "2026-08-19T06:00:00", 101.5);
    let selected = select_for_light(&conn, &near).unwrap();
    assert!(!selected.flat.is_empty());
    assert!(
        !names(&selected.flat).contains(&"no-angle".to_string()),
        "{:?}",
        names(&selected.flat)
    );

    // A light at 11.1° has nothing else: they stay.
    let far = light(temp.path(), "2026-08-19T06:30:00", 11.1);
    let selected = select_for_light(&conn, &far).unwrap();
    assert_eq!(names(&selected.flat), ["no-angle"]);
}

#[test]
fn a_reducer_swap_is_another_family_not_new_dust() {
    let temp = tempfile::tempdir().unwrap();
    // The same filter, angle and gain, a week apart: the native focal
    // length, then a reducer.
    let mut frames = write_session(temp.path(), &BEFORE_HA_0, 1);
    let mut reduced = write_session(temp.path(), &AFTER_HA_0, 2);
    for meta in &mut frames {
        meta.focal_length_mm = Some(2350.0);
    }
    for meta in &mut reduced {
        meta.focal_length_mm = Some(1645.0);
    }
    frames.extend(reduced);
    let (conn, dir) = measured_frames(temp.path(), &frames);
    let rows = super::store::load_responses(&conn).unwrap();
    assert_eq!(rows.len(), 2);
    assert_ne!(rows[0].family_key, rows[1].family_key);
    assert!(super::store::load_comparisons(&conn).unwrap().is_empty());
    assert!(super::load_book(&conn).unwrap().events.is_empty());

    // A catalog measured before the key took the focal length: its rows
    // are renamed on the next pass, and nothing is read again.
    conn.execute("UPDATE psf_guard_flat_response SET family_key = 'old'", [])
        .unwrap();
    let (outcome, _) = pass_counting_yields(&conn, &dir);
    assert_eq!((outcome.measured, outcome.compared), (0, 0));
    let renamed = super::store::load_responses(&conn).unwrap();
    assert!(renamed.iter().all(|row| row.family_key != "old"));
    assert_ne!(renamed[0].family_key, renamed[1].family_key);
}

/// A second shadow, for a second change.
const SECOND: (f32, f32, f32, f32) = (60.0, 110.0, 14.0, 0.03);

/// Shadows `(x, y, radius, depth)` on a session's flats.
type Shadows = &'static [(f32, f32, f32, f32)];

/// Ha and OIII at 0° on four mornings: clean, a shadow twice, then a second
/// shadow too. Ha at 90° on the mornings given (name, time, shadows), days
/// from any other angle, so the rig's angle term is unmeasured.
fn two_changes(dir: &Path, ninety: &[(&'static str, &'static str, Shadows)]) -> Connection {
    let at_zero: [(&'static str, &'static str, &'static str, Shadows); 8] = [
        ("ha-clean", "2026-09-10T14:00", "Ha", &[]),
        ("oiii-clean", "2026-09-10T14:10", "OIII", &[]),
        ("ha-one", "2026-09-14T14:00", "Ha", &[DUST]),
        ("oiii-one", "2026-09-14T14:10", "OIII", &[DUST]),
        ("ha-one-again", "2026-09-15T14:00", "Ha", &[DUST]),
        ("oiii-one-again", "2026-09-15T14:10", "OIII", &[DUST]),
        ("ha-two", "2026-09-18T14:00", "Ha", &[DUST, SECOND]),
        ("oiii-two", "2026-09-18T14:10", "OIII", &[DUST, SECOND]),
    ];
    let sessions = at_zero
        .iter()
        .map(|(name, captured, filter, dust)| (*name, *captured, *filter, 0.0, *dust))
        .chain(
            ninety
                .iter()
                .map(|(name, captured, dust)| (*name, *captured, "Ha", 90.0, *dust)),
        );
    let mut frames = Vec::new();
    for (seed, (name, captured, filter, rotation, dust)) in sessions.enumerate() {
        let session = Session {
            name,
            captured,
            filter,
            rotation: Some(rotation),
            dusty: false,
        };
        // Ha at 90° is its own response family (see [`write_session`]).
        let mut shadows = dust.to_vec();
        if rotation == 90.0 {
            shadows.push(AT_90);
        }
        frames.extend(write_session_with(dir, &session, seed as u64 + 1, &shadows));
    }
    measured_frames(dir, &frames).0
}

#[test]
fn flats_across_two_changes_are_never_carried_across_one() {
    let _settings = settings_lock();
    let temp = tempfile::tempdir().unwrap();
    let conn = two_changes(
        temp.path(),
        &[("ha-90", "2026-09-22T14:00", &[DUST, SECOND])],
    );
    let book = super::load_book(&conn).unwrap();
    assert_eq!(book.events.len(), 2, "{:?}", book.events);
    assert!(book.events.iter().all(|event| event.camera_side));

    // Shot before both: its only flats at 90° carry both shadows, and one
    // change's map would take out one of them.
    let before = light(temp.path(), "2026-09-09T04:00:00", 90.0);
    let selected = select_for_light(&conn, &before).unwrap();
    assert_eq!(names(&selected.flat), ["ha-90"]);
    assert!(
        matches!(selected.flat_basis, FlatBasis::NoSameSide { .. }),
        "{:?}",
        selected.flat_basis
    );
    assert!(selected.flat_basis.correction().is_none());
    assert!(selected.flat_basis.flags());

    // Shot between them: one change away, it is carried across that one.
    let between = light(temp.path(), "2026-09-15T04:00:00", 90.0);
    let selected = select_for_light(&conn, &between).unwrap();
    let correction = selected.flat_basis.correction().expect("corrected");
    assert_eq!(correction.light_side, Side::Before);
    assert!(!selected.flat_basis.flags());
}

#[test]
fn older_flats_across_another_change_leave_the_nearest_corrected() {
    let _settings = settings_lock();
    let temp = tempfile::tempdir().unwrap();
    // As NGC 7331's L lights on C925: the nearest flats at its angle are
    // one change away, and older ones further back lie across another.
    let conn = two_changes(
        temp.path(),
        &[
            ("ha-90-early", "2026-09-09T02:00", &[]),
            ("ha-90-mid", "2026-09-17T02:00", &[DUST]),
        ],
    );
    let after = light(temp.path(), "2026-09-25T04:00:00", 90.0);
    let selected = select_for_light(&conn, &after).unwrap();
    assert_eq!(names(&selected.flat)[0], "ha-90-mid");
    let correction = selected
        .flat_basis
        .correction()
        .unwrap_or_else(|| panic!("{:?}", selected.flat_basis));
    assert_eq!(correction.light_side, Side::After);
    assert!(!selected.flat_basis.flags());
}

#[test]
fn flats_of_unknown_angle_alone_on_the_lights_side_are_kept() {
    // A camera-side change between 09-10 and 09-14, seen by OIII at 0° and
    // 90°. Ha's only flats after it record no angle (the rig's newest
    // flats, so no later 0° night to measure them alike with). A 0° Ha
    // light of 09-15 takes them: they are the only flats on its side. A
    // 0° Ha flat from before the change must not push them out as the
    // last resort and leave the light corrected from before it.
    let _settings = settings_lock();
    let temp = tempfile::tempdir().unwrap();
    let sessions = [
        ("ha-0-clean", "2026-09-10T14:00", "Ha", Some(0.0), false),
        ("oiii-0-clean", "2026-09-10T14:10", "OIII", Some(0.0), false),
        (
            "oiii-90-clean",
            "2026-09-10T14:20",
            "OIII",
            Some(90.0),
            false,
        ),
        ("oiii-0-dusty", "2026-09-14T14:10", "OIII", Some(0.0), true),
        (
            "oiii-90-dusty",
            "2026-09-14T14:20",
            "OIII",
            Some(90.0),
            true,
        ),
        ("no-angle", "2026-09-14T14:30", "Ha", None, true),
    ];
    let mut frames = Vec::new();
    for (seed, (name, captured, filter, rotation, dusty)) in sessions.into_iter().enumerate() {
        let session = Session {
            name,
            captured,
            filter,
            rotation,
            dusty,
        };
        let mut dust: Vec<(f32, f32, f32, f32)> = Vec::new();
        if dusty {
            dust.push(DUST);
        }
        if rotation == Some(90.0) {
            dust.push(AT_90);
        }
        frames.extend(write_session_with(
            temp.path(),
            &session,
            seed as u64 + 1,
            &dust,
        ));
    }
    let (conn, _) = measured_frames(temp.path(), &frames);
    let book = super::load_book(&conn).unwrap();
    assert!(
        book.events.iter().any(|event| event.camera_side),
        "{:?}",
        book.events
    );
    let after = light(temp.path(), "2026-09-15T04:00:00", 0.0);
    let selected = select_for_light(&conn, &after).unwrap();
    assert_eq!(
        names(&selected.flat).first().map(String::as_str),
        Some("no-angle"),
        "{:?} {:?}",
        names(&selected.flat),
        selected.flat_basis
    );
}

#[test]
fn focal_lengths_within_the_tolerance_are_one_class() {
    let temp = tempfile::tempdir().unwrap();
    let focal = |session: &Session, seed, mm: Option<f64>| {
        let mut metas = write_session(temp.path(), session, seed);
        for meta in &mut metas {
            meta.focal_length_mm = mm;
        }
        metas
    };
    let class_of = |conn: &Connection| {
        let library = crate::calibration::LibrarySnapshot::load(conn).unwrap();
        let classes = super::job::focal_classes(library.frames(), |_| 1.0);
        let mut named: Vec<(String, Option<f64>)> = library
            .frames()
            .iter()
            .map(|frame| {
                let name = frame.source_path.file_stem().unwrap().to_string_lossy();
                (
                    name.rsplit_once('-').unwrap().0.to_string(),
                    classes[&frame.frame_uuid],
                )
            })
            .collect();
        named.sort_by(|a, b| a.0.cmp(&b.0));
        named.dedup();
        named
    };
    // 2,350 mm and 2,350.6 mm, and a session that records none: one rig,
    // one focal length.
    let mut frames = focal(&BEFORE_HA_0, 1, Some(2350.0));
    frames.extend(focal(&AFTER_HA_0, 2, Some(2350.6)));
    frames.extend(focal(&BEFORE_OIII_0, 3, None));
    let (conn, _) = measured_frames(temp.path(), &frames);
    assert_eq!(
        class_of(&conn),
        [
            ("after-ha-0".to_string(), Some(2350.0)),
            ("before-ha-0".to_string(), Some(2350.0)),
            ("before-oiii-0".to_string(), Some(2350.0)),
        ]
    );
    let rows = super::store::load_responses(&conn).unwrap();
    let ha: Vec<&String> = rows
        .iter()
        .filter(|row| row.filter.as_deref() == Some("Ha"))
        .map(|row| &row.family_key)
        .collect();
    assert_eq!(ha.len(), 2);
    assert_eq!(ha[0], ha[1], "0.6 mm apart is one light path");

    // With a reducer at 1,645 mm too, a flat with no focal length could be
    // either: it keeps to itself.
    let more = focal(&AFTER_OIII_0, 4, Some(1645.0));
    {
        let tx = conn.unchecked_transaction().unwrap();
        crate::calibration::import_calibration_frames(&tx, &more, Some("profile")).unwrap();
        tx.commit().unwrap();
    }
    let classes = class_of(&conn);
    assert!(classes.contains(&("after-oiii-0".to_string(), Some(1645.0))));
    assert!(classes.contains(&("before-oiii-0".to_string(), None)));
    assert!(classes.contains(&("before-ha-0".to_string(), Some(2350.0))));
}

/// Flats at 82.2° and 72.6° shot one morning, alike, as C925's OIII: one
/// response family. A light at either angle takes its own angle's flats
/// first and the other angle's after them, since their sessions measured
/// alike. A light at 77°, between them, has no flats at its angle: the
/// header alone hands it none, as before families.
#[test]
fn a_light_takes_its_familys_other_angle_only_where_measured_alike() {
    let _settings = settings_lock();
    let temp = tempfile::tempdir().unwrap();
    let (conn, _) = measured_library(
        temp.path(),
        &[
            Session {
                name: "ha-82",
                captured: "2026-09-10T14:00",
                filter: "Ha",
                rotation: Some(82.2),
                dusty: false,
            },
            Session {
                name: "ha-72",
                captured: "2026-09-10T14:30",
                filter: "Ha",
                rotation: Some(72.6),
                dusty: false,
            },
        ],
    );
    let book = super::load_book(&conn).unwrap();
    assert_eq!(book.families.len(), 1, "{:?}", book.families);
    assert_eq!(book.angle_label(0), "72.6° & 82.2°");

    let at_72 = light(temp.path(), "2026-09-12T05:00:00", 72.6);
    let selected = select_for_light(&conn, &at_72).unwrap();
    assert_eq!(names(&selected.flat), ["ha-72", "ha-82"]);
    let family = selected.flat_family.as_ref().expect("its family");
    assert_eq!(family.label, "Ha at 72.6° & 82.2°");
    assert!(selected.match_tolerances().light_rotation.is_recorded());
    assert!(super::describe_family(family)
        .unwrap()
        .contains("whose flats at those angles measure alike"));

    // Between them: nothing at its angle, and nothing measured for it.
    let between = light(temp.path(), "2026-09-12T04:00:00", 77.0);
    let selected = select_for_light(&conn, &between).unwrap();
    assert!(selected.flat.is_empty(), "{:?}", names(&selected.flat));
    assert!(selected.flat_family.is_none());

    // Grouping by time knows no families: the 72.6° light takes its own
    // angle's flats alone.
    let _time = crate::calibration::tests::RulesInForce::global(&[(
        "flat_grouping",
        serde_json::json!("time"),
    )]);
    let selected = select_for_light(&conn, &at_72).unwrap();
    assert_eq!(names(&selected.flat), ["ha-72"]);
    assert!(selected.flat_family.is_none());
}

/// Flats at 0° and at 8° of `filter`, one morning, apart: a shadow ahead of
/// the rotator lands at 8° only.
fn eight_and_zero(dir: &Path, filter: &'static str, seed: u64) -> Vec<FrameMeta> {
    let mut frames = Vec::new();
    for (offset, (name, captured, rotation, shadows)) in [
        (
            if filter == "Ha" { "ha-0" } else { "oiii-0" },
            "2026-09-10T14:00",
            0.0,
            &[][..],
        ),
        (
            if filter == "Ha" { "ha-8" } else { "oiii-8" },
            "2026-09-10T14:30",
            8.0,
            &[AT_90][..],
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let session = Session {
            name,
            captured,
            filter,
            rotation: Some(rotation),
            dusty: false,
        };
        frames.extend(write_session_with(
            dir,
            &session,
            seed + offset as u64,
            shadows,
        ));
    }
    frames
}

/// Five lights of `filter` recorded at 4°, shot through the 8° light path,
/// in the catalog as N.I.N.A. records them, from `first` on.
fn lights_through_eight(conn: &Connection, dir: &Path, filter: &str, first: u64, minute: u32) {
    for index in 0..5u64 {
        let id = first + index;
        let name = format!("light-{id}.fits");
        let path = dir.join(&name);
        let plane = super::testing::synthetic(SIZE.0, SIZE.1, &[AT_90], 0.0, &[], 0.002, 50 + id);
        let captured = format!("2026-09-11T04:{:02}:00", minute + index as u32);
        write_frame_with(
            &path,
            "LIGHT",
            SIZE,
            &captured,
            &["ROTATANG=                  4.0"],
            |x, y| 1_000.0 * f64::from(plane.data[y * SIZE.0 + x]),
        );
        let at = chrono::NaiveDateTime::parse_from_str(&captured, "%Y-%m-%dT%H:%M:%S")
            .unwrap()
            .and_utc()
            .timestamp();
        conn.execute(
            "INSERT INTO acquiredimage (Id, projectId, targetId, acquireddate, filtername,
             gradingStatus, metadata) VALUES (?1, 1, 1, ?2, ?3, 0, ?4)",
            rusqlite::params![
                id as i64,
                at,
                filter,
                serde_json::json!({ "FileName": path, "RotatorMechanicalPosition": 4.0 })
                    .to_string()
            ],
        )
        .unwrap();
    }
}

fn light_tables(conn: &Connection) {
    conn.execute_batch(
        "CREATE TABLE acquiredimage (Id INTEGER PRIMARY KEY, projectId INTEGER,
            targetId INTEGER, acquireddate INTEGER, filtername TEXT, gradingStatus INTEGER,
            metadata TEXT);
         CREATE TABLE target (Id INTEGER PRIMARY KEY, name TEXT);
         INSERT INTO target (Id, name) VALUES (1, 'M 27');",
    )
    .unwrap();
}

fn pass_with_lights(conn: &Connection, responses: &Path, dir: &Path) -> super::job::PassOutcome {
    super::job::run(super::job::Pass {
        conn,
        dir: responses,
        scope: super::job::Scope::everything(),
        keep_going: &|| true,
        yield_now: &|| {},
        progress: &mut |_| {},
        goes_first: &|_| false,
        find_light: &|name: &str| Some(dir.join(name)).filter(|path| path.is_file()),
        pool: None,
    })
    .unwrap()
}

/// Lights recorded at 4° lie within the angle prior of both Hα families
/// and at the angle of neither; they were shot through the 8° light path.
/// Their own pixels point to 8°, but one filter's word alone moves nothing:
/// the light is flagged and gets no flats from another angle. Once the
/// session's OIII lights, through the same path, point to the OIII 8°
/// family too, the move stands, and the stack reads the light at 8°.
#[test]
fn the_lights_own_pixels_move_a_light_only_with_another_filter_agreeing() {
    let _settings = settings_lock();
    let temp = tempfile::tempdir().unwrap();
    let mut frames = eight_and_zero(temp.path(), "Ha", 1);
    frames.extend(eight_and_zero(temp.path(), "OIII", 11));
    let (conn, responses) = measured_frames(temp.path(), &frames);
    let book = super::load_book(&conn).unwrap();
    assert_eq!(book.families.len(), 4, "{:?}", book.families);
    light_tables(&conn);
    lights_through_eight(&conn, temp.path(), "Ha", 1, 0);
    let dir = temp.path().to_path_buf();
    let outcome = pass_with_lights(&conn, &responses, &dir);
    assert_eq!(outcome.families, 1, "{outcome:?}");

    let ha_light = read_frame_meta(&temp.path().join("light-3.fits"));
    let selected = select_for_light(&conn, &ha_light).unwrap();
    let family = selected.flat_family.as_ref().expect("its family");
    assert!(family.flags(), "{family:?}");
    assert_eq!(family.pointed.as_deref(), Some("Ha at 8.0°"));
    assert!(selected.flat.is_empty(), "{:?}", names(&selected.flat));
    assert!(super::describe_family_flag(family)
        .unwrap()
        .contains("neither its other filters nor the rotator's epochs confirm the move"));

    // The session's OIII lights point the same way.
    lights_through_eight(&conn, temp.path(), "OIII", 11, 10);
    let outcome = pass_with_lights(&conn, &responses, &dir);
    assert_eq!(
        outcome.families, 1,
        "only the OIII lights are read: {outcome:?}"
    );
    let selected = select_for_light(&conn, &ha_light).unwrap();
    let family = selected.flat_family.as_ref().expect("its family");
    assert!(
        matches!(
            &family.basis,
            super::lights::FamilyBasis::Pixels { moved: true, corroborated: Some(by), .. }
                if by == "its OIII lights"
        ),
        "{family:?}"
    );
    assert_eq!(names(&selected.flat)[0], "ha-8");
    assert_eq!(
        selected.match_tolerances().light_rotation,
        seiza_calibration::LightRotation::Measured { degrees: 8.0 }
    );

    // A further pass reads nothing: every answer is kept under its key.
    let again = pass_with_lights(&conn, &responses, &dir);
    assert_eq!((again.families, again.frames_read), (0, 0), "{again:?}");
}

/// Lights at 8.5°, recorded at their family's angle and shot through it:
/// their own pixels confirm the family at their recorded angle, freely.
#[test]
fn the_lights_own_pixels_confirm_their_recorded_family() {
    let _settings = settings_lock();
    let temp = tempfile::tempdir().unwrap();
    let frames = eight_and_zero(temp.path(), "Ha", 1);
    let (conn, responses) = measured_frames(temp.path(), &frames);
    light_tables(&conn);
    for index in 0..5u64 {
        let id = index + 1;
        let path = temp.path().join(format!("light-{id}.fits"));
        let plane = super::testing::synthetic(SIZE.0, SIZE.1, &[AT_90], 0.0, &[], 0.002, 70 + id);
        let captured = format!("2026-09-11T04:0{index}:00");
        write_frame_with(
            &path,
            "LIGHT",
            SIZE,
            &captured,
            &["ROTATANG=                  7.5"],
            |x, y| 1_000.0 * f64::from(plane.data[y * SIZE.0 + x]),
        );
        let at = chrono::NaiveDateTime::parse_from_str(&captured, "%Y-%m-%dT%H:%M:%S")
            .unwrap()
            .and_utc()
            .timestamp();
        conn.execute(
            "INSERT INTO acquiredimage (Id, projectId, targetId, acquireddate, filtername,
             gradingStatus, metadata) VALUES (?1, 1, 1, ?2, 'Ha', 0, ?3)",
            rusqlite::params![
                id as i64,
                at,
                serde_json::json!({ "FileName": path, "RotatorMechanicalPosition": 7.5 })
                    .to_string()
            ],
        )
        .unwrap();
    }
    let dir = temp.path().to_path_buf();
    pass_with_lights(&conn, &responses, &dir);
    let light = read_frame_meta(&temp.path().join("light-2.fits"));
    let selected = select_for_light(&conn, &light).unwrap();
    let family = selected.flat_family.as_ref().expect("its family");
    assert!(
        matches!(
            family.basis,
            super::lights::FamilyBasis::Pixels { moved: false, .. }
        ),
        "{family:?}"
    );
    assert!(!family.flags());
    assert_eq!(names(&selected.flat), ["ha-8"]);
    assert!(super::describe_family(family)
        .unwrap()
        .starts_with("Chosen by its own pixels"));
}

/// A catalog of these sessions with the comparisons `dust` names, no pixels
/// read.
fn faked_library(
    dir: &Path,
    sessions: &[Session],
    dust: impl Fn(&super::store::ResponseRow, &super::store::ResponseRow) -> f64,
) -> Connection {
    let frames: Vec<FrameMeta> = sessions
        .iter()
        .enumerate()
        .flat_map(|(seed, session)| write_session(dir, session, seed as u64 + 1))
        .collect();
    let path = dir.join("catalog.sqlite");
    let mut conn = Connection::open(&path).unwrap();
    let tx = conn.transaction().unwrap();
    import_calibration_frames(&tx, &frames, Some("profile")).unwrap();
    tx.commit().unwrap();
    super::testing::measure_as(&conn, dust);
    conn
}

/// Ha at 80° on days 1 and 7, at 140° on day 4. 80°/day 1 and 140°/day 4
/// measure alike as time neighbours; 140°/day 4 and 80°/day 7 measure 3x
/// apart (the angle matters); the two 80° nights are alike. Nothing was
/// shot within one window. A neighbour pair apart keeps the two angles
/// apart, so no change is made of an angle difference, and a 140° light
/// keeps its own angle's flats, read as recorded.
#[test]
fn a_neighbour_pair_apart_keeps_two_angles_two_families() {
    let _settings = settings_lock();
    let temp = tempfile::tempdir().unwrap();
    let sessions = [
        Session {
            name: "ha-80a",
            captured: "2026-09-01T14:00",
            filter: "Ha",
            rotation: Some(80.0),
            dusty: false,
        },
        Session {
            name: "ha-140a",
            captured: "2026-09-04T14:00",
            filter: "Ha",
            rotation: Some(140.0),
            dusty: false,
        },
        Session {
            name: "ha-80b",
            captured: "2026-09-07T14:00",
            filter: "Ha",
            rotation: Some(80.0),
            dusty: false,
        },
    ];
    let day5 = chrono::NaiveDate::from_ymd_opt(2026, 9, 5)
        .unwrap()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc()
        .timestamp();
    let conn = faked_library(temp.path(), &sessions, |a, b| {
        let late = |row: &super::store::ResponseRow| row.captured_first.unwrap() > day5;
        if a.rotation != b.rotation && (late(a) || late(b)) {
            0.03
        } else {
            0.002
        }
    });
    let book = super::load_book(&conn).unwrap();
    assert_eq!(book.families.len(), 2, "{:?}", book.families);
    assert!(book.events.is_empty(), "{:?}", book.events);

    let light = light(temp.path(), "2026-09-08T04:00:00", 140.0);
    let selected = select_for_light(&conn, &light).unwrap();
    assert_eq!(names(&selected.flat), vec!["ha-140a"]);
    assert!(selected.match_tolerances().light_rotation.is_recorded());
}

/// A split between two angles of one family is never a change. Ha at 80°
/// and 140° measure alike as neighbours (one family), and 80° flats a
/// week later differ from the 140° ones only by a pair never compared at
/// two angles: no event is made of it.
#[test]
fn a_split_between_two_angles_of_a_family_makes_no_change() {
    let _settings = settings_lock();
    let temp = tempfile::tempdir().unwrap();
    let conn = faked_library(
        temp.path(),
        &[
            Session {
                name: "ha-80a",
                captured: "2026-09-01T14:00",
                filter: "Ha",
                rotation: Some(80.0),
                dusty: false,
            },
            Session {
                name: "ha-140a",
                captured: "2026-09-04T14:00",
                filter: "Ha",
                rotation: Some(140.0),
                dusty: false,
            },
        ],
        |_, _| 0.002,
    );
    let book = super::load_book(&conn).unwrap();
    assert_eq!(book.families.len(), 1, "{:?}", book.families);
    assert!(book.events.is_empty());
}

/// Ha families at 82° and 90°, measured apart: a light at 82° has two
/// candidates and, until its lights are read, takes the family at its
/// recorded angle. New flats at exactly 82°, not measured yet, still serve
/// it, as before families: the family at its recorded angle refuses
/// nothing at its angle.
#[test]
fn unmeasured_flats_at_the_lights_own_angle_still_serve_it() {
    let _settings = settings_lock();
    let temp = tempfile::tempdir().unwrap();
    let conn = faked_library(
        temp.path(),
        &[
            Session {
                name: "ha-82",
                captured: "2026-09-01T14:00",
                filter: "Ha",
                rotation: Some(82.0),
                dusty: false,
            },
            Session {
                name: "ha-90",
                captured: "2026-09-01T14:30",
                filter: "Ha",
                rotation: Some(90.0),
                dusty: false,
            },
        ],
        |a, b| if a.rotation == b.rotation { 0.0 } else { 0.03 },
    );
    let fresh = write_session(
        temp.path(),
        &Session {
            name: "ha-82new",
            captured: "2026-09-05T14:00",
            filter: "Ha",
            rotation: Some(82.0),
            dusty: false,
        },
        9,
    );
    {
        let tx = conn.unchecked_transaction().unwrap();
        import_calibration_frames(&tx, &fresh, Some("profile")).unwrap();
        tx.commit().unwrap();
    }
    let light = light(temp.path(), "2026-09-06T04:00:00", 82.0);
    let selected = select_for_light(&conn, &light).unwrap();
    let family = selected.flat_family.as_ref().unwrap();
    assert_eq!(
        family.basis,
        super::lights::FamilyBasis::Nearest { undecided: false }
    );
    assert!(family.at_recorded);
    assert_eq!(
        names(&selected.flat)[0],
        "ha-82new",
        "{:?}",
        names(&selected.flat)
    );
}

/// One Ha family at 0°, nothing else. A light recorded at 8° (the rig's
/// tolerance is 2°) had no flats on main, and has none now: the header
/// alone never widens the rig's rotation tolerance, nor reads the light at
/// another angle.
#[test]
fn the_header_alone_never_widens_the_rotation_tolerance() {
    let _settings = settings_lock();
    let temp = tempfile::tempdir().unwrap();
    let conn = faked_library(
        temp.path(),
        &[Session {
            name: "ha-0",
            captured: "2026-09-01T14:00",
            filter: "Ha",
            rotation: Some(0.0),
            dusty: false,
        }],
        |_, _| 0.0,
    );
    let light = light(temp.path(), "2026-09-02T04:00:00", 8.0);
    let selected = select_for_light(&conn, &light).unwrap();
    assert!(selected.flat.is_empty(), "{:?}", names(&selected.flat));
    assert!(selected.flat_family.is_none());
    assert!(selected.match_tolerances().light_rotation.is_recorded());
}
