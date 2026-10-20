//! Quality scan and database-wide analysis routes, and the scan of one
//! target that the quality scan runner runs.

use super::*;

// ---------------- Spatial (occlusion) metrics scan ----------------

/// Extract the FITS basename from an acquiredimage metadata JSON blob.
pub(crate) fn filename_from_metadata(metadata_json: &str) -> Option<String> {
    let metadata: serde_json::Value = serde_json::from_str(metadata_json).ok()?;
    let filename = metadata["FileName"].as_str()?;
    filename
        .split(&['\\', '/'][..])
        .next_back()
        .map(|s| s.to_string())
}

/// Fetch the filename-validated stored scan entry for an image, if any.
pub(crate) fn stored_entry_for(
    store: &crate::server::spatial_scan::SharedSpatialStore,
    image_id: i32,
    metadata_json: &str,
    mapped_source_revision: Option<&str>,
) -> Option<crate::server::spatial_scan::StoredSpatialMetrics> {
    let file_only = filename_from_metadata(metadata_json)?;
    crate::server::spatial_scan::valid_quality_entry_for_source(
        store,
        image_id,
        &file_only,
        mapped_source_revision,
    )
}

/// Run the cross-frame photometric pass (transparency, localized extinction,
/// per-cell temporal baselines) over one filter group and merge the signals
/// into its `ImageMetrics`. `entries` parallels `metrics` index-for-index;
/// frames without a stored scan entry contribute empty inputs and receive no
/// signals. Frames are bucketed by exposure, and by instrument where the
/// group holds several (flux ratios are only meaningful within one rig and
/// one exposure length), and split into sessions before the pass.
pub(crate) fn merge_photometric_signals(
    metrics: &mut [crate::sequence_analysis::ImageMetrics],
    entries: &[Option<crate::server::spatial_scan::StoredSpatialMetrics>],
    session_gap_minutes: u64,
) {
    use crate::photometry::{
        sequence_screening_signals, split_sessions, FrameInputs, PhotometryConfig,
    };

    if metrics.len() != entries.len() {
        return;
    }

    // Time order (the analyzer applies the same stable sort later).
    let mut order: Vec<usize> = (0..metrics.len()).collect();
    order.sort_by_key(|&i| (metrics[i].timestamp.unwrap_or(0), metrics[i].image_id));

    // Bucket by exposure seconds, and by instrument where the group holds
    // two or more (see sequence_analysis::instruments_differ).
    let instruments_differ = crate::sequence_analysis::instruments_differ(
        metrics.iter().map(|metric| metric.instrument.as_deref()),
    );
    let mut buckets: std::collections::BTreeMap<(Option<String>, i64), Vec<usize>> =
        std::collections::BTreeMap::new();
    for &i in &order {
        let exposure = entries[i]
            .as_ref()
            .and_then(|e| e.exposure_s)
            .map(|e| e.round() as i64)
            .unwrap_or(-1);
        buckets
            .entry((
                instruments_differ
                    .then(|| metrics[i].instrument.clone())
                    .flatten(),
                exposure,
            ))
            .or_default()
            .push(i);
    }

    let phot_config = PhotometryConfig::default();
    let gap_seconds = (session_gap_minutes * 60) as i64;
    for indices in buckets.values() {
        let timestamps: Vec<Option<i64>> = indices.iter().map(|&i| metrics[i].timestamp).collect();
        for session in split_sessions(&timestamps, gap_seconds) {
            let session_idx: Vec<usize> = session.iter().map(|&s| indices[s]).collect();
            let inputs: Vec<FrameInputs> = session_idx
                .iter()
                .map(|&i| match &entries[i] {
                    Some(e) => FrameInputs {
                        catalog: e.catalog.clone(),
                        star_cell_counts: e.star_cell_counts.clone(),
                        bg_cell_medians: e.bg_cell_medians.clone(),
                    },
                    None => FrameInputs::default(),
                })
                .collect();
            let dims = session_idx.iter().find_map(|&i| {
                entries[i].as_ref().and_then(|e| {
                    (e.width > 0 && e.grid_cols > 0).then_some((
                        e.width,
                        e.height,
                        e.grid_cols,
                        e.grid_rows,
                    ))
                })
            });
            let Some((width, height, grid_cols, grid_rows)) = dims else {
                continue;
            };
            let signals = sequence_screening_signals(
                &inputs,
                width,
                height,
                (grid_cols, grid_rows),
                &phot_config,
            );
            for (&i, sig) in session_idx.iter().zip(signals) {
                let m = &mut metrics[i];
                m.transparency = sig.transparency;
                m.extinction_cell_fraction = sig.extinction_cell_fraction;
                m.star_cell_drop_fraction = sig.star_cell_drop_fraction;
                m.bg_cell_rise_fraction = sig.bg_cell_rise_fraction;
                m.bg_cell_fall_fraction = sig.bg_cell_fall_fraction;
                if let Some(entry) = entries[i].as_ref() {
                    let cells = entry.grid_cols.saturating_mul(entry.grid_rows);
                    if cells > 0 && entry.width > 0 && entry.height > 0 {
                        let low_star_cells = if entry.star_dead_cells.len() == cells {
                            entry.star_dead_cells.clone()
                        } else if entry.star_cell_counts.len() == cells {
                            crate::spatial_analysis::star_dead_cell_flags(
                                &entry.star_cell_counts,
                                crate::spatial_analysis::SpatialAnalysisConfig::default()
                                    .dead_cell_ratio,
                            )
                        } else {
                            vec![false; cells]
                        };
                        m.spatial_evidence =
                            Some(crate::sequence_analysis::QualityRegionEvidence::from_scan(
                                (entry.grid_cols, entry.grid_rows),
                                (entry.width, entry.height),
                                &low_star_cells,
                                &entry.bg_glow_cells,
                                &sig,
                                phot_config.local_extinction_ratio,
                            ));
                    }
                }
            }
        }
    }
}

