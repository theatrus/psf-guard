#[test]
fn hot_pixel_filter_follows_each_session_s_dark() {
    use super::SessionCosmetics;
    use crate::calibration::CalibrationMode;
    let filter = Some(seiza_stacking::ImpulseFilterOptions::default());
    assert_eq!(
        super::impulse_filter_for(CalibrationMode::Auto, false),
        filter
    );
    assert_eq!(super::impulse_filter_for(CalibrationMode::Auto, true), None);
    assert_eq!(super::impulse_filter_for(CalibrationMode::Off, false), None);

    let mixed = SessionCosmetics {
        per_session: vec![None, filter, None],
        bypassed: filter,
    };
    assert_eq!(mixed.for_frame(0, false), None);
    assert_eq!(mixed.for_frame(1, false), filter);
    // Masters refused: the frame stacked raw and keeps only the filter.
    assert_eq!(mixed.for_frame(0, true), filter);
    assert_eq!(
        mixed.whole_group(),
        None,
        "the live stacker holds one filter"
    );
    assert_eq!(
        mixed.card_note().as_deref(),
        Some(
            "Hot pixels suppressed in 1 of 3 calibration sessions, the ones with no dark \
             master to subtract them"
        )
    );

    let none_have_darks = SessionCosmetics {
        per_session: vec![filter, filter],
        bypassed: filter,
    };
    assert_eq!(none_have_darks.whole_group(), filter);
    assert_eq!(
        none_have_darks.card_note().as_deref(),
        Some("Hot pixels suppressed in each light (no dark master to subtract them)")
    );
    let all_have_darks = SessionCosmetics::none(2);
    assert_eq!(all_have_darks.card_note(), None);
    assert_eq!(all_have_darks.whole_group(), None);
}

#[test]
fn processing_selection_rejects_a_stale_displayed_revision() {
    assert!(super::validate_processing_revision(None, "current").is_ok());
    assert!(super::validate_processing_revision(Some("current"), "current").is_ok());
    assert!(matches!(
        super::validate_processing_revision(Some("old"), "current"),
        Err(super::AppError::Conflict(_))
    ));
}

/// The stacking pipeline opens frames on its own threads and reports only
/// the disposition, so the exposure has to come from the record. Both
/// N.I.N.A. and PSF Guard's importer write it, but they disagree on the
/// key and on whether it is a number or text.
#[test]
fn exposure_reads_every_spelling_a_catalog_uses() {
    for metadata in [
        r#"{"ExposureDuration": 300.0}"#,
        r#"{"ExposureTime": 300}"#,
        r#"{"EXPTIME": "300.0"}"#,
        r#"{"ExposureDuration": "300"}"#,
    ] {
        assert_eq!(
            super::exposure_seconds_from_metadata(metadata),
            300.0,
            "{metadata}"
        );
    }
}

/// A record that does not say falls back to zero, which the orientation
/// vote reads as one vote per frame rather than a zero-weight frame.
#[test]
fn a_record_without_an_exposure_reads_as_zero() {
    for metadata in [
        r#"{"FileName": "light.fits"}"#,
        r#"{"ExposureDuration": null}"#,
        r#"{"ExposureDuration": "not a number"}"#,
        r#"{"ExposureDuration": 0}"#,
        r#"{"ExposureDuration": -5}"#,
        "not json at all",
    ] {
        assert_eq!(
            super::exposure_seconds_from_metadata(metadata),
            0.0,
            "{metadata}"
        );
    }
}
use super::*;

#[test]
fn an_unreadable_processing_selection_keeps_every_stretch() {
    let cache = tempfile::tempdir().unwrap();
    let stretch = crate::server::storage::stack_folder(
        cache.path(),
        crate::server::storage::stack_kind::STRETCH,
    )
    .join("e".repeat(64));
    std::fs::create_dir_all(&stretch).unwrap();
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(30 * 86_400);
    filetime::set_file_mtime(&stretch, filetime::FileTime::from_system_time(old)).unwrap();
    let selections = crate::server::storage::stack_processing(cache.path());
    std::fs::create_dir_all(&selections).unwrap();
    // A selection torn by a crash: it might have named this stretch.
    std::fs::write(selections.join("torn.json"), "").unwrap();

    StackPreviewManager::default().prune_cache(cache.path());

    assert!(stretch.exists());
    std::fs::remove_file(selections.join("torn.json")).unwrap();
    StackPreviewManager::default().prune_cache(cache.path());
    assert!(!stretch.exists());
}

#[test]
fn an_unreadable_index_stops_the_sweep() {
    let cache = tempfile::tempdir().unwrap();
    let stacks = cache.path().join("stack-previews");
    let job = stacks.join("d".repeat(64));
    std::fs::create_dir_all(&job).unwrap();
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3 * 86_400);
    filetime::set_file_mtime(&job, filetime::FileTime::from_system_time(old)).unwrap();
    // An index from a newer build, or a torn write: what does it name?
    std::fs::write(stacks.join("latest-project-1.json"), "{not json").unwrap();

    StackPreviewManager::default().prune_cache(cache.path());

    assert!(job.exists());
    std::fs::remove_file(stacks.join("latest-project-1.json")).unwrap();
    StackPreviewManager::default().prune_cache(cache.path());
    assert!(!job.exists());
}

fn sky_orientation() -> StackSkyOrientation {
    StackSkyOrientation::source_frame(100, 80, orientation_source::SKY_ANCHOR)
}

fn ready_group(target_id: i32, filter_name: &str, image_id: i32) -> StackGroupStatus {
    StackGroupStatus {
        index: 0,
        target_id,
        target_name: format!("Target {target_id}"),
        filter_name: filter_name.into(),
        exposure_group: None,
        state: StackGroupState::Ready,
        phase: "ready".into(),
        total_candidates: 2,
        eligible_frames: 2,
        quality_excluded: 0,
        missing_files: 0,
        calibration_excluded: 0,
        processed_frames: 2,
        accepted_frames: 2,
        rejected_frames: 0,
        reused_frames: 0,
        resume_note: None,
        output_channels: 1,
        sky_orientation: Some(sky_orientation()),
        reference_image_id: Some(image_id),
        color_calibration: None,
        color_calibration_basis: None,
        color_calibration_outdated: None,
        preview_stretch_version: None,
        preview_revision: None,
        live_preview: None,
        preview_render_failed: None,
        total_exposure_seconds: 120.0,
        preview_url: None,
        fits_url: None,
        snr: None,
        snr_url: None,
        final_pass: None,
        calibration_progress: None,
        error: None,
        calibration: crate::calibration::AppliedCalibration::default(),
        input_images: vec![StackInputImage {
            image_id,
            grading_status: 1,
        }],
        frames: Vec::new(),
    }
}

#[test]
fn a_reply_leaves_out_each_frames_mapping() {
    let pixels = seiza_stacking::LinearImage::new(512, 512, 1, vec![100.0; 512 * 512]).unwrap();
    let mut group = ready_group(5, "Ha", 11);
    group.frames = (0..4)
        .map(|index| StackFrameDecision {
            disposition: "integrated".into(),
            reason: None,
            registered_mapping: Some(seiza_stacking::RegisteredFrameMapping::identity(&pixels)),
            ..excluded_decision_for(11 + index, String::new())
        })
        .collect();
    let job = completed_job("job-a", vec![group]);
    let stored = serde_json::to_string(&job).unwrap();
    let sent = job.clone().for_client();

    assert!(sent.groups[0]
        .frames
        .iter()
        .all(|frame| frame.registered_mapping.is_none()));
    assert_eq!(sent.groups[0].frames.len(), 4);
    assert_eq!(sent.groups[0].frames[0].disposition, "integrated");
    // The stored job keeps them for the final pass and the sky coverage.
    assert!(job.groups[0].frames[0].registered_mapping.is_some());
    assert!(serde_json::to_string(&sent).unwrap().len() < stored.len());
}

fn completed_job(job_id: &str, groups: Vec<StackGroupStatus>) -> StackPreviewJob {
    StackPreviewJob {
        automatic: false,
        schema_version: 2,
        job_id: job_id.into(),
        database_id: "db-test".into(),
        project_id: 7,
        state: StackJobState::Completed,
        accepted_only: false,
        created_unix_seconds: 100,
        artifact_revision: format!("revision-{job_id}"),
        cache_version: STACK_PREVIEW_CACHE_VERSION,
        stacking_version: SEIZA_STACKING_OUTPUT_VERSION.into(),
        order: snr::StackFrameOrder::Capture,
        scoring: StackScoringSettings::default(),
        method: Default::default(),
        groups,
        error: None,
        color_defaults: None,
        preparing: None,
        replaced_by: Vec::new(),
    }
}

fn exposure_group(key: &str, seconds: f64) -> ExposureGroup {
    ExposureGroup {
        key: key.into(),
        label: format!("{seconds} s"),
        min_seconds: Some(seconds),
        max_seconds: Some(seconds),
    }
}

#[test]
fn latest_exposure_groups_are_independent_and_replace_only_their_own_family() {
    let cache = tempfile::tempdir().unwrap();
    let mut short = ready_group(42, "Ha", 1);
    short.exposure_group = Some(exposure_group("short", 10.0));
    let mut long = ready_group(42, "Ha", 2);
    long.exposure_group = Some(exposure_group("long", 300.0));
    persist_latest_groups(
        cache.path(),
        &completed_job("first", vec![short.clone(), long]),
    )
    .unwrap();
    persist_latest_groups(cache.path(), &completed_job("second", vec![short])).unwrap();
    let latest: LatestStackPreviews =
        serde_json::from_slice(&std::fs::read(latest_path(cache.path(), 7)).unwrap()).unwrap();
    assert_eq!(latest.groups.len(), 2);
    assert_eq!(
        latest
            .groups
            .iter()
            .find(|entry| exposure_group_key(&entry.group) == Some("short"))
            .unwrap()
            .job_id,
        "second"
    );
    assert_eq!(
        latest
            .groups
            .iter()
            .find(|entry| exposure_group_key(&entry.group) == Some("long"))
            .unwrap()
            .job_id,
        "first"
    );
}

#[test]
fn latest_group_must_match_the_current_project_setting_and_every_input() {
    use crate::server::exposure_groups::ProjectProcessingSettings;
    let short = exposure_group("short", 10.0);
    let mut current = ProjectExposureGroups {
        settings: ProjectProcessingSettings {
            split_exposure_groups: true,
            process_folder: None,
        },
        by_image: HashMap::from([(1, short.clone()), (2, exposure_group("long", 300.0))]),
    };
    let mut group = ready_group(42, "Ha", 1);
    assert!(!group_matches_project_exposures(&group, &current));
    group.exposure_group = Some(short);
    assert!(group_matches_project_exposures(&group, &current));
    group.input_images.push(StackInputImage {
        image_id: 2,
        grading_status: 1,
    });
    assert!(!group_matches_project_exposures(&group, &current));
    group.input_images.pop();
    current.by_image.remove(&1);
    assert!(!group_matches_project_exposures(&group, &current));
    current.settings.split_exposure_groups = false;
    assert!(!group_matches_project_exposures(&group, &current));
    group.exposure_group = None;
    assert!(group_matches_project_exposures(&group, &current));
}

#[test]
fn old_stack_groups_without_exposure_metadata_remain_readable() {
    let mut value = serde_json::to_value(ready_group(42, "Ha", 1)).unwrap();
    value.as_object_mut().unwrap().remove("exposure_group");
    let group: StackGroupStatus = serde_json::from_value(value).unwrap();
    assert!(group.exposure_group.is_none());
}

#[test]
fn successful_split_publish_retires_only_obsolete_split_families() {
    use crate::server::exposure_groups::ProjectProcessingSettings;
    let cache = tempfile::tempdir().unwrap();
    let unsplit = ready_group(42, "Ha", 1);
    let mut old_short = ready_group(42, "Ha", 1);
    old_short.exposure_group = Some(exposure_group("old-short", 11.0));
    let mut long = ready_group(42, "Ha", 2);
    long.exposure_group = Some(exposure_group("long", 300.0));
    persist_latest_groups(
        cache.path(),
        &completed_job("previous", vec![unsplit, old_short, long]),
    )
    .unwrap();
    let short = exposure_group("new-short", 10.0);
    let current = ProjectExposureGroups {
        settings: ProjectProcessingSettings {
            split_exposure_groups: true,
            process_folder: None,
        },
        by_image: HashMap::from([(1, short.clone()), (2, exposure_group("long", 300.0))]),
    };
    let mut replacement = ready_group(42, "Ha", 1);
    replacement.exposure_group = Some(short);
    persist_latest_groups_with_exposures(
        cache.path(),
        &completed_job("current", vec![replacement]),
        Some(&current),
    )
    .unwrap();
    let mut queued_before_regroup = ready_group(42, "Ha", 1);
    queued_before_regroup.exposure_group = Some(exposure_group("old-short", 11.0));
    persist_latest_groups_with_exposures(
        cache.path(),
        &completed_job("stale-queued", vec![queued_before_regroup]),
        Some(&current),
    )
    .unwrap();
    let latest: LatestStackPreviews =
        serde_json::from_slice(&std::fs::read(latest_path(cache.path(), 7)).unwrap()).unwrap();
    let keys: HashSet<_> = latest
        .groups
        .iter()
        .map(|entry| exposure_group_key(&entry.group))
        .collect();
    assert_eq!(keys, HashSet::from([None, Some("new-short"), Some("long")]));
}

