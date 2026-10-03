//! Seiza's automatic reference choice for a stack group.
//!
//! The reference fixes the stack's grid and, under local background
//! normalization, the background every frame is matched to. Seiza scores each
//! frame from its own stars and sky noise; among frames close to the best it
//! takes the flattest sky. The scores are cached by source fingerprint, so a
//! rebuild reads only frames it has not scored, and the same frames always
//! choose the same reference, which an additive rebuild needs.

use super::{snr, PreparedFrame, SEIZA_STACKING_VERSION};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// Seiza's rule: frames scoring within this share of the best are close
/// enough that the flattest sky wins among them. Seiza keeps the constant
/// private; its README states the 70%.
const CANDIDATE_SHARE: f32 = 0.7;

/// The part of a Seiza score the choice reads.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(super) struct Score {
    pub score: f32,
    pub background_variation: f32,
}

#[derive(Serialize, Deserialize)]
struct CachedScore {
    stacking_version: String,
    /// `None` when Seiza could not score the frame.
    score: Option<Score>,
}

fn cache_path(stack_root: &Path, source_fingerprint: &str) -> PathBuf {
    let digest = Sha256::digest(source_fingerprint.as_bytes());
    let name = digest
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    crate::server::storage::stack_folder(
        stack_root,
        crate::server::storage::stack_kind::REFERENCE_SCORES,
    )
    .join(format!("{name}.json"))
}

fn read_cached(stack_root: &Path, source_fingerprint: &str) -> Option<Option<Score>> {
    let bytes = std::fs::read(cache_path(stack_root, source_fingerprint)).ok()?;
    let cached: CachedScore = serde_json::from_slice(&bytes).ok()?;
    (cached.stacking_version == SEIZA_STACKING_VERSION).then_some(cached.score)
}

fn write_cached(stack_root: &Path, source_fingerprint: &str, score: Option<Score>) {
    let path = cache_path(stack_root, source_fingerprint);
    let record = CachedScore {
        stacking_version: SEIZA_STACKING_VERSION.into(),
        score,
    };
    if let Err(error) = super::stretch::write_json_atomic(&path, &record) {
        // Only costs a read next time.
        tracing::debug!("Could not cache a reference score: {error}");
    }
}

/// Score one frame, read through PSF Guard's frame reader so an XISF frame
/// declaring a 0..1 range sits on the same scale as the rest. `Err` when
/// the frame could not be read or scoring failed outright, which may pass;
/// `Ok(None)` when Seiza read it and found nothing to score, which will not.
fn score_frame(path: &Path) -> Result<Option<Score>, ()> {
    let frame = crate::image_io::open_linear_frame(path).map_err(|error| {
        tracing::debug!(
            "Reference scoring could not read {}: {error}",
            path.display()
        );
    })?;
    // One frame Seiza cannot handle must not take the whole job with it.
    std::panic::catch_unwind(|| seiza_stacking::reference_score(&frame))
        .map_err(|_| {
            tracing::warn!("Reference scoring panicked on {}", path.display());
        })
        .map(|score| {
            score.map(|score| Score {
                score: score.score,
                background_variation: score.background_variation,
            })
        })
}

/// Remove cached scores another Seiza version wrote; nothing reads them.
pub(super) fn prune(stack_root: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(crate::server::storage::stack_folder(
        stack_root,
        crate::server::storage::stack_kind::REFERENCE_SCORES,
    )) else {
        return 0;
    };
    let mut removed = 0;
    for path in entries.flatten().map(|entry| entry.path()) {
        let current = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<CachedScore>(&bytes).ok())
            .is_some_and(|cached| cached.stacking_version == SEIZA_STACKING_VERSION);
        if !current
            && path
                .extension()
                .is_some_and(|extension| extension == "json")
            && std::fs::remove_file(&path).is_ok()
        {
            removed += 1;
        }
    }
    removed
}

/// The index of the chosen frame, or `None` when no frame scored.
pub(super) fn choose(scores: &[Option<Score>]) -> Option<usize> {
    let top = scores
        .iter()
        .flatten()
        .map(|score| score.score)
        .fold(f32::NEG_INFINITY, f32::max);
    scores
        .iter()
        .enumerate()
        .filter_map(|(index, score)| score.map(|score| (index, score)))
        .filter(|(_, score)| score.score >= CANDIDATE_SHARE * top)
        .min_by(|left, right| {
            left.1
                .background_variation
                .total_cmp(&right.1.background_variation)
        })
        .map(|(index, _)| index)
}

/// Move `chosen` to the front and put the rest back in the order the build
/// pushes them: capture order, or best-graded first.
pub(super) fn reorder(frames: &mut Vec<PreparedFrame>, chosen: usize, order: snr::StackFrameOrder) {
    let reference = frames.remove(chosen);
    match order {
        snr::StackFrameOrder::Capture => {
            frames.sort_by_key(|frame| (frame.acquired_date.unwrap_or(0), frame.image_id));
        }
        snr::StackFrameOrder::Quality => frames.sort_by(|left, right| {
            right
                .quality_score
                .unwrap_or(0.0)
                .total_cmp(&left.quality_score.unwrap_or(0.0))
                .then_with(|| left.acquired_date.cmp(&right.acquired_date))
                .then_with(|| left.image_id.cmp(&right.image_id))
        }),
    }
    frames.insert(0, reference);
}

