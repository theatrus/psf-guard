//! Authenticated, per-database image ingest for remote acquisition clients.
//!
//! The URL database, echoed database header, selected receive root, and
//! bearer-token hash all come from the same `DatabaseContext`. Uploads are
//! streamed to a temporary sibling, verified, published without clobbering,
//! and passed through the normal one-frame image importer. Light frames land
//! in Target Scheduler-compatible tables; calibration frames land only in
//! PSF Guard's sibling calibration tables.

use axum::{
    extract::Multipart,
    http::{header::AUTHORIZATION, HeaderMap},
    Json,
};
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::io::Read;
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;

use crate::calibration::{self, CalibrationKind};
use crate::commands::import::{self, ImportOptions, ImportOutcome};
use crate::db_registry::RemoteImageUploadPlacement;
use crate::server::api::{
    ApiResponse, RemoteCalibrationResolution, RemoteImageResolution, RemoteImageUploadResponse,
};
use crate::server::database_context::open_scheduler_connection_with_flags;
use crate::server::extract::DbContext;
use crate::server::handlers::AppError;

pub const MAX_IMAGE_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_MULTIPART_BYTES: usize = MAX_IMAGE_BYTES as usize + 1024 * 1024;

const DATABASE_ID_HEADER: &str = "x-psf-guard-database-id";
const CONTENT_SHA256_HEADER: &str = "x-content-sha256";
const MAX_UPLOAD_DIRECTORY_COMPONENT_BYTES: usize = 120;
const CAPTURE_IDENTITY_TIME_TOLERANCE_SECS: u64 = 2;

pub async fn upload_image(
    ctx: DbContext,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Result<Json<ApiResponse<RemoteImageUploadResponse>>, AppError> {
    let config = ctx
        .remote_image_upload
        .as_ref()
        .filter(|config| config.enabled)
        .cloned()
        .ok_or_else(|| {
            AppError::Forbidden("remote image upload is disabled for this database".into())
        })?;
    let upload_dir = ctx.remote_image_upload_dir.clone().ok_or_else(|| {
        AppError::Forbidden("remote image upload is disabled for this database".into())
    })?;

    require_database_identity(&headers, &ctx.id)?;
    require_bearer_token(&headers, &config)?;
    let expected_sha256 = required_sha256_header(&headers)?;

    let mut received = None;
    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|error| AppError::BadRequest(format!("reading multipart upload: {error}")))?
    {
        if field.name() != Some("image") {
            continue;
        }
        if received.is_some() {
            return Err(AppError::BadRequest(
                "multipart request must contain exactly one image field".into(),
            ));
        }

        let filename = field
            .file_name()
            .map(str::to_string)
            .ok_or_else(|| AppError::BadRequest("image field has no filename".into()))?;
        validate_filename(&filename)?;

        // The temporary lands in one of this database's scanned image roots,
        // so it must not carry a frame extension: a folder scan running
        // alongside the upload would pick up the half-written file as a
        // frame. The header read below is told the declared name instead.
        let temporary = tempfile::NamedTempFile::new_in(&upload_dir).map_err(|error| {
            AppError::InternalError(format!(
                "creating upload temporary file in {}: {error}",
                upload_dir.display()
            ))
        })?;
        let reopened = temporary.reopen().map_err(|error| {
            AppError::InternalError(format!("opening upload temporary file: {error}"))
        })?;
        let mut output = tokio::fs::File::from_std(reopened);
        let mut hasher = Sha256::new();
        let mut bytes = 0u64;

        while let Some(chunk) = field
            .chunk()
            .await
            .map_err(|error| AppError::BadRequest(format!("reading uploaded image: {error}")))?
        {
            bytes = bytes
                .checked_add(chunk.len() as u64)
                .ok_or_else(|| AppError::BadRequest("uploaded image is too large".into()))?;
            if bytes > MAX_IMAGE_BYTES {
                return Err(AppError::BadRequest(format!(
                    "uploaded image exceeds the {} MiB limit",
                    MAX_IMAGE_BYTES / 1024 / 1024
                )));
            }
            hasher.update(&chunk);
            output.write_all(&chunk).await.map_err(|error| {
                AppError::InternalError(format!("writing uploaded image: {error}"))
            })?;
        }
        output
            .sync_all()
            .await
            .map_err(|error| AppError::InternalError(format!("syncing uploaded image: {error}")))?;
        drop(output);

        if bytes == 0 {
            return Err(AppError::BadRequest("uploaded image is empty".into()));
        }
        let actual_sha256 = encode_digest(hasher.finalize());
        if !constant_time_eq(expected_sha256.as_bytes(), actual_sha256.as_bytes()) {
            return Err(AppError::BadRequest(
                "uploaded image SHA-256 does not match X-Content-SHA256".into(),
            ));
        }
        received = Some((temporary, filename, bytes, actual_sha256));
    }

    let (temporary, filename, bytes, sha256) = received.ok_or_else(|| {
        AppError::BadRequest("multipart request must contain one image field".into())
    })?;
    let temporary_path = temporary.path().to_path_buf();
    let declared_name = PathBuf::from(&filename);
    let frame = tokio::task::spawn_blocking(move || {
        import::headers::read_frame_meta_named(&temporary_path, &declared_name)
    })
    .await
    .map_err(|error| {
        AppError::InternalError(format!("image header validation task failed: {error}"))
    })?;
    if !frame.readable {
        return Err(AppError::BadRequest(
            "uploaded image is not a readable FITS or XISF file".into(),
        ));
    }
    let calibration_kind = calibration::kind_from_meta(&frame);
    if !frame.is_light() && calibration_kind.is_none() {
        return Err(AppError::BadRequest(
            "uploaded image must be a light, bias, dark, dark-flat, or flat frame".into(),
        ));
    }

    let _upload_guard = ctx.image_import_mutex.try_lock().map_err(|_| {
        AppError::Conflict("another image import is already running for this database".into())
    })?;
    let database_path = ctx.database_path.clone();
    let database_id = ctx.id.clone();
    let directory_layout = UploadDirectoryLayout {
        placement: config.placement,
        template: config.directory_template().to_string(),
        explicit: config.directory_layout.is_some(),
    };
    let registered_roots = ctx.image_dir_paths.clone();
    let response_sha256 = sha256.clone();
    let response_filename = filename.clone();
    let published = tokio::task::spawn_blocking(move || {
        publish_and_import(
            &database_id,
            &database_path,
            &upload_dir,
            &registered_roots,
            &directory_layout,
            temporary,
            frame,
            filename,
            bytes,
            sha256,
            calibration_kind,
        )
    })
    .await
    .map_err(|error| AppError::InternalError(format!("image import task failed: {error}")))??;

    if published.light_mapping_changed
        && let Some(resolution) = published.response.resolution.as_ref()
        && let Ok(image_id) = i32::try_from(resolution.image_id)
    {
        ctx.clear_remote_image_verification(image_id);
        if crate::server::spatial_scan::invalidate_image_source(
            &ctx.spatial_metrics,
            &ctx.cache_dir_path,
            image_id,
        ) {
            tracing::info!(
                db = %ctx.id,
                image_id,
                "Invalidated pixel-quality evidence after recording remote image provenance"
            );
        }
    }
    ctx.clear_directory_tree_cache();
    ctx.file_check_cache.write().unwrap().clear();
    let _ = ctx.ensure_cache_available();
    tracing::info!(
        "Remote image received for db={}: {} ({} bytes, sha256={})",
        ctx.id,
        response_filename,
        bytes,
        response_sha256
    );
    Ok(Json(ApiResponse::success(published.response)))
}

struct PublishedRemoteImage {
    response: RemoteImageUploadResponse,
    light_mapping_changed: bool,
}

struct UploadDirectoryLayout {
    placement: RemoteImageUploadPlacement,
    template: String,
    explicit: bool,
}

