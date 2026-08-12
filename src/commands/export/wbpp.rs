//! The runner script that hands a WBPP-layout export to PixInsight.
//!
//! WeightedBatchPreprocessing 3.x is driven from PixInsight's command line
//! rather than through a script API: `automationMode=true` suppresses the
//! dialog, `dir=` adds a directory, and `outputDirectory=` says where the
//! results go. So the useful thing to generate is not PJSR but the invocation
//! itself, which a user can read, edit, and re-run.
//!
//! Verified against WBPP 3.0.1, the version in PixInsight 1.9.4.

use super::ExportPlan;

/// WBPP release these scripts were written against. Its automation parameters
/// are stable within a major version but have changed across them, so the
/// generated script says what it expects.
pub const TARGET_WBPP_VERSION: &str = "3.0.1";

/// The four roots the WBPP layout writes, in the order the script scans them.
///
/// Named explicitly rather than scanning the export root, because `dir=` is
/// recursive: one root would sweep up `wbpp-out/` on a second run and feed
/// WBPP its own output.
const FRAME_ROOTS: [&str; 4] = ["lights", "flats", "darks", "bias"];

/// Where WBPP writes. A sibling of the frame roots, never inside one.
const OUTPUT_DIRECTORY: &str = "wbpp-out";

/// Whether the generated script runs the pipeline or only loads it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WbppRun {
    /// Load the frames and groups, then stop with the dialog open. WBPP's
    /// grouping and reference choices are worth a look before an hour of
    /// integration starts, so this is the default.
    #[default]
    LoadOnly,
    /// Run the whole pipeline headless and exit.
    Full,
}

/// Which roots a plan actually filled. A script that scans an empty directory
/// makes WBPP complain, and a missing one is worth seeing in the script rather
/// than in a stack trace.
fn populated_roots(plan: &ExportPlan) -> Vec<&'static str> {
    FRAME_ROOTS
        .iter()
        .copied()
        .filter(|root| {
            plan.items.iter().any(|item| {
                item.relative_dest
                    .components()
                    .next()
                    .is_some_and(|first| first.as_os_str() == *root)
            })
        })
        .collect()
}

/// The POSIX runner, for Linux and macOS.
pub fn shell_script(plan: &ExportPlan, run: WbppRun) -> String {
    let roots = populated_roots(plan);
    let mut script = String::new();
    script.push_str("#!/bin/sh\n");
    script.push_str(&preamble("# "));
    script.push_str(
        "\n\
         set -e\n\
         HERE=$(cd \"$(dirname \"$0\")\" && pwd)\n\
         \n\
         # Point these at your install if they are somewhere else.\n\
         if [ \"$(uname)\" = \"Darwin\" ]; then\n\
         \x20 PI_ROOT=\"${PI_ROOT:-/Applications/PixInsight}\"\n\
         \x20 PI_BIN=\"${PI_BIN:-$PI_ROOT/PixInsight.app/Contents/MacOS/PixInsight}\"\n\
         else\n\
         \x20 PI_ROOT=\"${PI_ROOT:-/opt/PixInsight}\"\n\
         \x20 PI_BIN=\"${PI_BIN:-$PI_ROOT/bin/PixInsight.sh}\"\n\
         fi\n\
         WBPP=\"${WBPP:-$PI_ROOT/src/scripts/BatchPreprocessing/WBPP.js}\"\n\
         \n",
    );

    script.push_str("PARAMS=\"$WBPP,automationMode=true\"\n");
    for root in &roots {
        script.push_str(&format!("PARAMS=\"$PARAMS,dir=$HERE/{root}\"\n"));
    }
    script.push_str(&format!(
        "PARAMS=\"$PARAMS,outputDirectory=$HERE/{OUTPUT_DIRECTORY}\"\n"
    ));
    match run {
        WbppRun::LoadOnly => script.push_str(
            "\n\
             # Loads the frames and groups, then stops with the dialog open so\n\
             # you can check them. Delete this line to run the pipeline.\n\
             PARAMS=\"$PARAMS,loadOnly\"\n",
        ),
        WbppRun::Full => script.push_str(
            "\n\
             # Runs the whole pipeline headless. Add\n\
             #   PARAMS=\"$PARAMS,loadOnly\"\n\
             # to stop at the dialog and check the groups first.\n",
        ),
    }
    script.push_str(&format!(
        "\nmkdir -p \"$HERE/{OUTPUT_DIRECTORY}\"\n\
         exec \"$PI_BIN\" -n --automation-mode -r=\"$PARAMS\" --force-exit\n"
    ));
    script
}

