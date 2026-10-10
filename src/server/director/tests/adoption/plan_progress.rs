use super::activation::activated;
use super::*;

#[tokio::test]
async fn progress_counts_each_objectives_exposure_plans_and_keeps_the_rest_apart() {
    let a = activated().await;
    let path = format!("/projects/{}/plan/progress", a.project);

    // Before activation the rig has no project yet: its objective shows, empty.
    let (status, before) = call(&a.f.app, "GET", &path, Value::Null, None).await;
    assert_eq!(status, StatusCode::OK, "{before}");
    let rig = &before["data"]["rigs"][0];
    assert_eq!(rig["project"], Value::Null);
    assert_eq!(
        rig["objectives"][0]["objective_id"],
        a.objective.to_string()
    );
    assert_eq!(rig["objectives"][0]["exposure_plans"], 0);

    let (_, preview) = call(
        &a.f.app,
        "POST",
        &format!("/projects/{}/activation/preview", a.project),
        json!({}),
        None,
    )
    .await;
    let (status, applied) = call(
        &a.f.app,
        "POST",
        &format!("/projects/{}/activation/apply", a.project),
        json!({"preview_digest": preview["data"]["preview_digest"]}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{applied}");
    // N.I.N.A. took frames on both panels and graded one rejected.
    a.db.execute("UPDATE exposureplan SET acquired = 5, accepted = 3", [])
        .unwrap();
    let (project, target, plan): (i64, i64, i64) = a.db.query_row(
        "SELECT t.projectid, t.Id, e.Id FROM exposureplan e JOIN target t ON t.Id = e.targetid ORDER BY e.Id LIMIT 1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    ).unwrap();
    a.db.execute(
        "INSERT INTO acquiredimage (projectId, targetId, acquireddate, filtername, gradingStatus, metadata, exposureId)
         VALUES (?1, ?2, 0, 'Ha', 2, '{}', ?3)",
        rusqlite::params![project, target, plan],
    ).unwrap();
    // Rows added by hand: one with the plan's template, one no objective has.
    a.db.execute(
        "INSERT INTO exposuretemplate (profileId, name, filtername, defaultexposure) VALUES ('profile-a', 'OIII 300', 'OIII', 300)",
        [],
    ).unwrap();
    let oiii = a.db.last_insert_rowid();
    a.db.execute(
        "INSERT INTO exposureplan (profileId, exposure, desired, acquired, accepted, targetid, exposureTemplateId)
         VALUES ('profile-a', 300, 10, 2, 2, ?1, 1), ('profile-a', 300, 20, 4, 1, ?1, ?2)",
        rusqlite::params![target, oiii],
    ).unwrap();
    let desired: i64 =
        a.db.query_row(
            "SELECT SUM(desired) FROM exposureplan WHERE exposureTemplateId = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();

    let (status, after) = call(&a.f.app, "GET", &path, Value::Null, None).await;
    assert_eq!(status, StatusCode::OK, "{after}");
    let rig = &after["data"]["rigs"][0];
    assert_eq!(rig["project"], json!({"name": "Heart Nebula", "state": 1}));
    let objective = &rig["objectives"][0];
    assert_eq!(
        objective["exposure_plans"], 3,
        "two activated panels and the hand row with its template"
    );
    assert_eq!(
        objective["frames"],
        json!({"desired": desired, "acquired": 12, "accepted": 8, "rejected": 1})
    );
    assert_eq!(
        rig["other"],
        json!({"desired": 20, "acquired": 4, "accepted": 1, "rejected": 0})
    );
    assert_eq!(rig["total"], objective["frames"]);
    assert_eq!(rig["note"], Value::Null);

    // A reject removed from the catalog still counts as rejected.
    crate::commands::reject_removal::ensure_schema(&a.db).unwrap();
    a.db.execute(
        "INSERT INTO psf_guard_removed_image (acquired_image_guid, acquired_image_id, project_id, target_id, exposure_id,
            batch_id, removed_at, trash_until, rows_json, files_json)
         VALUES ('gone', 99, ?1, ?2, ?3, 'b', 1, 1, '{}', '[]')",
        rusqlite::params![project, target, plan],
    )
    .unwrap();
    let (_, removed) = call(&a.f.app, "GET", &path, Value::Null, None).await;
    assert_eq!(
        removed["data"]["rigs"][0]["objectives"][0]["frames"]["rejected"], 2,
        "{removed}"
    );
}
