//! Where a rig's nights split.
//!
//! Calibration groups a rig's frames by night: an evening's darks, the
//! flats shot at dusk and those shot at the dawn after it. A night runs from
//! one local noon at the frame's site to the next, the way N.I.N.A.'s
//! `DATEMINUS12` folders date it, so no dusk or dawn flat run straddles the
//! split. A night is named by the local date its evening began.
//!
//! Where a frame's local noon falls, first found wins:
//!
//! 1. the `night_zone` setting, rig then global ([`super::rules`]): noon at
//!    that UTC offset;
//! 2. the site longitude the frame records (SITELONG, from the N.I.N.A.
//!    profile): mean solar noon there, 12:00 UTC less four minutes per
//!    degree east;
//! 3. the frame's own clock, DATE-LOC less DATE-OBS: noon on that clock;
//! 4. the longitude its rig recorded nearest in time, from its other frames
//!    and every site an import saw it at;
//! 5. the clock its rig recorded nearest in time;
//! 6. its rig's newest site, for a frame with no capture time;
//! 7. the catalog's quiet hours, where Images splits nights
//!    ([`crate::server::sky_coverage::night_boundary`]).
//!
//! A frame's own site comes first and its rig's nearest in time next, so a
//! rig taken to a star party splits those nights where it was and keeps its
//! home nights, even for home frames whose file is away or whose header
//! names no site. The server's own time zone is never used: it often runs
//! somewhere else, or on UTC. Neither the setting nor a longitude moves
//! with daylight saving; a clock does, and a frame takes the one it, or its
//! nearest neighbour, was shot by. An hour either side of noon is still the
//! middle of the day, so no night notices.
//!
//! A split is carried as a shift, not a time of day: a night is the UTC
//! date of `at − shift`, with the shift `12 h − offset` left unwrapped, so
//! a site past +12 h (New Zealand in summer, Tonga) or close to 180° W
//! names its night by the local date of its evening.
//!
//! Sites are kept per frame on `psf_guard_calibration_frame`, per rig in
//! `psf_guard_rig_site` (the newest) and `psf_guard_rig_site_seen` (each
//! site with the span of captures it was seen over). An import records
//! them from each frame's header; [`backfill_frame_sites`] reads them for
//! frames imported before, as the server's frame-sites job.

use super::rules::rules_for_rig;
use crate::commands::import::headers::{parse_fits_datetime, FrameMeta};
use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

const NOON: i64 = 12 * 3_600;

/// Frames whose headers are read between commits.
pub const SITE_BACKFILL_CHUNK: usize = 100;

/// How long a frame whose file was away waits before it is read again.
const SITE_RETRY_SECONDS: i64 = 7 * 86_400;

/// Tries of a commit the catalog was too busy for, the first after this
/// wait, doubling.
const COMMIT_TRIES: u32 = 5;
const COMMIT_WAIT: Duration = Duration::from_millis(200);

pub(crate) const SITE_TABLE_SQL: &str = r#"
    CREATE TABLE IF NOT EXISTS psf_guard_rig_site (
        rig_uuid          TEXT PRIMARY KEY,
        longitude         REAL,
        utc_offset_s      INTEGER,
        frame_captured_at INTEGER,
        read_at           INTEGER NOT NULL
    );
    CREATE TABLE IF NOT EXISTS psf_guard_rig_site_seen (
        rig_uuid     TEXT NOT NULL,
        site_key     TEXT NOT NULL,
        longitude    REAL,
        utc_offset_s INTEGER,
        first_at     INTEGER NOT NULL,
        last_at      INTEGER NOT NULL,
        PRIMARY KEY (rig_uuid, site_key)
    );
"#;

/// The per-frame site columns of `psf_guard_calibration_frame`, a table
/// PSF Guard owns. `site_read_at` is when the header was read, negative
/// when its file was away then, and NULL when it has not been read.
const FRAME_SITE_COLUMNS: [(&str, &str); 3] = [
    ("site_longitude", "REAL"),
    ("site_utc_offset_s", "INTEGER"),
    ("site_read_at", "INTEGER"),
];

/// Add the site tables and columns to a catalog that predates them.
pub(crate) fn add_site_table(conn: &Connection) -> Result<bool> {
    if conn.is_readonly(rusqlite::MAIN_DB).unwrap_or(true) {
        return Ok(false);
    }
    let mut changed = false;
    if !table_exists(conn, "psf_guard_rig_site") || !table_exists(conn, "psf_guard_rig_site_seen") {
        conn.execute_batch(SITE_TABLE_SQL)?;
        changed = true;
    }
    if super::schema_exists(conn) {
        for (column, declaration) in FRAME_SITE_COLUMNS {
            changed |= super::add_column_if_missing(
                conn,
                "psf_guard_calibration_frame",
                column,
                declaration,
            )?;
        }
    }
    Ok(changed)
}

fn table_exists(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [name],
        |_| Ok(()),
    )
    .optional()
    .ok()
    .flatten()
    .is_some()
}

fn frame_columns_exist(conn: &Connection) -> bool {
    conn.prepare(
        "SELECT site_longitude, site_utc_offset_s, site_read_at
         FROM psf_guard_calibration_frame LIMIT 0",
    )
    .is_ok()
}

/// What one frame's header says about where and when its rig is.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Site {
    /// Degrees, east positive.
    pub longitude: Option<f64>,
    /// The rig computer's offset from UTC, in seconds.
    pub utc_offset_s: Option<i64>,
}

impl Site {
    pub fn of(frame: &FrameMeta) -> Self {
        // N.I.N.A. writes 0°, 0° for a profile with no site set: that is
        // the Gulf of Guinea, not an observatory. The header reader has
        // folded 0–360° east into ±180° already.
        let unset = frame.site_latitude.unwrap_or(0.0) == 0.0;
        let longitude = frame
            .site_longitude
            .filter(|value| value.is_finite() && (-180.0..=180.0).contains(value))
            .filter(|value| *value != 0.0 || !unset);
        let utc_offset_s = match (&frame.date_local, &frame.date_obs_utc) {
            (Some(local), Some(utc)) => {
                match (parse_fits_datetime(local), parse_fits_datetime(utc)) {
                    // To the quarter hour, which every zone keeps to.
                    (Some(local), Some(utc)) => {
                        Some(((local - utc) as f64 / 900.0).round() as i64 * 900)
                            .filter(|offset| offset.abs() <= 14 * 3_600)
                    }
                    _ => None,
                }
            }
            _ => None,
        };
        Self {
            longitude,
            utc_offset_s,
        }
    }

    fn is_empty(&self) -> bool {
        self.longitude.is_none() && self.utc_offset_s.is_none()
    }

    /// One key per place and clock, to the hundredth of a degree.
    fn key(&self) -> String {
        format!(
            "{}|{}",
            self.longitude
                .map(|value| format!("{value:.2}"))
                .unwrap_or_default(),
            self.utc_offset_s
                .map(|value| value.to_string())
                .unwrap_or_default()
        )
    }
}

/// The shift of a site's nights at `longitude` degrees east: 12 h less
/// four minutes a degree, to the minute. Its time of day is mean solar
/// noon in UTC; left unwrapped, it names nights by their local date.
pub fn solar_noon(longitude: f64) -> i64 {
    let minutes = (longitude * 4.0).round() as i64;
    NOON - minutes * 60
}

/// The shift of nights on a clock `utc_offset_s` ahead of UTC, unwrapped:
/// noon on that clock, from −2 h (UTC+14) to 24 h (UTC−12).
pub fn local_noon(utc_offset_s: i64) -> i64 {
    NOON - utc_offset_s
}

/// Where a split came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NightSource {
    /// The `night_zone` setting.
    Setting,
    /// A site longitude, the frame's own or its rig's.
    Site,
    /// A rig computer's clock, the frame's own or its rig's.
    Clock,
    /// The catalog's quiet hours.
    Catalog,
}

/// One rig's split, or one frame's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct RigNight {
    /// The shift: a night is the UTC date of `at − shift`. Taken modulo a
    /// day, the time of day in UTC the nights split at.
    pub boundary_seconds: i64,
    pub source: NightSource,
}

/// A rig's split, from its setting, its newest site and the catalog's.
pub fn resolve(setting: Option<i64>, site: Option<&Site>, catalog: i64) -> RigNight {
    let (boundary_seconds, source) = match (setting, site) {
        (Some(offset), _) => (local_noon(offset), NightSource::Setting),
        (
            None,
            Some(Site {
                longitude: Some(longitude),
                ..
            }),
        ) => (solar_noon(*longitude), NightSource::Site),
        (
            None,
            Some(Site {
                utc_offset_s: Some(offset),
                ..
            }),
        ) => (local_noon(*offset), NightSource::Clock),
        _ => (catalog, NightSource::Catalog),
    };
    RigNight {
        boundary_seconds,
        source,
    }
}

