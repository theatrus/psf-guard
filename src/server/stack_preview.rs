//! Project-scoped, per-target/per-filter stacking previews.
//!
//! PSF Guard owns frame selection and provenance. `seiza-stacking` owns
//! FITS decoding, calibration, registration, normalization, admission, and
//! ordered accumulation. Jobs are process-global and run one at a time so a
//! multi-database server cannot multiply the stacker's full-frame buffers.

pub mod artifact;
pub mod color;
mod janitor;
mod resume;
pub mod stretch;

use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{
        header::{CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_TYPE},
        StatusCode,
    },
    response::Response,
    Json,
};
use rayon::ThreadPoolBuilder;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path as FsPath, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::Semaphore;
use tokio_util::io::ReaderStream;

use crate::acquisition_context::FramingResolver;
use crate::db::Database;
use crate::models::AcquiredImage;
use crate::sequence_analysis::{
    extract_metrics_from_metadata, ImageQualityResult, SequenceAnalyzer, SequenceAnalyzerConfig,
};
use crate::server::api::ApiResponse;
use crate::server::database_context::DatabaseContext;
use crate::server::extract::DbContext;
use crate::server::handlers::AppError;
use crate::server::state::AppState;

pub const SEIZA_STACKING_VERSION: &str = "0.2.1";
/// Bump whenever stack admission, rendering, or persisted artifact semantics
/// change. This deliberately versions PSF Guard policy separately from Seiza.
pub(super) const STACK_PREVIEW_CACHE_VERSION: u32 = 11;
const MAX_REQUEST_IMAGES: usize = 10_000;
const MAX_REMEMBERED_JOBS: usize = 64;
const PREVIEW_MAX_DIMENSION: u32 = 2400;
const STACK_BYTES_PER_OUTPUT_SAMPLE: u64 = 40;

