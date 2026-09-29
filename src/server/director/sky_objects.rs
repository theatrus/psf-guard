//! Marks for the framing view: deep-sky objects from the Seiza object
//! catalog, comets and asteroids from its minor-body catalog at a given
//! time, and the Sun, Moon and planets from the ephemeris. Each layer says
//! whether its catalog is on this server; a missing one is a note, not an
//! error, since the survey picture stands on its own.

use std::sync::Arc;

use axum::{
    extract::{Query, State},
    Json,
};
use seiza::{
    minor_bodies::{MinorBodyCatalog, MinorBodyKind},
    objects::{ObjectCatalog, ObjectQuery, ObjectSort, SkyRegion},
};
use serde::{Deserialize, Serialize};

use super::{enabled, Error};
use crate::server::{api::ApiResponse, state::AppState};

const MAX_LIMIT: usize = 1000;
const DEFAULT_LIMIT: usize = 300;
/// Minor bodies fainter than this are not worth a mark on a survey image.
const DEFAULT_LIMIT_MAG: f64 = 16.0;
const DEG: f64 = std::f64::consts::PI / 180.0;
/// Catalogs left out unless the request says otherwise. PGC lists hundreds
/// of faint galaxies per square degree and HD every naked-eye star; both
/// crowd out the marks an imager frames around.
const DEFAULT_HIDDEN: &[&str] = &["PGC", "HD"];
const MAX_HIDDEN: usize = 32;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MarksQuery {
    ra: f64,
    dec: f64,
    /// Width of the view in stage degrees, as the stage draws it.
    fov: f64,
    /// Width over height of the view; 4:3 when unsaid.
    #[serde(default = "default_aspect")]
    aspect: f64,
    /// Unix milliseconds; now when unsaid. Comets and planets move.
    at: Option<i64>,
    #[serde(default = "default_limit")]
    limit: usize,
    #[serde(default = "default_limit_mag")]
    limit_mag: f64,
    /// Comma-separated catalog prefixes to leave out, matched against the
    /// letters a designation starts with ("PGC,HD" when unsaid; empty to
    /// hide nothing).
    hide: Option<String>,
    /// Comma-separated catalog prefixes to keep, the same way; every catalog
    /// when unsaid. With this given, nothing is hidden unless `hide` says so.
    catalogs: Option<String>,
}
fn default_aspect() -> f64 {
    4.0 / 3.0
}
fn default_limit() -> usize {
    DEFAULT_LIMIT
}
fn default_limit_mag() -> f64 {
    DEFAULT_LIMIT_MAG
}

#[derive(Serialize)]
pub(super) struct Layer<T> {
    /// Whether the catalog behind this layer is on the server.
    pub available: bool,
    /// Why it is not, when it is not.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub items: Vec<T>,
}

#[derive(Serialize)]
pub(super) struct ObjectMark {
    pub id: String,
    pub name: String,
    pub common_name: String,
    pub kind: &'static str,
    pub ra_degrees: f64,
    pub dec_degrees: f64,
    pub mag: Option<f32>,
    pub major_arcmin: Option<f32>,
    pub minor_arcmin: Option<f32>,
    pub position_angle_degrees: Option<f32>,
    /// The catalog's 0..1 guess at how much of the field this object is.
    pub prominence: f64,
}

#[derive(Serialize)]
pub(super) struct MinorBodyMark {
    pub name: String,
    pub kind: &'static str,
    pub ra_degrees: f64,
    pub dec_degrees: f64,
    pub mag: f64,
    pub distance_au: f64,
    pub motion_arcsec_per_hour: Option<f64>,
    /// A comet's tail direction, or an asteroid's direction of motion, east of north.
    pub direction_pa_degrees: Option<f64>,
}

#[derive(Serialize)]
pub(super) struct SkyMarks {
    /// The catalog prefixes left out of `objects`, as applied.
    pub hidden: Vec<String>,
    /// The catalog prefixes `objects` was kept to, when the request named them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub catalogs: Option<Vec<String>>,
    pub at_ms: i64,
    /// The angular radius searched around the center.
    pub radius_degrees: f64,
    pub objects: Layer<ObjectMark>,
    pub minor_bodies: Layer<MinorBodyMark>,
    pub solar_system: Vec<crate::ephemeris::Placed>,
}

