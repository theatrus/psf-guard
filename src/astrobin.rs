//! AstroBin acquisition CSV export.
//!
//! AstroBin's image upload form imports a CSV of long-exposure acquisition
//! sessions: one row per date, filter and exposure length, with the number
//! of frames and, optionally, camera settings and calibration counts. Only
//! `number` and `duration` are required; AstroBin ignores columns it does
//! not know and values it cannot read. The `filter` column is the numeric
//! id of a filter in AstroBin's equipment database, which the catalog cannot
//! know, so the person maps their filter names to ids once in the settings.
//!
//! Two levels of detail are offered. **Essentials** is the date, filter,
//! count and duration: what every upload needs, built from the catalog
//! alone. **Full** adds binning, gain, sensor and ambient temperature from
//! the catalog's metadata, and the f-number and matched calibration counts
//! from one light's header per night and filter.

use anyhow::{Context, Result};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::directory_tree::DirectoryTree;
use crate::models::{AcquiredImage, GradingStatus};
use crate::server::sky_coverage::{night_boundary, night_of};

/// How much of AstroBin's CSV to fill in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AstroBinDetail {
    /// Date, filter, number and duration.
    #[default]
    Essentials,
    /// Everything the catalog and the frames' headers can supply.
    Full,
}

impl AstroBinDetail {
    pub fn as_str(self) -> &'static str {
        match self {
            AstroBinDetail::Essentials => "essentials",
            AstroBinDetail::Full => "full",
        }
    }
}

/// One light as the export sees it: what the catalog's metadata records.
#[derive(Debug, Clone, PartialEq)]
pub struct AstroBinFrame {
    pub captured_at: i64,
    pub filter: String,
    pub exposure_s: f64,
    pub gain: Option<f64>,
    /// The x binning factor.
    pub binning: Option<u32>,
    /// The sensor's temperature at capture (`CameraTemp`).
    pub sensor_temp: Option<f64>,
    /// The focuser's temperature probe (`FocuserTemp`), the nearest thing
    /// a N.I.N.A. rig records to the air temperature.
    pub ambient_temp: Option<f64>,
}

impl AstroBinFrame {
    /// The frame a catalog row describes, or `None` when the row lacks a
    /// capture time or an exposure length, without which AstroBin has
    /// nothing to count.
    pub fn from_image(image: &AcquiredImage) -> Option<Self> {
        let captured_at = image.acquired_date?;
        let metadata: serde_json::Value =
            serde_json::from_str(&image.metadata).unwrap_or(serde_json::Value::Null);
        let exposure_s = ["ExposureDuration", "ExposureTime", "EXPTIME"]
            .iter()
            .find_map(|key| number_of(&metadata[key]))
            .filter(|seconds| *seconds > 0.0)?;
        let filter = if image.filter_name.trim().is_empty() {
            metadata["FilterName"]
                .as_str()
                .map(str::trim)
                .unwrap_or("")
                .to_string()
        } else {
            image.filter_name.trim().to_string()
        };
        Some(Self {
            captured_at,
            filter,
            exposure_s,
            gain: number_of(&metadata["Gain"]),
            binning: binning_of(&metadata),
            sensor_temp: number_of(&metadata["CameraTemp"]),
            ambient_temp: number_of(&metadata["FocuserTemp"]),
        })
    }
}

/// A number written as a number or as numeric text; anything else is absent.
fn number_of(value: &serde_json::Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str()?.trim().parse().ok())
        .filter(|number: &f64| number.is_finite())
}

/// Target Scheduler writes `Binning` as `"1x1"`; other producers write a
/// bare number or `XBINNING`. The x factor is what AstroBin asks for.
fn binning_of(metadata: &serde_json::Value) -> Option<u32> {
    let text = match &metadata["Binning"] {
        serde_json::Value::String(text) => Some(text.clone()),
        serde_json::Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
    .or_else(|| metadata["XBINNING"].as_f64().map(|x| x.to_string()))?;
    let compact = text.replace(char::is_whitespace, "");
    let x = compact.split(['x', 'X']).next()?;
    let factor = x.parse::<f64>().ok()?;
    (factor >= 1.0 && factor.fract() == 0.0).then_some(factor as u32)
}

/// What one light's header and the calibration library add to every row of
/// its night and filter.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct NightFilterExtras {
    pub f_number: Option<f64>,
    pub darks: Option<usize>,
    pub flats: Option<usize>,
    pub flat_darks: Option<usize>,
    pub bias: Option<usize>,
}

