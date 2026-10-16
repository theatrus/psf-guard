//! Flat grouping's own tables, beside the calibration library's.
//!
//! What a pass measured is kept in the catalog, so a regroup after a
//! setting changes needs no pixels and a copy of the catalog carries it:
//!
//! - `psf_guard_flat_response`: one row per measured flat session;
//! - `psf_guard_flat_comparison`: two sessions' metrics, limit-free;
//! - `psf_guard_flat_light_evidence`: what an imaging session's own lights
//!   say about one change.
//!
//! The response planes themselves are cache files ([`super::response_path`]).
//! The tables are created the way the grade table was: additively, without
//! raising the calibration schema version, so older builds ignore them.

use super::measure::metric::{Comparison, Spot};
pub use super::measure::rows::{ComparisonRow, EvidenceFit, EvidenceRow, ResponseRow, Verdict};
use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

pub(crate) const TABLES_SQL: &str = r#"
    CREATE TABLE IF NOT EXISTS psf_guard_flat_response (
        response_key       TEXT PRIMARY KEY,
        session_key        TEXT NOT NULL,
        rig_uuid           TEXT NOT NULL,
        family_key         TEXT NOT NULL,
        night              TEXT NOT NULL,
        filter_name        TEXT,
        rotation           REAL,
        frame_uuids        TEXT NOT NULL,
        frames             INTEGER NOT NULL,
        captured_first     INTEGER,
        captured_last      INTEGER,
        level_adu          REAL,
        pedestal_adu       REAL,
        noise              REAL,
        width              INTEGER,
        height             INTEGER,
        bin                INTEGER,
        algorithm_version  INTEGER NOT NULL,
        error              TEXT,
        measured_at        INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS idx_psf_guard_flat_response_family
        ON psf_guard_flat_response(rig_uuid, family_key);
    CREATE TABLE IF NOT EXISTS psf_guard_flat_comparison (
        earlier_key        TEXT NOT NULL,
        later_key          TEXT NOT NULL,
        algorithm_version  INTEGER NOT NULL,
        broad              REAL NOT NULL,
        dust               REAL NOT NULL,
        sigma              REAL NOT NULL,
        detail_json        TEXT NOT NULL,
        measured_at        INTEGER NOT NULL,
        PRIMARY KEY (earlier_key, later_key, algorithm_version)
    );
    CREATE TABLE IF NOT EXISTS psf_guard_flat_light_evidence (
        session_key        TEXT NOT NULL,
        event_key          TEXT NOT NULL,
        algorithm_version  INTEGER NOT NULL,
        verdict            TEXT NOT NULL,
        detail_json        TEXT NOT NULL,
        measured_at        INTEGER NOT NULL,
        PRIMARY KEY (session_key, event_key, algorithm_version)
    );
"#;

/// Whether this catalog has the tables.
pub(crate) fn tables_exist(conn: &Connection) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'psf_guard_flat_comparison'",
        [],
        |_| Ok(()),
    )
    .optional()
    .map(|row| row.is_some())
    .unwrap_or(false)
}

/// Create the tables in a writable catalog that holds a calibration
/// library. Safe to run twice; adds only.
pub(crate) fn add_tables(conn: &Connection) -> Result<bool> {
    if tables_exist(conn) || conn.is_readonly(rusqlite::MAIN_DB).unwrap_or(true) {
        return Ok(false);
    }
    conn.execute_batch(TABLES_SQL)
        .context("creating the flat grouping tables")?;
    Ok(true)
}

