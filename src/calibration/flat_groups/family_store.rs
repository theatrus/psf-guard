//! `psf_guard_flat_family_evidence`: what an imaging session's own lights
//! said, per filter, about which response family their flats come from.
//!
//! Kept beside the other flat grouping tables ([`super::store`]), and made
//! the same way: additively, in a writable catalog, never in a read-only
//! one. A catalog without the table chooses by angle alone.

use super::lights::FAMILY_EVIDENCE_VERSION;
use super::measure::rows::{FamilyEvidenceRow, FamilyPairRow};
use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

pub(crate) const TABLE_SQL: &str = r#"
    CREATE TABLE IF NOT EXISTS psf_guard_flat_family_evidence (
        session_key        TEXT NOT NULL,
        filter_name        TEXT NOT NULL,
        algorithm_version  INTEGER NOT NULL,
        evidence_key       TEXT NOT NULL,
        chosen             TEXT,
        detail_json        TEXT NOT NULL,
        measured_at        INTEGER NOT NULL,
        PRIMARY KEY (session_key, filter_name, algorithm_version)
    );
"#;

pub(crate) fn table_exists(conn: &Connection) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type = 'table'
         AND name = 'psf_guard_flat_family_evidence'",
        [],
        |_| Ok(()),
    )
    .optional()
    .map(|row| row.is_some())
    .unwrap_or(false)
}

/// Create the table in a writable catalog. Safe to run twice.
pub(crate) fn add_table(conn: &Connection) -> Result<()> {
    if table_exists(conn) || conn.is_readonly(rusqlite::MAIN_DB).unwrap_or(true) {
        return Ok(());
    }
    conn.execute_batch(TABLE_SQL)
        .context("creating the flat family evidence table")
}

#[derive(Serialize, Deserialize)]
struct Detail {
    candidates: Vec<String>,
    lights: usize,
    targets: Vec<String>,
    pairs: Vec<FamilyPairRow>,
}

pub(crate) fn upsert(conn: &Connection, row: &FamilyEvidenceRow) -> Result<()> {
    let detail = Detail {
        candidates: row.candidates.clone(),
        lights: row.lights,
        targets: row.targets.clone(),
        pairs: row.pairs.clone(),
    };
    conn.execute(
        "INSERT OR REPLACE INTO psf_guard_flat_family_evidence
            (session_key, filter_name, algorithm_version, evidence_key, chosen, detail_json,
             measured_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            row.session_key,
            row.filter,
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
    let mut statement = conn.prepare(
        "SELECT session_key, filter_name, evidence_key, chosen, detail_json
         FROM psf_guard_flat_family_evidence WHERE algorithm_version = ?1",
    )?;
    let rows = statement
        .query_map([FAMILY_EVIDENCE_VERSION], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, String>(4)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows
        .into_iter()
        .filter_map(|(session_key, filter, key, chosen, detail)| {
            let detail: Detail = serde_json::from_str(&detail).ok()?;
            Some(FamilyEvidenceRow {
                session_key,
                filter,
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

/// Drop the rows a pass no longer asks for: of an older algorithm, of an
/// imaging session the catalog lost (not in `all`), or of a session whose
/// rig the pass judged (in `judged`) that `keep` does not name with its
/// current key, `(session, filter, key)`. Other rigs' rows wait for their
/// own pass.
pub(crate) fn retain(
    conn: &Connection,
    keep: &[(String, String, String)],
    judged: &std::collections::HashSet<String>,
    all: &std::collections::HashSet<String>,
) -> Result<usize> {
    if !table_exists(conn) {
        return Ok(0);
    }
    let wanted: std::collections::HashSet<(&str, &str, &str)> = keep
        .iter()
        .map(|(s, f, k)| (s.as_str(), f.as_str(), k.as_str()))
        .collect();
    let rows: Vec<(String, String, String, u32)> = conn
        .prepare(
            "SELECT session_key, filter_name, evidence_key, algorithm_version
             FROM psf_guard_flat_family_evidence",
        )?
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let mut removed = 0;
    for (session, filter, key, version) in rows {
        let stale = version != FAMILY_EVIDENCE_VERSION
            || !all.contains(&session)
            || (judged.contains(&session)
                && !wanted.contains(&(session.as_str(), filter.as_str(), key.as_str())));
        if stale {
            removed += conn.execute(
                "DELETE FROM psf_guard_flat_family_evidence
                 WHERE session_key = ?1 AND filter_name = ?2 AND algorithm_version = ?3",
                params![session, filter, version],
            )?;
        }
    }
    Ok(removed)
}

/// Fold the table into a digest of what grouping reads.
pub(crate) fn digest(conn: &Connection, hasher: &mut impl sha2::Digest) -> Result<()> {
    if !table_exists(conn) {
        return Ok(());
    }
    let mut statement = conn.prepare(
        "SELECT session_key, filter_name, algorithm_version, evidence_key, COALESCE(chosen, '')
         FROM psf_guard_flat_family_evidence
         ORDER BY session_key, filter_name, algorithm_version",
    )?;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        for index in 0..5 {
            match row.get_ref(index)? {
                rusqlite::types::ValueRef::Text(text) => hasher.update(text),
                rusqlite::types::ValueRef::Integer(value) => hasher.update(value.to_le_bytes()),
                _ => hasher.update([0u8]),
            }
            hasher.update([0x1f]);
        }
        hasher.update([0x1e]);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn family_evidence_reads_back_and_goes_when_not_asked_for() {
        let conn = Connection::open_in_memory().unwrap();
        add_table(&conn).unwrap();
        let row = FamilyEvidenceRow {
            session_key: "p:1".into(),
            filter: "oiii".into(),
            key: "k1".into(),
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
        };
        upsert(&conn, &row).unwrap();
        assert_eq!(load(&conn).unwrap(), vec![row.clone()]);
        let sessions: std::collections::HashSet<String> = ["p:1".to_string()].into();
        let none = std::collections::HashSet::new();
        let keep = |key: &str| vec![("p:1".to_string(), "oiii".to_string(), key.to_string())];
        assert_eq!(retain(&conn, &keep("k1"), &sessions, &sessions).unwrap(), 0);
        // Another rig's pass leaves it alone.
        assert_eq!(retain(&conn, &[], &none, &sessions).unwrap(), 0);
        // Other candidates now: the row is asked again.
        assert_eq!(retain(&conn, &keep("k2"), &sessions, &sessions).unwrap(), 1);
        assert!(load(&conn).unwrap().is_empty());
        // A session the catalog lost goes, whoever's pass it is.
        upsert(&conn, &row).unwrap();
        assert_eq!(retain(&conn, &[], &none, &none).unwrap(), 1);
    }
}