/// Extras keyed by `(date, filter)`.
pub type NightFilterExtrasMap = HashMap<(String, String), NightFilterExtras>;

/// One row of the CSV: the frames of one night, filter and exposure length
/// (and, in full detail, one binning and gain).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AstroBinRow {
    /// The night the frames belong to, as `YYYY-MM-DD`.
    pub date: String,
    /// The catalog's filter name; the CSV carries `filter_id` instead.
    pub filter: String,
    /// The AstroBin equipment id the filter name maps to, when it does.
    pub filter_id: Option<u32>,
    pub number: usize,
    /// Seconds, to four decimals.
    pub duration: f64,
    pub binning: Option<u32>,
    pub gain: Option<f64>,
    /// The mean sensor temperature, to the degree.
    pub sensor_cooling: Option<i32>,
    pub f_number: Option<f64>,
    pub darks: Option<usize>,
    pub flats: Option<usize>,
    pub flat_darks: Option<usize>,
    pub bias: Option<usize>,
    /// The mean ambient temperature, to two decimals.
    pub temperature: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct RowKey {
    date: String,
    filter: String,
    /// Duration in units of 0.0001 s, the CSV's own precision.
    duration_key: i64,
    binning: Option<u32>,
    /// Gain in hundredths, the CSV's own precision.
    gain_key: Option<i64>,
}

#[derive(Default)]
struct RowTally {
    number: usize,
    sensor_temps: Vec<f64>,
    ambient_temps: Vec<f64>,
}

/// Fold frames into CSV rows. `boundary` is where the catalog's nights
/// split (see [`night_boundary`]); `extras` is keyed by `(date, filter)`.
pub fn build_rows(
    frames: &[AstroBinFrame],
    boundary: i64,
    detail: AstroBinDetail,
    filter_ids: &BTreeMap<String, u32>,
    extras: &NightFilterExtrasMap,
) -> Vec<AstroBinRow> {
    let mut tallies: BTreeMap<RowKey, RowTally> = BTreeMap::new();
    for frame in frames {
        let key = RowKey {
            date: night_of(frame.captured_at, boundary),
            filter: frame.filter.clone(),
            duration_key: (frame.exposure_s * 10_000.0).round() as i64,
            binning: match detail {
                AstroBinDetail::Full => frame.binning,
                AstroBinDetail::Essentials => None,
            },
            gain_key: match detail {
                AstroBinDetail::Full => frame.gain.map(|gain| (gain * 100.0).round() as i64),
                AstroBinDetail::Essentials => None,
            },
        };
        let tally = tallies.entry(key).or_default();
        tally.number += 1;
        tally.sensor_temps.extend(frame.sensor_temp);
        tally.ambient_temps.extend(frame.ambient_temp);
    }

    tallies
        .into_iter()
        .map(|(key, tally)| {
            let full = detail == AstroBinDetail::Full;
            let extra = extras
                .get(&(key.date.clone(), key.filter.clone()))
                .copied()
                .unwrap_or_default();
            AstroBinRow {
                filter_id: filter_ids.get(&key.filter).copied(),
                number: tally.number,
                duration: key.duration_key as f64 / 10_000.0,
                binning: key.binning,
                gain: key.gain_key.map(|gain| gain as f64 / 100.0),
                sensor_cooling: full
                    .then(|| mean(&tally.sensor_temps).map(|temp| temp.round() as i32))
                    .flatten(),
                f_number: full.then_some(extra.f_number).flatten(),
                darks: full.then_some(extra.darks).flatten(),
                flats: full.then_some(extra.flats).flatten(),
                flat_darks: full.then_some(extra.flat_darks).flatten(),
                bias: full.then_some(extra.bias).flatten(),
                temperature: full
                    .then(|| mean(&tally.ambient_temps).map(|temp| (temp * 100.0).round() / 100.0))
                    .flatten(),
                date: key.date,
                filter: key.filter,
            }
        })
        .collect()
}

fn mean(values: &[f64]) -> Option<f64> {
    (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64)
}

/// A number with at most `max_decimals` places and no trailing zeros, the
/// way AstroBin's importer reads it.
pub fn decimal(value: f64, max_decimals: usize) -> String {
    let text = format!("{value:.max_decimals$}");
    if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        text
    }
}