/// The Windows runner, so an export can be zipped and taken to another machine.
pub fn batch_script(plan: &ExportPlan, run: WbppRun) -> String {
    let roots = populated_roots(plan);
    let mut script = String::new();
    script.push_str("@echo off\r\n");
    script.push_str(&preamble("REM ").replace('\n', "\r\n"));
    script.push_str(
        "\r\n\
         setlocal\r\n\
         set \"HERE=%~dp0\"\r\n\
         set \"HERE=%HERE:~0,-1%\"\r\n\
         \r\n\
         REM Point these at your install if it is somewhere else.\r\n\
         if not defined PI_ROOT set \"PI_ROOT=C:\\Program Files\\PixInsight\"\r\n\
         if not defined PI_BIN set \"PI_BIN=%PI_ROOT%\\bin\\PixInsight.exe\"\r\n\
         if not defined WBPP set \"WBPP=%PI_ROOT%\\src\\scripts\\BatchPreprocessing\\WBPP.js\"\r\n\
         \r\n",
    );
    script.push_str("set \"PARAMS=%WBPP%,automationMode=true\"\r\n");
    for root in &roots {
        script.push_str(&format!("set \"PARAMS=%PARAMS%,dir=%HERE%\\{root}\"\r\n"));
    }
    script.push_str(&format!(
        "set \"PARAMS=%PARAMS%,outputDirectory=%HERE%\\{OUTPUT_DIRECTORY}\"\r\n"
    ));
    if run == WbppRun::LoadOnly {
        script.push_str(
            "\r\n\
             REM Loads the frames and groups, then stops with the dialog open so\r\n\
             REM you can check them. Delete this line to run the pipeline.\r\n\
             set \"PARAMS=%PARAMS%,loadOnly\"\r\n",
        );
    }
    script.push_str(&format!(
        "\r\nif not exist \"%HERE%\\{OUTPUT_DIRECTORY}\" mkdir \"%HERE%\\{OUTPUT_DIRECTORY}\"\r\n\
         \"%PI_BIN%\" -n --automation-mode -r=\"%PARAMS%\" --force-exit\r\n"
    ));
    script
}

fn preamble(comment: &str) -> String {
    format!(
        "{comment}Hand this export to PixInsight's WeightedBatchPreprocessing.\n\
         {comment}Generated by PSF Guard for WBPP {TARGET_WBPP_VERSION}.\n\
         {comment}\n\
         {comment}WBPP reads each frame's type from its IMAGETYP header, so the\n\
         {comment}folders below are for your benefit rather than its. It groups\n\
         {comment}lights by filter, exposure, binning and gain the same way.\n\
         {comment}\n\
         {comment}Each directory is scanned recursively, which is why the frame\n\
         {comment}roots are listed one by one: scanning the export root would\n\
         {comment}sweep up {OUTPUT_DIRECTORY}/ on a second run and feed WBPP its\n\
         {comment}own output.\n\
         {comment}\n\
         {comment}PixInsight prints nothing to the terminal in this mode: WBPP\n\
         {comment}writes to its own console, so a finished run and a failed one\n\
         {comment}look alike from outside. Read\n\
         {comment}  {OUTPUT_DIRECTORY}/logs/*.log\n\
         {comment}for what actually happened, and look in {OUTPUT_DIRECTORY}/master\n\
         {comment}and {OUTPUT_DIRECTORY}/calibrated for the results.\n"
    )
}

