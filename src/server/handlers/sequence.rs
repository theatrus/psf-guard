//! Sequence analysis and per-image quality routes.

use super::*;

// Sequence analysis handlers

#[axum::debug_handler(state = Arc<AppState>)]
pub async fn analyze_sequence(
    ctx: DbContext,
    Query(params): Query<crate::server::api::SequenceAnalysisQuery>,
) -> Result<Json<ApiResponse<crate::server::api::SequenceAnalysisResponse>>, AppError> {
    use crate::sequence_analysis::{
        extract_light_metrics, QualityWeights, SequenceAnalyzer, SequenceAnalyzerConfig,
    };

    let filter_name = params.filter_name.clone();
    let session_gap = params.session_gap_minutes;
    let weight_star_count = params.weight_star_count;
    let weight_hfr = params.weight_hfr;
    let weight_eccentricity = params.weight_eccentricity;
    let weight_snr = params.weight_snr;
    let weight_background = params.weight_background;
    let weight_spatial = params.weight_spatial;
    let weight_pointing = params.weight_pointing;
    let scoring_overrides = params.scoring_overrides();

    // Fetch images from the requested target, project, or database. Wider
    // scopes still score each target/filter group independently and only read
    // evidence already stored in metadata or caches.
    let (images_data, expected_by_image, planned_rotation_by_image, mapped_sources) = {
        let conn = ctx.db();
        let conn = conn.lock().map_err(AppError::db)?;
        let db = Database::new(&conn);

        let target_ids = match (params.target_id, params.project_id, params.all_projects) {
            (Some(target_id), None, false) => {
                let targets = db.get_targets_by_ids(&[target_id]).map_err(AppError::db)?;
                if targets.is_empty() {
                    return Err(AppError::BadRequest(format!(
                        "Target {} not found",
                        target_id
                    )));
                }
                Some(std::collections::HashSet::from([target_id]))
            }
            (None, Some(project_id), false) => Some(
                db.get_targets_with_images(project_id)
                    .map_err(AppError::db)?
                    .into_iter()
                    .map(|(target, _, _, _)| target.id)
                    .collect(),
            ),
            (None, None, true) => None,
            _ => {
                return Err(AppError::BadRequest(
                    "Specify exactly one of target_id, project_id, or all_projects=true"
                        .to_string(),
                ));
            }
        };

        let mut resolver =
            crate::acquisition_context::FramingResolver::new(&conn).map_err(AppError::db)?;
        let mut images_data = Vec::new();
        let mut expected_by_image = std::collections::HashMap::new();
        let mut planned_rotation_by_image = std::collections::HashMap::new();
        let all_images = db
            .query_images_scoped(None, params.project_id, params.target_id, None, 0)
            .map_err(AppError::db)?;
        for image in all_images.into_iter().filter(|(image, _, _)| {
            target_ids
                .as_ref()
                .is_none_or(|target_ids| target_ids.contains(&image.target_id))
                && filter_name.as_ref().is_none_or(|filter| {
                    crate::utils::filter_names_match(&image.filter_name, filter)
                })
        }) {
            expected_by_image.insert(
                image.0.id,
                resolver
                    .expected_for_grading(&conn, &image.0)
                    .map_err(AppError::db)?,
            );
            planned_rotation_by_image.insert(
                image.0.id,
                resolver
                    .expected_framing(&conn, &image.0)
                    .map_err(AppError::db)?
                    .and_then(|framing| framing.rotation_deg),
            );
            images_data.push(image);
        }

        let mapped_sources = crate::server::remote_upload::mapped_light_sources(
            &conn,
            &ctx.image_dir_paths,
            images_data.iter().map(|(image, _, _)| image),
        )?;
        (
            images_data,
            expected_by_image,
            planned_rotation_by_image,
            mapped_sources,
        )
    };

    if images_data.is_empty() {
        return Ok(Json(ApiResponse::success(
            crate::server::api::SequenceAnalysisResponse {
                sequences: vec![],
                target_filter_rollups: vec![],
            },
        )));
    }

    // Extract metrics and group by filter. A prior quality scan supplies fresh
    // star/HFR measurements plus the spatial fields N.I.N.A. does not store.
    crate::server::spatial_scan::ensure_loaded(&ctx.spatial_metrics, &ctx.cache_dir_path);
    let spatial_store = ctx.spatial_metrics.clone();
    let astrometry_cache_dir = ctx.cache_dir_path.clone();
    let astrometry_evidence = ctx.astrometry_evidence.clone();
    let result = tokio::task::spawn_blocking(move || {
        let mut config = SequenceAnalyzerConfig::default();
        if let Some(gap) = session_gap {
            config.session_gap_minutes = gap;
        }
        // Apply weight overrides from query params if any are provided
        if weight_star_count.is_some()
            || weight_hfr.is_some()
            || weight_eccentricity.is_some()
            || weight_snr.is_some()
            || weight_background.is_some()
            || weight_spatial.is_some()
            || weight_pointing.is_some()
        {
            config.quality_weights = QualityWeights {
                star_count: weight_star_count.unwrap_or(config.quality_weights.star_count),
                hfr: weight_hfr.unwrap_or(config.quality_weights.hfr),
                eccentricity: weight_eccentricity.unwrap_or(config.quality_weights.eccentricity),
                snr: weight_snr.unwrap_or(config.quality_weights.snr),
                background: weight_background.unwrap_or(config.quality_weights.background),
                spatial: weight_spatial.unwrap_or(config.quality_weights.spatial),
                transparency: config.quality_weights.transparency,
                pointing: weight_pointing.unwrap_or(config.quality_weights.pointing),
            };
        }
        // Scoring overrides: penalty scales (how hard satellite, pointing,
        // and temporal evidence hits the score) and absolute reject limits.
        scoring_overrides.apply_to(&mut config);

        let session_gap_minutes = config.session_gap_minutes;
        let analyzer = SequenceAnalyzer::new(config);

        // Group by target and filter so project analysis preserves the same
        // comparison boundaries as one-target Sequence analysis.
        let mut by_group: std::collections::HashMap<(i32, String, String), Vec<_>> =
            std::collections::HashMap::new();
        let mut entries_by_group: std::collections::HashMap<(i32, String, String), Vec<_>> =
            std::collections::HashMap::new();
        let mut display_filter_names: std::collections::HashMap<(i32, String, String), String> =
            std::collections::HashMap::new();
        for (img, _project_name, target_name) in &images_data {
            let mut metrics = extract_light_metrics(img);
            let mapped_source = mapped_sources.get(&img.id);
            let mapped_source_revision = mapped_sources.quality_revision(img.id);
            merge_spatial_metrics(
                &mut metrics,
                &spatial_store,
                &img.metadata,
                mapped_source_revision,
            );
            merge_astrometry_metrics(
                &mut metrics,
                &astrometry_cache_dir,
                &img.metadata,
                &astrometry_evidence,
                expected_by_image.get(&img.id).copied().flatten(),
                planned_rotation_by_image.get(&img.id).copied().flatten(),
                mapped_source.map(|source| source.path.as_path()),
                mapped_sources.is_invalid(img.id),
            );
            // Key on the normalized filter so case or whitespace variants of
            // one physical filter form ONE comparison cohort; the raw name
            // of the group's first frame stays as the display name.
            let group = (
                img.target_id,
                target_name.clone(),
                crate::utils::normalized_filter_name(&img.filter_name),
            );
            display_filter_names
                .entry(group.clone())
                .or_insert_with(|| img.filter_name.clone());
            entries_by_group
                .entry(group.clone())
                .or_default()
                .push(stored_entry_for(
                    &spatial_store,
                    img.id,
                    &img.metadata,
                    mapped_source_revision,
                ));
            by_group.entry(group).or_default().push(metrics);
        }

        let mut all_sequences = Vec::new();
        let mut target_filter_rollups = Vec::new();
        for ((target_id, target_name, filter), mut metrics) in by_group {
            let key = (target_id, target_name.clone(), filter.clone());
            if let Some(entries) = entries_by_group.get(&key) {
                merge_photometric_signals(&mut metrics, entries, session_gap_minutes);
            }
            let display_filter = display_filter_names
                .get(&key)
                .cloned()
                .unwrap_or_else(|| filter.clone());
            let (scored, rollup) = analyzer.analyze_with_target_filter_rollup(
                &metrics,
                target_id,
                &target_name,
                &display_filter,
            );
            all_sequences.extend(scored);
            if let Some(rollup) = rollup {
                target_filter_rollups.push(rollup);
            }
        }

        (all_sequences, target_filter_rollups)
    })
    .await
    .map_err(|e| AppError::InternalError(format!("Analysis task failed: {}", e)))?;

    let mut sequences: Vec<crate::server::api::ScoredSequenceResponse> =
        result.0.into_iter().map(Into::into).collect();
    let mut target_filter_rollups: Vec<crate::server::api::TargetFilterRollupResponse> =
        result.1.into_iter().map(Into::into).collect();
    sequences.sort_by(|a, b| match (a.session_start, b.session_start) {
        (Some(left), Some(right)) => left
            .cmp(&right)
            .then_with(|| a.target_id.cmp(&b.target_id))
            .then_with(|| a.filter_name.cmp(&b.filter_name)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a
            .target_id
            .cmp(&b.target_id)
            .then_with(|| a.filter_name.cmp(&b.filter_name)),
    });
    target_filter_rollups.sort_by(|a, b| {
        a.target_id
            .cmp(&b.target_id)
            .then_with(|| a.filter_name.cmp(&b.filter_name))
    });

    Ok(Json(ApiResponse::success(
        crate::server::api::SequenceAnalysisResponse {
            sequences,
            target_filter_rollups,
        },
    )))
}

#[axum::debug_handler(state = Arc<AppState>)]
pub async fn get_image_quality(
    ctx: DbContext,
    Path((_db_id, image_id)): Path<(String, i32)>,
    Query(overrides): Query<crate::server::api::ScoringOverrideQuery>,
) -> Result<Json<ApiResponse<crate::server::api::ImageQualityContextResponse>>, AppError> {
    use crate::sequence_analysis::{
        extract_light_metrics, SequenceAnalyzer, SequenceAnalyzerConfig,
    };

    // Get the target image and its context from database
    let (
        target_image,
        all_filter_images,
        target_name,
        expected_by_image,
        planned_rotation_by_image,
        mapped_sources,
    ) = {
        let conn = ctx.db();
        let conn = conn.lock().map_err(AppError::db)?;
        let db = Database::new(&conn);

        let images = db.get_images_by_ids(&[image_id]).map_err(AppError::db)?;
        let target_image = images.into_iter().next().ok_or(AppError::NotFound)?;

        // Get target name
        let targets = db
            .get_targets_by_ids(&[target_image.target_id])
            .map_err(AppError::db)?;
        let target = targets.into_iter().next().ok_or(AppError::NotFound)?;
        let target_name = target.name.clone();

        // Get all images for the same target + filter
        let all_images = db
            .query_images_scoped(None, None, Some(target_image.target_id), None, 0)
            .map_err(AppError::db)?;

        let filter_images: Vec<_> = all_images
            .into_iter()
            .filter(|(img, _, _)| {
                img.target_id == target_image.target_id
                    && crate::utils::filter_names_match(&img.filter_name, &target_image.filter_name)
            })
            .collect();
        let mut resolver =
            crate::acquisition_context::FramingResolver::new(&conn).map_err(AppError::db)?;
        let expected_by_image = filter_images
            .iter()
            .map(|(image, _, _)| {
                resolver
                    .expected_for_grading(&conn, image)
                    .map(|expected| (image.id, expected))
            })
            .collect::<Result<std::collections::HashMap<_, _>, _>>()
            .map_err(AppError::db)?;
        let planned_rotation_by_image = filter_images
            .iter()
            .map(|(image, _, _)| {
                resolver
                    .expected_framing(&conn, image)
                    .map(|framing| (image.id, framing.and_then(|framing| framing.rotation_deg)))
            })
            .collect::<Result<std::collections::HashMap<_, _>, _>>()
            .map_err(AppError::db)?;

        let mapped_sources = crate::server::remote_upload::mapped_light_sources(
            &conn,
            &ctx.image_dir_paths,
            filter_images.iter().map(|(image, _, _)| image),
        )?;

        (
            target_image,
            filter_images,
            target_name,
            expected_by_image,
            planned_rotation_by_image,
            mapped_sources,
        )
    };

    if all_filter_images.is_empty() {
        return Ok(Json(ApiResponse::success(
            crate::server::api::ImageQualityContextResponse {
                image_id,
                quality: None,
                sequence_target_id: None,
                sequence_filter_name: None,
                sequence_image_count: None,
                reference_values: None,
            },
        )));
    }

    let filter_name = target_image.filter_name.clone();
    let filter_name_for_task = filter_name.clone();
    let seq_target_id = target_image.target_id;
    let target_name_clone = target_name.clone();
    crate::server::spatial_scan::ensure_loaded(&ctx.spatial_metrics, &ctx.cache_dir_path);
    let spatial_store = ctx.spatial_metrics.clone();
    let astrometry_cache_dir = ctx.cache_dir_path.clone();
    let astrometry_evidence = ctx.astrometry_evidence.clone();

    let result = tokio::task::spawn_blocking(move || {
        let mut config = SequenceAnalyzerConfig::default();
        overrides.apply_to(&mut config);
        let session_gap_minutes = config.session_gap_minutes;
        let analyzer = SequenceAnalyzer::new(config);

        let mut metrics: Vec<_> = Vec::with_capacity(all_filter_images.len());
        let mut entries = Vec::with_capacity(all_filter_images.len());
        for (img, _, _) in &all_filter_images {
            let mut m = extract_light_metrics(img);
            let mapped_source = mapped_sources.get(&img.id);
            let mapped_source_revision = mapped_sources.quality_revision(img.id);
            merge_spatial_metrics(
                &mut m,
                &spatial_store,
                &img.metadata,
                mapped_source_revision,
            );
            merge_astrometry_metrics(
                &mut m,
                &astrometry_cache_dir,
                &img.metadata,
                &astrometry_evidence,
                expected_by_image.get(&img.id).copied().flatten(),
                planned_rotation_by_image.get(&img.id).copied().flatten(),
                mapped_source.map(|source| source.path.as_path()),
                mapped_sources.is_invalid(img.id),
            );
            entries.push(stored_entry_for(
                &spatial_store,
                img.id,
                &img.metadata,
                mapped_source_revision,
            ));
            metrics.push(m);
        }
        merge_photometric_signals(&mut metrics, &entries, session_gap_minutes);

        analyzer.analyze(
            &metrics,
            seq_target_id,
            &target_name_clone,
            &filter_name_for_task,
        )
    })
    .await
    .map_err(|e| AppError::InternalError(format!("Analysis task failed: {}", e)))?;

    // Find our image in the results
    for seq in &result {
        if let Some(quality) = seq.images.iter().find(|r| r.image_id == image_id) {
            return Ok(Json(ApiResponse::success(
                crate::server::api::ImageQualityContextResponse {
                    image_id,
                    quality: Some(quality.clone()),
                    sequence_target_id: Some(seq.target_id),
                    sequence_filter_name: Some(seq.filter_name.clone()),
                    sequence_image_count: Some(seq.image_count),
                    reference_values: Some(seq.reference_values.clone()),
                },
            )));
        }
    }

    // Image was not in any scored sequence (too short, etc.)
    Ok(Json(ApiResponse::success(
        crate::server::api::ImageQualityContextResponse {
            image_id,
            quality: None,
            sequence_target_id: Some(seq_target_id),
            sequence_filter_name: Some(filter_name),
            sequence_image_count: None,
            reference_values: None,
        },
    )))
}
