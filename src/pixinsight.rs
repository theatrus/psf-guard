//! Finding PixInsight on this machine and running WBPP under it.
//!
//! PixInsight is a desktop program with a command line: `-n` skips the
//! startup dialogs, `--automation-mode` lets a script run without stopping
//! at its own, `-r=` names the script and its parameters, and
//! `--force-exit` closes PixInsight when the script ends. It still needs a
//! display; on a Linux server without one, `xvfb-run` supplies a virtual
//! one. Nothing reaches the terminal: WBPP writes its own log below the
//! output folder, which is where a run's progress and its result are read.

use anyhow::{Context, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};

use crate::commands::export::wbpp::BPP_MAIN_BELOW_INSTALL;

/// A PixInsight install with WBPP in it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PixInsightInstall {
    /// The executable to launch.
    pub binary: PathBuf,
    /// The install root the executable sits in.
    pub root: PathBuf,
    /// WBPP's entry file below the root, which the runner script includes.
    pub bpp_main: PathBuf,
    /// The WBPP version that file's sources declare, when readable.
    pub wbpp_version: Option<String>,
}

/// Where the executable came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallSource {
    /// The settings named it.
    Configured,
    /// Found in a standard place.
    Detected,
}

/// What looking for PixInsight found.
#[derive(Debug, Clone, Serialize)]
pub struct Detection {
    pub install: Option<PixInsightInstall>,
    pub source: Option<InstallSource>,
    /// The places tried, in order, for the person to see.
    pub checked: Vec<PathBuf>,
    /// Why a configured path was not taken, if it was not.
    pub problem: Option<String>,
}

/// The standard places PixInsight installs to on this platform, plus the
/// per-user install its Linux tarball makes when unpacked at home.
pub fn candidate_binaries() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if cfg!(target_os = "macos") {
        candidates.push(PathBuf::from(
            "/Applications/PixInsight/PixInsight.app/Contents/MacOS/PixInsight",
        ));
    } else if cfg!(target_os = "windows") {
        candidates.push(PathBuf::from(
            r"C:\Program Files\PixInsight\bin\PixInsight.exe",
        ));
    } else {
        candidates.push(PathBuf::from("/opt/PixInsight/bin/PixInsight.sh"));
        candidates.push(PathBuf::from("/usr/local/PixInsight/bin/PixInsight.sh"));
        if let Some(home) = std::env::var_os("HOME") {
            candidates.push(PathBuf::from(home).join("PixInsight/bin/PixInsight.sh"));
        }
    }
    candidates
}

/// The install root an executable belongs to: the folder above `bin/`, or
/// above the macOS app bundle.
fn root_of(binary: &Path) -> Option<PathBuf> {
    let parent = binary.parent()?;
    let dir_name = parent.file_name()?.to_string_lossy().to_string();
    if dir_name == "MacOS" {
        // <root>/PixInsight.app/Contents/MacOS/PixInsight
        return parent.parent()?.parent()?.parent().map(Path::to_path_buf);
    }
    if dir_name == "bin" {
        return parent.parent().map(Path::to_path_buf);
    }
    None
}

/// The WBPP version its sources declare, from `BPP-Global.js` beside
/// `BPP-Main.js`.
fn wbpp_version(bpp_main: &Path) -> Option<String> {
    let global = bpp_main.with_file_name("BPP-Global.js");
    let text = std::fs::read_to_string(global).ok()?;
    let start = text.find("WBPP_VERSION:")? + "WBPP_VERSION:".len();
    let rest = &text[start..];
    let open = rest.find('"')? + 1;
    let close = rest[open..].find('"')? + open;
    Some(rest[open..close].to_string())
}

/// The install an executable belongs to, if WBPP is in it.
pub fn install_from_binary(binary: &Path) -> Result<PixInsightInstall> {
    if !binary.is_file() {
        anyhow::bail!("{} is not a file", binary.display());
    }
    let root = root_of(binary).ok_or_else(|| {
        anyhow::anyhow!(
            "{} does not sit in a PixInsight install (expected <install>/bin/ or the app bundle)",
            binary.display()
        )
    })?;
    let bpp_main = root.join(BPP_MAIN_BELOW_INSTALL);
    if !bpp_main.is_file() {
        anyhow::bail!(
            "{} has no WBPP: {} is missing",
            root.display(),
            bpp_main.display()
        );
    }
    Ok(PixInsightInstall {
        wbpp_version: wbpp_version(&bpp_main),
        binary: binary.to_path_buf(),
        root,
        bpp_main,
    })
}

