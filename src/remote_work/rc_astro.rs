//! One RC-Astro tool over one image, as a worker task.
//!
//! The server writes the image to `input.fits` and sends the tool's schema,
//! the request and the headers rc-astro reads. The worker runs the tool and
//! writes `processed.fits`, and `stars.fits` when star removal kept the
//! stars. The server picks the schema, so every worker that claims the task
//! runs the same tool version, which is part of the result's cache identity.

use std::path::{Path, PathBuf};

use seiza_stacking::{ExternalToolRequest, ExternalToolSchema, LinearImage, RcAstroCli};
use serde::{Deserialize, Serialize};

use super::protocol::HeaderCard;

pub const INPUT: &str = "input.fits";
pub const PROCESSED: &str = "processed.fits";
pub const STARS: &str = "stars.fits";

/// The tools PSF Guard runs, in the order a chain runs them.
pub const TOOLS: [&str; 3] = ["bxt", "nxt", "sxt"];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepSpec {
    pub schema: ExternalToolSchema,
    pub request: ExternalToolRequest,
    /// The headers rc-astro reads with the image.
    pub headers: Vec<HeaderCard>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StepResult {
    /// The compute device rc-astro reported, such as a GPU's name.
    pub device: Option<String>,
    pub warnings: Vec<String>,
}

/// The installed rc-astro: the one `executable` gives, else the one
/// `PSF_GUARD_RC_ASTRO` gives, else the one on the standard paths.
/// `PSF_GUARD_RC_ASTRO=off` leaves rc-astro to the connected workers.
pub fn locate(executable: Option<&Path>) -> Option<RcAstroCli> {
    let variable = std::env::var_os("PSF_GUARD_RC_ASTRO").filter(|path| !path.is_empty());
    if executable.is_none()
        && variable
            .as_ref()
            .is_some_and(|value| value.eq_ignore_ascii_case("off"))
    {
        return None;
    }
    // A service unit often runs with a minimal PATH; PSF_GUARD_RC_ASTRO
    // gives the executable directly.
    let configured = executable
        .map(Path::to_path_buf)
        .or_else(|| variable.map(PathBuf::from));
    let located = match configured {
        Some(path) => Some(RcAstroCli::with_executable(path)),
        None => RcAstroCli::locate(),
    };
    located.map(|cli| cli.with_host(format!("psf-guard-{}", env!("CARGO_PKG_VERSION"))))
}

/// Every tool's live schema, and the last problem reading one. Spawns
/// short-lived processes.
pub fn probe(cli: &RcAstroCli) -> (Vec<ExternalToolSchema>, Option<String>) {
    let mut tools = Vec::new();
    let mut problem = None;
    for tool in TOOLS {
        match cli.tool_schema(tool) {
            Ok(schema) => tools.push(schema),
            Err(error) => problem = Some(error.to_string()),
        }
    }
    (tools, problem)
}

/// Write an image for a task, through a temporary file.
pub fn write_image(path: &Path, image: &LinearImage) -> Result<(), String> {
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    seiza_stacking::write_processed_image_fits_f32(&temporary, image, &[], &[])
        .map_err(|error| format!("Writing {}: {error}", path.display()))?;
    std::fs::rename(&temporary, path).map_err(|error| {
        let _ = std::fs::remove_file(&temporary);
        format!("Writing {}: {error}", path.display())
    })
}

/// Read an image a task wrote.
pub fn read_image(path: &Path) -> Result<LinearImage, String> {
    crate::image_io::open_linear_frame(path)
        .map(|frame| frame.image)
        .map_err(|error| format!("Reading {}: {error}", path.display()))
}

/// Run the step: read `input`, run the tool, and write its outputs into
/// `output_dir`. `cancel` kills the tool when it fires.
pub fn run(
    cli: &RcAstroCli,
    spec: &StepSpec,
    input: &Path,
    output_dir: &Path,
    cancel: &seiza_stacking::CancelSignal,
    progress: &mut dyn FnMut(f32),
) -> Result<StepResult, String> {
    let image = read_image(input)?;
    let headers = HeaderCard::to_fits(&spec.headers);
    let processed = cli
        .process_image(
            &spec.schema,
            &spec.request,
            &image,
            &headers,
            Some(cancel),
            progress,
        )
        .map_err(|error| error.to_string())?;
    write_image(&output_dir.join(PROCESSED), &processed.image)?;
    if let Some(stars) = &processed.stars {
        write_image(&output_dir.join(STARS), stars)?;
    }
    Ok(StepResult {
        device: processed.device,
        warnings: processed.warnings,
    })
}
