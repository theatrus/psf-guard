//! Schema-adaptive Target Scheduler framing and capture context.
//!
//! Target Scheduler has evolved without every optional field being present in
//! every database. Keep these reads isolated from the core models so older
//! databases can still be inspected and graded.

use crate::astrometry::target_scheduler_coordinates;
use crate::models::AcquiredImage;
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExpectedFraming {
    pub ra_deg: f64,
    pub dec_deg: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epoch_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotation_deg: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roi_percent: Option<f64>,
    pub source: String,
    /// Absolute offset grading is currently valid for J2000/unspecified TS
    /// coordinates. Other epochs remain useful display metadata but require a
    /// precession implementation before they can drive grades.
    pub grading_eligible: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AcquisitionContext {
    pub image_id: i32,
    pub target_id: i32,
    pub project_id: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exposure_id: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_framing: Option<ExpectedFraming>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_is_mosaic: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guiding_rms_arcsec: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guiding_ra_arcsec: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guiding_dec_arcsec: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pier_side: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotator_position_deg: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotator_mechanical_position_deg: Option<f64>,
}

impl AcquisitionContext {
    pub fn expected_for_grading(&self) -> Option<(f64, f64)> {
        self.expected_framing
            .as_ref()
            .filter(|framing| framing.grading_eligible)
            .map(|framing| (framing.ra_deg, framing.dec_deg))
    }
}

/// Resolves expected framing for many images with one schema probe and one
/// target-table query per distinct target. Request paths that iterate a whole
/// sequence must use this instead of [`load`]: `load` re-probes the schema and
/// re-reads the target row per call, which is pathological on slow (network
/// mounted) scheduler databases.
pub struct FramingResolver {
    target_query: String,
    by_target: std::collections::HashMap<i32, Option<ExpectedFraming>>,
}

impl FramingResolver {
    pub fn new(conn: &Connection) -> rusqlite::Result<Self> {
        let target_columns = table_columns(conn, "target")?;
        let target_metadata = column(&target_columns, "metadata");
        let ra = column(&target_columns, "ra");
        let dec = column(&target_columns, "dec");
        let epoch = column(&target_columns, "epochcode");
        let rotation = column(&target_columns, "rotation");
        let roi = column(&target_columns, "roi");
        let target_query = format!(
            "SELECT {}, {}, {}, {}, {}, {} FROM target WHERE Id = ?",
            ra.unwrap_or("NULL"),
            dec.unwrap_or("NULL"),
            epoch.unwrap_or("NULL"),
            rotation.unwrap_or("NULL"),
            roi.unwrap_or("NULL"),
            target_metadata.unwrap_or("NULL")
        );
        Ok(Self {
            target_query,
            by_target: std::collections::HashMap::new(),
        })
    }

    fn target_framing(
        &mut self,
        conn: &Connection,
        target_id: i32,
    ) -> rusqlite::Result<Option<ExpectedFraming>> {
        if let Some(cached) = self.by_target.get(&target_id) {
            return Ok(cached.clone());
        }
        let target = conn
            .query_row(&self.target_query, [target_id], |row| {
                Ok((
                    row.get::<_, Option<f64>>(0)?,
                    row.get::<_, Option<f64>>(1)?,
                    row.get::<_, Option<i32>>(2)?,
                    row.get::<_, Option<f64>>(3)?,
                    row.get::<_, Option<f64>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                ))
            })
            .optional()?;
        let framing = target.and_then(|(ra, dec, epoch_code, rotation, roi, metadata)| {
            let direct = ra
                .zip(dec)
                .and_then(|(ra_hours, dec_deg)| target_scheduler_coordinates(ra_hours, dec_deg))
                .map(|(ra_deg, dec_deg)| (ra_deg, dec_deg, "target_scheduler".to_string()));
            let metadata = metadata
                .as_deref()
                .and_then(|json| serde_json::from_str::<serde_json::Value>(json).ok());
            let fallback = metadata.as_ref().and_then(coordinates_from_target_metadata);
            direct
                .or(fallback)
                .map(|(ra_deg, dec_deg, source)| ExpectedFraming {
                    ra_deg,
                    dec_deg,
                    epoch_code,
                    rotation_deg: rotation.or_else(|| {
                        metadata
                            .as_ref()
                            .and_then(|m| number(m, &["Rotation", "rotation"]))
                    }),
                    roi_percent: roi.or_else(|| {
                        metadata
                            .as_ref()
                            .and_then(|m| number(m, &["ROI", "Roi", "roi"]))
                    }),
                    source,
                    // N.I.N.A.'s Epoch enum: JNOW=0, B1950=1, J2000=2, J2050=3.
                    // TS writes `(int)Epoch.J2000` (= 2) for every target it
                    // creates; only J2000 (or an unspecified epoch) is ICRS-
                    // comparable without a precession implementation.
                    grading_eligible: epoch_code.is_none_or(|code| code == 2),
                })
        });
        self.by_target.insert(target_id, framing.clone());
        Ok(framing)
    }

    pub fn expected_framing(
        &mut self,
        conn: &Connection,
        image: &AcquiredImage,
    ) -> rusqlite::Result<Option<ExpectedFraming>> {
        if let Some(framing) = self.target_framing(conn, image.target_id)? {
            return Ok(Some(framing));
        }
        // Capture-level fallback needs only the image's own metadata JSON,
        // which the caller already holds in memory — no extra queries.
        Ok(serde_json::from_str::<serde_json::Value>(&image.metadata)
            .ok()
            .as_ref()
            .and_then(coordinates_from_capture_metadata)
            .map(|(ra_deg, dec_deg, source)| ExpectedFraming {
                ra_deg,
                dec_deg,
                epoch_code: None,
                rotation_deg: None,
                roi_percent: None,
                source,
                grading_eligible: true,
            }))
    }

    pub fn expected_for_grading(
        &mut self,
        conn: &Connection,
        image: &AcquiredImage,
    ) -> rusqlite::Result<Option<(f64, f64)>> {
        Ok(self
            .expected_framing(conn, image)?
            .filter(|framing| framing.grading_eligible)
            .map(|framing| (framing.ra_deg, framing.dec_deg)))
    }
}

pub fn load(conn: &Connection, image: &AcquiredImage) -> rusqlite::Result<AcquisitionContext> {
    let project_columns = table_columns(conn, "project")?;
    let image_columns = table_columns(conn, "acquiredimage")?;

    let mut resolver = FramingResolver::new(conn)?;
    let expected_framing = resolver.expected_framing(conn, image)?;
    let image_metadata = serde_json::from_str::<serde_json::Value>(&image.metadata).ok();
    let exposure_id = if let Some(name) = column(&image_columns, "exposureid") {
        conn.query_row(
            &format!("SELECT {name} FROM acquiredimage WHERE Id = ?"),
            [image.id],
            |row| row.get::<_, Option<i32>>(0),
        )?
    } else {
        None
    };
    let project_is_mosaic = if let Some(name) = column(&project_columns, "ismosaic") {
        conn.query_row(
            &format!("SELECT {name} FROM project WHERE Id = ?"),
            [image.project_id],
            |row| row.get::<_, Option<bool>>(0),
        )
        .optional()?
        .flatten()
    } else {
        None
    };

    Ok(AcquisitionContext {
        image_id: image.id,
        target_id: image.target_id,
        project_id: image.project_id,
        exposure_id,
        session_id: image_metadata
            .as_ref()
            .and_then(|m| text(m, &["SessionId", "SessionID", "sessionId"])),
        expected_framing,
        project_is_mosaic,
        guiding_rms_arcsec: image_metadata
            .as_ref()
            .and_then(|m| number(m, &["GuidingRMS", "GuidingRms", "GuidingRMSTotal"])),
        guiding_ra_arcsec: image_metadata
            .as_ref()
            .and_then(|m| number(m, &["GuidingRMSRA", "GuidingRa", "GuidingRA"])),
        guiding_dec_arcsec: image_metadata
            .as_ref()
            .and_then(|m| number(m, &["GuidingRMSDec", "GuidingDec", "GuidingDEC"])),
        pier_side: image_metadata
            .as_ref()
            .and_then(|m| text(m, &["PierSide", "SideOfPier"])),
        rotator_position_deg: image_metadata
            .as_ref()
            .and_then(|m| number(m, &["RotatorPosition", "RotatorPositionDegrees"])),
        rotator_mechanical_position_deg: image_metadata.as_ref().and_then(|m| {
            number(
                m,
                &[
                    "RotatorMechanicalPosition",
                    "RotatorMechanicalPositionDegrees",
                ],
            )
        }),
    })
}

fn coordinates_from_target_metadata(value: &serde_json::Value) -> Option<(f64, f64, String)> {
    let coordinates = value.get("Coordinates").unwrap_or(value);
    if let (Some(ra_deg), Some(dec_deg)) = (
        number(coordinates, &["RaDegrees", "RADegrees", "ra_deg"]),
        number(coordinates, &["DecDegrees", "DECdegrees", "dec_deg"]),
    ) && ra_deg.is_finite()
        && dec_deg.is_finite()
        && (-90.0..=90.0).contains(&dec_deg)
    {
        return Some((
            ra_deg.rem_euclid(360.0),
            dec_deg,
            "target_scheduler_metadata_degrees".to_string(),
        ));
    }
    let ra_hours = number(coordinates, &["RA", "Ra", "RightAscension", "ra"])?;
    let dec_deg = number(coordinates, &["Dec", "DEC", "Declination", "dec"])?;
    target_scheduler_coordinates(ra_hours, dec_deg).map(|(ra_deg, dec_deg)| {
        (
            ra_deg,
            dec_deg,
            "target_scheduler_metadata_hours".to_string(),
        )
    })
}

fn coordinates_from_capture_metadata(value: &serde_json::Value) -> Option<(f64, f64, String)> {
    let target = value.get("Target").unwrap_or(value);
    if let (Some(ra_deg), Some(dec_deg)) = (
        number(target, &["TargetRaDegrees", "TargetRADegrees", "RaDegrees"]),
        number(
            target,
            &["TargetDecDegrees", "TargetDECdegrees", "DecDegrees"],
        ),
    ) && ra_deg.is_finite()
        && dec_deg.is_finite()
        && (-90.0..=90.0).contains(&dec_deg)
    {
        return Some((
            ra_deg.rem_euclid(360.0),
            dec_deg,
            "capture_target_metadata_degrees".to_string(),
        ));
    }
    let ra_hours = number(target, &["TargetRA", "TargetRa", "TargetRightAscension"])?;
    let dec_deg = number(target, &["TargetDec", "TargetDEC", "TargetDeclination"])?;
    target_scheduler_coordinates(ra_hours, dec_deg)
        .map(|(ra_deg, dec_deg)| (ra_deg, dec_deg, "capture_target_metadata_hours".to_string()))
}

fn table_columns(conn: &Connection, table: &str) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    stmt.query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()
}

