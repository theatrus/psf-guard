//! Plans taken in from a Target Scheduler project, and the draft revisions
//! the import saved. While a plan's drafts still have those revisions and it
//! was never activated, nobody has edited it here, so it follows the
//! project's rows: a later change in Target Scheduler is taken in again.
use super::*;

/// The project a plan's drafts came from, and their revisions then: the
/// framing's layout revision and the plan's revision. Zero stands for a
/// draft the import did not make.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DraftImport {
    pub project_id: Uuid,
    pub catalog_id: Uuid,
    pub source_project_guid: Uuid,
    pub framing_revision: u64,
    pub plan_revision: u64,
}

pub(crate) fn create_table(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS draft_import(
            project_id TEXT PRIMARY KEY NOT NULL REFERENCES global_project(id),
            catalog_id TEXT NOT NULL,
            source_project_guid TEXT NOT NULL,
            framing_revision INTEGER NOT NULL CHECK(framing_revision>=0),
            plan_revision INTEGER NOT NULL CHECK(plan_revision>=0));",
    )?;
    Ok(())
}

pub(crate) fn validate_table(conn: &Connection) -> Result<(), Error> {
    conn.prepare(
        "SELECT project_id,catalog_id,source_project_guid,framing_revision,plan_revision FROM draft_import LIMIT 0",
    )
    .map_err(|_| Error::CorruptDatabase)?;
    Ok(())
}

/// Plans an earlier build imported, found by what an import leaves: both
/// drafts at revision 1, saved in the same millisecond, one source project
/// and no activation. A plan made here saves its framing and plan apart.
pub(crate) fn backfill(conn: &Connection) -> Result<(), Error> {
    let mut statement = conn.prepare(
        "SELECT f.project_id, f.payload, p.payload, c.catalog_id, c.source_project_guid
         FROM framing_draft f
         JOIN plan_draft p ON p.project_id = f.project_id
         JOIN project_catalog c ON c.project_id = f.project_id
         WHERE f.revision = 1 AND p.revision = 1
           AND NOT EXISTS (SELECT 1 FROM activation a WHERE a.project_id = f.project_id)
           AND (SELECT count(*) FROM project_catalog o WHERE o.project_id = f.project_id) = 1",
    )?;
    let rows: Vec<(String, String, String, String, String)> = statement
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })?
        .collect::<Result<_, _>>()?;
    let saved_at = |payload: &str| {
        serde_json::from_str::<serde_json::Value>(payload)
            .ok()
            .and_then(|value| value["updated_at_ms"].as_u64())
    };
    for (project, framing, plan, catalog, source) in rows {
        let at = saved_at(&framing);
        if at.is_none() || at != saved_at(&plan) {
            continue;
        }
        conn.execute(
            "INSERT OR IGNORE INTO draft_import VALUES(?1,?2,?3,1,1)",
            params![project, catalog, source],
        )?;
    }
    Ok(())
}

fn revision_of(value: u64) -> Result<i64, Error> {
    i64::try_from(value).map_err(|_| Error::InvalidInput)
}

impl MetaStore {
    /// Where a plan's drafts were imported from, if they were.
    pub fn draft_import(&self, project: Uuid) -> Result<Option<DraftImport>, Error> {
        valid_id(project)?;
        let row = self
            .connection
            .query_row(
                "SELECT catalog_id, source_project_guid, framing_revision, plan_revision
                 FROM draft_import WHERE project_id=?1",
                [project.to_string()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                },
            )
            .optional()?;
        row.map(|(catalog, source, framing, plan)| {
            Ok(DraftImport {
                project_id: project,
                catalog_id: parse_id(&catalog)?,
                source_project_guid: parse_id(&source)?,
                framing_revision: u64::try_from(framing).map_err(|_| Error::CorruptDatabase)?,
                plan_revision: u64::try_from(plan).map_err(|_| Error::CorruptDatabase)?,
            })
        })
        .transpose()
    }

    /// Record, or move on, the revisions a plan's import saved.
    pub fn record_draft_import(&mut self, import: &DraftImport) -> Result<(), Error> {
        valid_id(import.project_id)?;
        valid_id(import.catalog_id)?;
        valid_id(import.source_project_guid)?;
        self.connection.execute(
            "INSERT INTO draft_import VALUES(?1,?2,?3,?4,?5)
             ON CONFLICT(project_id) DO UPDATE SET catalog_id=excluded.catalog_id,
                source_project_guid=excluded.source_project_guid,
                framing_revision=excluded.framing_revision, plan_revision=excluded.plan_revision",
            params![
                import.project_id.to_string(),
                import.catalog_id.to_string(),
                import.source_project_guid.to_string(),
                revision_of(import.framing_revision)?,
                revision_of(import.plan_revision)?,
            ],
        )?;
        Ok(())
    }

    /// Stop a plan following its source: someone saved over the import.
    pub fn forget_draft_import(&mut self, project: Uuid) -> Result<(), Error> {
        valid_id(project)?;
        self.connection.execute(
            "DELETE FROM draft_import WHERE project_id=?1",
            [project.to_string()],
        )?;
        Ok(())
    }
}
