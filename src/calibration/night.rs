//! Where a rig's nights split.
//!
//! Calibration groups a rig's frames by night: an evening's darks, the
//! flats shot at dusk and those shot at the dawn after it. A night runs from
//! one local noon at the rig's site to the next, the way N.I.N.A.'s
//! `DATEMINUS12` folders date it, so no dusk or dawn flat run straddles the
//! split. A night is named by the date its evening began.
//!
//! Where a rig's local noon falls, first found wins:
//!
//! 1. the `night_zone` setting, rig then global ([`super::rules`]): noon at
//!    that UTC offset;
//! 2. the site longitude the rig's frames record (SITELONG, from the
//!    N.I.N.A. profile): mean solar noon there, 12:00 UTC less four minutes
//!    per degree east;
//! 3. the rig computer's clock, DATE-LOC less DATE-OBS: noon on that clock;
//! 4. the catalog's quiet hours, where Images splits nights
//!    ([`crate::server::sky_coverage::night_boundary`]).
//!
//! The server's own time zone is never used: it often runs somewhere else,
//! or on UTC. Neither the setting nor the longitude moves with daylight
//! saving, and the clock is read once per rig; an hour either side of noon
//! is still the middle of the day, so no night notices.
//!
//! The site is kept per rig in `psf_guard_rig_site`. An import records it
//! from each frame's header; [`backfill_rig_sites`] reads it, after startup,
//! off a few of each rig's newest frames for rigs imported before.

use super::rules::rules_for_rig;
use crate::commands::import::headers::{parse_fits_datetime, FrameMeta};
use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

const DAY: i64 = 86_400;
const NOON: i64 = 12 * 3_600;

/// Frames read per rig, newest first, looking for a site.
const SITE_READS_PER_RIG: usize = 5;

pub(crate) const SITE_TABLE_SQL: &str = r#"
    CREATE TABLE IF NOT EXISTS psf_guard_rig_site (
        rig_uuid          TEXT PRIMARY KEY,
        longitude         REAL,
        utc_offset_s      INTEGER,
        frame_captured_at INTEGER,
        read_at           INTEGER NOT NULL
    );
"#;

/// Add the site table to a catalog that predates it.
pub(crate) fn add_site_table(conn: &Connection) -> Result<bool> {
    if table_exists(conn) || conn.is_readonly(rusqlite::MAIN_DB).unwrap_or(true) {
        return Ok(false);
    }
    conn.execute_batch(SITE_TABLE_SQL)?;
    Ok(true)
}

fn table_exists(conn: &Connection) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'psf_guard_rig_site'",
        [],
        |_| Ok(()),
    )
    .optional()
    .ok()
    .flatten()
    .is_some()
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
        // the Gulf of Guinea, not an observatory.
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
}

/// Seconds after 00:00 UTC of mean solar noon at `longitude` degrees east.
pub fn solar_noon(longitude: f64) -> i64 {
    // Four minutes a degree, to the minute.
    let minutes = (longitude * 4.0).round() as i64;
    (NOON - minutes * 60).rem_euclid(DAY)
}

/// Seconds after 00:00 UTC of noon on a clock `utc_offset_s` ahead of UTC.
pub fn local_noon(utc_offset_s: i64) -> i64 {
    (NOON - utc_offset_s).rem_euclid(DAY)
}

/// Where a rig's split came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum NightSource {
    /// The `night_zone` setting.
    Setting,
    /// The site longitude its frames record.
    Site,
    /// The rig computer's clock in its frames.
    Clock,
    /// The catalog's quiet hours.
    Catalog,
}

/// One rig's split.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct RigNight {
    /// Seconds after 00:00 UTC.
    pub boundary_seconds: i64,
    pub source: NightSource,
}

/// A rig's split, from its setting, its recorded site and the catalog's.
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

/// Where each rig's nights split in one catalog.
#[derive(Debug, Clone)]
pub struct Nights {
    catalog: i64,
    rigs: Arc<HashMap<String, RigNight>>,
}

