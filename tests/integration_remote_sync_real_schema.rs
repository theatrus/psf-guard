//! Remote sync against the real vendored Target Scheduler schema.
//!
//! Every other sync test builds a seven-table shorthand of the scheduler
//! database. That shorthand cannot catch anything that depends on the real
//! shape: columns added by later migrations, NOT NULL columns a client has
//! never heard of, or foreign keys between the tables a bundle materializes.
//! These tests run the same protocol against `ts_schema`, so they move with
//! the schema the plugin actually talks to.

use std::sync::Arc;

use axum::{
    body::Body,
    extract::DefaultBodyLimit,
    http::{Request, StatusCode},
    routing::{get, post},
    Router,
};
use http_body_util::BodyExt;
use psf_guard::{
    cli::PregenerationConfig,
    db_registry::{DbEntry, RemoteImageUploadConfig},
    server::{remote_sync, state::AppState},
};
use rusqlite::Connection;
use serde_json::{json, Value};
use tempfile::tempdir;
use tower::ServiceExt;

const SOURCE_TOKEN: &str = "source-remote-api-key-1234567890";
const DESTINATION_TOKEN: &str = "destination-remote-api-key-123456";

/// One observed target, written through named columns so the row survives
/// migrations that add or reorder fields.
const TELESCOPE_ROWS: &str = "
    INSERT INTO project (Id,profileId,name,description,state,priority,isMosaic,flatsHandling,guid)
        VALUES (1,'profile','M42','telescope settings',2,8,0,0,'project-guid');
    INSERT INTO target (Id,name,active,ra,dec,epochcode,projectid,guid)
        VALUES (1,'M42',1,5.5,-5.4,0,1,'target-guid');
    INSERT INTO exposuretemplate (Id,profileId,name,filtername,gain,guid)
        VALUES (1,'profile','Ha 300','Ha',120,'template-guid');
    INSERT INTO exposureplan (Id,profileId,exposure,desired,acquired,accepted,targetid,exposureTemplateId,enabled,guid)
        VALUES (1,'profile',300,40,18,16,1,1,1,'plan-guid');
    INSERT INTO ruleweight (Id,name,weight,projectid) VALUES (1,'Priority',2.5,1);
    INSERT INTO acquiredimage (Id,projectId,targetId,acquireddate,filtername,gradingStatus,metadata,profileId,exposureId,guid)
        VALUES (1,1,1,1750000000,'Ha',1,'{\"FileName\":\"one.fits\"}','profile',1,'image-one');
    INSERT INTO acquiredimage (Id,projectId,targetId,acquireddate,filtername,gradingStatus,metadata,rejectreason,profileId,exposureId,guid)
        VALUES (2,1,1,1750000600,'Ha',2,'{\"FileName\":\"two.fits\"}','clouds','profile',1,'image-two');
    INSERT INTO imagedata (Id,tag,imagedata,acquiredimageid,width,height)
        VALUES (1,'thumb',X'89504E470D0A1A0A',1,64,48);";

struct Harness {
    _directory: tempfile::TempDir,
    router: Router,
    destination_path: std::path::PathBuf,
}

impl Harness {
    fn new() -> Self {
        let directory = tempdir().unwrap();
        let source_path = directory.path().join("telescope.sqlite");
        let destination_path = directory.path().join("review.sqlite");
        let image_path = directory.path().join("images");
        std::fs::create_dir(&image_path).unwrap();
        for path in [&source_path, &destination_path] {
            let connection = Connection::open(path).unwrap();
            psf_guard::ts_schema::apply_schema(&connection).unwrap();
        }
        Connection::open(&source_path)
            .unwrap()
            .execute_batch(TELESCOPE_ROWS)
            .unwrap();

        let image_dirs = vec![image_path.to_string_lossy().into_owned()];
        let state = Arc::new(
            AppState::from_databases(
                vec![
                    DbEntry {
                        id: "source".into(),
                        name: "Telescope".into(),
                        db_path: source_path.to_string_lossy().into_owned(),
                        image_dirs: image_dirs.clone(),
                        reject_archive: None,
                        remote_image_upload: Some(config(SOURCE_TOKEN)),
                        export_dir: None,
                    },
                    DbEntry {
                        id: "destination".into(),
                        name: "Review copy".into(),
                        db_path: destination_path.to_string_lossy().into_owned(),
                        image_dirs,
                        reject_archive: None,
                        remote_image_upload: Some(config(DESTINATION_TOKEN)),
                        export_dir: None,
                    },
                ],
                directory
                    .path()
                    .join("cache")
                    .to_string_lossy()
                    .into_owned(),
                PregenerationConfig::default(),
            )
            .unwrap(),
        );
        Self {
            _directory: directory,
            router: Router::new()
                .route(
                    "/api/sync/v1/previews",
                    post(remote_sync::create_preview)
                        .layer(DefaultBodyLimit::max(remote_sync::MAX_SYNC_BODY_BYTES)),
                )
                .route(
                    "/api/sync/v1/previews/{preview_id}/apply",
                    post(remote_sync::apply_preview),
                )
                .route("/api/sync/v1/exports", post(remote_sync::create_export))
                .route(
                    "/api/sync/v1/exports/{export_id}",
                    get(remote_sync::get_export),
                )
                .with_state(state),
            destination_path,
        }
    }

