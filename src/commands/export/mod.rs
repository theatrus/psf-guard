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
    /// Calibration frames the library matched but keeps out: graded poor,
    /// kept out by hand, or marked for stray light.
    pub kept_out: std::collections::BTreeSet<PathBuf>,
    /// Lights left out because they cannot be calibrated properly, as
    /// stacks leave them out ([`ExportOptions::exclude_uncalibrated`]):
    /// image id, file name and why.
    pub uncalibrated: Vec<(i32, String, String)>,
    /// The calibration settings that chose the frames and differ from PSF
    /// Guard's defaults, in words, with who set them: "Use darks: off (for
    /// Askar 120 · ZWO ASI2600MM Pro)". Empty when every light was matched
    /// by the defaults.
    pub calibration_settings: Vec<String>,
    /// The rigs whose settings leave darks out, so their lights export
    /// with none: rig names, or "every rig" when the server's settings do.
    pub without_darks: Vec<String>,
    /// The measured flat changes the exported lights' flats were chosen
    /// around, in words with their windows.
    pub flat_changes: Vec<String>,
    /// Lights whose flats were chosen around a measured change: image id,
    /// file name and how.
    pub flat_choices: Vec<(i32, String, crate::calibration::FlatChoice)>,
}

impl ExportPlan {
    /// Lights a PSF Guard stack would calibrate otherwise than WBPP can
    /// here, or that may not match their flats: flats carried across a
    /// measured change in a stack, and lights flagged for their flats.
    pub fn flat_warnings(&self) -> Vec<&(i32, String, crate::calibration::FlatChoice)> {
        self.flat_choices
            .iter()
            .filter(|(_, _, choice)| choice.flagged || choice.basis == "corrected")
            .collect()
    }

    /// One of [`Self::flat_warnings`] and why, for a summary that counts them.
    pub fn flat_warning_example(&self) -> Option<String> {
        self.flat_warnings()
            .first()
            .map(|(_, name, choice)| format!("{name}: {}", choice.text))
    }

    /// The lights of [`Self::flat_warnings`], one per line with why, for an
    /// export that carries a note beside its frames. `None` when there are
    /// none.
    pub fn flat_note(&self) -> Option<String> {
        let warnings = self.flat_warnings();
        if warnings.is_empty() {
            return None;
        }
        let mut note = format!(
            "{} light(s) of this export may not match their flats, or take flats a PSF Guard \
             stack carries across a measured change, which this export cannot. Changes in \
             force:\n",
            warnings.len()
        );
        for change in &self.flat_changes {
            note.push_str(&format!("  {change}\n"));
        }
        note.push('\n');
        for (id, name, choice) in warnings {
            note.push_str(&format!("{name} (image {id}): {}\n", choice.text));
        }
        Some(note)
    }

    /// One light left out because it cannot be calibrated, and why, for a
    /// summary that counts them: "acc_0001.fits: No matching flat".
    pub fn uncalibrated_example(&self) -> Option<String> {
        self.uncalibrated
            .first()
            .map(|(_, name, reason)| format!("{name}: {reason}"))
    }

    /// The lights left out because they cannot be calibrated, one per line
    /// with why, for an export that carries a note beside its frames.
    /// `None` when none was.
    pub fn uncalibrated_note(&self) -> Option<String> {
        if self.uncalibrated.is_empty() {
            return None;
        }
        let mut note = format!(
            "{} light(s) that can't be calibrated were left out of this export, as stacks \
             leave them out (Settings > Calibration > Stacks and exports > Leave out lights \
             that can't be calibrated):\n\n",
            self.uncalibrated.len()
        );
        for (id, name, reason) in &self.uncalibrated {
            note.push_str(&format!("{name} (image {id}): {reason}\n"));
        }
        Some(note)
    }
}

/// The note a zip export carries when it left lights out.
pub const UNCALIBRATED_NOTE: &str = "LEFT-OUT-UNCALIBRATED.txt";

/// The note a zip export carries when some lights' flats are from across a
/// measured change ([`ExportPlan::flat_note`]).
pub const FLATS_NOTE: &str = "FLATS-ACROSS-A-CHANGE.txt";

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

/// The grouping keyword that keeps two dark masters of one exposure and gain
/// apart, when lights in one export were matched to different darks. WBPP
/// lets a calibration group serve a light only when every keyword the group
/// carries equals the light's (`keywordsMatchCount`, strict direct), and
/// prefers the group matching most keywords; a group without the keyword
/// still serves every light. So tagged darks serve only their lights, and
/// bias, dark-flats and flats, untagged, serve all.
pub const DARKSET_KEYWORD: &str = "DARKSET";

pub fn darkset_component(label: &str) -> String {
    format!("{DARKSET_KEYWORD}_{}", sanitize_component(label))
}

/// Name each distinct dark set of one exposure and gain by its nearest
/// night, with a short digest of its frames when two sets share a night.
/// Only groups holding more than one set get names: one set needs no keyword.
fn dark_set_labels(
    sets: &[crate::calibration::DarkSet],
) -> HashMap<crate::calibration::DarkSet, String> {
    use sha2::{Digest, Sha256};
    let mut by_group: std::collections::BTreeMap<
        &str,
        std::collections::BTreeSet<&crate::calibration::DarkSet>,
    > = Default::default();
    for set in sets {
        by_group.entry(set.group.as_str()).or_default().insert(set);
    }
    let mut labels = HashMap::new();
    for distinct in by_group.values().filter(|distinct| distinct.len() > 1) {
        let mut nights: HashMap<&str, usize> = HashMap::new();
        for set in distinct {
            *nights.entry(set.night.as_str()).or_default() += 1;
        }
        for set in distinct {
            let label = if nights[set.night.as_str()] > 1 {
                let digest = Sha256::digest(set.frame_uuids.join(",").as_bytes());
                let hex: String = digest
                    .iter()
                    .take(3)
                    .map(|byte| format!("{byte:02x}"))
                    .collect();
                format!("{}-{hex}", set.night)
            } else {
                set.night.clone()
            };
            labels.insert((*set).clone(), label);
        }
    }
    labels
}

/// `path` with one more folder just above its file name.
/// Name the flat sets that cannot share their night's session. Lights of
/// one night with different flats need a session each: the night with a
/// short digest of the set's frames. A set carried across a measured change
/// always gets its own, `<night>-across-<key>`, so its lights stand apart;
/// with the digest too when one change carries several sets of the night
/// across, as at two angles. Sets alone on their night keep the night.
fn flat_set_labels<'a>(keys: impl Iterator<Item = &'a FlatSetKey>) -> HashMap<FlatSetKey, String> {
    use sha2::{Digest, Sha256};
    let mut by_night: std::collections::BTreeMap<
        (&str, &str, &str),
        std::collections::BTreeSet<&FlatSetKey>,
    > = Default::default();
    for key in keys {
        by_night
            .entry((key.0.as_str(), key.1.as_str(), key.2.night.as_str()))
            .or_default()
            .insert(key);
    }
    let digest = |set: &crate::calibration::FlatSet| -> String {
        Sha256::digest(set.frame_uuids.join(",").as_bytes())
            .iter()
            .take(3)
            .map(|byte| format!("{byte:02x}"))
            .collect()
    };
    let mut labels = HashMap::new();
    for distinct in by_night.values() {
        let plain = distinct
            .iter()
            .filter(|key| key.2.correction.is_none())
            .count();
        let mut across: HashMap<&str, usize> = HashMap::new();
        for key in distinct {
            if let Some(correction) = &key.2.correction {
                *across.entry(correction.as_str()).or_default() += 1;
            }
        }
        for key in distinct {
            let set = &key.2;
            let label = match &set.correction {
                Some(key) if across[key.as_str()] > 1 => format!(
                    "{}-across-{}-{}",
                    set.night,
                    &key[..key.len().min(6)],
                    digest(set)
                ),
                Some(key) => format!("{}-across-{}", set.night, &key[..key.len().min(6)]),
                None if plain > 1 => format!("{}-{}", set.night, digest(set)),
                None => continue,
            };
            labels.insert((*key).clone(), label);
        }
    }
    labels
}

