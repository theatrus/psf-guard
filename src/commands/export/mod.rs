//! Export ("take out") graded lights into a stacking-friendly folder tree.
//!
//! Selects non-rejected acquired images (Accepted always; Pending with
//! `include_pending`), resolves each to a file on disk via the basename
//! directory index, and lays them out WBPP-style:
//!
//! ```text
//! <dest>/<target>/LIGHT/<filter>/<basename>.fits
//! ```
//!
//! Rejected frames are never exported. Matching calibration frames from the
//! PSF Guard library join the same plan under
//! `<target>/FLAT/<filter>/SESSION_<night>/`, `<dest>/DARK/<exposure>_G<gain>/`,
//! `<dest>/DARKFLAT/...`, and `<dest>/BIAS/`. The flats are the coherent set a
//! master would build from, per light, so two nights that need different
//! flats get two session folders.
//!
//! Placement is copy (default), hardlink or reflink (same filesystem),
//! symlink (the tree points at the originals), or reference (nothing is
//! placed; the WBPP runner lists the originals where they are). Existing
//! destination files with matching size are skipped, so re-running an export
//! after a new night only adds the new subs.

use crate::db::Database;
use crate::directory_tree::DirectoryTree;
use crate::models::GradingStatus;
use anyhow::{Context, Result};
use rusqlite::Connection;
pub mod wbpp;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// What a frame is for the stacking pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameKind {
    Light,
    Flat,
    Dark,
    DarkFlat,
    Bias,
}

#[derive(Debug, Clone)]
pub struct ExportItem {
    pub image_id: i32,
    pub calibration_frame_id: Option<i64>,
    pub kind: FrameKind,
    pub source: PathBuf,
    /// Path below the destination root (also the archive entry name).
    pub relative_dest: PathBuf,
    pub size_bytes: u64,
}

#[derive(Debug, Default)]
pub struct ExportPlan {
    pub items: Vec<ExportItem>,
    /// (image id, basename) rows whose file was not found in any image dir.
    pub missing: Vec<(i32, String)>,
    /// Rows without a FileName in their metadata.
    pub unresolvable: usize,
}

/// How an export arranges its files under the destination root.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExportLayout {
    /// PSF Guard's own tree, grouped by target first:
    /// `<target>/LIGHT/<filter>/`, `<target>/FLAT/<filter>/`, and `BIAS/`,
    /// `DARK/`, `DARKFLAT/` at the root.
    #[default]
    Standard,
    /// One root per frame type, which is what WeightedBatchPreprocessing
    /// wants: `lights/`, `flats/`, `darks/`, `bias/`.
    ///
    /// Dark flats live under `darks/` beside the lights' darks, because WBPP
    /// has no dark-flat type — it matches a dark to a flat by exposure like
    /// any other. Keeping them apart, as the standard layout does, means
    /// adding one folder to WBPP twice.
    Wbpp,
}

/// The grouping keyword the WBPP layout writes into paths, and hands WBPP
/// as `keywords=SESSION`. WBPP reads a keyword's value out of a path
/// component shaped `SESSION_<value>`, and pairs lights with the flats
/// whose value matches; bias and darks carry no value and serve every
/// session.
pub const SESSION_KEYWORD: &str = "SESSION";

/// The path component that carries a session label to WBPP.
pub fn session_component(label: &str) -> String {
    format!("{SESSION_KEYWORD}_{}", sanitize_component(label))
}

