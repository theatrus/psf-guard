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
    // A second listing changes nothing: the drafts are the operator's now.
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
    assert_eq!(row2["plan"]["revision"], 1);
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
        .all(|o| o["priority"] == 2));
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
