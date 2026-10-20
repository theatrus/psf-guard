//! Project-scoped, per-target/per-filter stacking previews.
//!
//! PSF Guard owns frame selection and provenance. `seiza-stacking` owns
//! FITS decoding, calibration, registration, normalization, admission, and
//! ordered accumulation. Jobs are process-global and run one at a time so a
//! multi-database server cannot multiply the stacker's full-frame buffers.

pub mod artifact;
pub mod automatic;
mod build_set;
pub mod calibration_masters;
pub mod color;
pub mod color_calibration;
mod execution;
mod final_integration;
mod janitor;
pub mod journal;
pub mod method;
mod processing_queue;
pub mod rc_astro;
pub(crate) mod reapply;
mod reference;
mod resume;
pub mod snr;
pub mod stretch;
pub mod wbpp_stacks;

use crate::concurrency::Stop;
#[cfg(test)]
use build_set::{
    exclusion_reason, fallback_quality, prefer_target_filter_scores, prepare_job, start_prepared,
    Placement, Started,
};
use build_set::{prepare_channels, validate_request};
pub use build_set::{start_stack_preview_channels, start_stack_previews};

pub(super) use crate::server::job_line::Entry;
use crate::server::job_line::{DispatchRules, Lease, Tier, Turn};
pub(super) use crate::server::job_line::{Lane, LineWork, Needs, Submission, Task, Worker};
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
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path as FsPath, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio_util::io::ReaderStream;

use crate::acquisition_context::FramingResolver;
use crate::db::Database;
use crate::models::AcquiredImage;
use crate::sequence_analysis::{
    extract_light_metrics, ImageQualityResult, SequenceAnalyzer, SequenceAnalyzerConfig,
};
use crate::server::api::ApiResponse;
use crate::server::database_context::DatabaseContext;
use crate::server::exposure_groups::{ExposureGroup, ProjectExposureGroups};
use crate::server::extract::DbContext;
use crate::server::handlers::AppError;
use crate::server::state::AppState;
pub use method::{StackFinalPass, StackMethod, StackReference, StackWeighting};

/// The Seiza release whose stacking output this build reproduces. Stack
/// ids, checkpoints, reference scores and calibration masters key on it
/// rather than on the crate version, so a Seiza upgrade that changes no
/// output keeps every stack and master. Seiza moves it when its stacking
/// output changes, and holds it there with a test.
pub const SEIZA_STACKING_OUTPUT_VERSION: &str = seiza_stacking::OUTPUT_VERSION;

/// The stretch release every stack id was built with when the card preview
/// still entered the id. Ids keep it, so a Seiza release that changes only
/// how previews render builds no stack again: Build renders the previews
/// again from each stack's FITS instead (see [`preview_needs_render`]).
const JOB_ID_STRETCH_VERSION: &str = "0.3.0";

/// Bump whenever stack admission, rendering, or persisted artifact semantics
/// change. This deliberately versions PSF Guard policy separately from Seiza.
pub(super) const STACK_PREVIEW_CACHE_VERSION: u32 = 15;

/// Which calibration sessions get the spatial impulse filter.
///
/// A dark master subtracts hot pixels with real measurements; a session
/// without one has only the filter. Calibration switched off means the
/// user asked for raw frames, so nothing is filtered.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct SessionCosmetics {
    per_session: Vec<Option<seiza_stacking::ImpulseFilterOptions>>,
    /// The filter for a frame whose session masters were refused and that
    /// stacked raw: nothing subtracted its hot pixels either.
    bypassed: Option<seiza_stacking::ImpulseFilterOptions>,
}

impl SessionCosmetics {
    pub(super) fn for_plan(
        plan: &crate::calibration::CalibrationPlan,
        mode: crate::calibration::CalibrationMode,
    ) -> Self {
        Self {
            per_session: plan
                .sessions
                .iter()
                .map(|session| impulse_filter_for(mode, session.applied.dark_master.is_some()))
                .collect(),
            bypassed: impulse_filter_for(mode, false),
        }
    }

    #[cfg(test)]
    pub(super) fn none(sessions: usize) -> Self {
        Self {
            per_session: vec![None; sessions],
            bypassed: None,
        }
    }

    /// The filter for one frame: its session's, or the raw-frame filter
    /// when its masters were bypassed.
    pub(super) fn for_frame(
        &self,
        session: usize,
        calibration_bypassed: bool,
    ) -> Option<seiza_stacking::ImpulseFilterOptions> {
        if calibration_bypassed {
            return self.bypassed;
        }
        self.per_session.get(session).copied().flatten()
    }

    /// The one filter the live stacker can hold for the whole group: set
    /// only when every session lacks a dark, as before sessions could differ.
    pub(super) fn whole_group(&self) -> Option<seiza_stacking::ImpulseFilterOptions> {
        if !self.per_session.is_empty() && self.per_session.iter().all(Option::is_some) {
            self.per_session[0]
        } else {
            None
        }
    }

    /// What the calibration card says about hot pixels, if anything.
    pub(super) fn card_note(&self) -> Option<String> {
        let filtered = self.per_session.iter().filter(|c| c.is_some()).count();
        let sessions = self.per_session.len();
        match filtered {
            0 => None,
            n if n == sessions => {
                Some("Hot pixels suppressed in each light (no dark master to subtract them)".into())
            }
            n => Some(format!(
                "Hot pixels suppressed in {n} of {sessions} calibration sessions, the ones \
                 with no dark master to subtract them"
            )),
        }
    }
}

fn impulse_filter_for(
    mode: crate::calibration::CalibrationMode,
    has_dark_master: bool,
) -> Option<seiza_stacking::ImpulseFilterOptions> {
    (!has_dark_master && mode != crate::calibration::CalibrationMode::Off)
        .then(seiza_stacking::ImpulseFilterOptions::default)
}
const MAX_REQUEST_IMAGES: usize = 10_000;
const MAX_REMEMBERED_JOBS: usize = 64;
/// Queued builds are small until they run, so a project queued one channel
/// at a time may go past [`MAX_REMEMBERED_JOBS`] up to this many.
const MAX_QUEUED_JOBS: usize = 1024;
const PREVIEW_MAX_DIMENSION: u32 = 2400;
const STACK_BYTES_PER_OUTPUT_SAMPLE: u64 = 96;

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    /// How this build calibrates its lights: `auto` (default) applies the
    /// safe masters, `on` forces every buildable master including the ones
    /// auto refuses, and `off` stacks the raw frames.
    #[serde(default)]
    pub calibration: crate::calibration::CalibrationMode,
    /// Per-channel exceptions to `calibration`. A channel is one target and
    /// filter group; a channel without an entry uses `calibration`.
    #[serde(default)]
    pub calibration_overrides: Vec<CalibrationOverride>,
    /// The order frames are integrated in, which decides what the progressive
    /// signal-to-noise curve answers. `capture` (default) is chronological and
    /// asks whether another night would help; `quality` puts the best-graded
    /// frames first and asks which of these frames are worth keeping.
    #[serde(default)]
    pub order: snr::StackFrameOrder,
    /// The caller's scoring overrides (penalty scales, absolute reject
    /// limits). Frame exclusion keys off the same reject recommendations
    /// every other scoring surface shows, so the preview must score with
    /// the same settings — otherwise a satellite penalty of 0 would stop
    /// rejections everywhere while the stack still silently dropped the
    /// trailed frames. The job records and hashes the normalized settings so
    /// callers can tell whether a remembered result matches their policy.
    #[serde(default)]
    pub scoring: crate::server::api::ScoringOverrideQuery,
    /// How the frames integrate. Omitted uses the server's stacking
    /// method, set on the Stacking settings page.
    #[serde(default)]
    pub method: Option<StackMethod>,
    /// The display pipeline a target's first color preview gets once this
    /// build leaves none of its channels waiting. Omitted, a build only
    /// rebuilds the color previews already composed for the target.
    #[serde(default)]
    pub color_defaults: Option<color::StackColorProcessing>,
    /// Build only this channel of the request's images. The rest of the
    /// images still teach it the pier-side mapping, so a channel built alone
    /// is the same build as the one Build stacks queues for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel: Option<StackChannelKey>,
}

/// One channel of a request: a target, a filter, and an exposure group when
/// the project splits them.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct StackChannelKey {
    pub target_id: i32,
    pub filter_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exposure_group_key: Option<String>,
}

impl StackChannelKey {
    fn matches(&self, target_id: i32, filter_name: &str, exposure_group_key: Option<&str>) -> bool {
        self.target_id == target_id
            && self.filter_name == filter_name
            && self.exposure_group_key.as_deref() == exposure_group_key
    }
}

/// One channel's calibration mode, overriding the request-wide choice.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalibrationOverride {
    pub target_id: i32,
    #[serde(default)]
    pub filter_name: String,
    #[serde(default)]
    pub exposure_group_key: Option<String>,
    pub calibration: crate::calibration::CalibrationMode,
}

impl StackPreviewRequest {
    /// The mode one channel stacks under: its override, or the request-wide
    /// mode.
    fn calibration_for(
        &self,
        target_id: i32,
        filter_name: &str,
        exposure_group_key: Option<&str>,
    ) -> crate::calibration::CalibrationMode {
        self.calibration_overrides
            .iter()
            .find(|entry| {
                entry.target_id == target_id
                    && entry.filter_name == filter_name
                    && entry.exposure_group_key.as_deref() == exposure_group_key
            })
            .map(|entry| entry.calibration)
            .unwrap_or(self.calibration)
    }
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
    /// Asked for and in the line, but not yet read: its frames, scores and
    /// calibration are being worked out off the request that queued it.
    Preparing,
    /// Prepared into the builds `replaced_by` names, which a client follows.
    Replaced,
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
    /// Per-channel noise Seiza measured on this frame after normalization.
    /// Present only for an admitted frame of a noise-weighted stack.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub noise_sigma: Option<Vec<f32>>,
    /// Per-channel weight this frame carried in the stack mean. Present only
    /// for an admitted frame of a noise-weighted stack; the reference weighs
    /// 1. The final rejection pass replays with exactly these weights.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub integration_weight: Option<Vec<f32>>,
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
    pub(crate) fn is_current(&self) -> bool {
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
    #[serde(default)]
    pub exposure_group: Option<ExposureGroup>,
    pub state: StackGroupState,
    #[serde(default)]
    pub phase: String,
    pub total_candidates: usize,
    pub eligible_frames: usize,
    pub quality_excluded: usize,
    pub missing_files: usize,
    /// Frames left out because they cannot be calibrated properly: no
    /// matching flat, no bias or dark, or flats past the age limit, where
    /// the library holds such frames for their sensor. Each frame's reason
    /// is on its decision.
    #[serde(default)]
    pub calibration_excluded: usize,
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
    /// What color calibration did for a one-shot-color stack's preview.
    /// The stack's FITS stays as integrated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_calibration: Option<color_calibration::StackColorCalibrationOutcome>,
    /// The solve and Gaia catalog that calibration rested on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_calibration_basis: Option<color_calibration::StackColorCalibrationBasis>,
    /// Why the calibrated preview is out of date, worked out when the stack
    /// is served: a new solve or Gaia catalog since. Never stored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_calibration_outdated: Option<String>,
    /// The Seiza stretch release that rendered the card preview. Absent on
    /// a manifest written before it was recorded, whose preview
    /// [`JOB_ID_STRETCH_VERSION`] rendered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_stretch_version: Option<String>,
    /// Set each time the card preview is rendered again from the stack's
    /// FITS. The page adds it to the preview's address, so a browser fetches
    /// the new image while the stack keeps its artifact revision.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_revision: Option<String>,
    /// The stretch release at which rendering the card preview again from
    /// the FITS failed. No Build tries again until the release changes or
    /// the stack is rebuilt; the card's note says why it failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview_render_failed: Option<String>,
    pub total_exposure_seconds: f64,
    pub preview_url: Option<String>,
    pub fits_url: Option<String>,
    /// The progressive signal-to-noise curve this build measured, and what it
    /// reads as. Absent until the build has passed its first checkpoint.
    #[serde(default)]
    pub snr: Option<snr::ProgressiveSnr>,
    /// Where the curve is published as JSON, beside the stack's own FITS.
    #[serde(default)]
    pub snr_url: Option<String>,
    /// Where the final rejection pass is, while it runs. The frame counts
    /// above are complete by then, so this is what still moves.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub final_pass: Option<FinalPassProgress>,
    /// The calibration master being built for this channel, while one is.
    /// A value this build cannot read, such as the older shape a stopped
    /// build saved, reads as none rather than failing the whole manifest.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "lenient_master_progress"
    )]
    pub calibration_progress: Option<crate::calibration::MasterBuildProgress>,
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
    /// The order every group integrated its frames in.
    #[serde(default)]
    pub order: snr::StackFrameOrder,
    /// Fully populated scoring settings used for frame admission. Manifests
    /// written before this field existed used the calibrated defaults.
    #[serde(default)]
    pub scoring: StackScoringSettings,
    /// How every group integrated its frames. `None` only on a manifest
    /// written before the method could be chosen, with Seiza 0.18: such a
    /// stack is out of date whatever the method now is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<StackMethod>,
    pub groups: Vec<StackGroupStatus>,
    pub error: Option<String>,
    /// See [`StackPreviewRequest::color_defaults`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_defaults: Option<color::StackColorProcessing>,
    /// Built by the automatic refresh rather than asked for. Such a build
    /// steps aside for one the user starts.
    #[serde(default)]
    pub automatic: bool,
    /// While preparing: what was asked for, in a few words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preparing: Option<String>,
    /// Once prepared: the builds this request became, already queued,
    /// running or finished.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub replaced_by: Vec<String>,
}

/// A concrete scoring-policy snapshot attached to every stack artifact.
/// Unlike the request overrides, every penalty has a value and disabled
/// absolute limits are explicit, making persisted artifacts comparable.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StackScoringSettings {
    pub penalty_satellite: f64,
    pub penalty_pointing: f64,
    pub penalty_temporal: f64,
    pub penalty_hfr_rise: f64,
    pub penalty_star_loss: f64,
    pub hfr_reject_above: Option<f64>,
    pub star_count_reject_below: Option<f64>,
}

impl Default for StackScoringSettings {
    fn default() -> Self {
        let config = SequenceAnalyzerConfig::default();
        Self {
            penalty_satellite: config.penalty_scales.satellite,
            penalty_pointing: config.penalty_scales.pointing,
            penalty_temporal: config.penalty_scales.temporal,
            penalty_hfr_rise: config.penalty_scales.hfr_rise,
            penalty_star_loss: config.penalty_scales.star_loss,
            hfr_reject_above: config.hfr_reject_above,
            star_count_reject_below: config.star_count_reject_below,
        }
    }
}

impl StackScoringSettings {
    fn from_overrides(overrides: &crate::server::api::ScoringOverrideQuery) -> Self {
        let mut config = SequenceAnalyzerConfig::default();
        overrides.apply_to(&mut config);
        // The analyzer's own normalization, so the snapshot recorded on the
        // artifact is exactly what scored its frames.
        let config = config.normalized();
        Self {
            penalty_satellite: config.penalty_scales.satellite,
            penalty_pointing: config.penalty_scales.pointing,
            penalty_temporal: config.penalty_scales.temporal,
            penalty_hfr_rise: config.penalty_scales.hfr_rise,
            penalty_star_loss: config.penalty_scales.star_loss,
            hfr_reject_above: config.hfr_reject_above,
            star_count_reject_below: config.star_count_reject_below,
        }
    }

    fn apply_to(self, config: &mut SequenceAnalyzerConfig) {
        config.penalty_scales.satellite = self.penalty_satellite;
        config.penalty_scales.pointing = self.penalty_pointing;
        config.penalty_scales.temporal = self.penalty_temporal;
        config.penalty_scales.hfr_rise = self.penalty_hfr_rise;
        config.penalty_scales.star_loss = self.penalty_star_loss;
        config.hfr_reject_above = self.hfr_reject_above;
        config.star_count_reject_below = self.star_count_reject_below;
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LatestStackPreviewGroup {
    pub job_id: String,
    pub artifact_revision: String,
    pub accepted_only: bool,
    pub created_unix_seconds: i64,
    #[serde(default)]
    pub cache_version: u32,
    /// The order used to build this persisted artifact. Old indices default
    /// to capture order, which was the only order available before this field.
    #[serde(default)]
    pub order: snr::StackFrameOrder,
    /// Scoring settings used for this artifact. Old indices used calibrated
    /// defaults, represented by the type's default value.
    #[serde(default)]
    pub scoring: StackScoringSettings,
    /// How this artifact integrated its frames. `None` only on an index
    /// written before the method could be chosen, with Seiza 0.18.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<StackMethod>,
    /// Set when WBPP made this stack and PSF Guard only took it in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wbpp: Option<wbpp_stacks::WbppStackSource>,
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

impl StackGroupStatus {
    /// Leave out what only the server reads. Each frame's registered mapping
    /// holds its background normalization tiles, about 11 KB a frame; the
    /// final pass and the sky coverage read them from the stored job, and
    /// the page never does.
    fn strip_for_client(&mut self) {
        for frame in &mut self.frames {
            frame.registered_mapping = None;
        }
    }
}

impl StackPreviewJob {
    /// The job as a reply sends it: the page polls a running job every
    /// second, and with its frame mappings a 122-frame stack ran to 1.1 MB.
    pub fn for_client(mut self) -> Self {
        self.groups
            .iter_mut()
            .for_each(StackGroupStatus::strip_for_client);
        self
    }
}

impl LatestStackPreviews {
    /// The index as a reply sends it, without each frame's mapping.
    pub fn for_client(mut self) -> Self {
        for latest in &mut self.groups {
            latest.group.strip_for_client();
        }
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StackActivityKind {
    Mono,
    Color,
    /// A stretch, deconvolution or RC-Astro run on a finished stack.
    Processing,
}

/// How far the final rejection pass has read: each admitted frame once per
/// pass, three passes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct FinalPassProgress {
    /// 1-based.
    pub pass: usize,
    pub passes: usize,
    /// Frames read in this pass, 1-based.
    pub frame: usize,
    pub frames: usize,
}

impl FinalPassProgress {
    pub fn done(&self) -> usize {
        (self.pass.saturating_sub(1)) * self.frames + self.frame
    }

    pub fn total(&self) -> usize {
        self.passes * self.frames
    }
}

/// Work done and to do in one channel, in frame reads: the live pass reads
/// each eligible frame once, and a final pass three more times per admitted
/// frame. Until the final pass starts its size is estimated from the
/// eligible frames, so the share only steps forward when frames drop out.
/// `Master flat L · reading frame 12/40`, or `Master dark · integrating
/// frame 3/8`: the master, Seiza's stage, and the step under way.
fn lenient_master_progress<'de, D>(
    deserializer: D,
) -> Result<Option<crate::calibration::MasterBuildProgress>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let value = Option::<serde_json::Value>::deserialize(deserializer)?;
    Ok(value.and_then(|value| serde_json::from_value(value).ok()))
}

fn master_progress_label(progress: &crate::calibration::MasterBuildProgress) -> String {
    use crate::calibration::CalibrationKind;
    let mut label = format!(
        "Master {}",
        match progress.kind {
            CalibrationKind::Bias => "bias",
            CalibrationKind::Dark => "dark",
            CalibrationKind::DarkFlat => "dark flat",
            CalibrationKind::Flat => "flat",
        }
    );
    if matches!(
        progress.kind,
        CalibrationKind::Flat | CalibrationKind::DarkFlat
    ) && let Some(filter) = &progress.filter
    {
        label.push(' ');
        label.push_str(filter);
    }
    use crate::calibration::MasterBuildStage;
    let what = match progress.stage {
        MasterBuildStage::Read => "reading frame",
        MasterBuildStage::Reread => "rereading kept frame",
        MasterBuildStage::Integrate => "integrating frame",
        MasterBuildStage::Combine => "combining tile",
    };
    let step = (progress.done + 1).min(progress.total);
    label.push_str(&format!(" · {what} {step}/{}", progress.total));
    label
}

/// Calibrate the lights pushed next with a session's masters, judged under
/// its gate: the rig tolerances its masters were matched by. The gate goes
/// first, so the masters are never weighed under another session's.
/// Frames already admitted keep the gate they were admitted under.
pub(crate) fn use_session(
    stacker: &mut seiza_stacking::LiveStacker,
    session: &crate::calibration::CalibrationSession,
) -> seiza_stacking::Result<()> {
    stacker.set_match_tolerances(session.match_tolerances)?;
    stacker.set_calibration(session.masters.clone())
}

fn group_work(group: &StackGroupStatus, reintegrates: bool) -> (usize, usize) {
    let live = group.eligible_frames;
    let estimate = if reintegrates && live >= 3 {
        3 * live
    } else {
        0
    };
    // Matching and building calibration masters weighs as one pass over the
    // frames. It is settled once the channel is past it, whether masters
    // were built, found in the cache, or not wanted.
    let calibration = live;
    let calibrated = match group.state {
        StackGroupState::Queued => 0,
        StackGroupState::Running if group.phase == "calibration" => group
            .calibration_progress
            .as_ref()
            .map_or(0, |master| (master.share() * calibration as f64) as usize),
        _ => calibration,
    };
    match (group.state, group.final_pass) {
        (StackGroupState::Skipped, _) => (0, 0),
        (StackGroupState::Ready, Some(pass)) => {
            let total = calibration + live + pass.total();
            (total, total)
        }
        (StackGroupState::Ready, None) => {
            let total = calibration + live + estimate;
            (total, total)
        }
        (_, Some(pass)) => (
            calibrated + live + pass.done().min(pass.total()),
            calibration + live + pass.total(),
        ),
        (_, None) => (
            calibrated + group.processed_frames.min(live),
            calibration + live + estimate,
        ),
    }
}

/// One queued or running stack build, described well enough for a header
/// indicator and for a panel to re-attach to a job it did not start.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StackActivityEntry {
    pub kind: StackActivityKind,
    pub job_id: String,
    pub database_id: String,
    pub project_id: i32,
    /// The target of the work in flight, when it has one target: the header
    /// links the row to its stacks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_id: Option<i32>,
    pub state: StackJobState,
    /// Target and channel of the work in flight.
    pub label: String,
    /// Short phase text, for example `Registering frames`.
    pub detail: String,
    pub processed_units: usize,
    pub total_units: usize,
    pub created_unix_seconds: i64,
    /// Started by the automatic refresh, not by a person.
    #[serde(default)]
    pub automatic: bool,
    /// Place in the line of waiting builds, 0 next; `None` once running.
    #[serde(default)]
    pub queue_position: Option<usize>,
    /// The progress in words, when the units alone would mislead: the
    /// final rejection pass reads frames the live pass already counted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress_label: Option<String>,
    /// What a processing run does and where it stands; only on
    /// `processing` entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub processing: Option<processing_queue::ProcessingActivity>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StackActivity {
    pub schema_version: u32,
    pub active: Vec<StackActivityEntry>,
    /// Automatic refreshes, soonest first: those still settling, and those
    /// due and `waiting` for the worker. They become builds in `active` when
    /// they start.
    #[serde(default)]
    pub scheduled: Vec<ScheduledRefreshView>,
    /// Processing being re-applied to refreshed stacks that waits for its
    /// turn, out of the line until the worker is free.
    #[serde(default)]
    pub waiting_reapplies: Vec<WaitingReapplyView>,
}

/// A waiting automatic refresh, with the names a person reads.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledRefreshView {
    pub database_id: String,
    pub database_name: String,
    pub project_id: Option<i32>,
    /// `None` for a whole-database refresh, or a project that is gone.
    pub project_name: Option<String>,
    /// `arrival`, `sync` or `grade`.
    pub reason: automatic::RefreshReason,
    pub due_in_seconds: u64,
    /// The channels it expects to restack, then the ones it will stack for
    /// the first time, as people read them. A refresh that expects to do
    /// nothing is not listed.
    pub channels: Vec<String>,
    /// Due, and waiting for the worker rather than settling.
    #[serde(default)]
    pub waiting: bool,
    /// What it waits for, in words that fit "Waiting for …": a person's
    /// work, such as `a stack build`, or the build holding the worker.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting_for: Option<String>,
}

