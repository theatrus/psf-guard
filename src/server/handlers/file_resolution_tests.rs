use super::{
    canonical_image_roots, fill_missing_star_metadata, find_fits_file, find_fits_file_cached,
    mapped_file_sha256, metadata_suffix_candidates, preview_cache_key, quality_scan_source,
    resolve_copy, resolve_mapped_remote_image_with, with_copy, AppError, DatabaseContext,
};
use crate::commands::sync::ChangedAcquiredImage;
use crate::models::AcquiredImage;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Barrier};
use std::time::Duration;

const CAPTURE_TIME: i64 = 1_784_869_200;

fn context(temp: &tempfile::TempDir) -> (DatabaseContext, PathBuf) {
    let (ctx, roots) = context_with_roots(temp, &["images"]);
    (ctx, roots.into_iter().next().unwrap())
}

fn context_with_roots(temp: &tempfile::TempDir, names: &[&str]) -> (DatabaseContext, Vec<PathBuf>) {
    let database = temp.path().join("scheduler.sqlite");
    crate::ts_schema::create_fresh_db(&database).unwrap();
    let image_roots: Vec<PathBuf> = names.iter().map(|name| temp.path().join(name)).collect();
    for image_root in &image_roots {
        std::fs::create_dir_all(image_root).unwrap();
    }
    let ctx = DatabaseContext::new(
        "test".into(),
        "Test".into(),
        database.to_string_lossy().into_owned(),
        image_roots
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect(),
        None,
        None,
        None,
        temp.path().join("cache").to_string_lossy().into_owned(),
    )
    .unwrap();
    (ctx, image_roots)
}

fn image(metadata_path: &str, filter: &str) -> AcquiredImage {
    AcquiredImage {
        id: 42,
        project_id: 7,
        target_id: 3,
        acquired_date: Some(CAPTURE_TIME),
        filter_name: filter.into(),
        grading_status: 0,
        metadata: serde_json::json!({
            "FileName": metadata_path,
            "ExposureStartTime": "2026-07-24T05:00:00Z",
            "FilterName": filter,
        })
        .to_string(),
        reject_reason: None,
        profile_id: None,
        guid: None,
    }
}

fn fits_card(output: &mut Vec<u8>, text: &str) {
    let mut bytes = text.as_bytes().to_vec();
    assert!(bytes.len() <= 80);
    bytes.resize(80, b' ');
    output.extend_from_slice(&bytes);
}

fn write_fits(path: &Path, filter: &str, date_obs: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut header = Vec::new();
    fits_card(&mut header, "SIMPLE  =                    T");
    fits_card(&mut header, "BITPIX  =                   16");
    fits_card(&mut header, "NAXIS   =                    2");
    fits_card(&mut header, "NAXIS1  =                   10");
    fits_card(&mut header, "NAXIS2  =                   10");
    fits_card(&mut header, "IMAGETYP= 'LIGHT'");
    fits_card(&mut header, &format!("FILTER  = '{filter}'"));
    fits_card(&mut header, &format!("DATE-OBS= '{date_obs}'"));
    fits_card(&mut header, "EXPTIME =                300.0");
    fits_card(&mut header, "END");
    header.resize(header.len().div_ceil(2880) * 2880, b' ');
    let mut file = std::fs::File::create(path).unwrap();
    file.write_all(&header).unwrap();
    file.write_all(&[0u8; 2880]).unwrap();
}