fn by_longitude(longitude: f64) -> RigNight {
    RigNight {
        boundary_seconds: solar_noon(longitude),
        source: NightSource::Site,
    }
}

fn by_clock(offset: i64) -> RigNight {
    RigNight {
        boundary_seconds: local_noon(offset),
        source: NightSource::Clock,
    }
}

/// The value nearest `at` in a rig's time-sorted observations.
fn nearest<T: Copy>(
    timeline: &[(i64, Site)],
    at: i64,
    pick: impl Fn(&Site) -> Option<T>,
) -> Option<T> {
    timeline
        .iter()
        .filter_map(|(when, site)| Some((when.abs_diff(at), pick(site)?)))
        .min_by_key(|(distance, _)| *distance)
        .map(|(_, value)| value)
}

/// Where each rig's nights split in one catalog, and each frame's.
///
/// Loading reads the catalog's light times (for its quiet hours), each
/// frame's own site and each rig's observed sites: a scan of the frame
/// table per load, the same order of cost as the light scan the catalog's
/// split always took. Callers load once per request or pass.
#[derive(Debug, Clone)]
pub struct Nights {
    catalog: i64,
    rigs: Arc<HashMap<String, RigNight>>,
    rig_sites: Arc<HashMap<String, Site>>,
    frames: Arc<HashMap<String, Site>>,
    /// Each rig's observed sites, by capture time.
    timelines: Arc<HashMap<String, Vec<(i64, Site)>>>,
}

impl Nights {
    /// Every rig and frame of the catalog, as settings and sites say.
    pub fn load(conn: &Connection) -> Self {
        let catalog = super::catalog_night_boundary(conn);
        let sites = read_sites(conn);
        let mut rigs = HashMap::new();
        for rig in rig_uuids(conn) {
            let night = resolve(
                rules_for_rig(&rig).night_offset_seconds,
                sites.get(&rig),
                catalog,
            );
            rigs.insert(rig, night);
        }
        let (frames, timelines) = read_frame_sites(conn);
        Self {
            catalog,
            rigs: Arc::new(rigs),
            rig_sites: Arc::new(sites),
            frames: Arc::new(frames),
            timelines: Arc::new(timelines),
        }
    }

    /// Every rig split at `catalog`, unless its setting says otherwise.
    pub fn fixed(catalog: i64) -> Self {
        Self {
            catalog,
            rigs: Arc::new(HashMap::new()),
            rig_sites: Arc::new(HashMap::new()),
            frames: Arc::new(HashMap::new()),
            timelines: Arc::new(HashMap::new()),
        }
    }

    /// [`Nights::fixed`], with some rigs' splits given.
    #[cfg(test)]
    pub fn with_rigs(catalog: i64, rigs: impl IntoIterator<Item = (String, RigNight)>) -> Self {
        Self {
            rigs: Arc::new(rigs.into_iter().collect()),
            ..Self::fixed(catalog)
        }
    }

    /// Where the catalog's lights split, as Images groups them.
    pub fn catalog(&self) -> i64 {
        self.catalog
    }

    /// A rig's split by its setting and newest site, for a frame known by
    /// nothing else.
    pub fn rig(&self, rig_uuid: &str) -> RigNight {
        self.rigs.get(rig_uuid).copied().unwrap_or_else(|| {
            resolve(
                rules_for_rig(rig_uuid).night_offset_seconds,
                None,
                self.catalog,
            )
        })
    }

    /// One frame's split, shot at `at`: its own site, then its rig's
    /// nearest in time.
    pub fn frame(&self, rig_uuid: &str, frame_uuid: &str, at: i64) -> RigNight {
        let rig = self.rig(rig_uuid);
        if rig.source == NightSource::Setting {
            return rig;
        }
        let own = self.frames.get(frame_uuid);
        if let Some(longitude) = own.and_then(|site| site.longitude) {
            return by_longitude(longitude);
        }
        if let Some(offset) = own.and_then(|site| site.utc_offset_s) {
            return by_clock(offset);
        }
        let timeline = self
            .timelines
            .get(rig_uuid)
            .map(Vec::as_slice)
            .unwrap_or_default();
        if let Some(longitude) = nearest(timeline, at, |site| site.longitude) {
            return by_longitude(longitude);
        }
        if let Some(offset) = nearest(timeline, at, |site| site.utc_offset_s) {
            return by_clock(offset);
        }
        rig
    }

    /// The shift of a rig's nights, for a frame known by nothing else.
    pub fn boundary(&self, rig_uuid: &str) -> i64 {
        self.rig(rig_uuid).boundary_seconds
    }

    /// The night a rig's frame known by nothing else, shot at `at`,
    /// belongs to.
    pub fn night_of(&self, rig_uuid: &str, at: i64) -> String {
        super::night_of(at, self.boundary(rig_uuid))
    }

    /// The night one frame shot at `at` belongs to.
    pub fn night_of_frame(&self, rig_uuid: &str, frame_uuid: &str, at: i64) -> String {
        super::night_of(at, self.frame(rig_uuid, frame_uuid, at).boundary_seconds)
    }

    /// The rig's newest clock offset, in seconds ahead of UTC.
    pub fn clock_offset(&self, rig_uuid: &str) -> Option<i64> {
        self.rig_sites.get(rig_uuid)?.utc_offset_s
    }

    /// Every split, for a cache key such as calibration health's: the
    /// catalog's alone while no rig or frame records a site, so keys made
    /// before sites were kept stay put until the first site is read, and
    /// then change once.
    pub fn digest(&self) -> String {
        let mut own: Vec<(&String, i64)> = self
            .rigs
            .iter()
            .filter(|(_, night)| night.boundary_seconds != self.catalog)
            .map(|(rig, night)| (rig, night.boundary_seconds))
            .collect();
        own.sort();
        let mut digest = self.catalog.to_string();
        for (rig, boundary) in own {
            digest.push_str(&format!(",{rig}={boundary}"));
        }
        // Frames' own sites and rigs' observed ones, in one hash.
        let mut sites: Vec<String> = self
            .frames
            .iter()
            .map(|(uuid, site)| format!("{uuid}:{}", site.key()))
            .chain(self.timelines.iter().flat_map(|(rig, timeline)| {
                timeline
                    .iter()
                    .map(move |(at, site)| format!("{rig}@{at}:{}", site.key()))
            }))
            .collect();
        if !sites.is_empty() {
            sites.sort();
            use sha2::Digest;
            let hash = sha2::Sha256::digest(sites.join(";").as_bytes());
            digest.push_str(&format!(
                ",sites={}",
                hash[..8]
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>()
            ));
        }
        digest
    }
}

/// From when a rig's clock kept an offset: the time of the first frame
/// that recorded it, and the offset in seconds ahead of UTC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ClockRun {
    pub from: i64,
    pub offset_seconds: i64,
}

