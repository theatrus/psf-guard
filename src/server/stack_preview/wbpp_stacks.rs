//! Stacks WBPP made, taken into PSF Guard's stack store.
//!
//! When a WBPP run ends with masters, each master light becomes a stack in
//! the same store as PSF Guard's own: a FITS, a stretched preview, and an
//! entry in the project's WBPP index (`wbpp-project-<id>.json`, beside the
//! `latest-project-<id>.json` of PSF Guard's stacks). The Stacks view lists
//! them, and color composition offers them as channel sources next to PSF
//! Guard's, so a WBPP stack gets PSF Guard's color preview without a step in
//! PixInsight.
//!
//! WBPP groups frames by filter, exposure and binning, not by target, so a
//! master is only tied to a target when the run covered one: a run for one
//! target, or for a project with a single target. A run across several
//! targets mixes them, and its masters are not taken in.

use super::{
    LatestStackPreviewGroup, LatestStackPreviews, StackGroupState, StackGroupStatus, StackJobState,
    StackPreviewJob, SEIZA_STACKING_VERSION, STACK_PREVIEW_CACHE_VERSION,
};
use crate::server::database_context::DatabaseContext;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// Where a WBPP stack came from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WbppStackSource {
    /// The master's file name, as WBPP wrote it.
    pub master_file: String,
    /// The WBPP output folder it was taken from.
    pub output_dir: String,
    /// Sub-exposure length WBPP put in the name, in seconds.
    pub exposure_seconds: Option<f64>,
    pub drizzle: bool,
    pub autocrop: bool,
    pub imported_unix_seconds: i64,
}

/// What a master light's name says. WBPP writes, for example,
/// `masterLight_BIN-1_6248x4176_EXPOSURE-75.00s_FILTER-L_mono_autocrop.xisf`.
#[derive(Debug, Clone, PartialEq)]
pub struct MasterName {
    pub filter: String,
    pub exposure_seconds: Option<f64>,
    pub drizzle: bool,
    pub autocrop: bool,
}

pub fn parse_master_name(file_name: &str) -> Option<MasterName> {
    let stem = file_name
        .strip_suffix(".xisf")
        .or_else(|| file_name.strip_suffix(".fits"))
        .or_else(|| file_name.strip_suffix(".fit"))?;
    let rest = stem.strip_prefix("masterLight")?;
    let parts = rest
        .split('_')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    let filter = parts
        .iter()
        .find_map(|part| part.strip_prefix("FILTER-"))
        .map(str::to_string)
        .unwrap_or_default();
    let exposure_seconds = parts
        .iter()
        .find_map(|part| part.strip_prefix("EXPOSURE-"))
        .and_then(|value| value.trim_end_matches('s').parse::<f64>().ok());
    Some(MasterName {
        filter,
        exposure_seconds,
        drizzle: parts.iter().any(|part| part.starts_with("drizzle")),
        autocrop: parts.contains(&"autocrop"),
    })
}

/// One master per filter and exposure. Without drizzle comes first, since
/// drizzle changes the scale and color composition aligns channels of one
/// scale; then cropped, since the edges WBPP crops are the thin ones.
pub fn choose_masters(files: &[PathBuf]) -> Vec<(PathBuf, MasterName)> {
    let mut chosen: Vec<(PathBuf, MasterName)> = Vec::new();
    let rank = |name: &MasterName| (name.drizzle, !name.autocrop);
    for path in files {
        let Some(name) = path
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(parse_master_name)
        else {
            continue;
        };
        let key = |other: &MasterName| {
            other.filter == name.filter && other.exposure_seconds == name.exposure_seconds
        };
        match chosen.iter_mut().find(|(_, existing)| key(existing)) {
            Some(slot) if rank(&name) < rank(&slot.1) => *slot = (path.clone(), name),
            Some(_) => {}
            None => chosen.push((path.clone(), name)),
        }
    }
    chosen.sort_by(|left, right| left.1.filter.cmp(&right.1.filter));
    chosen
}

