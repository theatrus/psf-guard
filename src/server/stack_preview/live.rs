//! Temporary card images of a running stack. Rendering stays on the build's
//! leased pool, between preparation batches, without publishing a stack artifact.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use seiza_stacking::{LinearImage, LiveStacker};
use serde::{Deserialize, Serialize};

use super::{stack_dir, stretch};
use crate::server::state::AppState;

const INTERVAL: Duration = Duration::from_secs(5);
const MAX_DIMENSION: usize = 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LiveStackPreview {
    pub url: String,
    pub accepted_frames: u32,
}

pub(super) fn path(root: &Path, job: &str, group: usize) -> PathBuf {
    stack_dir(root, job).join(format!("group-{group}-live.png"))
}

/// When a running image is due, and how many frames each batch pushes, so
/// that a new image is due about every five seconds.
#[derive(Default)]
pub(super) struct Cadence {
    last_attempt: Option<(Instant, u32)>,
    frame_time: Option<Duration>,
}

impl Cadence {
    fn due(&self, now: Instant, frames: u32, force: bool) -> bool {
        self.last_attempt.is_none_or(|(at, previous)| {
            frames > previous && (force || now.duration_since(at) >= INTERVAL)
        })
    }

    pub fn observe_batch(&mut self, elapsed: Duration, frames: usize) {
        if frames > 0 {
            self.frame_time = Some(elapsed.div_f64(frames as f64));
        }
    }

    // The accumulator is borrowed by Seiza during a batch. Size the next one
    // from measured throughput so even deep stacks keep yielding a display,
    // without cancelling preparation and throwing away prefetched frames.
    pub fn batch_frames(&self) -> usize {
        self.frame_time.map_or(1, |frame| {
            (INTERVAL.as_secs_f64() / frame.as_secs_f64().max(0.001))
                .floor()
                .max(1.0) as usize
        })
    }

    /// Render `stacker`'s running image to `destination` when one is due.
    /// Returns `None` when none was due. A failure is not tried again until
    /// the next image is due, so it cannot slow every frame.
    pub fn render(
        &mut self,
        stacker: &LiveStacker,
        accepted: u32,
        force: bool,
        pool: &rayon::ThreadPool,
        destination: &Path,
    ) -> Option<Result<(), String>> {
        if !self.due(Instant::now(), accepted, force) {
            return None;
        }
        let result = pool.install(|| {
            // Export copies only the mean, with Seiza's coverage masking and
            // Bayer-drizzle gap filling. Preparation buffers have been released.
            let snapshot = stacker
                .export_snapshot()
                .map_err(|error| error.to_string())?;
            render(snapshot.image, destination)
        });
        self.last_attempt = Some((Instant::now(), accepted));
        Some(result)
    }
}

/// Show the running image at [`path`] on a channel's card.
fn show(state: &AppState, job_id: &str, group: usize, accepted: u32) {
    let revision = uuid::Uuid::new_v4();
    state.stack_previews.update(job_id, |job| {
        job.groups[group].live_preview = Some(LiveStackPreview {
            url: format!(
                "/api/db/{}/stack-previews/{}/{}/preview?live=true&v={revision}",
                job.database_id, job_id, group
            ),
            accepted_frames: accepted,
        });
    });
}

/// Show a running image a worker sent on a channel's card.
pub(super) fn show_received(
    state: &AppState,
    root: &Path,
    job_id: &str,
    group: usize,
    png: &[u8],
    accepted: u32,
) {
    let destination = path(root, job_id, group);
    let temporary = destination.with_extension(format!("{}.tmp.png", super::temporary_tag()));
    // Nothing else writes to a fresh build's folder before the worker's
    // outputs arrive.
    let written = std::fs::create_dir_all(destination.parent().expect("a job folder"))
        .and_then(|()| std::fs::write(&temporary, png))
        .and_then(|()| std::fs::rename(&temporary, &destination));
    match written {
        Ok(()) => show(state, job_id, group, accepted),
        Err(error) => {
            let _ = std::fs::remove_file(&temporary);
            tracing::warn!(
                job_id,
                group_index = group,
                "A worker's running image could not be written: {error}"
            );
        }
    }
}