/// Processing re-applied to a refreshed stack, waiting for its turn.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WaitingReapplyView {
    pub database_id: String,
    pub project_id: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_id: Option<i32>,
    /// The channel, such as `M44 · R`.
    pub label: String,
    /// Following a build a person started: their work, not automatic.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub personal: bool,
    /// What it waits for, in words that fit "Waiting for …".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting_for: Option<String>,
}

fn channel_label(target_name: &str, filter_name: &str) -> String {
    if filter_name.is_empty() {
        target_name.to_string()
    } else if target_name.is_empty() {
        // A composition still being prepared has not read its target yet.
        filter_name.to_string()
    } else {
        format!("{target_name} · {filter_name}")
    }
}

fn mono_activity(job: &StackPreviewJob) -> StackActivityEntry {
    let pending = job.groups.iter().find(|group| group_in_flight(group));
    let label = match pending {
        _ if job.state == StackJobState::Preparing => job
            .preparing
            .clone()
            .unwrap_or_else(|| "Stack build".into()),
        Some(group) => {
            let mut base = channel_label(&group.target_name, &group.filter_name);
            if let Some(exposure_group) = &group.exposure_group {
                base.push_str(" · ");
                base.push_str(&exposure_group.label);
            }
            let remaining = job
                .groups
                .iter()
                .filter(|entry| entry.index != group.index && group_in_flight(entry))
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
        _ if job.state == StackJobState::Preparing => {
            "Reading frames and matching calibration".to_string()
        }
        Some(group) if group.phase == PHASE_RENDER_QUEUED => {
            "Waiting to render preview".to_string()
        }
        Some(group) if group.state == StackGroupState::Queued => "Waiting for stacker".to_string(),
        Some(group) => match group.phase.as_str() {
            "calibration" => "Building calibration masters".to_string(),
            "rendering" => "Rendering preview".to_string(),
            phase if phase.starts_with("Rejecting transients:") => phase.to_string(),
            _ => "Registering frames".to_string(),
        },
        None => "Preparing stack".to_string(),
    };
    let reintegrates = job
        .method
        .is_none_or(|method| method.final_pass == StackFinalPass::Reintegrate);
    let (processed_units, total_units) = job
        .groups
        .iter()
        .map(|group| group_work(group, reintegrates))
        .fold((0, 0), |(done, total), (group_done, group_total)| {
            (done + group_done, total + group_total)
        });
    // What a person reads: frames while registering, then where the final
    // pass is.
    let progress_label = pending.and_then(|group| match group.final_pass {
        _ if group.state == StackGroupState::Running && group.phase == "calibration" => group
            .calibration_progress
            .as_ref()
            .map(master_progress_label),
        Some(pass) if group.state == StackGroupState::Running => Some(format!(
            "Rejecting transients · pass {}/{} · frame {}/{}",
            pass.pass, pass.passes, pass.frame, pass.frames
        )),
        _ if group.state == StackGroupState::Running && group.eligible_frames > 0 => Some(format!(
            "{}/{} frames",
            group.processed_frames.min(group.eligible_frames),
            group.eligible_frames
        )),
        _ => None,
    });
    // The group in flight, or the job's only target when nothing is.
    let target_id = pending.map(|group| group.target_id).or_else(|| {
        let first = job.groups.first()?.target_id;
        job.groups
            .iter()
            .all(|group| group.target_id == first)
            .then_some(first)
    });
    StackActivityEntry {
        kind: StackActivityKind::Mono,
        job_id: job.job_id.clone(),
        database_id: job.database_id.clone(),
        project_id: job.project_id,
        target_id,
        state: job.state,
        label,
        detail,
        processed_units,
        total_units,
        created_unix_seconds: job.created_unix_seconds,
        automatic: job.automatic,
        queue_position: None,
        progress_label,
        processing: None,
    }
}

fn color_activity(job: &color::StackColorJob) -> StackActivityEntry {
    // Hundredths of a unit, so an RC-Astro tool's own progress moves the
    // line's percent between units.
    let (processed_units, total_units) = if job.progress.total_units > 0 {
        let partial = (job.progress.partial_units * 100.0).round().max(0.0) as usize;
        (
            (job.progress.completed_units * 100 + partial).min(job.progress.total_units * 100),
            job.progress.total_units * 100,
        )
    } else {
        (job.processed_channels, job.total_channels)
    };
    let running = job.state == StackJobState::Running;
    StackActivityEntry {
        kind: StackActivityKind::Color,
        job_id: job.job_id.clone(),
        database_id: job.database_id.clone(),
        project_id: job.project_id,
        target_id: Some(job.target_id),
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
        automatic: job.automatic,
        queue_position: None,
        // What it is doing, as `H-alpha: BlurXTerminator (42%)`: step counts
        // say nothing while one tool runs for minutes.
        progress_label: running.then(|| {
            if job.phase.is_empty() {
                "Composing color".to_string()
            } else {
                job.phase.clone()
            }
        }),
        processing: None,
    }
}

pub struct StackPreviewManager {
    jobs: Mutex<HashMap<String, StackPreviewJob>>,
    color_jobs: Mutex<HashMap<String, color::StackColorJob>>,
    artifact_jobs: Mutex<HashMap<String, artifact::ArtifactSearchJob>>,
    latest_write: Mutex<()>,
    /// The line builds, compositions and processing runs wait in, and the
    /// worker they share. A color composition a person asked for that
    /// prepares nothing at length runs in its quick lane, beside the worker
    /// rather than behind it.
    line: crate::server::job_line::JobLine,
    /// Every build a server stop interrupted, in the order they would have
    /// run. A stopped build is no longer tracked by the line; this keeps its
    /// journal record for the journal's last write.
    stopped_for_shutdown: Mutex<Option<Vec<(String, journal::JournaledStackJob)>>>,
    /// Stretch, deconvolution and RC-Astro runs waiting in the line or
    /// running, by run id. The journal keeps them through the line's
    /// journal records.
    processing: Mutex<HashMap<String, processing_queue::ProcessingRun>>,
    /// Processing runs that published their result after a server stop
    /// began, so its last journal write leaves them out.
    completed_during_stop: Mutex<HashSet<String>>,
    /// Processing carried to refreshed stacks and waiting to run again, or
    /// running, by channel.
    reapplies: Mutex<HashMap<String, reapply::Reapply>>,
    /// Re-applies stopped to make way for a person's work, which wait again.
    yielded: Mutex<HashSet<String>>,
    /// Builds a person asked for that wait to be prepared, in click order.
    preparing: Mutex<build_set::PreparationLine>,
    /// The longest the next turn is kept for a person's re-apply that has
    /// not joined the line yet, once that turn holds anything up.
    turn_held_for: std::time::Duration,
}

impl StackPreviewManager {
    pub fn new() -> Self {
        Self {
            jobs: Mutex::new(HashMap::new()),
            color_jobs: Mutex::new(HashMap::new()),
            artifact_jobs: Mutex::new(HashMap::new()),
            latest_write: Mutex::new(()),
            line: crate::server::job_line::JobLine::new(),
            stopped_for_shutdown: Mutex::new(None),
            processing: Mutex::new(HashMap::new()),
            completed_during_stop: Mutex::new(HashSet::new()),
            reapplies: Mutex::new(HashMap::new()),
            yielded: Mutex::new(HashSet::new()),
            preparing: Mutex::new(build_set::PreparationLine::default()),
            turn_held_for: reapply::TURN_HELD_FOR,
        }
    }

    /// Hold a place in the line for a request still being prepared: a
    /// person's ahead of automatic work. The work it becomes takes the
    /// place.
    fn hold_place(&self, job_id: &str, holds: Vec<String>, automatic: bool) {
        self.line.hold_place(job_id, holds, tier_of(automatic));
    }

    /// Give up a place a request held, unless work took it.
    fn release_place(&self, job_id: &str) {
        self.line.release_place(job_id);
    }

    /// Whether the job runs, or waits to run, in the quick lane.
    fn is_quick(&self, job_id: &str) -> bool {
        self.line.lane_of(job_id) == Some(Lane::Quick)
    }

    /// Move a waiting build to `position` in the line (0 is next), clamped to
    /// the line. False when the build is not waiting.
    pub fn move_in_line(&self, job_id: &str, position: usize) -> bool {
        self.line.move_to(job_id, position)
    }

    fn line_position(&self, job_id: &str) -> Option<usize> {
        self.line.position(job_id)
    }

    pub fn get(&self, job_id: &str) -> Option<StackPreviewJob> {
        self.jobs.lock().unwrap().get(job_id).cloned()
    }

    /// Track a job about to be queued, with a new stop flag.
    fn track_job(&self, job_id: &str) -> Stop {
        self.line.track(job_id)
    }

    fn forget_job(&self, job_id: &str) {
        self.line.forget(job_id);
    }

    /// Forget a job's stop flag and journal record, but only while the flag
    /// is still this task's. A refresh that resumes a stopped build asks for
    /// the same job id, so the stopped task, finishing late, must not clear
    /// the new one's flag.
    fn forget_job_of(&self, job_id: &str, flag: &Stop) {
        self.line.forget_if_current(job_id, flag);
    }

    fn remember_origin(&self, job_id: &str, origin: journal::JournaledStackJob) {
        self.line.set_journal(job_id, &origin);
    }

    fn job_state(&self, job_id: &str) -> Option<StackJobState> {
        self.jobs
            .lock()
            .unwrap()
            .get(job_id)
            .map(|job| job.state)
            .or_else(|| {
                self.color_jobs
                    .lock()
                    .unwrap()
                    .get(job_id)
                    .map(|job| job.state)
            })
            .or_else(|| self.processing_state(job_id))
    }

    /// Every build and processing run still queued or running, as it was
    /// asked for, in the order it will run: running first, then the line.
    pub fn journal_entries(&self) -> Vec<journal::JournaledStackJob> {
        let mut entries = self.ordered_origins();
        let Some(stopped) = self.stopped_for_shutdown.lock().unwrap().clone() else {
            return entries.into_iter().map(|(_, origin)| origin).collect();
        };
        // A build the stop interrupted comes back unless it finished first.
        // One that stopped at its checkpoint was not cut off, so it does not
        // count against its attempts; one still running would be. A
        // processing run has no checkpoint: one the stop found running lost
        // the step in hand, so it keeps the running mark the stop wrote. A
        // request prepared before the stop reached it comes back as the
        // builds it became, which are journaled themselves.
        let mut kept = Vec::with_capacity(stopped.len() + entries.len());
        for (job_id, mut origin) in stopped {
            let state = self.job_state(&job_id);
            if matches!(
                state,
                Some(StackJobState::Completed | StackJobState::Failed | StackJobState::Replaced)
            ) {
                continue;
            }
            if !origin.is_processing() {
                origin.set_running(state == Some(StackJobState::Running));
            }
            entries.retain(|(entry_id, _)| entry_id != &job_id);
            kept.push(origin);
        }
        kept.extend(entries.into_iter().map(|(_, origin)| origin));
        kept
    }

    fn ordered_origins(&self) -> Vec<(String, journal::JournaledStackJob)> {
        let origins = self.line.journals::<journal::JournaledStackJob>();
        let running = |job_id: &str| self.job_state(job_id) == Some(StackJobState::Running);
        let mut entries = origins
            .into_iter()
            .map(|(job_id, origin)| {
                let rank = if running(&job_id) {
                    (0, 0)
                } else {
                    (1, self.line_position(&job_id).unwrap_or(usize::MAX))
                };
                (rank, job_id, origin)
            })
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
        entries
            .into_iter()
            .map(|(rank, job_id, mut origin)| {
                origin.set_running(rank.0 == 0);
                (job_id, origin)
            })
            .collect()
    }

    /// Stop every build for a server stop. A running build checkpoints the
    /// frames it integrated and a waiting one leaves the line; the journal
    /// puts all of them back on the next start. A running RC-Astro tool is
    /// killed rather than waited for; the journal, written first, marks its
    /// run as cut off, and the chain resumes from its last finished step.
    pub fn stop_for_shutdown(&self) {
        let interrupted = self.ordered_origins();
        *self.stopped_for_shutdown.lock().unwrap() = Some(interrupted);
        for job_id in self.line.tracked_ids() {
            self.request_cancel(&job_id);
        }
    }

    /// Ask a queued or running job to stop. Returns false when the job is not
    /// in flight, which includes a job that already finished.
    pub fn request_cancel(&self, job_id: &str) -> bool {
        // A build that has ended has nothing to stop, though its flag may
        // not be forgotten yet.
        if matches!(
            self.job_state(job_id),
            Some(
                StackJobState::Completed
                    | StackJobState::Failed
                    | StackJobState::Cancelled
                    | StackJobState::Replaced
            )
        ) {
            return false;
        }
        let Some(flag) = self.line.stop_flag(job_id) else {
            return false;
        };
        flag.request();
        // A build still being prepared has nothing to stop: it ends now, and
        // what its preparation finds is dropped. A restart does not bring it
        // back; a server stop journaled it before asking it to stop.
        let mut ended = false;
        self.update(job_id, |job| {
            if job.state == StackJobState::Preparing {
                job.state = StackJobState::Cancelled;
                job.preparing = None;
                ended = true;
            }
        });
        self.update_color(job_id, |job| {
            if job.state == StackJobState::Preparing {
                job.state = StackJobState::Cancelled;
                job.preparing = None;
                job.phase = "Stopped before composing".into();
                ended = true;
            }
        });
        if ended {
            self.line.clear_journal(job_id);
            self.release_place(job_id);
        }
        // A build still waiting sees it at once and leaves the line.
        self.line.changed();
        true
    }

    /// Every queued or running stack build, oldest first. Jobs outlive the
    /// page that started them, so a client that navigated away can still see
    /// the work and re-attach to it.
    /// A build the automatic refresh started becomes the user's: it keeps
    /// running, and no interruption stops it.
    pub fn adopt(&self, job_id: &str) {
        if let Some(job) = self.jobs.lock().unwrap().get_mut(job_id) {
            job.automatic = false;
        }
        if let Some(job) = self.color_jobs.lock().unwrap().get_mut(job_id) {
            job.automatic = false;
        }
        self.adopt_processing(job_id);
        // It runs as the person's: ahead of automatic work, and not stopped
        // for another person's work as automatic work is.
        self.line.set_tier(job_id, Tier::Person);
        // A restart brings it back as the person's too.
        self.line
            .update_journal(job_id, |origin: &mut journal::JournaledStackJob| {
                origin.set_automatic(false)
            });
    }

    /// Whether a build is the automatic refresh's, read now: a person can
    /// adopt it while it waits or runs, and it then takes their share.
    pub(super) fn is_automatic(&self, job_id: &str) -> bool {
        self.jobs
            .lock()
            .unwrap()
            .get(job_id)
            .map(|job| job.automatic)
            .or_else(|| {
                self.color_jobs
                    .lock()
                    .unwrap()
                    .get(job_id)
                    .map(|job| job.automatic)
            })
            .unwrap_or(false)
    }

    /// Stop every re-apply queued or running so other work gets the worker.
    /// Each waits for its turn again rather than ending.
    pub(super) fn yield_automatic_processing(&self) {
        for run_id in self.automatic_processing_ids() {
            self.yielded.lock().unwrap().insert(run_id.clone());
            self.request_cancel(&run_id);
        }
    }

    /// Whether a run was stopped to make way for other work, rather than
    /// for good. Asking forgets it.
    pub(super) fn take_yielded(&self, run_id: &str) -> bool {
        self.yielded.lock().unwrap().remove(run_id)
    }

    pub fn active(&self) -> Vec<StackActivityEntry> {
        let mut active: Vec<StackActivityEntry> = {
            let jobs = self.jobs.lock().unwrap();
            jobs.values()
                .filter(|job| {
                    matches!(
                        job.state,
                        StackJobState::Preparing | StackJobState::Queued | StackJobState::Running
                    )
                })
                .map(mono_activity)
                .collect()
        };
        {
            let color_jobs = self.color_jobs.lock().unwrap();
            active.extend(
                color_jobs
                    .values()
                    .filter(|job| {
                        matches!(
                            job.state,
                            StackJobState::Preparing
                                | StackJobState::Queued
                                | StackJobState::Running
                        )
                    })
                    .map(color_activity),
            );
        }
        active.extend(self.processing_activity());
        for entry in &mut active {
            entry.queue_position = self.line_position(&entry.job_id);
        }
        // Running work first, then the line in the order it will run, then
        // anything between the two, oldest first.
        active.sort_by(|left, right| {
            let rank = |entry: &StackActivityEntry| match (entry.state, entry.queue_position) {
                (StackJobState::Running, _) => (0, 0),
                (_, Some(position)) => (1, position),
                _ => (2, 0),
            };
            rank(left)
                .cmp(&rank(right))
                .then_with(|| left.created_unix_seconds.cmp(&right.created_unix_seconds))
                .then_with(|| left.job_id.cmp(&right.job_id))
        });
        active
    }

    /// Everything on disk that is still referenced: jobs a panel may poll,
    /// and jobs a durable latest index still names. Reads every project's
    /// index because a stack root hosts every project of a database. Fails
    /// with the index it could not read: without it, everything that index
    /// names would look unreferenced.
    fn cache_keep_set(&self, stack_root: &FsPath) -> Result<janitor::KeepSet, PathBuf> {
        use crate::server::storage::{stack_folder, stack_kind, stacks};
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
        let stacks = stacks(stack_root);
        for latest in try_read_indices::<LatestStackPreviews>(&stacks, "latest-project-")?
            .into_iter()
            .chain(try_read_indices::<LatestStackPreviews>(
                &stacks,
                "wbpp-project-",
            )?)
        {
            for group in latest.groups {
                mono_job_ids.insert(group.job_id);
            }
        }
        for latest in try_read_indices::<color::LatestStackColorPreviews>(
            &stack_folder(stack_root, stack_kind::COLOR),
            "latest-project-",
        )? {
            for job in latest.jobs {
                color_job_ids.insert(job.job_id);
                if let Some(input_id) = job.linear_input_id {
                    color_input_ids.insert(input_id);
                }
            }
        }
        let (stretch_ids, deconvolution_ids) = stretch::kept_processing(stack_root)?;
        let mut artifact_search_ids: std::collections::HashSet<String> =
            self.artifact_jobs.lock().unwrap().keys().cloned().collect();
        artifact_search_ids.extend(artifact::kept_searches(
            stack_root,
            &mono_job_ids,
            &color_job_ids,
        ));
        Ok(janitor::KeepSet {
            mono_job_ids,
            color_job_ids,
            color_input_ids,
            stretch_ids,
            deconvolution_ids,
            artifact_search_ids,
        })
    }

    /// Sweep superseded artifacts after a build settles, and each hour.
    /// Failures are logged; pruning never fails a build.
    pub(super) fn prune_cache(&self, stack_root: &FsPath) {
        let keep = match self.cache_keep_set(stack_root) {
            Ok(keep) => keep,
            Err(unreadable) => {
                tracing::warn!(
                    "Not pruning stacks below {}: {} could not be read, so it is unclear \
                     which stacks are still in use",
                    stack_root.display(),
                    unreadable.display()
                );
                return;
            }
        };
        janitor::prune(stack_root, &keep, SEIZA_STACKING_OUTPUT_VERSION);
        let active_sources = self
            .jobs
            .lock()
            .unwrap()
            .values()
            .map(|job| (job.job_id.clone(), job.artifact_revision.clone()))
            .collect();
        rc_astro::prune_rc_astro_cache(stack_root, &active_sources);
    }

    /// The stack worker, when nothing holds it: work that removes files a
    /// build may read takes it so none runs meanwhile.
    pub(crate) fn try_maintenance_permit(&self) -> Option<Lease> {
        self.line.try_reserve_main()
    }

    /// [`Self::try_maintenance_permit`], waiting for the worker to come free.
    pub(crate) async fn acquire_maintenance_permit(&self) -> Result<Lease, String> {
        Ok(self.line.reserve_main().await)
    }

    fn insert(&self, job: StackPreviewJob) -> bool {
        let mut jobs = self.jobs.lock().unwrap();
        if jobs.len() >= MAX_REMEMBERED_JOBS && !jobs.contains_key(&job.job_id) {
            let Some(oldest) = jobs
                .values()
                .filter(|entry| {
                    matches!(
                        entry.state,
                        StackJobState::Completed
                            | StackJobState::Failed
                            | StackJobState::Cancelled
                            | StackJobState::Replaced
                    )
                })
                .min_by_key(|entry| entry.created_unix_seconds)
                .map(|entry| entry.job_id.clone())
            else {
                if matches!(job.state, StackJobState::Queued | StackJobState::Preparing)
                    && jobs.len() < MAX_QUEUED_JOBS
                {
                    jobs.insert(job.job_id.clone(), job);
                    return true;
                }
                return false;
            };
            jobs.remove(&oldest);
        }
        jobs.insert(job.job_id.clone(), job);
        true
    }

    /// Whether another queued or running build still has channels of this
    /// target to stack, so color for it should wait for that build.
    fn target_still_building(
        &self,
        database_id: &str,
        project_id: i32,
        target_id: i32,
        finished_job: &str,
    ) -> bool {
        self.jobs.lock().unwrap().values().any(|job| {
            job.job_id != finished_job
                && job.database_id == database_id
                && job.project_id == project_id
                && matches!(job.state, StackJobState::Queued | StackJobState::Running)
                && job.groups.iter().any(|group| {
                    group.target_id == target_id
                        && matches!(
                            group.state,
                            StackGroupState::Queued | StackGroupState::Running
                        )
                })
        })
    }

    fn update(&self, job_id: &str, update: impl FnOnce(&mut StackPreviewJob)) {
        if let Some(job) = self.jobs.lock().unwrap().get_mut(job_id) {
            update(job);
        }
    }

    /// Queue a preview render on a finished build, if it is still finished:
    /// the check and the change happen under one lock, so two Builds that
    /// find the build at once queue one render. A build no longer in memory
    /// is put back first. False when the build is queued or running, or
    /// there is no room for it. A person's Build makes the job theirs; an
    /// automatic one leaves a person's job theirs.
    fn begin_preview_render(
        &self,
        finished: &StackPreviewJob,
        stale: &[usize],
        automatic: bool,
        color_defaults: Option<color::StackColorProcessing>,
    ) -> bool {
        if self.get(&finished.job_id).is_none() && !self.insert(finished.clone()) {
            return false;
        }
        let mut jobs = self.jobs.lock().unwrap();
        let Some(job) = jobs.get_mut(&finished.job_id) else {
            return false;
        };
        if job.state != StackJobState::Completed {
            return false;
        }
        job.state = StackJobState::Queued;
        job.automatic &= automatic;
        if color_defaults.is_some() {
            job.color_defaults = color_defaults;
        }
        for &index in stale {
            if let Some(group) = job.groups.get_mut(index) {
                group.phase = PHASE_RENDER_QUEUED.into();
            }
        }
        true
    }

    /// Point the cards at a finished build's channels. Returns each card
    /// that moved, with the entry it showed before.
    fn persist_latest(
        &self,
        ctx: &DatabaseContext,
        job: &StackPreviewJob,
    ) -> Result<Vec<(LatestStackPreviewGroup, LatestStackPreviewGroup)>, String> {
        let current = {
            let db = ctx.db();
            let conn = db.lock().map_err(|error| error.to_string())?;
            crate::server::exposure_groups::cached_project_groups(ctx, &conn, job.project_id)
                .map_err(|error| format!("Failed to load project exposure groups: {error:?}"))?
        };
        let _guard = self.latest_write.lock().unwrap();
        persist_latest_groups_with_exposures(&ctx.stack_root, job, Some(&current))
    }
}

#[cfg(test)]
impl StackPreviewManager {
    /// Put a build that does nothing in line under `job_id`, stopped by its
    /// own flag.
    pub(super) fn queue_test_build(&self, job_id: &str) {
        let known = self.line.stop_flag(job_id);
        let stop = known.unwrap_or_else(|| self.track_job(job_id));
        assert!(self
            .line
            .submit(
                crate::server::job_line::testing::entry(job_id, Lane::Main, stop),
                None
            )
            .is_ok());
    }

    /// Dispatch the main lane once, as its dispatcher would: the entry
    /// that runs, or one settled because it was stopped while it waited.
    /// The lane is free again as soon as this returns.
    pub(super) async fn dispatch_next(&self) -> (bool, String) {
        use crate::server::job_line::Dispatch;
        match self.line.next(Lane::Main).await {
            Dispatch::Run { id, .. } => (true, id),
            Dispatch::Stopped { id, .. } => (false, id),
        }
    }
}

impl Default for StackPreviewManager {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Serialize, Deserialize)]
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
    crate::server::exposure_groups::exposure_seconds_from_metadata(metadata_json).unwrap_or(0.0)
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
        noise_sigma: None,
        integration_weight: None,
    }
}