#[derive(Debug, Clone, Deserialize)]
pub struct StackPreviewRequest {
    pub image_ids: Vec<i32>,
    #[serde(default)]
    pub accepted_only: bool,
    #[serde(default)]
    pub force: bool,
    /// Reproject the integration onto the canonical north-up, east-left grid.
    /// Off by default: a stack keeps the rotation of its reference frame, and
    /// registration already absorbs a meridian flip. Turn this on when several
    /// stacks must share one sky frame, such as a mosaic.
    #[serde(default)]
    pub north_up: bool,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StackPreviewImageSize {
    #[default]
    Screen,
    Original,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub struct StackPreviewImageQuery {
    #[serde(default)]
    pub size: StackPreviewImageSize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StackJobState {
    Queued,
    Running,
    Completed,
    Failed,
    /// Stopped on request. Whatever the job had built is discarded; the
    /// project's last successful previews are left alone.
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StackGroupState {
    Queued,
    Running,
    Ready,
    Skipped,
    Error,
    /// Stopped before this channel finished, on request.
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StackFrameDecision {
    pub image_id: i32,
    pub disposition: String,
    pub reason: Option<String>,
    pub quality_score: Option<f64>,
    pub matched_stars: Option<usize>,
    pub registration_rms_pixels: Option<f64>,
    pub registration_drift_pixels: Option<f64>,
    #[serde(default)]
    pub registered_mapping: Option<seiza_stacking::RegisteredFrameMapping>,
    #[serde(default)]
    pub normalization_mean_gain: Option<f32>,
    #[serde(default)]
    pub normalization_mean_offset: Option<f32>,
    #[serde(default)]
    pub source_fingerprint: Option<String>,
    pub overlap_fraction: Option<f32>,
    pub integrated_fraction: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StackInputImage {
    pub image_id: i32,
    pub grading_status: i32,
}

/// Convention recorded when a stack keeps the rotation of its reference frame.
/// The stored mapping is the identity, so every consumer of `source_to_output`
/// works the same way for an oriented and an unoriented stack.
pub const SOURCE_ORIENTATION_NAME: &str = "source_frame";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StackSkyOrientation {
    pub convention: String,
    pub version: u32,
    pub source: String,
    pub output_width: usize,
    pub output_height: usize,
    pub source_to_output: seiza_stacking::AffineTransform,
}

impl StackSkyOrientation {
    /// The mapping recorded when the integration is published in the reference
    /// frame's own rotation. `decided_by` names the anchor that chose it.
    fn source_frame(width: usize, height: usize, decided_by: &str) -> Self {
        Self {
            convention: SOURCE_ORIENTATION_NAME.into(),
            version: seiza_stacking::SKY_ORIENTATION_VERSION,
            source: decided_by.into(),
            output_width: width,
            output_height: height,
            source_to_output: seiza_stacking::AffineTransform::IDENTITY,
        }
    }

    /// The mapping recorded when the integration is turned half a turn out of
    /// the reference frame's rotation.
    fn source_frame_half_turn(width: usize, height: usize, decided_by: &str) -> Self {
        Self {
            source_to_output: half_turn_transform(width, height),
            ..Self::source_frame(width, height, decided_by)
        }
    }

    /// Whether this record still describes how the current code lays out a
    /// stack. Artifacts written before stacks recorded a mapping fail here.
    pub(super) fn is_current(&self) -> bool {
        self.version == seiza_stacking::SKY_ORIENTATION_VERSION
            && matches!(
                self.convention.as_str(),
                seiza_stacking::SKY_ORIENTATION_NAME | SOURCE_ORIENTATION_NAME
            )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StackGroupStatus {
    pub index: usize,
    pub target_id: i32,
    pub target_name: String,
    pub filter_name: String,
    pub state: StackGroupState,
    #[serde(default)]
    pub phase: String,
    pub total_candidates: usize,
    pub eligible_frames: usize,
    pub quality_excluded: usize,
    pub missing_files: usize,
    pub processed_frames: usize,
    pub accepted_frames: usize,
    pub rejected_frames: usize,
    /// Frames restored from a saved accumulator instead of being registered
    /// and integrated again. Zero for a from-scratch build.
    #[serde(default)]
    pub reused_frames: usize,
    /// Why a checkpoint that existed could not be extended, when that
    /// happened. Absent for a resumed build and for a first build.
    #[serde(default)]
    pub resume_note: Option<String>,
    #[serde(default)]
    pub output_channels: usize,
    #[serde(default)]
    pub sky_orientation: Option<StackSkyOrientation>,
    pub reference_image_id: Option<i32>,
    pub total_exposure_seconds: f64,
    pub preview_url: Option<String>,
    pub fits_url: Option<String>,
    pub error: Option<String>,
    #[serde(default)]
    pub calibration: crate::calibration::AppliedCalibration,
    #[serde(default)]
    pub input_images: Vec<StackInputImage>,
    pub frames: Vec<StackFrameDecision>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StackPreviewJob {
    pub schema_version: u32,
    pub job_id: String,
    pub database_id: String,
    pub project_id: i32,
    pub state: StackJobState,
    pub accepted_only: bool,
    pub created_unix_seconds: i64,
    #[serde(default)]
    pub artifact_revision: String,
    #[serde(default)]
    pub cache_version: u32,
    pub stacking_version: String,
    pub groups: Vec<StackGroupStatus>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LatestStackPreviewGroup {
    pub job_id: String,
    pub artifact_revision: String,
    pub accepted_only: bool,
    pub created_unix_seconds: i64,
    #[serde(default)]
    pub cache_version: u32,
    pub group: StackGroupStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LatestStackPreviews {
    pub schema_version: u32,
    pub database_id: String,
    pub project_id: i32,
    pub updated_unix_seconds: i64,
    pub groups: Vec<LatestStackPreviewGroup>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StackActivityKind {
    Mono,
    Color,
}

/// One queued or running stack build, described well enough for a header
/// indicator and for a panel to re-attach to a job it did not start.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StackActivityEntry {
    pub kind: StackActivityKind,
    pub job_id: String,
    pub database_id: String,
    pub project_id: i32,
    pub state: StackJobState,
    /// Target and channel of the work in flight.
    pub label: String,
    /// Short phase text, for example `Registering frames`.
    pub detail: String,
    pub processed_units: usize,
    pub total_units: usize,
    pub created_unix_seconds: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StackActivity {
    pub schema_version: u32,
    pub active: Vec<StackActivityEntry>,
}

fn channel_label(target_name: &str, filter_name: &str) -> String {
    if filter_name.is_empty() {
        target_name.to_string()
    } else {
        format!("{target_name} · {filter_name}")
    }
}

fn mono_activity(job: &StackPreviewJob) -> StackActivityEntry {
    let pending = job.groups.iter().find(|group| {
        matches!(
            group.state,
            StackGroupState::Running | StackGroupState::Queued
        )
    });
    let label = match pending {
        Some(group) => {
            let base = channel_label(&group.target_name, &group.filter_name);
            let remaining = job
                .groups
                .iter()
                .filter(|entry| {
                    entry.index != group.index
                        && matches!(
                            entry.state,
                            StackGroupState::Running | StackGroupState::Queued
                        )
                })
                .count();
            if remaining > 0 {
                format!("{base} +{remaining} more")
            } else {
                base
            }
        }
        None => format!("{} channels", job.groups.len()),
    };
    let detail = match pending {
        Some(group) if group.state == StackGroupState::Queued => "Waiting for stacker".to_string(),
        Some(group) => match group.phase.as_str() {
            "calibration" => "Building calibration masters".to_string(),
            "rendering" => "Rendering preview".to_string(),
            _ => "Registering frames".to_string(),
        },
        None => "Preparing stack".to_string(),
    };
    let total_units = job
        .groups
        .iter()
        .filter(|group| group.state != StackGroupState::Skipped)
        .map(|group| group.eligible_frames)
        .sum();
    let processed_units = job
        .groups
        .iter()
        .filter(|group| group.state != StackGroupState::Skipped)
        .map(|group| {
            if group.state == StackGroupState::Ready {
                group.eligible_frames
            } else {
                group.processed_frames.min(group.eligible_frames)
            }
        })
        .sum();
    StackActivityEntry {
        kind: StackActivityKind::Mono,
        job_id: job.job_id.clone(),
        database_id: job.database_id.clone(),
        project_id: job.project_id,
        state: job.state,
        label,
        detail,
        processed_units,
        total_units,
        created_unix_seconds: job.created_unix_seconds,
    }
}

fn color_activity(job: &color::StackColorJob) -> StackActivityEntry {
    let (processed_units, total_units) = if job.progress.total_units > 0 {
        (job.progress.completed_units, job.progress.total_units)
    } else {
        (job.processed_channels, job.total_channels)
    };
    StackActivityEntry {
        kind: StackActivityKind::Color,
        job_id: job.job_id.clone(),
        database_id: job.database_id.clone(),
        project_id: job.project_id,
        state: job.state,
        label: channel_label(&job.target_name, &job.label),
        detail: if job.phase.is_empty() {
            "Composing color".to_string()
        } else {
            job.phase.clone()
        },
        processed_units,
        total_units,
        created_unix_seconds: job.created_unix_seconds,
    }
}

pub struct StackPreviewManager {
    jobs: Mutex<HashMap<String, StackPreviewJob>>,
    color_jobs: Mutex<HashMap<String, color::StackColorJob>>,
    artifact_jobs: Mutex<HashMap<String, artifact::ArtifactSearchJob>>,
    /// Stop flags for stack jobs that are queued or running. The worker reads
    /// its own flag between frames and phases; the entry is dropped when the
    /// job leaves the queue.
    cancels: Mutex<HashMap<String, Arc<AtomicBool>>>,
    latest_write: Mutex<()>,
    permit: Arc<Semaphore>,
}

impl StackPreviewManager {
    pub fn new() -> Self {
        Self {
            jobs: Mutex::new(HashMap::new()),
            color_jobs: Mutex::new(HashMap::new()),
            artifact_jobs: Mutex::new(HashMap::new()),
            cancels: Mutex::new(HashMap::new()),
            latest_write: Mutex::new(()),
            permit: Arc::new(Semaphore::new(1)),
        }
    }

    pub fn get(&self, job_id: &str) -> Option<StackPreviewJob> {
        self.jobs.lock().unwrap().get(job_id).cloned()
    }

    /// Register a stop flag for a job about to be queued.
    fn track_cancel(&self, job_id: &str) -> Arc<AtomicBool> {
        let flag = Arc::new(AtomicBool::new(false));
        self.cancels
            .lock()
            .unwrap()
            .insert(job_id.to_string(), Arc::clone(&flag));
        flag
    }

    fn forget_cancel(&self, job_id: &str) {
        self.cancels.lock().unwrap().remove(job_id);
    }

    /// Ask a queued or running job to stop. Returns false when the job is not
    /// in flight, which includes a job that already finished.
    pub fn request_cancel(&self, job_id: &str) -> bool {
        let Some(flag) = self.cancels.lock().unwrap().get(job_id).cloned() else {
            return false;
        };
        flag.store(true, Ordering::Relaxed);
        true
    }

    /// Every queued or running stack build, oldest first. Jobs outlive the
    /// page that started them, so a client that navigated away can still see
    /// the work and re-attach to it.
    pub fn active(&self) -> Vec<StackActivityEntry> {
        let mut active: Vec<StackActivityEntry> = {
            let jobs = self.jobs.lock().unwrap();
            jobs.values()
                .filter(|job| matches!(job.state, StackJobState::Queued | StackJobState::Running))
                .map(mono_activity)
                .collect()
        };
        {
            let color_jobs = self.color_jobs.lock().unwrap();
            active.extend(
                color_jobs
                    .values()
                    .filter(|job| {
                        matches!(job.state, StackJobState::Queued | StackJobState::Running)
                    })
                    .map(color_activity),
            );
        }
        active.sort_by(|left, right| {
            left.created_unix_seconds
                .cmp(&right.created_unix_seconds)
                .then_with(|| left.job_id.cmp(&right.job_id))
        });
        active
    }

    /// Everything on disk that is still referenced: jobs a panel may poll,
    /// and jobs a durable latest index still names. Reads every project's
    /// index because a cache root hosts every project of a database.
    fn cache_keep_set(&self, cache_root: &FsPath) -> janitor::KeepSet {
        let mut mono_job_ids: std::collections::HashSet<String> =
            self.jobs.lock().unwrap().keys().cloned().collect();
        let mut color_job_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut color_input_ids: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        for job in self.color_jobs.lock().unwrap().values() {
            color_job_ids.insert(job.job_id.clone());
            if let Some(input_id) = &job.linear_input_id {
                color_input_ids.insert(input_id.clone());
            }
        }
        for latest in read_latest_indices::<LatestStackPreviews>(&cache_root.join("stack-previews"))
        {
            for group in latest.groups {
                mono_job_ids.insert(group.job_id);
            }
        }
        for (job_id, input_id) in color::latest_color_references(cache_root) {
            color_job_ids.insert(job_id);
            if let Some(input_id) = input_id {
                color_input_ids.insert(input_id);
            }
        }
        janitor::KeepSet {
            mono_job_ids,
            color_job_ids,
            color_input_ids,
        }
    }

    /// Sweep superseded artifacts after a build settles. Failures are logged;
    /// pruning never fails a build.
    pub(super) fn prune_cache(&self, cache_root: &FsPath) {
        let keep = self.cache_keep_set(cache_root);
        janitor::prune(cache_root, &keep, SEIZA_STACKING_VERSION);
    }

    pub(crate) async fn acquire_maintenance_permit(
        &self,
    ) -> Result<tokio::sync::OwnedSemaphorePermit, String> {
        Arc::clone(&self.permit)
            .acquire_owned()
            .await
            .map_err(|_| "stack preview worker has stopped".to_string())
    }

    fn insert(&self, job: StackPreviewJob) -> bool {
        let mut jobs = self.jobs.lock().unwrap();
        if jobs.len() >= MAX_REMEMBERED_JOBS && !jobs.contains_key(&job.job_id) {
            let Some(oldest) = jobs
                .values()
                .filter(|entry| {
                    matches!(
                        entry.state,
                        StackJobState::Completed | StackJobState::Failed | StackJobState::Cancelled
                    )
                })
                .min_by_key(|entry| entry.created_unix_seconds)
                .map(|entry| entry.job_id.clone())
            else {
                return false;
            };
            jobs.remove(&oldest);
        }
        jobs.insert(job.job_id.clone(), job);
        true
    }

    fn update(&self, job_id: &str, update: impl FnOnce(&mut StackPreviewJob)) {
        if let Some(job) = self.jobs.lock().unwrap().get_mut(job_id) {
            update(job);
        }
    }

    fn persist_latest(&self, cache_root: &FsPath, job: &StackPreviewJob) -> Result<(), String> {
        let _guard = self.latest_write.lock().unwrap();
        persist_latest_groups(cache_root, job)
    }
}

impl Default for StackPreviewManager {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone)]
struct PreparedFrame {
    image_id: i32,
    acquired_date: Option<i64>,
    quality_score: Option<f64>,
    path: PathBuf,
    source_fingerprint: String,
    expected_target: Option<(f64, f64)>,
    /// Exposure length from the catalog, for weighting the orientation vote
    /// and totalling a group's integration time.
    ///
    /// Taken from the record rather than the opened frame: the stacking
    /// pipeline opens frames on its own threads and reports back only the
    /// disposition, and PSF Guard already knows this from the import.
    exposure_seconds: f64,
}

/// Exposure length in seconds from an acquired-image record, or zero when the
/// record does not say.
///
/// N.I.N.A. and PSF Guard's own importer spell this differently, and a value
/// can arrive as a number or as text, so take the first that reads as a finite
/// positive number.
fn exposure_seconds_from_metadata(metadata_json: &str) -> f64 {
    let Ok(metadata) = serde_json::from_str::<serde_json::Value>(metadata_json) else {
        return 0.0;
    };
    ["ExposureDuration", "ExposureTime", "EXPTIME"]
        .iter()
        .find_map(|key| {
            let value = &metadata[*key];
            value
                .as_f64()
                .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
        })
        .filter(|value: &f64| value.is_finite() && *value > 0.0)
        .unwrap_or(0.0)
}

/// The decision recorded for a frame that did not reach the accumulator,
/// whether the stack turned it away or it could not be read at all.
fn rejected_decision(frame: &PreparedFrame, reason: String) -> StackFrameDecision {
    StackFrameDecision {
        image_id: frame.image_id,
        disposition: "rejected".into(),
        reason: Some(reason),
        quality_score: frame.quality_score,
        matched_stars: None,
        registration_rms_pixels: None,
        registration_drift_pixels: None,
        registered_mapping: None,
        normalization_mean_gain: None,
        normalization_mean_offset: None,
        source_fingerprint: Some(frame.source_fingerprint.clone()),
        overlap_fraction: None,
        integrated_fraction: None,
    }
}

#[derive(Clone)]
struct PreparedGroup {
    index: usize,
    frames: Vec<PreparedFrame>,
}

struct PreparedJob {
    public: StackPreviewJob,
    groups: Vec<PreparedGroup>,
    cache_root: PathBuf,
    north_up: bool,
}

/// Weighs how much integrated exposure faces the same way as the reference
/// frame against how much faces half a turn away.
///
/// A German equatorial mount turns the field 180 degrees across a meridian
/// flip. Registration matches star triangles at any angle, so both halves of
/// such a night stack cleanly, but the reference frame alone decides which way
/// up the result comes out — and the reference is the best-scoring frame, which
/// may sit on either side of the flip and may move between rebuilds. Following
/// the exposure instead keeps the published stack facing the way most of the
/// night was shot, and keeps it there when scores change.
#[derive(Debug, Default)]
struct OrientationVote {
    upright: f64,
    half_turned: f64,
}

impl OrientationVote {
    /// Count one integrated frame at its registered rotation. Frames without a
    /// usable exposure time count once each, so a session that records no
    /// exposure still votes by frame.
    fn add(&mut self, rotation_radians: f64, exposure_seconds: f64) {
        let weight = if exposure_seconds > 0.0 {
            exposure_seconds
        } else {
            1.0
        };
        if is_half_turn(rotation_radians) {
            self.half_turned += weight;
        } else {
            self.upright += weight;
        }
    }

    /// Whether most of the integrated exposure sits half a turn from the
    /// reference frame. A tie keeps the reference frame's own rotation.
    fn prefers_half_turn(&self) -> bool {
        self.half_turned > self.upright
    }
}

/// What decided which way up a stack was published. Recorded so a reader can
/// tell an anchored choice from a guess.
mod orientation_source {
    /// The reference frame's own solved or embedded sky rotation.
    pub const SKY_ANCHOR: &str = "sky_anchor";
    /// The reference frame's side of the pier.
    pub const PIER_SIDE: &str = "pier_side";
    /// Which way most of the stack's own exposure faced.
    pub const EXPOSURE_MAJORITY: &str = "exposure_majority";
}

/// Whether a solved frame already faces north-ish up.
///
/// `CD` maps a pixel offset to a world offset, so its second row is the
/// gradient of declination across the frame — the direction north points in
/// pixel space. Seiza's unrotated, unflipped WCS puts north toward decreasing
/// Y, so a negative Y component means the frame is already the right way up.
///
/// A frame whose north runs along a row has no meaningful Y component, and
/// solve noise either side of zero would split two channels that should agree.
/// Those fall back to the X component, which is far from zero exactly when the
/// Y one is not.
///
/// Returns `None` for a matrix that cannot describe a sky rotation at all, so
/// a broken solve falls through to the next anchor rather than being trusted
/// over it.
fn faces_north_up(cd: [[f64; 2]; 2]) -> Option<bool> {
    let determinant = cd[0][0].mul_add(cd[1][1], -cd[0][1] * cd[1][0]);
    if !determinant.is_finite() || determinant == 0.0 {
        return None;
    }
    let north_x = cd[1][0];
    let north_y = cd[1][1];
    if !north_x.is_finite() || !north_y.is_finite() {
        return None;
    }
    if north_y.abs() > north_x.abs() * 1.0e-6 {
        return Some(north_y < 0.0);
    }
    Some(north_x > 0.0)
}

/// Whether a frame sits west of the pier. `None` when the mount recorded
/// nothing usable.
fn is_west_of_pier(pier_side: &str) -> Option<bool> {
    let side = pier_side.trim().to_ascii_lowercase();
    let side = side.strip_prefix("pier").unwrap_or(&side).trim_start();
    match side.chars().next()? {
        'w' => Some(true),
        'e' => Some(false),
        _ => None,
    }
}

/// The reference frame's pier side, as N.I.N.A. and friends record it.
fn pier_side_from_headers(headers: &[(String, seiza_fits::HeaderValue)]) -> Option<&str> {
    headers.iter().find_map(|(key, value)| {
        if !key.eq_ignore_ascii_case("PIERSIDE") {
            return None;
        }
        match value {
            seiza_fits::HeaderValue::String(text) => Some(text.as_str()),
            _ => None,
        }
    })
}

/// What one group's reference frame says about which way up it is.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct ReferenceOrientation {
    /// Whether its own solved or embedded sky rotation puts north in the upper
    /// half of the frame.
    north_up: Option<bool>,
    /// Whether it sits west of the pier.
    west_of_pier: Option<bool>,
}

/// Whether a frame west of the pier faces north up on this rig, learned from
/// every reference frame in the job that reported both.
///
/// Pier side on its own says nothing about where north is, so canonicalising on
/// it is an arbitrary choice — and an arbitrary choice does not have to match
/// what a solved sibling channel decided. One solved frame anywhere in the job
/// settles it for the unsolved ones, which is what keeps the channels of a
/// target facing the same way.
///
/// Disagreeing frames are counted rather than trusted in order, so a single bad
/// solve cannot invert every unsolved channel. A tie teaches nothing.
fn calibrate_pier_side(orientations: &[ReferenceOrientation]) -> Option<bool> {
    let (mut west_up, mut west_down) = (0usize, 0usize);
    for orientation in orientations {
        let (Some(north_up), Some(west)) = (orientation.north_up, orientation.west_of_pier) else {
            continue;
        };
        if north_up == west {
            west_up += 1;
        } else {
            west_down += 1;
        }
    }
    match west_up.cmp(&west_down) {
        std::cmp::Ordering::Greater => Some(true),
        std::cmp::Ordering::Less => Some(false),
        std::cmp::Ordering::Equal => None,
    }
}

/// Which way up one reference frame faces, and what settled it.
///
/// Its own sky rotation wins. Failing that, its pier side read through the
/// job's calibration — or, when nothing in the job was solved, through an
/// arbitrary but shared assumption, which still leaves every channel agreeing
/// with the others.
fn anchored_north_up(
    orientation: ReferenceOrientation,
    pier_calibration: Option<bool>,
) -> Option<(bool, &'static str)> {
    if let Some(north_up) = orientation.north_up {
        return Some((north_up, orientation_source::SKY_ANCHOR));
    }
    let west = orientation.west_of_pier?;
    let west_is_north_up = pier_calibration.unwrap_or(true);
    Some((west == west_is_north_up, orientation_source::PIER_SIDE))
}

/// Whether the finished stack should be turned half a turn, and what decided
/// it.
///
/// An anchored reference frame is absolute, so every channel of a target
/// reaches the same answer on its own and the cards agree. The exposure
/// majority is only relative to the stack's own reference frame: it keeps one
/// stack self-consistent but cannot make two of them agree, so it is the last
/// resort.
fn half_turn_decision(
    anchor: Option<(bool, &'static str)>,
    vote: &OrientationVote,
) -> (bool, &'static str) {
    match anchor {
        Some((north_up, source)) => (!north_up, source),
        None => (
            vote.prefers_half_turn(),
            orientation_source::EXPOSURE_MAJORITY,
        ),
    }
}

/// Whether a registered rotation lies closer to half a turn than to none.
/// Ordinary field rotation and guiding drift stay near zero; only a meridian
/// flip lands a frame out here.
fn is_half_turn(rotation_radians: f64) -> bool {
    if !rotation_radians.is_finite() {
        return false;
    }
    let wrapped = rotation_radians.rem_euclid(std::f64::consts::TAU);
    let from_upright = wrapped.min(std::f64::consts::TAU - wrapped);
    from_upright > std::f64::consts::FRAC_PI_2
}

/// Turn a row-major, channel-interleaved image half a turn about its centre.
/// This is an exact reversal of the pixel order, so it costs no resampling and
/// loses no accuracy.
fn half_turn(mut image: seiza_stacking::LinearImage) -> seiza_stacking::LinearImage {
    let channels = image.channels;
    let mut turned = Vec::with_capacity(image.data.len());
    for pixel in image.data.chunks_exact(channels).rev() {
        turned.extend_from_slice(pixel);
    }
    image.data = turned;
    image
}

/// The source-to-output mapping matching [`half_turn`]: pixel `(0, 0)` lands at
/// the far corner and the grid keeps its size.
fn half_turn_transform(width: usize, height: usize) -> seiza_stacking::AffineTransform {
    seiza_stacking::AffineTransform {
        matrix: [[-1.0, 0.0], [0.0, -1.0]],
        translation_x: width.saturating_sub(1) as f64,
        translation_y: height.saturating_sub(1) as f64,
    }
}

pub async fn start_stack_previews(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db_id, project_id)): Path<(String, i32)>,
    Json(request): Json<StackPreviewRequest>,
) -> Result<Json<ApiResponse<StackPreviewJob>>, AppError> {
    validate_request(&request)?;

    let ctx_arc = Arc::clone(&ctx.0);
    let request_for_prepare = request.clone();
    let prepared = tokio::task::spawn_blocking(move || {
        prepare_job(&ctx_arc, project_id, &request_for_prepare)
    })
    .await
    .map_err(|error| {
        AppError::InternalError(format!("Stack preparation task failed: {error}"))
    })??;

    let manifest_path = manifest_path(&prepared.cache_root, &prepared.public.job_id);
    if let Some(existing) = state.stack_previews.get(&prepared.public.job_id)
        && (matches!(
            existing.state,
            StackJobState::Queued | StackJobState::Running
        ) || (!request.force && existing.state == StackJobState::Completed))
    {
        if existing.state == StackJobState::Completed
            && let Err(error) = state
                .stack_previews
                .persist_latest(&prepared.cache_root, &existing)
        {
            tracing::warn!("Failed to refresh latest stack preview index: {error}");
        }
        return Ok(Json(ApiResponse::success(existing)));
    }
    if !request.force
        && let Ok(bytes) = std::fs::read(&manifest_path)
        && let Ok(existing) = serde_json::from_slice::<StackPreviewJob>(&bytes)
        && existing.state == StackJobState::Completed
    {
        if let Err(error) = state
            .stack_previews
            .persist_latest(&prepared.cache_root, &existing)
        {
            tracing::warn!("Failed to refresh latest stack preview index: {error}");
        }
        let _ = state.stack_previews.insert(existing.clone());
        return Ok(Json(ApiResponse::success(existing)));
    }

    let response = prepared.public.clone();
    if !state.stack_previews.insert(response.clone()) {
        return Err(AppError::BadRequest(format!(
            "At most {MAX_REMEMBERED_JOBS} stack preview jobs may be active at once"
        )));
    }
    enqueue_job(Arc::clone(&state), prepared);
    Ok(Json(ApiResponse::success(response)))
}

/// Ask a queued or running stack build to stop. Returns the job as it stands;
/// the worker settles it into `cancelled` within a frame's work. A job that
/// already finished is refused rather than silently accepted, so the UI can
/// tell "stopped" from "too late".
pub async fn cancel_stack_preview_job(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db_id, project_id, job_id)): Path<(String, i32, String)>,
) -> Result<Json<ApiResponse<StackPreviewJob>>, AppError> {
    validate_job_id(&job_id)?;
    let job = state
        .stack_previews
        .get(&job_id)
        .ok_or(AppError::NotFound)?;
    if job.database_id != ctx.id || job.project_id != project_id {
        return Err(AppError::NotFound);
    }
    if !state.stack_previews.request_cancel(&job_id) {
        return Err(AppError::BadRequest(
            "This stack build has already finished".into(),
        ));
    }
    Ok(Json(ApiResponse::success(
        state.stack_previews.get(&job_id).unwrap_or(job),
    )))
}

/// Cross-database view of stack builds still in flight. The manager is
/// process-global, so this stays outside the per-database routes and lets the
/// header report stacking from any view.
pub async fn get_stack_activity(
    State(state): State<Arc<AppState>>,
) -> Json<ApiResponse<StackActivity>> {
    Json(ApiResponse::success(StackActivity {
        schema_version: 1,
        active: state.stack_previews.active(),
    }))
}

pub async fn get_latest_stack_previews(
    ctx: DbContext,
    Path((_db_id, project_id)): Path<(String, i32)>,
) -> Result<Json<ApiResponse<LatestStackPreviews>>, AppError> {
    let path = latest_path(&ctx.cache_dir_path, project_id);
    let latest = match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice::<LatestStackPreviews>(&bytes).map_err(|error| {
            AppError::InternalError(format!("Invalid latest stack preview index: {error}"))
        })?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => LatestStackPreviews {
            schema_version: 1,
            database_id: ctx.id.clone(),
            project_id,
            updated_unix_seconds: 0,
            groups: Vec::new(),
        },
        Err(error) => {
            return Err(AppError::InternalError(format!(
                "Failed to read latest stack preview index: {error}"
            )))
        }
    };
    let latest = current_latest_stacks(latest);
    if latest.database_id != ctx.id || latest.project_id != project_id {
        return Err(AppError::NotFound);
    }
    Ok(Json(ApiResponse::success(latest)))
}

pub async fn get_stack_preview_job(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db_id, project_id, job_id)): Path<(String, i32, String)>,
) -> Result<Json<ApiResponse<StackPreviewJob>>, AppError> {
    validate_job_id(&job_id)?;
    if let Some(job) = state.stack_previews.get(&job_id) {
        if job.database_id != ctx.id || job.project_id != project_id {
            return Err(AppError::NotFound);
        }
        return Ok(Json(ApiResponse::success(job)));
    }
    let path = manifest_path(&ctx.cache_dir_path, &job_id);
    let bytes = std::fs::read(path).map_err(|_| AppError::NotFound)?;
    let job: StackPreviewJob = serde_json::from_slice(&bytes)
        .map_err(|error| AppError::InternalError(format!("Invalid stack manifest: {error}")))?;
    if job.database_id != ctx.id || job.project_id != project_id {
        return Err(AppError::NotFound);
    }
    let _ = state.stack_previews.insert(job.clone());
    Ok(Json(ApiResponse::success(job)))
}

pub async fn get_stack_preview_image(
    ctx: DbContext,
    Path((_db_id, job_id, group_index)): Path<(String, String, usize)>,
    Query(query): Query<StackPreviewImageQuery>,
) -> Result<Response, AppError> {
    validate_job_id(&job_id)?;
    let path = match query.size {
        StackPreviewImageSize::Screen => preview_path(&ctx.cache_dir_path, &job_id, group_index),
        StackPreviewImageSize::Original => {
            original_preview_path(&ctx.cache_dir_path, &job_id, group_index)
        }
    };
    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|_| AppError::NotFound)?;
    let length = file
        .metadata()
        .await
        .map_err(|error| AppError::InternalError(format!("Failed to stat stack PNG: {error}")))?
        .len();
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "image/png")
        .header(CONTENT_LENGTH, length)
        .header(CACHE_CONTROL, "private, max-age=31536000, immutable")
        .body(Body::from_stream(ReaderStream::new(file)))
        .map_err(|error| {
            AppError::InternalError(format!("Failed to build stack PNG response: {error}"))
        })
}

pub async fn download_stack_preview_fits(
    ctx: DbContext,
    Path((_db_id, job_id, group_index)): Path<(String, String, usize)>,
) -> Result<Response, AppError> {
    validate_job_id(&job_id)?;
    let path = fits_path(&ctx.cache_dir_path, &job_id, group_index);
    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|_| AppError::NotFound)?;
    let length = file
        .metadata()
        .await
        .map_err(|error| AppError::InternalError(format!("Failed to stat stack FITS: {error}")))?
        .len();
    let filename = format!("psf-guard-stack-{}-{group_index}.fits", &job_id[..12]);
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "application/fits")
        .header(CONTENT_LENGTH, length)
        .header(
            CONTENT_DISPOSITION,
            format!("attachment; filename=\"{filename}\""),
        )
        .header(CACHE_CONTROL, "private, max-age=31536000, immutable")
        .body(Body::from_stream(ReaderStream::new(file)))
        .map_err(|error| {
            AppError::InternalError(format!("Failed to build stack FITS response: {error}"))
        })
}