/// Find PixInsight: the configured executable first, else the standard
/// places. A configured path that does not work is reported, not silently
/// replaced by a detected one.
pub fn detect(configured: Option<&str>) -> Detection {
    let mut checked = Vec::new();
    if let Some(configured) = configured.map(str::trim).filter(|value| !value.is_empty()) {
        let binary = PathBuf::from(configured);
        checked.push(binary.clone());
        return match install_from_binary(&binary) {
            Ok(install) => Detection {
                install: Some(install),
                source: Some(InstallSource::Configured),
                checked,
                problem: None,
            },
            Err(error) => Detection {
                install: None,
                source: None,
                checked,
                problem: Some(format!("{error:#}")),
            },
        };
    }
    for candidate in candidate_binaries() {
        checked.push(candidate.clone());
        if let Ok(install) = install_from_binary(&candidate) {
            return Detection {
                install: Some(install),
                source: Some(InstallSource::Detected),
                checked,
                problem: None,
            };
        }
    }
    Detection {
        install: None,
        source: None,
        checked,
        problem: None,
    }
}

/// How PixInsight gets the display it needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "path")]
pub enum DisplayPlan {
    /// This process has one (a desktop session, or macOS and Windows).
    Own,
    /// No display, but `xvfb-run` can make a virtual one.
    Xvfb(PathBuf),
    /// No display and no way to make one; a run would fail at launch.
    Missing,
}

/// Where a program is on `PATH`, if anywhere.
fn on_path(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}

pub fn display_plan() -> DisplayPlan {
    if !cfg!(target_os = "linux") {
        return DisplayPlan::Own;
    }
    let has = |name: &str| std::env::var_os(name).is_some_and(|value| !value.is_empty());
    if has("DISPLAY") || has("WAYLAND_DISPLAY") {
        return DisplayPlan::Own;
    }
    match on_path("xvfb-run") {
        Some(path) => DisplayPlan::Xvfb(path),
        None => DisplayPlan::Missing,
    }
}

/// A launch, ready to spawn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WbppCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
    /// The whole line as a person would type it, for the log.
    pub display_line: String,
}

/// The command that runs `runner` (a `run-wbpp.js`) headless, with WBPP
/// writing below `output_dir`. `extra` are further `name=value` WBPP
/// parameters, which the runner forwards.
pub fn wbpp_command(
    install: &PixInsightInstall,
    display: &DisplayPlan,
    runner: &Path,
    output_dir: &Path,
    extra: &[String],
) -> Result<WbppCommand> {
    for path in [runner, output_dir] {
        if path.to_string_lossy().contains(',') {
            anyhow::bail!(
                "{} contains a comma, which WBPP's command line uses to separate parameters",
                path.display()
            );
        }
    }
    let here = runner
        .parent()
        .ok_or_else(|| anyhow::anyhow!("{} has no parent folder", runner.display()))?;
    let mut params = vec![
        runner.to_string_lossy().to_string(),
        format!("here={}", here.display()),
        format!("outputDirectory={}", output_dir.display()),
        "run".to_string(),
    ];
    params.extend(extra.iter().cloned());
    let pi_args = vec![
        "-n".to_string(),
        "--automation-mode".to_string(),
        format!("-r={}", params.join(",")),
        "--force-exit".to_string(),
    ];
    let (program, args) = match display {
        DisplayPlan::Xvfb(xvfb_run) => {
            let mut args = vec![
                "-a".to_string(),
                "--server-args=-screen 0 1920x1200x24".to_string(),
                install.binary.to_string_lossy().to_string(),
            ];
            args.extend(pi_args);
            (xvfb_run.clone(), args)
        }
        DisplayPlan::Own => (install.binary.clone(), pi_args),
        DisplayPlan::Missing => anyhow::bail!(
            "PixInsight needs a display and this server has none; install xvfb (xvfb-run) \
             to give it a virtual one"
        ),
    };
    let display_line = std::iter::once(program.to_string_lossy().to_string())
        .chain(args.iter().map(|arg| {
            if arg.contains(' ') || arg.contains(',') {
                format!("\"{arg}\"")
            } else {
                arg.clone()
            }
        }))
        .collect::<Vec<_>>()
        .join(" ");
    Ok(WbppCommand {
        program,
        args,
        display_line,
    })
}

/// WBPP's newest log below an output folder, if it has written one yet.
pub fn newest_log(output_dir: &Path) -> Option<PathBuf> {
    let logs = std::fs::read_dir(output_dir.join("logs")).ok()?;
    logs.flatten()
        .filter(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "log")
        })
        .max_by_key(|entry| {
            entry
                .metadata()
                .and_then(|meta| meta.modified())
                .ok()
                .unwrap_or(std::time::UNIX_EPOCH)
        })
        .map(|entry| entry.path())
}

