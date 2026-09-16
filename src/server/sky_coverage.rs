//! Sky coverage: where a catalog's lights pointed, how long they looked, and
//! on which nights. One response per database; the Sky page merges them.
//!
//! Coordinates leave here in ICRS degrees. The Target Scheduler stores right
//! ascension in hours, and this is the boundary where that is converted.
//!
//! A target's footprint on the sky comes from the best source at hand: a
//! pixel-derived plate solution of one of its frames when the astrometry
//! cache holds one, else the sensor geometry read from one frame's header per
//! capture profile together with the planned rotation, else nothing but the
//! target's coordinates. The response says which.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use axum::Json;
use rusqlite::Connection;
use serde::Serialize;

use super::api::ApiResponse;
use super::database_context::DatabaseContext;
use super::extract::DbContext;
use super::handlers::AppError;

#[derive(Debug, Clone, Serialize)]
pub struct SkyCoverageResponse {
    pub targets: Vec<SkyTarget>,
    /// One row per night, target, and filter, in capture order.
    pub nights: Vec<SkyNight>,
    /// Every filter name seen, in first-seen order.
    pub filters: Vec<String>,
    pub totals: SkyTotals,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct SkyTotals {
    pub frames: i64,
    pub accepted_frames: i64,
    pub seconds: f64,
    pub accepted_seconds: f64,
    pub nights: i64,
    pub first_capture: Option<i64>,
    pub last_capture: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkyTarget {
    pub id: i32,
    pub name: String,
    pub project_id: i32,
    pub project_name: String,
    pub ra_deg: Option<f64>,
    pub dec_deg: Option<f64>,
    /// The rotation the scheduler planned for the camera, in degrees.
    pub rotation_deg: Option<f64>,
    pub footprint: Option<SkyFootprint>,
    pub frames: i64,
    pub accepted_frames: i64,
    pub seconds: f64,
    pub accepted_seconds: f64,
    pub nights: i64,
    pub first_capture: Option<i64>,
    pub last_capture: Option<i64>,
    pub filters: Vec<SkyFilterTotal>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkyFilterTotal {
    pub filter: String,
    pub frames: i64,
    pub accepted_frames: i64,
    pub seconds: f64,
    pub accepted_seconds: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkyFootprint {
    pub width_deg: f64,
    pub height_deg: f64,
    /// Camera angle east of north, when known. A solved footprint carries
    /// its vertices instead.
    pub rotation_deg: Option<f64>,
    /// `solved` from a plate solution, `header` from a frame's FITS geometry.
    pub source: &'static str,
    /// ICRS vertices in boundary order, for a solved footprint.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vertices: Option<Vec<[f64; 2]>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkyNight {
    /// The civil date the night began on, `YYYY-MM-DD`.
    pub night: String,
    pub target_id: i32,
    pub filter: String,
    pub frames: i64,
    pub accepted_frames: i64,
    pub seconds: f64,
    pub accepted_seconds: f64,
}

/// A light as the coverage pass needs it.
struct LightRow {
    id: i32,
    target_id: i32,
    captured_at: Option<i64>,
    filter: String,
    accepted: bool,
    seconds: f64,
    profile: Option<String>,
    file_name: Option<String>,
}

struct TargetRow {
    id: i32,
    name: String,
    project_id: i32,
    project_name: String,
    ra_hours: Option<f64>,
    dec_deg: Option<f64>,
    rotation_deg: Option<f64>,
}

/// GET /api/db/{db}/sky/coverage
pub async fn get_sky_coverage(
    ctx: DbContext,
) -> Result<Json<ApiResponse<SkyCoverageResponse>>, AppError> {
    let (targets, lights) = {
        let conn = ctx.db();
        let conn = conn.lock().map_err(AppError::db)?;
        (
            load_targets(&conn).map_err(AppError::db)?,
            load_lights(&conn).map_err(AppError::db)?,
        )
    };
    let context = ctx.0.clone();
    let response = tokio::task::spawn_blocking(move || {
        let footprints = footprints_for(&context, &targets, &lights);
        assemble(targets, lights, footprints)
    })
    .await
    .map_err(|error| AppError::InternalError(format!("sky coverage task failed: {error}")))?;
    Ok(Json(ApiResponse::success(response)))
}

/// Whether a table has a column, for the optional ones older schemas lack.
fn has_column(conn: &Connection, table: &str, column: &str) -> bool {
    conn.prepare(&format!("SELECT {column} FROM {table} LIMIT 0"))
        .is_ok()
}

fn load_targets(conn: &Connection) -> rusqlite::Result<Vec<TargetRow>> {
    // `rotation` arrived with a later Target Scheduler schema; a catalog
    // without it has no planned rotation to report.
    let rotation = if has_column(conn, "target", "rotation") {
        "t.rotation"
    } else {
        "NULL"
    };
    let mut statement = conn.prepare(&format!(
        "SELECT t.Id, t.name, t.projectid, p.name, t.ra, t.dec, {rotation}
         FROM target t
         JOIN project p ON p.Id = t.projectid
         ORDER BY t.Id"
    ))?;
    let rows = statement.query_map([], |row| {
        Ok(TargetRow {
            id: row.get(0)?,
            name: row.get(1)?,
            project_id: row.get(2)?,
            project_name: row.get(3)?,
            ra_hours: row.get(4)?,
            dec_deg: row.get(5)?,
            rotation_deg: row.get(6)?,
        })
    })?;
    rows.collect()
}

fn load_lights(conn: &Connection) -> rusqlite::Result<Vec<LightRow>> {
    let profile_column = if has_column(conn, "acquiredimage", "profileId") {
        "profileId"
    } else {
        "NULL"
    };
    let mut statement = conn.prepare(&format!(
        "SELECT Id, targetId, acquireddate, filtername, gradingStatus, metadata, {profile_column}
         FROM acquiredimage
         ORDER BY acquireddate, Id"
    ))?;
    let rows = statement.query_map([], |row| {
        let metadata: String = row.get(5)?;
        let grading: i32 = row.get(4)?;
        Ok(LightRow {
            id: row.get(0)?,
            target_id: row.get(1)?,
            captured_at: row.get(2)?,
            filter: row.get(3)?,
            accepted: grading == 1,
            seconds: super::exposure_groups::exposure_seconds_from_metadata(&metadata)
                .unwrap_or(0.0),
            profile: row.get(6)?,
            file_name: super::handlers::filename_from_metadata(&metadata),
        })
    })?;
    rows.collect()
}

/// The civil date a capture's night began on: local evenings and the small
/// hours that follow them share one key. Twelve hours back from UTC lands
/// every capture between local noon and local noon on the evening's date
/// for any site within about eight hours of Greenwich, which covers the
/// nights this catalog was built for without needing the site.
pub fn night_of(captured_at: i64) -> String {
    chrono::DateTime::from_timestamp(captured_at - 12 * 3600, 0)
        .map(|at| at.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|| captured_at.to_string())
}

fn assemble(
    targets: Vec<TargetRow>,
    lights: Vec<LightRow>,
    footprints: HashMap<i32, SkyFootprint>,
) -> SkyCoverageResponse {
    #[derive(Default)]
    struct Tally {
        frames: i64,
        accepted_frames: i64,
        seconds: f64,
        accepted_seconds: f64,
    }
    impl Tally {
        fn add(&mut self, light: &LightRow) {
            self.frames += 1;
            self.seconds += light.seconds;
            if light.accepted {
                self.accepted_frames += 1;
                self.accepted_seconds += light.seconds;
            }
        }
    }

    let mut filters_seen: Vec<String> = Vec::new();
    let mut per_target: HashMap<i32, Tally> = HashMap::new();
    let mut per_target_filter: HashMap<i32, BTreeMap<String, Tally>> = HashMap::new();
    let mut per_target_nights: HashMap<i32, HashSet<String>> = HashMap::new();
    let mut per_target_span: HashMap<i32, (i64, i64)> = HashMap::new();
    let mut nights: BTreeMap<(String, i32, String), Tally> = BTreeMap::new();
    let mut totals = SkyTotals::default();
    let mut all_nights: HashSet<String> = HashSet::new();

    for light in &lights {
        if !filters_seen.contains(&light.filter) {
            filters_seen.push(light.filter.clone());
        }
        per_target.entry(light.target_id).or_default().add(light);
        per_target_filter
            .entry(light.target_id)
            .or_default()
            .entry(light.filter.clone())
            .or_default()
            .add(light);
        totals.frames += 1;
        totals.seconds += light.seconds;
        if light.accepted {
            totals.accepted_frames += 1;
            totals.accepted_seconds += light.seconds;
        }
        if let Some(at) = light.captured_at {
            let night = night_of(at);
            all_nights.insert(night.clone());
            per_target_nights
                .entry(light.target_id)
                .or_default()
                .insert(night.clone());
            nights
                .entry((night, light.target_id, light.filter.clone()))
                .or_default()
                .add(light);
            per_target_span
                .entry(light.target_id)
                .and_modify(|(first, last)| {
                    *first = (*first).min(at);
                    *last = (*last).max(at);
                })
                .or_insert((at, at));
            totals.first_capture = Some(totals.first_capture.map_or(at, |first| first.min(at)));
            totals.last_capture = Some(totals.last_capture.map_or(at, |last| last.max(at)));
        }
    }
    totals.nights = all_nights.len() as i64;

    let mut footprints = footprints;
    let targets = targets
        .into_iter()
        .map(|target| {
            let tally = per_target.remove(&target.id).unwrap_or_default();
            let span = per_target_span.get(&target.id).copied();
            let (ra_deg, dec_deg) = match (target.ra_hours, target.dec_deg) {
                (Some(ra), Some(dec)) => crate::astrometry::target_scheduler_coordinates(ra, dec)
                    .map_or((None, None), |(ra, dec)| (Some(ra), Some(dec))),
                _ => (None, None),
            };
            SkyTarget {
                id: target.id,
                name: target.name,
                project_id: target.project_id,
                project_name: target.project_name,
                ra_deg,
                dec_deg,
                rotation_deg: target.rotation_deg.filter(|value| value.is_finite()),
                footprint: footprints.remove(&target.id),
                frames: tally.frames,
                accepted_frames: tally.accepted_frames,
                seconds: tally.seconds,
                accepted_seconds: tally.accepted_seconds,
                nights: per_target_nights
                    .get(&target.id)
                    .map_or(0, |nights| nights.len() as i64),
                first_capture: span.map(|(first, _)| first),
                last_capture: span.map(|(_, last)| last),
                filters: per_target_filter
                    .remove(&target.id)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(filter, tally)| SkyFilterTotal {
                        filter,
                        frames: tally.frames,
                        accepted_frames: tally.accepted_frames,
                        seconds: tally.seconds,
                        accepted_seconds: tally.accepted_seconds,
                    })
                    .collect(),
            }
        })
        .collect();

    SkyCoverageResponse {
        targets,
        nights: nights
            .into_iter()
            .map(|((night, target_id, filter), tally)| SkyNight {
                night,
                target_id,
                filter,
                frames: tally.frames,
                accepted_frames: tally.accepted_frames,
                seconds: tally.seconds,
                accepted_seconds: tally.accepted_seconds,
            })
            .collect(),
        filters: filters_seen,
        totals,
    }
}

/// Sensor geometry read once per frame file and kept for the process: the
/// same profile's frames share it, and headers on network storage are slow.
#[derive(Debug, Clone, Copy)]
struct HeaderGeometry {
    width_deg: f64,
    height_deg: f64,
}

fn header_geometry_cache() -> &'static Mutex<HashMap<PathBuf, Option<HeaderGeometry>>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Option<HeaderGeometry>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn header_geometry(path: &Path) -> Option<HeaderGeometry> {
    if let Some(known) = header_geometry_cache()
        .lock()
        .ok()
        .and_then(|cache| cache.get(path).copied())
    {
        return known;
    }
    let geometry = crate::astrometry_headers::FitsAstrometryHeaders::from_path(path)
        .ok()
        .and_then(|headers| {
            let scale = headers.pixel_scale_arcsec_per_pixel?.value;
            let width = f64::from(headers.width?.value);
            let height = f64::from(headers.height?.value);
            (scale.is_finite() && scale > 0.0 && width > 0.0 && height > 0.0).then(|| {
                HeaderGeometry {
                    width_deg: width * scale / 3600.0,
                    height_deg: height * scale / 3600.0,
                }
            })
        });
    if let Ok(mut cache) = header_geometry_cache().lock() {
        cache.insert(path.to_path_buf(), geometry);
    }
    geometry
}

/// Frame ids with a plate solution on disk, from one listing of the
/// astrometry cache rather than one probe per frame.
fn solved_image_ids(cache_dir: &Path) -> HashSet<i32> {
    let Ok(entries) = std::fs::read_dir(cache_dir.join("astrometry")) else {
        return HashSet::new();
    };
    entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let name = entry.file_name();
            let name = name.to_str()?;
            name.strip_suffix(".json")?.parse().ok()
        })
        .collect()
}

