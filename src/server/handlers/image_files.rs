//! Image previews, and finding the file of a frame: on disk, under the
//! database's image folders, or from a remote upload.

use super::*;

// Image preview endpoint. Cache hit → 200 PNG; miss → enqueue on the bounded
// interactive queue and return 202 (never generates inside the request).
#[axum::debug_handler(state = Arc<AppState>)]
pub async fn get_image_preview(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db_id, image_id)): Path<(String, i32)>,
    Query(options): Query<PreviewOptions>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let size = options.size.as_deref().unwrap_or("screen");
    let stretch = options.stretch.unwrap_or(true);
    let midtone = options.midtone.unwrap_or(0.2);
    let shadow = options.shadow.unwrap_or(-2.8);
    let color = options
        .color
        .unwrap_or_else(|| state.preview_color_default());

    let (image, file_only, target_name) = resolve_image_meta(&ctx, image_id)?;
    let mapping_revision = rendered_artifact_mapping_revision(&ctx, &image)?;
    let copy = resolve_copy_async(&ctx.0, &image, options.copy.as_deref()).await?;
    let cache_key = with_copy(
        preview_cache_key(
            &image,
            &file_only,
            size,
            stretch,
            midtone,
            shadow,
            color,
            mapping_revision.as_deref(),
        ),
        copy.as_ref(),
    );
    let cache_path = artifact_cache_path(&ctx, "previews", &cache_key, state.preview_encoding())?;
    if let Some(copy) = copy {
        if cache_path.exists() {
            return serve_cached_png(&cache_path, &headers).await;
        }
        state.enqueue_preview(crate::server::preview_queue::GenJob {
            fits_path: copy.path,
            cache_path,
            kind: crate::server::preview_queue::GenKind::Preview {
                midtone,
                shadow,
                max_dimensions: crate::server::preview_queue::max_dimensions_for_size(size),
                color,
            },
            encoding: state.preview_encoding(),
        });
        return Ok(generating_response());
    }

    let delayed_probe = ctx.delayed_file_probe(image.id);
    if delayed_probe == crate::server::database_context::DelayedFileProbe::NotPending
        && cache_path.exists()
    {
        return serve_cached_png(&cache_path, &headers).await;
    }

    // Miss: resolve the source (404 if truly missing), hand generation to the
    // bounded interactive queue, and tell the client to poll.
    if delayed_probe == crate::server::database_context::DelayedFileProbe::Wait {
        return Ok(generating_response());
    }
    let source = if delayed_probe == crate::server::database_context::DelayedFileProbe::Probe {
        find_fits_file_cached_async(&ctx.0, &image, &target_name, &file_only).await
    } else {
        find_fits_file_async(&ctx.0, &image, &target_name, &file_only).await
    };
    let fits_path = match source {
        Ok(path) => path,
        Err(AppError::NotFound)
            if delayed_probe == crate::server::database_context::DelayedFileProbe::Probe =>
        {
            return Ok(generating_response());
        }
        Err(error) => return Err(error),
    };
    if !ctx.delayed_source_is_stable(image.id, &fits_path) {
        return Ok(generating_response());
    }
    if delayed_probe == crate::server::database_context::DelayedFileProbe::Probe
        && tracked_cache_matches_source(&state, &cache_path, &fits_path)?
    {
        if ctx.complete_delayed_file_probe(image.id, &fits_path) {
            return serve_cached_png(&cache_path, &headers).await;
        }
        return Ok(generating_response());
    }
    state.enqueue_preview(crate::server::preview_queue::GenJob {
        fits_path,
        cache_path,
        kind: crate::server::preview_queue::GenKind::Preview {
            midtone,
            shadow,
            max_dimensions: crate::server::preview_queue::max_dimensions_for_size(size),
            color,
        },
        encoding: state.preview_encoding(),
    });
    Ok(generating_response())
}

// Helper function to find FITS file
pub fn find_fits_file(
    ctx: &DatabaseContext,
    image: &crate::models::AcquiredImage,
    target_name: &str,
    filename: &str,
) -> Result<std::path::PathBuf, AppError> {
    let policy = if ctx.requires_delayed_file_identity(image.id) {
        FileResolutionPolicy::DelayedRemote
    } else {
        FileResolutionPolicy::Ordinary
    };
    find_fits_file_inner(ctx, image, target_name, filename, policy, true, None)
}