/// A light with a calibrated and a registered copy recorded, both on
/// disk under the image root.
fn light_with_copies(
    temp: &tempfile::TempDir,
) -> (DatabaseContext, AcquiredImage, PathBuf, PathBuf, PathBuf) {
    use crate::frame_derivatives::{ensure_schema, record_pairing, DerivativeRecord};
    use crate::image_io::{FrameKind, KindEvidence, ProcessingSteps, Producer};
    let (ctx, root) = context(temp);
    let raw = root.join("LIGHT/frame_0115.fits");
    let calibrated = root.join("calibrated/Light_B/frame_0115_c.fits");
    let registered = root.join("registered/Light_B/frame_0115_c_r.fits");
    for path in [&raw, &calibrated, &registered] {
        write_fits(path, "B", "2026-07-24T05:00:00");
    }
    let mut light = image("D:\\pictures\\LIGHT\\frame_0115.fits", "B");
    light.guid = Some("light-guid".into());
    {
        let connection = ctx.db();
        let connection = connection.lock().unwrap();
        ensure_schema(&connection).unwrap();
        for (uuid, path, kind, steps) in [
            (
                "copy-c",
                &calibrated,
                FrameKind::Calibrated,
                ProcessingSteps::CALIBRATED,
            ),
            (
                "copy-r",
                &registered,
                FrameKind::Registered,
                ProcessingSteps::CALIBRATED.with(ProcessingSteps::REGISTERED),
            ),
        ] {
            record_pairing(
                &connection,
                &DerivativeRecord {
                    derivative_uuid: uuid.into(),
                    acquired_image_guid: "light-guid".into(),
                    kind,
                    steps,
                    primary_source: false,
                    file_name: path.file_name().unwrap().to_string_lossy().into_owned(),
                    source_tail: None,
                    size: Some(std::fs::metadata(path).unwrap().len() as i64),
                    mtime: None,
                    width: Some(10),
                    height: Some(10),
                    producer: Producer::Pixinsight,
                    evidence: KindEvidence::Header,
                    created_at: 1,
                    updated_at: 1,
                },
            )
            .unwrap();
        }
    }
    (ctx, light, raw, calibrated, registered)
}

#[test]
fn a_copy_renders_under_its_own_key_and_only_for_its_light() {
    let temp = tempfile::tempdir().unwrap();
    let (ctx, light, _, calibrated, _) = light_with_copies(&temp);
    let copy = resolve_copy(&ctx, &light, "copy-c").unwrap();
    assert_eq!(copy.path, calibrated);

    let own = preview_cache_key(
        &light,
        "frame_0115.fits",
        "screen",
        true,
        0.2,
        -2.8,
        false,
        None,
    );
    let keyed = with_copy(own.clone(), Some(&copy));
    assert!(
        keyed.starts_with(&own),
        "the light's own key stays where it is"
    );
    assert!(keyed.contains("copy-c"));
    assert_ne!(keyed, own);

    let mut other = light.clone();
    other.guid = Some("another-light".into());
    assert!(matches!(
        resolve_copy(&ctx, &other, "copy-c"),
        Err(AppError::NotFound)
    ));
    assert!(matches!(
        resolve_copy(&ctx, &light, "no-such-copy"),
        Err(AppError::NotFound)
    ));
}

#[test]
fn quality_scans_measure_the_calibrated_copy_only_when_asked() {
    let temp = tempfile::tempdir().unwrap();
    let (ctx, light, raw, calibrated, _) = light_with_copies(&temp);
    let (path, revision) = quality_scan_source(&ctx, &light, "Target", "frame_0115.fits").unwrap();
    assert_eq!(path, raw);
    assert!(revision.unwrap().starts_with("file:"));

    ctx.scan_calibrated_copies.set(true);
    let (path, revision) = quality_scan_source(&ctx, &light, "Target", "frame_0115.fits").unwrap();
    // The calibrated copy, never the registered one.
    assert_eq!(path, calibrated);
    let revision = revision.unwrap();
    assert!(revision.starts_with(crate::server::spatial_scan::CALIBRATED_SOURCE_PREFIX));
    // Its stars never fill the light's metadata.
    assert_eq!(
        crate::server::spatial_scan::star_metrics_metadata_patch(
            "{\"FileName\":\"frame_0115.fits\"}",
            120,
            2.5,
            Some(&revision),
        ),
        None
    );
}

#[test]
fn a_light_whose_own_file_is_calibrated_is_measured_as_calibrated() {
    // The catalog took this light from its calibrated copy (no raw
    // frame yet): its scan is marked calibrated whatever the option, so
    // scoring keeps it out of the raw frames' cohort.
    let temp = tempfile::tempdir().unwrap();
    let (ctx, mut light, _, calibrated, _) = light_with_copies(&temp);
    light.metadata = serde_json::json!({ "FileName": "frame_0115_c.fits" }).to_string();
    {
        let connection = ctx.db();
        let connection = connection.lock().unwrap();
        connection
            .execute(
                "UPDATE psf_guard_frame_derivative SET primary_source = 1
                 WHERE derivative_uuid = 'copy-c'",
                [],
            )
            .unwrap();
    }
    let (path, revision) =
        quality_scan_source(&ctx, &light, "Target", "frame_0115_c.fits").unwrap();
    assert_eq!(path, calibrated);
    assert!(revision
        .unwrap()
        .starts_with(crate::server::spatial_scan::CALIBRATED_SOURCE_PREFIX));
}

