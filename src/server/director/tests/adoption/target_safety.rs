//! What activation does to targets that already have frames, and what the
//! Library and Target Scheduler read back: regressions from the review of
//! per-panel angles and shot panels.
use super::activation::activated;
use super::*;
use psf_guard_director_core::framing::{Layout, Mosaic};
use psf_guard_director_meta::framing::FramingDraft;

fn changes(report: &Value) -> Vec<String> {
    report["rigs"][0]["changes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["kind"] == "target")
        .map(|c| {
            format!(
                "{} {} | {}",
                c["action"].as_str().unwrap(),
                c["name"].as_str().unwrap(),
                c["detail"].as_str().unwrap()
            )
        })
        .collect()
}

async fn preview(f: &Fixture, project: Uuid) -> Value {
    let (status, preview) = call(
        &f.app,
        "POST",
        &format!("/projects/{project}/activation/preview"),
        json!({}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    preview["data"].clone()
}

async fn activate_now(f: &Fixture, project: Uuid) -> Value {
    let data = preview(f, project).await;
    let (status, applied) = call(
        &f.app,
        "POST",
        &format!("/projects/{project}/activation/apply"),
        json!({"preview_digest": data["preview_digest"]}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{applied}");
    applied["data"].clone()
}

fn edit_framing(f: &Fixture, project: Uuid, edit: impl FnOnce(&mut FramingDraft)) {
    let mut store = f.state.director.as_ref().unwrap().writer.lock().unwrap();
    let mut framing = store.framing_draft(project).unwrap().unwrap();
    let revision = framing.revision;
    edit(&mut framing);
    store.save_framing_draft(&framing, revision).unwrap();
}

/// A linked project with one target that has 40 frames, under another
/// name, at `(ra_degrees, dec)`.
fn link_lone_target(a: &super::activation::Activated, name: &str, ra_degrees: f64, dec: f64) {
    let source = Uuid::new_v4();
    a.db.execute(
        "INSERT INTO project (Id, profileId, name, description, state, priority, isMosaic, flatsHandling, guid)
         VALUES (1, 'profile-a', 'By hand', '', 1, 1, 0, 0, ?1)",
        [source.to_string()],
    )
    .unwrap();
    a.db.execute(
        "INSERT INTO target (Id, name, active, ra, dec, epochcode, rotation, roi, projectid, guid)
         VALUES (1, ?1, 1, ?2, ?3, 2, 15.0, 100, 1, ?4)",
        rusqlite::params![name, ra_degrees / 15.0, dec, Uuid::new_v4().to_string()],
    )
    .unwrap();
    a.db.execute(
        "INSERT INTO exposureplan (Id, profileId, exposure, desired, acquired, accepted, targetid, exposureTemplateId, enabled, guid)
         VALUES (11, 'profile-a', 300, 72, 40, 40, 1, 1, 1, 'aaaaaaaa-0000-4000-8000-000000000011')",
        [],
    )
    .unwrap();
    let catalog = crate::catalog_identity::read(&a.db).unwrap().unwrap().id;
    a.f.state
        .director
        .as_ref()
        .unwrap()
        .writer
        .lock()
        .unwrap()
        .link_catalog_project(&psf_guard_director_meta::catalog::ProjectMapping {
            catalog_id: catalog,
            source_project_guid: source,
            source_profile_id: "profile-a".into(),
            project_id: a.project,
            rig_id: a.rig,
        })
        .unwrap();
}

/// A lone target with 40 frames at the framing's center, and an unlocked
/// 2 x 1 grid whose `r1c1` sits half a step off it. The target stands for
/// `r1c1`, but taking it over would move it and its frames 35′: it stays
/// where it was shot, and the grid gets targets of its own.
#[tokio::test]
async fn a_target_with_frames_is_not_taken_over_from_far_away() {
    let a = activated().await;
    link_lone_target(&a, "My heart", 38.2, 61.45);
    let data = activate_now(&a.f, a.project).await;
    let targets = changes(&data);
    let (ra, dec, acquired): (f64, f64, i64) = a
        .db
        .query_row(
            "SELECT t.ra, t.dec, e.acquired FROM target t JOIN exposureplan e ON e.targetid=t.Id WHERE t.Id=1 AND e.Id=11",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert!(
        (ra - 38.2 / 15.0).abs() < 1e-12 && (dec - 61.45).abs() < 1e-12,
        "{targets:#?}"
    );
    assert_eq!(acquired, 40);
    assert!(
        targets.iter().all(|c| c.starts_with("create")),
        "{targets:#?}"
    );
}

/// The same target named as the panel would be: found by its name, but
/// still too far to take over with its frames, so it is left as it is and
/// the preview says why.
#[tokio::test]
async fn a_target_found_by_name_is_left_alone_when_far_with_frames() {
    let a = activated().await;
    link_lone_target(&a, "IC 1805 r1c1", 38.2, 61.45);
    let data = activate_now(&a.f, a.project).await;
    let targets = changes(&data);
    assert!(
        targets.iter().any(|c| c.starts_with("create IC 1805 r1c1")
            && c.contains("the existing target IC 1805 r1c1 sits")
            && c.contains("with its 40 frames, so it is left as it is")),
        "{targets:#?}"
    );
    let (ra, dec): (f64, f64) =
        a.db.query_row("SELECT ra, dec FROM target WHERE Id=1", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap();
    assert!((ra - 38.2 / 15.0).abs() < 1e-12 && (dec - 61.45).abs() < 1e-12);
}

/// The same with the lone target an unrelated object 60° away: it is no
/// claim at all.
#[tokio::test]
async fn an_unrelated_far_target_is_not_claimed() {
    let a = activated().await;
    link_lone_target(&a, "NGC 7000", 20.98 * 15.0, 44.3);
    let data = preview(&a.f, a.project).await;
    let targets = changes(&data);
    assert!(
        targets
            .iter()
            .all(|c| c.starts_with("create") && !c.contains("takes over")),
        "{targets:#?}"
    );
}

/// A target with frames stored at the framing's angle, as activations
/// before per-panel angles wrote it, whose panel then moves far: the new
/// target at new sky takes the panel's own angle, so the next preview
/// reads every target unchanged.
#[tokio::test]
async fn a_far_moved_panel_gets_its_own_angle_and_activation_stays_idempotent() {
    let a = activated().await;
    activate_now(&a.f, a.project).await;
    a.db.execute(
        "UPDATE target SET rotation=15.0 WHERE name='IC 1805 r1c1'",
        [],
    )
    .unwrap();
    a.db.execute("UPDATE exposureplan SET acquired=5 WHERE targetid=(SELECT Id FROM target WHERE name='IC 1805 r1c1')", []).unwrap();
    edit_framing(&a.f, a.project, |framing| {
        framing.layout = Layout::grid(Mosaic {
            rows: 3,
            ..framing.layout.as_grid().unwrap()
        })
    });
    activate_now(&a.f, a.project).await;
    let again = preview(&a.f, a.project).await;
    let second = changes(&again);
    assert!(
        second.iter().all(|c| c.starts_with("unchanged")),
        "{second:#?}"
    );
}

/// A single-panel plan whose center moved far after frames: the old target
/// is retired (`r1c1~n`) and a new one made. The Library reads one panel,
/// not a two-panel mosaic.
#[tokio::test]
async fn a_retired_single_panel_is_not_read_as_a_mosaic() {
    let a = activated().await;
    edit_framing(&a.f, a.project, |framing| {
        framing.layout = Layout::single(20);
    });
    activate_now(&a.f, a.project).await;
    a.db.execute("UPDATE exposureplan SET acquired=5", [])
        .unwrap();
    edit_framing(&a.f, a.project, |framing| {
        framing.center.dec_degrees += 5.0;
    });
    activate_now(&a.f, a.project).await;
    let ids: Vec<String> = {
        let mut s =
            a.db.prepare("SELECT panel_id FROM psf_guard_director_target ORDER BY panel_id")
                .unwrap();
        s.query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    };
    assert_eq!(ids.len(), 2, "{ids:?}");
    assert!(ids[1].starts_with("r1c1~"), "{ids:?}");
    let row =
        a.db.query_row("SELECT Id FROM project", [], |r| r.get::<_, i32>(0))
            .unwrap();
    let mosaic = crate::server::mosaic_scope::project_mosaic(&a.db, row).unwrap();
    assert!(mosaic.is_none(), "{:?}", mosaic.map(|m| m.panels.len()));
}

/// Activate a 2 x 2 grid with leaned angles, then take Target Scheduler's
/// values back: the framing reads back unchanged (the angle with the lean
/// taken off, the same places), far north and south and across 0h.
#[tokio::test]
async fn a_leaned_grid_reads_back_unchanged() {
    for (ra, dec, theta) in [
        (38.2, 80.0, 30.0),
        (38.2, 61.45, 0.0),
        (359.8, 61.45, 359.5),
        (0.1, -75.0, 200.0),
        (120.0, 88.5, 10.0),
    ] {
        let a = activated().await;
        edit_framing(&a.f, a.project, |framing| {
            framing.center.ra_degrees = ra;
            framing.center.dec_degrees = dec;
            framing.position_angle_degrees = theta;
            framing.layout = Layout::grid(Mosaic {
                rows: 2,
                columns: 2,
                overlap_percent: 20,
            });
        });
        activate_now(&a.f, a.project).await;
        let before = {
            let store = a.f.state.director.as_ref().unwrap().writer.lock().unwrap();
            store.framing_draft(a.project).unwrap().unwrap()
        };
        a.db.execute("UPDATE exposureplan SET desired=90", [])
            .unwrap();
        let (status, taken) = super::activation::take_values(&a.f, a.project, a.rig).await;
        assert_eq!(status, StatusCode::OK, "{taken}");
        assert_eq!(
            taken["data"]["taken"],
            json!(["Ha: 90 frames"]),
            "({ra},{dec},{theta}) {taken}"
        );
        assert_eq!(taken["data"]["framing_revision"], before.revision);
        let again = preview(&a.f, a.project).await;
        let second = changes(&again);
        assert!(
            second.iter().all(|c| c.starts_with("unchanged")),
            "({ra},{dec},{theta}) {second:#?}"
        );
    }
}

/// A wide free layout far north: one row of five panels and one above the
/// middle. The Library bands the panels by where their targets point and
/// reads one row of five with one above it.
#[tokio::test]
async fn the_library_bands_a_wide_free_row_far_north() {
    use psf_guard_director_core::framing::PanelPlacement;
    let a = activated().await;
    edit_framing(&a.f, a.project, |framing| {
        framing.center.dec_degrees = 70.0;
        framing.position_angle_degrees = 0.0;
        let mut layout = Layout::grid(Mosaic {
            rows: 1,
            columns: 5,
            overlap_percent: 20,
        });
        layout.panels.push(PanelPlacement::at("p6", 0.0, 0.8));
        framing.layout = layout;
    });
    activate_now(&a.f, a.project).await;
    let row =
        a.db.query_row("SELECT Id FROM project", [], |r| r.get::<_, i32>(0))
            .unwrap();
    let mosaic = crate::server::mosaic_scope::project_mosaic(&a.db, row)
        .unwrap()
        .unwrap();
    assert_eq!((mosaic.rows, mosaic.columns), (2, 5));
}