pub async fn apply_stack_preview_stretch(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db_id, job_id, group_index)): Path<(String, String, usize)>,
    Json(request): Json<stretch::StackViewProcessingRequest>,
) -> Result<Json<ApiResponse<stretch::StackStretchPreview>>, AppError> {
    validate_job_id(&job_id)?;
    let job = if let Some(job) = state.stack_previews.get(&job_id) {
        job
    } else {
        let bytes = std::fs::read(manifest_path(&ctx.cache_dir_path, &job_id))
            .map_err(|_| AppError::NotFound)?;
        serde_json::from_slice::<StackPreviewJob>(&bytes).map_err(|error| {
            AppError::InternalError(format!("Invalid stack preview manifest: {error}"))
        })?
    };
    if job.database_id != ctx.id {
        return Err(AppError::NotFound);
    }
    let group = job
        .groups
        .get(group_index)
        .filter(|group| group.index == group_index && group.state == StackGroupState::Ready)
        .ok_or(AppError::NotFound)?;
    let source = fits_path(&ctx.cache_dir_path, &job_id, group.index);
    let result = stretch::apply_to_fits(
        state,
        ctx.id.clone(),
        ctx.cache_dir_path.clone(),
        format!("mono:{job_id}:{}", group.index),
        job.artifact_revision,
        source,
        request,
    )
    .await?;
    Ok(stretch::response(result))
}

