//! Resumable mono stack accumulation.
//!
//! A finished or stopped group build checkpoints its Seiza live-stack context
//! beside a manifest of every frame it pushed. A later build of the same
//! target/filter whose ordered frame sequence only grew at the end reopens
//! that context and pushes the new frames, instead of registering and
//! integrating the whole set again.
//!
//! The manifest is the proof of "only grew": its frame ledger must be an exact
//! ordered prefix of the new request, every source fingerprint and exposure
//! must match, and the calibration fingerprint must match, or the build starts
//! fresh. Inserting, removing, reordering, regrading, or correcting exposure
//! metadata never reuses a stale accumulator. A transient read failure is not
//! checkpointed. Seiza validates the context itself — format version,
//! dimensions, configuration, checksum — when it reopens it.

use super::snr;
use super::{StackFrameDecision, StackScoringSettings};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// Bumped whenever the recorded shape or the stacking pipeline changes in a
/// way that makes an old accumulator wrong to extend.
pub(super) const RESUME_SCHEMA_VERSION: u32 = 2;

/// One frame the checkpointed stack already integrated or turned away, with
/// everything needed to replay its ledger entry and its orientation vote.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct ResumeFrame {
    pub decision: StackFrameDecision,
    pub exposure_seconds: f64,
    /// A read or pipeline error may clear on retry, so no checkpoint that
    /// contains one is safe to extend.
    #[serde(default)]
    pub retryable_failure: bool,
    /// Registration rotation for an accepted frame; `None` for a rejection.
    pub rotation_radians: Option<f64>,
}

/// Everything a later build needs to prove the checkpoint extends its request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct ResumeManifest {
    pub schema_version: u32,
    pub stacking_version: String,
    pub target_id: i32,
    pub filter_name: String,
    pub accepted_only: bool,
    /// The scoring policy that decided which frames reached this accumulator.
    /// Checkpoints from before this field existed used calibrated defaults.
    #[serde(default)]
    pub scoring: StackScoringSettings,
    pub calibration_fingerprint: String,
    /// The order the frames were pushed in. Only a capture-order stack is
    /// ever resumed, but the checkpoint records what it was so an older one
    /// cannot be extended under a different reading.
    #[serde(default)]
    pub order: snr::StackFrameOrder,
    /// The progressive signal-to-noise curve measured so far. Appending to a
    /// stack appends to its curve, so a target stacked one night at a time
    /// still ends with one curve over the whole season.
    #[serde(default)]
    pub snr_points: Vec<snr::SnrPoint>,
    /// Frames in push order, the reference first.
    pub frames: Vec<ResumeFrame>,
}

/// A validated checkpoint: the reopened-context path plus the ledger to
/// replay. Frames the new request adds are whatever the manifest lacks.
pub(super) struct ResumeState {
    pub context_path: PathBuf,
    pub manifest: ResumeManifest,
}

/// The Seiza context and our ledger are published separately. Refuse a pair
/// interrupted between those writes rather than integrating frames twice.
pub(super) fn context_matches_manifest(
    context_accepted_frames: u32,
    context_rejected_frames: u32,
    manifest: &ResumeManifest,
) -> bool {
    let accepted = manifest
        .frames
        .iter()
        .filter(|frame| frame.rotation_radians.is_some())
        .count();
    let rejected = manifest.frames.len() - accepted;
    accepted == context_accepted_frames as usize && rejected == context_rejected_frames as usize
}

/// Whether a build may extend the checkpoint or must start over, and — when a
/// checkpoint existed but could not be used — the reason, stated for the user.
pub(super) enum ResumeDecision {
    Resume(Box<ResumeState>),
    /// `None` when no checkpoint existed: starting fresh is unremarkable.
    Fresh(Option<&'static str>),
}

impl ResumeDecision {
    pub fn state(self) -> Option<ResumeState> {
        match self {
            Self::Resume(state) => Some(*state),
            Self::Fresh(_) => None,
        }
    }

    pub fn fresh_reason(&self) -> Option<&'static str> {
        match self {
            Self::Resume(_) => None,
            Self::Fresh(reason) => *reason,
        }
    }
}