fn column<'a>(columns: &'a [String], wanted: &str) -> Option<&'a str> {
    columns
        .iter()
        .find(|name| name.eq_ignore_ascii_case(wanted))
        .map(String::as_str)
}

fn number(value: &serde_json::Value, keys: &[&str]) -> Option<f64> {
    keys.iter().find_map(|key| {
        value.get(*key).and_then(|v| {
            v.as_f64()
                .or_else(|| v.as_str().and_then(|text| text.parse().ok()))
        })
    })
}

fn text(value: &serde_json::Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(|v| v.as_str()))
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(metadata: &str) -> AcquiredImage {
        AcquiredImage {
            id: 7,
            project_id: 2,
            target_id: 3,
            acquired_date: Some(100),
            filter_name: "L".to_string(),
            grading_status: 0,
            metadata: metadata.to_string(),
            reject_reason: None,
            profile_id: None,
            guid: None,
        }
    }

    #[test]
    fn reads_current_target_scheduler_context() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE target (Id INTEGER, ra REAL, dec REAL, epochCode INTEGER, rotation REAL, roi REAL);
             CREATE TABLE project (Id INTEGER, isMosaic INTEGER);
             CREATE TABLE acquiredimage (Id INTEGER, exposureId INTEGER);
             INSERT INTO target VALUES (3, 5.5, -12.0, 2, 90.0, 80.0);
             INSERT INTO project VALUES (2, 1);
             INSERT INTO acquiredimage VALUES (7, 44);",
        )
        .unwrap();
        let context = load(
            &conn,
            &image(r#"{"SessionId":"night-1","GuidingRMS":0.8,"PierSide":"East"}"#),
        )
        .unwrap();
        let expected = context.expected_framing.unwrap();
        assert_eq!(expected.ra_deg, 82.5);
        assert_eq!(expected.dec_deg, -12.0);
        assert!(expected.grading_eligible);
        assert_eq!(context.exposure_id, Some(44));
        assert_eq!(context.session_id.as_deref(), Some("night-1"));
        assert_eq!(context.project_is_mosaic, Some(true));
        assert_eq!(context.guiding_rms_arcsec, Some(0.8));
    }

    #[test]
    fn jnow_epoch_abstains_from_absolute_grading() {
        // NINA Epoch::JNOW = 0: coordinates precess away from ICRS, so
        // absolute grading must abstain (they were eligible before the
        // epoch-code fix — J2000 is enum value 2, not 0).
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE target (Id INTEGER, ra REAL, dec REAL, epochCode INTEGER, rotation REAL, roi REAL);
             CREATE TABLE project (Id INTEGER, isMosaic INTEGER);
             CREATE TABLE acquiredimage (Id INTEGER, exposureId INTEGER);
             INSERT INTO target VALUES (3, 5.5, -12.0, 0, 90.0, 80.0);
             INSERT INTO project VALUES (2, 0);
             INSERT INTO acquiredimage VALUES (7, 44);",
        )
        .unwrap();
        let context = load(&conn, &image("{}")).unwrap();
        let expected = context.expected_framing.as_ref().unwrap();
        assert!(!expected.grading_eligible);
        assert_eq!(context.expected_for_grading(), None);
    }

    #[test]
    fn falls_back_to_target_metadata_and_abstains_for_unknown_epoch() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE target (Id INTEGER, ra REAL, dec REAL, epochcode INTEGER, metadata TEXT);
             CREATE TABLE project (Id INTEGER);
             CREATE TABLE acquiredimage (Id INTEGER);
             INSERT INTO target VALUES (3, NULL, NULL, 1, '{\"RA\": 2.0, \"Dec\": 10.0}');
             INSERT INTO project VALUES (2);
             INSERT INTO acquiredimage VALUES (7);",
        )
        .unwrap();
        let context = load(&conn, &image("{}")).unwrap();
        let expected = context.expected_framing.unwrap();
        assert_eq!(expected.ra_deg, 30.0);
        assert!(!expected.grading_eligible);
        assert_eq!(expected.source, "target_scheduler_metadata_hours");
    }
}
