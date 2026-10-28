//! Flat grouping's tables about lights, beside [`super::store`]'s:
//!
//! - `psf_guard_flat_family_evidence`: what an imaging session's own
//!   lights said, per filter, target and recorded angle, about which
//!   response family their flats come from.
//! - `psf_guard_flat_light_angle`: each light's sky − mechanical rotator
//!   offset, from N.I.N.A.'s record or PSF Guard's pixel solve, which the
//!   rotator's angle epochs are built from (see [`super::epochs`]).
//!
//! Both are made by the measuring job, additively, in a writable catalog,
//! never in a read-only one, and read by the book without touching any
//! other storage. A catalog without them chooses by angle alone.

use super::lights::FAMILY_EVIDENCE_VERSION;
use super::measure::rows::{FamilyEvidenceRow, FamilyPairRow};
use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub(crate) const TABLE_SQL: &str = r#"
    CREATE TABLE IF NOT EXISTS psf_guard_flat_family_evidence (
        session_key        TEXT NOT NULL,
        filter_name        TEXT NOT NULL,
        target             TEXT NOT NULL,
        angle_key          TEXT NOT NULL,
        algorithm_version  INTEGER NOT NULL,
        evidence_key       TEXT NOT NULL,
        chosen             TEXT,
        detail_json        TEXT NOT NULL,
        measured_at        INTEGER NOT NULL,
        PRIMARY KEY (session_key, filter_name, target, angle_key, algorithm_version)
    );
    CREATE TABLE IF NOT EXISTS psf_guard_flat_light_angle (
        light_id           INTEGER PRIMARY KEY,
        rig_uuid           TEXT NOT NULL,
        captured_at        INTEGER NOT NULL,
        offset_deg         REAL NOT NULL,
        solved             INTEGER NOT NULL,
        solved_pa          REAL,
        solve_stamp        TEXT
    );
"#;

fn has_table(conn: &Connection, name: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [name],
        |_| Ok(()),
    )
    .optional()
    .map(|row| row.is_some())
    .unwrap_or(false)
}

pub(crate) fn table_exists(conn: &Connection) -> bool {
    has_table(conn, "psf_guard_flat_family_evidence")
}

/// Create the tables in a writable catalog. Safe to run twice. A family
/// evidence table from before rows named their target is a cache of an
/// older shape: it is dropped and made again, and the lights asked again.
pub(crate) fn add_table(conn: &Connection) -> Result<()> {
    if conn.is_readonly(rusqlite::MAIN_DB).unwrap_or(true) {
        return Ok(());
    }
    if table_exists(conn)
        && conn
            .prepare("SELECT target FROM psf_guard_flat_family_evidence LIMIT 0")
            .is_err()
    {
        conn.execute_batch("DROP TABLE psf_guard_flat_family_evidence")?;
    }
    if table_exists(conn) && has_table(conn, "psf_guard_flat_light_angle") {
        return Ok(());
    }
    conn.execute_batch(TABLE_SQL)
        .context("creating the flat family tables")
}

#[derive(Serialize, Deserialize)]
struct Detail {
    candidates: Vec<String>,
    lights: usize,
    targets: Vec<String>,
    pairs: Vec<FamilyPairRow>,
    angle: f64,
    angle_low: f64,
    angle_high: f64,
    start: i64,
    end: i64,
}

pub(crate) fn upsert(conn: &Connection, row: &FamilyEvidenceRow) -> Result<()> {
    let detail = Detail {
        candidates: row.candidates.clone(),
        lights: row.lights,
        targets: row.targets.clone(),
        pairs: row.pairs.clone(),
        angle: row.angle,
        angle_low: row.angle_low,
        angle_high: row.angle_high,
        start: row.start,
        end: row.end,
    };
    conn.execute(
        "INSERT OR REPLACE INTO psf_guard_flat_family_evidence
            (session_key, filter_name, target, angle_key, algorithm_version, evidence_key,
             chosen, detail_json, measured_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            row.session_key,
            row.filter,
            row.target,
            row.angle_key(),
            FAMILY_EVIDENCE_VERSION,
            row.key,
            row.chosen,
            serde_json::to_string(&detail)?,
            chrono::Utc::now().timestamp(),
        ],
    )?;
    Ok(())
}

pub(crate) fn load(conn: &Connection) -> Result<Vec<FamilyEvidenceRow>> {
    if !table_exists(conn) {
        return Ok(Vec::new());
    }
    let Ok(mut statement) = conn.prepare(
        "SELECT session_key, filter_name, target, evidence_key, chosen, detail_json
         FROM psf_guard_flat_family_evidence WHERE algorithm_version = ?1",
    ) else {
        // An older shape, not made again yet: nothing to read.
        return Ok(Vec::new());
    };
    let rows = statement
        .query_map([FAMILY_EVIDENCE_VERSION], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, String>(5)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows
        .into_iter()
        .filter_map(|(session_key, filter, target, key, chosen, detail)| {
            let detail: Detail = serde_json::from_str(&detail).ok()?;
            Some(FamilyEvidenceRow {
                session_key,
                filter,
                target,
                angle: detail.angle,
                angle_low: detail.angle_low,
                angle_high: detail.angle_high,
                start: detail.start,
                end: detail.end,
                key,
                candidates: detail.candidates,
                chosen,
                lights: detail.lights,
                targets: detail.targets,
                pairs: detail.pairs,
            })
        })
        .collect())
}

