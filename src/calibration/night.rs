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
//! 4. its rig's dominant place: the longitude, or for a profile with no
//!    site the clock, recorded on the most nights, the latest winning a tie;
//! 5. the catalog's quiet hours, where Images splits nights
//!    ([`crate::server::sky_coverage::night_boundary`]).
//!
//! A rig is assumed to live somewhere: an observatory, or a home its
//! travels return to. A frame shot away records its own site, and so
//! splits where it was; a frame that records none (its file away, a
//! header from other software) splits where its rig usually is. The
//! server's own time zone is never used: it often runs somewhere else, or
//! on UTC. Neither the setting nor a longitude moves with daylight saving;
//! a frame's own clock does, which is the clock it was shot by. An hour
//! either side of noon is still the middle of the day, so no night notices.
//!
//! A split is carried as a shift, not a time of day: a night is the UTC
//! date of `at − shift`, with the shift `12 h − offset` left unwrapped, so
//! a site past +12 h (New Zealand in summer, Tonga) or close to 180° W
//! names its night by the local date of its evening.
//!
//! Sites are kept per frame on `psf_guard_calibration_frame`, and per rig
//! in `psf_guard_rig_site` (the newest) and `psf_guard_rig_site_night`
//! (the nights each place was recorded on, one row a night). An import
//! records them from each frame's header, lights' too; [`backfill_frame_sites`]
//! reads them for frames imported before, as the server's frame-sites job.

use super::rules::{rules_for_rig, LightNightSplit};
use crate::commands::import::headers::{parse_fits_datetime, FrameMeta};
use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

const NOON: i64 = 12 * 3_600;

/// Frames whose headers are read between commits.
pub const SITE_BACKFILL_CHUNK: usize = 100;

/// How long a frame whose file was away waits before it is read again.
pub const SITE_RETRY_SECONDS: i64 = 7 * 86_400;

