//! The overview pages: projects, targets and overall statistics.

use super::*;

// Overview API endpoints
pub async fn get_projects_overview(
    ctx: DbContext,
) -> Result<Json<ApiResponse<Vec<ProjectOverviewResponse>>>, AppError> {
    const RECENT_IMAGES_PER_PROJECT: usize = 6;

    tracing::debug!("📋 Getting projects overview");

    let conn = ctx.db();
    let conn = conn.lock().map_err(AppError::db)?;
    let db = Database::new(&conn);

    // Get all projects with images and profile info
    let projects = db
        .get_projects_with_images_and_profile_info()
        .map_err(AppError::db)?;
    let navigation_metadata = db.get_project_navigation_metadata().map_err(AppError::db)?;

    let ambiguous_names = db.ambiguous_project_names().map_err(AppError::db)?;

    // Get file existence map
    let file_existence_map: std::collections::HashMap<i32, bool> = {
        let cache = ctx.file_check_cache.read().unwrap();
        cache.projects_with_files.clone()
    };
    let mut recent_images_by_project: HashMap<i32, Vec<crate::models::RecentImageSummary>> =
        HashMap::new();
    for image in db
        .get_recent_images_by_project(RECENT_IMAGES_PER_PROJECT)
        .map_err(AppError::db)?
    {
        recent_images_by_project
            .entry(image.project_id)
            .or_default()
            .push(image);
    }

    let mut response = Vec::new();

    for project_with_profile in projects {
        let project = &project_with_profile.project;
        // Get detailed stats for this project
        let stats = db
            .get_project_overview_stats(project.id)
            .map_err(AppError::db)?;

        // Get desired values for this project
        let desired_stats =
            db.get_project_desired_stats(project.id)
                .unwrap_or(ProjectDesiredStats {
                    total_desired: 0,
                    total_acquired: 0,
                    total_accepted: 0,
                    rejected_count: 0,
                    filters_used: vec![],
                });

        let target_count = db
            .get_target_count_for_project(project.id)
            .map_err(AppError::db)?;

        // Get basic file statistics (simplified for performance)
        let files_found = if file_existence_map
            .get(&project.id)
            .copied()
            .unwrap_or(false)
        {
            stats.total_images // Optimistic assumption: if we think project has files, assume all files exist
        } else {
            0
        };
        let files_missing = stats.total_images - files_found;

        let span_days = match (stats.earliest_date, stats.latest_date) {
            (Some(start), Some(end)) => {
                let days = (end - start) / 86400; // seconds to days
                Some(days as i32)
            }
            _ => None,
        };

        let display_name =
            display_project_name(&project.name, &project.profile_id, &ambiguous_names);

        response.push(ProjectOverviewResponse {
            id: project.id,
            guid: project.guid.clone(),
            profile_id: project.profile_id.clone(),
            profile_name: project_with_profile.profile_name.clone(),
            name: project.name.clone(),
            display_name,
            description: project.description.clone(),
            has_files: file_existence_map
                .get(&project.id)
                .copied()
                .unwrap_or(false),
            state: navigation_metadata
                .get(&project.id)
                .map(|metadata| metadata.state)
                .unwrap_or(1),
            target_count,
            total_images: stats.total_images,
            accepted_images: stats.accepted_images,
            rejected_images: stats.rejected_images,
            pending_images: stats.pending_images,
            total_desired: desired_stats.total_desired,
            files_found,
            files_missing,
            date_range: DateRange {
                earliest: stats.earliest_date,
                latest: stats.latest_date,
                span_days,
            },
            filters_used: stats.filters_used,
            recent_images: recent_images_by_project
                .remove(&project.id)
                .unwrap_or_default(),
        });
    }

    Ok(Json(ApiResponse::success(response)))
}

