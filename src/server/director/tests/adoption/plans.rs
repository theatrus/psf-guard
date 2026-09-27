use super::activation::activated;
use super::*;

/// Register a schema-23 database with the given projects, returning its path.
fn register(
    f: &Fixture,
    slug: &str,
    name: &str,
    projects: &[(i64, &str, Option<Uuid>)],
) -> std::path::PathBuf {
    let path = f._dir.path().join(format!("{slug}.sqlite"));
    let db = crate::ts_schema::create_fresh_db(&path).unwrap();
    for (id, label, guid) in projects {
        db.execute(
            "INSERT INTO project (Id, profileId, name, description, state, priority, isMosaic, flatsHandling, guid) VALUES (?1, 'profile-x', ?2, '', 1, 1, 0, 0, ?3)",
            rusqlite::params![id, label, guid.map(|g| g.to_string())],
        )
        .unwrap();
    }
    let context = crate::server::database_context::DatabaseContext::new(
        slug.into(),
        name.into(),
        path.to_string_lossy().into(),
        vec![f._dir.path().to_string_lossy().into()],
        None,
        None,
        None,
        f._dir.path().join("cache").to_string_lossy().into(),
    )
    .unwrap();
    f.state
        .databases
        .write()
        .unwrap()
        .insert(slug.into(), Arc::new(context));
    path
}