/// How far from the center the stage reaches: the stereographic plane's
/// corner radius taken back to the sky, never past the hemisphere.
/// The catalog a designation belongs to: its leading letters, lower-cased,
/// so "PGC 2557" is "pgc", "Sh2-101" is "sh" and "vdB 1" is "vdb".
pub(super) fn designation_prefix(name: &str) -> String {
    name.chars()
        .take_while(|c| c.is_ascii_alphabetic())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// A comma-separated list of catalog prefixes, lower-cased; `None` when it
/// is too long to be honest.
pub(super) fn catalog_prefixes(list: &str) -> Option<Vec<String>> {
    let prefixes: Vec<String> = list
        .split(',')
        .map(|p| designation_prefix(p.trim()))
        .filter(|p| !p.is_empty())
        .collect();
    (prefixes.len() <= MAX_HIDDEN).then_some(prefixes)
}

/// The prefixes a request hides: its own list when it gives one (which may
/// be empty), nothing when it names the catalogs it wants instead, and the
/// default set when it says neither.
pub(super) fn hidden_prefixes(hide: Option<&str>, catalogs_named: bool) -> Option<Vec<String>> {
    match hide {
        Some(list) => catalog_prefixes(list),
        None if catalogs_named => Some(Vec::new()),
        None => Some(
            DEFAULT_HIDDEN
                .iter()
                .map(|p| p.to_ascii_lowercase())
                .collect(),
        ),
    }
}

pub(super) fn search_radius(fov_degrees: f64, aspect: f64) -> f64 {
    let rho = (fov_degrees / 2.0) * DEG;
    let across = 2.0 * (rho / 2.0).atan() / DEG;
    (across * (1.0 + 1.0 / (aspect * aspect)).sqrt() * 1.05).min(90.0)
}

fn separation(ra1: f64, dec1: f64, ra2: f64, dec2: f64) -> f64 {
    let (d1, d2) = (dec1 * DEG, dec2 * DEG);
    let cos = d1.sin() * d2.sin() + d1.cos() * d2.cos() * ((ra1 - ra2) * DEG).cos();
    cos.clamp(-1.0, 1.0).acos() / DEG
}

/// Bearing from one place to another, degrees east of north.
fn bearing(ra1: f64, dec1: f64, ra2: f64, dec2: f64) -> f64 {
    let (d1, d2) = (dec1 * DEG, dec2 * DEG);
    let dra = (ra2 - ra1) * DEG;
    let y = dra.sin() * d2.cos();
    let x = d1.cos() * d2.sin() - d1.sin() * d2.cos() * dra.cos();
    (y.atan2(x) / DEG).rem_euclid(360.0)
}

/// What a marks request asks for, once checked.
pub(super) struct MarksAsked<'a> {
    pub center: (f64, f64),
    pub radius: f64,
    pub jd: f64,
    pub limit: usize,
    pub limit_mag: f64,
    pub hidden: &'a [String],
    /// Keep only these catalogs, when given.
    pub catalogs: Option<&'a [String]>,
}

