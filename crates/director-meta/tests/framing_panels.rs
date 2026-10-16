//! Mosaic panels placed one by one: the schema 29 migration from every
//! older stored shape, the schema 30 measured panels, locks, and what
//! counts as a layout change.

use psf_guard_director_core::{
    framing::{
        FramingRequest, Layout, MeasuredGeometry, MeasuredSource, Mosaic, PanelPlacement, PanelSize,
    },
    visibility::IcrsPosition,
};
use psf_guard_director_meta::{
    framing::{FramingDraft, RigFraming},
    Error, MetaStore, Uuid,
};
use rusqlite::{params, Connection};
use serde_json::json;
use tempfile::TempDir;

fn grid(rows: u32, columns: u32, overlap_percent: u32) -> Mosaic {
    Mosaic {
        rows,
        columns,
        overlap_percent,
    }
}

/// A draft as schema 28 stored it, with a `mosaic` grid.
fn grid_payload(project: Uuid, revision: u64, mosaic: Mosaic) -> serde_json::Value {
    json!({
        "project_id": project,
        "revision": revision,
        "target_name": "Heart",
        "center": { "ra_degrees": 38.2, "dec_degrees": 61.45 },
        "position_angle_degrees": 35.0,
        "mosaic": mosaic,
        "panel_rig_id": null,
        "panel": { "width_degrees": 1.4, "height_degrees": 1.05 },
        "shown_rig_ids": [],
        "survey_id": "dss2_color",
        "view_fov_degrees": 6.0,
        "updated_at_ms": 1_000,
        "rig_framings": [],
        "layout_revision": revision,
    })
}

fn store_raw(conn: &Connection, project: Uuid, revision: u64, payload: &serde_json::Value) {
    conn.execute(
        "INSERT INTO framing_draft(project_id,revision,payload) VALUES(?1,?2,?3)",
        params![project.to_string(), revision as i64, payload.to_string()],
    )
    .unwrap();
}

fn raw(conn: &Connection, project: Uuid) -> String {
    conn.query_row(
        "SELECT CAST(payload AS TEXT) FROM framing_draft WHERE project_id=?1",
        [project.to_string()],
        |row| row.get(0),
    )
    .unwrap()
}

/// Each panel's center on the sky, by id.
fn centers(draft: &FramingDraft) -> Vec<(String, IcrsPosition)> {
    FramingRequest {
        center: draft.center,
        position_angle_degrees: draft.position_angle_degrees,
        panel: draft.panel.unwrap(),
        layout: draft.layout.clone(),
        overlays: vec![],
        view: None,
    }
    .preview()
    .unwrap()
    .panels
    .into_iter()
    .map(|panel| (panel.footprint.id, panel.footprint.center))
    .collect()
}

