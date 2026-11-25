use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::time::{Instant, UNIX_EPOCH};

use crate::studio::{MemoryBudget, RegionReader, SampleFormat, StoreOptions, TileStore};
use crate::studio_frames::FrameSource;

/// Writes a PSF-Studio tile store from a FITS or XISF frame.
///
/// The frame is read by region and the store is written tile by tile, so
/// neither is held in memory.
pub fn studio_store(input: &str, output: Option<String>, half_floats: bool) -> Result<()> {
    let input = Path::new(input);
    let output = output.map_or_else(|| input.with_extension("pgtiles"), PathBuf::from);
    let frame = FrameSource::open(input)?;
    let shape = frame.shape();

    // The key changes when the frame is replaced or the format changes.
    let metadata =
        std::fs::metadata(input).with_context(|| format!("reading {}", input.display()))?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |since| since.as_nanos());
    let sample = if half_floats {
        SampleFormat::F16
    } else {
        SampleFormat::F32
    };
    let options = StoreOptions {
        sample,
        budget: MemoryBudget::new(1 << 30),
        key: format!(
            "frame:{}:{}:{modified}:{sample:?}",
            input.display(),
            metadata.len()
        ),
        ..StoreOptions::new("")
    };

    let started = Instant::now();
    let store = TileStore::create(&output, &frame, &options)?;
    let bytes = std::fs::metadata(&output)
        .with_context(|| format!("reading {}", output.display()))?
        .len();
    println!(
        "Wrote {}: {} x {} pixels, {} channel(s), {} levels, {:.1} MB, in {:.1} s",
        output.display(),
        shape.width,
        shape.height,
        shape.channels,
        store.levels(),
        bytes as f64 / (1 << 20) as f64,
        started.elapsed().as_secs_f64()
    );
    Ok(())
}