#[test]
fn prepare_job_uses_persisted_full_project_exposure_groups_for_subsets() {
    let directory = tempfile::tempdir().unwrap();
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    crate::ts_schema::apply_schema(&conn).unwrap();
    conn.execute_batch("INSERT INTO project(Id,name,profileId,guid) VALUES(1,'Project','profile','project-one');
        INSERT INTO target(Id,name,projectId,active,ra,dec,epochcode,rotation,roi,guid)
            VALUES(1,'Target',1,1,10,20,0,0,100,'target-one');
        CREATE TABLE psf_guard_project_processing(project_key TEXT PRIMARY KEY,split_exposure_groups INTEGER NOT NULL,split_chosen INTEGER);
        INSERT INTO psf_guard_project_processing VALUES('guid:project-one',1,1);").unwrap();
    for (id, exposure) in [(1, 10.0), (2, 11.0), (3, 300.0), (4, 310.0)] {
        conn.execute("INSERT INTO acquiredimage(Id,projectId,targetId,gradingStatus,metadata,acquireddate,filtername)
            VALUES(?1,1,1,1,?2,?1,'Ha')", rusqlite::params![id, serde_json::json!({"ExposureDuration":exposure}).to_string()]).unwrap();
    }
    let mut ctx = DatabaseContext::new_for_test(conn);
    ctx.use_storage_for_test(directory.path());
    let ctx = Arc::new(ctx);
    let request = |ids: Vec<i32>| {
        serde_json::from_value::<StackPreviewRequest>(
            serde_json::json!({"image_ids":ids,"calibration":"off"}),
        )
        .unwrap()
    };
    let full = prepare_job(&ctx, 1, &request(vec![1, 2, 3, 4])).unwrap();
    let subset = prepare_job(&ctx, 1, &request(vec![2, 4])).unwrap();
    assert_eq!(full.public.groups.len(), 2);
    assert_eq!(subset.public.groups.len(), 2);
    for group in &subset.public.groups {
        assert_eq!(group.total_candidates, 1);
        let whole_group = full
            .public
            .groups
            .iter()
            .find(|candidate| exposure_group_key(candidate) == exposure_group_key(group))
            .unwrap();
        assert_eq!(whole_group.exposure_group, group.exposure_group);
        assert_eq!(whole_group.total_candidates, 2);
    }
    {
        let db = ctx.db();
        db.lock()
            .unwrap()
            .execute(
                "UPDATE psf_guard_project_processing SET split_exposure_groups=0, split_chosen=1",
                [],
            )
            .unwrap();
    }
    let unsplit = prepare_job(&ctx, 1, &request(vec![2, 4])).unwrap();
    assert_eq!(unsplit.public.groups.len(), 1);
    assert!(unsplit.public.groups[0].exposure_group.is_none());
    assert_ne!(unsplit.public.job_id, subset.public.job_id);
}

/// A tiny FITS frame with the readings calibration matching compares.
fn write_calibration_test_fits(path: &FsPath, kind: &str, rotation: f64) {
    use std::io::Write as _;
    let cards = [
        "SIMPLE  =                    T".to_string(),
        "BITPIX  =                   16".to_string(),
        "NAXIS   =                    2".to_string(),
        "NAXIS1  =                    4".to_string(),
        "NAXIS2  =                    4".to_string(),
        format!("IMAGETYP= '{kind}'"),
        "FILTER  = 'Ha'".to_string(),
        "EXPTIME =                300.0".to_string(),
        "GAIN    =                  100".to_string(),
        "OFFSET  =                   20".to_string(),
        "XBINNING=                    1".to_string(),
        "YBINNING=                    1".to_string(),
        "CCD-TEMP=                -10.0".to_string(),
        "TELESCOP= 'Scope'".to_string(),
        "INSTRUME= 'Camera'".to_string(),
        format!("ROTATANG= {rotation:20.1}"),
        "END".to_string(),
    ];
    let mut header = Vec::new();
    for card in cards {
        let mut bytes = card.into_bytes();
        bytes.resize(80, b' ');
        header.extend(bytes);
    }
    header.resize(header.len().div_ceil(2880) * 2880, b' ');
    let mut payload = Vec::new();
    for _ in 0..16 {
        payload.extend(1_000i16.to_be_bytes());
    }
    payload.resize(2880, 0);
    let mut file = std::fs::File::create(path).unwrap();
    file.write_all(&header).unwrap();
    file.write_all(&payload).unwrap();
}

#[test]
fn a_light_the_library_cannot_calibrate_is_left_out_and_says_why() {
    let directory = tempfile::tempdir().unwrap();
    let images = directory.path().join("images");
    std::fs::create_dir(&images).unwrap();
    let mut conn = rusqlite::Connection::open_in_memory().unwrap();
    crate::ts_schema::apply_schema(&conn).unwrap();
    conn.execute_batch(
        "INSERT INTO project(Id,name,profileId,guid) VALUES(1,'Project','profile','project-one');
        INSERT INTO target(Id,name,projectId,active,ra,dec,epochcode,rotation,roi,guid)
            VALUES(1,'Target',1,1,10,20,0,0,100,'target-one');",
    )
    .unwrap();
    // Two lights at the flats' angle, one turned away from them.
    for (id, rotation) in [(1, 94.7), (2, 94.7), (3, 120.0)] {
        let name = format!("light-{id}.fits");
        write_calibration_test_fits(&images.join(&name), "LIGHT", rotation);
        conn.execute(
            "INSERT INTO acquiredimage(Id,projectId,targetId,gradingStatus,metadata,acquireddate,filtername)
             VALUES(?1,1,1,1,?2,?1,'Ha')",
            rusqlite::params![
                id,
                serde_json::json!({"FileName": name, "ExposureDuration": 300.0}).to_string()
            ],
        )
        .unwrap();
    }
    let mut flats = Vec::new();
    for index in 0..3 {
        let path = directory.path().join(format!("flat-{index}.fits"));
        write_calibration_test_fits(&path, "FLAT", 94.7);
        flats.push(crate::commands::import::headers::read_frame_meta(&path));
    }
    let tx = conn.transaction().unwrap();
    crate::calibration::import_calibration_frames(&tx, &flats, Some("profile")).unwrap();
    tx.commit().unwrap();

    let mut ctx = DatabaseContext::new_for_test(conn);
    ctx.use_storage_for_test(directory.path());
    ctx.image_dir_paths = vec![images];
    let ctx = Arc::new(ctx);
    let request = |calibration: &str| {
        serde_json::from_value::<StackPreviewRequest>(
            serde_json::json!({"image_ids": [1, 2, 3], "calibration": calibration}),
        )
        .unwrap()
    };

    // Off unless a person turns it on: every light stacks.
    let kept = prepare_job(&ctx, 1, &request("auto")).unwrap();
    assert_eq!(kept.public.groups[0].eligible_frames, 3);
    assert_eq!(kept.public.groups[0].calibration_excluded, 0);

    struct Restore;
    impl Drop for Restore {
        fn drop(&mut self) {
            crate::calibration::configure_exclude_uncalibrated(false);
        }
    }
    let _restore = Restore;
    crate::calibration::configure_exclude_uncalibrated(true);
    let auto = prepare_job(&ctx, 1, &request("auto")).unwrap();
    assert_ne!(auto.public.job_id, kept.public.job_id);
    let group = &auto.public.groups[0];
    assert_eq!(group.eligible_frames, 2);
    assert_eq!(group.calibration_excluded, 1);
    let left_out = group
        .frames
        .iter()
        .find(|frame| frame.image_id == 3)
        .expect("the turned light is listed");
    assert_eq!(left_out.disposition, "excluded");
    assert_eq!(
        left_out.reason.as_deref(),
        Some("No matching flat: the nearest Ha flats are 25° off its rotation")
    );

    // With calibration off nothing is missing, and the job differs.
    let off = prepare_job(&ctx, 1, &request("off")).unwrap();
    assert_eq!(off.public.groups[0].eligible_frames, 3);
    assert_eq!(off.public.groups[0].calibration_excluded, 0);
    assert_ne!(off.public.job_id, auto.public.job_id);
}

#[test]
fn a_channel_build_is_the_same_build_whether_asked_alone_or_split() {
    let directory = tempfile::tempdir().unwrap();
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    crate::ts_schema::apply_schema(&conn).unwrap();
    conn.execute_batch(
        "INSERT INTO project(Id,name,profileId,guid) VALUES(1,'Project','profile','project-one');
        INSERT INTO target(Id,name,projectId,active,ra,dec,epochcode,rotation,roi,guid)
            VALUES(1,'Target',1,1,10,20,0,0,100,'target-one');",
    )
    .unwrap();
    for (id, filter) in [(1, "Ha"), (2, "Ha"), (3, "OIII"), (4, "OIII")] {
        conn.execute("INSERT INTO acquiredimage(Id,projectId,targetId,gradingStatus,metadata,acquireddate,filtername)
            VALUES(?1,1,1,1,'{}',?1,?2)", rusqlite::params![id, filter]).unwrap();
    }
    let mut ctx = DatabaseContext::new_for_test(conn);
    ctx.use_storage_for_test(directory.path());
    let ctx = Arc::new(ctx);
    let request: StackPreviewRequest =
        serde_json::from_value(serde_json::json!({"image_ids":[1, 2, 3, 4],"calibration":"off"}))
            .unwrap();
    let whole = prepare_job(&ctx, 1, &request).unwrap();
    // With no exposure recorded, the project's exposure split still
    // names the group.
    let oiii = StackChannelKey {
        target_id: 1,
        filter_name: "OIII".into(),
        exposure_group_key: whole.public.groups[1]
            .exposure_group
            .as_ref()
            .map(|group| group.key.clone()),
    };
    let mut alone = request.clone();
    alone.channel = Some(oiii.clone());
    let first = prepare_job(&ctx, 1, &alone).unwrap();
    let again = prepare_job(&ctx, 1, &alone).unwrap();
    assert_eq!(first.public.groups.len(), 1);
    assert_eq!(first.public.groups[0].filter_name, "OIII");
    assert_eq!(first.public.groups[0].index, 0);
    assert_eq!(
        first.public.job_id, again.public.job_id,
        "a channel build is stable"
    );
    assert_ne!(first.public.job_id, whole.public.job_id);

    // No frame has a file, so the split has nothing to queue: both
    // channels come back together as one build that settles at once,
    // the same build the single route makes for their images.
    let split = prepare_channels(&ctx, 1, &request, None).unwrap();
    assert_eq!(split.len(), 1);
    assert_eq!(split[0].1.public.groups.len(), 2);
    assert!(split[0]
        .1
        .public
        .groups
        .iter()
        .all(|group| group.state == StackGroupState::Skipped));
    assert_eq!(split[0].1.public.job_id, whole.public.job_id);
    // Asked by name, a channel with too few frames still comes back,
    // as itself.
    let named = prepare_channels(&ctx, 1, &request, Some(std::slice::from_ref(&oiii))).unwrap();
    assert_eq!(named.len(), 1);
    assert_eq!(named[0].0.channel.as_ref(), Some(&oiii));
    assert_eq!(named[0].1.public.job_id, first.public.job_id);
}

#[test]
fn the_final_pass_keeps_the_progress_moving() {
    let mut group = ready_group(42, "Ha", 1);
    group.eligible_frames = 10;
    group.processed_frames = 4;
    group.state = StackGroupState::Running;
    // Calibration settled, then 4 of 10 frames registered, with 20
    // final reads still to come: each frame read once, then integrated.
    assert_eq!(group_work(&group, true), (14, 40));
    assert_eq!(group_work(&group, false), (14, 20));
    // The live pass is done, all 8 admitted frames are read, and 3 of
    // them integrated.
    group.processed_frames = 10;
    group.final_pass = Some(FinalPassProgress {
        pass: 2,
        passes: 2,
        frame: 3,
        frames: 8,
    });
    assert_eq!(group_work(&group, true), (31, 36));
    group.state = StackGroupState::Ready;
    assert_eq!(group_work(&group, true), (36, 36));

    let mut job = completed_job("final", vec![group]);
    job.state = StackJobState::Running;
    job.groups[0].state = StackGroupState::Running;
    let entry = mono_activity(&job);
    assert_eq!((entry.processed_units, entry.total_units), (31, 36));
    assert_eq!(
        entry.progress_label.as_deref(),
        Some("Rejecting transients · pass 2/2 · frame 3/8")
    );
}

#[test]
fn a_queue_entry_names_the_target_it_works_on() {
    let mut running = ready_group(42, "Ha", 2);
    running.state = StackGroupState::Running;
    let mut job = completed_job("targets", vec![ready_group(9, "R", 1), running]);
    job.state = StackJobState::Running;
    assert_eq!(mono_activity(&job).target_id, Some(42));

    // Nothing in flight: the job's target, when it has only one.
    job.groups[1].state = StackGroupState::Ready;
    assert_eq!(mono_activity(&job).target_id, None);
    let single = completed_job(
        "single",
        vec![ready_group(9, "R", 1), ready_group(9, "G", 2)],
    );
    assert_eq!(mono_activity(&single).target_id, Some(9));
}

#[test]
fn a_master_build_names_itself_and_its_frame_in_the_queue() {
    let mut group = ready_group(42, "L", 1);
    group.eligible_frames = 10;
    group.processed_frames = 0;
    group.state = StackGroupState::Running;
    group.phase = "calibration".into();
    let mut job = completed_job("masters", vec![group]);
    job.state = StackJobState::Running;
    assert_eq!(mono_activity(&job).progress_label, None);

    job.groups[0].calibration_progress = Some(crate::calibration::MasterBuildProgress {
        kind: crate::calibration::CalibrationKind::Dark,
        filter: Some("L".into()),
        stage: crate::calibration::MasterBuildStage::Integrate,
        done: 3,
        total: 20,
        fraction: 0.575,
        build: 1,
    });
    let entry = mono_activity(&job);
    assert_eq!(entry.detail, "Building calibration masters");
    // The first master is a little past half way: about half of the half
    // of the calibration share the first build covers.
    assert_eq!((entry.processed_units, entry.total_units), (2, 40));
    assert_eq!(
        entry.progress_label.as_deref(),
        Some("Master dark · integrating frame 4/20")
    );
    job.groups[0].calibration_progress = Some(crate::calibration::MasterBuildProgress {
        kind: crate::calibration::CalibrationKind::Flat,
        filter: Some("L".into()),
        stage: crate::calibration::MasterBuildStage::Read,
        done: 6,
        total: 30,
        fraction: 0.14,
        build: 3,
    });
    let entry = mono_activity(&job);
    assert_eq!(
        entry.progress_label.as_deref(),
        Some("Master flat L · reading frame 7/30")
    );
    // A third build starts with three quarters of the share behind it.
    assert_eq!((entry.processed_units, entry.total_units), (7, 40));
}

#[test]
fn a_saved_channel_with_the_old_master_progress_still_loads() {
    let mut value = serde_json::to_value(ready_group(42, "Ha", 1)).unwrap();
    value["calibration_progress"] = serde_json::json!({
        "kind": "dark", "frame": 3, "frames": 8, "pass": 2, "passes": 2, "build": 1
    });
    let group: StackGroupStatus = serde_json::from_value(value.clone()).unwrap();
    assert!(group.calibration_progress.is_none());
    value["calibration_progress"] = serde_json::Value::Null;
    let group: StackGroupStatus = serde_json::from_value(value).unwrap();
    assert!(group.calibration_progress.is_none());
}

#[test]
fn a_project_queued_by_channel_may_go_past_the_remembered_cap() {
    let manager = StackPreviewManager::new();
    for index in 0..MAX_REMEMBERED_JOBS + 10 {
        let mut queued = completed_job(&format!("{index:064x}"), vec![]);
        queued.state = StackJobState::Queued;
        assert!(manager.insert(queued), "queued channel {index} was refused");
    }
    // A job that is not waiting to run still meets the cap.
    let running = {
        let mut job = completed_job(&format!("{:064x}", 9999), vec![]);
        job.state = StackJobState::Running;
        job
    };
    assert!(!manager.insert(running));
}

#[test]
fn color_waits_while_another_build_still_stacks_the_target() {
    let manager = StackPreviewManager::new();
    let mut red = ready_group(42, "R", 0);
    red.state = StackGroupState::Queued;
    let mut waiting = completed_job("waiting-red", vec![red]);
    waiting.state = StackJobState::Queued;
    assert!(manager.insert(waiting));
    assert!(manager.insert(completed_job("done-green", vec![ready_group(42, "G", 0)])));

    let database = "db-test";
    let still = |target| manager.target_still_building(database, 7, target, "done-green");
    assert!(still(42), "R is still waiting to stack for target 42");
    assert!(!still(43), "nothing waits for another target");
    // The waiting build itself does not hold its own color back.
    assert!(!manager.target_still_building(database, 7, 42, "waiting-red"));
    manager.update("waiting-red", |job| {
        job.state = StackJobState::Completed;
        job.groups[0].state = StackGroupState::Ready;
    });
    assert!(!still(42));
}

#[test]
fn the_calibration_share_only_grows_across_masters() {
    let progress = |build, fraction| crate::calibration::MasterBuildProgress {
        kind: crate::calibration::CalibrationKind::Dark,
        filter: None,
        stage: crate::calibration::MasterBuildStage::Read,
        done: 0,
        total: 8,
        fraction,
        build,
    };
    let mut last = 0.0;
    for build in 1..=4 {
        for fraction in [0.0, 0.3, 0.5, 1.0] {
            let share = progress(build, fraction).share();
            assert!(
                share >= last,
                "build {build} at {fraction}: {share} < {last}"
            );
            assert!(share < 1.0);
            last = share;
        }
    }
}

#[tokio::test]
async fn the_line_decides_which_waiting_build_runs_next() {
    let manager = StackPreviewManager::new();
    // Something holds the worker while three builds line up.
    let busy = manager.line.reserve_main().await;
    for job in ["a", "b", "c"] {
        manager.queue_test_build(job);
    }
    assert_eq!(manager.line_position("c"), Some(2));
    // The person moves c to the front and stops a.
    assert!(manager.move_in_line("c", 0));
    assert!(manager.request_cancel("a"));
    assert!(!manager.move_in_line("never-queued", 0));
    assert_eq!(
        manager.dispatch_next().await,
        (false, "a".to_string()),
        "a stopped build leaves the line though the worker is busy"
    );
    assert_eq!(manager.line_position("a"), None);
    drop(busy);
    assert_eq!(manager.dispatch_next().await, (true, "c".to_string()));
    assert_eq!(manager.dispatch_next().await, (true, "b".to_string()));
    assert!(manager.line.order().is_empty());
}

#[test]
fn a_server_stop_keeps_its_builds_without_counting_a_cut_off() {
    let origin = |project_id, running| journal::JournaledStackJob::Mono {
        database_id: "db-test".into(),
        project_id,
        automatic: false,
        request: serde_json::from_value(serde_json::json!({
            "image_ids": [1, 2],
            "method": {"final_pass": "draft"}
        }))
        .unwrap(),
        split: false,
        job_id: None,
        attempts: 0,
        running,
    };
    let manager = StackPreviewManager::new();
    for (job_id, project_id, state) in [
        ("running", 1, StackJobState::Running),
        ("waiting", 2, StackJobState::Queued),
        ("finishing", 3, StackJobState::Running),
    ] {
        let mut job = completed_job(job_id, vec![]);
        job.project_id = project_id;
        job.state = state;
        assert!(manager.insert(job));
        manager.track_job(job_id);
        manager.remember_origin(job_id, origin(project_id, false));
    }
    manager.queue_test_build("waiting");

    manager.stop_for_shutdown();
    assert!(
        manager
            .line
            .tracked_ids()
            .iter()
            .all(|id| manager.line.stop_flag(id).unwrap().requested()),
        "every build is asked to stop"
    );
    // Killed now, the builds still running would count as cut off.
    assert_eq!(
        manager.journal_entries(),
        vec![origin(3, true), origin(1, true), origin(2, false)]
    );

    // One stops at its checkpoint, one leaves the line, and one was
    // already writing its result and finishes.
    for (job_id, state) in [
        ("running", StackJobState::Cancelled),
        ("waiting", StackJobState::Cancelled),
        ("finishing", StackJobState::Completed),
    ] {
        manager.update(job_id, |job| job.state = state);
        manager.line.remove(job_id);
        manager.forget_job(job_id);
    }
    assert_eq!(
        manager.journal_entries(),
        vec![origin(1, false), origin(2, false)],
        "the stopped builds come back in order, not counted as cut off"
    );
}

#[test]
fn cancel_reaches_a_tracked_job_and_nothing_else() {
    let manager = StackPreviewManager::new();
    assert!(!manager.request_cancel("never-queued"));

    let flag = manager.track_job("queued");
    assert!(!flag.requested());
    assert!(manager.request_cancel("queued"));
    assert!(flag.requested());

    // A job that has left the queue cannot be cancelled again, so the
    // handler can answer "too late" instead of pretending it stopped.
    manager.forget_job("queued");
    assert!(!manager.request_cancel("queued"));
}

#[test]
fn cancelling_keeps_the_channels_that_already_finished() {
    let ready = ready_group(42, "Ha", 1);
    let mut running = ready_group(42, "OIII", 2);
    running.index = 1;
    running.state = StackGroupState::Running;
    let mut queued = ready_group(42, "SII", 3);
    queued.index = 2;
    queued.state = StackGroupState::Queued;
    let mut job = completed_job("mixed", vec![ready, running, queued]);

    cancel_unfinished_groups(&mut job);

    assert_eq!(job.groups[0].state, StackGroupState::Ready);
    assert_eq!(job.groups[0].phase, "ready");
    assert_eq!(job.groups[1].state, StackGroupState::Cancelled);
    assert_eq!(job.groups[2].state, StackGroupState::Cancelled);
}

#[test]
fn a_stop_that_lands_after_the_last_channel_leaves_the_job_complete() {
    let mut skipped = ready_group(42, "SII", 3);
    skipped.index = 1;
    skipped.state = StackGroupState::Skipped;
    let finished = completed_job("finished", vec![ready_group(42, "Ha", 1), skipped]);
    assert!(every_channel_settled(&finished));

    let mut stopped = finished.clone();
    stopped.groups[1].state = StackGroupState::Cancelled;
    assert!(!every_channel_settled(&stopped));

    let mut queued = finished;
    queued.groups[1].state = StackGroupState::Queued;
    assert!(!every_channel_settled(&queued));
}

#[test]
fn a_cancelled_job_still_remembers_the_channel_it_finished() {
    let cache = tempfile::tempdir().unwrap();
    let mut job = completed_job(
        "stopped",
        vec![ready_group(42, "Ha", 1), {
            let mut cancelled = ready_group(42, "OIII", 2);
            cancelled.index = 1;
            cancelled.state = StackGroupState::Cancelled;
            cancelled
        }],
    );
    job.state = StackJobState::Cancelled;

    persist_latest_groups(cache.path(), &job).unwrap();

    let latest: LatestStackPreviews =
        serde_json::from_slice(&std::fs::read(latest_path(cache.path(), job.project_id)).unwrap())
            .unwrap();
    assert_eq!(latest.groups.len(), 1);
    assert_eq!(latest.groups[0].group.filter_name, "Ha");
}

#[test]
fn active_reports_running_jobs_and_hides_finished_ones() {
    let manager = StackPreviewManager::new();

    let mut running_group = ready_group(42, "Ha", 1);
    running_group.state = StackGroupState::Running;
    running_group.phase = "stacking".into();
    running_group.processed_frames = 1;
    let mut queued_group = ready_group(42, "OIII", 2);
    queued_group.index = 1;
    queued_group.state = StackGroupState::Queued;
    queued_group.processed_frames = 0;
    let mut running = completed_job("running", vec![running_group, queued_group]);
    running.state = StackJobState::Running;
    assert!(manager.insert(running));
    assert!(manager.insert(completed_job("finished", vec![ready_group(42, "SII", 3)])));

    let active = manager.active();
    assert_eq!(active.len(), 1);
    let entry = &active[0];
    assert_eq!(entry.kind, StackActivityKind::Mono);
    assert_eq!(entry.job_id, "running");
    assert_eq!(entry.database_id, "db-test");
    assert_eq!(entry.project_id, 7);
    assert_eq!(entry.label, "Target 42 · Ha +1 more");
    assert_eq!(entry.detail, "Registering frames");
    // Each channel weighs its calibration and its two frames; the running
    // one is past calibration and has one frame.
    assert_eq!(entry.processed_units, 3);
    assert_eq!(entry.total_units, 8);
}

#[test]
fn active_counts_ready_groups_as_finished_frames() {
    let manager = StackPreviewManager::new();
    let mut running_group = ready_group(42, "OIII", 2);
    running_group.index = 1;
    running_group.state = StackGroupState::Running;
    running_group.phase = "calibration".into();
    running_group.processed_frames = 0;
    let mut job = completed_job("mixed", vec![ready_group(42, "Ha", 1), running_group]);
    job.state = StackJobState::Running;
    assert!(manager.insert(job));

    let active = manager.active();
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].label, "Target 42 · OIII");
    assert_eq!(active[0].detail, "Building calibration masters");
    // The ready channel counts in full; the other is still calibrating.
    assert_eq!(active[0].processed_units, 4);
    assert_eq!(active[0].total_units, 8);
}

#[test]
fn stack_quality_prefers_comparable_target_filter_scores() {
    let mut session_one = fallback_quality(1);
    session_one.quality_score = 1.0;
    let mut session_two = fallback_quality(2);
    session_two.quality_score = 1.0;
    let mut rollup_two = session_two.clone();
    rollup_two.quality_score = 0.2;

    let merged =
        prefer_target_filter_scores(vec![session_one, session_two], Some(vec![rollup_two]));
    let by_id = merged
        .iter()
        .map(|image| (image.image_id, image.quality_score))
        .collect::<HashMap<_, _>>();
    assert_eq!(by_id.get(&1), Some(&1.0));
    assert_eq!(by_id.get(&2), Some(&0.2));
    assert_eq!(merged[0].image_id, 1);
    assert_eq!(merged[1].image_id, 2);
}

#[test]
fn request_requires_unique_pair_or_more() {
    assert!(validate_request(&StackPreviewRequest {
        image_ids: vec![1],
        accepted_only: false,
        force: false,
        north_up: false,
        calibration: crate::calibration::CalibrationMode::Auto,
        calibration_overrides: Vec::new(),
        order: snr::StackFrameOrder::Capture,
        scoring: Default::default(),
        method: Default::default(),
        color_defaults: None,
        channel: None,
    })
    .is_err());
    assert!(validate_request(&StackPreviewRequest {
        image_ids: vec![1, 1],
        accepted_only: false,
        force: false,
        north_up: false,
        calibration: crate::calibration::CalibrationMode::Auto,
        calibration_overrides: Vec::new(),
        order: snr::StackFrameOrder::Capture,
        scoring: Default::default(),
        method: Default::default(),
        color_defaults: None,
        channel: None,
    })
    .is_err());
    assert!(validate_request(&StackPreviewRequest {
        image_ids: vec![1, 2],
        accepted_only: false,
        force: false,
        north_up: false,
        calibration: crate::calibration::CalibrationMode::Auto,
        calibration_overrides: Vec::new(),
        order: snr::StackFrameOrder::Capture,
        scoring: Default::default(),
        method: Default::default(),
        color_defaults: None,
        channel: None,
    })
    .is_ok());
}

#[test]
fn a_channel_override_wins_over_the_request_mode() {
    use crate::calibration::CalibrationMode;
    let mut request = StackPreviewRequest {
        image_ids: vec![1, 2],
        accepted_only: false,
        force: false,
        north_up: false,
        calibration: CalibrationMode::Auto,
        order: snr::StackFrameOrder::Capture,
        scoring: Default::default(),
        method: Default::default(),
        calibration_overrides: vec![CalibrationOverride {
            target_id: 7,
            filter_name: "Ha".into(),
            exposure_group_key: None,
            calibration: CalibrationMode::Off,
        }],
        color_defaults: None,
        channel: None,
    };
    assert_eq!(request.calibration_for(7, "Ha", None), CalibrationMode::Off);
    assert_eq!(
        request.calibration_for(7, "OIII", None),
        CalibrationMode::Auto
    );
    assert_eq!(
        request.calibration_for(8, "Ha", None),
        CalibrationMode::Auto
    );
    assert_eq!(
        request.calibration_for(7, "Ha", Some("short")),
        CalibrationMode::Auto
    );
    request.calibration_overrides[0].exposure_group_key = Some("short".into());
    assert_eq!(
        request.calibration_for(7, "Ha", Some("short")),
        CalibrationMode::Off
    );
    assert_eq!(
        request.calibration_for(7, "Ha", Some("long")),
        CalibrationMode::Auto
    );
    assert_eq!(
        request.calibration_for(7, "Ha", None),
        CalibrationMode::Auto
    );
}

#[test]
fn artifact_paths_are_namespaced_by_job_and_group() {
    assert_eq!(
        preview_path(FsPath::new("/cache/db"), "abc", 2),
        PathBuf::from("/cache/db/stack-previews/abc/group-2.png")
    );
    assert_eq!(
        original_preview_path(FsPath::new("/cache/db"), "abc", 2),
        PathBuf::from("/cache/db/stack-previews/abc/group-2-original.png")
    );
    assert_eq!(
        fits_path(FsPath::new("/cache/db"), "abc", 2),
        PathBuf::from("/cache/db/stack-previews/abc/group-2.fits")
    );
    assert_eq!(
        latest_path(FsPath::new("/cache/db"), 7),
        PathBuf::from("/cache/db/stack-previews/latest-project-7.json")
    );
}

#[test]
fn artifact_revisions_are_safe_cache_busters() {
    let revision = new_artifact_revision();
    assert!(!revision.is_empty());
    assert!(revision
        .bytes()
        .all(|byte| byte.is_ascii_hexdigit() || byte == b'-'));
}

#[test]
fn a_failed_snr_sidecar_is_not_advertised() {
    let cache = tempfile::tempdir().unwrap();
    let blocked_parent = cache.path().join("not-a-directory");
    std::fs::write(&blocked_parent, b"file").unwrap();
    let progressive = snr::ProgressiveSnr::new(snr::StackFrameOrder::Capture, Vec::new(), &[]);

    let url = publish_snr_artifact(
        &blocked_parent.join("curve.json"),
        &progressive,
        "/curve.json".into(),
    );

    assert!(url.is_none());
}

#[test]
fn a_channel_whose_frames_are_now_rejected_loses_its_card() {
    let cache = tempfile::tempdir().unwrap();
    let built = completed_job(
        "built",
        vec![
            ready_group(10, "SII", 1),
            ready_group(10, "OIII", 2),
            ready_group(10, "HA", 3),
        ],
    );
    persist_latest_groups(cache.path(), &built).unwrap();
    let skipped = |filter_name: &str| StackGroupStatus {
        state: StackGroupState::Skipped,
        phase: "skipped".into(),
        eligible_frames: 1,
        quality_excluded: 102,
        reference_image_id: None,
        ..ready_group(10, filter_name, 4)
    };
    let filters = || {
        let bytes = std::fs::read(latest_path(cache.path(), 7)).unwrap();
        let latest: LatestStackPreviews = serde_json::from_slice(&bytes).unwrap();
        latest
            .groups
            .into_iter()
            .map(|entry| entry.group.filter_name)
            .collect::<Vec<_>>()
    };

    // A file that went missing may come back, and so may a calibration
    // match: the card keeps the stack it has.
    let missing = StackGroupStatus {
        missing_files: 3,
        ..skipped("OIII")
    };
    let uncalibrated = StackGroupStatus {
        calibration_excluded: 3,
        ..skipped("HA")
    };
    persist_latest_groups(
        cache.path(),
        &completed_job("unavailable", vec![missing, uncalibrated]),
    )
    .unwrap();
    assert_eq!(filters(), ["HA", "OIII", "SII"]);

    // Grades left too few frames: the card no longer shows the stack.
    persist_latest_groups(
        cache.path(),
        &completed_job("rejected", vec![skipped("SII")]),
    )
    .unwrap();
    assert_eq!(filters(), ["HA", "OIII"]);

    // An automatic refresh finds the channel skipped again and leaves
    // the index as it is.
    let mut index: LatestStackPreviews =
        serde_json::from_slice(&std::fs::read(latest_path(cache.path(), 7)).unwrap()).unwrap();
    index.updated_unix_seconds = 1;
    std::fs::write(
        latest_path(cache.path(), 7),
        serde_json::to_vec(&index).unwrap(),
    )
    .unwrap();
    persist_latest_groups(cache.path(), &completed_job("again", vec![skipped("SII")])).unwrap();
    let after: LatestStackPreviews =
        serde_json::from_slice(&std::fs::read(latest_path(cache.path(), 7)).unwrap()).unwrap();
    assert_eq!(after.updated_unix_seconds, 1, "nothing was written");
}

#[test]
fn latest_index_replaces_only_the_rebuilt_channel() {
    let cache = tempfile::tempdir().unwrap();
    let first = completed_job(
        "first",
        vec![ready_group(10, "B", 1), ready_group(10, "R", 2)],
    );
    persist_latest_groups(cache.path(), &first).unwrap();

    let mut rebuilt_blue = ready_group(10, "B", 3);
    rebuilt_blue.index = 4;
    let mut second = completed_job("second", vec![rebuilt_blue]);
    second.order = snr::StackFrameOrder::Quality;
    second.scoring = StackScoringSettings {
        hfr_reject_above: Some(2.5),
        ..StackScoringSettings::default()
    };
    persist_latest_groups(cache.path(), &second).unwrap();

    let bytes = std::fs::read(latest_path(cache.path(), 7)).unwrap();
    let latest: LatestStackPreviews = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(latest.groups.len(), 2);
    let blue = latest
        .groups
        .iter()
        .find(|entry| entry.group.filter_name == "B")
        .unwrap();
    let red = latest
        .groups
        .iter()
        .find(|entry| entry.group.filter_name == "R")
        .unwrap();
    assert_eq!(blue.job_id, "second");
    assert_eq!(blue.group.reference_image_id, Some(3));
    assert_eq!(blue.order, snr::StackFrameOrder::Quality);
    assert_eq!(blue.scoring, second.scoring);
    assert_eq!(red.job_id, "first");
    assert_eq!(red.group.reference_image_id, Some(2));
    assert_eq!(red.order, snr::StackFrameOrder::Capture);
    assert_eq!(red.scoring, StackScoringSettings::default());
}

#[test]
fn latest_index_hides_artifacts_from_older_cache_versions() {
    let current = LatestStackPreviewGroup {
        job_id: "current".into(),
        artifact_revision: "current-revision".into(),
        accepted_only: false,
        created_unix_seconds: 100,
        cache_version: STACK_PREVIEW_CACHE_VERSION,
        order: snr::StackFrameOrder::Capture,
        scoring: StackScoringSettings::default(),
        method: Default::default(),
        wbpp: None,
        group: ready_group(10, "B", 1),
    };
    let mut legacy = current.clone();
    legacy.job_id = "legacy".into();
    legacy.cache_version = STACK_PREVIEW_CACHE_VERSION - 1;
    legacy.group.sky_orientation = None;
    let latest = current_latest_stacks(LatestStackPreviews {
        schema_version: 2,
        database_id: "db-test".into(),
        project_id: 7,
        updated_unix_seconds: 100,
        groups: vec![legacy, current],
    });

    assert_eq!(latest.groups.len(), 1);
    assert_eq!(latest.groups[0].job_id, "current");
}

#[test]
fn an_old_latest_entry_defaults_to_capture_order_and_calibrated_scoring() {
    let current = LatestStackPreviewGroup {
        job_id: "legacy".into(),
        artifact_revision: "legacy-revision".into(),
        accepted_only: false,
        created_unix_seconds: 100,
        cache_version: STACK_PREVIEW_CACHE_VERSION,
        order: snr::StackFrameOrder::Capture,
        scoring: StackScoringSettings::default(),
        method: Default::default(),
        wbpp: None,
        group: ready_group(10, "B", 1),
    };
    let mut serialized = serde_json::to_value(current).unwrap();
    serialized.as_object_mut().unwrap().remove("order");
    serialized.as_object_mut().unwrap().remove("scoring");

    serialized.as_object_mut().unwrap().remove("method");

    let restored: LatestStackPreviewGroup = serde_json::from_value(serialized).unwrap();

    assert_eq!(restored.order, snr::StackFrameOrder::Capture);
    assert_eq!(restored.scoring, StackScoringSettings::default());
    assert_eq!(restored.method, None);
}

#[test]
fn an_old_job_defaults_to_calibrated_scoring() {
    let mut serialized = serde_json::to_value(completed_job("legacy", Vec::new())).unwrap();
    serialized.as_object_mut().unwrap().remove("scoring");
    serialized.as_object_mut().unwrap().remove("method");

    let restored: StackPreviewJob = serde_json::from_value(serialized).unwrap();

    assert_eq!(restored.scoring, StackScoringSettings::default());
    assert_eq!(restored.method, None);
}

#[test]
fn the_whole_method_enters_the_job_id() {
    let directory = tempfile::tempdir().unwrap();
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    crate::ts_schema::apply_schema(&conn).unwrap();
    conn.execute_batch(
        "INSERT INTO project(Id,name,profileId,guid) VALUES(1,'Project','profile','project-one');
        INSERT INTO target(Id,name,projectId,active,ra,dec,epochcode,rotation,roi,guid)
            VALUES(1,'Target',1,1,10,20,0,0,100,'target-one');",
    )
    .unwrap();
    for id in 1..=3 {
        conn.execute("INSERT INTO acquiredimage(Id,projectId,targetId,gradingStatus,metadata,acquireddate,filtername)
            VALUES(?1,1,1,1,'{}',?1,'Ha')", rusqlite::params![id]).unwrap();
    }
    let mut ctx = DatabaseContext::new_for_test(conn);
    ctx.use_storage_for_test(directory.path());
    let ctx = Arc::new(ctx);
    let job = |extra: serde_json::Value| {
        let mut body = serde_json::json!({"image_ids":[1,2,3],"calibration":"off"});
        body.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        prepare_job(&ctx, 1, &serde_json::from_value(body).unwrap()).unwrap()
    };
    let omitted = job(serde_json::json!({}));
    let recommended = job(serde_json::json!({"method": StackMethod::default()}));
    let draft = job(serde_json::json!({"method": {"final_pass": "draft"}}));
    let equal = job(serde_json::json!({"method": {"weighting": "equal"}}));
    // Without a method the request takes the server's, which defaults to
    // Seiza's recommended one.
    assert_eq!(omitted.public.method, Some(StackMethod::default()));
    assert_eq!(omitted.public.job_id, recommended.public.job_id);
    // Even the final pass alone makes another artifact.
    assert_ne!(draft.public.job_id, recommended.public.job_id);
    assert_eq!(
        draft.public.method.unwrap().final_pass,
        StackFinalPass::Draft
    );
    assert_ne!(equal.public.job_id, recommended.public.job_id);
    assert_eq!(
        equal.public.method.unwrap().weighting,
        StackWeighting::Equal
    );
}

#[test]
fn noise_weighting_asks_seiza_for_inverse_noise_variance() {
    assert_eq!(
        StackWeighting::Equal.frame_weighting(),
        seiza_stacking::FrameWeighting::Equal
    );
    assert_eq!(
        StackWeighting::Noise.frame_weighting(),
        seiza_stacking::FrameWeighting::inverse_noise_variance()
    );
}

#[test]
fn scoring_snapshot_normalizes_request_overrides() {
    let overrides = crate::server::api::ScoringOverrideQuery {
        penalty_satellite: Some(3.0),
        penalty_pointing: Some(-0.0),
        penalty_temporal: Some(f64::NAN),
        penalty_hfr_rise: Some(2.5),
        penalty_star_loss: Some(0.4),
        hfr_reject_above: Some(0.0),
        star_count_reject_below: Some(35.0),
    };

    let normalized = StackScoringSettings::from_overrides(&overrides);
    assert_eq!(
        normalized,
        StackScoringSettings {
            penalty_satellite: 2.0,
            penalty_pointing: 0.0,
            penalty_temporal: 1.0,
            penalty_hfr_rise: 2.0,
            penalty_star_loss: 0.4,
            hfr_reject_above: None,
            star_count_reject_below: Some(35.0),
        }
    );
    assert_eq!(normalized.penalty_pointing.to_bits(), 0.0_f64.to_bits());
}

#[test]
fn latest_index_keeps_both_orientation_conventions() {
    let source_frame = LatestStackPreviewGroup {
        job_id: "source-frame".into(),
        artifact_revision: "source-revision".into(),
        accepted_only: false,
        created_unix_seconds: 100,
        cache_version: STACK_PREVIEW_CACHE_VERSION,
        order: snr::StackFrameOrder::Capture,
        scoring: StackScoringSettings::default(),
        method: Default::default(),
        wbpp: None,
        group: ready_group(10, "B", 1),
    };
    let mut north_up = source_frame.clone();
    north_up.job_id = "north-up".into();
    north_up.group.filter_name = "R".into();
    north_up.group.sky_orientation = Some(StackSkyOrientation {
        convention: seiza_stacking::SKY_ORIENTATION_NAME.into(),
        version: seiza_stacking::SKY_ORIENTATION_VERSION,
        source: "embedded_wcs".into(),
        output_width: 120,
        output_height: 90,
        source_to_output: seiza_stacking::AffineTransform::IDENTITY,
    });
    let latest = current_latest_stacks(LatestStackPreviews {
        schema_version: 2,
        database_id: "db-test".into(),
        project_id: 7,
        updated_unix_seconds: 100,
        groups: vec![source_frame, north_up],
    });

    assert_eq!(latest.groups.len(), 2);
}

#[test]
fn requests_keep_the_source_rotation_by_default() {
    let request: StackPreviewRequest =
        serde_json::from_str(r#"{"image_ids":[1,2]}"#).expect("request parses");
    assert!(!request.north_up);

    let request: StackPreviewRequest =
        serde_json::from_str(r#"{"image_ids":[1,2],"north_up":true}"#).expect("request parses");
    assert!(request.north_up);
}

/// A TAN WCS at the given roll, in Seiza's convention: no roll puts north
/// A TAN WCS at the given roll, in Seiza's convention: no roll puts north
/// toward decreasing Y and east toward decreasing X.
fn wcs_at_roll(roll_degrees: f64) -> seiza::Wcs {
    let scale = 0.0004;
    let (sine, cosine) = roll_degrees.to_radians().sin_cos();
    seiza::Wcs {
        crval: (305.5, 40.25),
        crpix: (512.0, 384.0),
        cd: [
            [-scale * cosine, scale * sine],
            [-scale * sine, -scale * cosine],
        ],
        sip: None,
    }
}

fn solved(roll_degrees: f64, west_of_pier: Option<bool>) -> ReferenceOrientation {
    ReferenceOrientation {
        north_up: faces_north_up(wcs_at_roll(roll_degrees).cd),
        west_of_pier,
    }
}

fn pier_only(west_of_pier: bool) -> ReferenceOrientation {
    ReferenceOrientation {
        north_up: None,
        west_of_pier: Some(west_of_pier),
    }
}

/// Every frame's vote must be weighed in the same unit. Reading the
/// reference's exposure from its opened header while the rest come from
/// the catalog gave the reference hundreds of seconds against one vote
/// each for every other frame, so a whole night on the far side of a
/// meridian flip could not outvote it and the stack published upside
/// down.
#[test]
fn a_night_past_a_flip_outvotes_the_reference_frame() {
    // What a catalog with no recorded exposure yields: zero, which the
    // vote reads as one vote per frame.
    let mut vote = OrientationVote::default();
    vote.add(0.0, 0.0);
    for _ in 0..8 {
        vote.add(std::f64::consts::PI, 0.0);
    }
    assert!(
        vote.prefers_half_turn(),
        "eight flipped frames must outvote one reference"
    );

    // And the reference still wins when it really is the majority.
    let mut vote = OrientationVote::default();
    for _ in 0..8 {
        vote.add(0.0, 300.0);
    }
    vote.add(std::f64::consts::PI, 300.0);
    assert!(!vote.prefers_half_turn());
}

/// The mixed-unit failure itself, so nobody reintroduces it by sourcing
/// one weight differently from the others.
#[test]
fn a_reference_weighed_in_seconds_would_outvote_frames_weighed_by_count() {
    let mut mixed = OrientationVote::default();
    mixed.add(0.0, 300.0);
    for _ in 0..8 {
        mixed.add(std::f64::consts::PI, 0.0);
    }
    assert!(
        !mixed.prefers_half_turn(),
        "this is the behaviour the fix exists to prevent; if it ever \
         changes, the guard above is what matters"
    );
}

/// Whether a channel ends up turned, given the job it was built in.
fn turned(group: ReferenceOrientation, job: &[ReferenceOrientation]) -> bool {
    let anchor = anchored_north_up(group, calibrate_pier_side(job));
    half_turn_decision(anchor, &OrientationVote::default()).0
}

#[test]
fn two_channels_agree_when_their_references_sit_across_a_flip() {
    // The whole point of the anchor: one channel's best frame landed
    // before the meridian flip and the other's after, so their reference
    // frames are half a turn apart. Each decides on its own, and the two
    // cards must still come out facing the same way.
    for roll in [0.0, 17.0, 90.0, 143.0, 270.0] {
        let before = solved(roll, None);
        let after = solved(roll + 180.0, None);
        let job = [before, after];

        // Exactly one of the pair turns, which lands both the same way up.
        assert_ne!(
            turned(before, &job),
            turned(after, &job),
            "roll {roll} left both channels facing the same way before turning"
        );
    }
}

#[test]
fn an_unsolved_channel_follows_a_solved_one_rather_than_a_guess() {
    // Two channels whose reference frames face the same way — both west of
    // the pier, both north-down — where only one was ever solved. Reading
    // the pier side on its own would canonicalize on the opposite
    // assumption and pull them apart, so the unsolved channel has to learn
    // the mapping from its solved sibling.
    let solved_channel = solved(180.0, Some(true));
    let unsolved_channel = pier_only(true);
    assert_eq!(
        solved_channel.north_up,
        Some(false),
        "fixture is north-down"
    );
    let job = [solved_channel, unsolved_channel];

    assert_eq!(
        turned(solved_channel, &job),
        turned(unsolved_channel, &job),
        "the unsolved channel ignored what the solved one learned"
    );
    assert_eq!(
        anchored_north_up(unsolved_channel, calibrate_pier_side(&job)),
        Some((false, orientation_source::PIER_SIDE))
    );

    // The same holds with the sides swapped, which is the other half of the
    // mapping rather than a repeat of the first.
    let solved_east = solved(0.0, Some(false));
    let unsolved_east = pier_only(false);
    let job = [solved_east, unsolved_east];
    assert_eq!(turned(solved_east, &job), turned(unsolved_east, &job));
}

#[test]
fn a_job_that_was_never_solved_still_agrees_with_itself() {
    // Nothing to calibrate from, so the assumption is arbitrary — but it
    // has to be the same arbitrary assumption in every channel.
    let west = pier_only(true);
    let east = pier_only(false);
    let job = [west, east];
    assert_ne!(turned(west, &job), turned(east, &job));
    assert!(calibrate_pier_side(&job).is_none());
}

#[test]
fn one_bad_solve_does_not_invert_every_unsolved_channel() {
    // Three channels agree that west is north-up and one disagrees. The
    // majority decides, so the outlier cannot flip the rest of the job.
    let job = [
        solved(0.0, Some(true)),
        solved(0.0, Some(true)),
        solved(0.0, Some(true)),
        solved(180.0, Some(true)),
    ];
    assert_eq!(calibrate_pier_side(&job), Some(true));

    // An even split teaches nothing rather than picking a side by order.
    let split = [solved(0.0, Some(true)), solved(180.0, Some(true))];
    assert_eq!(calibrate_pier_side(&split), None);
}

#[test]
fn the_anchor_leaves_a_north_up_reference_alone() {
    assert_eq!(faces_north_up(wcs_at_roll(0.0).cd), Some(true));
    assert_eq!(faces_north_up(wcs_at_roll(180.0).cd), Some(false));
    // A camera rolled onto its side has no usable Y component; the choice
    // still has to be steady rather than riding on solve noise.
    assert_eq!(
        faces_north_up(wcs_at_roll(90.0).cd),
        faces_north_up(wcs_at_roll(90.000_001).cd)
    );
}

#[test]
fn a_broken_solve_is_not_trusted_over_the_pier() {
    // A matrix that cannot describe a sky rotation must not outrank a
    // usable pier side, nor decide anything by itself.
    assert_eq!(faces_north_up([[0.0, 0.0], [0.0, 0.0]]), None);
    assert_eq!(faces_north_up([[f64::NAN, 0.0], [0.0, -1.0]]), None);
    assert_eq!(faces_north_up([[1.0, 2.0], [2.0, 4.0]]), None);

    let broken = ReferenceOrientation {
        north_up: faces_north_up([[0.0, 0.0], [0.0, 0.0]]),
        west_of_pier: Some(true),
    };
    assert_eq!(
        anchored_north_up(broken, None),
        Some((true, orientation_source::PIER_SIDE))
    );
}

#[test]
fn pier_side_spellings_that_mounts_actually_write() {
    assert_eq!(is_west_of_pier("West"), Some(true));
    assert_eq!(is_west_of_pier("East"), Some(false));
    // N.I.N.A. writes the prefixed spelling; both forms mean the same side.
    assert_eq!(is_west_of_pier("pierEast"), is_west_of_pier("east"));
    assert_eq!(is_west_of_pier("  pierWest "), Some(true));
    // Anything unusable must not be read as a side.
    assert_eq!(is_west_of_pier("Unknown"), None);
    assert_eq!(is_west_of_pier(""), None);
}

#[test]
fn the_exposure_majority_is_the_last_resort() {
    use std::f64::consts::PI;

    let mut flipped = OrientationVote::default();
    flipped.add(0.0, 60.0);
    flipped.add(PI, 60.0);
    flipped.add(PI, 60.0);

    // With nothing absolute to anchor to, the stack still faces the way
    // most of its own exposure faced.
    assert_eq!(
        half_turn_decision(None, &flipped),
        (true, orientation_source::EXPOSURE_MAJORITY)
    );
    // An anchor outranks it, even when the majority disagrees.
    assert_eq!(
        half_turn_decision(anchored_north_up(solved(0.0, None), None), &flipped),
        (false, orientation_source::SKY_ANCHOR)
    );
    assert_eq!(
        half_turn_decision(anchored_north_up(pier_only(true), None), &flipped),
        (false, orientation_source::PIER_SIDE)
    );
}

#[test]
fn pier_side_is_read_from_the_reference_headers() {
    let headers = vec![
        (
            "EXPOSURE".to_string(),
            seiza_fits::HeaderValue::Float(300.0),
        ),
        (
            "pierside".to_string(),
            seiza_fits::HeaderValue::String("West".into()),
        ),
    ];
    assert_eq!(pier_side_from_headers(&headers), Some("West"));
    assert_eq!(pier_side_from_headers(&headers[..1]), None);
}
#[test]
fn a_source_frame_stack_records_an_identity_mapping() {
    let orientation = StackSkyOrientation::source_frame(120, 90, orientation_source::SKY_ANCHOR);
    assert!(orientation.is_current());
    assert_eq!(orientation.convention, SOURCE_ORIENTATION_NAME);
    assert_eq!(
        orientation.source_to_output,
        seiza_stacking::AffineTransform::IDENTITY
    );
    assert_eq!(
        (orientation.output_width, orientation.output_height),
        (120, 90)
    );
}

#[test]
fn only_a_meridian_flip_counts_as_half_a_turn() {
    use std::f64::consts::{FRAC_PI_2, PI, TAU};

    // Guiding drift and ordinary field rotation stay upright.
    assert!(!is_half_turn(0.0));
    assert!(!is_half_turn(0.02));
    assert!(!is_half_turn(-0.02));
    assert!(!is_half_turn(FRAC_PI_2));
    assert!(!is_half_turn(TAU));

    // A meridian flip lands near half a turn, from either direction.
    assert!(is_half_turn(PI));
    assert!(is_half_turn(-PI));
    assert!(is_half_turn(PI - 0.02));
    assert!(is_half_turn(PI + 0.02));

    // A rotation that never fitted must not vote.
    assert!(!is_half_turn(f64::NAN));
    assert!(!is_half_turn(f64::INFINITY));
}

#[test]
fn the_stack_faces_the_side_of_the_flip_it_mostly_came_from() {
    use std::f64::consts::PI;

    // Reference on the thin side: three 60 s frames outvote it.
    let mut minority_reference = OrientationVote::default();
    minority_reference.add(0.0, 60.0);
    for _ in 0..3 {
        minority_reference.add(PI, 60.0);
    }
    assert!(minority_reference.prefers_half_turn());

    // Reference on the thick side: the stack stays as it is.
    let mut majority_reference = OrientationVote::default();
    for _ in 0..3 {
        majority_reference.add(0.0, 60.0);
    }
    majority_reference.add(PI, 60.0);
    assert!(!majority_reference.prefers_half_turn());

    // Long subs outweigh a larger count of short ones.
    let mut by_exposure = OrientationVote::default();
    by_exposure.add(0.0, 30.0);
    by_exposure.add(0.0, 30.0);
    by_exposure.add(PI, 300.0);
    assert!(by_exposure.prefers_half_turn());

    // Missing exposure times fall back to counting frames.
    let mut unmeasured = OrientationVote::default();
    unmeasured.add(0.0, 0.0);
    unmeasured.add(PI, 0.0);
    unmeasured.add(PI, 0.0);
    assert!(unmeasured.prefers_half_turn());

    // An even split keeps the reference frame's own rotation.
    let mut tied = OrientationVote::default();
    tied.add(0.0, 60.0);
    tied.add(PI, 60.0);
    assert!(!tied.prefers_half_turn());

    // A night that never flipped is never turned.
    let mut unflipped = OrientationVote::default();
    for _ in 0..5 {
        unflipped.add(0.001, 60.0);
    }
    assert!(!unflipped.prefers_half_turn());
}

#[test]
fn a_half_turn_reverses_the_pixels_and_records_its_mapping() {
    let mono =
        seiza_stacking::LinearImage::new(3, 2, 1, vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0]).unwrap();
    let turned = half_turn(mono.clone());
    assert_eq!(turned.data, vec![6.0, 5.0, 4.0, 3.0, 2.0, 1.0]);
    assert_eq!((turned.width, turned.height), (3, 2));
    // Turning twice returns the original, so the operation loses nothing.
    assert_eq!(half_turn(turned), mono);

    // RGB samples are interleaved, so channel order survives the reversal.
    let rgb =
        seiza_stacking::LinearImage::new(2, 1, 3, vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0]).unwrap();
    assert_eq!(half_turn(rgb).data, vec![4.0, 5.0, 6.0, 1.0, 2.0, 3.0]);

    // The recorded mapping must match what the pixels did: the first pixel
    // lands in the far corner, and the grid keeps its size.
    let orientation =
        StackSkyOrientation::source_frame_half_turn(3, 2, orientation_source::EXPOSURE_MAJORITY);
    assert!(orientation.is_current());
    assert_eq!(orientation.convention, SOURCE_ORIENTATION_NAME);
    assert_eq!(orientation.source, orientation_source::EXPOSURE_MAJORITY);
    assert_eq!(
        (orientation.output_width, orientation.output_height),
        (3, 2)
    );
    assert_eq!(orientation.source_to_output.apply(0.0, 0.0), (2.0, 1.0));
    assert_eq!(orientation.source_to_output.apply(2.0, 1.0), (0.0, 0.0));
    assert_eq!(orientation.source_to_output.apply(1.0, 0.0), (1.0, 1.0));
    orientation
        .source_to_output
        .validate()
        .expect("the half-turn mapping is a usable affine");
}

#[test]
fn job_ids_must_be_sha256_hex() {
    assert!(validate_job_id(&"a".repeat(64)).is_ok());
    assert!(validate_job_id(&"A".repeat(64)).is_ok());
    assert!(validate_job_id("../manifest.json").is_err());
    assert!(validate_job_id(&"g".repeat(64)).is_err());
}

#[test]
fn selection_policy_keeps_regrades_and_database_grades_authoritative() {
    let mut image = AcquiredImage {
        id: 7,
        project_id: 1,
        target_id: 2,
        acquired_date: Some(123),
        filter_name: "Ha".into(),
        grading_status: 0,
        metadata: "{}".into(),
        reject_reason: None,
        profile_id: None,
        guid: None,
    };
    let mut quality = fallback_quality(image.id);

    assert!(exclusion_reason(&image, &quality, false).is_none());
    assert!(exclusion_reason(&image, &quality, true)
        .unwrap()
        .contains("Accepted-only"));

    quality.regrade_reason = Some("[Auto] Off target".into());
    assert_eq!(
        exclusion_reason(&image, &quality, false).as_deref(),
        Some("[Auto] Off target")
    );

    image.grading_status = 2;
    assert_eq!(
        exclusion_reason(&image, &quality, false).as_deref(),
        Some("Database grade is Rejected")
    );
}

/// A build that holds the worker: a running Ha channel of target 42.
fn running_build(job_id: &str) -> StackPreviewJob {
    let mut group = ready_group(42, "Ha", 1);
    group.state = StackGroupState::Running;
    group.phase = "stacking".into();
    let mut job = completed_job(job_id, vec![group]);
    job.state = StackJobState::Running;
    job
}

fn test_state() -> Arc<AppState> {
    Arc::new(AppState::new_for_test(
        rusqlite::Connection::open_in_memory().unwrap(),
    ))
}

#[test]
fn the_activity_endpoint_lists_a_processing_run_beside_builds() {
    let state = test_state();
    assert!(state.stack_previews.insert(running_build("held")));
    let busy = state.stack_previews.line.try_reserve_main().unwrap();
    let run_id = "f".repeat(64);
    state.stack_previews.queue_processing(
        &run_id,
        processing_queue::ProcessingScope {
            database_id: "db-test".into(),
            project_id: 7,
            target_id: 42,
            target_name: "Target 42".into(),
            channel: "R".into(),
            exposure: None,
        },
        "BlurXTerminator".into(),
        true,
        processing_queue::test_origin(&run_id),
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let Json(response) = runtime.block_on(get_stack_activity(State(Arc::clone(&state))));
    let json = serde_json::to_value(response).unwrap();
    let active = json["data"]["active"].as_array().unwrap();
    assert_eq!(active[0]["kind"], "mono");
    assert!(active[0].get("processing").is_none());
    assert_eq!(
        active[1],
        serde_json::json!({
            "kind": "processing",
            "job_id": run_id,
            "database_id": "db-test",
            "project_id": 7,
            "target_id": 42,
            "state": "queued",
            "label": "Target 42 · R",
            "detail": "Waiting for the stack build of Target 42 · Ha to finish",
            "processed_units": 0,
            "total_units": 0,
            "created_unix_seconds": active[1]["created_unix_seconds"],
            "automatic": false,
            "queue_position": 0,
            "processing": {
                "operation": "BlurXTerminator",
                "channel": "R",
                "stack_job_id": "e".repeat(64),
                "group_index": 0,
                "waiting_for": "the stack build of Target 42 · Ha",
                "stoppable": true,
            },
        })
    );
    drop(busy);
    state.stack_previews.finish_processing(&run_id);
}

/// Poll until `check` holds, or fail after ten seconds.
async fn eventually(what: &str, mut check: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !check() {
        assert!(std::time::Instant::now() < deadline, "timed out: {what}");
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
}

/// One finished mono stack of target 42's R channel in project 7, on
/// disk as a server finds it.
fn stack_on_disk(cache: &FsPath, job_id: &str, revision: &str) -> StackPreviewJob {
    channel_stack_on_disk(cache, job_id, "R", revision)
}

/// As [`stack_on_disk`], for target 42's `filter` channel.
fn channel_stack_on_disk(
    cache: &FsPath,
    job_id: &str,
    filter: &str,
    revision: &str,
) -> StackPreviewJob {
    let mut job = completed_job(job_id, vec![ready_group(42, filter, 1)]);
    job.database_id = "test".into();
    job.artifact_revision = revision.into();
    let manifest = manifest_path(cache, job_id);
    std::fs::create_dir_all(manifest.parent().unwrap()).unwrap();
    stretch::write_json_atomic(&manifest, &job).unwrap();
    let fits = fits_path(cache, job_id, 0);
    if !fits.is_file() {
        let data = (0..256)
            .map(|index| 100.0 + (index % 17) as f32 * 7.0)
            .collect();
        let image = seiza_stacking::LinearImage::new(16, 16, 1, data).unwrap();
        seiza_stacking::write_processed_image_fits_f32(&fits, &image, &[], &[]).unwrap();
    }
    job
}

/// A server whose `test` database keeps its stacks in `cache`.
fn state_on_disk(cache: &FsPath, registry: Option<&FsPath>) -> Arc<AppState> {
    let mut ctx = crate::server::database_context::DatabaseContext::new_for_test(
        rusqlite::Connection::open_in_memory().unwrap(),
    );
    ctx.stack_root = cache.to_path_buf();
    let state = test_state();
    state
        .databases
        .write()
        .unwrap()
        .insert(ctx.id.clone(), Arc::new(ctx));
    *state.registry_path.write().unwrap() = registry.map(FsPath::to_path_buf);
    state
}

#[tokio::test]
async fn live_preview_requests_are_not_cached_or_served_from_the_finished_preview() {
    let cache = tempfile::tempdir().unwrap();
    let state = state_on_disk(cache.path(), None);
    let job = "a".repeat(64);
    let live_path = live::path(cache.path(), &job, 0);
    std::fs::create_dir_all(live_path.parent().unwrap()).unwrap();
    std::fs::write(&live_path, b"running image").unwrap();
    std::fs::write(preview_path(cache.path(), &job, 0), b"finished image").unwrap();
    for is_live in [true, false] {
        let response = get_stack_preview_image(
            DbContext(state.get_database("test").unwrap()),
            Path(("test".into(), job.clone(), 0)),
            Query(StackPreviewImageQuery {
                live: is_live,
                ..Default::default()
            }),
        )
        .await
        .unwrap();
        assert_eq!(
            response.headers()[CACHE_CONTROL],
            if is_live {
                "private, no-store"
            } else {
                "private, max-age=31536000, immutable"
            }
        );
        let bytes = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        assert_eq!(
            bytes.as_ref(),
            if is_live {
                b"running image".as_slice()
            } else {
                b"finished image".as_slice()
            }
        );
    }
}

/// A test's turn with the process-wide setting, which re-apply tests
/// share with the automatic refresh's. It is on again when the turn
/// ends.
struct PolicyTurn {
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl Drop for PolicyTurn {
    fn drop(&mut self) {
        reapply::configure(true);
    }
}

fn reapply_policy(enabled: bool) -> PolicyTurn {
    let guard = automatic::POLICY_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    reapply::configure(enabled);
    PolicyTurn { _guard: guard }
}

fn mtf_request(midtone: f64) -> stretch::StackViewProcessingRequest {
    serde_json::from_value(serde_json::json!({
        "model": {"type": "mtf", "shadows": 0.0, "midtone": midtone, "highlights": 1.0},
    }))
    .unwrap()
}

fn r_source(state: &Arc<AppState>, job_id: &str) -> stretch::StretchSource {
    let ctx = state.get_database("test").unwrap();
    stack_processing_source(state, &ctx, job_id, 0).unwrap()
}

/// A person applies processing to a stack and it lands.
async fn apply_and_wait(
    state: &Arc<AppState>,
    job_id: &str,
    request: stretch::StackViewProcessingRequest,
) -> String {
    let ctx = state.get_database("test").unwrap();
    match stretch::apply_to_fits(
        Arc::clone(state),
        ctx.id.clone(),
        ctx.stack_root.clone(),
        r_source(state, job_id),
        request,
        true,
    )
    .await
    .unwrap()
    {
        stretch::StretchApplyOutcome::Ready(preview) => preview.stretch_id,
        stretch::StretchApplyOutcome::Pending { stretch_id } => {
            eventually("the processing lands", || {
                shown(state, job_id).is_some_and(|shown| shown.stretch_id == stretch_id)
            })
            .await;
            stretch_id
        }
    }
}

/// What the stack's card shows: its own processing, or the previous
/// revision's while it is made again.
fn shown(state: &Arc<AppState>, job_id: &str) -> Option<stretch::StackStretchPreview> {
    let ctx = state.get_database("test").unwrap();
    let source = r_source(state, job_id);
    stretch::selected_or_carried(
        &state.stack_previews,
        &ctx.stack_root,
        &source.key,
        &source.revision,
    )
    .unwrap()
}

/// A build publishes a new revision of the R stack over the card's old
/// one.
fn refresh(state: &Arc<AppState>, cache: &FsPath, job_id: &str, revision: &str) -> MovedCards {
    publish(state, cache, job_id, revision, false)
}

/// As [`refresh`], from a build a person asked for.
fn rebuild(state: &Arc<AppState>, cache: &FsPath, job_id: &str, revision: &str) -> MovedCards {
    publish(state, cache, job_id, revision, true)
}

fn publish(
    state: &Arc<AppState>,
    cache: &FsPath,
    job_id: &str,
    revision: &str,
    personal: bool,
) -> MovedCards {
    let job = stack_on_disk(cache, job_id, revision);
    let moved = persist_latest_groups(cache, &job).unwrap();
    let ctx = state.get_database("test").unwrap();
    reapply::after_build(state, &ctx, 7, moved.clone(), personal);
    moved
}

/// Put a test entry in the line, dispatched like any other.
fn submit_test_entry(state: &Arc<AppState>, entry: crate::server::job_line::Entry) {
    ensure_dispatching(state);
    assert!(state.stack_previews.line.submit(entry, None).is_ok());
}

/// A build a person queued, waiting in the line. It never runs: the
/// test ends it.
fn queued_build(state: &AppState, job_id: &str) {
    let mut job = completed_job(job_id, vec![]);
    job.state = StackJobState::Queued;
    assert!(state.stack_previews.insert(job));
    state.stack_previews.hold_place(job_id, Vec::new(), false);
}

/// The re-apply's run in the activity list, once it is queued.
fn the_reapply_run(state: &AppState) -> Option<StackActivityEntry> {
    let run_id = state
        .stack_previews
        .reapplies
        .lock()
        .unwrap()
        .values()
        .find_map(|reapply| reapply.run_id.clone())?;
    state
        .stack_previews
        .active()
        .into_iter()
        .find(|entry| entry.job_id == run_id)
}

/// A server whose R card shows `1…` with a person's processing.
async fn processed_card(cache: &FsPath) -> (Arc<AppState>, String) {
    let state = state_on_disk(cache, None);
    let first = stack_on_disk(cache, &"1".repeat(64), "rev-1");
    persist_latest_groups(cache, &first).unwrap();
    let applied = apply_and_wait(&state, &"1".repeat(64), mtf_request(0.3)).await;
    (state, applied)
}

/// A color input reading the stack `job_id` at `revision`.
fn color_input(
    role: color::StackColorRole,
    job_id: &str,
    revision: &str,
) -> color::StackColorSource {
    color::StackColorSource {
        role,
        filter_name: "R".into(),
        label: "R".into(),
        exposure_group: None,
        job_id: job_id.into(),
        group_index: 0,
        artifact_revision: revision.into(),
        accepted_frames: 1,
        wbpp: false,
        total_exposure_seconds: None,
        reference_image_id: None,
        sky_orientation: None,
        registration_transform: None,
    }
}

fn deconvolved(fwhm: f32) -> stretch::StackViewProcessingRequest {
    let mut request = mtf_request(0.3);
    request.deconvolution = Some(seiza_deconvolution::DeconvolutionConfig::conservative(fwhm));
    request
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_color_build_takes_what_each_channel_card_applied() {
    use color::StackColorRole::{Green, Red};
    let _policy = reapply_policy(false);
    let cache = tempfile::tempdir().unwrap();
    let state = state_on_disk(cache.path(), None);
    let red = "1".repeat(64);
    let green = "2".repeat(64);
    persist_latest_groups(cache.path(), &stack_on_disk(cache.path(), &red, "rev-1")).unwrap();
    stack_on_disk(cache.path(), &green, "rev-g");
    apply_and_wait(&state, &red, deconvolved(2.0)).await;

    let inputs = [
        color_input(Red, &red, "rev-1"),
        color_input(Green, &green, "rev-g"),
    ];
    let cards = color::channel_card_processing(cache.path(), &inputs).unwrap();
    assert_eq!(
        cards[&Red].deconvolution,
        Some(seiza_deconvolution::DeconvolutionConfig::conservative(2.0))
    );
    assert_eq!(cards[&Red].rc_astro, None);
    // Green's card shows no processing: its input runs no tools.
    assert_eq!(cards[&Green], color::FollowedChannelProcessing::default());
    // Another revision of Red is another stack, with its own card.
    let other_revision = [color_input(Red, &red, "rev-0")];
    assert_eq!(
        color::channel_card_processing(cache.path(), &other_revision).unwrap()[&Red],
        color::FollowedChannelProcessing::default()
    );

    // The card applies other settings: what the build took no longer
    // matches, which makes it out of date.
    apply_and_wait(&state, &red, deconvolved(3.0)).await;
    assert_ne!(
        color::channel_card_processing(cache.path(), &inputs).unwrap(),
        cards
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_color_build_following_the_cards_records_them_and_goes_out_of_date_when_one_changes() {
    use color::StackColorRole::{Blue, Green, Red};
    let _policy = reapply_policy(false);
    let cache = tempfile::tempdir().unwrap();
    let state = state_on_disk(cache.path(), None);
    let (red, green, blue) = ("1".repeat(64), "2".repeat(64), "3".repeat(64));
    for (job_id, filter) in [(&red, "R"), (&green, "G"), (&blue, "B")] {
        let job = channel_stack_on_disk(cache.path(), job_id, filter, &format!("rev-{filter}"));
        persist_latest_groups(cache.path(), &job).unwrap();
    }
    apply_and_wait(&state, &red, deconvolved(2.0)).await;
    let ctx = state.get_database("test").unwrap();
    // The latest stacks are read against the project's exposure groups.
    {
        let db = ctx.db();
        let conn = db.lock().unwrap();
        crate::ts_schema::apply_schema(&conn).unwrap();
        conn.execute_batch(
            "INSERT INTO project(Id,name,profileId,guid) VALUES(7,'Project','profile','project-7');
            CREATE TABLE psf_guard_project_processing(project_key TEXT PRIMARY KEY,
                split_exposure_groups INTEGER NOT NULL,split_chosen INTEGER);
            INSERT INTO psf_guard_project_processing VALUES('guid:project-7',0,1);",
        )
        .unwrap();
    }
    let request: color::StackColorRequest = serde_json::from_value(serde_json::json!({
        "target_id": 42,
        "kind": "rgb",
        "processing": {
            "input_processing": "channels",
            "input_stretches": {},
            "output_stretches": [],
        },
    }))
    .unwrap();
    let prepare = || {
        color::prepare_color_job(&ctx, &state.astrometry, 7, &request)
            .unwrap()
            .public
    };

    // The build records what each card applied, including cards that
    // applied nothing, and runs Red's deconvolution.
    let built = prepare();
    let deconvolution = seiza_deconvolution::DeconvolutionConfig::conservative(2.0);
    assert_eq!(
        built.followed_channel_processing[&Red].deconvolution,
        Some(deconvolution)
    );
    assert_eq!(
        built.followed_channel_processing[&Green],
        color::FollowedChannelProcessing::default()
    );
    assert_eq!(
        built.followed_channel_processing[&Blue],
        color::FollowedChannelProcessing::default()
    );
    assert_eq!(
        built.processing.as_ref().unwrap().input_deconvolutions[&Red],
        deconvolution
    );

    // While the cards are as the build read them, it stays current.
    color::write_placeholder_artifacts(cache.path(), &built.job_id);
    assert_eq!(
        color::outdated_reason(&ctx, &state.astrometry, &built).unwrap(),
        None
    );

    // Red's card applies other settings: the build is out of date, and the
    // next build is another job.
    apply_and_wait(&state, &red, deconvolved(3.0)).await;
    assert_eq!(
        color::outdated_reason(&ctx, &state.astrometry, &built)
            .unwrap()
            .as_deref(),
        Some("a channel card's deconvolution or RC-Astro steps changed")
    );
    assert_ne!(prepare().job_id, built.job_id);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refreshed_stack_shows_its_carried_processing_to_color_builds() {
    let _policy = reapply_policy(true);
    let cache = tempfile::tempdir().unwrap();
    let state = state_on_disk(cache.path(), None);
    let first = "1".repeat(64);
    persist_latest_groups(cache.path(), &stack_on_disk(cache.path(), &first, "rev-1")).unwrap();
    apply_and_wait(&state, &first, deconvolved(2.0)).await;
    let second = "2".repeat(64);
    refresh(&state, cache.path(), &second, "rev-2");

    // Whether the re-apply has run yet or not, the new stack's card shows
    // the same settings, so a color build of it takes them.
    let inputs = [color_input(color::StackColorRole::Red, &second, "rev-2")];
    let cards = color::channel_card_processing(cache.path(), &inputs).unwrap();
    assert_eq!(
        cards[&color::StackColorRole::Red].deconvolution,
        Some(seiza_deconvolution::DeconvolutionConfig::conservative(2.0))
    );
}

fn the_automatic_run(state: &AppState) -> Option<StackActivityEntry> {
    state
        .stack_previews
        .active()
        .into_iter()
        .find(|entry| entry.kind == StackActivityKind::Processing && entry.automatic)
}

fn model_of(preview: &stretch::StackStretchPreview) -> serde_json::Value {
    serde_json::to_value(&preview.request.as_ref().unwrap().stretch.model).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refreshed_stack_gets_its_processing_again_and_shows_the_old_meanwhile() {
    let _policy = reapply_policy(true);
    let cache = tempfile::tempdir().unwrap();
    let (state, applied) = processed_card(cache.path()).await;
    let second = "2".repeat(64);
    refresh(&state, cache.path(), &second, "rev-2");

    // Meanwhile the new card shows the previous result, marked.
    let carried = shown(&state, &second).unwrap();
    assert_eq!(carried.stretch_id, applied);
    let reprocessing = carried.reprocessing.unwrap();
    assert_eq!(reprocessing.from_revision, "rev-1");
    assert_ne!(reprocessing.state, stretch::ReprocessingState::Failed);

    // Then its own, from the same request, with no browser asking.
    eventually("the re-apply lands", || {
        shown(&state, &second).is_some_and(|shown| shown.reprocessing.is_none())
    })
    .await;
    let own = shown(&state, &second).unwrap();
    assert_ne!(own.stretch_id, applied);
    assert_eq!(
        model_of(&own),
        serde_json::to_value(mtf_request(0.3).stretch.model).unwrap()
    );
    // The card shows the result a moment before the run leaves the
    // re-apply ledger, so wait for the count rather than read it once.
    eventually("the re-apply ledger empties", || {
        state.stack_previews.reapply_count() == 0
    })
    .await;
    // The selection carried: a reload of the card restores it, and the
    // old card keeps its own.
    let source = r_source(&state, &second);
    let ctx = state.get_database("test").unwrap();
    assert_eq!(
        stretch::selected_processing(&ctx.stack_root, &source.key, &source.revision)
            .unwrap()
            .map(|selected| selected.stretch_id),
        Some(own.stretch_id.clone())
    );
    assert_eq!(shown(&state, &"1".repeat(64)).unwrap().stretch_id, applied);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_newer_stack_drops_the_stale_reapply_and_carries_on() {
    let _policy = reapply_policy(true);
    let cache = tempfile::tempdir().unwrap();
    let (state, applied) = processed_card(cache.path()).await;
    // A build holds the worker, so the re-apply waits in the line.
    let busy = state.stack_previews.line.reserve_main().await;
    let second = "2".repeat(64);
    refresh(&state, cache.path(), &second, "rev-2");
    eventually("the re-apply is queued", || {
        the_automatic_run(&state).is_some()
    })
    .await;
    let stale = the_automatic_run(&state).unwrap();
    assert_eq!(stale.state, StackJobState::Queued);
    assert_eq!(stale.label, "Target 42 · R");
    assert_eq!(
        shown(&state, &second).unwrap().reprocessing.unwrap().state,
        stretch::ReprocessingState::Queued
    );

    // A newer stack lands while it waits: only the newest is processed.
    let third = "3".repeat(64);
    refresh(&state, cache.path(), &third, "rev-3");
    eventually("the stale run leaves the line", || {
        !state
            .stack_previews
            .active()
            .iter()
            .any(|entry| entry.job_id == stale.job_id)
    })
    .await;
    assert!(
        shown(&state, &second).is_none(),
        "the stale card carries nothing"
    );
    let carried = shown(&state, &third).unwrap();
    assert_eq!(carried.stretch_id, applied, "the first result carries on");
    drop(busy);
    eventually("the newest re-apply lands", || {
        shown(&state, &third).is_some_and(|shown| shown.reprocessing.is_none())
    })
    .await;
    // The card shows the result a moment before the run leaves the
    // re-apply ledger, so wait for the count rather than read it once.
    eventually("the re-apply ledger empties", || {
        state.stack_previews.reapply_count() == 0
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn turning_reapplies_off_drops_the_waiting_one_and_queues_no_more() {
    let _policy = reapply_policy(true);
    let cache = tempfile::tempdir().unwrap();
    let (state, _) = processed_card(cache.path()).await;
    // A person's work keeps the re-apply waiting.
    let person = state.begin_interactive_job();
    let second = "2".repeat(64);
    refresh(&state, cache.path(), &second, "rev-2");
    assert_eq!(state.stack_previews.reapply_count(), 1);
    assert!(shown(&state, &second).is_some());

    // As the settings route does.
    reapply::configure(false);
    reapply::turned_off(&state);
    assert_eq!(state.stack_previews.reapply_count(), 0);
    assert!(shown(&state, &second).is_none(), "nothing carried shows");
    refresh(&state, cache.path(), &"3".repeat(64), "rev-3");
    assert_eq!(state.stack_previews.reapply_count(), 0);
    assert!(shown(&state, &"3".repeat(64)).is_none());
    drop(person);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_persons_own_apply_replaces_the_reapply() {
    let _policy = reapply_policy(true);
    let cache = tempfile::tempdir().unwrap();
    let (state, _) = processed_card(cache.path()).await;
    let busy = state.stack_previews.line.reserve_main().await;
    let second = "2".repeat(64);
    refresh(&state, cache.path(), &second, "rev-2");
    eventually("the re-apply is queued", || {
        the_automatic_run(&state).is_some()
    })
    .await;
    let automatic = the_automatic_run(&state).unwrap();

    // Other settings: the re-apply stops and theirs runs.
    let ctx = state.get_database("test").unwrap();
    let theirs = stretch::apply_to_fits(
        Arc::clone(&state),
        ctx.id.clone(),
        ctx.stack_root.clone(),
        r_source(&state, &second),
        mtf_request(0.6),
        true,
    )
    .await
    .unwrap();
    let stretch::StretchApplyOutcome::Pending { stretch_id: theirs } = theirs else {
        panic!("the worker is busy");
    };
    eventually("the re-apply leaves the line", || {
        !state
            .stack_previews
            .active()
            .iter()
            .any(|entry| entry.job_id == automatic.job_id)
    })
    .await;
    // The card shows the result a moment before the run leaves the
    // re-apply ledger, so wait for the count rather than read it once.
    eventually("the re-apply ledger empties", || {
        state.stack_previews.reapply_count() == 0
    })
    .await;
    assert!(
        shown(&state, &second).is_none(),
        "their own result replaces the carried one once it lands"
    );
    drop(busy);
    eventually("their processing lands", || {
        shown(&state, &second).is_some_and(|shown| shown.stretch_id == theirs)
    })
    .await;
    assert_eq!(
        model_of(&shown(&state, &second).unwrap()),
        serde_json::to_value(mtf_request(0.6).stretch.model).unwrap()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_same_request_takes_the_queued_reapply_over() {
    let _policy = reapply_policy(true);
    let cache = tempfile::tempdir().unwrap();
    let (state, _) = processed_card(cache.path()).await;
    let busy = state.stack_previews.line.reserve_main().await;
    let second = "2".repeat(64);
    refresh(&state, cache.path(), &second, "rev-2");
    eventually("the re-apply is queued", || {
        the_automatic_run(&state).is_some()
    })
    .await;
    let automatic = the_automatic_run(&state).unwrap();
    let ctx = state.get_database("test").unwrap();
    let theirs = stretch::apply_to_fits(
        Arc::clone(&state),
        ctx.id.clone(),
        ctx.stack_root.clone(),
        r_source(&state, &second),
        mtf_request(0.3),
        true,
    )
    .await
    .unwrap();
    assert!(matches!(
        theirs,
        stretch::StretchApplyOutcome::Pending { stretch_id } if stretch_id == automatic.job_id
    ));
    let adopted = state
        .stack_previews
        .active()
        .into_iter()
        .find(|entry| entry.job_id == automatic.job_id)
        .unwrap();
    assert!(!adopted.automatic, "the run is theirs now");
    drop(busy);
    eventually("it lands", || {
        shown(&state, &second).is_some_and(|shown| shown.stretch_id == automatic.job_id)
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_reapply_keeps_the_old_result_and_is_not_tried_again() {
    let _policy = reapply_policy(true);
    let cache = tempfile::tempdir().unwrap();
    let (state, applied) = processed_card(cache.path()).await;
    // The new stack's pixels cannot be read.
    let second = "2".repeat(64);
    stack_on_disk(cache.path(), &second, "rev-2");
    std::fs::write(fits_path(cache.path(), &second, 0), b"not a FITS file").unwrap();
    let moved = refresh(&state, cache.path(), &second, "rev-2");
    eventually("the re-apply fails", || {
        shown(&state, &second).is_some_and(|shown| {
            shown.reprocessing.as_ref().is_some_and(|reprocessing| {
                reprocessing.state == stretch::ReprocessingState::Failed
            })
        })
    })
    .await;
    let failed = shown(&state, &second).unwrap();
    assert_eq!(failed.stretch_id, applied, "the previous result stays");
    assert!(failed.reprocessing.unwrap().error.is_some());
    // The card shows the result a moment before the run leaves the
    // re-apply ledger, so wait for the count rather than read it once.
    eventually("the re-apply ledger empties", || {
        state.stack_previews.reapply_count() == 0
    })
    .await;

    // The card moving to that stack again starts nothing.
    let ctx = state.get_database("test").unwrap();
    reapply::after_build(&state, &ctx, 7, moved, false);
    assert_eq!(state.stack_previews.reapply_count(), 0);
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    assert!(the_automatic_run(&state).is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_queued_reapply_waits_in_line_behind_a_persons_work() {
    let _policy = reapply_policy(true);
    let cache = tempfile::tempdir().unwrap();
    let (state, _) = processed_card(cache.path()).await;
    let busy = state.stack_previews.line.reserve_main().await;
    let second = "2".repeat(64);
    refresh(&state, cache.path(), &second, "rev-2");
    eventually("the re-apply is queued", || {
        the_automatic_run(&state).is_some()
    })
    .await;
    // A person starts work: the queued re-apply keeps its place, behind
    // the person's work.
    interrupt_automatic(&state);
    let person = crate::server::job_line::testing::entry("person", Lane::Main, Stop::new());
    assert!(state.stack_previews.line.submit(person, None).is_ok());
    let run = the_automatic_run(&state).expect("it left the line");
    assert_eq!(state.stack_previews.line.order()[0], "person");
    assert_eq!(run.state, StackJobState::Queued);
    state.stack_previews.line.remove("person");
    drop(busy);
    eventually("it runs once the worker is free", || {
        shown(&state, &second).is_some_and(|shown| shown.reprocessing.is_none())
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_persons_rebuild_reapplies_next_ahead_of_their_other_builds() {
    let _policy = reapply_policy(true);
    let cache = tempfile::tempdir().unwrap();
    let (state, _) = processed_card(cache.path()).await;
    // The person's next build holds the worker, and one more waits.
    let busy = state.stack_previews.line.reserve_main().await;
    let person = state.begin_interactive_job();
    queued_build(&state, "later");
    let second = "2".repeat(64);
    rebuild(&state, cache.path(), &second, "rev-2");

    eventually("the re-apply is queued", || {
        the_reapply_run(&state).is_some()
    })
    .await;
    let run = the_reapply_run(&state).unwrap();
    assert!(!run.automatic, "it is the person's work");
    assert_eq!(run.state, StackJobState::Queued);
    assert_eq!(run.queue_position, Some(0), "it runs next");
    assert_eq!(state.stack_previews.line_position("later"), Some(1));
    assert!(state.stack_previews.journal_entries().iter().any(|origin| {
        matches!(origin, journal::JournaledStackJob::Processing { run_id, automatic: false, .. }
            if run_id == &run.job_id)
    }));
    // Work a person starts does not push it aside.
    interrupt_automatic(&state);
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert_eq!(
        the_reapply_run(&state).map(|entry| entry.job_id),
        Some(run.job_id.clone())
    );

    // The build in hand ends: the re-apply runs before the one waiting.
    drop(person);
    drop(busy);
    eventually("the re-apply lands", || {
        shown(&state, &second).is_some_and(|shown| shown.reprocessing.is_none())
    })
    .await;
    assert_eq!(state.stack_previews.line_position("later"), Some(0));
    state.stack_previews.release_place("later");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_persons_reapply_takes_the_worker_before_their_next_build() {
    let _policy = reapply_policy(true);
    let cache = tempfile::tempdir().unwrap();
    let (state, _) = processed_card(cache.path()).await;
    // The person's build holds the worker, and their next one waits for
    // its turn.
    let busy = state.stack_previews.line.reserve_main().await;
    let cancel = state.stack_previews.track_job("later");
    let (took, took_worker) = tokio::sync::oneshot::channel();
    let (release, released) = std::sync::mpsc::channel::<()>();
    let (done, later) = tokio::sync::oneshot::channel::<()>();
    submit_test_entry(
        &state,
        crate::server::job_line::testing::entry_running("later", Lane::Main, cancel, move || {
            let _ = took.send(());
            let _ = released.recv();
            let _ = done.send(());
        }),
    );
    tokio::task::yield_now().await;

    // The build publishes and lets go of the worker at once.
    let second = "2".repeat(64);
    rebuild(&state, cache.path(), &second, "rev-2");
    drop(busy);
    took_worker.await.unwrap();
    // The next build holds the worker now, so the re-apply had it first.
    eventually("the re-apply lands before the next build", || {
        shown(&state, &second).is_some_and(|shown| shown.reprocessing.is_none())
    })
    .await;
    release.send(()).unwrap();
    later.await.unwrap();
    assert!(state.stack_previews.line.order().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_automatic_reapply_waits_in_line_for_builds_of_its_target() {
    let _policy = reapply_policy(true);
    let cache = tempfile::tempdir().unwrap();
    let (state, _) = processed_card(cache.path()).await;
    // A build of the same target waits in the line, holding its key.
    state
        .stack_previews
        .hold_place("later", vec![channel_key("test", 7, 42)], false);
    let second = "2".repeat(64);
    refresh(&state, cache.path(), &second, "rev-2");
    tokio::time::sleep(std::time::Duration::from_secs(4)).await;
    reapply::start_due(Arc::clone(&state)).await;
    // It joins the line as automatic work, and waits for the build.
    let run = the_reapply_run(&state).expect("the re-apply is in the line");
    assert!(run.automatic, "it is still background work");
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert_eq!(
        the_reapply_run(&state).unwrap().state,
        StackJobState::Queued,
        "it ran before the build of its target"
    );
    state.stack_previews.release_place("later");
    eventually("it runs once the build leaves the line", || {
        shown(&state, &second).is_some_and(|shown| shown.reprocessing.is_none())
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_persons_reapply_stays_theirs_after_a_restart() {
    let _policy = reapply_policy(true);
    let cache = tempfile::tempdir().unwrap();
    let (before, _) = processed_card(cache.path()).await;
    let busy = before.stack_previews.line.reserve_main().await;
    let second = "2".repeat(64);
    rebuild(&before, cache.path(), &second, "rev-2");
    eventually("the re-apply is queued", || {
        the_reapply_run(&before).is_some()
    })
    .await;
    let queued = the_reapply_run(&before).unwrap().job_id;
    let origin = before
        .stack_previews
        .journal_entries()
        .into_iter()
        .find(|origin| {
            matches!(origin, journal::JournaledStackJob::Processing { run_id, .. }
                if run_id == &queued)
        })
        .unwrap();
    assert!(!origin.automatic(), "it is journaled as the person's");
    // The server stops while it waits.
    before.shutdown.begin();
    before.stack_previews.stop_for_shutdown();
    tokio::time::timeout(std::time::Duration::from_secs(5), before.shutdown.drained())
        .await
        .unwrap();
    drop(busy);

    // The journal brings the run back, still theirs and still the
    // re-apply.
    let after = state_on_disk(cache.path(), None);
    assert!(stretch::restore_run(&after, origin).await.is_ok());
    let reapply = after
        .stack_previews
        .reapplies
        .lock()
        .unwrap()
        .values()
        .next()
        .cloned()
        .unwrap();
    assert!(reapply.personal);
    let run_id = reapply.run_id.unwrap();
    assert!(!after.stack_previews.is_automatic_processing(&run_id));
    eventually("it lands", || {
        shown(&after, &second).is_some_and(|shown| shown.reprocessing.is_none())
    })
    .await;
    eventually("the re-apply ledger empties", || {
        after.stack_previews.reapply_count() == 0
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_persons_waiting_reapply_stays_theirs_after_a_restart() {
    let _policy = reapply_policy(true);
    let cache = tempfile::tempdir().unwrap();
    let (before, _) = processed_card(cache.path()).await;
    // The server is stopping as the build lands, so its turn never comes.
    before.shutdown.begin();
    let second = "2".repeat(64);
    rebuild(&before, cache.path(), &second, "rev-2");
    assert_eq!(before.stack_previews.reapply_count(), 1);

    let after = state_on_disk(cache.path(), None);
    reapply::recover(&after);
    let reapply = after
        .stack_previews
        .reapplies
        .lock()
        .unwrap()
        .values()
        .next()
        .cloned()
        .unwrap();
    assert!(reapply.personal, "the selection remembers whose it is");
    eventually("it runs on the restarted server", || {
        shown(&after, &second).is_some_and(|shown| shown.reprocessing.is_none())
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reapply_turned_off_while_the_server_was_down_does_not_come_back() {
    let policy = reapply_policy(true);
    let cache = tempfile::tempdir().unwrap();
    let (before, applied) = processed_card(cache.path()).await;
    // The server is stopping as the build lands, so its turn never comes.
    before.shutdown.begin();
    let second = "2".repeat(64);
    rebuild(&before, cache.path(), &second, "rev-2");
    assert_eq!(shown(&before, &second).unwrap().stretch_id, applied);

    // It starts again with re-applying off.
    reapply::configure(false);
    let after = state_on_disk(cache.path(), None);
    reapply::recover(&after);
    assert_eq!(after.stack_previews.reapply_count(), 0);
    assert!(after.stack_previews.line.order().is_empty());
    assert!(shown(&after, &second).is_none(), "nothing carried shows");
    drop(policy);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_waiting_reapply_comes_back_after_a_restart() {
    let _policy = reapply_policy(true);
    let cache = tempfile::tempdir().unwrap();
    let (before, _) = processed_card(cache.path()).await;
    let person = before.begin_interactive_job();
    let second = "2".repeat(64);
    refresh(&before, cache.path(), &second, "rev-2");
    assert_eq!(before.stack_previews.reapply_count(), 1);
    // The server stops before its turn comes.
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    assert_eq!(before.stack_previews.reapply_count(), 1);

    let after = state_on_disk(cache.path(), None);
    reapply::recover(&after);
    assert_eq!(after.stack_previews.reapply_count(), 1);
    eventually("it runs on the restarted server", || {
        shown(&after, &second).is_some_and(|shown| shown.reprocessing.is_none())
    })
    .await;
    drop(person);
}

/// A server whose R card was refreshed to `2…`, with the re-apply still
/// waiting because a person's work holds it off.
async fn waiting_reapply(
    cache: &FsPath,
) -> (
    Arc<AppState>,
    String,
    crate::server::state::InteractiveJobGuard,
) {
    let (state, applied) = processed_card(cache).await;
    let person = state.begin_interactive_job();
    refresh(&state, cache, &"2".repeat(64), "rev-2");
    assert_eq!(state.stack_previews.reapply_count(), 1);
    (state, applied, person)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_activity_endpoint_lists_a_reapply_waiting_for_a_persons_work() {
    let _policy = reapply_policy(true);
    let cache = tempfile::tempdir().unwrap();
    let (state, _) = processed_card(cache.path()).await;
    let person = state.begin_named_interactive_job("a color composition");
    refresh(&state, cache.path(), &"2".repeat(64), "rev-2");

    let Json(response) = get_stack_activity(State(Arc::clone(&state))).await;
    let json = serde_json::to_value(response).unwrap();
    assert_eq!(
        json["data"]["waiting_reapplies"],
        serde_json::json!([{
            "database_id": "test",
            "project_id": 7,
            "target_id": 42,
            "label": "Target 42 · R",
            "waiting_for": "a color composition",
        }]),
        "{json}"
    );
    drop(person);
    // Once it runs, it is a processing run in the line, not waiting.
    eventually("the re-apply runs", || {
        state.stack_previews.waiting_reapplies().is_empty()
    })
    .await;
}

#[test]
fn a_stopped_task_finishing_late_keeps_the_new_jobs_stop_flag() {
    let manager = StackPreviewManager::new();
    let stopped = manager.track_job("same");
    // A resumed refresh asks for the same job id.
    let resumed = manager.track_job("same");
    manager.forget_job_of("same", &stopped);
    assert!(manager.request_cancel("same"), "the new flag was forgotten");
    assert!(resumed.requested());
    assert!(!stopped.requested());
    manager.forget_job_of("same", &resumed);
    assert!(!manager.request_cancel("same"));
}

#[test]
fn a_reapply_whose_run_joined_the_line_is_listed_once() {
    let waiting = |label: &str, personal: bool| WaitingReapplyView {
        database_id: "db".into(),
        project_id: 1,
        target_id: Some(42),
        label: label.into(),
        personal,
        waiting_for: None,
    };
    let mut run = mono_activity(&running_build("run"));
    run.kind = StackActivityKind::Processing;
    run.database_id = "db".into();
    run.project_id = 1;
    run.label = "M44 · R".into();
    run.automatic = true;
    let labels = |listed: Vec<WaitingReapplyView>| {
        listed
            .into_iter()
            .map(|view| view.label)
            .collect::<Vec<_>>()
    };
    let listed = reapplies_not_started(
        vec![waiting("M44 · R", false), waiting("M44 · G", false)],
        std::slice::from_ref(&run),
    );
    assert_eq!(labels(listed), ["M44 · G"]);
    // A person's re-apply runs as their work, not automatic.
    run.automatic = false;
    let listed = reapplies_not_started(
        vec![waiting("M44 · R", true), waiting("M44 · G", true)],
        std::slice::from_ref(&run),
    );
    assert_eq!(labels(listed), ["M44 · G"]);
}

#[test]
fn a_waiting_refresh_whose_build_joined_the_line_is_listed_once() {
    let refresh = |project_id: Option<i32>, waiting: bool| ScheduledRefreshView {
        database_id: "db".into(),
        database_name: "Askar".into(),
        project_id,
        project_name: None,
        reason: automatic::RefreshReason::Arrival,
        due_in_seconds: 0,
        channels: vec!["M44 · R".into()],
        waiting,
        waiting_for: None,
    };
    let mut build = mono_activity(&running_build("auto"));
    build.database_id = "db".into();
    build.project_id = 1;
    let scheduled = vec![
        refresh(Some(1), true),
        refresh(Some(2), true),
        refresh(Some(1), false),
    ];
    // A person's build of project 1 does not stand for the refresh.
    let listed = without_started(scheduled.clone(), std::slice::from_ref(&build));
    assert_eq!(listed.len(), 3);
    build.automatic = true;
    let listed = without_started(scheduled, std::slice::from_ref(&build));
    assert_eq!(
        listed
            .iter()
            .map(|refresh| (refresh.project_id, refresh.waiting))
            .collect::<Vec<_>>(),
        [(Some(2), true), (Some(1), false)]
    );
}

/// The re-apply's start, coming back from probing the tools late.
async fn start_reapply_again(
    state: &Arc<AppState>,
    request: stretch::StackViewProcessingRequest,
    still_wanted: bool,
) -> Result<stretch::ReapplyStart, String> {
    let ctx = state.get_database("test").unwrap();
    stretch::start_reapply(
        state,
        &ctx,
        &"2".repeat(64),
        0,
        "rev-2",
        request,
        false,
        &move || still_wanted,
    )
    .await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reapply_that_crash_loops_the_server_is_given_up_for_good() {
    let _policy = reapply_policy(true);
    let cache = tempfile::tempdir().unwrap();
    let registry_dir = tempfile::tempdir().unwrap();
    let registry = registry_dir.path().join("reg.json");
    let (before, _, person) = waiting_reapply(cache.path()).await;
    // The journal from the last crash: the re-apply was running, for the
    // third restart in a row.
    let source = r_source(&before, &"2".repeat(64));
    let entry = journal::JournaledStackJob::Processing {
        database_id: "test".into(),
        project_id: 7,
        job_id: source.job_id.clone(),
        group_index: 0,
        revision: "rev-2".into(),
        run_id: "c".repeat(64),
        request: Box::new(mtf_request(0.3)),
        automatic: true,
        attempts: 2,
        running: true,
    };
    stretch::write_json_atomic(
        &journal::journal_path(&registry),
        &serde_json::json!({"schema_version": 2, "stack": [entry]}),
    )
    .unwrap();
    drop(person);

    let after = state_on_disk(cache.path(), Some(&registry));
    journal::restore(&after).await;
    assert_eq!(
        after.stack_previews.reapply_count(),
        0,
        "neither the journal nor the remembered carry brings it back"
    );
    let failed = shown(&after, &"2".repeat(64))
        .unwrap()
        .reprocessing
        .unwrap();
    assert_eq!(failed.state, stretch::ReprocessingState::Failed);
    assert!(failed.error.unwrap().contains("restarts in a row"));
    // Another restart finds nothing to revive either.
    reapply::recover(&after);
    assert_eq!(after.stack_previews.reapply_count(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reapply_starting_late_keeps_a_persons_apply() {
    let _policy = reapply_policy(true);
    let cache = tempfile::tempdir().unwrap();
    let (state, _, person) = waiting_reapply(cache.path()).await;
    // While the re-apply probes the tools, the person applies their own.
    drop(person);
    let theirs = apply_and_wait(&state, &"2".repeat(64), mtf_request(0.6)).await;
    assert_eq!(
        start_reapply_again(&state, mtf_request(0.3), true).await,
        Ok(stretch::ReapplyStart::Withdrawn)
    );
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    assert!(the_automatic_run(&state).is_none());
    let shown_now = shown(&state, &"2".repeat(64)).unwrap();
    assert_eq!(
        shown_now.stretch_id, theirs,
        "their processing stays selected"
    );
    assert!(shown_now.reprocessing.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reapply_starting_late_keeps_a_persons_revert() {
    let _policy = reapply_policy(true);
    let cache = tempfile::tempdir().unwrap();
    let (state, _, _person) = waiting_reapply(cache.path()).await;
    // The person reverts, as the DELETE route does.
    let ctx = state.get_database("test").unwrap();
    let source = r_source(&state, &"2".repeat(64));
    reapply::replaced_by_person(&state, &source, None);
    stretch::clear_selected_processing(&ctx.stack_root, &source.key, &source.revision).unwrap();
    assert_eq!(
        start_reapply_again(&state, mtf_request(0.3), true).await,
        Ok(stretch::ReapplyStart::Withdrawn)
    );
    assert!(the_automatic_run(&state).is_none());
    assert!(
        shown(&state, &"2".repeat(64)).is_none(),
        "the revert stands"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reapply_no_longer_wanted_queues_nothing() {
    let _policy = reapply_policy(true);
    let cache = tempfile::tempdir().unwrap();
    let (state, _, _person) = waiting_reapply(cache.path()).await;
    // A newer stack dropped it while it probed the tools.
    assert_eq!(
        start_reapply_again(&state, mtf_request(0.3), false).await,
        Ok(stretch::ReapplyStart::Withdrawn)
    );
    assert!(the_automatic_run(&state).is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refresh_build_goes_ahead_of_a_reapply_holding_the_worker() {
    let _policy = reapply_policy(true);
    let cache = tempfile::tempdir().unwrap();
    let (state, _) = processed_card(cache.path()).await;
    let busy = state.stack_previews.line.reserve_main().await;
    refresh(&state, cache.path(), &"2".repeat(64), "rev-2");
    eventually("the re-apply is queued", || {
        the_automatic_run(&state).is_some()
    })
    .await;
    let run = the_automatic_run(&state).unwrap();
    // A refresh of another project leaves it be.
    state
        .stack_previews
        .yield_automatic_processing(&run.database_id, run.project_id + 1);
    assert!(the_automatic_run(&state).is_some());
    // The refresh's build goes ahead; the re-apply waits for its turn.
    state
        .stack_previews
        .yield_automatic_processing(&run.database_id, run.project_id);
    eventually("the re-apply steps aside", || {
        the_automatic_run(&state).is_none()
    })
    .await;
    assert_eq!(state.stack_previews.reapply_count(), 1);
    assert_eq!(
        shown(&state, &"2".repeat(64))
            .unwrap()
            .reprocessing
            .unwrap()
            .state,
        stretch::ReprocessingState::Waiting
    );
    drop(busy);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reapply_yielding_after_the_setting_went_off_stops_waiting() {
    let _policy = reapply_policy(true);
    let cache = tempfile::tempdir().unwrap();
    let (state, _) = processed_card(cache.path()).await;
    let busy = state.stack_previews.line.reserve_main().await;
    refresh(&state, cache.path(), &"2".repeat(64), "rev-2");
    eventually("the re-apply is queued", || {
        the_automatic_run(&state).is_some()
    })
    .await;
    // Switched off while it waits, which leaves it be; then it steps
    // aside for a refresh's build.
    reapply::configure(false);
    let run = the_automatic_run(&state).unwrap();
    state
        .stack_previews
        .yield_automatic_processing(&run.database_id, run.project_id);
    eventually("it settles", || state.stack_previews.reapply_count() == 0).await;
    assert!(
        shown(&state, &"2".repeat(64)).is_none(),
        "the card no longer says it is reprocessing"
    );
    drop(busy);
}

#[test]
fn a_preview_rendered_from_the_fits_matches_the_one_the_build_rendered() {
    let cache = tempfile::tempdir().unwrap();
    for channels in [1, 3] {
        let (width, height) = (96, 64);
        let data = (0..width * height * channels)
            .map(|index| {
                let pixel = index / channels;
                let (x, y) = (pixel % width, pixel / width);
                let star = if (x * 7 + y * 3) % 211 == 0 {
                    9000.0
                } else {
                    0.0
                };
                800.0 + (x as f32) * 1.5 + (y as f32) * 0.75 + star + (index % 13) as f32
            })
            .collect();
        let image = seiza_stacking::LinearImage::new(width, height, channels, data).unwrap();
        let directory = cache.path().join(format!("channels-{channels}"));
        std::fs::create_dir_all(&directory).unwrap();
        let fits = directory.join("group-0.fits");
        // The build copies the reference frame's headers, which for a
        // one-shot-color camera name its Bayer pattern.
        let headers = [
            (
                "BAYERPAT".to_string(),
                seiza_fits::HeaderValue::String("RGGB".into()),
            ),
            ("EXPTIME".to_string(), seiza_fits::HeaderValue::Float(120.0)),
        ];
        seiza_stacking::write_linear_image_fits_f32(&fits, &image, &headers, &[]).unwrap();
        let render = |image: &seiza_stacking::LinearImage, name: &str| {
            let screen = directory.join(format!("{name}.png"));
            let original = directory.join(format!("{name}-original.png"));
            stretch::render_image_previews_atomic(
                image,
                &stretch::default_linear_config(),
                stretch::StackStretchSourceTransfer::Linear,
                &screen,
                &original,
            )
            .unwrap();
            (
                std::fs::read(screen).unwrap(),
                std::fs::read(original).unwrap(),
            )
        };
        let built = render(&image, "built");
        let reread = crate::image_io::open_linear_frame(&fits).unwrap();
        assert_eq!(reread.image.channels, channels);
        let rendered = render(&reread.image, "rendered");
        assert!(
            built == rendered,
            "{channels}-channel preview differs from the FITS"
        );
    }
}

#[test]
fn only_a_finished_stack_with_an_older_preview_renders_again() {
    let cache = tempfile::tempdir().unwrap();
    let state = state_on_disk(cache.path(), None);
    let ctx = state.get_database("test").unwrap();
    let job_id = "3".repeat(64);
    let job = stack_on_disk(cache.path(), &job_id, "rev-1");
    let mut group = job.groups[0].clone();
    // A manifest from before the version was recorded rendered at the
    // version its id was built with.
    let renders_old = JOB_ID_STRETCH_VERSION != stretch::SEIZA_STRETCH_OUTPUT_VERSION;
    assert_eq!(
        preview_needs_render(&state, &ctx, &job_id, &group),
        renders_old
    );
    group.preview_stretch_version = Some(stretch::SEIZA_STRETCH_OUTPUT_VERSION.into());
    assert!(!preview_needs_render(&state, &ctx, &job_id, &group));
    group.preview_stretch_version = Some("0.2.2".into());
    assert!(preview_needs_render(&state, &ctx, &job_id, &group));
    // A render that failed at this release is not tried again.
    group.preview_render_failed = Some(stretch::SEIZA_STRETCH_OUTPUT_VERSION.into());
    assert!(!preview_needs_render(&state, &ctx, &job_id, &group));
    group.preview_render_failed = Some("0.2.2".into());
    assert!(preview_needs_render(&state, &ctx, &job_id, &group));
    // Nothing to render from without a finished channel and its FITS.
    group.state = StackGroupState::Error;
    assert!(!preview_needs_render(&state, &ctx, &job_id, &group));
    group.state = StackGroupState::Ready;
    assert!(!preview_needs_render(&state, &ctx, &"4".repeat(64), &group));
}

/// Project 1 of a catalog with four Ha frames in two exposure groups,
/// registered on a fresh server state.
fn state_with_frames(directory: &FsPath) -> (Arc<AppState>, Arc<DatabaseContext>) {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    crate::ts_schema::apply_schema(&conn).unwrap();
    conn.execute_batch("INSERT INTO project(Id,name,profileId,guid) VALUES(1,'Project','profile','project-one');
        INSERT INTO target(Id,name,projectId,active,ra,dec,epochcode,rotation,roi,guid)
            VALUES(1,'Target',1,1,10,20,0,0,100,'target-one');
        CREATE TABLE psf_guard_project_processing(project_key TEXT PRIMARY KEY,split_exposure_groups INTEGER NOT NULL,split_chosen INTEGER);
        INSERT INTO psf_guard_project_processing VALUES('guid:project-one',1,1);").unwrap();
    for (id, exposure) in [(1, 10.0), (2, 11.0), (3, 300.0), (4, 310.0)] {
        conn.execute("INSERT INTO acquiredimage(Id,projectId,targetId,gradingStatus,metadata,acquireddate,filtername)
            VALUES(?1,1,1,1,?2,?1,'Ha')", rusqlite::params![id, serde_json::json!({"ExposureDuration":exposure}).to_string()]).unwrap();
    }
    let mut ctx = DatabaseContext::new_for_test(conn);
    ctx.use_storage_for_test(directory);
    let ctx = Arc::new(ctx);
    let state = test_state();
    state
        .databases
        .write()
        .unwrap()
        .insert(ctx.id.clone(), Arc::clone(&ctx));
    (state, ctx)
}

fn build_request(image_ids: Vec<i32>) -> StackPreviewRequest {
    serde_json::from_value(serde_json::json!({"image_ids": image_ids, "calibration": "off"}))
        .unwrap()
}

/// The build once its preparation has ended.
async fn prepared(state: &AppState, job_id: &str) -> StackPreviewJob {
    for _ in 0..500 {
        if let Some(job) = state.stack_previews.get(job_id)
            && job.state != StackJobState::Preparing
        {
            return job;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    panic!("the build was never prepared");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_build_joins_the_line_before_it_is_prepared_then_becomes_its_channels() {
    let directory = tempfile::tempdir().unwrap();
    let (state, ctx) = state_with_frames(directory.path());
    // The catalog is busy, so preparing cannot read it yet: queueing
    // still answers at once.
    let db = ctx.db();
    let busy = db.lock().unwrap();
    let job = build_set::queue_preparation(&state, &ctx, 1, build_request(vec![1, 2, 3, 4]), true)
        .unwrap();
    assert_eq!(job.state, StackJobState::Preparing);
    assert_eq!(job.preparing.as_deref(), Some("4 frames"));
    let listed = state.stack_previews.active();
    let entry = listed
        .iter()
        .find(|entry| entry.job_id == job.job_id)
        .unwrap();
    assert_eq!(entry.label, "4 frames");
    assert_eq!(entry.detail, "Reading frames and matching calibration");
    // The same click again is the same build.
    let again =
        build_set::queue_preparation(&state, &ctx, 1, build_request(vec![4, 3, 2, 1]), true)
            .unwrap();
    assert_eq!(again.job_id, job.job_id);
    drop(busy);

    let done = prepared(&state, &job.job_id).await;
    assert_eq!(done.state, StackJobState::Replaced);
    assert!(done.preparing.is_none());
    let channels = prepare_channels(&ctx, 1, &build_request(vec![1, 2, 3, 4]), None)
        .unwrap()
        .into_iter()
        .map(|(_, prepared)| prepared.public.job_id)
        .collect::<Vec<_>>();
    assert!(!channels.is_empty());
    assert_eq!(done.replaced_by, channels);
    for job_id in &done.replaced_by {
        assert!(state.stack_previews.get(job_id).is_some());
        state.stack_previews.request_cancel(job_id);
    }
    // Once prepared, the next click is a new request.
    let later =
        build_set::queue_preparation(&state, &ctx, 1, build_request(vec![1, 2, 3, 4]), true)
            .unwrap();
    assert_ne!(later.job_id, job.job_id);
    prepared(&state, &later.job_id).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_single_build_is_prepared_into_the_build_it_names() {
    let directory = tempfile::tempdir().unwrap();
    let (state, ctx) = state_with_frames(directory.path());
    let request = build_request(vec![1, 2, 3, 4]);
    let job = build_set::queue_preparation(&state, &ctx, 1, request.clone(), false).unwrap();
    let done = prepared(&state, &job.job_id).await;
    assert_eq!(done.state, StackJobState::Replaced);
    let expected = prepare_job(&ctx, 1, &request).unwrap().public.job_id;
    assert_eq!(done.replaced_by, vec![expected.clone()]);
    assert!(state.stack_previews.get(&expected).is_some());
    // Stopping the stand-in now stops nothing; the build is the one to stop.
    assert!(!state.stack_previews.request_cancel(&job.job_id));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_build_whose_frames_change_while_it_waits_runs_as_a_new_build() {
    let directory = tempfile::tempdir().unwrap();
    let (state, ctx) = state_with_frames(directory.path());
    // The worker is busy, so the build waits in line after its preview.
    let busy = state.stack_previews.line.try_reserve_main().unwrap();
    let request = build_request(vec![1, 2, 3, 4]);
    let slot = build_set::queue_preparation(&state, &ctx, 1, request.clone(), false).unwrap();
    let previewed = prepared(&state, &slot.job_id).await.replaced_by[0].clone();
    assert_eq!(
        state.stack_previews.get(&previewed).unwrap().state,
        StackJobState::Queued
    );
    // A frame is rejected while the build waits.
    ctx.db()
        .lock()
        .unwrap()
        .execute(
            "UPDATE acquiredimage SET gradingStatus = 2 WHERE Id = 2",
            [],
        )
        .unwrap();
    let expected = prepare_job(&ctx, 1, &request).unwrap().public.job_id;
    assert_ne!(expected, previewed, "the rejection changes the build");
    drop(busy);
    let mut done = state.stack_previews.get(&previewed).unwrap();
    for _ in 0..500 {
        done = state.stack_previews.get(&previewed).unwrap();
        if done.state == StackJobState::Replaced {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    // The previewed build points to the build prepared at its turn.
    assert_eq!(done.state, StackJobState::Replaced);
    assert_eq!(done.replaced_by, vec![expected.clone()]);
    assert!(state.stack_previews.get(&expected).is_some());
    state.stack_previews.request_cancel(&expected);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_build_that_cannot_be_prepared_fails_with_why() {
    let directory = tempfile::tempdir().unwrap();
    let (state, ctx) = state_with_frames(directory.path());
    let job =
        build_set::queue_preparation(&state, &ctx, 1, build_request(vec![1, 99]), false).unwrap();
    assert_eq!(job.state, StackJobState::Preparing);
    let done = prepared(&state, &job.job_id).await;
    assert_eq!(done.state, StackJobState::Failed);
    assert_eq!(
        done.error.as_deref(),
        Some("Every requested image must belong to the selected project")
    );
    assert!(done.replaced_by.is_empty());
    assert!(state.stack_previews.active().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_build_stopped_while_it_is_prepared_queues_nothing() {
    let directory = tempfile::tempdir().unwrap();
    let (state, ctx) = state_with_frames(directory.path());
    let db = ctx.db();
    let busy = db.lock().unwrap();
    let job = build_set::queue_preparation(&state, &ctx, 1, build_request(vec![1, 2, 3, 4]), true)
        .unwrap();
    assert!(state.stack_previews.request_cancel(&job.job_id));
    assert_eq!(
        state.stack_previews.get(&job.job_id).unwrap().state,
        StackJobState::Cancelled,
        "it ends at once"
    );
    drop(busy);
    // Its preparation finishes and is dropped.
    for _ in 0..100 {
        if state.stack_previews.line.stop_flag(&job.job_id).is_none() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let done = state.stack_previews.get(&job.job_id).unwrap();
    assert_eq!(done.state, StackJobState::Cancelled);
    assert!(done.replaced_by.is_empty());
    assert!(state.stack_previews.active().is_empty());
}

#[tokio::test]
async fn a_build_being_prepared_holds_its_place_without_holding_up_the_line() {
    let manager = StackPreviewManager::new();
    manager.queue_test_build("ahead");
    manager.hold_place("slot", Vec::new(), false);
    manager.queue_test_build("behind");
    assert_eq!(manager.line.order(), ["ahead", "slot", "behind"]);
    assert_eq!(manager.dispatch_next().await, (true, "ahead".to_string()));
    // The held place keeps its position but holds up nothing behind it.
    manager.queue_test_build("last");
    assert_eq!(
        manager.dispatch_next().await,
        (true, "behind".to_string()),
        "a build still being prepared does not hold up the one behind it"
    );
    // The builds it becomes take its place, ahead of the one queued
    // after it, and the place goes once it is prepared.
    for job_id in ["red", "green"] {
        let stop = manager.track_job(job_id);
        assert!(manager
            .line
            .submit(
                crate::server::job_line::testing::entry(job_id, Lane::Main, stop),
                Some("slot")
            )
            .is_ok());
    }
    manager.release_place("slot");
    assert_eq!(manager.line.order(), ["red", "green", "last"]);
    // Work already in the line keeps its place.
    let again = crate::server::job_line::testing::entry("last", Lane::Main, Stop::new());
    assert!(manager.line.submit(again, Some("red")).is_err());
    assert_eq!(manager.line.order(), ["red", "green", "last"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_build_being_prepared_comes_back_after_a_restart_under_its_id() {
    let directory = tempfile::tempdir().unwrap();
    let registry_dir = tempfile::tempdir().unwrap();
    let registry = registry_dir.path().join("reg.json");
    let (before, ctx) = state_with_frames(directory.path());
    // The catalog is busy, so both clicks are still being prepared.
    let db = ctx.db();
    let busy = db.lock().unwrap();
    let channels_request = build_request(vec![1, 2, 3, 4]);
    let single_request = build_request(vec![1, 2]);
    let split =
        build_set::queue_preparation(&before, &ctx, 1, channels_request.clone(), true).unwrap();
    let single =
        build_set::queue_preparation(&before, &ctx, 1, single_request.clone(), false).unwrap();
    assert_eq!(
        before.stack_previews.journal_entries(),
        vec![
            journal::JournaledStackJob::requested(
                &ctx.id,
                1,
                &channels_request,
                true,
                &split.job_id,
                false
            ),
            journal::JournaledStackJob::requested(
                &ctx.id,
                1,
                &single_request,
                false,
                &single.job_id,
                false
            ),
        ],
        "journaled in click order as soon as they are asked for"
    );
    // A build prepared before the restart comes first in the line.
    let (after, after_ctx) = state_with_frames(directory.path());
    let queued_request = build_request(vec![3, 4]);
    let queued_id = prepare_job(&after_ctx, 1, &queued_request).unwrap().public;
    let mut journal = journal::snapshot(&before);
    journal.stack.insert(
        0,
        journal::JournaledStackJob::mono(&ctx.id, 1, &queued_request, &queued_id, false),
    );
    let queued_id = queued_id.job_id;
    stretch::write_json_atomic(&journal::journal_path(&registry), &journal).unwrap();
    for job_id in [&split.job_id, &single.job_id] {
        before.stack_previews.request_cancel(job_id);
    }
    drop(busy);

    // The restart reads nothing: it puts each build back in its place,
    // under the id a page left open follows, while the catalog is busy.
    *after.registry_path.write().unwrap() = Some(registry.clone());
    let after_db = after_ctx.db();
    let (held, holding) = std::sync::mpsc::channel();
    let (release, released) = std::sync::mpsc::channel::<()>();
    let holder = std::thread::spawn(move || {
        let _busy = after_db.lock().unwrap();
        held.send(()).unwrap();
        let _ = released.recv();
    });
    holding.recv().unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), journal::restore(&after))
        .await
        .expect("the restore prepares nothing itself");
    assert_eq!(
        after.stack_previews.line.order(),
        [
            queued_id.clone(),
            split.job_id.clone(),
            single.job_id.clone()
        ]
    );
    for job_id in [&queued_id, &split.job_id, &single.job_id] {
        assert_eq!(
            after.stack_previews.get(job_id).unwrap().state,
            StackJobState::Preparing
        );
    }
    release.send(()).unwrap();
    holder.join().unwrap();

    // Prepared under its own id, the queued build is itself again.
    let queued = prepared(&after, &queued_id).await;
    assert_ne!(queued.state, StackJobState::Replaced);
    assert!(queued.replaced_by.is_empty());
    let channels = prepare_channels(&after_ctx, 1, &channels_request, None)
        .unwrap()
        .into_iter()
        .map(|(_, prepared)| prepared.public.job_id)
        .collect::<Vec<_>>();
    assert_eq!(prepared(&after, &split.job_id).await.replaced_by, channels);
    let expected = prepare_job(&after_ctx, 1, &single_request)
        .unwrap()
        .public
        .job_id;
    assert_eq!(
        prepared(&after, &single.job_id).await.replaced_by,
        vec![expected.clone()]
    );
    for job_id in std::iter::once(&queued_id)
        .chain(&channels)
        .chain(std::iter::once(&expected))
    {
        after.stack_previews.request_cancel(job_id);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_build_put_back_that_cannot_be_prepared_yet_waits_to_try_again() {
    let directory = tempfile::tempdir().unwrap();
    let (state, ctx) = state_with_frames(directory.path());
    // After a restart, a frame it names cannot be found yet, as on a disk
    // still being mounted: it keeps its place and its journal entry.
    let request = build_request(vec![1, 99]);
    let job = build_set::queue(
        &state,
        &ctx,
        build_set::Asked {
            project_id: 1,
            what: build_set::What::Mono {
                request: Box::new(request.clone()),
                split: false,
            },
            automatic: false,
            job_id: Some("restored".into()),
            retry_until: Some(std::time::Instant::now() + std::time::Duration::from_secs(60)),
        },
    )
    .unwrap();
    assert_eq!(job, "restored");
    for _ in 0..500 {
        let idle = state.stack_previews.preparing.is_idle(&ctx.id);
        if idle {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let waiting = state.stack_previews.get("restored").unwrap();
    assert_eq!(waiting.state, StackJobState::Preparing);
    assert!(waiting.error.is_none());
    assert_eq!(state.stack_previews.line.order(), ["restored"]);
    assert_eq!(
        state.stack_previews.journal_entries(),
        vec![journal::JournaledStackJob::requested(
            &ctx.id, 1, &request, false, "restored", false
        )]
    );
    assert!(state.stack_previews.request_cancel("restored"));
}

#[tokio::test]
async fn a_persons_stop_after_a_preemption_keeps_the_build_stopped() {
    let manager = StackPreviewManager::new();
    let mut job = running_build("auto");
    job.automatic = true;
    assert!(manager.insert(job));
    let stop = manager.track_job("auto");
    let entry =
        crate::server::job_line::testing::entry("auto", Lane::Main, stop).tier(Tier::Automatic);
    assert!(manager.line.submit(entry, None).is_ok());
    let running = manager.line.next(Lane::Main).await;
    // A person's build preempts it, then the person stops it too.
    let person = crate::server::job_line::testing::entry("person", Lane::Main, Stop::new());
    assert!(manager.line.submit(person, None).is_ok());
    assert!(manager.request_cancel("auto"));
    assert!(
        !manager.line.take_preempted("auto"),
        "it would go back in the line"
    );
    drop(running);
}

#[test]
fn a_stopped_build_is_queued_again_with_its_stopped_channels_waiting() {
    let state = AppState::new_for_test(rusqlite::Connection::open_in_memory().unwrap());
    let mut job = running_build("auto");
    job.automatic = true;
    cancel_unfinished_groups(&mut job);
    job.state = StackJobState::Cancelled;
    assert!(state.stack_previews.insert(job));
    assert!(requeue_stopped_build(&state, "auto"));
    let requeued = state.stack_previews.get("auto").unwrap();
    assert_eq!(requeued.state, StackJobState::Queued);
    assert!(requeued
        .groups
        .iter()
        .all(|group| group.state != StackGroupState::Cancelled));
    // A build that finished before the stop reached it is not queued
    // again.
    let mut done = running_build("done");
    done.state = StackJobState::Completed;
    assert!(state.stack_previews.insert(done));
    assert!(!requeue_stopped_build(&state, "done"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_persons_request_goes_ahead_of_an_automatic_one_still_being_prepared() {
    let directory = tempfile::tempdir().unwrap();
    let (state, ctx) = state_with_frames(directory.path());
    let db = ctx.db();
    let busy = db.lock().unwrap();
    let asked = |automatic| build_set::Asked {
        project_id: 1,
        what: build_set::What::Mono {
            request: Box::new(build_request(if automatic {
                vec![1, 2, 3, 4]
            } else {
                vec![1, 2]
            })),
            split: false,
        },
        automatic,
        job_id: None,
        retry_until: None,
    };
    // An automatic refresh's request, not yet prepared, then a person's.
    let automatic = build_set::queue(&state, &ctx, asked(true)).unwrap();
    let person = build_set::queue(&state, &ctx, asked(false)).unwrap();
    assert_eq!(
        state.stack_previews.line.order(),
        [person.clone(), automatic.clone()]
    );
    // The automatic one keeps its place and its journal entry.
    assert_eq!(state.stack_previews.journal_entries().len(), 2);
    drop(busy);
    for job_id in [&person, &automatic] {
        let done = prepared(&state, job_id).await;
        for built in &done.replaced_by {
            state.stack_previews.request_cancel(built);
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_composition_asked_for_while_its_channels_build_waits_for_them() {
    let directory = tempfile::tempdir().unwrap();
    let (state, ctx) = state_with_frames(directory.path());
    // A build of target 7's channels waits in the line.
    state
        .stack_previews
        .hold_place("building", vec![channel_key(&ctx.id, 1, 7)], false);
    let rgb: color::StackColorRequest =
        serde_json::from_value(serde_json::json!({"target_id": 7, "kind": "rgb"})).unwrap();
    let lrgb: color::StackColorRequest =
        serde_json::from_value(serde_json::json!({"target_id": 7, "kind": "lrgb"})).unwrap();
    let waiting = build_set::queue_color_preparation(&state, &ctx, 1, rgb.clone()).unwrap();
    let stopped = build_set::queue_color_preparation(&state, &ctx, 1, lrgb).unwrap();
    assert_eq!(waiting.state, StackJobState::Preparing);
    assert_eq!(waiting.preparing.as_deref(), Some("RGB"));
    let waits = |job_id: &str| {
        state
            .stack_previews
            .get_color(job_id)
            .is_some_and(|job| job.phase == "Waiting for the channel builds to finish")
    };
    for _ in 0..500 {
        if waits(&waiting.job_id) && waits(&stopped.job_id) {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let held = state.stack_previews.get_color(&waiting.job_id).unwrap();
    assert_eq!(held.state, StackJobState::Preparing);
    assert_eq!(held.phase, "Waiting for the channel builds to finish");
    assert!(
        state
            .stack_previews
            .active()
            .iter()
            .any(|entry| entry.job_id == waiting.job_id && entry.label == "RGB"),
        "the header lists it"
    );
    // A person's cancel ends one at once; the other keeps its place and
    // its journal entry.
    assert!(state.stack_previews.request_cancel(&stopped.job_id));
    assert_eq!(
        state
            .stack_previews
            .get_color(&stopped.job_id)
            .unwrap()
            .state,
        StackJobState::Cancelled
    );
    assert_eq!(
        state.stack_previews.line.order(),
        ["building".to_string(), waiting.job_id.clone()]
    );
    assert_eq!(
        state.stack_previews.journal_entries(),
        vec![journal::JournaledStackJob::color(
            &ctx.id,
            1,
            &rgb,
            Some(&waiting.job_id),
            false
        )]
    );

    // The build ends: the composition is prepared from the stacks there
    // are now. This catalog has none, so it says so.
    state.stack_previews.release_place("building");
    let mut done = held;
    for _ in 0..500 {
        done = state.stack_previews.get_color(&waiting.job_id).unwrap();
        if done.state != StackJobState::Preparing {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(done.state, StackJobState::Failed);
    assert_eq!(
        done.error.as_deref(),
        Some("No completed channel stacks are available for that target")
    );
    assert!(state.stack_previews.line.order().is_empty());
    for _ in 0..100 {
        if state.stack_previews.journal_entries().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(state.stack_previews.journal_entries().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_composition_put_back_by_the_journal_keeps_its_id() {
    let directory = tempfile::tempdir().unwrap();
    let (state, ctx) = state_with_frames(directory.path());
    let request: color::StackColorRequest =
        serde_json::from_value(serde_json::json!({"target_id": 7, "kind": "rgb"})).unwrap();
    let job_id = build_set::queue(
        &state,
        &ctx,
        build_set::Asked {
            project_id: 1,
            what: build_set::What::Color(Box::new(request)),
            automatic: false,
            job_id: Some("restored-color".into()),
            retry_until: None,
        },
    )
    .unwrap();
    assert_eq!(job_id, "restored-color");
    let done = loop {
        let job = state.stack_previews.get_color("restored-color").unwrap();
        if job.state != StackJobState::Preparing {
            break job;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    };
    assert_eq!(done.state, StackJobState::Failed, "nothing to compose here");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stop_keeps_a_build_being_prepared_and_a_cancel_drops_it() {
    let directory = tempfile::tempdir().unwrap();
    let (state, ctx) = state_with_frames(directory.path());
    let db = ctx.db();
    let busy = db.lock().unwrap();
    let request = build_request(vec![1, 2, 3, 4]);
    let cancelled =
        build_set::queue_preparation(&state, &ctx, 1, build_request(vec![1, 2]), false).unwrap();
    let kept = build_set::queue_preparation(&state, &ctx, 1, request.clone(), true).unwrap();
    // A person's cancel ends it and frees its place at once, and a
    // restart does not bring it back.
    assert!(state.stack_previews.request_cancel(&cancelled.job_id));
    assert_eq!(
        state.stack_previews.line.order(),
        std::slice::from_ref(&kept.job_id)
    );
    let entry =
        journal::JournaledStackJob::requested(&ctx.id, 1, &request, true, &kept.job_id, false);
    assert_eq!(state.stack_previews.journal_entries(), vec![entry.clone()]);
    // A server stop asks it to stop, and the journal keeps it.
    state.stack_previews.stop_for_shutdown();
    drop(busy);
    prepared(&state, &kept.job_id).await;
    for _ in 0..100 {
        if state.stack_previews.line.stop_flag(&kept.job_id).is_none() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(state.stack_previews.journal_entries(), vec![entry]);
    assert!(state.stack_previews.line.order().is_empty());
}

/// A Build of the stack `job_id`, as the request's preparation hands it
/// over before the cache is consulted.
fn build_again(state: &Arc<AppState>, cache: &FsPath, job_id: &str) -> Started {
    let ctx = state.get_database("test").unwrap();
    let mut public = completed_job(job_id, vec![ready_group(42, "R", 1)]);
    public.database_id = "test".into();
    public.state = StackJobState::Queued;
    let prepared = PreparedJob {
        public,
        groups: Vec::new(),
        orientation_peers: Vec::new(),
        stack_root: cache.to_path_buf(),
        calibration_root: cache.join("calibration"),
        north_up: false,
        order: snr::StackFrameOrder::Capture,
    };
    let request: StackPreviewRequest =
        serde_json::from_value(serde_json::json!({"image_ids": [1, 2]})).unwrap();
    start_prepared(state, &ctx, 7, &request, prepared, Placement::default()).unwrap()
}

/// [`state_on_disk`] with a catalog holding project 7, whose exposures
/// are not split, so the cards can be pointed at a stack.
fn state_with_project(cache: &FsPath) -> Arc<AppState> {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    crate::ts_schema::apply_schema(&conn).unwrap();
    conn.execute_batch(
        "INSERT INTO project(Id,name,profileId,guid) VALUES(7,'Project','profile','project-7');
        CREATE TABLE psf_guard_project_processing(project_key TEXT PRIMARY KEY,
            split_exposure_groups INTEGER NOT NULL,split_chosen INTEGER);
        INSERT INTO psf_guard_project_processing VALUES('guid:project-7',0,1);",
    )
    .unwrap();
    let mut ctx = crate::server::database_context::DatabaseContext::new_for_test(conn);
    ctx.stack_root = cache.to_path_buf();
    let state = test_state();
    state
        .databases
        .write()
        .unwrap()
        .insert(ctx.id.clone(), Arc::new(ctx));
    state
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn build_renders_an_older_preview_again_from_the_fits_and_keeps_the_stack() {
    let cache = tempfile::tempdir().unwrap();
    let state = state_with_project(cache.path());
    let job_id = "3".repeat(64);
    let mut job = stack_on_disk(cache.path(), &job_id, "rev-1");
    job.groups[0].preview_stretch_version = Some("0.2.2".into());
    stretch::write_json_atomic(&manifest_path(cache.path(), &job_id), &job).unwrap();

    // Another build holds the worker, so the render waits in line.
    let busy = state.stack_previews.line.reserve_main().await;
    let first = build_again(&state, cache.path(), &job_id);
    assert_eq!(first.job.state, StackJobState::Queued);
    assert_eq!(first.job.groups[0].phase, PHASE_RENDER_QUEUED);
    assert!(first.cached.is_none());
    // The channel stays Ready, so its processing and FITS stay reachable.
    assert_eq!(first.job.groups[0].state, StackGroupState::Ready);
    let ctx = state.get_database("test").unwrap();
    assert!(stack_processing_source(&state, &ctx, &job_id, 0).is_ok());
    let activity = mono_activity(&first.job);
    assert_eq!(activity.detail, "Waiting to render preview");
    // A second Build while it waits joins it rather than queueing more.
    let second = build_again(&state, cache.path(), &job_id);
    assert_eq!(second.job.state, StackJobState::Queued);
    assert_eq!(state.stack_previews.line_position(&job_id), Some(0));
    drop(busy);
    // The job shows Completed only once its manifest and the latest
    // index are written, so both can be read as soon as it does.
    eventually("the preview renders", || {
        state
            .stack_previews
            .get(&job_id)
            .is_some_and(|job| job.state == StackJobState::Completed)
    })
    .await;

    let stored: StackPreviewJob =
        serde_json::from_slice(&std::fs::read(manifest_path(cache.path(), &job_id)).unwrap())
            .unwrap();
    assert_eq!(stored.state, StackJobState::Completed);
    assert_eq!(
        stored.artifact_revision, "rev-1",
        "the stack keeps its revision"
    );
    let group = &stored.groups[0];
    assert_eq!(group.state, StackGroupState::Ready);
    assert_eq!(
        group.preview_stretch_version.as_deref(),
        Some(stretch::SEIZA_STRETCH_OUTPUT_VERSION)
    );
    assert!(group.preview_revision.is_some());
    assert!(preview_path(cache.path(), &job_id, 0).is_file());
    assert!(original_preview_path(cache.path(), &job_id, 0).is_file());
    // The card reads the new preview revision from the latest index.
    let latest: LatestStackPreviews =
        serde_json::from_slice(&std::fs::read(latest_path(cache.path(), 7)).unwrap()).unwrap();
    assert_eq!(latest.groups[0].artifact_revision, "rev-1");
    assert_eq!(
        latest.groups[0].group.preview_revision,
        group.preview_revision
    );
    // Now current, so the next Build reuses it as it is.
    let third = build_again(&state, cache.path(), &job_id);
    assert_eq!(third.job.state, StackJobState::Completed);
    assert!(third.cached.is_some());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stopped_preview_render_leaves_the_stack_finished() {
    let cache = tempfile::tempdir().unwrap();
    let state = state_on_disk(cache.path(), None);
    let job_id = "3".repeat(64);
    let mut job = stack_on_disk(cache.path(), &job_id, "rev-1");
    job.groups[0].preview_stretch_version = Some("0.2.2".into());
    stretch::write_json_atomic(&manifest_path(cache.path(), &job_id), &job).unwrap();
    let busy = state.stack_previews.line.reserve_main().await;
    build_again(&state, cache.path(), &job_id);
    assert!(state.stack_previews.request_cancel(&job_id));
    eventually("the render stops", || {
        state
            .stack_previews
            .get(&job_id)
            .is_some_and(|job| job.state == StackJobState::Completed)
    })
    .await;
    drop(busy);
    let stopped = state.stack_previews.get(&job_id).unwrap();
    assert_eq!(stopped.groups[0].state, StackGroupState::Ready);
    assert_eq!(stopped.groups[0].preview_revision, None);
    // Still a finished stack: the next Build renders, it does not stack.
    let next = build_again(&state, cache.path(), &job_id);
    assert_eq!(next.job.groups[0].phase, PHASE_RENDER_QUEUED);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_render_says_why_and_the_next_build_reuses_the_stack() {
    let cache = tempfile::tempdir().unwrap();
    let state = state_with_project(cache.path());
    let job_id = "3".repeat(64);
    let mut job = stack_on_disk(cache.path(), &job_id, "rev-1");
    job.groups[0].preview_stretch_version = Some("0.2.2".into());
    stretch::write_json_atomic(&manifest_path(cache.path(), &job_id), &job).unwrap();
    std::fs::write(fits_path(cache.path(), &job_id, 0), b"not a FITS file").unwrap();

    build_again(&state, cache.path(), &job_id);
    eventually("the render ends", || {
        state
            .stack_previews
            .get(&job_id)
            .is_some_and(|job| job.state == StackJobState::Completed)
    })
    .await;
    let group = state.stack_previews.get(&job_id).unwrap().groups[0].clone();
    assert_eq!(group.state, StackGroupState::Ready);
    assert_eq!(group.preview_revision, None);
    assert_eq!(
        group.preview_render_failed.as_deref(),
        Some(stretch::SEIZA_STRETCH_OUTPUT_VERSION)
    );
    assert!(group
        .resume_note
        .unwrap()
        .starts_with("The preview could not be rendered again from the stack's FITS"));
    let next = build_again(&state, cache.path(), &job_id);
    assert_eq!(next.job.state, StackJobState::Completed);
    assert!(next.cached.is_some(), "the next Build reuses the stack");
}

/// Target 42's R, G and B, stacked and shown on their cards.
fn rgb_channels_on_disk(cache: &FsPath) -> Vec<StackPreviewJob> {
    [('1', "R"), ('2', "G"), ('3', "B")]
        .into_iter()
        .map(|(digit, filter)| {
            let job = channel_stack_on_disk(cache, &digit.to_string().repeat(64), filter, "rev");
            persist_latest_groups(cache, &job).unwrap();
            job
        })
        .collect()
}

/// The request of an automatic refresh, carrying the default color pipeline.
fn automatic_refresh(state: &AppState) -> StackPreviewRequest {
    let mut refresh = build_request(Vec::new());
    refresh.color_defaults = Some(crate::server::processing_setups::default_color_processing(
        state,
    ));
    refresh
}

/// An automatic build of target 42's B from before color defaults existed,
/// its preview rendered by this version.
fn automatic_blue(channels: &[StackPreviewJob]) -> StackPreviewJob {
    let mut found = channels[2].clone();
    found.automatic = true;
    found.color_defaults = None;
    found.groups[0].preview_stretch_version = Some(stretch::SEIZA_STRETCH_OUTPUT_VERSION.into());
    found
}

fn color_jobs(state: &AppState) -> Vec<color::StackColorJob> {
    state
        .stack_previews
        .color_jobs
        .lock()
        .unwrap()
        .values()
        .cloned()
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refresh_that_finds_a_targets_channels_built_composes_its_first_color_preview() {
    use color::StackColorRole::{Blue, Green, Red};
    let cache = tempfile::tempdir().unwrap();
    let state = state_with_project(cache.path());
    let ctx = state.get_database("test").unwrap();
    // The composition stays queued while the test looks at it.
    let busy = state.stack_previews.line.reserve_main().await;
    let channels = rgb_channels_on_disk(cache.path());
    let refresh = automatic_refresh(&state);
    let render_queued =
        automatic::reuse_finished_build(&state, &ctx, automatic_blue(&channels), &refresh).await;
    assert!(!render_queued, "its preview is current");

    let composed = color_jobs(&state);
    assert_eq!(composed.len(), 1, "one first color preview");
    let first = &composed[0];
    assert_eq!(
        (first.target_id, first.kind),
        (42, color::StackColorKind::Rgb)
    );
    assert!(first.automatic);
    let processing = first.processing.as_ref().unwrap();
    assert_eq!(
        processing
            .input_stretches
            .keys()
            .copied()
            .collect::<Vec<_>>(),
        [Red, Green, Blue]
    );
    assert!(processing.color_calibration.is_some(), "RGB is calibrated");

    // Once the target has a color preview, the default pipeline plans
    // nothing new for it: its preview follows its channels.
    let mut shown = first.clone();
    shown.state = StackJobState::Completed;
    color::persist_latest_color(cache.path(), &shown).unwrap();
    assert!(
        color::requests_after_build(&ctx, 7, 42, refresh.color_defaults.as_ref())
            .unwrap()
            .requests
            .is_empty()
    );
    state.stack_previews.request_cancel(&first.job_id);
    drop(busy);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refresh_that_renders_a_finished_build_again_gives_it_the_color_pipeline() {
    let cache = tempfile::tempdir().unwrap();
    let state = state_with_project(cache.path());
    let ctx = state.get_database("test").unwrap();
    let busy = state.stack_previews.line.reserve_main().await;
    let channels = rgb_channels_on_disk(cache.path());
    let refresh = automatic_refresh(&state);
    let mut found = automatic_blue(&channels);
    found.groups[0].preview_stretch_version = Some("0.2.2".into());
    let job_id = found.job_id.clone();
    assert!(automatic::reuse_finished_build(&state, &ctx, found, &refresh).await);
    // The render composes the target's color when it ends, with this.
    let rendering = state.stack_previews.get(&job_id).unwrap();
    assert_eq!(rendering.state, StackJobState::Queued);
    assert_eq!(
        serde_json::to_value(&rendering.color_defaults).unwrap(),
        serde_json::to_value(&refresh.color_defaults).unwrap()
    );
    state.stack_previews.request_cancel(&job_id);
    drop(busy);
}

/// Mark a color job the server keeps in memory as failed.
fn fail_color_job(state: &AppState, job_id: &str) {
    let mut jobs = state.stack_previews.color_jobs.lock().unwrap();
    let job = jobs.get_mut(job_id).unwrap();
    job.state = StackJobState::Failed;
    job.error = Some("could not register the channels".into());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_first_color_preview_that_failed_is_not_tried_again_by_each_refresh() {
    let cache = tempfile::tempdir().unwrap();
    let state = state_with_project(cache.path());
    let ctx = state.get_database("test").unwrap();
    // The compositions stay queued while the test looks at them.
    let busy = state.stack_previews.line.reserve_main().await;
    let channels = rgb_channels_on_disk(cache.path());
    let refresh = automatic_refresh(&state);
    automatic::reuse_finished_build(&state, &ctx, automatic_blue(&channels), &refresh).await;
    let first = color_jobs(&state).remove(0);
    fail_color_job(&state, &first.job_id);

    // The next refresh leaves it failed.
    automatic::reuse_finished_build(&state, &ctx, automatic_blue(&channels), &refresh).await;
    let after = color_jobs(&state);
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].state, StackJobState::Failed);

    // A person's build tries it again.
    let mut persons = channels[2].clone();
    persons.color_defaults = refresh.color_defaults.clone();
    automatic::compose_colors_after(&state, &ctx, &persons);
    let retried = state.stack_previews.get_color(&first.job_id).unwrap();
    assert_eq!(retried.state, StackJobState::Queued);
    assert!(!retried.automatic);
    state.stack_previews.request_cancel(&first.job_id);
    drop(busy);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refresh_tries_again_a_failed_recomposition_of_an_existing_color_preview() {
    let cache = tempfile::tempdir().unwrap();
    let state = state_with_project(cache.path());
    let ctx = state.get_database("test").unwrap();
    let busy = state.stack_previews.line.reserve_main().await;
    let channels = rgb_channels_on_disk(cache.path());
    let refresh = automatic_refresh(&state);
    automatic::reuse_finished_build(&state, &ctx, automatic_blue(&channels), &refresh).await;
    // The target shows this preview, composed from an older stack of each
    // channel: a refresh composes it again from the current ones.
    let composed = color_jobs(&state).remove(0);
    let mut shown = composed.clone();
    shown.state = StackJobState::Completed;
    for source in &mut shown.sources {
        source.artifact_revision = "older".into();
    }
    color::persist_latest_color(cache.path(), &shown).unwrap();
    let planned = color::requests_after_build(&ctx, 7, 42, None).unwrap();
    assert!(!planned.first);
    assert_eq!(planned.requests.len(), 1);
    // That composition failed, perhaps for a moment.
    fail_color_job(&state, &composed.job_id);

    automatic::reuse_finished_build(&state, &ctx, automatic_blue(&channels), &refresh).await;
    let retried = state.stack_previews.get_color(&composed.job_id).unwrap();
    assert_eq!(retried.state, StackJobState::Queued);
    assert!(retried.automatic);
    state.stack_previews.request_cancel(&composed.job_id);
    drop(busy);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_automatic_render_leaves_a_persons_build_and_its_color_defaults_as_they_were() {
    let cache = tempfile::tempdir().unwrap();
    let state = state_with_project(cache.path());
    let ctx = state.get_database("test").unwrap();
    let job_id = "3".repeat(64);
    let mut job = stack_on_disk(cache.path(), &job_id, "rev-1");
    job.groups[0].preview_stretch_version = Some("0.2.2".into());
    job.color_defaults = Some(color::StackColorProcessing::default());
    let busy = state.stack_previews.line.reserve_main().await;
    let queued = queue_preview_render(&state, &ctx, &job, true, None).unwrap();
    assert!(!queued.automatic, "a person's build stays theirs");
    assert_eq!(
        serde_json::to_value(&queued.color_defaults).unwrap(),
        serde_json::to_value(&job.color_defaults).unwrap()
    );
    // The same build found again while it waits queues nothing more.
    assert!(queue_preview_render(&state, &ctx, &job, false, None).is_none());
    assert_eq!(state.stack_previews.line_position(&job_id), Some(0));
    drop(busy);
    eventually("the preview renders", || {
        state
            .stack_previews
            .get(&job_id)
            .is_some_and(|job| job.state == StackJobState::Completed)
    })
    .await;
}

/// Processing runs driven through the stand-in `rc-astro`, a shell
/// script, and the helpers only they use.
#[cfg(unix)]
mod processing_runs {
    use super::*;

    /// A small linear stack on disk and the scope of its R channel.
    fn processing_source(cache: &FsPath, revision: &str) -> stretch::StretchSource {
        let path = cache.join(format!("source-{revision}.fits"));
        let data = (0..256)
            .map(|index| 100.0 + (index % 17) as f32 * 7.0)
            .collect();
        let image = seiza_stacking::LinearImage::new(16, 16, 1, data).unwrap();
        seiza_stacking::write_processed_image_fits_f32(&path, &image, &[], &[]).unwrap();
        stretch::StretchSource {
            key: format!("mono:{}:0", "e".repeat(64)),
            revision: revision.into(),
            path,
            scope: processing_queue::ProcessingScope {
                database_id: "db-test".into(),
                project_id: 7,
                target_id: 42,
                target_name: "Target 42".into(),
                channel: "R".into(),
                exposure: None,
            },
            job_id: "e".repeat(64),
            group_index: 0,
            calibration: None,
        }
    }

    fn rc_astro_request(tools: &[&str]) -> stretch::StackViewProcessingRequest {
        let steps = tools
            .iter()
            .map(|tool| serde_json::json!({"tool": tool, "parameters": {}}))
            .collect::<Vec<_>>();
        serde_json::from_value(serde_json::json!({
            "model": {"type": "identity"},
            "rc_astro": {"steps": steps},
        }))
        .unwrap()
    }

    /// Re-send a request as the client's poll does until the run settles:
    /// its result, or the failure it parked.
    async fn poll_until_settled(
        state: &Arc<AppState>,
        cache: &FsPath,
        revision: &str,
        tools: &[&str],
    ) -> Result<Box<stretch::StackStretchPreview>, AppError> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            match stretch::apply_to_fits(
                Arc::clone(state),
                "db-test".into(),
                cache.into(),
                processing_source(cache, revision),
                rc_astro_request(tools),
                false,
            )
            .await
            {
                Ok(stretch::StretchApplyOutcome::Pending { .. }) => {}
                Ok(stretch::StretchApplyOutcome::Ready(preview)) => return Ok(preview),
                Err(error) => return Err(error),
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the run never settled"
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }

    fn pending_json(state: &AppState, stretch_id: &str) -> serde_json::Value {
        serde_json::to_value(stretch::pending_status(stretch_id, &state.stack_previews)).unwrap()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_queued_run_names_what_holds_the_worker_then_reports_its_progress() {
        let tool = rc_astro::test_tool::install(true);
        let cache = tempfile::tempdir().unwrap();
        let state = test_state();
        // A build holds the worker.
        assert!(state.stack_previews.insert(running_build("held")));
        let busy = state.stack_previews.line.reserve_main().await;

        let source = processing_source(cache.path(), "queued-run");
        let outcome = stretch::apply_to_fits(
            Arc::clone(&state),
            "db-test".into(),
            cache.path().into(),
            source,
            rc_astro_request(&["bxt"]),
            true,
        )
        .await
        .unwrap();
        let stretch::StretchApplyOutcome::Pending { stretch_id } = outcome else {
            panic!("a run behind a build must answer pending");
        };

        // Waiting: a stage naming the build, and no fraction.
        let waiting = pending_json(&state, &stretch_id);
        assert_eq!(waiting["pending"], true);
        assert_eq!(waiting["queued"], true);
        assert_eq!(
            waiting["stage"],
            "Waiting for the stack build of Target 42 · Ha to finish"
        );
        assert_eq!(waiting["waiting_for"], "the stack build of Target 42 · Ha");
        assert_eq!(waiting["queue_position"], 0);
        assert!(waiting.get("fraction").is_none());

        // The header lists it behind the build, in the line.
        let active = state.stack_previews.active();
        assert_eq!(active.len(), 2);
        assert_eq!(active[0].kind, StackActivityKind::Mono);
        let queued = &active[1];
        assert_eq!(queued.kind, StackActivityKind::Processing);
        assert_eq!(queued.job_id, stretch_id);
        assert_eq!(queued.state, StackJobState::Queued);
        assert_eq!(queued.queue_position, Some(0));
        assert_eq!(queued.label, "Target 42 · R");
        let processing = queued.processing.as_ref().unwrap();
        assert_eq!(processing.operation, "BlurXTerminator");
        // The card of the stack's channel finds the run by its build and group.
        assert_eq!(
            processing.stack_job_id.as_deref(),
            Some("e".repeat(64).as_str())
        );
        assert_eq!(processing.group_index, Some(0));
        assert_eq!(
            processing.waiting_for.as_deref(),
            Some("the stack build of Target 42 · Ha")
        );

        // The build finishes; the run takes the worker and reports progress.
        state.stack_previews.update("held", |job| {
            job.state = StackJobState::Completed;
        });
        drop(busy);
        eventually("the tool reports half done", || {
            pending_json(&state, &stretch_id)["fraction"].as_f64() == Some(0.5)
        })
        .await;
        let running = pending_json(&state, &stretch_id);
        assert_eq!(running["queued"], false);
        assert_eq!(running["stage"], "RC-Astro BlurXTerminator");
        let entry = state.stack_previews.active().remove(0);
        assert_eq!(entry.state, StackJobState::Running);
        assert_eq!(entry.queue_position, None);
        assert_eq!(
            entry.progress_label.as_deref(),
            Some("BlurXTerminator · 50%")
        );
        assert_eq!(entry.processing.unwrap().fraction, Some(0.5));
        // Identical requests join the run instead of starting another.
        let again = stretch::apply_to_fits(
            Arc::clone(&state),
            "db-test".into(),
            cache.path().into(),
            processing_source(cache.path(), "queued-run"),
            rc_astro_request(&["bxt"]),
            true,
        )
        .await
        .unwrap();
        assert!(matches!(
            again,
            stretch::StretchApplyOutcome::Pending { .. }
        ));

        // Done: the poll gets the result and the run leaves the list.
        tool.release();
        let preview = poll_until_settled(&state, cache.path(), "queued-run", &["bxt"])
            .await
            .unwrap();
        assert_eq!(preview.stretch_id, stretch_id);
        assert_eq!(preview.rc_astro.unwrap().steps.len(), 1);
        eventually("the run leaves the list", || {
            state.stack_previews.active().is_empty()
        })
        .await;
        assert_eq!(tool.invocations(), 1);
        assert!(state.shutdown.running().is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stopping_a_running_rc_astro_run_kills_the_tool_and_removes_its_folder() {
        let tool = rc_astro::test_tool::install(true);
        let cache = tempfile::tempdir().unwrap();
        let state = test_state();
        // Held at first, so the request answers at once instead of waiting
        // for the result.
        let busy = state.stack_previews.line.reserve_main().await;
        let source = processing_source(cache.path(), "stopped-run");
        let outcome = stretch::apply_to_fits(
            Arc::clone(&state),
            "db-test".into(),
            cache.path().into(),
            source,
            rc_astro_request(&["bxt"]),
            true,
        );
        let stretch::StretchApplyOutcome::Pending { stretch_id } = outcome.await.unwrap() else {
            panic!("a queued run must answer pending");
        };
        drop(busy);
        eventually("the tool starts", || {
            tool.pid().is_some() && tool.workdir().is_some()
        })
        .await;
        let pid = tool.pid().unwrap();
        let workdir = tool.workdir().unwrap();
        assert!(workdir.is_dir());

        // The header's Stop reaches it, as it reaches a build.
        assert!(!state.stack_previews.processing_unstoppable(&stretch_id));
        assert!(state.stack_previews.request_cancel(&stretch_id));
        let refusal = poll_until_settled(&state, cache.path(), "stopped-run", &["bxt"])
            .await
            .err()
            .unwrap();
        assert!(
            matches!(&refusal, AppError::BadRequest(message) if message == stretch::STOPPED_MESSAGE),
            "{refusal:?}"
        );
        let alive = std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success();
        assert!(!alive, "the tool was killed");
        assert!(!workdir.exists(), "its exchange folder was removed");
        eventually("the run leaves the list", || {
            state.stack_previews.active().is_empty()
        })
        .await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_server_stop_kills_the_tool_and_journals_the_run_as_cut_off() {
        let tool = rc_astro::test_tool::install(true);
        let cache = tempfile::tempdir().unwrap();
        let state = test_state();
        // Held at first, so both requests answer at once.
        let busy = state.stack_previews.line.reserve_main().await;
        let start = |tools: &'static [&'static str], revision: &'static str, select| {
            let state = Arc::clone(&state);
            let root = cache.path().to_path_buf();
            async move {
                let source = processing_source(&root, revision);
                stretch::apply_to_fits(
                    state,
                    "db-test".into(),
                    root,
                    source,
                    rc_astro_request(tools),
                    select,
                )
                .await
            }
        };
        let stretch::StretchApplyOutcome::Pending { stretch_id: chain } =
            start(&["bxt", "nxt"], "stop-chain", true).await.unwrap()
        else {
            panic!("pending");
        };
        let stretch::StretchApplyOutcome::Pending { stretch_id: behind } =
            start(&["bxt"], "stop-behind", true).await.unwrap()
        else {
            panic!("pending");
        };
        drop(busy);
        eventually("BlurXTerminator runs", || {
            tool.pid().is_some() && tool.workdir().is_some()
        })
        .await;
        let pid = tool.pid().unwrap();
        let workdir = tool.workdir().unwrap();
        assert_eq!(state.shutdown.running().len(), 2, "both runs hold the stop");

        state.shutdown.begin();
        state.stack_previews.stop_for_shutdown();
        // Written before the kill lands: the run in hand is cut off, the one
        // waiting is not.
        let journaled = |entries: Vec<journal::JournaledStackJob>| {
            entries
                .into_iter()
                .map(|entry| match entry {
                    journal::JournaledStackJob::Processing {
                        run_id,
                        running,
                        attempts,
                        ..
                    } => (run_id, running, attempts),
                    other => panic!("unexpected {other:?}"),
                })
                .collect::<Vec<_>>()
        };
        let expected = vec![(chain.clone(), true, 0), (behind.clone(), false, 0)];
        assert_eq!(journaled(state.stack_previews.journal_entries()), expected);

        // The stop waits for the kill and the reap, not for the tool.
        tokio::time::timeout(std::time::Duration::from_secs(5), state.shutdown.drained())
            .await
            .expect("the stop does not wait for the tool to finish");
        let alive = std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success();
        assert!(!alive, "the tool was killed");
        assert!(!workdir.exists(), "its exchange folder was removed");
        assert_eq!(tool.invocations(), 1, "NoiseXTerminator never started");
        assert!(state.stack_previews.active().is_empty());
        // The last journal write still has both, in their order.
        assert_eq!(journaled(state.stack_previews.journal_entries()), expected);
        // A page still polling hears that the run is pending, not that it
        // failed: it finds the run again after the restart.
        for (revision, tools) in [
            ("stop-chain", &["bxt", "nxt"][..]),
            ("stop-behind", &["bxt"][..]),
        ] {
            let poll = stretch::apply_to_fits(
                Arc::clone(&state),
                "db-test".into(),
                cache.path().into(),
                processing_source(cache.path(), revision),
                rc_astro_request(tools),
                false,
            )
            .await;
            assert!(
                matches!(poll, Ok(stretch::StretchApplyOutcome::Pending { .. })),
                "{revision}"
            );
        }
    }

    /// A server with one stack on disk, as a restarted server finds it.
    fn state_with_stack_on_disk(cache: &FsPath, registry: &FsPath) -> (Arc<AppState>, String) {
        let job_id = "d".repeat(64);
        stack_on_disk(cache, &job_id, "revision-on-disk");
        (state_on_disk(cache, Some(registry)), job_id)
    }

    /// What the browser sends for the R stack, first or as a poll.
    async fn apply_on_disk(
        state: &Arc<AppState>,
        job_id: &str,
        tools: &[&str],
        select: bool,
    ) -> Result<stretch::StretchApplyOutcome, AppError> {
        let ctx = state.get_database("test").unwrap();
        let source = stack_processing_source(state, &ctx, job_id, 0).unwrap();
        stretch::apply_to_fits(
            Arc::clone(state),
            ctx.id.clone(),
            ctx.stack_root.clone(),
            source,
            rc_astro_request(tools),
            select,
        )
        .await
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_run_cut_off_by_a_restart_comes_back_and_resumes_from_its_cached_step() {
        let tool = rc_astro::test_tool::install(true);
        let cache = tempfile::tempdir().unwrap();
        let registry_dir = tempfile::tempdir().unwrap();
        let registry = registry_dir.path().join("reg.json");
        let (before, job_id) = state_with_stack_on_disk(cache.path(), &registry);
        let busy = before.stack_previews.line.reserve_main().await;
        let stretch::StretchApplyOutcome::Pending { stretch_id } =
            apply_on_disk(&before, &job_id, &["bxt", "nxt"], true)
                .await
                .unwrap()
        else {
            panic!("pending");
        };
        drop(busy);
        // BlurXTerminator finishes and is cached; NoiseXTerminator is in hand
        // when the server stops.
        eventually("BlurXTerminator runs", || tool.invocations() == 1).await;
        tool.release();
        eventually("NoiseXTerminator runs", || tool.invocations() == 2).await;
        before.shutdown.begin();
        before.stack_previews.stop_for_shutdown();
        tokio::time::timeout(std::time::Duration::from_secs(5), before.shutdown.drained())
            .await
            .unwrap();
        journal::write_now(&before);
        let path = journal::journal_path(&registry);
        let written = journal::read(&path).unwrap();
        assert!(matches!(
            written.stack.as_slice(),
            [journal::JournaledStackJob::Processing { run_id, running: true, attempts: 0, .. }]
                if run_id == &stretch_id
        ));
        drop(before);

        // The restart: a new server reads the journal and puts the run back.
        let (after, _) = state_with_stack_on_disk(cache.path(), &registry);
        journal::restore(&after).await;
        // The page left open re-sends its poll and joins the restored run,
        // under the same id.
        let poll = apply_on_disk(&after, &job_id, &["bxt", "nxt"], false).await;
        assert!(matches!(
            &poll,
            Ok(stretch::StretchApplyOutcome::Pending { stretch_id: id }) if id == &stretch_id
        ));
        assert_eq!(pending_json(&after, &stretch_id)["pending"], true);
        assert!(matches!(
            after.stack_previews.journal_entries().as_slice(),
            [journal::JournaledStackJob::Processing { attempts: 1, .. }]
        ));
        // Only the step that was cut runs again.
        eventually("NoiseXTerminator runs again", || tool.invocations() == 3).await;
        tool.release();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let preview = loop {
            if let Ok(stretch::StretchApplyOutcome::Ready(preview)) =
                apply_on_disk(&after, &job_id, &["bxt", "nxt"], false).await
            {
                break preview;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the run never finished"
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        };
        assert_eq!(preview.stretch_id, stretch_id);
        assert_eq!(preview.rc_astro.unwrap().steps.len(), 2);
        assert_eq!(
            tool.invocations_log().lines().collect::<Vec<_>>(),
            vec!["bxt", "nxt", "nxt"]
        );
        // The selection the person made before the restart now shows it.
        let ctx = after.get_database("test").unwrap();
        let source = stack_processing_source(&after, &ctx, &job_id, 0).unwrap();
        let selected =
            stretch::selected_processing(&ctx.stack_root, &source.key, &source.revision).unwrap();
        assert_eq!(
            selected.map(|selected| selected.stretch_id),
            Some(stretch_id)
        );
        eventually("the run leaves the list", || {
            after.stack_previews.active().is_empty()
        })
        .await;
        assert!(after.stack_previews.journal_entries().is_empty());
    }
}
#[test]
fn only_a_color_stack_preview_goes_out_of_date_with_the_calibration() {
    let current = || color_calibration::StackCalibrationSource {
        reference_image_id: Some(1),
        orientation: None,
        solution: None,
        catalog: Some(crate::astrometry::AstrometryCatalogFileSignature {
            name: "gaia_photometry".into(),
            path: "/catalogs/stars-gaia-photometry.bin".into(),
            format: "SEIZAPH1".into(),
            size_bytes: 1,
            modified_unix_seconds: 1,
            modified_subsec_nanos: 0,
            sha256: None,
        }),
    };
    let skipped = color_calibration::StackColorCalibrationOutcome::Skipped(
        color_calibration::StackColorCalibrationSkip {
            reason: color_calibration::StackColorCalibrationSkipReason::NoGaiaCatalog,
            detail: "Gaia colors are not installed".into(),
        },
    );
    let mut color = ready_group(42, "L", 1);
    color.output_channels = 3;
    color.color_calibration = Some(skipped.clone());
    color.color_calibration_basis = Some(color_calibration::StackColorCalibrationBasis {
        reference_image_id: Some(1),
        solution_fingerprint: None,
        catalog: None,
    });
    assert_eq!(
        preview_calibration_outdated(&color, current).as_deref(),
        Some(
            "Out of date — Gaia colors were installed; click Build current set to render \
             the preview again"
        )
    );

    // A mono stack never is, and nothing is read for it.
    let mut mono = color.clone();
    mono.output_channels = 1;
    assert_eq!(
        preview_calibration_outdated(&mono, || panic!("a mono stack reads no solve")),
        None
    );
    // Nor is a color stack whose build did not calibrate.
    let mut uncalibrated = color;
    uncalibrated.color_calibration = None;
    assert_eq!(
        preview_calibration_outdated(&uncalibrated, || panic!("nothing to compare")),
        None
    );
}

/// A clipped map sits beside its stack, `group-2-clipped.fits` for
/// `group-2.fits`, reads back as written, and a rebuild that clipped
/// nothing removes the old one; a missing or multi-channel map reads as
/// none.
#[test]
fn a_clipped_map_lives_beside_its_stack_and_goes_when_nothing_clipped() {
    let root = tempfile::tempdir().unwrap();
    let path = clipped_path(root.path(), "job", 2);
    assert_eq!(path, clipped_path_beside(&fits_path(root.path(), "job", 2)));
    assert_eq!(path.file_name().unwrap(), "group-2-clipped.fits");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    assert!(read_clipped(&path).is_none());

    let map =
        seiza_stacking::LinearImage::new(3, 2, 1, vec![0.0, 0.25, 1.0, 0.5, 0.0, 0.75]).unwrap();
    write_clipped(&path, Some(&map)).unwrap();
    assert_eq!(read_clipped(&path).unwrap(), map);

    write_clipped(&path, None).unwrap();
    assert!(!path.exists());
    write_clipped(&path, None).unwrap();

    let rgb = seiza_stacking::LinearImage::new(1, 1, 3, vec![1.0, 1.0, 1.0]).unwrap();
    write_clipped(&path, Some(&rgb)).unwrap();
    assert!(read_clipped(&path).is_none());
}

#[test]
fn the_sweep_reads_targets_and_skips_one_without_a_project() {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE target (Id INTEGER PRIMARY KEY, name TEXT, projectid INTEGER);
         INSERT INTO target VALUES (1, 'M31', 4), (2, 'Stray', NULL), (3, NULL, 5);",
    )
    .unwrap();
    let targets = targets_by_id(&conn).unwrap();
    assert_eq!(
        targets,
        HashMap::from([(1, (4, "M31".to_string())), (3, (5, String::new()))])
    );
}