/// One group identity has one checkpoint, replaced on every settle. The key
/// hashes the identity so filter names never reach the filesystem.
fn group_key(database_id: &str, target_id: i32, filter_name: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(database_id.as_bytes());
    hasher.update(target_id.to_le_bytes());
    hasher.update(filter_name.as_bytes());
    let mut output = String::with_capacity(64);
    for byte in hasher.finalize() {
        write!(&mut output, "{byte:02x}").expect("writing to a String cannot fail");
    }
    output
}

fn resume_dir(cache_root: &Path) -> PathBuf {
    cache_root.join("stack-previews").join("resume")
}

pub(super) fn context_path(
    cache_root: &Path,
    database_id: &str,
    target_id: i32,
    filter_name: &str,
) -> PathBuf {
    resume_dir(cache_root).join(format!(
        "{}.seiza-stack",
        group_key(database_id, target_id, filter_name)
    ))
}

pub(super) fn manifest_path(
    cache_root: &Path,
    database_id: &str,
    target_id: i32,
    filter_name: &str,
) -> PathBuf {
    resume_dir(cache_root).join(format!(
        "{}.json",
        group_key(database_id, target_id, filter_name)
    ))
}

/// Load and validate a checkpoint for this exact request. A fresh decision is
/// always a correct answer; when a checkpoint existed but was refused, the
/// decision carries the reason so the build can say why it starts over.
#[allow(clippy::too_many_arguments)]
pub(super) fn load(
    cache_root: &Path,
    database_id: &str,
    target_id: i32,
    filter_name: &str,
    accepted_only: bool,
    scoring: StackScoringSettings,
    stacking_version: &str,
    calibration_fingerprint: &str,
    order: snr::StackFrameOrder,
    requested: &[(i32, &str, f64)],
) -> ResumeDecision {
    let manifest_path = manifest_path(cache_root, database_id, target_id, filter_name);
    let context_path = context_path(cache_root, database_id, target_id, filter_name);
    if !context_path.exists() {
        return ResumeDecision::Fresh(None);
    }
    // A quality-ordered build cannot extend anything: a frame added later can
    // sort into the middle of the order, and the accumulator has already
    // integrated everything that would come after it.
    if !order.resumable() {
        return ResumeDecision::Fresh(Some("quality order integrates every frame again"));
    }
    let manifest: Option<ResumeManifest> = std::fs::read(&manifest_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok());
    let Some(manifest) = manifest else {
        return ResumeDecision::Fresh(Some("the last checkpoint was unreadable"));
    };
    if manifest.schema_version != RESUME_SCHEMA_VERSION
        || manifest.stacking_version != stacking_version
        || manifest.target_id != target_id
        || manifest.filter_name != filter_name
    {
        return ResumeDecision::Fresh(Some("the stacking pipeline changed"));
    }
    if manifest.accepted_only != accepted_only {
        return ResumeDecision::Fresh(Some("the Accepted-only policy changed"));
    }
    if manifest.scoring != scoring {
        return ResumeDecision::Fresh(Some("the scoring policy changed"));
    }
    if manifest.calibration_fingerprint != calibration_fingerprint {
        return ResumeDecision::Fresh(Some("calibration changed"));
    }
    if manifest.order != order {
        return ResumeDecision::Fresh(Some("the frame order changed"));
    }
    if manifest.frames.iter().any(|frame| frame.retryable_failure) {
        return ResumeDecision::Fresh(Some("a frame could not be read"));
    }
    // The accumulator is order-sensitive, so membership is not enough. Its
    // complete ledger must be the exact prefix the new build would push,
    // including the first frame that defines registration and WCS provenance.
    let exact_prefix = manifest.frames.len() <= requested.len()
        && manifest
            .frames
            .iter()
            .zip(requested)
            .all(|(frame, requested)| {
                frame.decision.image_id == requested.0
                    && frame.decision.source_fingerprint.as_deref() == Some(requested.1)
                    && frame.exposure_seconds.to_bits() == requested.2.to_bits()
            });
    if !exact_prefix {
        return ResumeDecision::Fresh(Some("the frame sequence changed since the last build"));
    }
    ResumeDecision::Resume(Box::new(ResumeState {
        context_path,
        manifest,
    }))
}

