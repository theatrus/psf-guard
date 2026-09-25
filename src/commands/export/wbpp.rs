//! The runner scripts that hand a WBPP-layout export to PixInsight.
//!
//! WeightedBatchPreprocessing 3.x is driven from PixInsight's command line:
//! `automationMode=true` suppresses the dialog, `dir=` and `file=` add
//! frames, `outputDirectory=` says where the results go, and a long list of
//! `name=value` parameters sets what the dialog's controls would. Two
//! settings people reach for have no parameter, because WBPP keeps them per
//! light group: drizzle, and Fast Integration, which WBPP switches on by
//! itself for any group of 150 frames or more. So the useful thing to
//! generate is a PixInsight script, `run-wbpp.js`, that carries the frames,
//! the parameters and those two per-group settings, plus a shell and a
//! batch launcher for it that a person can read, edit and re-run.
//!
//! Checked against PixInsight 1.9.5 with WBPP 3.1.0.

use super::{ExportPlan, SESSION_KEYWORD};
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};

/// WBPP release these scripts were written against. Its automation parameters
/// are stable within a major version but have changed across them, so the
/// generated script says what it expects.
pub const TARGET_WBPP_VERSION: &str = "3.1.0";

/// The four roots the WBPP layout writes, in the order the script scans them.
///
/// Named explicitly rather than scanning the export root, because `dir=` is
/// recursive: one root would sweep up `wbpp-out/` on a second run and feed
/// WBPP its own output.
const FRAME_ROOTS: [&str; 4] = ["lights", "flats", "darks", "bias"];

/// Where WBPP writes. A sibling of the frame roots, never inside one.
pub const OUTPUT_DIRECTORY: &str = "wbpp-out";

/// Where WBPP's sources sit below a PixInsight install, on every platform.
pub const BPP_MAIN_BELOW_INSTALL: &str = "src/scripts/BatchPreprocessing/BPP-Main.js";

/// Whether the generated script runs the pipeline or only loads it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
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
    /// Where they already are: `run-wbpp.js` carries the list and hands it
    /// to WBPP from inside PixInsight, so no command line has to hold it.
    ///
    /// `local_root` is the folder the list is relative to, as this machine
    /// sees it; absent, the frames' common parent. `remote_root` is the same
    /// folder as the machine running PixInsight sees it, when that is
    /// another machine (a drive letter or share for a Linux server's mount);
    /// absent, the local root. A frame outside the local root is named by
    /// its full path.
    Referenced {
        local_root: Option<PathBuf>,
        remote_root: Option<String>,
    },
}

/// How hard WBPP works at local normalization, as its own Presets dialog
/// puts it. A headless run starts from WBPP's defaults, which are the
/// maximum, so this mostly buys time back on a run that can afford less.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WbppQuality {
    /// Local normalization on, PSF type Auto, every star WBPP will take.
    #[default]
    Maximum,
    /// Local normalization on, Moffat 4, 500 stars.
    Good,
    /// Local normalization off.
    Fast,
}

/// Whether light groups integrate the fast, in-memory way. WBPP switches
/// any group of 150 frames or more to it on its own, which a person running
/// headless would never see happen.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WbppFastIntegration {
    /// Never: every group takes the full weighted integration.
    #[default]
    Off,
    /// Let WBPP switch large groups to it, as it does in the dialog.
    Auto,
    /// Every light group.
    On,
}

/// Drizzle integration of the light groups, with WBPP's default drop shrink
/// and kernel and its fast overlap tables.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum WbppDrizzle {
    #[default]
    #[serde(rename = "off")]
    Off,
    #[serde(rename = "2x")]
    Scale2,
    #[serde(rename = "3x")]
    Scale3,
}

impl WbppDrizzle {
    pub fn scale(self) -> Option<u8> {
        match self {
            WbppDrizzle::Off => None,
            WbppDrizzle::Scale2 => Some(2),
            WbppDrizzle::Scale3 => Some(3),
        }
    }
}

/// The pixel rejection algorithm for the light integration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WbppRejection {
    PercentileClip,
    WinsorizedSigma,
    LinearFit,
    Esd,
    Rcr,
    Auto,
}

impl WbppRejection {
    /// WBPP's index for the `rejection_N` parameter.
    pub fn index(self) -> u8 {
        match self {
            WbppRejection::PercentileClip => 0,
            WbppRejection::WinsorizedSigma => 1,
            WbppRejection::LinearFit => 2,
            WbppRejection::Esd => 3,
            WbppRejection::Rcr => 4,
            WbppRejection::Auto => 5,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            WbppRejection::PercentileClip => "percentile clipping",
            WbppRejection::WinsorizedSigma => "Winsorized sigma clipping",
            WbppRejection::LinearFit => "linear fit clipping",
            WbppRejection::Esd => "generalized ESD",
            WbppRejection::Rcr => "robust Chauvenet",
            WbppRejection::Auto => "automatic",
        }
    }
}

/// What a run asks of WBPP beyond the frames.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WbppOptions {
    #[serde(default)]
    pub quality: WbppQuality,
    #[serde(default)]
    pub fast_integration: WbppFastIntegration,
    #[serde(default)]
    pub drizzle: WbppDrizzle,
    /// Crop the integrated masters to the area every frame covers. Absent
    /// leaves WBPP's default (on).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autocrop: Option<bool>,
    /// The light integration's rejection algorithm. Absent leaves WBPP's
    /// default (automatic).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rejection: Option<WbppRejection>,
}

/// The number of stars WBPP's own maximum-quality preset lets local
/// normalization use.
const LN_MAX_STARS: u32 = 24576;

/// WBPP's index of the light image type in `rejection_N`.
const LIGHT_TYPE_INDEX: u8 = 4;

