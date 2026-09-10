use super::*;
use crate::calibration::CalibrationSessionDetail;

fn label(kind: CalibrationKind, marker: char) -> String {
    format!("{}-{}.fits", kind.as_str(), marker.to_string().repeat(64))
}

fn signature(bias: Option<&str>, flat: Option<&str>) -> String {
    format!(
        "bias={};dark=none;dark_flat=none;flat={}",
        bias.unwrap_or("none"),
        flat.unwrap_or("none"),
    )
}

#[test]
fn signatures_require_all_kinds_and_reject_invalid_or_duplicate_references() {
    let flat = label(CalibrationKind::Flat, 'a');
    assert_eq!(
        signature_masters(&format!(
            "{};pedestal=estimated-125.0",
            signature(None, Some(&flat))
        ))
        .unwrap(),
        vec![(CalibrationKind::Flat, flat.clone())],
    );
    for invalid in [
        format!("flat={flat}"),
        format!("{};flat={flat}", signature(None, Some(&flat))),
        format!("bias={flat};dark=none;dark_flat=none;flat=none"),
        signature(None, Some("../flat-secret.fits")),
        signature(None, Some("C:\\secret.fits")),
        format!("{};other=none", signature(None, None)),
    ] {
        assert!(signature_masters(&invalid).is_err(), "{invalid}");
    }
    assert!(signature_masters(&signature(None, None))
        .unwrap()
        .is_empty());
}

#[test]
fn sessions_deduplicate_files_without_losing_channel_or_pedestal_usage() {
    let bias = label(CalibrationKind::Bias, 'b');
    let first_flat = label(CalibrationKind::Flat, 'a');
    let second_flat = label(CalibrationKind::Flat, 'c');
    let mut calibration = AppliedCalibration {
        sessions: 2,
        session_details: vec![
            CalibrationSessionDetail {
                fingerprint: "first".into(),
                masters_signature: signature(Some(&bias), Some(&first_flat)),
                estimated_pedestal_adu: Some(120.0),
                lights: 11,
            },
            CalibrationSessionDetail {
                fingerprint: "second".into(),
                masters_signature: signature(Some(&bias), Some(&second_flat)),
                estimated_pedestal_adu: None,
                lights: 7,
            },
        ],
        ..Default::default()
    };
    let mut masters = BTreeMap::new();
    let mut notes = Vec::new();
    add_calibration(&calibration, "Red", 18, &mut masters, &mut notes);
    calibration.session_details.truncate(1);
    add_calibration(&calibration, "Green", 11, &mut masters, &mut notes);
    assert!(notes.is_empty());
    assert_eq!(masters.len(), 3);
    let usages = &masters[&(CalibrationKind::Bias, bias)].usages;
    assert_eq!(usages.len(), 3);
    assert_eq!((usages[0].session, usages[0].lights), (1, 11));
    assert_eq!(usages[0].estimated_pedestal_adu, Some(120.0));
    assert_eq!((usages[1].session, usages[1].lights), (2, 7));
    assert_eq!(usages[2].channel, "Green");
}

#[test]
fn legacy_labels_are_used_only_without_recorded_sessions() {
    let flat = label(CalibrationKind::Flat, 'a');
    let mut calibration = AppliedCalibration {
        flat_master: Some(flat.clone()),
        estimated_pedestal_adu: Some(80.0),
        ..Default::default()
    };
    let mut masters = BTreeMap::new();
    let mut notes = Vec::new();
    add_calibration(&calibration, "Ha", 9, &mut masters, &mut notes);
    assert_eq!(masters[&(CalibrationKind::Flat, flat)].usages[0].lights, 9);
    assert!(notes.is_empty());
    calibration.sessions = 2;
    masters.clear();
    add_calibration(&calibration, "Ha", 9, &mut masters, &mut notes);
    assert!(masters.is_empty());
    assert!(notes[0].contains("did not retain per-session"));
    calibration.session_details.push(CalibrationSessionDetail {
        fingerprint: "invalid".into(),
        masters_signature: "flat=../../private.fits".into(),
        estimated_pedestal_adu: None,
        lights: 9,
    });
    notes.clear();
    add_calibration(&calibration, "Ha", 9, &mut masters, &mut notes);
    assert!(
        masters.is_empty(),
        "invalid sessions must not fall back to unrelated labels"
    );
    assert!(notes[0].contains("references are invalid"));
}