fn validate_request(request: &StackPreviewRequest) -> Result<(), AppError> {
    if request.image_ids.len() < 2 {
        return Err(AppError::BadRequest(
            "Stack previews require at least two image IDs".into(),
        ));
    }
    if request.image_ids.len() > MAX_REQUEST_IMAGES {
        return Err(AppError::BadRequest(format!(
            "Stack preview requests are limited to {MAX_REQUEST_IMAGES} images"
        )));
    }
    let unique = request.image_ids.iter().copied().collect::<HashSet<_>>();
    if unique.len() != request.image_ids.len() {
        return Err(AppError::BadRequest(
            "Stack preview image IDs must be unique".into(),
        ));
    }
    Ok(())
}

fn validate_job_id(job_id: &str) -> Result<(), AppError> {
    if job_id.len() == 64 && job_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(AppError::BadRequest("Invalid stack preview job ID".into()))
    }
}

fn new_artifact_revision() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{nanos:x}-{:x}", std::process::id())
}

fn prepare_job(
    ctx: &Arc<DatabaseContext>,
    project_id: i32,
    request: &StackPreviewRequest,
) -> Result<PreparedJob, AppError> {
    let requested = request.image_ids.iter().copied().collect::<HashSet<_>>();
    let (project_images, expected_by_image) = {
        let conn = ctx.db();
        let conn = conn.lock().map_err(AppError::db)?;
        let db = Database::new(&conn);
        let images = db
            .get_images_by_project_id(project_id)
            .map_err(AppError::db)?;
        if images.is_empty() {
            return Err(AppError::BadRequest(format!(
                "Project {project_id} has no images"
            )));
        }
        let found = images
            .iter()
            .filter(|(image, _, _)| requested.contains(&image.id))
            .count();
        if found != requested.len() {
            return Err(AppError::BadRequest(
                "Every requested image must belong to the selected project".into(),
            ));
        }
        let selected_groups = images
            .iter()
            .filter(|(image, _, _)| requested.contains(&image.id))
            .map(|(image, _, _)| (image.target_id, image.filter_name.clone()))
            .collect::<HashSet<_>>();
        let relevant = images
            .into_iter()
            .filter(|(image, _, _)| {
                selected_groups.contains(&(image.target_id, image.filter_name.clone()))
            })
            .collect::<Vec<_>>();
        let mut resolver = FramingResolver::new(&conn).map_err(AppError::db)?;
        let expected = relevant
            .iter()
            .map(|(image, _, _)| {
                resolver
                    .expected_for_grading(&conn, image)
                    .map(|value| (image.id, value))
            })
            .collect::<Result<HashMap<_, _>, _>>()
            .map_err(AppError::db)?;
        (relevant, expected)
    };

    let quality = quality_results(ctx, &project_images, &expected_by_image);
    let quality_by_id = quality
        .into_iter()
        .map(|result| (result.image_id, result))
        .collect::<HashMap<_, _>>();

    let mut grouped: BTreeMap<(i32, String, String), Vec<(AcquiredImage, ImageQualityResult)>> =
        BTreeMap::new();
    for (image, _project_name, target_name) in project_images {
        if !requested.contains(&image.id) {
            continue;
        }
        let scored = quality_by_id
            .get(&image.id)
            .cloned()
            .unwrap_or_else(|| fallback_quality(image.id));
        grouped
            .entry((image.target_id, target_name, image.filter_name.clone()))
            .or_default()
            .push((image, scored));
    }

    let mut public_groups = Vec::new();
    let mut prepared_groups = Vec::new();
    let artifact_revision = new_artifact_revision();
    let mut hasher = Sha256::new();
    hasher.update(ctx.id.as_bytes());
    hasher.update(project_id.to_le_bytes());
    hasher.update([request.accepted_only as u8]);
    hasher.update([request.north_up as u8]);
    hasher.update(STACK_PREVIEW_CACHE_VERSION.to_le_bytes());
    hasher.update(SEIZA_STACKING_VERSION.as_bytes());
    hasher.update(seiza_stacking::SKY_ORIENTATION_VERSION.to_le_bytes());
    hasher.update(seiza_stacking::SKY_ORIENTATION_NAME.as_bytes());
    hasher.update(PREVIEW_MAX_DIMENSION.to_le_bytes());
    hasher.update(stretch::SEIZA_STRETCH_VERSION.as_bytes());

    for (index, ((target_id, target_name, filter_name), mut entries)) in
        grouped.into_iter().enumerate()
    {
        hasher.update(target_id.to_le_bytes());
        hasher.update(target_name.as_bytes());
        hasher.update(filter_name.as_bytes());
        entries.sort_by_key(|(image, _)| (image.acquired_date.unwrap_or(0), image.id));
        let total_candidates = entries.len();
        let input_images = entries
            .iter()
            .map(|(image, _)| StackInputImage {
                image_id: image.id,
                grading_status: image.grading_status,
            })
            .collect();
        let mut quality_excluded = 0usize;
        let mut missing_files = 0usize;
        let mut decisions = Vec::new();
        let mut frames = Vec::new();

        for (image, scored) in entries {
            hasher.update(image.id.to_le_bytes());
            hasher.update(image.grading_status.to_le_bytes());
            hasher.update(image.acquired_date.unwrap_or(0).to_le_bytes());
            hasher.update(scored.quality_score.to_le_bytes());
            if let Some(reason) = scored.regrade_reason.as_deref() {
                hasher.update(reason.as_bytes());
            }

            let exclusion = exclusion_reason(&image, &scored, request.accepted_only);
            if let Some(reason) = exclusion {
                quality_excluded += 1;
                decisions.push(excluded_decision(&image, &scored, reason));
                continue;
            }

            let Some(filename) = super::handlers::filename_from_metadata(&image.metadata) else {
                missing_files += 1;
                decisions.push(excluded_decision(
                    &image,
                    &scored,
                    "Metadata has no FITS filename".into(),
                ));
                continue;
            };
            let path = match super::handlers::find_fits_file(ctx, &image, &target_name, &filename) {
                Ok(path) => path,
                Err(_) => {
                    missing_files += 1;
                    decisions.push(excluded_decision(
                        &image,
                        &scored,
                        "FITS file was not found".into(),
                    ));
                    continue;
                }
            };
            let source_fingerprint = source_fingerprint(&path);
            hasher.update(source_fingerprint.as_bytes());
            frames.push(PreparedFrame {
                image_id: image.id,
                acquired_date: image.acquired_date,
                quality_score: Some(scored.quality_score),
                path,
                source_fingerprint,
                expected_target: expected_by_image.get(&image.id).copied().flatten(),
                exposure_seconds: exposure_seconds_from_metadata(&image.metadata),
            });
        }

        frames.sort_by(|left, right| {
            right
                .quality_score
                .unwrap_or(0.0)
                .total_cmp(&left.quality_score.unwrap_or(0.0))
                .then_with(|| left.acquired_date.cmp(&right.acquired_date))
                .then_with(|| left.image_id.cmp(&right.image_id))
        });
        let reference_image_id = frames.first().map(|frame| frame.image_id);
        if frames.len() > 1 {
            frames[1..].sort_by_key(|frame| (frame.acquired_date.unwrap_or(0), frame.image_id));
        }
        if !frames.is_empty() {
            let directory_tree = ctx.get_directory_tree().map_err(AppError::db)?;
            let conn = ctx.db();
            let conn = conn.lock().map_err(AppError::db)?;
            for frame in &frames {
                let fingerprint = crate::calibration::selection_fingerprint(
                    &conn,
                    &frame.path,
                    Some(&directory_tree),
                )
                .map_err(AppError::db)?;
                hasher.update(fingerprint.as_bytes());
            }
        }
        let eligible_frames = frames.len();
        public_groups.push(StackGroupStatus {
            index,
            target_id,
            target_name,
            filter_name,
            state: if eligible_frames >= 2 {
                StackGroupState::Queued
            } else {
                StackGroupState::Skipped
            },
            phase: if eligible_frames >= 2 {
                "queued".into()
            } else {
                "skipped".into()
            },
            total_candidates,
            eligible_frames,
            quality_excluded,
            missing_files,
            processed_frames: 0,
            accepted_frames: 0,
            rejected_frames: 0,
            reused_frames: 0,
            resume_note: None,
            output_channels: 0,
            sky_orientation: None,
            reference_image_id,
            total_exposure_seconds: 0.0,
            preview_url: None,
            fits_url: None,
            error: (eligible_frames < 2).then(|| "Fewer than two eligible FITS frames".to_string()),
            calibration: crate::calibration::AppliedCalibration::default(),
            input_images,
            frames: decisions,
        });
        prepared_groups.push(PreparedGroup { index, frames });
    }

    let digest = hasher.finalize();
    let mut job_id = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut job_id, "{byte:02x}").expect("writing to a String cannot fail");
    }
    for group in &mut public_groups {
        if group.state == StackGroupState::Queued {
            group.preview_url = Some(format!(
                "/api/db/{}/stack-previews/{}/{}/preview?v={}",
                ctx.id, job_id, group.index, artifact_revision
            ));
            group.fits_url = Some(format!(
                "/api/db/{}/stack-previews/{}/{}/fits?v={}",
                ctx.id, job_id, group.index, artifact_revision
            ));
        }
    }
    let now = chrono::Utc::now().timestamp();
    Ok(PreparedJob {
        public: StackPreviewJob {
            schema_version: 2,
            job_id,
            database_id: ctx.id.clone(),
            project_id,
            state: StackJobState::Queued,
            accepted_only: request.accepted_only,
            created_unix_seconds: now,
            artifact_revision,
            cache_version: STACK_PREVIEW_CACHE_VERSION,
            stacking_version: SEIZA_STACKING_VERSION.into(),
            groups: public_groups,
            error: None,
        },
        groups: prepared_groups,
        cache_root: ctx.cache_dir_path.clone(),
        north_up: request.north_up,
    })
}