/// A rig's clock over time, as its frames' headers record it, oldest
/// first, one run per change (daylight saving, a move): what its times are
/// shown on. Its `night_zone` setting, when it has one, is the one run.
/// Empty when neither says, and times are shown on the viewer's clock.
pub fn clock_runs(conn: &Connection, rig_uuid: &str) -> Vec<ClockRun> {
    if let Some(offset) = rules_for_rig(rig_uuid).night_offset_seconds {
        return vec![ClockRun {
            from: 0,
            offset_seconds: offset,
        }];
    }
    if !frame_columns_exist(conn) {
        return Vec::new();
    }
    let frames: Vec<(i64, i64)> = conn
        .prepare(
            "SELECT captured_at, site_utc_offset_s FROM psf_guard_calibration_frame
             WHERE rig_uuid = ?1 AND captured_at IS NOT NULL AND site_utc_offset_s IS NOT NULL
             ORDER BY captured_at",
        )
        .and_then(|mut statement| {
            statement
                .query_map([rig_uuid], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .unwrap_or_default();
    let mut runs: Vec<ClockRun> = Vec::new();
    for (at, offset) in frames {
        if runs.last().is_none_or(|run| run.offset_seconds != offset) {
            runs.push(ClockRun {
                from: at,
                offset_seconds: offset,
            });
        }
    }
    runs
}

/// Every rig's clock runs ([`clock_runs`]), for pages that show many rigs'
/// times.
pub fn all_clock_runs(conn: &Connection) -> HashMap<String, Vec<ClockRun>> {
    rig_uuids(conn)
        .into_iter()
        .map(|rig| {
            let runs = clock_runs(conn, &rig);
            (rig, runs)
        })
        .filter(|(_, runs)| !runs.is_empty())
        .collect()
}

fn rig_uuids(conn: &Connection) -> Vec<String> {
    let query = |sql: &str| -> Vec<String> {
        conn.prepare(sql)
            .and_then(|mut statement| {
                statement
                    .query_map([], |row| row.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .unwrap_or_default()
    };
    let mut rigs = query("SELECT rig_uuid FROM psf_guard_rig");
    rigs.extend(query(
        "SELECT DISTINCT rig_uuid FROM psf_guard_calibration_frame",
    ));
    rigs.sort();
    rigs.dedup();
    rigs
}

fn read_sites(conn: &Connection) -> HashMap<String, Site> {
    if !table_exists(conn, "psf_guard_rig_site") {
        return HashMap::new();
    }
    conn.prepare("SELECT rig_uuid, longitude, utc_offset_s FROM psf_guard_rig_site")
        .and_then(|mut statement| {
            statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        Site {
                            longitude: row.get(1)?,
                            utc_offset_s: row.get(2)?,
                        },
                    ))
                })?
                .collect::<rusqlite::Result<HashMap<_, _>>>()
        })
        .unwrap_or_default()
}

type FrameSites = (HashMap<String, Site>, HashMap<String, Vec<(i64, Site)>>);

/// Each frame's own site, for the frames that record one, and each rig's
/// observations by capture time: its frames' sites, and both ends of every
/// span an import saw it at.
fn read_frame_sites(conn: &Connection) -> FrameSites {
    let mut frames = HashMap::new();
    let mut timelines: HashMap<String, Vec<(i64, Site)>> = HashMap::new();
    if frame_columns_exist(conn) {
        let rows = conn
            .prepare(
                "SELECT frame_uuid, rig_uuid, captured_at, site_longitude, site_utc_offset_s
                 FROM psf_guard_calibration_frame
                 WHERE site_longitude IS NOT NULL OR site_utc_offset_s IS NOT NULL",
            )
            .and_then(|mut statement| {
                statement
                    .query_map([], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, Option<i64>>(2)?,
                            Site {
                                longitude: row.get(3)?,
                                utc_offset_s: row.get(4)?,
                            },
                        ))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .unwrap_or_default();
        for (frame, rig, at, site) in rows {
            if let Some(at) = at {
                timelines.entry(rig).or_default().push((at, site));
            }
            frames.insert(frame, site);
        }
    }
    if table_exists(conn, "psf_guard_rig_site_seen") {
        let rows = conn
            .prepare(
                "SELECT rig_uuid, longitude, utc_offset_s, first_at, last_at
                 FROM psf_guard_rig_site_seen",
            )
            .and_then(|mut statement| {
                statement
                    .query_map([], |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            Site {
                                longitude: row.get(1)?,
                                utc_offset_s: row.get(2)?,
                            },
                            row.get::<_, i64>(3)?,
                            row.get::<_, i64>(4)?,
                        ))
                    })?
                    .collect::<rusqlite::Result<Vec<_>>>()
            })
            .unwrap_or_default();
        for (rig, site, first, last) in rows {
            let timeline = timelines.entry(rig).or_default();
            timeline.push((first, site));
            if last != first {
                timeline.push((last, site));
            }
        }
    }
    for timeline in timelines.values_mut() {
        timeline.sort_by_key(|(at, _)| *at);
    }
    (frames, timelines)
}

/// Keep what an imported frame, a light's or a calibration frame's, says
/// about its rig's site: the rig's newest site, and the span of captures it
/// was seen at that site. A newer frame replaces the newest site's longitude
/// and clock; an older one only fills what is missing. A frame that says
/// nothing changes nothing.
pub(crate) fn record_site(conn: &Connection, rig_uuid: &str, frame: &FrameMeta) -> Result<()> {
    let site = Site::of(frame);
    if site.is_empty() || !table_exists(conn, "psf_guard_rig_site") {
        return Ok(());
    }
    Ok(write_site(conn, rig_uuid, &site, frame.timestamp)?)
}

/// Keep what an imported calibration frame's header says of its own site.
/// A frame whose header could not be read is left for the backfill.
pub(crate) fn record_frame_site(
    conn: &Connection,
    source_path: &str,
    frame: &FrameMeta,
) -> Result<()> {
    if !frame.readable || !frame_columns_exist(conn) {
        return Ok(());
    }
    let site = Site::of(frame);
    conn.execute(
        "UPDATE psf_guard_calibration_frame
         SET site_longitude = ?2, site_utc_offset_s = ?3, site_read_at = ?4
         WHERE source_path = ?1",
        params![
            source_path,
            site.longitude,
            site.utc_offset_s,
            chrono::Utc::now().timestamp()
        ],
    )?;
    Ok(())
}

fn write_site(
    conn: &Connection,
    rig_uuid: &str,
    site: &Site,
    captured_at: Option<i64>,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO psf_guard_rig_site
            (rig_uuid, longitude, utc_offset_s, frame_captured_at, read_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(rig_uuid) DO UPDATE SET
            longitude = CASE
                WHEN COALESCE(excluded.frame_captured_at, 0)
                     >= COALESCE(psf_guard_rig_site.frame_captured_at, 0)
                THEN COALESCE(excluded.longitude, psf_guard_rig_site.longitude)
                ELSE COALESCE(psf_guard_rig_site.longitude, excluded.longitude) END,
            utc_offset_s = CASE
                WHEN COALESCE(excluded.frame_captured_at, 0)
                     >= COALESCE(psf_guard_rig_site.frame_captured_at, 0)
                THEN COALESCE(excluded.utc_offset_s, psf_guard_rig_site.utc_offset_s)
                ELSE COALESCE(psf_guard_rig_site.utc_offset_s, excluded.utc_offset_s) END,
            frame_captured_at = MAX(COALESCE(excluded.frame_captured_at, 0),
                                    COALESCE(psf_guard_rig_site.frame_captured_at, 0)),
            read_at = excluded.read_at",
        params![
            rig_uuid,
            site.longitude,
            site.utc_offset_s,
            captured_at,
            chrono::Utc::now().timestamp()
        ],
    )?;
    if let Some(at) = captured_at
        && table_exists(conn, "psf_guard_rig_site_seen")
    {
        conn.execute(
            "INSERT INTO psf_guard_rig_site_seen
                (rig_uuid, site_key, longitude, utc_offset_s, first_at, last_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)
             ON CONFLICT(rig_uuid, site_key) DO UPDATE SET
                first_at = MIN(psf_guard_rig_site_seen.first_at, excluded.first_at),
                last_at = MAX(psf_guard_rig_site_seen.last_at, excluded.last_at)",
            params![rig_uuid, site.key(), site.longitude, site.utc_offset_s, at],
        )?;
    }
    Ok(())
}

/// What one pass of [`backfill_frame_sites`] did.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct SiteBackfillOutcome {
    /// Frames whose site had not been read when the pass began, or whose
    /// file was away a week or more ago.
    pub pending: usize,
    /// Of those, how many the pass handled.
    pub done: usize,
    /// Frames that named a longitude.
    pub located: usize,
    /// Frames that named only the computer's clock.
    pub clocked: usize,
    /// Frames read that name neither.
    pub unplaced: usize,
    /// Frames whose file was away while others were read; read again a
    /// week later, or when an import brings them.
    pub unreadable: usize,
    /// No file the pass tried could be read, as when the volume is not
    /// mounted: nothing was marked away, and the pass stopped to be tried
    /// again.
    pub volume_away: bool,
    /// The pass ended early on an error it could not get past, such as a
    /// catalog too busy to commit; what it committed before stays.
    pub error: Option<String>,
    /// The rigs some frame of which found a site, whose nights may now
    /// split elsewhere.
    pub placed: Vec<String>,
}

/// What a backfill pass needs from its caller.
pub struct SiteBackfill<'a> {
    /// Asked before each chunk; false ends the pass at a commit.
    pub keep_going: &'a dyn Fn() -> bool,
    /// Called before each chunk: waits while interactive work runs.
    pub yield_now: &'a dyn Fn(),
    /// `(done, total)` frames, after each chunk.
    pub progress: &'a mut dyn FnMut(usize, usize),
    /// The rigs a committed chunk placed, as soon as it commits, so a pass
    /// that ends early has reported what it did.
    pub placed: &'a mut dyn FnMut(&[String]),
}

/// [`backfill_frame_sites`] with no progress to report.
pub fn backfill_rig_sites(
    conn: &Connection,
    keep_going: impl Fn() -> bool,
    yield_now: impl Fn(),
) -> Result<SiteBackfillOutcome> {
    backfill_frame_sites(
        conn,
        SiteBackfill {
            keep_going: &keep_going,
            yield_now: &yield_now,
            progress: &mut |_, _| {},
            placed: &mut |_| {},
        },
    )
}