#[test]
fn display_options_are_bounded_finite_and_canonicalized() {
    assert_eq!(
        DisplayOptions::default().normalized().unwrap(),
        ("screen", 0.2, -2.8)
    );
    for options in [
        DisplayOptions {
            size: Some("thumbnail".into()),
            ..Default::default()
        },
        DisplayOptions {
            midtone: Some(f64::NAN),
            ..Default::default()
        },
        DisplayOptions {
            midtone: Some(1.0),
            ..Default::default()
        },
        DisplayOptions {
            shadow: Some(f64::NEG_INFINITY),
            ..Default::default()
        },
        DisplayOptions {
            shadow: Some(-10.01),
            ..Default::default()
        },
        DisplayOptions {
            shadow: Some(0.01),
            ..Default::default()
        },
    ] {
        assert!(matches!(options.normalized(), Err(AppError::BadRequest(_))));
    }
    assert_eq!(
        DisplayOptions {
            size: Some("original".into()),
            midtone: Some(0.2001),
            shadow: Some(-2.801)
        }
        .normalized()
        .unwrap(),
        ("original", 0.2, -2.8),
    );
}

struct Fixture {
    _directory: tempfile::TempDir,
    state: Arc<AppState>,
    ctx: Arc<DatabaseContext>,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let mut ctx =
            DatabaseContext::new_for_test(rusqlite::Connection::open_in_memory().unwrap());
        ctx.cache_dir_path = directory.path().join("cache").join("test");
        ctx.cache_dir = ctx.cache_dir_path.to_string_lossy().into_owned();
        std::fs::create_dir_all(ctx.cache_dir_path.join("calibration-masters")).unwrap();
        Self {
            _directory: directory,
            state: Arc::new(AppState::new_for_test(
                rusqlite::Connection::open_in_memory().unwrap(),
            )),
            ctx: Arc::new(ctx),
        }
    }

    fn stack(&self, calibration: AppliedCalibration) -> (StackPreviewJob, MasterSource) {
        let group: StackGroupStatus = serde_json::from_value(serde_json::json!({
            "index": 0, "target_id": 1, "target_name": "Target", "filter_name": "Ha",
            "state": "ready", "phase": "ready", "total_candidates": 3, "eligible_frames": 3,
            "quality_excluded": 0, "missing_files": 0, "processed_frames": 3,
            "accepted_frames": 3, "rejected_frames": 0, "reused_frames": 0,
            "output_channels": 1, "total_exposure_seconds": 300.0,
            "calibration": calibration, "input_images": [], "frames": [],
        }))
        .unwrap();
        let job = StackPreviewJob {
            schema_version: 2,
            job_id: "a".repeat(64),
            database_id: self.ctx.id.clone(),
            project_id: 7,
            state: StackJobState::Completed,
            accepted_only: false,
            created_unix_seconds: 100,
            artifact_revision: "revision-old".into(),
            cache_version: super::super::STACK_PREVIEW_CACHE_VERSION,
            stacking_version: super::super::SEIZA_STACKING_VERSION.into(),
            order: super::super::snr::StackFrameOrder::Capture,
            scoring: super::super::StackScoringSettings::default(),
            groups: vec![group],
            error: None,
        };
        let source = MasterSource {
            kind: SourceKind::Mono,
            job_id: job.job_id.clone(),
            group_index: Some(0),
            artifact_revision: job.artifact_revision.clone(),
        };
        assert!(self.state.stack_previews.insert(job.clone()));
        (job, source)
    }

    fn master(&self, kind: CalibrationKind, marker: char) -> RecordedMaster {
        let label = label(kind, marker);
        RecordedMaster {
            id: master_id(kind, &label),
            kind,
            label,
            usages: Vec::new(),
        }
    }

    fn write_master(&self, master: &RecordedMaster) -> PathBuf {
        let path = self
            .ctx
            .cache_dir_path
            .join("calibration-masters")
            .join(&master.label);
        let data = (0..64 * 64)
            .map(|index| 1.0 + (index % 64) as f32 * 0.001)
            .collect();
        let image = seiza_stacking::LinearImage::new(64, 64, 1, data).unwrap();
        seiza_stacking::write_processed_image_fits_f32(&path, &image, &[], &[]).unwrap();
        path
    }
}