#[test]
fn stored_grids_from_every_older_shape_become_their_panels_in_place() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("meta.sqlite");
    let mut store = MetaStore::create(&path).unwrap();
    let names = [
        "Schema 7",
        "No center",
        "Schema 28",
        "Single",
        "Damaged",
        "Both",
    ];
    let projects: Vec<Uuid> = names
        .iter()
        .map(|name| store.create_project(Uuid::new_v4(), name).unwrap().id)
        .collect();
    let rig = store.create_rig(Uuid::new_v4(), "C925").unwrap().id;
    drop(store);
    let conn = Connection::open(&path).unwrap();

    // As first stored: no rig framings, no layout revision.
    let mut oldest = grid_payload(projects[0], 4, grid(2, 3, 15));
    let fields = oldest.as_object_mut().unwrap();
    fields.remove("rig_framings");
    fields.remove("layout_revision");
    store_raw(&conn, projects[0], 4, &oldest);
    // A rig framed on its own before rig framings had a center.
    let mut no_center = grid_payload(projects[1], 2, grid(1, 2, 20));
    no_center["rig_framings"] = json!([{
        "rig_id": rig,
        "position_angle_degrees": 90.0,
        "mosaic": grid(3, 1, 10),
        "panel": null,
    }]);
    store_raw(&conn, projects[1], 2, &no_center);
    // The last grid shape: a rig framing with a center of its own.
    let mut latest = grid_payload(projects[2], 7, grid(4, 4, 25));
    latest["layout_revision"] = json!(5);
    latest["rig_framings"] = json!([{
        "rig_id": rig,
        "center": { "ra_degrees": 39.0, "dec_degrees": 61.0 },
        "position_angle_degrees": null,
        "mosaic": grid(2, 2, 10),
        "panel": { "width_degrees": 0.6, "height_degrees": 0.4 },
    }]);
    store_raw(&conn, projects[2], 7, &latest);
    store_raw(
        &conn,
        projects[3],
        1,
        &grid_payload(projects[3], 1, grid(1, 1, 20)),
    );
    // A record that never read is left byte for byte.
    conn.execute(
        "INSERT INTO framing_draft(project_id,revision,payload) VALUES(?1,3,'{\"project_id\":')",
        [projects[4].to_string()],
    )
    .unwrap();
    // A payload with both shapes is not a draft; it stays as it was.
    let mut both = grid_payload(projects[5], 1, grid(1, 2, 20));
    both["layout"] = serde_json::to_value(Layout::grid(grid(1, 2, 20))).unwrap();
    store_raw(&conn, projects[5], 1, &both);
    let damaged_before = raw(&conn, projects[4]);
    let both_before = raw(&conn, projects[5]);
    conn.pragma_update(None, "user_version", 28).unwrap();

    // Read the way schema 28 laid them out: the grid, centered.
    let before: Vec<Option<FramingDraft>> = projects[..4]
        .iter()
        .map(|project| serde_json::from_str(&raw(&conn, *project)).ok())
        .collect();
    assert!(before.iter().all(Option::is_some));

    let store = MetaStore::open(&path).unwrap();
    assert_eq!(
        conn.pragma_query_value(None, "user_version", |r| r.get::<_, i32>(0))
            .unwrap(),
        32
    );
    for (index, project) in projects[..4].iter().enumerate() {
        let stored = raw(&conn, *project);
        assert!(
            stored.contains("\"layout\"") && !stored.contains("\"mosaic\""),
            "{}: {stored}",
            names[index]
        );
        let draft = store.framing_draft(*project).unwrap().unwrap();
        let mut old = before[index].clone().unwrap();
        // A draft saved before layouts were tracked reads its revision.
        if old.layout_revision == 0 {
            old.layout_revision = old.revision;
        }
        let old = &old;
        // Nothing but the shape changed: the revisions stay, so no open
        // editor is told it is stale and no activation is asked for.
        assert_eq!(draft.revision, old.revision);
        assert_eq!(draft.layout_revision, old.layout_revision);
        assert_eq!(&draft, old, "{}", names[index]);
        assert!(draft.layout.as_grid().is_some());
        assert!(draft.layout.panels.iter().all(|panel| !panel.locked));
        // Every panel where the grid had it, under the grid's own id.
        for ((id, center), (old_id, old_center)) in centers(&draft).iter().zip(centers(old)) {
            assert_eq!(id, &old_id);
            assert!(
                (center.ra_degrees - old_center.ra_degrees).abs() < 1e-12
                    && (center.dec_degrees - old_center.dec_degrees).abs() < 1e-12
            );
        }
    }
    let oldest = store.framing_draft(projects[0]).unwrap().unwrap();
    assert_eq!(oldest.layout, Layout::grid(grid(2, 3, 15)));
    assert_eq!(oldest.layout_revision, 4);
    let no_center = store.framing_draft(projects[1]).unwrap().unwrap();
    assert_eq!(
        no_center.rig_framings[0].layout,
        Layout::grid(grid(3, 1, 10))
    );
    assert_eq!(no_center.rig_framings[0].center, None);
    let latest = store.framing_draft(projects[2]).unwrap().unwrap();
    assert_eq!(latest.layout.panels.len(), 16);
    assert_eq!(latest.layout_revision, 5);
    assert_eq!(latest.rig_framings[0].layout, Layout::grid(grid(2, 2, 10)));
    let single = store.framing_draft(projects[3]).unwrap().unwrap();
    assert_eq!(single.layout.panels.len(), 1);
    assert_eq!(single.layout.panels[0].id, "r1c1");
    assert_eq!(single.layout.min_overlap_percent, 20);

    assert_eq!(raw(&conn, projects[4]), damaged_before);
    assert!(store.framing_draft(projects[4]).is_err());
    assert_eq!(raw(&conn, projects[5]), both_before);
    assert!(store.framing_draft(projects[5]).is_err());

    // Opening again changes nothing more.
    let after: Vec<String> = projects
        .iter()
        .map(|project| raw(&conn, *project))
        .collect();
    drop(store);
    drop(MetaStore::open(&path).unwrap());
    let again: Vec<String> = projects
        .iter()
        .map(|project| raw(&conn, *project))
        .collect();
    assert_eq!(after, again);
}

