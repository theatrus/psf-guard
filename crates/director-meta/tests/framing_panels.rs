//! Mosaic panels placed one by one: the schema 29 migration from every
//! older stored shape, locks, and what counts as a layout change.

use psf_guard_director_core::{
    framing::{FramingRequest, Layout, Mosaic, PanelPlacement, PanelSize},
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
        29
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
    // The other panels still move freely.
    let mut dragged = stricter.clone();
    dragged.layout.panels[2].x = 0.05;
    let dragged = store.save_framing_draft(&dragged, 4).unwrap();
    assert_eq!(dragged.layout_revision, 5);
    // A new size keeps a locked panel at the center where it is.
    let mut resized = dragged.clone();
    resized.panel = Some(PanelSize {
        width_degrees: 1.2,
        height_degrees: 0.9,
    });
    let resized = store.save_framing_draft(&resized, 5).unwrap();
    // With a locked panel off the center, a new size would move it.
    let mut anchored = resized.clone();
    anchored.layout.panels[1].locked = true;
    let anchored = store.save_framing_draft(&anchored, 6).unwrap();
    let mut shrunk = anchored.clone();
    shrunk.panel = Some(PanelSize {
        width_degrees: 1.0,
        height_degrees: 0.9,
    });
    assert_eq!(anchored.locked_moves(&shrunk), vec!["p2".to_string()]);
    assert!(store.save_framing_draft(&shrunk, 7).is_err());

    // Unlocking is an edit of its own, and may move the panel in the same save.
    let mut unlocked = anchored.clone();
    unlocked.layout.panels[1].locked = false;
    unlocked.layout.panels[1].x = -0.75;
    let unlocked = store.save_framing_draft(&unlocked, 7).unwrap();
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