    async fn export(&self, operation: &str) -> Value {
        let (status, exported) = call(
            self.router.clone(),
            "POST",
            "/api/sync/v1/exports",
            SOURCE_TOKEN,
            Some(json!({
                "protocol_version": 1,
                "catalog_id": "source",
                "operation": operation,
                "reviewed_only": false,
                // The round trip asserts thumbnail blobs survive the wire,
                // and thumbnails are opt-in on merge exports.
                "include_thumbnails": true
            })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{exported:#}");
        exported["data"]["bundle"].clone()
    }

    async fn apply(&self, operation: &str, bundle: Value) -> Value {
        let (status, preview) = call(
            self.router.clone(),
            "POST",
            "/api/sync/v1/previews",
            DESTINATION_TOKEN,
            Some(json!({
                "protocol_version": 1,
                "catalog_id": "destination",
                "operation": operation,
                "bundle": bundle
            })),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{preview:#}");
        let preview_id = preview["data"]["preview_id"].as_str().unwrap();
        let (status, applied) = call(
            self.router.clone(),
            "POST",
            &format!("/api/sync/v1/previews/{preview_id}/apply"),
            DESTINATION_TOKEN,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{applied:#}");
        applied
    }

    fn destination(&self) -> Connection {
        Connection::open(&self.destination_path).unwrap()
    }
}

fn config(token: &str) -> RemoteImageUploadConfig {
    let mut config = RemoteImageUploadConfig {
        enabled: false,
        sync_enabled: true,
        ..Default::default()
    };
    config.set_token(token).unwrap();
    config
}

async fn call(
    app: Router,
    method: &str,
    uri: &str,
    token: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header("authorization", format!("Bearer {token}"));
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    let response = app
        .oneshot(
            builder
                .body(body.map_or_else(Body::empty, |value| {
                    Body::from(serde_json::to_vec(&value).unwrap())
                }))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn a_merge_round_trips_through_the_real_scheduler_schema() {
    let harness = Harness::new();

    let bundle = harness.export("merge").await;
    assert_eq!(
        bundle["source"]["schema_version"],
        psf_guard::ts_schema::TS_SCHEMA_VERSION
    );
    harness.apply("merge", bundle).await;

    let destination = harness.destination();
    let (name, mosaic): (String, i64) = destination
        .query_row("SELECT name, isMosaic FROM project", [], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap();
    assert_eq!(name, "M42");
    assert_eq!(mosaic, 0);
    let images: i64 = destination
        .query_row("SELECT count(*) FROM acquiredimage", [], |row| row.get(0))
        .unwrap();
    assert_eq!(images, 2);
    let blob: Vec<u8> = destination
        .query_row("SELECT imagedata FROM imagedata", [], |row| row.get(0))
        .unwrap();
    assert_eq!(blob, vec![0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
}

#[tokio::test]
async fn a_client_that_predates_a_column_still_merges() {
    // A plugin built against an older Target Scheduler sends rows without the
    // columns later migrations added — including NOT NULL ones. Those must
    // fall back to the destination's own defaults, not fail the insert.
    let harness = Harness::new();
    let mut bundle = harness.export("merge").await;
    drop_columns(&mut bundle, "project", &["isMosaic", "flatsHandling"]);

    harness.apply("merge", bundle).await;

    let destination = harness.destination();
    let (name, mosaic, flats): (String, Option<i64>, Option<i64>) = destination
        .query_row(
            "SELECT name, isMosaic, flatsHandling FROM project",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(name, "M42", "the row the client did send still lands");
    assert_eq!(mosaic, Some(0), "the schema default fills the gap");
    assert_eq!(flats, Some(0));
}

#[tokio::test]
async fn a_grade_push_reads_the_real_schemas_join() {
    // The join a grade push depends on runs against the real table shapes,
    // not the shorthand the other tests declare.
    let harness = Harness::new();
    harness
        .destination()
        .execute_batch(
            "INSERT INTO project (Id,profileId,name,isMosaic,flatsHandling,guid)
                VALUES (7,'profile','M42',0,0,'project-guid');
             INSERT INTO target (Id,name,active,epochcode,projectid,guid)
                VALUES (7,'M42',1,0,7,'target-guid');
             INSERT INTO acquiredimage (Id,projectId,targetId,acquireddate,filtername,gradingStatus,metadata,profileId,guid)
                VALUES (7,7,7,1750000600,'Ha',0,'{}','profile','image-two');",
        )
        .unwrap();

    let bundle = harness.export("push_grades").await;
    let applied = harness.apply("push_grades", bundle).await;
    assert_eq!(applied["data"]["summary"]["grades_changed"], 1);

    let (grade, reason): (i64, Option<String>) = harness
        .destination()
        .query_row(
            "SELECT gradingStatus, rejectreason FROM acquiredimage WHERE guid = 'image-two'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(grade, 2);
    assert_eq!(reason.as_deref(), Some("clouds"));
}

/// Remove named columns from one bundle table, values included, the way a
/// client that has never heard of them would have built it.
fn drop_columns(bundle: &mut Value, table: &str, unknown: &[&str]) {
    let table = bundle["tables"][table].as_object_mut().unwrap();
    let dropped: Vec<usize> = table["columns"]
        .as_array()
        .unwrap()
        .iter()
        .enumerate()
        .filter(|(_, column)| unknown.contains(&column["name"].as_str().unwrap()))
        .map(|(index, _)| index)
        .collect();
    assert_eq!(dropped.len(), unknown.len(), "column names must all exist");
    let keep = |values: &Vec<Value>| -> Vec<Value> {
        values
            .iter()
            .enumerate()
            .filter(|(index, _)| !dropped.contains(index))
            .map(|(_, value)| value.clone())
            .collect()
    };
    let columns = keep(table["columns"].as_array().unwrap());
    let rows: Vec<Value> = table["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| json!({ "values": keep(row["values"].as_array().unwrap()) }))
        .collect();
    table.insert("columns".into(), Value::Array(columns));
    table.insert("rows".into(), Value::Array(rows));
}