fn cell<T: ToString>(value: Option<T>) -> String {
    value.map(|value| value.to_string()).unwrap_or_default()
}

/// The CSV AstroBin imports. Essentials carries the four columns every
/// upload needs; full carries every column the export can fill. No value
/// ever needs quoting: dates, ids and numbers only.
pub fn render_csv(rows: &[AstroBinRow], detail: AstroBinDetail) -> String {
    let mut out = String::new();
    match detail {
        AstroBinDetail::Essentials => out.push_str("date,filter,number,duration\n"),
        AstroBinDetail::Full => out.push_str(
            "date,filter,number,duration,binning,gain,sensorCooling,fNumber,darks,flats,flatDarks,bias,temperature\n",
        ),
    }
    for row in rows {
        let essentials = [
            row.date.clone(),
            cell(row.filter_id),
            row.number.to_string(),
            decimal(row.duration, 4),
        ];
        let line = match detail {
            AstroBinDetail::Essentials => essentials.join(","),
            AstroBinDetail::Full => {
                let mut cells = essentials.to_vec();
                cells.extend([
                    cell(row.binning),
                    cell(row.gain.map(|gain| decimal(gain, 2))),
                    cell(row.sensor_cooling),
                    cell(row.f_number.map(|f| decimal(f, 2))),
                    cell(row.darks),
                    cell(row.flats),
                    cell(row.flat_darks),
                    cell(row.bias),
                    cell(row.temperature.map(|temp| decimal(temp, 2))),
                ]);
                cells.join(",")
            }
        };
        out.push_str(&line);
        out.push('\n');
    }
    out
}

/// What to export.
#[derive(Debug, Clone, Default)]
pub struct AstroBinExportRequest {
    pub project_id: Option<i32>,
    pub target_id: Option<i32>,
    /// Count ungraded lights along with accepted ones. Rejects never count.
    pub include_pending: bool,
    pub detail: AstroBinDetail,
}

/// The export: the rows, the CSV they render to, and what to tell the
/// person about it.
#[derive(Debug, Clone, Serialize)]
pub struct AstroBinExport {
    /// A file name for the download, from the scope's name.
    pub filename: String,
    pub detail: AstroBinDetail,
    /// The target or project the rows describe.
    pub scope: String,
    pub rows: Vec<AstroBinRow>,
    pub csv: String,
    pub frames: usize,
    pub nights: usize,
    pub total_exposure_seconds: f64,
    /// Filter names with no AstroBin id yet; their rows have a blank
    /// `filter` cell until the person maps them.
    pub unmapped_filters: Vec<String>,
    /// Lights whose file was not found, so their night and filter took its
    /// f-number and calibration counts from another light, or left them
    /// blank.
    pub lights_missing_files: usize,
    pub notes: Vec<String>,
}