pub(crate) fn upsert_response(conn: &Connection, row: &ResponseRow) -> Result<()> {
    conn.execute(
        "INSERT INTO psf_guard_flat_response (
            response_key, session_key, rig_uuid, family_key, night, filter_name, rotation,
            frame_uuids, frames, captured_first, captured_last, level_adu, pedestal_adu, noise,
            width, height, bin, algorithm_version, error, measured_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17,
                 ?18, ?19, ?20)
         ON CONFLICT(response_key) DO UPDATE SET
            session_key = excluded.session_key, rig_uuid = excluded.rig_uuid,
            family_key = excluded.family_key, night = excluded.night,
            filter_name = excluded.filter_name, rotation = excluded.rotation,
            frame_uuids = excluded.frame_uuids, frames = excluded.frames,
            captured_first = excluded.captured_first, captured_last = excluded.captured_last,
            level_adu = excluded.level_adu, pedestal_adu = excluded.pedestal_adu,
            noise = excluded.noise, width = excluded.width, height = excluded.height,
            bin = excluded.bin, algorithm_version = excluded.algorithm_version,
            error = excluded.error, measured_at = excluded.measured_at",
        params![
            row.response_key,
            row.session_key,
            row.rig_uuid,
            row.family_key,
            row.night,
            row.filter,
            row.rotation,
            serde_json::to_string(&row.frame_uuids)?,
            row.frames as i64,
            row.captured_first,
            row.captured_last,
            row.level_adu,
            row.pedestal_adu,
            row.noise,
            row.width.map(|v| v as i64),
            row.height.map(|v| v as i64),
            row.bin.map(|v| v as i64),
            row.algorithm_version,
            row.error,
            chrono::Utc::now().timestamp(),
        ],
    )?;
    Ok(())
}

