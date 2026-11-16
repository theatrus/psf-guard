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
    located.map(|cli| cli.with_host(host_identity()))
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

/// How long `rc-astro license --json` may take before it is killed. It
/// makes one online check.
pub const LICENSE_CHECK_LIMIT: std::time::Duration = std::time::Duration::from_secs(60);

/// The first contract version whose CLI has the `license` command.
const LICENSE_COMMAND_SCHEMA: u64 = 5;

/// One product's line of RC-Astro's license table.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LicenseProduct {
    /// The product's key, such as `nxt`.
    pub product: String,
    pub name: String,
    /// RC-Astro's status token. The vocabulary is open: show `message`.
    pub status: String,
    /// The text RC-Astro prints for this product, as given.
    pub message: String,
    /// The command that resolves this state, when one does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub days_left: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ml_version: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub used: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<i64>,
    /// The address this product is registered to, on a computer with
    /// licenses under more than one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
}

/// A step RC-Astro needs a person to take at a terminal.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LicenseAttention {
    pub reason: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
}

/// RC-Astro's account status report.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LicenseReport {
    /// The computer the report is about, as `user@computer`: the account
    /// rc-astro ran under.
    pub computer: String,
    /// The RC-Astro account this computer is registered to. `None` means
    /// it is registered to none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_email: Option<String>,
    /// Where a person gets an activation code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activation_url: Option<String>,
    pub products: Vec<LicenseProduct>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attention: Option<LicenseAttention>,
}

/// What one run of `rc-astro license --json` gave: the report, an error, or
/// both, since a refusal sends its report first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LicenseCheck {
    /// The rc-astro that ran.
    pub executable: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cli_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<LicenseReport>,
    /// What RC-Astro itself reported as an error, in its words: a refusal,
    /// or that it could not reach its server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Why PSF Guard got no answer from rc-astro, in PSF Guard's words:
    /// it could not be started, it ended without a report, it took too
    /// long, or it is too old to have the check.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
}

impl LicenseCheck {
    /// Whether the check gave a report and RC-Astro reported no error.
    pub fn succeeded(&self) -> bool {
        self.report.is_some() && self.error.is_none() && self.failure.is_none()
    }
}