#[test]
fn remapped_upload_cannot_commit_stale_star_metadata() {
    let temp = tempfile::tempdir().unwrap();
    let (ctx, image_root) = context(&temp);
    let metadata = serde_json::json!({ "FileName": "remote/repeated.fits" }).to_string();
    let old_path = image_root.join("old/repeated.fits");
    let new_path = image_root.join("new/repeated.fits");

    let old_revision = {
        let connection = ctx.db();
        let connection = connection.lock().unwrap();
        connection
            .execute_batch(
                "INSERT INTO project (Id, profileId, name, isMosaic, flatsHandling, guid)
                     VALUES (7, 'profile', 'Project', 0, 0, 'project-guid');
                 INSERT INTO target (Id, name, active, epochcode, projectId, guid)
                     VALUES (3, 'Target', 1, 0, 7, 'target-guid');
                 CREATE TABLE psf_guard_remote_image_file (
                     acquiredimage_id INTEGER PRIMARY KEY,
                     acquiredimage_guid TEXT,
                     acquiredimage_identity TEXT,
                     source_path TEXT NOT NULL,
                     source_sha256 TEXT NOT NULL,
                     updated_at INTEGER NOT NULL
                 );",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO acquiredimage
                    (Id, projectId, targetId, acquireddate, filtername, gradingStatus,
                     metadata, profileId, guid)
                 VALUES (42, 7, 3, ?1, 'Ha', 0, ?2, 'profile', 'image-guid')",
                rusqlite::params![CAPTURE_TIME, metadata],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO psf_guard_remote_image_file
                    (acquiredimage_id, acquiredimage_guid, acquiredimage_identity,
                     source_path, source_sha256, updated_at)
                 VALUES (42, 'image-guid', NULL, ?1, 'old-sha', 1)",
                [old_path.to_string_lossy().as_ref()],
            )
            .unwrap();
        let images = crate::db::Database::new(&connection)
            .get_images_by_ids(&[42])
            .unwrap();
        crate::server::remote_upload::mapped_light_sources(
            &connection,
            &ctx.image_dir_paths,
            images.iter(),
        )
        .unwrap()
        .quality_revision(42)
        .unwrap()
        .to_string()
    };

    ctx.spatial_metrics.write().unwrap().metrics.insert(
        42,
        crate::server::spatial_scan::StoredSpatialMetrics {
            image_id: 42,
            filename: "repeated.fits".into(),
            source_revision: Some(old_revision.clone()),
            detector: crate::server::spatial_scan::QUALITY_DETECTOR.into(),
            detector_version: crate::server::spatial_scan::QUALITY_DETECTOR_VERSION,
            star_count: 123,
            avg_hfr: 2.4,
            dead_cell_fraction: None,
            star_uniformity: None,
            bg_cell_spread: 0.0,
            bg_cell_max_dev: 0.0,
            median_adu: 1000.0,
            sky_noise_adu: None,
            zero_point: None,
            zero_point_version: 0,
            computed_at: 0,
            catalog: Default::default(),
            star_cell_counts: vec![1.0],
            star_dead_cells: vec![false],
            bg_cell_medians: vec![1000.0],
            grid_cols: 1,
            grid_rows: 1,
            width: 10,
            height: 10,
            exposure_s: Some(300.0),
            bg_glow_max: 0.0,
            bg_glow_cells: vec![false],
        },
    );
    let pending = vec![(
        42,
        "repeated.fits".to_string(),
        metadata.clone(),
        Some(old_revision),
    )];

    assert_eq!(fill_missing_star_metadata(&ctx, &pending), 1);
    {
        let connection = ctx.db();
        let connection = connection.lock().unwrap();
        connection
            .execute(
                "UPDATE acquiredimage SET metadata = ?1 WHERE Id = 42",
                [&metadata],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE psf_guard_remote_image_file
                 SET source_path = ?1, source_sha256 = 'new-sha', updated_at = 2
                 WHERE acquiredimage_id = 42",
                [new_path.to_string_lossy().as_ref()],
            )
            .unwrap();
    }

    assert_eq!(fill_missing_star_metadata(&ctx, &pending), 0);
    let stored: String = {
        let connection = ctx.db();
        let connection = connection.lock().unwrap();
        connection
            .query_row(
                "SELECT metadata FROM acquiredimage WHERE Id = 42",
                [],
                |row| row.get(0),
            )
            .unwrap()
    };
    assert_eq!(stored, metadata);
}

