use super::*;
use crate::commands::sync::ChangedAcquiredImage;
use crate::db_registry::DbEntry;
use crate::server::preview_queue::GenerationState;
use rusqlite::Connection;
use std::io::Write;

const IMAGE_ID: i32 = 700;
const PROJECT_ID: i32 = 7;
const TARGET_ID: i32 = 70;
const CAPTURE_TIME: i64 = 1_784_869_200;

fn write_header(path: &FsPath) {
    let mut header = Vec::new();
    for card in [
        "SIMPLE  =                    T",
        "BITPIX  =                    8",
        "NAXIS   =                    0",
        "FILTER  = 'Ha'",
        "DATE-OBS= '2026-07-24T05:00:00'",
        "END",
    ] {
        let mut padded = format!("{card:<80}").into_bytes();
        padded.truncate(80);
        header.extend(padded);
    }
    header.resize(2880, b' ');
    std::fs::File::create(path)
        .unwrap()
        .write_all(&header)
        .unwrap();
}

fn setup() -> (
    tempfile::TempDir,
    Arc<AppState>,
    Arc<DatabaseContext>,
    crate::models::AcquiredImage,
    PathBuf,
) {
    let temp = tempfile::tempdir().unwrap();
    let database_path = temp.path().join("scheduler.sqlite");
    crate::ts_schema::create_fresh_db(&database_path).unwrap();
    let image_root = temp.path().join("images");
    std::fs::create_dir_all(&image_root).unwrap();
    let source = image_root.join("arrived.fits");
    write_header(&source);
    let metadata = serde_json::json!({
        "FileName": source.to_string_lossy(),
        "ExposureStartTime": "2026-07-24T05:00:00Z",
    });
    let connection = Connection::open(&database_path).unwrap();
    connection
        .execute_batch(&format!(
            "INSERT INTO project (Id, profileId, name) VALUES ({PROJECT_ID}, 'default', 'Project');
             INSERT INTO target (Id, name, active, epochcode, projectid)
                VALUES ({TARGET_ID}, 'Target', 1, 0, {PROJECT_ID});"
        ))
        .unwrap();
    connection
        .execute(
            "INSERT INTO acquiredimage
                (Id, projectId, targetId, acquireddate, filtername, gradingStatus, metadata, profileId)
             VALUES (?1, ?2, ?3, ?4, 'Ha', 0, ?5, 'default')",
            rusqlite::params![IMAGE_ID, PROJECT_ID, TARGET_ID, CAPTURE_TIME, metadata.to_string()],
        )
        .unwrap();
    drop(connection);

    let state = Arc::new(
        AppState::from_databases(
            vec![DbEntry {
                ts_sync: None,
                id: "test".into(),
                name: "Test".into(),
                db_path: database_path.to_string_lossy().into_owned(),
                image_dirs: vec![image_root.to_string_lossy().into_owned()],
                reject_archive: None,
                remote_image_upload: None,
                export_dir: None,
                process_dir: None,
                autoimport: None,
                scan_rules: None,
                analyze_new_frames: false,
                pair_calibrated_copies: true,
                scan_calibrated_copies: false,
                processing_overrides: None,
            }],
            temp.path().join("cache").to_string_lossy().into_owned(),
            crate::cli::PregenerationConfig::default(),
        )
        .unwrap(),
    );
    let ctx = state.get_database("test").unwrap();
    let image = {
        let connection = ctx.db();
        let connection = connection.lock().unwrap();
        Database::new(&connection)
            .get_images_by_ids(&[IMAGE_ID])
            .unwrap()
            .pop()
            .unwrap()
    };
    (temp, state, ctx, image, source)
}

#[tokio::test]
async fn validated_ready_status_makes_immediate_preview_and_annotated_gets_cache_hits() {
    let (_temp, state, ctx, image, source) = setup();
    let images = HashMap::from([(IMAGE_ID, image.clone())]);
    let targets = HashMap::from([(TARGET_ID, "Target".to_string())]);

    for kind in ["preview", "annotated"] {
        ctx.register_remote_image_arrivals(&[ChangedAcquiredImage {
            id: i64::from(IMAGE_ID),
            project_id: i64::from(PROJECT_ID),
            target_id: i64::from(TARGET_ID),
            acquired_date: Some(CAPTURE_TIME),
        }]);
        let cache_path = if kind == "annotated" {
            artifact_cache_path(
                &ctx,
                "annotated",
                &annotated_cache_key(&image, "arrived.fits", "screen", 1000, None),
                state.preview_encoding(),
            )
            .unwrap()
        } else {
            artifact_cache_path(
                &ctx,
                "previews",
                &preview_cache_key(
                    &image,
                    "arrived.fits",
                    "screen",
                    true,
                    0.2,
                    -2.8,
                    state.preview_color_default(),
                    None,
                ),
                state.preview_encoding(),
            )
            .unwrap()
        };
        std::fs::write(&cache_path, b"cached image").unwrap();
        state
            .preview_queue
            .remember_completed_source_for_test(cache_path, &source);
        assert!(!ctx.delayed_source_is_stable(IMAGE_ID, &source));

        let delayed_probe = ctx
            .delayed_file_probes_for_batch(&[IMAGE_ID], MAX_DELAYED_FILE_PROBES_PER_STATUS_BATCH)
            [&IMAGE_ID];
        let mut canonical_roots = None;
        let mapped_sources = crate::server::remote_upload::MappedLightSources::default();
        let lookup = GenerationStatusLookup {
            images: &images,
            target_names: &targets,
            mapped_sources: &mapped_sources,
        };
        let status = status_for_item(
            &state,
            &ctx,
            &GenStatusItem {
                image_id: IMAGE_ID,
                copy: None,
                kind: Some(kind.to_string()),
                size: Some("screen".to_string()),
                stretch: None,
                midtone: None,
                shadow: None,
                max_stars: None,
                color: None,
            },
            &lookup,
            delayed_probe,
            &mut canonical_roots,
        );
        assert_eq!(status.state, GenerationState::Ready);
        assert!(canonical_roots.is_some());
        assert_eq!(
            ctx.delayed_file_probe(IMAGE_ID),
            crate::server::database_context::DelayedFileProbe::NotPending
        );

        let options = PreviewOptions {
            size: Some("screen".into()),
            copy: None,
            stretch: None,
            midtone: None,
            shadow: None,
            max_stars: None,
            color: None,
        };
        let response = if kind == "annotated" {
            get_annotated_image(
                State(Arc::clone(&state)),
                DbContext(Arc::clone(&ctx)),
                Path(("test".into(), IMAGE_ID)),
                Query(options),
                HeaderMap::new(),
            )
            .await
            .unwrap()
        } else {
            get_image_preview(
                State(Arc::clone(&state)),
                DbContext(Arc::clone(&ctx)),
                Path(("test".into(), IMAGE_ID)),
                Query(options),
                HeaderMap::new(),
            )
            .await
            .unwrap()
        };
        assert_eq!(response.status(), StatusCode::OK, "{kind}");
    }
}

#[test]
fn delayed_duplicate_descriptors_consume_one_probe_observation() {
    use crate::server::database_context::DelayedFileProbe;

    let probes = HashMap::from([(IMAGE_ID, DelayedFileProbe::Probe)]);
    let mut consumed = std::collections::HashSet::new();
    assert_eq!(
        consume_delayed_batch_probe(&probes, IMAGE_ID, &mut consumed),
        DelayedFileProbe::Probe
    );
    assert_eq!(
        consume_delayed_batch_probe(&probes, IMAGE_ID, &mut consumed),
        DelayedFileProbe::Wait
    );
}
