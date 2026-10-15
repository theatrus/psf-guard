//! End to end on synthetic frames: flats written as FITS, measured by the
//! pass, grouped, and then chosen for lights on either side of a change.

use super::model::{Placement, Side};
use super::{metric, FlatBasis};
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

/// One morning's flats: three frames of one filter at one angle.
struct Session {
    name: &'static str,
    captured: &'static str,
    filter: &'static str,
    rotation: f64,
    dusty: bool,
}

fn write_session(dir: &Path, session: &Session, seed: u64) -> Vec<FrameMeta> {
    (0..3)
        .map(|index| {
            let path = dir.join(format!("{}-{index}.fits", session.name));
            let dust: &[(f32, f32, f32, f32)] = if session.dusty { &[DUST] } else { &[] };
            let plane = metric::tests::synthetic(
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
            let angle = format!("ROTATANG= {:20.1}", session.rotation);
            write_frame_with(&path, "FLAT", SIZE, &captured, &[angle.as_str()], |x, y| {
                30_000.0 * f64::from(plane.data[y * SIZE.0 + x])
            });
            let mut meta = read_frame_meta(&path);
            meta.filter = Some(session.filter.to_string());
            meta.rotator_position = Some(session.rotation);
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
    let path = dir.join("catalog.sqlite");
    let mut conn = Connection::open(&path).unwrap();
    let tx = conn.transaction().unwrap();
    import_calibration_frames(&tx, &frames, Some("profile")).unwrap();
    tx.commit().unwrap();
    super::store::add_tables(&conn).unwrap();
    let responses = dir.join("responses");
    super::register_response_dir(&path, responses.clone());
    let outcome = super::job::run(super::job::Pass {
        conn: &conn,
        dir: &responses,
        keep_going: &|| true,
        yield_now: &|| {},
        progress: &mut |_, _, _| {},
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
    let path = dir.join(format!("light-{captured}.fits"));
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
    rotation: 0.0,
    dusty: false,
};
const BEFORE_OIII_0: Session = Session {
    name: "before-oiii-0",
    captured: "2026-09-10T14:10",
    filter: "OIII",
    rotation: 0.0,
    dusty: false,
};
const BEFORE_OIII_90: Session = Session {
    name: "before-oiii-90",
    captured: "2026-09-10T14:20",
    filter: "OIII",
    rotation: 90.0,
    dusty: false,
};
const AFTER_HA_0: Session = Session {
    name: "after-ha-0",
    captured: "2026-09-14T14:00",
    filter: "Ha",
    rotation: 0.0,
    dusty: true,
};
const AFTER_HA_90: Session = Session {
    name: "after-ha-90",
    // Days later, so no pair at two angles measures the rig.
    captured: "2026-09-17T14:00",
    filter: "Ha",
    rotation: 90.0,
    dusty: true,
};
const AFTER_OIII_0: Session = Session {
    name: "after-oiii-0",
    captured: "2026-09-14T14:20",
    filter: "OIII",
    rotation: 0.0,
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