#[test]
fn durable_upload_mapping_wins_after_restart_and_rejects_changed_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let (ctx, image_root) = context(&temp);
    let database_path = PathBuf::from(&ctx.database_path);
    let mapped = image_root.join("M 31/LIGHT/Ha/repeated.fits");
    let duplicate = image_root.join("M 42/LIGHT/Ha/repeated.fits");
    std::fs::create_dir_all(mapped.parent().unwrap()).unwrap();
    std::fs::create_dir_all(duplicate.parent().unwrap()).unwrap();
    std::fs::write(&mapped, b"mapped upload without identity headers").unwrap();
    std::fs::write(&duplicate, b"different file with the same basename").unwrap();
    let sha256 = mapped_file_sha256(&mapped).unwrap();
    let mapped_path = std::fs::canonicalize(&mapped)
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let metadata = serde_json::json!({ "FileName": r"C:\remote\repeated.fits" });

    {
        let connection = ctx.db();
        let connection = connection.lock().unwrap();
        connection
            .execute_batch(
                "INSERT INTO project (Id, profileId, name, isMosaic, flatsHandling, guid)
                     VALUES (7, 'profile', 'Project', 0, 0, 'project-guid');
                 INSERT INTO target (Id, name, active, epochcode, projectId, guid)
                     VALUES (3, 'M 31', 1, 0, 7, 'target-guid');
                 CREATE TABLE psf_guard_remote_image_file (
                     acquiredimage_id INTEGER PRIMARY KEY,
                     acquiredimage_guid TEXT,
                     acquiredimage_identity TEXT,
                     source_path TEXT NOT NULL,
                     source_sha256 TEXT NOT NULL,
                     updated_at INTEGER NOT NULL
                 );",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO acquiredimage
                    (Id, projectId, targetId, acquireddate, filtername, gradingStatus,
                     metadata, profileId, guid)
                 VALUES (42, 7, 3, NULL, '', 0, ?1, 'profile', 'image-guid')",
                [metadata.to_string()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO psf_guard_remote_image_file
                    (acquiredimage_id, acquiredimage_guid, acquiredimage_identity,
                     source_path, source_sha256, updated_at)
                 VALUES (42, 'image-guid', NULL, ?1, ?2, 1)",
                rusqlite::params![mapped_path, sha256],
            )
            .unwrap();
    }
    drop(ctx);

    let restarted = Arc::new(
        DatabaseContext::new(
            "test".into(),
            "Test".into(),
            database_path.to_string_lossy().into_owned(),
            vec![image_root.to_string_lossy().into_owned()],
            None,
            None,
            None,
            temp.path()
                .join("cache-restarted")
                .to_string_lossy()
                .into_owned(),
        )
        .unwrap(),
    );
    let mut scheduler_image = image(r"C:\remote\repeated.fits", "");
    scheduler_image.acquired_date = None;
    scheduler_image.metadata = metadata.to_string();
    scheduler_image.guid = Some("image-guid".into());

    let worker_count = 4;
    let start = Arc::new(Barrier::new(worker_count));
    let hash_calls = Arc::new(AtomicUsize::new(0));
    let canonical_roots = Arc::new(canonical_image_roots(&restarted));
    let workers: Vec<_> = (0..worker_count)
        .map(|_| {
            let restarted = Arc::clone(&restarted);
            let scheduler_image = scheduler_image.clone();
            let start = Arc::clone(&start);
            let hash_calls = Arc::clone(&hash_calls);
            let canonical_roots = Arc::clone(&canonical_roots);
            std::thread::spawn(move || {
                start.wait();
                resolve_mapped_remote_image_with(
                    &restarted,
                    &scheduler_image,
                    canonical_roots.as_slice(),
                    |path| {
                        hash_calls.fetch_add(1, Ordering::SeqCst);
                        std::thread::sleep(Duration::from_millis(50));
                        mapped_file_sha256(path)
                    },
                )
                .unwrap()
                .unwrap()
            })
        })
        .collect();
    for worker in workers {
        assert_eq!(
            worker.join().unwrap(),
            dunce::canonicalize(&mapped).unwrap()
        );
    }
    assert_eq!(
        hash_calls.load(Ordering::SeqCst),
        1,
        "concurrent cold resolvers share one full-file hash"
    );

    assert_eq!(
        find_fits_file(&restarted, &scheduler_image, "M 31", "repeated.fits").unwrap(),
        dunce::canonicalize(&mapped).unwrap()
    );
    assert_eq!(
        find_fits_file(&restarted, &scheduler_image, "M 31", "repeated.fits").unwrap(),
        dunce::canonicalize(&mapped).unwrap(),
        "the verified mapping remains a cheap cache hit"
    );

    std::fs::write(&mapped, b"mapped upload was changed after verification").unwrap();
    match find_fits_file(&restarted, &scheduler_image, "M 31", "repeated.fits") {
        Err(AppError::Conflict(message)) => {
            assert!(message.contains("no longer matches"), "{message}");
        }
        other => panic!("changed mapped bytes were not refused: {other:?}"),
    }

    std::fs::write(&mapped, b"mapped upload without identity headers").unwrap();
    {
        let connection = restarted.db();
        connection
            .lock()
            .unwrap()
            .execute(
                "UPDATE acquiredimage SET guid = 'replacement-guid' WHERE Id = 42",
                [],
            )
            .unwrap();
    }
    scheduler_image.guid = Some("replacement-guid".into());
    match find_fits_file(&restarted, &scheduler_image, "M 31", "repeated.fits") {
        Err(AppError::Conflict(message)) => {
            assert!(
                message.contains("provenance no longer matches"),
                "{message}"
            );
        }
        other => panic!("stale provenance allowed basename fallback: {other:?}"),
    }
    let mapping_count: i64 = {
        let connection = restarted.db();
        let connection = connection.lock().unwrap();
        connection
            .query_row(
                "SELECT COUNT(*) FROM psf_guard_remote_image_file WHERE acquiredimage_id = 42",
                [],
                |row| row.get(0),
            )
            .unwrap()
    };
    assert_eq!(
        mapping_count, 1,
        "read-side resolution preserves provenance"
    );
}

