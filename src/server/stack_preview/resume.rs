//! Resumable mono stack accumulation.
//!
//! A finished or stopped group build checkpoints its Seiza live-stack context
//! beside a manifest of every frame it pushed. A later build of the same
//! target/filter whose frame set only grew reopens that context and pushes the
//! new frames, instead of registering and integrating the whole set again.
//!
//! The manifest is the proof of "only grew": every recorded frame must still
//! be requested with an identical source fingerprint, and the calibration
//! fingerprint must match, or the build starts fresh. Removing a frame,
//! regrading one in place, or changing calibration therefore never reuses a
//! stale accumulator. Seiza validates the context itself — format version,
//! dimensions, configuration, checksum — when it reopens it.

use super::StackFrameDecision;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// Bumped whenever the recorded shape or the stacking pipeline changes in a
/// way that makes an old accumulator wrong to extend.
pub(super) const RESUME_SCHEMA_VERSION: u32 = 1;

/// One frame the checkpointed stack already integrated or turned away, with
/// everything needed to replay its ledger entry and its orientation vote.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct ResumeFrame {
    pub decision: StackFrameDecision,
    pub exposure_seconds: f64,
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
    pub calibration_fingerprint: String,
    /// Frames in push order, the reference first.
    pub frames: Vec<ResumeFrame>,
}

/// A validated checkpoint: the reopened-context path plus the ledger to
/// replay. Frames the new request adds are whatever the manifest lacks.
pub(super) struct ResumeState {
    pub context_path: PathBuf,
    pub manifest: ResumeManifest,
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
    stacking_version: &str,
    calibration_fingerprint: &str,
    requested: &[(i32, &str)],
) -> ResumeDecision {
    let manifest_path = manifest_path(cache_root, database_id, target_id, filter_name);
    let context_path = context_path(cache_root, database_id, target_id, filter_name);
    if !context_path.exists() {
        return ResumeDecision::Fresh(None);
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
    if manifest.calibration_fingerprint != calibration_fingerprint {
        return ResumeDecision::Fresh(Some("calibration changed"));
    }
    // Every checkpointed frame must still be requested, byte-identical. A
    // fingerprint mismatch means the file changed under the same image id.
    let requested_fingerprints: std::collections::HashMap<i32, &str> =
        requested.iter().copied().collect();
    let all_present = manifest.frames.iter().all(|frame| {
        let recorded = frame.decision.source_fingerprint.as_deref();
        matches!(
            (requested_fingerprints.get(&frame.decision.image_id), recorded),
            (Some(requested), Some(recorded)) if *requested == recorded
        )
    });
    if !all_present {
        return ResumeDecision::Fresh(Some("frames were removed or changed since the last build"));
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
            schema_version: RESUME_SCHEMA_VERSION,
            stacking_version: "test".into(),
            target_id: 7,
            filter_name: "Ha".into(),
            accepted_only: false,
            calibration_fingerprint: "cal-1".into(),
            frames,
        }
    }

    fn frame(image_id: i32, fingerprint: &str) -> ResumeFrame {
        ResumeFrame {
            decision: decision(image_id, fingerprint),
            exposure_seconds: 300.0,
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
        load(cache_root, "db", 7, "Ha", false, "test", "cal-1", requested).state()
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
    fn the_identical_set_resumes_with_nothing_to_add() {
        let cache = tempfile::tempdir().unwrap();
        store(
            cache.path(),
            &manifest(vec![frame(1, "f1"), frame(2, "f2")]),
        );
        assert!(try_load(cache.path(), &[(1, "f1"), (2, "f2")]).is_some());
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
            "test",
            "cal-2",
            &[(1, "f1"), (2, "f2")],
        );
        assert_eq!(decision.fresh_reason(), Some("calibration changed"));
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
            "newer",
            "cal-1",
            &[(1, "f1"), (2, "f2")],
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
            "test",
            "cal-1",
            &[(1, "f1"), (2, "f2")],
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