pub(super) fn wbpp_index_path(cache_root: &Path, project_id: i32) -> PathBuf {
    cache_root
        .join("stack-previews")
        .join(format!("wbpp-project-{project_id}.json"))
}

/// Every WBPP index in a cache, for the janitor's keep-set.
pub(super) fn read_wbpp_indices(directory: &Path) -> Vec<LatestStackPreviews> {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with("wbpp-project-") && name.ends_with(".json"))
        })
        .filter_map(|entry| std::fs::read(entry.path()).ok())
        .filter_map(|bytes| serde_json::from_slice(&bytes).ok())
        .collect()
}

/// A project's WBPP stacks, empty when it has none.
pub fn load_index(ctx: &DatabaseContext, project_id: i32) -> LatestStackPreviews {
    std::fs::read(wbpp_index_path(&ctx.cache_dir_path, project_id))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<LatestStackPreviews>(&bytes).ok())
        .filter(|index| index.database_id == ctx.id && index.project_id == project_id)
        .unwrap_or_else(|| LatestStackPreviews {
            schema_version: 1,
            database_id: ctx.id.clone(),
            project_id,
            updated_unix_seconds: 0,
            groups: Vec::new(),
        })
}

/// The target a run's masters belong to, or why there is none.
pub fn run_target(
    conn: &rusqlite::Connection,
    project_id: i32,
    target_id: Option<i32>,
) -> Result<(i32, String), String> {
    let targets = conn
        .prepare("SELECT Id, name FROM target WHERE projectId = ?1 ORDER BY Id")
        .and_then(|mut statement| {
            statement
                .query_map([project_id], |row| {
                    Ok((row.get::<_, i32>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()
        })
        .map_err(|error| error.to_string())?;
    match target_id {
        Some(target_id) => targets
            .into_iter()
            .find(|(id, _)| *id == target_id)
            .ok_or_else(|| format!("target {target_id} is not in project {project_id}")),
        None if targets.len() == 1 => Ok(targets.into_iter().next().expect("one target")),
        None => Err(format!(
            "the run covered all {} targets of the project, and WBPP integrates them together",
            targets.len()
        )),
    }
}

/// What one import did.
#[derive(Debug, Clone, Default, Serialize)]
pub struct WbppImport {
    pub imported: Vec<String>,
    pub skipped: Vec<String>,
}

fn job_id_for(path: &Path, metadata: &std::fs::Metadata) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"wbpp-stack-v1\0");
    hasher.update(path.to_string_lossy().as_bytes());
    hasher.update(metadata.len().to_le_bytes());
    if let Ok(modified) = metadata.modified()
        && let Ok(since) = modified.duration_since(std::time::UNIX_EPOCH)
    {
        hasher.update(since.as_nanos().to_le_bytes());
    }
    let mut id = String::with_capacity(64);
    for byte in hasher.finalize() {
        write!(&mut id, "{byte:02x}").expect("writing to a String cannot fail");
    }
    id
}

static IMPORT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Write one master's FITS and previews into the stack store, unless an
/// earlier import already left both. Returns its channel count.
fn write_master(cache_root: &Path, job_id: &str, master: &Path) -> Result<usize, String> {
    let fits = super::fits_path(cache_root, job_id, 0);
    let preview = super::preview_path(cache_root, job_id, 0);
    let original = super::original_preview_path(cache_root, job_id, 0);
    let source = if fits.is_file() {
        fits.as_path()
    } else {
        master
    };
    let frame = crate::image_io::open_linear_frame(source).map_err(|error| error.to_string())?;
    if !fits.is_file() {
        let directory = super::stack_dir(cache_root, job_id);
        std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
        let temporary = directory.join(format!(
            "group-0.fits.{}.part",
            uuid::Uuid::new_v4().simple()
        ));
        let written = seiza_stacking::write_linear_image_fits_f32(
            &temporary,
            &frame.image,
            &frame.headers,
            &[],
        )
        .map_err(|error| error.to_string())
        .and_then(|()| std::fs::rename(&temporary, &fits).map_err(|error| error.to_string()));
        if let Err(error) = written {
            let _ = std::fs::remove_file(&temporary);
            return Err(error);
        }
    }
    if !preview.is_file() || !original.is_file() {
        super::stretch::render_image_previews_atomic(
            &frame.image,
            &super::stretch::default_linear_config(),
            super::stretch::StackStretchSourceTransfer::Linear,
            &preview,
            &original,
        )
        .map_err(|error| error.to_string())?;
    }
    Ok(frame.image.channels)
}

/// Take a finished run's master lights into the project's WBPP stacks. Each
/// becomes a FITS and preview in the stack store, read through PSF Guard's
/// frame reader; a master already taken in is left as it is. A master that
/// fails is listed as skipped, and the rest still go in. Newer masters
/// for the same target, filter and exposure replace older ones in the index.
pub fn import_masters(
    ctx: &DatabaseContext,
    project_id: i32,
    (target_id, target_name): (i32, String),
    output_dir: &Path,
) -> Result<WbppImport, String> {
    let files = crate::pixinsight::list_outputs(output_dir)
        .into_iter()
        .filter(|file| file.kind == "master")
        .map(|file| output_dir.join(&file.path))
        .collect::<Vec<_>>();
    let masters = choose_masters(&files);
    let mut outcome = WbppImport::default();
    if masters.is_empty() {
        return Err("the run wrote no master lights".into());
    }
    let cache_root = &ctx.cache_dir_path;
    // One import at a time: the automatic one and a click can name the same
    // masters, and the index is read, changed, and written whole.
    let _guard = IMPORT_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut index = load_index(ctx, project_id);
    let now = chrono::Utc::now().timestamp();
    for (path, name) in masters {
        let label = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let metadata = match std::fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) => {
                outcome.skipped.push(format!("{label}: {error}"));
                continue;
            }
        };
        let job_id = job_id_for(&path, &metadata);
        let revision = format!("wbpp-{}", &job_id[..12]);
        let channels = match write_master(cache_root, &job_id, &path) {
            Ok(channels) => channels,
            Err(error) => {
                outcome.skipped.push(format!("{label}: {error}"));
                continue;
            }
        };
        let group = StackGroupStatus {
            index: 0,
            target_id,
            target_name: target_name.clone(),
            filter_name: name.filter.clone(),
            exposure_group: None,
            state: StackGroupState::Ready,
            phase: "ready".into(),
            total_candidates: 0,
            eligible_frames: 0,
            quality_excluded: 0,
            missing_files: 0,
            processed_frames: 0,
            accepted_frames: 0,
            rejected_frames: 0,
            reused_frames: 0,
            resume_note: None,
            output_channels: channels,
            sky_orientation: None,
            reference_image_id: None,
            total_exposure_seconds: 0.0,
            preview_url: Some(format!(
                "/api/db/{}/stack-previews/{job_id}/0/preview?v={revision}",
                ctx.id
            )),
            fits_url: Some(format!(
                "/api/db/{}/stack-previews/{job_id}/0/fits?v={revision}",
                ctx.id
            )),
            snr: None,
            snr_url: None,
            final_pass: None,
            calibration_progress: None,
            error: None,
            calibration: crate::calibration::AppliedCalibration::default(),
            input_images: Vec::new(),
            frames: Vec::new(),
        };
        let job = StackPreviewJob {
            schema_version: 2,
            job_id: job_id.clone(),
            database_id: ctx.id.clone(),
            project_id,
            state: StackJobState::Completed,
            accepted_only: false,
            created_unix_seconds: now,
            artifact_revision: revision.clone(),
            cache_version: STACK_PREVIEW_CACHE_VERSION,
            stacking_version: SEIZA_STACKING_VERSION.into(),
            order: Default::default(),
            scoring: Default::default(),
            method: None,
            groups: vec![group.clone()],
            error: None,
            automatic: false,
            color_defaults: None,
        };
        if let Err(error) =
            super::stretch::write_json_atomic(&super::manifest_path(cache_root, &job_id), &job)
        {
            outcome.skipped.push(format!("{label}: {error}"));
            continue;
        }
        let entry = LatestStackPreviewGroup {
            job_id: job_id.clone(),
            artifact_revision: revision,
            accepted_only: false,
            created_unix_seconds: now,
            cache_version: STACK_PREVIEW_CACHE_VERSION,
            order: Default::default(),
            scoring: Default::default(),
            method: None,
            wbpp: Some(WbppStackSource {
                master_file: label.clone(),
                output_dir: output_dir.display().to_string(),
                exposure_seconds: name.exposure_seconds,
                drizzle: name.drizzle,
                autocrop: name.autocrop,
                imported_unix_seconds: now,
            }),
            group,
        };
        let same = |existing: &LatestStackPreviewGroup| {
            existing.group.target_id == entry.group.target_id
                && existing.group.filter_name == entry.group.filter_name
                && existing
                    .wbpp
                    .as_ref()
                    .and_then(|source| source.exposure_seconds)
                    == name.exposure_seconds
        };
        match index.groups.iter_mut().find(|existing| same(existing)) {
            Some(existing) => *existing = entry,
            None => index.groups.push(entry),
        }
        outcome.imported.push(label);
    }
    index.groups.sort_by(|left, right| {
        left.group
            .target_name
            .cmp(&right.group.target_name)
            .then_with(|| left.group.filter_name.cmp(&right.group.filter_name))
    });
    index.updated_unix_seconds = now;
    super::stretch::write_json_atomic(&wbpp_index_path(cache_root, project_id), &index)?;
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_master_name_says_its_filter_exposure_and_treatment() {
        assert_eq!(
            parse_master_name(
                "masterLight_BIN-1_6248x4176_EXPOSURE-75.00s_FILTER-L_mono_autocrop.xisf"
            ),
            Some(MasterName {
                filter: "L".into(),
                exposure_seconds: Some(75.0),
                drizzle: false,
                autocrop: true,
            })
        );
        let drizzled = parse_master_name(
            "masterLight_BIN-1_6248x4176_EXPOSURE-300.00s_FILTER-Ha_mono_drizzle_2x_autocrop.xisf",
        )
        .unwrap();
        assert!(drizzled.drizzle && drizzled.autocrop);
        assert_eq!(drizzled.filter, "Ha");
        assert_eq!(
            parse_master_name("masterDark_BIN-1_EXPOSURE-300.00s.xisf"),
            None
        );
        assert_eq!(parse_master_name("notes.txt"), None);
    }

    #[test]
    fn one_master_per_filter_preferring_no_drizzle_then_cropped() {
        let files = [
            "masterLight_BIN-1_EXPOSURE-75.00s_FILTER-L_mono_drizzle_2x_autocrop.xisf",
            "masterLight_BIN-1_EXPOSURE-75.00s_FILTER-L_mono.xisf",
            "masterLight_BIN-1_EXPOSURE-75.00s_FILTER-L_mono_autocrop.xisf",
            "masterLight_BIN-1_EXPOSURE-75.00s_FILTER-R_mono_drizzle_2x.xisf",
        ]
        .map(PathBuf::from);
        let chosen = choose_masters(&files);
        let names = chosen
            .iter()
            .map(|(path, _)| path.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            names,
            [
                "masterLight_BIN-1_EXPOSURE-75.00s_FILTER-L_mono_autocrop.xisf",
                // A drizzled master is taken when it is the only one.
                "masterLight_BIN-1_EXPOSURE-75.00s_FILTER-R_mono_drizzle_2x.xisf",
            ]
        );
    }

    #[test]
    fn masters_belong_to_a_target_only_when_the_run_covered_one() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        crate::ts_schema::apply_schema(&conn).unwrap();
        conn.execute_batch(
            "INSERT INTO project(Id,name,profileId,guid) VALUES(1,'One','p','g1'),(2,'Mosaic','p','g2');
             INSERT INTO target(Id,name,projectId,active,ra,dec,epochcode,rotation,roi,guid)
               VALUES(10,'Sh2 86',1,1,1,1,0,0,100,'t10'),
                     (20,'Panel 1',2,1,1,1,0,0,100,'t20'),
                     (21,'Panel 2',2,1,1,1,0,0,100,'t21');",
        )
        .unwrap();
        assert_eq!(run_target(&conn, 1, None).unwrap(), (10, "Sh2 86".into()));
        assert_eq!(
            run_target(&conn, 2, Some(21)).unwrap(),
            (21, "Panel 2".into())
        );
        assert!(run_target(&conn, 2, None)
            .unwrap_err()
            .contains("all 2 targets"));
        assert!(run_target(&conn, 1, Some(21)).is_err());
    }

    #[test]
    fn an_import_takes_good_masters_skips_a_broken_one_and_repeats_cleanly() {
        let temp = tempfile::tempdir().unwrap();
        let database_path = temp.path().join("scheduler.sqlite");
        let connection = crate::ts_schema::create_fresh_db(&database_path).unwrap();
        connection
            .execute_batch(
                "INSERT INTO project (Id, profileId, name) VALUES (1, 'default', 'Project');
                 INSERT INTO target (Id, name, active, epochcode, projectId)
                    VALUES (1, 'Target', 1, 0, 1);",
            )
            .unwrap();
        drop(connection);
        let ctx = DatabaseContext::new(
            "test".into(),
            "Test".into(),
            database_path.to_string_lossy().into_owned(),
            vec![temp.path().to_string_lossy().into_owned()],
            None,
            None,
            None,
            temp.path().join("cache").to_string_lossy().into_owned(),
        )
        .unwrap();
        let run = temp.path().join("run");
        let masters = run.join("master");
        std::fs::create_dir_all(&masters).unwrap();
        let image = seiza_stacking::LinearImage {
            width: 32,
            height: 24,
            channels: 1,
            data: (0..32 * 24).map(|value| value as f32 / 1000.0).collect(),
        };
        seiza_stacking::write_linear_image_fits_f32(
            masters.join("masterLight_BIN-1_32x24_EXPOSURE-60.00s_FILTER-R_mono.fits"),
            &image,
            &[],
            &[],
        )
        .unwrap();
        std::fs::write(
            masters.join("masterLight_BIN-1_32x24_EXPOSURE-60.00s_FILTER-G_mono.fits"),
            b"not a FITS file",
        )
        .unwrap();

        let target = (1, "Target".to_string());
        let first = import_masters(&ctx, 1, target.clone(), &run).unwrap();
        assert_eq!(first.imported.len(), 1, "{first:?}");
        assert_eq!(first.skipped.len(), 1, "{first:?}");
        assert!(first.skipped[0].contains("FILTER-G"));

        let index = load_index(&ctx, 1);
        assert_eq!(index.groups.len(), 1);
        let entry = &index.groups[0];
        assert_eq!(entry.group.filter_name, "R");
        assert_eq!(entry.group.output_channels, 1);
        assert_eq!(entry.wbpp.as_ref().unwrap().exposure_seconds, Some(60.0));
        let cache = &ctx.cache_dir_path;
        assert!(super::super::fits_path(cache, &entry.job_id, 0).is_file());
        assert!(super::super::preview_path(cache, &entry.job_id, 0).is_file());

        // A preview lost after the FITS was written comes back on the next
        // import, and the master is not listed twice.
        std::fs::remove_file(super::super::preview_path(cache, &entry.job_id, 0)).unwrap();
        let second = import_masters(&ctx, 1, target, &run).unwrap();
        assert_eq!(second.imported.len(), 1);
        assert!(super::super::preview_path(cache, &entry.job_id, 0).is_file());
        assert_eq!(load_index(&ctx, 1).groups.len(), 1);
        let leftovers = std::fs::read_dir(super::super::stack_dir(cache, &entry.job_id))
            .unwrap()
            .flatten()
            .filter(|file| file.file_name().to_string_lossy().ends_with(".part"))
            .count();
        assert_eq!(leftovers, 0);
    }
}