impl Nights {
    /// Every rig of the catalog, as its settings and recorded site say.
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
        Self {
            catalog,
            rigs: Arc::new(rigs),
        }
    }

    /// Every rig split at `catalog`, unless its setting says otherwise.
    pub fn fixed(catalog: i64) -> Self {
        Self {
            catalog,
            rigs: Arc::new(HashMap::new()),
        }
    }

    /// [`Nights::fixed`], with some rigs' splits given.
    #[cfg(test)]
    pub fn with_rigs(catalog: i64, rigs: impl IntoIterator<Item = (String, RigNight)>) -> Self {
        Self {
            catalog,
            rigs: Arc::new(rigs.into_iter().collect()),
        }
    }

    /// Where the catalog's lights split, as Images groups them.
    pub fn catalog(&self) -> i64 {
        self.catalog
    }

    pub fn rig(&self, rig_uuid: &str) -> RigNight {
        self.rigs.get(rig_uuid).copied().unwrap_or_else(|| {
            resolve(
                rules_for_rig(rig_uuid).night_offset_seconds,
                None,
                self.catalog,
            )
        })
    }

    /// Seconds after 00:00 UTC at which a rig's nights split.
    pub fn boundary(&self, rig_uuid: &str) -> i64 {
        self.rig(rig_uuid).boundary_seconds
    }

    /// The night a rig's frame shot at `at` belongs to.
    pub fn night_of(&self, rig_uuid: &str, at: i64) -> String {
        super::night_of(at, self.boundary(rig_uuid))
    }

    /// Every split, for a cache key: the catalog's alone while every rig
    /// splits with it, so keys made before rigs had their own stay put.
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
        digest
    }
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
    if !table_exists(conn) {
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

/// Keep what an imported frame says about its rig's site. A newer frame
/// replaces what an older one said; an older one only fills what is
/// missing; a frame that says nothing changes nothing.
pub(crate) fn record_site(conn: &Connection, rig_uuid: &str, frame: &FrameMeta) -> Result<()> {
    let site = Site::of(frame);
    if site.is_empty() || !table_exists(conn) {
        return Ok(());
    }
    write_site(conn, rig_uuid, &site, frame.timestamp)
}

fn write_site(
    conn: &Connection,
    rig_uuid: &str,
    site: &Site,
    captured_at: Option<i64>,
) -> Result<()> {
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
    Ok(())
}

/// What one pass of [`backfill_rig_sites`] did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SiteBackfillOutcome {
    /// Rigs with frames and no site row when the pass began.
    pub pending: usize,
    /// Rigs whose frames named a longitude.
    pub located: usize,
    /// Rigs whose frames named only the computer's clock.
    pub clocked: usize,
    /// Rigs whose frames were read and name neither; marked, so they are
    /// not read again until an import brings a frame that does.
    pub unplaced: usize,
    /// Rigs none of whose files could be read; tried again next time.
    pub unreadable: usize,
    /// The rigs that found a longitude or a clock, whose nights may now
    /// split elsewhere.
    pub placed: Vec<String>,
}

/// Read the site of every rig imported before the catalog kept one, off
/// the open path: up to five of its newest frames' headers each, stopping
/// at the first that names a longitude.
pub fn backfill_rig_sites(
    conn: &Connection,
    keep_going: impl Fn() -> bool,
) -> Result<SiteBackfillOutcome> {
    if !super::schema_exists(conn) || !table_exists(conn) {
        return Ok(SiteBackfillOutcome::default());
    }
    let pending: Vec<String> = conn
        .prepare(
            "SELECT DISTINCT f.rig_uuid FROM psf_guard_calibration_frame f
             LEFT JOIN psf_guard_rig_site s ON s.rig_uuid = f.rig_uuid
             WHERE s.rig_uuid IS NULL
             ORDER BY f.rig_uuid",
        )?
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let mut outcome = SiteBackfillOutcome {
        pending: pending.len(),
        ..Default::default()
    };
    for rig in pending {
        if !keep_going() {
            break;
        }
        let paths: Vec<String> = conn
            .prepare(
                "SELECT source_path FROM psf_guard_calibration_frame
                 WHERE rig_uuid = ?1
                 ORDER BY captured_at IS NULL, captured_at DESC, id DESC
                 LIMIT ?2",
            )?
            .query_map(params![rig, SITE_READS_PER_RIG as i64], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let mut found = Site::default();
        let mut captured_at = None;
        let mut readable = false;
        for path in paths {
            let meta = crate::commands::import::headers::read_frame_meta(Path::new(&path));
            if !meta.readable {
                continue;
            }
            readable = true;
            let site = Site::of(&meta);
            if found.utc_offset_s.is_none() {
                found.utc_offset_s = site.utc_offset_s;
                captured_at = captured_at.or(meta.timestamp);
            }
            if site.longitude.is_some() {
                found.longitude = site.longitude;
                captured_at = meta.timestamp.or(captured_at);
                break;
            }
        }
        if !readable {
            outcome.unreadable += 1;
            continue;
        }
        match (found.longitude, found.utc_offset_s) {
            (Some(_), _) => outcome.located += 1,
            (None, Some(_)) => outcome.clocked += 1,
            (None, None) => outcome.unplaced += 1,
        }
        if !found.is_empty() {
            outcome.placed.push(rig.clone());
        }
        write_site(conn, &rig, &found, captured_at)?;
    }
    Ok(outcome)
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
        conn.execute("DELETE FROM psf_guard_rig_site", []).unwrap();
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
        let found = backfill_rig_sites(&conn, || true).unwrap();
        assert_eq!(found.located, 1, "{found:?}");
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
        // A rig with frames on disk the backfill cannot read stays pending.
        conn.execute(
            "INSERT INTO psf_guard_calibration_frame (
                frame_uuid, rig_uuid, kind, source_path, source_fingerprint,
                captured_at, added_at, updated_at)
             VALUES ('f', 'away', 'flat', '/nowhere/flat.fits', 'x', 0, 0, 0)",
            [],
        )
        .unwrap();
        let outcome = backfill_rig_sites(&conn, || true).unwrap();
        assert_eq!(
            outcome,
            SiteBackfillOutcome {
                pending: 1,
                unreadable: 1,
                ..Default::default()
            }
        );
        assert!(!read_sites(&conn).contains_key("away"));
    }
}