/// How often a pass hands on the rigs it placed, at most: each hand-on
/// grades the catalog again, so not once a chunk, but often enough that
/// flat measuring does not wait for the whole pass.
pub const SITE_FLUSH_EVERY: Duration = Duration::from_secs(45);

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
    CREATE TABLE IF NOT EXISTS psf_guard_rig_site_night (
        rig_uuid     TEXT NOT NULL,
        place        TEXT NOT NULL,
        night        TEXT NOT NULL,
        longitude    REAL,
        utc_offset_s INTEGER,
        PRIMARY KEY (rig_uuid, place, night)
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
    if !table_exists(conn, "psf_guard_rig_site") || !table_exists(conn, "psf_guard_rig_site_night")
    {
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

    /// Where it says the rig is: its longitude to the hundredth of a
    /// degree (about a kilometre), or, without one, its clock.
    fn place(&self) -> Option<String> {
        match (self.longitude, self.utc_offset_s) {
            (Some(longitude), _) => Some(format!("lon:{longitude:.2}")),
            (None, Some(offset)) => Some(format!("clock:{offset}")),
            (None, None) => None,
        }
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

/// A rig's split, from its setting, its place and the catalog's.
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

/// A rig's usual place and clock, from the nights it recorded them on.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Dominant {
    /// The place on the most nights: a longitude, or a clock alone.
    place: Site,
    /// The clock on the most nights, for its times.
    clock: Option<i64>,
}

/// Where each rig's nights split in one catalog, and each frame's.
///
/// Loading reads the catalog's light times (for its quiet hours), each
/// frame's own site and each rig's nights per place: a scan of the frame
/// table per load, the same order of cost as the light scan the catalog's
/// split always took. Callers load once per request or pass.
#[derive(Debug, Clone)]
pub struct Nights {
    catalog: i64,
    rigs: Arc<HashMap<String, RigNight>>,
    dominant: Arc<HashMap<String, Dominant>>,
    frames: Arc<HashMap<String, Site>>,
}

impl seiza_calibration::library::NightSplit for Nights {
    fn boundary(&self, rig_uuid: &str, frame_uuid: &str) -> i64 {
        self.frame(rig_uuid, frame_uuid).boundary_seconds
    }
}

impl Nights {
    /// Every rig and frame of the catalog, as settings and sites say.
    pub fn load(conn: &Connection) -> Self {
        Self::load_with_catalog(conn, super::catalog_night_boundary(conn))
    }

    /// [`Nights::load`] with the catalog's split given, as a caller that
    /// keeps it (`sky_coverage::cached_night_boundary`) has it, so the
    /// load does not read every light's time again.
    pub fn load_with_catalog(conn: &Connection, catalog: i64) -> Self {
        Self {
            frames: Arc::new(read_frame_sites(conn)),
            ..Self::load_rigs(conn, catalog)
        }
    }

    /// Each rig's split without the frames' own sites, which only
    /// calibration frames have: what lights need ([`LightNights`]), at the
    /// cost of the rig and site tables alone, not a scan of every frame.
    pub fn load_rigs(conn: &Connection, catalog: i64) -> Self {
        let dominant = read_dominant(conn);
        let mut rigs = HashMap::new();
        for rig in rig_uuids(conn) {
            let night = resolve(
                rules_for_rig(&rig).night_offset_seconds,
                dominant.get(&rig).map(|usual| &usual.place),
                catalog,
            );
            rigs.insert(rig, night);
        }
        Self {
            catalog,
            rigs: Arc::new(rigs),
            dominant: Arc::new(dominant),
            frames: Arc::new(HashMap::new()),
        }
    }

    /// Every rig split at `catalog`, unless its setting says otherwise.
    pub fn fixed(catalog: i64) -> Self {
        Self {
            catalog,
            rigs: Arc::new(HashMap::new()),
            dominant: Arc::new(HashMap::new()),
            frames: Arc::new(HashMap::new()),
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

    /// A rig's split by its setting and its dominant place: for a frame
    /// that records no site of its own.
    pub fn rig(&self, rig_uuid: &str) -> RigNight {
        self.rigs.get(rig_uuid).copied().unwrap_or_else(|| {
            resolve(
                rules_for_rig(rig_uuid).night_offset_seconds,
                None,
                self.catalog,
            )
        })
    }

    /// One frame's split: its rig's setting, else its own longitude, else
    /// its own clock, else its rig's dominant place.
    pub fn frame(&self, rig_uuid: &str, frame_uuid: &str) -> RigNight {
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
        rig
    }

    /// The shift of a rig's nights, for a frame with no site of its own.
    pub fn boundary(&self, rig_uuid: &str) -> i64 {
        self.rig(rig_uuid).boundary_seconds
    }

    /// The night a rig's frame with no site of its own, shot at `at`,
    /// belongs to.
    pub fn night_of(&self, rig_uuid: &str, at: i64) -> String {
        super::night_of(at, self.boundary(rig_uuid))
    }

    /// The night one frame shot at `at` belongs to.
    pub fn night_of_frame(&self, rig_uuid: &str, frame_uuid: &str, at: i64) -> String {
        super::night_of(at, self.frame(rig_uuid, frame_uuid).boundary_seconds)
    }

    /// The rig's dominant clock: the offset its frames recorded on the most
    /// nights, in seconds ahead of UTC.
    pub fn clock_offset(&self, rig_uuid: &str) -> Option<i64> {
        self.dominant.get(rig_uuid)?.clock
    }

    /// What decides the splits, for a cache key such as calibration
    /// health's: the catalog's split, each rig's split and dominant clock,
    /// and the split of each frame with a site of its own. A light or a
    /// frame at a rig's usual place leaves it alone; reading the sites of
    /// frames imported before they were kept changes it once.
    pub fn digest(&self) -> String {
        let mut parts: Vec<String> = self
            .rigs
            .iter()
            .filter(|(rig, night)| {
                night.boundary_seconds != self.catalog || self.dominant.contains_key(*rig)
            })
            .map(|(rig, night)| {
                format!(
                    "{rig}={}/{:?}",
                    night.boundary_seconds,
                    self.dominant.get(rig).and_then(|usual| usual.clock)
                )
            })
            .collect();
        parts.sort();
        let mut digest = self.catalog.to_string();
        for part in &parts {
            digest.push(',');
            digest.push_str(part);
        }
        let mut frames: Vec<String> = self
            .frames
            .iter()
            .map(|(uuid, site)| {
                let night = match (site.longitude, site.utc_offset_s) {
                    (Some(longitude), _) => solar_noon(longitude),
                    (None, Some(offset)) => local_noon(offset),
                    (None, None) => 0,
                };
                format!("{uuid}:{night}")
            })
            .collect();
        if !frames.is_empty() {
            frames.sort();
            use sha2::Digest;
            let hash = sha2::Sha256::digest(frames.join(";").as_bytes());
            digest.push_str(&format!(
                ",frames={}",
                hash[..8]
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>()
            ));
        }
        digest
    }
}

/// Where lights split, for the views that group them by night: Images'
/// date groups and night marks, Sequence's marks, the detail's next and
/// previous, and the calibration reports. A light carries no site in the
/// catalog, but its Target Scheduler profile is bound to the rigs it was
/// shot with (`psf_guard_rig_binding`, written as calibration frames and
/// lights are imported), and those rigs know their local noon ([`Nights`]).
///
/// A profile's lights split as its rigs do. Where its rigs disagree, as
/// two cameras of one profile placed by different means, the surest source
/// wins (a setting, then a site, then a clock), then the split most of
/// them share. A light whose profile no rig is bound to splits at the
/// global night setting if there is one, else where every placed rig of
/// the catalog agrees, else at the catalog's quiet hours, as before.
///
/// The `light_nights` setting can put a rig's lights, or every light that
/// no rig is bound to, back on the catalog's quiet hours. A view's override
/// (`nights=` in its link) sets one split for every light it shows.
#[derive(Debug, Clone)]
pub struct LightNights {
    by_profile: HashMap<String, RigNight>,
    unbound: RigNight,
}

impl LightNights {
    pub fn load(conn: &Connection, view: Option<LightNightSplit>) -> Self {
        Self::from_nights(conn, &Nights::load(conn), view)
    }

    /// [`LightNights::load`] with the catalog's split given, reading the
    /// rig tables alone ([`Nights::load_rigs`]).
    pub fn load_with_catalog(
        conn: &Connection,
        catalog: i64,
        view: Option<LightNightSplit>,
    ) -> Self {
        Self::from_nights(conn, &Nights::load_rigs(conn, catalog), view)
    }

    /// Each profile's split from `nights`. `view` overrides the
    /// `light_nights` setting for every light, as a view's link asks.
    pub fn from_nights(conn: &Connection, nights: &Nights, view: Option<LightNightSplit>) -> Self {
        let catalog = RigNight {
            boundary_seconds: nights.catalog,
            source: NightSource::Catalog,
        };
        if view == Some(LightNightSplit::Catalog) {
            return Self {
                by_profile: HashMap::new(),
                unbound: catalog,
            };
        }
        // No rig is called "": its rules are the global ones.
        let split_of = |rig: &str| view.unwrap_or_else(|| rules_for_rig(rig).light_nights);

        let mut rigs_by_profile: HashMap<String, Vec<(String, RigNight)>> = HashMap::new();
        if table_exists(conn, "psf_guard_rig_binding") {
            let bindings: Vec<(String, String)> = conn
                .prepare("SELECT profile_id, rig_uuid FROM psf_guard_rig_binding")
                .and_then(|mut statement| {
                    statement
                        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                        .collect::<rusqlite::Result<Vec<_>>>()
                })
                .unwrap_or_default();
            for (profile, rig) in bindings {
                let night = nights.rig(&rig);
                rigs_by_profile
                    .entry(profile)
                    .or_default()
                    .push((rig, night));
            }
        }
        // A profile's lights follow its surest rig, and that rig's setting
        // says whether they split at its noon or the catalog's.
        let by_profile = rigs_by_profile
            .into_iter()
            .filter_map(|(profile, rigs)| {
                let (rig, night) = surest(&rigs)?;
                let night = match split_of(&rig) {
                    LightNightSplit::Rig => night,
                    LightNightSplit::Catalog => catalog,
                };
                Some((profile, night))
            })
            .collect();

        let global = nights.rig("");
        // Rigs placed by a setting, a site or a clock, whose own lights
        // split at their noon: a rig set to the catalog split lends its
        // noon to no other light.
        let placed_rigs: Vec<RigNight> = nights
            .rigs
            .iter()
            .filter(|(rig, night)| {
                night.source != NightSource::Catalog && split_of(rig) == LightNightSplit::Rig
            })
            .map(|(_, night)| *night)
            .collect();
        let placed: BTreeSet<i64> = placed_rigs
            .iter()
            .map(|night| night.boundary_seconds)
            .collect();
        let unbound = if split_of("") == LightNightSplit::Catalog {
            catalog
        } else if global.source == NightSource::Setting {
            global
        } else if placed.len() == 1 {
            placed_rigs
                .iter()
                .copied()
                .min_by_key(|night| source_rank(night.source))
                .unwrap_or(global)
        } else {
            catalog
        };
        Self {
            by_profile,
            unbound,
        }
    }

    /// The split of a light shot under `profile_id`.
    pub fn of_profile(&self, profile_id: Option<&str>) -> RigNight {
        profile_id
            .and_then(|profile| self.by_profile.get(profile))
            .copied()
            .unwrap_or(self.unbound)
    }

    /// The night a light shot at `at` under `profile_id` belongs to.
    pub fn night_of(&self, profile_id: Option<&str>, at: i64) -> String {
        super::night_of(at, self.of_profile(profile_id).boundary_seconds)
    }
}

fn source_rank(source: NightSource) -> u8 {
    match source {
        NightSource::Setting => 0,
        NightSource::Site => 1,
        NightSource::Clock => 2,
        NightSource::Catalog => 3,
    }
}

/// The rig whose split a profile's lights take: the surest source, then
/// the split most of them share, then the earliest split, then the first
/// rig by id.
fn surest(rigs: &[(String, RigNight)]) -> Option<(String, RigNight)> {
    let best = rigs
        .iter()
        .map(|(_, night)| source_rank(night.source))
        .min()?;
    let mut counts: BTreeMap<i64, usize> = BTreeMap::new();
    for (_, night) in rigs
        .iter()
        .filter(|(_, night)| source_rank(night.source) == best)
    {
        *counts.entry(night.boundary_seconds).or_default() += 1;
    }
    let (boundary_seconds, _) = counts
        .iter()
        .max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0)))?;
    rigs.iter()
        .filter(|(_, night)| {
            night.boundary_seconds == *boundary_seconds && source_rank(night.source) == best
        })
        .min_by(|a, b| a.0.cmp(&b.0))
        .cloned()
}

/// From when a rig's clock kept an offset: the time of the first frame
/// that recorded it, and the offset in seconds ahead of UTC.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ClockRun {
    pub from: i64,
    pub offset_seconds: i64,
}