pub(crate) fn load_responses(conn: &Connection) -> Result<Vec<ResponseRow>> {
    if !tables_exist(conn) {
        return Ok(Vec::new());
    }
    let mut statement = conn.prepare(
        "SELECT response_key, session_key, rig_uuid, family_key, night, filter_name, rotation,
                frame_uuids, frames, captured_first, captured_last, level_adu, pedestal_adu,
                noise, width, height, bin, algorithm_version, error
         FROM psf_guard_flat_response ORDER BY captured_first, response_key",
    )?;
    let rows = statement
        .query_map([], |row| {
            let uuids: String = row.get(7)?;
            Ok(ResponseRow {
                response_key: row.get(0)?,
                session_key: row.get(1)?,
                rig_uuid: row.get(2)?,
                family_key: row.get(3)?,
                night: row.get(4)?,
                filter: row.get(5)?,
                rotation: row.get(6)?,
                frame_uuids: serde_json::from_str(&uuids).unwrap_or_default(),
                frames: row.get::<_, i64>(8)? as usize,
                captured_first: row.get(9)?,
                captured_last: row.get(10)?,
                level_adu: row.get(11)?,
                pedestal_adu: row.get(12)?,
                noise: row.get(13)?,
                width: row.get::<_, Option<i64>>(14)?.map(|v| v as usize),
                height: row.get::<_, Option<i64>>(15)?.map(|v| v as usize),
                bin: row.get::<_, Option<i64>>(16)?.map(|v| v as usize),
                algorithm_version: row.get(17)?,
                error: row.get(18)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Drop response rows no current session has, with their comparisons.
pub(crate) fn retain_responses(conn: &Connection, keep: &[String]) -> Result<usize> {
    let tx = conn.unchecked_transaction()?;
    tx.execute_batch("CREATE TEMP TABLE IF NOT EXISTS flat_keep (key TEXT PRIMARY KEY)")?;
    tx.execute("DELETE FROM flat_keep", [])?;
    {
        let mut insert = tx.prepare("INSERT OR IGNORE INTO flat_keep (key) VALUES (?1)")?;
        for key in keep {
            insert.execute([key])?;
        }
    }
    let removed = tx.execute(
        "DELETE FROM psf_guard_flat_response
         WHERE response_key NOT IN (SELECT key FROM flat_keep)",
        [],
    )?;
    tx.execute(
        "DELETE FROM psf_guard_flat_comparison
         WHERE earlier_key NOT IN (SELECT key FROM flat_keep)
            OR later_key NOT IN (SELECT key FROM flat_keep)",
        [],
    )?;
    tx.execute("DELETE FROM flat_keep", [])?;
    tx.commit()?;
    Ok(removed)
}

/// A session's pedestal changed, as when a dark-flat arrived after it was
/// measured: keep the new one, and forget its comparisons, which were made
/// with the old one taken out, so the next pass makes them again.
/// Rename a measured session's family and session keys. Its plane and its
/// comparisons stand: they are keyed by the frames, not the family.
pub(crate) fn set_family(
    conn: &Connection,
    response_key: &str,
    family_key: &str,
    session_key: &str,
) -> Result<()> {
    conn.execute(
        "UPDATE psf_guard_flat_response SET family_key = ?2, session_key = ?3
         WHERE response_key = ?1",
        params![response_key, family_key, session_key],
    )?;
    Ok(())
}

pub(crate) fn set_pedestal(
    conn: &Connection,
    response_key: &str,
    pedestal: Option<f64>,
) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "UPDATE psf_guard_flat_response SET pedestal_adu = ?2 WHERE response_key = ?1",
        params![response_key, pedestal],
    )?;
    tx.execute(
        "DELETE FROM psf_guard_flat_comparison WHERE earlier_key = ?1 OR later_key = ?1",
        [response_key],
    )?;
    tx.commit()?;
    Ok(())
}

/// Frames that left the library, or merged into a twin: the responses
/// built from any of them go, with their comparisons, so the next pass
/// measures those sessions again from the frames the library holds now.
/// Returns how many responses went.
pub(crate) fn forget_frames(conn: &Connection, frame_uuids: &[String]) -> Result<usize> {
    if frame_uuids.is_empty() || !tables_exist(conn) {
        return Ok(0);
    }
    let wanted: std::collections::HashSet<&str> = frame_uuids.iter().map(String::as_str).collect();
    let keys: Vec<String> = load_responses(conn)?
        .into_iter()
        .filter(|row| {
            row.frame_uuids
                .iter()
                .any(|uuid| wanted.contains(uuid.as_str()))
        })
        .map(|row| row.response_key)
        .collect();
    let mut comparisons = conn.prepare(
        "DELETE FROM psf_guard_flat_comparison WHERE earlier_key = ?1 OR later_key = ?1",
    )?;
    let mut responses =
        conn.prepare("DELETE FROM psf_guard_flat_response WHERE response_key = ?1")?;
    for key in &keys {
        comparisons.execute([key])?;
        responses.execute([key])?;
    }
    Ok(keys.len())
}

#[derive(Serialize, Deserialize)]
struct Detail {
    spots: Vec<Spot>,
    broad_x: f32,
    broad_y: f32,
    star_fraction: f32,
}

pub(crate) fn insert_comparison(
    conn: &Connection,
    earlier: &str,
    later: &str,
    comparison: &Comparison,
) -> Result<()> {
    let detail = Detail {
        spots: comparison.spots.clone(),
        broad_x: comparison.broad_x,
        broad_y: comparison.broad_y,
        star_fraction: comparison.star_fraction,
    };
    conn.execute(
        "INSERT OR REPLACE INTO psf_guard_flat_comparison
            (earlier_key, later_key, algorithm_version, broad, dust, sigma, detail_json,
             measured_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            earlier,
            later,
            comparison.algorithm_version,
            f64::from(comparison.broad),
            f64::from(comparison.dust),
            f64::from(comparison.sigma),
            serde_json::to_string(&detail)?,
            chrono::Utc::now().timestamp(),
        ],
    )?;
    Ok(())
}

pub(crate) fn load_comparisons(conn: &Connection) -> Result<Vec<ComparisonRow>> {
    if !tables_exist(conn) {
        return Ok(Vec::new());
    }
    let mut statement = conn.prepare(
        "SELECT earlier_key, later_key, broad, dust, sigma, detail_json
         FROM psf_guard_flat_comparison WHERE algorithm_version = ?1",
    )?;
    let rows = statement
        .query_map([super::metric::COMPARE_ALGORITHM_VERSION], |row| {
            let detail: String = row.get(5)?;
            let detail: Option<Detail> = serde_json::from_str(&detail).ok();
            Ok(ComparisonRow {
                earlier_key: row.get(0)?,
                later_key: row.get(1)?,
                broad: row.get(2)?,
                dust: row.get(3)?,
                sigma: row.get(4)?,
                spots: detail.as_ref().map(|d| d.spots.clone()).unwrap_or_default(),
                broad_x: detail.as_ref().map_or(0.5, |d| f64::from(d.broad_x)),
                broad_y: detail.as_ref().map_or(0.5, |d| f64::from(d.broad_y)),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub(crate) fn upsert_evidence(conn: &Connection, row: &EvidenceRow, version: u32) -> Result<()> {
    conn.execute(
        "INSERT OR REPLACE INTO psf_guard_flat_light_evidence
            (session_key, event_key, algorithm_version, verdict, detail_json, measured_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            row.session_key,
            row.event_key,
            version,
            row.verdict.as_str(),
            serde_json::to_string(&row.fits)?,
            chrono::Utc::now().timestamp(),
        ],
    )?;
    Ok(())
}

pub(crate) fn load_evidence(conn: &Connection, version: u32) -> Result<Vec<EvidenceRow>> {
    if !tables_exist(conn) {
        return Ok(Vec::new());
    }
    let mut statement = conn.prepare(
        "SELECT session_key, event_key, verdict, detail_json
         FROM psf_guard_flat_light_evidence WHERE algorithm_version = ?1",
    )?;
    let rows = statement
        .query_map([version], |row| {
            let verdict: String = row.get(2)?;
            let detail: String = row.get(3)?;
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                verdict,
                detail,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows
        .into_iter()
        .filter_map(|(session_key, event_key, verdict, detail)| {
            Some(EvidenceRow {
                session_key,
                event_key,
                verdict: Verdict::parse(&verdict)?,
                fits: serde_json::from_str(&detail).unwrap_or_default(),
            })
        })
        .collect())
}

/// A digest of everything the grouping reads from the catalog, for keys
/// that must change when it does (health, the memoized book).
pub(crate) fn digest_rows(conn: &Connection, hasher: &mut impl sha2::Digest) -> Result<()> {
    if !tables_exist(conn) {
        return Ok(());
    }
    for sql in [
        "SELECT response_key, error FROM psf_guard_flat_response ORDER BY response_key",
        "SELECT earlier_key, later_key, algorithm_version, broad, dust FROM psf_guard_flat_comparison
         ORDER BY earlier_key, later_key, algorithm_version",
        "SELECT session_key, event_key, algorithm_version, verdict FROM psf_guard_flat_light_evidence
         ORDER BY session_key, event_key, algorithm_version",
    ] {
        let mut statement = conn.prepare(sql)?;
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

    #[test]
    fn evidence_without_a_spread_reads_back_with_its_fits() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(TABLES_SQL).unwrap();
        let fit = |filter: &str, spread, verdict| EvidenceFit {
            filter: filter.into(),
            targets: vec!["NGC 7023".into()],
            lights: 8,
            carried: 0.1,
            spread,
            verdict,
        };
        let row = EvidenceRow {
            session_key: "p:1".into(),
            event_key: "e".into(),
            verdict: Verdict::Before,
            // Too few placebos fit beside a large change: no spread.
            fits: vec![
                fit("B", Some(0.2), Verdict::Before),
                fit("L", None, Verdict::Undecided),
            ],
        };
        upsert_evidence(&conn, &row, 1).unwrap();
        assert_eq!(load_evidence(&conn, 1).unwrap(), vec![row]);
    }
}