/// A light's flat set within its target and filter, which an export
/// already keeps apart by folder.
type FlatSetKey = (String, String, crate::calibration::FlatSet);

/// `path` with its `SESSION_<night>` component renamed to `label`.
fn with_session(path: &Path, night: &str, label: &str) -> PathBuf {
    let from = session_component(night);
    let to = session_component(label);
    path.components()
        .map(|component| {
            if component.as_os_str() == from.as_str() {
                std::ffi::OsString::from(&to)
            } else {
                component.as_os_str().to_os_string()
            }
        })
        .collect()
}

fn with_folder_before_file(path: &Path, folder: &str) -> PathBuf {
    match (path.parent(), path.file_name()) {
        (Some(parent), Some(name)) => parent.join(folder).join(name),
        _ => path.to_path_buf(),
    }
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
    /// Leave out lights that cannot be calibrated properly, as stacks do
    /// when **Leave out lights that can't be calibrated** is on. Every
    /// export, in either layout, passes
    /// [`crate::calibration::exclude_uncalibrated_enabled`].
    pub exclude_uncalibrated: bool,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ExportSummary {
    pub planned: usize,
    /// Stopped before every planned file was placed.
    #[serde(default)]
    pub stopped: bool,
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
    /// Lights left out because they cannot be calibrated properly, as
    /// stacks leave them out ([`ExportPlan::uncalibrated`]).
    #[serde(default)]
    pub uncalibrated: usize,
    /// One of them and why: "acc_0001.fits: No matching flat".
    #[serde(default)]
    pub uncalibrated_example: Option<String>,
    /// Lights whose flats are from across a measured change, so WBPP may
    /// not calibrate them as a PSF Guard stack would
    /// ([`ExportPlan::flat_warnings`]).
    #[serde(default)]
    pub flat_warnings: usize,
    /// One of them and why.
    #[serde(default)]
    pub flat_warning_example: Option<String>,
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
    let dir_paths: Vec<&Path> = image_dirs.iter().map(Path::new).collect();
    let tree = DirectoryTree::build_multiple(&dir_paths).context("indexing image directories")?;
    plan_export_in(conn, &tree, options)
}

/// [`plan_export`] against a directory tree already built, such as the
/// server's cached one.
pub fn plan_export_in(
    conn: &Connection,
    tree: &DirectoryTree,
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

    let mut plan = ExportPlan::default();
    // Guard against two source files mapping onto one destination name
    // (same basename for a target+filter, e.g. after a manual file copy).
    let mut used_dests: HashMap<PathBuf, usize> = HashMap::new();
    let mut calibration_items = Vec::new();
    // For the WBPP layout: which planned light, and which planned dark,
    // belongs to which dark set, settled once every light is matched.
    let mut light_dark_sets: Vec<(usize, crate::calibration::DarkSet)> = Vec::new();
    let mut dark_item_sets: Vec<(usize, crate::calibration::DarkSet)> = Vec::new();
    // Likewise which planned light, and which planned flat, takes which
    // flat set, so lights that need different flats get different sessions.
    let mut light_flat_sets: Vec<(usize, FlatSetKey)> = Vec::new();
    let mut flat_item_sets: Vec<(usize, FlatSetKey)> = Vec::new();
    let mut flat_changes = std::collections::BTreeSet::new();
    let nights = crate::calibration::Nights::load(conn);
    // The library read once, and each calibration file checked once.
    let mut matcher = crate::calibration::LightMatcher::load(conn, false)?;
    let mut notes = SettingsNotes::default();

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
        let mut dark_set = None;
        if light_meta.readable {
            let calibration = crate::calibration::export_destinations_with(
                &mut matcher,
                &light_meta,
                &target_name,
                Some(tree),
                options.layout,
                &nights,
                options.exclude_uncalibrated,
            )
            .context("matching export calibration frames")?;
            // A light a stack would leave out stays out of the export too,
            // so WBPP never mixes it with properly calibrated frames.
            if options.exclude_uncalibrated
                && let Some(reason) = &calibration.gap
            {
                plan.uncalibrated
                    .push((image.id, basename.clone(), reason.clone()));
                continue;
            }
            notes.note(&calibration.rules, options.exclude_uncalibrated);
            if let Some(set) = &calibration.dark_set {
                for (offset, (kind, _, _)) in calibration.items.iter().enumerate() {
                    if *kind == crate::calibration::CalibrationKind::Dark {
                        dark_item_sets.push((calibration_items.len() + offset, set.clone()));
                    }
                }
            }
            // Grouping flats by time exports as it always did: every light
            // of a night in that night's session.
            if let Some(set) = &calibration.flat_set
                && calibration.rules.flat_grouping
                    == crate::calibration::rules::FlatGrouping::Measured
            {
                let key = (target_name.clone(), image.filter_name.clone(), set.clone());
                for (offset, (kind, _, _)) in calibration.items.iter().enumerate() {
                    if *kind == crate::calibration::CalibrationKind::Flat {
                        flat_item_sets.push((calibration_items.len() + offset, key.clone()));
                    }
                }
                light_flat_sets.push((plan.items.len(), key));
            }
            if let Some(choice) = calibration.flat_choice {
                flat_changes.insert(choice.change.clone());
                plan.flat_choices.push((image.id, basename.clone(), choice));
            }
            calibration_items.extend(calibration.items);
            plan.kept_out.extend(calibration.kept_out);
            flat_session = calibration.flat_session;
            dark_set = calibration.dark_set;
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

        if let Some(set) = dark_set {
            light_dark_sets.push((plan.items.len(), set));
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

    // Lights matched to different darks of one exposure and gain: give each
    // set its own DARKSET folder, and each light its set's, so WBPP builds
    // the same masters a stack would instead of averaging every night's.
    if options.layout == ExportLayout::Wbpp {
        let sets: Vec<_> = light_dark_sets.iter().map(|(_, set)| set.clone()).collect();
        let labels = dark_set_labels(&sets);
        if !labels.is_empty() {
            for (index, set) in &light_dark_sets {
                if let Some(label) = labels.get(set) {
                    let item = &mut plan.items[*index];
                    item.relative_dest =
                        with_folder_before_file(&item.relative_dest, &darkset_component(label));
                }
            }
            for (index, set) in &dark_item_sets {
                if let Some(label) = labels.get(set) {
                    let dest = &mut calibration_items[*index].2;
                    *dest = with_folder_before_file(dest, &darkset_component(label));
                }
            }
        }
    }

    // Lights of one night that take different flats, or flats carried
    // across a change: a session of their own each, so WBPP integrates the
    // flats a stack would for each light rather than one master for all.
    let labels = flat_set_labels(light_flat_sets.iter().map(|(_, key)| key));
    if !labels.is_empty() {
        for (index, key) in &light_flat_sets {
            if let Some(label) = labels.get(key) {
                let item = &mut plan.items[*index];
                item.relative_dest = with_session(&item.relative_dest, &key.2.night, label);
            }
        }
        for (index, key) in &flat_item_sets {
            if let Some(label) = labels.get(key) {
                let dest = &mut calibration_items[*index].2;
                *dest = with_session(dest, &key.2.night, label);
            }
        }
    }
    plan.flat_changes = flat_changes.into_iter().collect();

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
    (plan.calibration_settings, plan.without_darks) = notes.finish();
    Ok(plan)
}

/// The calibration settings an export's lights were matched by, gathered
/// once per distinct set of rules.
#[derive(Default)]
struct SettingsNotes {
    seen: HashSet<*const crate::calibration::Rules>,
    /// Global settings first, then each rig's, each in the panel's order.
    lines: std::collections::BTreeSet<(u8, String, usize, String)>,
    without_darks: std::collections::BTreeSet<String>,
    /// Kept alive so the pointers in `seen` stay unique.
    held: Vec<std::sync::Arc<crate::calibration::Rules>>,
}

impl SettingsNotes {
    /// Note what these rules set that changes which frames an export takes:
    /// matching and grading, and the checks that make a light one to leave
    /// out when the export leaves such lights out.
    fn note(&mut self, rules: &std::sync::Arc<crate::calibration::Rules>, gaps_matter: bool) {
        if !self.seen.insert(std::sync::Arc::as_ptr(rules)) {
            return;
        }
        self.held.push(std::sync::Arc::clone(rules));
        use crate::calibration::rules::{field, fields, Effect};
        use crate::calibration::RuleSource;
        let rig = rules
            .rig
            .as_ref()
            .map(|rig| rig.name.clone())
            .unwrap_or_default();
        for note in rules.notes() {
            let order = fields()
                .iter()
                .position(|spec| spec.key == note.key)
                .unwrap_or(usize::MAX);
            let effect = field(note.key).map(|spec| spec.effect);
            let gap_check = matches!(
                note.key,
                "flat_max_age_days"
                    | "check_no_flat"
                    | "check_no_bias_or_dark"
                    | "check_stale_flats"
            );
            if effect == Some(Effect::Flags) && !(gaps_matter && gap_check) {
                continue;
            }
            let line = match note.source {
                RuleSource::Rig => (
                    1,
                    rig.clone(),
                    order,
                    format!("{}: {} (for {rig})", note.label, note.text),
                ),
                _ => (
                    0,
                    String::new(),
                    order,
                    format!("{}: {} (global)", note.label, note.text),
                ),
            };
            self.lines.insert(line);
        }
        if !rules.use_darks {
            self.without_darks.insert(match rules.source("use_darks") {
                RuleSource::Rig => rig,
                _ => "every rig".to_string(),
            });
        }
    }

    fn finish(self) -> (Vec<String>, Vec<String>) {
        (
            self.lines.into_iter().map(|(_, _, _, line)| line).collect(),
            self.without_darks.into_iter().collect(),
        )
    }
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
    /// export is the scripts alone. The originals' paths carry no session
    /// component, so the runner hands WBPP each frame's session itself.
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
    execute_plan_until(plan, dest_root, placement, dry_run, progress, &|| true)
}

/// [`execute_plan_with`] that stops before the next item once `keep_going`
/// turns false, with [`ExportSummary::stopped`] set. The files placed stay,
/// and a later export skips them.
pub fn execute_plan_until(
    plan: &ExportPlan,
    dest_root: &Path,
    placement: Placement,
    dry_run: bool,
    progress: &mut dyn FnMut(usize, usize),
    keep_going: &dyn Fn() -> bool,
) -> ExportSummary {
    let mut summary = ExportSummary {
        planned: plan.items.len(),
        missing: plan.missing.len(),
        uncalibrated: plan.uncalibrated.len(),
        uncalibrated_example: plan.uncalibrated_example(),
        flat_warnings: plan.flat_warnings().len(),
        flat_warning_example: plan.flat_warning_example(),
        ..Default::default()
    };

    let total = plan.items.len();
    for (index, item) in plan.items.iter().enumerate() {
        if !keep_going() {
            summary.stopped = true;
            break;
        }
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
    spec: &wbpp::WbppScriptSpec,
) -> Result<Vec<PathBuf>> {
    if let Some(reason) = wbpp::unusable_destination(dest_root) {
        anyhow::bail!(reason);
    }
    std::fs::create_dir_all(dest_root)
        .with_context(|| format!("creating {}", dest_root.display()))?;
    let mut written = Vec::new();
    // Frames an earlier export left under the scanned roots, which WBPP
    // would take along with this one's: the scripts warn about them.
    let mut placed;
    let spec = if matches!(spec.files, wbpp::WbppFiles::Placed) {
        placed = spec.clone();
        placed.stale_frames = wbpp::stale_frames(plan, dest_root);
        &placed
    } else {
        spec
    };
    let scripts = [
        ("run-wbpp.sh", wbpp::shell_script(plan, spec)),
        ("run-wbpp.cmd", wbpp::batch_script(plan, spec)),
        (wbpp::JS_RUNNER, wbpp::js_runner(plan, spec)),
    ];
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

    #[test]
    fn two_sets_one_change_carries_across_in_a_night_get_a_session_each() {
        let set = |uuids: &[&str], correction: Option<&str>| {
            (
                "M 31".to_string(),
                "R".to_string(),
                crate::calibration::FlatSet {
                    night: "2026-09-18".into(),
                    frame_uuids: uuids.iter().map(|uuid| uuid.to_string()).collect(),
                    correction: correction.map(str::to_string),
                },
            )
        };
        // The same change carries the flats at 3° and at 114° across.
        let at_3 = set(&["a", "b"], Some("0123456789abcdef"));
        let at_114 = set(&["c", "d"], Some("0123456789abcdef"));
        let labels = flat_set_labels([&at_3, &at_114].into_iter());
        assert_ne!(labels[&at_3], labels[&at_114]);
        assert!(
            labels[&at_3].starts_with("2026-09-18-across-012345-"),
            "{labels:?}"
        );
        // Alone, a carried set keeps the shorter name.
        let labels = flat_set_labels([&at_3].into_iter());
        assert_eq!(labels[&at_3], "2026-09-18-across-012345");
    }

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

    /// Two lights, each with a complete night of darks of its own, in one
    /// export: the darks go to two dark-set folders and each light names
    /// its set, so WBPP builds the two masters a stack would. Lights sharing
    /// one set keep the plain layout.
    fn dark_set_export(second_light: &str) -> Vec<String> {
        let dir = tempfile::tempdir().unwrap();
        let mut conn = seed(dir.path());
        write_test_fits_with(
            &dir.path().join("acc_Ha_0001.fits"),
            "LIGHT",
            &["DATE-OBS= '2026-01-10T03:00:00'"],
        );
        let second = dir.path().join("acc_Ha_0009.fits");
        write_test_fits_with(&second, "LIGHT", &[&format!("DATE-OBS= '{second_light}'")]);
        conn.execute(
            "INSERT INTO acquiredimage (Id, projectId, targetId, acquireddate, filtername,
             gradingStatus, metadata) VALUES (9, 1, 1, 200, 'Ha', 1, ?1)",
            [serde_json::json!({ "FileName": second.display().to_string() }).to_string()],
        )
        .unwrap();
        let mut calibration = Vec::new();
        for (night, day) in [("jan", "2026-01-09"), ("mar", "2026-03-09")] {
            for index in 0..10 {
                let path = dir.path().join(format!("dark-{night}-{index}.fits"));
                write_test_fits_with(
                    &path,
                    "DARK",
                    &[&format!("DATE-OBS= '{day}T20:{index:02}:00'")],
                );
                calibration.push(crate::commands::import::headers::read_frame_meta(&path));
            }
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
        plan_export(&conn, &dirs, &options)
            .unwrap()
            .items
            .iter()
            .map(|item| item.relative_dest.to_string_lossy().replace('\\', "/"))
            .collect()
    }

    #[test]
    fn dark_sets_are_named_by_night_with_a_digest_only_when_nights_collide() {
        let set = |group: &str, night: &str, uuids: &[&str]| crate::calibration::DarkSet {
            group: group.into(),
            night: night.into(),
            frame_uuids: uuids.iter().map(|uuid| uuid.to_string()).collect(),
        };
        let a = set("300s_G100", "2026-01-09", &["a1", "a2"]);
        let b = set("300s_G100", "2026-03-09", &["b1", "b2"]);
        let c = set("300s_G100", "2026-03-09", &["b1", "c2"]);
        let lone = set("60s_G100", "2026-01-09", &["x"]);
        let labels = dark_set_labels(&[a.clone(), a.clone(), b.clone(), lone.clone()]);
        assert_eq!(labels[&a], "2026-01-09");
        assert_eq!(labels[&b], "2026-03-09");
        // One set in its group needs no keyword.
        assert!(!labels.contains_key(&lone));
        let labels = dark_set_labels(&[a, b.clone(), c.clone()]);
        assert!(labels[&b].starts_with("2026-03-09-") && labels[&c].starts_with("2026-03-09-"));
        assert_ne!(labels[&b], labels[&c]);
    }

    #[test]
    fn lights_matched_to_different_darks_get_a_dark_set_folder_each() {
        let destinations = dark_set_export("2026-03-10T03:00:00");
        let set_of = |path: &str| {
            path.split('/')
                .find_map(|part| part.strip_prefix("DARKSET_"))
                .map(str::to_string)
        };
        let darks: Vec<&String> = destinations
            .iter()
            .filter(|path| path.starts_with("darks/"))
            .collect();
        assert_eq!(darks.len(), 20, "{destinations:?}");
        for dark in &darks {
            let set = set_of(dark).unwrap_or_else(|| panic!("untagged dark {dark}"));
            let month = if dark.contains("dark-jan") {
                "2026-01"
            } else {
                "2026-03"
            };
            assert!(set.starts_with(month), "{dark}");
            assert!(dark.starts_with("darks/300s_G100/DARKSET_"), "{dark}");
        }
        let light = |name: &str| {
            destinations
                .iter()
                .find(|path| path.ends_with(name))
                .unwrap()
        };
        assert!(
            set_of(light("acc_Ha_0001.fits"))
                .unwrap()
                .starts_with("2026-01"),
            "{destinations:?}"
        );
        assert!(
            set_of(light("acc_Ha_0009.fits"))
                .unwrap()
                .starts_with("2026-03"),
            "{destinations:?}"
        );
    }

    #[test]
    fn lights_sharing_one_dark_set_keep_the_plain_layout() {
        // Both lights in January: both take January's complete night.
        let destinations = dark_set_export("2026-01-11T03:00:00");
        assert!(
            destinations.iter().all(|path| !path.contains("DARKSET_")),
            "{destinations:?}"
        );
        assert_eq!(
            destinations
                .iter()
                .filter(|path| path.starts_with("darks/300s_G100/dark-jan"))
                .count(),
            10,
            "{destinations:?}"
        );
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
        let script = wbpp::js_runner(&plan, &wbpp::WbppScriptSpec::default());
        assert!(
            script.contains("var psfSessionGrouping = true;"),
            "{script}"
        );
    }

    #[test]
    fn a_stopped_export_places_nothing_more_and_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        let conn = seed(dir.path());
        let dirs = vec![dir.path().to_string_lossy().into_owned()];
        let plan = plan_export(&conn, &dirs, &ExportOptions::default()).unwrap();
        assert!(!plan.items.is_empty());

        let summary = execute_plan_until(
            &plan,
            dest.path(),
            Placement::Copy,
            false,
            &mut |_, _| {},
            &|| false,
        );
        assert!(summary.stopped);
        assert_eq!((summary.copied, summary.errors), (0, 0));
        assert_eq!(std::fs::read_dir(dest.path()).unwrap().count(), 0);
        let summary = execute_plan_with(&plan, dest.path(), Placement::Copy, false, &mut |_, _| {});
        assert!(!summary.stopped && summary.copied > 0);
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

    /// An export matches each light by its own rig's calibration settings,
    /// the way a stack does, so WBPP calibrates with the frames a stack
    /// would have taken.
    mod settings {
        use super::*;
        use crate::calibration::tests::{RulesInForce, POLICY_LOCK};
        use serde_json::json;

        /// `seed`'s catalog and a calibration library, as tiny FITS files in
        /// a folder of their own.
        struct Library {
            dir: tempfile::TempDir,
            conn: Connection,
        }

        impl Library {
            /// The catalog's one accepted light carries `cards`.
            fn new(cards: &[&str]) -> Self {
                let dir = tempfile::tempdir().unwrap();
                // On disk, as a server's catalog is, so flat grouping can
                // keep its measurements beside it.
                let catalog = dir.path().join("catalog.sqlite");
                seed(dir.path())
                    .execute("VACUUM INTO ?1", [catalog.to_string_lossy()])
                    .unwrap();
                let conn = Connection::open(&catalog).unwrap();
                write_test_fits_with(&dir.path().join("acc_Ha_0001.fits"), "LIGHT", cards);
                Self { dir, conn }
            }

            /// Another accepted light.
            fn light(&mut self, id: i32, name: &str, cards: &[&str]) {
                let path = self.dir.path().join(name);
                write_test_fits_with(&path, "LIGHT", cards);
                self.conn
                    .execute(
                        "INSERT INTO acquiredimage (Id, projectId, targetId, acquireddate,
                         filtername, gradingStatus, metadata) VALUES (?1, 1, 1, 200, 'Ha', 1, ?2)",
                        rusqlite::params![
                            id,
                            serde_json::json!({ "FileName": path.display().to_string() })
                                .to_string()
                        ],
                    )
                    .unwrap();
            }

            /// `count` frames of `kind`, `<stem>-<n>.fits`, shot a minute
            /// apart from `hour` ("2026-09-10T20"), each with `cards`.
            fn frames(&mut self, stem: &str, kind: &str, hour: &str, count: usize, cards: &[&str]) {
                let mut metas = Vec::new();
                for index in 0..count {
                    let path = self.dir.path().join(format!("{stem}-{index}.fits"));
                    let date = format!("DATE-OBS= '{hour}:{index:02}:00'");
                    let mut all = vec![date.as_str()];
                    all.extend_from_slice(cards);
                    write_test_fits_with(&path, kind, &all);
                    metas.push(crate::commands::import::headers::read_frame_meta(&path));
                }
                let tx = self.conn.transaction().unwrap();
                crate::calibration::import_calibration_frames(&tx, &metas, Some("p")).unwrap();
                tx.commit().unwrap();
            }

            /// The rig the lights were shot with, as the settings name it.
            fn rig(&self) -> String {
                crate::calibration::rules::light_rig_uuid(
                    &crate::commands::import::headers::read_frame_meta(
                        &self.dir.path().join("acc_Ha_0001.fits"),
                    ),
                )
            }

            fn plan(&self, exclude_uncalibrated: bool) -> ExportPlan {
                self.plan_in(ExportLayout::Wbpp, exclude_uncalibrated)
            }

            fn plan_in(&self, layout: ExportLayout, exclude_uncalibrated: bool) -> ExportPlan {
                plan_export(
                    &self.conn,
                    &[self.dir.path().to_string_lossy().into_owned()],
                    &ExportOptions {
                        layout,
                        exclude_uncalibrated,
                        ..Default::default()
                    },
                )
                .unwrap()
            }
        }

        fn destinations(plan: &ExportPlan) -> Vec<String> {
            plan.items
                .iter()
                .map(|item| item.relative_dest.to_string_lossy().replace('\\', "/"))
                .collect()
        }

        /// The files placed below `root`, by file name.
        fn names_below(plan: &ExportPlan, root: &str) -> Vec<String> {
            let mut names: Vec<String> = destinations(plan)
                .into_iter()
                .filter(|path| path.starts_with(root))
                .map(|path| path.rsplit('/').next().unwrap().to_string())
                .collect();
            names.sort();
            names
        }

        fn numbered(stem: &str, count: usize) -> Vec<String> {
            (0..count)
                .map(|index| format!("{stem}-{index}.fits"))
                .collect()
        }

        const LIGHT_AT: &str = "DATE-OBS= '2026-09-11T03:00:00'";

        /// Flats past the default rotation or focal length limit go to the
        /// export only once the light's rig, or the server, widens the limit,
        /// and then into the light's session folder.
        #[test]
        fn the_light_s_rig_tolerances_choose_the_flats_an_export_places() {
            let _settings = POLICY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
            for (key, flat_cards) in [
                (
                    "rotation_tolerance_deg",
                    [
                        "ROTATANG=                104.0",
                        "FOCALLEN=                600.0",
                    ],
                ),
                (
                    "focal_length_tolerance_mm",
                    [
                        "ROTATANG=                100.0",
                        "FOCALLEN=                603.0",
                    ],
                ),
            ] {
                let mut library = Library::new(&[
                    LIGHT_AT,
                    "ROTATANG=                100.0",
                    "FOCALLEN=                600.0",
                ]);
                library.frames("flat", "FLAT", "2026-09-10T20", 2, &flat_cards);
                let rig = library.rig();

                let plan = library.plan(false);
                assert!(names_below(&plan, "flats/").is_empty(), "{key}: {plan:?}");
                assert!(
                    destinations(&plan)
                        .contains(&"lights/M42_Trapezium/Ha/acc_Ha_0001.fits".to_string()),
                    "{key}: {plan:?}"
                );
                assert!(plan.calibration_settings.is_empty(), "{key}: {plan:?}");

                // Another rig's wider limit changes nothing here.
                {
                    let _rules = RulesInForce::with(
                        &[],
                        &[("another-rig", "Other rig", &[(key, json!(5))])],
                    );
                    let plan = library.plan(false);
                    assert!(names_below(&plan, "flats/").is_empty(), "{key}: {plan:?}");
                    assert!(plan.calibration_settings.is_empty(), "{key}: {plan:?}");
                }

                // This rig's places them, in the light's session folder.
                {
                    let _rules =
                        RulesInForce::with(&[], &[(rig.as_str(), "Test rig", &[(key, json!(5))])]);
                    let plan = library.plan(false);
                    let placed = destinations(&plan);
                    for expected in [
                        "lights/M42_Trapezium/Ha/SESSION_2026-09-10/acc_Ha_0001.fits",
                        "flats/M42_Trapezium/Ha/SESSION_2026-09-10/flat-0.fits",
                        "flats/M42_Trapezium/Ha/SESSION_2026-09-10/flat-1.fits",
                    ] {
                        assert!(placed.contains(&expected.to_string()), "{key}: {placed:?}");
                    }
                    assert_eq!(plan.calibration_settings.len(), 1, "{key}: {plan:?}");
                    assert!(
                        plan.calibration_settings[0].ends_with("(for Test rig)"),
                        "{key}: {plan:?}"
                    );
                    let script = wbpp::shell_script(&plan, &wbpp::WbppScriptSpec::default());
                    assert!(
                        script.contains(&format!("#   {}", plan.calibration_settings[0])),
                        "{script}"
                    );
                }

                // As does the server's.
                {
                    let _rules = RulesInForce::global(&[(key, json!(5))]);
                    let plan = library.plan(false);
                    assert_eq!(names_below(&plan, "flats/"), numbered("flat", 2), "{key}");
                    assert!(
                        plan.calibration_settings[0].ends_with("(global)"),
                        "{key}: {plan:?}"
                    );
                }
            }
        }

        /// Sky flats from two evenings are two sessions by default, each
        /// light with its own; a longer flat session pools them into one
        /// folder that both lights share, as one master would take them.
        #[test]
        fn the_flat_session_decides_which_nights_share_a_session_folder() {
            let _settings = POLICY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
            let mut library = Library::new(&[LIGHT_AT]);
            library.light(9, "acc_Ha_0009.fits", &["DATE-OBS= '2026-09-13T03:00:00'"]);
            library.frames("flat-a", "FLAT", "2026-09-10T20", 2, &[]);
            library.frames("flat-b", "FLAT", "2026-09-12T20", 2, &[]);
            let rig = library.rig();

            let placed = destinations(&library.plan(false));
            for expected in [
                "lights/M42_Trapezium/Ha/SESSION_2026-09-10/acc_Ha_0001.fits",
                "flats/M42_Trapezium/Ha/SESSION_2026-09-10/flat-a-0.fits",
                "lights/M42_Trapezium/Ha/SESSION_2026-09-12/acc_Ha_0009.fits",
                "flats/M42_Trapezium/Ha/SESSION_2026-09-12/flat-b-0.fits",
            ] {
                assert!(placed.contains(&expected.to_string()), "{placed:?}");
            }

            let _rules = RulesInForce::with(
                &[],
                &[(
                    rig.as_str(),
                    "Test rig",
                    &[("flat_session_hours", json!(72))],
                )],
            );
            let plan = library.plan(false);
            let placed = destinations(&plan);
            for expected in [
                "lights/M42_Trapezium/Ha/SESSION_2026-09-10/acc_Ha_0001.fits",
                "lights/M42_Trapezium/Ha/SESSION_2026-09-10/acc_Ha_0009.fits",
                "flats/M42_Trapezium/Ha/SESSION_2026-09-10/flat-a-0.fits",
                "flats/M42_Trapezium/Ha/SESSION_2026-09-10/flat-b-1.fits",
            ] {
                assert!(placed.contains(&expected.to_string()), "{placed:?}");
            }
            assert!(
                placed
                    .iter()
                    .all(|path| !path.contains("SESSION_2026-09-12")),
                "{placed:?}"
            );
            assert_eq!(names_below(&plan, "flats/").len(), 4, "{placed:?}");
            assert!(
                plan.calibration_settings.iter().any(
                    |line| line.starts_with("Flat session") && line.ends_with("(for Test rig)")
                ),
                "{plan:?}"
            );
        }

        /// The night of a fake flat session, for `measure_as`.
        fn night(row: &crate::calibration::flat_groups::store::ResponseRow) -> &str {
            row.night.as_str()
        }

        /// Flats of the light's angle exist only after new dust; the rig's
        /// flats measure alike across angles, so they are one response
        /// family. The export takes the family's flats from before the dust
        /// at the other angle, never the dusty set.
        #[test]
        fn a_light_before_new_dust_exports_its_side_s_flats_at_another_angle() {
            let _settings = POLICY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
            let mut library = Library::new(&[LIGHT_AT, "ROTATANG=                 90.0"]);
            library.frames(
                "before-0",
                "FLAT",
                "2026-09-12T14",
                3,
                &["ROTATANG=                  0.0"],
            );
            library.frames(
                "after-0",
                "FLAT",
                "2026-09-16T14",
                3,
                &["ROTATANG=                  0.0"],
            );
            library.frames(
                "after-90",
                "FLAT",
                "2026-09-16T15",
                3,
                &["ROTATANG=                 90.0"],
            );
            crate::calibration::flat_groups::testing::measure_as(&library.conn, |a, b| {
                if night(a) != night(b) {
                    0.02
                } else {
                    0.0
                }
            });

            let plan = library.plan(false);
            assert_eq!(
                names_below(&plan, "flats/"),
                numbered("before-0", 3),
                "{plan:?}"
            );
            let placed = destinations(&plan);
            assert!(
                placed.contains(
                    &"lights/M42_Trapezium/Ha/SESSION_2026-09-12/acc_Ha_0001.fits".to_string()
                ),
                "{placed:?}"
            );
            // Its family's flats on its side are its nearest: nothing to
            // warn about.
            assert!(plan.flat_warnings().is_empty());

            // Grouped by time, it takes the dusty flats at its angle, as before.
            let _rules = RulesInForce::global(&[("flat_grouping", json!("time"))]);
            let plan = library.plan(false);
            assert_eq!(names_below(&plan, "flats/"), numbered("after-90", 3));
            assert!(plan.flat_changes.is_empty());
        }

        /// No flats from the light's side at any allowed angle: a stack
        /// carries the dusty set across the change, which WBPP cannot. The
        /// export gives the light a session of its own and says so in the
        /// runner, the summary and a note.
        #[test]
        fn flats_a_stack_would_correct_get_their_own_session_and_a_warning() {
            let _settings = POLICY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
            let mut library = Library::new(&[
                "DATE-OBS= '2026-09-07T03:00:00'",
                "ROTATANG=                 90.0",
            ]);
            // Two angles split at the same change two days apart, so no pair
            // of angles shares a flat session to measure the angle term.
            library.frames(
                "before-45",
                "FLAT",
                "2026-09-08T14",
                3,
                &["ROTATANG=                 45.0"],
            );
            library.frames(
                "before-0",
                "FLAT",
                "2026-09-10T14",
                3,
                &["ROTATANG=                  0.0"],
            );
            library.frames(
                "after-0",
                "FLAT",
                "2026-09-14T14",
                3,
                &["ROTATANG=                  0.0"],
            );
            library.frames(
                "after-45",
                "FLAT",
                "2026-09-16T14",
                3,
                &["ROTATANG=                 45.0"],
            );
            library.frames(
                "after-90",
                "FLAT",
                "2026-09-18T14",
                3,
                &["ROTATANG=                 90.0"],
            );
            // The angles differ by their own shadows: three response
            // families, none of them measured alike with another.
            crate::calibration::flat_groups::testing::measure_as(&library.conn, |a, b| {
                if (night(a) < "2026-09-12" && night(b) > "2026-09-12") || a.rotation != b.rotation
                {
                    0.02
                } else {
                    0.0
                }
            });

            let plan = library.plan(false);
            assert_eq!(
                names_below(&plan, "flats/"),
                numbered("after-90", 3),
                "{plan:?}"
            );
            let placed = destinations(&plan);
            let light = placed
                .iter()
                .find(|path| path.ends_with("acc_Ha_0001.fits"))
                .unwrap();
            assert!(
                light.contains("/SESSION_2026-09-18-across-"),
                "{light} {:?}",
                plan.flat_choices
            );
            let session = light.split('/').nth(3).unwrap();
            assert!(
                placed
                    .iter()
                    .filter(|path| path.starts_with("flats/"))
                    .all(|path| path.contains(session)),
                "{placed:?}"
            );
            assert_eq!(plan.flat_warnings().len(), 1);
            assert!(plan.flat_note().unwrap().contains("acc_Ha_0001.fits"));
            let script = wbpp::shell_script(&plan, &wbpp::WbppScriptSpec::default());
            assert!(
                script.contains("WARNING: 1 light have flats only from the other side"),
                "{script}"
            );
            let out = tempfile::tempdir().unwrap();
            let summary = execute_plan(&plan, out.path(), false, true);
            assert_eq!(summary.flat_warnings, 1);
            assert_eq!(plan.flat_choices[0].2.basis, "corrected");

            // Without the change's measurements nothing can carry the
            // flats across: the light is promised no correction, and is
            // flagged for flats from the other side instead.
            let catalog = library.conn.path().unwrap().to_string();
            std::fs::remove_dir_all(format!("{catalog}.responses")).unwrap();
            let plan = library.plan(false);
            assert_eq!(plan.flat_choices[0].2.basis, "no_same_side", "{plan:?}");
            assert!(plan.flat_choices[0].2.flagged);
            let light = destinations(&plan)
                .into_iter()
                .find(|path| path.ends_with("acc_Ha_0001.fits"))
                .unwrap();
            assert!(!light.contains("-across-"), "{light}");
        }

        /// Two lights of one night that need different flats go to two
        /// sessions, so WBPP builds each its own master.
        #[test]
        fn two_flat_sets_in_one_night_give_two_sessions() {
            let _settings = POLICY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
            let mut library = Library::new(&[LIGHT_AT, "ROTATANG=                  0.0"]);
            library.light(
                9,
                "acc_Ha_0009.fits",
                &[LIGHT_AT, "ROTATANG=                 90.0"],
            );
            library.frames(
                "flat-0",
                "FLAT",
                "2026-09-10T20",
                3,
                &["ROTATANG=                  0.0"],
            );
            library.frames(
                "flat-90",
                "FLAT",
                "2026-09-10T21",
                3,
                &["ROTATANG=                 90.0"],
            );

            let placed = destinations(&library.plan(false));
            let session_of = |name: &str| {
                placed
                    .iter()
                    .find(|path| path.ends_with(name))
                    .map(|path| path.split('/').nth(3).unwrap().to_string())
                    .unwrap()
            };
            let (first, second) = (
                session_of("acc_Ha_0001.fits"),
                session_of("acc_Ha_0009.fits"),
            );
            assert_ne!(first, second, "{placed:?}");
            assert!(first.starts_with("SESSION_2026-09-10-"), "{first}");
            assert_eq!(session_of("flat-0-0.fits"), first);
            assert_eq!(session_of("flat-90-0.fits"), second);

            // Grouping flats by time exports as it always did: the night's
            // one session.
            let _rules = crate::calibration::tests::RulesInForce::global(&[(
                "flat_grouping",
                serde_json::json!("time"),
            )]);
            let placed = destinations(&library.plan(false));
            let sessions: HashSet<String> = placed
                .iter()
                .filter(|path| path.contains("/SESSION_") || path.starts_with("flats/"))
                .filter_map(|path| path.split('/').find(|part| part.starts_with("SESSION_")))
                .map(str::to_string)
                .collect();
            assert_eq!(
                sessions,
                HashSet::from(["SESSION_2026-09-10".to_string()]),
                "{placed:?}"
            );
        }

        /// A longer flat session pools two evenings into one session when
        /// their flats measure alike, and keeps each light to its own
        /// evening's when they differ.
        #[test]
        fn alike_nights_share_a_session_and_nights_that_differ_do_not() {
            let _settings = POLICY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
            let mut library = Library::new(&[LIGHT_AT]);
            library.light(9, "acc_Ha_0009.fits", &["DATE-OBS= '2026-09-13T03:00:00'"]);
            // Dawn flats after the first light, dusk flats before the second.
            library.frames("flat-a", "FLAT", "2026-09-11T05", 3, &[]);
            library.frames("flat-b", "FLAT", "2026-09-12T20", 3, &[]);
            let rig = library.rig();
            let _rules = RulesInForce::with(
                &[],
                &[(
                    rig.as_str(),
                    "Test rig",
                    &[("flat_session_hours", json!(72))],
                )],
            );

            crate::calibration::flat_groups::testing::measure_as(&library.conn, |_, _| 0.002);
            let placed = destinations(&library.plan(false));
            for expected in [
                "lights/M42_Trapezium/Ha/SESSION_2026-09-10/acc_Ha_0001.fits",
                "lights/M42_Trapezium/Ha/SESSION_2026-09-10/acc_Ha_0009.fits",
                "flats/M42_Trapezium/Ha/SESSION_2026-09-10/flat-b-2.fits",
            ] {
                assert!(placed.contains(&expected.to_string()), "{placed:?}");
            }

            crate::calibration::flat_groups::testing::measure_as(&library.conn, |_, _| 0.02);
            let plan = library.plan(false);
            let placed = destinations(&plan);
            for expected in [
                "lights/M42_Trapezium/Ha/SESSION_2026-09-10/acc_Ha_0001.fits",
                "lights/M42_Trapezium/Ha/SESSION_2026-09-12/acc_Ha_0009.fits",
            ] {
                assert!(placed.contains(&expected.to_string()), "{placed:?}");
            }
            assert_eq!(names_below(&plan, "flats/").len(), 6, "{placed:?}");
            assert!(
                placed
                    .iter()
                    .filter(|path| path.contains("SESSION_2026-09-10/flat"))
                    .all(|path| path.contains("flat-a")),
                "{placed:?}"
            );
        }

        /// A rig set to do without darks exports none, keeps its bias, and
        /// the runner says why the darks are missing.
        #[test]
        fn a_rig_without_darks_exports_none_and_its_script_says_so() {
            let _settings = POLICY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
            let mut library = Library::new(&[LIGHT_AT]);
            library.frames("dark", "DARK", "2026-09-10T20", 10, &[]);
            library.frames("bias", "BIAS", "2026-09-10T21", 2, &[]);
            let rig = library.rig();

            let plan = library.plan(false);
            assert_eq!(names_below(&plan, "darks/"), numbered("dark", 10));
            assert_eq!(names_below(&plan, "bias/"), numbered("bias", 2));
            assert!(plan.without_darks.is_empty());
            let script = wbpp::shell_script(&plan, &wbpp::WbppScriptSpec::default());
            assert!(
                script.contains("matched by PSF Guard's default limits."),
                "{script}"
            );
            assert!(!script.contains("No darks are exported"), "{script}");

            let _rules = RulesInForce::with(
                &[],
                &[(rig.as_str(), "Test rig", &[("use_darks", json!(false))])],
            );
            let plan = library.plan(false);
            assert!(names_below(&plan, "darks/").is_empty(), "{plan:?}");
            assert_eq!(names_below(&plan, "bias/"), numbered("bias", 2));
            assert_eq!(plan.without_darks, vec!["Test rig".to_string()]);
            let script = wbpp::shell_script(&plan, &wbpp::WbppScriptSpec::default());
            assert!(
                script.contains("# No darks are exported for Test rig"),
                "{script}"
            );
            assert!(
                plan.calibration_settings
                    .iter()
                    .any(|line| line.starts_with("Use darks") && line.ends_with("(for Test rig)")),
                "{plan:?}"
            );
        }

        /// Darks within the default temperature tolerance stay out once the
        /// limit is tightened below their distance from the light.
        #[test]
        fn the_dark_temperature_limit_keeps_warmer_darks_out_of_an_export() {
            let _settings = POLICY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
            let mut library = Library::new(&[LIGHT_AT, "CCD-TEMP=                -10.0"]);
            library.frames(
                "dark",
                "DARK",
                "2026-09-10T20",
                10,
                &["CCD-TEMP=                 -8.0"],
            );

            assert_eq!(
                names_below(&library.plan(false), "darks/"),
                numbered("dark", 10)
            );

            let _rules = RulesInForce::global(&[("dark_temperature_tolerance_c", json!(1))]);
            let plan = library.plan(false);
            assert!(names_below(&plan, "darks/").is_empty(), "{plan:?}");
            assert!(
                plan.calibration_settings
                    .iter()
                    .any(|line| line.starts_with("Dark temperature") && line.ends_with("(global)")),
                "{plan:?}"
            );
        }

        /// The darks an export places follow the dark choice and the dark
        /// reach: a complete night two months off by default, the nearer,
        /// shorter set when the rig asks for the nearest, and only that set
        /// once the complete one is out of reach.
        #[test]
        fn dark_reach_and_dark_choice_pick_the_darks_an_export_places() {
            let _settings = POLICY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
            let mut library = Library::new(&[
                "DATE-OBS= '2026-03-10T03:00:00'",
                "CCD-TEMP=                -10.0",
            ]);
            library.frames(
                "dark-jan",
                "DARK",
                "2026-01-09T20",
                10,
                &["CCD-TEMP=                -10.0"],
            );
            library.frames(
                "dark-mar",
                "DARK",
                "2026-03-09T20",
                3,
                &["CCD-TEMP=                 -8.5"],
            );
            let rig = library.rig();

            assert_eq!(
                names_below(&library.plan(false), "darks/"),
                numbered("dark-jan", 10)
            );
            {
                let _rules = RulesInForce::with(
                    &[],
                    &[(
                        rig.as_str(),
                        "Test rig",
                        &[("dark_choice", json!("nearest"))],
                    )],
                );
                let plan = library.plan(false);
                assert_eq!(names_below(&plan, "darks/"), numbered("dark-mar", 3));
                assert!(
                    plan.calibration_settings
                        .iter()
                        .any(|line| line.starts_with("Dark selection preference")),
                    "{plan:?}"
                );
            }
            {
                let _rules = RulesInForce::with(
                    &[],
                    &[(rig.as_str(), "Test rig", &[("dark_reach_days", json!(30))])],
                );
                assert_eq!(
                    names_below(&library.plan(false), "darks/"),
                    numbered("dark-mar", 3)
                );
            }
        }

        /// A master from other software is exported while the setting uses
        /// such masters, and left behind when it ignores them.
        #[test]
        fn the_external_masters_setting_decides_whether_an_export_takes_one() {
            let _settings = POLICY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
            let mut library = Library::new(&[LIGHT_AT]);
            library.frames("dark", "DARK", "2026-09-10T20", 2, &[]);
            library.frames("masterDark", "MASTER DARK", "2026-09-10T22", 1, &[]);

            let placed = names_below(&library.plan(false), "darks/");
            assert!(
                placed.contains(&"masterDark-0.fits".to_string()),
                "{placed:?}"
            );

            let _rules = RulesInForce::global(&[("external_masters", json!("ignore"))]);
            let plan = library.plan(false);
            assert_eq!(names_below(&plan, "darks/"), numbered("dark", 2));
            assert!(
                plan.calibration_settings
                    .iter()
                    .any(|line| line.starts_with("External masters")),
                "{plan:?}"
            );
        }

        /// A light whose missing flat is flagged: the library holds flats
        /// for this camera, none at the light's angle.
        fn light_without_a_flat() -> Library {
            let mut library = Library::new(&[LIGHT_AT, "ROTATANG=                100.0"]);
            library.frames(
                "flat",
                "FLAT",
                "2026-09-10T20",
                2,
                &["ROTATANG=                110.0"],
            );
            library
        }

        /// With **Leave out lights that can't be calibrated** on, every
        /// export leaves out a light whose gap is flagged, in either layout,
        /// as a stack does, and says how many and why. Off, or with the
        /// check at Check only or Off, the light goes.
        #[test]
        fn every_export_leaves_out_the_lights_a_stack_leaves_out() {
            let _settings = POLICY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
            let library = light_without_a_flat();
            let rig = library.rig();
            for (layout, light) in [
                (
                    ExportLayout::Standard,
                    "M42_Trapezium/LIGHT/Ha/acc_Ha_0001.fits",
                ),
                (
                    ExportLayout::Wbpp,
                    "lights/M42_Trapezium/Ha/acc_Ha_0001.fits",
                ),
            ] {
                let light = light.to_string();

                // Kept unless the setting asks.
                let plan = library.plan_in(layout, false);
                assert!(destinations(&plan).contains(&light), "{layout:?}: {plan:?}");
                assert!(plan.uncalibrated.is_empty(), "{layout:?}");
                assert_eq!(plan.uncalibrated_note(), None);

                let plan = library.plan_in(layout, true);
                assert!(plan.items.is_empty(), "{layout:?}: {plan:?}");
                assert_eq!(plan.uncalibrated.len(), 1, "{layout:?}: {plan:?}");
                let (id, name, reason) = &plan.uncalibrated[0];
                assert_eq!((*id, name.as_str()), (1, "acc_Ha_0001.fits"));
                assert!(reason.starts_with("No matching flat"), "{reason}");
                // The summary every export reports, and the note a download
                // carries, say how many and why.
                let dest = tempfile::tempdir().unwrap();
                let summary =
                    execute_plan_with(&plan, dest.path(), Placement::Copy, false, &mut |_, _| {});
                assert_eq!(summary.uncalibrated, 1, "{layout:?}");
                let example = summary.uncalibrated_example.unwrap();
                assert!(
                    example.starts_with("acc_Ha_0001.fits: No matching flat"),
                    "{example}"
                );
                let note = plan.uncalibrated_note().unwrap();
                assert!(
                    note.starts_with("1 light(s) that can't be calibrated were left out"),
                    "{note}"
                );
                assert!(
                    note.contains("acc_Ha_0001.fits (image 1): No matching flat"),
                    "{note}"
                );

                for mode in ["advisory", "off"] {
                    let _rules = RulesInForce::with(
                        &[],
                        &[(rig.as_str(), "Test rig", &[("check_no_flat", json!(mode))])],
                    );
                    let plan = library.plan_in(layout, true);
                    assert!(
                        destinations(&plan).contains(&light),
                        "{layout:?} {mode}: {plan:?}"
                    );
                    assert!(plan.uncalibrated.is_empty(), "{layout:?} {mode}: {plan:?}");
                    // The check matters to which lights went, so it is named.
                    assert!(
                        plan.calibration_settings
                            .iter()
                            .any(|line| line.starts_with("No matching flat")
                                && line.ends_with("(for Test rig)")),
                        "{layout:?} {mode}: {plan:?}"
                    );
                    // An export that keeps such lights has no use for it.
                    assert!(
                        library
                            .plan_in(layout, false)
                            .calibration_settings
                            .is_empty(),
                        "{layout:?} {mode}"
                    );
                }
            }

            // The WBPP runner says so too.
            let plan = library.plan(true);
            let script = wbpp::shell_script(&plan, &wbpp::WbppScriptSpec::default());
            assert!(
                script.contains("# 1 light that can't be calibrated was left out, as stacks leave"),
                "{script}"
            );
            assert!(
                script.contains("acc_Ha_0001.fits: No matching flat"),
                "{script}"
            );
        }

        /// The AstroBin acquisition counts only the lights a stack would
        /// integrate when the setting is on, and says why one is missing.
        #[test]
        fn the_astrobin_export_leaves_out_the_lights_a_stack_leaves_out() {
            use crate::astrobin::{AstroBinDetail, AstroBinExportRequest};
            let _settings = POLICY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
            let library = light_without_a_flat();
            // AstroBin counts a light by its exposure, from the catalog.
            let path = library.dir.path().join("acc_Ha_0001.fits");
            library
                .conn
                .execute(
                    "UPDATE acquiredimage SET metadata = ?1 WHERE Id = 1",
                    [serde_json::json!({
                        "FileName": path.display().to_string(),
                        "ExposureDuration": 300,
                    })
                    .to_string()],
                )
                .unwrap();
            let rig = library.rig();
            let tree = crate::directory_tree::DirectoryTree::build(library.dir.path()).unwrap();
            let export =
                |exclude_uncalibrated: bool,
                 tree: Option<&crate::directory_tree::DirectoryTree>| {
                    crate::astrobin::export(
                        &library.conn,
                        tree,
                        &std::collections::BTreeMap::new(),
                        &AstroBinExportRequest {
                            target_id: Some(1),
                            detail: AstroBinDetail::Essentials,
                            exclude_uncalibrated,
                            ..Default::default()
                        },
                    )
                    .unwrap()
                };

            let kept = export(false, Some(&tree));
            assert_eq!((kept.frames, kept.uncalibrated_lights), (1, 0));

            let left_out = export(true, Some(&tree));
            assert_eq!((left_out.frames, left_out.uncalibrated_lights), (0, 1));
            assert!(
                left_out.notes.iter().any(|note| note.starts_with(
                    "1 light(s) that can't be calibrated were left out, as stacks leave them out"
                ) && note
                    .contains("acc_Ha_0001.fits: No matching flat")),
                "{:?}",
                left_out.notes
            );

            // Without the folders a light can't be judged, and the export says so.
            let unjudged = export(true, None);
            assert_eq!(unjudged.frames, 1);
            assert!(
                unjudged
                    .notes
                    .iter()
                    .any(|note| note.contains("could not be left out")),
                "{:?}",
                unjudged.notes
            );

            for mode in ["advisory", "off"] {
                let _rules = RulesInForce::with(
                    &[],
                    &[(rig.as_str(), "Test rig", &[("check_no_flat", json!(mode))])],
                );
                let export = export(true, Some(&tree));
                assert_eq!(
                    (export.frames, export.uncalibrated_lights),
                    (1, 0),
                    "{mode}"
                );
            }
        }

        /// WBPP scans the frame roots recursively, so a frame an earlier
        /// export left there would join this one. The runner warns about it;
        /// nothing is deleted.
        #[test]
        fn a_placed_export_warns_about_frames_an_earlier_one_left() {
            let _settings = POLICY_LOCK.lock().unwrap_or_else(|p| p.into_inner());
            let mut library = Library::new(&[LIGHT_AT]);
            library.frames("flat", "FLAT", "2026-09-10T20", 2, &[]);
            let plan = library.plan(false);
            let dest = tempfile::tempdir().unwrap();
            let stale = dest
                .path()
                .join("lights/M42_Trapezium/Ha/SESSION_2026-08-01");
            std::fs::create_dir_all(&stale).unwrap();
            std::fs::write(stale.join("old.fits"), b"old").unwrap();
            std::fs::write(stale.join("notes.txt"), b"not a frame").unwrap();
            // WBPP's own output is not a scanned root.
            std::fs::create_dir_all(dest.path().join(wbpp::OUTPUT_DIRECTORY)).unwrap();
            std::fs::write(
                dest.path().join(wbpp::OUTPUT_DIRECTORY).join("master.xisf"),
                b"m",
            )
            .unwrap();

            let summary =
                execute_plan_with(&plan, dest.path(), Placement::Copy, false, &mut |_, _| {});
            assert_eq!(summary.errors, 0);
            assert_eq!(
                wbpp::stale_frames(&plan, dest.path()),
                vec!["lights/M42_Trapezium/Ha/SESSION_2026-08-01/old.fits".to_string()]
            );
            write_wbpp_scripts(&plan, dest.path(), &wbpp::WbppScriptSpec::default()).unwrap();
            let script = std::fs::read_to_string(dest.path().join("run-wbpp.sh")).unwrap();
            assert!(
                script.contains("# WARNING: 1 file under the frame folders is not part of this"),
                "{script}"
            );
            assert!(
                script.contains("lights/M42_Trapezium/Ha/SESSION_2026-08-01/old.fits"),
                "{script}"
            );
            assert!(stale.join("old.fits").exists());

            // A fresh folder has nothing to warn about.
            let fresh = tempfile::tempdir().unwrap();
            execute_plan_with(&plan, fresh.path(), Placement::Copy, false, &mut |_, _| {});
            write_wbpp_scripts(&plan, fresh.path(), &wbpp::WbppScriptSpec::default()).unwrap();
            let script = std::fs::read_to_string(fresh.path().join("run-wbpp.sh")).unwrap();
            assert!(!script.contains("WARNING"), "{script}");
        }
    }
}