/// Whether a destination can be expressed in a WBPP command line at all.
///
/// Parameters are comma-separated inside one `-r=` argument, so a comma in the
/// export's own path would split it. The frame roots below it are fixed names,
/// so only the root the user chose can carry one.
pub fn unusable_destination(dest_root: &std::path::Path) -> Option<String> {
    dest_root.to_string_lossy().contains(',').then(|| {
        format!(
            "the export path {} contains a comma, which WBPP's command line uses \
             to separate parameters; the frames were written but the runner script \
             was not, because it could not be expressed",
            dest_root.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::export::{ExportItem, FrameKind};
    use std::path::PathBuf;

    fn plan_with(roots: &[&str]) -> ExportPlan {
        let mut plan = ExportPlan::default();
        for root in roots {
            plan.items.push(ExportItem {
                image_id: 0,
                calibration_frame_id: None,
                kind: FrameKind::Light,
                source: PathBuf::from("/tmp/x.fits"),
                relative_dest: PathBuf::from(root).join("a").join("x.fits"),
                size_bytes: 0,
            });
        }
        plan
    }

    /// The invocation this produces was run against PixInsight 1.9.4 with WBPP
    /// 3.0.1: it built master bias, dark and flat, matched the dark and flat to
    /// the lights, and calibrated them. These assertions pin the parts that
    /// made that work.
    #[test]
    fn the_shell_runner_matches_the_invocation_that_was_verified() {
        let script = shell_script(
            &plan_with(&["lights", "flats", "darks", "bias"]),
            WbppRun::Full,
        );

        assert!(script.starts_with("#!/bin/sh\n"));
        assert!(script.contains("automationMode=true"), "{script}");
        assert!(script.contains("--automation-mode"));
        assert!(script.contains("--force-exit"));
        assert!(script.contains("WBPP.js"));
        // Every frame root listed separately, never the export root.
        for root in ["lights", "flats", "darks", "bias"] {
            assert!(
                script.contains(&format!("dir=$HERE/{root}")),
                "missing {root}"
            );
        }
        assert!(script.contains(&format!("outputDirectory=$HERE/{OUTPUT_DIRECTORY}")));
        // A full run must set no loadOnly parameter. The comment showing how
        // to add one does mention it, so look only at what executes.
        let active_load_only = script
            .lines()
            .any(|line| !line.trim_start().starts_with('#') && line.contains("loadOnly"));
        assert!(
            !active_load_only,
            "a full run must not stop at the dialog:\n{script}"
        );
    }

    /// The default stops before an hour of integration starts.
    #[test]
    fn the_default_run_only_loads() {
        assert_eq!(WbppRun::default(), WbppRun::LoadOnly);
        let script = shell_script(&plan_with(&["lights"]), WbppRun::LoadOnly);
        let active_load_only = script
            .lines()
            .any(|line| !line.trim_start().starts_with('#') && line.contains("loadOnly"));
        assert!(active_load_only, "{script}");
    }

    /// A root with no frames must not be scanned: WBPP complains about an
    /// empty directory, and an export filtered to one target often has no
    /// bias or darks of its own.
    #[test]
    fn only_the_roots_a_plan_filled_are_scanned() {
        let script = shell_script(&plan_with(&["lights", "flats"]), WbppRun::LoadOnly);
        assert!(script.contains("dir=$HERE/lights"));
        assert!(script.contains("dir=$HERE/flats"));
        assert!(!script.contains("dir=$HERE/darks"), "{script}");
        assert!(!script.contains("dir=$HERE/bias"), "{script}");
    }

    /// PixInsight prints nothing to the terminal in this mode, so a run that
    /// failed looks exactly like one that worked. The script has to say where
    /// to look.
    #[test]
    fn the_runner_says_where_the_real_output_is() {
        let script = shell_script(&plan_with(&["lights"]), WbppRun::LoadOnly);
        assert!(
            script.contains(&format!("{OUTPUT_DIRECTORY}/logs/*.log")),
            "{script}"
        );
    }

    #[test]
    fn the_windows_runner_uses_crlf_and_percent_expansion() {
        let script = batch_script(&plan_with(&["lights", "bias"]), WbppRun::LoadOnly);
        assert!(script.starts_with("@echo off\r\n"));
        assert!(script.contains("dir=%HERE%\\lights"), "{script}");
        assert!(script.contains("dir=%HERE%\\bias"), "{script}");
        assert!(!script.contains("dir=%HERE%\\darks"));
        // Every line ends CRLF, or cmd.exe mangles it.
        assert!(
            !script.contains("\n")
                || script.matches('\n').count() == script.matches("\r\n").count()
        );
    }

    /// WBPP takes every parameter in one comma-separated argument, so a comma
    /// in the export path would split it into nonsense. The frame roots below
    /// it are fixed names, so only the chosen root can carry one.
    #[test]
    fn a_comma_in_the_export_path_is_refused_rather_than_mangled() {
        assert!(unusable_destination(std::path::Path::new("/data/M42, Trapezium")).is_some());
        assert!(unusable_destination(std::path::Path::new("/data/M42_Trapezium")).is_none());
    }
}
