//! Project-scoped, per-target/per-filter stacking previews.
//!
//! PSF Guard owns frame selection and provenance. `seiza-stacking` owns
//! FITS decoding, calibration, registration, normalization, admission, and
//! ordered accumulation. Jobs are process-global and run one at a time so a
//! multi-database server cannot multiply the stacker's full-frame buffers.

mod activity;
pub mod artifact;
pub mod automatic;
mod build_set;
pub mod calibration_masters;
pub mod color;
pub mod color_calibration;
mod execution;
mod final_integration;
pub mod gpu;
mod integrate;
mod janitor;
pub mod journal;
mod line_work;
mod live;
pub use live::LiveStackPreview;
pub mod method;
mod mono_build;
mod mosaic;
mod processing_queue;
pub mod rc_astro;
pub(crate) mod reapply;
mod reference;
pub(crate) mod remote_channel;
mod render;
mod resume;
pub mod snr;
pub mod stretch;
pub mod wbpp_stacks;

use crate::concurrency::Stop;
pub use activity::{
    cancel_stack_activity_job, get_stack_activity, move_stack_job, run_scheduled_refresh_now,
    skip_scheduled_refresh, MoveStackJobRequest, ScheduledRefreshRequest,
};
#[cfg(test)]
use activity::{name_scheduled, reapplies_not_started, without_started};
pub(crate) use activity::{stack_activity, RefreshPlanning};
#[cfg(test)]
use build_set::{
    exclusion_reason, fallback_quality, prefer_target_filter_scores, prepare_job, start_prepared,
    Placement, Started,
};
use build_set::{prepare_channels, validate_request};
pub use build_set::{start_stack_preview_channels, start_stack_previews};
use line_work::{
    channel_key, enqueue_job, project_channels_key, submit, submit_entry, tier_of, BuildOutput,
};
#[cfg(test)]
use line_work::{ensure_dispatching, requeue_stopped_build};
#[cfg(test)]
use mono_build::every_channel_settled;
use mono_build::{
    add_note, cancel_unfinished_groups, execute_build, job_priority, publish_build,
    render_group_previews,
};
#[cfg(test)]
use render::preview_needs_render;
use render::queue_preview_render;

use crate::server::file_body::file_body;
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
pub use gpu::StackGpu;
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
    /// Integrate the project’s panels on one expanded Seiza canvas per channel.
    #[serde(default)]
    pub mosaic: bool,
    #[serde(default)]
    pub accepted_only: bool,
    #[serde(default)]
    pub force: bool,
    /// Reproject the integration onto the canonical north-up, east-left grid.
    /// Off by default: a stack keeps the rotation of its reference frame, and
    /// registration already absorbs a meridian flip. Turn this on when several
    /// single-field stacks need the same north-up convention. Mosaic builds
    /// use their expanded canvas and cannot combine this option with mosaic.
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
    #[serde(default)]
    pub live: bool,
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
    /// Catalog target ID, or the negative project ID for an integrated mosaic.
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
    /// A temporary display of the running accumulator, separate from the finished stack.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live_preview: Option<LiveStackPreview>,
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