/// Resolve a source on Tokio's blocking pool. A durable remote-upload mapping
/// can require one full-file SHA-256 on a cold process, which must not run on
/// an async request worker.
pub(crate) async fn find_fits_file_async(
    ctx: &Arc<DatabaseContext>,
    image: &crate::models::AcquiredImage,
    target_name: &str,
    filename: &str,
) -> Result<std::path::PathBuf, AppError> {
    find_fits_file_on_blocking_pool(ctx, image, target_name, filename, false).await
}

pub(super) async fn find_fits_file_cached_async(
    ctx: &Arc<DatabaseContext>,
    image: &crate::models::AcquiredImage,
    target_name: &str,
    filename: &str,
) -> Result<std::path::PathBuf, AppError> {
    find_fits_file_on_blocking_pool(ctx, image, target_name, filename, true).await
}

pub(super) async fn find_fits_file_on_blocking_pool(
    ctx: &Arc<DatabaseContext>,
    image: &crate::models::AcquiredImage,
    target_name: &str,
    filename: &str,
    cached_only: bool,
) -> Result<std::path::PathBuf, AppError> {
    let ctx = Arc::clone(ctx);
    let image = image.clone();
    let target_name = target_name.to_string();
    let filename = filename.to_string();
    tokio::task::spawn_blocking(move || {
        if cached_only {
            find_fits_file_cached(&ctx, &image, &target_name, &filename)
        } else {
            find_fits_file(&ctx, &image, &target_name, &filename)
        }
    })
    .await
    .map_err(|error| {
        AppError::InternalError(format!("FITS source resolution task failed: {error}"))
    })?
}

/// A delayed remote row needs stronger identity proof than an ordinary local
/// catalog lookup. Its file may still be in flight, and an older capture with
/// the same basename may already be present below another configured root.
/// Under either policy, several files of one capture are copies of the frame
/// (a copy between image folders, for example), and the largest is used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FileResolutionPolicy {
    Ordinary,
    DelayedRemote,
}

pub(super) fn find_fits_file_cached(
    ctx: &DatabaseContext,
    image: &crate::models::AcquiredImage,
    target_name: &str,
    filename: &str,
) -> Result<std::path::PathBuf, AppError> {
    find_fits_file_inner(
        ctx,
        image,
        target_name,
        filename,
        FileResolutionPolicy::DelayedRemote,
        false,
        None,
    )
}

pub(super) fn find_fits_file_cached_with_roots(
    ctx: &DatabaseContext,
    image: &crate::models::AcquiredImage,
    target_name: &str,
    filename: &str,
    canonical_roots: &[PathBuf],
) -> Result<std::path::PathBuf, AppError> {
    find_fits_file_inner(
        ctx,
        image,
        target_name,
        filename,
        FileResolutionPolicy::DelayedRemote,
        false,
        Some(canonical_roots),
    )
}