/// Read the NDJSON `rc-astro license --json` printed. Unknown events, keys
/// and tokens are passed over, as RC-Astro's contract asks; a line that is
/// not JSON is skipped.
pub fn parse_license_stream(executable: &str, stdout: &str) -> LicenseCheck {
    let mut check = LicenseCheck {
        executable: executable.into(),
        ..Default::default()
    };
    let mut schema_version = None;
    let text = |value: &serde_json::Value, key: &str| {
        value
            .get(key)
            .and_then(|found| found.as_str())
            .map(String::from)
    };
    for line in stdout.lines() {
        let Ok(event) = serde_json::from_str::<serde_json::Value>(line.trim()) else {
            continue;
        };
        match (
            event.get("event").and_then(|found| found.as_str()),
            event.get("topic").and_then(|found| found.as_str()),
        ) {
            (Some("info"), Some("version")) => {
                check.cli_version = text(&event, "cliVersion");
                schema_version = event.get("schemaVersion").and_then(|found| found.as_u64());
            }
            (Some("info"), Some("licenseReport")) => {
                let number = |value: &serde_json::Value, key: &str| {
                    value.get(key).and_then(|found| found.as_i64())
                };
                check.report = Some(LicenseReport {
                    computer: text(&event, "computer").unwrap_or_default(),
                    account_email: event
                        .get("account")
                        .and_then(|account| text(account, "email")),
                    activation_url: text(&event, "activationUrl"),
                    products: event
                        .get("licenseStatus")
                        .and_then(|found| found.as_array())
                        .map(|products| {
                            products
                                .iter()
                                .map(|product| LicenseProduct {
                                    product: text(product, "product").unwrap_or_default(),
                                    name: text(product, "name").unwrap_or_default(),
                                    status: text(product, "status").unwrap_or_default(),
                                    message: text(product, "message").unwrap_or_default(),
                                    command: text(product, "command"),
                                    days_left: number(product, "daysLeft"),
                                    ml_version: number(product, "mlVersion"),
                                    used: number(product, "used"),
                                    limit: number(product, "limit"),
                                    email: text(product, "email"),
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                    attention: event.get("attention").map(|attention| LicenseAttention {
                        reason: text(attention, "reason").unwrap_or_default(),
                        message: text(attention, "message").unwrap_or_default(),
                        command: text(attention, "command"),
                    }),
                });
            }
            (Some("error"), _) => {
                check.error = text(&event, "message").or(Some("rc-astro reported an error".into()));
            }
            _ => {}
        }
    }
    if check.report.is_none()
        && check.error.is_none()
        && schema_version.is_some_and(|version| version < LICENSE_COMMAND_SCHEMA)
    {
        check.failure = Some(TOO_OLD.into());
    }
    check
}

/// The message for an rc-astro from before RC-Astro's account system.
const TOO_OLD: &str =
    "The installed rc-astro is too old for the account check. Update RC-Astro's tools.";

/// How PSF Guard identifies itself to rc-astro, on every run.
pub fn host_identity() -> String {
    format!("psf-guard-{}", env!("CARGO_PKG_VERSION"))
}

/// A process for `executable` that opens no console window on Windows, as a
/// worker in the tray or a service needs.
fn quiet_command(executable: &Path) -> std::process::Command {
    #[allow(unused_mut)]
    let mut command = std::process::Command::new(executable);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// Whether rc-astro's plain-text complaint is that it does not know the
/// `license` command, as a CLI from before the account system says.
fn rejects_license_command(stderr: &str) -> bool {
    let said = stderr.to_ascii_lowercase();
    said.contains("license")
        && [
            "not expected",
            "unknown",
            "unrecognized",
            "invalid",
            "no such",
        ]
        .iter()
        .any(|phrase| said.contains(phrase))
}

/// Run `rc-astro license --json` once and read its report. It makes an
/// online check, so run it only when a person asks. No retries: an offline
/// computer answers with RC-Astro's own error. A run past `limit`, or one
/// `stop` ends, is killed.
///
/// `seiza_stacking::RcAstroCli` has no way to run a command of its own
/// choosing, so this builds the process itself, with the host identity
/// [`locate`] gives the CLI and the no-window flag Seiza uses. The
/// follow-up is a `license` method on `RcAstroCli`, which would own the
/// process, the version event and the schema rule.
pub fn check_license(
    cli: &RcAstroCli,
    limit: std::time::Duration,
    stop: &dyn Fn() -> bool,
) -> LicenseCheck {
    let executable = cli.executable();
    let shown = executable.display().to_string();
    let failed = |failure: String| LicenseCheck {
        executable: shown.clone(),
        failure: Some(failure),
        ..Default::default()
    };
    // The contract version, from a tool's local document: no network. It
    // says whether this CLI has the command at all, and whether it takes
    // `--host`.
    let schema_version = TOOLS
        .iter()
        .find_map(|tool| cli.tool_schema(tool).ok())
        .map(|schema| u64::from(schema.schema_version));
    if schema_version.is_some_and(|version| version < LICENSE_COMMAND_SCHEMA) {
        return failed(TOO_OLD.into());
    }
    let mut command = quiet_command(executable);
    command.args(["license", "--json"]);
    if schema_version.is_some_and(|version| version >= 6) {
        command.arg("--host").arg(host_identity());
    }
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return failed(format!("PSF Guard could not start {shown}: {error}.")),
    };
    // The report is a few lines, well inside a pipe's buffer, so it is read
    // once the process has ended.
    let began = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if stop() => {
                let _ = child.kill();
                let _ = child.wait();
                return failed("The license check was stopped.".into());
            }
            Ok(None) if began.elapsed() >= limit => {
                let _ = child.kill();
                let _ = child.wait();
                return failed(format!(
                    "rc-astro did not answer the license check within {} seconds, so PSF Guard \
                     stopped it.",
                    limit.as_secs()
                ));
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(50)),
            Err(error) => return failed(format!("PSF Guard lost track of rc-astro: {error}.")),
        }
    };
    let read = |pipe: Option<&mut dyn std::io::Read>| {
        let mut text = String::new();
        if let Some(pipe) = pipe {
            let _ = pipe.read_to_string(&mut text);
        }
        text
    };
    let stdout = read(
        child
            .stdout
            .as_mut()
            .map(|pipe| pipe as &mut dyn std::io::Read),
    );
    let stderr = read(
        child
            .stderr
            .as_mut()
            .map(|pipe| pipe as &mut dyn std::io::Read),
    );
    let mut check = parse_license_stream(&shown, &stdout);
    if check.report.is_none() && check.error.is_none() && check.failure.is_none() {
        // rc-astro printed neither a report nor an error event. Only a CLI
        // that turns the command down is too old; anything else is its own
        // failure, such as a library it could not load, and is reported as
        // that.
        let said: Vec<&str> = stderr
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .take(3)
            .collect();
        check.failure = Some(if !status.success() && rejects_license_command(&stderr) {
            TOO_OLD.to_string()
        } else {
            format!(
                "rc-astro ended ({status}) without a license report{}",
                if said.is_empty() {
                    " and printed nothing.".to_string()
                } else {
                    format!(". It printed: {}", said.join(" | "))
                }
            )
        });
    }
    check
}

/// RC-Astro's tools as providers of processing steps.
pub struct RcAstroEngine;

/// The message when no worker has rc-astro.
pub const NOT_INSTALLED: &str = "rc-astro is not installed on this server or on a connected worker";

impl super::steps::StepRunner for RcAstroEngine {
    fn offers(&self, tools: &super::LocalTools) -> (Vec<ExternalToolSchema>, Option<String>) {
        match &tools.rc_astro {
            Some(cli) => probe(cli),
            None => (Vec::new(), None),
        }
    }

    fn payload(
        &self,
        schema: &ExternalToolSchema,
        request: &ExternalToolRequest,
        headers: &[(String, seiza_fits::HeaderValue)],
        _person: bool,
        _threads: usize,
    ) -> Result<serde_json::Value, String> {
        serde_json::to_value(StepSpec {
            schema: schema.clone(),
            request: request.clone(),
            headers: HeaderCard::from_fits(headers),
        })
        .map_err(|error| error.to_string())
    }

    fn outputs(&self) -> Vec<(String, bool)> {
        vec![(PROCESSED.into(), true), (STARS.into(), false)]
    }

    fn reads_holes(&self) -> bool {
        false
    }

    fn run(
        &self,
        payload: serde_json::Value,
        context: super::steps::RunContext<'_>,
    ) -> Result<serde_json::Value, String> {
        let spec: StepSpec = serde_json::from_value(payload)
            .map_err(|error| format!("The RC-Astro task could not be read: {error}"))?;
        let cli = context
            .tools
            .rc_astro
            .as_ref()
            .ok_or_else(|| "rc-astro is not installed on this worker".to_string())?;
        let input = context
            .inputs
            .get(INPUT)
            .ok_or_else(|| "The RC-Astro task has no input image".to_string())?;
        let cancel = {
            let stop = context.stop.clone();
            seiza_stacking::CancelSignal::new(move || stop.requested())
        };
        let report = context.report;
        let result = run(
            cli,
            &spec,
            input,
            context.output_dir,
            &cancel,
            &mut |done| report.fraction(done),
        )?;
        serde_json::to_value(result).map_err(|error| error.to_string())
    }

    fn missing(&self) -> (&'static str, super::steps::Remedy) {
        (NOT_INSTALLED, super::steps::Remedy::InstallRcAstro)
    }
}

#[cfg(test)]
mod license_tests {
    use super::*;

    const VERSION: &str =
        r#"{"event":"info","topic":"version","cliVersion":"2.6.10","schemaVersion":6}"#;

    fn stream(lines: &[&str]) -> String {
        lines.join("\n")
    }

    #[test]
    fn a_registered_computer_s_report_lists_each_product_as_rc_astro_says_it() {
        let check = parse_license_stream(
            "/opt/rc-astro/rc-astro",
            &stream(&[
                VERSION,
                r#"{"event":"info","topic":"licenseReport","operation":"check","computer":"ann@studio","account":{"email":"user@example.com"},"activationUrl":"https://www.rc-astro.com/account","licenseStatus":[{"product":"bxt","name":"BlurXTerminator","status":"permanent","message":"Permanent license through ML5","mlVersion":5},{"product":"nxt","name":"NoiseXTerminator","status":"available","message":"Available - activate: rc-astro license --activate nxt","command":"rc-astro license --activate nxt"},{"product":"sxt","name":"StarXTerminator","status":"trial","message":"Trial - 12 days left","daysLeft":12},{"product":"gxt","name":"GradientXTerminator","status":"limit","message":"All 3 activations in use","used":3,"limit":3,"email":"other@example.com"}]}"#,
            ]),
        );
        assert_eq!(check.executable, "/opt/rc-astro/rc-astro");
        assert_eq!(check.cli_version.as_deref(), Some("2.6.10"));
        assert_eq!(check.error, None);
        let report = check.report.clone().unwrap();
        assert_eq!(report.computer, "ann@studio");
        assert_eq!(report.account_email.as_deref(), Some("user@example.com"));
        assert_eq!(
            report.activation_url.as_deref(),
            Some("https://www.rc-astro.com/account")
        );
        assert_eq!(report.attention, None);
        assert_eq!(report.products.len(), 4);
        assert_eq!(report.products[0].ml_version, Some(5));
        assert_eq!(
            report.products[1].command.as_deref(),
            Some("rc-astro license --activate nxt")
        );
        assert_eq!(report.products[2].days_left, Some(12));
        assert_eq!(
            (report.products[3].used, report.products[3].limit),
            (Some(3), Some(3))
        );
    }

    #[test]
    fn a_computer_with_no_account_says_so_and_what_to_run() {
        let check = parse_license_stream(
            "rc-astro",
            &stream(&[
                VERSION,
                r#"{"event":"info","topic":"licenseReport","operation":"check","computer":"svc@worker","activationUrl":"https://www.rc-astro.com/account","licenseStatus":[{"product":"bxt","name":"BlurXTerminator","status":"required","message":"Not activated"}],"attention":{"reason":"unregistered","message":"This computer is not registered to an RC-Astro account","command":"rc-astro license <code>"}}"#,
            ]),
        );
        let report = check.report.unwrap();
        // No account object is the "unregistered" signal.
        assert_eq!(report.account_email, None);
        let attention = report.attention.unwrap();
        assert_eq!(attention.reason, "unregistered");
        assert_eq!(
            attention.command.as_deref(),
            Some("rc-astro license <code>")
        );
    }

    #[test]
    fn an_offline_check_is_an_error_with_no_report() {
        let check = parse_license_stream(
            "rc-astro",
            &stream(&[
                VERSION,
                r#"{"event":"error","message":"Could not reach the RC-Astro license server"}"#,
            ]),
        );
        assert_eq!(check.report, None);
        assert_eq!(
            check.error.as_deref(),
            Some("Could not reach the RC-Astro license server")
        );
    }

    #[test]
    fn a_refusal_gives_its_report_and_then_its_error() {
        let check = parse_license_stream(
            "rc-astro",
            &stream(&[
                VERSION,
                r#"{"event":"info","topic":"licenseReport","operation":"check","computer":"ann@studio","activationUrl":"https://www.rc-astro.com/account","licenseStatus":[],"attention":{"reason":"mixedAccounts","message":"This computer has RC-Astro licenses from more than one account","command":"rc-astro license"}}"#,
                r#"{"event":"error","message":"Licenses from more than one account"}"#,
            ]),
        );
        assert_eq!(
            check.report.unwrap().attention.unwrap().reason,
            "mixedAccounts"
        );
        assert_eq!(
            check.error.as_deref(),
            Some("Licenses from more than one account")
        );
    }

    #[test]
    fn unknown_events_keys_and_tokens_are_passed_over() {
        let check = parse_license_stream(
            "rc-astro",
            &stream(&[
                "not json at all",
                VERSION,
                r#"{"event":"info","topic":"modelDownload","file":"x","status":"downloading"}"#,
                r#"{"event":"sparkle","topic":"licenseReport"}"#,
                r#"{"event":"info","topic":"licenseReport","operation":"audit","computer":"ann@studio","activationUrl":"https://www.rc-astro.com/account","futureKey":{"a":1},"licenseStatus":[{"product":"zxt","name":"ZoomXTerminator","status":"leased","message":"Leased until Friday","futureField":true,"action":"teleported"}],"attention":{"reason":"somethingNew","message":"Ask at the desk"}}"#,
            ]),
        );
        let report = check.report.unwrap();
        assert_eq!(report.products[0].status, "leased");
        assert_eq!(report.products[0].message, "Leased until Friday");
        let attention = report.attention.unwrap();
        assert_eq!(attention.message, "Ask at the desk");
        assert_eq!(attention.command, None);
        assert_eq!(check.error, None);
    }

    #[test]
    fn an_rc_astro_from_before_the_account_system_is_called_too_old() {
        let check = parse_license_stream(
            "rc-astro",
            r#"{"event":"info","topic":"version","cliVersion":"2.5.0","schemaVersion":4}"#,
        );
        assert_eq!(check.report, None);
        assert_eq!(check.error, None);
        assert!(check.failure.unwrap().contains("too old"));
    }

    /// A stand-in rc-astro. `schema` is the contract version its tools'
    /// documents give, or `None` for one whose documents cannot be read;
    /// `license` is what it does when asked for the license report.
    #[cfg(unix)]
    fn stand_in(folder: &Path, schema: Option<u32>, license: &str) -> RcAstroCli {
        use std::os::unix::fs::PermissionsExt;
        let path = folder.join("rc-astro");
        let document = match schema {
            Some(version) => format!(
                r#"echo '{{"schemaVersion":{version},"cliVersion":"9.9.9-test","key":"'"$1"'","name":"RC-Astro '"$1"'","mlVersion":4,"parameters":[]}}'; exit 0"#
            ),
            None => "exit 1".to_string(),
        };
        std::fs::write(
            &path,
            format!(
                "#!/bin/sh\nif [ \"$1\" != license ]; then {document}; fi\necho \"$@\" >> \"{}/calls\"\n{license}\n",
                folder.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        // Another test's thread may fork while this file is open for
        // writing, and its child then holds the file open for a moment.
        // Until that child is gone the script cannot be run ("Text file
        // busy"), so wait here, not in the test that runs it. A first
        // argument other than `license` records no call.
        for _ in 0..400 {
            match std::process::Command::new(&path).arg("probe").output() {
                Err(error) if error.raw_os_error() == Some(26) => {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                _ => break,
            }
        }
        RcAstroCli::with_executable(path)
    }

    #[cfg(unix)]
    fn calls(folder: &Path) -> String {
        std::fs::read_to_string(folder.join("calls")).unwrap_or_default()
    }

    // Only the stand-in script's tests, which run on Unix, stop a check.
    #[cfg(unix)]
    const NEVER: &dyn Fn() -> bool = &|| false;

    #[cfg(unix)]
    #[test]
    fn the_check_runs_rc_astro_once_as_psf_guard_and_reads_what_it_printed() {
        let folder = tempfile::tempdir().unwrap();
        // A refusal: a report, then an error, then exit 77.
        let cli = stand_in(
            folder.path(),
            Some(6),
            r#"echo '{"event":"info","topic":"version","cliVersion":"9.9.9-test","schemaVersion":6}'
echo '{"event":"info","topic":"licenseReport","operation":"check","computer":"ann@studio","account":{"email":"user@example.com"},"activationUrl":"https://www.rc-astro.com/account","licenseStatus":[{"product":"nxt","name":"NoiseXTerminator","status":"permanent","message":"Permanent license"}]}'
echo '{"event":"error","message":"One product refused"}'
exit 77"#,
        );
        let check = check_license(&cli, LICENSE_CHECK_LIMIT, NEVER);
        assert_eq!(check.executable, cli.executable().display().to_string());
        assert_eq!(check.cli_version.as_deref(), Some("9.9.9-test"));
        assert_eq!(check.report.as_ref().unwrap().products[0].product, "nxt");
        // RC-Astro's words are RC-Astro's; PSF Guard adds none of its own.
        assert_eq!(check.error.as_deref(), Some("One product refused"));
        assert_eq!(check.failure, None);
        assert!(!check.succeeded());
        // One run, no retry after the refusal, with the host identity every
        // other rc-astro run carries.
        assert_eq!(
            calls(folder.path()),
            format!("license --json --host {}\n", host_identity())
        );
    }

    #[cfg(unix)]
    #[test]
    fn only_an_rc_astro_without_the_command_is_called_too_old() {
        // A contract before the account system: no online check is made.
        let folder = tempfile::tempdir().unwrap();
        let cli = stand_in(folder.path(), Some(4), "exit 0");
        let said = check_license(&cli, LICENSE_CHECK_LIMIT, NEVER)
            .failure
            .unwrap();
        assert!(said.contains("too old"), "{said}");
        assert_eq!(calls(folder.path()), "");

        // Its documents unreadable, and it turns the command down itself.
        let folder = tempfile::tempdir().unwrap();
        let cli = stand_in(
            folder.path(),
            None,
            "echo 'The following argument was not expected: license' >&2; exit 109",
        );
        let said = check_license(&cli, LICENSE_CHECK_LIMIT, NEVER)
            .failure
            .unwrap();
        assert!(said.contains("too old"), "{said}");
        // No --host for a CLI whose contract version is unknown.
        assert_eq!(calls(folder.path()), "license --json\n");

        // A current rc-astro that dies before it prints a report, such as
        // one that cannot load a library under a service account, is not
        // too old: its exit status and its own words are reported.
        let folder = tempfile::tempdir().unwrap();
        let cli = stand_in(
            folder.path(),
            Some(6),
            "echo 'error while loading shared libraries: libtorch.so' >&2; exit 127",
        );
        let check = check_license(&cli, LICENSE_CHECK_LIMIT, NEVER);
        let said = check.failure.unwrap();
        assert!(!said.contains("too old"), "{said}");
        assert!(said.contains("127"), "{said}");
        assert!(said.contains("libtorch.so"), "{said}");
        assert_eq!(check.error, None);

        // One that printed its version and then crashed, likewise.
        let folder = tempfile::tempdir().unwrap();
        let cli = stand_in(
            folder.path(),
            None,
            r#"echo '{"event":"info","topic":"version","cliVersion":"2.6.10","schemaVersion":6}'; exit 3"#,
        );
        let check = check_license(&cli, LICENSE_CHECK_LIMIT, NEVER);
        let said = check.failure.unwrap();
        assert!(!said.contains("too old"), "{said}");
        assert!(said.contains("printed nothing"), "{said}");
        assert_eq!(check.cli_version.as_deref(), Some("2.6.10"));
    }

    #[cfg(unix)]
    #[test]
    fn a_slow_a_stopped_and_a_missing_rc_astro_each_fail_in_psf_guard_s_words() {
        let folder = tempfile::tempdir().unwrap();
        let slow = stand_in(folder.path(), None, "sleep 30");
        let began = std::time::Instant::now();
        let check = check_license(&slow, std::time::Duration::from_millis(300), NEVER);
        assert!(check.failure.unwrap().contains("did not answer"));
        assert_eq!(check.error, None);
        // Stop ends a check under way.
        let stopped = check_license(&slow, LICENSE_CHECK_LIMIT, &|| true);
        assert_eq!(
            stopped.failure.as_deref(),
            Some("The license check was stopped.")
        );
        assert!(began.elapsed() < std::time::Duration::from_secs(10));

        let missing = RcAstroCli::with_executable(folder.path().join("nowhere"));
        let said = check_license(&missing, LICENSE_CHECK_LIMIT, NEVER)
            .failure
            .unwrap();
        assert!(said.starts_with("PSF Guard could not start"), "{said}");
    }
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