#[derive(Clone, Serialize, Deserialize)]
struct PreparedGroup {
    index: usize,
    /// The mode this channel calibrates under: its request override, or the
    /// request-wide mode.
    calibration: crate::calibration::CalibrationMode,
    /// Captured with the cache key, so a settings save cannot change a queued job.
    flat_star_masking: bool,
    frames: Vec<PreparedFrame>,
}

#[derive(Serialize, Deserialize)]
struct PreparedJob {
    public: StackPreviewJob,
    groups: Vec<PreparedGroup>,
    /// Reference frames of channels queued beside this one, stacked in jobs
    /// of their own. They only teach the pier-side mapping, so a channel
    /// that was never solved still faces the way its solved siblings do.
    orientation_peers: Vec<PreparedFrame>,
    stack_root: PathBuf,
    calibration_root: PathBuf,
    north_up: bool,
    order: snr::StackFrameOrder,
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

/// A finished channel waiting for its card preview to be rendered again. The
/// channel stays Ready throughout: its stack is finished, so its processing,
/// FITS and searches stay available.
const PHASE_RENDER_QUEUED: &str = "render-queued";
/// A finished channel whose card preview is being rendered again.
const PHASE_RENDERING: &str = "rendering";

/// Whether a channel is still in a job's work: building, or a finished
/// channel whose preview is being rendered again.
fn group_in_flight(group: &StackGroupStatus) -> bool {
    match group.state {
        StackGroupState::Queued | StackGroupState::Running => true,
        StackGroupState::Ready => {
            group.phase == PHASE_RENDER_QUEUED || group.phase == PHASE_RENDERING
        }
        _ => false,
    }
}

/// Whether a finished channel's card preview should be rendered again from
/// its FITS: an older stretch release rendered it, or a one-shot-color
/// stack's color calibration is out of date. The stack itself is current. A
/// render that already failed at this stretch release is not tried again.
fn preview_needs_render(
    state: &AppState,
    ctx: &DatabaseContext,
    job_id: &str,
    group: &StackGroupStatus,
) -> bool {
    if group.state != StackGroupState::Ready
        || group.preview_render_failed.as_deref() == Some(stretch::SEIZA_STRETCH_OUTPUT_VERSION)
        || !fits_path(&ctx.stack_root, job_id, group.index).is_file()
    {
        return false;
    }
    let rendered_by = group
        .preview_stretch_version
        .as_deref()
        .unwrap_or(JOB_ID_STRETCH_VERSION);
    rendered_by != stretch::SEIZA_STRETCH_OUTPUT_VERSION
        || preview_calibration_outdated(group, || {
            osc_calibration_source(
                state,
                ctx,
                group.reference_image_id,
                group.sky_orientation.clone(),
            )
        })
        .is_some()
}

/// Queue a render of a finished build's out-of-date card previews from the
/// FITS it wrote, when any are out of date, and return the job as it then
/// stands. The stack is not built again. The job runs in the build line
/// while it renders and is completed again afterwards, stopped or not.
/// `automatic` is false for a person's Build, which makes the job theirs;
/// `color_defaults` replaces the job's stored defaults when given. Reads
/// files, so call it off the async runtime where that matters.
fn queue_preview_render(
    state: &Arc<AppState>,
    ctx: &Arc<DatabaseContext>,
    finished: &StackPreviewJob,
    automatic: bool,
    color_defaults: Option<color::StackColorProcessing>,
) -> Option<StackPreviewJob> {
    let stale = finished
        .groups
        .iter()
        .filter(|group| preview_needs_render(state, ctx, &finished.job_id, group))
        .map(|group| group.index)
        .collect::<Vec<_>>();
    if stale.is_empty()
        || !state
            .stack_previews
            .begin_preview_render(finished, &stale, automatic, color_defaults)
    {
        return None;
    }
    let job_id = finished.job_id.clone();
    let cancel = state.stack_previews.track_job(&job_id);
    let work = state.shutdown.hold(format!(
        "stack preview render for project {} (db={})",
        finished.project_id, finished.database_id
    ));
    // However it ends, the build settles as the finished build it was.
    let (settle_ctx, settle_job) = (Arc::clone(ctx), job_id.clone());
    let (unrun_ctx, unrun_job) = (Arc::clone(ctx), job_id.clone());
    let failed_job = job_id.clone();
    let spec = RenderSpec {
        database_id: ctx.id.clone(),
        job_id: job_id.clone(),
        stale,
    };
    let submission = Submission::<RenderWork>::new(
        "a stack preview render",
        &spec,
        move |state, rendered| {
            if let Err(error) = rendered {
                tracing::warn!(job_id = failed_job, "Stack preview render failed: {error}");
            }
            finish_preview_render(state, &settle_ctx, &settle_job);
        },
        move |state| finish_preview_render(state, &unrun_ctx, &unrun_job),
    )
    .map(|submission| {
        let (job_id, cancel) = (job_id.clone(), cancel.clone());
        let database_id = ctx.id.clone();
        submission.finally(move |state, _| {
            state.stack_previews.forget_job_of(&job_id, &cancel);
            drop(work);
            // Stopped for a person's work: render what is still out of date
            // again, after it.
            if state.stack_previews.line.take_preempted(&job_id)
                && let Some(finished) = state.stack_previews.get(&job_id)
                && let Some(ctx) = state.get_database(&database_id)
            {
                queue_preview_render(state, &ctx, &finished, true, None);
            }
        })
    });
    match submission {
        Ok(submission) => submit(
            state,
            &job_id,
            Lane::Main,
            None,
            cancel,
            Vec::new(),
            automatic,
            Box::new(submission),
        ),
        Err(error) => {
            tracing::warn!(job_id, "Stack preview render could not be queued: {error}");
            finish_preview_render(state, ctx, &job_id);
            state.stack_previews.forget_job_of(&job_id, &cancel);
        }
    }
    state.stack_previews.get(&job_id)
}

/// A finished build's card previews rendered again from its FITS.
struct RenderWork;

#[derive(Serialize, Deserialize)]
struct RenderSpec {
    database_id: String,
    job_id: String,
    /// The channels whose previews are out of date.
    stale: Vec<usize>,
}

impl LineWork for RenderWork {
    type Spec = RenderSpec;
    type Output = ();
    /// It reads the build's FITS from this server's stack cache and records
    /// each channel's progress on the build.
    const NEEDS: Needs = Needs::ThisServer;

    fn execute(spec: RenderSpec, worker: &Worker<'_>) {
        let state = worker.server();
        let Some(ctx) = state.get_database(&spec.database_id) else {
            return;
        };
        let rendered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            render_stale_previews(state, &ctx, &spec.job_id, &spec.stale, worker.stop_flag())
        }));
        if rendered.is_err() {
            tracing::warn!(job_id = spec.job_id, "Stack preview render panicked");
        }
    }
}

/// Render each listed channel's card previews again from its FITS. A
/// channel that cannot be rendered keeps the preview it had, says why on
/// its card, and is not tried again at this stretch release.
fn render_stale_previews(
    state: &Arc<AppState>,
    ctx: &DatabaseContext,
    job_id: &str,
    stale: &[usize],
    cancel: &Stop,
) {
    state.stack_previews.update(job_id, |job| {
        job.state = StackJobState::Running;
    });
    let priority = job_priority(state.stack_previews.is_automatic(job_id));
    let worker_policy = state.worker_policy();
    for &index in stale {
        if cancel.requested() {
            return;
        }
        state.stack_previews.update(job_id, |job| {
            job.groups[index].phase = PHASE_RENDERING.into();
        });
        let fits = fits_path(&ctx.stack_root, job_id, index);
        let budget = crate::concurrency::plan_workers(
            None,
            &worker_policy,
            priority,
            crate::concurrency::probe_frame_pixels(&fits),
        );
        let lease = state.lease_workers(priority, budget.workers);
        let rendered = crate::concurrency::ComputePool::take("stack-preview-render", lease.workers)
            .map_err(|error| error.to_string())
            .and_then(|pool| {
                pool.install(|| {
                    let frame = crate::image_io::open_linear_frame(&fits)
                        .map_err(|error| error.to_string())?;
                    render_group_previews(state, ctx, &ctx.stack_root, job_id, index, &frame.image)
                })
            });
        let revision = new_artifact_revision();
        state.stack_previews.update(job_id, |job| {
            let group = &mut job.groups[index];
            match &rendered {
                Ok(()) => {
                    group.preview_revision = Some(revision);
                    group.preview_render_failed = None;
                }
                Err(error) => {
                    tracing::warn!(job_id, group_index = index, "Stack preview render failed: {error}");
                    group.preview_render_failed = Some(stretch::SEIZA_STRETCH_OUTPUT_VERSION.into());
                    add_note(
                        &mut group.resume_note,
                        &format!("The preview could not be rendered again from the stack's FITS: {error}"),
                    );
                }
            }
            group.phase = "ready".into();
        });
    }
}

/// Complete a preview render however it ended, and point the cards at the
/// new previews. Every channel is ready: the stack was finished before the
/// render and still is, so the next Build reuses it. Color is not composed
/// again: it never waited for the render, and its inputs did not change.
fn finish_preview_render(state: &Arc<AppState>, ctx: &Arc<DatabaseContext>, job_id: &str) {
    let settle = |job: &mut StackPreviewJob| {
        for group in &mut job.groups {
            if group.state == StackGroupState::Ready && group_in_flight(group) {
                group.phase = "ready".into();
            }
        }
        job.state = StackJobState::Completed;
    };
    let Some(mut job) = state.stack_previews.get(job_id) else {
        return;
    };
    settle(&mut job);
    // On disk before it reads as finished: a client that sees it finished
    // reads the latest index next, and must find the new preview there.
    if let Err(error) = persist_manifest(&ctx.stack_root, &job) {
        tracing::warn!("Failed to persist stack preview manifest: {error}");
    }
    // The artifact revision is unchanged, so no card moves and no processing
    // is applied again; the cards only take the new preview revision.
    if let Err(error) = persist_latest_and_reapply(state, ctx, &job, !job.automatic) {
        tracing::warn!("Failed to persist latest stack preview index: {error}");
    }
    state.stack_previews.update(job_id, settle);
}

/// Point the cards at a finished build, and carry the processing each card
/// showed to the new stack it shows. `personal` when a person asked for the
/// build, so the processing is theirs to run next.
pub(super) fn persist_latest_and_reapply(
    state: &Arc<AppState>,
    ctx: &DatabaseContext,
    job: &StackPreviewJob,
    personal: bool,
) -> Result<(), String> {
    let moved = state.stack_previews.persist_latest(ctx, job)?;
    if !moved.is_empty() {
        reapply::after_build(state, ctx, job.project_id, moved, personal);
    }
    Ok(())
}