#[allow(clippy::too_many_arguments)]
fn publish_and_import(
    database_id: &str,
    database_path: &str,
    upload_dir: &Path,
    registered_roots: &[PathBuf],
    directory_layout: &UploadDirectoryLayout,
    temporary: tempfile::NamedTempFile,
    mut frame: import::headers::FrameMeta,
    filename: String,
    bytes: u64,
    sha256: String,
    calibration_kind: Option<CalibrationKind>,
) -> Result<PublishedRemoteImage, AppError> {
    let mut connection = open_scheduler_connection_with_flags(
        database_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(AppError::db)?;
    ensure_remote_upload_provenance_schema(&connection)?;

    let existing_resolution =
        find_resolution(&connection, registered_roots, &filename, &sha256, &frame)?;
    if existing_resolution.is_some() && calibration_kind.is_some() {
        return Err(AppError::Conflict(format!(
            "{filename} is already registered as a light in this database"
        )));
    }

    // Retries follow the path already registered in this catalog before they
    // derive a path from today's target name or placement setting. That keeps
    // a rename or Flat/TargetTree toggle from publishing the same frame twice.
    let registered_destination = match calibration_kind {
        None => match existing_resolution.as_ref() {
            Some(existing) => {
                if let Some(path) =
                    find_mapped_remote_image_file(&connection, registered_roots, existing, &sha256)?
                {
                    Some(path)
                } else if let Some(path) =
                    registered_receive_file(registered_roots, &existing.registered_path)
                {
                    Some(path)
                } else {
                    find_legacy_flat_upload(
                        registered_roots,
                        &filename,
                        &sha256,
                        directory_layout.placement == RemoteImageUploadPlacement::Flat,
                    )?
                }
            }
            None => None,
        },
        Some(kind) => find_registered_calibration_file(
            &connection,
            registered_roots,
            &filename,
            &sha256,
            kind,
            directory_layout.placement,
        )?,
    };
    let destination = if let Some(registered) = registered_destination {
        registered
    } else {
        let relative = upload_relative_destination(
            &connection,
            &frame,
            calibration_kind,
            existing_resolution
                .as_ref()
                .map(|existing| &existing.resolution),
            &filename,
            directory_layout,
        )?;
        upload_dir.join(relative)
    };
    let (destination_root, destination) =
        prepare_registered_upload_destination(upload_dir, registered_roots, &destination)?;
    validate_publish_destination(&destination_root, &destination)?;

    let already_present = if destination.is_file() {
        let existing_sha256 = sha256_file(&destination)?;
        validate_publish_destination(&destination_root, &destination)?;
        if !constant_time_eq(existing_sha256.as_bytes(), sha256.as_bytes()) {
            return Err(AppError::Conflict(format!(
                "{filename} already exists in the receive directory with different content"
            )));
        }
        true
    } else {
        persist_uploaded_file(
            temporary,
            &destination_root,
            &destination,
            &filename,
            &sha256,
        )?
    };

    // N.I.N.A. may commit the scheduler row after the initial lookup but
    // before these bytes finish publishing. Recheck at that boundary so the
    // ordinary import does not race a row that has just appeared.
    let resolution_after_publish = if calibration_kind.is_none() && existing_resolution.is_none() {
        match find_resolution(&connection, registered_roots, &filename, &sha256, &frame) {
            Ok(resolution) => resolution,
            Err(error) => {
                if !already_present && matches!(&error, AppError::Conflict(_)) {
                    cleanup_published_file(&destination_root, &destination, &sha256);
                }
                return Err(error);
            }
        }
    } else {
        None
    };
    let outcome = if calibration_kind.is_none()
        && (existing_resolution.is_some() || resolution_after_publish.is_some())
    {
        ImportOutcome {
            scanned: 1,
            skipped_existing: 1,
            ..Default::default()
        }
    } else {
        frame.path = destination.clone();
        match import::import_frames(
            &mut connection,
            vec![frame.clone()],
            &ImportOptions::default(),
        ) {
            Ok(outcome) => {
                let accepted = if calibration_kind.is_none() {
                    match light_import_outcome_is_accepted(
                        &connection,
                        registered_roots,
                        &filename,
                        &sha256,
                        &frame,
                        &outcome,
                    ) {
                        Ok(accepted) => accepted,
                        Err(error) => {
                            // A database/IO failure is indeterminate: keep the
                            // already verified bytes so a retry can finish the
                            // mapping. Only a proven identity mismatch makes
                            // this publish unsafe to retain.
                            if !already_present && matches!(&error, AppError::Conflict(_)) {
                                cleanup_published_file(&destination_root, &destination, &sha256);
                            }
                            return Err(error);
                        }
                    }
                } else {
                    outcome.calibration.imported
                        + outcome.calibration.updated
                        + outcome.calibration.skipped_existing
                        == 1
                };
                if accepted {
                    outcome
                } else {
                    if !already_present {
                        cleanup_published_file(&destination_root, &destination, &sha256);
                    }
                    return Err(AppError::BadRequest(format!(
                        "uploaded image was not imported (unreadable={}, non_light={}, duplicate={}, calibration_imported={}, calibration_updated={}, calibration_duplicate={})",
                        outcome.unreadable,
                        outcome.non_light,
                        outcome.skipped_existing,
                        outcome.calibration.imported,
                        outcome.calibration.updated,
                        outcome.calibration.skipped_existing,
                    )));
                }
            }
            Err(error) => {
                if !already_present {
                    cleanup_published_file(&destination_root, &destination, &sha256);
                }
                return Err(AppError::DatabaseError(format!(
                    "importing uploaded image: {error:#}"
                )));
            }
        }
    };

    let (frame_kind, resolution, calibration, light_mapping_changed) = match calibration_kind {
        None => {
            let resolution =
                find_resolution(&connection, registered_roots, &filename, &sha256, &frame)?
                    .ok_or_else(|| {
                        AppError::InternalError(
                            "uploaded light was imported but cannot be resolved".into(),
                        )
                    })?;
            let mapping_changed =
                record_remote_image_file(&connection, &resolution, &destination, &sha256)?;
            (
                "light".to_string(),
                Some(resolution.resolution),
                None,
                mapping_changed,
            )
        }
        Some(kind) => {
            let calibration = find_calibration_resolution(&connection, &destination, kind)?
                .ok_or_else(|| {
                    AppError::InternalError(
                        "uploaded calibration frame was imported but cannot be resolved".into(),
                    )
                })?;
            record_remote_calibration_file(&connection, &calibration, &destination, &sha256)?;
            (kind.as_str().to_string(), None, Some(calibration), false)
        }
    };
    Ok(PublishedRemoteImage {
        response: RemoteImageUploadResponse {
            database_id: database_id.to_string(),
            filename,
            bytes,
            sha256,
            already_present,
            frame_kind,
            resolution,
            calibration,
            import: outcome,
        },
        light_mapping_changed,
    })
}

fn persist_uploaded_file(
    temporary: tempfile::NamedTempFile,
    destination_root: &Path,
    destination: &Path,
    filename: &str,
    expected_sha256: &str,
) -> Result<bool, AppError> {
    validate_publish_destination(destination_root, destination)?;
    match temporary.persist_noclobber(destination) {
        Ok(_) => Ok(false),
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            validate_concurrent_upload(destination_root, destination, filename, expected_sha256)
        }
        Err(error) if error.error.kind() == std::io::ErrorKind::CrossesDevices => {
            // A retry can follow durable provenance back to another registered
            // image root, including a different Windows volume. Copy into a
            // temporary sibling there, sync it, then use the same atomic
            // no-clobber publish within that filesystem.
            let source = error.file;
            let parent = destination.parent().ok_or_else(|| {
                AppError::InternalError("remote upload destination has no parent".into())
            })?;
            validate_publish_destination(destination_root, destination)?;
            let mut sibling = tempfile::NamedTempFile::new_in(parent).map_err(|copy_error| {
                AppError::InternalError(format!(
                    "creating upload temporary file beside {}: {copy_error}",
                    destination.display()
                ))
            })?;
            let mut input = source.reopen().map_err(|copy_error| {
                AppError::InternalError(format!(
                    "reopening the received upload for {}: {copy_error}",
                    destination.display()
                ))
            })?;
            std::io::copy(&mut input, sibling.as_file_mut()).map_err(|copy_error| {
                AppError::InternalError(format!(
                    "copying the received upload beside {}: {copy_error}",
                    destination.display()
                ))
            })?;
            sibling.as_file().sync_all().map_err(|copy_error| {
                AppError::InternalError(format!(
                    "syncing the received upload beside {}: {copy_error}",
                    destination.display()
                ))
            })?;
            validate_publish_destination(destination_root, destination)?;
            match sibling.persist_noclobber(destination) {
                Ok(_) => Ok(false),
                Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
                    validate_concurrent_upload(
                        destination_root,
                        destination,
                        filename,
                        expected_sha256,
                    )
                }
                Err(error) => Err(AppError::InternalError(format!(
                    "publishing uploaded image {}: {}",
                    destination.display(),
                    error.error
                ))),
            }
        }
        Err(error) => Err(AppError::InternalError(format!(
            "publishing uploaded image {}: {}",
            destination.display(),
            error.error
        ))),
    }
}

fn validate_concurrent_upload(
    destination_root: &Path,
    destination: &Path,
    filename: &str,
    expected_sha256: &str,
) -> Result<bool, AppError> {
    validate_publish_destination(destination_root, destination)?;
    let existing_sha256 = sha256_file(destination)?;
    if !constant_time_eq(existing_sha256.as_bytes(), expected_sha256.as_bytes()) {
        return Err(AppError::Conflict(format!(
            "{filename} appeared concurrently with different content"
        )));
    }
    Ok(true)
}

