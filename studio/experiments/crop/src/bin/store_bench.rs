//! Measures the tile store in `crates/psf-studio-core/src/studio/store.rs`: how fast it writes and
//! reads, and how large its files are.
//!
//! usage: store_bench <folder> <results.json> [width height channels]
//!
//! The source is computed as it is read, so the image is never in memory.
//! Run it under `/usr/bin/time -l` to see the process's peak memory.

use psf_studio_core::{image_steps, studio};

use image_steps::{Domain, ImageShape, Region};
use serde_json::json;
use std::path::Path;
use std::time::Instant;
use studio::{MemoryBudget, RegionReader, SampleFormat, StoreOptions, StudioError, TileStore};

/// An image computed from pixel coordinates.
struct Computed(ImageShape);

impl RegionReader for Computed {
    fn shape(&self) -> ImageShape {
        self.0
    }
    fn domain(&self) -> Domain {
        Domain::Linear
    }
    fn peak_bytes(&self, region: Region) -> u64 {
        (region.width * region.height * self.0.channels * 4) as u64
    }
    fn read(&self, region: Region) -> Result<Vec<f32>, StudioError> {
        let mut out = Vec::with_capacity(region.width * region.height * self.0.channels);
        for y in region.y..region.bottom() {
            for x in region.x..region.right() {
                // Smooth structure plus per-pixel noise, like a stack.
                let smooth = ((x as f32) * 0.002).sin() * ((y as f32) * 0.003).cos();
                let hash =
                    (x as u32).wrapping_mul(0x9e37_79b9) ^ (y as u32).wrapping_mul(0x85eb_ca6b);
                let noise = (hash >> 8) as f32 / (1 << 24) as f32;
                for channel in 0..self.0.channels {
                    out.push(0.02 + 0.01 * smooth + 0.004 * noise + 0.001 * channel as f32);
                }
            }
        }
        Ok(out)
    }
}

fn seconds(started: Instant) -> f64 {
    started.elapsed().as_secs_f64()
}

/// The median time of reading each region, in milliseconds.
fn read_ms(reader: &dyn RegionReader, regions: &[Region]) -> f64 {
    let mut times: Vec<f64> = regions
        .iter()
        .map(|region| {
            let started = Instant::now();
            std::hint::black_box(reader.read(*region).expect("a region in the image"));
            seconds(started) * 1000.0
        })
        .collect();
    times.sort_by(f64::total_cmp);
    times[times.len() / 2]
}

fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let [folder, output, rest @ ..] = arguments.as_slice() else {
        eprintln!("usage: store_bench <folder> <results.json> [width height channels]");
        std::process::exit(2);
    };
    let number = |index: usize, default: usize| {
        rest.get(index)
            .and_then(|value| value.parse().ok())
            .unwrap_or(default)
    };
    let shape = ImageShape::new(number(0, 12_000), number(1, 8000), number(2, 3));
    let source = Computed(shape);
    let megabytes = |bytes: u64| bytes as f64 / (1 << 20) as f64;
    let raw = (shape.width * shape.height * shape.channels * 4) as u64;

    // How long the source alone takes, so it can be taken off the write time.
    let started = Instant::now();
    for y in (0..shape.height).step_by(512) {
        for x in (0..shape.width).step_by(512) {
            let region = Region::new(x, y, 512.min(shape.width - x), 512.min(shape.height - y));
            std::hint::black_box(source.read(region).expect("a tile"));
        }
    }
    let source_seconds = seconds(started);

    let mut state = 0x9e37_79b9_7f4a_7c15u64;
    let mut random = move |limit: usize| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state % limit.max(1) as u64) as usize
    };

    let mut results = Vec::new();
    for sample in [SampleFormat::F32, SampleFormat::F16] {
        let path = Path::new(folder).join(format!("bench-{sample:?}.pgtiles"));
        let options = StoreOptions {
            tile: 512,
            sample,
            budget: MemoryBudget::new(256 << 20),
            key: "bench".to_string(),
            channels: Vec::new(),
        };
        let started = Instant::now();
        let store = TileStore::create(&path, &source, &options).expect("writing the store");
        let write_seconds = seconds(started);
        let bytes = std::fs::metadata(&path).expect("the store").len();

        let level0 = store.level(0).expect("level 0");
        let crops: Vec<Region> = (0..200)
            .map(|_| {
                Region::new(
                    random(shape.width - 1024),
                    random(shape.height - 1024),
                    1024,
                    1024,
                )
            })
            .collect();
        // A 4K view of the image at one to one, and at the level that shows
        // all of it.
        let views: Vec<Region> = (0..50)
            .map(|_| {
                Region::new(
                    random(shape.width - 3840),
                    random(shape.height - 2160),
                    3840,
                    2160,
                )
            })
            .collect();
        let top = store.level(store.levels() - 1).expect("the top level");
        let top_shape = top.shape();

        results.push(json!({
            "sample": format!("{sample:?}"),
            "file_mb": megabytes(bytes),
            "file_over_raw_f32": bytes as f64 / raw as f64,
            "levels": store.levels(),
            "write_seconds": write_seconds,
            "write_seconds_without_the_source": write_seconds - source_seconds,
            "write_mb_per_second": megabytes(bytes) / (write_seconds - source_seconds),
            "read_1024_crop_ms": read_ms(&level0, &crops),
            "read_4k_view_ms": read_ms(&level0, &views),
            "read_whole_top_level_ms": read_ms(&top, &[top_shape.region()]),
            "top_level": [top_shape.width, top_shape.height],
        }));
        std::fs::remove_file(&path).expect("removing the store");
    }

    let report = json!({
        "image": { "width": shape.width, "height": shape.height, "channels": shape.channels },
        "raw_f32_mb": megabytes(raw),
        "source_seconds": source_seconds,
        "stores": results,
    });
    let text = serde_json::to_string_pretty(&report).expect("JSON");
    std::fs::write(output, &text).expect("writing the report");
    println!("{text}");
}