/// One frame of a quality scan: the work item, the expected target position,
/// and whether it needs the spatial, astrometry and satellite stages.
pub(super) type QualityScanItem = (
    crate::server::spatial_scan::ScanWorkItem,
    Option<(f64, f64)>,
    bool,
    bool,
    bool,
);

/// Give every scanned frame that wants one a photometric zero point, from
/// its aperture-measured stars and its fresh plate solve (just persisted or
/// cached).
///
/// Only a Gaia catalog is used: its single G band keeps the zero point from
/// drifting with star color, which Tycho's mixed VT and BT would not.
pub(super) fn record_frame_zero_points(
    ctx: &DatabaseContext,
    astrometry: &crate::astrometry::AstrometryContext,
    items: &[QualityScanItem],
) {
    let Some(catalog) = astrometry
        .star_catalog()
        .ok()
        .filter(|catalog| catalog.attribution().contains("Gaia"))
    else {
        return;
    };
    let mut attempts = Vec::new();
    for (item, expected, ..) in items {
        let entry = {
            let store = ctx.spatial_metrics.read().unwrap();
            match store.metrics.get(&item.image_id) {
                Some(entry) if entry.filename == item.filename && entry.wants_zero_point() => {
                    entry.clone()
                }
                _ => continue,
            }
        };
        let Some(solution) = astrometry
            .validated_persisted_pixel_analysis(&ctx.cache_dir_path, item.image_id, *expected)
            .and_then(|analysis| analysis.solution)
        else {
            continue;
        };
        let zero_point = entry.exposure_s.and_then(|exposure_s| {
            crate::zero_point::measure(
                &crate::astrometry::wcs_from_response(&solution.wcs),
                solution.image_width as usize,
                solution.image_height as usize,
                &entry.measured_stars(),
                exposure_s,
                catalog.as_ref(),
            )
        });
        attempts.push((item.image_id, zero_point));
    }
    crate::server::spatial_scan::record_zero_points(
        &ctx.spatial_metrics,
        &ctx.cache_dir_path,
        &attempts,
    );
}