#[test]
fn a_store_from_before_draft_imports_migrates_its_grids_too() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("meta.sqlite");
    let mut store = MetaStore::create(&path).unwrap();
    let project = store.create_project(Uuid::new_v4(), "Old").unwrap().id;
    drop(store);
    let conn = Connection::open(&path).unwrap();
    store_raw(&conn, project, 3, &grid_payload(project, 3, grid(3, 2, 30)));
    conn.execute_batch("DROP TABLE draft_import; PRAGMA user_version=25")
        .unwrap();
    let store = MetaStore::open(&path).unwrap();
    let draft = store.framing_draft(project).unwrap().unwrap();
    assert_eq!(draft.layout, Layout::grid(grid(3, 2, 30)));
    assert_eq!(draft.revision, 3);
}

fn place(id: &str, x: f64, y: f64, locked: bool) -> PanelPlacement {
    PanelPlacement {
        id: id.into(),
        x,
        y,
        locked,
        unlocked_by_hand: false,
        measured: None,
        angle_offset_degrees: 0.0,
    }
}

fn single(project: Uuid) -> FramingDraft {
    FramingDraft {
        project_id: project,
        revision: 0,
        target_name: "Heart".into(),
        center: IcrsPosition {
            ra_degrees: 38.2,
            dec_degrees: 61.45,
        },
        position_angle_degrees: 35.0,
        layout: Layout::single(20),
        panel_rig_id: None,
        panel: Some(PanelSize {
            width_degrees: 1.4,
            height_degrees: 1.05,
        }),
        shown_rig_ids: vec![],
        survey_id: "dss2_color".into(),
        view_fov_degrees: 6.0,
        updated_at_ms: 1_000,
        rig_framings: vec![],
        layout_revision: 0,
    }
}

