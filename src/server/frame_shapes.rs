//! Frame width and height for the grid's thumbnails.
//!
//! The Images grid and the Sequence strip shape each card's preview like
//! the frame, so a square or 4:3 sensor does not sit in a 3:2 box with bands
//! at its sides. The catalog does not record a frame's size, but everything
//! that sets it does: the N.I.N.A. profile (one camera), its binning and its
//! ROI. Frames that share those share a size, so one header read answers for
//! all of them. A catalog holds a handful of such setups, and the answer is
//! kept for the life of the server.
//!
//! The image list waits a short while for a setup it has not seen. A share
//! slower than that still answers: the read carries on and the next load
//! has the size. A frame whose size is not known shows a 3:2 preview.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::models::AcquiredImage;

/// How long the image list waits for the sizes it has not seen.
const PROBE_WAIT: Duration = Duration::from_millis(1500);
/// How long a setup whose frames could not be read waits before another try.
const RETRY_AFTER: Duration = Duration::from_secs(600);
/// Frames tried per setup before it counts as unreadable for now.
const CANDIDATES_PER_SETUP: usize = 3;

/// A frame's size in pixels, as its header's `NAXIS1` and `NAXIS2`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct FrameSize {
    pub width: u32,
    pub height: u32,
}

/// What sets a frame's size: the profile's camera, its binning and its ROI.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SetupKey {
    profile_id: Option<String>,
    binning: String,
    roi: String,
}