#[test]
fn delayed_metadata_suffix_is_found_and_folded_into_both_caches() {
    let temp = tempfile::tempdir().unwrap();
    let (ctx, image_root) = context(&temp);

    ctx.get_directory_tree().unwrap();
    let tree_age_before = ctx.get_directory_tree_stats().unwrap().age;
    let file_cache_updated_at = {
        let mut cache = ctx.file_check_cache.write().unwrap();
        cache.has_initial_data = true;
        cache.projects_with_files.insert(7, false);
        cache.targets_with_files.insert(3, false);
        cache
            .project_latest_image_dates
            .insert(7, CAPTURE_TIME - 60);
        cache.last_updated
    };
    std::thread::sleep(Duration::from_millis(5));

    let delayed = image_root.join("rig-a/odd-layout/delayed.fits");
    write_fits(&delayed, "Ha", "2026-07-24T05:00:00");
    let image = image(r"C:\remote-captures\rig-a\odd-layout\delayed.fits", "Ha");

    let resolved = find_fits_file_cached(&ctx, &image, "Unrelated Target", "delayed.fits").unwrap();
    assert_eq!(resolved, delayed);

    {
        let tree = ctx.directory_tree_cache.read().unwrap();
        let tree = tree.as_ref().unwrap();
        assert_eq!(tree.find_file_first("delayed.fits"), Some(&delayed));
        assert!(tree.stats().age >= tree_age_before);
    }

    let cache = ctx.file_check_cache.read().unwrap();
    assert_eq!(cache.projects_with_files.get(&7), Some(&true));
    assert_eq!(cache.targets_with_files.get(&3), Some(&true));
    assert_eq!(
        cache.project_latest_image_dates.get(&7),
        Some(&CAPTURE_TIME)
    );
    assert_eq!(cache.last_updated, file_cache_updated_at);
}

