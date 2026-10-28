use super::*;

fn legacy_target_tree_entry() -> crate::db_registry::DbEntry {
    crate::db_registry::DbEntry {
        ts_sync: None,
        id: "catalog".into(),
        name: "Catalog".into(),
        db_path: "catalog.sqlite".into(),
        image_dirs: vec!["images".into()],
        reject_archive: None,
        remote_image_upload: Some(crate::db_registry::RemoteImageUploadConfig {
            placement: crate::db_registry::RemoteImageUploadPlacement::TargetTree,
            ..Default::default()
        }),
        export_dir: None,
        process_dir: None,
        autoimport: None,
        scan_rules: None,
        analyze_new_frames: false,
        pair_calibrated_copies: true,
        scan_calibrated_copies: false,
        processing_overrides: None,
    }
}

fn update() -> RemoteImageUploadUpdate {
    RemoteImageUploadUpdate {
        enabled: false,
        image_directory: None,
        token: None,
        sync_enabled: None,
        placement: Some(crate::db_registry::RemoteImageUploadPlacement::TargetTree),
        directory_template: None,
        rescan_directory_layout: false,
        directory_template_source: None,
    }
}

fn catalog_matched_entry() -> crate::db_registry::DbEntry {
    let mut entry = legacy_target_tree_entry();
    // Disabled, like the test request: enabling needs a token, and the
    // enabled flag flipping is itself a reason to scan.
    entry.remote_image_upload = Some(crate::db_registry::RemoteImageUploadConfig {
        enabled: false,
        image_dir: "images".into(),
        placement: crate::db_registry::RemoteImageUploadPlacement::TargetTree,
        directory_layout: Some(crate::db_registry::RemoteImageUploadDirectoryLayout {
            template: "ZWO ASI2600MM Pro/%TARGET%/NIGHT_%NIGHT%/%FILTER%/%TYPE%".into(),
            fallback_template: "%YEAR%/%TARGET%/%NIGHT%/%TYPE%".into(),
            source: crate::db_registry::RemoteImageUploadTemplateSource::Catalog,
            samples: 157,
        }),
        ..Default::default()
    });
    entry
}

fn detected(
    template: &str,
    samples: usize,
) -> crate::server::remote_upload_layout::DetectedDirectoryLayout {
    crate::server::remote_upload_layout::DetectedDirectoryLayout {
        template: template.into(),
        samples,
    }
}

#[test]
fn a_first_target_tree_save_still_scans_the_catalog() {
    // The desktop form sends `preset` on every save that has no match yet
    // (there is nothing else to send), so that must not suppress the
    // first detection, and the detection must still win: the preset was
    // only what the form showed by default.
    let mut previous_flat = legacy_target_tree_entry();
    previous_flat
        .remote_image_upload
        .as_mut()
        .unwrap()
        .placement = crate::db_registry::RemoteImageUploadPlacement::Flat;
    let mut entry = previous_flat.clone();
    let mut request = update();
    request.directory_template = Some("%TARGET%/%TYPE%/%FILTER%".into());
    request.directory_template_source =
        Some(crate::db_registry::RemoteImageUploadTemplateSource::Preset);
    // The form only says `preset` for a layout the person touched, so on
    // this first switch the pick holds against the scan.
    let chosen = apply_remote_image_upload_update(&mut entry, &request).unwrap();
    assert!(chosen, "a touched pick holds even on the first switch");
    adopt_detected_layout(
        entry.remote_image_upload.as_mut().unwrap(),
        Some(detected("%YEAR%/%TARGET%/%NIGHT%/%TYPE%", 40)),
        chosen,
    );
    assert_eq!(
        entry
            .remote_image_upload
            .as_ref()
            .unwrap()
            .directory_template(),
        "%TARGET%/%TYPE%/%FILTER%"
    );

    // An older client, or an untouched form, sends no source: on the
    // first switch the detection still decides.
    let mut entry = previous_flat.clone();
    let mut request = update();
    request.directory_template = Some("%TARGET%/%TYPE%/%FILTER%".into());
    let chosen = apply_remote_image_upload_update(&mut entry, &request).unwrap();
    assert!(
        !chosen,
        "without a source there is no way to tell a pick from the default"
    );
    assert_eq!(
        entry
            .remote_image_upload
            .as_ref()
            .unwrap()
            .directory_template(),
        "%TARGET%/%TYPE%/%FILTER%",
        "until the scan reports, the preset is in force"
    );
    assert!(
        remote_image_layout_needs_scan(&previous_flat, &entry, Some(&request)),
        "switching to the catalog tree is what triggers detection"
    );
    let config = entry.remote_image_upload.as_mut().unwrap();
    // The first detection wins, with the preset kept as the fallback.
    adopt_detected_layout(
        config,
        Some(detected("%YEAR%/%TARGET%/%NIGHT%/%TYPE%", 40)),
        chosen,
    );
    assert_eq!(
        config.directory_template(),
        "%YEAR%/%TARGET%/%NIGHT%/%TYPE%"
    );
    assert_eq!(
        config.directory_layout.as_ref().unwrap().fallback_template,
        "%TARGET%/%TYPE%/%FILTER%"
    );
    // An empty scan returns a catalog match to its fallback.
    adopt_detected_layout(config, None, false);
    assert_eq!(config.directory_template(), "%TARGET%/%TYPE%/%FILTER%");
}