pub(super) fn find_fits_file_inner(
    ctx: &DatabaseContext,
    image: &crate::models::AcquiredImage,
    target_name: &str,
    filename: &str,
    policy: FileResolutionPolicy,
    allow_tree_build: bool,
    shared_canonical_roots: Option<&[PathBuf]>,
) -> Result<std::path::PathBuf, AppError> {
    use crate::commands::filter_rejected::get_possible_paths;

    tracing::debug!(
        "🔍 find_fits_file called for image_id={}, filename={}, target={}, base_dirs={:?}",
        image.id,
        filename,
        target_name,
        ctx.image_dirs
    );
    let owned_canonical_roots = shared_canonical_roots
        .is_none()
        .then(|| canonical_image_roots(ctx));
    let canonical_roots = shared_canonical_roots
        .or(owned_canonical_roots.as_deref())
        .unwrap_or_default();

    if let Some(path) = resolve_mapped_remote_image(ctx, image, canonical_roots)? {
        tracing::debug!(
            image_id = image.id,
            path = %path.display(),
            "Resolved image through durable remote-upload provenance"
        );
        ctx.record_resolved_image_file(image, &path);
        return Ok(path);
    }

    // Date-based layouts are a fast path, not a prerequisite. A remote sync
    // can legitimately carry no acquireddate while retaining the original
    // FileName and ExposureStartTime in metadata.
    let mut all_possible_paths = Vec::new();
    let layout_timestamp = image
        .acquired_date
        .or_else(|| metadata_exposure_start(image));
    if let Some(acquired_date) =
        layout_timestamp.and_then(|timestamp| chrono::DateTime::from_timestamp(timestamp, 0))
    {
        let date_str = acquired_date.format("%Y-%m-%d").to_string();
        tracing::debug!("📅 Date string for image {}: {}", image.id, date_str);
        for base_dir in &ctx.image_dirs {
            all_possible_paths.extend(get_possible_paths(
                base_dir,
                &date_str,
                target_name,
                filename,
            ));
        }
    } else {
        tracing::debug!(
            "Skipping date-layout probes for image {} because no usable capture time is available",
            image.id
        );
    }

    tracing::debug!(
        "🔎 Checking {} possible paths for image {} across {} directories",
        all_possible_paths.len(),
        image.id,
        ctx.image_dirs.len()
    );

    // Verify all base directories exist (they were checked during startup)
    for base_dir in &ctx.image_dirs {
        tracing::debug!("✅ Base directory exists: {}", base_dir);
    }

    if policy == FileResolutionPolicy::Ordinary {
        for (idx, path) in all_possible_paths.iter().enumerate() {
            tracing::debug!("  📁 Path {}: {:?}", idx + 1, path);
            if let Some(path) = existing_file_under_configured_root(path, canonical_roots) {
                tracing::info!("✅ Found file at path {}: {:?}", idx + 1, path);
                let path =
                    most_complete_of_listed_copies(ctx, image, filename, path, canonical_roots);
                ctx.record_resolved_image_file(image, &path);
                return Ok(path);
            }
        }
    }

    // A scheduler row sent by another N.I.N.A. host carries that host's
    // absolute FileName. Remote copy tools commonly preserve the meaningful
    // tail (target/night/LIGHT/file) below a different local image root. Probe
    // those tails directly so a file that arrived after the directory scan is
    // visible immediately. Components are joined one at a time and the final
    // canonical path must remain under a configured root.
    let mut resolution_candidates = if policy == FileResolutionPolicy::DelayedRemote {
        all_possible_paths
    } else {
        Vec::new()
    };
    resolution_candidates.extend(
        metadata_file_name(image)
            .map(|metadata_path| metadata_suffix_candidates(ctx, &metadata_path, filename))
            .unwrap_or_default(),
    );
    // Some copy tools flatten the remote hierarchy directly into an image
    // root. Treat those as basename candidates, never exact-path hits. Several
    // candidates count as copies of the frame only when their capture headers
    // say so; a delayed remote lookup requires matching headers even for one.
    resolution_candidates.extend(ctx.image_dir_paths.iter().map(|root| root.join(filename)));
    if !allow_tree_build {
        let cache = ctx.directory_tree_cache.read().unwrap();
        if let Some(tree) = cache.as_ref()
            && let Some(candidates) = tree.find_file(filename)
        {
            resolution_candidates.extend(candidates.iter().cloned());
        }
    }

    tracing::debug!(
        "❌ File not found in standard paths for image {}, trying directory tree cache lookup",
        image.id
    );

    // Try directory tree cache lookup as fallback
    let search_start = std::time::Instant::now();
    let tree_candidates = if allow_tree_build {
        let tree = ctx.get_directory_tree().map_err(|e| {
            tracing::error!("Failed to get directory tree cache: {}", e);
            AppError::InternalError("Directory cache error".to_string())
        })?;
        tree.find_file(filename).cloned().unwrap_or_default()
    } else {
        Vec::new()
    };
    if tree_candidates.is_empty() {
        tracing::debug!(
            "🔍 No matches in directory tree cache for filename: {}",
            filename
        );
    }

    resolution_candidates.extend(tree_candidates);
    let resolved = match policy {
        FileResolutionPolicy::Ordinary => {
            contained_frame_file(image, filename, resolution_candidates, canonical_roots)?
        }
        FileResolutionPolicy::DelayedRemote => {
            header_matched_frame_file(image, filename, resolution_candidates, canonical_roots)?
        }
    };
    if let Some(path) = resolved {
        tracing::info!(
            "✅ Found file via directory tree cache in {:?}: {:?}",
            search_start.elapsed(),
            path
        );
        ctx.record_resolved_image_file(image, &path);
        return Ok(path);
    }

    tracing::warn!(
        "❌ File not found in directory tree cache after {:?} for image {} ({})",
        search_start.elapsed(),
        image.id,
        filename
    );
    Err(AppError::NotFound)
}