#[test]
fn delayed_flattened_copy_is_found_without_rebuilding_the_tree() {
    let temp = tempfile::tempdir().unwrap();
    let (ctx, image_root) = context(&temp);
    ctx.get_directory_tree().unwrap();

    let flattened = image_root.join("flattened.fits");
    write_fits(&flattened, "Ha", "2026-07-24T05:00:00");
    let mut image = image(r"C:\remote\deep\layout\flattened.fits", "Ha");
    image.acquired_date = None;

    assert_eq!(
        find_fits_file_cached(&ctx, &image, "M 31", "flattened.fits").unwrap(),
        flattened
    );
}

#[test]
fn metadata_suffixes_cannot_traverse_or_degrade_to_a_basename() {
    let temp = tempfile::tempdir().unwrap();
    let (ctx, image_root) = context(&temp);

    assert!(
        metadata_suffix_candidates(&ctx, r"C:\remote\..\outside\frame.fits", "frame.fits")
            .is_empty()
    );
    let candidates =
        metadata_suffix_candidates(&ctx, r"C:\remote\rig\night\frame.fits", "frame.fits");
    assert!(candidates.iter().all(|path| path.starts_with(&image_root)));
    assert!(candidates.contains(&image_root.join("rig/night/frame.fits")));
    assert!(!candidates.contains(&image_root.join("frame.fits")));
}

#[test]
fn ordinary_unique_basename_accepts_legacy_coarse_timestamp() {
    let temp = tempfile::tempdir().unwrap();
    let (ctx, image_root) = context(&temp);
    let expected = image_root.join("archive/legacy.fits");
    write_fits(&expected, "Ha", "2026-07-24T05:00:00");
    ctx.get_directory_tree().unwrap();
    let mut image = image("legacy.fits", "Ha");
    image.acquired_date = Some(CAPTURE_TIME + 3_600);
    image.metadata = r#"{"FileName":"legacy.fits"}"#.into();

    assert_eq!(
        find_fits_file(&ctx, &image, "M 31", "legacy.fits").unwrap(),
        expected
    );
}

#[test]
fn expired_remote_arrival_stops_polling_but_keeps_strict_identity() {
    let temp = tempfile::tempdir().unwrap();
    let (ctx, image_root) = context(&temp);
    write_fits(
        &image_root.join("archive/remote.fits"),
        "Ha",
        "2026-07-24T05:00:03",
    );
    ctx.get_directory_tree().unwrap();
    let image = image(r"C:\remote\remote.fits", "Ha");
    ctx.register_remote_image_arrivals(&[ChangedAcquiredImage {
        id: i64::from(image.id),
        project_id: i64::from(image.project_id),
        target_id: i64::from(image.target_id),
        acquired_date: image.acquired_date,
    }]);

    assert!(matches!(
        find_fits_file(&ctx, &image, "M 31", "remote.fits"),
        Err(AppError::NotFound)
    ));
    assert_eq!(
        ctx.delayed_file_probe(image.id),
        crate::server::database_context::DelayedFileProbe::Probe,
        "strict resolution must not consume the preview probe"
    );

    ctx.expire_delayed_remote_image_for_test(image.id);
    assert_eq!(
        ctx.delayed_file_probe(image.id),
        crate::server::database_context::DelayedFileProbe::NotPending
    );
    assert!(ctx.requires_delayed_file_identity(image.id));
    assert!(matches!(
        find_fits_file(&ctx, &image, "M 31", "remote.fits"),
        Err(AppError::NotFound)
    ));

    let expected = image_root.join("archive/remote.fits");
    write_fits(&expected, "Ha", "2026-07-24T05:00:00");
    assert_eq!(
        find_fits_file(&ctx, &image, "M 31", "remote.fits").unwrap(),
        expected
    );
}