#[test]
fn a_locked_panel_stays_put_until_it_is_unlocked() {
    let dir = TempDir::new().unwrap();
    let mut store = MetaStore::create(&dir.path().join("meta.sqlite")).unwrap();
    let project = store.create_project(Uuid::new_v4(), "Heart").unwrap().id;
    let first = store.save_framing_draft(&single(project), 0).unwrap();

    // Lock the framed target and add panels east and north of it: locking
    // and the minimum overlap are not layout changes, the new panels are.
    let mut locked = first.clone();
    locked.layout.panels[0].locked = true;
    let locked = store.save_framing_draft(&locked, 1).unwrap();
    assert_eq!((locked.revision, locked.layout_revision), (2, 1));
    let mut grown = locked.clone();
    grown.layout.panels.push(place("p2", -0.8, 0.0, false));
    grown.layout.panels.push(place("p3", 0.0, 0.8, false));
    let grown = store.save_framing_draft(&grown, 2).unwrap();
    assert_eq!(grown.layout_revision, 3);
    let mut stricter = grown.clone();
    stricter.layout.min_overlap_percent = 25;
    let stricter = store.save_framing_draft(&stricter, 3).unwrap();
    assert_eq!(stricter.layout_revision, 3);

    // Moving the locked panel, or the center, angle or size under it, is refused.
    let refused = |store: &mut MetaStore, change: &dyn Fn(&mut FramingDraft)| {
        let mut next = stricter.clone();
        change(&mut next);
        assert!(!stricter.locked_moves(&next).is_empty());
        assert!(matches!(
            store.save_framing_draft(&next, 4),
            Err(Error::InvalidInput)
        ));
    };
    refused(&mut store, &|d| d.layout.panels[0].x = 0.1);
    refused(&mut store, &|d| d.center.ra_degrees += 0.5);
    refused(&mut store, &|d| d.position_angle_degrees = 36.0);
    // Turning it on its own is a move too.
    refused(&mut store, &|d| {
        d.layout.panels[0].angle_offset_degrees = 5.0
    });
    // The other panels still move and turn freely, and a turn is a layout
    // change of its own.
    let mut dragged = stricter.clone();
    dragged.layout.panels[2].x = 0.05;
    let dragged = store.save_framing_draft(&dragged, 4).unwrap();
    assert_eq!(dragged.layout_revision, 5);
    let mut turned = dragged.clone();
    turned.layout.panels[2].angle_offset_degrees = -12.5;
    assert!(!dragged.same_layout(&turned));
    let turned = store.save_framing_draft(&turned, 5).unwrap();
    assert_eq!(turned.layout_revision, 6);
    assert_eq!(turned.layout.panels[2].angle_offset_degrees, -12.5);
    let mut dragged = turned.clone();
    dragged.layout.panels[2].angle_offset_degrees = 0.0;
    let dragged = store.save_framing_draft(&dragged, 6).unwrap();
    assert_eq!(dragged.layout_revision, 7);
    // A new size keeps a locked panel at the center where it is.
    let mut resized = dragged.clone();
    resized.panel = Some(PanelSize {
        width_degrees: 1.2,
        height_degrees: 0.9,
    });
    let resized = store.save_framing_draft(&resized, 7).unwrap();
    // With a locked panel off the center, a new size would move it.
    let mut anchored = resized.clone();
    anchored.layout.panels[1].locked = true;
    let anchored = store.save_framing_draft(&anchored, 8).unwrap();
    let mut shrunk = anchored.clone();
    shrunk.panel = Some(PanelSize {
        width_degrees: 1.0,
        height_degrees: 0.9,
    });
    assert_eq!(anchored.locked_moves(&shrunk), vec!["p2".to_string()]);
    assert!(store.save_framing_draft(&shrunk, 9).is_err());

    // Unlocking is an edit of its own, and may move the panel in the same save.
    let mut unlocked = anchored.clone();
    unlocked.layout.panels[1].locked = false;
    unlocked.layout.panels[1].x = -0.75;
    let unlocked = store.save_framing_draft(&unlocked, 9).unwrap();
    assert_eq!(unlocked.layout.panels[1].x, -0.75);
}

#[test]
fn a_rig_framed_on_its_own_keeps_its_locks_too() {
    let dir = TempDir::new().unwrap();
    let mut store = MetaStore::create(&dir.path().join("meta.sqlite")).unwrap();
    let project = store.create_project(Uuid::new_v4(), "Heart").unwrap().id;
    let rig = store.create_rig(Uuid::new_v4(), "C925").unwrap().id;
    let mut draft = single(project);
    let mut own = Layout::single(20);
    own.panels[0].locked = true;
    draft.rig_framings.push(RigFraming {
        rig_id: rig,
        center: Some(IcrsPosition {
            ra_degrees: 39.0,
            dec_degrees: 61.0,
        }),
        position_angle_degrees: Some(90.0),
        layout: own,
        panel: Some(PanelSize {
            width_degrees: 0.5,
            height_degrees: 0.4,
        }),
    });
    let saved = store.save_framing_draft(&draft, 0).unwrap();
    let mut moved = saved.clone();
    moved.rig_framings[0].center = Some(IcrsPosition {
        ra_degrees: 39.5,
        dec_degrees: 61.0,
    });
    assert_eq!(saved.locked_moves(&moved), vec![format!("{rig}:r1c1")]);
    assert!(store.save_framing_draft(&moved, 1).is_err());
    // The shared framing is free: none of its panels is locked.
    let mut shared = saved.clone();
    shared.center.ra_degrees = 40.0;
    assert!(store.save_framing_draft(&shared, 1).is_ok());
}

#[test]
fn an_older_client_saves_a_grid_and_reads_back_panels() {
    let dir = TempDir::new().unwrap();
    let mut store = MetaStore::create(&dir.path().join("meta.sqlite")).unwrap();
    let project = store.create_project(Uuid::new_v4(), "Heart").unwrap().id;
    let body = grid_payload(project, 0, grid(2, 2, 20));
    let draft: FramingDraft = serde_json::from_value(body).unwrap();
    let saved = store.save_framing_draft(&draft, 0).unwrap();
    assert_eq!(saved.layout, Layout::grid(grid(2, 2, 20)));
    let written = serde_json::to_value(&saved).unwrap();
    assert!(written.get("layout").is_some() && written.get("mosaic").is_none());
    // The same panels in another order are the same layout.
    let mut shuffled = saved.clone();
    shuffled.layout.panels.reverse();
    assert!(saved.same_layout(&shuffled));
    let mut nudged = saved.clone();
    nudged.layout.panels[3].y -= 0.01;
    assert!(!saved.same_layout(&nudged));
}