impl ExportLayout {
    /// Where one light frame lands. `session` is the flat session the light
    /// calibrates in; the WBPP layout tags the light's path with it so WBPP
    /// pairs the light with that session's flats.
    fn light_destination(
        self,
        target: &str,
        filter: &str,
        session: Option<&str>,
        basename: &str,
    ) -> PathBuf {
        let (target, filter, basename) = (
            sanitize_component(target),
            sanitize_component(filter),
            sanitize_component(basename),
        );
        match self {
            Self::Standard => PathBuf::from(target)
                .join("LIGHT")
                .join(filter)
                .join(basename),
            Self::Wbpp => {
                let mut path = PathBuf::from("lights").join(target).join(filter);
                if let Some(session) = session {
                    path.push(session_component(session));
                }
                path.join(basename)
            }
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ExportOptions {
    pub include_pending: bool,
    /// Substring filters, matching the rest of the CLI.
    pub project_filter: Option<String>,
    pub target_filter: Option<String>,
    /// Exact-id filters (used by the server endpoint).
    pub project_id: Option<i32>,
    pub target_id: Option<i32>,
    /// Restrict to one filter name (exact, case-insensitive).
    pub filter_name: Option<String>,
    /// How the destination tree is arranged.
    pub layout: ExportLayout,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ExportSummary {
    pub planned: usize,
    pub copied: usize,
    pub linked: usize,
    /// Placed as copy-on-write clones (reflink). A clone is an independent
    /// copy that shares extents until either side is written, so it costs
    /// no time or space on a supporting filesystem yet stays safe to edit.
    #[serde(default)]
    pub reflinked: usize,
    /// Placed as symbolic links to the originals.
    #[serde(default)]
    pub symlinked: usize,
    /// Left where they are and listed by path in the WBPP runner.
    #[serde(default)]
    pub referenced: usize,
    pub skipped_existing: usize,
    pub missing: usize,
    pub errors: usize,
    pub bytes: u64,
}

/// Make a name safe as a single path component (target and filter names are
/// free text in the scheduler DB).
pub fn sanitize_component(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.').to_string();
    if trimmed.is_empty() {
        "unnamed".to_string()
    } else {
        trimmed
    }
}

/// Select frames and resolve them to on-disk files. Pure planning — no
/// filesystem writes — shared by the CLI and the server's archive stream.
pub fn plan_export(
    conn: &Connection,
    image_dirs: &[String],
    options: &ExportOptions,
) -> Result<ExportPlan> {
    let db = Database::new(conn);

    let mut rows = db
        .query_images(
            Some(GradingStatus::Accepted),
            options.project_filter.as_deref(),
            options.target_filter.as_deref(),
            None,
        )
        .context("querying accepted images")?;
    if options.include_pending {
        rows.extend(
            db.query_images(
                Some(GradingStatus::Pending),
                options.project_filter.as_deref(),
                options.target_filter.as_deref(),
                None,
            )
            .context("querying pending images")?,
        );
    }

    let dir_paths: Vec<&Path> = image_dirs.iter().map(Path::new).collect();
    let tree = DirectoryTree::build_multiple(&dir_paths).context("indexing image directories")?;

    let mut plan = ExportPlan::default();
    // Guard against two source files mapping onto one destination name
    // (same basename for a target+filter, e.g. after a manual file copy).
    let mut used_dests: HashMap<PathBuf, usize> = HashMap::new();
    let mut calibration_items = Vec::new();

    for (image, _project_name, target_name) in rows {
        if options.project_id.is_some_and(|id| image.project_id != id)
            || options.target_id.is_some_and(|id| image.target_id != id)
        {
            continue;
        }
        if let Some(wanted) = &options.filter_name
            && !crate::utils::filter_names_match(&image.filter_name, wanted)
        {
            continue;
        }
        let Some(basename) = crate::utils::extract_filename(&image.metadata) else {
            plan.unresolvable += 1;
            continue;
        };
        let Some(source) = tree.find_file_first(&basename).cloned() else {
            plan.missing.push((image.id, basename));
            continue;
        };
        let size_bytes = std::fs::metadata(&source).map(|m| m.len()).unwrap_or(0);
        let light_meta = crate::commands::import::headers::read_frame_meta(&source);
        let mut flat_session = None;
        if light_meta.readable {
            let calibration = crate::calibration::export_destinations(
                conn,
                &light_meta,
                &target_name,
                Some(&tree),
                options.layout,
            )
            .context("matching export calibration frames")?;
            calibration_items.extend(calibration.items);
            flat_session = calibration.flat_session;
        }

        // The basename comes from the row's metadata JSON; sanitize it too so
        // a degenerate FileName (e.g. "..") can never shift the destination
        // or produce a traversal-shaped archive entry name.
        let mut relative_dest = options.layout.light_destination(
            &target_name,
            &image.filter_name,
            flat_session.as_deref(),
            &basename,
        );
        let clashes = used_dests.entry(relative_dest.clone()).or_insert(0);
        *clashes += 1;
        if *clashes > 1 {
            let stem = source
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "frame".into());
            let ext = source
                .extension()
                .map(|e| format!(".{}", e.to_string_lossy()))
                .unwrap_or_default();
            relative_dest = relative_dest.with_file_name(format!("{}.{}{}", stem, *clashes, ext));
        }

        plan.items.push(ExportItem {
            image_id: image.id,
            calibration_frame_id: None,
            kind: FrameKind::Light,
            source,
            relative_dest,
            size_bytes,
        });
    }

    let mut seen = HashSet::new();
    for (kind, frame, mut relative_dest) in calibration_items {
        if !seen.insert((frame.source_path.clone(), relative_dest.clone())) {
            continue;
        }
        if !frame.source_verified || !frame.source_path.is_file() {
            plan.missing
                .push((0, frame.source_path.to_string_lossy().into_owned()));
            continue;
        }
        let clashes = used_dests.entry(relative_dest.clone()).or_insert(0);
        *clashes += 1;
        if *clashes > 1 {
            let stem = frame
                .source_path
                .file_stem()
                .map(|value| value.to_string_lossy().into_owned())
                .unwrap_or_else(|| "calibration".into());
            let extension = frame
                .source_path
                .extension()
                .map(|value| format!(".{}", value.to_string_lossy()))
                .unwrap_or_default();
            relative_dest = relative_dest.with_file_name(format!("{stem}.{}{extension}", *clashes));
        }
        let size_bytes = std::fs::metadata(&frame.source_path)
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        let frame_kind = match kind {
            crate::calibration::CalibrationKind::Bias => FrameKind::Bias,
            crate::calibration::CalibrationKind::Dark => FrameKind::Dark,
            crate::calibration::CalibrationKind::DarkFlat => FrameKind::DarkFlat,
            crate::calibration::CalibrationKind::Flat => FrameKind::Flat,
        };
        plan.items.push(ExportItem {
            image_id: 0,
            calibration_frame_id: Some(frame.id),
            kind: frame_kind,
            source: frame.source_path,
            relative_dest,
            size_bytes,
        });
    }

    // Deterministic order: by destination path.
    plan.items
        .sort_by(|a, b| a.relative_dest.cmp(&b.relative_dest));
    Ok(plan)
}

/// How planned files land at the destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Placement {
    Copy,
    /// Hardlink, falling back to copy across filesystems. The export then
    /// shares inodes with the originals — editing one edits both.
    Hardlink,
    /// Copy-on-write clone (reflink), falling back to copy when the
    /// filesystem cannot clone. Free like a hardlink, independent like a
    /// copy — the right default for a tree another program will consume.
    Reflink,
    /// Symbolic links to the originals, which can sit on another
    /// filesystem, such as a network mount. The tree keeps its shape, so a
    /// stacker still sees the session folders, and costs no space. It never
    /// falls back to a copy: a link that cannot be made is an error, since
    /// the point was not to duplicate the frames. Whatever reads the tree
    /// must see the originals at the same paths.
    Symlink,
    /// Place nothing. The WBPP runner lists every frame where it is, so the
    /// export is the scripts alone. Since the paths are the originals', they
    /// carry no session component and WBPP pools flats by filter.
    Reference,
}

impl Placement {
    /// Whether this placement writes any frame under the destination.
    pub fn places_files(self) -> bool {
        self != Self::Reference
    }
}

/// Place the planned files under `dest_root`. `link` uses hardlinks (falling
/// back to copy when the link fails, e.g. across filesystems).
pub fn execute_plan(
    plan: &ExportPlan,
    dest_root: &Path,
    link: bool,
    dry_run: bool,
) -> ExportSummary {
    let placement = if link {
        Placement::Hardlink
    } else {
        Placement::Copy
    };
    execute_plan_with(plan, dest_root, placement, dry_run, &mut |_, _| {})
}

/// [`execute_plan`] with an explicit placement mode and a progress callback
/// receiving `(placed, total)` after every item.
pub fn execute_plan_with(
    plan: &ExportPlan,
    dest_root: &Path,
    placement: Placement,
    dry_run: bool,
    progress: &mut dyn FnMut(usize, usize),
) -> ExportSummary {
    let mut summary = ExportSummary {
        planned: plan.items.len(),
        missing: plan.missing.len(),
        ..Default::default()
    };

    let total = plan.items.len();
    for (index, item) in plan.items.iter().enumerate() {
        if placement == Placement::Reference {
            // Nothing lands here; the runner names the original.
            summary.referenced += 1;
            summary.bytes += item.size_bytes;
            progress(index + 1, total);
            continue;
        }
        let dest = dest_root.join(&item.relative_dest);
        if let Ok(meta) = std::fs::metadata(&dest)
            && meta.len() == item.size_bytes
        {
            summary.skipped_existing += 1;
            progress(index + 1, total);
            continue;
        }
        if dry_run {
            // Count what a live run would do, by intent; a live run can
            // still fall back to copy where the filesystem cannot link or
            // clone.
            match placement {
                Placement::Copy => summary.copied += 1,
                Placement::Hardlink => summary.linked += 1,
                Placement::Reflink => summary.reflinked += 1,
                Placement::Symlink => summary.symlinked += 1,
                Placement::Reference => summary.referenced += 1,
            }
            summary.bytes += item.size_bytes;
            progress(index + 1, total);
            continue;
        }
        if let Some(parent) = dest.parent()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            eprintln!("⚠️  {}: {}", parent.display(), e);
            summary.errors += 1;
            progress(index + 1, total);
            continue;
        }
        if placement == Placement::Symlink {
            match place_symlink(&item.source, &dest) {
                Ok(()) => {
                    summary.symlinked += 1;
                    summary.bytes += item.size_bytes;
                }
                Err(e) => {
                    eprintln!("⚠️  {} → {}: {}", item.source.display(), dest.display(), e);
                    summary.errors += 1;
                }
            }
            progress(index + 1, total);
            continue;
        }
        if placement == Placement::Hardlink {
            match std::fs::hard_link(&item.source, &dest) {
                Ok(()) => {
                    summary.linked += 1;
                    summary.bytes += item.size_bytes;
                    progress(index + 1, total);
                    continue;
                }
                Err(_) => { /* cross-device or unsupported — fall through to copy */ }
            }
        }
        if placement == Placement::Reflink {
            match reflink_copy::reflink(&item.source, &dest) {
                Ok(()) => {
                    summary.reflinked += 1;
                    summary.bytes += item.size_bytes;
                    progress(index + 1, total);
                    continue;
                }
                Err(_) => { /* filesystem cannot clone — fall through to copy */ }
            }
        }
        match std::fs::copy(&item.source, &dest) {
            Ok(bytes) => {
                summary.copied += 1;
                summary.bytes += bytes;
            }
            Err(e) => {
                eprintln!("⚠️  {} → {}: {}", item.source.display(), dest.display(), e);
                summary.errors += 1;
            }
        }
        progress(index + 1, total);
    }
    summary
}

/// Link `dest` to `source` as a symbolic link to its absolute path, so the
/// link resolves from anywhere the tree is opened. A stale link left by an
/// earlier export (its target moved, so the size check above could not read
/// it) is replaced rather than reported as already there.
fn place_symlink(source: &Path, dest: &Path) -> std::io::Result<()> {
    let target = std::fs::canonicalize(source)?;
    if std::fs::symlink_metadata(dest).is_ok_and(|meta| meta.file_type().is_symlink()) {
        std::fs::remove_file(dest)?;
    }
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&target, dest)
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_file(&target, dest)
    }
}

