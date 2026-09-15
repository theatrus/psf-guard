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

/// The script a referenced export writes beside its runners.
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
        WbppFiles::Referenced { .. } => {
            script.push_str(&format!(
                "\n# {JS_RUNNER} carries the frame list and hands it to WBPP from inside\n\
                 # PixInsight; it also decides whether to stop at the dialog. If PixInsight\n\
                 # sees the frames' folder elsewhere, set PSF_SOURCE_ROOT to it, or edit\n\
                 # psfSourceRoot in the script.\n\
                 PARAMS=\"$HERE/{JS_RUNNER},outputDirectory=$HERE/{OUTPUT_DIRECTORY}\"\n\
                 [ -n \"$PSF_SOURCE_ROOT\" ] && PARAMS=\"$PARAMS,sourceRoot=$PSF_SOURCE_ROOT\"\n\
                 mkdir -p \"$HERE/{OUTPUT_DIRECTORY}\"\n\
                 exec \"$PI_BIN\" -n --automation-mode -r=\"$PARAMS\" --force-exit\n"
            ));
            return script;
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
        WbppFiles::Referenced { .. } => {
            script.push_str(&format!(
                "\r\nREM {JS_RUNNER} carries the frame list and hands it to WBPP from inside\r\n\
                 REM PixInsight; it also decides whether to stop at the dialog. If this\r\n\
                 REM machine sees the frames' folder elsewhere, set PSF_SOURCE_ROOT to it,\r\n\
                 REM or edit psfSourceRoot in the script.\r\n\
                 set \"PARAMS=%HERE%\\{JS_RUNNER},outputDirectory=%HERE%\\{OUTPUT_DIRECTORY}\"\r\n\
                 if defined PSF_SOURCE_ROOT set \"PARAMS=%PARAMS%,sourceRoot=%PSF_SOURCE_ROOT%\"\r\n\
                 if not exist \"%HERE%\\{OUTPUT_DIRECTORY}\" mkdir \"%HERE%\\{OUTPUT_DIRECTORY}\"\r\n\
                 \"%PI_BIN%\" -n --automation-mode -r=\"%PARAMS%\" --force-exit\r\n"
            ));
            return script;
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
            "{comment}Nothing was copied: {JS_RUNNER} names every frame where it already\n\
             {comment}is, and WBPP reads each one's type from its IMAGETYP header. The\n\
             {comment}script also tells WBPP which night's flats each light calibrates\n\
             {comment}with, as a {SESSION_KEYWORD} grouping keyword, since the originals'\n\
             {comment}paths carry none.\n"
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

/// The PixInsight script of a referenced export: the frame list, and WBPP
/// itself, so the list never has to fit a command line.
///
/// WBPP reads its parameters from `Runtime.jsArguments`, which the core
/// makes read-only. The script includes WBPP's entry file inside a function
/// where a Proxy named `Runtime` answers `jsArguments` with the list built
/// here and everything else from the real object, then calls WBPP's entry
/// point the way `WBPP.js` does. WBPP reads a grouping keyword's value out
/// of a frame's path, and the originals' paths carry none, so the script
/// also replaces WBPP's path reader with one that answers `SESSION` for
/// its own frames; WBPP consults it on every regroup, so the value holds.
/// `#engine v8` is what `WBPP.js` declares;
/// without it the core compiles the include with an engine that rejects
/// WBPP's classes. The core's preprocessor also opens a block comment at
/// any `/*`, even inside a `//` comment, and closes it at the next `*/`
/// wherever that is, so the script must not contain one; a glob in a
/// comment once swallowed everything up to WBPP's own entry call.
/// Checked against PixInsight 1.9.4 with WBPP 3.0.1.
///
/// `None` for a placed export, which scans its roots instead.
pub fn js_runner(plan: &ExportPlan, run: WbppRun, files: &WbppFiles) -> Option<String> {
    let WbppFiles::Referenced {
        local_root,
        remote_root,
    } = files
    else {
        return None;
    };
    let (root, referenced) = referenced_files(plan, local_root.as_deref())?;
    let local = slashed(&root);
    let source_root = remote_root
        .as_deref()
        .map(trim_root)
        .map(|root| root.replace('\\', "/"))
        .unwrap_or_else(|| local.clone());
    let entry = |path: &Path, session: Option<&str>| match session {
        Some(session) => format!(
            "{{ path: {}, session: {} }}",
            js_string(&slashed(path)),
            js_string(session)
        ),
        None => format!("{{ path: {} }}", js_string(&slashed(path))),
    };
    let mut below = Vec::new();
    let mut outside = Vec::new();
    for file in referenced {
        match file.place {
            ReferencedPlace::Below(path) => below.push(entry(&path, file.session.as_deref())),
            ReferencedPlace::Outside(path) => outside.push(entry(&path, file.session.as_deref())),
        }
    }
    let load_only = match run {
        WbppRun::LoadOnly => "true",
        WbppRun::Full => "false",
    };
    let mut script = preamble("// ", files);
    script.push_str(&format!(
        "//\n\
         // Run it with run-wbpp.sh or run-wbpp.cmd, which pass the output folder,\n\
         // or from PixInsight's Script > Execute Script File. WBPP must be where\n\
         // the #include lines below expect; edit them for another install.\n\
         \n\
         #engine v8\n\
         #include <pjsr/StdButton.jsh>\n\
         #include <pjsr/StdIcon.jsh>\n\
         \n\
         // The folder the frames are listed below, as PixInsight sees it. On the\n\
         // machine that made this export it is\n\
         //   {local}\n\
         // Edit this, or pass sourceRoot=<folder>, if PixInsight sees it elsewhere.\n\
         var psfSourceRoot = {source_root};\n\
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
    script
        .push_str("];\n\n// Frames outside that folder, by full path.\nvar psfOutsideFrames = [\n");
    for path in &outside {
        script.push_str(&format!("   {path},\n"));
    }
    script.push_str(&format!(
        "];\n\
         \n\
         var psfRealRuntime = Runtime;\n\
         (function () {{\n\
         \x20  var given = {{}};\n\
         \x20  for (var i = 0; i < psfRealRuntime.jsArguments.length; ++i) {{\n\
         \x20     var item = psfRealRuntime.jsArguments[i];\n\
         \x20     var at = item.indexOf(\"=\");\n\
         \x20     if (at > 0) given[item.slice(0, at)] = item.slice(at + 1); else given[item] = true;\n\
         \x20  }}\n\
         \x20  var root = (given.sourceRoot || psfSourceRoot).replace(/\\\\/g, \"/\").replace(/\\/+$/, \"\");\n\
         \x20  var out = given.outputDirectory || psfOutputDirectory || (File.homeDirectory + \"/{OUTPUT_DIRECTORY}\");\n\
         \x20  if (!File.directoryExists(out)) File.createDirectory(out, true);\n\
         \x20  var loadOnly = given.run ? false : given.loadOnly ? true : psfLoadOnly;\n\
         \x20  var paths = [];\n\
         \x20  var sessionByPath = {{}};\n\
         \x20  var withSession = 0;\n\
         \x20  function take(frame, path) {{\n\
         \x20     paths.push(path);\n\
         \x20     if (frame.session) {{ sessionByPath[path] = frame.session; ++withSession; }}\n\
         \x20  }}\n\
         \x20  for (var j = 0; j < psfFrames.length; ++j) take(psfFrames[j], root + \"/\" + psfFrames[j].path);\n\
         \x20  for (var k = 0; k < psfOutsideFrames.length; ++k) take(psfOutsideFrames[k], psfOutsideFrames[k].path);\n\
         \n\
         \x20  // WBPP drops a frame it cannot find without a word, and a wrong root\n\
         \x20  // then opens an empty dialog. Look first, and say what is missing.\n\
         \x20  var missing = paths.filter(function (path) {{ return !File.exists(path); }});\n\
         \x20  console.noteln(\"PSF Guard: \" + paths.length + \" frames below \" + root + \", \" + missing.length + \" not found\");\n\
         \x20  if (missing.length > 0) {{\n\
         \x20     var report = missing.length + \" of \" + paths.length + \" frames were not found. \" +\n\
         \x20        \"The list is relative to psfSourceRoot = \" + root + \"; edit it or pass \" +\n\
         \x20        \"sourceRoot=<folder> so that the first one exists:\\n\" + missing.slice(0, 5).join(\"\\n\");\n\
         \x20     console.criticalln(report);\n\
         \x20     if (missing.length == paths.length) {{\n\
         \x20        if (loadOnly) (new MessageBox(report, \"PSF Guard export\", StdIcon_Error, StdButton_Ok)).execute();\n\
         \x20        throw new Error(\"No frame found below \" + root);\n\
         \x20     }}\n\
         \x20  }}\n\
         \n\
         \x20  var args = [\"automationMode=true\"];\n\
         \x20  if (withSession > 0) args.push(\"groupingKeywordsEnabled=true\", \"keywords={SESSION_KEYWORD}\");\n\
         \x20  for (var m = 0; m < paths.length; ++m) args.push(\"file=\" + paths[m]);\n\
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
         #ifeq __PI_PLATFORM__ MSWINDOWS\n\
         #include \"C:/Program Files/PixInsight/src/scripts/BatchPreprocessing/BPP-Main.js\"\n\
         #else\n\
         #ifeq __PI_PLATFORM__ MACOSX\n\
         #include \"/Applications/PixInsight/src/scripts/BatchPreprocessing/BPP-Main.js\"\n\
         #else\n\
         #include \"/opt/PixInsight/src/scripts/BatchPreprocessing/BPP-Main.js\"\n\
         #endif\n\
         #endif\n\
         \n\
         \x20  // WBPP reads a grouping keyword's value out of a frame's path, and\n\
         \x20  // these paths are the originals', which carry none. So the reader it\n\
         \x20  // consults answers {SESSION_KEYWORD} for our frames from the list above and\n\
         \x20  // leaves every other question to WBPP. It is consulted again whenever\n\
         \x20  // WBPP regroups, so the value stays.\n\
         \x20  if (withSession > 0) {{\n\
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
         \x20  // What WBPP.js itself does after its includes; false is fastMode.\n\
         \x20  CoreApplication.ensureMinimumVersion(1, 9, 4);\n\
         \x20  BPPmain(false, BPP.Version.WBPP_ID, BPP.Version.WBPP_TITLE,\n\
         \x20     BPP.Version.WBPP_SETTINGS_KEY_BASE, BPP.Version.WBPP_VERSION);\n\
         }})();\n"
    ));
    Some(script)
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
            script.contains(&format!("{OUTPUT_DIRECTORY}/logs")),
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

    /// A referenced export puts the frame list in the PixInsight script,
    /// under one root the reader can point elsewhere, and the runners only
    /// launch that script: no frame travels on a command line.
    #[test]
    fn a_referenced_export_lists_the_originals_in_the_script() {
        let plan = plan_with_sources(&[
            "/mnt/nas/astro/2026/M42/LIGHT/a.fits",
            "/mnt/nas/astro/_Calibration/FLAT/f.fits",
        ]);
        let files = WbppFiles::Referenced {
            local_root: None,
            remote_root: None,
        };
        let js = js_runner(&plan, WbppRun::LoadOnly, &files).unwrap();
        assert!(js.starts_with("// "), "{js}");
        assert!(js.contains("#engine v8"), "{js}");
        assert!(
            js.contains("var psfSourceRoot = \"/mnt/nas/astro\";"),
            "{js}"
        );
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
        let full = js_runner(&plan, WbppRun::Full, &files).unwrap();
        assert!(full.contains("var psfLoadOnly = false;"), "{full}");

        let shell = shell_script(&plan, WbppRun::LoadOnly, &files);
        assert!(
            shell.contains(&format!(
                "$HERE/{JS_RUNNER},outputDirectory=$HERE/{OUTPUT_DIRECTORY}"
            )),
            "{shell}"
        );
        assert!(!shell.contains("dir="), "{shell}");
        assert!(!shell.contains("file="), "{shell}");
        assert!(!shell.contains("keywords="), "{shell}");
        let batch = batch_script(&plan, WbppRun::LoadOnly, &files);
        assert!(
            batch.contains(&format!(
                "%HERE%\\{JS_RUNNER},outputDirectory=%HERE%\\{OUTPUT_DIRECTORY}"
            )),
            "{batch}"
        );
        assert!(!batch.contains("file="), "{batch}");

        // A placed export scans its roots and writes no script of its own.
        assert!(js_runner(&plan, WbppRun::LoadOnly, &WbppFiles::Placed).is_none());
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
        let files = WbppFiles::Referenced {
            local_root: None,
            remote_root: None,
        };
        let js = js_runner(&plan, WbppRun::LoadOnly, &files).unwrap();
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
        let files = WbppFiles::Referenced {
            local_root: Some(PathBuf::from("/mnt/nas/astro")),
            remote_root: Some("P:\\".into()),
        };
        let js = js_runner(&plan, WbppRun::LoadOnly, &files).unwrap();
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

        let share = WbppFiles::Referenced {
            local_root: None,
            remote_root: Some("\\\\nas\\astro\\".into()),
        };
        let js = js_runner(&plan, WbppRun::LoadOnly, &share).unwrap();
        assert!(js.contains("var psfSourceRoot = \"//nas/astro\";"), "{js}");
    }
}