pub(super) fn resolve_mapped_remote_image(
    ctx: &DatabaseContext,
    image: &crate::models::AcquiredImage,
    canonical_roots: &[PathBuf],
) -> Result<Option<PathBuf>, AppError> {
    resolve_mapped_remote_image_with(ctx, image, canonical_roots, mapped_file_sha256)
}

pub(super) fn resolve_mapped_remote_image_with(
    ctx: &DatabaseContext,
    image: &crate::models::AcquiredImage,
    canonical_roots: &[PathBuf],
    hash_file: impl FnOnce(&FsPath) -> Result<String, AppError>,
) -> Result<Option<PathBuf>, AppError> {
    use crate::server::remote_upload::{resolve_mapped_light_file, MappedLightFile};

    let mapping = {
        let connection = ctx.db();
        let connection = connection.lock().map_err(AppError::db)?;
        resolve_mapped_light_file(&connection, canonical_roots, image)?
    };
    let MappedLightFile::Ready {
        path,
        sha256: expected_sha256,
        ..
    } = mapping
    else {
        return match mapping {
            MappedLightFile::Absent => Ok(None),
            MappedLightFile::Invalid => Err(AppError::Conflict(
                "remote image provenance no longer matches this scheduler row".into(),
            )),
            MappedLightFile::Missing { .. } => Err(AppError::NotFound),
            MappedLightFile::Ready { .. } => unreachable!(),
        };
    };

    let observed_identity = remote_image_file_identity(&path).ok_or(AppError::NotFound)?;
    if ctx.remote_image_verification_is_current(image.id, &observed_identity, &expected_sha256) {
        return Ok(Some(path));
    }

    let verification_lock =
        ctx.remote_image_verification_lock(&observed_identity, &expected_sha256);
    let _verification_guard = verification_lock
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    // A concurrent resolver may have completed while this one waited. Re-stat
    // before trusting its result so a replace cannot move this request onto a
    // lock keyed for different bytes.
    let before = remote_image_file_identity(&path).ok_or(AppError::NotFound)?;
    if before != observed_identity {
        ctx.clear_remote_image_verification(image.id);
        return Err(AppError::Conflict(format!(
            "mapped remote image {} changed while its upload identity was being verified",
            path.display()
        )));
    }
    if ctx.remote_image_verification_is_current(image.id, &before, &expected_sha256) {
        return Ok(Some(path));
    }
    if ctx.remote_image_verification_is_known(&before, &expected_sha256) {
        ctx.record_remote_image_verification(image.id, before, &expected_sha256);
        return Ok(Some(path));
    }

    let actual_sha256 = hash_file(&path)?;
    let after = remote_image_file_identity(&path).ok_or(AppError::NotFound)?;
    if before != after {
        ctx.clear_remote_image_verification(image.id);
        return Err(AppError::Conflict(format!(
            "mapped remote image {} changed while its upload identity was being verified",
            path.display()
        )));
    }
    if !actual_sha256.eq_ignore_ascii_case(&expected_sha256) {
        ctx.clear_remote_image_verification(image.id);
        return Err(AppError::Conflict(format!(
            "mapped remote image {} no longer matches the content accepted during upload",
            path.display()
        )));
    }

    ctx.record_remote_image_verification(image.id, after, &expected_sha256);
    Ok(Some(path))
}

pub(super) fn mapped_file_sha256(path: &FsPath) -> Result<String, AppError> {
    let mut file = std::fs::File::open(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            AppError::NotFound
        } else {
            AppError::InternalError(format!(
                "opening mapped remote image {}: {error}",
                path.display()
            ))
        }
    })?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|error| {
            AppError::InternalError(format!(
                "reading mapped remote image {}: {error}",
                path.display()
            ))
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let digest = hasher.finalize();
    let mut encoded = String::with_capacity(64);
    for byte in digest {
        let _ = write!(&mut encoded, "{byte:02x}");
    }
    Ok(encoded)
}

pub(super) const CAPTURE_IDENTITY_TIME_TOLERANCE_SECS: u64 = 2;

pub(super) fn metadata_file_name(image: &crate::models::AcquiredImage) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(&image.metadata)
        .ok()?
        .get("FileName")?
        .as_str()
        .map(str::to_string)
}

