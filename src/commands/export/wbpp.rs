//! The runner script that hands a WBPP-layout export to PixInsight.
//!
//! WeightedBatchPreprocessing 3.x is driven from PixInsight's command line
//! rather than through a script API: `automationMode=true` suppresses the
//! dialog, `dir=` adds a directory, and `outputDirectory=` says where the
//! results go. So the useful thing to generate is not PJSR but the invocation
//! itself, which a user can read, edit, and re-run.
//!
//! Verified against WBPP 3.0.1, the version in PixInsight 1.9.4.

use super::{ExportPlan, SESSION_KEYWORD};
use std::path::{Component, Path, PathBuf};

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

/// Where the runner finds the frames.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum WbppFiles {
    /// Under the frame roots beside the script, which it scans with `dir=`.
    #[default]
    Placed,
    /// Where they already are: the script names every frame with `file=`,
    /// below one source root it takes from an environment variable. The
    /// root defaults to the frames' common parent as this machine sees it,
    /// or to `remote_root` when the machine running PixInsight mounts that
    /// folder somewhere else (a Windows share for a Linux server's mount).
    Referenced { remote_root: Option<String> },
}

/// Whether any planned path carries a session component, which is what
/// tells the runner to turn WBPP's keyword grouping on.
fn has_sessions(plan: &ExportPlan) -> bool {
    let prefix = format!("{SESSION_KEYWORD}_");
    plan.items.iter().any(|item| {
        item.relative_dest
            .components()
            .any(|component| component.as_os_str().to_string_lossy().starts_with(&prefix))
    })
}

/// The longest directory every planned source sits under. `None` for an
/// empty plan or sources with no common parent (two Windows drives).
pub fn common_source_root(plan: &ExportPlan) -> Option<PathBuf> {
    let mut root: Option<Vec<Component>> = None;
    for item in &plan.items {
        let parent = item.source.parent()?;
        let components: Vec<Component> = parent.components().collect();
        root = Some(match root {
            None => components,
            Some(current) => current
                .iter()
                .zip(components.iter())
                .take_while(|(left, right)| left == right)
                .map(|(left, _)| *left)
                .collect(),
        });
    }
    let root: PathBuf = root?.iter().collect();
    (root.components().count() > 0).then_some(root)
}