/// Merge fresh detector and spatial results from the per-DB quality cache.
/// The scan's star count and HFR go in `scan_stars`, beside the capture
/// software's, for `choose_star_source` to pick between per set; the spatial
/// fields fill values that N.I.N.A. does not store.
pub(crate) fn merge_spatial_metrics(
    metrics: &mut crate::sequence_analysis::ImageMetrics,
    store: &crate::server::spatial_scan::SharedSpatialStore,
    metadata_json: &str,
    mapped_source_revision: Option<&str>,
) {
    let Some(file_only) = filename_from_metadata(metadata_json) else {
        return;
    };
    if let Some(entry) = crate::server::spatial_scan::valid_entry_for_source(
        store,
        metrics.image_id,
        &file_only,
        mapped_source_revision,
    ) {
        let current_detector = crate::server::spatial_scan::is_quality_detector(&entry.detector)
            && entry.detector_version == crate::server::spatial_scan::QUALITY_DETECTOR_VERSION;
        let fallback = entry.detector == crate::server::spatial_scan::FALLBACK_DETECTOR;
        // Measured on calibrated pixels: bias and dark removed, flat
        // divided. Such a frame is scored only against others like it.
        if entry.source_revision.as_deref().is_some_and(|revision| {
            revision.starts_with(crate::server::spatial_scan::CALIBRATED_SOURCE_PREFIX)
        }) {
            metrics.pixel_domain = crate::sequence_analysis::PixelDomain::Calibrated;
        }
        let measured = entry.star_count as f64;
        let recorded = metrics.star_count;
        // One measurement that finds no stars where the other finds plenty
        // is a failed measurement, not a ruined frame: drop it, and what the
        // same pass derived from it, so the score renormalizes instead.
        let scan_failed = current_detector
            && measured == 0.0
            && recorded.is_some_and(|stars| stars >= crate::sequence_analysis::CORROBORATING_STARS);
        if scan_failed {
            tracing::debug!(
                "image {}: quality scan found no stars where the capture software found {:?}; \
                 ignoring the scan's star measurements",
                metrics.image_id,
                recorded
            );
            // Still a scan: the set's source must not flip because of it.
            metrics.scan_stars = Some(crate::sequence_analysis::StarMeasure::default());
        } else if current_detector {
            let corroborates_zero =
                recorded == Some(0.0) && measured < crate::sequence_analysis::CORROBORATING_STARS;
            if recorded == Some(0.0) && !corroborates_zero {
                metrics.star_count = None;
                metrics.hfr = None;
            }
            // A handful of stars beside a recorded zero agrees with it: the
            // frame is as good as starless.
            metrics.scan_stars = Some(if corroborates_zero {
                crate::sequence_analysis::StarMeasure {
                    star_count: Some(0.0),
                    hfr: None,
                }
            } else if fallback {
                // HocusFocus's HFR is on another scale from Fast's; scored
                // beside the set's it could become the reference for all.
                metrics.fallback_stars = Some(crate::sequence_analysis::StarMeasure {
                    star_count: Some(measured),
                    hfr: (entry.avg_hfr > 0.0).then_some(entry.avg_hfr),
                });
                crate::sequence_analysis::StarMeasure {
                    star_count: Some(measured),
                    hfr: None,
                }
            } else {
                crate::sequence_analysis::StarMeasure {
                    star_count: Some(measured),
                    hfr: (entry.avg_hfr > 0.0).then_some(entry.avg_hfr),
                }
            });
        }
        if metrics.dead_cell_fraction.is_none() && !scan_failed {
            metrics.dead_cell_fraction = entry.dead_cell_fraction;
        }
        if metrics.bg_cell_spread.is_none() {
            metrics.bg_cell_spread = Some(entry.bg_cell_spread);
        }
        if metrics.bg_glow_max.is_none() && entry.bg_glow_max > 0.0 {
            metrics.bg_glow_max = Some(entry.bg_glow_max);
        }
        // Capture software rarely records an SNR; the scan's zero point and
        // sky noise fill the scoring dimension that would otherwise drop out.
        if metrics.snr.is_none() {
            metrics.snr = entry.signal_to_noise();
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn merge_astrometry_metrics(
    metrics: &mut crate::sequence_analysis::ImageMetrics,
    cache_dir: &std::path::Path,
    metadata_json: &str,
    evidence: &crate::astrometry::AstrometryEvidenceCache,
    expected_target: Option<(f64, f64)>,
    planned_rotation_deg: Option<f64>,
    mapped_source_path: Option<&std::path::Path>,
    mapped_source_invalid: bool,
) {
    if mapped_source_invalid {
        return;
    }
    let Some(file_only) = filename_from_metadata(metadata_json) else {
        return;
    };
    let Some(analysis) = evidence.evidence_for_source(cache_dir, metrics.image_id, expected_target)
    else {
        return;
    };
    if !cached_pixel_source_matches(
        &analysis.source_fingerprint.canonical_path,
        &file_only,
        mapped_source_path,
    ) {
        return;
    }
    metrics.astrometry = crate::sequence_analysis::astrometry_metrics_from_analysis(&analysis);
    if let Some(astrometry) = metrics.astrometry.as_mut() {
        astrometry.planned_rotation_deg = planned_rotation_deg;
        astrometry.rotator_position_deg = rotator_position_from_metadata(metadata_json);
    }
    metrics.satellite =
        crate::satellites::persisted_analysis(cache_dir, metrics.image_id, &analysis)
            .as_ref()
            .map(crate::sequence_analysis::SatelliteFrameMetrics::from);
}

/// The sky angle the rotator reported, from N.I.N.A.'s `RotatorPosition`.
/// None when no rotator was connected: N.I.N.A. then writes the lowest
/// double as the mechanical position, and its sky angle means nothing.
pub(crate) fn rotator_position_from_metadata(metadata_json: &str) -> Option<f64> {
    let metadata = serde_json::from_str::<serde_json::Value>(metadata_json).ok()?;
    if let Some(mechanical) = metadata.get("RotatorMechanicalPosition")
        && !mechanical
            .as_f64()
            .is_some_and(|degrees| degrees.is_finite() && degrees > -1e300)
    {
        return None;
    }
    metadata
        .get("RotatorPosition")?
        .as_f64()
        .filter(|degrees| degrees.is_finite())
}

/// The file a quality scan measures for this light and the revision it
/// records. A light whose own file is a calibrated copy (no raw frame yet)
/// records `calibrated:<fingerprint>` for that file. With the database's
/// option on, a light with a calibrated copy is measured from it, under the
/// same prefix; otherwise, or when no copy is on disk, from its own file.
/// The prefix is what keeps calibrated measurements out of a raw cohort. A registered copy is never measured:
/// resampling changes the stars, the HFR and the pointing.
pub(super) fn quality_scan_source(
    ctx: &DatabaseContext,
    image: &crate::models::AcquiredImage,
    target_name: &str,
    file_only: &str,
) -> Result<(PathBuf, Option<String>), AppError> {
    if let Some(revision) = rendered_artifact_mapping_revision(ctx, image)? {
        let path = find_fits_file(ctx, image, target_name, file_only)?;
        return Ok((path, Some(format!("mapping:{revision}"))));
    }
    let calibrated = |token: String| {
        format!(
            "{}{token}",
            crate::server::spatial_scan::CALIBRATED_SOURCE_PREFIX
        )
    };
    if own_file_is_calibrated(ctx, image) {
        let path = find_fits_file(ctx, image, target_name, file_only)?;
        let revision = source_file_cache_token(&path).map(calibrated);
        return Ok((path, revision));
    }
    if ctx.scan_calibrated_copies.is_on()
        && let Some(copy) = calibrated_copy_for_scan(ctx, image)
    {
        let revision = format!(
            "{}{}",
            crate::server::spatial_scan::CALIBRATED_SOURCE_PREFIX,
            copy.token
        );
        return Ok((copy.path, Some(revision)));
    }
    let path = find_fits_file(ctx, image, target_name, file_only)?;
    let revision = source_file_cache_token(&path).map(|revision| format!("file:{revision}"));
    Ok((path, revision))
}

/// Whether the light's own file is a calibrated copy: the catalog took it
/// from one because it had no raw frame.
pub(super) fn own_file_is_calibrated(
    ctx: &DatabaseContext,
    image: &crate::models::AcquiredImage,
) -> bool {
    let Some(guid) = image.guid.as_deref() else {
        return false;
    };
    let connection = ctx.db();
    let Ok(connection) = connection.lock() else {
        return false;
    };
    crate::frame_derivatives::derivatives_for_light(&connection, guid)
        .is_ok_and(|records| records.iter().any(|record| record.primary_source))
}

/// The light's calibrated copy with the most steps that is on disk.
pub(super) fn calibrated_copy_for_scan(
    ctx: &DatabaseContext,
    image: &crate::models::AcquiredImage,
) -> Option<ResolvedCopy> {
    let guid = image.guid.as_deref()?;
    let mut records = {
        let connection = ctx.db();
        let connection = connection.lock().ok()?;
        crate::frame_derivatives::derivatives_for_light(&connection, guid).ok()?
    };
    records.retain(|record| {
        record.kind == crate::image_io::FrameKind::Calibrated && !record.primary_source
    });
    records.sort_by_key(|record| std::cmp::Reverse(record.steps.count()));
    records
        .iter()
        .find_map(|record| resolve_copy(ctx, image, &record.derivative_uuid).ok())
}

pub(super) fn cached_pixel_source_matches(
    cached_path: &str,
    filename: &str,
    mapped_source_path: Option<&std::path::Path>,
) -> bool {
    let cached_path = std::path::Path::new(cached_path);
    if let Some(mapped_source_path) = mapped_source_path {
        #[cfg(windows)]
        {
            return dunce::simplified(cached_path)
                .to_string_lossy()
                .eq_ignore_ascii_case(&dunce::simplified(mapped_source_path).to_string_lossy());
        }
        #[cfg(not(windows))]
        {
            return cached_path == mapped_source_path;
        }
    }
    cached_path.file_name().and_then(|name| name.to_str()) == Some(filename)
}

#[cfg(test)]
mod cached_pixel_source_tests;

/// POST /api/db/{db_id}/analysis/spatial-scan
///
/// Queue a person's quality scan of a target's frames on the database's
/// quality scan runner, and return at once. Scans a person requested run
/// before the database-wide analysis. `started` is false only while the
/// server shuts down. The progress shows the scan as preparing until the
/// runner has found the frames that need computing; a scan that finds none
/// finishes without measuring. Poll GET on the same path for progress.
pub async fn start_spatial_scan(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Json(req): Json<SpatialScanRequest>,
) -> Result<Json<ApiResponse<SpatialScanStatusResponse>>, AppError> {
    // One indexed lookup, so a request for a target that is not there fails
    // at once; the runner reads the target's frames later.
    {
        let conn = ctx.db();
        let conn = conn.lock().map_err(AppError::db)?;
        let found = Database::new(&conn)
            .get_targets_by_ids(&[req.target_id])
            .map_err(AppError::db)?;
        if found.is_empty() {
            return Err(AppError::BadRequest(format!(
                "Target {} not found",
                req.target_id
            )));
        }
    }
    let started = crate::server::quality_scans::queue_scan(&state, &ctx.0, req);
    let (progress, cached_count) =
        crate::server::spatial_scan::progress_snapshot(&ctx.spatial_metrics);
    Ok(Json(ApiResponse::success(SpatialScanStatusResponse {
        started,
        progress,
        cached_count,
        scope: None,
    })))
}

/// Scan one target's frames for quality on the calling thread, the quality
/// scan runner's: gather the frames that need spatial, astrometry or
/// satellite work, then measure them. `stop`
/// ends the scan at its next frame; what was measured is still saved.
/// Returns false when no frame needed computing.
#[allow(clippy::too_many_arguments)]
pub(crate) fn scan_target(
    state: &Arc<AppState>,
    ctx: &Arc<DatabaseContext>,
    req: &SpatialScanRequest,
    priority: crate::concurrency::Priority,
    include_satellites: bool,
    stop: &crate::concurrency::Stop,
    runtime: &tokio::runtime::Handle,
) -> Result<bool, AppError> {
    use crate::server::spatial_scan as scan;

    scan::ensure_loaded(&ctx.spatial_metrics, &ctx.cache_dir_path);

    // This is an explicit user-triggered background action, so it may seed the
    // durable orbital cache for an exposure. Read-only sequence queries and
    // CLI regrading remain cache-only.
    let satellite_context = Arc::clone(&state.satellites);

    // Collect this target's images together with schema-adaptive intended
    // framing. Unsupported coordinate epochs are deliberately omitted from
    // absolute grading, though the solver may still use FITS hints.
    // One target's rows, read on the runner thread; the request that queued
    // the scan does not wait for them.
    let (candidates, mapped_sources) = {
        let conn = ctx.db();
        let conn = conn.lock().map_err(AppError::db)?;
        let db = Database::new(&conn);

        let targets = db
            .get_targets_by_ids(&[req.target_id])
            .map_err(AppError::db)?;
        let target = targets
            .into_iter()
            .next()
            .ok_or_else(|| AppError::BadRequest(format!("Target {} not found", req.target_id)))?;
        let target_name = target.name.clone();

        let all_images = db
            .query_images_scoped(None, None, Some(req.target_id), None, 0)
            .map_err(AppError::db)?;

        let mut resolver =
            crate::acquisition_context::FramingResolver::new(&conn).map_err(AppError::db)?;
        let mut candidates = Vec::new();
        for (img, _, _) in all_images.into_iter().filter(|(img, _, _)| {
            img.target_id == req.target_id
                && req
                    .filter_name
                    .as_ref()
                    .is_none_or(|f| crate::utils::filter_names_match(&img.filter_name, f))
        }) {
            let expected = resolver
                .expected_for_grading(&conn, &img)
                .map_err(AppError::db)?;
            candidates.push((img, target_name.clone(), expected));
        }
        let mapped_sources = crate::server::remote_upload::mapped_light_sources(
            &conn,
            &ctx.image_dir_paths,
            candidates.iter().map(|(image, _, _)| image),
        )?;
        (candidates, mapped_sources)
    };

    // Keep the union of spatial, astrometry, and satellite work. One side may
    // already be cached while another still needs computation.
    let mut work = Vec::new();
    // Frames whose metadata never got star metrics (header-first imports):
    // once the scan has measurements, they are written back to the DB —
    // unless the caller opted out of metadata writes.
    let fill_metadata = req.fill_metadata.unwrap_or(true);
    let mut star_fill: Vec<(i32, String, String, Option<String>)> = Vec::new();
    let mut skipped_cached = 0usize;
    let force_spatial = req.force || req.force_spatial;
    let force_astrometry = req.force || req.force_astrometry;
    let force_satellites = req.force || req.force_satellites;
    // A measurement taken from a file that has changed since — a copy still
    // arriving when it was scanned, a frame replaced — does not describe the
    // pixels there now, so it is measured again. Each check reads the file's
    // metadata, which can wait on a network mount.
    let changed_sources: std::collections::HashSet<i32> = if force_spatial {
        Default::default()
    } else {
        let checks: Vec<_> = candidates
            .iter()
            .filter(|(img, _, _)| mapped_sources.quality_revision(img.id).is_none())
            .filter_map(|(img, target_name, _)| {
                let file_only = filename_from_metadata(&img.metadata)?;
                let measured_from =
                    scan::valid_entry(&ctx.spatial_metrics, img.id, &file_only)?.source_revision?;
                // Only own-file and calibrated-copy measurements can go stale
                // this way; a mapped upload has its own revision check.
                if !(measured_from.starts_with("file:")
                    || measured_from.starts_with(scan::CALIBRATED_SOURCE_PREFIX))
                {
                    return None;
                }
                Some((img.clone(), target_name.clone(), file_only, measured_from))
            })
            .collect();
        checks
            .into_iter()
            .filter(|(img, target_name, file_only, measured_from)| {
                // A changed file, or the option to measure calibrated
                // copies turned on or off, both mean another source.
                quality_scan_source(ctx, img, target_name, file_only)
                    .ok()
                    .and_then(|(_, revision)| revision)
                    .is_some_and(|now| now != *measured_from)
            })
            .map(|(img, ..)| img.id)
            .collect()
    };
    for (img, target_name, expected) in candidates {
        if stop.requested() {
            break;
        }
        let Some(file_only) = filename_from_metadata(&img.metadata) else {
            continue;
        };
        if fill_metadata && crate::server::spatial_scan::metadata_wants_star_fill(&img.metadata) {
            star_fill.push((
                img.id,
                file_only.clone(),
                img.metadata.clone(),
                mapped_sources.quality_revision(img.id).map(str::to_owned),
            ));
        }
        let spatial_cached = !force_spatial
            && !changed_sources.contains(&img.id)
            && scan::valid_quality_entry_for_source(
                &ctx.spatial_metrics,
                img.id,
                &file_only,
                mapped_sources.quality_revision(img.id),
            )
            .is_some();
        if spatial_cached {
            skipped_cached += 1;
        }
        let cached_astrometry = (!force_astrometry)
            .then(|| {
                state.astrometry.validated_persisted_pixel_analysis(
                    &ctx.cache_dir_path,
                    img.id,
                    expected,
                )
            })
            .flatten()
            .filter(|analysis| {
                !mapped_sources.is_invalid(img.id)
                    && cached_pixel_source_matches(
                        &analysis.source_fingerprint.canonical_path,
                        &file_only,
                        mapped_sources
                            .get(&img.id)
                            .map(|source| source.path.as_path()),
                    )
            });
        let astrometry_cached = cached_astrometry.is_some();
        // A frame measured and solved earlier, but never given a zero point,
        // needs neither stage again, only the cheap catalog match.
        let zero_point_pending = spatial_cached
            && astrometry_cached
            && ctx
                .spatial_metrics
                .read()
                .unwrap()
                .metrics
                .get(&img.id)
                .is_some_and(crate::server::spatial_scan::StoredSpatialMetrics::wants_zero_point);
        let satellite_cached = !include_satellites
            || (!force_satellites
                && cached_astrometry.as_ref().is_some_and(|analysis| {
                    crate::satellites::persisted_analysis(&ctx.cache_dir_path, img.id, analysis)
                        .is_some()
                }));
        if !spatial_cached || !astrometry_cached || !satellite_cached || zero_point_pending {
            work.push((
                img,
                target_name,
                file_only,
                expected,
                !spatial_cached,
                !astrometry_cached,
                !satellite_cached,
            ));
        }
    }

    if work.is_empty() || stop.requested() {
        // Nothing to compute, but cached measurements may still be missing
        // from imported rows (e.g. a catalog scanned before write-back
        // existed): fill them in without forcing a rescan.
        if !star_fill.is_empty() {
            fill_missing_star_metadata(ctx, &star_fill);
        }
        scan::end_preparing(&ctx.spatial_metrics);
        return Ok(false);
    }

    scan::begin_scan(
        &ctx.spatial_metrics,
        req.target_id,
        req.filter_name.clone(),
        work.iter().filter(|item| item.4).count(),
        skipped_cached,
        priority == crate::concurrency::Priority::Background,
    );

    tracing::info!(
        "📐 Quality scan started for db={} target={} ({} images, {} spatial cached)",
        ctx.id,
        req.target_id,
        work.len(),
        skipped_cached
    );

    let ctx_arc = Arc::clone(ctx);
    let target_id = req.target_id;
    let worker_policy = state.worker_policy();
    let astrometry = Arc::clone(&state.astrometry);
    let runtime_handle = runtime.clone();
    // A person's scan is interactive work for its whole run, so background
    // pre-generation yields cores and memory to it. Dropped when the scan
    // returns or panics.
    let _interactive_guard = (priority == crate::concurrency::Priority::Interactive)
        .then(|| state.begin_named_interactive_job("a quality scan"));
    let scheduling_state = Arc::clone(state);
    {
        // A panic below would leave the scan marked running; it is caught,
        // and the scan is always finalized.
        let scan_body = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // Resolve FITS paths first; unresolvable files count as errors.
            let mut items = Vec::with_capacity(work.len());
            for (
                img,
                target_name,
                file_only,
                expected,
                need_spatial,
                need_astrometry,
                need_satellite,
            ) in &work
            {
                let source_generation = crate::server::spatial_scan::source_generation(
                    &ctx_arc.spatial_metrics,
                    img.id,
                );
                let resolved = quality_scan_source(&ctx_arc, img, target_name, file_only).and_then(
                    |(path, source_revision)| {
                        if source_generation
                            != crate::server::spatial_scan::source_generation(
                                &ctx_arc.spatial_metrics,
                                img.id,
                            )
                        {
                            return Err(AppError::Conflict(
                                "image source changed while preparing the quality scan".into(),
                            ));
                        }
                        Ok((path, source_revision))
                    },
                );
                match resolved {
                    Ok((path, source_revision)) => items.push((
                        crate::server::spatial_scan::ScanWorkItem {
                            image_id: img.id,
                            filename: file_only.clone(),
                            fits_path: path,
                            source_generation,
                            source_revision,
                        },
                        *expected,
                        *need_spatial,
                        *need_astrometry,
                        *need_satellite,
                    )),
                    Err(error) => {
                        let mut s = ctx_arc.spatial_metrics.write().unwrap();
                        // The stage total counts only spatial work; advancing
                        // it for an astrometry-only item would overshoot it.
                        if *need_spatial {
                            s.progress.processed += 1;
                        }
                        s.progress.errors += 1;
                        s.progress.last_error = Some(match error {
                            AppError::Conflict(message) => message,
                            AppError::NotFound => format!("{file_only}: FITS file not found"),
                            other => {
                                tracing::warn!(
                                    "Source resolution failed during quality scan for image {}: {:?}",
                                    img.id,
                                    other
                                );
                                format!("{file_only}: source file could not be resolved")
                            }
                        });
                    }
                }
            }
            let spatial_items = items
                .iter()
                .filter(|item| item.2)
                .map(|item| item.0.clone())
                .collect::<Vec<_>>();
            // Size the worker pool to the machine (configured core ratio) and
            // the sensor (peak memory of one in-flight frame, probed from the
            // first resolvable file). Leaves headroom for request serving.
            let frame_pixels = items
                .first()
                .and_then(|it| crate::concurrency::probe_frame_pixels(&it.0.fits_path));
            let budget =
                crate::concurrency::plan_workers(None, &worker_policy, priority, frame_pixels);
            // Workers come out of the budget other jobs of this priority
            // share, and go back when the scan ends.
            let lease =
                scheduling_state.lease_workers(priority, budget.workers.min(items.len().max(1)));
            // False once the server is stopping: the frames left wait for
            // the next start, and what was measured is still saved below.
            // While it waits, the scan's status names what it waits for, so
            // the header lists it as waiting rather than stalled.
            let wait_for_turn = || {
                if stop.requested() {
                    return false;
                }
                if priority != crate::concurrency::Priority::Background {
                    return !scheduling_state.shutdown.is_stopping();
                }
                scheduling_state.wait_for_interactive_work(
                    std::time::Duration::from_millis(250),
                    |waiting_for| {
                        crate::server::spatial_scan::set_waiting_for(
                            &ctx_arc.spatial_metrics,
                            waiting_for,
                        )
                    },
                )
            };
            if !spatial_items.is_empty() {
                tracing::info!(
                    "📐 Spatial scan concurrency: {} worker(s) — {}; {}",
                    lease.workers,
                    budget.rationale,
                    lease.summary()
                );
                crate::server::spatial_scan::run_scan(
                    &ctx_arc.spatial_metrics,
                    &ctx_arc.cache_dir_path,
                    &spatial_items,
                    lease.workers,
                    &wait_for_turn,
                );
            }

            // The star-detection stage is done (fresh or cached), so imported
            // rows without star metrics can take the measured values now,
            // before the (much slower) astrometry stage.
            fill_missing_star_metadata(&ctx_arc, &star_fill);

            let astrometry_items = items
                .iter()
                .filter(|item| item.3 || item.4)
                .collect::<Vec<_>>();
            if !astrometry_items.is_empty() {
                crate::server::spatial_scan::begin_astrometry_stage(
                    &ctx_arc.spatial_metrics,
                    astrometry_items.len(),
                );
                // Solves run side by side within the same budget as the
                // spatial stage; Seiza's own threads stay inside it too.
                let workers = lease.workers.min(astrometry_items.len());
                tracing::info!(
                    "📐 Astrometry concurrency: {workers} worker(s) — {}; {}",
                    budget.rationale,
                    lease.summary()
                );
                crate::concurrency::parallel_in_pool(astrometry_items.len(), workers, |index| {
                    let (item, expected, _, need_astrometry, need_satellite) =
                        astrometry_items[index];
                    if !wait_for_turn() {
                        return;
                    }
                    crate::server::spatial_scan::begin_astrometry_item(
                        &ctx_arc.spatial_metrics,
                        &item.filename,
                    );
                    // No per-database solve lock: the worker budget bounds the
                    // scan's memory, and an on-demand solve is interactive, so
                    // the scan waits for it before its next frame instead.
                    let outcome = if *need_astrometry {
                        astrometry.solve_image_for_quality(
                            item.image_id,
                            &item.fits_path,
                            *expected,
                        )
                    } else {
                        astrometry
                            .validated_persisted_pixel_analysis(
                                &ctx_arc.cache_dir_path,
                                item.image_id,
                                *expected,
                            )
                            .ok_or_else(|| "cached plate solution became unavailable".to_string())
                    };
                    match outcome {
                        Ok(analysis) => {
                            let attempt = analysis.solve_attempt.as_ref();
                            let solved = attempt.is_some_and(|attempt| {
                                attempt.outcome
                                    == crate::astrometry::AstrometryAttemptOutcome::Solved
                            });
                            let quality_failure = attempt
                                .is_some_and(|attempt| attempt.image_quality_evidence && !solved);
                            let operational_error =
                                if attempt.is_some_and(|attempt| attempt.cacheable) {
                                    crate::astrometry::persist_pixel_analysis(
                                        &ctx_arc.cache_dir_path,
                                        &analysis,
                                    )
                                    .err()
                                } else if !solved && !quality_failure {
                                    analysis.error.clone()
                                } else {
                                    None
                                };
                            if *need_satellite && solved {
                                match runtime_handle
                                    .block_on(satellite_context.load_for_exposure(&item.fits_path))
                                {
                                    Ok(snapshot) => {
                                        if let Err(error) = crate::satellites::predict_tracks(
                                            item.image_id,
                                            &item.fits_path,
                                            &analysis,
                                            &snapshot,
                                        )
                                        .and_then(|prediction| {
                                            crate::satellites::persist_analysis(
                                                &ctx_arc.cache_dir_path,
                                                &prediction,
                                            )
                                        }) {
                                            tracing::warn!(
                                                "Satellite prediction unavailable for {}: {}",
                                                item.filename,
                                                error
                                            );
                                        }
                                    }
                                    Err(error) => tracing::warn!(
                                        "Satellite elements unavailable for {}: {}",
                                        item.filename,
                                        error
                                    ),
                                }
                            }
                            crate::server::spatial_scan::record_astrometry_result(
                                &ctx_arc.spatial_metrics,
                                &item.filename,
                                solved,
                                quality_failure,
                                operational_error,
                            );
                        }
                        Err(error) => crate::server::spatial_scan::record_astrometry_result(
                            &ctx_arc.spatial_metrics,
                            &item.filename,
                            false,
                            false,
                            Some(error),
                        ),
                    }
                });
            }
            record_frame_zero_points(&ctx_arc, &astrometry, &items);
            crate::server::spatial_scan::finalize_scan(&ctx_arc.spatial_metrics);
        }));
        if scan_body.is_err() {
            tracing::error!(
                "📐 Spatial scan panicked for db={} target={}; finalizing progress",
                ctx_arc.id,
                target_id
            );
            crate::server::spatial_scan::finalize_scan(&ctx_arc.spatial_metrics);
        } else {
            tracing::info!(
                "📐 Quality scan finished for db={} target={}",
                ctx_arc.id,
                target_id
            );
        }
    }
    Ok(true)
}

