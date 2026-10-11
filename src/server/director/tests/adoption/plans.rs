use super::activation::activated;
use super::*;
use psf_guard_director_meta::plan::Goal;

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
        f._dir.path().join("cache"),
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
    let redcat = register(
        &f,
        "redcat",
        "Redcat data",
        &[(1, "Andromeda", Some(shared))],
    );
    // Target Scheduler already points the shared project somewhere (RA in hours).
    rusqlite::Connection::open(&redcat)
        .unwrap()
        .execute(
            "INSERT INTO target (name, active, ra, dec, epochcode, rotation, roi, projectid, guid) VALUES ('M 31', 1, 0.7123, 41.269, 2, 35.0, 100, 1, 'target-m31')",
            [],
        )
        .unwrap();
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
        f._dir.path().join("cache"),
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
    // The catalog target became Director's framing draft on first sight: hours to degrees.
    let framing = &andromeda[0]["framing"];
    assert_eq!(framing["source"], "draft", "{framing}");
    assert_eq!(framing["revision"], 1);
    assert_eq!(framing["target_name"], "M 31");
    assert!(
        (framing["center"]["ra_degrees"].as_f64().unwrap() - 10.6845).abs() < 1e-6,
        "{framing}"
    );
    assert_eq!(framing["position_angle_degrees"], 35.0);
    assert_eq!(framing["panels"], 1);
    // No optics on that rig yet, so no panel size and no panel rig.
    assert_eq!(framing["panel"], Value::Null);
    assert_eq!(framing["panel_rig_id"], Value::Null);
    // No exposure plans there, so no plan draft was made up.
    assert_eq!(andromeda[0]["plan"], Value::Null);
    assert_eq!(links[1]["targets"][0]["center"]["dec_degrees"], 41.269);
    // A project whose databases hold no target has nothing to frame from.
    assert_eq!(by_name("Only here")[0]["framing"], Value::Null);
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
    // Two of the ten were graded rejected; the list tells rejected from pending.
    a.db.execute_batch(
        "INSERT INTO acquiredimage (projectId, targetId, acquireddate, filtername, gradingStatus, metadata)
         SELECT projectid, Id, 0, 'Ha', 2, '{}' FROM target WHERE name = 'IC 1805 r1c1';
         INSERT INTO acquiredimage (projectId, targetId, acquireddate, filtername, gradingStatus, metadata)
         SELECT projectid, Id, 0, 'Ha', 2, '{}' FROM target WHERE name = 'IC 1805 r1c1';
         INSERT INTO acquiredimage (projectId, targetId, acquireddate, filtername, gradingStatus, metadata)
         SELECT projectid, Id, 0, 'Ha', 1, '{}' FROM target WHERE name = 'IC 1805 r1c1';",
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
        json!({"desired": 144, "acquired": 10, "accepted": 8, "rejected": 2, "targets": 2}),
        "{row}"
    );
    // The list carries what a thumbnail needs: where to look and what to draw.
    assert_eq!(row["framing"]["center"]["ra_degrees"], 38.2);
    assert_eq!(row["framing"]["position_angle_degrees"], 15.0);
    assert_eq!(row["framing"]["survey_id"], "dss2_color");
    assert_eq!(row["framing"]["mosaic"]["rows"], 2);
    assert_eq!(row["framing"]["panel"]["width_degrees"], 2.0);
    assert!(
        row["framing"]["extent"]["height_degrees"].as_f64().unwrap() > 2.0,
        "{}",
        row["framing"]
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

#[tokio::test]
async fn a_copied_database_file_is_named_and_left_out_of_planning() {
    let a = activated().await;
    // Adopt the rig database, then copy the file byte for byte and register the copy.
    let (status, _) = call(&a.f.app, "GET", "/plans", Value::Null, None).await;
    assert_eq!(status, StatusCode::OK);
    let original = a.f._dir.path().join("rig.sqlite");
    let copy = a.f._dir.path().join("rig-copy.sqlite");
    std::fs::copy(&original, &copy).unwrap();
    let context = crate::server::database_context::DatabaseContext::new(
        "rig-copy".into(),
        "RedCat copy".into(),
        copy.to_string_lossy().into(),
        vec![a.f._dir.path().to_string_lossy().into()],
        None,
        None,
        None,
        a.f._dir.path().join("cache"),
    )
    .unwrap();
    a.f.state
        .databases
        .write()
        .unwrap()
        .insert("rig-copy".into(), Arc::new(context));

    let (status, listed) = call(&a.f.app, "GET", "/plans", Value::Null, None).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    let warnings = listed["data"]["warnings"].as_array().unwrap();
    assert!(
        warnings.iter().any(|w| w
            .as_str()
            .unwrap()
            .starts_with("RedCat copy: carries the same catalog identity as RedCat rig")),
        "{warnings:?}"
    );
    // No plan links to the copy; the original keeps its link.
    for row in listed["data"]["rows"].as_array().unwrap() {
        for link in row["links"].as_array().unwrap() {
            assert_ne!(link["catalog_slug"], "rig-copy", "{row}");
        }
    }
    let (_, rigs) = call(&a.f.app, "GET", "/rigs/profiles", Value::Null, None).await;
    let slugs: Vec<&str> = rigs["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["catalog_slug"].as_str().unwrap())
        .collect();
    assert!(
        slugs.contains(&"rig") && !slugs.contains(&"rig-copy"),
        "{slugs:?}"
    );
    // Activation writes the original, never the copy, and says why.
    let (status, preview) = call(
        &a.f.app,
        "POST",
        &format!("/projects/{}/activation/preview", a.project),
        json!({}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    assert_eq!(preview["data"]["rigs"][0]["catalog_slug"], "rig");
    assert!(
        preview["data"]["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w.as_str().unwrap().contains("RedCat copy")),
        "{preview}"
    );
    let _ = (a.rig, a.objective);
}

/// A folder import can give one band a short plan first (flats or test
/// frames taken as lights). The draft takes the band's main plan instead.
#[tokio::test]
async fn an_imported_plan_takes_each_bands_main_exposure_plan() {
    let f = Fixture::new();
    let path = register(&f, "blue", "Blue rig", &[(1, "M42", Some(Uuid::new_v4()))]);
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch(
        "INSERT INTO exposuretemplate (Id, profileId, name, filtername, gain, offset, bin, readoutmode, twilightlevel, moonavoidanceenabled,
            moonavoidanceseparation, moonavoidancewidth, maximumhumidity, defaultexposure, moonrelaxscale, moonrelaxmaxaltitude,
            moonrelaxminaltitude, moondownenabled, ditherevery, minutesOffset, guid)
         VALUES (1, 'profile-x', 'B G100 O30 1x1', 'B', 100, 30, 1, -1, 0, 0, 60, 7, 0, 0.4, 0, 5, -15, 0, -1, 0, 'tmpl-b');
         INSERT INTO target (Id, name, active, ra, dec, epochcode, rotation, roi, projectid, guid)
         VALUES (1, 'M42', 1, 5.588, -5.39, 2, 0.0, 100, 1, 'tgt-1');
         INSERT INTO exposureplan (profileId, exposure, desired, acquired, accepted, targetid, exposureTemplateId, enabled, guid)
         VALUES ('profile-x', 0.4, 12, 12, 0, 1, 1, 1, 'ep-short'), ('profile-x', 180, 40, 40, 30, 1, 1, 1, 'ep-main');",
    )
    .unwrap();
    let (status, listed) = call(&f.app, "GET", "/plans", Value::Null, None).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    let project = listed["data"]["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["project"]["name"] == "M42")
        .unwrap()["project"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (_, plan) = call(
        &f.app,
        "GET",
        &format!("/projects/{project}/plan"),
        Value::Null,
        None,
    )
    .await;
    let contribution = &plan["data"]["plan"]["contributions"][0];
    assert_eq!(contribution["exposure_seconds"], 180.0, "{plan}");
    assert_eq!(plan["data"]["plan"]["objectives"][0]["goal"]["value"], 40);
}

#[tokio::test]
async fn a_target_scheduler_project_is_imported_as_framing_and_plan_drafts_once() {
    let f = Fixture::new();
    let guid = Uuid::new_v4();
    let path = register(&f, "heart", "Heart rig", &[(1, "Heart Nebula", Some(guid))]);
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute("UPDATE project SET priority=2 WHERE Id=1", [])
        .unwrap();
    db.execute_batch(
        "INSERT INTO exposuretemplate (Id, profileId, name, filtername, gain, offset, bin, readoutmode, twilightlevel, moonavoidanceenabled,
            moonavoidanceseparation, moonavoidancewidth, maximumhumidity, defaultexposure, moonrelaxscale, moonrelaxmaxaltitude,
            moonrelaxminaltitude, moondownenabled, ditherevery, minutesOffset, guid)
         VALUES (1, 'profile-x', 'Ha 300', 'Ha', 100, 30, 1, -1, 0, 0, 60, 7, 0, 300, 0, 5, -15, 0, -1, 0, 'tmpl-ha'),
                (2, 'profile-x', 'OIII 180', 'OIII', 100, 30, 1, -1, 0, 0, 60, 7, 0, 180, 0, 5, -15, 0, -1, 0, 'tmpl-o3');
         INSERT INTO target (Id, name, active, ra, dec, epochcode, rotation, roi, projectid, guid)
         VALUES (1, 'Heart r1c1', 1, 2.5333, 0.0, 2, 0.0, 100, 1, 'tgt-1'),
                (2, 'Heart r1c2', 1, 2.4267, 0.0, 2, 0.0, 100, 1, 'tgt-2');
         INSERT INTO exposureplan (profileId, exposure, desired, acquired, accepted, targetid, exposureTemplateId, enabled, guid)
         VALUES ('profile-x', 300, 30, 4, 3, 1, 1, 1, 'ep-1'), ('profile-x', 300, 30, 0, 0, 2, 1, 1, 'ep-2'),
                ('profile-x', 180, 20, 0, 0, 1, 2, 1, 'ep-3'), ('profile-x', 180, 20, 0, 0, 2, 2, 1, 'ep-4');",
    )
    .unwrap();
    let (status, listed) = call(&f.app, "GET", "/plans", Value::Null, None).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    let row = listed["data"]["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["project"]["name"] == "Heart Nebula")
        .unwrap();
    let framing = &row["framing"];
    assert_eq!(framing["source"], "draft", "{framing}");
    assert_eq!(framing["target_name"], "Heart");
    assert_eq!(framing["mosaic"]["rows"], 1);
    assert_eq!(framing["mosaic"]["columns"], 2);
    assert_eq!(framing["panels"], 2);
    // The center is the midpoint of the two targets, in degrees.
    assert!(
        (framing["center"]["ra_degrees"].as_f64().unwrap() - 37.2).abs() < 1e-2,
        "{framing}"
    );
    assert_eq!(row["plan"]["objectives"], 2);
    assert_eq!(row["plan"]["rigs"], 1);
    assert_eq!(row["progress"]["accepted"], 3);
    let project = row["project"]["id"].as_str().unwrap();
    let (_, plan) = call(
        &f.app,
        "GET",
        &format!("/projects/{project}/plan"),
        Value::Null,
        None,
    )
    .await;
    let objectives = plan["data"]["plan"]["objectives"].as_array().unwrap();
    assert!(objectives.iter().all(|o| o["priority"] == 2));
    let ha = objectives
        .iter()
        .find(|o| o["bandpass_id"] == "h_alpha")
        .expect("H-alpha objective");
    assert_eq!(ha["goal"]["kind"], "frames", "{ha}");
    assert_eq!(ha["goal"]["value"], 30);
    let o3 = objectives
        .iter()
        .find(|o| o["bandpass_id"] == "oiii")
        .unwrap();
    assert_eq!(o3["goal"]["value"], 20);
    let contributions = plan["data"]["plan"]["contributions"].as_array().unwrap();
    assert_eq!(contributions.len(), 2);
    let ha_c = contributions
        .iter()
        .find(|c| c["objective_id"] == ha["id"])
        .unwrap();
    assert_eq!(ha_c["template"]["template_id"], 1);
    assert_eq!(ha_c["template"]["filter_name"], "Ha");
    assert_eq!(ha_c["exposure_seconds"], 300.0);
    assert_eq!(ha_c["enabled"], true);
    // Nobody has edited the plan here, so a change in Target Scheduler is
    // taken in; the framing, the same there, is not saved again.
    db.execute("UPDATE project SET priority=0 WHERE Id=1", [])
        .unwrap();
    let (_, again) = call(&f.app, "GET", "/plans", Value::Null, None).await;
    let row2 = again["data"]["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["project"]["name"] == "Heart Nebula")
        .unwrap();
    assert_eq!(row2["framing"]["revision"], 1);
    assert_eq!(row2["plan"]["revision"], 2);
    let (_, retained) = call(
        &f.app,
        "GET",
        &format!("/projects/{project}/plan"),
        Value::Null,
        None,
    )
    .await;
    assert!(retained["data"]["plan"]["objectives"]
        .as_array()
        .unwrap()
        .iter()
        .all(|o| o["priority"] == 0));
}

/// Separate targets that are not one mosaic: the drafts frame the first
/// alone, from its own exposure plans, and the listing says so. Merging the
/// others' plans would raise the first target's desired counts.
#[tokio::test]
async fn a_project_of_separate_targets_is_imported_as_its_first_target_alone() {
    let f = Fixture::new();
    let path = register(
        &f,
        "galaxies",
        "Galaxy rig",
        &[(1, "Local group", Some(Uuid::new_v4()))],
    );
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch(
        "INSERT INTO exposuretemplate (Id, profileId, name, filtername, gain, offset, bin, readoutmode, twilightlevel, moonavoidanceenabled,
            moonavoidanceseparation, moonavoidancewidth, maximumhumidity, defaultexposure, moonrelaxscale, moonrelaxmaxaltitude,
            moonrelaxminaltitude, moondownenabled, ditherevery, minutesOffset, guid)
         VALUES (1, 'profile-x', 'Ha 300', 'Ha', 100, 30, 1, -1, 0, 0, 60, 7, 0, 300, 0, 5, -15, 0, -1, 0, 'tmpl-ha');
         INSERT INTO target (Id, name, active, ra, dec, epochcode, rotation, roi, projectid, guid)
         VALUES (1, 'M 31', 1, 0.7123, 41.27, 2, 0.0, 100, 1, 'tgt-m31'),
                (2, 'M 33', 1, 1.5642, 30.66, 2, 0.0, 100, 1, 'tgt-m33');
         INSERT INTO exposureplan (profileId, exposure, desired, acquired, accepted, targetid, exposureTemplateId, enabled, guid)
         VALUES ('profile-x', 300, 20, 0, 0, 1, 1, 1, 'ep-m31'), ('profile-x', 300, 50, 0, 0, 2, 1, 1, 'ep-m33');",
    )
    .unwrap();
    let (status, listed) = call(&f.app, "GET", "/plans", Value::Null, None).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert!(
        listed["data"]["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w
                .as_str()
                .unwrap()
                .contains("Local group has 2 separate targets")),
        "{listed}"
    );
    let row = listed["data"]["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["project"]["name"] == "Local group")
        .unwrap();
    assert_eq!(row["framing"]["target_name"], "M 31", "{row}");
    assert_eq!(row["framing"]["panels"], 1);
    let project = row["project"]["id"].as_str().unwrap();
    let (_, plan) = call(
        &f.app,
        "GET",
        &format!("/projects/{project}/plan"),
        Value::Null,
        None,
    )
    .await;
    let objectives = plan["data"]["plan"]["objectives"].as_array().unwrap();
    assert_eq!(objectives.len(), 1);
    // M 31's own 20, not M 33's 50.
    assert_eq!(objectives[0]["goal"]["value"], 20, "{plan}");
}

/// A server without database management still plans over its catalogs. The
/// file is only read and gets a derived identity; the first managing server
/// to list it writes that same identity into the file, so the rig is stable.
#[tokio::test]
async fn planning_reads_catalogs_without_database_management_and_keeps_the_rig_when_it_is_granted()
{
    let f = Fixture::new();
    let guid = Uuid::new_v4();
    let path = register(&f, "shed", "Shed data", &[(1, "Pelican", Some(guid))]);
    f.state.set_allow_database_management(false);
    let (status, listed) = call(&f.app, "GET", "/plans", Value::Null, None).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    let rows = listed["data"]["rows"].as_array().unwrap();
    let row = rows
        .iter()
        .find(|r| r["links"][0]["catalog_slug"] == "shed")
        .unwrap_or_else(|| panic!("no plan for the read-only catalog: {listed}"));
    assert_eq!(row["project"]["name"], "Pelican");
    let rig = row["links"][0]["rig"]["id"].as_str().unwrap().to_owned();
    let has_identity = || {
        rusqlite::Connection::open(&path)
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name = 'psf_guard_catalog_identity'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .unwrap()
            == 1
    };
    assert!(
        !has_identity(),
        "a read-only server must not write the file"
    );
    // Listing again keeps the same rig without touching the file.
    let (_, again) = call(&f.app, "GET", "/plans", Value::Null, None).await;
    let same = again["data"]["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["links"][0]["catalog_slug"] == "shed")
        .unwrap();
    assert_eq!(same["links"][0]["rig"]["id"], rig);
    assert!(!has_identity());

    f.state.set_allow_database_management(true);
    let (status, managed) = call(&f.app, "GET", "/plans", Value::Null, None).await;
    assert_eq!(status, StatusCode::OK, "{managed}");
    let kept = managed["data"]["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["links"][0]["catalog_slug"] == "shed")
        .unwrap();
    assert_eq!(
        kept["links"][0]["rig"]["id"], rig,
        "the rig survives adoption"
    );
    assert!(
        has_identity(),
        "a managing server writes the identity it planned under"
    );
}

/// Two databases that each made their own project for one target are two
/// plans; attaching one to the other moves its links and retires it, and
/// detaching hands a database's project a plan of its own again.
#[tokio::test]
async fn attaching_a_plan_moves_its_links_and_detaching_hands_them_back() {
    let f = Fixture::new();
    let c925_guid = Uuid::new_v4();
    let redcat_guid = Uuid::new_v4();
    register(
        &f,
        "c925",
        "C925 data",
        &[(1, "Heart by C925", Some(c925_guid))],
    );
    let redcat = register(
        &f,
        "redcat",
        "Redcat data",
        &[(1, "Heart", Some(redcat_guid))],
    );
    rusqlite::Connection::open(&redcat)
        .unwrap()
        .execute(
            "INSERT INTO target (name, active, ra, dec, epochcode, rotation, roi, projectid, guid) VALUES ('IC 1805', 1, 2.5467, 61.45, 2, 15.0, 100, 1, 'target-heart')",
            [],
        )
        .unwrap();
    let (status, listed) = call(&f.app, "GET", "/plans", Value::Null, None).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    let rows = listed["data"]["rows"].as_array().unwrap().clone();
    let plan_of = |name: &str| {
        rows.iter()
            .find(|r| r["project"]["name"] == name)
            .unwrap_or_else(|| panic!("{name} in {rows:?}"))
            .clone()
    };
    let heart = plan_of("Heart");
    let by_c925 = plan_of("Heart by C925");
    let heart_id = heart["project"]["id"].as_str().unwrap().to_owned();
    let by_c925_id = by_c925["project"]["id"].as_str().unwrap().to_owned();
    assert_eq!(heart["framing"]["source"], "draft");

    let (status, attached) = call(
        &f.app,
        "POST",
        &format!("/projects/{heart_id}/attach"),
        json!({"from_project_id": by_c925_id}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{attached}");
    assert_eq!(attached["data"]["moved_links"], 1);
    assert_eq!(attached["data"]["absorbed"]["name"], "Heart by C925");
    assert_eq!(
        attached["data"]["framing_taken"], false,
        "the plan kept its own framing"
    );
    // One plan with two rigs; the absorbed plan is gone, and its drafts with it.
    let (_, listed) = call(&f.app, "GET", "/plans", Value::Null, None).await;
    let rows = listed["data"]["rows"].as_array().unwrap();
    assert!(
        rows.iter().all(|r| r["project"]["name"] != "Heart by C925"),
        "{rows:?}"
    );
    let heart = rows
        .iter()
        .find(|r| r["project"]["id"] == heart_id)
        .unwrap();
    let links = heart["links"].as_array().unwrap();
    assert_eq!(links.len(), 2, "{links:?}");
    assert!(links
        .iter()
        .any(|l| l["catalog_name"] == "C925 data" && l["source_name"] == "Heart by C925"));
    assert_eq!(heart["framing"]["target_name"], "IC 1805");
    // The absorbed plan is not there to attach again, and a plan cannot absorb itself.
    assert_eq!(
        call(
            &f.app,
            "POST",
            &format!("/projects/{heart_id}/attach"),
            json!({"from_project_id": by_c925_id}),
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &f.app,
            "POST",
            &format!("/projects/{heart_id}/attach"),
            json!({"from_project_id": heart_id}),
            None
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );

    // Detaching gives the C925 project a plan of its own again, under its own name.
    let (status, detached) = call(
        &f.app,
        "POST",
        &format!("/projects/{heart_id}/detach"),
        json!({"catalog_slug": "c925", "source_project_guid": c925_guid, "name": "Heart by C925"}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{detached}");
    let fresh = detached["data"]["id"].as_str().unwrap().to_owned();
    assert_ne!(fresh, by_c925_id, "a new plan, not the old one back");
    let (_, listed) = call(&f.app, "GET", "/plans", Value::Null, None).await;
    let rows = listed["data"]["rows"].as_array().unwrap();
    let again = rows.iter().find(|r| r["project"]["id"] == fresh).unwrap();
    assert_eq!(again["project"]["name"], "Heart by C925");
    assert_eq!(again["links"].as_array().unwrap().len(), 1);
    assert_eq!(
        rows.iter()
            .find(|r| r["project"]["id"] == heart_id)
            .unwrap()["links"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    // Two plans on the same database cannot be joined: a rig shoots one project per plan.
    let (status, twice) = call(
        &f.app,
        "POST",
        &format!("/projects/{heart_id}/attach"),
        json!({"from_project_id": fresh}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{twice}");
    assert_eq!(
        call(
            &f.app,
            "POST",
            &format!("/projects/{heart_id}/detach"),
            json!({"catalog_slug": "redcat", "source_project_guid": c925_guid, "name": "x"}),
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
}

/// The listing's row for a plan, by name.
fn plan_named(listed: &Value, name: &str) -> Value {
    listed["data"]["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["project"]["name"] == name)
        .unwrap_or_else(|| panic!("no {name} in {listed}"))
        .clone()
}

fn warns(listed: &Value, text: &str) -> bool {
    listed["data"]["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|w| w.as_str().unwrap().contains(text))
}

/// A rig database N.I.N.A. is writing is read beside its writer, and the
/// listing leaves no lock behind. One locked outright shows what the last
/// listing read; with no earlier read its links stay, their state unknown
/// rather than gone.
#[tokio::test]
async fn the_plan_list_reads_rig_databases_beside_their_writers() {
    let f = Fixture::new();
    let path = register(
        &f,
        "nina",
        "NINA rig",
        &[(1, "Pelican", Some(Uuid::new_v4()))],
    );
    let nina = rusqlite::Connection::open(&path).unwrap();
    nina.execute(
        "INSERT INTO target (name, active, ra, dec, epochcode, rotation, roi, projectid, guid) VALUES ('IC 5070', 1, 20.85, 44.35, 2, 0.0, 100, 1, 'target-pelican')",
        [],
    )
    .unwrap();
    let (status, first) = call(&f.app, "GET", "/plans", Value::Null, None).await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(
        plan_named(&first, "Pelican")["links"][0]["source_row_id"],
        1
    );

    // N.I.N.A. mid-write holds the database's write lock.
    nina.busy_timeout(std::time::Duration::ZERO).unwrap();
    nina.execute_batch("BEGIN IMMEDIATE; UPDATE project SET description='writing' WHERE Id=1;")
        .unwrap();
    let (status, beside) = call(&f.app, "GET", "/plans", Value::Null, None).await;
    assert_eq!(status, StatusCode::OK, "{beside}");
    assert!(!warns(&beside, "NINA rig"), "{beside}");
    assert_eq!(
        plan_named(&beside, "Pelican")["links"][0]["source_state"],
        1
    );
    // Its commit needs every reader gone, and none is left.
    nina.execute_batch("COMMIT").unwrap();

    nina.execute_batch("BEGIN EXCLUSIVE").unwrap();
    let started = std::time::Instant::now();
    let (status, locked) = call(&f.app, "GET", "/plans", Value::Null, None).await;
    assert_eq!(status, StatusCode::OK, "{locked}");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
    assert!(
        warns(
            &locked,
            "NINA rig: is busy, so the list shows what it held at the last listing"
        ),
        "{locked}"
    );
    let link = &plan_named(&locked, "Pelican")["links"][0];
    assert_eq!(link["source_row_id"], 1, "{link}");
    assert_eq!(link["source_unread"], false);
    assert_eq!(link["targets"][0]["name"], "IC 5070");

    // A server that has not read it yet keeps the link and says so.
    f.state
        .director
        .as_ref()
        .unwrap()
        .plans_memo
        .lock()
        .unwrap()
        .reads
        .clear();
    let (status, unread) = call(&f.app, "GET", "/plans", Value::Null, None).await;
    assert_eq!(status, StatusCode::OK, "{unread}");
    assert!(
        warns(
            &unread,
            "NINA rig: is busy, so its plans show no rows or progress until it can be read"
        ),
        "{unread}"
    );
    let link = &plan_named(&unread, "Pelican")["links"][0];
    assert_eq!(link["catalog_slug"], "nina", "{link}");
    assert_eq!(link["source_unread"], true);
    assert_eq!(link["source_row_id"], Value::Null);

    nina.execute_batch("ROLLBACK").unwrap();
    let (_, free) = call(&f.app, "GET", "/plans", Value::Null, None).await;
    assert_eq!(
        plan_named(&free, "Pelican")["links"][0]["source_unread"],
        false
    );
}

/// A new database N.I.N.A. is writing becomes a rig and its projects plans
/// at once. Its identity row waits for a listing that finds the file free,
/// and the rig stays the same.
#[tokio::test]
async fn a_busy_new_database_is_planned_at_once_and_gets_its_identity_row_later() {
    let f = Fixture::new();
    let path = register(&f, "veil", "Veil rig", &[(1, "Veil", Some(Uuid::new_v4()))]);
    let nina = rusqlite::Connection::open(&path).unwrap();
    nina.execute_batch("BEGIN IMMEDIATE; UPDATE project SET description='writing' WHERE Id=1;")
        .unwrap();
    let has_identity = || {
        rusqlite::Connection::open(&path)
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name = 'psf_guard_catalog_identity'",
                [],
                |r| r.get::<_, i64>(0),
            )
            .unwrap()
            == 1
    };
    let (status, busy) = call(&f.app, "GET", "/plans", Value::Null, None).await;
    assert_eq!(status, StatusCode::OK, "{busy}");
    assert!(
        warns(
            &busy,
            "Veil rig: busy, so its identity is written on a later listing"
        ),
        "{busy}"
    );
    let rig = plan_named(&busy, "Veil")["links"][0]["rig"]["id"].clone();
    assert!(rig.is_string(), "{busy}");
    assert!(!has_identity());

    nina.execute_batch("COMMIT").unwrap();
    let (status, free) = call(&f.app, "GET", "/plans", Value::Null, None).await;
    assert_eq!(status, StatusCode::OK, "{free}");
    assert!(!warns(&free, "Veil rig"), "{free}");
    assert!(has_identity());
    assert_eq!(plan_named(&free, "Veil")["links"][0]["rig"]["id"], rig);
}

/// While a listing searches frame headers, which can scan every image
/// folder, it holds no store gate and no rig database lock: framing edits,
/// rig check-ins and N.I.N.A.'s own writes go on beside it.
#[tokio::test]
async fn writes_and_check_ins_go_on_while_a_listing_reads_frame_headers() {
    let f = Fixture::new();
    let path = register(
        &f,
        "nina",
        "NINA rig",
        &[(1, "Pelican", Some(Uuid::new_v4()))],
    );
    let (status, first) = call(&f.app, "GET", "/plans", Value::Null, None).await;
    assert_eq!(status, StatusCode::OK, "{first}");
    let rig = Uuid::parse_str(
        plan_named(&first, "Pelican")["links"][0]["rig"]["id"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    // A new frame sends the rig's headers to be searched again, and a new
    // project is there to take in.
    let nina = rusqlite::Connection::open(&path).unwrap();
    nina.busy_timeout(std::time::Duration::ZERO).unwrap();
    nina.execute_batch(
        "INSERT INTO acquiredimage (projectId, targetId, acquireddate, filtername, gradingStatus, metadata)
         VALUES (1, 0, 0, 'Ha', 0, '{\"FileName\":\"frame.fits\"}')",
    )
    .unwrap();
    nina.execute(
        "INSERT INTO project (Id, profileId, name, description, state, priority, isMosaic, flatsHandling, guid) VALUES (2, 'profile-x', 'Veil', '', 1, 1, 0, 0, ?1)",
        [Uuid::new_v4().to_string()],
    )
    .unwrap();

    let service = f.state.director.clone().unwrap();
    let (held_tx, held) = tokio::sync::oneshot::channel();
    let (release, release_rx) = std::sync::mpsc::channel();
    service.plans_memo.lock().unwrap().hold = Some((held_tx, release_rx));
    let listing = tokio::spawn({
        let app = f.app.clone();
        async move { call(&app, "GET", "/plans", Value::Null, None).await }
    });
    held.await.unwrap();

    let project = Uuid::new_v4();
    let (status, created) = call(
        &f.app,
        "POST",
        "/projects",
        json!({"id": project, "name": "Crescent"}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let draft = json!({
        "project_id": project, "revision": 0, "target_name": "NGC 6888",
        "center": {"ra_degrees": 303.0, "dec_degrees": 38.35},
        "position_angle_degrees": 0.0,
        "mosaic": {"rows": 1, "columns": 1, "overlap_percent": 20},
        "panel_rig_id": null, "panel": null,
        "shown_rig_ids": [], "survey_id": "dss2_color", "view_fov_degrees": 4.0,
        "updated_at_ms": 0,
    });
    let (status, framed) = call(
        &f.app,
        "PUT",
        &format!("/projects/{project}/framing"),
        draft,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{framed}");
    let status_report = json!({
        "coordinator_instance_id": service.instance_id, "catalog_id": rig,
        "session_id": "night-1", "reported_at_ms": 1, "status": {"phase": "exposing"},
    });
    let (status, reported) = call(
        &f.app,
        "POST",
        &format!("/rigs/{rig}/status"),
        status_report,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{reported}");
    nina.execute(
        "UPDATE project SET description='still writing' WHERE Id=1",
        [],
    )
    .unwrap();
    let (status, profile) = call(
        &f.app,
        "GET",
        "/catalogs/nina/rig/profile",
        Value::Null,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{profile}");
    assert!(!listing.is_finished());

    release.send(()).unwrap();
    let (status, listed) = listing.await.unwrap();
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert_eq!(
        plan_named(&listed, "Veil")["links"][0]["catalog_slug"],
        "nina"
    );
    assert_eq!(plan_named(&listed, "Crescent")["framing"]["revision"], 1);
}

/// One unreadable record, or one database that lost its project table,
/// costs only its own part of the list.
#[tokio::test]
async fn an_unreadable_record_or_database_costs_only_its_own_part_of_the_list() {
    let f = Fixture::new();
    let path = register(
        &f,
        "nina",
        "NINA rig",
        &[
            (1, "Pelican", Some(Uuid::new_v4())),
            (2, "Veil", Some(Uuid::new_v4())),
        ],
    );
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch(
            "INSERT INTO target (name, active, ra, dec, epochcode, rotation, roi, projectid, guid)
             VALUES ('IC 5070', 1, 20.85, 44.35, 2, 0.0, 100, 1, 'target-pelican'),
                    ('NGC 6960', 1, 20.76, 30.71, 2, 0.0, 100, 2, 'target-veil')",
        )
        .unwrap();
    let gone = register(
        &f,
        "gone",
        "Gone rig",
        &[(1, "Crescent", Some(Uuid::new_v4()))],
    );
    let (status, first) = call(&f.app, "GET", "/plans", Value::Null, None).await;
    assert_eq!(status, StatusCode::OK, "{first}");
    let pelican = plan_named(&first, "Pelican")["project"]["id"].clone();
    assert_eq!(plan_named(&first, "Pelican")["framing"]["source"], "draft");

    rusqlite::Connection::open(f._dir.path().join("meta.sqlite"))
        .unwrap()
        .execute(
            "UPDATE framing_draft SET payload='{}' WHERE project_id=?1",
            [pelican.as_str().unwrap()],
        )
        .unwrap();
    rusqlite::Connection::open(&gone)
        .unwrap()
        .execute_batch("DROP TABLE project")
        .unwrap();
    let (status, listed) = call(&f.app, "GET", "/plans", Value::Null, None).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    assert!(
        warns(
            &listed,
            "Pelican: its framing draft could not be read, so the list leaves it out"
        ),
        "{listed}"
    );
    assert_eq!(plan_named(&listed, "Pelican")["framing"], Value::Null);
    assert_eq!(
        plan_named(&listed, "Pelican")["links"][0]["source_row_id"],
        1
    );
    assert_eq!(
        plan_named(&listed, "Veil")["framing"]["target_name"],
        "NGC 6960"
    );
    assert!(
        warns(&listed, "Gone rig: has no Target Scheduler project table"),
        "{listed}"
    );
    let link = &plan_named(&listed, "Crescent")["links"][0];
    assert_eq!(link["catalog_slug"], "gone", "{link}");
    assert_eq!(link["source_unread"], true);
}

/// The list names what it leaves out and the real reason, rather than
/// stopping short or blaming a missing table.
#[tokio::test]
async fn the_plan_list_names_what_it_leaves_out_and_why() {
    let f = Fixture::new();
    let path = register(&f, "big", "Big rig", &[]);
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch("BEGIN").unwrap();
    for id in 1..=4097 {
        db.execute(
            "INSERT INTO project (Id, profileId, name, description, state, priority, isMosaic, flatsHandling, guid) VALUES (?1, 'profile-x', 'Project', '', 1, 1, 0, 0, ?2)",
            rusqlite::params![id, Uuid::new_v4().to_string()],
        )
        .unwrap();
    }
    db.execute_batch("COMMIT").unwrap();
    {
        let mut store = f.state.director.as_ref().unwrap().writer.lock().unwrap();
        for i in 0..psf_guard_director_meta::MAX_PLANS {
            store
                .create_project(Uuid::new_v4(), &format!("Plan {i}"))
                .unwrap();
        }
    }
    // A thousand plans is more than `call` reads back.
    let response = f
        .app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/director/v1/plans")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let listed: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap();
    assert!(
        warns(&listed, "Big rig: holds more than 4096 projects"),
        "{}",
        listed["data"]["warnings"]
    );
    assert!(
        !warns(&listed, "has no Target Scheduler project table"),
        "{}",
        listed["data"]["warnings"]
    );
    assert!(
        warns(&listed, "Only the first 1024 plans are listed."),
        "{}",
        listed["data"]["warnings"]
    );
    assert_eq!(
        listed["data"]["rows"].as_array().unwrap().len(),
        psf_guard_director_meta::MAX_PLANS
    );
}

/// A file planned under its derived identity keeps it: an explicit review
/// cannot give it a second catalog and rig and leave the first behind.
#[tokio::test]
async fn an_explicit_review_cannot_replace_the_identity_a_file_is_planned_under() {
    let f = Fixture::new();
    let guid = Uuid::new_v4();
    let path = register(&f, "shed", "Shed data", &[(1, "Pelican", Some(guid))]);
    f.state.set_allow_database_management(false);
    let (status, listed) = call(&f.app, "GET", "/plans", Value::Null, None).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    let pelican = plan_named(&listed, "Pelican");
    let rig = pelican["links"][0]["rig"]["id"].clone();
    f.state.set_allow_database_management(true);

    let other = Uuid::new_v4();
    let (status, refused) = call(
        &f.app,
        "POST",
        "/catalogs/shed/rig/preview",
        json!({"catalog_id": other}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    let mapping = json!({"catalog_id": other, "mappings": [{
        "catalog_id": other, "source_project_guid": guid, "source_profile_id": "profile-x",
        "project_id": pelican["project"]["id"], "rig_id": rig,
    }]});
    let (status, refused) = call(
        &f.app,
        "POST",
        "/catalogs/shed/adoption/preview",
        mapping,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");

    // Reviewing its own identity applies the binding it already has.
    let (status, reviewed) = call(
        &f.app,
        "POST",
        "/catalogs/shed/rig/preview",
        json!({"catalog_id": rig}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{reviewed}");
    let (status, applied) = call(
        &f.app,
        "POST",
        "/catalogs/shed/rig/apply",
        json!({"plan": {"catalog_id": rig}, "preview_digest": reviewed["data"]["preview_digest"]}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{applied}");
    assert_eq!(applied["data"]["binding"]["rig"]["id"], rig);
    let written = crate::catalog_identity::read(&rusqlite::Connection::open(&path).unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(json!(written.id), rig);
    let (_, again) = call(&f.app, "GET", "/plans", Value::Null, None).await;
    assert_eq!(plan_named(&again, "Pelican")["links"][0]["rig"]["id"], rig);
}

/// A plan taken in from Target Scheduler and not touched here follows the
/// project: a desired count raised there is taken in on the next listing,
/// under the same objectives. Once the plan is saved here it is the
/// operator's, and Target Scheduler's later changes stay out of it.
#[tokio::test]
async fn an_imported_plan_follows_target_scheduler_until_it_is_saved_here() {
    let f = Fixture::new();
    let path = register(
        &f,
        "heart",
        "Heart rig",
        &[(1, "Heart Nebula", Some(Uuid::new_v4()))],
    );
    let db = rusqlite::Connection::open(&path).unwrap();
    db.execute_batch(
        "INSERT INTO exposuretemplate (Id, profileId, name, filtername, gain, offset, bin, readoutmode, twilightlevel, moonavoidanceenabled,
            moonavoidanceseparation, moonavoidancewidth, maximumhumidity, defaultexposure, moonrelaxscale, moonrelaxmaxaltitude,
            moonrelaxminaltitude, moondownenabled, ditherevery, minutesOffset, guid)
         VALUES (1, 'profile-x', 'Ha 300', 'Ha', 100, 30, 1, -1, 0, 0, 60, 7, 0, 300, 0, 5, -15, 0, -1, 0, 'tmpl-ha');
         INSERT INTO target (Id, name, active, ra, dec, epochcode, rotation, roi, projectid, guid)
         VALUES (1, 'Heart r1c1', 1, 2.5333, 0.0, 2, 0.0, 100, 1, 'tgt-1'),
                (2, 'Heart r1c2', 1, 2.4267, 0.0, 2, 0.0, 100, 1, 'tgt-2');
         INSERT INTO exposureplan (profileId, exposure, desired, acquired, accepted, targetid, exposureTemplateId, enabled, guid)
         VALUES ('profile-x', -1, 30, 4, 3, 1, 1, 1, 'ep-1'), ('profile-x', -1, 30, 0, 0, 2, 1, 1, 'ep-2');",
    )
    .unwrap();
    let list = || {
        let app = f.app.clone();
        async move {
            let (status, listed) = call(&app, "GET", "/plans", Value::Null, None).await;
            assert_eq!(status, StatusCode::OK, "{listed}");
            listed
        }
    };
    let listed = list().await;
    let project = Uuid::parse_str(
        listed["data"]["rows"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["project"]["name"] == "Heart Nebula")
            .unwrap()["project"]["id"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    let saved = || {
        let store = f.state.director.as_ref().unwrap().writer.lock().unwrap();
        (
            store.framing_draft(project).unwrap().unwrap(),
            store.plan_draft(project).unwrap().unwrap(),
            store.draft_import(project).unwrap(),
        )
    };
    let (framing, plan, import) = saved();
    assert_eq!((framing.revision, plan.revision), (1, 1));
    let import = import.expect("the import is recorded");
    assert_eq!((import.framing_revision, import.plan_revision), (1, 1));
    assert_eq!(plan.objectives[0].goal, Goal::Frames { value: 30 });

    // Another survey is how the person looks at it, not an edit.
    {
        let mut store = f.state.director.as_ref().unwrap().writer.lock().unwrap();
        let mut viewed = framing.clone();
        viewed.survey_id = "nina:FramingAssistantCache".into();
        let viewed = store.save_framing_draft(&viewed, 1).unwrap();
        assert_eq!((viewed.revision, viewed.layout_revision), (2, 1));
    }
    // Raised in Target Scheduler: the plan takes it in, framing untouched.
    db.execute("UPDATE exposureplan SET desired=45", [])
        .unwrap();
    list().await;
    let (framing2, plan2, import2) = saved();
    assert_eq!(framing2.revision, 2);
    assert_eq!(framing2.survey_id, "nina:FramingAssistantCache");
    assert_eq!(plan2.revision, 2);
    assert_eq!(plan2.objectives[0].goal, Goal::Frames { value: 45 });
    assert_eq!(
        plan2.objectives[0].id, plan.objectives[0].id,
        "same objective"
    );
    assert_eq!(plan2.contributions[0].id, plan.contributions[0].id);
    assert_eq!(import2.unwrap().plan_revision, 2);
    // Nothing changed there: nothing is saved again.
    list().await;
    assert_eq!(saved().1.revision, 2);
    // A target moved there moves the framing, and the survey stays.
    db.execute("UPDATE target SET dec=1.0", []).unwrap();
    list().await;
    let (moved, _, _) = saved();
    assert_eq!((moved.revision, moved.layout_revision), (3, 3));
    assert!((moved.center.dec_degrees - 1.0).abs() < 1e-3, "{moved:?}");
    assert_eq!(moved.survey_id, "nina:FramingAssistantCache");

    // Saved here: the plan is the operator's from now on.
    {
        let mut store = f.state.director.as_ref().unwrap().writer.lock().unwrap();
        let mut edited = plan2.clone();
        edited.objectives[0].goal = Goal::Frames { value: 60 };
        store.save_plan_draft(&edited, 2).unwrap();
    }
    db.execute("UPDATE exposureplan SET desired=80", [])
        .unwrap();
    list().await;
    let (_, plan3, _) = saved();
    assert_eq!(plan3.revision, 3);
    assert_eq!(plan3.objectives[0].goal, Goal::Frames { value: 60 });
}