/// A rig's clock over time, as its frames' headers record it, oldest
/// first, one run per change (daylight saving, a move): what its frames'
/// times are shown on. Its `night_zone` setting, when it has one, is the
/// one run. Empty when neither says, and times are shown on the viewer's
/// clock.
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

/// One row of `psf_guard_rig_site_night`: rig, place, night, longitude,
/// clock.
type NightRow = (String, String, String, Option<f64>, Option<i64>);

/// Each rig's dominant place and clock: counted once a night, the latest
/// night winning a tie.
fn read_dominant(conn: &Connection) -> HashMap<String, Dominant> {
    if !table_exists(conn, "psf_guard_rig_site_night") {
        return HashMap::new();
    }
    let rows: Vec<NightRow> = conn
        .prepare(
            "SELECT rig_uuid, place, night, longitude, utc_offset_s
             FROM psf_guard_rig_site_night",
        )
        .and_then(|mut statement| {
            statement
                .query_map([], |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()
        })
        .unwrap_or_default();
    // Per rig: per place its nights, latest night and site; per clock its
    // nights and latest night.
    type Tally<'a> = BTreeMap<String, (BTreeSet<&'a str>, &'a str, Site)>;
    let mut places: HashMap<&str, Tally<'_>> = HashMap::new();
    let mut clocks: HashMap<&str, BTreeMap<i64, (BTreeSet<&str>, &str)>> = HashMap::new();
    for (rig, place, night, longitude, offset) in &rows {
        let site = Site {
            longitude: *longitude,
            utc_offset_s: if longitude.is_some() { None } else { *offset },
        };
        let entry = places
            .entry(rig.as_str())
            .or_default()
            .entry(place.clone())
            .or_insert_with(|| (BTreeSet::new(), night.as_str(), site));
        entry.0.insert(night.as_str());
        if night.as_str() > entry.1 {
            entry.1 = night.as_str();
        }
        if let Some(offset) = offset {
            let clock = clocks
                .entry(rig.as_str())
                .or_default()
                .entry(*offset)
                .or_insert_with(|| (BTreeSet::new(), night.as_str()));
            clock.0.insert(night.as_str());
            if night.as_str() > clock.1 {
                clock.1 = night.as_str();
            }
        }
    }
    places
        .into_iter()
        .map(|(rig, tally)| {
            let place = tally
                .values()
                .max_by(|a, b| a.0.len().cmp(&b.0.len()).then(a.1.cmp(b.1)))
                .map(|(_, _, site)| *site)
                .unwrap_or_default();
            let clock = clocks.get(rig).and_then(|clocks| {
                clocks
                    .iter()
                    .max_by(|a, b| a.1 .0.len().cmp(&b.1 .0.len()).then(a.1 .1.cmp(b.1 .1)))
                    .map(|(offset, _)| *offset)
            });
            (rig.to_string(), Dominant { place, clock })
        })
        .collect()
}

/// Each rig's newest site, as `psf_guard_rig_site` keeps it.
#[cfg(test)]
fn read_sites(conn: &Connection) -> HashMap<String, Site> {
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

/// Each frame's own site, for the frames that record one.
fn read_frame_sites(conn: &Connection) -> HashMap<String, Site> {
    if !frame_columns_exist(conn) {
        return HashMap::new();
    }
    conn.prepare(
        "SELECT frame_uuid, site_longitude, site_utc_offset_s FROM psf_guard_calibration_frame
         WHERE site_longitude IS NOT NULL OR site_utc_offset_s IS NOT NULL",
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
                ))
            })?
            .collect::<rusqlite::Result<HashMap<_, _>>>()
    })
    .unwrap_or_default()
}