/// A person's work takes the stacking worker: the automatic entry running
/// on it, if any, stops at its checkpoint and goes back in the line behind
/// the person's work. Automatic builds still waiting stay in line. Re-applies
/// of processing, queued or running, step aside and wait for their turn
/// again, as they did before the line ordered work by tier.
pub(super) fn interrupt_automatic(state: &AppState) {
    state.stack_previews.line.preempt_automatic(Lane::Main);
    state.stack_previews.yield_automatic_processing();
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
        state
            .stack_previews
            .get(&job_id)
            .unwrap_or(job)
            .for_client(),
    )))
}

/// Cross-database view of stack builds still in flight. The manager is
/// process-global, so this stays outside the per-database routes and lets the
/// header report stacking from any view.
pub async fn get_stack_activity(
    State(state): State<Arc<AppState>>,
) -> Json<ApiResponse<StackActivity>> {
    // Names of scheduled refreshes come from the catalogs: a short read
    // each, off the runtime.
    let activity =
        tokio::task::spawn_blocking(move || stack_activity(&state, RefreshPlanning::Plan))
            .await
            .unwrap_or_else(|_| StackActivity {
                schema_version: 1,
                active: Vec::new(),
                scheduled: Vec::new(),
                waiting_reapplies: Vec::new(),
            });
    Json(ApiResponse::success(activity))
}

/// Whether listing stack work may plan a waiting refresh to learn which
/// channels it would restack.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum RefreshPlanning {
    /// Plan on the database's shared connection when the cache has no
    /// answer: the Stacks page's route.
    Plan,
    /// Read only what the refresh scheduler cached: the header's jobs
    /// route, which must not wait for a shared connection.
    CachedOnly,
}

/// The stack work queued, running or waiting on every database. Reads a
/// short query per scheduled refresh to name its project, so call it off
/// the async runtime.
pub(crate) fn stack_activity(state: &Arc<AppState>, planning: RefreshPlanning) -> StackActivity {
    let scheduled = state.auto_stacks.scheduled();
    let scheduled = if scheduled.is_empty() {
        Vec::new()
    } else {
        name_scheduled(state, scheduled, planning)
    };
    let waiting_reapplies = waiting_reapplies(state, &scheduled);
    // Read after the refreshes and re-applies, so one that started
    // meanwhile is in the line by now; it is listed once, as its build or
    // run.
    let active = state.stack_previews.active();
    StackActivity {
        schema_version: 1,
        scheduled: without_started(scheduled, &active),
        waiting_reapplies: reapplies_not_started(waiting_reapplies, &active),
        active,
    }
}

/// What automatic stack work waits for, in words that fit "Waiting for …".
/// A due refresh waits only for a quality analysis of its database, which
/// rewrites the scores it would stack by; its builds then wait in the line.
/// A re-apply waits for the build or run holding the worker, or the line
/// ahead of it, a person's work, or a quiet moment after it. `None` when
/// it is free to start at the scheduler's next look.
fn automatic_work_waits_for(state: &AppState, database_id: &str, reapply: bool) -> Option<String> {
    let quality_analysis = || {
        let ctx = state.get_database(database_id)?;
        crate::server::quality_backfill::snapshot(&ctx.quality_backfill)
            .running
            .then(|| format!("the quality analysis of {}", ctx.name))
    };
    if !reapply {
        return quality_analysis();
    }
    let manager = &state.stack_previews;
    let holder = (!manager.active().is_empty()).then(|| {
        manager
            .worker_holder("")
            .or_else(|| manager.quick_lane_holder())
            .unwrap_or_else(|| "the work in the stack line".to_string())
    });
    holder
        .or_else(|| state.interactive_job_name())
        .or_else(|| {
            (!automatic::quiet_for_automatic_work(state))
                .then(|| "a quiet moment after your work".to_string())
        })
        .or_else(quality_analysis)
}

/// A re-apply whose run has just joined the line is listed as that run,
/// not twice.
fn reapplies_not_started(
    waiting: Vec<WaitingReapplyView>,
    active: &[StackActivityEntry],
) -> Vec<WaitingReapplyView> {
    waiting
        .into_iter()
        .filter(|reapply| {
            !active.iter().any(|entry| {
                entry.automatic != reapply.personal
                    && entry.kind == StackActivityKind::Processing
                    && entry.database_id == reapply.database_id
                    && entry.project_id == reapply.project_id
                    && entry.label == reapply.label
            })
        })
        .collect()
}

/// A waiting refresh whose build has just joined the line is listed as that
/// build, not twice.
fn without_started(
    scheduled: Vec<ScheduledRefreshView>,
    active: &[StackActivityEntry],
) -> Vec<ScheduledRefreshView> {
    scheduled
        .into_iter()
        .filter(|refresh| {
            !refresh.waiting
                || !active.iter().any(|entry| {
                    entry.automatic
                        && entry.kind != StackActivityKind::Processing
                        && entry.database_id == refresh.database_id
                        && refresh
                            .project_id
                            .is_none_or(|project_id| project_id == entry.project_id)
                })
        })
        .collect()
}

/// Re-applies of processing out of the line, waiting for their turn.
fn waiting_reapplies(
    state: &AppState,
    scheduled: &[ScheduledRefreshView],
) -> Vec<WaitingReapplyView> {
    let mut waiting = state.stack_previews.waiting_reapplies();
    waiting.sort_by_key(|(since, _)| *since);
    waiting
        .into_iter()
        .map(|(_, mut view)| {
            if view.personal {
                // A person's waits only for their re-apply ahead of it.
                view.waiting_for = state
                    .stack_previews
                    .personal_reapply_in_line()
                    .map(|label| format!("the processing of {label}"));
                return view;
            }
            view.waiting_for =
                automatic_work_waits_for(state, &view.database_id, true).or_else(|| {
                    // A refresh of its project may bring a newer stack first.
                    scheduled
                        .iter()
                        .any(|refresh| {
                            refresh.database_id == view.database_id
                                && refresh
                                    .project_id
                                    .is_none_or(|project_id| project_id == view.project_id)
                        })
                        .then(|| "its project's stacks to refresh".to_string())
                });
            view
        })
        .collect()
}

fn name_scheduled(
    state: &AppState,
    scheduled: Vec<automatic::ScheduledRefresh>,
    planning: RefreshPlanning,
) -> Vec<ScheduledRefreshView> {
    // Soonest first: a later refresh whose channels an earlier one of the
    // same database already restacks would find nothing left, so it is not
    // listed either.
    let mut claimed: HashSet<(String, String)> = HashSet::new();
    scheduled
        .into_iter()
        .filter_map(|refresh| {
            let ctx = state.get_database(&refresh.database_id)?;
            let expected = match planning {
                RefreshPlanning::Plan => Some(automatic::expected_channels(&ctx, &refresh)),
                RefreshPlanning::CachedOnly => automatic::cached_expected_channels(&refresh),
            };
            // Not planned yet: a refresh due and waiting is listed, since
            // what it restacks cannot be told yet.
            let unknown = expected.is_none() && refresh.waiting;
            let expected = expected.unwrap_or_default();
            let fresh = expected
                .iter()
                .filter(|(key, _)| claimed.insert((refresh.database_id.clone(), key.clone())))
                .count();
            let channels = expected
                .into_iter()
                .map(|(_, name)| name)
                .collect::<Vec<_>>();
            (fresh > 0 || unknown).then_some((refresh, channels))
        })
        .map(|(refresh, channels)| {
            let ctx = state.get_database(&refresh.database_id);
            let project_name = match (&ctx, refresh.project_id) {
                // Not waiting for the shared connection: a busy one leaves
                // the name out, and the row shows the project's number.
                (Some(ctx), Some(project_id)) => ctx.db().try_lock().ok().and_then(|conn| {
                    conn.query_row(
                        "SELECT name FROM project WHERE Id = ?1",
                        [project_id],
                        |row| row.get::<_, String>(0),
                    )
                    .ok()
                }),
                _ => None,
            };
            let waiting_for = refresh
                .waiting
                .then(|| automatic_work_waits_for(state, &refresh.database_id, false))
                .flatten();
            ScheduledRefreshView {
                database_name: ctx
                    .map(|ctx| ctx.name.clone())
                    .unwrap_or_else(|| refresh.database_id.clone()),
                database_id: refresh.database_id,
                project_id: refresh.project_id,
                project_name,
                reason: refresh.reason,
                due_in_seconds: refresh.due_in_seconds,
                channels,
                waiting: refresh.waiting,
                waiting_for,
            }
        })
        .collect()
}

#[derive(Debug, Deserialize)]
pub struct ScheduledRefreshRequest {
    pub database_id: String,
    #[serde(default)]
    pub project_id: Option<i32>,
}

impl ScheduledRefreshRequest {
    fn key(self) -> automatic::RefreshKey {
        automatic::RefreshKey {
            database_id: self.database_id,
            project_id: self.project_id,
        }
    }
}

/// `POST /api/stack-activity/scheduled/skip` — drop a waiting automatic
/// refresh. The next arrival or grade change schedules it again.
pub async fn skip_scheduled_refresh(
    State(state): State<Arc<AppState>>,
    Json(request): Json<ScheduledRefreshRequest>,
) -> Result<Json<ApiResponse<StackActivity>>, AppError> {
    if !state.auto_stacks.skip(&request.key()) {
        return Err(AppError::NotFound);
    }
    Ok(get_stack_activity(State(state)).await)
}

/// `POST /api/stack-activity/scheduled/run-now` — start a waiting automatic
/// refresh at the scheduler's next look, within seconds.
pub async fn run_scheduled_refresh_now(
    State(state): State<Arc<AppState>>,
    Json(request): Json<ScheduledRefreshRequest>,
) -> Result<Json<ApiResponse<StackActivity>>, AppError> {
    if !state.auto_stacks.run_now(&request.key()) {
        return Err(AppError::NotFound);
    }
    Ok(get_stack_activity(State(state)).await)
}

#[derive(Debug, Deserialize)]
pub struct MoveStackJobRequest {
    /// Place in the line of waiting builds, 0 next.
    pub position: usize,
}

/// `POST /api/stack-activity/{job_id}/move` — change when a waiting build runs.
pub async fn move_stack_job(
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<String>,
    Json(request): Json<MoveStackJobRequest>,
) -> Result<Json<ApiResponse<StackActivity>>, AppError> {
    validate_job_id(&job_id)?;
    if !state.stack_previews.move_in_line(&job_id, request.position) {
        return Err(AppError::BadRequest(
            "This build is not waiting; only a waiting build can move".into(),
        ));
    }
    Ok(get_stack_activity(State(state)).await)
}

/// `POST /api/stack-activity/{job_id}/cancel` — stop a mono or color build,
/// or a processing run, from any view, waiting or running.
pub async fn cancel_stack_activity_job(
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<String>,
) -> Result<Json<ApiResponse<StackActivity>>, AppError> {
    validate_job_id(&job_id)?;
    // A running color composition stops at its next channel or step, and a
    // running RC-Astro tool in it is killed.
    // A running stretch or deconvolution has no point to stop at; it
    // finishes in moments. An RC-Astro run stops its tool at once.
    if state.stack_previews.processing_unstoppable(&job_id) {
        return Err(AppError::BadRequest(
            "A running stretch or deconvolution cannot be stopped; it finishes shortly".into(),
        ));
    }
    if !state.stack_previews.request_cancel(&job_id) {
        return Err(AppError::BadRequest(
            "This build has already finished".into(),
        ));
    }
    Ok(get_stack_activity(State(state)).await)
}

/// `GET /api/db/{db}/projects/{project}/stack-previews/wbpp` — the stacks
/// WBPP made for this project and PSF Guard took in.
pub async fn get_wbpp_stacks(
    ctx: DbContext,
    Path((_db_id, project_id)): Path<(String, i32)>,
) -> Json<ApiResponse<LatestStackPreviews>> {
    Json(ApiResponse::success(wbpp_stacks::load_index(
        &ctx, project_id,
    )))
}

#[derive(Debug, Deserialize)]
pub struct ImportWbppStacksRequest {
    /// The target the masters belong to; omitted takes the run's own target,
    /// or the project's only target.
    #[serde(default)]
    pub target_id: Option<i32>,
}

/// `POST /api/db/{db}/projects/{project}/stack-previews/wbpp/import` — take
/// this project's last WBPP run in as stacks. A run that finishes in PSF
/// Guard is taken in on its own; this covers one that finished before this
/// feature or whose import failed. Only the run's own folder is read.
pub async fn import_wbpp_stacks(
    ctx: DbContext,
    Path((_db_id, project_id)): Path<(String, i32)>,
    Json(request): Json<ImportWbppStacksRequest>,
) -> Result<Json<ApiResponse<wbpp_stacks::WbppImport>>, AppError> {
    let (output_dir, last_target) = {
        let store = ctx.wbpp_run.read().unwrap();
        if store.progress.output_dir.is_empty() {
            return Err(AppError::BadRequest(
                "No WBPP run to take stacks from".into(),
            ));
        }
        if store.progress.project_id != Some(project_id) {
            return Err(AppError::BadRequest(
                "The last WBPP run was for another project".into(),
            ));
        }
        (
            std::path::PathBuf::from(&store.progress.output_dir),
            crate::server::wbpp_run::last_target(&store),
        )
    };
    let ctx = Arc::clone(&ctx.0);
    let target_id = request.target_id.or(last_target);
    tokio::task::spawn_blocking(move || {
        let target = {
            let conn = ctx.db();
            let conn = conn.lock().map_err(|error| error.to_string())?;
            wbpp_stacks::run_target(&conn, project_id, target_id)?
        };
        wbpp_stacks::import_masters(&ctx, project_id, target, &output_dir)
    })
    .await
    .map_err(|error| AppError::InternalError(error.to_string()))?
    .map_err(AppError::BadRequest)
    .map(|outcome| Json(ApiResponse::success(outcome)))
}

pub async fn get_latest_stack_previews(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db_id, project_id)): Path<(String, i32)>,
) -> Result<Json<ApiResponse<LatestStackPreviews>>, AppError> {
    let path = latest_path(&ctx.stack_root, project_id);
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
    if latest.database_id != ctx.id || latest.project_id != project_id {
        return Err(AppError::NotFound);
    }
    let latest = current_project_latest_stacks(&ctx, project_id, latest)?;
    let mut latest = latest.for_client();
    // A one-shot-color stack's calibrated preview goes out of date with a
    // new solve or Gaia catalog; a mono stack's never does.
    for entry in &mut latest.groups {
        let group = &mut entry.group;
        group.color_calibration_outdated = preview_calibration_outdated(group, || {
            osc_calibration_source(
                &state,
                &ctx,
                group.reference_image_id,
                group.sky_orientation.clone(),
            )
        });
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
        return Ok(Json(ApiResponse::success(job.for_client())));
    }
    let path = manifest_path(&ctx.stack_root, &job_id);
    let bytes = std::fs::read(path).map_err(|_| AppError::NotFound)?;
    let job: StackPreviewJob = serde_json::from_slice(&bytes)
        .map_err(|error| AppError::InternalError(format!("Invalid stack manifest: {error}")))?;
    if job.database_id != ctx.id || job.project_id != project_id {
        return Err(AppError::NotFound);
    }
    let _ = state.stack_previews.insert(job.clone());
    Ok(Json(ApiResponse::success(job.for_client())))
}

pub async fn get_stack_preview_image(
    ctx: DbContext,
    Path((_db_id, job_id, group_index)): Path<(String, String, usize)>,
    Query(query): Query<StackPreviewImageQuery>,
) -> Result<Response, AppError> {
    validate_job_id(&job_id)?;
    let path = match query.size {
        StackPreviewImageSize::Screen => preview_path(&ctx.stack_root, &job_id, group_index),
        StackPreviewImageSize::Original => {
            original_preview_path(&ctx.stack_root, &job_id, group_index)
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

/// A group's progressive signal-to-noise curve. Served from the sidecar the
/// build wrote, so it survives a server restart and outlives the in-memory
/// job list.
pub async fn get_stack_preview_snr(
    ctx: DbContext,
    Path((_db_id, job_id, group_index)): Path<(String, String, usize)>,
) -> Result<Response, AppError> {
    validate_job_id(&job_id)?;
    let path = snr_path(&ctx.stack_root, &job_id, group_index);
    let body = tokio::fs::read(&path)
        .await
        .map_err(|_| AppError::NotFound)?;
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "application/json")
        .header(CONTENT_LENGTH, body.len())
        .header(CACHE_CONTROL, "private, max-age=31536000, immutable")
        .body(Body::from(body))
        .map_err(|error| {
            AppError::InternalError(format!("Failed to build stack SNR response: {error}"))
        })
}

pub async fn download_stack_preview_fits(
    ctx: DbContext,
    Path((_db_id, job_id, group_index)): Path<(String, String, usize)>,
) -> Result<Response, AppError> {
    validate_job_id(&job_id)?;
    let path = fits_path(&ctx.stack_root, &job_id, group_index);
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
    Query(query): Query<StackProcessingSelectionQuery>,
    Json(request): Json<stretch::StackViewProcessingRequest>,
) -> Result<axum::response::Response, AppError> {
    let source = stack_processing_source(&state, &ctx, &job_id, group_index)?;
    validate_processing_revision(query.v.as_deref(), &source.revision)?;
    let result = stretch::apply_to_fits(
        Arc::clone(&state),
        ctx.id.clone(),
        ctx.stack_root.clone(),
        source,
        request,
        !query.poll,
    )
    .await?;
    Ok(stretch::apply_response(result, &state.stack_previews))
}

#[derive(Debug, Default, Deserialize)]
pub struct StackProcessingSelectionQuery {
    pub v: Option<String>,
    #[serde(default)]
    pub poll: bool,
}

pub async fn get_stack_preview_processing(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db_id, job_id, group_index)): Path<(String, String, usize)>,
    Query(query): Query<StackProcessingSelectionQuery>,
) -> Result<Json<ApiResponse<Option<stretch::StackStretchPreview>>>, AppError> {
    let source = stack_processing_source(&state, &ctx, &job_id, group_index)?;
    validate_processing_revision(query.v.as_deref(), &source.revision)?;
    let mut preview = stretch::selected_or_carried(
        &state.stack_previews,
        &ctx.stack_root,
        &source.key,
        &source.revision,
    )?;
    // A one-shot-color stack's processing goes out of date with a new solve
    // or Gaia catalog, as its build preview does.
    if let (Some(preview), Some(current)) = (preview.as_mut(), source.calibration.as_ref()) {
        preview.color_calibration_outdated = color_calibration::calibration_outdated(
            preview.color_calibration_basis.as_ref(),
            preview.color_calibration.as_ref(),
            current,
        )
        .map(|reason| format!("{reason}; apply the processing again"));
    }
    Ok(Json(ApiResponse::success(preview)))
}

pub async fn clear_stack_preview_processing(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db_id, job_id, group_index)): Path<(String, String, usize)>,
    Query(query): Query<StackProcessingSelectionQuery>,
) -> Result<StatusCode, AppError> {
    let source = stack_processing_source(&state, &ctx, &job_id, group_index)?;
    validate_processing_revision(query.v.as_deref(), &source.revision)?;
    // Reverting also stops processing the server was about to re-apply.
    reapply::replaced_by_person(&state, &source, None);
    stretch::clear_selected_processing(&ctx.stack_root, &source.key, &source.revision)?;
    Ok(StatusCode::NO_CONTENT)
}

fn validate_processing_revision(expected: Option<&str>, actual: &str) -> Result<(), AppError> {
    if expected.is_some_and(|expected| expected != actual) {
        return Err(AppError::Conflict(
            "The stack was rebuilt; refresh its processing selection".into(),
        ));
    }
    Ok(())
}