/// Build the export for one target, one project, or the whole catalog.
///
/// `directory_tree` is needed only for full detail, where one light per
/// night and filter is read from disk (header only) for its f-number and
/// matched against the calibration library the way a stack build would.
pub fn export(
    conn: &Connection,
    directory_tree: Option<&DirectoryTree>,
    filter_ids: &BTreeMap<String, u32>,
    request: &AstroBinExportRequest,
) -> Result<AstroBinExport> {
    let db = crate::db::Database::new(conn);
    let rows = db
        .query_images_scoped(None, request.project_id, request.target_id, None, 0)
        .context("querying lights")?;

    let scope = rows
        .first()
        .map(|(_, project_name, target_name)| {
            if request.target_id.is_some() {
                target_name.clone()
            } else if request.project_id.is_some() {
                project_name.clone()
            } else {
                "catalog".to_string()
            }
        })
        .unwrap_or_else(|| "export".to_string());

    let counted = |image: &AcquiredImage| {
        image.grading_status == GradingStatus::Accepted as i32
            || (request.include_pending && image.grading_status == GradingStatus::Pending as i32)
    };
    let mut frames = Vec::new();
    let mut basenames: Vec<(AstroBinFrame, Option<String>)> = Vec::new();
    for (image, _, _) in rows.iter().filter(|(image, _, _)| counted(image)) {
        if let Some(frame) = AstroBinFrame::from_image(image) {
            frames.push(frame.clone());
            basenames.push((frame, crate::utils::extract_filename(&image.metadata)));
        }
    }

    // Nights split where the whole catalog is quiet, as the Sky view's do,
    // so one target's rows agree with every other's.
    let boundary = night_boundary(catalog_capture_times(conn)?);

    let mut notes = Vec::new();
    let mut lights_missing_files = 0usize;
    let mut extras = NightFilterExtrasMap::new();
    if request.detail == AstroBinDetail::Full {
        match directory_tree {
            Some(tree) => {
                let (found, missing) = night_filter_extras(conn, tree, &basenames, boundary)?;
                extras = found;
                lights_missing_files = missing;
            }
            None => notes.push(
                "No image folders were given, so the f-number and calibration counts are blank."
                    .to_string(),
            ),
        }
    }

    let rows = build_rows(&frames, boundary, request.detail, filter_ids, &extras);
    let csv = render_csv(&rows, request.detail);
    let nights: BTreeSet<&str> = rows.iter().map(|row| row.date.as_str()).collect();
    let unmapped_filters: BTreeSet<String> = rows
        .iter()
        .filter(|row| row.filter_id.is_none())
        .map(|row| row.filter.clone())
        .collect();
    let total_exposure_seconds = rows
        .iter()
        .map(|row| row.number as f64 * row.duration)
        .sum();

    Ok(AstroBinExport {
        filename: format!(
            "astrobin-{}-{}.csv",
            file_stem(&scope),
            request.detail.as_str()
        ),
        detail: request.detail,
        scope,
        frames: frames.len(),
        nights: nights.len(),
        total_exposure_seconds,
        unmapped_filters: unmapped_filters.into_iter().collect(),
        lights_missing_files,
        notes,
        rows,
        csv,
    })
}

/// Every light's capture time, for the night split.
fn catalog_capture_times(conn: &Connection) -> Result<Vec<i64>> {
    let mut statement = conn
        .prepare("SELECT acquireddate FROM acquiredimage WHERE acquireddate IS NOT NULL")
        .context("preparing capture time query")?;
    let times = statement
        .query_map([], |row| row.get::<_, i64>(0))
        .context("querying capture times")?
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("reading capture times")?;
    Ok(times)
}

/// Read one representative light per night and filter for its f-number and
/// its calibration match. The median frame stands for its group; when its
/// file is missing the walk moves outward so one lost file does not blank a
/// night. Returns the extras and how many lights had no file at all.
fn night_filter_extras(
    conn: &Connection,
    tree: &DirectoryTree,
    frames: &[(AstroBinFrame, Option<String>)],
    boundary: i64,
) -> Result<(NightFilterExtrasMap, usize)> {
    // (capture time, file basename) per night and filter.
    type Lights<'a> = Vec<(i64, Option<&'a str>)>;
    let mut groups: BTreeMap<(String, String), Lights> = BTreeMap::new();
    for (frame, basename) in frames {
        groups
            .entry((night_of(frame.captured_at, boundary), frame.filter.clone()))
            .or_default()
            .push((frame.captured_at, basename.as_deref()));
    }

    let mut extras = NightFilterExtrasMap::new();
    let mut missing = 0usize;
    for (key, mut lights) in groups {
        lights.sort();
        let middle = lights.len() / 2;
        let mut order: Vec<usize> = (0..lights.len()).collect();
        order.sort_by_key(|index| index.abs_diff(middle));
        let path = order.iter().find_map(|index| {
            let basename = lights[*index].1?;
            tree.find_file_first(basename).cloned()
        });
        let Some(path) = path else {
            missing += lights.len();
            continue;
        };
        let meta = crate::commands::import::headers::read_frame_meta(&path);
        let selection = crate::calibration::select_for_light(conn, &meta)
            .with_context(|| format!("matching calibration for {}", path.display()))?;
        let counts = crate::calibration::master_input_counts(&selection);
        extras.insert(
            key,
            NightFilterExtras {
                f_number: meta.f_number(),
                darks: Some(counts.dark),
                flats: Some(counts.flat),
                flat_darks: Some(counts.dark_flat),
                bias: Some(counts.bias),
            },
        );
    }
    Ok((extras, missing))
}