/// Keep what an imported frame, a light's or a calibration frame's, says
/// about its rig's site: the rig's newest site, and the night it was seen
/// at that place. A frame that says nothing changes nothing.
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

/// The night a capture counts toward its rig's place by: its UTC date
/// twelve hours earlier, which keeps a night's evening and dawn together
/// for any rig west of Greenwich and is all a count needs.
fn tally_night(at: i64) -> String {
    super::night_of(at, NOON)
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
    if let (Some(at), Some(place)) = (captured_at, site.place())
        && table_exists(conn, "psf_guard_rig_site_night")
    {
        // One row a night and place: a second frame, or a light, of a
        // night already counted changes nothing.
        conn.execute(
            "INSERT OR IGNORE INTO psf_guard_rig_site_night
                (rig_uuid, place, night, longitude, utc_offset_s)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                rig_uuid,
                place,
                tally_night(at),
                site.longitude,
                site.utc_offset_s
            ],
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
    /// mounted: nothing was marked, to be tried again.
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
    /// The rigs committed chunks placed: at most every
    /// [`SITE_FLUSH_EVERY`], and once more when the pass ends, early or
    /// not, so it has handed on all it did.
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
struct ReadFrame {
    id: i64,
    rig: String,
    path: String,
    captured_at: Option<i64>,
    site: Option<Site>,
    /// Whether no pass had read it before: not yet marked away.
    never_read: bool,
}

/// Whether frames marked away are due to be read again: the frame-sites
/// job queues a pass for them.
pub fn away_frames_due(conn: &Connection) -> bool {
    if !frame_columns_exist(conn) {
        return false;
    }
    let due = chrono::Utc::now().timestamp() - SITE_RETRY_SECONDS;
    conn.query_row(
        "SELECT 1 FROM psf_guard_calibration_frame
         WHERE site_read_at < 0 AND -site_read_at <= ?1 LIMIT 1",
        [due],
        |_| Ok(()),
    )
    .optional()
    .ok()
    .flatten()
    .is_some()
}

/// Read the site of every calibration frame imported before the catalog
/// kept one, off the open path, a hundred headers between commits, each
/// frame's rig keeping what its frames said ([`write_site`]).
///
/// Files that cannot be read before any file of the pass was read are held
/// back unmarked: once one is read, they are marked away with the rest;
/// if none ever is, as when the volume is not mounted, nothing is marked
/// and the pass reports `volume_away`. A pass of frames that were all
/// marked away already, and still cannot be read (files deleted, most
/// likely), marks them away again, to wait another week, rather than
/// calling the volume away. A commit the catalog is too busy for
/// is tried again; one that still fails ends the pass with its `error`,
/// keeping what earlier chunks committed. A write lands only on a row an
/// import has not rewritten since it was read.
pub fn backfill_frame_sites(
    conn: &Connection,
    hooks: SiteBackfill<'_>,
) -> Result<SiteBackfillOutcome> {
    backfill_with(conn, hooks, SITE_FLUSH_EVERY)
}

fn backfill_with(
    conn: &Connection,
    hooks: SiteBackfill<'_>,
    flush_every: Duration,
) -> Result<SiteBackfillOutcome> {
    if !super::schema_exists(conn)
        || !table_exists(conn, "psf_guard_rig_site")
        || !frame_columns_exist(conn)
    {
        return Ok(SiteBackfillOutcome::default());
    }
    let now = chrono::Utc::now().timestamp();
    let pending: Vec<(i64, String, String, Option<i64>, bool)> = conn
        .prepare(
            "SELECT id, rig_uuid, source_path, captured_at, site_read_at IS NULL
             FROM psf_guard_calibration_frame
             WHERE site_read_at IS NULL OR (site_read_at < 0 AND -site_read_at <= ?1)
             ORDER BY rig_uuid, captured_at DESC, id DESC",
        )?
        .query_map([now - SITE_RETRY_SECONDS], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let mut outcome = SiteBackfillOutcome {
        pending: pending.len(),
        ..Default::default()
    };
    (hooks.progress)(0, pending.len());
    let mut placed: BTreeSet<String> = BTreeSet::new();
    let mut unflushed: BTreeSet<String> = BTreeSet::new();
    let mut flushed_at = Instant::now();
    // Files away before any file was read: marked only once one is.
    let mut held: Vec<ReadFrame> = Vec::new();
    for chunk in pending.chunks(SITE_BACKFILL_CHUNK) {
        if !(hooks.keep_going)() {
            break;
        }
        (hooks.yield_now)();
        if !(hooks.keep_going)() {
            break;
        }
        let read: Vec<ReadFrame> = chunk
            .iter()
            .map(|(id, rig, path, captured_at, never_read)| {
                let meta = crate::commands::import::headers::read_frame_meta(Path::new(path));
                ReadFrame {
                    id: *id,
                    rig: rig.clone(),
                    path: path.clone(),
                    captured_at: *captured_at,
                    site: meta.readable.then(|| Site::of(&meta)),
                    never_read: *never_read,
                }
            })
            .collect();
        let none_read = held.len() == outcome.done && read.iter().all(|f| f.site.is_none());
        outcome.done += read.len();
        if none_read {
            held.extend(read);
            (hooks.progress)(outcome.done, pending.len());
            continue;
        }
        let writes: Vec<ReadFrame> = std::mem::take(&mut held).into_iter().chain(read).collect();
        let mut tries = 0;
        let chunk_placed = loop {
            match write_chunk(conn, &writes, now) {
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
            outcome.done -= writes.len();
            break;
        };
        for frame in &writes {
            match (frame.site.as_ref(), frame.site.and_then(|s| s.longitude)) {
                (Some(_), Some(_)) => outcome.located += 1,
                (Some(site), None) if site.utc_offset_s.is_some() => outcome.clocked += 1,
                (Some(_), None) => outcome.unplaced += 1,
                (None, _) => outcome.unreadable += 1,
            }
        }
        unflushed.extend(chunk_placed.iter().cloned());
        placed.extend(chunk_placed);
        if !unflushed.is_empty() && flushed_at.elapsed() >= flush_every {
            let rigs: Vec<String> = std::mem::take(&mut unflushed).into_iter().collect();
            (hooks.placed)(&rigs);
            flushed_at = Instant::now();
        }
        (hooks.progress)(outcome.done, pending.len());
    }
    if !held.is_empty() && held.len() == outcome.done {
        if held.iter().any(|frame| frame.never_read) {
            // Nothing of the pass could be read, some of it never read
            // before: the volume is away. Leave it all to a later pass.
            outcome.volume_away = true;
            outcome.done = 0;
        } else {
            // Only frames already marked away, still unreadable: most
            // likely deleted. They wait another week.
            match write_chunk(conn, &held, now) {
                Ok(_) => outcome.unreadable += held.len(),
                Err(error) => {
                    outcome.done = 0;
                    outcome.error = Some(format!("{error}"));
                }
            }
        }
    }
    if !unflushed.is_empty() {
        let rigs: Vec<String> = unflushed.into_iter().collect();
        (hooks.placed)(&rigs);
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

/// One batch of sites in one commit; the rigs it placed.
fn write_chunk(conn: &Connection, read: &[ReadFrame], now: i64) -> rusqlite::Result<Vec<String>> {
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
                    write_site(&tx, &frame.rig, site, frame.captured_at)?;
                    placed.insert(frame.rig.clone());
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
        assert_eq!(c925().digest(), "43200,c925=72300/None");
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
             DELETE FROM psf_guard_rig_site_night;
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
             DELETE FROM psf_guard_rig_site_night;
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
        assert_eq!(nights.frame(&rig, &summer).boundary_seconds, 19 * HOUR);
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

    /// A light shot at `site`, as an import notes its rig: a camera of its
    /// own per name, bound to `profile`.
    fn note_light(conn: &Connection, profile: &str, camera: &str, site: Option<f64>) {
        let light = FrameMeta {
            camera: Some(camera.into()),
            telescope: Some("scope".into()),
            width: Some(6248),
            height: Some(4176),
            timestamp: Some(utc("2026-08-19T05:00:00")),
            site_longitude: site,
            site_latitude: site.map(|_| 38.66),
            ..Default::default()
        };
        super::super::note_light_rig(conn, Some(profile), &light).unwrap();
    }

    /// Lights split at their profile's rig's local noon, not the catalog's
    /// quiet hours: an evening at the C925's site west of Greenwich and
    /// the small hours after it are one night, named by the evening.
    #[test]
    fn lights_split_where_their_profiles_rig_does() {
        let _policy = super::super::tests::POLICY_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let conn = Connection::open_in_memory().unwrap();
        super::super::ensure_schema(&conn).unwrap();
        note_light(&conn, "c925", "ASI2600MM", Some(C925_LONGITUDE));
        let lights = LightNights::load(&conn, None);
        let split = lights.of_profile(Some("c925"));
        assert_eq!(split.source, NightSource::Site);
        assert_eq!(split.boundary_seconds, solar_noon(C925_LONGITUDE));
        // 19:00 UTC is 11:55 at the site: still the night before.
        assert_eq!(
            lights.night_of(Some("c925"), utc("2026-08-19T19:00:00")),
            "2026-08-18"
        );
        assert_eq!(
            lights.night_of(Some("c925"), utc("2026-08-19T21:00:00")),
            "2026-08-19"
        );
        assert_eq!(
            lights.night_of(Some("c925"), utc("2026-08-20T11:00:00")),
            "2026-08-19"
        );
        // A profile no rig is bound to splits where the catalog's one placed
        // rig does.
        assert_eq!(lights.of_profile(Some("other")), split);
        assert_eq!(lights.of_profile(None), split);
    }

    /// Where the catalog's placed rigs disagree, a light no rig vouches for
    /// keeps the catalog's split; each bound profile keeps its own.
    #[test]
    fn unbound_lights_keep_the_catalog_split_when_rigs_disagree() {
        let _policy = super::super::tests::POLICY_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let conn = Connection::open_in_memory().unwrap();
        super::super::ensure_schema(&conn).unwrap();
        note_light(&conn, "home", "ASI2600MM", Some(C925_LONGITUDE));
        note_light(&conn, "remote", "ASI6200MM", Some(149.07));
        let lights = LightNights::load(&conn, None);
        assert_eq!(
            lights.of_profile(Some("home")).boundary_seconds,
            solar_noon(C925_LONGITUDE)
        );
        assert_eq!(
            lights.of_profile(Some("remote")).boundary_seconds,
            solar_noon(149.07)
        );
        let unbound = lights.of_profile(Some("other"));
        assert_eq!(unbound.source, NightSource::Catalog);
        assert_eq!(
            unbound.boundary_seconds,
            super::super::catalog_night_boundary(&conn)
        );
    }

    /// The `light_nights` setting puts a rig's lights, or every light, back
    /// on the catalog's quiet hours; a view's override beats the setting.
    #[test]
    fn the_setting_and_a_view_choose_the_rig_or_the_catalog_split() {
        use super::super::rules::{set_test_rule_book, RigLayer, RuleBook};
        let _policy = super::super::tests::POLICY_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let conn = Connection::open_in_memory().unwrap();
        super::super::ensure_schema(&conn).unwrap();
        note_light(&conn, "home", "ASI2600MM", Some(C925_LONGITUDE));
        let rig: String = conn
            .query_row(
                "SELECT rig_uuid FROM psf_guard_rig_binding WHERE profile_id = 'home'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let catalog = super::super::catalog_night_boundary(&conn);
        let split = |view| LightNights::load(&conn, view).of_profile(Some("home"));

        assert_eq!(split(None).boundary_seconds, solar_noon(C925_LONGITUDE));
        let by_catalog = split(Some(LightNightSplit::Catalog));
        assert_eq!(
            (by_catalog.boundary_seconds, by_catalog.source),
            (catalog, NightSource::Catalog)
        );

        // The rig's own setting.
        set_test_rule_book(Some(RuleBook::new(
            BTreeMap::new(),
            BTreeMap::from([(
                rig.clone(),
                RigLayer {
                    name: None,
                    values: BTreeMap::from([(
                        "light_nights".to_string(),
                        serde_json::json!("catalog"),
                    )]),
                },
            )]),
        )));
        assert_eq!(split(None).source, NightSource::Catalog);
        // A view that asks for rig nights shows them anyway.
        assert_eq!(
            split(Some(LightNightSplit::Rig)).boundary_seconds,
            solar_noon(C925_LONGITUDE)
        );
        // A rig set to the catalog split lends its noon to no light that no
        // rig is bound to, though it is the catalog's only placed rig.
        assert_eq!(
            LightNights::load(&conn, None)
                .of_profile(Some("other"))
                .source,
            NightSource::Catalog
        );

        // The global setting: every light, bound or not.
        set_test_rule_book(Some(RuleBook::new(
            BTreeMap::from([("light_nights".to_string(), serde_json::json!("catalog"))]),
            BTreeMap::new(),
        )));
        assert_eq!(split(None).source, NightSource::Catalog);
        assert_eq!(
            LightNights::load(&conn, None)
                .of_profile(Some("other"))
                .source,
            NightSource::Catalog
        );
        set_test_rule_book(None);
    }

    /// Two rigs of one profile placed by different means: the site wins
    /// over a rig placed only by the catalog.
    #[test]
    fn a_profiles_surest_rig_sets_its_split() {
        let catalog = RigNight {
            boundary_seconds: 12 * HOUR,
            source: NightSource::Catalog,
        };
        let site = by_longitude(C925_LONGITUDE);
        let clock = by_clock(-7 * HOUR);
        let rigs = |nights: &[(&str, RigNight)]| -> Vec<(String, RigNight)> {
            nights
                .iter()
                .map(|(rig, night)| (rig.to_string(), *night))
                .collect()
        };
        assert_eq!(
            surest(&rigs(&[("a", catalog), ("b", site), ("c", clock)])),
            Some(("b".to_string(), site))
        );
        assert_eq!(
            surest(&rigs(&[("a", catalog), ("c", clock), ("b", clock)])),
            Some(("b".to_string(), clock))
        );
        assert_eq!(
            surest(&rigs(&[("a", catalog)])),
            Some(("a".to_string(), catalog))
        );
        assert_eq!(surest(&[]), None);
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
    /// its rig's dominant place: one star-party import does not move the
    /// home dusk run's split to Australia's noon.
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
            let (conn, rig) = library(&frames);
            // A home light the next night: the rig lives at home.
            let mut light = frames[0].clone();
            light.timestamp = Some(utc("2026-08-20T04:00:00"));
            light.site_longitude = Some(C925_LONGITUDE);
            light.site_latitude = Some(38.66);
            record_site(&conn, &rig, &light).unwrap();
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
             DELETE FROM psf_guard_rig_site_night;
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
             DELETE FROM psf_guard_rig_site_night;
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

    /// A file marked away a week ago and since deleted: the pass that
    /// comes for it reads nothing, yet that is no volume away. It is marked
    /// away again, to wait another week, so the watch does not queue it
    /// every hour.
    #[test]
    fn a_deleted_file_waits_another_week_and_is_no_volume_away() {
        let _policy = super::super::tests::POLICY_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let (conn, _rig, _path) = unread_library(temp.path(), 2);
        std::fs::remove_file(temp.path().join("f1.fits")).unwrap();
        let first = backfill_rig_sites(&conn, || true, || {}).unwrap();
        assert_eq!((first.clocked, first.unreadable), (1, 1));
        // A week on, only the deleted file is due.
        let week_ago = chrono::Utc::now().timestamp() - SITE_RETRY_SECONDS - 60;
        conn.execute(
            "UPDATE psf_guard_calibration_frame SET site_read_at = ?1 WHERE site_read_at < 0",
            [-week_ago],
        )
        .unwrap();
        assert!(away_frames_due(&conn));
        let again = backfill_rig_sites(&conn, || true, || {}).unwrap();
        assert_eq!(
            (again.pending, again.unreadable, again.volume_away),
            (1, 1, false)
        );
        assert!(!away_frames_due(&conn), "it waits another week");
        let stamped: i64 = conn
            .query_row(
                "SELECT site_read_at FROM psf_guard_calibration_frame
                 WHERE source_path LIKE '%f1.fits'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(stamped < -week_ago);
        // With a frame never read among them, nothing read means the volume.
        conn.execute(
            "UPDATE psf_guard_calibration_frame SET site_read_at = ?1 WHERE site_read_at < 0",
            [-week_ago],
        )
        .unwrap();
        conn.execute(
            "UPDATE psf_guard_calibration_frame SET site_read_at = NULL
             WHERE source_path LIKE '%f0.fits'",
            [],
        )
        .unwrap();
        std::fs::remove_file(temp.path().join("f0.fits")).unwrap();
        let away = backfill_rig_sites(&conn, || true, || {}).unwrap();
        assert!(away.volume_away);
        assert!(away_frames_due(&conn), "nothing marked");
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
            rig: rig.clone(),
            path: path.clone(),
            captured_at: None,
            site: Some(Site {
                longitude: None,
                utc_offset_s: Some(-7 * HOUR),
            }),
            never_read: true,
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

    /// A home frame that records no clock of its own takes its rig's
    /// dominant one, not a trip's: a week in Hawaii (UTC−10) does not move
    /// the home nights, nor stick after it.
    #[test]
    fn a_trip_west_does_not_become_the_clock_of_home_frames_that_record_none() {
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
            frame("home-aug-2", utc("2026-08-21T03:00:00"), -7 * HOUR),
            frame("hawaii-sep", utc("2026-09-20T06:00:00"), -10 * HOUR),
            frame("home-oct", utc("2026-10-11T03:00:00"), -7 * HOUR),
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
            nights.frame(&rig, &home),
            RigNight {
                boundary_seconds: 19 * HOUR,
                source: NightSource::Clock
            }
        );
        assert_eq!(
            nights.clock_offset(&rig),
            Some(-7 * HOUR),
            "the clock on the most nights, not the newest or smallest"
        );
    }

    /// The newest hundred frames' files are gone (moved off, or a rig's
    /// NAS share unmounted while the rig sorted first); an older frame is on
    /// disk. The pass still reads the older one, and then marks the hundred
    /// away.
    #[test]
    fn a_first_chunk_of_files_away_does_not_stall_the_rest() {
        let _policy = super::super::tests::POLICY_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let (conn, _rig, _path) = unread_library(temp.path(), SITE_BACKFILL_CHUNK + 1);
        for index in 1..=SITE_BACKFILL_CHUNK {
            std::fs::remove_file(temp.path().join(format!("f{index}.fits"))).unwrap();
        }
        let outcome = backfill_rig_sites(&conn, || true, || {}).unwrap();
        let read: Option<i64> = conn
            .query_row(
                "SELECT site_read_at FROM psf_guard_calibration_frame
                 WHERE source_path LIKE '%f0.fits'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            read.is_some_and(|at| at > 0),
            "f0 is on disk but never read: {outcome:?}"
        );
        assert_eq!(
            (outcome.unreadable, outcome.clocked, outcome.volume_away),
            (SITE_BACKFILL_CHUNK, 1, false)
        );
        let away: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM psf_guard_calibration_frame WHERE site_read_at < 0",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(away, SITE_BACKFILL_CHUNK as i64);
    }

    /// A home profile with no site set (N.I.N.A. writes 0°, 0°) records only
    /// the clock. One trip with a site set does not beat the home clock its
    /// rig recorded on more nights, for a home frame whose file is away.
    #[test]
    fn a_home_clock_on_more_nights_beats_a_far_longitude() {
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
        let frame = |name: &str, at: i64, offset: Option<i64>, longitude: Option<f64>| FrameMeta {
            path: format!("/nowhere/{name}.fits").into(),
            readable: true,
            image_type: Some("FLAT".into()),
            filter: Some("L".into()),
            timestamp: Some(at),
            date_obs_utc: Some(text(at)),
            date_local: offset.map(|offset| text(at + offset)),
            site_longitude: longitude,
            site_latitude: longitude.map(|_| -31.27),
            exposure_s: Some(2.0),
            telescope: Some("C925".into()),
            camera: Some("ZWO ASI2600MM Pro".into()),
            ..Default::default()
        };
        // Adapted: home is the place on the most nights (two here), so the
        // rig's dominant place is its home clock, not the trip's longitude.
        let frames = [
            frame("home-a", utc("2026-08-19T03:00:00"), Some(-7 * HOUR), None),
            frame("home-b", utc("2026-08-19T03:10:00"), Some(-7 * HOUR), None),
            frame("home-c", utc("2026-08-21T03:00:00"), Some(-7 * HOUR), None),
            frame("trip", utc("2026-09-20T09:00:00"), None, Some(149.07)),
        ];
        let (conn, rig) = library(&frames);
        conn.execute(
            "UPDATE psf_guard_calibration_frame
             SET site_utc_offset_s = NULL, site_read_at = -1
             WHERE source_path LIKE '%home-b.fits'",
            [],
        )
        .unwrap();
        let home: String = conn
            .query_row(
                "SELECT frame_uuid FROM psf_guard_calibration_frame
                 WHERE source_path LIKE '%home-b.fits'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let night = Nights::load(&conn).frame(&rig, &home);
        assert_eq!(
            night,
            by_clock(-7 * HOUR),
            "a home frame took Australia's noon from a trip a month away"
        );
    }

    /// Home lights every month for three years, a star party at the same
    /// far site each May. A November home frame whose file is away keeps
    /// home, the place on the most nights.
    #[test]
    fn home_between_two_trips_to_one_site_keeps_home() {
        let _policy = super::super::tests::POLICY_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let frames = [FrameMeta {
            path: "/nowhere/home-nov.fits".into(),
            readable: true,
            image_type: Some("FLAT".into()),
            filter: Some("L".into()),
            timestamp: Some(utc("2025-11-15T03:00:00")),
            exposure_s: Some(2.0),
            telescope: Some("C925".into()),
            camera: Some("ZWO ASI2600MM Pro".into()),
            ..Default::default()
        }];
        let (conn, rig) = library(&frames);
        let home = Site {
            longitude: Some(-118.0),
            utc_offset_s: None,
        };
        let away = Site {
            longitude: Some(149.07),
            utc_offset_s: None,
        };
        // Home lights monthly, Jan 2024 to Oct 2026, as imports record them.
        let mut at = utc("2024-01-15T06:00:00");
        while at < utc("2026-10-15T06:00:00") {
            write_site(&conn, &rig, &home, Some(at)).unwrap();
            at += 30 * 86_400;
        }
        write_site(&conn, &rig, &away, Some(utc("2025-05-20T10:00:00"))).unwrap();
        write_site(&conn, &rig, &away, Some(utc("2026-05-20T10:00:00"))).unwrap();
        // Home's lights were last written after the trip, so the newest site
        // is home; the frame has no site of its own.
        write_site(&conn, &rig, &home, Some(utc("2026-10-15T06:00:00"))).unwrap();
        let uuid: String = conn
            .query_row(
                "SELECT frame_uuid FROM psf_guard_calibration_frame",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let night = Nights::load(&conn).frame(&rig, &uuid);
        assert_eq!(
            night,
            by_longitude(-118.0),
            "a home frame took the star party's noon"
        );
    }

    /// A pass hands its rigs on at most every [`SITE_FLUSH_EVERY`] and once
    /// at its end, not each chunk: each hand-on grades the catalog again.
    #[test]
    fn one_rig_is_handed_on_once_for_a_short_pass() {
        let _policy = super::super::tests::POLICY_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let temp = tempfile::tempdir().unwrap();
        let (conn, _rig, _path) = unread_library(temp.path(), 2 * SITE_BACKFILL_CHUNK + 1);
        let mut calls = 0;
        backfill_frame_sites(
            &conn,
            SiteBackfill {
                keep_going: &|| true,
                yield_now: &|| {},
                progress: &mut |_, _| {},
                placed: &mut |_| calls += 1,
            },
        )
        .unwrap();
        assert!(calls <= 1, "the same rig was handed on {calls} times");
    }

    /// Every light import records its rig's site (note_light_rig ->
    /// record_site). A light at the rig's usual place splits nothing
    /// elsewhere, so the digest, and every project's calibration-health
    /// key, stays.
    #[test]
    fn a_light_at_the_same_site_leaves_the_digest_alone() {
        let _policy = super::super::tests::POLICY_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let frames = dawn_run(true);
        let (conn, rig) = library(&frames);
        let before = Nights::load(&conn).digest();
        let site = Site::of(&frames[0]);
        assert!(site.longitude.is_some());
        // Tonight's light, same rig, same site.
        let later = frames[0].timestamp.unwrap() + 30 * 86_400;
        write_site(&conn, &rig, &site, Some(later)).unwrap();
        assert_eq!(Nights::load(&conn).digest(), before, "health keys moved");
        // A trip's light splits nothing at home either: home stays dominant.
        let trip = Site {
            longitude: Some(149.07),
            utc_offset_s: None,
        };
        write_site(&conn, &rig, &trip, Some(later + 86_400)).unwrap();
        write_site(&conn, &rig, &site, Some(later + 2 * 86_400)).unwrap();
        assert_eq!(Nights::load(&conn).digest(), before);
    }

    /// With no wait between hand-ons, each committed chunk's rigs go on as
    /// it commits; with the usual wait, a short pass hands them on once.
    #[test]
    fn hand_ons_are_throttled_and_flushed_at_the_end() {
        let _policy = super::super::tests::POLICY_LOCK
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        for (wait, expected) in [(Duration::ZERO, 3), (SITE_FLUSH_EVERY, 1)] {
            let temp = tempfile::tempdir().unwrap();
            let (conn, rig, _path) = unread_library(temp.path(), 2 * SITE_BACKFILL_CHUNK + 1);
            let mut calls: Vec<Vec<String>> = Vec::new();
            backfill_with(
                &conn,
                SiteBackfill {
                    keep_going: &|| true,
                    yield_now: &|| {},
                    progress: &mut |_, _| {},
                    placed: &mut |rigs| calls.push(rigs.to_vec()),
                },
                wait,
            )
            .unwrap();
            assert_eq!(calls.len(), expected, "{wait:?}");
            assert!(calls.iter().all(|rigs| rigs == &vec![rig.clone()]));
        }
        // A pass stopped early still hands on what it committed.
        let temp = tempfile::tempdir().unwrap();
        let (conn, rig, _path) = unread_library(temp.path(), SITE_BACKFILL_CHUNK + 1);
        let chunks = std::cell::Cell::new(0);
        let mut calls: Vec<Vec<String>> = Vec::new();
        backfill_with(
            &conn,
            SiteBackfill {
                keep_going: &|| chunks.get() <= 1,
                yield_now: &|| chunks.set(chunks.get() + 1),
                progress: &mut |_, _| {},
                placed: &mut |rigs| calls.push(rigs.to_vec()),
            },
            SITE_FLUSH_EVERY,
        )
        .unwrap();
        assert_eq!(calls, vec![vec![rig]]);
    }
}
