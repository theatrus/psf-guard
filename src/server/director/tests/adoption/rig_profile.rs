use super::database_rig::{RIG_APPLY, RIG_PREVIEW};
use super::*;

const PROFILE: &str = "/catalogs/catalog/rig/profile";

/// A minimal FITS header the way N.I.N.A. writes a binned frame.
fn write_fits(path: &std::path::Path, cards: &[&str]) {
    let mut header = Vec::new();
    for card in [
        "SIMPLE  =                    T",
        "BITPIX  =                   16",
        "NAXIS   =                    2",
        "NAXIS1  =                 3124",
        "NAXIS2  =                 2088",
    ]
    .iter()
    .chain(cards)
    .chain(["END"].iter())
    {
        let mut bytes = card.as_bytes().to_vec();
        bytes.resize(80, b' ');
        header.extend(bytes);
    }
    header.resize(header.len().div_ceil(2880) * 2880, b' ');
    let mut contents = header;
    contents.extend(vec![0_u8; 2880 * 5]);
    std::fs::write(path, contents).unwrap();
}

pub(super) async fn bound_fixture() -> (Fixture, Uuid) {
    let f = Fixture::new();
    f.source
        .execute_batch(
            r#"ALTER TABLE acquiredimage ADD COLUMN metadata TEXT;
             ALTER TABLE acquiredimage ADD COLUMN acquireddate INTEGER;
             UPDATE acquiredimage SET metadata='{"FileName":"old.fits"}', acquireddate=10;
             INSERT INTO acquiredimage(Id,gradingStatus,metadata,acquireddate) VALUES(2,1,'{"FileName":"C:\\frames\\newest.fits","ExposureDuration":60.0}',20);"#,
        )
        .unwrap();
    write_fits(
        &f._dir.path().join("newest.fits"),
        &[
            "XBINNING=                    2",
            "XPIXSZ  =                 7.52",
            "FOCALLEN=                250.0",
            "FOCRATIO=                  4.9",
            "SITELAT =              34.2000",
            "SITELONG=            -118.3000",
            "SITEELEV=                400.0",
        ],
    );
    let (status, reviewed) = call(
        &f.app,
        "POST",
        RIG_PREVIEW,
        json!({"catalog_id":f.catalog}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{reviewed}");
    let (status, applied) = call(
        &f.app,
        "POST",
        RIG_APPLY,
        json!({"plan":{"catalog_id":f.catalog},"preview_digest":reviewed["data"]["preview_digest"]}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{applied}");
    let rig = Uuid::parse_str(applied["data"]["binding"]["rig"]["id"].as_str().unwrap()).unwrap();
    (f, rig)
}

#[tokio::test]
async fn header_defaults_are_offered_and_edits_use_compare_and_set() {
    let (f, rig) = bound_fixture().await;
    let (status, first) = call(&f.app, "GET", PROFILE, Value::Null, None).await;
    assert_eq!(status, StatusCode::OK, "{first}");
    let data = &first["data"];
    assert_eq!(data["rig"]["id"], rig.to_string());
    assert_eq!(data["profile"]["revision"], 0);
    assert_eq!(data["profile"]["optics"], Value::Null);
    assert_eq!(data["field_of_view"], Value::Null);
    let optics = &data["defaults"]["optics"];
    assert_eq!(optics["source"]["file_name"], "newest.fits");
    assert_eq!(optics["value"]["sensor_width_px"], 6248);
    assert_eq!(optics["value"]["sensor_height_px"], 4176);
    assert_eq!(optics["value"]["pixel_size_um"], 3.76);
    assert_eq!(optics["value"]["focal_length_mm"], 250.0);
    assert!((optics["value"]["aperture_mm"].as_f64().unwrap() - 51.02).abs() < 0.01);
    assert_eq!(data["defaults"]["site"]["value"]["latitude_degrees"], 34.2);
    assert!(
        (data["defaults"]["field_of_view"]["width_degrees"]
            .as_f64()
            .unwrap()
            - 5.384)
            .abs()
            < 0.01
    );

    let edit = |revision: u64| {
        json!({
            "expected_revision": revision,
            "optics": {"value": optics["value"], "source": optics["source"]},
            "site": {"value": data["defaults"]["site"]["value"], "source": {"kind":"manual"}},
            "horizon": null,
            "sky_quality": {"value": {"bortle_class": 6, "sqm_mag_per_arcsec2": null}, "source": {"kind":"manual"}},
            "limits": {"value": {"minimum_altitude_degrees": 25.0, "maximum_altitude_degrees": 88.0,
                "meridian_exclusion": {"before_ms": 600000, "after_ms": 0}}, "source": {"kind":"manual"}},
        })
    };
    let (status, saved) = call(&f.app, "PUT", PROFILE, edit(0), None).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["data"]["profile"]["revision"], 1);
    assert_eq!(
        saved["data"]["profile"]["sky_quality"]["value"]["bortle_class"],
        6
    );
    assert!(
        (saved["data"]["field_of_view"]["pixel_scale_arcsec"]
            .as_f64()
            .unwrap()
            - 3.102)
            .abs()
            < 0.01
    );
    let first_stamp = saved["data"]["profile"]["optics"]["reported_at_ms"].clone();
    // A stale revision loses; the same body under the current one keeps its stamps.
    assert_eq!(
        call(&f.app, "PUT", PROFILE, edit(0), None).await.0,
        StatusCode::CONFLICT
    );
    let (status, again) = call(&f.app, "PUT", PROFILE, edit(1), None).await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again["data"]["profile"]["revision"], 1);
    assert_eq!(
        again["data"]["profile"]["optics"]["reported_at_ms"],
        first_stamp
    );
    // Operators cannot claim the plugin as a source or type a configuration.
    let mut claimed = edit(1);
    claimed["limits"]["source"] = json!({"kind":"plugin"});
    assert_eq!(
        call(&f.app, "PUT", PROFILE, claimed, None).await.0,
        StatusCode::BAD_REQUEST
    );
    let mut extra = edit(1);
    extra["configuration"] = json!({});
    assert_eq!(
        call(&f.app, "PUT", PROFILE, extra, None).await.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let (status, read) = call(&f.app, "GET", PROFILE, Value::Null, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(read["data"]["profile"]["revision"], 1);
}

#[tokio::test]
async fn the_plugin_reports_equipment_only_for_its_own_bound_rig() {
    let (f, rig) = bound_fixture().await;
    let instance = f.state.director.as_ref().unwrap().instance_id;
    let program: Value = serde_json::from_str(include_str!(
        "../../../../../crates/director-core/tests/fixtures/execution-program.json"
    ))
    .unwrap();
    let mut configuration = program["configuration"].clone();
    configuration["rig_id"] = json!(rig.to_string());
    let report = |instance: Uuid, catalog: Uuid| {
        json!({
            "coordinator_instance_id": instance,
            "catalog_id": catalog,
            "configuration": configuration,
            "optics": {"sensor_width_px": 4144, "sensor_height_px": 2822, "pixel_size_um": 4.63,
                "focal_length_mm": 2350.0, "aperture_mm": 235.0, "rotation": {"mode":"rotator"}},
            "site": {"latitude_degrees": 34.2, "longitude_degrees": -118.3, "elevation_meters": 400.0},
            "horizon": {"mode":"custom","points":[{"azimuth_degrees":0.0,"altitude_degrees":15.0},{"azimuth_degrees":360.0,"altitude_degrees":15.0}]},
            "limits": null,
            "reported_at_ms": 1_700_000_000_000u64,
        })
    };
    let path = format!("/rigs/{rig}/equipment");
    assert_eq!(
        call(
            &f.app,
            "PUT",
            &path,
            report(Uuid::new_v4(), f.catalog),
            None
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(&f.app, "PUT", &path, report(instance, Uuid::new_v4()), None)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    let other = format!("/rigs/{}/equipment", Uuid::new_v4());
    assert_eq!(
        call(&f.app, "PUT", &other, report(instance, f.catalog), None)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    for expected in [1, 1] {
        let (status, saved) = call(&f.app, "PUT", &path, report(instance, f.catalog), None).await;
        assert_eq!(status, StatusCode::OK, "{saved}");
        let profile = &saved["data"]["profile"];
        assert_eq!(profile["revision"], expected);
        assert_eq!(profile["configuration"]["source"], json!({"kind":"plugin"}));
        assert_eq!(profile["configuration"]["value"]["rig_id"], rig.to_string());
        assert_eq!(
            profile["optics"]["value"]["rotation"],
            json!({"mode":"rotator"})
        );
        assert_eq!(profile["horizon"]["source"], json!({"kind":"plugin"}));
        assert_eq!(profile["limits"]["source"], json!({"kind":"manual"}));
        assert!(
            (saved["data"]["field_of_view"]["pixel_scale_arcsec"]
                .as_f64()
                .unwrap()
                - 0.406)
                .abs()
                < 0.01
        );
    }
    // An operator edit after the report keeps the plugin's configuration.
    let (_, read) = call(&f.app, "GET", PROFILE, Value::Null, None).await;
    let edit = json!({
        "expected_revision": 1,
        "optics": {"value": read["data"]["profile"]["optics"]["value"], "source": {"kind":"manual"}},
        "site": null, "horizon": null, "sky_quality": null,
        "limits": {"value": read["data"]["profile"]["limits"]["value"], "source": {"kind":"manual"}},
    });
    let (status, saved) = call(&f.app, "PUT", PROFILE, edit, None).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["data"]["profile"]["revision"], 2);
    assert_eq!(saved["data"]["profile"]["site"], Value::Null);
    assert_eq!(
        saved["data"]["profile"]["configuration"]["source"],
        json!({"kind":"plugin"})
    );
}

#[tokio::test]
async fn an_unbound_database_has_no_profile() {
    let f = Fixture::new();
    assert_eq!(
        call(&f.app, "GET", PROFILE, Value::Null, None).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        call(
            &f.app,
            "GET",
            "/catalogs/missing/rig/profile",
            Value::Null,
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
}

/// The profile form is a read: it answers beside a write in progress, and
/// its header search holds no store gate.
#[tokio::test]
async fn the_rig_profile_reads_while_a_write_holds_the_store() {
    let (f, rig) = bound_fixture().await;
    let service = f.state.director.clone().unwrap();
    let (started, start) = tokio::sync::oneshot::channel();
    let (release, wait) = std::sync::mpsc::channel::<()>();
    let write = tokio::spawn(service.clone().run(move |store| {
        started.send(()).unwrap();
        wait.recv().unwrap();
        store.create_project(Uuid::new_v4(), "M33")
    }));
    start.await.unwrap();
    let (status, profile) = call(&f.app, "GET", PROFILE, Value::Null, None).await;
    assert_eq!(status, StatusCode::OK, "{profile}");
    assert_eq!(profile["data"]["rig"]["id"], rig.to_string());
    assert_eq!(
        profile["data"]["defaults"]["optics"]["source"]["file_name"],
        "newest.fits"
    );
    release.send(()).unwrap();
    write.await.unwrap().unwrap();
}