/// The marks within `radius` of a place at `jd`, from whatever catalogs are there.
pub(super) fn marks_for(
    objects: Result<Arc<ObjectCatalog>, String>,
    minor_bodies: Result<Arc<MinorBodyCatalog>, String>,
    asked: &MarksAsked<'_>,
) -> SkyMarks {
    let MarksAsked {
        center,
        radius,
        jd,
        limit,
        limit_mag,
        hidden,
        catalogs,
    } = *asked;
    let objects = match objects {
        Ok(catalog) => {
            // The catalog cuts to its limit before this filter could run, so
            // it is asked for every hit and cut here.
            let query = ObjectQuery {
                limit: None,
                sort: ObjectSort::Prominence,
                include_extent_overlaps: true,
                ..ObjectQuery::default()
            };
            match catalog.query_region(
                &SkyRegion::Cone {
                    center,
                    radius_deg: radius,
                },
                &query,
            ) {
                Ok(hits) => Layer {
                    available: true,
                    note: None,
                    items: hits
                        .into_iter()
                        .filter(|hit| {
                            let prefix = designation_prefix(&hit.object.name);
                            !hidden.contains(&prefix)
                                && catalogs.is_none_or(|kept| kept.contains(&prefix))
                        })
                        .take(limit)
                        .map(|hit| ObjectMark {
                            // Older catalog files carry no record ids; the
                            // designation then stands in, so every mark has a key.
                            id: if hit.object.metadata.id.is_empty() {
                                hit.object.name.clone()
                            } else {
                                hit.object.metadata.id.clone()
                            },
                            name: hit.object.name.clone(),
                            common_name: hit.object.common_name.clone(),
                            kind: hit.object.kind.as_str(),
                            ra_degrees: hit.object.ra,
                            dec_degrees: hit.object.dec,
                            mag: hit.object.mag,
                            major_arcmin: hit.object.major_arcmin,
                            minor_arcmin: hit.object.minor_arcmin,
                            position_angle_degrees: hit.object.position_angle_deg,
                            prominence: hit.predicted_prominence,
                        })
                        .collect(),
                },
                Err(error) => Layer {
                    available: true,
                    note: Some(format!("Object catalog query failed: {error}")),
                    items: Vec::new(),
                },
            }
        }
        Err(note) => Layer {
            available: false,
            note: Some(note),
            items: Vec::new(),
        },
    };
    let solar_system: Vec<_> = crate::ephemeris::solar_system_at(jd)
        .into_iter()
        .filter(|body| separation(body.ra_degrees, body.dec_degrees, center.0, center.1) <= radius)
        .collect();
    let sun = crate::ephemeris::solar_system_at(jd)
        .into_iter()
        .find(|body| body.name == "Sun")
        .map(|sun| (sun.ra_degrees, sun.dec_degrees));
    let minor_bodies = match minor_bodies {
        Ok(catalog) => {
            const HALF_HOUR_DAYS: f64 = 0.5 / 24.0;
            let mut items: Vec<MinorBodyMark> = catalog
                .bodies()
                .iter()
                .filter_map(|body| {
                    let (ra, dec, mag, distance_au) = MinorBodyCatalog::position_at(body, jd)?;
                    if mag > limit_mag || separation(ra, dec, center.0, center.1) > radius {
                        return None;
                    }
                    let motion = MinorBodyCatalog::position_at(body, jd - HALF_HOUR_DAYS)
                        .zip(MinorBodyCatalog::position_at(body, jd + HALF_HOUR_DAYS));
                    let motion_arcsec_per_hour = motion.map(|(before, after)| {
                        separation(before.0, before.1, after.0, after.1) * 3600.0
                    });
                    let direction_pa_degrees = match body.kind {
                        MinorBodyKind::Comet => sun.map(|(sun_ra, sun_dec)| {
                            (bearing(ra, dec, sun_ra, sun_dec) + 180.0).rem_euclid(360.0)
                        }),
                        MinorBodyKind::Asteroid => motion
                            .map(|(before, after)| bearing(before.0, before.1, after.0, after.1)),
                    };
                    Some(MinorBodyMark {
                        name: body.name.clone(),
                        kind: match body.kind {
                            MinorBodyKind::Comet => "comet",
                            MinorBodyKind::Asteroid => "asteroid",
                        },
                        ra_degrees: ra,
                        dec_degrees: dec,
                        mag,
                        distance_au,
                        motion_arcsec_per_hour,
                        direction_pa_degrees,
                    })
                })
                .collect();
            items.sort_by(|a, b| a.mag.total_cmp(&b.mag));
            items.truncate(limit);
            Layer {
                available: true,
                note: None,
                items,
            }
        }
        Err(note) => Layer {
            available: false,
            note: Some(note),
            items: Vec::new(),
        },
    };
    SkyMarks {
        hidden: hidden.to_vec(),
        catalogs: catalogs.map(<[String]>::to_vec),
        at_ms: ((jd - 2_440_587.5) * 86_400_000.0).round() as i64,
        radius_degrees: radius,
        objects,
        minor_bodies,
        solar_system,
    }
}

