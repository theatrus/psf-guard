//! Read-only inspection of the master files recorded by a stack artifact.

use super::{
    color, LatestStackPreviews, StackGroupState, StackGroupStatus, StackJobState, StackPreviewJob,
};
use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{
        header::{CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_TYPE},
        StatusCode,
    },
    response::{IntoResponse, Response},
    Json,
};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashSet},
    path::{Path as FsPath, PathBuf},
    sync::Arc,
};
use tokio_util::io::ReaderStream;

use crate::{
    calibration::{AppliedCalibration, CalibrationKind},
    server::{
        api::ApiResponse,
        database_context::DatabaseContext,
        extract::DbContext,
        handlers::{AppError, GenerationStatusBatch},
        preview_queue::{GenJob, GenKind, GenerationState, GenerationStatus},
        state::AppState,
    },
};

const PREVIEW_VERSION: u32 = 1;
const MAX_STATUS_BATCH: usize = 100;
const KINDS: [CalibrationKind; 4] = [
    CalibrationKind::Bias,
    CalibrationKind::Dark,
    CalibrationKind::DarkFlat,
    CalibrationKind::Flat,
];

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Mono,
    Color,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MasterSource {
    pub kind: SourceKind,
    pub job_id: String,
    pub group_index: Option<usize>,
    pub artifact_revision: String,
}

#[derive(Deserialize)]
pub struct MasterPath {
    job_id: String,
    group_index: Option<usize>,
    master_id: Option<String>,
}