#[test]
fn the_form_echoing_a_legacy_entry_leaves_it_legacy() {
    // A target_tree entry with no layout at all is summarized as the
    // default template with source `preset`, which the form echoes on
    // every save. That echo must not create an explicit layout: one
    // moves calibration frames from BIAS/ to the template's tree.
    let mut entry = legacy_target_tree_entry();
    let mut request = update();
    request.directory_template =
        Some(crate::db_registry::DEFAULT_REMOTE_UPLOAD_DIRECTORY_TEMPLATE.into());
    request.directory_template_source =
        Some(crate::db_registry::RemoteImageUploadTemplateSource::Preset);
    let chosen = apply_remote_image_upload_update(&mut entry, &request).unwrap();
    assert!(!chosen);
    assert!(entry
        .remote_image_upload
        .unwrap()
        .directory_layout
        .is_none());
}

#[test]
fn an_explicit_rescan_beats_a_layout_in_force() {
    // A Preset layout is in force and the form is in preset mode, so the
    // save says `preset` with the same template; the person also clicked
    // Rescan. The rescan is the later wish and its result is adopted.
    let mut entry = legacy_target_tree_entry();
    entry.remote_image_upload.as_mut().unwrap().directory_layout =
        Some(crate::db_registry::RemoteImageUploadDirectoryLayout {
            template: "%TARGET%/%DATE%/%TYPE%".into(),
            fallback_template: "%TARGET%/%DATE%/%TYPE%".into(),
            source: crate::db_registry::RemoteImageUploadTemplateSource::Preset,
            samples: 0,
        });
    let mut request = update();
    request.directory_template = Some("%TARGET%/%DATE%/%TYPE%".into());
    request.directory_template_source =
        Some(crate::db_registry::RemoteImageUploadTemplateSource::Preset);
    request.rescan_directory_layout = true;
    let chosen = apply_remote_image_upload_update(&mut entry, &request).unwrap();
    assert!(!chosen, "the same template is no new choice");
    let config = entry.remote_image_upload.as_mut().unwrap();
    adopt_detected_layout(
        config,
        Some(detected("%YEAR%/%TARGET%/%NIGHT%/%TYPE%", 12)),
        chosen && !request.rescan_directory_layout,
    );
    assert_eq!(
        config.directory_template(),
        "%YEAR%/%TARGET%/%NIGHT%/%TYPE%"
    );
    assert_eq!(
        config.directory_layout.as_ref().unwrap().fallback_template,
        "%TARGET%/%DATE%/%TYPE%"
    );
}

#[test]
fn an_older_client_echoing_the_fallback_changes_nothing() {
    // The summary hands the fallback back as `directory_template`; a
    // client from before the source field re-sends it on unrelated
    // saves. That is not a choice.
    let mut entry = catalog_matched_entry();
    let mut request = update();
    request.directory_template = Some("%YEAR%/%TARGET%/%NIGHT%/%TYPE%".into());
    let chosen = apply_remote_image_upload_update(&mut entry, &request).unwrap();
    assert!(!chosen);
    let layout = entry.remote_image_upload.unwrap().directory_layout.unwrap();
    assert_eq!(
        layout.source,
        crate::db_registry::RemoteImageUploadTemplateSource::Catalog
    );
    assert_eq!(layout.samples, 157);
}