pub(super) fn metadata_exposure_start(image: &crate::models::AcquiredImage) -> Option<i64> {
    serde_json::from_str::<serde_json::Value>(&image.metadata)
        .ok()?
        .get("ExposureStartTime")?
        .as_str()
        .and_then(crate::commands::import::headers::parse_fits_datetime)
}

/// Candidate paths made only from safe suffixes of a remote scheduler path.
/// At least one parent component is retained: basename-only discovery belongs
/// to the header-validated fallback below.
pub(super) fn metadata_suffix_candidates(
    ctx: &DatabaseContext,
    metadata_path: &str,
    filename: &str,
) -> Vec<PathBuf> {
    const MAX_METADATA_PATH_BYTES: usize = 4096;
    const MAX_METADATA_PATH_COMPONENTS: usize = 128;
    if metadata_path.len() > MAX_METADATA_PATH_BYTES || metadata_path.chars().any(char::is_control)
    {
        return Vec::new();
    }
    let components: Vec<&str> = metadata_path
        .split(['/', '\\'])
        .filter(|component| !component.is_empty())
        .collect();
    if components.len() < 2
        || components.len() > MAX_METADATA_PATH_COMPONENTS
        || !components
            .last()
            .is_some_and(|last| last.eq_ignore_ascii_case(filename))
        || components
            .iter()
            .any(|component| matches!(*component, "." | "..") || component.contains('\0'))
    {
        return Vec::new();
    }

    let mut candidates = Vec::new();
    for start in 0..components.len() - 1 {
        let suffix = &components[start..];
        if suffix.iter().any(|component| component.contains(':')) {
            continue;
        }
        for root in &ctx.image_dir_paths {
            let mut candidate = root.clone();
            for component in suffix {
                candidate.push(component);
            }
            if !candidates.contains(&candidate) {
                candidates.push(candidate);
            }
        }
    }
    candidates
}

pub(super) fn existing_file_under_configured_root(
    candidate: &FsPath,
    canonical_roots: &[PathBuf],
) -> Option<PathBuf> {
    if !candidate.is_file() {
        return None;
    }
    canonical_file_under_roots(candidate, canonical_roots)
}

pub(super) fn canonical_image_roots(ctx: &DatabaseContext) -> Vec<PathBuf> {
    ctx.image_dir_paths
        .iter()
        .filter_map(|root| std::fs::canonicalize(root).ok())
        .collect()
}

pub(super) fn canonical_file_under_roots(
    candidate: &FsPath,
    canonical_roots: &[PathBuf],
) -> Option<PathBuf> {
    let canonical_candidate = std::fs::canonicalize(candidate).ok()?;
    let is_configured = canonical_roots
        .iter()
        .any(|root| canonical_candidate.starts_with(root));
    is_configured.then(|| candidate.to_path_buf())
}

/// The distinct files among `candidates` that exist under a configured root,
/// in the order they were listed. Two paths to one file (a symlink, or the
/// same file reached through two candidate lists) count once.
fn distinct_contained_files(candidates: Vec<PathBuf>, canonical_roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut identities: Vec<PathBuf> = Vec::new();
    let mut files = Vec::new();
    for candidate in candidates {
        let Some(candidate) = existing_file_under_configured_root(&candidate, canonical_roots)
        else {
            continue;
        };
        let identity = std::fs::canonicalize(&candidate).unwrap_or_else(|_| candidate.clone());
        if !identities.contains(&identity) {
            identities.push(identity);
            files.push(candidate);
        }
    }
    files
}

/// The files among several whose header can be read. A file whose header
/// cannot be read, such as a copy that has only just started, cannot be
/// rendered, so it gives way to any readable file. When no header can be
/// read, every file is kept and later steps decide.
fn readable_files(files: Vec<PathBuf>) -> Vec<PathBuf> {
    let readable: Vec<PathBuf> = files
        .iter()
        .filter(|path| crate::commands::import::headers::read_frame_meta(path).readable)
        .cloned()
        .collect();
    if readable.is_empty() {
        files
    } else {
        readable
    }
}