#[test]
fn historical_catalog_does_not_need_current_catalog_rows_or_a_database_lock() {
    let fixture = Fixture::new();
    let master = fixture.master(CalibrationKind::Flat, 'b');
    fixture.write_master(&master);
    let (_, source) = fixture.stack(AppliedCalibration {
        flat_master: Some(master.label.clone()),
        ..Default::default()
    });
    let catalog = catalog(&fixture.state, &fixture.ctx, &source).unwrap();
    assert_eq!(catalog.masters.len(), 1);
    let entry = &catalog.masters[0];
    assert!(entry.available);
    assert!(entry.source_count.is_none());
    assert!(entry
        .preview_url
        .as_deref()
        .unwrap()
        .contains("revision=revision-old"));
    assert!(entry.fits_url.as_deref().unwrap().contains(&master.id));
    let db = fixture.ctx.db();
    let _lock = db.lock().unwrap();
    let catalog = super::catalog(&fixture.state, &fixture.ctx, &source).unwrap();
    assert!(
        catalog.masters[0].available,
        "optional metadata must not block inspection"
    );
}

#[test]
fn optional_provenance_is_reported_without_recreating_forgotten_catalog_rows() {
    let fixture = Fixture::new();
    let master = fixture.master(CalibrationKind::Flat, 'b');
    let path = fixture.write_master(&master);
    let (_, source) = fixture.stack(AppliedCalibration {
        flat_master: Some(master.label),
        ..Default::default()
    });
    let db = fixture.ctx.db();
    {
        let conn = db.lock().unwrap();
        conn.execute_batch("CREATE TABLE psf_guard_calibration_master (cache_path TEXT, source_count INTEGER, statistics_json TEXT)").unwrap();
        conn.execute(
            "INSERT INTO psf_guard_calibration_master VALUES (?1, 8, ?2)",
            rusqlite::params![path.to_string_lossy(), serde_json::json!({
                "rejection_method": "MEDIAN_MAD", "masked_samples": 120,
                "rejected_samples": 9, "flat_star_masking": { "minimum_clean_samples": 2, "maximum_clean_samples": 8 }
            }).to_string()],
        ).unwrap();
    }
    let before = catalog(&fixture.state, &fixture.ctx, &source).unwrap();
    let entry = &before.masters[0];
    assert_eq!(entry.source_count, Some(8));
    assert_eq!(entry.rejection_method.as_deref(), Some("MEDIAN_MAD"));
    assert_eq!(entry.masked_samples, Some(120));
    assert_eq!(entry.rejected_samples, Some(9));
    assert_eq!(
        (entry.minimum_clean_samples, entry.maximum_clean_samples),
        (Some(2), Some(8))
    );
    db.lock()
        .unwrap()
        .execute("DELETE FROM psf_guard_calibration_master", [])
        .unwrap();
    let after = catalog(&fixture.state, &fixture.ctx, &source).unwrap();
    assert!(after.masters[0].available);
    assert_eq!(after.masters[0].source_count, None);
    let count: i64 = db
        .lock()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM psf_guard_calibration_master",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0, "inspection must never rebuild or record masters");
}

#[test]
fn numeric_display_query_fields_decode_from_real_url_parameters() {
    let uri = "/masters/preview?revision=revision-old&size=original&midtone=0.325&shadow=-3.75"
        .parse()
        .unwrap();
    let Query(query) = Query::<MasterQuery>::try_from_uri(&uri).unwrap();
    assert_eq!(query.revision, "revision-old");
    assert_eq!(
        query.display().normalized().unwrap(),
        ("original", 0.325, -3.75)
    );
}