#[test]
fn a_chosen_layout_replaces_the_catalog_match_and_is_not_scanned_away() {
    // #399: the person chose Custom over a wrong catalog match, and the
    // save filed the custom layout as the fallback while the catalog
    // match stayed in force, then re-scanned and kept it that way.
    let previous = catalog_matched_entry();
    let mut entry = catalog_matched_entry();
    let mut request = update();
    request.placement = Some(crate::db_registry::RemoteImageUploadPlacement::TargetTree);
    request.directory_template = Some("%CAMERA%/%TARGET%/NIGHT_%NIGHT%/%FILTER%/%TYPE%".into());
    request.directory_template_source =
        Some(crate::db_registry::RemoteImageUploadTemplateSource::Preset);
    apply_remote_image_upload_update(&mut entry, &request).unwrap();

    let config = entry.remote_image_upload.as_ref().unwrap();
    let layout = config.directory_layout.as_ref().unwrap();
    assert_eq!(
        layout.template,
        "%CAMERA%/%TARGET%/NIGHT_%NIGHT%/%FILTER%/%TYPE%"
    );
    assert_eq!(layout.fallback_template, layout.template);
    assert_eq!(
        layout.source,
        crate::db_registry::RemoteImageUploadTemplateSource::Preset
    );
    assert_eq!(config.directory_template(), layout.template);
    assert!(
        !remote_image_layout_needs_scan(&previous, &entry, Some(&request)),
        "a layout change alone is no reason to scan"
    );
    // Even with a rescan queued in the same save, the scan's result does
    // not climb back over the layout the person just chose.
    let mut entry = catalog_matched_entry();
    request.rescan_directory_layout = true;
    let chosen = apply_remote_image_upload_update(&mut entry, &request).unwrap();
    let config = entry.remote_image_upload.as_mut().unwrap();
    adopt_detected_layout(
        config,
        Some(detected(
            "ZWO ASI2600MM Pro/%TARGET%/NIGHT_%NIGHT%/%FILTER%/%TYPE%",
            157,
        )),
        chosen,
    );
    assert_eq!(
        config.directory_template(),
        "%CAMERA%/%TARGET%/NIGHT_%NIGHT%/%FILTER%/%TYPE%"
    );

    // The same choice without naming its source is still a choice: the
    // template differs from the one in force.
    let mut entry = catalog_matched_entry();
    let mut request = update();
    request.directory_template = Some("%TARGET%/%NIGHT%/%TYPE%".into());
    apply_remote_image_upload_update(&mut entry, &request).unwrap();
    let layout = entry.remote_image_upload.unwrap().directory_layout.unwrap();
    assert_eq!(layout.template, "%TARGET%/%NIGHT%/%TYPE%");
    assert_eq!(
        layout.source,
        crate::db_registry::RemoteImageUploadTemplateSource::Preset
    );
}

#[test]
fn keeping_match_catalog_ignores_any_template_sent_along() {
    let previous = catalog_matched_entry();
    let mut entry = catalog_matched_entry();
    let mut request = update();
    request.directory_template = Some("%TARGET%/%DATE%/%TYPE%".into());
    request.directory_template_source =
        Some(crate::db_registry::RemoteImageUploadTemplateSource::Catalog);
    apply_remote_image_upload_update(&mut entry, &request).unwrap();
    let config = entry.remote_image_upload.as_ref().unwrap();
    let layout = config.directory_layout.as_ref().unwrap();
    assert_eq!(
        layout.template,
        "ZWO ASI2600MM Pro/%TARGET%/NIGHT_%NIGHT%/%FILTER%/%TYPE%"
    );
    assert_eq!(layout.fallback_template, "%YEAR%/%TARGET%/%NIGHT%/%TYPE%");
    assert_eq!(layout.samples, 157);
    assert!(
        !remote_image_layout_needs_scan(&previous, &entry, Some(&request)),
        "nothing changed, so nothing to rescan"
    );
}

#[test]
fn unrelated_save_preserves_legacy_layout_and_skips_the_tree_scan() {
    let previous = legacy_target_tree_entry();
    let mut updated = previous.clone();
    apply_remote_image_upload_update(&mut updated, &update()).unwrap();

    assert!(updated
        .remote_image_upload
        .as_ref()
        .unwrap()
        .directory_layout
        .is_none());
    assert!(!remote_image_layout_needs_scan(
        &previous,
        &updated,
        Some(&update())
    ));

    let mut rescan = update();
    rescan.rescan_directory_layout = true;
    assert!(remote_image_layout_needs_scan(
        &previous,
        &updated,
        Some(&rescan)
    ));
}

#[test]
fn a_failed_or_empty_scan_returns_a_catalog_layout_to_its_preset() {
    let mut config = crate::db_registry::RemoteImageUploadConfig {
        directory_layout: Some(crate::db_registry::RemoteImageUploadDirectoryLayout {
            template: "%YEAR%/%TARGET%/%NIGHT%/%TYPE%".into(),
            fallback_template: "%TARGET%/%DATE%/%TYPE%".into(),
            source: crate::db_registry::RemoteImageUploadTemplateSource::Catalog,
            samples: 42,
        }),
        ..Default::default()
    };

    reset_catalog_directory_layout(&mut config);

    let layout = config.directory_layout.unwrap();
    assert_eq!(layout.template, "%TARGET%/%DATE%/%TYPE%");
    assert_eq!(layout.fallback_template, "%TARGET%/%DATE%/%TYPE%");
    assert_eq!(
        layout.source,
        crate::db_registry::RemoteImageUploadTemplateSource::Preset
    );
    assert_eq!(layout.samples, 0);
}