/// Where a row is kept: session, filter, target, angle key and evidence
/// key.
pub(crate) type Kept = seiza_calibration::library::RetentionKey;

/// The place a row is kept under.
#[cfg(test)]
pub(crate) fn kept(row: &FamilyEvidenceRow) -> Kept {
    Kept::of(row)
}

/// Drop the rows a pass no longer asks for: of an older algorithm, of an
/// imaging session the catalog lost (not in `all`), or of a session whose
/// rig the pass judged (in `judged`) that `keep` does not name with its
/// current key. Other rigs' rows wait for their own pass.
pub(crate) fn retain(
    conn: &Connection,
    keep: &[Kept],
    judged: &HashSet<String>,
    all: &HashSet<String>,
) -> Result<usize> {
    if !table_exists(conn) {
        return Ok(0);
    }
    let wanted: HashSet<&Kept> = keep.iter().collect();
    let Ok(mut statement) = conn.prepare(
        "SELECT session_key, filter_name, target, angle_key, evidence_key, algorithm_version
         FROM psf_guard_flat_family_evidence",
    ) else {
        return Ok(0);
    };
    let rows: Vec<(Kept, u32)> = statement
        .query_map([], |row| {
            Ok((
                Kept {
                    session_key: row.get(0)?,
                    filter: row.get(1)?,
                    target: row.get(2)?,
                    angle: row.get(3)?,
                    key: row.get(4)?,
                },
                row.get(5)?,
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let mut removed = 0;
    for (place, version) in rows {
        let session = &place.session_key;
        let stale = version != FAMILY_EVIDENCE_VERSION
            || !all.contains(session)
            || (judged.contains(session) && !wanted.contains(&place));
        if stale {
            removed += conn.execute(
                "DELETE FROM psf_guard_flat_family_evidence
                 WHERE session_key = ?1 AND filter_name = ?2 AND target = ?3 AND angle_key = ?4
                   AND algorithm_version = ?5",
                params![
                    place.session_key,
                    place.filter,
                    place.target,
                    place.angle,
                    version
                ],
            )?;
        }
    }
    Ok(removed)
}

/// One light's rotator offset, as the job stores it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LightAngle {
    pub light_id: i64,
    pub rig: String,
    pub at: i64,
    pub offset: f64,
    /// Whether a PSF Guard pixel solve gave the sky angle.
    pub solved: bool,
    /// That solve's position angle, kept so an unchanged solve is not
    /// read again when the rig's convention is worked out.
    pub solved_pa: Option<f64>,
    /// What the solve was read from: its cache file's modification time,
    /// so an unchanged solve is not read again.
    pub solve_stamp: Option<String>,
}

pub(crate) fn load_angles(conn: &Connection) -> Result<Vec<LightAngle>> {
    if !has_table(conn, "psf_guard_flat_light_angle") {
        return Ok(Vec::new());
    }
    let mut statement = conn.prepare(
        "SELECT light_id, rig_uuid, captured_at, offset_deg, solved, solved_pa, solve_stamp
         FROM psf_guard_flat_light_angle",
    )?;
    let rows = statement
        .query_map([], |row| {
            Ok(LightAngle {
                light_id: row.get(0)?,
                rig: row.get(1)?,
                at: row.get(2)?,
                offset: row.get(3)?,
                solved: row.get::<_, i64>(4)? != 0,
                solved_pa: row.get(5)?,
                solve_stamp: row.get(6)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Replace the stored offsets with `angles`: rows that changed are
/// written, rows of lights no longer there go. Returns how many changed.
pub(crate) fn store_angles(conn: &Connection, angles: &[LightAngle]) -> Result<usize> {
    let known: std::collections::HashMap<i64, LightAngle> = load_angles(conn)?
        .into_iter()
        .map(|angle| (angle.light_id, angle))
        .collect();
    let tx = conn.unchecked_transaction()?;
    let mut changed = 0;
    let wanted: HashSet<i64> = angles.iter().map(|angle| angle.light_id).collect();
    for angle in angles {
        if known.get(&angle.light_id) == Some(angle) {
            continue;
        }
        tx.execute(
            "INSERT OR REPLACE INTO psf_guard_flat_light_angle
                (light_id, rig_uuid, captured_at, offset_deg, solved, solved_pa, solve_stamp)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                angle.light_id,
                angle.rig,
                angle.at,
                angle.offset,
                i64::from(angle.solved),
                angle.solved_pa,
                angle.solve_stamp
            ],
        )?;
        changed += 1;
    }
    for id in known.keys().filter(|id| !wanted.contains(id)) {
        tx.execute(
            "DELETE FROM psf_guard_flat_light_angle WHERE light_id = ?1",
            [id],
        )?;
        changed += 1;
    }
    tx.commit()?;
    Ok(changed)
}

/// Fold both tables into a digest of what grouping reads.
pub(crate) fn digest(conn: &Connection, hasher: &mut impl sha2::Digest) -> Result<()> {
    for (table, sql) in [
        (
            "psf_guard_flat_family_evidence",
            "SELECT session_key, filter_name, target, angle_key, algorithm_version, evidence_key,
                    COALESCE(chosen, '')
             FROM psf_guard_flat_family_evidence
             ORDER BY session_key, filter_name, target, angle_key, algorithm_version",
        ),
        (
            "psf_guard_flat_light_angle",
            "SELECT COUNT(*), COALESCE(MAX(light_id), 0), TOTAL(offset_deg), TOTAL(solved),
                    TOTAL(captured_at)
             FROM psf_guard_flat_light_angle",
        ),
    ] {
        if !has_table(conn, table) {
            continue;
        }
        let Ok(mut statement) = conn.prepare(sql) else {
            continue;
        };
        let columns = statement.column_count();
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            for index in 0..columns {
                match row.get_ref(index)? {
                    rusqlite::types::ValueRef::Text(text) => hasher.update(text),
                    rusqlite::types::ValueRef::Integer(value) => hasher.update(value.to_le_bytes()),
                    rusqlite::types::ValueRef::Real(value) => hasher.update(value.to_le_bytes()),
                    _ => hasher.update([0u8]),
                }
                hasher.update([0x1f]);
            }
            hasher.update([0x1e]);
        }
        hasher.update([0x1d]);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(key: &str) -> FamilyEvidenceRow {
        FamilyEvidenceRow {
            session_key: "p:1".into(),
            filter: "oiii".into(),
            target: "M 27".into(),
            angle: 82.0,
            angle_low: 81.8,
            angle_high: 82.3,
            start: 100,
            end: 900,
            key: key.into(),
            candidates: vec!["a".into(), "b".into()],
            chosen: Some("b".into()),
            lights: 8,
            targets: vec!["M 27".into()],
            pairs: vec![FamilyPairRow {
                first: "a".into(),
                second: "b".into(),
                outcome: "fit".into(),
                carried: Some(0.97),
                spread: Some(0.05),
                verdict: Some(super::super::measure::rows::Verdict::After),
            }],
        }
    }

    #[test]
    fn family_evidence_reads_back_and_goes_when_not_asked_for() {
        let conn = Connection::open_in_memory().unwrap();
        add_table(&conn).unwrap();
        upsert(&conn, &row("k1")).unwrap();
        // Another target of the same session and filter is a row of its
        // own.
        let other = FamilyEvidenceRow {
            target: "M 57".into(),
            ..row("k1")
        };
        upsert(&conn, &other).unwrap();
        assert_eq!(load(&conn).unwrap().len(), 2);
        let sessions: HashSet<String> = ["p:1".to_string()].into();
        let none = HashSet::new();
        let keep = vec![kept(&row("k1")), kept(&other)];
        assert_eq!(retain(&conn, &keep, &sessions, &sessions).unwrap(), 0);
        // Another rig's pass leaves them alone.
        assert_eq!(retain(&conn, &[], &none, &sessions).unwrap(), 0);
        // Other candidates now for M 27: its row is asked again.
        let keep = vec![kept(&row("k2")), kept(&other)];
        assert_eq!(retain(&conn, &keep, &sessions, &sessions).unwrap(), 1);
        assert_eq!(load(&conn).unwrap(), vec![other]);
        // A session the catalog lost goes, whoever's pass it is.
        assert_eq!(retain(&conn, &[], &none, &none).unwrap(), 1);
    }

    #[test]
    fn a_table_of_the_older_shape_is_made_again() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE psf_guard_flat_family_evidence (session_key TEXT, filter_name TEXT,
             algorithm_version INTEGER, evidence_key TEXT, chosen TEXT, detail_json TEXT,
             measured_at INTEGER)",
        )
        .unwrap();
        add_table(&conn).unwrap();
        upsert(&conn, &row("k1")).unwrap();
        assert_eq!(load(&conn).unwrap().len(), 1);
    }

    #[test]
    fn light_angles_are_written_only_when_they_change() {
        let conn = Connection::open_in_memory().unwrap();
        add_table(&conn).unwrap();
        let angle = |id: i64, offset: f64| LightAngle {
            light_id: id,
            rig: "rig".into(),
            at: 100 * id,
            offset,
            solved: false,
            solved_pa: None,
            solve_stamp: None,
        };
        assert_eq!(
            store_angles(&conn, &[angle(1, 76.0), angle(2, 76.1)]).unwrap(),
            2
        );
        assert_eq!(
            store_angles(&conn, &[angle(1, 76.0), angle(2, 76.1)]).unwrap(),
            0
        );
        assert_eq!(store_angles(&conn, &[angle(1, 79.0)]).unwrap(), 2);
        assert_eq!(load_angles(&conn).unwrap(), vec![angle(1, 79.0)]);
    }
}