fn upload_relative_destination(
    connection: &rusqlite::Connection,
    frame: &import::headers::FrameMeta,
    calibration_kind: Option<CalibrationKind>,
    existing_resolution: Option<&RemoteImageResolution>,
    filename: &str,
    directory_layout: &UploadDirectoryLayout,
) -> Result<PathBuf, AppError> {
    if directory_layout.placement == RemoteImageUploadPlacement::Flat {
        return Ok(PathBuf::from(filename));
    }
    if !directory_layout.explicit {
        let filter = upload_directory_component(frame.filter.as_deref().unwrap_or("NONE"), "NONE");
        match calibration_kind {
            Some(CalibrationKind::Flat) => {
                let target = upload_directory_component(
                    frame.object.as_deref().unwrap_or("Unsorted"),
                    "Unsorted",
                );
                return Ok(PathBuf::from(target)
                    .join("FLAT")
                    .join(filter)
                    .join(filename));
            }
            Some(CalibrationKind::Bias) => return Ok(PathBuf::from("BIAS").join(filename)),
            Some(CalibrationKind::Dark) => return Ok(PathBuf::from("DARK").join(filename)),
            Some(CalibrationKind::DarkFlat) => {
                return Ok(PathBuf::from("DARKFLAT").join(filename));
            }
            None => {}
        }
    }

    crate::server::remote_upload_layout::validate_directory_template(&directory_layout.template)
        .map_err(|error| {
            AppError::InternalError(format!("invalid configured remote upload layout: {error}"))
        })?;
    let (target, project) =
        upload_directory_identity(connection, frame, calibration_kind, existing_resolution)?;
    let frame_type = match calibration_kind {
        None => "LIGHT",
        Some(CalibrationKind::Flat) => "FLAT",
        Some(CalibrationKind::Bias) => "BIAS",
        Some(CalibrationKind::Dark) => "DARK",
        Some(CalibrationKind::DarkFlat) => "DARKFLAT",
    };
    let (capture_date, observing_night) = upload_capture_dates(frame);
    let exposure = frame
        .exposure_s
        .map(format_upload_exposure)
        .unwrap_or_else(|| "UNKNOWN".to_string());
    let gain = frame
        .gain
        .map(|value| value.to_string())
        .unwrap_or_else(|| "UNKNOWN".to_string());
    let capture_year = observing_night
        .split_once('-')
        .map(|(year, _)| year)
        .filter(|year| year.len() == 4 && year.bytes().all(|byte| byte.is_ascii_digit()))
        .unwrap_or("Unknown Year");
    let replacements = [
        ("%TARGET%", target.as_str(), "Unknown Target"),
        ("%PROJECT%", project.as_str(), "Unknown Project"),
        ("%DATE%", capture_date.as_str(), "Unknown Date"),
        ("%NIGHT%", observing_night.as_str(), "Unknown Date"),
        ("%YEAR%", capture_year, "Unknown Year"),
        ("%TYPE%", frame_type, "LIGHT"),
        (
            "%FILTER%",
            frame.filter.as_deref().unwrap_or("NONE"),
            "NONE",
        ),
        (
            "%TELESCOPE%",
            frame.telescope.as_deref().unwrap_or("Unknown Telescope"),
            "Unknown Telescope",
        ),
        (
            "%CAMERA%",
            frame.camera.as_deref().unwrap_or("Unknown Camera"),
            "Unknown Camera",
        ),
        ("%EXPOSURE%", exposure.as_str(), "UNKNOWN"),
        ("%GAIN%", gain.as_str(), "UNKNOWN"),
    ];
    let mut relative = PathBuf::new();
    for component in directory_layout.template.split(['/', '\\']) {
        let rendered = render_upload_template_component(component, &replacements);
        relative.push(upload_directory_component(&rendered, "Unsorted"));
    }
    relative.push(filename);
    Ok(relative)
}

fn render_upload_template_component(
    component: &str,
    replacements: &[(&str, &str, &str)],
) -> String {
    let mut rendered = String::with_capacity(component.len());
    let mut remaining = component;
    while let Some(start) = remaining.find('%') {
        rendered.push_str(&remaining[..start]);
        let token_start = &remaining[start..];
        let end = token_start[1..]
            .find('%')
            .map(|offset| offset + 2)
            .expect("validated remote upload templates contain complete tokens");
        let token = &token_start[..end];
        let (_, value, fallback) = replacements
            .iter()
            .find(|(candidate, _, _)| *candidate == token)
            .expect("validated remote upload templates contain known tokens");
        rendered.push_str(&upload_directory_component(value, fallback));
        remaining = &token_start[end..];
    }
    rendered.push_str(remaining);
    rendered
}

fn upload_directory_identity(
    connection: &rusqlite::Connection,
    frame: &import::headers::FrameMeta,
    calibration_kind: Option<CalibrationKind>,
    existing_resolution: Option<&RemoteImageResolution>,
) -> Result<(String, String), AppError> {
    if let Some(resolution) = existing_resolution {
        return Ok((
            resolution.target_name.clone(),
            resolution.project_name.clone(),
        ));
    }
    if calibration_kind.is_some() {
        let target = frame
            .object
            .clone()
            .unwrap_or_else(|| match calibration_kind {
                Some(CalibrationKind::Flat) => "Unsorted".to_string(),
                _ => "Calibration".to_string(),
            });
        return Ok((target.clone(), target));
    }

    if let Some(identity) = import::resolve_existing_target_identity(
        connection,
        frame,
        import::DEFAULT_MATCH_RADIUS_DEG,
    )
    .map_err(|error| AppError::DatabaseError(format!("resolving the upload target: {error:#}")))?
    {
        return Ok(identity);
    }
    let target = frame
        .object
        .clone()
        .unwrap_or_else(|| "Unknown Target".to_string());
    Ok((target.clone(), target))
}

fn upload_capture_dates(frame: &import::headers::FrameMeta) -> (String, String) {
    let local = frame.date_local.as_deref().or(frame.date_obs.as_deref());
    let timestamp = local
        .and_then(import::headers::parse_fits_datetime)
        .or(frame.timestamp);
    let Some(timestamp) =
        timestamp.and_then(|value| chrono::DateTime::<chrono::Utc>::from_timestamp(value, 0))
    else {
        return ("Unknown Date".to_string(), "Unknown Date".to_string());
    };
    let date = timestamp.format("%Y-%m-%d").to_string();
    let night = (timestamp - chrono::Duration::hours(12))
        .format("%Y-%m-%d")
        .to_string();
    (date, night)
}

fn format_upload_exposure(value: f64) -> String {
    let mut formatted = format!("{value:.3}");
    while formatted.ends_with('0') {
        formatted.pop();
    }
    if formatted.ends_with('.') {
        formatted.pop();
    }
    formatted
}

/// Create and canonicalize the server-derived parent before publishing. This
/// also refuses a pre-existing symlink that would redirect a target folder
/// outside the configured receive root.
fn prepare_upload_destination(upload_dir: &Path, relative: &Path) -> Result<PathBuf, AppError> {
    let parent = relative.parent().unwrap_or_else(|| Path::new(""));
    let canonical_root = dunce::canonicalize(upload_dir).map_err(|error| {
        AppError::InternalError(format!(
            "resolving remote upload directory {}: {error}",
            upload_dir.display()
        ))
    })?;
    let mut canonical_parent = canonical_root.clone();
    for component in parent.components() {
        let std::path::Component::Normal(segment) = component else {
            return Err(AppError::InternalError(
                "server-derived remote upload path was not relative".into(),
            ));
        };
        let next = canonical_parent.join(segment);
        match std::fs::symlink_metadata(&next) {
            Ok(metadata) if metadata_is_link(&metadata) => {
                return Err(AppError::Conflict(
                    "remote upload target directory cannot contain symbolic links".into(),
                ));
            }
            Ok(metadata) if !metadata.is_dir() => {
                return Err(AppError::Conflict(format!(
                    "remote upload target path is not a directory: {}",
                    next.display()
                )));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(&next).map_err(|error| {
                    AppError::InternalError(format!(
                        "creating remote upload directory {}: {error}",
                        next.display()
                    ))
                })?;
            }
            Err(error) => {
                return Err(AppError::InternalError(format!(
                    "checking remote upload directory {}: {error}",
                    next.display()
                )));
            }
        }
        canonical_parent = dunce::canonicalize(&next).map_err(|error| {
            AppError::InternalError(format!(
                "resolving remote upload directory {}: {error}",
                next.display()
            ))
        })?;
        if !canonical_parent.starts_with(&canonical_root) {
            return Err(AppError::Conflict(
                "remote upload target directory resolves outside the configured image root".into(),
            ));
        }
    }
    Ok(canonical_parent.join(
        relative
            .file_name()
            .expect("a server-derived upload path always has a filename"),
    ))
}

/// Resolve a destination below one of the database's registered roots and
/// create its parent tree without following a symlink or Windows reparse
/// point. Stored provenance paths may name parents that no longer exist.
fn prepare_registered_upload_destination(
    upload_dir: &Path,
    registered_roots: &[PathBuf],
    destination: &Path,
) -> Result<(PathBuf, PathBuf), AppError> {
    let mut roots = registered_roots.to_vec();
    if !roots.iter().any(|root| root == upload_dir) {
        roots.push(upload_dir.to_path_buf());
    }

    for root in roots {
        let Ok(canonical_root) = dunce::canonicalize(&root) else {
            continue;
        };
        let relative = destination
            .strip_prefix(&canonical_root)
            .or_else(|_| destination.strip_prefix(&root));
        let Ok(relative) = relative else {
            continue;
        };
        if relative.as_os_str().is_empty()
            || relative
                .components()
                .any(|component| !matches!(component, std::path::Component::Normal(_)))
        {
            continue;
        }
        let prepared = prepare_upload_destination(&canonical_root, relative)?;
        return Ok((canonical_root, prepared));
    }

    Err(AppError::Conflict(format!(
        "remote upload destination is outside the database's registered image roots: {}",
        destination.display()
    )))
}