#[test]
fn missing_files_stay_unavailable_and_unrecorded_files_are_not_downloadable() {
    let fixture = Fixture::new();
    let missing = fixture.master(CalibrationKind::Flat, 'b');
    let unrecorded = fixture.master(CalibrationKind::Flat, 'c');
    fixture.write_master(&unrecorded);
    let (_, source) = fixture.stack(AppliedCalibration {
        flat_master: Some(missing.label.clone()),
        ..Default::default()
    });
    let catalog = catalog(&fixture.state, &fixture.ctx, &source).unwrap();
    assert!(!catalog.masters[0].available);
    assert!(catalog.masters[0].unavailable_reason.is_some());
    assert!(catalog.masters[0].preview_url.is_none());
    assert!(catalog.masters[0].fits_url.is_none());
    assert!(matches!(
        resolve_master(&fixture.state, &fixture.ctx, &source, &missing.id),
        Err(AppError::NotFound)
    ));
    assert!(matches!(
        resolve_master(&fixture.state, &fixture.ctx, &source, &unrecorded.id),
        Err(AppError::NotFound)
    ));
    assert!(!fixture
        .ctx
        .cache_dir_path
        .join("calibration-masters")
        .join(&missing.label)
        .exists());
}

#[test]
fn invalid_paths_wrong_kind_and_other_database_are_refused() {
    let fixture = Fixture::new();
    let mut master = fixture.master(CalibrationKind::Flat, 'b');
    fixture.write_master(&master);
    for invalid in [
        "../private.fits".to_string(),
        "C:\\private.fits".into(),
        label(CalibrationKind::Bias, 'c'),
    ] {
        master.label = invalid;
        assert!(matches!(
            master_path(&fixture.ctx, &master),
            Err(AppError::BadRequest(_))
        ));
    }
    let (_, source) = fixture.stack(AppliedCalibration::default());
    let mut other = DatabaseContext::new_for_test(rusqlite::Connection::open_in_memory().unwrap());
    other.id = "other-db".into();
    other.cache_dir_path = fixture.ctx.cache_dir_path.clone();
    assert!(matches!(
        load_mono_group(&fixture.state, &other, &source),
        Err(AppError::NotFound)
    ));
    let mut invalid = source.clone();
    invalid.job_id = "../private".into();
    assert!(matches!(
        validate_source(&invalid),
        Err(AppError::BadRequest(_))
    ));
    invalid = source.clone();
    invalid.artifact_revision = "revision&size=original".into();
    assert!(matches!(
        validate_source(&invalid),
        Err(AppError::BadRequest(_))
    ));
    invalid = source;
    invalid.kind = SourceKind::Color;
    assert!(matches!(
        validate_source(&invalid),
        Err(AppError::BadRequest(_))
    ));
}

#[test]
fn a_forced_rebuild_keeps_the_retained_completed_revision_inspectable() {
    let fixture = Fixture::new();
    let old_master = fixture.master(CalibrationKind::Flat, 'b');
    let (mut job, source) = fixture.stack(AppliedCalibration {
        flat_master: Some(old_master.label.clone()),
        ..Default::default()
    });
    super::super::persist_latest_groups(&fixture.ctx.cache_dir_path, &job).unwrap();
    job.artifact_revision = "revision-new".into();
    job.state = StackJobState::Running;
    job.groups[0].state = StackGroupState::Running;
    job.groups[0].calibration.flat_master = Some(label(CalibrationKind::Flat, 'c'));
    assert!(fixture.state.stack_previews.insert(job.clone()));
    super::super::persist_manifest(&fixture.ctx.cache_dir_path, &job).unwrap();
    let loaded = load_mono_group(&fixture.state, &fixture.ctx, &source).unwrap();
    assert_eq!(
        loaded.calibration.flat_master.as_deref(),
        Some(old_master.label.as_str())
    );
    let mut unknown = source.clone();
    unknown.artifact_revision = "never-built".into();
    assert!(matches!(
        load_mono_group(&fixture.state, &fixture.ctx, &unknown),
        Err(AppError::Conflict(_))
    ));
    job.state = StackJobState::Completed;
    job.groups[0].state = StackGroupState::Ready;
    super::super::persist_latest_groups(&fixture.ctx.cache_dir_path, &job).unwrap();
    assert!(fixture.state.stack_previews.insert(job));
    assert!(matches!(
        load_mono_group(&fixture.state, &fixture.ctx, &source),
        Err(AppError::Conflict(_))
    ));
}