pub async fn get_targets_overview(
    ctx: DbContext,
) -> Result<Json<ApiResponse<Vec<TargetOverviewResponse>>>, AppError> {
    tracing::debug!("🎯 Getting targets overview");

    // Everything this handler needs from the catalog, then the shared
    // connection goes back. It used to be held for the whole response,
    // including a query pair per target; the Overview page asks for this and
    // the overall stats together, so a slow pass here blocked its own page.
    let (targets_data, mut spans, mut filters_by_target) = {
        let conn = ctx.db();
        let conn = conn.lock().map_err(AppError::db)?;
        let db = Database::new(&conn);
        (
            db.get_all_targets_with_desired_stats()
                .map_err(AppError::db)?,
            db.get_acquisition_spans().map_err(AppError::db)?,
            db.get_filters_by_target().map_err(AppError::db)?,
        )
    };

    // Get file existence map
    let file_existence_map: std::collections::HashMap<i32, bool> = {
        let cache = ctx.file_check_cache.read().unwrap();
        cache.targets_with_files.clone()
    };

    let mut response = Vec::new();

    for target_data in targets_data {
        let (earliest, latest) = spans.remove(&target_data.target.id).unwrap_or((None, None));
        let filters = filters_by_target
            .remove(&target_data.target.id)
            .unwrap_or_default();

        let span_days = match (earliest, latest) {
            (Some(start), Some(end)) => {
                let days = (end - start) / 86400;
                Some(days as i32)
            }
            _ => None,
        };

        // Get basic file statistics (simplified for performance)
        let files_found = if file_existence_map
            .get(&target_data.target.id)
            .copied()
            .unwrap_or(false)
        {
            target_data.total_images // Optimistic assumption: if we think target has files, assume all files exist
        } else {
            0
        };
        let files_missing = target_data.total_images - files_found;

        response.push(TargetOverviewResponse {
            id: target_data.target.id,
            name: target_data.target.name.clone(),
            ra: target_data.target.ra,
            dec: target_data.target.dec,
            active: target_data.target.active,
            project_id: target_data.target.project_id,
            project_name: target_data.project_name,
            image_count: target_data.total_images,
            accepted_count: target_data.accepted_images,
            rejected_count: target_data.rejected_images,
            pending_count: target_data.pending_images,
            total_desired: target_data.total_desired,
            files_found,
            files_missing,
            has_files: file_existence_map
                .get(&target_data.target.id)
                .copied()
                .unwrap_or(false),
            date_range: DateRange {
                earliest,
                latest,
                span_days,
            },
            filters_used: filters,
            coordinates_display: format_coordinates(target_data.target.ra, target_data.target.dec),
        });
    }

    Ok(Json(ApiResponse::success(response)))
}

pub async fn get_overall_stats(
    ctx: DbContext,
) -> Result<Json<ApiResponse<OverallStatsResponse>>, AppError> {
    tracing::debug!("📊 Getting overall statistics");

    let conn = ctx.db();
    let conn = conn.lock().map_err(AppError::db)?;
    let db = Database::new(&conn);

    let stats = db.get_overall_statistics().map_err(AppError::db)?;

    // Get overall desired statistics
    let desired_stats = db
        .get_overall_desired_statistics()
        .unwrap_or(OverallDesiredStats {
            total_desired: 0,
            total_acquired: 0,
            total_accepted: 0,
        });

    let span_days = match (stats.earliest_date, stats.latest_date) {
        (Some(start), Some(end)) => {
            let days = (end - start) / 86400;
            Some(days as i32)
        }
        _ => None,
    };

    // Calculate overall file statistics (simplified for performance)
    let total_files_found = if stats.active_projects > 0 {
        stats.total_images
    } else {
        0
    }; // Optimistic: assume all files exist for active projects
    let total_files_missing = stats.total_images - total_files_found;

    // For now, we'll return empty recent activity - this could be enhanced later
    let recent_activity = Vec::new();
    let night_boundary_seconds = crate::server::sky_coverage::cached_night_boundary(&ctx, &conn)?;

    let response = OverallStatsResponse {
        total_projects: stats.total_projects,
        active_projects: stats.active_projects,
        total_targets: stats.total_targets,
        active_targets: stats.active_targets,
        total_images: stats.total_images,
        accepted_images: stats.accepted_images,
        rejected_images: stats.rejected_images,
        pending_images: stats.pending_images,
        total_desired: desired_stats.total_desired,
        files_found: total_files_found,
        files_missing: total_files_missing,
        unique_filters: stats.unique_filters,
        date_range: DateRange {
            earliest: stats.earliest_date,
            latest: stats.latest_date,
            span_days,
        },
        recent_activity,
        night_boundary_seconds,
    };

    Ok(Json(ApiResponse::success(response)))
}
