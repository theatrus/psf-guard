//! A GPUI window hosting the tiled wgpu canvas of the PSF-Studio spike.
//!
//! GPUI draws the panel and receives all input. The canvas is a native view
//! that GPUI composites beneath its own content; wgpu owns that view's layer.
//!
//! Run with `--bench <file>` to play the scripted measurement and write its
//! results as JSON.

// The document model, shared with PSF Guard through `psf-studio-core`, so
// the two programs read and write the same documents and stores.
#[cfg(target_os = "macos")]
use psf_studio_core::{image_steps, studio as studio_model};

#[cfg(target_os = "macos")]
mod bench;
/// What the canvas draws, and the Seiza picture it becomes.
#[cfg(target_os = "macos")]
mod canvas;
/// Planes worked out from the whole image while a person edits.
#[cfg(target_os = "macos")]
mod derived;
/// The histogram of the finished picture.
#[cfg(target_os = "macos")]
mod histogram;
/// The output stretches of the combined picture.
#[cfg(target_os = "macos")]
mod output_stretch;
#[cfg(target_os = "macos")]
mod store_tiles;
#[cfg(target_os = "macos")]
mod studio;
/// The Tone and color step's settings.
#[cfg(target_os = "macos")]
mod tone_step;

#[cfg(target_os = "macos")]
fn main() {
    let mut arguments = std::env::args().skip(1);
    let mut options = studio::Options::default();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--bench" => options.bench_output = arguments.next().map(Into::into),
            "--no-vsync" => options.vsync = false,
            "--no-control" => options.control = false,
            "--open" => options.open = arguments.next().map(Into::into),
            "--document" => options.document = arguments.next().map(Into::into),
            "--psf-guard" => options.psf_guard = arguments.next().map(Into::into),
            "--make-store" => {
                // Writes part of the synthetic document to a tile store and
                // exits: something to open with --open.
                let path: std::path::PathBuf = arguments.next().unwrap_or_default().into();
                let mut size = |default: usize| {
                    arguments
                        .next()
                        .and_then(|value| value.parse().ok())
                        .unwrap_or(default)
                };
                let (width, height) = (size(12_000), size(8000));
                let started = std::time::Instant::now();
                match store_tiles::write_synthetic_store(&path, width, height) {
                    Ok(()) => println!(
                        "wrote {} in {:.1} s",
                        path.display(),
                        started.elapsed().as_secs_f64()
                    ),
                    Err(error) => {
                        eprintln!("{error}");
                        std::process::exit(1);
                    }
                }
                return;
            }
            "--output" => options.output = arguments.next().unwrap_or_default(),
            "--capture" => options.capture_prefix = arguments.next().map(Into::into),
            "--parity-report" => options.parity_report = arguments.next().map(Into::into),
            "--frames" => {
                options.bench_frames = arguments
                    .next()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(options.bench_frames);
            }
            other => {
                eprintln!("unknown argument: {other}");
                std::process::exit(2);
            }
        }
    }
    studio::run(options);
}

#[cfg(not(target_os = "macos"))]
fn main() {
    // Upstream GPUI composites a native view on macOS only. See the results
    // in docs/design/psf-studio.md for what Windows and Linux need.
    eprintln!("This spike runs on macOS only.");
}