impl SetupKey {
    pub fn of(image: &AcquiredImage, metadata: &serde_json::Value) -> Self {
        let text = |key: &str| match &metadata[key] {
            serde_json::Value::String(value) => value.trim().to_string(),
            serde_json::Value::Number(value) => value.to_string(),
            _ => String::new(),
        };
        Self {
            profile_id: image.profile_id.clone(),
            binning: text("Binning"),
            roi: text("ROI"),
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum Entry {
    Known(FrameSize),
    /// A read is under way.
    Reading,
    /// No frame of the setup could be read at this time.
    Unreadable(Instant),
}

/// One database's frame sizes by setup.
#[derive(Debug, Default)]
pub struct FrameShapes {
    entries: HashMap<SetupKey, Entry>,
}

impl FrameShapes {
    fn known(&self, key: &SetupKey) -> Option<FrameSize> {
        match self.entries.get(key) {
            Some(Entry::Known(size)) => Some(*size),
            _ => None,
        }
    }

    /// Whether to read this setup now, marking it as being read if so.
    fn claim(&mut self, key: &SetupKey, now: Instant) -> bool {
        let due = match self.entries.get(key) {
            None => true,
            Some(Entry::Unreadable(since)) => now.duration_since(*since) >= RETRY_AFTER,
            Some(Entry::Known(_) | Entry::Reading) => false,
        };
        if due {
            self.entries.insert(key.clone(), Entry::Reading);
        }
        due
    }

    fn settle(&mut self, key: SetupKey, size: Option<FrameSize>, now: Instant) {
        let entry = match size {
            Some(size) => Entry::Known(size),
            None => Entry::Unreadable(now),
        };
        self.entries.insert(key, entry);
    }
}

/// A frame of one setup the list can read the size from: the row, its
/// target's name, and the file name the catalog holds.
pub struct Candidate {
    pub image: AcquiredImage,
    pub target_name: String,
    pub file_name: String,
}

/// The size a frame's header declares, read without its pixels.
pub fn read_frame_size(path: &Path) -> Option<FrameSize> {
    let headers = seiza_image_io::read_header(path)
        .map(|header| header.cards)
        .ok()?;
    let axis = |key: &str| -> Option<u32> {
        headers
            .iter()
            .find(|(name, _)| name == key)
            .and_then(|(_, value)| value.as_i64())
            .and_then(|value| u32::try_from(value).ok())
            .filter(|value| *value > 0)
    };
    Some(FrameSize {
        width: axis("NAXIS1")?,
        height: axis("NAXIS2")?,
    })
}

/// The size of every setup in `candidates` that is known or can be read
/// within [`PROBE_WAIT`]. `resolve` finds a candidate's file; it runs on
/// the blocking pool, as do the header reads.
pub async fn sizes_for<R>(
    cache: &Arc<Mutex<FrameShapes>>,
    candidates: HashMap<SetupKey, Vec<Candidate>>,
    resolve: R,
) -> HashMap<SetupKey, FrameSize>
where
    R: Fn(&Candidate) -> Option<std::path::PathBuf> + Send + Sync + 'static,
{
    let now = Instant::now();
    let mut sizes = HashMap::new();
    let mut to_read = Vec::new();
    {
        let mut shapes = cache.lock().unwrap_or_else(|poison| poison.into_inner());
        for (key, frames) in candidates {
            if let Some(size) = shapes.known(&key) {
                sizes.insert(key, size);
            } else if shapes.claim(&key, now) {
                to_read.push((key, frames));
            }
        }
    }
    if to_read.is_empty() {
        return sizes;
    }

    let keys: Vec<SetupKey> = to_read.iter().map(|(key, _)| key.clone()).collect();
    let writer = Arc::clone(cache);
    let reading = tokio::task::spawn_blocking(move || {
        for (key, frames) in to_read {
            let size = frames
                .iter()
                .take(CANDIDATES_PER_SETUP)
                .find_map(|frame| resolve(frame).and_then(|path| read_frame_size(&path)));
            writer
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .settle(key, size, Instant::now());
        }
    });
    // A slow share keeps reading after the wait; the next list has it.
    let _ = tokio::time::timeout(PROBE_WAIT, reading).await;

    let shapes = cache.lock().unwrap_or_else(|poison| poison.into_inner());
    for key in keys {
        if let Some(size) = shapes.known(&key) {
            sizes.insert(key, size);
        }
    }
    sizes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(profile: &str) -> AcquiredImage {
        AcquiredImage {
            id: 1,
            project_id: 1,
            target_id: 1,
            acquired_date: None,
            filter_name: "L".into(),
            grading_status: 0,
            metadata: String::new(),
            reject_reason: None,
            profile_id: Some(profile.into()),
            guid: None,
        }
    }

    #[test]
    fn a_setup_is_its_profile_binning_and_roi() {
        let full = serde_json::json!({ "Binning": "1x1", "ROI": 100.0 });
        let binned = serde_json::json!({ "Binning": "2x2", "ROI": 100.0 });
        assert_eq!(
            SetupKey::of(&image("a"), &full),
            SetupKey::of(&image("a"), &full)
        );
        assert_ne!(
            SetupKey::of(&image("a"), &full),
            SetupKey::of(&image("a"), &binned)
        );
        assert_ne!(
            SetupKey::of(&image("a"), &full),
            SetupKey::of(&image("b"), &full)
        );
    }

    #[test]
    fn a_setup_is_read_once_and_retried_only_after_a_while() {
        let key = SetupKey::of(&image("a"), &serde_json::json!({}));
        let start = Instant::now();
        let mut shapes = FrameShapes::default();
        assert!(shapes.claim(&key, start));
        // Being read: another list does not read it again.
        assert!(!shapes.claim(&key, start));
        shapes.settle(key.clone(), None, start);
        assert!(!shapes.claim(&key, start + Duration::from_secs(5)));
        assert!(shapes.claim(&key, start + RETRY_AFTER));
        let size = FrameSize {
            width: 4144,
            height: 2822,
        };
        shapes.settle(key.clone(), Some(size), start);
        assert_eq!(shapes.known(&key), Some(size));
        assert!(!shapes.claim(&key, start + RETRY_AFTER * 2));
    }

    /// A FITS header declaring a `width` x `height` frame, then one block
    /// of zero pixels.
    fn write_fits(path: &Path, width: u32, height: u32) {
        let cards = [
            "SIMPLE  =                    T".to_string(),
            format!("BITPIX  = {:>20}", 16),
            "NAXIS   =                    2".to_string(),
            format!("NAXIS1  = {width:>20}"),
            format!("NAXIS2  = {height:>20}"),
            "END".to_string(),
        ];
        let mut bytes: Vec<u8> = cards
            .iter()
            .flat_map(|card| format!("{card:<80}").into_bytes())
            .collect();
        bytes.resize(2880, b' ');
        bytes.resize(2880 * 2, 0);
        std::fs::write(path, bytes).unwrap();
    }

    #[tokio::test]
    async fn the_list_answers_from_one_frame_per_setup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("frame.fits");
        write_fits(&path, 4, 3);
        let cache = Arc::new(Mutex::new(FrameShapes::default()));
        let key = SetupKey::of(&image("a"), &serde_json::json!({ "Binning": "1x1" }));
        let reads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let candidates = || {
            HashMap::from([(
                key.clone(),
                vec![Candidate {
                    image: image("a"),
                    target_name: "M1".into(),
                    file_name: "frame.fits".into(),
                }],
            )])
        };
        let resolver = |reads: Arc<std::sync::atomic::AtomicUsize>, path: std::path::PathBuf| {
            move |_: &Candidate| {
                reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Some(path.clone())
            }
        };
        let sizes = sizes_for(&cache, candidates(), resolver(reads.clone(), path.clone())).await;
        assert_eq!(
            sizes.get(&key),
            Some(&FrameSize {
                width: 4,
                height: 3
            })
        );
        // The second list reads nothing.
        let again = sizes_for(&cache, candidates(), resolver(reads.clone(), path)).await;
        assert_eq!(
            again.get(&key),
            Some(&FrameSize {
                width: 4,
                height: 3
            })
        );
        assert_eq!(reads.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}