/// What twelve solved frames of the framed target say: shot a little off
/// the plan, with a wider camera.
fn measured() -> MeasuredGeometry {
    MeasuredGeometry {
        center: IcrsPosition {
            ra_degrees: 38.25,
            dec_degrees: 61.47,
        },
        position_angle_degrees: 36.5,
        size: PanelSize {
            width_degrees: 1.6,
            height_degrees: 1.1,
        },
        source: MeasuredSource::Solved,
        size_source: MeasuredSource::Solved,
        frames: 12,
    }
}

#[test]
fn a_schema_29_store_opens_unchanged_and_keeps_measured_panels() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("meta.sqlite");
    let mut store = MetaStore::create(&path).unwrap();
    let project = store.create_project(Uuid::new_v4(), "Heart").unwrap().id;
    let plain = store.create_project(Uuid::new_v4(), "Plain").unwrap().id;
    let mut draft = single(project);
    draft.layout.panels[0].locked = true;
    draft.layout.panels.push(place("p2", -0.8, 0.0, false));
    let saved = store.save_framing_draft(&draft, 0).unwrap();
    store.save_framing_draft(&single(plain), 0).unwrap();
    drop(store);
    let conn = Connection::open(&path).unwrap();
    // A draft without measured geometry never writes the field, so a
    // schema 29 build reads what this one saved.
    let stored = raw(&conn, project);
    assert!(
        !stored.contains("measured") && !stored.contains("angle_offset"),
        "{stored}"
    );
    conn.pragma_update(None, "user_version", 29).unwrap();

    // Schema 30 rewrites nothing.
    let before = [raw(&conn, project), raw(&conn, plain)];
    let mut store = MetaStore::open(&path).unwrap();
    assert_eq!(
        conn.pragma_query_value(None, "user_version", |r| r.get::<_, i32>(0))
            .unwrap(),
        32
    );
    assert_eq!([raw(&conn, project), raw(&conn, plain)], before);
    assert_eq!(store.framing_draft(project).unwrap().unwrap(), saved);

    // A locked panel keeps the geometry its frames were shot with. Read
    // where the panel was planned, it is the server's reading of the same
    // place, not a move: activation keeps that target as it is.
    let mut shot = saved.clone();
    shot.layout.panels[0].measured = Some(measured());
    assert!(saved.same_layout(&shot));
    let shot = store.save_framing_draft(&shot, saved.revision).unwrap();
    assert_eq!(shot.layout_revision, saved.layout_revision);
    // Solves that put it a whole panel away move it.
    let mut far = shot.clone();
    far.layout.panels[0]
        .measured
        .as_mut()
        .unwrap()
        .center
        .dec_degrees += 1.2;
    assert!(!shot.same_layout(&far));
    let far = store.save_framing_draft(&far, shot.revision).unwrap();
    assert_eq!(far.layout_revision, far.revision);
    let shot = store.save_framing_draft(&shot, far.revision).unwrap();
    assert_eq!(
        store.framing_draft(project).unwrap().unwrap().layout.panels[0].measured,
        Some(measured())
    );
    assert!(raw(&conn, project).contains("\"measured\""));
    // Saving it unchanged is no change.
    let again = store.save_framing_draft(&shot, shot.revision).unwrap();
    assert_eq!(again.revision, shot.revision);

    // Only a locked panel holds measured geometry.
    let mut loose = shot.clone();
    loose.layout.panels[0].locked = false;
    assert!(matches!(
        store.save_framing_draft(&loose, shot.revision),
        Err(Error::InvalidInput)
    ));
    // Unlocking drops it, and the panel is planned again.
    loose.layout.panels[0].measured = None;
    store.save_framing_draft(&loose, shot.revision).unwrap();
}