impl WbppOptions {
    /// The `name=value` parameters WBPP's automation mode takes for these
    /// options, in the spelling of WBPP 3.x.
    pub fn automation_params(&self) -> Vec<String> {
        let mut params = Vec::new();
        match self.quality {
            WbppQuality::Maximum => params.extend([
                "localNormalization=true".to_string(),
                "localNormalizationPsfType=6".to_string(),
                format!("localNormalizationPsfMaxStars={LN_MAX_STARS}"),
            ]),
            WbppQuality::Good => params.extend([
                "localNormalization=true".to_string(),
                "localNormalizationPsfType=2".to_string(),
                "localNormalizationPsfMaxStars=500".to_string(),
            ]),
            WbppQuality::Fast => params.push("localNormalization=false".to_string()),
        }
        // Read on import, before any frame is added, so no group ever
        // crosses the threshold that switches it.
        if self.fast_integration == WbppFastIntegration::Off {
            params.push("autoIntegrationMode=false".to_string());
        }
        if let Some(autocrop) = self.autocrop {
            params.push(format!("autocrop={autocrop}"));
        }
        if let Some(rejection) = self.rejection {
            params.push(format!(
                "rejection_{LIGHT_TYPE_INDEX}={}",
                rejection.index()
            ));
        }
        params
    }

    /// The per-group drizzle setting as the script's JavaScript literal, or
    /// `null` for none.
    fn drizzle_literal(&self) -> String {
        match self.drizzle.scale() {
            Some(scale) => format!("{{ enabled: true, fast: true, scale: {scale} }}"),
            None => "null".to_string(),
        }
    }

    /// The per-group Fast Integration setting as a JavaScript literal:
    /// `true`, `false`, or `null` to leave WBPP to it.
    fn fast_integration_literal(&self) -> &'static str {
        match self.fast_integration {
            WbppFastIntegration::Off => "false",
            WbppFastIntegration::On => "true",
            WbppFastIntegration::Auto => "null",
        }
    }

    /// One line per option, for the script's header and the person.
    pub fn describe(&self) -> Vec<String> {
        let mut lines = vec![
            match self.quality {
                WbppQuality::Maximum => {
                    "Quality: maximum (local normalization on, PSF Auto, every star)".to_string()
                }
                WbppQuality::Good => {
                    "Quality: good (local normalization on, Moffat 4, 500 stars)".to_string()
                }
                WbppQuality::Fast => "Quality: fast (local normalization off)".to_string(),
            },
            match self.fast_integration {
                WbppFastIntegration::Off => {
                    "Fast Integration: off for every light group".to_string()
                }
                WbppFastIntegration::Auto => {
                    "Fast Integration: WBPP decides (on for groups of 150 frames or more)"
                        .to_string()
                }
                WbppFastIntegration::On => "Fast Integration: on for every light group".to_string(),
            },
            match self.drizzle.scale() {
                Some(scale) => format!("Drizzle: {scale}x, WBPP's default drop shrink and kernel"),
                None => "Drizzle: off".to_string(),
            },
        ];
        if let Some(autocrop) = self.autocrop {
            lines.push(format!("Autocrop: {}", if autocrop { "on" } else { "off" }));
        }
        if let Some(rejection) = self.rejection {
            lines.push(format!("Light rejection: {}", rejection.label()));
        }
        lines
    }
}

/// Everything the scripts need to know besides the plan.
#[derive(Debug, Clone, Default)]
pub struct WbppScriptSpec {
    pub run: WbppRun,
    pub files: WbppFiles,
    pub options: WbppOptions,
    /// WBPP's `BPP-Main.js` on the machine that will run the script, when
    /// known (an in-app run). Absent, the script tries each platform's
    /// standard install path.
    pub bpp_main: Option<PathBuf>,
}

/// The script a WBPP export writes beside its runners.
pub const JS_RUNNER: &str = "run-wbpp.js";

/// A path as PixInsight spells it on every platform: forward slashes.
fn slashed(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// A string as a JavaScript literal.
fn js_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".into())
}