/// Whether every file's headers record the same capture: a readable
/// DATE-OBS within the capture-identity tolerance of the first file's, and
/// the same filter. Such files are copies of one frame.
fn files_share_one_capture(files: &[PathBuf]) -> bool {
    let metas: Vec<_> = files
        .iter()
        .map(|path| crate::commands::import::headers::read_frame_meta(path))
        .collect();
    let Some(first) = metas.first() else {
        return false;
    };
    let Some(first_timestamp) = first.timestamp.filter(|_| first.readable) else {
        return false;
    };
    metas.iter().all(|meta| {
        let same_time = meta.readable
            && meta.timestamp.is_some_and(|timestamp| {
                timestamp.abs_diff(first_timestamp) <= CAPTURE_IDENTITY_TIME_TOLERANCE_SECS
            });
        let same_filter = match (first.filter.as_deref(), meta.filter.as_deref()) {
            (Some(expected), Some(actual)) => crate::utils::filter_names_match(expected, actual),
            (None, None) => true,
            _ => false,
        };
        same_time && same_filter
    })
}

/// The most complete of several copies of one frame: the largest file. A
/// copy still being written between image folders is shorter than its
/// source, while finished copies are the same size. A tie goes to the copy
/// listed first.
fn most_complete_copy(image_id: i32, filename: &str, copies: Vec<PathBuf>) -> Option<PathBuf> {
    let size = |path: &PathBuf| std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
    let largest = copies.iter().map(size).max()?;
    let chosen = copies.iter().find(|path| size(path) == largest)?.clone();
    if copies.len() > 1 {
        tracing::info!(
            image_id,
            copies = copies.len(),
            chosen = %chosen.display(),
            "{filename} has {} copies of one capture in the image directories; using the largest",
            copies.len()
        );
    }
    Some(chosen)
}

/// `found`, or a more complete copy of it: another file of the same name
/// under the image roots, as the last directory scan listed it, that records
/// the image's capture (or, when no file matches the image's capture, the
/// capture `found` records). A frame being copied into its final folder is
/// found there by the layout probes before the copy is complete, while the
/// complete file is still in the folder it is copied from. Files of a
/// different capture are left out: a file in the frame's own target folder
/// is better evidence than a name.
fn most_complete_of_listed_copies(
    ctx: &DatabaseContext,
    image: &crate::models::AcquiredImage,
    filename: &str,
    found: PathBuf,
    canonical_roots: &[PathBuf],
) -> PathBuf {
    let listed: Vec<PathBuf> = {
        let cache = ctx.directory_tree_cache.read().unwrap();
        cache
            .as_ref()
            .and_then(|tree| tree.find_file(filename))
            .cloned()
            .unwrap_or_default()
    };
    let mut candidates = vec![found.clone()];
    candidates.extend(listed);
    let files = distinct_contained_files(candidates, canonical_roots);
    if files.len() <= 1 {
        return found;
    }
    let files = readable_files(files);
    let matching: Vec<PathBuf> = files
        .iter()
        .filter(|path| basename_headers_match(path, image))
        .cloned()
        .collect();
    let copies = if !matching.is_empty() {
        matching
    } else if files.contains(&found) {
        files
            .into_iter()
            .filter(|path| {
                *path == found || files_share_one_capture(&[found.clone(), path.clone()])
            })
            .collect()
    } else {
        return found;
    };
    most_complete_copy(image.id, filename, copies).unwrap_or(found)
}

/// Resolve an ordinary catalog's basename candidates. One file is the frame.
/// Several files are copies of the frame when their capture headers match
/// the image, or, when the image records no usable capture identity, when
/// they all record the same capture; then the most complete copy is used.
/// Several files of different captures that neither rule tells apart are
/// refused, since showing the wrong frame is worse than showing none.
pub(super) fn contained_frame_file(
    image: &crate::models::AcquiredImage,
    filename: &str,
    candidates: Vec<PathBuf>,
    canonical_roots: &[PathBuf],
) -> Result<Option<PathBuf>, AppError> {
    let mut files = distinct_contained_files(candidates, canonical_roots);
    if files.len() <= 1 {
        return Ok(files.pop());
    }
    let files = readable_files(files);
    let matching: Vec<PathBuf> = files
        .iter()
        .filter(|path| basename_headers_match(path, image))
        .cloned()
        .collect();
    let copies = if !matching.is_empty() {
        matching
    } else if files_share_one_capture(&files) {
        files
    } else {
        tracing::warn!(
            image_id = image.id,
            "Refusing {} files named {} that hold different captures",
            files.len(),
            filename
        );
        return Err(AppError::Conflict(format!(
            "{filename} matches multiple files in configured image directories"
        )));
    };
    Ok(most_complete_copy(image.id, filename, copies))
}

