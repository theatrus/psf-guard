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
    pub at_ms: i64,
    /// The angular radius searched around the center.
    pub radius_degrees: f64,
    pub objects: Layer<ObjectMark>,
    pub minor_bodies: Layer<MinorBodyMark>,
    pub solar_system: Vec<crate::ephemeris::Placed>,
}

/// How far from the center the stage reaches: the stereographic plane's
/// corner radius taken back to the sky, never past the hemisphere.
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

/// The marks within `radius` of a place at `jd`, from whatever catalogs are there.
pub(super) fn marks_for(
    objects: Result<Arc<ObjectCatalog>, String>,
    minor_bodies: Result<Arc<MinorBodyCatalog>, String>,
    center: (f64, f64),
    radius: f64,
    jd: f64,
    limit: usize,
    limit_mag: f64,
) -> SkyMarks {
    let objects = match objects {
        Ok(catalog) => {
            let query = ObjectQuery {
                limit: Some(limit),
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
                        .map(|hit| ObjectMark {
                            id: hit.object.metadata.id.clone(),
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
    let jd = crate::ephemeris::julian_date_from_unix_ms(at_ms);
    let radius = search_radius(query.fov, query.aspect);
    let astrometry = state.astrometry.clone();
    let marks = tokio::task::spawn_blocking(move || {
        marks_for(
            astrometry.object_catalog(),
            astrometry.minor_body_catalog(),
            (query.ra, query.dec),
            radius,
            jd,
            query.limit,
            query.limit_mag,
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
            (10.68, 41.27),
            90.0,
            jd,
            300,
            30.0,
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
            (189.0, 26.0),
            1.0,
            jd,
            300,
            16.0,
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
