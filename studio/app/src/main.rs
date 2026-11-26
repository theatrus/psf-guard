//! A GPUI window hosting the tiled wgpu canvas of the PSF-Studio spike.
//!
//! GPUI draws the panel and receives all input. The canvas is a native view
//! that GPUI composites beneath its own content; wgpu owns that view's layer.
//!
//! Run with `--bench <file>` to play the scripted measurement and write its
//! results as JSON.

// The document model lives in PSF Guard's main crate. The spike compiles the
// same source files, so the panes are written against the real model
// without building all of PSF Guard under GPUI's toolchain.
#[cfg(target_os = "macos")]
#[allow(dead_code, unused_imports)]
#[path = "../../../src/image_steps/mod.rs"]
mod image_steps;
#[cfg(target_os = "macos")]
#[allow(dead_code, unused_imports)]
#[path = "../../../src/studio/mod.rs"]
mod studio_model;

#[cfg(target_os = "macos")]
mod bench;
#[cfg(target_os = "macos")]
mod output_stretch;
#[cfg(target_os = "macos")]
mod store_tiles;
#[cfg(target_os = "macos")]
mod studio;

#[cfg(target_os = "macos")]
fn main() {
    let mut arguments = std::env::args().skip(1);
    let mut options = studio::Options::default();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--bench" => options.bench_output = arguments.next().map(Into::into),
            "--no-vsync" => options.vsync = false,
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
            "--working" => options.working_profile = arguments.next().map(Into::into),
            "--capture" => options.capture_prefix = arguments.next().map(Into::into),
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