/// Write the WBPP runner scripts beside a finished export.
///
/// Both platforms' scripts are written, because an export is often zipped and
/// opened on the machine PixInsight runs on rather than the one that made it.
pub fn write_wbpp_scripts(
    plan: &ExportPlan,
    dest_root: &Path,
    run: wbpp::WbppRun,
    files: &wbpp::WbppFiles,
) -> Result<Vec<PathBuf>> {
    if let Some(reason) = wbpp::unusable_destination(dest_root) {
        anyhow::bail!(reason);
    }
    std::fs::create_dir_all(dest_root)
        .with_context(|| format!("creating {}", dest_root.display()))?;
    let mut written = Vec::new();
    let mut scripts = vec![
        ("run-wbpp.sh", wbpp::shell_script(plan, run, files)),
        ("run-wbpp.cmd", wbpp::batch_script(plan, run, files)),
    ];
    if let Some(body) = wbpp::js_runner(plan, run, files) {
        scripts.push((wbpp::JS_RUNNER, body));
    }
    for (name, body) in scripts {
        let path = dest_root.join(name);
        std::fs::write(&path, body).with_context(|| format!("writing {}", path.display()))?;
        #[cfg(unix)]
        if name.ends_with(".sh") {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755));
        }
        written.push(path);
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ts_schema;
    use std::io::Write;

    fn fits_card(output: &mut Vec<u8>, value: &str) {
        let mut card = value.as_bytes().to_vec();
        card.resize(80, b' ');
        output.extend(card);
    }

    fn write_test_fits(path: &Path, kind: &str) {
        write_test_fits_with(path, kind, &[]);
    }

    /// A tiny frame whose header carries `extra` cards after the fixed set,
    /// for a capture time or a filter the test needs.
    fn write_test_fits_with(path: &Path, kind: &str, extra: &[&str]) {
        let mut header = Vec::new();
        fits_card(&mut header, "SIMPLE  =                    T");
        fits_card(&mut header, "BITPIX  =                   16");
        fits_card(&mut header, "NAXIS   =                    2");
        fits_card(&mut header, "NAXIS1  =                   10");
        fits_card(&mut header, "NAXIS2  =                   10");
        fits_card(&mut header, &format!("IMAGETYP= '{kind}'"));
        fits_card(&mut header, "FILTER  = 'Ha'");
        fits_card(&mut header, "EXPTIME =                300.0");
        fits_card(&mut header, "GAIN    =                  100");
        fits_card(&mut header, "OFFSET  =                   30");
        fits_card(&mut header, "XBINNING=                    1");
        fits_card(&mut header, "YBINNING=                    1");
        fits_card(&mut header, "INSTRUME= 'TestCam'");
        for card in extra {
            fits_card(&mut header, card);
        }
        fits_card(&mut header, "END");
        header.resize(header.len().div_ceil(2880) * 2880, b' ');
        let mut file = std::fs::File::create(path).unwrap();
        file.write_all(&header).unwrap();
        file.write_all(&[0_u8; 2880]).unwrap();
    }

    /// Fresh v23 DB with one project/target and three graded images whose
    /// FileName points into `dir`.
    fn seed(dir: &Path) -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        ts_schema::apply_schema(&conn).unwrap();
        conn.execute_batch(
            "INSERT INTO project (Id, profileId, name) VALUES (1, 'p', 'Proj');
             INSERT INTO target (Id, name, active, ra, dec, epochcode, projectid)
             VALUES (1, 'M42/Trapezium', 1, 5.5, -5.4, 2, 1);",
        )
        .unwrap();
        for (id, name, status) in [
            (1, "acc_Ha_0001.fits", 1),
            (2, "rej_Ha_0002.fits", 2),
            (3, "pend_OIII_0003.fits", 0),
        ] {
            let path = dir.join(name);
            std::fs::write(&path, b"fitsdata").unwrap();
            let filter = if name.contains("OIII") { "OIII" } else { "Ha" };
            // Serialize via serde_json so Windows path backslashes are
            // escaped exactly as the plugin's own writer would (a format!()
            // string produced invalid JSON on Windows and emptied the plan).
            let metadata =
                serde_json::json!({ "FileName": path.display().to_string() }).to_string();
            conn.execute(
                "INSERT INTO acquiredimage (Id, projectId, targetId, acquireddate, filtername,
                 gradingStatus, metadata) VALUES (?1, 1, 1, 100, ?2, ?3, ?4)",
                rusqlite::params![id, filter, status, metadata],
            )
            .unwrap();
        }
        conn
    }

    #[test]
    fn plan_excludes_rejects_and_optionally_includes_pending() {
        let dir = tempfile::tempdir().unwrap();
        let conn = seed(dir.path());
        let dirs = vec![dir.path().to_string_lossy().into_owned()];

        let plan = plan_export(&conn, &dirs, &ExportOptions::default()).unwrap();
        assert_eq!(plan.items.len(), 1, "accepted only by default");
        assert_eq!(plan.items[0].image_id, 1);

        let plan = plan_export(
            &conn,
            &dirs,
            &ExportOptions {
                include_pending: true,
                ..Default::default()
            },
        )
        .unwrap();
        let ids: Vec<i32> = plan.items.iter().map(|i| i.image_id).collect();
        assert!(ids.contains(&1) && ids.contains(&3) && !ids.contains(&2));
    }

    #[test]
    fn the_standard_layout_groups_by_target_and_sanitizes_names() {
        let dir = tempfile::tempdir().unwrap();
        let conn = seed(dir.path());
        let dirs = vec![dir.path().to_string_lossy().into_owned()];
        let plan = plan_export(&conn, &dirs, &ExportOptions::default()).unwrap();
        // Target "M42/Trapezium" must not create nested directories.
        assert_eq!(
            plan.items[0].relative_dest,
            PathBuf::from("M42_Trapezium/LIGHT/Ha/acc_Ha_0001.fits")
        );
    }

    /// WBPP wants one root per frame type, and has no dark-flat type at all
    /// — a dark flat is a dark it pairs to a flat by exposure. Keeping them in
    /// their own folder, as the standard layout does, means adding one folder
    /// to WBPP twice.
    #[test]
    fn the_wbpp_layout_gives_each_frame_type_one_root() {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = seed(dir.path());
        let light_path = dir.path().join("acc_Ha_0001.fits");
        write_test_fits(&light_path, "LIGHT");

        let mut calibration = Vec::new();
        for (name, kind) in [
            ("bias-0.fits", "BIAS"),
            ("dark-0.fits", "DARK"),
            ("flat-0.fits", "FLAT"),
        ] {
            let path = dir.path().join(name);
            write_test_fits(&path, kind);
            calibration.push(crate::commands::import::headers::read_frame_meta(&path));
        }
        {
            let tx = conn.transaction().unwrap();
            crate::calibration::import_calibration_frames(&tx, &calibration, Some("p")).unwrap();
            tx.commit().unwrap();
        }

        let dirs = vec![dir.path().to_string_lossy().into_owned()];
        let options = ExportOptions {
            layout: ExportLayout::Wbpp,
            ..ExportOptions::default()
        };
        let plan = plan_export(&conn, &dirs, &options).unwrap();
        let destinations: Vec<String> = plan
            .items
            .iter()
            .map(|item| item.relative_dest.to_string_lossy().replace('\\', "/"))
            .collect();

        // The light has a flat, so it and the flat share a session folder;
        // a flat without a capture time makes an undated session.
        assert!(
            destinations
                .iter()
                .any(|path| path == "lights/M42_Trapezium/Ha/SESSION_undated/acc_Ha_0001.fits"),
            "{destinations:?}"
        );
        assert!(
            destinations
                .iter()
                .any(|path| path == "flats/M42_Trapezium/Ha/SESSION_undated/flat-0.fits"),
            "{destinations:?}"
        );
        assert!(
            destinations.iter().any(|path| path.starts_with("bias/")),
            "{destinations:?}"
        );
        // Nothing may land in the standard layout's roots.
        for path in &destinations {
            assert!(
                !path.starts_with("DARKFLAT/") && !path.contains("/LIGHT/"),
                "the standard tree leaked into a WBPP export: {path}"
            );
        }
    }

    /// Two nights that need different flats get two session folders, and
    /// each light's path names the session of the flats it calibrates with.
    /// Pooling them would have WBPP integrate one master flat for both.
    #[test]
    fn each_night_keeps_its_own_flats_in_a_session_folder() {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = seed(dir.path());
        // Lights on two nights, both accepted.
        write_test_fits_with(
            &dir.path().join("acc_Ha_0001.fits"),
            "LIGHT",
            &["DATE-OBS= '2026-09-11T03:00:00'"],
        );
        let second = dir.path().join("acc_Ha_0009.fits");
        write_test_fits_with(&second, "LIGHT", &["DATE-OBS= '2026-09-13T03:00:00'"]);
        conn.execute(
            "INSERT INTO acquiredimage (Id, projectId, targetId, acquireddate, filtername,
             gradingStatus, metadata) VALUES (9, 1, 1, 200, 'Ha', 1, ?1)",
            [serde_json::json!({ "FileName": second.display().to_string() }).to_string()],
        )
        .unwrap();
        // Two flat sets: one the evening before the first light, one the
        // morning after the second.
        let mut calibration = Vec::new();
        for (name, when) in [
            ("flat-a1.fits", "2026-09-10T23:45:00"),
            ("flat-a2.fits", "2026-09-10T23:45:30"),
            ("flat-b1.fits", "2026-09-13T11:52:00"),
            ("flat-b2.fits", "2026-09-13T11:52:30"),
        ] {
            let path = dir.path().join(name);
            write_test_fits_with(&path, "FLAT", &[&format!("DATE-OBS= '{when}'")]);
            calibration.push(crate::commands::import::headers::read_frame_meta(&path));
        }
        {
            let tx = conn.transaction().unwrap();
            crate::calibration::import_calibration_frames(&tx, &calibration, Some("p")).unwrap();
            tx.commit().unwrap();
        }

        let dirs = vec![dir.path().to_string_lossy().into_owned()];
        let options = ExportOptions {
            layout: ExportLayout::Wbpp,
            ..ExportOptions::default()
        };
        let plan = plan_export(&conn, &dirs, &options).unwrap();
        let destinations: Vec<String> = plan
            .items
            .iter()
            .map(|item| item.relative_dest.to_string_lossy().replace('\\', "/"))
            .collect();
        for expected in [
            "lights/M42_Trapezium/Ha/SESSION_2026-09-10/acc_Ha_0001.fits",
            "flats/M42_Trapezium/Ha/SESSION_2026-09-10/flat-a1.fits",
            "flats/M42_Trapezium/Ha/SESSION_2026-09-10/flat-a2.fits",
            "lights/M42_Trapezium/Ha/SESSION_2026-09-12/acc_Ha_0009.fits",
            "flats/M42_Trapezium/Ha/SESSION_2026-09-12/flat-b1.fits",
            "flats/M42_Trapezium/Ha/SESSION_2026-09-12/flat-b2.fits",
        ] {
            assert!(
                destinations.contains(&expected.to_string()),
                "{expected} in {destinations:?}"
            );
        }
        // Each flat is exported once, in its own session only.
        assert_eq!(
            plan.items
                .iter()
                .filter(|item| item.kind == FrameKind::Flat)
                .count(),
            4,
            "{destinations:?}"
        );
        // And the runner turns the session keyword on.
        let script = wbpp::shell_script(&plan, wbpp::WbppRun::LoadOnly, &wbpp::WbppFiles::Placed);
        assert!(script.contains("keywords=SESSION"), "{script}");
    }

    /// A symlinked export costs no space and can point across filesystems,
    /// so the tree keeps its shape while the frames stay where they are.
    #[cfg(unix)]
    #[test]
    fn symlink_mode_points_the_tree_at_the_originals() {
        let dir = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        let conn = seed(dir.path());
        let dirs = vec![dir.path().to_string_lossy().into_owned()];
        let plan = plan_export(&conn, &dirs, &ExportOptions::default()).unwrap();

        let summary = execute_plan_with(
            &plan,
            dest.path(),
            Placement::Symlink,
            false,
            &mut |_, _| {},
        );
        assert_eq!(
            (summary.symlinked, summary.copied, summary.errors),
            (1, 0, 0)
        );
        let link = dest.path().join("M42_Trapezium/LIGHT/Ha/acc_Ha_0001.fits");
        assert!(std::fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(std::fs::read(&link).unwrap(), b"fitsdata");

        // A second run finds the link in place and skips it.
        let again = execute_plan_with(
            &plan,
            dest.path(),
            Placement::Symlink,
            false,
            &mut |_, _| {},
        );
        assert_eq!(again.skipped_existing, 1);
    }

    /// A referenced export places nothing; the plan is only what the runner
    /// will name.
    #[test]
    fn reference_mode_places_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        let conn = seed(dir.path());
        let dirs = vec![dir.path().to_string_lossy().into_owned()];
        let plan = plan_export(&conn, &dirs, &ExportOptions::default()).unwrap();
        let summary = execute_plan_with(
            &plan,
            dest.path(),
            Placement::Reference,
            false,
            &mut |_, _| {},
        );
        assert_eq!(
            (summary.referenced, summary.copied, summary.errors),
            (1, 0, 0)
        );
        assert_eq!(std::fs::read_dir(dest.path()).unwrap().count(), 0);
    }

    /// The default is untouched, so an existing export folder keeps its shape.
    #[test]
    fn the_default_layout_is_still_the_standard_one() {
        assert_eq!(ExportOptions::default().layout, ExportLayout::Standard);
    }

    #[test]
    fn execute_copies_then_skips_idempotently() {
        let dir = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        let conn = seed(dir.path());
        let dirs = vec![dir.path().to_string_lossy().into_owned()];
        let plan = plan_export(&conn, &dirs, &ExportOptions::default()).unwrap();

        let s1 = execute_plan(&plan, dest.path(), false, false);
        assert_eq!((s1.copied, s1.errors), (1, 0));
        assert!(dest
            .path()
            .join("M42_Trapezium/LIGHT/Ha/acc_Ha_0001.fits")
            .is_file());

        let s2 = execute_plan(&plan, dest.path(), false, false);
        assert_eq!(s2.skipped_existing, 1);
        assert_eq!(s2.copied, 0);
    }

    #[test]
    fn hardlink_mode_links_same_filesystem() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("out"); // same fs as sources
        let conn = seed(dir.path());
        let dirs = vec![dir.path().to_string_lossy().into_owned()];
        let plan = plan_export(&conn, &dirs, &ExportOptions::default()).unwrap();
        let s = execute_plan(&plan, &dest, true, false);
        assert_eq!((s.linked, s.copied, s.errors), (1, 0, 0));
    }

    #[test]
    fn reflink_mode_places_every_file_and_reports_progress() {
        // Whether the filesystem clones or the fallback copies, every
        // planned file must land and the summary must account for each.
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("out"); // same fs, so a clone is possible
        let conn = seed(dir.path());
        let dirs = vec![dir.path().to_string_lossy().into_owned()];
        let plan = plan_export(&conn, &dirs, &ExportOptions::default()).unwrap();

        let mut updates = Vec::new();
        let summary = execute_plan_with(
            &plan,
            &dest,
            Placement::Reflink,
            false,
            &mut |placed, total| {
                updates.push((placed, total));
            },
        );
        assert_eq!(summary.errors, 0);
        assert_eq!(summary.reflinked + summary.copied, 1);
        assert!(dest
            .join("M42_Trapezium/LIGHT/Ha/acc_Ha_0001.fits")
            .is_file());
        assert_eq!(updates.last(), Some(&(1, 1)));
    }

    #[test]
    fn plan_adds_matching_calibration_frames() {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = seed(dir.path());
        let light_path = dir.path().join("acc_Ha_0001.fits");
        write_test_fits(&light_path, "LIGHT");
        let mut calibration = Vec::new();
        for index in 0..2 {
            let path = dir.path().join(format!("bias-{index}.fits"));
            write_test_fits(&path, "BIAS");
            calibration.push(crate::commands::import::headers::read_frame_meta(&path));
        }
        {
            let tx = conn.transaction().unwrap();
            crate::calibration::import_calibration_frames(&tx, &calibration, Some("p")).unwrap();
            tx.commit().unwrap();
        }

        let dirs = vec![dir.path().to_string_lossy().into_owned()];
        let plan = plan_export(&conn, &dirs, &ExportOptions::default()).unwrap();
        assert_eq!(
            plan.items
                .iter()
                .filter(|item| item.kind == FrameKind::Light)
                .count(),
            1
        );
        assert_eq!(
            plan.items
                .iter()
                .filter(|item| item.kind == FrameKind::Bias)
                .count(),
            2
        );
        assert!(plan
            .items
            .iter()
            .filter(|item| item.kind == FrameKind::Bias)
            .all(|item| item.relative_dest.starts_with("BIAS")));
    }

    #[test]
    fn missing_files_are_reported_not_fatal() {
        let dir = tempfile::tempdir().unwrap();
        let conn = seed(dir.path());
        std::fs::remove_file(dir.path().join("acc_Ha_0001.fits")).unwrap();
        let dirs = vec![dir.path().to_string_lossy().into_owned()];
        let plan = plan_export(&conn, &dirs, &ExportOptions::default()).unwrap();
        assert_eq!(plan.items.len(), 0);
        assert_eq!(plan.missing.len(), 1);
    }

    #[test]
    fn filter_name_restricts_selection() {
        let dir = tempfile::tempdir().unwrap();
        let conn = seed(dir.path());
        let dirs = vec![dir.path().to_string_lossy().into_owned()];
        let plan = plan_export(
            &conn,
            &dirs,
            &ExportOptions {
                include_pending: true,
                filter_name: Some("oiii".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(plan.items.len(), 1);
        assert_eq!(plan.items[0].image_id, 3);
    }
}