/// GET /api/db/{db_id}/analysis/spatial-scan — progress + store size.
pub async fn get_spatial_scan_progress(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Query(query): Query<SpatialScanStatusQuery>,
) -> Result<Json<ApiResponse<SpatialScanStatusResponse>>, AppError> {
    use crate::server::spatial_scan as scan;

    scan::ensure_loaded(&ctx.spatial_metrics, &ctx.cache_dir_path);
    let scope = if let Some(target_id) = query.target_id {
        let images = {
            let conn = ctx.db();
            let conn = conn.lock().map_err(AppError::db)?;
            Database::new(&conn)
                .query_images_scoped(None, None, Some(target_id), None, 0)
                .map_err(AppError::db)?
        };
        let mut total_frames = 0usize;
        let mut new_frames = 0usize;
        let mut outdated_frames = 0usize;
        let store = ctx.spatial_metrics.read().unwrap();
        for (image, _, _) in images.into_iter().filter(|(image, _, _)| {
            query
                .filter_name
                .as_ref()
                .is_none_or(|filter| crate::utils::filter_names_match(&image.filter_name, filter))
        }) {
            let Some(filename) = filename_from_metadata(&image.metadata) else {
                continue;
            };
            total_frames += 1;
            match store.metrics.get(&image.id) {
                Some(entry) if scan::quality_entry_is_current(entry, &filename) => {}
                Some(entry) if entry.filename == filename => outdated_frames += 1,
                _ => new_frames += 1,
            }
        }
        drop(store);
        let pending_frames = new_frames + outdated_frames;
        Some(QualityScanScopeStatus {
            target_id,
            filter_name: query.filter_name,
            total_frames,
            pending_frames,
            new_frames,
            outdated_frames,
            needs_analysis: pending_frames > 0,
        })
    } else {
        None
    };
    let (mut progress, cached_count) = scan::progress_snapshot(&ctx.spatial_metrics);
    // Between one scan's end and the next's start, a queued scan still runs
    // as far as a page following it is concerned.
    if !progress.running && crate::server::quality_scans::scan_queued(&state, &ctx) {
        progress.running = true;
    }
    Ok(Json(ApiResponse::success(SpatialScanStatusResponse {
        started: progress.running,
        progress,
        cached_count,
        scope,
    })))
}
