//! Authenticated, per-database image ingest for remote acquisition clients.
//!
//! The URL database, echoed database header, selected receive root, and
//! bearer-token hash all come from the same `DatabaseContext`. Uploads are
//! streamed to a temporary sibling, verified, published without clobbering,
//! and passed through the normal one-frame FITS importer.

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

use crate::commands::import::{self, ImportOptions, ImportOutcome};
use crate::server::api::{ApiResponse, RemoteImageResolution, RemoteImageUploadResponse};
use crate::server::extract::DbContext;
use crate::server::handlers::AppError;

pub const MAX_IMAGE_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_MULTIPART_BYTES: usize = MAX_IMAGE_BYTES as usize + 1024 * 1024;

const DATABASE_ID_HEADER: &str = "x-psf-guard-database-id";
const CONTENT_SHA256_HEADER: &str = "x-content-sha256";

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
    let frame =
        tokio::task::spawn_blocking(move || import::headers::read_frame_meta(&temporary_path))
            .await
            .map_err(|error| {
                AppError::InternalError(format!("FITS header validation task failed: {error}"))
            })?;
    if !frame.readable {
        return Err(AppError::BadRequest(
            "uploaded image is not a readable FITS file".into(),
        ));
    }
    if !frame.is_light() {
        return Err(AppError::BadRequest(
            "only light frames can be imported into an image database".into(),
        ));
    }

    let _upload_guard = ctx.image_import_mutex.try_lock().map_err(|_| {
        AppError::Conflict("another image import is already running for this database".into())
    })?;
    let database_path = ctx.database_path.clone();
    let database_id = ctx.id.clone();
    let destination = upload_dir.join(&filename);
    let response_sha256 = sha256.clone();
    let response_filename = filename.clone();
    let result = tokio::task::spawn_blocking(move || {
        publish_and_import(
            &database_id,
            &database_path,
            destination,
            temporary,
            frame,
            filename,
            bytes,
            sha256,
        )
    })
    .await
    .map_err(|error| AppError::InternalError(format!("image import task failed: {error}")))??;

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
    Ok(Json(ApiResponse::success(result)))
}

#[allow(clippy::too_many_arguments)]
fn publish_and_import(
    database_id: &str,
    database_path: &str,
    destination: PathBuf,
    temporary: tempfile::NamedTempFile,
    mut frame: import::headers::FrameMeta,
    filename: String,
    bytes: u64,
    sha256: String,
) -> Result<RemoteImageUploadResponse, AppError> {
    let mut connection = rusqlite::Connection::open_with_flags(
        database_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(AppError::db)?;

    let existing_resolution = find_resolution(&connection, &filename)?;
    if existing_resolution.is_some() && !destination.is_file() {
        return Err(AppError::Conflict(format!(
            "{filename} is already registered in this database at another location"
        )));
    }

    let already_present = if destination.is_file() {
        let existing_sha256 = sha256_file(&destination)?;
        if !constant_time_eq(existing_sha256.as_bytes(), sha256.as_bytes()) {
            return Err(AppError::Conflict(format!(
                "{filename} already exists in the receive directory with different content"
            )));
        }
        true
    } else {
        match temporary.persist_noclobber(&destination) {
            Ok(_) => false,
            Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
                let existing_sha256 = sha256_file(&destination)?;
                if !constant_time_eq(existing_sha256.as_bytes(), sha256.as_bytes()) {
                    return Err(AppError::Conflict(format!(
                        "{filename} appeared concurrently with different content"
                    )));
                }
                true
            }
            Err(error) => {
                return Err(AppError::InternalError(format!(
                    "publishing uploaded image {}: {}",
                    destination.display(),
                    error.error
                )));
            }
        }
    };

    let outcome = if existing_resolution.is_some() {
        ImportOutcome {
            scanned: 1,
            skipped_existing: 1,
            ..Default::default()
        }
    } else {
        frame.path = destination.clone();
        match import::import_frames(&mut connection, vec![frame], &ImportOptions::default()) {
            Ok(outcome) if outcome.imported == 1 => outcome,
            Ok(outcome) => {
                if !already_present {
                    let _ = std::fs::remove_file(&destination);
                }
                return Err(AppError::BadRequest(format!(
                    "uploaded image was not imported (unreadable={}, non_light={}, duplicate={})",
                    outcome.unreadable, outcome.non_light, outcome.skipped_existing
                )));
            }
            Err(error) => {
                if !already_present {
                    let _ = std::fs::remove_file(&destination);
                }
                return Err(AppError::DatabaseError(format!(
                    "importing uploaded image: {error:#}"
                )));
            }
        }
    };

    let resolution = find_resolution(&connection, &filename)?.ok_or_else(|| {
        AppError::InternalError("uploaded image was imported but cannot be resolved".into())
    })?;
    Ok(RemoteImageUploadResponse {
        database_id: database_id.to_string(),
        filename,
        bytes,
        sha256,
        already_present,
        resolution,
        import: outcome,
    })
}

fn find_resolution(
    connection: &rusqlite::Connection,
    filename: &str,
) -> Result<Option<RemoteImageResolution>, AppError> {
    let mut statement = connection
        .prepare(
            "SELECT ai.Id, ai.metadata, p.Id, p.name, t.Id, t.name
             FROM acquiredimage ai
             JOIN project p ON p.Id = ai.projectId
             JOIN target t ON t.Id = ai.targetId
             WHERE ai.metadata LIKE ?1 ESCAPE '!'
             ORDER BY ai.Id",
        )
        .map_err(AppError::db)?;
    let pattern = format!("%{}%", escape_like(filename));
    let rows = statement
        .query_map([pattern], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, i64>(4)?,
                row.get::<_, String>(5)?,
            ))
        })
        .map_err(AppError::db)?;

    let mut matches = Vec::new();
    for row in rows {
        let (image_id, metadata, project_id, project_name, target_id, target_name) =
            row.map_err(AppError::db)?;
        let registered_filename = serde_json::from_str::<serde_json::Value>(&metadata)
            .ok()
            .and_then(|value| {
                value
                    .get("FileName")
                    .and_then(|value| value.as_str())
                    .map(str::to_string)
            })
            .and_then(|path| path.rsplit(['/', '\\']).next().map(str::to_string));
        if registered_filename
            .as_deref()
            .is_some_and(|registered| registered.eq_ignore_ascii_case(filename))
        {
            matches.push(RemoteImageResolution {
                image_id,
                project_id,
                project_name,
                target_id,
                target_name,
            });
        }
    }
    match matches.len() {
        0 => Ok(None),
        1 => Ok(matches.pop()),
        count => Err(AppError::Conflict(format!(
            "{filename} matches {count} database rows; remote ingest requires an unambiguous basename"
        ))),
    }
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
    if filename.is_empty()
        || filename.len() > 240
        || filename == "."
        || filename == ".."
        || filename.ends_with(['.', ' '])
        || filename
            .chars()
            .any(|character| character.is_control() || r#"<>:"/\|?*"#.contains(character))
    {
        return Err(AppError::BadRequest(
            "image filename is not filesystem-safe".into(),
        ));
    }
    let extension = Path::new(filename)
        .extension()
        .and_then(|extension| extension.to_str());
    if !extension.is_some_and(|extension| {
        extension.eq_ignore_ascii_case("fit") || extension.eq_ignore_ascii_case("fits")
    }) {
        return Err(AppError::BadRequest(
            "remote image upload currently accepts only .fit and .fits files".into(),
        ));
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