/// Score every frame of a group, from the cache where it can, a few frames
/// at a time on `pool`. `progress` hears how many are done. Returns `None`
/// when cancelled.
pub(super) fn scores(
    frames: &[PreparedFrame],
    stack_root: &Path,
    pool: &rayon::ThreadPool,
    cancel: &AtomicBool,
    mut progress: impl FnMut(usize, usize),
) -> Option<Vec<Option<Score>>> {
    let mut scores = frames
        .iter()
        .map(|frame| read_cached(stack_root, &frame.source_fingerprint))
        .collect::<Vec<_>>();
    let missing = scores
        .iter()
        .enumerate()
        .filter(|(_, score)| score.is_none())
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let mut done = frames.len() - missing.len();
    progress(done, frames.len());
    // Each worker holds one frame, so a chunk is as wide as the pool.
    for chunk in missing.chunks(pool.current_num_threads().max(1)) {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        let measured = pool.install(|| {
            chunk
                .par_iter()
                .map(|&index| score_frame(&frames[index].path))
                .collect::<Vec<_>>()
        });
        for (&index, measured) in chunk.iter().zip(measured) {
            // Only Seiza's own answer is kept; a failed read is retried next
            // time.
            if let Ok(score) = measured {
                write_cached(stack_root, &frames[index].source_fingerprint, score);
            }
            scores[index] = Some(measured.ok().flatten());
        }
        done += chunk.len();
        progress(done, frames.len());
    }
    Some(scores.into_iter().map(Option::flatten).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn score(score: f32, background_variation: f32) -> Option<Score> {
        Some(Score {
            score,
            background_variation,
        })
    }

    #[test]
    fn the_flattest_sky_wins_among_frames_near_the_best() {
        // 0 is best but hazy; 2 is close enough and flatter; 3 is flattest
        // but its stars are too poor to be considered.
        let scores = [score(10.0, 5.0), None, score(7.5, 2.0), score(5.0, 1.0)];
        assert_eq!(choose(&scores), Some(2));
        assert_eq!(choose(&[None, None]), None);
    }

    fn frame(image_id: i32, acquired_date: i64, quality_score: f64) -> PreparedFrame {
        PreparedFrame {
            image_id,
            acquired_date: Some(acquired_date),
            quality_score: Some(quality_score),
            path: PathBuf::new(),
            source_fingerprint: image_id.to_string(),
            expected_target: None,
            exposure_seconds: 60.0,
        }
    }

    #[test]
    fn the_chosen_frame_leads_and_the_rest_keep_the_build_order() {
        // Best-graded first, then capture order: how the job plans a group.
        let planned = || {
            vec![
                frame(3, 30, 0.9),
                frame(1, 10, 0.5),
                frame(2, 20, 0.7),
                frame(4, 40, 0.6),
            ]
        };
        let ids = |frames: &[PreparedFrame]| frames.iter().map(|f| f.image_id).collect::<Vec<_>>();

        let mut capture = planned();
        reorder(&mut capture, 2, snr::StackFrameOrder::Capture);
        assert_eq!(ids(&capture), [2, 1, 3, 4]);

        let mut quality = planned();
        reorder(&mut quality, 3, snr::StackFrameOrder::Quality);
        assert_eq!(ids(&quality), [4, 3, 2, 1]);
    }

    #[test]
    fn scores_are_cached_by_fingerprint_and_version() {
        let cache = tempfile::tempdir().unwrap();
        assert_eq!(read_cached(cache.path(), "a"), None);
        write_cached(cache.path(), "a", score(3.0, 1.0));
        write_cached(cache.path(), "b", None);
        assert_eq!(read_cached(cache.path(), "a"), Some(score(3.0, 1.0)));
        // A frame Seiza could not score is remembered as such.
        assert_eq!(read_cached(cache.path(), "b"), Some(None));
        let stale = CachedScore {
            stacking_version: "0.0.0".into(),
            score: score(1.0, 1.0),
        };
        std::fs::write(
            cache_path(cache.path(), "a"),
            serde_json::to_vec(&stale).unwrap(),
        )
        .unwrap();
        assert_eq!(read_cached(cache.path(), "a"), None);
    }

    #[test]
    fn an_unreadable_frame_is_not_remembered_and_old_versions_are_pruned() {
        let cache = tempfile::tempdir().unwrap();
        let missing = frame(9, 0, 0.5);
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(1)
            .build()
            .unwrap();
        let scores = scores(
            std::slice::from_ref(&missing),
            cache.path(),
            &pool,
            &AtomicBool::new(false),
            |_, _| {},
        )
        .unwrap();
        assert_eq!(scores, vec![None]);
        assert_eq!(read_cached(cache.path(), &missing.source_fingerprint), None);

        write_cached(cache.path(), "kept", score(1.0, 1.0));
        let stale = CachedScore {
            stacking_version: "0.0.0".into(),
            score: None,
        };
        std::fs::write(
            cache_path(cache.path(), "old"),
            serde_json::to_vec(&stale).unwrap(),
        )
        .unwrap();
        assert_eq!(prune(cache.path()), 1);
        assert_eq!(read_cached(cache.path(), "kept"), Some(score(1.0, 1.0)));
    }
}