fn exclusion_reason(
    image: &AcquiredImage,
    scored: &ImageQualityResult,
    accepted_only: bool,
) -> Option<String> {
    if image.grading_status == 2 {
        Some("Database grade is Rejected".to_string())
    } else if accepted_only && image.grading_status != 1 {
        Some("Accepted-only policy excludes Pending images".to_string())
    } else {
        scored.regrade_reason.clone()
    }
}

fn quality_results(
    ctx: &DatabaseContext,
    images: &[(AcquiredImage, String, String)],
    expected_by_image: &HashMap<i32, Option<(f64, f64)>>,
) -> Vec<ImageQualityResult> {
    crate::server::spatial_scan::ensure_loaded(&ctx.spatial_metrics, &ctx.cache_dir_path);
    let mut grouped: BTreeMap<(i32, String, String), Vec<&AcquiredImage>> = BTreeMap::new();
    for (image, _, target_name) in images {
        grouped
            .entry((
                image.target_id,
                target_name.clone(),
                image.filter_name.clone(),
            ))
            .or_default()
            .push(image);
    }
    let config = SequenceAnalyzerConfig::default();
    let session_gap = config.session_gap_minutes;
    let analyzer = SequenceAnalyzer::new(config);
    let mut output = Vec::new();
    for ((target_id, target_name, filter_name), group) in grouped {
        let mut metrics = Vec::with_capacity(group.len());
        let mut entries = Vec::with_capacity(group.len());
        for image in group {
            let mut value =
                extract_metrics_from_metadata(image.id, &image.metadata, image.acquired_date);
            super::handlers::merge_spatial_metrics(
                &mut value,
                &ctx.spatial_metrics,
                &image.metadata,
            );
            super::handlers::merge_astrometry_metrics(
                &mut value,
                &ctx.cache_dir_path,
                &image.metadata,
                &ctx.astrometry_evidence,
                expected_by_image.get(&image.id).copied().flatten(),
            );
            entries.push(super::handlers::stored_entry_for(
                &ctx.spatial_metrics,
                image.id,
                &image.metadata,
            ));
            metrics.push(value);
        }
        super::handlers::merge_photometric_signals(&mut metrics, &entries, session_gap);
        let (sequences, rollup) = analyzer.analyze_with_target_filter_rollup(
            &metrics,
            target_id,
            &target_name,
            &filter_name,
        );
        let session_results = sequences
            .into_iter()
            .flat_map(|sequence| sequence.images)
            .collect();
        output.extend(prefer_target_filter_scores(
            session_results,
            rollup.map(|rollup| rollup.sequence.images),
        ));
    }
    output
}

fn prefer_target_filter_scores(
    session_results: Vec<ImageQualityResult>,
    rollup_results: Option<Vec<ImageQualityResult>>,
) -> Vec<ImageQualityResult> {
    let mut rollup_by_id = rollup_results
        .into_iter()
        .flatten()
        .map(|image| (image.image_id, image))
        .collect::<HashMap<_, _>>();
    session_results
        .into_iter()
        .map(|session| rollup_by_id.remove(&session.image_id).unwrap_or(session))
        .collect()
}

fn fallback_quality(image_id: i32) -> ImageQualityResult {
    use crate::sequence_analysis::NormalizedMetrics;

    ImageQualityResult {
        image_id,
        quality_score: 1.0,
        temporal_anomaly_score: 0.0,
        category: None,
        flags: Vec::new(),
        normalized_metrics: NormalizedMetrics {
            star_count: None,
            hfr: None,
            eccentricity: None,
            snr: None,
            background: None,
            spatial_coverage: None,
            transparency: None,
            pointing: None,
        },
        regrade_reason: None,
        spatial_overlay: None,
        pointing: None,
        satellite: None,
        details: None,
    }
}

fn excluded_decision(
    image: &AcquiredImage,
    scored: &ImageQualityResult,
    reason: String,
) -> StackFrameDecision {
    StackFrameDecision {
        image_id: image.id,
        disposition: "excluded".into(),
        reason: Some(reason),
        quality_score: Some(scored.quality_score),
        matched_stars: None,
        registration_rms_pixels: None,
        registration_drift_pixels: None,
        registered_mapping: None,
        normalization_mean_gain: None,
        normalization_mean_offset: None,
        source_fingerprint: None,
        overlap_fraction: None,
        integrated_fraction: None,
    }
}

fn source_fingerprint(path: &FsPath) -> String {
    let mut hasher = Sha256::new();
    hasher.update(path.to_string_lossy().as_bytes());
    if let Ok(metadata) = path.metadata() {
        hasher.update(metadata.len().to_le_bytes());
        if let Ok(modified) = metadata.modified()
            && let Ok(duration) = modified.duration_since(UNIX_EPOCH)
        {
            hasher.update(duration.as_secs().to_le_bytes());
            hasher.update(duration.subsec_nanos().to_le_bytes());
        }
    }
    let mut output = String::with_capacity(64);
    for byte in hasher.finalize() {
        write!(&mut output, "{byte:02x}").expect("writing to a String cannot fail");
    }
    output
}

fn enqueue_job(state: Arc<AppState>, prepared: PreparedJob) {
    let permit = Arc::clone(&state.stack_previews.permit);
    let cancel = state.stack_previews.track_cancel(&prepared.public.job_id);
    tokio::spawn(async move {
        let job_id = prepared.public.job_id.clone();
        let Ok(_permit) = permit.acquire_owned().await else {
            state.stack_previews.forget_cancel(&job_id);
            return;
        };
        // Stack jobs run one at a time, so a job can wait here for minutes.
        // A cancel during that wait means the work never starts.
        if cancel.load(Ordering::Relaxed) {
            state.stack_previews.update(&job_id, |job| {
                cancel_unfinished_groups(job);
                job.state = StackJobState::Cancelled;
            });
            state.stack_previews.forget_cancel(&job_id);
            return;
        }
        let guard = state.begin_interactive_job();
        let state_for_job = Arc::clone(&state);
        let cancel_for_job = Arc::clone(&cancel);
        let result = tokio::task::spawn_blocking(move || {
            let _guard = guard;
            run_job(&state_for_job, prepared, &cancel_for_job)
        })
        .await;
        if let Err(error) = result {
            state.stack_previews.update(&job_id, |job| {
                job.state = StackJobState::Failed;
                job.error = Some(format!("Stack worker panicked: {error}"));
            });
        }
        state.stack_previews.forget_cancel(&job_id);
    });
}

/// Whether every channel reached a state the job cannot improve on. A channel
/// that stopped short is not settled: it is work the stop took away.
fn every_channel_settled(job: &StackPreviewJob) -> bool {
    job.groups.iter().all(|group| {
        matches!(
            group.state,
            StackGroupState::Ready | StackGroupState::Skipped | StackGroupState::Error
        )
    })
}

/// Mark whatever has not finished as cancelled. Channels that already produced
/// an artifact keep their Ready state and their preview.
fn cancel_unfinished_groups(job: &mut StackPreviewJob) {
    for group in &mut job.groups {
        if matches!(
            group.state,
            StackGroupState::Queued | StackGroupState::Running
        ) {
            group.state = StackGroupState::Cancelled;
            group.phase = "cancelled".into();
        }
    }
}

/// Which way up each group's reference frame faces, keyed by group index.
///
/// Every reference frame is read once, before any stacking, so the pier-to-sky
/// mapping learned from a solved channel is available to the channels that were
/// never solved. A frame that cannot be read contributes nothing rather than
/// failing the job; that group falls back to its own exposure.
fn reference_anchors(
    state: &Arc<AppState>,
    database_id: &str,
    groups: &[PreparedGroup],
) -> HashMap<usize, Option<(bool, &'static str)>> {
    let Some(ctx) = state.get_database(database_id) else {
        return HashMap::new();
    };
    let mut orientations = Vec::new();
    for group in groups {
        let Some(reference) = group.frames.first() else {
            continue;
        };
        if group.frames.len() < 2 {
            continue;
        }
        let headers = crate::image_io::read_header(&reference.path).unwrap_or_default();
        let orientation = ReferenceOrientation {
            north_up: cached_or_embedded_wcs(&ctx, reference, &headers)
                .and_then(|(wcs, _)| faces_north_up(wcs.cd)),
            west_of_pier: pier_side_from_headers(&headers).and_then(is_west_of_pier),
        };
        orientations.push((group.index, orientation));
    }
    let calibration = calibrate_pier_side(
        &orientations
            .iter()
            .map(|(_, orientation)| *orientation)
            .collect::<Vec<_>>(),
    );
    orientations
        .into_iter()
        .map(|(index, orientation)| (index, anchored_north_up(orientation, calibration)))
        .collect()
}