/// Persist a settled group's checkpoint: the manifest first to a temporary
/// name, then into place, beside the context Seiza already wrote atomically.
pub(super) fn store_manifest(path: &Path, manifest: &ResumeManifest) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let bytes = serde_json::to_vec_pretty(manifest).map_err(|error| error.to_string())?;
    let temporary = path.with_extension(format!("{}.tmp.json", std::process::id()));
    std::fs::write(&temporary, bytes).map_err(|error| error.to_string())?;
    std::fs::rename(&temporary, path).map_err(|error| error.to_string())
}

/// Drop a group's checkpoint. Used when a fresh build replaces it and fails
/// to save its own, so a later build cannot resume from the wrong ancestor.
pub(super) fn discard(cache_root: &Path, database_id: &str, target_id: i32, filter_name: &str) {
    let _ = std::fs::remove_file(manifest_path(
        cache_root,
        database_id,
        target_id,
        filter_name,
    ));
    let _ = std::fs::remove_file(context_path(
        cache_root,
        database_id,
        target_id,
        filter_name,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decision(image_id: i32, fingerprint: &str) -> StackFrameDecision {
        StackFrameDecision {
            image_id,
            disposition: "accepted".into(),
            reason: None,
            quality_score: None,
            matched_stars: None,
            registration_rms_pixels: None,
            registration_drift_pixels: None,
            registered_mapping: None,
            normalization_mean_gain: None,
            normalization_mean_offset: None,
            source_fingerprint: Some(fingerprint.into()),
            overlap_fraction: None,
            integrated_fraction: None,
        }
    }

    fn manifest(frames: Vec<ResumeFrame>) -> ResumeManifest {
        ResumeManifest {
            order: snr::StackFrameOrder::Capture,
            snr_points: Vec::new(),
            schema_version: RESUME_SCHEMA_VERSION,
            stacking_version: "test".into(),
            target_id: 7,
            filter_name: "Ha".into(),
            accepted_only: false,
            scoring: StackScoringSettings::default(),
            calibration_fingerprint: "cal-1".into(),
            frames,
        }
    }

    fn frame(image_id: i32, fingerprint: &str) -> ResumeFrame {
        ResumeFrame {
            decision: decision(image_id, fingerprint),
            exposure_seconds: 300.0,
            retryable_failure: false,
            rotation_radians: Some(0.0),
        }
    }

    fn store(cache_root: &Path, manifest: &ResumeManifest) {
        store_manifest(
            &manifest_path(cache_root, "db", manifest.target_id, &manifest.filter_name),
            manifest,
        )
        .unwrap();
        // The context itself is Seiza's; its presence is what load checks.
        std::fs::write(
            context_path(cache_root, "db", manifest.target_id, &manifest.filter_name),
            b"context",
        )
        .unwrap();
    }

    fn try_load(cache_root: &Path, requested: &[(i32, &str)]) -> Option<ResumeState> {
        let requested = requested
            .iter()
            .map(|&(image_id, fingerprint)| (image_id, fingerprint, 300.0))
            .collect::<Vec<_>>();
        try_load_with_exposures(cache_root, &requested)
    }

    fn try_load_with_exposures(
        cache_root: &Path,
        requested: &[(i32, &str, f64)],
    ) -> Option<ResumeState> {
        load(
            cache_root,
            "db",
            7,
            "Ha",
            false,
            StackScoringSettings::default(),
            "test",
            "cal-1",
            snr::StackFrameOrder::Capture,
            requested,
        )
        .state()
    }

    #[test]
    fn a_superset_of_the_checkpoint_resumes() {
        let cache = tempfile::tempdir().unwrap();
        store(
            cache.path(),
            &manifest(vec![frame(1, "f1"), frame(2, "f2")]),
        );
        let resumed = try_load(cache.path(), &[(1, "f1"), (2, "f2"), (3, "f3")]).unwrap();
        assert_eq!(resumed.manifest.frames.len(), 2);
    }

    #[test]
    fn a_context_newer_than_its_manifest_is_rejected() {
        let recorded = manifest(vec![frame(1, "f1"), frame(2, "f2")]);
        assert!(context_matches_manifest(2, 0, &recorded));
        assert!(!context_matches_manifest(3, 0, &recorded));
        assert!(!context_matches_manifest(2, 1, &recorded));

        let mut rejected = frame(2, "f2");
        rejected.decision.disposition = "rejected".into();
        rejected.rotation_radians = None;
        let recorded = manifest(vec![frame(1, "f1"), rejected]);
        assert!(context_matches_manifest(1, 1, &recorded));
    }

    #[test]
    fn a_frame_inserted_inside_the_checkpoint_prefix_rebuilds_from_scratch() {
        let cache = tempfile::tempdir().unwrap();
        store(
            cache.path(),
            &manifest(vec![frame(1, "f1"), frame(2, "f2")]),
        );
        assert!(try_load(cache.path(), &[(1, "f1"), (3, "f3"), (2, "f2")]).is_none());
    }

    #[test]
    fn a_changed_reference_rebuilds_from_scratch() {
        let cache = tempfile::tempdir().unwrap();
        store(
            cache.path(),
            &manifest(vec![frame(1, "f1"), frame(2, "f2")]),
        );
        assert!(try_load(cache.path(), &[(3, "f3"), (1, "f1"), (2, "f2")]).is_none());
    }

    #[test]
    fn a_changed_exposure_rebuilds_from_scratch() {
        let cache = tempfile::tempdir().unwrap();
        store(
            cache.path(),
            &manifest(vec![frame(1, "f1"), frame(2, "f2")]),
        );
        assert!(
            try_load_with_exposures(cache.path(), &[(1, "f1", 300.0), (2, "f2", 600.0)]).is_none()
        );
    }

    #[test]
    fn a_retryable_frame_failure_never_resumes() {
        let cache = tempfile::tempdir().unwrap();
        let mut failed = frame(2, "f2");
        failed.retryable_failure = true;
        store(cache.path(), &manifest(vec![frame(1, "f1"), failed]));

        let decision = load(
            cache.path(),
            "db",
            7,
            "Ha",
            false,
            StackScoringSettings::default(),
            "test",
            "cal-1",
            snr::StackFrameOrder::Capture,
            &[(1, "f1", 300.0), (2, "f2", 300.0)],
        );

        assert_eq!(decision.fresh_reason(), Some("a frame could not be read"));
    }

    #[test]
    fn the_identical_set_resumes_with_nothing_to_add() {
        let cache = tempfile::tempdir().unwrap();
        store(
            cache.path(),
            &manifest(vec![frame(1, "f1"), frame(2, "f2")]),
        );
        assert!(try_load(cache.path(), &[(1, "f1"), (2, "f2")]).is_some());
    }

    #[test]
    fn a_resumed_build_carries_the_curve_it_already_measured() {
        // The whole point of persisting the curve: a target stacked one night
        // at a time ends with one curve over the season, not one per session.
        let cache = tempfile::tempdir().unwrap();
        let mut recorded = manifest(vec![frame(1, "f1"), frame(2, "f2")]);
        recorded.snr_points = vec![
            snr::SnrPoint {
                frames: 1,
                exposure_seconds: 300.0,
                noise: 20.0,
                background: 1000.0,
                signal: 500.0,
                snr: 25.0,
                channel_noise: vec![20.0],
            },
            snr::SnrPoint {
                frames: 2,
                exposure_seconds: 600.0,
                noise: 14.1,
                background: 1000.0,
                signal: 500.0,
                snr: 35.5,
                channel_noise: vec![14.1],
            },
        ];
        store(cache.path(), &recorded);
        let resumed = try_load(cache.path(), &[(1, "f1"), (2, "f2"), (3, "f3")]).unwrap();
        assert_eq!(resumed.manifest.snr_points.len(), 2);
        assert_eq!(resumed.manifest.snr_points[1].frames, 2);
        assert!((resumed.manifest.snr_points[1].noise - 14.1).abs() < 1e-9);
    }

    #[test]
    fn a_quality_ordered_build_never_resumes() {
        // A frame added later can sort into the middle of a quality order, and
        // the accumulator has already integrated everything after it.
        let cache = tempfile::tempdir().unwrap();
        store(
            cache.path(),
            &manifest(vec![frame(1, "f1"), frame(2, "f2")]),
        );
        let decision = load(
            cache.path(),
            "db",
            7,
            "Ha",
            false,
            StackScoringSettings::default(),
            "test",
            "cal-1",
            snr::StackFrameOrder::Quality,
            &[(1, "f1", 300.0), (2, "f2", 300.0), (3, "f3", 300.0)],
        );
        assert_eq!(
            decision.fresh_reason(),
            Some("quality order integrates every frame again")
        );
    }

    #[test]
    fn a_capture_build_does_not_extend_a_quality_checkpoint() {
        let cache = tempfile::tempdir().unwrap();
        let mut recorded = manifest(vec![frame(1, "f1")]);
        recorded.order = snr::StackFrameOrder::Quality;
        store(cache.path(), &recorded);
        let decision = load(
            cache.path(),
            "db",
            7,
            "Ha",
            false,
            StackScoringSettings::default(),
            "test",
            "cal-1",
            snr::StackFrameOrder::Capture,
            &[(1, "f1", 300.0), (2, "f2", 300.0)],
        );
        assert_eq!(decision.fresh_reason(), Some("the frame order changed"));
    }

    #[test]
    fn a_removed_frame_rebuilds_from_scratch() {
        let cache = tempfile::tempdir().unwrap();
        store(
            cache.path(),
            &manifest(vec![frame(1, "f1"), frame(2, "f2")]),
        );
        assert!(try_load(cache.path(), &[(1, "f1"), (3, "f3")]).is_none());
    }

    #[test]
    fn a_changed_file_under_the_same_id_rebuilds_from_scratch() {
        let cache = tempfile::tempdir().unwrap();
        store(
            cache.path(),
            &manifest(vec![frame(1, "f1"), frame(2, "f2")]),
        );
        assert!(try_load(cache.path(), &[(1, "f1"), (2, "regraded")]).is_none());
    }

    #[test]
    fn changed_calibration_rebuilds_from_scratch() {
        let cache = tempfile::tempdir().unwrap();
        store(cache.path(), &manifest(vec![frame(1, "f1")]));
        let decision = load(
            cache.path(),
            "db",
            7,
            "Ha",
            false,
            StackScoringSettings::default(),
            "test",
            "cal-2",
            snr::StackFrameOrder::Capture,
            &[(1, "f1", 300.0), (2, "f2", 300.0)],
        );
        assert_eq!(decision.fresh_reason(), Some("calibration changed"));
    }

    #[test]
    fn changed_scoring_policy_rebuilds_from_scratch() {
        let cache = tempfile::tempdir().unwrap();
        store(cache.path(), &manifest(vec![frame(1, "f1")]));
        let changed = StackScoringSettings {
            penalty_satellite: 0.0,
            ..StackScoringSettings::default()
        };
        let decision = load(
            cache.path(),
            "db",
            7,
            "Ha",
            false,
            changed,
            "test",
            "cal-1",
            snr::StackFrameOrder::Capture,
            &[(1, "f1", 300.0), (2, "f2", 300.0)],
        );

        assert_eq!(decision.fresh_reason(), Some("the scoring policy changed"));
    }

    #[test]
    fn checkpoint_without_scoring_uses_calibrated_defaults() {
        let cache = tempfile::tempdir().unwrap();
        let recorded = manifest(vec![frame(1, "f1")]);
        let path = manifest_path(
            cache.path(),
            "db",
            recorded.target_id,
            &recorded.filter_name,
        );
        let mut legacy = serde_json::to_value(&recorded).unwrap();
        legacy.as_object_mut().unwrap().remove("scoring");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, serde_json::to_vec_pretty(&legacy).unwrap()).unwrap();
        std::fs::write(
            context_path(
                cache.path(),
                "db",
                recorded.target_id,
                &recorded.filter_name,
            ),
            b"legacy context",
        )
        .unwrap();

        assert!(try_load(cache.path(), &[(1, "f1"), (2, "f2")]).is_some());
    }

    #[test]
    fn another_pipeline_version_rebuilds_from_scratch() {
        let cache = tempfile::tempdir().unwrap();
        store(cache.path(), &manifest(vec![frame(1, "f1")]));
        let decision = load(
            cache.path(),
            "db",
            7,
            "Ha",
            false,
            StackScoringSettings::default(),
            "newer",
            "cal-1",
            snr::StackFrameOrder::Capture,
            &[(1, "f1", 300.0), (2, "f2", 300.0)],
        );
        assert_eq!(
            decision.fresh_reason(),
            Some("the stacking pipeline changed")
        );
    }

    #[test]
    fn a_pre_snr_v1_checkpoint_rebuilds_the_complete_curve() {
        let cache = tempfile::tempdir().unwrap();
        let recorded = manifest(vec![frame(1, "f1"), frame(2, "f2")]);
        let path = manifest_path(
            cache.path(),
            "db",
            recorded.target_id,
            &recorded.filter_name,
        );
        let mut legacy = serde_json::to_value(&recorded).unwrap();
        legacy["schema_version"] = serde_json::Value::from(1);
        legacy.as_object_mut().unwrap().remove("snr_points");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, serde_json::to_vec_pretty(&legacy).unwrap()).unwrap();
        std::fs::write(
            context_path(
                cache.path(),
                "db",
                recorded.target_id,
                &recorded.filter_name,
            ),
            b"legacy context",
        )
        .unwrap();

        let decision = load(
            cache.path(),
            "db",
            7,
            "Ha",
            false,
            StackScoringSettings::default(),
            "test",
            "cal-1",
            snr::StackFrameOrder::Capture,
            &[(1, "f1", 300.0), (2, "f2", 300.0), (3, "f3", 300.0)],
        );
        assert_eq!(
            decision.fresh_reason(),
            Some("the stacking pipeline changed")
        );
    }

    #[test]
    fn a_different_accepted_only_policy_rebuilds_from_scratch() {
        let cache = tempfile::tempdir().unwrap();
        store(cache.path(), &manifest(vec![frame(1, "f1")]));
        let decision = load(
            cache.path(),
            "db",
            7,
            "Ha",
            true,
            StackScoringSettings::default(),
            "test",
            "cal-1",
            snr::StackFrameOrder::Capture,
            &[(1, "f1", 300.0), (2, "f2", 300.0)],
        );
        assert_eq!(
            decision.fresh_reason(),
            Some("the Accepted-only policy changed")
        );
    }

    #[test]
    fn a_missing_context_never_resumes_even_with_a_manifest() {
        let cache = tempfile::tempdir().unwrap();
        let manifest_value = manifest(vec![frame(1, "f1")]);
        store_manifest(&manifest_path(cache.path(), "db", 7, "Ha"), &manifest_value).unwrap();
        assert!(try_load(cache.path(), &[(1, "f1"), (2, "f2")]).is_none());
    }

    #[test]
    fn discard_removes_both_files() {
        let cache = tempfile::tempdir().unwrap();
        store(cache.path(), &manifest(vec![frame(1, "f1")]));
        discard(cache.path(), "db", 7, "Ha");
        assert!(try_load(cache.path(), &[(1, "f1"), (2, "f2")]).is_none());
        assert!(!manifest_path(cache.path(), "db", 7, "Ha").exists());
    }

    #[test]
    fn group_keys_separate_databases_targets_and_filters() {
        let keys = [
            group_key("db-a", 7, "Ha"),
            group_key("db-b", 7, "Ha"),
            group_key("db-a", 8, "Ha"),
            group_key("db-a", 7, "OIII"),
        ];
        let unique: std::collections::HashSet<_> = keys.iter().collect();
        assert_eq!(unique.len(), keys.len());
    }
}