/// What a WBPP log says so far.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct LogSummary {
    /// The step WBPP last announced, as it worded it.
    pub stage: Option<String>,
    /// Steps announced so far.
    pub steps: usize,
    /// WBPP printed its closing time line.
    pub finished: bool,
    /// Lines WBPP marked as errors, most recent last.
    pub errors: Vec<String>,
    /// How long WBPP said the run took, once finished.
    pub elapsed: Option<String>,
}

/// The `[2026-09-24 20:47:48] ` PixInsight's log writer puts before each
/// line, so the markers behind it can be read.
fn strip_timestamp(line: &str) -> &str {
    let Some(rest) = line.strip_prefix('[') else {
        return line;
    };
    match rest.find(']') {
        Some(close) if close == 19 && rest.as_bytes().get(4) == Some(&b'-') => {
            rest[close + 1..].trim_start()
        }
        _ => line,
    }
}

/// PixInsight console markup a log line may carry.
fn strip_markup(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(open) = rest.find('<') {
        let after = &rest[open + 1..];
        match after.find('>') {
            // A short tag such as <raw>, </b> or <end>.
            Some(close) if close <= 12 && !after[..close].contains(' ') => {
                out.push_str(&rest[..open]);
                rest = &after[close + 1..];
            }
            _ => {
                out.push_str(&rest[..open + 1]);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Read a WBPP log: the last `tail` lines, cleaned, and what they say.
pub fn read_log(path: &Path, tail: usize) -> Result<(Vec<String>, LogSummary)> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(summarize_log(&text, tail))
}

pub fn summarize_log(text: &str, tail: usize) -> (Vec<String>, LogSummary) {
    let mut summary = LogSummary::default();
    let mut lines: Vec<String> = Vec::new();
    for raw in text.lines() {
        let line = strip_markup(raw.trim_end());
        let trimmed = strip_timestamp(line.trim());
        if trimmed.is_empty() {
            continue;
        }
        if let Some(step) = trimmed.strip_prefix("* ") {
            if let Some((title, elapsed)) = step
                .split_once(": ")
                .filter(|(title, _)| title.ends_with("Preprocessing"))
            {
                let _ = title;
                summary.finished = true;
                summary.elapsed = Some(elapsed.trim().to_string());
            } else if step.starts_with("Begin ") || step.starts_with("Writing master") {
                // WBPP prefixes many informational lines with "* "; only its
                // step openings say where the run is.
                summary.stage = Some(step.trim_end_matches([':', '.']).trim().to_string());
                summary.steps += 1;
            }
        }
        let lower = trimmed.to_ascii_lowercase();
        if lower.starts_with("*** error")
            || lower.starts_with("error:")
            || lower.contains("*** error:")
        {
            summary.errors.push(trimmed.to_string());
        }
        // The tail is for reading, so it drops the stamp too.
        lines.push(trimmed.to_string());
    }
    if summary.errors.len() > 20 {
        let keep = summary.errors.len() - 20;
        summary.errors.drain(..keep);
    }
    let start = lines.len().saturating_sub(tail);
    (lines.split_off(start), summary)
}

/// One file WBPP wrote.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OutputFile {
    /// Below the output folder, with forward slashes.
    pub path: String,
    pub size_bytes: u64,
    /// `master`, `calibrated`, `registered`, `log`, or `other`, from the
    /// folder WBPP put it in.
    pub kind: String,
}

/// Everything below the output folder, masters first.
pub fn list_outputs(output_dir: &Path) -> Vec<OutputFile> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<OutputFile>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(root, &path, out);
                continue;
            }
            let Ok(relative) = path.strip_prefix(root) else {
                continue;
            };
            let relative = relative.to_string_lossy().replace('\\', "/");
            let first = relative.split('/').next().unwrap_or("").to_string();
            let kind = match first.as_str() {
                "master" => "master",
                "calibrated" => "calibrated",
                "registered" => "registered",
                "logs" => "log",
                _ => "other",
            };
            out.push(OutputFile {
                path: relative,
                size_bytes: entry.metadata().map(|meta| meta.len()).unwrap_or(0),
                kind: kind.to_string(),
            });
        }
    }
    let mut files = Vec::new();
    walk(output_dir, output_dir, &mut files);
    let rank = |kind: &str| match kind {
        "master" => 0,
        "calibrated" => 2,
        "registered" => 3,
        "log" => 1,
        _ => 4,
    };
    files.sort_by(|a, b| {
        rank(&a.kind)
            .cmp(&rank(&b.kind))
            .then_with(|| a.path.cmp(&b.path))
    });
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fake_install(root: &Path, with_wbpp: bool) -> PathBuf {
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let binary = bin.join("PixInsight.sh");
        std::fs::write(&binary, "#!/bin/sh\n").unwrap();
        if with_wbpp {
            let scripts = root.join("src/scripts/BatchPreprocessing");
            std::fs::create_dir_all(&scripts).unwrap();
            std::fs::write(scripts.join("BPP-Main.js"), "// main\n").unwrap();
            std::fs::write(
                scripts.join("BPP-Global.js"),
                "BPP.Version = {\n   WBPP_ID: \"WeightedBatchPreprocessing\",\n   WBPP_VERSION:          \"3.1.0\",\n};\n",
            )
            .unwrap();
        }
        binary
    }

    #[test]
    fn an_install_is_recognised_by_its_wbpp_sources() {
        let dir = tempfile::tempdir().unwrap();
        let binary = fake_install(dir.path(), true);
        let install = install_from_binary(&binary).unwrap();
        assert_eq!(install.root, dir.path());
        assert_eq!(
            install.bpp_main,
            dir.path()
                .join("src/scripts/BatchPreprocessing/BPP-Main.js")
        );
        assert_eq!(install.wbpp_version.as_deref(), Some("3.1.0"));

        let bare = tempfile::tempdir().unwrap();
        let binary = fake_install(bare.path(), false);
        let error = install_from_binary(&binary).unwrap_err().to_string();
        assert!(error.contains("no WBPP"), "{error}");

        let stray = dir.path().join("elsewhere");
        std::fs::create_dir_all(&stray).unwrap();
        std::fs::write(stray.join("PixInsight.sh"), "").unwrap();
        let error = install_from_binary(&stray.join("PixInsight.sh"))
            .unwrap_err()
            .to_string();
        assert!(error.contains("does not sit in"), "{error}");
    }

    #[test]
    fn a_mac_bundle_roots_above_the_app() {
        let binary = Path::new("/Applications/PixInsight/PixInsight.app/Contents/MacOS/PixInsight");
        assert_eq!(
            root_of(binary),
            Some(PathBuf::from("/Applications/PixInsight"))
        );
        assert_eq!(
            root_of(Path::new("/opt/PixInsight/bin/PixInsight.sh")),
            Some(PathBuf::from("/opt/PixInsight"))
        );
    }

    #[test]
    fn a_configured_path_that_fails_is_reported_not_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let detection = detect(Some(dir.path().join("nope").to_str().unwrap()));
        assert!(detection.install.is_none());
        assert!(detection.problem.as_deref().unwrap().contains("not a file"));
        assert_eq!(detection.checked.len(), 1);

        let binary = fake_install(dir.path(), true);
        let detection = detect(Some(binary.to_str().unwrap()));
        assert_eq!(detection.source, Some(InstallSource::Configured));
        assert!(detection.install.is_some());
    }

    #[test]
    fn the_command_runs_the_script_headless_and_forwards_extras() {
        let install = PixInsightInstall {
            binary: PathBuf::from("/home/me/PixInsight/bin/PixInsight.sh"),
            root: PathBuf::from("/home/me/PixInsight"),
            bpp_main: PathBuf::from(
                "/home/me/PixInsight/src/scripts/BatchPreprocessing/BPP-Main.js",
            ),
            wbpp_version: Some("3.1.0".into()),
        };
        let command = wbpp_command(
            &install,
            &DisplayPlan::Own,
            Path::new("/var/cache/psf-guard/wbpp/run-1/run-wbpp.js"),
            Path::new("/var/cache/psf-guard/wbpp/run-1/wbpp-out"),
            &["autocrop=false".to_string()],
        )
        .unwrap();
        assert_eq!(command.program, install.binary);
        assert_eq!(
            command.args,
            vec![
                "-n",
                "--automation-mode",
                "-r=/var/cache/psf-guard/wbpp/run-1/run-wbpp.js,here=/var/cache/psf-guard/wbpp/run-1,outputDirectory=/var/cache/psf-guard/wbpp/run-1/wbpp-out,run,autocrop=false",
                "--force-exit",
            ]
        );

        let virtual_display = wbpp_command(
            &install,
            &DisplayPlan::Xvfb(PathBuf::from("/usr/bin/xvfb-run")),
            Path::new("/tmp/run/run-wbpp.js"),
            Path::new("/tmp/run/wbpp-out"),
            &[],
        )
        .unwrap();
        assert_eq!(virtual_display.program, PathBuf::from("/usr/bin/xvfb-run"));
        assert_eq!(virtual_display.args[0], "-a");
        assert_eq!(
            virtual_display.args[2],
            "/home/me/PixInsight/bin/PixInsight.sh"
        );
        assert!(virtual_display.args.contains(&"--force-exit".to_string()));
        assert!(virtual_display
            .display_line
            .starts_with("/usr/bin/xvfb-run -a"));

        let comma = wbpp_command(
            &install,
            &DisplayPlan::Own,
            Path::new("/tmp/M42, wide/run-wbpp.js"),
            Path::new("/tmp/out"),
            &[],
        );
        assert!(comma.unwrap_err().to_string().contains("comma"));
        let no_display = wbpp_command(
            &install,
            &DisplayPlan::Missing,
            Path::new("/tmp/run/run-wbpp.js"),
            Path::new("/tmp/out"),
            &[],
        );
        assert!(no_display.unwrap_err().to_string().contains("xvfb"));
    }

    #[test]
    fn a_log_yields_the_current_step_errors_and_the_close() {
        let text = "\
************************************************************
Weighted Batch Preprocessing Script 3.1.0
************************************************************
automation mode parameter: localNormalization = true
add file: <raw>/mnt/nas/a.fits</raw>

* Begin registration of light frames
Registering 12 frames
*** Error: Unable to read file /mnt/nas/b.fits
* End registration of light frames
* Begin local normalization of light frames
";
        let (lines, summary) = summarize_log(text, 3);
        assert_eq!(
            summary.stage.as_deref(),
            Some("Begin local normalization of light frames")
        );
        // PixInsight's log writer stamps every line; the stamp is not the step,
        // a line that is only a stamp is blank, and only WBPP's step openings
        // move the stage among the many lines it starts with "* ".
        let stamped = "[2026-09-24 20:47:48] * Begin registration of light frames\n\
                       [2026-09-24 20:47:48]\n\
                       [2026-09-24 20:47:49] *** Error: no stars\n\
                       [2026-09-24 20:47:50] * Estimating global scale factors\n\
                       [2026-09-24 20:47:51] * Writing master Dark frame:\n";
        let (stamped_lines, summary_stamped) = summarize_log(stamped, 5);
        assert_eq!(
            summary_stamped.stage.as_deref(),
            Some("Writing master Dark frame")
        );
        assert_eq!(summary_stamped.steps, 2);
        assert_eq!(summary_stamped.errors.len(), 1);
        assert_eq!(stamped_lines.len(), 4);
        assert_eq!(stamped_lines[0], "* Begin registration of light frames");
        assert_eq!(stamped_lines[3], "* Writing master Dark frame:");
        assert_eq!(summary.steps, 2);
        assert!(!summary.finished);
        assert_eq!(
            summary.errors,
            vec!["*** Error: Unable to read file /mnt/nas/b.fits"]
        );
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "*** Error: Unable to read file /mnt/nas/b.fits");
        // Markup goes.
        let (lines, _) = summarize_log("add file: <raw>/x/y.fits</raw>\n", 5);
        assert_eq!(lines, vec!["add file: /x/y.fits"]);

        let done = format!("{text}* End local normalization of light frames\n<end><cbr><br>* WeightedBatchPreprocessing: 01:23:45.678\n");
        let (_, summary) = summarize_log(&done, 3);
        assert!(summary.finished);
        assert_eq!(summary.elapsed.as_deref(), Some("01:23:45.678"));
    }

    #[test]
    fn outputs_list_masters_first_and_find_the_newest_log() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path();
        std::fs::create_dir_all(out.join("master")).unwrap();
        std::fs::create_dir_all(out.join("calibrated/light")).unwrap();
        std::fs::create_dir_all(out.join("logs")).unwrap();
        std::fs::write(out.join("calibrated/light/a_c.xisf"), b"12345").unwrap();
        std::fs::write(
            out.join("master/masterLight_BIN-1_FILTER-L.xisf"),
            b"1234567",
        )
        .unwrap();
        std::fs::write(out.join("logs/20260924.log"), b"log").unwrap();
        let files = list_outputs(out);
        assert_eq!(files[0].path, "master/masterLight_BIN-1_FILTER-L.xisf");
        assert_eq!(files[0].kind, "master");
        assert_eq!(files[0].size_bytes, 7);
        assert_eq!(files[1].kind, "log");
        assert_eq!(files[2].kind, "calibrated");
        assert_eq!(newest_log(out), Some(out.join("logs/20260924.log")));
        assert_eq!(newest_log(&out.join("nowhere")), None);
    }
}
