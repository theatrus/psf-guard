use super::activation::activated;
use super::*;
use psf_guard_director_core::priority::Scope;
use psf_guard_director_meta::preferences::Settings;

/// Two projects made in N.I.N.A., one closed, beside the plan's own project.
fn hand_projects(db: &Connection) {
    for (id, name, state) in [(10, "Hand made", 1), (11, "Old and closed", 3)] {
        db.execute(
            "INSERT INTO project (Id, profileId, name, description, state, priority, isMosaic, flatsHandling, guid,
                minimumtime, minimumaltitude, maximumAltitude, usecustomhorizon, horizonoffset, meridianwindow)
             VALUES (?1, 'profile-a', ?2, '', ?3, 1, 0, 0, ?4, 30, 0, 0, 0, 0, 0)",
            rusqlite::params![id, name, state, Uuid::new_v4().to_string()],
        )
        .unwrap();
    }
}

fn changes(report: &Value) -> Vec<(String, String)> {
    report["data"]["projects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|project| {
            let lines = project["changes"]
                .as_array()
                .unwrap()
                .iter()
                .map(|c| {
                    format!(
                        "{} {} → {}",
                        c["label"].as_str().unwrap(),
                        c["was"].as_str().unwrap(),
                        c["now"].as_str().unwrap()
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            (project["name"].as_str().unwrap().to_owned(), lines)
        })
        .collect()
}

#[tokio::test]
async fn rig_limits_reach_every_project_on_apply_and_a_plans_own_limits_win() {
    let a = activated().await;
    // The plan's project, made by an activation before any limit was set.
    let (status, preview) = call(
        &a.f.app,
        "POST",
        &format!("/projects/{}/activation/preview", a.project),
        json!({}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    let (status, applied) = call(
        &a.f.app,
        "POST",
        &format!("/projects/{}/activation/apply", a.project),
        json!({"preview_digest": preview["data"]["preview_digest"]}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{applied}");
    hand_projects(&a.db);
    {
        let mut store = a.f.state.director.as_ref().unwrap().writer.lock().unwrap();
        let mut rig = Settings::empty(Scope::Rig, a.rig);
        rig.scheduling.minimum_altitude_degrees = Some(25.0);
        rig.scheduling.meridian_window_minutes = Some(20);
        store.save_observing_settings(&rig).unwrap();
        let mut project = Settings::empty(Scope::Project, a.project);
        project.scheduling.minimum_altitude_degrees = Some(30.0);
        store.save_observing_settings(&project).unwrap();
    }
    // A limit nobody set, changed by hand, stays; a set one changed by hand
    // goes back to the rig's.
    a.db.execute(
        "UPDATE project SET horizonoffset=5, meridianwindow=45 WHERE name='Hand made'",
        [],
    )
    .unwrap();
    let path = format!("/rigs/{}/scheduling", a.rig);

    let (status, preview) = call(&a.f.app, "GET", &path, Value::Null, None).await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    assert_eq!(
        changes(&preview),
        [
            (
                "Hand made".into(),
                "minimum altitude 0° → 25°, meridian window 45 min → 20 min".into()
            ),
            (
                "Heart Nebula".into(),
                "minimum altitude 0° → 30°, meridian window off → 20 min".into()
            ),
            (
                "Old and closed".into(),
                "minimum altitude 0° → 25°, meridian window off → 20 min".into()
            ),
        ]
    );
    let data = &preview["data"];
    assert_eq!(data["projects"][1]["plan"]["name"], "Heart Nebula");
    assert_eq!(data["projects"][0]["plan"], Value::Null);
    assert_eq!(data["projects"][2]["state"], 3);
    assert_eq!(data["matching"], 0);
    assert_eq!(data["applied"], false);
    assert!(data["unset"]
        .as_array()
        .unwrap()
        .iter()
        .any(|label| label == "horizon offset"));
    // A preview writes nothing.
    let altitude = |name: &str| {
        a.db.query_row(
            "SELECT minimumaltitude FROM project WHERE name=?1",
            [name],
            |row| row.get::<_, f64>(0),
        )
        .unwrap()
    };
    assert_eq!(altitude("Hand made"), 0.0);

    let digest = data["digest"].as_str().unwrap().to_owned();
    let (status, applied) = call(
        &a.f.app,
        "POST",
        &format!("{path}/apply"),
        json!({"digest": digest}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{applied}");
    assert_eq!(applied["data"]["applied"], true);
    assert_eq!(
        (
            altitude("Hand made"),
            altitude("Heart Nebula"),
            altitude("Old and closed")
        ),
        (25.0, 30.0, 25.0)
    );
    let row =
        a.db.query_row(
            "SELECT meridianwindow, horizonoffset FROM project WHERE name='Hand made'",
            [],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, f64>(1)?)),
        )
        .unwrap();
    assert_eq!(row, (20, 5.0), "the horizon offset nobody set stays");

    // Everything matches now, and the plan's next activation agrees.
    let (_, again) = call(&a.f.app, "GET", &path, Value::Null, None).await;
    assert!(changes(&again).is_empty(), "{again}");
    assert_eq!(again["data"]["matching"], 3);
    let (_, check) = call(
        &a.f.app,
        "GET",
        &format!("/projects/{}/activation/check", a.project),
        Value::Null,
        None,
    )
    .await;
    let limits = check["data"]["rigs"][0]["changes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|change| {
            change["name"]
                .as_str()
                .is_some_and(|name| name.ends_with("scheduling limits"))
        });
    assert!(!limits, "{check}");
}

#[tokio::test]
async fn apply_refuses_when_a_project_changed_since_the_preview() {
    let a = activated().await;
    hand_projects(&a.db);
    {
        let mut store = a.f.state.director.as_ref().unwrap().writer.lock().unwrap();
        let mut rig = Settings::empty(Scope::Rig, a.rig);
        rig.scheduling.minimum_altitude_degrees = Some(25.0);
        store.save_observing_settings(&rig).unwrap();
    }
    let path = format!("/rigs/{}/scheduling", a.rig);
    let (_, preview) = call(&a.f.app, "GET", &path, Value::Null, None).await;
    let digest = preview["data"]["digest"].as_str().unwrap().to_owned();
    a.db.execute(
        "UPDATE project SET minimumaltitude=12 WHERE name='Hand made'",
        [],
    )
    .unwrap();
    let (status, refused) = call(
        &a.f.app,
        "POST",
        &format!("{path}/apply"),
        json!({"digest": digest}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    let altitude = |name: &str| {
        a.db.query_row(
            "SELECT minimumaltitude FROM project WHERE name=?1",
            [name],
            |row| row.get::<_, f64>(0),
        )
        .unwrap()
    };
    assert_eq!(
        (altitude("Hand made"), altitude("Old and closed")),
        (12.0, 0.0),
        "nothing is written"
    );
    let (status, bad) = call(
        &a.f.app,
        "POST",
        &format!("{path}/apply"),
        json!({"digest": "not-a-digest"}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
}

#[tokio::test]
async fn a_rig_without_a_database_here_says_so() {
    let a = activated().await;
    let other = {
        let mut store = a.f.state.director.as_ref().unwrap().writer.lock().unwrap();
        store.create_rig(Uuid::new_v4(), "Elsewhere").unwrap().id
    };
    let (status, report) = call(
        &a.f.app,
        "GET",
        &format!("/rigs/{other}/scheduling"),
        Value::Null,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{report}");
    assert!(
        report.to_string().contains("no registered database"),
        "{report}"
    );
}