fn run_job(state: &Arc<AppState>, prepared: PreparedJob, cancel: &Arc<AtomicBool>) {
    let job_id = prepared.public.job_id.clone();
    let database_id = prepared.public.database_id.clone();
    let accepted_only = prepared.public.accepted_only;
    let PreparedJob {
        public: _,
        groups,
        cache_root,
        north_up,
    } = prepared;
    state.stack_previews.update(&job_id, |job| {
        job.state = StackJobState::Running;
    });
    let worker_policy = state.worker_policy();
    // Read every reference frame's orientation before the first stack is
    // built. A channel that was never solved has to borrow the pier-to-sky
    // mapping from one that was, and it cannot do that from a decision taken
    // after its own. Headers only, so this costs no pixel reads.
    let anchors = reference_anchors(state, &database_id, &groups);
    let group_job = GroupJob {
        database_id: &database_id,
        job_id: &job_id,
        cache_root: &cache_root,
        north_up,
        accepted_only,
        worker_policy: &worker_policy,
        cancel,
    };
    let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        for group in groups {
            if group.frames.len() < 2 {
                continue;
            }
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            state.stack_previews.update(&job_id, |job| {
                job.groups[group.index].state = StackGroupState::Running;
                job.groups[group.index].phase = "calibration".into();
            });
            let anchor = anchors.get(&group.index).copied().flatten();
            let result = run_group(state, &group_job, group.clone(), anchor);
            state.stack_previews.update(&job_id, |job| match result {
                Ok(GroupOutcome::Built) => {
                    job.groups[group.index].state = StackGroupState::Ready;
                    job.groups[group.index].phase = "ready".into();
                }
                Ok(GroupOutcome::Cancelled) => {
                    job.groups[group.index].state = StackGroupState::Cancelled;
                    job.groups[group.index].phase = "cancelled".into();
                }
                Err(error) => {
                    job.groups[group.index].state = StackGroupState::Error;
                    job.groups[group.index].phase = "error".into();
                    job.groups[group.index].error = Some(error);
                }
            });
        }
    }));
    let cancelled = cancel.load(Ordering::Relaxed);
    state.stack_previews.update(&job_id, |job| match run {
        // A stop that lands after the last channel finished took nothing away,
        // so the job completed. Calling it cancelled would also stop the next
        // build from reusing work that is sitting there finished.
        Ok(()) if cancelled && !every_channel_settled(job) => {
            cancel_unfinished_groups(job);
            job.state = StackJobState::Cancelled;
        }
        Ok(()) => job.state = StackJobState::Completed,
        Err(_) => {
            job.state = StackJobState::Failed;
            job.error = Some("Stack worker panicked".into());
        }
    });
    if let Some(job) = state.stack_previews.get(&job_id)
        && let Err(error) = persist_manifest(&cache_root, &job)
    {
        tracing::warn!("Failed to persist stack preview manifest: {error}");
    }
    // A cancelled job still remembers the channels it finished before the
    // stop: those artifacts are complete and are what the user asked for.
    if let Some(job) = state.stack_previews.get(&job_id)
        && matches!(
            job.state,
            StackJobState::Completed | StackJobState::Cancelled
        )
        && let Err(error) = state.stack_previews.persist_latest(&cache_root, &job)
    {
        tracing::warn!("Failed to persist latest stack preview index: {error}");
    }
    state.stack_previews.prune_cache(&cache_root);
}

/// Whether a channel produced an artifact or stopped on request. A failure is
/// the error arm instead: a cancel is not a fault to report.
enum GroupOutcome {
    Built,
    Cancelled,
}

/// Everything a group build needs from the job that owns it: where it writes,
/// how the caller asked for it, and how it is told to stop.
struct GroupJob<'a> {
    database_id: &'a str,
    job_id: &'a str,
    cache_root: &'a FsPath,
    north_up: bool,
    accepted_only: bool,
    worker_policy: &'a crate::concurrency::WorkerPolicy,
    cancel: &'a Arc<AtomicBool>,
}