pub(super) async fn marks(
    State(state): State<Arc<AppState>>,
    Query(query): Query<MarksQuery>,
) -> Result<Json<ApiResponse<SkyMarks>>, Error> {
    enabled(&state)?;
    let finite = |v: f64, lo: f64, hi: f64| v.is_finite() && (lo..=hi).contains(&v);
    if !finite(query.ra, 0.0, 360.0)
        || !finite(query.dec, -90.0, 90.0)
        || !finite(query.fov, 0.01, 180.0)
        || !finite(query.aspect, 0.2, 5.0)
        || !finite(query.limit_mag, -5.0, 30.0)
        || query.limit == 0
        || query.limit > MAX_LIMIT
    {
        return Err(Error::Invalid);
    }
    let at_ms = query.at.unwrap_or_else(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0)
    });
    if !(0..=4_102_444_800_000).contains(&at_ms) {
        return Err(Error::Invalid);
    }
    let catalogs = match query.catalogs.as_deref() {
        Some(list) => Some(catalog_prefixes(list).ok_or(Error::Invalid)?),
        None => None,
    };
    let hidden =
        hidden_prefixes(query.hide.as_deref(), catalogs.is_some()).ok_or(Error::Invalid)?;
    let jd = crate::ephemeris::julian_date_from_unix_ms(at_ms);
    let radius = search_radius(query.fov, query.aspect);
    let astrometry = state.astrometry.clone();
    let marks = tokio::task::spawn_blocking(move || {
        marks_for(
            astrometry.object_catalog(),
            astrometry.minor_body_catalog(),
            &MarksAsked {
                center: (query.ra, query.dec),
                radius,
                jd,
                limit: query.limit,
                limit_mag: query.limit_mag,
                hidden: &hidden,
                catalogs: catalogs.as_deref(),
            },
        )
    })
    .await
    .map_err(|_| Error::Internal)?;
    Ok(Json(ApiResponse::success(marks)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use seiza::{
        minor_bodies::MinorBody,
        objects::{ObjectKind, ObjectMetadata, SkyObject},
    };

    fn galaxy(name: &str, common: &str, ra: f64, dec: f64, major: f32) -> SkyObject {
        SkyObject {
            kind: ObjectKind::Galaxy,
            ra,
            dec,
            mag: Some(3.4),
            major_arcmin: Some(major),
            minor_arcmin: Some(major / 3.0),
            position_angle_deg: Some(35.0),
            name: name.to_owned(),
            common_name: common.to_owned(),
            metadata: ObjectMetadata {
                id: name.to_lowercase().replace(' ', ""),
                source: "test".to_owned(),
                aliases: Vec::new(),
                parent_ids: Vec::new(),
                alternate_ids: Vec::new(),
                alternate_sources: Vec::new(),
            },
        }
    }

    #[test]
    fn the_search_radius_covers_the_stage_corners_and_stops_at_a_hemisphere() {
        assert!((search_radius(4.0, 4.0 / 3.0) - 2.625).abs() < 0.05);
        assert!(search_radius(150.0, 4.0 / 3.0) <= 90.0);
        assert!(search_radius(60.0, 1.0) > search_radius(60.0, 2.0));
    }

    #[test]
    fn crowded_catalogs_are_hidden_unless_asked_for_and_never_eat_the_limit() {
        let center = (10.68, 41.27);
        let mut objects = vec![galaxy("M 31", "Andromeda Galaxy", 10.68, 41.27, 190.0)];
        // A swarm of faint PGC galaxies and HD stars around it, each bigger
        // on paper than the next real mark so prominence ranks them first.
        for i in 0..40 {
            objects.push(galaxy(
                &format!("PGC {i}"),
                "",
                10.7 + f64::from(i) * 0.01,
                41.3,
                20.0,
            ));
            objects.push(galaxy(
                &format!("HD {i}"),
                "",
                10.6 - f64::from(i) * 0.01,
                41.2,
                20.0,
            ));
        }
        objects.push(galaxy("NGC 205", "", 10.09, 41.68, 2.0));
        let jd = crate::ephemeris::julian_date_from_unix_ms(1_790_000_000_000);
        let none = Err("no minor bodies".to_owned());
        let names = |marks: &SkyMarks| {
            marks
                .objects
                .items
                .iter()
                .map(|o| o.name.clone())
                .collect::<Vec<_>>()
        };
        let default = hidden_prefixes(None, false).unwrap();
        assert_eq!(default, ["pgc", "hd"]);
        let catalog = Arc::new(ObjectCatalog::new(objects));
        let asked = |hidden: &[String]| MarksAsked {
            center,
            radius: 2.0,
            jd,
            limit: 10,
            limit_mag: 16.0,
            hidden: Box::leak(hidden.to_vec().into_boxed_slice()),
            catalogs: None,
        };
        let marks = marks_for(Ok(catalog.clone()), none.clone(), &asked(&default));
        assert_eq!(names(&marks), ["M 31", "NGC 205"]);
        assert_eq!(marks.hidden, ["pgc", "hd"]);
        // Asking for everything brings the swarm back, and the limit then bites.
        let all = hidden_prefixes(Some(""), false).unwrap();
        let marks = marks_for(Ok(catalog.clone()), none.clone(), &asked(&all));
        assert_eq!(marks.objects.items.len(), 10);
        assert!(names(&marks).iter().any(|n| n.starts_with("PGC")));
        // A list of its own, in any case, with spaces.
        let own = hidden_prefixes(Some(" ngc, Pgc "), false).unwrap();
        let marks = marks_for(Ok(catalog), none, &asked(&own));
        assert_eq!(names(&marks)[0], "M 31");
        assert!(names(&marks)
            .iter()
            .all(|n| !n.starts_with("NGC") && !n.starts_with("PGC")));
        assert!(names(&marks).iter().any(|n| n.starts_with("HD")));
        assert_eq!(designation_prefix("Sh2-101"), "sh");
        assert_eq!(designation_prefix("vdB 1"), "vdb");
        assert!(hidden_prefixes(Some(&"a,".repeat(MAX_HIDDEN + 1)), false).is_none());
        // Naming the catalogs wanted keeps only those, and lifts the default hiding.
        assert_eq!(hidden_prefixes(None, true).unwrap(), Vec::<String>::new());
        let kept = catalog_prefixes("M, PGC").unwrap();
        let marks = marks_for(
            Ok(Arc::new(ObjectCatalog::new(vec![
                galaxy("M 31", "Andromeda Galaxy", 10.68, 41.27, 190.0),
                galaxy("NGC 205", "", 10.09, 41.68, 2.0),
                galaxy("PGC 2557", "", 10.7, 41.3, 20.0),
            ]))),
            Err("no minor bodies".to_owned()),
            &MarksAsked {
                center,
                radius: 2.0,
                jd,
                limit: 10,
                limit_mag: 16.0,
                hidden: &[],
                catalogs: Some(&kept),
            },
        );
        assert_eq!(names(&marks), ["M 31", "PGC 2557"]);
        assert_eq!(
            marks.catalogs.as_deref(),
            Some(&["m".to_owned(), "pgc".to_owned()][..])
        );
    }

    #[test]
    fn marks_come_from_whatever_catalogs_are_there_and_say_what_is_missing() {
        let objects = ObjectCatalog::new(vec![
            galaxy("M 31", "Andromeda Galaxy", 10.68, 41.27, 190.0),
            galaxy("NGC 4565", "Needle Galaxy", 189.09, 25.99, 15.0),
        ]);
        // A comet on a made-up parabolic orbit; its place is not asserted, only its presence.
        let comet = MinorBody {
            kind: MinorBodyKind::Comet,
            name: "C/2026 T1 (Test)".to_owned(),
            epoch_jd: 2_461_000.0,
            q_or_a: 0.9,
            eccentricity: 0.999,
            inclination_deg: 30.0,
            node_deg: 100.0,
            arg_perihelion_deg: 200.0,
            mean_anomaly_deg: 0.0,
            h_mag: 6.0,
            slope: 4.0,
        };
        let bodies = MinorBodyCatalog::new(vec![comet]);
        let jd = crate::ephemeris::julian_date_from_unix_ms(1_790_000_000_000);
        let marks = marks_for(
            Ok(Arc::new(objects)),
            Ok(Arc::new(bodies)),
            &MarksAsked {
                center: (10.68, 41.27),
                radius: 90.0,
                jd,
                limit: 300,
                limit_mag: 30.0,
                hidden: &[],
                catalogs: None,
            },
        );
        assert!(marks.objects.available);
        assert_eq!(marks.objects.items[0].name, "M 31");
        assert_eq!(marks.objects.items[0].common_name, "Andromeda Galaxy");
        assert_eq!(marks.objects.items[0].kind, "galaxy");
        assert!(marks.minor_bodies.available);
        assert_eq!(marks.minor_bodies.items.len(), 1);
        assert_eq!(marks.minor_bodies.items[0].kind, "comet");
        assert!(marks.minor_bodies.items[0].direction_pa_degrees.is_some());
        assert!(!marks.solar_system.is_empty());

        // A narrow field around the Needle holds it alone, and says the minor-body catalog is missing.
        let objects = ObjectCatalog::new(vec![
            galaxy("M 31", "Andromeda Galaxy", 10.68, 41.27, 190.0),
            galaxy("NGC 4565", "Needle Galaxy", 189.09, 25.99, 15.0),
        ]);
        let marks = marks_for(
            Ok(Arc::new(objects)),
            Err("no minor bodies".to_owned()),
            &MarksAsked {
                center: (189.0, 26.0),
                radius: 1.0,
                jd,
                limit: 300,
                limit_mag: 16.0,
                hidden: &[],
                catalogs: None,
            },
        );
        assert_eq!(
            marks
                .objects
                .items
                .iter()
                .map(|o| o.name.as_str())
                .collect::<Vec<_>>(),
            ["NGC 4565"]
        );
        assert!(!marks.minor_bodies.available);
        assert_eq!(marks.minor_bodies.note.as_deref(), Some("no minor bodies"));
        assert!(
            marks.solar_system.is_empty()
                || marks.solar_system.iter().all(|b| separation(
                    b.ra_degrees,
                    b.dec_degrees,
                    189.0,
                    26.0
                ) <= 1.0)
        );
    }
}

#[cfg(test)]
mod bench {
    use super::*;
    use std::time::Instant;

    /// `PSF_GUARD_OBJECTS_BENCH=/path/to/objects.bin cargo test --release --lib
    /// server::director::sky_objects::bench -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn render_timing() {
        let Some(path) = std::env::var_os("PSF_GUARD_OBJECTS_BENCH") else {
            return;
        };
        let started = Instant::now();
        let catalog = Arc::new(ObjectCatalog::open(std::path::Path::new(&path)).unwrap());
        eprintln!("open {} objects in {:?}", catalog.len(), started.elapsed());
        for fov in [2.0, 10.0, 30.0, 60.0, 120.0, 180.0] {
            let radius = search_radius(fov, 4.0 / 3.0);
            let started = Instant::now();
            let marks = marks_for(
                Ok(catalog.clone()),
                Err("none".into()),
                &MarksAsked {
                    center: (10.68, 41.27),
                    radius,
                    jd: 2_461_000.5,
                    limit: 300,
                    limit_mag: 16.0,
                    hidden: &hidden_prefixes(None, false).unwrap(),
                    catalogs: None,
                },
            );
            let mut prefixes: std::collections::BTreeMap<String, usize> = Default::default();
            for mark in &marks.objects.items {
                let prefix: String = mark
                    .name
                    .chars()
                    .take_while(|c| c.is_ascii_alphabetic())
                    .collect();
                *prefixes.entry(prefix).or_default() += 1;
            }
            eprintln!(
                "fov {fov:>5} radius {radius:>6.1}: {} marks in {:?}; {:?}",
                marks.objects.items.len(),
                started.elapsed(),
                prefixes
            );
        }
        let mut prefixes: std::collections::BTreeMap<String, usize> = Default::default();
        let mut kinds: std::collections::BTreeMap<&str, usize> = Default::default();
        for object in catalog.objects() {
            let prefix: String = object
                .name
                .chars()
                .take_while(|c| c.is_ascii_alphabetic())
                .collect();
            *prefixes.entry(prefix).or_default() += 1;
            *kinds.entry(object.kind.as_str()).or_default() += 1;
        }
        let common: Vec<_> = prefixes.iter().filter(|(_, n)| **n >= 20).collect();
        eprintln!("whole catalog prefixes with 20+ entries: {common:?}\nkinds {kinds:?}");
        let started = Instant::now();
        let hits = catalog
            .query_region(
                &SkyRegion::Cone {
                    center: (10.68, 41.27),
                    radius_deg: 60.0,
                },
                &ObjectQuery {
                    include_extent_overlaps: true,
                    ..ObjectQuery::default()
                },
            )
            .unwrap();
        let mut prefixes: std::collections::BTreeMap<String, usize> = Default::default();
        let mut sources: std::collections::BTreeMap<String, usize> = Default::default();
        for hit in &hits {
            let prefix: String = hit
                .object
                .name
                .chars()
                .take_while(|c| c.is_ascii_alphabetic())
                .collect();
            *prefixes.entry(prefix).or_default() += 1;
            *sources
                .entry(hit.object.metadata.source.clone())
                .or_default() += 1;
        }
        eprintln!(
            "all hits within 60 deg: {} in {:?}\nprefixes {:?}\nsources {:?}",
            hits.len(),
            started.elapsed(),
            prefixes,
            sources
        );
    }
}
