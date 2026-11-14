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
/// `PSF_GUARD_RC_ASTRO` gives, else the first on `PATH`, else the first
/// standard install (see [`standard_installs`]).
/// `PSF_GUARD_RC_ASTRO=off` leaves rc-astro to the connected workers.
pub fn locate(executable: Option<&Path>) -> Option<RcAstroCli> {
    let variable = std::env::var_os("PSF_GUARD_RC_ASTRO").filter(|path| !path.is_empty());
    if executable.is_none() && turned_off() {
        return None;
    }
    // A service unit often runs with a minimal PATH; PSF_GUARD_RC_ASTRO
    // gives the executable directly.
    let configured = executable
        .map(Path::to_path_buf)
        .or_else(|| variable.map(PathBuf::from));
    let located = match configured {
        Some(path) => Some(RcAstroCli::with_executable(path)),
        // An app opened from the Finder, the Dock or the Start menu, and a
        // service, get a PATH without the folders a terminal adds, so the
        // installers' own folders are tried too.
        None => RcAstroCli::locate().or_else(|| {
            standard_installs(std::env::consts::OS, &|name| std::env::var_os(name))
                .into_iter()
                .find(|candidate| candidate.is_file())
                .map(RcAstroCli::with_executable)
        }),
    };
    located.map(|cli| cli.with_host(format!("psf-guard-{}", env!("CARGO_PKG_VERSION"))))
}

/// Whether `PSF_GUARD_RC_ASTRO=off` leaves rc-astro to the connected
/// workers.
pub fn turned_off() -> bool {
    std::env::var_os("PSF_GUARD_RC_ASTRO").is_some_and(|value| value.eq_ignore_ascii_case("off"))
}

/// Where rc-astro is on `os` when it is not on `PATH`, in the order tried:
/// the folders a terminal's `PATH` usually has, in that order, then the
/// installer's own folder, which a link in one of them usually points to.
/// `variable` reads an environment variable.
pub fn standard_installs(
    os: &str,
    variable: &dyn Fn(&str) -> Option<std::ffi::OsString>,
) -> Vec<PathBuf> {
    let home = || {
        variable("HOME")
            .filter(|home| !home.is_empty())
            .map(PathBuf::from)
    };
    match os {
        "macos" => [
            Some(PathBuf::from("/opt/homebrew/bin/rc-astro")),
            Some(PathBuf::from("/usr/local/bin/rc-astro")),
            Some(PathBuf::from("/Applications/RC-Astro/CLI/rc-astro")),
            home().map(|home| home.join("Applications/RC-Astro/CLI/rc-astro")),
        ]
        .into_iter()
        .flatten()
        .collect(),
        "windows" => {
            let mut found: Vec<PathBuf> = ["ProgramFiles", "ProgramW6432"]
                .into_iter()
                .filter_map(|name| variable(name).filter(|folder| !folder.is_empty()))
                .map(|folder| {
                    PathBuf::from(folder)
                        .join("RC-Astro")
                        .join("CLI")
                        .join("rc-astro.exe")
                })
                .collect();
            found.dedup();
            found
        }
        // install.sh links /usr/local/bin/rc-astro to /opt/rc-astro.
        _ => [
            home().map(|home| home.join(".local/bin/rc-astro")),
            Some(PathBuf::from("/usr/local/bin/rc-astro")),
            Some(PathBuf::from("/opt/rc-astro/rc-astro")),
        ]
        .into_iter()
        .flatten()
        .collect(),
    }
}

/// Where [`locate`] looks when nothing names the executable, for a message
/// that says rc-astro was not found.
pub fn searched_places() -> String {
    let installs = standard_installs(std::env::consts::OS, &|name| std::env::var_os(name));
    let installs: Vec<String> = installs
        .iter()
        .map(|path| path.display().to_string())
        .collect();
    format!("PATH, {}", installs.join(", "))
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    fn environment(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let pairs: Vec<(String, String)> = pairs
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect();
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| OsString::from(value))
        }
    }

    #[test]
    fn a_mac_app_finds_the_installer_s_folder_without_a_terminal_path() {
        let found = standard_installs("macos", &environment(&[("HOME", "/Users/ann")]));
        assert_eq!(
            found,
            [
                "/opt/homebrew/bin/rc-astro",
                "/usr/local/bin/rc-astro",
                "/Applications/RC-Astro/CLI/rc-astro",
                "/Users/ann/Applications/RC-Astro/CLI/rc-astro",
            ]
            .map(PathBuf::from)
        );
        // Without a home folder, the per-user folder is left out.
        assert_eq!(standard_installs("macos", &environment(&[])).len(), 3);
        // A terminal's PATH puts Homebrew's folder before /usr/local/bin.
        assert!(found[0].starts_with("/opt/homebrew"));
    }

    #[test]
    fn windows_looks_in_program_files_once() {
        let found = standard_installs(
            "windows",
            &environment(&[
                ("ProgramFiles", r"C:\Program Files"),
                ("ProgramW6432", r"C:\Program Files"),
            ]),
        );
        assert_eq!(
            found,
            [PathBuf::from(r"C:\Program Files")
                .join("RC-Astro")
                .join("CLI")
                .join("rc-astro.exe")]
        );
        assert!(standard_installs("windows", &environment(&[])).is_empty());
    }

    #[test]
    fn linux_finds_the_install_script_s_folder() {
        let found = standard_installs("linux", &environment(&[("HOME", "/home/ann")]));
        assert_eq!(
            found,
            [
                "/home/ann/.local/bin/rc-astro",
                "/usr/local/bin/rc-astro",
                "/opt/rc-astro/rc-astro",
            ]
            .map(PathBuf::from)
        );
    }
}