#[test]
fn ordinary_basename_fallback_rejects_ambiguity() {
    let temp = tempfile::tempdir().unwrap();
    let (ctx, image_root) = context(&temp);
    write_fits(
        &image_root.join("archive-a/repeated.fits"),
        "OIII",
        "2026-07-24T06:00:00",
    );
    write_fits(
        &image_root.join("archive-b/repeated.fits"),
        "Ha",
        "2026-07-24T05:00:00",
    );
    ctx.get_directory_tree().unwrap();
    let mut image = image("repeated.fits", "Ha");
    image.metadata = r#"{"FileName":"repeated.fits"}"#.into();

    match find_fits_file(&ctx, &image, "M 31", "repeated.fits") {
        Err(AppError::Conflict(message)) => {
            assert!(message.contains("matches multiple files"), "{message}");
        }
        _ => panic!("ordinary basename ambiguity must be refused"),
    }
}

#[test]
fn delayed_basename_fallback_requires_one_header_match() {
    let temp = tempfile::tempdir().unwrap();
    let (ctx, image_root) = context(&temp);
    let wrong = image_root.join("archive-a/repeated.fits");
    let correct = image_root.join("archive-b/repeated.fits");
    write_fits(&wrong, "OIII", "2026-07-24T05:00:00");
    write_fits(&correct, "Ha", "2026-07-24T05:00:00");
    ctx.get_directory_tree().unwrap();
    let image = image(r"C:\elsewhere\repeated.fits", "Ha");

    assert_eq!(
        find_fits_file_cached(&ctx, &image, "M 31", "repeated.fits").unwrap(),
        correct
    );

    write_fits(&wrong, "Ha", "2026-07-24T05:00:00");
    match find_fits_file_cached(&ctx, &image, "M 31", "repeated.fits") {
        Err(AppError::Conflict(message)) => {
            assert!(message.contains("matches multiple files"), "{message}");
        }
        _ => panic!("two header-matched basenames must be refused"),
    }
}

#[test]
fn delayed_basename_fallback_rejects_a_timestamp_mismatch() {
    let temp = tempfile::tempdir().unwrap();
    let (ctx, image_root) = context(&temp);
    write_fits(
        &image_root.join("archive/repeated.fits"),
        "Ha",
        "2026-07-24T05:00:03",
    );
    ctx.get_directory_tree().unwrap();
    let image = image(r"C:\elsewhere\repeated.fits", "Ha");

    assert!(matches!(
        find_fits_file_cached(&ctx, &image, "M 31", "repeated.fits"),
        Err(AppError::NotFound)
    ));
}

#[test]
fn delayed_metadata_suffix_requires_one_header_matched_path_across_roots() {
    let temp = tempfile::tempdir().unwrap();
    let (ctx, roots) = context_with_roots(&temp, &["images-a", "images-b"]);
    ctx.get_directory_tree().unwrap();
    let first = roots[0].join("rig/night/repeated.fits");
    let second = roots[1].join("rig/night/repeated.fits");
    write_fits(&first, "OIII", "2026-07-24T05:00:00");
    write_fits(&second, "Ha", "2026-07-24T05:00:00");
    let image = image(r"C:\remote\rig\night\repeated.fits", "Ha");

    assert_eq!(
        find_fits_file_cached(&ctx, &image, "M 31", "repeated.fits").unwrap(),
        second
    );

    write_fits(&first, "Ha", "2026-07-24T05:00:00");
    assert!(matches!(
        find_fits_file_cached(&ctx, &image, "M 31", "repeated.fits"),
        Err(AppError::Conflict(_))
    ));
}

#[test]
fn delayed_basename_fallback_prefers_exposure_start_over_acquired_date() {
    let temp = tempfile::tempdir().unwrap();
    let (ctx, image_root) = context(&temp);
    let expected = image_root.join("archive/repeated.fits");
    write_fits(&expected, "Ha", "2026-07-24T05:00:00");
    ctx.get_directory_tree().unwrap();
    let mut image = image(r"C:\elsewhere\repeated.fits", "Ha");
    image.acquired_date = Some(CAPTURE_TIME + 3_600);

    assert_eq!(
        find_fits_file_cached(&ctx, &image, "M 31", "repeated.fits").unwrap(),
        expected
    );
}