/// Recheck containment immediately before hashing or publishing. The earlier
/// preparation makes directories, while this pass narrows the window in which
/// another process could replace one with a redirecting filesystem object.
fn validate_publish_destination(root: &Path, destination: &Path) -> Result<(), AppError> {
    let canonical_root = dunce::canonicalize(root).map_err(|error| {
        AppError::InternalError(format!(
            "resolving remote upload root {}: {error}",
            root.display()
        ))
    })?;
    let relative = destination.strip_prefix(&canonical_root).map_err(|_| {
        AppError::Conflict("remote upload destination left its registered image root".into())
    })?;
    let parent = relative.parent().unwrap_or_else(|| Path::new(""));
    let mut current = canonical_root.clone();
    for component in parent.components() {
        let std::path::Component::Normal(segment) = component else {
            return Err(AppError::Conflict(
                "remote upload destination is not a safe relative path".into(),
            ));
        };
        current.push(segment);
        let metadata = std::fs::symlink_metadata(&current).map_err(|error| {
            AppError::InternalError(format!(
                "checking remote upload directory {}: {error}",
                current.display()
            ))
        })?;
        if metadata_is_link(&metadata) {
            return Err(AppError::Conflict(
                "remote upload target directory cannot contain symbolic links or junctions".into(),
            ));
        }
        if !metadata.is_dir() {
            return Err(AppError::Conflict(format!(
                "remote upload target path is not a directory: {}",
                current.display()
            )));
        }
    }
    let canonical_parent = dunce::canonicalize(destination.parent().unwrap_or(&canonical_root))
        .map_err(|error| {
            AppError::InternalError(format!(
                "resolving remote upload parent {}: {error}",
                destination.parent().unwrap_or(&canonical_root).display()
            ))
        })?;
    if !canonical_parent.starts_with(&canonical_root) {
        return Err(AppError::Conflict(
            "remote upload target directory resolves outside its registered image root".into(),
        ));
    }
    if let Ok(metadata) = std::fs::symlink_metadata(destination) {
        if metadata_is_link(&metadata) {
            return Err(AppError::Conflict(
                "remote upload destination cannot be a symbolic link or junction".into(),
            ));
        }
        if !metadata.is_file() {
            return Err(AppError::Conflict(format!(
                "remote upload destination is not a file: {}",
                destination.display()
            )));
        }
    }
    Ok(())
}

fn cleanup_published_file(root: &Path, destination: &Path, expected_sha256: &str) {
    if validate_publish_destination(root, destination).is_err() || !destination.is_file() {
        return;
    }
    let Ok(actual_sha256) = sha256_file(destination) else {
        return;
    };
    if constant_time_eq(actual_sha256.as_bytes(), expected_sha256.as_bytes()) {
        let _ = std::fs::remove_file(destination);
    }
}

fn metadata_is_link(metadata: &std::fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt as _;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    false
}

pub(super) fn upload_directory_component(value: &str, fallback: &str) -> String {
    let value = if value.trim().is_empty() {
        fallback
    } else {
        value
    };
    let mut cleaned = crate::commands::export::sanitize_component(value);
    if cleaned.len() > MAX_UPLOAD_DIRECTORY_COMPONENT_BYTES {
        let mut end = MAX_UPLOAD_DIRECTORY_COMPONENT_BYTES;
        while !cleaned.is_char_boundary(end) {
            end -= 1;
        }
        cleaned.truncate(end);
    }
    cleaned = cleaned.trim().trim_end_matches(['.', ' ']).to_string();
    if cleaned.is_empty() {
        cleaned = fallback.to_string();
    }
    if is_windows_reserved_component(&cleaned) {
        cleaned.insert(0, '_');
    }
    cleaned
}

fn is_windows_reserved_component(value: &str) -> bool {
    let stem = value
        .split('.')
        .next()
        .unwrap_or(value)
        .to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CLOCK$")
        || stem
            .strip_prefix("COM")
            .or_else(|| stem.strip_prefix("LPT"))
            .is_some_and(|suffix| suffix.len() == 1 && matches!(suffix.as_bytes()[0], b'1'..=b'9'))
}

fn find_calibration_resolution(
    connection: &rusqlite::Connection,
    source_path: &Path,
    kind: CalibrationKind,
) -> Result<Option<RemoteCalibrationResolution>, AppError> {
    use rusqlite::OptionalExtension as _;

    let source_path = std::fs::canonicalize(source_path)
        .unwrap_or_else(|_| source_path.to_path_buf())
        .to_string_lossy()
        .into_owned();
    connection
        .query_row(
            "SELECT frame_uuid, rig_uuid
             FROM psf_guard_calibration_frame
             WHERE source_path = ?1",
            [&source_path],
            |row| {
                Ok(RemoteCalibrationResolution {
                    frame_uuid: row.get(0)?,
                    rig_uuid: row.get(1)?,
                    kind,
                })
            },
        )
        .optional()
        .map_err(AppError::db)
}

fn find_registered_calibration_file(
    connection: &rusqlite::Connection,
    registered_roots: &[PathBuf],
    filename: &str,
    expected_sha256: &str,
    kind: CalibrationKind,
    placement: RemoteImageUploadPlacement,
) -> Result<Option<PathBuf>, AppError> {
    if !calibration::schema_exists(connection) {
        return Ok(None);
    }
    let mut statement = connection
        .prepare(
            "SELECT frame_uuid, source_path
             FROM psf_guard_calibration_frame
             WHERE kind = ?1
             ORDER BY id",
        )
        .map_err(AppError::db)?;
    let rows = statement
        .query_map([kind.as_str()], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(AppError::db)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(AppError::db)?;
    drop(statement);
    let mut matches = Vec::new();
    let mut saw_unrelated_mismatch = false;
    let mut saw_invalid_exact_mapping = false;
    for (frame_uuid, source_path) in rows {
        let mapping = find_remote_calibration_file_mapping(connection, &frame_uuid)?;
        if let Some((mapped_path, mapped_sha256)) = mapping.as_ref() {
            let mapped_filename = mapped_path.rsplit(['/', '\\']).next();
            if mapped_filename.is_some_and(|registered| registered.eq_ignore_ascii_case(filename)) {
                if !constant_time_eq(mapped_sha256.as_bytes(), expected_sha256.as_bytes()) {
                    saw_unrelated_mismatch = true;
                    continue;
                }
                if stored_path_is_registered(registered_roots, Path::new(mapped_path)) {
                    matches.push(PathBuf::from(mapped_path));
                } else {
                    saw_invalid_exact_mapping = true;
                }
                continue;
            }
        }

        let registered_filename = source_path.rsplit(['/', '\\']).next();
        if registered_filename.is_some_and(|registered| registered.eq_ignore_ascii_case(filename)) {
            let registered_path = Path::new(&source_path);
            if let Some(path) = registered_receive_file(registered_roots, registered_path) {
                let actual_sha256 = sha256_file(&path)?;
                if constant_time_eq(actual_sha256.as_bytes(), expected_sha256.as_bytes()) {
                    matches.push(path);
                } else {
                    saw_unrelated_mismatch = true;
                }
            } else {
                // A missing calibration source cannot safely be associated
                // with new bytes unless an earlier upload recorded its SHA.
                saw_unrelated_mismatch = true;
            }
        }
    }
    let unrelated_flat_names_can_coexist =
        kind == CalibrationKind::Flat && placement == RemoteImageUploadPlacement::TargetTree;
    if saw_invalid_exact_mapping || (saw_unrelated_mismatch && !unrelated_flat_names_can_coexist) {
        return Err(AppError::Conflict(format!(
            "{filename} matches a calibration record whose uploaded content or stored path does not match"
        )));
    }
    match matches.len() {
        0 => Ok(None),
        1 => Ok(matches.pop()),
        count => Err(AppError::Conflict(format!(
            "{filename} matches {count} registered calibration files; remote ingest requires an unambiguous basename"
        ))),
    }
}

fn registered_receive_file(
    registered_roots: &[PathBuf],
    registered_path: &Path,
) -> Option<PathBuf> {
    let canonical_file = dunce::canonicalize(registered_path).ok()?;
    let is_registered = registered_roots
        .iter()
        .any(|root| dunce::canonicalize(root).is_ok_and(|root| canonical_file.starts_with(root)));
    (canonical_file.is_file() && is_registered).then_some(canonical_file)
}

fn stored_path_is_registered(registered_roots: &[PathBuf], stored_path: &Path) -> bool {
    registered_roots.iter().any(|root| {
        if stored_path.starts_with(root) {
            return true;
        }
        let Ok(canonical_root) = dunce::canonicalize(root) else {
            return false;
        };
        stored_path.starts_with(&canonical_root)
    })
}

fn find_legacy_flat_upload(
    registered_roots: &[PathBuf],
    filename: &str,
    expected_sha256: &str,
    conflict_on_mismatch: bool,
) -> Result<Option<PathBuf>, AppError> {
    let mut matches = Vec::new();
    let mut saw_different_content = false;
    for root in registered_roots {
        let candidate = root.join(filename);
        if std::fs::symlink_metadata(&candidate)
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            return Err(AppError::Conflict(format!(
                "registered image root contains a symbolic link named {filename}"
            )));
        }
        let Some(candidate) = registered_receive_file(registered_roots, &candidate) else {
            continue;
        };
        let actual_sha256 = sha256_file(&candidate)?;
        if constant_time_eq(actual_sha256.as_bytes(), expected_sha256.as_bytes()) {
            if !matches.contains(&candidate) {
                matches.push(candidate);
            }
        } else {
            saw_different_content = true;
        }
    }
    if saw_different_content && conflict_on_mismatch {
        return Err(AppError::Conflict(format!(
            "{filename} already exists in a registered image root with different content"
        )));
    }
    match matches.len() {
        0 => Ok(None),
        1 => Ok(matches.pop()),
        count => Err(AppError::Conflict(format!(
            "{filename} exists in {count} registered image roots; remote ingest requires one unambiguous file"
        ))),
    }
}

struct ExistingImageResolution {
    resolution: RemoteImageResolution,
    guid: Option<String>,
    legacy_identity: Option<String>,
    registered_path: PathBuf,
}