/// Finished, catalog-owned artifacts; never resolve a caller-supplied path.
pub(crate) fn contribution_outputs(
    ctx: &DatabaseContext,
    project: i32,
    target: i32,
) -> Vec<(LatestStackPreviewGroup, PathBuf)> {
    let mut groups = std::fs::read(latest_path(&ctx.stack_root, project))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<LatestStackPreviews>(&bytes).ok())
        .filter(|i| i.database_id == ctx.id && i.project_id == project)
        .and_then(|i| current_project_latest_stacks(ctx, project, i).ok())
        .map(|i| i.groups)
        .unwrap_or_default();
    groups.extend(wbpp_stacks::load_index(ctx, project).groups);
    groups
        .into_iter()
        .filter(|g| g.group.target_id == target && g.group.state == StackGroupState::Ready)
        .filter_map(|mut g| {
            if validate_job_id(&g.job_id).is_err() {
                return None;
            }
            let path = fits_path(&ctx.stack_root, &g.job_id, g.group.index);
            g.group.strip_for_client();
            path.is_file().then_some((g, path))
        })
        .collect()
}

fn stack_processing_source(
    state: &Arc<AppState>,
    ctx: &DatabaseContext,
    job_id: &str,
    group_index: usize,
) -> Result<stretch::StretchSource, AppError> {
    validate_job_id(job_id)?;
    let job = if let Some(job) = state.stack_previews.get(job_id) {
        job
    } else {
        let bytes = std::fs::read(manifest_path(&ctx.stack_root, job_id))
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
    Ok(stretch::StretchSource {
        key: format!("mono:{job_id}:{}", group.index),
        revision: job.artifact_revision,
        path: fits_path(&ctx.stack_root, job_id, group.index),
        scope: processing_queue::ProcessingScope {
            database_id: ctx.id.clone(),
            project_id: job.project_id,
            target_id: group.target_id,
            target_name: group.target_name.clone(),
            channel: group.filter_name.clone(),
            exposure: group
                .exposure_group
                .as_ref()
                .map(|exposure| exposure.label.clone()),
        },
        job_id: job_id.to_string(),
        group_index: group.index,
        calibration: (group.output_channels == 3).then(|| {
            osc_calibration_source(
                state,
                ctx,
                group.reference_image_id,
                group.sky_orientation.clone(),
            )
        }),
    })
}

/// Why a stack's calibrated card preview is out of date, if it is. Only a
/// one-shot-color stack whose build calibrated, or tried to, can be: a mono
/// stack never is, and `current` is not read for one.
fn preview_calibration_outdated(
    group: &StackGroupStatus,
    current: impl FnOnce() -> color_calibration::StackCalibrationSource,
) -> Option<String> {
    if group.output_channels != 3 || group.color_calibration.is_none() {
        return None;
    }
    color_calibration::calibration_outdated(
        group.color_calibration_basis.as_ref(),
        group.color_calibration.as_ref(),
        &current(),
    )
    .map(|reason| format!("{reason}; click Build current set to render the preview again"))
}

/// What color calibration of a one-shot-color stack can use: its reference
/// frame's fresh pixel solve, its layout, and the installed Gaia catalog.
fn osc_calibration_source(
    state: &AppState,
    ctx: &DatabaseContext,
    reference_image_id: Option<i32>,
    orientation: Option<StackSkyOrientation>,
) -> color_calibration::StackCalibrationSource {
    color_calibration::StackCalibrationSource {
        reference_image_id,
        orientation,
        solution: reference_image_id
            .and_then(|image_id| {
                ctx.astrometry_evidence
                    .evidence_for_source(&ctx.cache_dir_path, image_id, None)
            })
            .and_then(|analysis| analysis.solution),
        catalog: state.astrometry.gaia_photometry_signature(),
    }
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

fn hex_digest(hasher: Sha256) -> String {
    let mut text = String::with_capacity(64);
    for byte in hasher.finalize() {
        write!(&mut text, "{byte:02x}").expect("writing to a String cannot fail");
    }
    text
}

/// Log once, as a group starts, the frames left out before stacking and
/// why. Preparation runs on every preview request, so it cannot log them
/// without repeating itself.
fn log_excluded_frames(state: &Arc<AppState>, job_id: &str, group_index: usize) {
    let Some(job) = state.stack_previews.get(job_id) else {
        return;
    };
    let Some(group) = job.groups.get(group_index) else {
        return;
    };
    let excluded: Vec<_> = group
        .frames
        .iter()
        .filter(|frame| frame.disposition == "excluded")
        .collect();
    if excluded.is_empty() {
        return;
    }
    tracing::info!(
        job_id,
        group_index,
        target = %group.target_name,
        filter = %group.filter_name,
        "Stack left out {} of {} frames before stacking",
        excluded.len(),
        group.total_candidates
    );
    for frame in excluded {
        tracing::info!(
            job_id,
            group_index,
            image_id = frame.image_id,
            "Stack left out a frame: {}",
            frame.reason.as_deref().unwrap_or("no reason recorded")
        );
    }
}

fn excluded_decision(
    image: &AcquiredImage,
    scored: &ImageQualityResult,
    reason: String,
) -> StackFrameDecision {
    StackFrameDecision {
        quality_score: Some(scored.quality_score),
        ..excluded_decision_for(image.id, reason)
    }
}

fn excluded_decision_for(image_id: i32, reason: String) -> StackFrameDecision {
    StackFrameDecision {
        image_id,
        disposition: "excluded".into(),
        reason: Some(reason),
        quality_score: None,
        matched_stars: None,
        registration_rms_pixels: None,
        registration_drift_pixels: None,
        registered_mapping: None,
        normalization_mean_gain: None,
        normalization_mean_offset: None,
        source_fingerprint: None,
        overlap_fraction: None,
        integrated_fraction: None,
        noise_sigma: None,
        integration_weight: None,
    }
}

pub(crate) fn source_fingerprint(path: &FsPath) -> String {
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

/// Queue a previewed build: at the back of the line, or in the place
/// `slot` held while the request it came from was previewed. The line
/// carries `request`, the build's own channel request, and the build is
/// prepared again from it at its turn; `prepared` is the preview, which
/// gives the build the keys it holds while it waits.
fn enqueue_job(
    state: Arc<AppState>,
    prepared: PreparedJob,
    request: &StackPreviewRequest,
    origin: journal::JournaledStackJob,
    slot: Option<&str>,
) {
    // While it waits and while it runs, compositions of its targets wait.
    let mut targets = prepared
        .public
        .groups
        .iter()
        .map(|group| group.target_id)
        .collect::<Vec<_>>();
    targets.sort_unstable();
    targets.dedup();
    let database_id = prepared.public.database_id.clone();
    let holds = targets
        .into_iter()
        .map(|target_id| channel_key(&database_id, prepared.public.project_id, target_id))
        .collect();
    let spec = BuildRequest {
        job_id: prepared.public.job_id.clone(),
        database_id,
        project_id: prepared.public.project_id,
        request: request.clone(),
    };
    submit_build(state, spec, holds, origin, prepared.public.automatic, slot);
}

/// Put a build in the line from its request. An automatic build that a
/// person's work stops goes back in the line as its request, and resumes
/// from its checkpoint at its turn.
fn submit_build(
    state: Arc<AppState>,
    spec: BuildRequest,
    holds: Vec<String>,
    origin: journal::JournaledStackJob,
    automatic: bool,
    slot: Option<&str>,
) {
    let job_id = spec.job_id.clone();
    let cancel = state.stack_previews.track_job(&job_id);
    state
        .stack_previews
        .remember_origin(&job_id, origin.clone());
    // A stop waits for the build to checkpoint, or to leave the line.
    let work = state.shutdown.hold(format!(
        "stack build for project {} (db={})",
        spec.project_id, spec.database_id
    ));
    let (stopped_job, failed_job) = (job_id.clone(), job_id.clone());
    let again = (spec.clone(), holds.clone(), origin);
    let submission = Submission::<BuildWork>::new(
        "a stack build",
        &spec,
        move |state, built| match built {
            Ok(Some(output)) => publish_build(state, output),
            // Prepared at its turn into another build, or failed there.
            Ok(None) => {}
            Err(error) => state.stack_previews.update(&failed_job, |job| {
                job.state = StackJobState::Failed;
                job.error = Some(format!("Stack worker failed: {error}"));
            }),
        },
        move |state| {
            // Stopped while it waited: the work never starts.
            state.stack_previews.update(&stopped_job, |job| {
                cancel_unfinished_groups(job);
                job.state = StackJobState::Cancelled;
            });
        },
    )
    .map(|submission| {
        let (job_id, cancel) = (job_id.clone(), cancel.clone());
        // A stop that lands as its turn comes still keeps it from starting.
        let start_stop = cancel.clone();
        submission
            .starting_with(move |_| !start_stop.requested())
            .finally(move |state, ending| {
                if let crate::server::job_line::Ending::Panicked(error) = ending {
                    state.stack_previews.update(&job_id, |job| {
                        job.state = StackJobState::Failed;
                        job.error = Some(format!("Stack worker panicked: {error}"));
                    });
                }
                state.stack_previews.forget_job_of(&job_id, &cancel);
                drop(work);
                if state.stack_previews.line.take_preempted(&job_id)
                    && requeue_stopped_build(state, &job_id)
                {
                    let (spec, holds, origin) = again;
                    let first = state.stack_previews.line.first_automatic();
                    submit_build(
                        Arc::clone(state),
                        spec,
                        holds,
                        origin,
                        true,
                        first.as_deref(),
                    );
                }
            })
    });
    match submission {
        Ok(submission) => submit(
            &state,
            &job_id,
            Lane::Main,
            slot,
            cancel,
            holds,
            automatic,
            Box::new(submission),
        ),
        Err(error) => {
            state.stack_previews.update(&job_id, |job| {
                job.state = StackJobState::Failed;
                job.error = Some(error);
            });
            state.stack_previews.forget_job_of(&job_id, &cancel);
        }
    }
}

/// Make a build that a person's work stopped queued again, with its
/// stopped channels waiting once more. False when it did not stop, such as
/// one that finished before the stop reached it: it is not queued again.
fn requeue_stopped_build(state: &AppState, job_id: &str) -> bool {
    let mut requeued = false;
    state.stack_previews.update(job_id, |job| {
        if job.state != StackJobState::Cancelled {
            return;
        }
        job.state = StackJobState::Queued;
        for group in &mut job.groups {
            if group.state == StackGroupState::Cancelled {
                group.state = StackGroupState::Queued;
                group.phase = "queued".into();
            }
        }
        requeued = true;
    });
    requeued
}

/// What a build holds in the line, waiting or running, for each target it
/// stacks: a color composition of that target waits until nothing does.
pub(super) fn channel_key(database_id: &str, project_id: i32, target_id: i32) -> String {
    format!("channels:{database_id}:{project_id}:{target_id}")
}

/// What a build still being prepared holds: its targets are not known
/// yet, so it holds every target of its project.
pub(super) fn project_channels_key(database_id: &str, project_id: i32) -> String {
    format!("channels:{database_id}:{project_id}:*")
}

/// Put work in the line under `job_id`, stopped by `cancel`: on the worker
/// lane in the place `before` held, or at the back. The lane's dispatcher
/// runs it in its turn.
#[allow(clippy::too_many_arguments)]
pub(super) fn submit(
    state: &Arc<AppState>,
    job_id: &str,
    lane: Lane,
    before: Option<&str>,
    cancel: Stop,
    holds: Vec<String>,
    automatic: bool,
    task: Box<dyn Task>,
) {
    submit_entry(
        state,
        Entry::new(job_id, lane, cancel, task)
            .holding(holds)
            .tier(tier_of(automatic)),
        before,
    );
}

/// The line's tier for work the automatic refresh started, or a person's.
pub(super) fn tier_of(automatic: bool) -> Tier {
    if automatic {
        Tier::Automatic
    } else {
        Tier::Person
    }
}

/// Put `entry` in the line, as [`submit`] does.
pub(super) fn submit_entry(state: &Arc<AppState>, entry: Entry, before: Option<&str>) {
    ensure_dispatching(state);
    let job_id = entry.id().to_string();
    if let Err(entry) = state.stack_previews.line.submit(entry, before) {
        tracing::warn!(
            job_id,
            "The same work is already in line; this copy is dropped"
        );
        drop(entry);
    }
}

/// How the stack line runs its entries. A person's work runs at a higher
/// priority than pre-generation and quality scans, and pauses background
/// work while it runs.
static STACK_LINE: DispatchRules = DispatchRules {
    name: "stack",
    line: |state| &state.stack_previews.line,
    lanes: &[Lane::Main, Lane::Quick],
    turn: |state, id| {
        let automatic = state.stack_previews.is_automatic(id)
            || state.stack_previews.is_automatic_processing(id);
        Turn {
            priority: job_priority(automatic),
            pauses_background_work: !automatic,
        }
    },
    // A person's work anywhere on the server comes first. A person's next
    // build that joins after automatic work started stops it, and the
    // automatic build resumes from its checkpoint.
    automatic_may_start: |state| !state.interactive_job_active(),
};

/// Start each lane's dispatcher if it is not running.
fn ensure_dispatching(state: &Arc<AppState>) {
    state
        .stack_previews
        .line
        .ensure_dispatching(state, &STACK_LINE);
}

/// A stack build on the line.
struct BuildWork;

impl LineWork for BuildWork {
    type Spec = BuildRequest;
    /// `None` when the build was prepared at its turn into another build,
    /// or could not be prepared.
    type Output = Option<BuildOutput>;
    /// It reads this server's catalog to prepare the build at its turn,
    /// opens the calibration catalog to choose and build masters, and
    /// reads its WCS and score caches.
    const NEEDS: Needs = Needs::ThisServer;

    fn execute(spec: BuildRequest, worker: &Worker<'_>) -> Option<BuildOutput> {
        let prepared = prepare_at_turn(&spec, worker.server())?;
        Some(execute_build(prepared, worker))
    }
}

/// A build in the line: its channel request, prepared at its turn.
#[derive(Clone, Serialize, Deserialize)]
struct BuildRequest {
    /// The id the build was listed under when it was previewed.
    job_id: String,
    database_id: String,
    project_id: i32,
    request: StackPreviewRequest,
}

/// Prepare a build again from its request, as the catalog is now. Returns
/// the prepared build when it is the one that was previewed. When frames,
/// grades or calibration changed while it waited, the request becomes
/// another build: that build is queued at the front of the line, or a
/// matching finished build is reused, and the previewed record points to
/// it. A build that cannot be prepared fails with the reason.
fn prepare_at_turn(spec: &BuildRequest, state: &Arc<AppState>) -> Option<PreparedJob> {
    let fail = |error: String| {
        state.stack_previews.update(&spec.job_id, |job| {
            cancel_unfinished_groups(job);
            job.state = StackJobState::Failed;
            job.error = Some(error);
        });
    };
    let Some(ctx) = state.get_database(&spec.database_id) else {
        fail("The database was removed while the build waited".into());
        return None;
    };
    // It holds the lane from here: listed as running, not as queued.
    state.stack_previews.update(&spec.job_id, |job| {
        job.state = StackJobState::Running;
    });
    let prepared = match build_set::prepare_job(&ctx, spec.project_id, &spec.request) {
        Ok(prepared) => prepared,
        Err(error) => {
            fail(error.message());
            return None;
        }
    };
    if prepared.public.job_id == spec.job_id {
        return Some(prepared);
    }
    let automatic = state.stack_previews.is_automatic(&spec.job_id);
    let placement = build_set::Placement {
        automatic,
        slot: None,
    };
    match build_set::start_prepared(
        state,
        &ctx,
        spec.project_id,
        &spec.request,
        prepared,
        placement,
    ) {
        Ok(started) => {
            let became = started.job.job_id.clone();
            tracing::info!(
                job_id = spec.job_id,
                became,
                "A queued build's frames changed while it waited; it runs as a new build"
            );
            // It keeps the turn the previewed build had.
            state.stack_previews.move_in_line(&became, 0);
            state.stack_previews.update(&spec.job_id, |job| {
                job.state = StackJobState::Replaced;
                job.replaced_by = vec![became.clone()];
            });
            if let Some(cached) = started.cached {
                automatic::compose_colors_after(state, &ctx, &cached);
            }
        }
        Err(error) => fail(error.message()),
    }
    None
}

/// Where a finished build's outputs are, for publishing.
struct BuildOutput {
    job_id: String,
    database_id: String,
    stack_root: PathBuf,
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
    peers: &[PreparedFrame],
) -> HashMap<usize, Option<(bool, &'static str)>> {
    let Some(ctx) = state.get_database(database_id) else {
        return HashMap::new();
    };
    let orient = |reference: &PreparedFrame| {
        let headers = crate::image_io::read_header(&reference.path).unwrap_or_default();
        ReferenceOrientation {
            north_up: cached_or_embedded_wcs(&ctx, reference, &headers)
                .and_then(|(wcs, _)| faces_north_up(wcs.cd)),
            west_of_pier: pier_side_from_headers(&headers).and_then(is_west_of_pier),
        }
    };
    let mut orientations = Vec::new();
    for group in groups {
        let Some(reference) = group.frames.first() else {
            continue;
        };
        if group.frames.len() < 2 {
            continue;
        }
        orientations.push((group.index, orient(reference)));
    }
    // Sibling channels stacked in their own jobs vote on the mapping too,
    // as they would have in one job.
    let calibration = calibrate_pier_side(
        &orientations
            .iter()
            .map(|(_, orientation)| *orientation)
            .chain(peers.iter().map(orient))
            .collect::<Vec<_>>(),
    );
    orientations
        .into_iter()
        .map(|(index, orientation)| (index, anchored_north_up(orientation, calibration)))
        .collect()
}

/// A frame's field rotation, from its cached plate solve or, failing that,
/// the WCS in its header. Only the reference choice reads it; registration
/// still measures every frame from its pixels.
fn solved_orientation(
    ctx: &DatabaseContext,
    frame: &PreparedFrame,
) -> Option<reference::Orientation> {
    let (wcs, _) = cached_or_embedded_wcs(ctx, frame, &[]).or_else(|| {
        let headers = crate::image_io::read_header(&frame.path).ok()?;
        cached_or_embedded_wcs(ctx, frame, &headers)
    })?;
    crate::sequence_analysis::field_rotation_from_cd(wcs.cd)
}

/// Put Seiza's choice of reference at the front of every group that will
/// stack, taken from the largest orientation group when the frames' plate
/// solves show more than one. Scores come from the cache where they can; a
/// group where no frame scores keeps its best-graded reference and says so.
#[allow(clippy::too_many_arguments)]
fn choose_references(
    state: &Arc<AppState>,
    database_id: &str,
    job_id: &str,
    stack_root: &FsPath,
    groups: &mut [PreparedGroup],
    order: snr::StackFrameOrder,
    rotation_margin_degrees: f64,
    worker_policy: &crate::concurrency::WorkerPolicy,
    cancel: &Stop,
) {
    let ctx = state.get_database(database_id);
    for group in groups.iter_mut().filter(|group| group.frames.len() >= 2) {
        if cancel.requested() {
            return;
        }
        let priority = job_priority(state.stack_previews.is_automatic(job_id));
        let budget = crate::concurrency::plan_workers(
            None,
            worker_policy,
            priority,
            crate::concurrency::probe_frame_pixels(&group.frames[0].path),
        );
        let lease = state.lease_workers(priority, budget.workers);
        let pool = match crate::concurrency::ComputePool::take("stack-reference", lease.workers) {
            Ok(pool) => pool,
            Err(error) => {
                tracing::warn!("Reference scoring pool: {error}");
                continue;
            }
        };
        let index = group.index;
        let Some(scores) =
            reference::scores(&group.frames, stack_root, &pool, cancel, |done, count| {
                state.stack_previews.update(job_id, |job| {
                    job.groups[index].phase = format!("Choosing a reference: frame {done}/{count}");
                });
            })
        else {
            return;
        };
        let oriented = ctx.as_deref().and_then(|ctx| {
            let orientations = group
                .frames
                .iter()
                .map(|frame| solved_orientation(ctx, frame))
                .collect::<Vec<_>>();
            let solved = orientations.iter().flatten().count();
            reference::orientation_candidates(&orientations, rotation_margin_degrees)
                .map(|(candidates, kept)| (candidates, kept, solved))
        });
        let chosen = match &oriented {
            Some((candidates, _, _)) => reference::choose_among(&scores, candidates),
            None => reference::choose(&scores),
        };
        match chosen {
            Some(chosen) => {
                let image_id = group.frames[chosen].image_id;
                let narrowed = oriented
                    .filter(|(candidates, _, _)| candidates[chosen])
                    .map(|(_, kept, solved)| {
                        format!(
                            "The reference comes from the largest orientation group, which keeps {kept} of {solved} plate-solved frames within the rotation limit"
                        )
                    });
                reference::reorder(&mut group.frames, chosen, order);
                state.stack_previews.update(job_id, |job| {
                    job.groups[index].reference_image_id = Some(image_id);
                    if let Some(narrowed) = &narrowed {
                        add_note(&mut job.groups[index].resume_note, narrowed);
                    }
                });
            }
            None => state.stack_previews.update(job_id, |job| {
                add_note(
                    &mut job.groups[index].resume_note,
                    "No frame could be scored as a reference; the best-graded frame is the reference",
                );
            }),
        }
    }
}

/// Add a sentence to a card's note without losing what is already there.
fn add_note(note: &mut Option<String>, sentence: &str) {
    *note = Some(match note.take() {
        Some(previous) if previous.contains(sentence) => previous,
        Some(previous) => format!("{previous}. {sentence}"),
        None => sentence.to_string(),
    });
}

/// The core share a build may use: a person's build takes the interactive
/// share, and an automatic refresh the background share, like the other
/// work nobody is waiting on.
pub(super) fn job_priority(automatic: bool) -> crate::concurrency::Priority {
    if automatic {
        crate::concurrency::Priority::Background
    } else {
        crate::concurrency::Priority::Interactive
    }
}

/// Stack each channel of a prepared build, recording how each one went.
fn execute_build(prepared: PreparedJob, worker: &Worker<'_>) -> BuildOutput {
    let state = worker.server();
    let cancel = worker.stop_flag();
    let job_id = prepared.public.job_id.clone();
    let database_id = prepared.public.database_id.clone();
    let accepted_only = prepared.public.accepted_only;
    let scoring = prepared.public.scoring;
    // Every job this process prepares records its method.
    let method = prepared.public.method.unwrap_or_default();
    let PreparedJob {
        public: _,
        mut groups,
        orientation_peers,
        stack_root,
        calibration_root,
        north_up,
        order,
    } = prepared;
    state.stack_previews.update(&job_id, |job| {
        job.state = StackJobState::Running;
    });
    let worker_policy = state.worker_policy();
    let group_job = GroupJob {
        database_id: &database_id,
        job_id: &job_id,
        stack_root: &stack_root,
        calibration_root: &calibration_root,
        north_up,
        accepted_only,
        scoring,
        method,
        order,
        worker_policy: &worker_policy,
        cancel,
    };
    let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // The reference decides the anchors below, so it is chosen first. A
        // stop while scoring falls through to the loop, which sees it.
        if method.reference == StackReference::Auto {
            choose_references(
                state,
                &database_id,
                &job_id,
                &stack_root,
                &mut groups,
                order,
                method.reference_rotation_margin_degrees,
                &worker_policy,
                cancel,
            );
        }
        // Read every reference frame's orientation before the first stack is
        // built. A channel that was never solved has to borrow the pier-to-sky
        // mapping from one that was, and it cannot do that from a decision taken
        // after its own. Headers only, so this costs no pixel reads.
        let anchors = reference_anchors(state, &database_id, &groups, &orientation_peers);
        for group in groups {
            if group.frames.len() < 2 {
                continue;
            }
            if cancel.requested() {
                return;
            }
            state.stack_previews.update(&job_id, |job| {
                job.groups[group.index].state = StackGroupState::Running;
                job.groups[group.index].phase = "calibration".into();
            });
            let anchor = anchors.get(&group.index).copied().flatten();
            let result = run_group(state, &group_job, group.clone(), anchor);
            state.stack_previews.update(&job_id, |job| {
                // However the channel ended, it is no longer building masters.
                job.groups[group.index].calibration_progress = None;
            });
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
    let cancelled = cancel.requested();
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
    BuildOutput {
        job_id,
        database_id,
        stack_root,
    }
}

/// Take a finished build in: record it, point the cards at what it built,
/// and start what follows it.
fn publish_build(state: &Arc<AppState>, output: BuildOutput) {
    let BuildOutput {
        job_id,
        database_id,
        stack_root,
    } = output;
    if let Some(job) = state.stack_previews.get(&job_id)
        && let Err(error) = persist_manifest(&stack_root, &job)
    {
        tracing::warn!("Failed to persist stack preview manifest: {error}");
    }
    // A cancelled job still remembers the channels it finished before the
    // stop: those artifacts are complete and are what the user asked for.
    // An automatic build that was stopped for a user's build keeps its
    // artifacts for its own next run but leaves the cards alone: a color
    // composition the user queued against the current cards must still
    // find them current when its turn comes.
    if let Some(job) = state.stack_previews.get(&job_id)
        && (job.state == StackJobState::Completed
            || (job.state == StackJobState::Cancelled && !job.automatic))
        && let Some(ctx) = state.get_database(&database_id)
        // A stopped build's finished channels are not the person's to
        // process now: Stop all must not start RC-Astro.
        && let Err(error) = persist_latest_and_reapply(
            state,
            &ctx,
            &job,
            job.state == StackJobState::Completed && !job.automatic,
        )
    {
        tracing::warn!("Failed to persist latest stack preview index: {error}");
    }
    // Color follows the channels: once no build is left for a target, its
    // color previews are composed again from the newest channel stacks.
    // A failed last channel still lets the channels before it reach their
    // color previews. A stopped one does not: Stop all must not start color
    // work, and the next build or a cache hit composes what is due.
    if let Some(job) = state.stack_previews.get(&job_id)
        && matches!(job.state, StackJobState::Completed | StackJobState::Failed)
        && let Some(ctx) = state.get_database(&database_id)
    {
        automatic::compose_colors_after(state, &ctx, &job);
    }
    state.stack_previews.prune_cache(&stack_root);
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
    stack_root: &'a FsPath,
    calibration_root: &'a FsPath,
    north_up: bool,
    accepted_only: bool,
    scoring: StackScoringSettings,
    method: StackMethod,
    order: snr::StackFrameOrder,
    worker_policy: &'a crate::concurrency::WorkerPolicy,
    cancel: &'a Stop,
}

fn run_group(
    state: &Arc<AppState>,
    job: &GroupJob<'_>,
    group: PreparedGroup,
    anchor: Option<(bool, &'static str)>,
) -> Result<GroupOutcome, String> {
    use seiza_stacking::{FrameDisposition, LiveStacker};

    let &GroupJob {
        database_id,
        job_id,
        stack_root,
        calibration_root,
        north_up,
        accepted_only,
        scoring,
        method,
        order,
        worker_policy,
        cancel,
    } = job;
    // Read for each channel: a build adopted part way takes the person's
    // share from its next channel on.
    let priority = job_priority(state.stack_previews.is_automatic(job_id));
    let weighting = method.weighting;
    let available_memory = crate::concurrency::available_memory_bytes();
    let reference_path = &group
        .frames
        .first()
        .ok_or_else(|| "Stack group has no input frames".to_string())?
        .path;
    let budget = crate::concurrency::plan_workers(
        None,
        worker_policy,
        priority,
        crate::concurrency::probe_frame_pixels(reference_path),
    );
    // Held until the group is built: its workers come out of the budget the
    // other jobs of this priority share.
    let lease = state.lease_workers(priority, budget.workers);
    let threads = execution::ThreadBudget::from_total(lease.workers);
    let pool = crate::concurrency::ComputePool::take("stack-preview", lease.workers)
        .map_err(|error| error.to_string())?;
    tracing::info!(
        job_id,
        group_index = group.index,
        total_worker_budget = budget.workers,
        phase_workers = lease.workers,
        compute_workers = threads.compute_workers,
        preparation_workers = threads.preparation_workers,
        serial = threads.serial,
        rationale = %budget.rationale,
        shared = %lease.summary(),
        "Stack worker budget configured"
    );
    log_excluded_frames(state, job_id, group.index);
    let ctx = state
        .get_database(database_id)
        .ok_or_else(|| format!("Database {database_id} is no longer configured"))?;
    // Through the shared opener, so this connection gets the same busy
    // timeout as every other and the same in-place upgrade of PSF Guard's
    // tables. Opening it raw is how a stack came to read a catalog whose
    // calibration tables predated a column the query names.
    let calibration_conn =
        crate::server::database_context::open_scheduler_connection(&ctx.database_path)
            .map_err(|error| format!("Opening calibration catalog: {error}"))?;
    let directory_tree = ctx
        .get_directory_tree()
        .map_err(|error| format!("Indexing calibration folders: {error}"))?;
    let light_paths = group
        .frames
        .iter()
        .map(|frame| frame.path.clone())
        .collect::<Vec<_>>();
    // Building a night of masters is minutes of work, so the stop flag reaches
    // between them too. The plan partitions a multi-night group into
    // sessions, each with its own masters; a single-night group gets one
    // session and stacks exactly as before.
    let calibration_mode = group.calibration;
    let flat_star_masking = group.flat_star_masking;
    // Each frame a master build reads moves the card, and a stop reaches
    // into the build instead of waiting for it.
    let watch = {
        let state = Arc::clone(state);
        let job_id = job_id.to_string();
        let index = group.index;
        crate::calibration::MasterBuildWatch {
            report: Some(Arc::new(move |progress| {
                state.stack_previews.update(&job_id, |job| {
                    job.groups[index].calibration_progress = Some(progress);
                });
            })),
            stop: Some(cancel.clone()),
            ..Default::default()
        }
    };
    let calibration_started = std::time::Instant::now();
    let plan = pool.install(move || {
        crate::calibration::resolve_or_build_master_plan_with_options(
            &calibration_conn,
            calibration_root,
            &light_paths,
            Some(&directory_tree),
            Some(cancel.as_flag()),
            crate::calibration::CalibrationPlanOptions {
                mode: calibration_mode,
                pinned: &[],
                flat_star_masking,
                watch,
            },
        )
    });
    tracing::info!(
        job_id,
        group_index = group.index,
        elapsed_seconds = calibration_started.elapsed().as_secs_f64(),
        "Stack calibration plan finished"
    );
    if cancel.requested() {
        return Ok(GroupOutcome::Cancelled);
    }
    // `{:#}` keeps the whole anyhow chain: "building master flat: <why>".
    // `to_string()` printed only the outermost context, which reported a
    // failed master build with no cause at all.
    //
    // Auto mode's contract reaches here too: a plan that cannot be built —
    // an unreadable cached master, a selection query refused — is a reason
    // to stack raw and say why, never to abandon the group. The cancel
    // check above already returned, so this cannot swallow a stop. Forced
    // calibration keeps the hard error.
    let plan = match plan {
        Ok(plan) => plan,
        Err(error) if group.calibration != crate::calibration::CalibrationMode::On => {
            tracing::warn!("calibration plan failed; stacking raw: {error:#}");
            let mut plan =
                crate::calibration::CalibrationPlan::without_calibration(group.frames.len());
            plan.applied.warning = Some(format!(
                "Stacked without calibration: the calibration plan could not be built — \
                 {error:#}"
            ));
            plan
        }
        Err(error) => {
            tracing::warn!("Stack group calibration failed: {error:#}");
            return Err(format!("{error:#}"));
        }
    };
    let mut applied_calibration = plan.applied.clone();
    // A session without a dark master keeps its lights' hot pixels, and a
    // handful of frames cannot reject them statistically, so the spatial
    // impulse filter runs over that session's frames. The live stacker
    // fixes its filter at construction, so the online pass filters only
    // when no session has a dark; the final integration, which produces
    // the stack, decides per session. Say so on the card; display only,
    // after the identity is computed.
    let session_cosmetics = SessionCosmetics::for_plan(&plan, group.calibration);
    let cosmetic = session_cosmetics.whole_group();
    if let Some(note) = session_cosmetics.card_note() {
        applied_calibration.warning = Some(match applied_calibration.warning.take() {
            Some(previous) => format!("{previous}. {note}"),
            None => note,
        });
    }
    // The resume checkpoint key carries the applied-master signature too:
    // the selection fingerprint is computed before any build, so after a
    // transient build failure a later run with the same selection could
    // otherwise resume an accumulator whose earlier frames integrated
    // under different masters — one stack mixing two calibrations.
    let calibration_fingerprint = format!(
        "{}|{}",
        applied_calibration.fingerprint, applied_calibration.masters_signature
    );
    state.stack_previews.update(job_id, |job| {
        let status = &mut job.groups[group.index];
        status.calibration = applied_calibration;
        status.calibration_progress = None;
        status.phase = "stacking".into();
    });
    let (group_target_id, group_filter_name, group_exposure_key) = state
        .stack_previews
        .get(job_id)
        .map(|job| {
            let status = &job.groups[group.index];
            (
                status.target_id,
                status.filter_name.clone(),
                exposure_group_key(status).map(str::to_owned),
            )
        })
        .ok_or_else(|| "Stack job disappeared while running".to_string())?;
    let requested = group
        .frames
        .iter()
        .map(|frame| {
            (
                frame.image_id,
                frame.source_fingerprint.as_str(),
                frame.exposure_seconds,
            )
        })
        .collect::<Vec<_>>();
    let decision = resume::load(
        stack_root,
        database_id,
        group_target_id,
        &group_filter_name,
        group_exposure_key.as_deref(),
        accepted_only,
        scoring,
        method,
        SEIZA_STACKING_OUTPUT_VERSION,
        &calibration_fingerprint,
        order,
        &requested,
    );
    if let Some(reason) = decision.fresh_reason() {
        tracing::info!(
            target_id = group_target_id,
            filter = %group_filter_name,
            "Full restack: {reason}"
        );
        resume::discard_unusable(
            &decision,
            order,
            stack_root,
            database_id,
            group_target_id,
            &group_filter_name,
            group_exposure_key.as_deref(),
        );
        state.stack_previews.update(job_id, |job| {
            add_note(
                &mut job.groups[group.index].resume_note,
                &format!("Full restack: {reason}"),
            );
        });
    }
    let checkpoint = decision.state();
    let reference_frame = crate::image_io::open_linear_frame(&group.frames[0].path)
        .map_err(|error| error.to_string())?;
    let reference_is_bayer = reference_frame.bayer.is_some();
    let output_channels = if reference_is_bayer {
        3_u64
    } else {
        reference_frame.image.channels as u64
    };
    let pixels = reference_frame.image.pixel_count();
    let pipeline_budget = execution::plan_pipeline(
        available_memory,
        worker_policy,
        pixels,
        pixels.saturating_mul(output_channels as usize),
        &plan,
        &threads,
    )?;
    let threads = pipeline_budget.threads;
    tracing::info!(
        job_id,
        group_index = group.index,
        compute_workers = threads.compute_workers,
        preparation_workers = threads.preparation_workers,
        serial = threads.serial,
        memory = %pipeline_budget.summary(),
        "Stack pipeline memory planned"
    );

    let (mut stacker, mut ledger, mut points, mut orientation_vote) = pool.install(|| {
        // A checkpoint that fails to reopen — a Seiza version change, a
        // truncated file — is discarded and the group builds from scratch.
        let restored = checkpoint.and_then(|checkpoint| {
            match LiveStacker::open_context(&checkpoint.context_path) {
                Ok(stacker)
                    if resume::context_matches_manifest(
                        stacker.view().accepted_frames,
                        stacker.view().rejected_frames,
                        &checkpoint.manifest,
                    ) =>
                {
                    Some((stacker, checkpoint.manifest))
                }
                Ok(_) => {
                    tracing::warn!(
                        "Stack checkpoint context and manifest did not match; rebuilding from scratch"
                    );
                    resume::discard(stack_root, database_id, group_target_id, &group_filter_name, group_exposure_key.as_deref());
                    state.stack_previews.update(job_id, |job| {
                        job.groups[group.index].resume_note =
                            Some("Full restack: the checkpoint files did not match".into());
                    });
                    None
                }
                Err(error) => {
                    tracing::warn!(
                        "Stack checkpoint could not be reopened ({error}); rebuilding from scratch"
                    );
                    resume::discard(stack_root, database_id, group_target_id, &group_filter_name, group_exposure_key.as_deref());
                    state.stack_previews.update(job_id, |job| {
                        job.groups[group.index].resume_note =
                            Some("Full restack: the checkpoint could not be reopened".into());
                    });
                    None
                }
            }
        });
        let mut orientation_vote = OrientationVote::default();
        let (stacker, ledger, points) = match restored {
            Some((stacker, manifest)) => {
                // Replay the checkpointed ledger: the per-frame record, the
                // counters, and each accepted frame's orientation vote. The
                // reference is the ledger's first entry and votes upright.
                let points = manifest.snr_points;
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
                    let exposures = accepted_exposures(&ledger);
                    status.snr = (!points.is_empty()).then(|| {
                        snr::ProgressiveSnr::new(order, points.clone(), &exposures)
                    });
                });
                (stacker, ledger, points)
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
                // With no dark master anywhere in the plan, nothing
                // subtracts the lights' hot pixels, and a handful of frames
                // cannot reject them statistically. The spatial impulse
                // filter is the remaining defense; when any session has a
                // dark, the dark does the job with real measurements.
                let mut options = method.stack_options(cosmetic);
                // The reference is judged against its session's masters
                // under that session's gate, its rig's tolerances.
                options.match_tolerances = plan.sessions[plan.assignments[0]].match_tolerances;
                // The reference calibrates with its own session's masters;
                // later sessions swap theirs in per batch.
                //
                // Creation validates the reference against those masters, so
                // this is a place a calibration refusal can escape — it did
                // once, killing two filters at frame zero over cached masters
                // that predate optics metadata. Auto mode's contract instead:
                // degrade to a raw stack, and say so and why. Forced
                // calibration keeps the hard error.
                let reference_masters = plan.sessions[plan.assignments[0]].masters.clone();
                let (stacker, calibration_bypassed) = match LiveStacker::new(reference_frame, reference_masters, options.clone()) {
                    Ok(stacker) => (stacker, false),
                    Err(error)
                        if group.calibration != crate::calibration::CalibrationMode::On =>
                    {
                        tracing::warn!(
                            "reference refused its session masters; stacking raw: {error}"
                        );
                        let note = format!(
                            "Stacked without calibration: the reference frame's masters were \
                             refused — {error}"
                        );
                        state.stack_previews.update(job_id, |job| {
                            let calibration = &mut job.groups[group.index].calibration;
                            calibration.warning = Some(match calibration.warning.take() {
                                Some(previous) if previous.contains(&note) => previous,
                                Some(previous) => format!("{previous}. {note}"),
                                None => note,
                            });
                        });
                        let reference_frame =
                            crate::image_io::open_linear_frame(&group.frames[0].path)
                                .map_err(|error| error.to_string())?;
                        (LiveStacker::new(
                            reference_frame,
                            seiza_stacking::CalibrationMasters::default(),
                            options,
                        )
                        .map_err(|error| error.to_string())?, true)
                    }
                    Err(error) => return Err(error.to_string()),
                };
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
                    // The reference defines the noise every other weight is
                    // read against, so it weighs 1 in every channel.
                    noise_sigma: (!stacker.reference_noise().is_empty())
                        .then(|| stacker.reference_noise().to_vec()),
                    integration_weight: (!weighting.is_equal()).then(|| {
                        let channels = match stacker.reference_noise().len() {
                            0 => output_channels as usize,
                            measured => measured,
                        };
                        vec![1.0; channels]
                    }),
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
                    calibration_bypassed,
                    retryable_failure: false,
                    rotation_radians: Some(0.0),
                }];
                (stacker, ledger, Vec::new())
            }
        };
        Ok::<_, String>((stacker, ledger, points, orientation_vote))
    })?;
    // The reference frame alone is the first depth on the curve and the
    // baseline every later depth is read against: one frame's noise.
    if points.is_empty()
        && let Some(sample) = pool.install(|| seiza_stacking::measure_depth(stacker.view()))
    {
        points.push(snr::point(sample, integrated_exposure(&ledger)));
    }
    let save_checkpoint =
        |stacker: &LiveStacker, ledger: &[resume::ResumeFrame], points: &[snr::SnrPoint]| {
            pool.install(|| {
                // A quality-ordered build is a full restack by nature. Writing its
                // accumulator here would leave a checkpoint no later build can
                // extend, in place of the capture-order one that can be.
                if !order.resumable() {
                    return;
                }
                if ledger.iter().any(|frame| frame.retryable_failure) {
                    resume::discard(
                        stack_root,
                        database_id,
                        group_target_id,
                        &group_filter_name,
                        group_exposure_key.as_deref(),
                    );
                    return;
                }
                let context_path = resume::context_path(
                    stack_root,
                    database_id,
                    group_target_id,
                    &group_filter_name,
                    group_exposure_key.as_deref(),
                );
                let manifest = resume::ResumeManifest {
                    schema_version: resume::RESUME_SCHEMA_VERSION,
                    stacking_version: SEIZA_STACKING_OUTPUT_VERSION.into(),
                    target_id: group_target_id,
                    filter_name: group_filter_name.clone(),
                    exposure_group_key: group_exposure_key.clone(),
                    accepted_only,
                    scoring,
                    method,
                    calibration_fingerprint: calibration_fingerprint.clone(),
                    order,
                    snr_points: points.to_vec(),
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
                                stack_root,
                                database_id,
                                group_target_id,
                                &group_filter_name,
                                group_exposure_key.as_deref(),
                            ),
                            &manifest,
                        )
                    });
                if let Err(error) = saved {
                    // A checkpoint is an optimization; a build never fails over
                    // it. Discard the pair so nothing resumes from half a save.
                    tracing::warn!("Failed to save stack checkpoint: {error}");
                    resume::discard(
                        stack_root,
                        database_id,
                        group_target_id,
                        &group_filter_name,
                        group_exposure_key.as_deref(),
                    );
                }
            })
        };

    // Pending frames keep their index into `group.frames`, which is what
    // the calibration plan's session assignments are aligned with.
    let pending: Vec<(usize, &PreparedFrame)> =
        group.frames.iter().enumerate().skip(ledger.len()).collect();
    // Reads, calibration, registration and normalization overlap across
    // frames while integration stays in this order, so the accumulator
    // sees exactly the sequence a frame-at-a-time loop would. A frame
    // declaring itself normalized is put on the same 16-bit scale the
    // rest of the catalog uses as it is read.
    let pipeline = pipeline_budget.options;
    let mut cancelled = false;
    // Depths already behind us were measured by the build that wrote the
    // checkpoint, so only the ones ahead split this run's batches.
    let start_depth = ledger.len();
    let mut checkpoints: std::collections::VecDeque<usize> =
        seiza_stacking::checkpoint_depths(start_depth + pending.len())
            .into_iter()
            .filter(|depth| *depth > start_depth)
            .collect();
    // Consecutive frames of one calibration session push as one
    // pipelined batch, with the session's masters swapped in first. The
    // frames after the reference are chronological, so a night is one
    // batch and a single-session group is exactly one call. On a
    // resumed stack this also replaces whatever masters the checkpoint
    // stored with the ones this batch needs.
    // Frames the stacker turned away for calibration, folded into the
    // group warning below: each carries its reason in the frame list,
    // but nobody reads a hundred rows to learn that six frames shared
    // one cause.
    let mut calibration_rejections: Vec<String> = Vec::new();
    // Whose masters the stacker holds, and whether that session stacks raw.
    // A fresh stack holds the reference's; a resumed one holds whatever its
    // checkpoint stored, so its first batch swaps. A batch also ends at each
    // depth the curve is measured at, and swapping in the masters already
    // held would only validate them again and add a set to Seiza's ledger.
    let mut active_masters: Option<(usize, bool)> =
        (start_depth == 1).then(|| (plan.assignments[0], ledger[0].calibration_bypassed));
    let mut batch_start = 0usize;
    while batch_start < pending.len() && !cancelled {
        let session = plan.assignments[pending[batch_start].0];
        let mut batch_end = pending[batch_start..]
            .iter()
            .position(|(index, _)| plan.assignments[*index] != session)
            .map(|offset| batch_start + offset)
            .unwrap_or(pending.len());
        // A batch also ends at the next depth the curve is measured at.
        // The accumulator can only be read between batches, and the
        // doubling ladder keeps this to about one extra boundary per
        // doubling — nine of them across five hundred frames.
        if let Some(&next) = checkpoints.front() {
            let limit = next.saturating_sub(start_depth);
            if limit > batch_start {
                batch_end = batch_end.min(limit);
            }
        }
        let batch = &pending[batch_start..batch_end];
        let paths: Vec<PathBuf> = batch.iter().map(|(_, frame)| frame.path.clone()).collect();
        // Auto mode's contract: a calibration problem is something to
        // warn about and work around, never a reason to abandon a stack
        // half-integrated. A swap the stacker refuses falls back to
        // stacking this session's frames raw, and the group warning says
        // so and why. Forced calibration keeps the hard error: the user
        // explicitly asked for these masters.
        let mut calibration_bypassed = false;
        if let Some((active, bypassed)) = active_masters
            && active == session
        {
            calibration_bypassed = bypassed;
        } else if let Err(error) =
            pool.install(|| use_session(&mut stacker, &plan.sessions[session]))
        {
            if group.calibration == crate::calibration::CalibrationMode::On {
                return Err(error.to_string());
            }
            tracing::warn!(
                "session {session} masters refused; stacking its frames uncalibrated: {error}"
            );
            pool.install(|| stacker.set_calibration(seiza_stacking::CalibrationMasters::default()))
                .map_err(|error| error.to_string())?;
            calibration_bypassed = true;
            let note = format!(
                "Session {} stacked uncalibrated: its masters were refused — {error}",
                session + 1
            );
            state.stack_previews.update(job_id, |job| {
                let calibration = &mut job.groups[group.index].calibration;
                calibration.warning = Some(match calibration.warning.take() {
                    Some(previous) if previous.contains(&note) => previous,
                    Some(previous) => format!("{previous}. {note}"),
                    None => note,
                });
            });
        }
        active_masters = Some((session, calibration_bypassed));
        let mut consumed = 0usize;
        // Seiza prepares frames as tasks in the build's pool and commits
        // outcomes in source order.
        let batch_started = std::time::Instant::now();
        let report = execution::run_pipeline(
            &mut stacker,
            &paths,
            &pipeline,
            &pool,
            &threads,
            |_, outcome| {
                let (_, frame) = batch[consumed];
                consumed += 1;
                let exposure = frame.exposure_seconds;
                let (decision, retryable_failure) = match outcome {
                    Ok(FrameDisposition::Accepted(diagnostics)) => {
                        orientation_vote
                            .add(diagnostics.mapping.transform().rotation_radians, exposure);
                        (
                            StackFrameDecision {
                                image_id: frame.image_id,
                                disposition: "accepted".into(),
                                reason: None,
                                quality_score: frame.quality_score,
                                matched_stars: Some(diagnostics.matched_stars),
                                registration_rms_pixels: Some(diagnostics.registration_rms_pixels),
                                registration_drift_pixels: Some(
                                    diagnostics.registration_drift_pixels,
                                ),
                                registered_mapping: Some(*diagnostics.mapping),
                                normalization_mean_gain: Some(diagnostics.normalization_mean_gain),
                                normalization_mean_offset: Some(
                                    diagnostics.normalization_mean_offset,
                                ),
                                source_fingerprint: Some(frame.source_fingerprint.clone()),
                                overlap_fraction: Some(diagnostics.overlap_fraction),
                                integrated_fraction: Some(diagnostics.integrated_fraction),
                                noise_sigma: (!diagnostics.noise.is_empty())
                                    .then(|| diagnostics.noise.clone()),
                                integration_weight: (!diagnostics.weight.is_empty())
                                    .then(|| diagnostics.weight.clone()),
                            },
                            false,
                        )
                    }
                    // A frame the stack turned away and one that could not be
                    // read are both "not integrated" to a caller reading the
                    // group's decisions; only the reason differs.
                    Ok(FrameDisposition::Rejected(reason)) => {
                        if let seiza_stacking::FrameRejectionReason::Calibration(message) = &reason
                        {
                            calibration_rejections.push(message.clone());
                        }
                        tracing::info!(
                            job_id,
                            group_index = group.index,
                            image_id = frame.image_id,
                            file = %frame.path.display(),
                            "Stack rejected a frame: {reason}"
                        );
                        (rejected_decision(frame, reason.to_string()), false)
                    }
                    Err(error) => {
                        tracing::warn!(
                            job_id,
                            group_index = group.index,
                            image_id = frame.image_id,
                            file = %frame.path.display(),
                            "Stack could not read a frame: {error}"
                        );
                        (rejected_decision(frame, error.to_string()), true)
                    }
                };
                ledger.push(resume::ResumeFrame {
                    decision: decision.clone(),
                    exposure_seconds: exposure,
                    calibration_bypassed,
                    retryable_failure,
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
                if cancel.requested() {
                    cancelled = true;
                    seiza_stacking::Continue::No
                } else {
                    seiza_stacking::Continue::Yes
                }
            },
        )
        .map_err(|error| error.to_string())?;
        tracing::info!(
            job_id,
            group_index = group.index,
            session,
            batch_frames = paths.len(),
            elapsed_seconds = batch_started.elapsed().as_secs_f64(),
            integrated = report.frames.integrated,
            rejected = report.frames.rejected,
            failed = report.frames.failed,
            preparation_workers = report.workers,
            execution = ?report.execution,
            "Stack preparation batch finished"
        );
        batch_start = batch_end;

        // Read the accumulator whenever this batch carried it past a
        // depth on the ladder, and once more wherever a stop landed, so a
        // stopped build still publishes the curve it paid for.
        let depth = start_depth + batch_start;
        let crossed = checkpoints.front().is_some_and(|next| *next <= depth);
        while checkpoints.front().is_some_and(|next| *next <= depth) {
            checkpoints.pop_front();
        }
        // Frames the stack turned away move the depth without moving the
        // accepted count, and a step that integrated nothing is not a
        // depth on the curve.
        let advanced = points
            .last()
            .is_none_or(|last| last.frames < stacker.view().accepted_frames);
        if (crossed || cancelled)
            && advanced
            && let Some(sample) = pool.install(|| seiza_stacking::measure_depth(stacker.view()))
        {
            points.push(snr::point(sample, integrated_exposure(&ledger)));
            let exposures = accepted_exposures(&ledger);
            let progressive = snr::ProgressiveSnr::new(order, points.clone(), &exposures);
            state.stack_previews.update(job_id, |job| {
                job.groups[group.index].snr = Some(progressive);
            });
        }
    }

    if cancelled {
        // The frames that did land are checkpointed, so building again
        // continues from them.
        save_checkpoint(&stacker, &ledger, &points);
        return Ok(GroupOutcome::Cancelled);
    }
    // The accumulator is complete: checkpoint it before the snapshot
    // consumes the stacker, so an additive rebuild can pick it up here.
    save_checkpoint(&stacker, &ledger, &points);
    // The per-frame reasons live in the frame list; the summary is what a
    // person actually reads. One line per distinct cause, with a count.
    if !calibration_rejections.is_empty() {
        let mut counts: Vec<(String, usize)> = Vec::new();
        for reason in &calibration_rejections {
            match counts.iter_mut().find(|(existing, _)| existing == reason) {
                Some((_, count)) => *count += 1,
                None => counts.push((reason.clone(), 1)),
            }
        }
        let note = format!(
            "{} frame(s) could not be calibrated and were left out — {}",
            calibration_rejections.len(),
            counts
                .iter()
                .map(|(reason, count)| format!("{count}× {reason}"))
                .collect::<Vec<_>>()
                .join("; ")
        );
        state.stack_previews.update(job_id, |job| {
            let calibration = &mut job.groups[group.index].calibration;
            calibration.warning = Some(match calibration.warning.take() {
                Some(previous) if previous.contains(&note) => previous,
                Some(previous) => format!("{previous}. {note}"),
                None => note,
            });
        });
    }
    // Last exit before the job writes anything. Orienting and rendering
    // follow, and a stop after this point would have to clean up published
    // artifacts.
    if cancel.requested() {
        return Ok(GroupOutcome::Cancelled);
    }
    pool.install(|| {
        let reference_headers = stacker.reference_headers().to_vec();
        let accepted_frames = stacker.view().accepted_frames;
        let rejected_frames = stacker.view().rejected_frames;
        let report_pass = |pass: seiza_stacking::BatchStackPass, index: usize, count: usize| {
            let pass = match pass {
                seiza_stacking::BatchStackPass::Estimate => 1,
                seiza_stacking::BatchStackPass::Refine => 2,
                seiza_stacking::BatchStackPass::Integrate => 3,
            };
            state.stack_previews.update(job_id, |job| {
                let group = &mut job.groups[group.index];
                group.phase = format!(
                    "Rejecting transients: pass {pass}/3, frame {}/{count}",
                    index + 1
                );
                group.final_pass = Some(FinalPassProgress {
                    pass,
                    passes: 3,
                    frame: index + 1,
                    frames: count,
                });
            });
        };
        // The final pass replays PSF Guard's ledger, keeping each frame's
        // background matched to the reference the live pass chose. Seiza
        // 0.19's own reintegration refits those backgrounds against the mean
        // of the best frames instead; on a 14-frame Sh2 230 night that mean
        // carried a corner gradient into the whole stack, a gradient the
        // live stack did not have. Seiza's command line does the same.
        let reintegrate = method.final_pass == StackFinalPass::Reintegrate && accepted_frames >= 3;
        let reintegrate = if reintegrate && method.bayer_drizzle && reference_is_bayer {
            // Only Seiza can fill the photosites no frame reached, and it
            // does so in the live stack.
            state.stack_previews.update(job_id, |job| {
                add_note(
                    &mut job.groups[group.index].resume_note,
                    "Final pass skipped: PSF Guard cannot yet replay a Bayer drizzle, so this is \
                     the live stack",
                );
            });
            false
        } else {
            reintegrate
        };
        let integrated = if reintegrate {
            // The live accumulator is no longer needed. Release it before
            // revisiting early transients.
            drop(stacker);
            tracing::info!(
                job_id,
                group_index = group.index,
                accepted_frames,
                "Starting final transient rejection"
            );
            let result = final_integration::integrate(
                &group,
                &ledger,
                method,
                &plan,
                &session_cosmetics,
                cancel,
                report_pass,
            );
            let result = match result {
                Ok(result) => result,
                Err(seiza_stacking::Error::Cancelled) => return Ok(GroupOutcome::Cancelled),
                Err(error) => return Err(format!("Final transient rejection failed: {error}")),
            };
            let mut diagnostics = result.frames.iter();
            state.stack_previews.update(job_id, |job| {
                for frame in &mut job.groups[group.index].frames {
                    if matches!(frame.disposition.as_str(), "reference" | "accepted")
                        && let Some(diagnostic) = diagnostics.next()
                    {
                        frame.integrated_fraction = Some(
                            diagnostic.integrated_samples as f32
                                / result.snapshot.image.sample_count() as f32,
                        );
                    }
                }
            });
            let rejected_samples: u64 = result
                .snapshot
                .rejected_samples
                .iter()
                .map(|&count| u64::from(count))
                .sum();
            tracing::info!(
                job_id,
                group_index = group.index,
                rejected_samples,
                "Final transient rejection completed"
            );
            result.snapshot.image
        } else {
            stacker
                .into_snapshot()
                .map_err(|error| error.to_string())?
                .image
        };
        if cancel.requested() {
            return Ok(GroupOutcome::Cancelled);
        }
        if north_up {
            state.stack_previews.update(job_id, |job| {
                job.groups[group.index].phase = "orienting".into();
            });
        }
        // Registration already absorbs a meridian flip, so a stack is published
        // in the reference frame's own rotation unless the caller asks for the
        // shared north-up grid that a mosaic needs. The one correction it still
        // makes is a half turn when the reference sits on the thinner side of a
        // flip, so the result faces the way most of the night was shot.
        let (image, sky_orientation, mut output_cards) = if north_up {
            let (source_wcs, orientation_source) =
                resolve_stack_wcs(state, &ctx, &group.frames[0], &reference_headers)?;
            let orientation = seiza_stacking::SkyOrientationPlan::new(
                integrated.width,
                integrated.height,
                &source_wcs,
            )
            .map_err(|error| error.to_string())?;
            let oriented = orientation
                .apply(&integrated)
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
            let (width, height) = (integrated.width, integrated.height);
            if turn {
                (
                    half_turn(integrated),
                    StackSkyOrientation::source_frame_half_turn(width, height, decided_by),
                    Vec::new(),
                )
            } else {
                (
                    integrated,
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
        let fits_destination = fits_path(stack_root, job_id, group.index);
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
        let exposures = accepted_exposures(&ledger);
        let progressive = snr::ProgressiveSnr::new(order, points, &exposures);
        output_cards.push(
            seiza_fits::WriteHeaderCard::new(
                "SNRORDER",
                seiza_fits::HeaderValue::String(order.as_str().into()),
            )
            .with_comment("frame order the SNR curve was measured in"),
        );
        if let Some(deepest) = progressive.points.last() {
            output_cards.push(
                seiza_fits::WriteHeaderCard::new(
                    "SNRNOISE",
                    seiza_fits::HeaderValue::Float(deepest.noise),
                )
                .with_comment("measured pixel-to-pixel noise"),
            );
            output_cards.push(
                seiza_fits::WriteHeaderCard::new(
                    "SNRVALUE",
                    seiza_fits::HeaderValue::Float(deepest.snr),
                )
                .with_comment("measured signal-to-noise ratio"),
            );
        }
        if let Some(analysis) = &progressive.analysis {
            output_cards.push(
                seiza_fits::WriteHeaderCard::new(
                    "SNRSLOPE",
                    seiza_fits::HeaderValue::Float(analysis.noise_exponent),
                )
                .with_comment("fitted noise exponent; -0.5 is perfect averaging"),
            );
            output_cards.push(
                seiza_fits::WriteHeaderCard::new(
                    "SNRVERDT",
                    seiza_fits::HeaderValue::String(analysis.verdict.as_str().into()),
                )
                .with_comment("reading of the progressive SNR curve"),
            );
        }
        seiza_stacking::write_linear_image_fits_f32(
            &fits_temporary,
            &image,
            &reference_headers,
            &output_cards,
        )
        .map_err(|error| error.to_string())?;
        std::fs::rename(&fits_temporary, &fits_destination).map_err(|error| error.to_string())?;
        render_group_previews(state, &ctx, stack_root, job_id, group.index, &image)?;
        let snr_url = state.stack_previews.get(job_id).and_then(|current| {
            publish_snr_artifact(
                &snr_path(stack_root, job_id, group.index),
                &progressive,
                format!(
                    "/api/db/{}/stack-previews/{}/{}/snr?v={}",
                    database_id, job_id, group.index, current.artifact_revision
                ),
            )
        });
        state.stack_previews.update(job_id, |job| {
            let status = &mut job.groups[group.index];
            status.snr = Some(progressive);
            status.snr_url = snr_url;
        });
        Ok(GroupOutcome::Built)
    })
}

/// Render a channel's card previews from its integrated image, and record
/// the stretch release that rendered them. A one-shot-color stack's preview
/// is color calibrated against Gaia first, as its processing is by default;
/// the FITS stays as integrated. A build renders the image it just
/// integrated, and a preview refresh the FITS that build wrote.
fn render_group_previews(
    state: &AppState,
    ctx: &DatabaseContext,
    stack_root: &FsPath,
    job_id: &str,
    group_index: usize,
    image: &seiza_stacking::LinearImage,
) -> Result<(), String> {
    let calibrated = if image.channels == 3 {
        let status = state
            .stack_previews
            .get(job_id)
            .and_then(|job| job.groups.get(group_index).cloned());
        let source = osc_calibration_source(
            state,
            ctx,
            status.as_ref().and_then(|status| status.reference_image_id),
            status.and_then(|status| status.sky_orientation),
        );
        let plan = source.plan(color_calibration::StackColorCalibration::default());
        let open_catalog = || {
            state
                .astrometry
                .gaia_photometry_catalog()
                .map(|(catalog, _)| catalog)
        };
        let (calibrated, outcome) = color_calibration::calibrate_stack(&plan, image, &open_catalog);
        let basis = source.basis();
        state.stack_previews.update(job_id, |job| {
            job.groups[group_index].color_calibration = Some(outcome);
            job.groups[group_index].color_calibration_basis = Some(basis);
        });
        calibrated
    } else {
        None
    };
    stretch::render_image_previews_atomic(
        calibrated.as_ref().unwrap_or(image),
        &stretch::default_linear_config(),
        stretch::StackStretchSourceTransfer::Linear,
        &preview_path(stack_root, job_id, group_index),
        &original_preview_path(stack_root, job_id, group_index),
    )
    .map_err(|error| error.to_string())?;
    state.stack_previews.update(job_id, |job| {
        job.groups[group_index].preview_stretch_version =
            Some(stretch::SEIZA_STRETCH_OUTPUT_VERSION.into());
    });
    Ok(())
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

fn persist_manifest(stack_root: &FsPath, job: &StackPreviewJob) -> Result<(), String> {
    let path = manifest_path(stack_root, &job.job_id);
    let parent = path
        .parent()
        .ok_or_else(|| "Stack manifest path has no parent".to_string())?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    let bytes = serde_json::to_vec_pretty(job).map_err(|error| error.to_string())?;
    std::fs::write(&temporary, bytes).map_err(|error| error.to_string())?;
    std::fs::rename(&temporary, path).map_err(|error| error.to_string())
}

type MovedCards = Vec<(LatestStackPreviewGroup, LatestStackPreviewGroup)>;

#[cfg(test)]
fn persist_latest_groups(stack_root: &FsPath, job: &StackPreviewJob) -> Result<MovedCards, String> {
    persist_latest_groups_with_exposures(stack_root, job, None)
}

/// Whether a skipped channel can no longer be stacked from its frames, so
/// the stack its card shows was built from frames the build now leaves out.
///
/// Only the build's own rules count: grades, quality regrades and the
/// accepted-only choice. A frame whose file is missing or that has no
/// matching calibration may come back, so the card keeps its stack then.
fn skipped_for_grades(group: &StackGroupStatus) -> bool {
    group.state == StackGroupState::Skipped
        && group.missing_files == 0
        && group.calibration_excluded == 0
}

/// Point the cards at a build's ready channels, and remove the card of a
/// channel the build skipped because fewer than two of its frames are still
/// eligible. Returns each card that now shows another stack, with the entry
/// it showed before.
fn persist_latest_groups_with_exposures(
    stack_root: &FsPath,
    job: &StackPreviewJob,
    current: Option<&ProjectExposureGroups>,
) -> Result<MovedCards, String> {
    let in_project = |group: &&StackGroupStatus| {
        current.is_none_or(|current| group_matches_project_exposures(group, current))
    };
    let ready = job
        .groups
        .iter()
        .filter(|group| group.state == StackGroupState::Ready)
        .filter(in_project)
        .cloned()
        .collect::<Vec<_>>();
    let retired = job
        .groups
        .iter()
        .filter(|group| skipped_for_grades(group))
        .filter(in_project)
        .collect::<Vec<_>>();
    if ready.is_empty() && retired.is_empty() {
        return Ok(Vec::new());
    }

    let path = latest_path(stack_root, job.project_id);
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

    // Only successful current split builds retire obsolete split identities.
    // A toggle, failed build, or cached old request never discards another
    // family's remembered result. Keep the unsplit result for switching back.
    if let Some(current) = current
        && current.settings.split_exposure_groups
        && ready
            .iter()
            .any(|group| group_matches_project_exposures(group, current))
    {
        latest.groups.retain(|entry| {
            entry.group.exposure_group.is_none()
                || group_matches_project_exposures(&entry.group, current)
        });
    }

    let before = latest.groups.len();
    latest.groups.retain(|entry| {
        !retired.iter().any(|group| {
            entry.group.target_id == group.target_id
                && entry.group.filter_name == group.filter_name
                && exposure_group_key(&entry.group) == exposure_group_key(group)
        })
    });
    // An automatic refresh finds the same skipped channel each time; once
    // its card is gone there is nothing to write.
    if ready.is_empty() && latest.groups.len() == before {
        return Ok(Vec::new());
    }

    let mut moved = Vec::new();
    for group in ready {
        let replacement = LatestStackPreviewGroup {
            job_id: job.job_id.clone(),
            artifact_revision: job.artifact_revision.clone(),
            accepted_only: job.accepted_only,
            created_unix_seconds: job.created_unix_seconds,
            cache_version: job.cache_version,
            order: job.order,
            scoring: job.scoring,
            method: job.method,
            wbpp: None,
            group,
        };
        if let Some(existing) = latest.groups.iter_mut().find(|existing| {
            existing.group.target_id == replacement.group.target_id
                && existing.group.filter_name == replacement.group.filter_name
                && exposure_group_key(&existing.group) == exposure_group_key(&replacement.group)
        }) {
            if existing.job_id != replacement.job_id
                || existing.artifact_revision != replacement.artifact_revision
                || existing.group.index != replacement.group.index
            {
                moved.push((existing.clone(), replacement.clone()));
            }
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
            .then_with(|| exposure_group_key(&left.group).cmp(&exposure_group_key(&right.group)))
    });
    latest.updated_unix_seconds = chrono::Utc::now().timestamp();

    let parent = path
        .parent()
        .ok_or_else(|| "Latest stack preview path has no parent".to_string())?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    let bytes = serde_json::to_vec_pretty(&latest).map_err(|error| error.to_string())?;
    std::fs::write(&temporary, bytes).map_err(|error| error.to_string())?;
    std::fs::rename(&temporary, path).map_err(|error| error.to_string())?;
    Ok(moved)
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

fn exposure_group_key(group: &StackGroupStatus) -> Option<&str> {
    group
        .exposure_group
        .as_ref()
        .map(|group| group.key.as_str())
}

fn group_matches_project_exposures(
    group: &StackGroupStatus,
    current: &ProjectExposureGroups,
) -> bool {
    if !current.settings.split_exposure_groups {
        return group.exposure_group.is_none();
    }
    let Some(key) = exposure_group_key(group) else {
        return false;
    };
    !group.input_images.is_empty()
        && group.input_images.iter().all(|image| {
            current
                .by_image
                .get(&image.image_id)
                .is_some_and(|group| group.key == key)
        })
}

pub(super) fn current_project_latest_stacks(
    ctx: &DatabaseContext,
    project_id: i32,
    latest: LatestStackPreviews,
) -> Result<LatestStackPreviews, AppError> {
    let mut latest = current_latest_stacks(latest);
    let conn = ctx.db();
    let conn = conn.lock().map_err(AppError::db)?;
    let current = crate::server::exposure_groups::cached_project_groups(ctx, &conn, project_id)?;
    latest
        .groups
        .retain(|entry| group_matches_project_exposures(&entry.group, &current));
    Ok(latest)
}

fn stack_dir(stack_root: &FsPath, job_id: &str) -> PathBuf {
    crate::server::storage::stacks(stack_root).join(job_id)
}

fn manifest_path(stack_root: &FsPath, job_id: &str) -> PathBuf {
    stack_dir(stack_root, job_id).join("manifest.json")
}

/// Sweep every database's stack folder an hour apart: the sweep after each
/// build only reaches databases that still build. Each pass also drops the
/// indices of projects a catalog no longer has. The first pass waits ten
/// minutes, past the journal restore. Holds the state weakly, so a server
/// the desktop app restarted stops its janitor with it.
pub async fn run_janitor(state: Arc<AppState>) {
    let shutdown = Arc::clone(&state.shutdown);
    let state = Arc::downgrade(&state);
    if !shutdown
        .sleep(std::time::Duration::from_secs(10 * 60))
        .await
    {
        return;
    }
    loop {
        let Some(live) = state.upgrade() else {
            return;
        };
        for ctx in live.all_databases() {
            if shutdown.is_stopping() {
                return;
            }
            // A sweep under way finishes the files it is moving.
            let work = shutdown.hold(format!("stack folder sweep (db={})", ctx.id));
            let state = Arc::clone(&live);
            let _ = tokio::task::spawn_blocking(move || {
                let _work = work;
                sweep_database(&state, &ctx)
            })
            .await;
        }
        drop(live);
        if !shutdown
            .sleep(std::time::Duration::from_secs(60 * 60))
            .await
        {
            return;
        }
    }
}

/// Move a merged project's stacks onto the project it merged into, so the
/// sweep does not take them as a gone project's.
pub fn move_project_stacks(ctx: &DatabaseContext, from: i32, to: i32) {
    janitor::move_project_indices(&ctx.stack_root, from, to);
}

fn sweep_database(state: &AppState, ctx: &DatabaseContext) {
    // The shared request connection is only borrowed when it is free.
    let live_projects: Option<std::collections::HashSet<i32>> = {
        let connection = ctx.db();
        let Ok(connection) = connection.try_lock() else {
            return;
        };
        connection
            .prepare("SELECT Id FROM project")
            .and_then(|mut statement| {
                statement
                    .query_map([], |row| row.get::<_, i32>(0))?
                    .collect::<rusqlite::Result<_>>()
            })
            .ok()
    };
    let orphaned = live_projects
        .as_ref()
        .map(|live| janitor::drop_orphaned_indices(&ctx.stack_root, live))
        .unwrap_or(0);
    if orphaned > 0 {
        tracing::info!(
            db = %ctx.id,
            orphaned_indices = orphaned,
            "Dropped the stack indices of projects the catalog no longer has"
        );
    }
    state.stack_previews.prune_cache(&ctx.stack_root);
    crate::server::master_cleanup::sweep(state, ctx, std::time::SystemTime::now());
}

/// Every `<prefix>*.json` index in a folder, or the first that could not be
/// read or parsed. A missing folder holds none.
pub(crate) fn try_read_indices<T: serde::de::DeserializeOwned>(
    directory: &FsPath,
    prefix: &str,
) -> Result<Vec<T>, PathBuf> {
    let entries = match std::fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(_) => return Err(directory.to_path_buf()),
    };
    let mut indices = Vec::new();
    for entry in entries.flatten() {
        let named = entry
            .file_name()
            .to_str()
            .is_some_and(|name| name.starts_with(prefix) && name.ends_with(".json"));
        if !named {
            continue;
        }
        let path = entry.path();
        let parsed = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok());
        match parsed {
            Some(index) => indices.push(index),
            None => return Err(path),
        }
    }
    Ok(indices)
}

/// Parse every `latest-project-*.json` directly inside a directory,
/// skipping any that cannot be read.
pub(crate) fn read_latest_indices<T: serde::de::DeserializeOwned>(directory: &FsPath) -> Vec<T> {
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

fn latest_path(stack_root: &FsPath, project_id: i32) -> PathBuf {
    crate::server::storage::stacks(stack_root).join(format!("latest-project-{project_id}.json"))
}

fn preview_path(stack_root: &FsPath, job_id: &str, group_index: usize) -> PathBuf {
    stack_dir(stack_root, job_id).join(format!("group-{group_index}.png"))
}

fn original_preview_path(stack_root: &FsPath, job_id: &str, group_index: usize) -> PathBuf {
    stack_dir(stack_root, job_id).join(format!("group-{group_index}-original.png"))
}

fn fits_path(stack_root: &FsPath, job_id: &str, group_index: usize) -> PathBuf {
    stack_dir(stack_root, job_id).join(format!("group-{group_index}.fits"))
}

/// A group's progressive signal-to-noise curve, as JSON beside its FITS. It
/// lives inside the job directory, so the cache janitor sweeps it with the
/// rest of the job.
fn snr_path(stack_root: &FsPath, job_id: &str, group_index: usize) -> PathBuf {
    stack_dir(stack_root, job_id).join(format!("group-{group_index}-snr.json"))
}

/// The exposure the accumulator has taken so far. Turned-away frames carry no
/// rotation and contribute none of their time.
fn integrated_exposure(ledger: &[resume::ResumeFrame]) -> f64 {
    ledger
        .iter()
        .filter(|frame| frame.rotation_radians.is_some())
        .map(|frame| frame.exposure_seconds)
        .sum()
}

fn accepted_exposures(ledger: &[resume::ResumeFrame]) -> Vec<f64> {
    ledger
        .iter()
        .filter(|frame| frame.rotation_radians.is_some())
        .map(|frame| frame.exposure_seconds)
        .collect()
}

/// Publish the curve beside the stack, through a temporary name like every
/// other artifact. The caller exposes its URL only after this succeeds.
fn write_snr_artifact(path: &FsPath, progressive: &snr::ProgressiveSnr) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "stack SNR path has no parent".to_string())?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let bytes = serde_json::to_vec_pretty(progressive).map_err(|error| error.to_string())?;
    let temporary = path.with_extension(format!("{}.tmp.json", std::process::id()));
    std::fs::write(&temporary, bytes).map_err(|error| error.to_string())?;
    if let Err(error) = std::fs::rename(&temporary, path) {
        let _ = std::fs::remove_file(&temporary);
        return Err(error.to_string());
    }
    Ok(())
}

fn publish_snr_artifact(
    path: &FsPath,
    progressive: &snr::ProgressiveSnr,
    url: String,
) -> Option<String> {
    match write_snr_artifact(path, progressive) {
        Ok(()) => Some(url),
        Err(error) => {
            tracing::warn!("Failed to write the stack SNR curve: {error}");
            None
        }
    }
}

#[cfg(test)]
mod tests {

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
        conn.execute_batch("INSERT INTO project(Id,name,profileId,guid) VALUES(1,'Project','profile','project-one');
            INSERT INTO target(Id,name,projectId,active,ra,dec,epochcode,rotation,roi,guid)
                VALUES(1,'Target',1,1,10,20,0,0,100,'target-one');").unwrap();
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
        conn.execute_batch("INSERT INTO project(Id,name,profileId,guid) VALUES(1,'Project','profile','project-one');
            INSERT INTO target(Id,name,projectId,active,ra,dec,epochcode,rotation,roi,guid)
                VALUES(1,'Target',1,1,10,20,0,0,100,'target-one');").unwrap();
        for (id, filter) in [(1, "Ha"), (2, "Ha"), (3, "OIII"), (4, "OIII")] {
            conn.execute("INSERT INTO acquiredimage(Id,projectId,targetId,gradingStatus,metadata,acquireddate,filtername)
                VALUES(?1,1,1,1,'{}',?1,?2)", rusqlite::params![id, filter]).unwrap();
        }
        let mut ctx = DatabaseContext::new_for_test(conn);
        ctx.use_storage_for_test(directory.path());
        let ctx = Arc::new(ctx);
        let request: StackPreviewRequest = serde_json::from_value(
            serde_json::json!({"image_ids":[1, 2, 3, 4],"calibration":"off"}),
        )
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
        // Calibration settled, then 4 of 10 frames registered, with 30
        // final reads still to come.
        assert_eq!(group_work(&group, true), (14, 50));
        assert_eq!(group_work(&group, false), (14, 20));
        // The live pass is done and pass 2 has read 3 of 8 admitted frames.
        group.processed_frames = 10;
        group.final_pass = Some(FinalPassProgress {
            pass: 2,
            passes: 3,
            frame: 3,
            frames: 8,
        });
        assert_eq!(group_work(&group, true), (31, 44));
        group.state = StackGroupState::Ready;
        assert_eq!(group_work(&group, true), (44, 44));

        let mut job = completed_job("final", vec![group]);
        job.state = StackJobState::Running;
        job.groups[0].state = StackGroupState::Running;
        let entry = mono_activity(&job);
        assert_eq!((entry.processed_units, entry.total_units), (31, 44));
        assert_eq!(
            entry.progress_label.as_deref(),
            Some("Rejecting transients · pass 2/3 · frame 3/8")
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
        assert_eq!((entry.processed_units, entry.total_units), (2, 50));
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
        assert_eq!((entry.processed_units, entry.total_units), (7, 50));
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
            manager.line.tracked_ids().iter().all(|id| manager
                .line
                .stop_flag(id)
                .unwrap()
                .requested()),
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
        conn.execute_batch("INSERT INTO project(Id,name,profileId,guid) VALUES(1,'Project','profile','project-one');
            INSERT INTO target(Id,name,projectId,active,ra,dec,epochcode,rotation,roi,guid)
                VALUES(1,'Target',1,1,10,20,0,0,100,'target-one');").unwrap();
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
        let mut job = completed_job(job_id, vec![ready_group(42, "R", 1)]);
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
        journal::mark_restored();
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
    async fn a_reapply_steps_aside_for_a_person_and_waits_again() {
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
        // A person starts work: automatic work yields, the re-apply too.
        interrupt_automatic(&state);
        eventually("the re-apply leaves the line", || {
            the_automatic_run(&state).is_none()
        })
        .await;
        assert_eq!(state.stack_previews.reapply_count(), 1);
        assert_eq!(
            shown(&state, &second).unwrap().reprocessing.unwrap().state,
            stretch::ReprocessingState::Waiting
        );
        drop(busy);
        reapply::start_due(Arc::clone(&state)).await;
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
            crate::server::job_line::testing::entry_running(
                "later",
                Lane::Main,
                cancel,
                move || {
                    let _ = took.send(());
                    let _ = released.recv();
                    let _ = done.send(());
                },
            ),
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
    async fn an_automatic_refresh_reapplies_only_after_the_queued_builds() {
        let _policy = reapply_policy(true);
        let cache = tempfile::tempdir().unwrap();
        let (state, _) = processed_card(cache.path()).await;
        queued_build(&state, "later");
        let second = "2".repeat(64);
        refresh(&state, cache.path(), &second, "rev-2");
        // Past the moment it looks for its turn: it keeps waiting.
        tokio::time::sleep(std::time::Duration::from_secs(4)).await;
        reapply::start_due(Arc::clone(&state)).await;
        assert_eq!(state.stack_previews.reapply_count(), 1);
        assert!(the_reapply_run(&state).is_none());
        assert_eq!(state.stack_previews.waiting_reapplies().len(), 1);

        state
            .stack_previews
            .update("later", |job| job.state = StackJobState::Cancelled);
        state.stack_previews.release_place("later");
        reapply::start_due(Arc::clone(&state)).await;
        let run = the_reapply_run(&state).unwrap();
        assert!(run.automatic, "it is still background work");
        eventually("it runs once the line is empty", || {
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
        // The refresh's build goes ahead; the re-apply waits for its turn.
        state.stack_previews.yield_automatic_processing();
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
        // Switched off while it runs, which leaves it be; then it steps
        // aside for a person.
        reapply::configure(false);
        interrupt_automatic(&state);
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
        let job =
            build_set::queue_preparation(&state, &ctx, 1, build_request(vec![1, 2, 3, 4]), true)
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
        let job = build_set::queue_preparation(&state, &ctx, 1, build_request(vec![1, 99]), false)
            .unwrap();
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
        let job =
            build_set::queue_preparation(&state, &ctx, 1, build_request(vec![1, 2, 3, 4]), true)
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
            let idle = state
                .stack_previews
                .preparing
                .lock()
                .unwrap()
                .is_idle(&ctx.id);
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
            build_set::queue_preparation(&state, &ctx, 1, build_request(vec![1, 2]), false)
                .unwrap();
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
            serde_json::to_value(stretch::pending_status(stretch_id, &state.stack_previews))
                .unwrap()
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
            let stretch::StretchApplyOutcome::Pending { stretch_id } = outcome.await.unwrap()
            else {
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
                stretch::selected_processing(&ctx.stack_root, &source.key, &source.revision)
                    .unwrap();
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
}