fn run_group(
    state: &Arc<AppState>,
    job: &GroupJob<'_>,
    group: PreparedGroup,
    anchor: Option<(bool, &'static str)>,
) -> Result<GroupOutcome, String> {
    use seiza_stacking::{FrameDisposition, LiveStacker, NormalizationMode, StackOptions};

    let &GroupJob {
        database_id,
        job_id,
        cache_root,
        north_up,
        accepted_only,
        worker_policy,
        cancel,
    } = job;
    let ctx = state
        .get_database(database_id)
        .ok_or_else(|| format!("Database {database_id} is no longer configured"))?;
    let calibration_conn = rusqlite::Connection::open(&ctx.database_path)
        .map_err(|error| format!("Opening calibration catalog: {error}"))?;
    calibration_conn
        .busy_timeout(std::time::Duration::from_secs(60))
        .map_err(|error| format!("Configuring calibration catalog: {error}"))?;
    let directory_tree = ctx
        .get_directory_tree()
        .map_err(|error| format!("Indexing calibration folders: {error}"))?;
    let light_paths = group
        .frames
        .iter()
        .map(|frame| frame.path.clone())
        .collect::<Vec<_>>();
    // Building a night of masters is minutes of work, so the stop flag reaches
    // between them too.
    let masters = crate::calibration::resolve_or_build_masters_for_group(
        &calibration_conn,
        cache_root,
        &light_paths,
        Some(&directory_tree),
        Some(cancel.as_ref()),
    );
    if cancel.load(Ordering::Relaxed) {
        return Ok(GroupOutcome::Cancelled);
    }
    let (calibration_masters, applied_calibration) = masters.map_err(|error| error.to_string())?;
    let calibration_fingerprint = applied_calibration.fingerprint.clone();
    state.stack_previews.update(job_id, |job| {
        let status = &mut job.groups[group.index];
        status.calibration = applied_calibration;
        status.phase = "stacking".into();
    });
    let (group_target_id, group_filter_name) = state
        .stack_previews
        .get(job_id)
        .map(|job| {
            let status = &job.groups[group.index];
            (status.target_id, status.filter_name.clone())
        })
        .ok_or_else(|| "Stack job disappeared while running".to_string())?;
    let requested = group
        .frames
        .iter()
        .map(|frame| (frame.image_id, frame.source_fingerprint.as_str()))
        .collect::<Vec<_>>();
    let decision = resume::load(
        cache_root,
        database_id,
        group_target_id,
        &group_filter_name,
        accepted_only,
        SEIZA_STACKING_VERSION,
        &calibration_fingerprint,
        &requested,
    );
    if let Some(reason) = decision.fresh_reason() {
        tracing::info!(
            target_id = group_target_id,
            filter = %group_filter_name,
            "Full restack: {reason}"
        );
        state.stack_previews.update(job_id, |job| {
            job.groups[group.index].resume_note = Some(format!("Full restack: {reason}"));
        });
    }
    let checkpoint = decision.state();
    let reference_frame = crate::image_io::open_linear_frame(&group.frames[0].path)
        .map_err(|error| error.to_string())?;
    let output_channels = if reference_frame.bayer.is_some() {
        3_u64
    } else {
        reference_frame.image.channels as u64
    };
    let pixels = reference_frame.image.pixel_count();
    let estimate = (pixels as u64)
        .saturating_mul(output_channels)
        .saturating_mul(STACK_BYTES_PER_OUTPUT_SAMPLE);
    if let Some(available) = crate::concurrency::available_memory_bytes()
        && estimate > (available as f64 * worker_policy.memory_budget_fraction) as u64
    {
        return Err(format!(
            "Estimated stack memory {} MiB exceeds the configured available-memory budget",
            estimate / (1024 * 1024)
        ));
    }
    let budget = crate::concurrency::plan_workers(
        None,
        worker_policy,
        crate::concurrency::Priority::Interactive,
        Some(pixels),
    );
    let pool = ThreadPoolBuilder::new()
        .num_threads(budget.workers)
        .thread_name(|index| format!("stack-preview-{index}"))
        .build()
        .map_err(|error| error.to_string())?;
    tracing::info!(
        "Stack preview {} group {}: {} worker(s) — {}",
        job_id,
        group.index,
        budget.workers,
        budget.rationale
    );

    pool.install(|| {
        // A checkpoint that fails to reopen — a Seiza version change, a
        // truncated file — is discarded and the group builds from scratch.
        let restored = checkpoint.and_then(|checkpoint| {
            match LiveStacker::open_context(&checkpoint.context_path) {
                Ok(stacker) => Some((stacker, checkpoint.manifest)),
                Err(error) => {
                    tracing::warn!(
                        "Stack checkpoint could not be reopened ({error}); rebuilding from scratch"
                    );
                    resume::discard(cache_root, database_id, group_target_id, &group_filter_name);
                    state.stack_previews.update(job_id, |job| {
                        job.groups[group.index].resume_note =
                            Some("Full restack: the checkpoint could not be reopened".into());
                    });
                    None
                }
            }
        });
        let mut orientation_vote = OrientationVote::default();
        let (mut stacker, mut ledger) = match restored {
            Some((stacker, manifest)) => {
                // Replay the checkpointed ledger: the per-frame record, the
                // counters, and each accepted frame's orientation vote. The
                // reference is the ledger's first entry and votes upright.
                let ledger = manifest.frames;
                for frame in &ledger {
                    if let Some(rotation) = frame.rotation_radians {
                        orientation_vote.add(rotation, frame.exposure_seconds);
                    }
                }
                tracing::info!(
                    "Stack preview {} group {}: resuming from a checkpoint of {} frame(s)",
                    job_id,
                    group.index,
                    ledger.len()
                );
                state.stack_previews.update(job_id, |job| {
                    let status = &mut job.groups[group.index];
                    status.processed_frames = ledger.len();
                    status.accepted_frames = ledger
                        .iter()
                        .filter(|frame| {
                            matches!(
                                frame.decision.disposition.as_str(),
                                "reference" | "accepted"
                            )
                        })
                        .count();
                    status.rejected_frames = ledger.len() - status.accepted_frames;
                    status.reused_frames = ledger.len();
                    status.output_channels = output_channels as usize;
                    status.total_exposure_seconds = ledger
                        .iter()
                        .filter(|frame| frame.rotation_radians.is_some())
                        .map(|frame| frame.exposure_seconds)
                        .sum();
                    status.frames = ledger.iter().map(|frame| frame.decision.clone()).collect();
                });
                (stacker, ledger)
            }
            None => {
                // From the record, like every other frame's. Reading this one
                // from its opened header instead would weigh it in seconds
                // against frames weighing 1.0 apiece whenever the catalog does
                // not record an exposure, and the reference alone would then
                // outvote the whole night.
                let reference_exposure = group.frames[0].exposure_seconds;
                // The reference frame defines zero rotation; it votes upright.
                orientation_vote.add(0.0, reference_exposure);
                let options = StackOptions {
                    normalization: NormalizationMode::Global,
                    ..StackOptions::default()
                };
                let stacker = LiveStacker::new(reference_frame, calibration_masters, options)
                    .map_err(|error| error.to_string())?;
                let reference_mapping = stacker.reference_mapping();
                let reference_decision = StackFrameDecision {
                    image_id: group.frames[0].image_id,
                    disposition: "reference".into(),
                    reason: None,
                    quality_score: group.frames[0].quality_score,
                    matched_stars: None,
                    registration_rms_pixels: None,
                    registration_drift_pixels: None,
                    registered_mapping: Some(reference_mapping),
                    normalization_mean_gain: Some(1.0),
                    normalization_mean_offset: Some(0.0),
                    source_fingerprint: Some(group.frames[0].source_fingerprint.clone()),
                    overlap_fraction: Some(1.0),
                    integrated_fraction: Some(1.0),
                };
                state.stack_previews.update(job_id, |job| {
                    let status = &mut job.groups[group.index];
                    status.processed_frames = 1;
                    status.accepted_frames = 1;
                    status.output_channels = output_channels as usize;
                    status.total_exposure_seconds = reference_exposure;
                    status.frames.push(reference_decision.clone());
                });
                let ledger = vec![resume::ResumeFrame {
                    decision: reference_decision,
                    exposure_seconds: reference_exposure,
                    rotation_radians: Some(0.0),
                }];
                (stacker, ledger)
            }
        };
        let already_integrated: std::collections::HashSet<i32> =
            ledger.iter().map(|frame| frame.decision.image_id).collect();
        let save_checkpoint = |stacker: &LiveStacker, ledger: &[resume::ResumeFrame]| {
            let context_path =
                resume::context_path(cache_root, database_id, group_target_id, &group_filter_name);
            let manifest = resume::ResumeManifest {
                schema_version: resume::RESUME_SCHEMA_VERSION,
                stacking_version: SEIZA_STACKING_VERSION.into(),
                target_id: group_target_id,
                filter_name: group_filter_name.clone(),
                accepted_only,
                calibration_fingerprint: calibration_fingerprint.clone(),
                frames: ledger.to_vec(),
            };
            let saved = context_path
                .parent()
                .ok_or_else(|| "checkpoint path has no parent".to_string())
                .and_then(|parent| {
                    std::fs::create_dir_all(parent).map_err(|error| error.to_string())
                })
                .and_then(|()| {
                    stacker
                        .save_context(&context_path)
                        .map_err(|error| error.to_string())
                })
                .and_then(|()| {
                    resume::store_manifest(
                        &resume::manifest_path(
                            cache_root,
                            database_id,
                            group_target_id,
                            &group_filter_name,
                        ),
                        &manifest,
                    )
                });
            if let Err(error) = saved {
                // A checkpoint is an optimization; a build never fails over
                // it. Discard the pair so nothing resumes from half a save.
                tracing::warn!("Failed to save stack checkpoint: {error}");
                resume::discard(cache_root, database_id, group_target_id, &group_filter_name);
            }
        };

        let pending: Vec<&PreparedFrame> = group
            .frames
            .iter()
            .filter(|frame| !already_integrated.contains(&frame.image_id))
            .collect();
        let paths: Vec<PathBuf> = pending.iter().map(|frame| frame.path.clone()).collect();
        // Reads, calibration, registration and normalization overlap across
        // frames while integration stays in this order, so the accumulator
        // sees exactly the sequence a frame-at-a-time loop would. A frame
        // declaring itself normalized is put on the same 16-bit scale the
        // rest of the catalog uses as it is read.
        let pipeline = seiza_stacking::PipelineOptions {
            normalized_full_scale: Some(crate::image_io::NORMALIZED_FULL_SCALE),
            ..seiza_stacking::PipelineOptions::default()
        };
        let mut cancelled = false;
        let mut consumed = 0usize;
        // Every frame's outcome is recorded in the callback above, so the
        // summary adds nothing here.
        let _report = stacker
            .push_fits_pipelined(&paths, &pipeline, |_, outcome| {
                let frame = pending[consumed];
                consumed += 1;
                let exposure = frame.exposure_seconds;
                let decision = match outcome {
                    Ok(FrameDisposition::Accepted(diagnostics)) => {
                        orientation_vote
                            .add(diagnostics.mapping.transform().rotation_radians, exposure);
                        StackFrameDecision {
                            image_id: frame.image_id,
                            disposition: "accepted".into(),
                            reason: None,
                            quality_score: frame.quality_score,
                            matched_stars: Some(diagnostics.matched_stars),
                            registration_rms_pixels: Some(diagnostics.registration_rms_pixels),
                            registration_drift_pixels: Some(diagnostics.registration_drift_pixels),
                            registered_mapping: Some(*diagnostics.mapping),
                            normalization_mean_gain: Some(diagnostics.normalization_mean_gain),
                            normalization_mean_offset: Some(diagnostics.normalization_mean_offset),
                            source_fingerprint: Some(frame.source_fingerprint.clone()),
                            overlap_fraction: Some(diagnostics.overlap_fraction),
                            integrated_fraction: Some(diagnostics.integrated_fraction),
                        }
                    }
                    // A frame the stack turned away and one that could not be
                    // read are both "not integrated" to a caller reading the
                    // group's decisions; only the reason differs.
                    Ok(FrameDisposition::Rejected(reason)) => {
                        rejected_decision(frame, reason.to_string())
                    }
                    Err(error) => rejected_decision(frame, error.to_string()),
                };
                ledger.push(resume::ResumeFrame {
                    decision: decision.clone(),
                    exposure_seconds: exposure,
                    rotation_radians: if decision.disposition == "accepted" {
                        decision
                            .registered_mapping
                            .as_ref()
                            .map(|mapping| mapping.transform().rotation_radians)
                    } else {
                        None
                    },
                });
                state.stack_previews.update(job_id, |job| {
                    let status = &mut job.groups[group.index];
                    status.processed_frames += 1;
                    if matches!(decision.disposition.as_str(), "accepted") {
                        status.accepted_frames += 1;
                        status.total_exposure_seconds += exposure;
                    } else {
                        status.rejected_frames += 1;
                    }
                    status.frames.push(decision);
                });

                // Integrating one frame is the unit of work, so this is where
                // a stop takes effect. Frames already prepared are discarded.
                if cancel.load(Ordering::Relaxed) {
                    cancelled = true;
                    seiza_stacking::Continue::No
                } else {
                    seiza_stacking::Continue::Yes
                }
            })
            .map_err(|error| error.to_string())?;

        if cancelled {
            // The frames that did land are checkpointed, so building again
            // continues from them.
            save_checkpoint(&stacker, &ledger);
            return Ok(GroupOutcome::Cancelled);
        }
        // The accumulator is complete: checkpoint it before the snapshot
        // consumes the stacker, so an additive rebuild can pick it up here.
        save_checkpoint(&stacker, &ledger);
        // Last exit before the job writes anything. Orienting and rendering
        // follow, and a stop after this point would have to clean up published
        // artifacts.
        if cancel.load(Ordering::Relaxed) {
            return Ok(GroupOutcome::Cancelled);
        }
        // Only a north-up build has a solve and a reprojection worth naming;
        // keeping the reference frame's rotation goes straight to rendering.
        if north_up {
            state.stack_previews.update(job_id, |job| {
                job.groups[group.index].phase = "orienting".into();
            });
        }
        let reference_headers = stacker.reference_headers().to_vec();
        let snapshot = stacker.into_snapshot().map_err(|error| error.to_string())?;
        let accepted_frames = snapshot.accepted_frames;
        let rejected_frames = snapshot.rejected_frames;
        // Registration already absorbs a meridian flip, so a stack is published
        // in the reference frame's own rotation unless the caller asks for the
        // shared north-up grid that a mosaic needs. The one correction it still
        // makes is a half turn when the reference sits on the thinner side of a
        // flip, so the result faces the way most of the night was shot.
        let (image, sky_orientation, mut output_cards) = if north_up {
            let (source_wcs, orientation_source) =
                resolve_stack_wcs(state, &ctx, &group.frames[0], &reference_headers)?;
            let orientation = seiza_stacking::SkyOrientationPlan::new(
                snapshot.image.width,
                snapshot.image.height,
                &source_wcs,
            )
            .map_err(|error| error.to_string())?;
            let oriented = orientation
                .apply(&snapshot.image)
                .map_err(|error| error.to_string())?;
            let record = StackSkyOrientation {
                convention: seiza_stacking::SKY_ORIENTATION_NAME.into(),
                version: seiza_stacking::SKY_ORIENTATION_VERSION,
                source: orientation_source,
                output_width: orientation.output_width(),
                output_height: orientation.output_height(),
                source_to_output: orientation.source_to_output(),
            };
            (oriented, record, orientation.fits_header_cards())
        } else {
            let (turn, decided_by) = half_turn_decision(anchor, &orientation_vote);
            tracing::info!(
                "Stack preview {} group {}: publishing {}, decided by {decided_by}",
                job_id,
                group.index,
                if turn {
                    "half a turn out of the reference frame"
                } else {
                    "in the reference frame's rotation"
                }
            );
            let (width, height) = (snapshot.image.width, snapshot.image.height);
            if turn {
                (
                    half_turn(snapshot.image),
                    StackSkyOrientation::source_frame_half_turn(width, height, decided_by),
                    Vec::new(),
                )
            } else {
                (
                    snapshot.image,
                    StackSkyOrientation::source_frame(width, height, decided_by),
                    Vec::new(),
                )
            }
        };
        state.stack_previews.update(job_id, |job| {
            let status = &mut job.groups[group.index];
            status.phase = "rendering".into();
            status.sky_orientation = Some(sky_orientation);
        });
        let fits_destination = fits_path(cache_root, job_id, group.index);
        let fits_parent = fits_destination
            .parent()
            .ok_or_else(|| "Stack FITS path has no parent".to_string())?;
        std::fs::create_dir_all(fits_parent).map_err(|error| error.to_string())?;
        let fits_temporary =
            fits_destination.with_extension(format!("{}.tmp.fits", std::process::id()));
        output_cards.push(
            seiza_fits::WriteHeaderCard::new(
                "STACKCNT",
                seiza_fits::HeaderValue::Integer(i64::from(accepted_frames)),
            )
            .with_comment("accepted input frames"),
        );
        output_cards.push(
            seiza_fits::WriteHeaderCard::new(
                "STACKREJ",
                seiza_fits::HeaderValue::Integer(i64::from(rejected_frames)),
            )
            .with_comment("rejected input frames"),
        );
        seiza_stacking::write_linear_image_fits_f32(
            &fits_temporary,
            &image,
            &reference_headers,
            &output_cards,
        )
        .map_err(|error| error.to_string())?;
        std::fs::rename(&fits_temporary, &fits_destination).map_err(|error| error.to_string())?;
        stretch::render_image_previews_atomic(
            &image,
            &stretch::default_linear_config(),
            stretch::StackStretchSourceTransfer::Linear,
            &preview_path(cache_root, job_id, group.index),
            &original_preview_path(cache_root, job_id, group.index),
        )
        .map(|_| GroupOutcome::Built)
    })
}

/// The reference frame's WCS from sources already at hand: a cached
/// pixel-derived solve first, then a valid embedded FITS WCS. Never solves, so
/// a caller that only wants to know which way up the frame is does not pay for
/// a plate solve.
fn cached_or_embedded_wcs(
    ctx: &DatabaseContext,
    reference: &PreparedFrame,
    headers: &[(String, seiza_fits::HeaderValue)],
) -> Option<(seiza::Wcs, String)> {
    if let Some(solution) = ctx
        .astrometry_evidence
        .evidence_for_source(
            &ctx.cache_dir_path,
            reference.image_id,
            reference.expected_target,
        )
        .and_then(|analysis| analysis.solution)
    {
        return Some((
            crate::astrometry::wcs_from_response(&solution.wcs),
            "cached_pixel_solve".into(),
        ));
    }
    let embedded =
        crate::astrometry_headers::FitsAstrometryHeaders::from_headers(headers).embedded_wcs?;
    let value = embedded.value;
    Some((
        seiza::Wcs {
            crval: (value.crval[0], value.crval[1]),
            crpix: (value.crpix[0], value.crpix[1]),
            cd: value.cd,
            sip: None,
        },
        "embedded_wcs".into(),
    ))
}

fn resolve_stack_wcs(
    state: &AppState,
    ctx: &DatabaseContext,
    reference: &PreparedFrame,
    headers: &[(String, seiza_fits::HeaderValue)],
) -> Result<(seiza::Wcs, String), String> {
    if let Some(resolved) = cached_or_embedded_wcs(ctx, reference, headers) {
        return Ok(resolved);
    }
    let analysis = state
        .astrometry
        .solve_image(
            reference.image_id,
            &reference.path,
            reference.expected_target,
        )
        .map_err(|error| format!("Plate solving the stack reference failed: {error}"))?;
    let solution = analysis.solution.ok_or_else(|| {
        let detail = analysis
            .error
            .unwrap_or_else(|| "the solver found no match".into());
        format!(
            "Stack preview needs a plate solution for north-up display, but {detail}. Install the Seiza solver catalogs or solve a source image first."
        )
    })?;
    Ok((
        crate::astrometry::wcs_from_response(&solution.wcs),
        "stack_reference_solve".into(),
    ))
}

fn save_png_atomic(image: &image::DynamicImage, destination: &FsPath) -> Result<(), String> {
    let temporary = destination.with_extension(format!("{}.tmp.png", std::process::id()));
    image.save(&temporary).map_err(|error| error.to_string())?;
    std::fs::rename(&temporary, destination).map_err(|error| error.to_string())
}

fn persist_manifest(cache_root: &FsPath, job: &StackPreviewJob) -> Result<(), String> {
    let path = manifest_path(cache_root, &job.job_id);
    let parent = path
        .parent()
        .ok_or_else(|| "Stack manifest path has no parent".to_string())?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    let bytes = serde_json::to_vec_pretty(job).map_err(|error| error.to_string())?;
    std::fs::write(&temporary, bytes).map_err(|error| error.to_string())?;
    std::fs::rename(&temporary, path).map_err(|error| error.to_string())
}

fn persist_latest_groups(cache_root: &FsPath, job: &StackPreviewJob) -> Result<(), String> {
    let ready = job
        .groups
        .iter()
        .filter(|group| group.state == StackGroupState::Ready)
        .cloned()
        .collect::<Vec<_>>();
    if ready.is_empty() {
        return Ok(());
    }

    let path = latest_path(cache_root, job.project_id);
    let mut latest = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<LatestStackPreviews>(&bytes).ok())
        .filter(|value| value.database_id == job.database_id && value.project_id == job.project_id)
        .map(current_latest_stacks)
        .unwrap_or_else(|| LatestStackPreviews {
            schema_version: 2,
            database_id: job.database_id.clone(),
            project_id: job.project_id,
            updated_unix_seconds: 0,
            groups: Vec::new(),
        });

    for group in ready {
        let replacement = LatestStackPreviewGroup {
            job_id: job.job_id.clone(),
            artifact_revision: job.artifact_revision.clone(),
            accepted_only: job.accepted_only,
            created_unix_seconds: job.created_unix_seconds,
            cache_version: job.cache_version,
            group,
        };
        if let Some(existing) = latest.groups.iter_mut().find(|existing| {
            existing.group.target_id == replacement.group.target_id
                && existing.group.filter_name == replacement.group.filter_name
        }) {
            *existing = replacement;
        } else {
            latest.groups.push(replacement);
        }
    }
    latest.groups.sort_by(|left, right| {
        left.group
            .target_name
            .cmp(&right.group.target_name)
            .then_with(|| left.group.filter_name.cmp(&right.group.filter_name))
            .then_with(|| left.group.target_id.cmp(&right.group.target_id))
    });
    latest.updated_unix_seconds = chrono::Utc::now().timestamp();

    let parent = path
        .parent()
        .ok_or_else(|| "Latest stack preview path has no parent".to_string())?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    let bytes = serde_json::to_vec_pretty(&latest).map_err(|error| error.to_string())?;
    std::fs::write(&temporary, bytes).map_err(|error| error.to_string())?;
    std::fs::rename(&temporary, path).map_err(|error| error.to_string())
}