fn find_resolution(
    connection: &rusqlite::Connection,
    registered_roots: &[PathBuf],
    filename: &str,
    expected_sha256: &str,
    frame: &import::headers::FrameMeta,
) -> Result<Option<ExistingImageResolution>, AppError> {
    let resolved_upload_target =
        import::resolve_existing_target_name(connection, frame, import::DEFAULT_MATCH_RADIUS_DEG)
            .map_err(|error| {
            AppError::DatabaseError(format!("resolving the synced upload target: {error:#}"))
        })?;
    let guid_column = if crate::db::SchemaCapabilities::detect(connection).has_acquiredimage_guid {
        "ai.guid"
    } else {
        "NULL"
    };
    let mut statement = connection
        .prepare(&format!(
            "SELECT ai.Id, ai.metadata, ai.acquireddate, ai.filtername, {guid_column},
                    p.Id, p.name, t.Id, t.name
             FROM acquiredimage ai
             JOIN project p ON p.Id = ai.projectId
             JOIN target t ON t.Id = ai.targetId
             WHERE ai.metadata LIKE ?1 ESCAPE '!'
                OR ai.Id IN (
                    SELECT acquiredimage_id
                    FROM psf_guard_remote_image_file
                    WHERE source_sha256 = ?2 OR source_path LIKE ?1 ESCAPE '!'
                )
             ORDER BY ai.Id"
        ))
        .map_err(AppError::db)?;
    let pattern = format!("%{}%", escape_like(filename));
    let rows = statement
        .query_map(rusqlite::params![pattern, expected_sha256], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<i64>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, i64>(7)?,
                row.get::<_, String>(8)?,
            ))
        })
        .map_err(AppError::db)?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(AppError::db)?;
    drop(statement);

    let mut basename_matches = 0usize;
    let mut matches = Vec::new();
    for row in rows {
        let (
            image_id,
            metadata,
            acquired_date,
            filter_name,
            guid,
            project_id,
            project_name,
            target_id,
            target_name,
        ) = row;
        let legacy_identity = legacy_image_identity(
            project_id,
            target_id,
            acquired_date,
            filter_name.as_deref(),
            &metadata,
        );
        let mapping = validate_remote_image_file_mapping(
            connection,
            image_id,
            guid.as_deref(),
            legacy_identity.as_deref(),
        )?;
        let metadata_value = serde_json::from_str::<serde_json::Value>(&metadata).ok();
        let registered_path = metadata_value
            .as_ref()
            .and_then(|value| metadata_text(value, "FileName"))
            .map(str::to_string);
        let registered_filename = registered_path
            .as_deref()
            .and_then(|path| path.rsplit(['/', '\\']).next());
        let metadata_basename_matches =
            registered_filename.is_some_and(|registered| registered.eq_ignore_ascii_case(filename));
        let mapping_basename_matches = mapping.mapping().is_some_and(|mapping| {
            mapping
                .source_path
                .rsplit(['/', '\\'])
                .next()
                .is_some_and(|registered| registered.eq_ignore_ascii_case(filename))
        });
        if metadata_basename_matches || mapping_basename_matches {
            basename_matches += 1;
            if mapping.is_stale() {
                continue;
            }
            let mapped_content_matches = mapping.mapping().is_some_and(|mapping| {
                mapping_basename_matches
                    && constant_time_eq(
                        mapping.source_sha256.as_bytes(),
                        expected_sha256.as_bytes(),
                    )
            });
            if mapping_basename_matches && !mapped_content_matches {
                continue;
            }
            let registered_content_matches = registered_path
                .as_deref()
                .and_then(|path| registered_receive_file(registered_roots, Path::new(path)))
                .map(|path| sha256_file(&path))
                .transpose()?
                .is_some_and(|actual| {
                    constant_time_eq(actual.as_bytes(), expected_sha256.as_bytes())
                });
            if !registered_content_matches && !mapped_content_matches {
                let capture_identity = light_capture_identity(
                    metadata_value.as_ref(),
                    acquired_date,
                    filter_name.as_deref(),
                    frame,
                );
                let target_identity = match resolved_upload_target.as_deref() {
                    Some(resolved) if resolved.eq_ignore_ascii_case(target_name.trim()) => {
                        CaptureIdentity::Match
                    }
                    Some(_) => CaptureIdentity::Mismatch,
                    None => CaptureIdentity::Incomplete,
                };
                match (capture_identity, target_identity) {
                    (CaptureIdentity::Mismatch, _) | (_, CaptureIdentity::Mismatch) => continue,
                    (CaptureIdentity::Match, _) | (_, CaptureIdentity::Match) => {}
                    (CaptureIdentity::Incomplete, CaptureIdentity::Incomplete) => continue,
                }
            }
            matches.push(ExistingImageResolution {
                resolution: RemoteImageResolution {
                    image_id,
                    project_id,
                    project_name,
                    target_id,
                    target_name,
                },
                guid,
                legacy_identity,
                registered_path: PathBuf::from(
                    registered_path
                        .or_else(|| mapping.valid().map(|mapping| mapping.source_path.clone()))
                        .expect("a matched basename has metadata or upload provenance"),
                ),
            });
        }
    }
    match matches.len() {
        0 if basename_matches == 0 => Ok(None),
        0 => Err(AppError::Conflict(format!(
            "{filename} matches an existing scheduler row, but its upload provenance, capture time or filter, or resolved target does not match the uploaded frame"
        ))),
        1 => Ok(matches.pop()),
        count => Err(AppError::Conflict(format!(
            "{filename} matches {count} database rows; remote ingest requires an unambiguous basename"
        ))),
    }
}

fn light_import_outcome_is_accepted(
    connection: &rusqlite::Connection,
    registered_roots: &[PathBuf],
    filename: &str,
    expected_sha256: &str,
    frame: &import::headers::FrameMeta,
    outcome: &ImportOutcome,
) -> Result<bool, AppError> {
    if outcome.imported == 1 {
        return Ok(true);
    }
    if outcome.skipped_existing != 1 {
        return Ok(false);
    }
    // Close the smaller race between the post-publish lookup and
    // import_frames. Only accept a concurrent scheduler row after the full
    // upload identity check.
    find_resolution(
        connection,
        registered_roots,
        filename,
        expected_sha256,
        frame,
    )
    .map(|resolution| resolution.is_some())
}