#[test]
fn a_measured_panel_holds_where_it_was_shot_under_a_new_camera() {
    let dir = TempDir::new().unwrap();
    let mut store = MetaStore::create(&dir.path().join("meta.sqlite")).unwrap();
    let project = store.create_project(Uuid::new_v4(), "Heart").unwrap().id;
    let mut draft = single(project);
    draft.layout.panels = vec![
        place("r1c1", 0.0, 0.0, true),
        place("r1c2", -0.8, 0.0, true),
        place("p3", 0.0, 0.8, false),
    ];
    draft.layout.panels[1].measured = Some(measured());
    let saved = store.save_framing_draft(&draft, 0).unwrap();

    // The plan's camera changes: the measured panel off the center stays
    // as shot, so the new size is no move.
    let mut resized = saved.clone();
    resized.panel = Some(PanelSize {
        width_degrees: 1.0,
        height_degrees: 0.8,
    });
    assert!(saved.locked_moves(&resized).is_empty());
    let resized = store.save_framing_draft(&resized, saved.revision).unwrap();
    // Its planned place, center and angle still hold.
    let mut dragged = resized.clone();
    dragged.layout.panels[1].x = -0.6;
    assert_eq!(resized.locked_moves(&dragged), vec!["r1c2".to_string()]);
    let mut turned = resized.clone();
    turned.position_angle_degrees = 40.0;
    assert_eq!(
        resized.locked_moves(&turned),
        vec!["r1c1".to_string(), "r1c2".to_string()]
    );

    // A rig framed on its own keeps its panels' shot geometry too, and a
    // refreshed reading is a layout change.
    let rig = store.create_rig(Uuid::new_v4(), "C925").unwrap().id;
    let mut own = Layout::single(20);
    own.panels[0].locked = true;
    let mut with_rig = resized.clone();
    with_rig.rig_framings.push(RigFraming {
        rig_id: rig,
        center: None,
        position_angle_degrees: None,
        layout: own,
        panel: None,
    });
    let with_rig = store
        .save_framing_draft(&with_rig, resized.revision)
        .unwrap();
    let mut shot = with_rig.clone();
    shot.rig_framings[0].layout.panels[0].measured = Some(measured());
    // Sized by its rig's field, which the store does not know, a reading
    // the server writes is taken as in place.
    assert!(with_rig.same_layout(&shot));
    let shot = store.save_framing_draft(&shot, with_rig.revision).unwrap();
    assert_eq!(
        shot.rig_framings[0].layout.panels[0].measured,
        Some(measured())
    );
    // More frames saying the same is no layout change; a reading that
    // moves the panel is.
    let mut counted = shot.clone();
    counted.layout.panels[1].measured.as_mut().unwrap().frames = 14;
    let counted = store.save_framing_draft(&counted, shot.revision).unwrap();
    assert_eq!(counted.layout_revision, shot.layout_revision);
    // Another night's solves nudge the median by arcseconds and a
    // hundredth of a degree: the reading is kept, the layout is the same.
    let mut nudged = counted.clone();
    {
        let reading = nudged.layout.panels[1].measured.as_mut().unwrap();
        reading.center.dec_degrees += 10.0 / 3600.0;
        reading.position_angle_degrees += 0.02;
        reading.frames = 20;
    }
    let nudged = store.save_framing_draft(&nudged, counted.revision).unwrap();
    assert_eq!(nudged.layout_revision, counted.layout_revision);
    assert_eq!(
        nudged.layout.panels[1].measured.as_ref().unwrap().frames,
        20
    );
    // A reading that turns the panel a degree moves it.
    let mut moved = nudged.clone();
    moved.layout.panels[1]
        .measured
        .as_mut()
        .unwrap()
        .position_angle_degrees = 37.5;
    let moved = store.save_framing_draft(&moved, nudged.revision).unwrap();
    assert_eq!(moved.layout_revision, moved.revision);
}

