#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

//! A GPUI window hosting the tiled wgpu canvas of the PSF-Studio spike.
//!
//! GPUI draws the panel and receives all input. Seiza draws the canvas with
//! wgpu, into a native view on macOS and as an image in GPUI on Windows.
//!
//! Run with `--bench <file>` to play the scripted measurement and write its
//! results as JSON.

// The document model, shared with PSF Guard through `psf-studio-core`, so
// the two programs read and write the same documents and stores.
#[cfg(any(target_os = "macos", windows))]
use psf_studio_core::{image_steps, studio as studio_model};

#[cfg(any(target_os = "macos", windows))]
mod bench;
/// What the canvas draws, and the Seiza picture it becomes.
#[cfg(any(target_os = "macos", windows))]
mod canvas;
/// Planes worked out from the whole image while a person edits.
#[cfg(any(target_os = "macos", windows))]
mod derived;
/// The histogram of the finished picture.
#[cfg(any(target_os = "macos", windows))]
mod histogram;
/// The output stretches of the combined picture.
#[cfg(any(target_os = "macos", windows))]
mod output_stretch;
#[cfg(any(target_os = "macos", windows))]
mod store_tiles;
#[cfg(any(target_os = "macos", windows))]
mod studio;
/// The Tone and color step's settings.
#[cfg(any(target_os = "macos", windows))]
mod tone_step;

#[cfg(any(target_os = "macos", windows))]
fn main() {
    let mut arguments = std::env::args().skip(1).peekable();
    #[cfg(windows)]
    if arguments
        .peek()
        .is_some_and(|argument| argument == "--open-link")
    {
        open_windows_link(arguments.skip(1));
        return;
    }
    let mut options = studio::Options::default();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--bench" => options.bench_output = arguments.next().map(Into::into),
            "--no-vsync" => options.vsync = false,
            #[cfg(target_os = "macos")]
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

/// The protocol handler accepts exactly one link. It never reads Studio's CLI flags.
#[cfg(windows)]
fn protocol_link(mut args: impl Iterator<Item = String>) -> Option<String> {
    let link = args.next()?;
    (link.starts_with("psf-studio://") && args.next().is_none()).then_some(link)
}

#[cfg(windows)]
fn open_windows_link(args: impl Iterator<Item = String>) {
    let Some(link) = protocol_link(args) else {
        eprintln!("Expected one psf-studio:// link after --open-link.");
        std::process::exit(2);
    };
    studio::queue_link(link);
    studio::run(studio::Options::default());
}

#[cfg(all(test, windows))]
mod tests {
    use super::protocol_link;

    #[test]
    fn protocol_handler_rejects_extra_arguments() {
        let link = "psf-studio://open?server=http%3A%2F%2Flocalhost&db=local&project=1&target=2";
        assert_eq!(
            protocol_link([link.to_owned()].into_iter()).as_deref(),
            Some(link)
        );
        assert_eq!(
            protocol_link([link, "--make-store"].map(str::to_owned).into_iter()),
            None
        );
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
fn main() {
    eprintln!("PSF Studio runs on macOS and Windows.");
    std::process::exit(1);
}