fn metadata_text<'a>(metadata: &'a serde_json::Value, name: &str) -> Option<&'a str> {
    metadata.as_object()?.iter().find_map(|(key, value)| {
        key.eq_ignore_ascii_case(name)
            .then(|| value.as_str())
            .flatten()
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CaptureIdentity {
    Match,
    Incomplete,
    Mismatch,
}

fn light_capture_identity(
    metadata: Option<&serde_json::Value>,
    acquired_date: Option<i64>,
    filter_name: Option<&str>,
    frame: &import::headers::FrameMeta,
) -> CaptureIdentity {
    let expected_timestamp = metadata
        .and_then(|value| metadata_text(value, "ExposureStartTime"))
        .and_then(import::headers::parse_fits_datetime)
        .or(acquired_date);
    let timestamp_identity = match (expected_timestamp, frame.timestamp) {
        (Some(expected), Some(actual))
            if expected.abs_diff(actual) <= CAPTURE_IDENTITY_TIME_TOLERANCE_SECS =>
        {
            CaptureIdentity::Match
        }
        (Some(_), Some(_)) => CaptureIdentity::Mismatch,
        _ => CaptureIdentity::Incomplete,
    };
    let expected_filter = filter_name
        .filter(|value| !value.trim().is_empty())
        .or_else(|| metadata.and_then(|value| metadata_text(value, "FilterName")));
    let filter_identity = match (expected_filter, frame.filter.as_deref()) {
        (Some(expected), Some(actual)) if crate::utils::filter_names_match(expected, actual) => {
            CaptureIdentity::Match
        }
        (Some(_), Some(_)) => CaptureIdentity::Mismatch,
        _ => CaptureIdentity::Incomplete,
    };
    if timestamp_identity == CaptureIdentity::Mismatch
        || filter_identity == CaptureIdentity::Mismatch
    {
        CaptureIdentity::Mismatch
    } else if timestamp_identity == CaptureIdentity::Match
        && filter_identity == CaptureIdentity::Match
    {
        CaptureIdentity::Match
    } else {
        CaptureIdentity::Incomplete
    }
}

fn legacy_image_identity(
    project_id: i64,
    target_id: i64,
    acquired_date: Option<i64>,
    filter_name: Option<&str>,
    metadata: &str,
) -> Option<String> {
    let metadata = serde_json::from_str::<serde_json::Value>(metadata).ok();
    let capture_time = acquired_date.or_else(|| {
        metadata
            .as_ref()
            .and_then(|value| metadata_text(value, "ExposureStartTime"))
            .and_then(import::headers::parse_fits_datetime)
    })?;
    let filter_name = filter_name
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            metadata
                .as_ref()
                .and_then(|value| metadata_text(value, "FilterName"))
        })?
        .trim()
        .to_ascii_lowercase();

    let mut hasher = Sha256::new();
    hasher.update(b"psf-guard-legacy-image-v2\0");
    hasher.update(project_id.to_le_bytes());
    hasher.update(target_id.to_le_bytes());
    hasher.update(capture_time.to_le_bytes());
    hasher.update(filter_name.as_bytes());
    if let Some(object) = metadata.and_then(|value| value.as_object().cloned()) {
        // Target Scheduler's metadata is not immutable: PSF Guard can add
        // DetectedStars/HFR after a quality scan and external tools can update
        // other derived measurements. Hash only acquisition settings that
        // identify the capture. FileName is intentionally absent because a
        // sync or operator may relocate the same row.
        const CAPTURE_KEYS: &[&str] = &[
            "SessionId",
            "ExposureStartTime",
            "ExposureDuration",
            "ExposureTime",
            "Gain",
            "Offset",
            "Binning",
            "ReadoutMode",
            "ROI",
            "FocuserPosition",
            "FocuserTemp",
            "RotatorPosition",
            "CameraTemp",
            "CameraTargetTemp",
            "PierSide",
        ];
        let stable = CAPTURE_KEYS
            .iter()
            .filter_map(|wanted| {
                object
                    .iter()
                    .find(|(key, _)| key.eq_ignore_ascii_case(wanted))
                    .map(|(_, value)| (wanted.to_ascii_lowercase(), value))
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        if let Ok(encoded) = serde_json::to_vec(&stable) {
            hasher.update(encoded);
        }
    }
    Some(format!("v2:{}", encode_digest(hasher.finalize())))
}

fn ensure_remote_upload_provenance_schema(
    connection: &rusqlite::Connection,
) -> Result<(), AppError> {
    if !table_exists(connection, "psf_guard_remote_image_file")?
        || !table_exists(connection, "psf_guard_remote_calibration_file")?
    {
        connection
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS psf_guard_remote_image_file (
                acquiredimage_id INTEGER PRIMARY KEY,
                acquiredimage_guid TEXT,
                acquiredimage_identity TEXT,
                source_path TEXT NOT NULL,
                source_sha256 TEXT NOT NULL,
                updated_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS psf_guard_remote_calibration_file (
                frame_uuid TEXT PRIMARY KEY,
                source_path TEXT NOT NULL,
                source_sha256 TEXT NOT NULL,
                updated_at INTEGER NOT NULL
            );",
            )
            .map_err(AppError::db)?;
    }
    if !table_has_column(
        connection,
        "psf_guard_remote_image_file",
        "acquiredimage_identity",
    )? {
        connection
            .execute(
                "ALTER TABLE psf_guard_remote_image_file ADD COLUMN acquiredimage_identity TEXT",
                [],
            )
            .map_err(AppError::db)?;
    }
    Ok(())
}

fn find_mapped_remote_image_file(
    connection: &rusqlite::Connection,
    registered_roots: &[PathBuf],
    resolution: &ExistingImageResolution,
    expected_sha256: &str,
) -> Result<Option<PathBuf>, AppError> {
    let mapping = validate_remote_image_file_mapping(
        connection,
        resolution.resolution.image_id,
        resolution.guid.as_deref(),
        resolution.legacy_identity.as_deref(),
    )?;
    let Some(mapping) = mapping.valid() else {
        return Ok(None);
    };
    if !constant_time_eq(mapping.source_sha256.as_bytes(), expected_sha256.as_bytes()) {
        return Err(AppError::Conflict(
            "the uploaded light does not match its recorded upload SHA-256".into(),
        ));
    }
    let path = PathBuf::from(&mapping.source_path);
    if !stored_path_is_registered(registered_roots, &path) {
        return Err(AppError::Conflict(
            "the uploaded light's recorded path is outside the registered image roots".into(),
        ));
    }
    Ok(Some(path))
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RemoteImageFileMapping {
    acquiredimage_guid: Option<String>,
    acquiredimage_identity: Option<String>,
    source_path: String,
    source_sha256: String,
}

enum MappingValidation {
    Absent,
    Stale(RemoteImageFileMapping),
    Valid(RemoteImageFileMapping),
}

impl MappingValidation {
    fn mapping(&self) -> Option<&RemoteImageFileMapping> {
        match self {
            Self::Absent => None,
            Self::Stale(mapping) | Self::Valid(mapping) => Some(mapping),
        }
    }

    fn valid(&self) -> Option<&RemoteImageFileMapping> {
        match self {
            Self::Valid(mapping) => Some(mapping),
            Self::Absent | Self::Stale(_) => None,
        }
    }

    fn is_stale(&self) -> bool {
        matches!(self, Self::Stale(_))
    }
}

fn validate_remote_image_file_mapping(
    connection: &rusqlite::Connection,
    image_id: i64,
    guid: Option<&str>,
    legacy_identity: Option<&str>,
) -> Result<MappingValidation, AppError> {
    let mapping = load_remote_image_file_mapping(connection, image_id)?;
    let Some(mapping) = mapping else {
        return Ok(MappingValidation::Absent);
    };
    let identity_matches = match guid {
        Some(guid) => mapping.acquiredimage_guid.as_deref() == Some(guid),
        None => {
            mapping.acquiredimage_guid.is_none()
                && legacy_identity.is_some()
                && mapping.acquiredimage_identity.as_deref() == legacy_identity
        }
    };
    if identity_matches {
        return Ok(MappingValidation::Valid(mapping));
    }
    connection
        .execute(
            "DELETE FROM psf_guard_remote_image_file WHERE acquiredimage_id = ?1",
            [image_id],
        )
        .map_err(AppError::db)?;
    Ok(MappingValidation::Stale(mapping))
}

fn load_remote_image_file_mapping(
    connection: &rusqlite::Connection,
    image_id: i64,
) -> Result<Option<RemoteImageFileMapping>, AppError> {
    use rusqlite::OptionalExtension as _;

    connection
        .query_row(
            "SELECT acquiredimage_guid, acquiredimage_identity, source_path, source_sha256
             FROM psf_guard_remote_image_file
             WHERE acquiredimage_id = ?1",
            [image_id],
            |row| {
                Ok(RemoteImageFileMapping {
                    acquiredimage_guid: row.get(0)?,
                    acquiredimage_identity: row.get(1)?,
                    source_path: row.get(2)?,
                    source_sha256: row.get(3)?,
                })
            },
        )
        .optional()
        .map_err(AppError::db)
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum MappedLightFile {
    Absent,
    Invalid,
    Missing {
        artifact_revision: String,
    },
    Ready {
        path: PathBuf,
        sha256: String,
        artifact_revision: String,
    },
}

fn remote_mapping_artifact_revision(mapping: &RemoteImageFileMapping) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"remote-image-artifact-v1");
    hasher.update([0]);
    hasher.update(mapping.source_path.as_bytes());
    hasher.update([0]);
    hasher.update(mapping.source_sha256.to_ascii_lowercase().as_bytes());
    encode_digest(hasher.finalize())[..16].to_string()
}

fn mapping_matches_image(
    mapping: &RemoteImageFileMapping,
    image: &crate::models::AcquiredImage,
) -> bool {
    match image.guid.as_deref() {
        Some(guid) => mapping.acquiredimage_guid.as_deref() == Some(guid),
        None => {
            let legacy_identity = legacy_image_identity(
                i64::from(image.project_id),
                i64::from(image.target_id),
                image.acquired_date,
                Some(&image.filter_name),
                &image.metadata,
            );
            mapping.acquiredimage_guid.is_none()
                && legacy_identity.is_some()
                && mapping.acquiredimage_identity.as_deref() == legacy_identity.as_deref()
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct MappedLightSource {
    pub revision: String,
    pub path: PathBuf,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct MappedLightSources {
    sources: std::collections::HashMap<i32, MappedLightSource>,
    invalid: std::collections::HashSet<i32>,
}

impl MappedLightSources {
    pub fn get(&self, image_id: &i32) -> Option<&MappedLightSource> {
        self.sources.get(image_id)
    }

    pub fn is_invalid(&self, image_id: i32) -> bool {
        self.invalid.contains(&image_id)
    }

    pub fn quality_revision(&self, image_id: i32) -> Option<&str> {
        if self.is_invalid(image_id) {
            Some("mapping:invalid")
        } else {
            self.get(&image_id).map(|source| source.revision.as_str())
        }
    }
}

/// Load live upload provenance for a set of scheduler rows in a single table
/// scan. Pixel-evidence consumers use this to reject stale caches after restart
/// without a per-image sidecar or one query per image.
pub(crate) fn mapped_light_sources<'a>(
    connection: &rusqlite::Connection,
    registered_roots: &[PathBuf],
    images: impl IntoIterator<Item = &'a crate::models::AcquiredImage>,
) -> Result<MappedLightSources, AppError> {
    let images = images
        .into_iter()
        .map(|image| (image.id, image))
        .collect::<std::collections::HashMap<_, _>>();
    let mut sources = MappedLightSources::default();
    if images.is_empty() || !table_exists(connection, "psf_guard_remote_image_file")? {
        return Ok(sources);
    }

    let mut statement = connection
        .prepare(
            "SELECT acquiredimage_id, acquiredimage_guid, acquiredimage_identity,
                    source_path, source_sha256
             FROM psf_guard_remote_image_file",
        )
        .map_err(AppError::db)?;
    let mappings = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i32>(0)?,
                RemoteImageFileMapping {
                    acquiredimage_guid: row.get(1)?,
                    acquiredimage_identity: row.get(2)?,
                    source_path: row.get(3)?,
                    source_sha256: row.get(4)?,
                },
            ))
        })
        .map_err(AppError::db)?;
    for mapping in mappings {
        let (image_id, mapping) = mapping.map_err(AppError::db)?;
        let Some(image) = images.get(&image_id) else {
            continue;
        };
        if mapping_matches_image(&mapping, image)
            && stored_path_is_registered(registered_roots, Path::new(&mapping.source_path))
        {
            sources.sources.insert(
                image_id,
                MappedLightSource {
                    revision: format!("mapping:{}", remote_mapping_artifact_revision(&mapping)),
                    path: PathBuf::from(&mapping.source_path),
                },
            );
        } else {
            sources.invalid.insert(image_id);
        }
    }
    Ok(sources)
}

/// Resolve upload provenance for a scheduler image without hashing its file.
/// GUID presence is exact: a NULL mapping can only match a currently NULL
/// scheduler GUID, and legacy rows must retain their stored capture identity.
pub(crate) fn resolve_mapped_light_file(
    connection: &rusqlite::Connection,
    registered_roots: &[PathBuf],
    image: &crate::models::AcquiredImage,
) -> Result<MappedLightFile, AppError> {
    if !table_exists(connection, "psf_guard_remote_image_file")? {
        return Ok(MappedLightFile::Absent);
    }
    let Some(mapping) = load_remote_image_file_mapping(connection, i64::from(image.id))? else {
        return Ok(MappedLightFile::Absent);
    };
    if !mapping_matches_image(&mapping, image) {
        return Ok(MappedLightFile::Invalid);
    };
    let stored_path = Path::new(&mapping.source_path);
    if !stored_path_is_registered(registered_roots, stored_path) {
        return Ok(MappedLightFile::Invalid);
    }
    let artifact_revision = remote_mapping_artifact_revision(&mapping);
    match validated_existing_mapped_file(registered_roots, stored_path)? {
        Some(path) => Ok(MappedLightFile::Ready {
            path,
            sha256: mapping.source_sha256.clone(),
            artifact_revision,
        }),
        None => Ok(MappedLightFile::Missing { artifact_revision }),
    }
}

fn validated_existing_mapped_file(
    registered_roots: &[PathBuf],
    stored_path: &Path,
) -> Result<Option<PathBuf>, AppError> {
    for root in registered_roots {
        let Ok(canonical_root) = dunce::canonicalize(root) else {
            continue;
        };
        let relative = stored_path
            .strip_prefix(&canonical_root)
            .or_else(|_| stored_path.strip_prefix(root));
        let Ok(relative) = relative else {
            continue;
        };
        let mut current = canonical_root.clone();
        for component in relative.components() {
            let std::path::Component::Normal(segment) = component else {
                return Ok(None);
            };
            current.push(segment);
            let metadata = match std::fs::symlink_metadata(&current) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(error) => {
                    return Err(AppError::InternalError(format!(
                        "checking mapped image path {}: {error}",
                        current.display()
                    )));
                }
            };
            if metadata_is_link(&metadata) {
                return Err(AppError::Conflict(
                    "mapped image path contains a symbolic link or junction".into(),
                ));
            }
        }
        let canonical_file = dunce::canonicalize(stored_path).map_err(|error| {
            AppError::InternalError(format!(
                "resolving mapped image file {}: {error}",
                stored_path.display()
            ))
        })?;
        return Ok(
            (canonical_file.is_file() && canonical_file.starts_with(&canonical_root))
                .then_some(canonical_file),
        );
    }
    Ok(None)
}