/// One frame's header, read: its site, or `None` when its file was away.
struct ReadFrame<'a> {
    id: i64,
    rig: &'a str,
    path: &'a str,
    captured_at: Option<i64>,
    site: Option<Site>,
}

/// Read the site of every calibration frame imported before the catalog
/// kept one, off the open path, a hundred headers between commits, each
/// frame's rig keeping what its frames said ([`write_site`]).
///
/// A chunk in which no file can be read, before any file of the pass was
/// read, means the volume is away (an unmounted NAS): nothing is marked
/// and the pass ends, with `volume_away`, for the caller to try again. A
/// commit the catalog is too busy for is tried again; one that still fails
/// ends the pass with its `error`, keeping what earlier chunks committed and
/// reported. A write lands only on a row an import has not rewritten since
/// it was read.
pub fn backfill_frame_sites(
    conn: &Connection,
    hooks: SiteBackfill<'_>,
) -> Result<SiteBackfillOutcome> {
    if !super::schema_exists(conn)
        || !table_exists(conn, "psf_guard_rig_site")
        || !frame_columns_exist(conn)
    {
        return Ok(SiteBackfillOutcome::default());
    }
    let now = chrono::Utc::now().timestamp();
    let pending: Vec<(i64, String, String, Option<i64>)> = conn
        .prepare(
            "SELECT id, rig_uuid, source_path, captured_at FROM psf_guard_calibration_frame
             WHERE site_read_at IS NULL OR (site_read_at < 0 AND -site_read_at <= ?1)
             ORDER BY rig_uuid, captured_at DESC, id DESC",
        )?
        .query_map([now - SITE_RETRY_SECONDS], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let mut outcome = SiteBackfillOutcome {
        pending: pending.len(),
        ..Default::default()
    };
    (hooks.progress)(0, pending.len());
    let mut placed: BTreeSet<String> = BTreeSet::new();
    let mut any_readable = false;
    for chunk in pending.chunks(SITE_BACKFILL_CHUNK) {
        if !(hooks.keep_going)() {
            break;
        }
        (hooks.yield_now)();
        if !(hooks.keep_going)() {
            break;
        }
        let read: Vec<ReadFrame<'_>> = chunk
            .iter()
            .map(|(id, rig, path, captured_at)| {
                let meta = crate::commands::import::headers::read_frame_meta(Path::new(path));
                ReadFrame {
                    id: *id,
                    rig,
                    path,
                    captured_at: *captured_at,
                    site: meta.readable.then(|| Site::of(&meta)),
                }
            })
            .collect();
        if !any_readable && read.iter().all(|frame| frame.site.is_none()) {
            outcome.volume_away = true;
            break;
        }
        any_readable = true;
        let mut tries = 0;
        let chunk_placed = loop {
            match write_chunk(conn, &read, now) {
                Ok(chunk_placed) => break Some(chunk_placed),
                Err(error) if is_busy(&error) && tries + 1 < COMMIT_TRIES => {
                    std::thread::sleep(COMMIT_WAIT * 2u32.pow(tries));
                    tries += 1;
                }
                Err(error) => {
                    outcome.error = Some(format!("{error}"));
                    break None;
                }
            }
        };
        let Some(chunk_placed) = chunk_placed else {
            break;
        };
        for frame in &read {
            match (frame.site.as_ref(), frame.site.and_then(|s| s.longitude)) {
                (Some(_), Some(_)) => outcome.located += 1,
                (Some(site), None) if site.utc_offset_s.is_some() => outcome.clocked += 1,
                (Some(_), None) => outcome.unplaced += 1,
                (None, _) => outcome.unreadable += 1,
            }
        }
        outcome.done += read.len();
        if !chunk_placed.is_empty() {
            (hooks.placed)(&chunk_placed);
            placed.extend(chunk_placed);
        }
        (hooks.progress)(outcome.done, pending.len());
    }
    outcome.placed = placed.into_iter().collect();
    Ok(outcome)
}

fn is_busy(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(failure, _)
            if matches!(
                failure.code,
                rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
            )
    )
}