impl MasterPath {
    fn source(&self, revision: String) -> MasterSource {
        MasterSource {
            kind: if self.group_index.is_some() {
                SourceKind::Mono
            } else {
                SourceKind::Color
            },
            job_id: self.job_id.clone(),
            group_index: self.group_index,
            artifact_revision: revision,
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct DisplayOptions {
    pub size: Option<String>,
    pub midtone: Option<f64>,
    pub shadow: Option<f64>,
}

impl DisplayOptions {
    fn normalized(&self) -> Result<(&str, f64, f64), AppError> {
        let size = self.size.as_deref().unwrap_or("screen");
        let midtone = self.midtone.unwrap_or(0.2);
        let shadow = self.shadow.unwrap_or(-2.8);
        if !matches!(size, "screen" | "original")
            || !midtone.is_finite()
            || !(0.01..=0.99).contains(&midtone)
            || !shadow.is_finite()
            || !(-10.0..=0.0).contains(&shadow)
        {
            return Err(AppError::BadRequest(
                "Invalid calibration master display settings".into(),
            ));
        }
        Ok((
            size,
            (midtone * 1000.0).round() / 1000.0,
            (shadow * 100.0).round() / 100.0,
        ))
    }
}

#[derive(Deserialize)]
pub struct MasterQuery {
    revision: String,
    size: Option<String>,
    midtone: Option<f64>,
    shadow: Option<f64>,
}

impl MasterQuery {
    fn display(&self) -> DisplayOptions {
        DisplayOptions {
            size: self.size.clone(),
            midtone: self.midtone,
            shadow: self.shadow,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct MasterStatusItem {
    pub source: MasterSource,
    pub master_id: String,
    #[serde(flatten)]
    pub display: DisplayOptions,
}

#[derive(Deserialize)]
pub struct MasterStatusRequest {
    requests: Vec<MasterStatusItem>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MasterUsage {
    pub channel: String,
    pub session: usize,
    pub lights: usize,
    pub estimated_pedestal_adu: Option<f32>,
}

#[derive(Debug, Clone)]
struct RecordedMaster {
    id: String,
    kind: CalibrationKind,
    label: String,
    usages: Vec<MasterUsage>,
}

#[derive(Debug, Serialize)]
pub struct MasterEntry {
    id: String,
    kind: CalibrationKind,
    label: String,
    available: bool,
    unavailable_reason: Option<String>,
    usages: Vec<MasterUsage>,
    preview_url: Option<String>,
    original_preview_url: Option<String>,
    fits_url: Option<String>,
    source_count: Option<usize>,
    rejection_method: Option<String>,
    masked_samples: Option<u64>,
    rejected_samples: Option<u64>,
    minimum_clean_samples: Option<u64>,
    maximum_clean_samples: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct MasterCatalog {
    masters: Vec<MasterEntry>,
    notes: Vec<String>,
}

fn validate_source(source: &MasterSource) -> Result<(), AppError> {
    super::validate_job_id(&source.job_id)?;
    if source.artifact_revision.is_empty()
        || source.artifact_revision.len() > 128
        || !source
            .artifact_revision
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
        || (source.kind == SourceKind::Mono) != source.group_index.is_some()
    {
        return Err(AppError::BadRequest(
            "Invalid calibration master source".into(),
        ));
    }
    Ok(())
}

fn stale() -> AppError {
    AppError::Conflict("The displayed stack changed; reopen its calibration masters".into())
}

fn matching_group(
    job: &StackPreviewJob,
    ctx: &DatabaseContext,
    source: &MasterSource,
) -> Option<StackGroupStatus> {
    if job.database_id != ctx.id
        || job.job_id != source.job_id
        || job.artifact_revision != source.artifact_revision
    {
        return None;
    }
    job.groups
        .iter()
        .find(|g| Some(g.index) == source.group_index && g.state == StackGroupState::Ready)
        .cloned()
}

fn load_mono_group(
    state: &AppState,
    ctx: &DatabaseContext,
    source: &MasterSource,
) -> Result<StackGroupStatus, AppError> {
    validate_source(source)?;
    let mut project_id = None;
    if let Some(job) = state
        .stack_previews
        .get(&source.job_id)
        .filter(|j| j.database_id == ctx.id && j.job_id == source.job_id)
    {
        if let Some(group) = matching_group(&job, ctx, source) {
            return Ok(group);
        }
        project_id = Some(job.project_id);
    }
    if let Ok(bytes) = std::fs::read(super::manifest_path(&ctx.cache_dir_path, &source.job_id)) {
        let job: StackPreviewJob = serde_json::from_slice(&bytes)
            .map_err(|_| AppError::InternalError("Invalid stack manifest".into()))?;
        if job.database_id != ctx.id || job.job_id != source.job_id {
            return Err(AppError::NotFound);
        }
        if let Some(group) = matching_group(&job, ctx, source) {
            return Ok(group);
        }
        project_id = Some(job.project_id);
    }
    // A forced rebuild can replace the live job while the UI still shows the
    // previous completed artifact. Its durable latest entry owns that history.
    if let Some(project_id) = project_id {
        if let Ok(bytes) = std::fs::read(super::latest_path(&ctx.cache_dir_path, project_id)) {
            let latest: LatestStackPreviews = serde_json::from_slice(&bytes)
                .map_err(|_| AppError::InternalError("Invalid latest stack index".into()))?;
            if latest.database_id != ctx.id || latest.project_id != project_id {
                return Err(AppError::NotFound);
            }
            if let Some(entry) = latest.groups.into_iter().find(|entry| {
                entry.job_id == source.job_id
                    && entry.artifact_revision == source.artifact_revision
                    && Some(entry.group.index) == source.group_index
                    && entry.group.state == StackGroupState::Ready
            }) {
                return Ok(entry.group);
            }
        }
        return Err(stale());
    }
    Err(AppError::NotFound)
}

fn load_color_job(
    state: &AppState,
    ctx: &DatabaseContext,
    source: &MasterSource,
) -> Result<color::StackColorJob, AppError> {
    validate_source(source)?;
    let mut project_id = None;
    let matches = |job: &color::StackColorJob| {
        job.database_id == ctx.id
            && job.job_id == source.job_id
            && job.artifact_revision == source.artifact_revision
            && job.state == StackJobState::Completed
    };
    if let Some(job) = state
        .stack_previews
        .get_color(&source.job_id)
        .filter(|j| j.database_id == ctx.id && j.job_id == source.job_id)
    {
        if matches(&job) {
            return Ok(job);
        }
        project_id = Some(job.project_id);
    }
    if let Ok(job) = color::load_persisted_color_job(&ctx.cache_dir_path, &source.job_id) {
        if job.database_id != ctx.id || job.job_id != source.job_id {
            return Err(AppError::NotFound);
        }
        if matches(&job) {
            return Ok(job);
        }
        project_id = Some(job.project_id);
    }
    if let Some(project_id) = project_id {
        if let Some(job) = color::retained_calibration_source(
            ctx,
            project_id,
            &source.job_id,
            &source.artifact_revision,
        )? {
            return Ok(job);
        }
        return Err(stale());
    }
    Err(AppError::NotFound)
}

fn valid_label(kind: CalibrationKind, label: &str) -> bool {
    label
        .strip_prefix(&format!("{}-", kind.as_str()))
        .and_then(|s| s.strip_suffix(".fits"))
        .is_some_and(|hash| hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
}

fn signature_masters(signature: &str) -> Result<Vec<(CalibrationKind, String)>, ()> {
    let mut seen = HashSet::new();
    let mut masters = Vec::new();
    for field in signature.split(';') {
        let (key, value) = field.split_once('=').ok_or(())?;
        if key == "pedestal" {
            continue;
        }
        let kind = KINDS
            .iter()
            .find(|kind| kind.as_str() == key)
            .copied()
            .ok_or(())?;
        if !seen.insert(kind) {
            return Err(());
        }
        if value != "none" {
            if !valid_label(kind, value) {
                return Err(());
            }
            masters.push((kind, value.to_string()));
        }
    }
    if seen.len() != KINDS.len() {
        return Err(());
    }
    Ok(masters)
}

fn master_id(kind: CalibrationKind, label: &str) -> String {
    digest_hex(format!("{}:{label}", kind.as_str()).as_bytes())
}

fn digest_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut value = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        let _ = write!(value, "{byte:02x}");
    }
    value
}

fn add_calibration(
    calibration: &AppliedCalibration,
    channel: &str,
    lights: usize,
    masters: &mut BTreeMap<(CalibrationKind, String), RecordedMaster>,
    notes: &mut Vec<String>,
) {
    if let Some(warning) = &calibration.warning {
        notes.push(format!("{channel}: {warning}"));
    }
    let mut add = |entries: Vec<(CalibrationKind, String)>, session, count, pedestal| {
        for (kind, label) in entries {
            let entry = masters
                .entry((kind, label.clone()))
                .or_insert_with(|| RecordedMaster {
                    id: master_id(kind, &label),
                    kind,
                    label,
                    usages: Vec::new(),
                });
            entry.usages.push(MasterUsage {
                channel: channel.to_string(),
                session,
                lights: count,
                estimated_pedestal_adu: pedestal,
            });
        }
    };
    if !calibration.session_details.is_empty() {
        for (index, session) in calibration.session_details.iter().enumerate() {
            match signature_masters(&session.masters_signature) {
                Ok(entries) => add(
                    entries,
                    index + 1,
                    session.lights,
                    session.estimated_pedestal_adu,
                ),
                Err(()) => notes.push(format!(
                    "{channel}, session {}: recorded master references are invalid",
                    index + 1
                )),
            }
        }
    } else if calibration.sessions <= 1 {
        let labels = [
            &calibration.bias_master,
            &calibration.dark_master,
            &calibration.dark_flat_master,
            &calibration.flat_master,
        ];
        let mut entries = Vec::new();
        for (kind, label) in KINDS.into_iter().zip(labels) {
            if let Some(label) = label {
                if valid_label(kind, label) {
                    entries.push((kind, label.clone()));
                } else {
                    notes.push(format!(
                        "{channel}: recorded {} master reference is invalid",
                        kind.as_str()
                    ));
                }
            }
        }
        add(entries, 1, lights, calibration.estimated_pedestal_adu);
    } else {
        notes.push(format!(
            "{channel}: this older stack did not retain per-session master references"
        ));
    }
}

fn recorded_masters(
    state: &AppState,
    ctx: &DatabaseContext,
    source: &MasterSource,
) -> Result<(Vec<RecordedMaster>, Vec<String>), AppError> {
    validate_source(source)?;
    let mut masters = BTreeMap::new();
    let mut notes = Vec::new();
    match source.kind {
        SourceKind::Mono => {
            let group = load_mono_group(state, ctx, source)?;
            let channel = if group.filter_name.is_empty() {
                "No filter"
            } else {
                &group.filter_name
            };
            add_calibration(
                &group.calibration,
                channel,
                group.accepted_frames,
                &mut masters,
                &mut notes,
            );
        }
        SourceKind::Color => {
            let color = load_color_job(state, ctx, source)?;
            for channel in color.sources {
                let source = MasterSource {
                    kind: SourceKind::Mono,
                    job_id: channel.job_id,
                    group_index: Some(channel.group_index),
                    artifact_revision: channel.artifact_revision,
                };
                match load_mono_group(state, ctx, &source) {
                    Ok(group) => add_calibration(
                        &group.calibration,
                        channel.role.label(),
                        group.accepted_frames,
                        &mut masters,
                        &mut notes,
                    ),
                    Err(_) => notes.push(format!(
                        "{}: the recorded channel stack is no longer available",
                        channel.role.label()
                    )),
                }
            }
        }
    }
    if masters.is_empty() && notes.is_empty() {
        notes.push("No calibration master files were recorded for this stack".into());
    }
    Ok((masters.into_values().collect(), notes))
}

fn master_path(ctx: &DatabaseContext, master: &RecordedMaster) -> Result<PathBuf, AppError> {
    if !valid_label(master.kind, &master.label) {
        return Err(AppError::BadRequest(
            "Invalid calibration master reference".into(),
        ));
    }
    let base = std::fs::canonicalize(&ctx.cache_dir_path).map_err(|_| AppError::NotFound)?;
    let root = std::fs::canonicalize(ctx.cache_dir_path.join("calibration-masters"))
        .map_err(|_| AppError::NotFound)?;
    let path = std::fs::canonicalize(root.join(&master.label)).map_err(|_| AppError::NotFound)?;
    if !root.starts_with(&base) || !path.starts_with(&root) || !path.is_file() {
        return Err(AppError::NotFound);
    }
    Ok(path)
}

fn source_base(ctx: &DatabaseContext, source: &MasterSource) -> String {
    match source.kind {
        SourceKind::Mono => format!(
            "/api/db/{}/stack-previews/{}/{}/calibration-masters",
            ctx.id,
            source.job_id,
            source.group_index.unwrap_or_default()
        ),
        SourceKind::Color => format!(
            "/api/db/{}/stack-previews/color/{}/calibration-masters",
            ctx.id, source.job_id
        ),
    }
}

fn supplement_entry(ctx: &DatabaseContext, entry: &mut MasterEntry) {
    // Catalog provenance is optional: forgotten inputs can remove this row
    // while the historical master remains on disk. Never rebuild or re-record it.
    let conn = ctx.db();
    let Ok(conn) = conn.try_lock() else {
        return;
    };
    let path = ctx
        .cache_dir_path
        .join("calibration-masters")
        .join(&entry.label);
    let row: Result<Option<(i64, String)>, _> = conn.query_row(
        "SELECT source_count, statistics_json FROM psf_guard_calibration_master WHERE cache_path = ?1",
        [path.to_string_lossy().as_ref()], |row| Ok((row.get(0)?, row.get(1)?)),
    ).optional();
    let Ok(Some((count, json))) = row else {
        return;
    };
    entry.source_count = usize::try_from(count).ok();
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&json) {
        entry.rejection_method = value
            .get("rejection_method")
            .and_then(|v| v.as_str())
            .map(str::to_owned);
        entry.masked_samples = value.get("masked_samples").and_then(|v| v.as_u64());
        entry.rejected_samples = value.get("rejected_samples").and_then(|v| v.as_u64());
        if let Some(mask) = value.get("flat_star_masking") {
            entry.minimum_clean_samples =
                mask.get("minimum_clean_samples").and_then(|v| v.as_u64());
            entry.maximum_clean_samples =
                mask.get("maximum_clean_samples").and_then(|v| v.as_u64());
        }
    }
}

fn catalog(
    state: &AppState,
    ctx: &DatabaseContext,
    source: &MasterSource,
) -> Result<MasterCatalog, AppError> {
    let (recorded, notes) = recorded_masters(state, ctx, source)?;
    let base = source_base(ctx, source);
    let masters = recorded
        .into_iter()
        .map(|master| {
            let available = master_path(ctx, &master).is_ok();
            let url = format!("{base}/{}", master.id);
            let mut entry = MasterEntry {
                id: master.id,
                kind: master.kind,
                label: master.label,
                available,
                unavailable_reason: (!available).then(|| {
                    "The recorded master file is no longer available in this database's cache"
                        .into()
                }),
                usages: master.usages,
                preview_url: available
                    .then(|| format!("{url}/preview?revision={}", source.artifact_revision)),
                original_preview_url: available.then(|| {
                    format!(
                        "{url}/preview?revision={}&size=original",
                        source.artifact_revision
                    )
                }),
                fits_url: available
                    .then(|| format!("{url}/fits?revision={}", source.artifact_revision)),
                source_count: None,
                rejection_method: None,
                masked_samples: None,
                rejected_samples: None,
                minimum_clean_samples: None,
                maximum_clean_samples: None,
            };
            supplement_entry(ctx, &mut entry);
            entry
        })
        .collect();
    Ok(MasterCatalog { masters, notes })
}

fn resolve_master(
    state: &AppState,
    ctx: &DatabaseContext,
    source: &MasterSource,
    id: &str,
) -> Result<(RecordedMaster, PathBuf), AppError> {
    super::validate_job_id(id)?;
    let (masters, _) = recorded_masters(state, ctx, source)?;
    let master = masters
        .into_iter()
        .find(|m| m.id == id)
        .ok_or(AppError::NotFound)?;
    let path = master_path(ctx, &master)?;
    Ok((master, path))
}

fn preview_job(
    ctx: &DatabaseContext,
    master: &RecordedMaster,
    path: PathBuf,
    options: &DisplayOptions,
) -> Result<GenJob, AppError> {
    let (size, midtone, shadow) = options.normalized()?;
    let fingerprint = super::source_fingerprint(&path);
    let key = digest_hex(
        format!(
            "{PREVIEW_VERSION}:{}:{}:{}:{}:{fingerprint}:{size}:{midtone}:{shadow}:png",
            env!("CARGO_PKG_VERSION"),
            super::stretch::SEIZA_STRETCH_VERSION,
            seiza_stacking::VERSION,
            master.id
        )
        .as_bytes(),
    );
    let base = std::fs::canonicalize(&ctx.cache_dir_path).map_err(|_| AppError::NotFound)?;
    let mut directory = base.clone();
    // Validate each existing ancestor before creating a child, including links.
    for component in ["previews", "calibration-masters"] {
        directory.push(component);
        match std::fs::create_dir(&directory) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(_) => {
                return Err(AppError::InternalError(
                    "Could not create master preview cache".into(),
                ))
            }
        }
        directory = std::fs::canonicalize(&directory).map_err(|_| AppError::NotFound)?;
        if !directory.starts_with(&base) || !directory.is_dir() {
            return Err(AppError::NotFound);
        }
    }
    let cache_path = directory.join(format!("{key}.png"));
    if cache_path.exists()
        && !std::fs::canonicalize(&cache_path)
            .map_err(|_| AppError::NotFound)?
            .starts_with(&directory)
    {
        return Err(AppError::NotFound);
    }
    Ok(GenJob {
        fits_path: path,
        cache_path,
        kind: GenKind::CalibrationMaster {
            midtone,
            shadow,
            max_dimensions: crate::server::preview_queue::max_dimensions_for_size(size),
        },
        encoding: crate::preview_format::PreviewEncoding::png(),
    })
}

fn generation_status(state: &Arc<AppState>, job: GenJob) -> GenerationStatus {
    if let Some(status) = state
        .preview_queue
        .status_for_source(&job.cache_path, &job.fits_path)
    {
        return status;
    }
    state.enqueue_preview(job);
    GenerationStatus {
        state: GenerationState::Generating,
        error: None,
    }
}

pub async fn get_catalog(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path(path): Path<MasterPath>,
    Query(query): Query<MasterQuery>,
) -> Result<Json<ApiResponse<MasterCatalog>>, AppError> {
    let source = path.source(query.revision);
    let result = tokio::task::spawn_blocking(move || catalog(&state, &ctx, &source))
        .await
        .map_err(|_| AppError::InternalError("Master inspection failed".into()))??;
    Ok(Json(ApiResponse::success(result)))
}

pub async fn get_preview(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path(path): Path<MasterPath>,
    Query(query): Query<MasterQuery>,
) -> Result<Response, AppError> {
    let display = query.display();
    let source = path.source(query.revision);
    let id = path.master_id.ok_or(AppError::NotFound)?;
    let (status, cache_path) = tokio::task::spawn_blocking(move || {
        let (master, source_path) = resolve_master(&state, &ctx, &source, &id)?;
        let job = preview_job(&ctx, &master, source_path, &display)?;
        let path = job.cache_path.clone();
        Ok::<_, AppError>((generation_status(&state, job), path))
    })
    .await
    .map_err(|_| AppError::InternalError("Master preview preparation failed".into()))??;
    match status.state {
        GenerationState::Ready => serve_file(&cache_path, "image/png", None).await,
        GenerationState::Generating => Ok((
            StatusCode::ACCEPTED,
            [(CACHE_CONTROL, "no-store")],
            Json(ApiResponse::success(status)),
        )
            .into_response()),
        GenerationState::Error => {
            Err(AppError::InternalError(status.error.unwrap_or_else(|| {
                "Master preview generation failed".into()
            })))
        }
    }
}

pub async fn download_fits(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path(path): Path<MasterPath>,
    Query(query): Query<MasterQuery>,
) -> Result<Response, AppError> {
    let source = path.source(query.revision);
    let id = path.master_id.ok_or(AppError::NotFound)?;
    let (master, path) =
        tokio::task::spawn_blocking(move || resolve_master(&state, &ctx, &source, &id))
            .await
            .map_err(|_| AppError::InternalError("Master download preparation failed".into()))??;
    serve_file(&path, "application/fits", Some(&master.label)).await
}

async fn serve_file(
    path: &FsPath,
    content_type: &str,
    filename: Option<&str>,
) -> Result<Response, AppError> {
    let file = tokio::fs::File::open(path)
        .await
        .map_err(|_| AppError::NotFound)?;
    let length = file
        .metadata()
        .await
        .map_err(|_| AppError::InternalError("Could not read master file metadata".into()))?
        .len();
    let mut response = Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, content_type)
        .header(CONTENT_LENGTH, length)
        .header(CACHE_CONTROL, "private, no-store");
    if let Some(filename) = filename {
        response = response.header(
            CONTENT_DISPOSITION,
            format!("attachment; filename=\"{filename}\""),
        );
    }
    response
        .body(Body::from_stream(ReaderStream::new(file)))
        .map_err(|_| AppError::InternalError("Could not serve master file".into()))
}

pub async fn post_generation_status(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Json(request): Json<MasterStatusRequest>,
) -> Result<Json<ApiResponse<GenerationStatusBatch>>, AppError> {
    if request.requests.len() > MAX_STATUS_BATCH {
        return Err(AppError::BadRequest(
            "Too many master preview status requests".into(),
        ));
    }
    let statuses = tokio::task::spawn_blocking(move || {
        request
            .requests
            .into_iter()
            .map(|item| {
                let result = resolve_master(&state, &ctx, &item.source, &item.master_id)
                    .and_then(|(master, path)| preview_job(&ctx, &master, path, &item.display));
                match result {
                    Ok(job) => generation_status(&state, job),
                    Err(AppError::Conflict(_)) => GenerationStatus {
                        state: GenerationState::Error,
                        error: Some(
                            "The displayed stack changed; reopen its calibration masters".into(),
                        ),
                    },
                    Err(_) => GenerationStatus {
                        state: GenerationState::Error,
                        error: Some("The recorded calibration master is unavailable".into()),
                    },
                }
            })
            .collect()
    })
    .await
    .map_err(|_| AppError::InternalError("Master preview status failed".into()))?;
    Ok(Json(ApiResponse::success(GenerationStatusBatch {
        statuses,
    })))
}

#[cfg(test)]
mod tests;