fn footprints_for(
    context: &Arc<DatabaseContext>,
    targets: &[TargetRow],
    lights: &[LightRow],
) -> HashMap<i32, SkyFootprint> {
    let mut footprints = HashMap::new();
    let cache_dir = context.cache_dir_path.clone();
    let solved = solved_image_ids(&cache_dir);

    // Newest accepted frame with a solution, per target.
    if !solved.is_empty() {
        for target in targets {
            let candidate = lights
                .iter()
                .rev()
                .filter(|light| light.target_id == target.id && solved.contains(&light.id))
                .max_by_key(|light| (light.accepted, light.captured_at));
            let Some(light) = candidate else {
                continue;
            };
            let Some(analysis) = crate::astrometry::persisted_pixel_analysis(&cache_dir, light.id)
            else {
                continue;
            };
            let Some(solution) = analysis.solution else {
                continue;
            };
            if solution.footprint.len() < 3 {
                continue;
            }
            let scale = solution.pixel_scale_arcsec_per_pixel / 3600.0;
            footprints.insert(
                target.id,
                SkyFootprint {
                    width_deg: f64::from(solution.image_width) * scale,
                    height_deg: f64::from(solution.image_height) * scale,
                    rotation_deg: None,
                    source: "solved",
                    vertices: Some(solution.footprint),
                },
            );
        }
    }

    // Header geometry for the rest, one file read per capture profile.
    let remaining: Vec<&TargetRow> = targets
        .iter()
        .filter(|target| !footprints.contains_key(&target.id))
        .collect();
    if remaining.is_empty() {
        return footprints;
    }
    let Ok(tree) = context.get_directory_tree() else {
        return footprints;
    };
    let mut by_profile: HashMap<Option<String>, Option<HeaderGeometry>> = HashMap::new();
    for target in remaining {
        // The profile most of this target's frames were shot under, and the
        // newest frame under it whose file can be found.
        let mut profile_counts: HashMap<Option<String>, usize> = HashMap::new();
        for light in lights.iter().filter(|light| light.target_id == target.id) {
            *profile_counts.entry(light.profile.clone()).or_default() += 1;
        }
        let Some((profile, _)) = profile_counts.into_iter().max_by_key(|(_, count)| *count) else {
            continue;
        };
        let geometry = *by_profile.entry(profile.clone()).or_insert_with(|| {
            lights
                .iter()
                .rev()
                .filter(|light| light.target_id == target.id && light.profile == profile)
                .filter_map(|light| light.file_name.as_deref())
                .filter_map(|name| tree.find_file_first(name).cloned())
                .take(3)
                .find_map(|path| header_geometry(&path))
        });
        if let Some(geometry) = geometry {
            footprints.insert(
                target.id,
                SkyFootprint {
                    width_deg: geometry.width_deg,
                    height_deg: geometry.height_deg,
                    rotation_deg: target.rotation_deg.filter(|value| value.is_finite()),
                    source: "header",
                    vertices: None,
                },
            );
        }
    }
    footprints
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_night_is_keyed_by_the_evening_it_began_on() {
        // 2026-09-09 05:44 UTC is 22:44 the evening before in the Pacific
        // zone, and 2026-09-09 10:50 UTC is 03:50 the same night.
        assert_eq!(night_of(1_788_932_675), "2026-09-08");
        assert_eq!(night_of(1_788_951_022), "2026-09-08");
        // Late afternoon UTC already belongs to that date's night.
        assert_eq!(night_of(1_788_975_600), "2026-09-09");
    }

    #[test]
    fn assembly_totals_frames_and_seconds_per_target_filter_and_night() {
        let targets = vec![
            TargetRow {
                id: 1,
                name: "M 31".into(),
                project_id: 1,
                project_name: "Autumn".into(),
                ra_hours: Some(0.712),
                dec_deg: Some(41.27),
                rotation_deg: Some(12.0),
            },
            TargetRow {
                id: 2,
                name: "Nothing yet".into(),
                project_id: 1,
                project_name: "Autumn".into(),
                ra_hours: None,
                dec_deg: None,
                rotation_deg: None,
            },
        ];
        let light = |id, at, filter: &str, accepted, seconds| LightRow {
            id,
            target_id: 1,
            captured_at: Some(at),
            filter: filter.into(),
            accepted,
            seconds,
            profile: None,
            file_name: None,
        };
        let night_one = 1_788_932_675; // 2026-09-08 night
        let night_two = night_one + 86_400;
        let lights = vec![
            light(1, night_one, "L", true, 300.0),
            light(2, night_one + 310, "L", false, 300.0),
            light(3, night_one + 620, "Ha", true, 600.0),
            light(4, night_two, "L", true, 300.0),
        ];
        let response = assemble(targets, lights, HashMap::new());

        assert_eq!(response.filters, vec!["L", "Ha"]);
        assert_eq!(response.totals.frames, 4);
        assert_eq!(response.totals.accepted_frames, 3);
        assert_eq!(response.totals.seconds, 1500.0);
        assert_eq!(response.totals.accepted_seconds, 1200.0);
        assert_eq!(response.totals.nights, 2);

        let m31 = &response.targets[0];
        assert_eq!((m31.ra_deg.unwrap() * 100.0).round() / 100.0, 10.68);
        assert_eq!(m31.dec_deg, Some(41.27));
        assert_eq!(m31.rotation_deg, Some(12.0));
        assert_eq!(m31.frames, 4);
        assert_eq!(m31.nights, 2);
        assert_eq!(m31.first_capture, Some(night_one));
        assert_eq!(m31.last_capture, Some(night_two));
        let luminance = m31.filters.iter().find(|f| f.filter == "L").unwrap();
        assert_eq!(luminance.frames, 3);
        assert_eq!(luminance.accepted_seconds, 600.0);

        let empty = &response.targets[1];
        assert_eq!(empty.frames, 0);
        assert_eq!(empty.ra_deg, None);
        assert!(empty.filters.is_empty());

        assert_eq!(
            response.nights.len(),
            3,
            "L and Ha on night one, L on night two"
        );
        let first = &response.nights[0];
        assert_eq!(first.night, "2026-09-08");
        assert_eq!((first.target_id, first.filter.as_str()), (1, "Ha"));
        let second = &response.nights[1];
        assert_eq!(
            (
                second.filter.as_str(),
                second.frames,
                second.accepted_frames
            ),
            ("L", 2, 1)
        );
    }
}