pub(super) struct Publisher<'a> {
    state: &'a AppState,
    root: &'a Path,
    job: &'a str,
    group: usize,
    pool: &'a rayon::ThreadPool,
    cadence: Cadence,
}

impl<'a> Publisher<'a> {
    pub fn new(
        state: &'a AppState,
        root: &'a Path,
        job: &'a str,
        group: usize,
        pool: &'a rayon::ThreadPool,
    ) -> Self {
        Self {
            state,
            root,
            job,
            group,
            pool,
            cadence: Cadence::default(),
        }
    }

    pub fn batch_frames(&self) -> usize {
        self.cadence.batch_frames()
    }

    pub fn observe_batch(&mut self, elapsed: Duration, frames: usize) {
        self.cadence.observe_batch(elapsed, frames);
    }

    pub fn publish(&mut self, stacker: &LiveStacker, accepted: u32, force: bool) {
        let destination = path(self.root, self.job, self.group);
        // A display failure must not fail a build.
        match self
            .cadence
            .render(stacker, accepted, force, self.pool, &destination)
        {
            None => {}
            Some(Ok(())) => show(self.state, self.job, self.group, accepted),
            Some(Err(error)) => tracing::warn!(
                job_id = self.job,
                group_index = self.group,
                "Live stack preview failed: {error}"
            ),
        }
    }
}

fn screen_image(image: LinearImage) -> Result<LinearImage, String> {
    let factor = image
        .width
        .max(image.height)
        .div_ceil(MAX_DIMENSION)
        .min(image.width.min(image.height))
        .max(1);
    if factor == 1 {
        Ok(image)
    } else {
        image
            .binned(factor as u32)
            .map_err(|error| error.to_string())
    }
}

fn render(image: LinearImage, destination: &Path) -> Result<(), String> {
    // Drop the full-size mean before normalizing and stretching. Display
    // buffers are capped at roughly one megapixel for normal camera shapes.
    let image = screen_image(image)?;
    stretch::render_image_preview_atomic(
        &image,
        &stretch::default_linear_config(),
        stretch::StackStretchSourceTransfer::Linear,
        destination,
    )
    .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cadence_updates_only_after_new_frames_and_bounds_the_next_batch() {
        let now = Instant::now();
        let mut cadence = Cadence::default();
        assert!(cadence.due(now, 1, false));
        assert_eq!(cadence.batch_frames(), 1);
        cadence.last_attempt = Some((now, 1));
        assert!(!cadence.due(now + Duration::from_secs(4), 2, false));
        assert!(!cadence.due(now + INTERVAL, 1, false));
        assert!(cadence.due(now + INTERVAL, 2, false));
        assert!(cadence.due(now, 2, true));
        cadence.observe_batch(Duration::from_secs(2), 4);
        assert_eq!(cadence.batch_frames(), 10);
        cadence.observe_batch(Duration::from_secs(20), 1);
        assert_eq!(cadence.batch_frames(), 1);
    }

    #[test]
    fn live_render_is_bounded_and_leaves_finished_outputs_alone() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let destination = path(root, "job", 0);
        std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
        let final_preview = super::super::preview_path(root, "job", 0);
        let fits = super::super::fits_path(root, "job", 0);
        std::fs::write(&final_preview, b"finished preview").unwrap();
        std::fs::write(&fits, b"finished FITS").unwrap();
        for channels in [1, 3] {
            let image = LinearImage::new(
                2048,
                1536,
                channels,
                (0..2048 * 1536 * channels)
                    .map(|i| 100.0 + (i % 2000) as f32)
                    .collect(),
            )
            .unwrap();
            render(image, &destination).unwrap();
            let rendered = image::open(&destination).unwrap();
            assert_eq!((rendered.width(), rendered.height()), (1024, 768));
        }
        assert_eq!(std::fs::read(final_preview).unwrap(), b"finished preview");
        assert_eq!(std::fs::read(fits).unwrap(), b"finished FITS");
    }
}