#[tokio::test]
async fn every_database_project_becomes_a_plan_and_shared_guids_become_one_plan() {
    let f = Fixture::new();
    let shared = Uuid::new_v4();
    let only_here = Uuid::new_v4();
    register(
        &f,
        "c925",
        "C925 data",
        &[
            (1, "Andromeda", Some(shared)),
            (2, "Only here", Some(only_here)),
            (3, "No GUID", None),
        ],
    );
    register(
        &f,
        "redcat",
        "Redcat data",
        &[(1, "Andromeda", Some(shared))],
    );
    // A registered file with no project table is reported, not fatal.
    let odd = f._dir.path().join("odd.sqlite");
    rusqlite::Connection::open(&odd)
        .unwrap()
        .execute_batch("CREATE TABLE t(x)")
        .unwrap();
    let context = crate::server::database_context::DatabaseContext::new(
        "odd".into(),
        "Odd file".into(),
        odd.to_string_lossy().into(),
        vec![f._dir.path().to_string_lossy().into()],
        None,
        None,
        None,
        f._dir.path().join("cache").to_string_lossy().into(),
    )
    .unwrap();
    f.state
        .databases
        .write()
        .unwrap()
        .insert("odd".into(), Arc::new(context));

    let (status, listed) = call(&f.app, "GET", "/plans", Value::Null, None).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    let data = &listed["data"];
    let warnings = data["warnings"].as_array().unwrap();
    assert!(
        warnings
            .iter()
            .any(|w| w.as_str().unwrap().starts_with("Odd file")),
        "{warnings:?}"
    );
    let rows = data["rows"].as_array().unwrap();
    let by_name = |name: &str| {
        rows.iter()
            .filter(|r| r["project"]["name"] == name)
            .collect::<Vec<_>>()
    };
    // Same GUID in two databases: one plan, two rigs.
    let andromeda = by_name("Andromeda");
    assert_eq!(andromeda.len(), 1, "{rows:?}");
    let links = andromeda[0]["links"].as_array().unwrap();
    assert_eq!(links.len(), 2);
    assert_eq!(links[0]["catalog_name"], "C925 data");
    assert_eq!(links[1]["catalog_name"], "Redcat data");
    assert_ne!(links[0]["rig"]["id"], links[1]["rig"]["id"]);
    assert_eq!(by_name("Only here").len(), 1);
    assert_eq!(by_name("No GUID").len(), 0);
    // The fixture's own hand-made catalog (M31 with a GUID) is adopted too.
    assert_eq!(by_name("M31").len(), 1);
    // Every database is now a rig with planning enabled.
    let (_, rigs) = call(&f.app, "GET", "/rigs/profiles", Value::Null, None).await;
    let slugs: Vec<&str> = rigs["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["catalog_slug"].as_str().unwrap())
        .collect();
    assert!(
        slugs.contains(&"c925") && slugs.contains(&"redcat") && slugs.contains(&"catalog"),
        "{slugs:?}"
    );
    // A second listing changes nothing.
    let (_, again) = call(&f.app, "GET", "/plans", Value::Null, None).await;
    assert_eq!(again["data"]["rows"].as_array().unwrap().len(), rows.len());
    assert_eq!(
        by_name("Andromeda")[0]["project"]["id"],
        again["data"]["rows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["project"]["name"] == "Andromeda")
            .unwrap()["project"]["id"]
    );
}

#[tokio::test]
async fn listing_fills_a_rig_without_optics_from_its_own_frames() {
    let (f, rig) = super::rig_profile::bound_fixture().await;
    let (_, before) = call(
        &f.app,
        "GET",
        "/catalogs/catalog/rig/profile",
        Value::Null,
        None,
    )
    .await;
    assert_eq!(before["data"]["profile"]["optics"], Value::Null);
    let (status, listed) = call(&f.app, "GET", "/plans", Value::Null, None).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    let (_, after) = call(
        &f.app,
        "GET",
        "/catalogs/catalog/rig/profile",
        Value::Null,
        None,
    )
    .await;
    let optics = &after["data"]["profile"]["optics"];
    assert_eq!(optics["source"]["file_name"], "newest.fits");
    assert_eq!(optics["value"]["focal_length_mm"], 250.0);
    assert_eq!(
        after["data"]["profile"]["site"]["value"]["latitude_degrees"],
        34.2
    );
    assert_eq!(after["data"]["profile"]["revision"], 1);
    // A second listing leaves an operator's later edits alone.
    let edit = json!({
        "expected_revision": 1,
        "optics": {"value": {"sensor_width_px": 100, "sensor_height_px": 80, "pixel_size_um": 5.0, "focal_length_mm": 400.0, "aperture_mm": null, "rotation": {"mode":"rotator"}}, "source": {"kind":"manual"}},
        "site": null, "horizon": null, "sky_quality": null,
        "limits": {"value": after["data"]["profile"]["limits"]["value"], "source": {"kind":"manual"}},
    });
    assert_eq!(
        call(&f.app, "PUT", "/catalogs/catalog/rig/profile", edit, None)
            .await
            .0,
        StatusCode::OK
    );
    call(&f.app, "GET", "/plans", Value::Null, None).await;
    let (_, again) = call(
        &f.app,
        "GET",
        "/catalogs/catalog/rig/profile",
        Value::Null,
        None,
    )
    .await;
    assert_eq!(
        again["data"]["profile"]["optics"]["value"]["focal_length_mm"],
        400.0
    );
    assert_eq!(again["data"]["profile"]["revision"], 2);
    let _ = rig;
}

#[tokio::test]
async fn the_plan_list_joins_framing_plan_and_activation_per_project() {
    let a = activated().await;
    let (status, before) = call(&a.f.app, "GET", "/plans", Value::Null, None).await;
    assert_eq!(status, StatusCode::OK, "{before}");
    let rows = before["data"]["rows"].as_array().unwrap();
    let heart = rows
        .iter()
        .find(|r| r["project"]["name"] == "Heart Nebula")
        .unwrap();
    assert_eq!(heart["links"], json!([]));
    assert_eq!(heart["framing"]["revision"], 1);
    assert_eq!(heart["framing"]["target_name"], "IC 1805");
    assert_eq!(heart["framing"]["panels"], 2);
    assert_eq!(heart["plan"]["objectives"], 1);
    assert_eq!(heart["plan"]["rigs"], 1);
    assert_eq!(heart["activation"], Value::Null);

    let (_, preview) = call(
        &a.f.app,
        "POST",
        &format!("/projects/{}/activation/preview", a.project),
        json!({}),
        None,
    )
    .await;
    let (status, _) = call(
        &a.f.app,
        "POST",
        &format!("/projects/{}/activation/apply", a.project),
        json!({"preview_digest": preview["data"]["preview_digest"]}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (_, after) = call(&a.f.app, "GET", "/plans", Value::Null, None).await;
    let heart = after["data"]["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["project"]["name"] == "Heart Nebula")
        .unwrap();
    assert_eq!(heart["activation"]["revision"], 1);
    assert_eq!(heart["activation"]["rigs"], 1);
    assert_eq!(heart["links"].as_array().unwrap().len(), 1);
    assert_eq!(heart["links"][0]["catalog_slug"], "rig");
    assert_eq!(heart["links"][0]["rig"]["id"], a.rig.to_string());
    assert_eq!(heart["links"][0]["source_name"], "Heart Nebula");
    let _ = a.objective;
}

#[tokio::test]
async fn listing_counts_frames_per_target_across_linked_databases() {
    let a = activated().await;
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
    a.db.execute(
        "UPDATE exposureplan SET acquired = 10, accepted = 8 WHERE targetid = (SELECT Id FROM target WHERE name = 'IC 1805 r1c1')",
        [],
    )
    .unwrap();
    let (status, listed) = call(&a.f.app, "GET", "/plans", Value::Null, None).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    let row = listed["data"]["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["project"]["id"] == a.project.to_string())
        .unwrap();
    assert_eq!(
        row["progress"],
        json!({"desired": 144, "acquired": 10, "accepted": 8, "targets": 2}),
        "{row}"
    );
    let targets = row["links"][0]["targets"].as_array().unwrap();
    assert_eq!(targets.len(), 2);
    assert_eq!(targets[0]["name"], "IC 1805 r1c1");
    assert_eq!(targets[0]["accepted"], 8);
    assert_eq!(targets[1]["name"], "IC 1805 r2c1");
    assert_eq!(targets[1]["desired"], 72);
    // A plan whose databases hold no target yet has no progress to report.
    let bare = listed["data"]["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["project"]["name"] == "M31")
        .unwrap();
    assert_eq!(bare["progress"], Value::Null);
    let _ = (a.rig, a.objective);
}