pub(super) fn current_latest_stacks(mut latest: LatestStackPreviews) -> LatestStackPreviews {
    latest.groups.retain(|entry| {
        entry.cache_version == STACK_PREVIEW_CACHE_VERSION
            && entry
                .group
                .sky_orientation
                .as_ref()
                .is_some_and(StackSkyOrientation::is_current)
    });
    latest
}

fn stack_dir(cache_root: &FsPath, job_id: &str) -> PathBuf {
    cache_root.join("stack-previews").join(job_id)
}

fn manifest_path(cache_root: &FsPath, job_id: &str) -> PathBuf {
    stack_dir(cache_root, job_id).join("manifest.json")
}

/// Parse every `latest-project-*.json` directly inside a directory.
pub(super) fn read_latest_indices<T: serde::de::DeserializeOwned>(directory: &FsPath) -> Vec<T> {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with("latest-project-") && name.ends_with(".json"))
        })
        .filter_map(|entry| std::fs::read(entry.path()).ok())
        .filter_map(|bytes| serde_json::from_slice(&bytes).ok())
        .collect()
}

fn latest_path(cache_root: &FsPath, project_id: i32) -> PathBuf {
    cache_root
        .join("stack-previews")
        .join(format!("latest-project-{project_id}.json"))
}

fn preview_path(cache_root: &FsPath, job_id: &str, group_index: usize) -> PathBuf {
    stack_dir(cache_root, job_id).join(format!("group-{group_index}.png"))
}

fn original_preview_path(cache_root: &FsPath, job_id: &str, group_index: usize) -> PathBuf {
    stack_dir(cache_root, job_id).join(format!("group-{group_index}-original.png"))
}

fn fits_path(cache_root: &FsPath, job_id: &str, group_index: usize) -> PathBuf {
    stack_dir(cache_root, job_id).join(format!("group-{group_index}.fits"))
}

#[cfg(test)]
mod tests {

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

    fn sky_orientation() -> StackSkyOrientation {
        StackSkyOrientation::source_frame(100, 80, orientation_source::SKY_ANCHOR)
    }

    fn ready_group(target_id: i32, filter_name: &str, image_id: i32) -> StackGroupStatus {
        StackGroupStatus {
            index: 0,
            target_id,
            target_name: format!("Target {target_id}"),
            filter_name: filter_name.into(),
            state: StackGroupState::Ready,
            phase: "ready".into(),
            total_candidates: 2,
            eligible_frames: 2,
            quality_excluded: 0,
            missing_files: 0,
            processed_frames: 2,
            accepted_frames: 2,
            rejected_frames: 0,
            reused_frames: 0,
            resume_note: None,
            output_channels: 1,
            sky_orientation: Some(sky_orientation()),
            reference_image_id: Some(image_id),
            total_exposure_seconds: 120.0,
            preview_url: None,
            fits_url: None,
            error: None,
            calibration: crate::calibration::AppliedCalibration::default(),
            input_images: vec![StackInputImage {
                image_id,
                grading_status: 1,
            }],
            frames: Vec::new(),
        }
    }

    fn completed_job(job_id: &str, groups: Vec<StackGroupStatus>) -> StackPreviewJob {
        StackPreviewJob {
            schema_version: 2,
            job_id: job_id.into(),
            database_id: "db-test".into(),
            project_id: 7,
            state: StackJobState::Completed,
            accepted_only: false,
            created_unix_seconds: 100,
            artifact_revision: format!("revision-{job_id}"),
            cache_version: STACK_PREVIEW_CACHE_VERSION,
            stacking_version: SEIZA_STACKING_VERSION.into(),
            groups,
            error: None,
        }
    }

    #[test]
    fn cancel_reaches_a_tracked_job_and_nothing_else() {
        let manager = StackPreviewManager::new();
        assert!(!manager.request_cancel("never-queued"));

        let flag = manager.track_cancel("queued");
        assert!(!flag.load(Ordering::Relaxed));
        assert!(manager.request_cancel("queued"));
        assert!(flag.load(Ordering::Relaxed));

        // A job that has left the queue cannot be cancelled again, so the
        // handler can answer "too late" instead of pretending it stopped.
        manager.forget_cancel("queued");
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

        let latest: LatestStackPreviews = serde_json::from_slice(
            &std::fs::read(latest_path(cache.path(), job.project_id)).unwrap(),
        )
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
        assert_eq!(entry.processed_units, 1);
        assert_eq!(entry.total_units, 4);
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
        assert_eq!(active[0].processed_units, 2);
        assert_eq!(active[0].total_units, 4);
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
        })
        .is_err());
        assert!(validate_request(&StackPreviewRequest {
            image_ids: vec![1, 1],
            accepted_only: false,
            force: false,
            north_up: false,
        })
        .is_err());
        assert!(validate_request(&StackPreviewRequest {
            image_ids: vec![1, 2],
            accepted_only: false,
            force: false,
            north_up: false,
        })
        .is_ok());
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
    fn latest_index_replaces_only_the_rebuilt_channel() {
        let cache = tempfile::tempdir().unwrap();
        let first = completed_job(
            "first",
            vec![ready_group(10, "B", 1), ready_group(10, "R", 2)],
        );
        persist_latest_groups(cache.path(), &first).unwrap();

        let mut rebuilt_blue = ready_group(10, "B", 3);
        rebuilt_blue.index = 4;
        let second = completed_job("second", vec![rebuilt_blue]);
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
        assert_eq!(red.job_id, "first");
        assert_eq!(red.group.reference_image_id, Some(2));
    }

    #[test]
    fn latest_index_hides_pre_orientation_artifacts() {
        let current = LatestStackPreviewGroup {
            job_id: "current".into(),
            artifact_revision: "current-revision".into(),
            accepted_only: false,
            created_unix_seconds: 100,
            cache_version: STACK_PREVIEW_CACHE_VERSION,
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
    fn latest_index_keeps_both_orientation_conventions() {
        let source_frame = LatestStackPreviewGroup {
            job_id: "source-frame".into(),
            artifact_revision: "source-revision".into(),
            accepted_only: false,
            created_unix_seconds: 100,
            cache_version: STACK_PREVIEW_CACHE_VERSION,
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
        let orientation =
            StackSkyOrientation::source_frame(120, 90, orientation_source::SKY_ANCHOR);
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
        let orientation = StackSkyOrientation::source_frame_half_turn(
            3,
            2,
            orientation_source::EXPOSURE_MAJORITY,
        );
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
}