/// Resolve a delayed remote row's candidates: only files whose capture
/// headers match the image count. Several such files are copies of the
/// frame, and the most complete copy is used.
pub(super) fn header_matched_frame_file(
    image: &crate::models::AcquiredImage,
    filename: &str,
    candidates: Vec<PathBuf>,
    canonical_roots: &[PathBuf],
) -> Result<Option<PathBuf>, AppError> {
    let copies: Vec<PathBuf> = distinct_contained_files(candidates, canonical_roots)
        .into_iter()
        .filter(|candidate| {
            let matches = basename_headers_match(candidate, image);
            if !matches {
                tracing::debug!(
                    "Ignoring path whose capture headers disagree for image {}: {:?}",
                    image.id,
                    candidate
                );
            }
            matches
        })
        .collect();
    Ok(most_complete_copy(image.id, filename, copies))
}

pub(super) fn basename_headers_match(path: &FsPath, image: &crate::models::AcquiredImage) -> bool {
    let headers = crate::commands::import::headers::read_frame_meta(path);
    if !headers.readable {
        return false;
    }
    // ExposureStartTime and FITS DATE-OBS describe the same camera event.
    // Prefer that direct pairing; acquireddate remains the fallback for older
    // scheduler rows whose metadata did not retain an exposure start.
    let expected_timestamp = metadata_exposure_start(image).or(image.acquired_date);
    let Some(expected_timestamp) = expected_timestamp else {
        return false;
    };
    let Some(actual_timestamp) = headers.timestamp else {
        return false;
    };
    if expected_timestamp.abs_diff(actual_timestamp) > CAPTURE_IDENTITY_TIME_TOLERANCE_SECS {
        return false;
    }

    let expected_filter = if image.filter_name.trim().is_empty() {
        serde_json::from_str::<serde_json::Value>(&image.metadata)
            .ok()
            .and_then(|metadata| {
                metadata
                    .get("FilterName")
                    .and_then(|value| value.as_str())
                    .map(str::to_string)
            })
    } else {
        Some(image.filter_name.clone())
    };
    match (expected_filter.as_deref(), headers.filter.as_deref()) {
        (Some(expected), Some(actual)) => crate::utils::filter_names_match(expected, actual),
        _ => false,
    }
}

