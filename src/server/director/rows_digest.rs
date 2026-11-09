//! A fingerprint of the Target Scheduler rows an activation sets for a
//! project, so the plan list can tell that the rows changed since the last
//! activation without running one.
//!
//! Apply stores the fingerprint in the activation record for each rig; the
//! plan list reads every project's fingerprint from each database with one
//! query and compares. Both sides call [`project_digests`], so they read
//! the same fields the same way.
//!
//! The fingerprint covers each target's GUID, coordinates and rotation and,
//! for each exposure plan on it, the GUID, exposure, desired frames, whether
//! it is enabled, and its template's GUID and settings (filter, gain,
//! offset, binning, readout mode and default exposure), since activation
//! chooses a template by those settings. It leaves out what changes while
//! the plan is in sync: the frame counts N.I.N.A. raises as it shoots, the
//! project's state and priority, a target's active switch and the
//! scheduling limits, which other PSF Guard settings write.

use super::import_drafts;
use psf_guard_director_meta::Uuid;
use rusqlite::{types::Value, Connection};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

/// Each project's fingerprint, by project GUID, or only `project`'s. A
/// project row without a GUID, or with one that does not parse, is left out.
/// Both forms run the same query and read the GUID the same way, so Apply
/// and the plan list agree on which rows belong to a project.
pub(super) fn project_digests(
    connection: &Connection,
    project: Option<Uuid>,
) -> rusqlite::Result<HashMap<Uuid, String>> {
    // Columns that arrived with later Target Scheduler schemas.
    let column = |table: &str, name: &str, alias: &str| {
        if import_drafts::has_column(connection, table, name) {
            format!("{alias}.{name}")
        } else {
            "NULL".to_owned()
        }
    };
    let rotation = column("target", "rotation", "t");
    let default_exposure = column("exposuretemplate", "defaultexposure", "x");
    let sql = format!(
        "SELECT p.guid, t.guid, t.ra, t.dec, {rotation},
                e.guid, e.exposure, e.desired, e.enabled,
                x.guid, x.filtername, x.gain, x.\"offset\", x.bin, x.readoutmode, {default_exposure}
         FROM project p
         JOIN target t ON t.projectid = p.Id
         LEFT JOIN exposureplan e ON e.targetid = t.Id
         LEFT JOIN exposuretemplate x ON x.Id = e.exposureTemplateId
         WHERE p.guid IS NOT NULL
         ORDER BY lower(p.guid), t.guid, t.Id, e.guid, e.Id"
    );
    let mut statement = connection.prepare(&sql)?;
    let columns = statement.column_count();
    let mut rows = statement.query([])?;
    let mut hashers: HashMap<Uuid, Sha256> = HashMap::new();
    while let Some(row) = rows.next()? {
        let Ok(guid) = Uuid::parse_str(&row.get::<_, String>(0)?) else {
            continue;
        };
        if project.is_some_and(|only| only != guid) {
            continue;
        }
        let hasher = hashers.entry(guid).or_default();
        for column in 1..columns {
            // Debug keeps the type apart (an integer 1 is not a real 1.0) and
            // prints a float the shortest way that reads back the same.
            hasher.update(format!("{:?}\u{1f}", row.get::<_, Value>(column)?).as_bytes());
        }
        hasher.update(b"\x1e");
    }
    Ok(hashers
        .into_iter()
        .map(|(guid, hasher)| {
            let digest = hasher.finalize();
            (
                guid,
                digest.iter().map(|byte| format!("{byte:02x}")).collect(),
            )
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn database() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE project(Id INTEGER PRIMARY KEY, guid TEXT, state INTEGER, priority INTEGER);
                 CREATE TABLE target(Id INTEGER PRIMARY KEY, guid TEXT, projectid INTEGER, ra REAL, dec REAL, rotation REAL, active INTEGER);
                 CREATE TABLE exposuretemplate(Id INTEGER PRIMARY KEY, guid TEXT, filtername TEXT, gain INTEGER,
                     \"offset\" INTEGER, bin INTEGER, readoutmode INTEGER, defaultexposure REAL);
                 CREATE TABLE exposureplan(Id INTEGER PRIMARY KEY, guid TEXT, targetid INTEGER, exposureTemplateId INTEGER,
                     exposure REAL, desired INTEGER, acquired INTEGER, accepted INTEGER, enabled INTEGER);
                 INSERT INTO project VALUES (1, '6F1E2D3C-0000-4000-8000-000000000001', 1, 1),
                                            (2, '6f1e2d3c-0000-4000-8000-000000000002', 1, 1);
                 INSERT INTO target VALUES (1, 'tgt-1', 1, 2.5, 61.4, 15.0, 1), (2, 'tgt-2', 2, 3.0, 10.0, 0.0, 1);
                 INSERT INTO exposuretemplate VALUES (1, 'tmpl-ha', 'Ha', 100, 30, 1, -1, 300),
                                                     (2, 'tmpl-oiii', 'OIII', 100, 30, 1, -1, 300);
                 INSERT INTO exposureplan VALUES (1, 'ep-1', 1, 1, 300, 30, 0, 0, 1);",
            )
            .unwrap();
        connection
    }

    #[test]
    fn a_digest_moves_with_the_rows_activation_sets_and_ignores_the_rest() {
        let db = database();
        let one = Uuid::parse_str("6f1e2d3c-0000-4000-8000-000000000001").unwrap();
        let all = project_digests(&db, None).unwrap();
        assert_eq!(all.len(), 2);
        // One project alone reads the same, whatever the GUID's case.
        let alone = project_digests(&db, Some(one)).unwrap();
        assert_eq!(alone.len(), 1);
        assert_eq!(alone[&one], all[&one]);
        let digest = || project_digests(&db, Some(one)).unwrap()[&one].clone();
        let start = digest();

        // Frames taken, the project's state and priority, and a target's
        // active switch change nothing.
        db.execute_batch(
            "UPDATE exposureplan SET acquired = 12, accepted = 10;
             UPDATE project SET state = 2, priority = 0;
             UPDATE target SET active = 0;",
        )
        .unwrap();
        assert_eq!(digest(), start);

        // Each field activation writes does.
        for edit in [
            "UPDATE target SET ra = 2.6 WHERE Id = 1",
            "UPDATE target SET rotation = 20 WHERE Id = 1",
            "UPDATE exposureplan SET desired = 45",
            "UPDATE exposureplan SET exposure = 180",
            "UPDATE exposureplan SET enabled = 0",
            "UPDATE exposureplan SET exposureTemplateId = 2",
            "UPDATE exposuretemplate SET gain = 200 WHERE Id = 1",
            "UPDATE exposuretemplate SET filtername = 'H' WHERE Id = 1",
            "UPDATE exposuretemplate SET defaultexposure = 180 WHERE Id = 1",
            "INSERT INTO exposureplan VALUES (2, 'ep-2', 1, 2, 300, 30, 0, 0, 1)",
            "DELETE FROM exposureplan",
        ] {
            let db2 = database();
            db2.execute_batch(edit).unwrap();
            assert_ne!(
                project_digests(&db2, Some(one)).unwrap().get(&one),
                Some(&start),
                "{edit}"
            );
        }
        // Another project's change leaves this one alone.
        db.execute("UPDATE target SET ra = 9 WHERE Id = 2", [])
            .unwrap();
        assert_eq!(digest(), start);
    }
}