/// Each source relative to the common root, in plan order.
fn referenced_files(plan: &ExportPlan) -> Option<(PathBuf, Vec<PathBuf>)> {
    let root = common_source_root(plan)?;
    let files = plan
        .items
        .iter()
        .map(|item| {
            item.source
                .strip_prefix(&root)
                .map(Path::to_path_buf)
                .unwrap_or_else(|_| item.source.clone())
        })
        .collect();
    Some((root, files))
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
pub fn shell_script(plan: &ExportPlan, run: WbppRun, files: &WbppFiles) -> String {
    let roots = populated_roots(plan);
    let mut script = String::new();
    script.push_str("#!/bin/sh\n");
    script.push_str(&preamble("# ", files));
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
    match files {
        WbppFiles::Placed => {
            for root in &roots {
                script.push_str(&format!("PARAMS=\"$PARAMS,dir=$HERE/{root}\"\n"));
            }
            if has_sessions(plan) {
                script.push_str(&session_grouping_note("# "));
                script.push_str(&format!(
                    "PARAMS=\"$PARAMS,groupingKeywordsEnabled=true,keywords={SESSION_KEYWORD}\"\n"
                ));
            }
        }
        WbppFiles::Referenced { remote_root } => {
            if let Some((root, relative)) = referenced_files(plan) {
                // A POSIX root is what this script can use; a Windows share
                // named for the other runner is not.
                let default_root = remote_root
                    .as_deref()
                    .filter(|root| root.starts_with('/'))
                    .map(str::to_string)
                    .unwrap_or_else(|| root.to_string_lossy().into_owned());
                script.push_str(&format!(
                    "\n# The frames stay where they are. If PixInsight runs on another\n\
                     # machine, set PSF_SOURCE_ROOT to this folder as that machine sees it.\n\
                     SRC=\"${{PSF_SOURCE_ROOT:-{default_root}}}\"\n"
                ));
                for path in relative {
                    script.push_str(&format!(
                        "PARAMS=\"$PARAMS,file=$SRC/{}\"\n",
                        path.to_string_lossy()
                    ));
                }
            }
        }
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
pub fn batch_script(plan: &ExportPlan, run: WbppRun, files: &WbppFiles) -> String {
    let roots = populated_roots(plan);
    let mut script = String::new();
    script.push_str("@echo off\r\n");
    script.push_str(&preamble("REM ", files).replace('\n', "\r\n"));
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
    match files {
        WbppFiles::Placed => {
            for root in &roots {
                script.push_str(&format!("set \"PARAMS=%PARAMS%,dir=%HERE%\\{root}\"\r\n"));
            }
            if has_sessions(plan) {
                script.push_str(&session_grouping_note("REM ").replace('\n', "\r\n"));
                script.push_str(&format!(
                    "set \"PARAMS=%PARAMS%,groupingKeywordsEnabled=true,keywords={SESSION_KEYWORD}\"\r\n"
                ));
            }
        }
        WbppFiles::Referenced { remote_root } => {
            if let Some((root, relative)) = referenced_files(plan) {
                let default_root = remote_root
                    .clone()
                    .unwrap_or_else(|| root.to_string_lossy().replace('/', "\\"));
                script.push_str(&format!(
                    "\r\nREM The frames stay where they are. If this machine mounts them\r\n\
                     REM somewhere else, set PSF_SOURCE_ROOT to that folder first.\r\n\
                     if not defined PSF_SOURCE_ROOT set \"PSF_SOURCE_ROOT={default_root}\"\r\n"
                ));
                for path in relative {
                    script.push_str(&format!(
                        "set \"PARAMS=%PARAMS%,file=%PSF_SOURCE_ROOT%\\{}\"\r\n",
                        path.to_string_lossy().replace('/', "\\")
                    ));
                }
            }
        }
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

/// Why the runner turns keyword grouping on, for the reader of the script.
fn session_grouping_note(comment: &str) -> String {
    format!(
        "\n{comment}Flats sit in a {SESSION_KEYWORD}_<night> folder with the lights they\n\
         {comment}calibrate. WBPP reads that folder as a grouping keyword and pairs\n\
         {comment}each night's lights with its own flats before calibration; bias\n\
         {comment}and darks carry no session and serve every night. The nights\n\
         {comment}integrate together afterwards.\n"
    )
}

fn preamble(comment: &str, files: &WbppFiles) -> String {
    let layout = match files {
        WbppFiles::Placed => format!(
            "{comment}WBPP reads each frame's type from its IMAGETYP header, so the\n\
             {comment}folders below are for your benefit rather than its. It groups\n\
             {comment}lights by filter, exposure, binning and gain the same way.\n\
             {comment}\n\
             {comment}Each directory is scanned recursively, which is why the frame\n\
             {comment}roots are listed one by one: scanning the export root would\n\
             {comment}sweep up {OUTPUT_DIRECTORY}/ on a second run and feed WBPP its\n\
             {comment}own output.\n"
        ),
        WbppFiles::Referenced { .. } => format!(
            "{comment}Nothing was copied: every frame is named below where it already\n\
             {comment}is, and WBPP reads each one's type from its IMAGETYP header.\n\
             {comment}Because the paths are the originals', they carry no session\n\
             {comment}folder, so WBPP pools a filter's flats from every night into one\n\
             {comment}master. Export with placed or linked files for per-night flats.\n\
             {comment}\n\
             {comment}The whole list travels in one command-line argument. Linux allows\n\
             {comment}about a thousand frames there, Windows about three hundred; a\n\
             {comment}longer list needs a placed export.\n"
        ),
    };
    format!(
        "{comment}Hand this export to PixInsight's WeightedBatchPreprocessing.\n\
         {comment}Generated by PSF Guard for WBPP {TARGET_WBPP_VERSION}.\n\
         {comment}\n\
         {layout}\
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
pub fn unusable_destination(dest_root: &Path) -> Option<String> {
    dest_root.to_string_lossy().contains(',').then(|| {
        format!(
            "the export path {} contains a comma, which WBPP's command line uses \
             to separate parameters; the frames were written but the runner script \
             was not, because it could not be expressed",
            dest_root.display()
        )
    })
}

/// Whether a referenced export's frames can be named on a WBPP command line.
/// Placed frames have fixed names under the destination, so only a runner
/// that names the originals can meet a comma in one of their paths.
pub fn unusable_sources(plan: &ExportPlan, files: &WbppFiles) -> Option<String> {
    if *files == WbppFiles::Placed {
        return None;
    }
    let offender = plan
        .items
        .iter()
        .find(|item| item.source.to_string_lossy().contains(','))?;
    Some(format!(
        "the frame path {} contains a comma, which WBPP's command line uses to \
         separate parameters; a runner that names the frames where they are \
         cannot express it, so export with placed or linked files instead",
        offender.source.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::export::{ExportItem, FrameKind};

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
            &WbppFiles::Placed,
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
        let script = shell_script(
            &plan_with(&["lights"]),
            WbppRun::LoadOnly,
            &WbppFiles::Placed,
        );
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
        let script = shell_script(
            &plan_with(&["lights", "flats"]),
            WbppRun::LoadOnly,
            &WbppFiles::Placed,
        );
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
        let script = shell_script(
            &plan_with(&["lights"]),
            WbppRun::LoadOnly,
            &WbppFiles::Placed,
        );
        assert!(
            script.contains(&format!("{OUTPUT_DIRECTORY}/logs/*.log")),
            "{script}"
        );
    }

    #[test]
    fn the_windows_runner_uses_crlf_and_percent_expansion() {
        let script = batch_script(
            &plan_with(&["lights", "bias"]),
            WbppRun::LoadOnly,
            &WbppFiles::Placed,
        );
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
        assert!(unusable_destination(Path::new("/data/M42, Trapezium")).is_some());
        assert!(unusable_destination(Path::new("/data/M42_Trapezium")).is_none());
    }

    fn plan_with_sources(sources: &[&str]) -> ExportPlan {
        let mut plan = ExportPlan::default();
        for (index, source) in sources.iter().enumerate() {
            plan.items.push(ExportItem {
                image_id: index as i32,
                calibration_frame_id: None,
                kind: FrameKind::Light,
                source: PathBuf::from(source),
                relative_dest: PathBuf::from("lights").join(format!("{index}.fits")),
                size_bytes: 0,
            });
        }
        plan
    }

    /// A session folder in the tree means nothing to WBPP until the runner
    /// names it as a grouping keyword; without one, the runner must not turn
    /// keyword grouping on and change how a plain export groups.
    #[test]
    fn session_folders_turn_keyword_grouping_on() {
        let mut plan = plan_with(&["lights", "flats"]);
        let without = shell_script(&plan, WbppRun::LoadOnly, &WbppFiles::Placed);
        assert!(!without.contains("keywords="), "{without}");

        plan.items[1].relative_dest = PathBuf::from("flats/M42/Ha/SESSION_2026-09-10/flat.fits");
        let with = shell_script(&plan, WbppRun::LoadOnly, &WbppFiles::Placed);
        assert!(
            with.contains("groupingKeywordsEnabled=true,keywords=SESSION"),
            "{with}"
        );
        let batch = batch_script(&plan, WbppRun::LoadOnly, &WbppFiles::Placed);
        assert!(batch.contains("keywords=SESSION"), "{batch}");
    }

    /// A referenced export names each frame under one root the reader can
    /// point elsewhere, and scans no directory of its own.
    #[test]
    fn a_referenced_export_lists_the_originals_under_a_movable_root() {
        let plan = plan_with_sources(&[
            "/mnt/nas/astro/2026/M42/LIGHT/a.fits",
            "/mnt/nas/astro/_Calibration/FLAT/f.fits",
        ]);
        let files = WbppFiles::Referenced { remote_root: None };
        let script = shell_script(&plan, WbppRun::LoadOnly, &files);
        assert!(
            script.contains("SRC=\"${PSF_SOURCE_ROOT:-/mnt/nas/astro}\""),
            "{script}"
        );
        assert!(
            script.contains("file=$SRC/2026/M42/LIGHT/a.fits"),
            "{script}"
        );
        assert!(
            script.contains("file=$SRC/_Calibration/FLAT/f.fits"),
            "{script}"
        );
        assert!(!script.contains("dir="), "{script}");
        assert!(!script.contains("keywords="), "{script}");

        // The Windows runner takes the share the other machine mounts.
        let files = WbppFiles::Referenced {
            remote_root: Some("\\\\nas\\astro".into()),
        };
        let batch = batch_script(&plan, WbppRun::LoadOnly, &files);
        assert!(
            batch.contains("set \"PSF_SOURCE_ROOT=\\\\nas\\astro\""),
            "{batch}"
        );
        assert!(
            batch.contains("file=%PSF_SOURCE_ROOT%\\2026\\M42\\LIGHT\\a.fits"),
            "{batch}"
        );
        // A Windows share is no use to the POSIX runner, which keeps the
        // local root instead.
        let shell = shell_script(&plan, WbppRun::LoadOnly, &files);
        assert!(shell.contains("PSF_SOURCE_ROOT:-/mnt/nas/astro"), "{shell}");
    }

    /// Only a runner that names the originals can meet a comma in their
    /// paths; a placed export's names are its own.
    #[test]
    fn a_comma_in_a_referenced_source_is_refused() {
        let plan = plan_with_sources(&["/data/M42, Trapezium/a.fits"]);
        assert!(unusable_sources(&plan, &WbppFiles::Placed).is_none());
        assert!(unusable_sources(&plan, &WbppFiles::Referenced { remote_root: None }).is_some());
    }
}