/// A root as typed by a person, without the separator a drive letter or a
/// share often ends in, so joining it to a relative path gives one separator.
fn trim_root(root: &str) -> &str {
    let trimmed = root.trim().trim_end_matches(['/', '\\']);
    // "/" alone is a root too, and trimming it would leave nothing.
    if trimmed.is_empty() && root.trim().starts_with('/') {
        "/"
    } else {
        trimmed
    }
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

/// One frame as the runner names it: below the root, or by its full path
/// when it sits outside. `session` is the flat session the placed layout
/// would have put in its path, which the script hands WBPP instead.
struct ReferencedFile {
    place: ReferencedPlace,
    session: Option<String>,
}

enum ReferencedPlace {
    Below(PathBuf),
    Outside(PathBuf),
}

/// The session label a placed path carries, if any.
fn session_of(relative_dest: &Path) -> Option<String> {
    let prefix = format!("{SESSION_KEYWORD}_");
    relative_dest.components().find_map(|component| {
        component
            .as_os_str()
            .to_string_lossy()
            .strip_prefix(&prefix)
            .map(str::to_string)
    })
}

/// The root and each source's place against it, in plan order.
fn referenced_files(
    plan: &ExportPlan,
    local_root: Option<&Path>,
) -> Option<(PathBuf, Vec<ReferencedFile>)> {
    let root = match local_root {
        Some(root) => root.to_path_buf(),
        None => common_source_root(plan)?,
    };
    let files = plan
        .items
        .iter()
        .map(|item| ReferencedFile {
            place: match item.source.strip_prefix(&root) {
                Ok(relative) => ReferencedPlace::Below(relative.to_path_buf()),
                Err(_) => ReferencedPlace::Outside(item.source.clone()),
            },
            session: session_of(&item.relative_dest),
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
pub fn shell_script(plan: &ExportPlan, spec: &WbppScriptSpec) -> String {
    let mut script = String::new();
    script.push_str("#!/bin/sh\n");
    script.push_str(&preamble("# ", plan, spec));
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
         \n",
    );
    script.push_str(&format!(
        "# {JS_RUNNER} carries the frames and every WBPP setting, and hands them\n\
         # to WBPP from inside PixInsight. Anything else added to PARAMS as\n\
         # name=value goes to WBPP as an automation parameter.\n\
         PARAMS=\"$HERE/{JS_RUNNER},here=$HERE,outputDirectory=$HERE/{OUTPUT_DIRECTORY}\"\n"
    ));
    if matches!(spec.files, WbppFiles::Referenced { .. }) {
        script.push_str(
            "# If PixInsight sees the frames' folder elsewhere, set PSF_SOURCE_ROOT to\n\
             # it, or edit psfSourceRoot in the script.\n\
             [ -n \"$PSF_SOURCE_ROOT\" ] && PARAMS=\"$PARAMS,sourceRoot=$PSF_SOURCE_ROOT\"\n",
        );
    }
    script.push_str(match spec.run {
        WbppRun::LoadOnly => {
            "\n# The script loads the frames and groups, then stops with the dialog\n\
             # open so you can check them. Append ,run to PARAMS to run the pipeline.\n"
        }
        WbppRun::Full => {
            "\n# The script runs the whole pipeline headless. Append ,loadOnly to\n\
             # PARAMS to stop at the dialog and check the groups first.\n"
        }
    });
    script.push_str(&format!(
        "\nmkdir -p \"$HERE/{OUTPUT_DIRECTORY}\"\n\
         exec \"$PI_BIN\" -n --automation-mode -r=\"$PARAMS\" --force-exit\n"
    ));
    script
}

/// The Windows runner, so an export can be zipped and taken to another machine.
pub fn batch_script(plan: &ExportPlan, spec: &WbppScriptSpec) -> String {
    let mut script = String::new();
    script.push_str("@echo off\r\n");
    script.push_str(&preamble("REM ", plan, spec).replace('\n', "\r\n"));
    script.push_str(
        "\r\n\
         setlocal\r\n\
         set \"HERE=%~dp0\"\r\n\
         set \"HERE=%HERE:~0,-1%\"\r\n\
         \r\n\
         REM Point these at your install if it is somewhere else.\r\n\
         if not defined PI_ROOT set \"PI_ROOT=C:\\Program Files\\PixInsight\"\r\n\
         if not defined PI_BIN set \"PI_BIN=%PI_ROOT%\\bin\\PixInsight.exe\"\r\n\
         \r\n",
    );
    script.push_str(&format!(
        "REM {JS_RUNNER} carries the frames and every WBPP setting, and hands them\r\n\
         REM to WBPP from inside PixInsight. Anything else added to PARAMS as\r\n\
         REM name=value goes to WBPP as an automation parameter.\r\n\
         set \"PARAMS=%HERE%\\{JS_RUNNER},here=%HERE%,outputDirectory=%HERE%\\{OUTPUT_DIRECTORY}\"\r\n"
    ));
    if matches!(spec.files, WbppFiles::Referenced { .. }) {
        script.push_str(
            "REM If this machine sees the frames' folder elsewhere, set PSF_SOURCE_ROOT\r\n\
             REM to it, or edit psfSourceRoot in the script.\r\n\
             if defined PSF_SOURCE_ROOT set \"PARAMS=%PARAMS%,sourceRoot=%PSF_SOURCE_ROOT%\"\r\n",
        );
    }
    script.push_str(match spec.run {
        WbppRun::LoadOnly => {
            "\r\nREM The script loads the frames and groups, then stops with the dialog\r\n\
             REM open so you can check them. Append ,run to PARAMS to run the pipeline.\r\n"
        }
        WbppRun::Full => {
            "\r\nREM The script runs the whole pipeline headless. Append ,loadOnly to\r\n\
             REM PARAMS to stop at the dialog and check the groups first.\r\n"
        }
    });
    script.push_str(&format!(
        "\r\nif not exist \"%HERE%\\{OUTPUT_DIRECTORY}\" mkdir \"%HERE%\\{OUTPUT_DIRECTORY}\"\r\n\
         \"%PI_BIN%\" -n --automation-mode -r=\"%PARAMS%\" --force-exit\r\n"
    ));
    script
}

/// Why the runner turns keyword grouping on, for the reader of the script.
fn session_grouping_note(comment: &str) -> String {
    format!(
        "{comment}\n\
         {comment}Flats sit in a {SESSION_KEYWORD}_<night> folder with the lights they\n\
         {comment}calibrate. WBPP reads that folder as a grouping keyword and pairs\n\
         {comment}each night's lights with its own flats before calibration; bias\n\
         {comment}and darks carry no session and serve every night. The nights\n\
         {comment}integrate together afterwards.\n"
    )
}

fn preamble(comment: &str, plan: &ExportPlan, spec: &WbppScriptSpec) -> String {
    let layout = match spec.files {
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
            "{comment}Nothing was copied: {JS_RUNNER} names every frame where it already\n\
             {comment}is, and WBPP reads each one's type from its IMAGETYP header. The\n\
             {comment}script also tells WBPP which night's flats each light calibrates\n\
             {comment}with, as a {SESSION_KEYWORD} grouping keyword, since the originals'\n\
             {comment}paths carry none.\n"
        ),
    };
    let sessions = if matches!(spec.files, WbppFiles::Placed) && has_sessions(plan) {
        session_grouping_note(comment)
    } else {
        String::new()
    };
    let options: String = spec
        .options
        .describe()
        .iter()
        .map(|line| format!("{comment}  {line}\n"))
        .collect();
    format!(
        "{comment}Hand this export to PixInsight's WeightedBatchPreprocessing.\n\
         {comment}Generated by PSF Guard for WBPP {TARGET_WBPP_VERSION}.\n\
         {comment}\n\
         {layout}\
         {sessions}\
         {comment}\n\
         {comment}Settings, as {JS_RUNNER} passes them to WBPP:\n\
         {options}\
         {comment}\n\
         {comment}PixInsight prints nothing to the terminal in this mode: WBPP\n\
         {comment}writes to its own console, so a finished run and a failed one\n\
         {comment}look alike from outside. Read the .log files in\n\
         {comment}  {OUTPUT_DIRECTORY}/logs\n\
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

/// The `#include` of WBPP's entry file: the known one for an in-app run,
/// else each platform's standard install path.
fn include_bpp_main(bpp_main: Option<&Path>) -> String {
    match bpp_main {
        Some(path) => format!("#include {}\n", js_string(&slashed(path))),
        None => format!(
            "#ifeq __PI_PLATFORM__ MSWINDOWS\n\
             #include \"C:/Program Files/PixInsight/{BPP_MAIN_BELOW_INSTALL}\"\n\
             #else\n\
             #ifeq __PI_PLATFORM__ MACOSX\n\
             #include \"/Applications/PixInsight/{BPP_MAIN_BELOW_INSTALL}\"\n\
             #else\n\
             #include \"/opt/PixInsight/{BPP_MAIN_BELOW_INSTALL}\"\n\
             #endif\n\
             #endif\n"
        ),
    }
}

/// The PixInsight script of an export: the frames, and WBPP itself, so no
/// list has to fit a command line and no setting is left to the dialog.
///
/// WBPP reads its parameters from `Runtime.jsArguments`, which the core
/// makes read-only. The script includes WBPP's entry file inside a function
/// where a Proxy named `Runtime` answers `jsArguments` with the list built
/// here and everything else from the real object, then calls WBPP's entry
/// point the way `WBPP.js` does. A referenced export's frames are the
/// originals, whose paths carry no session, so the script also replaces
/// WBPP's path reader with one that answers `SESSION` for them; WBPP
/// consults it on every regroup, so the value holds. Drizzle and Fast
/// Integration are per light group and have no parameter, so the script
/// sets them on every light group just before WBPP builds its pipeline,
/// whether the run started headless or from the dialog. `#engine v8` is
/// what `WBPP.js` declares; without it the core compiles the include with
/// an engine that rejects WBPP's classes. The core's preprocessor also opens
/// a block comment at any `/*`, even inside a `//` comment, and closes it at
/// the next `*/` wherever that is, so the script must not contain one; a
/// glob in a comment once swallowed everything up to WBPP's own entry call.
pub fn js_runner(plan: &ExportPlan, spec: &WbppScriptSpec) -> String {
    let entry = |path: &Path, session: Option<&str>| match session {
        Some(session) => format!(
            "{{ path: {}, session: {} }}",
            js_string(&slashed(path)),
            js_string(session)
        ),
        None => format!("{{ path: {} }}", js_string(&slashed(path))),
    };

    let mut script = preamble("// ", plan, spec);
    script.push_str(&format!(
        "//\n\
         // Run it with run-wbpp.sh or run-wbpp.cmd, which pass the folders, or\n\
         // from PixInsight's Script > Execute Script File. WBPP must be where the\n\
         // #include line below expects; edit it for another install.\n\
         \n\
         #engine v8\n\
         #include <pjsr/StdButton.jsh>\n\
         #include <pjsr/StdIcon.jsh>\n\
         \n\
         // Where WBPP writes. Passed as outputDirectory=<folder> by the runners;\n\
         // empty means a {OUTPUT_DIRECTORY} folder in your home directory.\n\
         var psfOutputDirectory = \"\";\n\
         \n\
         // true loads the frames and groups, then stops with the dialog open so\n\
         // you can check them; false runs the whole pipeline. Passing run or\n\
         // loadOnly as an argument overrides it.\n\
         var psfLoadOnly = {load_only};\n\
         \n\
         // WBPP's automation parameters for the settings chosen at export.\n\
         var psfSettings = [\n",
        load_only = match spec.run {
            WbppRun::LoadOnly => "true",
            WbppRun::Full => "false",
        },
    ));
    for param in spec.options.automation_params() {
        script.push_str(&format!("   {},\n", js_string(&param)));
    }
    script.push_str(&format!(
        "];\n\
         \n\
         // Per light group, which WBPP has no parameter for: drizzle (null for\n\
         // none) and Fast Integration (true, false, or null to let WBPP decide).\n\
         var psfDrizzle = {drizzle};\n\
         var psfFastIntegration = {fast_integration};\n\
         \n",
        drizzle = spec.options.drizzle_literal(),
        fast_integration = spec.options.fast_integration_literal(),
    ));

    match &spec.files {
        WbppFiles::Placed => {
            let roots: Vec<String> = populated_roots(plan)
                .iter()
                .map(|root| js_string(root))
                .collect();
            script.push_str(&format!(
                "// The frame folders beside this script, which WBPP scans. The runners\n\
                 // pass their parent as here=<folder>.\n\
                 var psfFrameRoots = [{roots}];\n\
                 // Whether the flats sit in {SESSION_KEYWORD}_<night> folders WBPP groups by.\n\
                 var psfSessionGrouping = {sessions};\n\
                 var psfSourceRoot = \"\";\n\
                 var psfFrames = [];\n\
                 var psfOutsideFrames = [];\n",
                roots = roots.join(", "),
                sessions = has_sessions(plan),
            ));
        }
        WbppFiles::Referenced {
            local_root,
            remote_root,
        } => {
            let (root, referenced) = referenced_files(plan, local_root.as_deref())
                .unwrap_or_else(|| (PathBuf::new(), Vec::new()));
            let local = slashed(&root);
            let source_root = remote_root
                .as_deref()
                .map(trim_root)
                .map(|root| root.replace('\\', "/"))
                .unwrap_or_else(|| local.clone());
            let mut below = Vec::new();
            let mut outside = Vec::new();
            for file in referenced {
                match file.place {
                    ReferencedPlace::Below(path) => {
                        below.push(entry(&path, file.session.as_deref()))
                    }
                    ReferencedPlace::Outside(path) => {
                        outside.push(entry(&path, file.session.as_deref()))
                    }
                }
            }
            script.push_str(&format!(
                "var psfFrameRoots = [];\n\
                 var psfSessionGrouping = false;\n\
                 \n\
                 // The folder the frames are listed below, as PixInsight sees it. On the\n\
                 // machine that made this export it is\n\
                 //   {local}\n\
                 // Edit this, or pass sourceRoot=<folder>, if PixInsight sees it elsewhere.\n\
                 var psfSourceRoot = {source_root};\n\
                 \n\
                 // The frames, relative to psfSourceRoot. A session is the night a\n\
                 // light's flats were shot; lights and flats of one night share it, and\n\
                 // WBPP calibrates each night with its own flats before integrating\n\
                 // the nights together.\n\
                 var psfFrames = [\n",
                source_root = js_string(&source_root),
            ));
            for path in &below {
                script.push_str(&format!("   {path},\n"));
            }
            script.push_str(
                "];\n\n// Frames outside that folder, by full path.\nvar psfOutsideFrames = [\n",
            );
            for path in &outside {
                script.push_str(&format!("   {path},\n"));
            }
            script.push_str("];\n");
        }
    }

    script.push_str(&format!(
        "\n\
         var psfRealRuntime = Runtime;\n\
         (function () {{\n\
         \x20  var given = {{}};\n\
         \x20  for (var i = 0; i < psfRealRuntime.jsArguments.length; ++i) {{\n\
         \x20     var item = psfRealRuntime.jsArguments[i];\n\
         \x20     var at = item.indexOf(\"=\");\n\
         \x20     if (at > 0) given[item.slice(0, at)] = item.slice(at + 1); else given[item] = true;\n\
         \x20  }}\n\
         \x20  var out = given.outputDirectory || psfOutputDirectory || (File.homeDirectory + \"/{OUTPUT_DIRECTORY}\");\n\
         \x20  if (!File.directoryExists(out)) File.createDirectory(out, true);\n\
         \x20  var loadOnly = given.run ? false : given.loadOnly ? true : psfLoadOnly;\n\
         \x20  var args = [\"automationMode=true\"];\n\
         \x20  var sessionByPath = {{}};\n\
         \x20  var withSession = psfSessionGrouping ? 1 : 0;\n\
         \n\
         \x20  // Placed frames: the folders beside the runner, scanned by WBPP.\n\
         \x20  if (psfFrameRoots.length > 0) {{\n\
         \x20     var here = String(given.here || \"\").replace(/\\\\/g, \"/\").replace(/\\/+$/, \"\");\n\
         \x20     if (!here) throw new Error(\"Pass here=<folder> naming the export folder, as run-wbpp.sh does.\");\n\
         \x20     for (var r = 0; r < psfFrameRoots.length; ++r) {{\n\
         \x20        var dir = here + \"/\" + psfFrameRoots[r];\n\
         \x20        if (File.directoryExists(dir)) args.push(\"dir=\" + dir);\n\
         \x20        else console.warningln(\"PSF Guard: no \" + dir + \" folder; skipped\");\n\
         \x20     }}\n\
         \x20  }}\n\
         \n\
         \x20  // Referenced frames: the originals, listed below one root.\n\
         \x20  var root = String(given.sourceRoot || psfSourceRoot).replace(/\\\\/g, \"/\").replace(/\\/+$/, \"\");\n\
         \x20  var paths = [];\n\
         \x20  function take(frame, path) {{\n\
         \x20     paths.push(path);\n\
         \x20     if (frame.session) {{ sessionByPath[path] = frame.session; ++withSession; }}\n\
         \x20  }}\n\
         \x20  for (var j = 0; j < psfFrames.length; ++j) take(psfFrames[j], root + \"/\" + psfFrames[j].path);\n\
         \x20  for (var k = 0; k < psfOutsideFrames.length; ++k) take(psfOutsideFrames[k], psfOutsideFrames[k].path);\n\
         \x20  if (paths.length > 0) {{\n\
         \x20     // WBPP drops a frame it cannot find without a word, and a wrong root\n\
         \x20     // then opens an empty dialog. Look first, and say what is missing.\n\
         \x20     var missing = paths.filter(function (path) {{ return !File.exists(path); }});\n\
         \x20     console.noteln(\"PSF Guard: \" + paths.length + \" frames below \" + root + \", \" + missing.length + \" not found\");\n\
         \x20     if (missing.length > 0) {{\n\
         \x20        var report = missing.length + \" of \" + paths.length + \" frames were not found. \" +\n\
         \x20           \"The list is relative to psfSourceRoot = \" + root + \"; edit it or pass \" +\n\
         \x20           \"sourceRoot=<folder> so that the first one exists:\\n\" + missing.slice(0, 5).join(\"\\n\");\n\
         \x20        console.criticalln(report);\n\
         \x20        if (missing.length == paths.length) {{\n\
         \x20           if (loadOnly) (new MessageBox(report, \"PSF Guard export\", StdIcon_Error, StdButton_Ok)).execute();\n\
         \x20           throw new Error(\"No frame found below \" + root);\n\
         \x20        }}\n\
         \x20     }}\n\
         \x20     for (var m = 0; m < paths.length; ++m) args.push(\"file=\" + paths[m]);\n\
         \x20  }}\n\
         \n\
         \x20  if (withSession > 0) args.push(\"groupingKeywordsEnabled=true\", \"keywords={SESSION_KEYWORD}\");\n\
         \x20  for (var s = 0; s < psfSettings.length; ++s) args.push(psfSettings[s]);\n\
         \x20  // Anything else the runner was given is a WBPP parameter, so a person\n\
         \x20  // can add one to PARAMS without editing this script.\n\
         \x20  var own = {{ here: 1, sourceRoot: 1, outputDirectory: 1, run: 1, loadOnly: 1 }};\n\
         \x20  for (var name in given) if (!own[name]) args.push(given[name] === true ? name : name + \"=\" + given[name]);\n\
         \x20  args.push(\"outputDirectory=\" + out);\n\
         \x20  if (loadOnly) args.push(\"loadOnly\");\n\
         \n\
         \x20  // WBPP takes its parameters from Runtime.jsArguments, which is read-only.\n\
         \x20  // Inside this function, Runtime is a view of the real one that answers\n\
         \x20  // jsArguments with the list above.\n\
         \x20  let Runtime = new Proxy(psfRealRuntime, {{\n\
         \x20     get: function (target, property) {{\n\
         \x20        return property === \"jsArguments\" ? args : Reflect.get(target, property);\n\
         \x20     }}\n\
         \x20  }});\n\
         \n\
         {include}\
         \n\
         \x20  // WBPP reads a grouping keyword's value out of a frame's path, and\n\
         \x20  // referenced paths are the originals', which carry none. So the reader\n\
         \x20  // it consults answers {SESSION_KEYWORD} for our frames from the list above and\n\
         \x20  // leaves every other question to WBPP. It is consulted again whenever\n\
         \x20  // WBPP regroups, so the value stays.\n\
         \x20  if (paths.length > 0 && withSession > 0) {{\n\
         \x20     var readKeyFromPath = WBPPUtils.smartNaming.getCustomKeyValueFromPath;\n\
         \x20     WBPPUtils.smartNaming.getCustomKeyValueFromPath = function (key, filePath) {{\n\
         \x20        if (key == \"{SESSION_KEYWORD}\") {{\n\
         \x20           var session = sessionByPath[filePath] || sessionByPath[String(filePath).replace(/\\\\/g, \"/\")];\n\
         \x20           if (session !== undefined) return session;\n\
         \x20        }}\n\
         \x20        return readKeyFromPath.call(this, key, filePath);\n\
         \x20     }};\n\
         \x20  }}\n\
         \n\
         \x20  // Drizzle and Fast Integration live on each light group, so they are\n\
         \x20  // set on every one just before WBPP builds its pipeline: after the\n\
         \x20  // frames are grouped, and whether the run started here or from the\n\
         \x20  // dialog's Run button.\n\
         \x20  if (psfDrizzle || psfFastIntegration !== null) {{\n\
         \x20     var buildPipeline = engine.pipelineManager.buildExecutionPipeline;\n\
         \x20     engine.pipelineManager.buildExecutionPipeline = function () {{\n\
         \x20        var groups = engine.groupsManager.groupsForMode(BPP.GroupingMode.POST);\n\
         \x20        for (var g = 0; g < groups.length; ++g) {{\n\
         \x20           var group = groups[g];\n\
         \x20           if (group.imageType != ImageType.Light) continue;\n\
         \x20           if (psfFastIntegration !== null) group.enableFastIntegration(psfFastIntegration, true);\n\
         \x20           if (psfDrizzle) {{\n\
         \x20              group.setDrizzleData(psfDrizzle);\n\
         \x20              if (!group.isDrizzleEnabled()) console.warningln(\"PSF Guard: drizzle is not available for group \" + group.id);\n\
         \x20           }}\n\
         \x20        }}\n\
         \x20        return buildPipeline.apply(this, arguments);\n\
         \x20     }};\n\
         \x20  }}\n\
         \n\
         \x20  // What WBPP.js itself does after its includes; false is fastMode.\n\
         \x20  CoreApplication.ensureMinimumVersion(1, 9, 4);\n\
         \x20  BPPmain(false, BPP.Version.WBPP_ID, BPP.Version.WBPP_TITLE,\n\
         \x20     BPP.Version.WBPP_SETTINGS_KEY_BASE, BPP.Version.WBPP_VERSION);\n\
         }})();\n",
        include = include_bpp_main(spec.bpp_main.as_deref()),
    ));
    script
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

    fn placed(run: WbppRun) -> WbppScriptSpec {
        WbppScriptSpec {
            run,
            ..Default::default()
        }
    }

    fn referenced(local_root: Option<&str>, remote_root: Option<&str>) -> WbppScriptSpec {
        WbppScriptSpec {
            files: WbppFiles::Referenced {
                local_root: local_root.map(PathBuf::from),
                remote_root: remote_root.map(str::to_string),
            },
            ..Default::default()
        }
    }

    /// Lines the runner executes, as opposed to explains.
    fn active_lines(script: &str) -> impl Iterator<Item = &str> {
        script.lines().filter(|line| {
            let line = line.trim_start();
            !line.starts_with('#') && !line.starts_with("REM ") && !line.starts_with("//")
        })
    }

    /// The invocation these produce was run against PixInsight 1.9.4 with
    /// WBPP: it built master bias, dark and flat, matched the dark and flat
    /// to the lights, and calibrated them. These assertions pin the parts
    /// that made that work.
    #[test]
    fn the_shell_runner_launches_the_script_with_the_folders_it_needs() {
        let plan = plan_with(&["lights", "flats", "darks", "bias"]);
        let script = shell_script(&plan, &placed(WbppRun::Full));

        assert!(script.starts_with("#!/bin/sh\n"));
        assert!(script.contains("--automation-mode"));
        assert!(script.contains("--force-exit"));
        assert!(
            script.contains(&format!(
                "PARAMS=\"$HERE/{JS_RUNNER},here=$HERE,outputDirectory=$HERE/{OUTPUT_DIRECTORY}\""
            )),
            "{script}"
        );
        // A full run must set no loadOnly parameter. The comment showing how
        // to add one does mention it, so look only at what executes.
        assert!(
            !active_lines(&script).any(|line| line.contains("loadOnly")),
            "a full run must not stop at the dialog:\n{script}"
        );

        let js = js_runner(&plan, &placed(WbppRun::Full));
        assert!(js.contains("var psfLoadOnly = false;"), "{js}");
        // Every frame root listed separately, never the export root.
        assert!(
            js.contains("var psfFrameRoots = [\"lights\", \"flats\", \"darks\", \"bias\"];"),
            "{js}"
        );
        assert!(js.contains("args.push(\"dir=\" + dir)"), "{js}");
        assert!(js.contains("automationMode=true"), "{js}");
    }

    /// The default stops before an hour of integration starts.
    #[test]
    fn the_default_run_only_loads() {
        assert_eq!(WbppRun::default(), WbppRun::LoadOnly);
        let js = js_runner(&plan_with(&["lights"]), &placed(WbppRun::LoadOnly));
        assert!(js.contains("var psfLoadOnly = true;"), "{js}");
        assert!(
            js.contains("if (loadOnly) args.push(\"loadOnly\");"),
            "{js}"
        );
    }

    /// A root with no frames must not be scanned: WBPP complains about an
    /// empty directory, and an export filtered to one target often has no
    /// bias or darks of its own.
    #[test]
    fn only_the_roots_a_plan_filled_are_scanned() {
        let js = js_runner(&plan_with(&["lights", "flats"]), &placed(WbppRun::LoadOnly));
        assert!(
            js.contains("var psfFrameRoots = [\"lights\", \"flats\"];"),
            "{js}"
        );
        assert!(!js.contains("\"darks\""), "{js}");
        assert!(!js.contains("\"bias\""), "{js}");
    }

    /// PixInsight prints nothing to the terminal in this mode, so a run that
    /// failed looks exactly like one that worked. The script has to say where
    /// to look.
    #[test]
    fn the_runner_says_where_the_real_output_is() {
        let script = shell_script(&plan_with(&["lights"]), &placed(WbppRun::LoadOnly));
        assert!(
            script.contains(&format!("{OUTPUT_DIRECTORY}/logs")),
            "{script}"
        );
    }

    #[test]
    fn the_windows_runner_uses_crlf_and_percent_expansion() {
        let script = batch_script(&plan_with(&["lights", "bias"]), &placed(WbppRun::LoadOnly));
        assert!(script.starts_with("@echo off\r\n"));
        assert!(
            script.contains(&format!(
                "set \"PARAMS=%HERE%\\{JS_RUNNER},here=%HERE%,outputDirectory=%HERE%\\{OUTPUT_DIRECTORY}\""
            )),
            "{script}"
        );
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
        let without = js_runner(&plan, &placed(WbppRun::LoadOnly));
        assert!(
            without.contains("var psfSessionGrouping = false;"),
            "{without}"
        );

        plan.items[1].relative_dest = PathBuf::from("flats/M42/Ha/SESSION_2026-09-10/flat.fits");
        let with = js_runner(&plan, &placed(WbppRun::LoadOnly));
        assert!(with.contains("var psfSessionGrouping = true;"), "{with}");
        assert!(
            with.contains("args.push(\"groupingKeywordsEnabled=true\", \"keywords=SESSION\")"),
            "{with}"
        );
        let shell = shell_script(&plan, &placed(WbppRun::LoadOnly));
        assert!(shell.contains("SESSION_<night> folder"), "{shell}");
    }

    /// A referenced export puts the frame list in the PixInsight script,
    /// under one root the reader can point elsewhere, and the runners only
    /// launch that script: no frame travels on a command line.
    #[test]
    fn a_referenced_export_lists_the_originals_in_the_script() {
        let plan = plan_with_sources(&[
            "/mnt/nas/astro/2026/M42/LIGHT/a.fits",
            "/mnt/nas/astro/_Calibration/FLAT/f.fits",
        ]);
        let spec = referenced(None, None);
        let js = js_runner(&plan, &spec);
        assert!(js.starts_with("// "), "{js}");
        assert!(js.contains("#engine v8"), "{js}");
        assert!(
            js.contains("var psfSourceRoot = \"/mnt/nas/astro\";"),
            "{js}"
        );
        assert!(js.contains("var psfFrameRoots = [];"), "{js}");
        assert!(
            js.contains("   { path: \"2026/M42/LIGHT/a.fits\" },\n"),
            "{js}"
        );
        assert!(
            js.contains("   { path: \"_Calibration/FLAT/f.fits\" },\n"),
            "{js}"
        );
        // No session in this plan, so no keyword grouping and no hook text
        // is needed; the script still carries the hook for a plan that has.
        assert!(js.contains("getCustomKeyValueFromPath"), "{js}");
        assert!(js.contains("var psfLoadOnly = true;"), "{js}");
        assert!(js.contains("new Proxy(psfRealRuntime"), "{js}");
        assert!(js.contains("#ifeq __PI_PLATFORM__ MSWINDOWS"), "{js}");
        assert!(js.contains("BPPmain(false"), "{js}");
        // A frame WBPP cannot find is dropped without a word, so the script
        // checks the paths itself and names the missing ones.
        assert!(js.contains("File.exists(path)"), "{js}");
        assert!(js.contains("not found"), "{js}");
        // The core's preprocessor treats a `/*` anywhere, comments included,
        // as the start of a block comment.
        assert!(!js.contains("/*"), "{js}");
        let full = js_runner(
            &plan,
            &WbppScriptSpec {
                run: WbppRun::Full,
                ..referenced(None, None)
            },
        );
        assert!(full.contains("var psfLoadOnly = false;"), "{full}");

        let shell = shell_script(&plan, &spec);
        assert!(
            shell.contains(&format!(
                "$HERE/{JS_RUNNER},here=$HERE,outputDirectory=$HERE/{OUTPUT_DIRECTORY}"
            )),
            "{shell}"
        );
        assert!(shell.contains("PSF_SOURCE_ROOT"), "{shell}");
        assert!(!shell.contains("file="), "{shell}");
        let batch = batch_script(&plan, &spec);
        assert!(
            batch.contains(&format!(
                "%HERE%\\{JS_RUNNER},here=%HERE%,outputDirectory=%HERE%\\{OUTPUT_DIRECTORY}"
            )),
            "{batch}"
        );
        assert!(!batch.contains("file="), "{batch}");
    }

    /// A frame the placed layout would tag with a session keeps that
    /// session in the script, so WBPP can pair each night's lights and
    /// flats although the originals' paths say nothing about it.
    #[test]
    fn a_referenced_export_carries_each_frames_session() {
        let mut plan = plan_with_sources(&[
            "/mnt/nas/astro/2026/M42/LIGHT/a.fits",
            "/mnt/nas/astro/_Calibration/FLAT/f.fits",
            "/mnt/nas/astro/_Calibration/BIAS/b.fits",
        ]);
        plan.items[0].relative_dest = PathBuf::from("lights/M42/Ha/SESSION_2026-09-10/a.fits");
        plan.items[1].relative_dest = PathBuf::from("flats/M42/Ha/SESSION_2026-09-10/f.fits");
        plan.items[2].relative_dest = PathBuf::from("bias/G100/b.fits");
        let js = js_runner(&plan, &referenced(None, None));
        assert!(
            js.contains("{ path: \"2026/M42/LIGHT/a.fits\", session: \"2026-09-10\" }"),
            "{js}"
        );
        assert!(
            js.contains("{ path: \"_Calibration/FLAT/f.fits\", session: \"2026-09-10\" }"),
            "{js}"
        );
        assert!(
            js.contains("{ path: \"_Calibration/BIAS/b.fits\" }"),
            "{js}"
        );
        assert!(js.contains("keywords=SESSION"), "{js}");
    }

    /// The person maps a folder they know to what the other machine calls
    /// it, so the given root wins over the frames' common parent, a drive
    /// letter's trailing separator goes, and a frame outside the root keeps
    /// its full path rather than a wrong relative one. Every path is spelled
    /// with forward slashes, which PixInsight takes on every platform.
    #[test]
    fn a_given_root_maps_to_the_remote_name_and_leaves_strays_absolute() {
        let plan = plan_with_sources(&[
            "/mnt/nas/astro/2026/M42/LIGHT/a.fits",
            "/mnt/nas/astro/2026/M42/LIGHT/b.fits",
            "/srv/other/flat.fits",
        ]);
        let js = js_runner(&plan, &referenced(Some("/mnt/nas/astro"), Some("P:\\")));
        assert!(js.contains("var psfSourceRoot = \"P:\";"), "{js}");
        assert!(js.contains("//   /mnt/nas/astro\n"), "{js}");
        assert!(
            js.contains("   { path: \"2026/M42/LIGHT/b.fits\" },\n"),
            "{js}"
        );
        assert!(
            js.contains("var psfOutsideFrames = [\n   { path: \"/srv/other/flat.fits\" },\n];"),
            "{js}"
        );
        // The frame list and roots carry no backslash; only the regex that
        // normalises a typed root does.
        let listing =
            &js[js.find("var psfSourceRoot").unwrap()..js.find("var psfRealRuntime").unwrap()];
        assert!(!listing.contains('\\'), "{listing}");

        let js = js_runner(&plan, &referenced(None, Some("\\\\nas\\astro\\")));
        assert!(js.contains("var psfSourceRoot = \"//nas/astro\";"), "{js}");
    }

    /// The defaults are what a person stacking for keeps wants and would
    /// not see go wrong headless: WBPP's maximum-quality preset spelled out,
    /// and no group quietly switched to Fast Integration.
    #[test]
    fn default_options_spell_out_maximum_quality_and_keep_fast_integration_off() {
        let options = WbppOptions::default();
        assert_eq!(
            options.automation_params(),
            vec![
                "localNormalization=true",
                "localNormalizationPsfType=6",
                "localNormalizationPsfMaxStars=24576",
                "autoIntegrationMode=false",
            ]
        );
        let js = js_runner(&plan_with(&["lights"]), &placed(WbppRun::Full));
        assert!(js.contains("var psfDrizzle = null;"), "{js}");
        assert!(js.contains("var psfFastIntegration = false;"), "{js}");
        assert!(js.contains("\"autoIntegrationMode=false\","), "{js}");
        assert!(
            js.contains("group.enableFastIntegration(psfFastIntegration, true)"),
            "{js}"
        );
    }

    /// Each option lands where WBPP reads it: the parameters it has, or the
    /// per-group hook for the two it lacks.
    #[test]
    fn options_become_parameters_and_per_group_settings() {
        let options = WbppOptions {
            quality: WbppQuality::Good,
            fast_integration: WbppFastIntegration::Auto,
            drizzle: WbppDrizzle::Scale2,
            autocrop: Some(false),
            rejection: Some(WbppRejection::Esd),
        };
        assert_eq!(
            options.automation_params(),
            vec![
                "localNormalization=true",
                "localNormalizationPsfType=2",
                "localNormalizationPsfMaxStars=500",
                "autocrop=false",
                "rejection_4=3",
            ]
        );
        let spec = WbppScriptSpec {
            run: WbppRun::Full,
            options: options.clone(),
            ..Default::default()
        };
        let js = js_runner(&plan_with(&["lights"]), &spec);
        assert!(
            js.contains("var psfDrizzle = { enabled: true, fast: true, scale: 2 };"),
            "{js}"
        );
        assert!(js.contains("var psfFastIntegration = null;"), "{js}");
        assert!(js.contains("group.setDrizzleData(psfDrizzle)"), "{js}");
        assert!(
            js.contains("engine.pipelineManager.buildExecutionPipeline = function"),
            "{js}"
        );
        // The header tells the reader what was chosen.
        let shell = shell_script(&plan_with(&["lights"]), &spec);
        assert!(shell.contains("#   Drizzle: 2x"), "{shell}");
        assert!(shell.contains("#   Quality: good"), "{shell}");
        assert!(
            shell.contains("#   Light rejection: generalized ESD"),
            "{shell}"
        );

        let fast = WbppOptions {
            quality: WbppQuality::Fast,
            fast_integration: WbppFastIntegration::On,
            ..Default::default()
        };
        assert_eq!(fast.automation_params(), vec!["localNormalization=false"]);
        let js = js_runner(
            &plan_with(&["lights"]),
            &WbppScriptSpec {
                options: fast,
                ..Default::default()
            },
        );
        assert!(js.contains("var psfFastIntegration = true;"), "{js}");
    }

    /// The option names travel through JSON as the settings and the API
    /// spell them.
    #[test]
    fn options_round_trip_through_json() {
        let json = r#"{"quality":"good","fast_integration":"auto","drizzle":"2x","autocrop":true,"rejection":"winsorized_sigma"}"#;
        let options: WbppOptions = serde_json::from_str(json).unwrap();
        assert_eq!(options.drizzle, WbppDrizzle::Scale2);
        assert_eq!(options.rejection, Some(WbppRejection::WinsorizedSigma));
        assert_eq!(serde_json::to_string(&options).unwrap(), json);
        let defaults: WbppOptions = serde_json::from_str("{}").unwrap();
        assert_eq!(defaults, WbppOptions::default());
        assert_eq!(
            serde_json::to_string(&defaults).unwrap(),
            r#"{"quality":"maximum","fast_integration":"off","drizzle":"off"}"#
        );
    }

    /// An in-app run knows where WBPP is and says so once, rather than
    /// guessing per platform.
    #[test]
    fn a_known_install_pins_the_include() {
        let spec = WbppScriptSpec {
            bpp_main: Some(PathBuf::from(
                "/home/me/PixInsight/src/scripts/BatchPreprocessing/BPP-Main.js",
            )),
            ..Default::default()
        };
        let js = js_runner(&plan_with(&["lights"]), &spec);
        assert!(
            js.contains(
                "#include \"/home/me/PixInsight/src/scripts/BatchPreprocessing/BPP-Main.js\"\n"
            ),
            "{js}"
        );
        assert!(!js.contains("#ifeq"), "{js}");
    }
}