/// A file-name-safe stem from a target or project name.
fn file_stem(name: &str) -> String {
    let mut stem = String::new();
    let mut last_dash = true;
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            stem.push(ch);
            last_dash = false;
        } else if !last_dash {
            stem.push('-');
            last_dash = true;
        }
    }
    let stem = stem.trim_end_matches('-').to_string();
    if stem.is_empty() {
        "export".to_string()
    } else {
        stem
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(captured_at: i64, filter: &str, exposure_s: f64) -> AstroBinFrame {
        AstroBinFrame {
            captured_at,
            filter: filter.to_string(),
            exposure_s,
            gain: Some(100.0),
            binning: Some(1),
            sensor_temp: Some(-10.0),
            ambient_temp: Some(12.5),
        }
    }

    // 2026-04-16 22:00 UTC.
    const EVENING: i64 = 1_776_376_800;
    const HOUR: i64 = 3_600;

    #[test]
    fn essentials_merge_across_gain_and_binning_and_keep_a_night_together() {
        let mut late = frame(EVENING + 5 * HOUR, "L", 300.0);
        late.gain = Some(200.0);
        late.binning = Some(2);
        let frames = vec![
            frame(EVENING, "L", 300.0),
            frame(EVENING + HOUR, "L", 300.0),
            late,
            frame(EVENING + 2 * HOUR, "Ha", 600.0),
        ];
        let boundary = night_boundary(frames.iter().map(|f| f.captured_at));
        let rows = build_rows(
            &frames,
            boundary,
            AstroBinDetail::Essentials,
            &BTreeMap::new(),
            &HashMap::new(),
        );
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].filter, "Ha");
        assert_eq!(rows[0].number, 1);
        assert_eq!(rows[1].filter, "L");
        assert_eq!(rows[1].number, 3);
        assert_eq!(rows[1].date, "2026-04-16");
        assert_eq!(rows[1].binning, None);
        assert_eq!(rows[1].gain, None);
        assert_eq!(rows[1].sensor_cooling, None);
    }

    #[test]
    fn full_splits_by_gain_and_binning_and_averages_temperatures() {
        let mut warm = frame(EVENING + HOUR, "L", 300.0);
        warm.sensor_temp = Some(-9.0);
        warm.ambient_temp = Some(13.0);
        let mut other_gain = frame(EVENING + 2 * HOUR, "L", 300.0);
        other_gain.gain = Some(200.0);
        let frames = vec![frame(EVENING, "L", 300.0), warm, other_gain];
        let mut extras = HashMap::new();
        extras.insert(
            ("2026-04-16".to_string(), "L".to_string()),
            NightFilterExtras {
                f_number: Some(6.9),
                darks: Some(30),
                flats: Some(25),
                flat_darks: Some(0),
                bias: Some(50),
            },
        );
        let mut ids = BTreeMap::new();
        ids.insert("L".to_string(), 4049);
        let boundary = night_boundary(frames.iter().map(|f| f.captured_at));
        let rows = build_rows(&frames, boundary, AstroBinDetail::Full, &ids, &extras);
        assert_eq!(rows.len(), 2);
        let gain_100 = &rows[0];
        assert_eq!(gain_100.gain, Some(100.0));
        assert_eq!(gain_100.number, 2);
        assert_eq!(gain_100.sensor_cooling, Some(-10));
        assert_eq!(gain_100.temperature, Some(12.75));
        assert_eq!(gain_100.f_number, Some(6.9));
        assert_eq!(gain_100.darks, Some(30));
        assert_eq!(gain_100.filter_id, Some(4049));
        assert_eq!(rows[1].gain, Some(200.0));
        assert_eq!(rows[1].number, 1);
        // The extras belong to the night and filter, so both rows carry them.
        assert_eq!(rows[1].bias, Some(50));
    }

    #[test]
    fn nights_split_where_the_catalog_is_quiet() {
        // A site far east captures around 10:00 UTC; its evening of the 16th
        // and small hours of the 17th (local) fall on one UTC date, and its
        // next session must land on the next night.
        let ten_utc = EVENING - 12 * HOUR;
        let frames = vec![
            frame(ten_utc, "L", 60.0),
            frame(ten_utc + 4 * HOUR, "L", 60.0),
            frame(ten_utc + 24 * HOUR, "L", 60.0),
        ];
        let boundary = night_boundary(frames.iter().map(|f| f.captured_at));
        let rows = build_rows(
            &frames,
            boundary,
            AstroBinDetail::Essentials,
            &BTreeMap::new(),
            &HashMap::new(),
        );
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].number, 2);
        assert_eq!(rows[1].number, 1);
        assert_ne!(rows[0].date, rows[1].date);
    }

    #[test]
    fn decimals_trim_trailing_zeros() {
        assert_eq!(decimal(300.0, 4), "300");
        assert_eq!(decimal(0.5, 4), "0.5");
        assert_eq!(decimal(30.00005, 4), "30.0001");
        assert_eq!(decimal(6.9, 2), "6.9");
        assert_eq!(decimal(12.75, 2), "12.75");
    }

    #[test]
    fn csv_carries_ids_not_names_and_blanks_unknowns() {
        let rows = vec![
            AstroBinRow {
                date: "2026-04-16".into(),
                filter: "L".into(),
                filter_id: Some(4049),
                number: 12,
                duration: 300.0,
                binning: Some(1),
                gain: Some(100.0),
                sensor_cooling: Some(-10),
                f_number: Some(6.9),
                darks: Some(30),
                flats: Some(25),
                flat_darks: None,
                bias: Some(50),
                temperature: Some(12.75),
            },
            AstroBinRow {
                date: "2026-04-16".into(),
                filter: "Ha".into(),
                filter_id: None,
                number: 4,
                duration: 600.5,
                binning: None,
                gain: None,
                sensor_cooling: None,
                f_number: None,
                darks: None,
                flats: None,
                flat_darks: None,
                bias: None,
                temperature: None,
            },
        ];
        assert_eq!(
            render_csv(&rows, AstroBinDetail::Essentials),
            "date,filter,number,duration\n2026-04-16,4049,12,300\n2026-04-16,,4,600.5\n"
        );
        assert_eq!(
            render_csv(&rows, AstroBinDetail::Full),
            "date,filter,number,duration,binning,gain,sensorCooling,fNumber,darks,flats,flatDarks,bias,temperature\n\
             2026-04-16,4049,12,300,1,100,-10,6.9,30,25,,50,12.75\n\
             2026-04-16,,4,600.5,,,,,,,,,\n"
        );
    }

    #[test]
    fn frames_read_target_scheduler_metadata() {
        let image = AcquiredImage {
            id: 1,
            project_id: 1,
            target_id: 1,
            acquired_date: Some(EVENING),
            filter_name: "OIII".into(),
            grading_status: 1,
            metadata: r#"{"ExposureDuration":600.0,"Gain":100,"Binning":"1x1","CameraTemp":-5.0,"FocuserTemp":15.31}"#.into(),
            reject_reason: None,
            profile_id: None,
            guid: None,
        };
        let frame = AstroBinFrame::from_image(&image).unwrap();
        assert_eq!(frame.filter, "OIII");
        assert_eq!(frame.exposure_s, 600.0);
        assert_eq!(frame.gain, Some(100.0));
        assert_eq!(frame.binning, Some(1));
        assert_eq!(frame.sensor_temp, Some(-5.0));
        assert_eq!(frame.ambient_temp, Some(15.31));

        let bare = AcquiredImage {
            metadata: r#"{"EXPTIME":"30","XBINNING":2}"#.into(),
            ..image.clone()
        };
        let frame = AstroBinFrame::from_image(&bare).unwrap();
        assert_eq!(frame.exposure_s, 30.0);
        assert_eq!(frame.binning, Some(2));

        let no_exposure = AcquiredImage {
            metadata: "{}".into(),
            ..image
        };
        assert!(AstroBinFrame::from_image(&no_exposure).is_none());
    }

    #[test]
    fn file_stems_are_safe() {
        assert_eq!(file_stem("NGC 7023 (Iris)"), "NGC-7023-Iris");
        assert_eq!(file_stem("///"), "export");
    }

    fn catalog() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE project (Id INTEGER PRIMARY KEY, profileId TEXT, name TEXT NOT NULL, description TEXT);
             CREATE TABLE target (Id INTEGER PRIMARY KEY, projectId INTEGER NOT NULL, name TEXT NOT NULL, active INTEGER NOT NULL DEFAULT 1, ra REAL, dec REAL);
             CREATE TABLE acquiredimage (Id INTEGER PRIMARY KEY, projectId INTEGER NOT NULL, targetId INTEGER NOT NULL, acquireddate INTEGER, filtername TEXT NOT NULL, gradingStatus INTEGER NOT NULL DEFAULT 0, metadata TEXT NOT NULL DEFAULT '{}', rejectreason TEXT, profileId TEXT);
             INSERT INTO project VALUES (1, 'p', 'Autumn', NULL);
             INSERT INTO target VALUES (1, 1, 'NGC 7023', 1, NULL, NULL), (2, 1, 'M31', 1, NULL, NULL);",
        )
        .unwrap();
        let mut insert = conn
            .prepare("INSERT INTO acquiredimage (projectId, targetId, acquireddate, filtername, gradingStatus, metadata) VALUES (1, ?1, ?2, ?3, ?4, ?5)")
            .unwrap();
        let meta = |seconds: f64| {
            format!(
                r#"{{"ExposureDuration":{seconds},"Gain":100,"Binning":"1x1","CameraTemp":-10}}"#
            )
        };
        // Target 1: two accepted L, one pending L, one rejected L, one accepted Ha.
        insert
            .execute(rusqlite::params![1, EVENING, "L", 1, meta(300.0)])
            .unwrap();
        insert
            .execute(rusqlite::params![1, EVENING + HOUR, "L", 1, meta(300.0)])
            .unwrap();
        insert
            .execute(rusqlite::params![
                1,
                EVENING + 2 * HOUR,
                "L",
                0,
                meta(300.0)
            ])
            .unwrap();
        insert
            .execute(rusqlite::params![
                1,
                EVENING + 3 * HOUR,
                "L",
                2,
                meta(300.0)
            ])
            .unwrap();
        insert
            .execute(rusqlite::params![
                1,
                EVENING + 4 * HOUR,
                "Ha",
                1,
                meta(600.0)
            ])
            .unwrap();
        // Target 2, the next night.
        insert
            .execute(rusqlite::params![
                2,
                EVENING + 24 * HOUR,
                "L",
                1,
                meta(120.0)
            ])
            .unwrap();
        drop(insert);
        conn
    }

    #[test]
    fn export_counts_accepted_lights_and_optionally_pending_ones() {
        let conn = catalog();
        let mut ids = BTreeMap::new();
        ids.insert("L".to_string(), 4049);
        let request = AstroBinExportRequest {
            target_id: Some(1),
            ..Default::default()
        };
        let export = export(&conn, None, &ids, &request).unwrap();
        assert_eq!(export.scope, "NGC 7023");
        assert_eq!(export.filename, "astrobin-NGC-7023-essentials.csv");
        assert_eq!(export.frames, 3);
        assert_eq!(export.nights, 1);
        assert_eq!(export.total_exposure_seconds, 1200.0);
        assert_eq!(export.unmapped_filters, vec!["Ha".to_string()]);
        assert_eq!(
            export.csv,
            "date,filter,number,duration\n2026-04-16,,1,600\n2026-04-16,4049,2,300\n"
        );

        let export = export_with(&conn, &ids, true, AstroBinDetail::Essentials);
        assert_eq!(export.frames, 4);
        assert_eq!(export.rows[1].number, 3);
    }

    #[test]
    fn full_detail_without_folders_says_so() {
        let conn = catalog();
        let export = export_with(&conn, &BTreeMap::new(), false, AstroBinDetail::Full);
        assert_eq!(export.filename, "astrobin-NGC-7023-full.csv");
        assert_eq!(export.notes.len(), 1);
        assert_eq!(export.rows[1].sensor_cooling, Some(-10));
        assert_eq!(export.rows[1].gain, Some(100.0));
        assert_eq!(export.rows[1].darks, None);
        assert!(export
            .csv
            .starts_with("date,filter,number,duration,binning,"));
    }

    #[test]
    fn project_scope_spans_targets_and_nights() {
        let conn = catalog();
        let request = AstroBinExportRequest {
            project_id: Some(1),
            ..Default::default()
        };
        let export = export(&conn, None, &BTreeMap::new(), &request).unwrap();
        assert_eq!(export.scope, "Autumn");
        assert_eq!(export.nights, 2);
        assert_eq!(export.frames, 4);
    }

    fn export_with(
        conn: &Connection,
        ids: &BTreeMap<String, u32>,
        include_pending: bool,
        detail: AstroBinDetail,
    ) -> AstroBinExport {
        export(
            conn,
            None,
            ids,
            &AstroBinExportRequest {
                target_id: Some(1),
                include_pending,
                detail,
                ..Default::default()
            },
        )
        .unwrap()
    }
}