#[test]
fn preview_identity_tracks_source_display_size_and_database_cache() {
    let fixture = Fixture::new();
    let master = fixture.master(CalibrationKind::Flat, 'b');
    let path = fixture.write_master(&master);
    let options = DisplayOptions::default();
    let original = preview_job(&fixture.ctx, &master, path.clone(), &options).unwrap();
    assert_eq!(original.cache_path.extension().unwrap(), "png");
    assert!(matches!(original.kind, GenKind::CalibrationMaster { .. }));
    let equivalent = preview_job(
        &fixture.ctx,
        &master,
        path.clone(),
        &DisplayOptions {
            midtone: Some(0.2001),
            shadow: Some(-2.801),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(equivalent.cache_path, original.cache_path);
    for options in [
        DisplayOptions {
            size: Some("original".into()),
            ..Default::default()
        },
        DisplayOptions {
            midtone: Some(0.3),
            ..Default::default()
        },
        DisplayOptions {
            shadow: Some(-3.0),
            ..Default::default()
        },
    ] {
        assert_ne!(
            preview_job(&fixture.ctx, &master, path.clone(), &options)
                .unwrap()
                .cache_path,
            original.cache_path
        );
    }
    std::fs::write(&path, b"changed source").unwrap();
    let changed = preview_job(&fixture.ctx, &master, path, &DisplayOptions::default()).unwrap();
    assert_ne!(changed.cache_path, original.cache_path);
    let other = Fixture::new();
    let other_path = other.write_master(&master);
    assert_ne!(
        preview_job(&other.ctx, &master, other_path, &DisplayOptions::default())
            .unwrap()
            .cache_path,
        original.cache_path
    );
}

fn color_fixture(fixture: &Fixture, mono: &MasterSource) -> MasterSource {
    let job: color::StackColorJob = serde_json::from_value(serde_json::json!({
        "schema_version": 1, "job_id": "d".repeat(64), "database_id": fixture.ctx.id,
        "project_id": 7, "target_id": 1, "target_name": "Target", "kind": "rgb",
        "palette": null, "label": "RGB", "state": "completed", "phase": "ready",
        "processed_channels": 2, "total_channels": 2, "created_unix_seconds": 100,
        "artifact_revision": "color-old", "cache_version": 1,
        "stacking_version": super::super::SEIZA_STACKING_VERSION,
        "sources": [
            { "role": "red", "filter_name": "R", "job_id": mono.job_id,
              "group_index": 0, "artifact_revision": mono.artifact_revision, "accepted_frames": 3 },
            { "role": "green", "filter_name": "G", "job_id": "e".repeat(64),
              "group_index": 0, "artifact_revision": "missing", "accepted_frames": 4 }
        ],
        "preview_url": "/preview", "fits_url": "/fits", "error": null,
    }))
    .unwrap();
    let source = MasterSource {
        kind: SourceKind::Color,
        job_id: job.job_id.clone(),
        group_index: None,
        artifact_revision: job.artifact_revision.clone(),
    };
    fixture
        .state
        .stack_previews
        .color_jobs
        .lock()
        .unwrap()
        .insert(job.job_id.clone(), job);
    source
}

#[test]
fn color_inspection_keeps_available_recorded_channels_and_reports_missing_sources() {
    let fixture = Fixture::new();
    let master = fixture.master(CalibrationKind::Flat, 'b');
    fixture.write_master(&master);
    let (_, mono) = fixture.stack(AppliedCalibration {
        flat_master: Some(master.label.clone()),
        ..Default::default()
    });
    let source = color_fixture(&fixture, &mono);
    let catalog = catalog(&fixture.state, &fixture.ctx, &source).unwrap();
    assert_eq!(catalog.masters.len(), 1);
    assert!(catalog.masters[0].available);
    assert_eq!(catalog.masters[0].usages[0].channel, "R");
    assert!(catalog
        .notes
        .iter()
        .any(|note| note.contains("G:") && note.contains("no longer available")));
    let mut stale_source = source;
    stale_source.artifact_revision = "another-color-revision".into();
    assert!(matches!(
        load_color_job(&fixture.state, &fixture.ctx, &stale_source),
        Err(AppError::Conflict(_))
    ));
}

#[cfg(any(unix, windows))]
fn create_test_symlink(target: &FsPath, link: &FsPath, directory: bool) -> bool {
    #[cfg(unix)]
    let result = {
        let _ = directory;
        std::os::unix::fs::symlink(target, link)
    };
    #[cfg(windows)]
    let result = if directory {
        std::os::windows::fs::symlink_dir(target, link)
    } else {
        std::os::windows::fs::symlink_file(target, link)
    };
    match result {
        Ok(()) => true,
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            eprintln!("symlink regression unavailable without symlink privilege: {error}");
            false
        }
        Err(error) => panic!("creating test symlink: {error}"),
    }
}

#[cfg(any(unix, windows))]
#[test]
fn master_symlinks_cannot_escape_the_database_cache() {
    let fixture = Fixture::new();
    let master = fixture.master(CalibrationKind::Flat, 'b');
    let outside = fixture._directory.path().join("private.fits");
    std::fs::write(&outside, b"private").unwrap();
    let link = fixture
        .ctx
        .cache_dir_path
        .join("calibration-masters")
        .join(&master.label);
    if !create_test_symlink(&outside, &link, false) {
        return;
    }
    assert!(matches!(
        master_path(&fixture.ctx, &master),
        Err(AppError::NotFound)
    ));
}

#[cfg(any(unix, windows))]
#[test]
fn preview_cache_parent_symlink_is_rejected_before_creating_outside_directories() {
    let fixture = Fixture::new();
    let master = fixture.master(CalibrationKind::Flat, 'b');
    let source = fixture.write_master(&master);
    let outside = fixture._directory.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    if !create_test_symlink(&outside, &fixture.ctx.cache_dir_path.join("previews"), true) {
        return;
    }
    assert!(matches!(
        preview_job(&fixture.ctx, &master, source, &DisplayOptions::default()),
        Err(AppError::NotFound)
    ));
    assert!(!outside.join("calibration-masters").exists());
}

#[cfg(any(unix, windows))]
#[test]
fn an_existing_preview_symlink_cannot_serve_an_unrelated_file() {
    let fixture = Fixture::new();
    let master = fixture.master(CalibrationKind::Flat, 'b');
    let source = fixture.write_master(&master);
    let job = preview_job(
        &fixture.ctx,
        &master,
        source.clone(),
        &DisplayOptions::default(),
    )
    .unwrap();
    let outside = fixture._directory.path().join("private.png");
    std::fs::write(&outside, b"private").unwrap();
    if !create_test_symlink(&outside, &job.cache_path, false) {
        return;
    }
    assert!(matches!(
        preview_job(&fixture.ctx, &master, source, &DisplayOptions::default()),
        Err(AppError::NotFound)
    ));
}

fn request_path(source: &MasterSource, master: &RecordedMaster) -> MasterPath {
    MasterPath {
        job_id: source.job_id.clone(),
        group_index: source.group_index,
        master_id: Some(master.id.clone()),
    }
}

fn request_query(source: &MasterSource) -> MasterQuery {
    MasterQuery {
        revision: source.artifact_revision.clone(),
        size: None,
        midtone: None,
        shadow: None,
    }
}

async fn wait_for_generation(state: &Arc<AppState>, job: &GenJob) -> GenerationStatus {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            if let Some(status) = state
                .preview_queue
                .status_for_source(&job.cache_path, &job.fits_path)
                && status.state != GenerationState::Generating
            {
                return status;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("bounded master preview generation must settle")
}

#[tokio::test]
async fn preview_queues_then_serves_png_and_download_preserves_original_fits() {
    let fixture = Fixture::new();
    let master = fixture.master(CalibrationKind::Flat, 'b');
    let source_path = fixture.write_master(&master);
    let original = std::fs::read(&source_path).unwrap();
    let (_, source) = fixture.stack(AppliedCalibration {
        flat_master: Some(master.label.clone()),
        ..Default::default()
    });
    let response = get_preview(
        State(fixture.state.clone()),
        DbContext(fixture.ctx.clone()),
        Path(request_path(&source, &master)),
        Query(request_query(&source)),
    )
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    assert_eq!(response.headers()[CACHE_CONTROL], "no-store");
    let job = preview_job(
        &fixture.ctx,
        &master,
        master_path(&fixture.ctx, &master).unwrap(),
        &DisplayOptions::default(),
    )
    .unwrap();
    assert_eq!(
        wait_for_generation(&fixture.state, &job).await.state,
        GenerationState::Ready
    );
    let response = get_preview(
        State(fixture.state.clone()),
        DbContext(fixture.ctx.clone()),
        Path(request_path(&source, &master)),
        Query(request_query(&source)),
    )
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[CONTENT_TYPE], "image/png");
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(
        image::guess_format(&bytes).unwrap(),
        image::ImageFormat::Png
    );
    let response = download_fits(
        State(fixture.state.clone()),
        DbContext(fixture.ctx.clone()),
        Path(request_path(&source, &master)),
        Query(request_query(&source)),
    )
    .await
    .unwrap();
    assert_eq!(response.headers()[CONTENT_TYPE], "application/fits");
    assert_eq!(response.headers()[CACHE_CONTROL], "private, no-store");
    assert!(response.headers()[CONTENT_DISPOSITION]
        .to_str()
        .unwrap()
        .contains(&master.label));
    let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(bytes.as_ref(), original);
}

#[tokio::test]
async fn generation_failure_is_terminal_and_status_batches_preserve_request_order() {
    let fixture = Fixture::new();
    let master = fixture.master(CalibrationKind::Flat, 'b');
    let source_path = fixture.write_master(&master);
    std::fs::write(&source_path, b"not a FITS file").unwrap();
    let (_, source) = fixture.stack(AppliedCalibration {
        flat_master: Some(master.label.clone()),
        ..Default::default()
    });
    let response = get_preview(
        State(fixture.state.clone()),
        DbContext(fixture.ctx.clone()),
        Path(request_path(&source, &master)),
        Query(request_query(&source)),
    )
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    let job = preview_job(
        &fixture.ctx,
        &master,
        master_path(&fixture.ctx, &master).unwrap(),
        &DisplayOptions::default(),
    )
    .unwrap();
    let status = wait_for_generation(&fixture.state, &job).await;
    assert_eq!(status.state, GenerationState::Error);
    assert!(!status
        .error
        .as_ref()
        .unwrap()
        .contains(&fixture.ctx.cache_dir));
    let response = get_preview(
        State(fixture.state.clone()),
        DbContext(fixture.ctx.clone()),
        Path(request_path(&source, &master)),
        Query(request_query(&source)),
    )
    .await;
    assert!(matches!(response, Err(AppError::InternalError(_))));
    let mut stale_source = source.clone();
    stale_source.artifact_revision = "unknown-revision".into();
    let item = MasterStatusItem {
        source,
        master_id: master.id.clone(),
        display: DisplayOptions::default(),
    };
    let missing = MasterStatusItem {
        master_id: "c".repeat(64),
        ..item.clone()
    };
    let stale = MasterStatusItem {
        source: stale_source,
        ..item.clone()
    };
    let Json(response) = post_generation_status(
        State(fixture.state.clone()),
        DbContext(fixture.ctx.clone()),
        Json(MasterStatusRequest {
            requests: vec![item.clone(), missing, stale],
        }),
    )
    .await
    .unwrap();
    let statuses = response.data.unwrap().statuses;
    assert_eq!(statuses.len(), 3);
    assert!(statuses
        .iter()
        .all(|status| status.state == GenerationState::Error));
    assert!(statuses[0]
        .error
        .as_deref()
        .unwrap()
        .contains("generation failed"));
    assert!(statuses[1]
        .error
        .as_deref()
        .unwrap()
        .contains("unavailable"));
    assert!(statuses[2]
        .error
        .as_deref()
        .unwrap()
        .contains("displayed stack changed"));
    assert!(matches!(
        post_generation_status(
            State(fixture.state.clone()),
            DbContext(fixture.ctx.clone()),
            Json(MasterStatusRequest {
                requests: vec![item; MAX_STATUS_BATCH + 1]
            })
        )
        .await,
        Err(AppError::BadRequest(_))
    ));
    assert!(!job.cache_path.exists());
}