/// How far the final rejection pass has got: reading each admitted frame
/// once, then integrating, two stages. A pass with no room for its scratch
/// files reads each frame once per pass instead, three passes.
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
/// each eligible frame once, and a final pass reads each admitted frame once
/// more and integrates it, weighed as a second read. Until the final pass starts its size is estimated from the
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
        2 * live
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
    /// What a server stop interrupted, for the journal's last write.
    stop_record: journal::StopRecord,
    /// One look at a time for a re-apply to start.
    reapply_looks: reapply::Looks,
    /// Whether the journal put back what waited before the restart.
    journal_restored: journal::Restored,
    /// Builds a restart could not prepare yet, retried for a while.
    deferred_restores: journal::DeferredRestores,
    /// Stretch, deconvolution and RC-Astro runs waiting in the line or
    /// running, by run id. The journal keeps them through the line's
    /// journal records.
    processing: Mutex<HashMap<String, processing_queue::ProcessingRun>>,
    /// Processing carried to refreshed stacks and waiting to run again, or
    /// running, by channel.
    reapplies: Mutex<HashMap<String, reapply::Reapply>>,
    /// Re-applies stopped to make way for a person's work, which wait again.
    yielded: Mutex<HashSet<String>>,
    /// Processing runs computing now, their progress, and failures kept for
    /// the next poll.
    stretch_runs: stretch::StretchRuns,
    /// Builds a person asked for that wait to be prepared, in click order.
    preparing: build_set::PreparationLine,
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
            stop_record: journal::StopRecord::default(),
            reapply_looks: reapply::Looks::default(),
            journal_restored: journal::Restored::default(),
            deferred_restores: journal::DeferredRestores::default(),
            stretch_runs: stretch::StretchRuns::default(),
            processing: Mutex::new(HashMap::new()),
            reapplies: Mutex::new(HashMap::new()),
            yielded: Mutex::new(HashSet::new()),
            preparing: build_set::PreparationLine::default(),
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
        let Some(stopped) = self.stop_record.interrupted() else {
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
        self.stop_record.begin(interrupted);
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
        // A person's Stop wins over a preemption: the work ends stopped
        // rather than going back in the line.
        self.line.forget_preempted(job_id);
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

    /// Stop the server's re-applies, queued or running, in one project, so
    /// a refresh's builds of that project get the worker. Each waits for its
    /// turn again rather than ending.
    pub(super) fn yield_automatic_processing(&self, database_id: &str, project_id: i32) {
        for run_id in self.automatic_processing_ids(database_id, project_id) {
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
        // A color build's channel deconvolutions sit in the cache the channel
        // cards use; the build keeps them as a selection would.
        let mut color_deconvolution_ids: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        for job in self.color_jobs.lock().unwrap().values() {
            color_job_ids.insert(job.job_id.clone());
            if let Some(input_id) = &job.linear_input_id {
                color_input_ids.insert(input_id.clone());
            }
            color_deconvolution_ids.extend(job.input_deconvolution_ids.values().cloned());
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
                color_deconvolution_ids.extend(job.input_deconvolution_ids.into_values());
            }
        }
        let (stretch_ids, mut deconvolution_ids) = stretch::kept_processing(stack_root)?;
        deconvolution_ids.extend(color_deconvolution_ids);
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
    /// Artifact searches waiting in the line or running, each with its
    /// place in line while it waits.
    pub fn artifact_searches_under_way(&self) -> Vec<(artifact::ArtifactSearchJob, Option<usize>)> {
        let searches: Vec<_> = self
            .artifact_jobs
            .lock()
            .unwrap()
            .values()
            .filter(|job| {
                matches!(
                    job.state,
                    artifact::ArtifactSearchState::Queued | artifact::ArtifactSearchState::Running
                )
            })
            .cloned()
            .collect();
        searches
            .into_iter()
            .map(|job| {
                let position = (job.state == artifact::ArtifactSearchState::Queued)
                    .then(|| self.line_position(&job.search_id))
                    .flatten();
                (job, position)
            })
            .collect()
    }

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
            crate::server::exposure_groups::cached_groups(
                ctx,
                &conn,
                job.project_id,
                job.groups
                    .iter()
                    .any(|group| group.target_id == -job.project_id),
            )
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
    #[serde(default)]
    target_id: i32,
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
    /// The fingerprint of the copy a worker reads, when it is not this
    /// server's file. The final pass checks that copy did not change;
    /// `source_fingerprint` stays this server's, which the decisions and
    /// the checkpoint record.
    #[serde(skip)]
    read_fingerprint: Option<String>,
}

impl PreparedFrame {
    /// The fingerprint the file at `path` has while it is stacked.
    fn fingerprint_while_read(&self) -> &str {
        self.read_fingerprint
            .as_deref()
            .unwrap_or(&self.source_fingerprint)
    }
}

/// Exposure length in seconds from an acquired-image record, or zero when the
/// record does not say.
///
/// N.I.N.A. and PSF Guard's own importer spell this differently, and a value
/// can arrive as a number or as text, so take the first that reads as a finite
/// positive number.
/// A temporary file's tag, unique to this write: the process id and a
/// counter. Two writers of one cache entry, such as a channel card and a
/// color build in the quick lane, never write the same temporary file.
fn temporary_tag() -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let next = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{}-{next}", std::process::id())
}

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
    #[serde(default)]
    mosaic: bool,
    #[serde(default)]
    mosaic_panels: Vec<i32>,
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
/// the person's work. A stopped re-apply waits for its turn again.
/// Automatic work still waiting stays in line, behind the person's.
pub(super) fn interrupt_automatic(state: &AppState) {
    state.stack_previews.line.preempt_automatic(Lane::Main);
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
    State(state): State<Arc<AppState>>,
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
    let protected = state
        .wbpp_storage
        .protect(output_dir.parent().unwrap_or(&output_dir));
    let ctx = Arc::clone(&ctx.0);
    let target_id = request.target_id.or(last_target);
    tokio::task::spawn_blocking(move || {
        let _protected = protected;
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
    let path = if query.live {
        live::path(&ctx.stack_root, &job_id, group_index)
    } else {
        match query.size {
            StackPreviewImageSize::Screen => preview_path(&ctx.stack_root, &job_id, group_index),
            StackPreviewImageSize::Original => {
                original_preview_path(&ctx.stack_root, &job_id, group_index)
            }
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
        .header(
            CACHE_CONTROL,
            if query.live {
                "private, no-store"
            } else {
                "private, max-age=31536000, immutable"
            },
        )
        .body(file_body(file))
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
        .body(file_body(file))
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
        key: stretch::mono_source_key(job_id, group.index),
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
    with_index_lock(stack_root, || {
        persist_latest_groups_locked(stack_root, job, current)
    })
}

fn persist_latest_groups_locked(
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
        let mosaic = job
            .groups
            .iter()
            .any(|group| group.target_id == -job.project_id);
        latest.groups.retain(|entry| {
            (entry.group.target_id == -job.project_id) != mosaic
                || entry.group.exposure_group.is_none()
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
    let mosaic_groups =
        crate::server::exposure_groups::cached_groups(ctx, &conn, project_id, true)?;
    latest.groups.retain(|entry| {
        group_matches_project_exposures(
            &entry.group,
            if entry.group.target_id == -project_id {
                &mosaic_groups
            } else {
                &current
            },
        )
    });
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
pub fn spawn_janitor(state: &Arc<AppState>) {
    crate::server::schedule::every(
        state,
        "stack folder sweep",
        crate::server::schedule::Every::after(
            std::time::Duration::from_secs(10 * 60),
            std::time::Duration::from_secs(60 * 60),
        ),
        |state| async move {
            for ctx in state.all_databases() {
                if state.shutdown.is_stopping() {
                    return;
                }
                // A sweep under way finishes the files it is moving.
                let work = state
                    .shutdown
                    .hold(format!("stack folder sweep (db={})", ctx.id));
                let state = Arc::clone(&state);
                let _ = tokio::task::spawn_blocking(move || {
                    let _work = work;
                    sweep_database(&state, &ctx)
                })
                .await;
            }
        },
    );
}

/// Move a merged project's stacks onto the project it merged into, so the
/// sweep does not take them as a gone project's.
pub fn move_project_stacks(ctx: &DatabaseContext, from: i32, to: i32) {
    janitor::move_project_indices(&ctx.stack_root, from, to);
}

/// Drop a deleted project's stack indices at once: Target Scheduler gives
/// its Id to the next project, which must not inherit them before the sweep.
pub fn forget_project_stacks(ctx: &DatabaseContext, project_id: i32) {
    with_index_lock(&ctx.stack_root, || {
        janitor::drop_project_indices(&ctx.stack_root, project_id)
    });
}

/// Run `change` while no other read, change and write of this database's
/// stack indices runs. Builds, WBPP imports, color previews, target and
/// project moves and the sweep each rewrite a whole index file, so two at
/// once would lose one's change. Nothing inside `change` may take the lock
/// again.
pub(crate) fn with_index_lock<T>(stack_root: &FsPath, change: impl FnOnce() -> T) -> T {
    static LOCKS: std::sync::LazyLock<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>> =
        std::sync::LazyLock::new(Default::default);
    let lock = LOCKS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .entry(stack_root.to_path_buf())
        .or_default()
        .clone();
    let _guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    change()
}

/// Move a target's stacks to the project it moved to, so its stack previews
/// stay with it. Returns how many index entries moved.
pub fn move_target_stacks(ctx: &DatabaseContext, target_id: i32, from: i32, to: i32) -> usize {
    janitor::move_target_indices(&ctx.stack_root, target_id, from, to)
}

/// Remove a merged-away target's stacks from its project's indices.
/// Returns how many index entries were removed.
pub fn drop_merged_target_stacks(ctx: &DatabaseContext, project: i32, target_id: i32) -> usize {
    janitor::drop_target_entries(&ctx.stack_root, project, target_id)
}

/// Each target's project and name, by target ID.
type TargetsById = HashMap<i32, (i32, String)>;

/// Each target's project and name, for moving stacks after their target.
/// A target without a project is left out.
fn targets_by_id(conn: &rusqlite::Connection) -> rusqlite::Result<TargetsById> {
    let mut statement = conn.prepare("SELECT Id, projectid, name FROM target")?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, i32>(0)?,
            row.get::<_, Option<i32>>(1)?,
            row.get::<_, Option<String>>(2)?,
        ))
    })?;
    let mut targets = HashMap::new();
    for row in rows {
        if let (id, Some(project), name) = row? {
            targets.insert(id, (project, name.unwrap_or_default()));
        }
    }
    Ok(targets)
}

fn sweep_database(state: &AppState, ctx: &DatabaseContext) {
    // The shared request connection is only borrowed when it is free.
    let (live_projects, projects_of_targets): (
        Option<std::collections::HashSet<i32>>,
        Option<TargetsById>,
    ) = {
        let connection = ctx.db();
        let Ok(connection) = connection.try_lock() else {
            return;
        };
        let live = connection
            .prepare("SELECT Id FROM project")
            .and_then(|mut statement| {
                statement
                    .query_map([], |row| row.get::<_, i32>(0))?
                    .collect::<rusqlite::Result<_>>()
            })
            .ok();
        let targets = targets_by_id(&connection)
            .inspect_err(|error| {
                tracing::warn!(db = %ctx.id, "Could not read targets for the stack sweep: {error}")
            })
            .ok();
        (live, targets)
    };
    // A build that finished while its target moved wrote to the old
    // project's index.
    let rehomed = projects_of_targets
        .as_ref()
        .map(|targets| janitor::rehome_moved_target_entries(&ctx.stack_root, targets))
        .unwrap_or(0);
    if rehomed > 0 {
        tracing::info!(
            db = %ctx.id,
            stack_entries = rehomed,
            "Moved stack previews to the projects their targets moved to"
        );
    }
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

/// Where a group's raw frames clipped, as a mono FITS beside its stack on
/// the same grid: each pixel's clipped share of its observations, between 0
/// and 1. Absent when nothing clipped, the sensor's saturation level is
/// unknown, or the stack was built before PSF Guard kept it. It lives inside the job
/// directory, so the cache janitor sweeps it with the rest of the job.
fn clipped_path(stack_root: &FsPath, job_id: &str, group_index: usize) -> PathBuf {
    clipped_path_beside(&fits_path(stack_root, job_id, group_index))
}

/// The clipped map beside the stack FITS at `fits`: `group-2.fits` keeps
/// it in `group-2-clipped.fits`. A processing run knows only its stack's
/// FITS, so it finds the map this way.
fn clipped_path_beside(fits: &FsPath) -> PathBuf {
    let stem = fits.file_stem().unwrap_or_default().to_string_lossy();
    fits.with_file_name(format!("{stem}-clipped.fits"))
}

/// Write `clipped` to `path` through a temporary file, or remove what is
/// there when there is no map, so a stale one never outlives its stack.
fn write_clipped(
    path: &FsPath,
    clipped: Option<&seiza_stacking::LinearImage>,
) -> Result<(), String> {
    let Some(clipped) = clipped else {
        return match std::fs::remove_file(path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(format!(
                "Cannot remove the stale clipped map {}: {error}",
                path.display()
            )),
            _ => Ok(()),
        };
    };
    let temporary = path.with_extension(format!("{}.tmp.fits", std::process::id()));
    seiza_stacking::write_processed_image_fits_f32(&temporary, clipped, &[], &[])
        .map_err(|error| error.to_string())?;
    std::fs::rename(&temporary, path).map_err(|error| error.to_string())
}

/// The clipped map [`write_clipped`] wrote at `path`, or `None` when there
/// is none. One that cannot be read is logged and treated as absent: the
/// stack still calibrates, only without rendering clipped cores neutral.
fn read_clipped(path: &FsPath) -> Option<seiza_stacking::LinearImage> {
    if !path.exists() {
        return None;
    }
    // Read as written: the map holds fractions, not samples a stack scales.
    match seiza_stacking::FitsFrame::open(path) {
        Ok(frame) if frame.image.channels == 1 => Some(frame.image),
        Ok(frame) => {
            tracing::warn!(
                "Ignoring clipped map {}: it has {} channels, not one",
                path.display(),
                frame.image.channels
            );
            None
        }
        Err(error) => {
            tracing::warn!(
                "Ignoring unreadable clipped map {}: {error}",
                path.display()
            );
            None
        }
    }
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
mod tests;