#[axum::debug_handler(state = Arc<AppState>)]
pub async fn get_image_stars(
    ctx: DbContext,
    Path((_db_id, image_id)): Path<(String, i32)>,
    Query(copy_query): Query<CopyQuery>,
) -> Result<Json<ApiResponse<StarDetectionResponse>>, AppError> {
    use crate::hocus_focus_star_detection::detect_stars_hocus_focus;
    use crate::image_analysis::FitsImage;
    use crate::psf_fitting::PSFType;

    // Get image metadata from database
    let (image, file_only, target_name) = {
        let conn = ctx.db();
        let conn = conn.lock().map_err(AppError::db)?;
        let db = Database::new(&conn);

        let images = db.get_images_by_ids(&[image_id]).map_err(AppError::db)?;

        let image = images.into_iter().next().ok_or(AppError::NotFound)?;

        // Get target name
        let targets = db
            .get_targets_by_ids(&[image.target_id])
            .map_err(AppError::db)?;

        let target = targets.into_iter().next().ok_or(AppError::NotFound)?;
        let target_name = target.name.clone();

        let metadata: serde_json::Value = serde_json::from_str(&image.metadata)
            .map_err(|_| AppError::BadRequest("Invalid metadata".to_string()))?;

        let filename = metadata["FileName"]
            .as_str()
            .ok_or_else(|| AppError::BadRequest("No filename in metadata".to_string()))?;

        let file_only = filename
            .split(&['\\', '/'][..])
            .next_back()
            .ok_or_else(|| AppError::BadRequest("Invalid filename format".to_string()))?
            .to_string();

        (image, file_only, target_name)
    };

    let (fits_path, source_token) =
        match resolve_copy_async(&ctx.0, &image, copy_query.copy.as_deref()).await? {
            // A copy's results sit beside the light's under its own token.
            Some(copy) => (
                copy.path,
                format!("{}_copy_{}", copy.token, copy.record.derivative_uuid),
            ),
            None => {
                let fits_path =
                    find_fits_file_async(&ctx.0, &image, &target_name, &file_only).await?;
                let source_token = source_file_cache_token(&fits_path)
                    .ok_or_else(|| AppError::Conflict("Source file is not ready".to_string()))?;
                (fits_path, source_token)
            }
        };

    // Create comprehensive cache key for star detection results. v2: the
    // detector picks a telescope-class preset from the frame's headers, so
    // v1 entries (fixed defaults) are stale for wide/long rigs.
    let cache_key = star_detection_cache_key(&image, &file_only, &source_token);
    let cache_manager = ctx.cache();
    cache_manager
        .ensure_category_dir("stars")
        .map_err(|e| AppError::InternalError(format!("Failed to create cache directory: {}", e)))?;
    let cache_path = cache_manager.get_cached_path("stars", &cache_key, "json");

    // Check if cached version exists
    if cache_manager.is_cached(&cache_path) {
        if let Ok(cached_data) = tokio::fs::read_to_string(&cache_path).await
            && let Ok(mut response) = serde_json::from_str::<StarDetectionResponse>(&cached_data)
        {
            crate::server::cache_budget::note_served_soon(&cache_path);
            // The tilt analysis is derived, cheap, and versionless: compute
            // it on serve rather than trusting what an older build cached.
            if let (Some(width), Some(height)) = (response.width, response.height) {
                let (cells, tilt) =
                    crate::server::api::tilt_analysis(&response.stars, width, height);
                response.cells = cells;
                response.tilt = Some(tilt);
            }
            return Ok(Json(ApiResponse::success(response)));
        }
        let _ = tokio::fs::remove_file(&cache_path).await;
    }

    // Move expensive operations to spawn_blocking
    let fits_path_str = fits_path.to_string_lossy().to_string();
    let (stars, detected_count, average_hfr, average_fwhm, frame_width, frame_height) =
        tokio::task::spawn_blocking(move || {
            // Load FITS file
            let fits = FitsImage::from_file(std::path::Path::new(&fits_path_str))?;

            // Telescope-class preset from the frame's headers, with the
            // endpoint's PSF fitting on top.
            let (mut params, _class) = crate::hocus_focus_star_detection::params_for_frame_path(
                std::path::Path::new(&fits_path_str),
            );
            params.psf_type = PSFType::Moffat4;

            let detection_result =
                detect_stars_hocus_focus(&fits.data, fits.width, fits.height, &params);

            // Convert to API response format
            let stars: Vec<StarInfo> = detection_result
                .stars
                .iter()
                .map(|star| {
                    // Major-axis direction in [0, π): the fit may express
                    // the same ellipse as either sigma being larger, and
                    // clients averaging directions must not see the two
                    // forms 90° apart. The rule this handler used to spell
                    // out now lives on the model itself.
                    let (eccentricity, theta) = match &star.psf_model {
                        Some(psf) => (psf.eccentricity, Some(psf.major_axis_theta())),
                        None => (0.0, None),
                    };

                    StarInfo {
                        x: star.position.0,
                        y: star.position.1,
                        hfr: star.hfr,
                        fwhm: star.fwhm,
                        brightness: star.brightness,
                        eccentricity,
                        theta,
                    }
                })
                .collect();

            Ok::<(Vec<StarInfo>, usize, f64, f64, usize, usize), anyhow::Error>((
                stars,
                detection_result.stars.len(),
                detection_result.average_hfr,
                detection_result.average_fwhm,
                fits.width,
                fits.height,
            ))
        })
        .await
        .map_err(|e| AppError::InternalError(format!("Star detection task panicked: {}", e)))?
        .map_err(|e| AppError::InternalError(format!("Failed to detect stars: {}", e)))?;

    let (cells, tilt) = crate::server::api::tilt_analysis(&stars, frame_width, frame_height);
    let response = StarDetectionResponse {
        detected_stars: detected_count,
        average_hfr,
        average_fwhm,
        width: Some(frame_width),
        height: Some(frame_height),
        stars,
        cells,
        tilt: Some(tilt),
    };

    // Save to cache
    let cached_data = serde_json::to_string(&response)
        .map_err(|_| AppError::InternalError("Failed to serialize response".to_string()))?;

    if source_file_cache_token(&fits_path).as_deref() != Some(source_token.as_str()) {
        return Err(AppError::Conflict(
            "Source file changed during star detection; retry shortly".to_string(),
        ));
    }
    write_cache_atomic(&cache_path, cached_data.as_bytes())
        .await
        .map_err(|_| AppError::InternalError("Failed to write cache".to_string()))?;

    Ok(Json(ApiResponse::success(response)))
}