fn record_remote_image_file(
    connection: &rusqlite::Connection,
    resolution: &ExistingImageResolution,
    destination: &Path,
    sha256: &str,
) -> Result<bool, AppError> {
    if resolution.guid.is_none() && resolution.legacy_identity.is_none() {
        return Ok(false);
    }
    let source_path = dunce::canonicalize(destination)
        .unwrap_or_else(|_| destination.to_path_buf())
        .to_string_lossy()
        .into_owned();
    let mapping = RemoteImageFileMapping {
        acquiredimage_guid: resolution.guid.clone(),
        acquiredimage_identity: resolution.legacy_identity.clone(),
        source_path: source_path.clone(),
        source_sha256: sha256.to_string(),
    };
    let artifact_revision = remote_mapping_artifact_revision(&mapping);
    let quality_source_revision = format!("mapping:{artifact_revision}");
    if load_remote_image_file_mapping(connection, resolution.resolution.image_id)?.as_ref()
        == Some(&mapping)
    {
        clear_stale_psf_guard_star_metadata(
            connection,
            resolution.resolution.image_id,
            &quality_source_revision,
        )?;
        return Ok(false);
    }
    connection
        .execute(
            "INSERT INTO psf_guard_remote_image_file
                (acquiredimage_id, acquiredimage_guid, acquiredimage_identity,
                 source_path, source_sha256, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(acquiredimage_id) DO UPDATE SET
                acquiredimage_guid = excluded.acquiredimage_guid,
                acquiredimage_identity = excluded.acquiredimage_identity,
                source_path = excluded.source_path,
                source_sha256 = excluded.source_sha256,
                updated_at = excluded.updated_at",
            rusqlite::params![
                resolution.resolution.image_id,
                resolution.guid,
                resolution.legacy_identity.as_deref(),
                source_path,
                sha256,
                chrono::Utc::now().timestamp()
            ],
        )
        .map_err(AppError::db)?;
    clear_stale_psf_guard_star_metadata(
        connection,
        resolution.resolution.image_id,
        &quality_source_revision,
    )?;
    Ok(true)
}

fn clear_stale_psf_guard_star_metadata(
    connection: &rusqlite::Connection,
    image_id: i64,
    expected_source_revision: &str,
) -> Result<bool, AppError> {
    use rusqlite::OptionalExtension as _;

    let Some(metadata) = connection
        .query_row(
            "SELECT metadata FROM acquiredimage WHERE Id = ?1",
            [image_id],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(AppError::db)?
    else {
        return Ok(false);
    };
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(&metadata) else {
        return Ok(false);
    };
    let Some(object) = value.as_object_mut() else {
        return Ok(false);
    };
    let source = object.iter().find_map(|(key, value)| {
        key.eq_ignore_ascii_case("PsfGuardQualitySource")
            .then(|| value.as_str())
            .flatten()
    });
    if source.is_none() || source == Some(expected_source_revision) {
        return Ok(false);
    }

    let owned_fields = object
        .iter()
        .find_map(|(key, value)| {
            key.eq_ignore_ascii_case("PsfGuardQualityFields")
                .then(|| value.as_array())
                .flatten()
        })
        .map(|fields| {
            fields
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_ascii_lowercase)
                .collect::<std::collections::HashSet<_>>()
        })
        .unwrap_or_else(|| {
            // Compatibility with the short-lived marker format that predated
            // per-field ownership.
            std::collections::HashSet::from(["detectedstars".into(), "hfr".into()])
        });
    object.retain(|key, _| {
        !key.eq_ignore_ascii_case("PsfGuardQualitySource")
            && !key.eq_ignore_ascii_case("PsfGuardQualityFields")
            && !owned_fields.contains(&key.to_ascii_lowercase())
    });
    let updated = serde_json::to_string(&value).map_err(|error| {
        AppError::InternalError(format!(
            "serializing cleared image quality metadata: {error}"
        ))
    })?;
    connection
        .execute(
            "UPDATE acquiredimage SET metadata = ?1 WHERE Id = ?2 AND metadata = ?3",
            rusqlite::params![updated, image_id, metadata],
        )
        .map(|changed| changed == 1)
        .map_err(AppError::db)
}

fn find_remote_calibration_file_mapping(
    connection: &rusqlite::Connection,
    frame_uuid: &str,
) -> Result<Option<(String, String)>, AppError> {
    use rusqlite::OptionalExtension as _;

    connection
        .query_row(
            "SELECT source_path, source_sha256
             FROM psf_guard_remote_calibration_file
             WHERE frame_uuid = ?1",
            [frame_uuid],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(AppError::db)
}

fn record_remote_calibration_file(
    connection: &rusqlite::Connection,
    resolution: &RemoteCalibrationResolution,
    destination: &Path,
    sha256: &str,
) -> Result<(), AppError> {
    let source_path = dunce::canonicalize(destination)
        .unwrap_or_else(|_| destination.to_path_buf())
        .to_string_lossy()
        .into_owned();
    connection
        .execute(
            "INSERT INTO psf_guard_remote_calibration_file
                (frame_uuid, source_path, source_sha256, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(frame_uuid) DO UPDATE SET
                source_path = excluded.source_path,
                source_sha256 = excluded.source_sha256,
                updated_at = excluded.updated_at",
            rusqlite::params![
                resolution.frame_uuid,
                source_path,
                sha256,
                chrono::Utc::now().timestamp()
            ],
        )
        .map_err(AppError::db)?;
    Ok(())
}

fn table_exists(connection: &rusqlite::Connection, table: &str) -> Result<bool, AppError> {
    use rusqlite::OptionalExtension as _;

    connection
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
            [table],
            |_| Ok(()),
        )
        .optional()
        .map(|row| row.is_some())
        .map_err(AppError::db)
}

fn table_has_column(
    connection: &rusqlite::Connection,
    table: &str,
    column: &str,
) -> Result<bool, AppError> {
    let mut statement = connection
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(AppError::db)?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(AppError::db)?;
    for existing in columns {
        if existing.map_err(AppError::db)? == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn escape_like(value: &str) -> String {
    value
        .replace('!', "!!")
        .replace('%', "!%")
        .replace('_', "!_")
}

fn sha256_file(path: &Path) -> Result<String, AppError> {
    let mut file = std::fs::File::open(path).map_err(|error| {
        AppError::InternalError(format!("opening {} for hashing: {error}", path.display()))
    })?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|error| {
            AppError::InternalError(format!("reading {} for hashing: {error}", path.display()))
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(encode_digest(hasher.finalize()))
}

fn require_database_identity(headers: &HeaderMap, database_id: &str) -> Result<(), AppError> {
    let echoed = headers
        .get(DATABASE_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| AppError::BadRequest(format!("{DATABASE_ID_HEADER} header is required")))?;
    if echoed != database_id {
        return Err(AppError::BadRequest(format!(
            "database identity mismatch: URL selects {database_id}, header selects {echoed}"
        )));
    }
    Ok(())
}

fn require_bearer_token(
    headers: &HeaderMap,
    config: &crate::db_registry::RemoteImageUploadConfig,
) -> Result<(), AppError> {
    let token = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .filter(|token| !token.is_empty());
    if token.is_none_or(|token| !config.token_matches(token)) {
        return Err(AppError::Forbidden(
            "remote image upload credentials are invalid".into(),
        ));
    }
    Ok(())
}

fn required_sha256_header(headers: &HeaderMap) -> Result<String, AppError> {
    let value = headers
        .get(CONTENT_SHA256_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| {
            AppError::BadRequest(format!("{CONTENT_SHA256_HEADER} header is required"))
        })?;
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(AppError::BadRequest(format!(
            "{CONTENT_SHA256_HEADER} must be a 64-character hexadecimal SHA-256"
        )));
    }
    Ok(value)
}

fn validate_filename(filename: &str) -> Result<(), AppError> {
    let reserved_filename = Path::new(filename)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .is_some_and(is_windows_reserved_component);
    if filename.is_empty()
        || filename.len() > 240
        || filename == "."
        || filename == ".."
        || filename.ends_with(['.', ' '])
        || filename
            .chars()
            .any(|character| character.is_control() || r#"<>:"/\|?*"#.contains(character))
        || reserved_filename
    {
        return Err(AppError::BadRequest(
            "image filename is not filesystem-safe".into(),
        ));
    }
    if !crate::image_io::is_image_path(Path::new(filename)) {
        return Err(AppError::BadRequest(format!(
            "remote image upload accepts only {} files",
            crate::image_io::IMAGE_EXTENSIONS
                .iter()
                .map(|extension| format!(".{extension}"))
                .collect::<Vec<_>>()
                .join(", ")
        )));
    }
    Ok(())
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut difference = 0u8;
    for (&left, &right) in left.iter().zip(right) {
        difference |= left ^ right;
    }
    difference == 0
}

fn encode_digest(digest: impl AsRef<[u8]>) -> String {
    let digest = digest.as_ref();
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(encoded, "{byte:02x}").expect("writing to a String cannot fail");
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rendered_artifact_revision_tracks_mapping_path_and_content() {
        let mapping = |path: &str, sha256: &str| RemoteImageFileMapping {
            acquiredimage_guid: Some("image-guid".into()),
            acquiredimage_identity: None,
            source_path: path.into(),
            source_sha256: sha256.into(),
        };
        let first = mapping("C:/images/first.fits", "aa11");
        let moved = mapping("C:/images/moved.fits", "aa11");
        let replaced = mapping("C:/images/first.fits", "bb22");
        let same_checksum_case = mapping("C:/images/first.fits", "AA11");

        assert_ne!(
            remote_mapping_artifact_revision(&first),
            remote_mapping_artifact_revision(&moved)
        );
        assert_ne!(
            remote_mapping_artifact_revision(&first),
            remote_mapping_artifact_revision(&replaced)
        );
        assert_eq!(
            remote_mapping_artifact_revision(&first),
            remote_mapping_artifact_revision(&same_checksum_case)
        );
    }

    #[test]
    fn skipped_import_accepts_scheduler_row_that_appeared_after_publish() {
        let temp = tempfile::tempdir().unwrap();
        let database_path = temp.path().join("scheduler.sqlite");
        crate::ts_schema::create_fresh_db(&database_path).unwrap();
        let connection = rusqlite::Connection::open(&database_path).unwrap();
        ensure_remote_upload_provenance_schema(&connection).unwrap();
        connection
            .execute_batch(
                "INSERT INTO project (Id, profileId, name, isMosaic, flatsHandling, guid)
                     VALUES (7, 'profile', 'Project', 0, 0, 'project-guid');
                 INSERT INTO target (Id, name, active, ra, dec, epochcode, projectId, guid)
                     VALUES (3, 'M 31', 1, 10.0, 20.0, 0, 7, 'target-guid');",
            )
            .unwrap();
        let filename = "race.fits";
        let frame = import::headers::FrameMeta {
            path: temp.path().join(filename),
            readable: true,
            image_type: Some("LIGHT".into()),
            object: Some("M 31".into()),
            filter: Some("Ha".into()),
            timestamp: Some(1_784_869_200),
            ..Default::default()
        };
        let outcome = ImportOutcome {
            scanned: 1,
            skipped_existing: 1,
            ..Default::default()
        };

        assert!(!light_import_outcome_is_accepted(
            &connection,
            &[],
            filename,
            "unused-sha256",
            &frame,
            &outcome,
        )
        .unwrap());

        let metadata = serde_json::json!({
            "FileName": format!(r"C:\remote\{filename}"),
            "ExposureStartTime": "2026-07-24T05:00:00Z",
            "FilterName": "Ha",
        });
        connection
            .execute(
                "INSERT INTO acquiredimage
                    (Id, projectId, targetId, acquireddate, filtername, gradingStatus,
                     metadata, profileId, guid)
                 VALUES (42, 7, 3, 1784869200, 'Ha', 0, ?1, 'profile', 'image-guid')",
                [metadata.to_string()],
            )
            .unwrap();

        assert!(light_import_outcome_is_accepted(
            &connection,
            &[],
            filename,
            "unused-sha256",
            &frame,
            &outcome,
        )
        .unwrap());
    }

    #[test]
    fn nina_local_date_drives_observing_night_folders() {
        let frame = import::headers::FrameMeta {
            date_obs: Some("2026-08-31T11:30:00Z".into()),
            date_local: Some("2026-08-31T04:30:00".into()),
            ..Default::default()
        };

        assert_eq!(
            upload_capture_dates(&frame),
            ("2026-08-31".into(), "2026-08-30".into())
        );
    }

    #[test]
    fn catalog_template_renders_server_owned_components() {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        let frame = import::headers::FrameMeta {
            object: Some("M 31".into()),
            filter: Some("Ha".into()),
            date_local: Some("2026-08-31T04:30:00".into()),
            exposure_s: Some(300.0),
            gain: Some(100),
            ..Default::default()
        };
        let resolution = RemoteImageResolution {
            image_id: 1,
            project_id: 2,
            project_name: "Andromeda project".into(),
            target_id: 3,
            target_name: "M 31".into(),
        };

        let path = upload_relative_destination(
            &connection,
            &frame,
            None,
            Some(&resolution),
            "frame.fits",
            &UploadDirectoryLayout {
                placement: RemoteImageUploadPlacement::TargetTree,
                template: "%YEAR%/%PROJECT%/%TARGET%/%NIGHT%/%TYPE%/%FILTER%/%EXPOSURE%s_G%GAIN%"
                    .into(),
                explicit: true,
            },
        )
        .unwrap();
        assert_eq!(
            path,
            PathBuf::from("2026")
                .join("Andromeda project")
                .join("M 31")
                .join("2026-08-30")
                .join("LIGHT")
                .join("Ha")
                .join("300s_G100")
                .join("frame.fits")
        );
    }

    #[test]
    fn project_token_uses_the_same_coordinate_aware_target_match() {
        let temp = tempfile::tempdir().unwrap();
        let database_path = temp.path().join("scheduler.sqlite");
        crate::ts_schema::create_fresh_db(&database_path).unwrap();
        let connection = rusqlite::Connection::open(database_path).unwrap();
        connection
            .execute_batch(
                "INSERT INTO project (Id, profileId, name, isMosaic, flatsHandling, guid)
                     VALUES (1, 'profile', 'Far project', 0, 0, 'project-far');
                 INSERT INTO project (Id, profileId, name, isMosaic, flatsHandling, guid)
                     VALUES (2, 'profile', 'Near project', 0, 0, 'project-near');
                 INSERT INTO target (Id, name, active, ra, dec, epochcode, projectId, guid)
                     VALUES (1, 'Shared target', 1, 1.0, 20.0, 0, 1, 'target-far');
                 INSERT INTO target (Id, name, active, ra, dec, epochcode, projectId, guid)
                     VALUES (2, 'Shared target', 1, 2.0, 20.0, 0, 2, 'target-near');",
            )
            .unwrap();
        let frame = import::headers::FrameMeta {
            object: Some("Shared target".into()),
            ra_deg: Some(30.0),
            dec_deg: Some(20.0),
            ..Default::default()
        };

        assert_eq!(
            upload_directory_identity(&connection, &frame, None, None).unwrap(),
            ("Shared target".into(), "Near project".into())
        );
    }

    #[test]
    fn token_shaped_metadata_is_not_rendered_twice() {
        let replacements = [
            ("%TARGET%", "%TYPE%", "Unknown Target"),
            ("%TYPE%", "LIGHT", "LIGHT"),
        ];
        assert_eq!(
            render_upload_template_component("%TARGET%_%TYPE%", &replacements),
            "%TYPE%_LIGHT"
        );
    }
}