/// One chunk's sites in one commit; the rigs it placed.
fn write_chunk(
    conn: &Connection,
    read: &[ReadFrame<'_>],
    now: i64,
) -> rusqlite::Result<Vec<String>> {
    let tx = conn.unchecked_transaction()?;
    let mut placed: BTreeSet<String> = BTreeSet::new();
    for frame in read {
        // Only a row still as it was read: an import may have rewritten
        // its path, or read its site, meanwhile.
        let guard = "WHERE id = ?1 AND source_path = ?2
                     AND (site_read_at IS NULL OR site_read_at < 0)";
        match &frame.site {
            Some(site) => {
                let changed = tx.execute(
                    &format!(
                        "UPDATE psf_guard_calibration_frame
                         SET site_longitude = ?3, site_utc_offset_s = ?4, site_read_at = ?5
                         {guard}"
                    ),
                    params![frame.id, frame.path, site.longitude, site.utc_offset_s, now],
                )?;
                if changed > 0 && !site.is_empty() {
                    write_site(&tx, frame.rig, site, frame.captured_at)?;
                    placed.insert(frame.rig.to_string());
                }
            }
            None => {
                tx.execute(
                    &format!("UPDATE psf_guard_calibration_frame SET site_read_at = ?3 {guard}"),
                    params![frame.id, frame.path, -now],
                )?;
            }
        }
    }
    tx.commit()?;
    Ok(placed.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: i64 = 3_600;

    /// Unix seconds of a UTC date and time.
    fn utc(text: &str) -> i64 {
        parse_fits_datetime(text).unwrap()
    }

    /// The C925's site, in the Sacramento valley.
    const C925_LONGITUDE: f64 = -121.163_247_222_222;

    #[test]
    fn solar_noon_falls_four_minutes_later_for_each_degree_west() {
        assert_eq!(solar_noon(0.0), 12 * HOUR);
        assert_eq!(solar_noon(-15.0), 13 * HOUR);
        assert_eq!(solar_noon(150.0), 2 * HOUR);
        // 121.16° west is 8 h 05 min behind Greenwich.
        assert_eq!(solar_noon(C925_LONGITUDE), 20 * HOUR + 5 * 60);
        assert_eq!(local_noon(-7 * HOUR), 19 * HOUR);
        assert_eq!(local_noon(-8 * HOUR), 20 * HOUR);
        assert_eq!(local_noon(9 * HOUR + 1_800), 2 * HOUR + 1_800);
    }

    fn c925() -> Nights {
        Nights::with_rigs(
            12 * HOUR,
            [(
                "c925".to_string(),
                resolve(
                    None,
                    Some(&Site {
                        longitude: Some(C925_LONGITUDE),
                        utc_offset_s: Some(-7 * HOUR),
                    }),
                    12 * HOUR,
                ),
            )],
        )
    }

    #[test]
    fn a_dawn_flat_run_across_noon_utc_stays_in_one_night() {
        // OIII at 104.2°, 04:58 to 05:00 PDT on 2026-08-19: 11:58 to 12:00
        // UTC. Noon UTC split it in two.
        let nights = c925();
        let run = [
            utc("2026-08-19T11:58:36"),
            utc("2026-08-19T11:59:54"),
            utc("2026-08-19T12:00:09"),
            utc("2026-08-19T12:00:56"),
        ];
        for at in run {
            assert_eq!(nights.night_of("c925", at), "2026-08-18");
        }
        let utc_noon = Nights::fixed(12 * HOUR);
        assert_ne!(
            utc_noon.night_of("other", run[0]),
            utc_noon.night_of("other", run[3])
        );
    }

    #[test]
    fn dusk_and_the_dawn_after_it_share_a_night_in_summer_and_winter() {
        let nights = c925();
        // PDT, UTC−7: dusk flats at 20:40 on 08-18, dawn flats at 05:00 on
        // 08-19, and the dusk flats of 08-19 that begin the next night.
        assert_eq!(
            nights.night_of("c925", utc("2026-08-19T03:40:00")),
            "2026-08-18"
        );
        assert_eq!(
            nights.night_of("c925", utc("2026-08-19T12:00:00")),
            "2026-08-18"
        );
        assert_eq!(
            nights.night_of("c925", utc("2026-08-20T03:40:00")),
            "2026-08-19"
        );
        // PST, UTC−8, after the clocks went back on 11-01: dusk at 17:15 on
        // 12-10, dawn at 06:30 on 12-11, both of the night of 12-10.
        assert_eq!(
            nights.night_of("c925", utc("2026-12-11T01:15:00")),
            "2026-12-10"
        );
        assert_eq!(
            nights.night_of("c925", utc("2026-12-11T14:30:00")),
            "2026-12-10"
        );
        // Noon, either side of it, on either clock.
        assert_eq!(
            nights.night_of("c925", utc("2026-08-19T19:30:00")),
            "2026-08-18"
        );
        assert_eq!(
            nights.night_of("c925", utc("2026-08-19T20:30:00")),
            "2026-08-19"
        );
        assert_eq!(
            nights.night_of("c925", utc("2026-12-11T19:30:00")),
            "2026-12-10"
        );
        assert_eq!(
            nights.night_of("c925", utc("2026-12-11T20:30:00")),
            "2026-12-11"
        );
    }

    #[test]
    fn a_rig_splits_at_its_setting_then_its_site_then_its_clock_then_the_catalog() {
        let catalog = 18 * HOUR;
        let site = Site {
            longitude: Some(C925_LONGITUDE),
            utc_offset_s: Some(0),
        };
        assert_eq!(
            resolve(Some(9 * HOUR), Some(&site), catalog),
            RigNight {
                boundary_seconds: 3 * HOUR,
                source: NightSource::Setting
            }
        );
        // A rig computer kept on UTC does not move the night to noon UTC.
        assert_eq!(
            resolve(None, Some(&site), catalog).source,
            NightSource::Site
        );
        let clock = Site {
            longitude: None,
            utc_offset_s: Some(-8 * HOUR),
        };
        assert_eq!(
            resolve(None, Some(&clock), catalog),
            RigNight {
                boundary_seconds: 20 * HOUR,
                source: NightSource::Clock
            }
        );
        assert_eq!(
            resolve(None, None, catalog),
            RigNight {
                boundary_seconds: catalog,
                source: NightSource::Catalog
            }
        );
    }

    #[test]
    fn a_frame_names_its_site_and_its_computer_clock() {
        let frame = FrameMeta {
            site_longitude: Some(C925_LONGITUDE),
            site_latitude: Some(38.66),
            date_local: Some("2026-08-19T04:58:36.4704120".into()),
            date_obs_utc: Some("2026-08-19T11:58:36.4704120".into()),
            ..Default::default()
        };
        assert_eq!(
            Site::of(&frame),
            Site {
                longitude: Some(C925_LONGITUDE),
                utc_offset_s: Some(-7 * HOUR)
            }
        );
        // An unset N.I.N.A. profile writes 0°, 0°.
        let unset = FrameMeta {
            site_longitude: Some(0.0),
            site_latitude: Some(0.0),
            ..Default::default()
        };
        assert_eq!(Site::of(&unset), Site::default());
    }

    #[test]
    fn the_digest_is_the_catalog_split_until_a_rig_has_its_own() {
        assert_eq!(Nights::fixed(72_000).digest(), "72000");
        let same = Nights::with_rigs(
            72_000,
            [(
                "a".to_string(),
                RigNight {
                    boundary_seconds: 72_000,
                    source: NightSource::Catalog,
                },
            )],
        );
        assert_eq!(same.digest(), "72000");
        assert_eq!(c925().digest(), "43200,c925=72300");
    }

    /// The C925's OIII dawn run of 2026-08-19, ten flats from 04:58:36 to
    /// 05:00:56 PDT, as N.I.N.A. writes them: with the profile's site, or
    /// without one and with the computer kept on UTC.
    fn dawn_run(site: bool) -> Vec<FrameMeta> {
        (0..10)
            .map(|index| {
                let at = utc("2026-08-19T11:58:36") + index * 15;
                let text = |at: i64| {
                    chrono::DateTime::from_timestamp(at, 0)
                        .unwrap()
                        .naive_utc()
                        .format("%Y-%m-%dT%H:%M:%S")
                        .to_string()
                };
                FrameMeta {
                    path: format!("/nowhere/oiii-{index}.fits").into(),
                    readable: true,
                    image_type: Some("FLAT".into()),
                    filter: Some("OIII".into()),
                    timestamp: Some(at),
                    date_obs_utc: Some(text(at)),
                    date_local: Some(text(if site { at - 7 * HOUR } else { at })),
                    exposure_s: Some(15.0),
                    gain: Some(100),
                    offset: Some(50),
                    width: Some(6248),
                    height: Some(4176),
                    binning_x: Some(1),
                    binning_y: Some(1),
                    telescope: Some("C925".into()),
                    camera: Some("ZWO ASI2600MM Pro".into()),
                    rotator_position: Some(104.22),
                    site_longitude: site.then_some(C925_LONGITUDE),
                    site_latitude: site.then_some(38.66),
                    ..Default::default()
                }
            })
            .collect()
    }

    fn library(frames: &[FrameMeta]) -> (Connection, String) {
        let mut conn = Connection::open_in_memory().unwrap();
        let tx = conn.transaction().unwrap();
        super::super::import_calibration_frames(&tx, frames, Some("profile")).unwrap();
        tx.commit().unwrap();
        let rig = conn
            .query_row("SELECT rig_uuid FROM psf_guard_rig", [], |row| row.get(0))
            .unwrap();
        (conn, rig)
    }

    /// Flat-group sessions and the browser's sessions of a library.
    fn sessions(conn: &Connection) -> (Vec<(String, usize)>, Vec<String>) {
        let snapshot = super::super::LibrarySnapshot::load(conn).unwrap();
        let plans =
            super::super::flat_groups::job::plan_sessions(snapshot.frames(), &Nights::load(conn))
                .into_iter()
                .map(|plan| (plan.night, plan.frames.len()))
                .collect();
        let mut browsed: Vec<String> = super::super::browse::library_frames(conn)
            .unwrap()
            .frames
            .into_iter()
            .map(|frame| format!("{} {}", frame.night.unwrap_or_default(), frame.session))
            .collect();
        browsed.dedup();
        (plans, browsed)
    }

    #[test]
    fn the_dawn_run_is_one_session_where_its_site_puts_noon() {
        let _policy = super::super::tests::POLICY_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let (conn, rig) = library(&dawn_run(true));
        assert_eq!(Nights::load(&conn).rig(&rig).source, NightSource::Site);
        let (plans, browsed) = sessions(&conn);
        assert_eq!(plans, vec![("2026-08-18".to_string(), 10)]);
        assert_eq!(browsed.len(), 1, "{browsed:?}");
        assert!(browsed[0].starts_with("2026-08-18 "));
    }

    #[test]
    fn without_a_site_or_lights_noon_utc_splits_the_run_and_a_rig_setting_mends_it() {
        let _policy = super::super::tests::POLICY_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        // No longitude, a computer on UTC, and no lights: noon UTC, which
        // falls between the sixth and seventh flat.
        let (conn, rig) = library(&dawn_run(false));
        assert_eq!(Nights::load(&conn).rig(&rig).source, NightSource::Clock);
        let (plans, browsed) = sessions(&conn);
        assert_eq!(
            plans,
            vec![("2026-08-18".to_string(), 6), ("2026-08-19".to_string(), 4)]
        );
        assert_eq!(browsed.len(), 2, "{browsed:?}");

        // The rig's own setting, UTC−8, wins over its clock.
        let rigs = [(
            rig.clone(),
            super::super::rules::RigLayer {
                name: Some("C925".into()),
                values: [("night_zone".to_string(), serde_json::json!("-8"))]
                    .into_iter()
                    .collect(),
            },
        )]
        .into_iter()
        .collect();
        super::super::rules::set_rule_book(super::super::rules::RuleBook::new(
            Default::default(),
            rigs,
        ));
        let night = Nights::load(&conn).rig(&rig);
        let (plans, _) = sessions(&conn);
        super::super::rules::set_rule_book(Default::default());
        assert_eq!(
            night,
            RigNight {
                boundary_seconds: 20 * HOUR,
                source: NightSource::Setting
            }
        );
        assert_eq!(plans, vec![("2026-08-18".to_string(), 10)]);
    }

    /// A measured response: its key, night, frame count and when measured.
    fn responses(conn: &Connection) -> Vec<(String, String, usize, Option<String>)> {
        let mut rows: Vec<_> = super::super::flat_groups::store::load_responses(conn)
            .unwrap()
            .into_iter()
            .map(|row| (row.response_key, row.night, row.frames, row.filter))
            .collect();
        rows.sort_by(|a, b| (&a.3, &a.1, a.2).cmp(&(&b.3, &b.1, b.2)));
        rows
    }

    fn measure(
        conn: &Connection,
        dir: &Path,
        scope: super::super::flat_groups::job::Scope,
    ) -> super::super::flat_groups::job::PassOutcome {
        super::super::flat_groups::job::run(super::super::flat_groups::job::Pass {
            conn,
            dir,
            scope,
            keep_going: &|| true,
            yield_now: &|| {},
            progress: &mut |_| {},
            goes_first: &|_| false,
            find_light: &|_| None,
            pool: None,
        })
        .unwrap()
    }

    #[test]
    fn upgrading_renames_measured_nights_and_measures_again_only_runs_that_merge() {
        let _policy = super::super::tests::POLICY_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let site = [
            "SITELAT =              38.6606",
            "SITELONG=            -121.1632",
        ];
        // Hα at 06:00 PDT, all after noon UTC; OIII across it, 04:59:50 to
        // 05:00:06 PDT.
        let runs = [
            ("HA", "2026-08-19T13:00:0", [0, 2, 4, 6]),
            ("OIII", "2026-08-19T11:59:5", [0, 2, 4, 6]),
            ("OIII", "2026-08-19T12:00:0", [0, 2, 4, 6]),
        ];
        let mut frames = Vec::new();
        for (filter, minute, seconds) in runs {
            for second in seconds {
                let captured = format!("{minute}{second}");
                let path = temp
                    .path()
                    .join(format!("{filter}-{}.fits", captured.replace(':', "")));
                let noise = |x: usize, y: usize| ((x * 7 + y * 13 + second) % 11) as f64;
                crate::calibration::test_frames::write_frame_with(
                    &path,
                    "FLAT",
                    (96, 64),
                    &captured,
                    &site,
                    |x, y| 30_000.0 - (x as f64 - 48.0).powi(2) - noise(x, y),
                );
                let mut meta = crate::commands::import::headers::read_frame_meta(&path);
                meta.filter = Some(filter.into());
                frames.push(meta);
            }
        }
        let path = temp.path().join("catalog.sqlite");
        let mut conn = Connection::open(&path).unwrap();
        let tx = conn.transaction().unwrap();
        super::super::import_calibration_frames(&tx, &frames, Some("profile")).unwrap();
        tx.commit().unwrap();
        super::super::flat_groups::store::add_tables(&conn).unwrap();
        let dir = temp.path().join("responses");
        super::super::flat_groups::register_response_dir(&path, dir.clone());

        // Measured before the catalog kept sites: noon UTC.
        conn.execute_batch(
            "DELETE FROM psf_guard_rig_site;
             DELETE FROM psf_guard_rig_site_seen;
             UPDATE psf_guard_calibration_frame
             SET site_longitude = NULL, site_utc_offset_s = NULL, site_read_at = NULL;",
        )
        .unwrap();
        let everything = super::super::flat_groups::job::Scope::everything();
        assert_eq!(measure(&conn, &dir, everything).measured, 3);
        let before = responses(&conn);
        assert_eq!(
            before
                .iter()
                .map(|(_, night, frames, _)| (night.as_str(), *frames))
                .collect::<Vec<_>>(),
            [("2026-08-19", 4), ("2026-08-18", 4), ("2026-08-19", 4)]
        );

        // The upgrade reads the site; the next pass plans by it.
        let found = backfill_rig_sites(&conn, || true, || {}).unwrap();
        assert_eq!((found.located, found.placed.len()), (12, 1), "{found:?}");
        // The server queues the rigs that found a site.
        let outcome = measure(
            &conn,
            &dir,
            super::super::flat_groups::job::Scope::flats(super::super::flat_groups::job::Rigs::of(
                found.placed,
            )),
        );
        let after = responses(&conn);
        assert_eq!(outcome.measured, 1, "only the merged OIII run is measured");
        // Hα kept its plane under the night its evening began.
        assert_eq!(after[0].0, before[0].0);
        assert_eq!(after[0].1, "2026-08-18");
        // OIII is one session of eight, and its halves are gone.
        assert_eq!(after.len(), 2);
        assert_eq!((after[1].1.as_str(), after[1].2), ("2026-08-18", 8));
        assert!(before[1..].iter().all(|old| old.0 != after[1].0));
    }

    /// A night is named by its evening's local date, as DATEMINUS12 names
    /// it. Offsets past +12 h (New Zealand in summer, Tonga, Chatham) and
    /// longitudes within 0.125° of 180° W would fold the split across 00:00
    /// UTC and name the night a day off, were the shift wrapped.
    #[test]
    fn nights_near_the_date_line_are_named_by_their_evening() {
        // NZDT, UTC+13: 21:00 on 2026-01-10 is 08:00 UTC the same day.
        let nzdt = resolve(Some(13 * HOUR), None, 12 * HOUR).boundary_seconds;
        assert_eq!(
            super::super::night_of(utc("2026-01-10T08:00:00"), nzdt),
            "2026-01-10"
        );
        // 179.95° W, UTC-12: 20:00 on 2026-08-19 is 08:00 UTC on 08-20.
        let west = solar_noon(-179.95);
        assert_eq!(
            super::super::night_of(utc("2026-08-20T08:00:00"), west),
            "2026-08-19"
        );
        // Tonga, UTC+14: 20:00 on 2026-06-01 is 06:00 UTC the same day, and
        // dawn at 06:30 on 06-02 is 16:30 UTC on 06-01.
        let tonga = resolve(Some(14 * HOUR), None, 12 * HOUR).boundary_seconds;
        assert_eq!(tonga, -2 * HOUR);
        assert_eq!(
            super::super::night_of(utc("2026-06-01T06:00:00"), tonga),
            "2026-06-01"
        );
        assert_eq!(
            super::super::night_of(utc("2026-06-01T16:30:00"), tonga),
            "2026-06-01"
        );
    }

    #[test]
    fn nights_east_of_greenwich_are_named_by_their_evening() {
        // Tokyo, 139.69° E: solar noon at 02:41 UTC. 19:00 JST on 03-10 is
        // 10:00 UTC; 04:30 JST on 03-11 is 19:30 UTC on 03-10; noon JST on
        // 03-11 begins the next night.
        let tokyo = solar_noon(139.69);
        assert_eq!(tokyo, 2 * HOUR + 41 * 60);
        assert_eq!(
            super::super::night_of(utc("2026-03-10T10:00:00"), tokyo),
            "2026-03-10"
        );
        assert_eq!(
            super::super::night_of(utc("2026-03-10T19:30:00"), tokyo),
            "2026-03-10"
        );
        assert_eq!(
            super::super::night_of(utc("2026-03-11T03:00:00"), tokyo),
            "2026-03-11"
        );
        // New Zealand in summer, UTC+13, from its clock: 21:30 on 01-10 is
        // 08:30 UTC, dawn at 05:30 on 01-11 is 16:30 UTC on 01-10.
        let auckland = local_noon(13 * HOUR);
        assert_eq!(
            super::super::night_of(utc("2026-01-10T08:30:00"), auckland),
            "2026-01-10"
        );
        assert_eq!(
            super::super::night_of(utc("2026-01-10T16:30:00"), auckland),
            "2026-01-10"
        );
    }

    /// A rig taken to a star party splits those nights where it was and
    /// keeps its home nights. With one site per rig, the newest frame's,
    /// California dusk flats at 19:00 PDT (02:00 UTC) split once its newest
    /// frame came from Australia, whose solar noon is 02:04 UTC.
    #[test]
    fn a_travel_rig_keeps_the_nights_of_where_each_frame_was() {
        let _policy = super::super::tests::POLICY_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let mut frames: Vec<FrameMeta> = dawn_run(true)
            .into_iter()
            .enumerate()
            .map(|(index, mut frame)| {
                let at = utc("2026-08-19T02:01:00") + index as i64 * 30;
                frame.timestamp = Some(at);
                frame.date_obs_utc = None;
                frame.date_local = None;
                frame
            })
            .collect();
        let (conn, rig) = library(&frames);
        let (plans, _) = sessions(&conn);
        assert_eq!(plans, vec![("2026-08-18".to_string(), 10)], "at home");
        // A month later, the rig shoots one flat at a star party in NSW.
        let mut away = frames[0].clone();
        away.path = "/nowhere/away.fits".into();
        away.filter = Some("HA".into());
        away.timestamp = Some(utc("2026-09-20T09:00:00"));
        away.site_longitude = Some(149.07);
        away.site_latitude = Some(-31.27);
        frames.push(away);
        let tx = conn.unchecked_transaction().unwrap();
        super::super::import_calibration_frames(&tx, &frames[10..], Some("profile")).unwrap();
        tx.commit().unwrap();
        assert_eq!(Nights::load(&conn).rig(&rig).source, NightSource::Site);
        let (plans, _) = sessions(&conn);
        assert!(
            plans.contains(&("2026-08-18".to_string(), 10)),
            "the California run split: {plans:?}"
        );
        // The NSW flat, 19:00 AEST on 09-20, is that evening's.
        let nights = Nights::load(&conn);
        let away_uuid: String = conn
            .query_row(
                "SELECT frame_uuid FROM psf_guard_calibration_frame WHERE source_path LIKE '%away.fits'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            nights.night_of_frame(&rig, &away_uuid, utc("2026-09-20T09:00:00")),
            "2026-09-20"
        );
    }

    /// Frames whose headers name only their computer's clock, PDT in summer
    /// and PST in winter: each splits at noon on its own clock, and the
    /// rig, for a frame known by nothing else, at noon on its newest.
    #[test]
    fn the_backfill_reads_a_clock_only_rig_and_each_frame_keeps_its_clock() {
        let _policy = super::super::tests::POLICY_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let mut frames = Vec::new();
        for (name, utc_text, local) in [
            ("summer", "2026-08-19T12:00:00", "2026-08-19T05:00:00"),
            ("winter", "2026-12-11T14:00:00", "2026-12-11T06:00:00"),
        ] {
            let path = temp.path().join(format!("{name}.fits"));
            let card = format!("DATE-LOC= '{local}'");
            crate::calibration::test_frames::write_frame_with(
                &path,
                "FLAT",
                (32, 32),
                utc_text,
                &[card.as_str()],
                |_, _| 20_000.0,
            );
            frames.push(crate::commands::import::headers::read_frame_meta(&path));
        }
        let (conn, rig) = library(&frames);
        // As a catalog imported before sites were kept.
        conn.execute_batch(
            "DELETE FROM psf_guard_rig_site;
             DELETE FROM psf_guard_rig_site_seen;
             UPDATE psf_guard_calibration_frame
             SET site_longitude = NULL, site_utc_offset_s = NULL, site_read_at = NULL;",
        )
        .unwrap();
        assert_eq!(Nights::load(&conn).rig(&rig).source, NightSource::Catalog);
        let found = backfill_rig_sites(&conn, || true, || {}).unwrap();
        assert_eq!(
            (found.clocked, found.placed.clone()),
            (2, vec![rig.clone()])
        );
        let nights = Nights::load(&conn);
        assert_eq!(
            nights.rig(&rig),
            RigNight {
                boundary_seconds: 20 * HOUR,
                source: NightSource::Clock
            }
        );
        assert_eq!(nights.clock_offset(&rig), Some(-8 * HOUR));
        let summer: String = conn
            .query_row(
                "SELECT frame_uuid FROM psf_guard_calibration_frame WHERE source_path LIKE '%summer.fits'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            nights
                .frame(&rig, &summer, utc("2026-08-19T12:00:00"))
                .boundary_seconds,
            19 * HOUR
        );
        // Its times read on its clock, one run per change.
        assert_eq!(
            clock_runs(&conn, &rig),
            vec![
                ClockRun {
                    from: utc("2026-08-19T12:00:00"),
                    offset_seconds: -7 * HOUR
                },
                ClockRun {
                    from: utc("2026-12-11T14:00:00"),
                    offset_seconds: -8 * HOUR
                },
            ]
        );
        // Read once: a second pass reads nothing.
        assert_eq!(
            backfill_rig_sites(&conn, || true, || {}).unwrap().pending,
            0
        );
    }

    /// Some software writes longitude as 0–360° east: 238.84° is the
    /// C925's 121.16° W.
    #[test]
    fn a_longitude_past_180_east_is_read_as_west() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("flat.fits");
        crate::calibration::test_frames::write_frame_with(
            &path,
            "FLAT",
            (32, 32),
            "2026-08-19T12:00:00",
            &[
                "SITELAT =              38.6606",
                "SITELONG=             238.8368",
            ],
            |_, _| 20_000.0,
        );
        let meta = crate::commands::import::headers::read_frame_meta(&path);
        let longitude = Site::of(&meta).longitude.unwrap();
        assert!((longitude + 121.1632).abs() < 1e-6, "{longitude}");
        assert_eq!(solar_noon(longitude), 20 * HOUR + 5 * 60);
    }

    #[test]
    fn imports_and_the_backfill_record_a_rig_site() {
        let conn = Connection::open_in_memory().unwrap();
        super::super::ensure_schema(&conn).unwrap();
        let newer = FrameMeta {
            timestamp: Some(utc("2026-08-19T11:58:36")),
            site_longitude: Some(C925_LONGITUDE),
            site_latitude: Some(38.66),
            ..Default::default()
        };
        record_site(&conn, "rig", &newer).unwrap();
        // An older frame from before the profile moved changes nothing.
        let older = FrameMeta {
            timestamp: Some(utc("2026-05-19T11:58:36")),
            site_longitude: Some(-100.0),
            site_latitude: Some(30.0),
            ..Default::default()
        };
        record_site(&conn, "rig", &older).unwrap();
        let sites = read_sites(&conn);
        assert_eq!(sites["rig"].longitude, Some(C925_LONGITUDE));
        // With no file of the pass readable, as when the volume is away,
        // nothing is marked: the frame stays pending, to be tried again.
        conn.execute(
            "INSERT INTO psf_guard_calibration_frame (
                frame_uuid, rig_uuid, kind, source_path, source_fingerprint,
                captured_at, added_at, updated_at)
             VALUES ('f', 'away', 'flat', '/nowhere/flat.fits', 'x', 0, 0, 0)",
            [],
        )
        .unwrap();
        let outcome = backfill_rig_sites(&conn, || true, || {}).unwrap();
        assert_eq!(
            outcome,
            SiteBackfillOutcome {
                pending: 1,
                volume_away: true,
                ..Default::default()
            }
        );
        assert!(!read_sites(&conn).contains_key("away"));
        let unread: Option<i64> = conn
            .query_row(
                "SELECT site_read_at FROM psf_guard_calibration_frame WHERE frame_uuid = 'f'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(unread, None, "not marked away");
    }

    /// A home frame with no longitude of its own (its file away, or a
    /// header that names only its clock) splits by its own clock, else by
    /// the site its rig was seen at nearest in time: one star-party import
    /// does not move the home dusk run's split to Australia's noon.
    #[test]
    fn home_frames_without_a_longitude_keep_their_nights_after_a_trip() {
        let _policy = super::super::tests::POLICY_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        for clock_only in [true, false] {
            let mut frames: Vec<FrameMeta> = dawn_run(true)
                .into_iter()
                .enumerate()
                .map(|(index, mut frame)| {
                    let at = utc("2026-08-19T02:01:00") + index as i64 * 30;
                    frame.timestamp = Some(at);
                    let text = |at: i64| {
                        chrono::DateTime::from_timestamp(at, 0)
                            .unwrap()
                            .naive_utc()
                            .format("%Y-%m-%dT%H:%M:%S")
                            .to_string()
                    };
                    frame.date_obs_utc = Some(text(at));
                    // PDT on the rig computer.
                    frame.date_local = Some(text(at - 7 * HOUR));
                    if clock_only {
                        frame.site_longitude = None;
                        frame.site_latitude = None;
                    }
                    frame
                })
                .collect();
            let (conn, _rig) = library(&frames);
            if !clock_only {
                // Files away since import: no site of their own.
                conn.execute(
                    "UPDATE psf_guard_calibration_frame
                     SET site_longitude = NULL, site_utc_offset_s = NULL, site_read_at = -1",
                    [],
                )
                .unwrap();
            }
            let (plans, _) = sessions(&conn);
            assert_eq!(plans, vec![("2026-08-18".to_string(), 10)], "at home");
            let mut away = frames[0].clone();
            away.path = "/nowhere/away.fits".into();
            away.filter = Some("HA".into());
            away.timestamp = Some(utc("2026-09-20T09:00:00"));
            away.date_obs_utc = None;
            away.date_local = None;
            away.site_longitude = Some(149.07);
            away.site_latitude = Some(-31.27);
            frames.push(away);
            let tx = conn.unchecked_transaction().unwrap();
            super::super::import_calibration_frames(&tx, &frames[10..], Some("profile")).unwrap();
            tx.commit().unwrap();
            let (plans, _) = sessions(&conn);
            assert!(
                plans.contains(&("2026-08-18".to_string(), 10)),
                "clock_only={clock_only}: the California run split after a trip: {plans:?}"
            );
        }
    }

    /// A commit that fails past its retries (here a trigger) ends the pass
    /// with its error, keeping what earlier chunks committed, rather than
    /// failing it: the rigs those chunks placed are reported.
    #[test]
    fn a_failed_chunk_keeps_and_reports_what_earlier_chunks_placed() {
        let _policy = super::super::tests::POLICY_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let mut frames = Vec::new();
        for index in 0..(SITE_BACKFILL_CHUNK + 1) {
            let path = temp.path().join(format!("f{index}.fits"));
            let at = utc("2026-08-19T12:00:00") + index as i64 * 60;
            let text = |at: i64| {
                chrono::DateTime::from_timestamp(at, 0)
                    .unwrap()
                    .naive_utc()
                    .format("%Y-%m-%dT%H:%M:%S")
                    .to_string()
            };
            let card = format!("DATE-LOC= '{}'", text(at - 7 * HOUR));
            crate::calibration::test_frames::write_frame_with(
                &path,
                "FLAT",
                (16, 16),
                &text(at),
                &[card.as_str()],
                |_, _| 20_000.0,
            );
            frames.push(crate::commands::import::headers::read_frame_meta(&path));
        }
        let (conn, _rig) = library(&frames);
        conn.execute_batch(
            "DELETE FROM psf_guard_rig_site;
             DELETE FROM psf_guard_rig_site_seen;
             UPDATE psf_guard_calibration_frame
             SET site_longitude = NULL, site_utc_offset_s = NULL, site_read_at = NULL;
             CREATE TRIGGER busy BEFORE UPDATE OF site_read_at ON psf_guard_calibration_frame
             WHEN NEW.id = (SELECT MIN(id) FROM psf_guard_calibration_frame)
             BEGIN SELECT RAISE(ABORT, 'database is locked'); END;",
        )
        .unwrap();
        let mut reported: Vec<String> = Vec::new();
        let result = backfill_frame_sites(
            &conn,
            SiteBackfill {
                keep_going: &|| true,
                yield_now: &|| {},
                progress: &mut |_, _| {},
                placed: &mut |rigs| reported.extend_from_slice(rigs),
            },
        );
        assert_eq!(
            reported,
            vec![_rig.clone()],
            "the first chunk's rig, as it committed"
        );
        assert!(result.as_ref().is_ok_and(|outcome| outcome.error.is_some()));
        let placed: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM psf_guard_calibration_frame WHERE site_read_at > 0",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            placed, SITE_BACKFILL_CHUNK as i64,
            "the first chunk committed"
        );
        assert!(
            result.is_ok(),
            "the pass failed, and the rigs it placed went unreported: {result:?}"
        );
    }

    /// Frames the backfill reads, for the busy and away tests: `count`
    /// flats on disk with a PDT clock, imported, their sites then cleared.
    fn unread_library(dir: &Path, count: usize) -> (Connection, String, std::path::PathBuf) {
        let mut frames = Vec::new();
        for index in 0..count {
            let path = dir.join(format!("f{index}.fits"));
            let at = utc("2026-08-19T12:00:00") + index as i64 * 60;
            let text = |at: i64| {
                chrono::DateTime::from_timestamp(at, 0)
                    .unwrap()
                    .naive_utc()
                    .format("%Y-%m-%dT%H:%M:%S")
                    .to_string()
            };
            let card = format!("DATE-LOC= '{}'", text(at - 7 * HOUR));
            crate::calibration::test_frames::write_frame_with(
                &path,
                "FLAT",
                (16, 16),
                &text(at),
                &[card.as_str()],
                |_, _| 20_000.0,
            );
            frames.push(crate::commands::import::headers::read_frame_meta(&path));
        }
        let path = dir.join("catalog.sqlite");
        let mut conn = Connection::open(&path).unwrap();
        let tx = conn.transaction().unwrap();
        super::super::import_calibration_frames(&tx, &frames, Some("profile")).unwrap();
        tx.commit().unwrap();
        conn.execute_batch(
            "DELETE FROM psf_guard_rig_site;
             DELETE FROM psf_guard_rig_site_seen;
             UPDATE psf_guard_calibration_frame
             SET site_longitude = NULL, site_utc_offset_s = NULL, site_read_at = NULL;",
        )
        .unwrap();
        let rig = conn
            .query_row("SELECT rig_uuid FROM psf_guard_rig", [], |row| row.get(0))
            .unwrap();
        (conn, rig, path)
    }

    #[test]
    fn a_busy_commit_is_tried_again() {
        let _policy = super::super::tests::POLICY_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let (conn, rig, path) = unread_library(temp.path(), 3);
        conn.busy_timeout(Duration::from_millis(0)).unwrap();
        // Another writer holds the catalog for a moment.
        let other = Connection::open(&path).unwrap();
        other.execute_batch("BEGIN IMMEDIATE;").unwrap();
        let release = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            other.execute_batch("COMMIT;").unwrap();
        });
        let outcome = backfill_rig_sites(&conn, || true, || {}).unwrap();
        release.join().unwrap();
        assert_eq!(outcome.error, None);
        assert_eq!((outcome.clocked, outcome.placed), (3, vec![rig]));
    }

    #[test]
    fn a_file_away_among_readable_ones_is_marked_and_tried_again_later() {
        let _policy = super::super::tests::POLICY_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let (conn, _rig, _path) = unread_library(temp.path(), 3);
        std::fs::remove_file(temp.path().join("f1.fits")).unwrap();
        let outcome = backfill_rig_sites(&conn, || true, || {}).unwrap();
        assert_eq!(
            (outcome.clocked, outcome.unreadable, outcome.volume_away),
            (2, 1, false)
        );
        // Marked away: not read again for a week.
        assert_eq!(
            backfill_rig_sites(&conn, || true, || {}).unwrap().pending,
            0
        );
        let away: i64 = conn
            .query_row(
                "SELECT site_read_at FROM psf_guard_calibration_frame
                 WHERE source_path LIKE '%f1.fits'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(away < 0);
        // A week on, it is.
        conn.execute(
            "UPDATE psf_guard_calibration_frame SET site_read_at = site_read_at + ?1
             WHERE site_read_at < 0",
            [SITE_RETRY_SECONDS + 60],
        )
        .unwrap();
        assert_eq!(
            backfill_rig_sites(&conn, || true, || {}).unwrap().pending,
            1
        );
    }

    #[test]
    fn a_row_an_import_rewrote_while_it_was_read_is_left_alone() {
        let _policy = super::super::tests::POLICY_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let (conn, rig, _path) = unread_library(temp.path(), 1);
        let (id, path): (i64, String) = conn
            .query_row(
                "SELECT id, source_path FROM psf_guard_calibration_frame",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        // Read under its old path; an import has since moved it.
        conn.execute(
            "UPDATE psf_guard_calibration_frame SET source_path = '/moved.fits'",
            [],
        )
        .unwrap();
        let read = [ReadFrame {
            id,
            rig: &rig,
            path: &path,
            captured_at: None,
            site: Some(Site {
                longitude: None,
                utc_offset_s: Some(-7 * HOUR),
            }),
        }];
        assert!(write_chunk(&conn, &read, 1).unwrap().is_empty());
        let stamped: Option<i64> = conn
            .query_row(
                "SELECT site_read_at FROM psf_guard_calibration_frame",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(stamped, None);
    }

    /// A home frame that records no clock of its own takes the clock its
    /// rig kept nearest in time, not a trip's: one week in Hawaii (UTC−10)
    /// does not move the summer's home nights, nor stick after it.
    #[test]
    fn a_trip_west_does_not_move_the_clock_of_home_frames_that_record_none() {
        let _policy = super::super::tests::POLICY_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let text = |at: i64| {
            chrono::DateTime::from_timestamp(at, 0)
                .unwrap()
                .naive_utc()
                .format("%Y-%m-%dT%H:%M:%S")
                .to_string()
        };
        let frame = |name: &str, at: i64, offset: i64| FrameMeta {
            path: format!("/nowhere/{name}.fits").into(),
            readable: true,
            image_type: Some("FLAT".into()),
            filter: Some("L".into()),
            timestamp: Some(at),
            date_obs_utc: Some(text(at)),
            date_local: Some(text(at + offset)),
            exposure_s: Some(2.0),
            telescope: Some("C925".into()),
            camera: Some("ZWO ASI2600MM Pro".into()),
            ..Default::default()
        };
        let frames = [
            frame("home-aug", utc("2026-08-19T03:00:00"), -7 * HOUR),
            frame("hawaii-sep", utc("2026-09-20T06:00:00"), -10 * HOUR),
            frame("home-dec", utc("2026-12-11T02:00:00"), -8 * HOUR),
        ];
        let (conn, rig) = library(&frames);
        // The August frame's own clock is lost, as for a file now away.
        conn.execute(
            "UPDATE psf_guard_calibration_frame
             SET site_utc_offset_s = NULL, site_read_at = -1
             WHERE source_path LIKE '%home-aug.fits'",
            [],
        )
        .unwrap();
        let home: String = conn
            .query_row(
                "SELECT frame_uuid FROM psf_guard_calibration_frame
                 WHERE source_path LIKE '%home-aug.fits'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let nights = Nights::load(&conn);
        // PDT, as the import saw it then: 20:00 on 08-18 is that evening's.
        assert_eq!(
            nights.frame(&rig, &home, utc("2026-08-19T03:00:00")),
            RigNight {
                boundary_seconds: 19 * HOUR,
                source: NightSource::Clock
            }
        );
        assert_eq!(
            nights.clock_offset(&rig),
            Some(-8 * HOUR),
            "the newest, not the smallest"
        );
    }
}