/// Drag a panel, press L, save: one body moves and locks it, and the lock
/// holds the place it is given.
#[test]
fn moving_then_locking_a_panel_in_one_save_is_taken() {
    let dir = TempDir::new().unwrap();
    let mut store = MetaStore::create(&dir.path().join("meta.sqlite")).unwrap();
    let project = store.create_project(Uuid::new_v4(), "Heart").unwrap().id;
    let mut draft = single(project);
    draft.layout.panels.push(place("p2", -0.8, 0.0, false));
    let saved = store.save_framing_draft(&draft, 0).unwrap();
    let mut next = saved.clone();
    next.layout.panels[1].x = -0.75;
    next.layout.panels[1].locked = true;
    assert!(saved.locked_moves(&next).is_empty());
    let locked = store.save_framing_draft(&next, saved.revision).unwrap();
    let mut moved = locked.clone();
    moved.layout.panels[1].x = -0.7;
    assert_eq!(locked.locked_moves(&moved), vec!["p2".to_string()]);
}

/// A panel size given or taken away moves a locked planned panel off the
/// center: the shared size, or a rig framing's typed size dropped back to
/// its rig's field.
#[test]
fn a_size_given_or_taken_away_under_a_locked_panel_off_the_center_is_refused() {
    let dir = TempDir::new().unwrap();
    let mut store = MetaStore::create(&dir.path().join("meta.sqlite")).unwrap();
    let project = store.create_project(Uuid::new_v4(), "Heart").unwrap().id;
    let rig = store.create_rig(Uuid::new_v4(), "C925").unwrap().id;
    let mut draft = single(project);
    draft.layout.panels.push(place("p2", -0.8, 0.0, true));
    draft.rig_framings.push(RigFraming {
        rig_id: rig,
        center: None,
        position_angle_degrees: None,
        layout: Layout {
            min_overlap_percent: 20,
            panels: vec![place("r1c1", 0.0, 0.0, false), place("p2", -0.8, 0.0, true)],
        },
        panel: Some(PanelSize {
            width_degrees: 0.5,
            height_degrees: 0.4,
        }),
    });
    let saved = store.save_framing_draft(&draft, 0).unwrap();
    let mut follow = saved.clone();
    follow.rig_framings[0].panel = None;
    assert_eq!(saved.locked_moves(&follow), vec![format!("{rig}:p2")]);
    assert!(store.save_framing_draft(&follow, saved.revision).is_err());
    let mut sizeless = saved.clone();
    sizeless.panel = None;
    assert_eq!(saved.locked_moves(&sizeless), vec!["p2".to_string()]);
    // A locked panel at the center has no place to lose.
    let mut centered = saved.clone();
    centered.layout.panels[1].locked = false;
    centered.rig_framings[0].layout.panels[1].locked = false;
    centered.layout.panels[0].locked = true;
    let centered = store.save_framing_draft(&centered, saved.revision).unwrap();
    let mut sizeless = centered.clone();
    sizeless.panel = None;
    assert!(centered.locked_moves(&sizeless).is_empty());
}

/// A failure part way through schema 29 leaves the store at 28, untouched,
/// and the next open migrates it whole.
#[test]
fn a_failed_grid_migration_rolls_back_whole() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("meta.sqlite");
    let mut store = MetaStore::create(&path).unwrap();
    let a = store.create_project(Uuid::new_v4(), "A").unwrap().id;
    let b = store.create_project(Uuid::new_v4(), "B").unwrap().id;
    drop(store);
    let conn = Connection::open(&path).unwrap();
    for (project, revision) in [(a, 2u64), (b, 3)] {
        store_raw(
            &conn,
            project,
            revision,
            &grid_payload(project, revision, grid(2, 2, 20)),
        );
    }
    let before = (raw(&conn, a), raw(&conn, b));
    conn.execute_batch(&format!(
        "CREATE TRIGGER boom BEFORE UPDATE ON framing_draft WHEN NEW.project_id='{b}'
         BEGIN SELECT RAISE(ABORT,'boom'); END;
         PRAGMA user_version=28;"
    ))
    .unwrap();
    assert!(MetaStore::open(&path).is_err());
    let version: i32 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap();
    assert_eq!(version, 28);
    assert_eq!((raw(&conn, a), raw(&conn, b)), before);
    conn.execute_batch("DROP TRIGGER boom").unwrap();
    let store = MetaStore::open(&path).unwrap();
    assert_eq!(
        store.framing_draft(a).unwrap().unwrap().layout.panels.len(),
        4
    );
    assert!(raw(&conn, a).contains("\"layout\"") && raw(&conn, b).contains("\"layout\""));
}
