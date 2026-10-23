//! Leave a collaboration: the plan an import created becomes an ordinary
//! local plan. See docs/design/collaboration.md, "Leaving a collaboration".
//!
//! Leaving removes the plan's link to its remote project, its imports with
//! their revisions, queued and acknowledged reports and capture records,
//! and the remote project from every nightly intake policy on that server.
//! It cancels unfinished file transfers for the imports. The plan keeps its
//! framing, objectives, recipes, links and activation record. The host ends
//! the rig databases' assignments and tells the coordinator.

use super::collaboration_connection::ConnectionBinding;
use super::*;
use std::collections::BTreeSet;

/// A task the coordinator assigned to one of this server's rigs for the
/// plan: what leaving declines there.
#[derive(Clone, Debug, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct RemoteTask {
    pub agent_id: String,
    pub task_id: String,
    /// The rig the agent is bound to, as the dialogs name it.
    pub rig: String,
}

/// The error a transfer cancelled by leaving carries. A later automatic
/// request for the same files, after the plan rejoins, replaces such a
/// transfer; a transfer a person cancelled stays cancelled.
pub const LEFT_COLLABORATION: &str = "Cancelled: the plan left its collaboration";

/// A nightly intake policy that lists the plan's remote project.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct NightlyIntake {
    pub connection_id: Uuid,
    pub name: String,
}

/// The collaboration an imported plan came from, and what leaving it
/// removes.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct PlanCollaboration {
    pub base_url: String,
    pub remote_project_id: String,
    /// The project's name in the newest import, when it has one.
    pub remote_project_name: Option<String>,
    pub import_ids: Vec<Uuid>,
    /// Reports not yet acknowledged by the coordinator.
    pub unsent_reports: u32,
    /// Reports the coordinator acknowledged.
    pub sent_reports: u32,
    /// Frames queued for or sent in a report. Every queued report records
    /// its frames here, so this counts them whether or not they were sent.
    pub reported_frames: u32,
    /// File transfers for the imports that have not finished.
    pub transfers: u32,
    pub nightly_intake: Vec<NightlyIntake>,
    pub tasks: Vec<RemoteTask>,
}

impl MetaStore {
    /// The collaboration a plan came from, or `None` for a plan of its own.
    pub fn plan_collaboration(&self, plan: Uuid) -> Result<Option<PlanCollaboration>, Error> {
        valid_id(plan)?;
        read(&self.connection, plan)
    }

    /// Leave the collaboration a plan came from, in one transaction. Returns
    /// what was removed. A plan that did not come from a collaboration is
    /// refused with [`Error::Conflict`].
    pub fn leave_collaboration(&mut self, plan: Uuid) -> Result<PlanCollaboration, Error> {
        valid_id(plan)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if read_named(&tx, Kind::Project, plan)?.is_none() {
            return Err(Error::NotFound);
        }
        let left = leave(&tx, plan)?.ok_or(Error::Conflict)?;
        tx.commit()?;
        Ok(left)
    }
}

/// Read what [`leave`] would remove.
fn read(conn: &Connection, plan: Uuid) -> Result<Option<PlanCollaboration>, Error> {
    let id = plan.to_string();
    let linked: Option<(String, String)> = conn
        .query_row(
            "SELECT base_url,remote_project_id FROM collaboration_project WHERE project_id=?1",
            [&id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((base_url, remote_project_id)) = linked else {
        return Ok(None);
    };
    let mut statement = conn.prepare(
        "SELECT i.id,i.agent_id,i.task_id,r.name FROM collaboration_import i
         JOIN rig r ON r.id=i.rig_id WHERE i.project_id=?1
         ORDER BY i.imported_at_ms DESC,i.id",
    )?;
    let rows = statement
        .query_map([&id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(statement);
    let mut import_ids = Vec::new();
    let mut tasks = BTreeSet::new();
    for (import, agent_id, task_id, rig) in rows {
        import_ids.push(parse_id(&import)?);
        tasks.insert(RemoteTask {
            agent_id,
            task_id,
            rig,
        });
    }
    let remote_project_name = match import_ids.first() {
        Some(newest) => super::collaboration::read_import(conn, *newest)?
            .and_then(|import| import.plan.share().name.clone())
            .map(|name| name.trim().to_owned())
            .filter(|name| !name.is_empty()),
        None => None,
    };
    let count = |sql: &str| -> Result<u32, Error> {
        let n: i64 = conn.query_row(sql, [&id], |row| row.get(0))?;
        u32::try_from(n).map_err(|_| Error::CorruptDatabase)
    };
    const IMPORTS: &str = "SELECT id FROM collaboration_import WHERE project_id=?1";
    let unsent_reports = count(&format!(
        "SELECT count(*) FROM collaboration_outbox WHERE acknowledged_at_ms IS NULL AND import_id IN ({IMPORTS})"
    ))?;
    let sent_reports = count(&format!(
        "SELECT count(*) FROM collaboration_outbox WHERE acknowledged_at_ms IS NOT NULL AND import_id IN ({IMPORTS})"
    ))?;
    let reported_frames = count(&format!(
        "SELECT count(*) FROM collaboration_capture WHERE import_id IN ({IMPORTS})"
    ))?;
    let transfers = count(&format!(
        "SELECT count(*) FROM collaboration_transfer t JOIN collaboration_transfer_definition d ON d.id=t.id
         WHERE json_extract(t.payload,'$.state') NOT IN ('complete','cancelled')
           AND json_extract(d.request,'$.selection.import_id') IN ({IMPORTS})"
    ))?;
    let nightly_intake = connections(conn, &base_url)?
        .into_iter()
        .filter(|(binding, _)| {
            binding
                .background
                .as_ref()
                .is_some_and(|policy| policy.project_ids.contains(&remote_project_id))
        })
        .map(|(binding, _)| NightlyIntake {
            connection_id: binding.id,
            name: binding.name,
        })
        .collect();
    Ok(Some(PlanCollaboration {
        base_url,
        remote_project_id,
        remote_project_name,
        import_ids,
        unsent_reports,
        sent_reports,
        reported_frames,
        transfers,
        nightly_intake,
        tasks: tasks.into_iter().collect(),
    }))
}

/// Every connection to one collaboration server, with its stored payload
/// for a compare-and-set update.
fn connections(
    conn: &Connection,
    base_url: &str,
) -> Result<Vec<(ConnectionBinding, String)>, Error> {
    let mut statement = conn.prepare(
        "SELECT id,payload FROM collaboration_connection WHERE base_url=?1 ORDER BY id LIMIT 257",
    )?;
    let rows = statement
        .query_map([base_url], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.into_iter()
        .map(|(id, payload)| {
            let binding: ConnectionBinding =
                super::configuration::decode(payload.clone().into_bytes())?;
            if binding.id.to_string() != id || binding.base_url != base_url {
                return Err(Error::CorruptDatabase);
            }
            Ok((binding, payload))
        })
        .collect()
}

/// Remove a plan's collaboration records inside `tx`, as
/// [`MetaStore::leave_collaboration`] describes. `None` when the plan did not
/// come from a collaboration. Rows go in foreign-key order: capture records
/// and reports name a revision, a revision names its import, and an import
/// names the plan.
pub(crate) fn leave(tx: &Connection, plan: Uuid) -> Result<Option<PlanCollaboration>, Error> {
    let Some(left) = read(tx, plan)? else {
        return Ok(None);
    };
    let id = plan.to_string();
    const IMPORTS: &str = "SELECT id FROM collaboration_import WHERE project_id=?1";
    cancel_transfers(tx, &id)?;
    for table in [
        "collaboration_capture",
        "collaboration_outbox",
        "collaboration_revision",
    ] {
        tx.execute(
            &format!("DELETE FROM {table} WHERE import_id IN ({IMPORTS})"),
            [&id],
        )?;
    }
    tx.execute(
        "DELETE FROM collaboration_import WHERE project_id=?1",
        [&id],
    )?;
    tx.execute(
        "DELETE FROM collaboration_project WHERE project_id=?1",
        [&id],
    )?;
    // A remote project ID is unique on its server only, so only policies
    // for the same server lose it.
    for (mut binding, payload) in connections(tx, &left.base_url)? {
        let Some(policy) = binding.background.as_mut() else {
            continue;
        };
        let before = policy.project_ids.len();
        policy.project_ids.retain(|p| *p != left.remote_project_id);
        if policy.project_ids.len() == before {
            continue;
        }
        // An enabled policy needs a project; with none left it is off.
        if policy.project_ids.is_empty() {
            policy.enabled = false;
        }
        let changed = tx.execute(
            "UPDATE collaboration_connection SET payload=?2 WHERE id=?1 AND payload=?3",
            params![
                binding.id.to_string(),
                super::configuration::encode(&binding)?,
                payload
            ],
        )?;
        if changed != 1 {
            return Err(Error::Conflict);
        }
        // As any policy change does: the next nightly run fetches again
        // rather than applying a response saved under the old policy.
        for table in ["collaboration_nightly_run", "collaboration_nightly_pending"] {
            tx.execute(
                &format!("DELETE FROM {table} WHERE connection_id=?1"),
                [binding.id.to_string()],
            )?;
        }
    }
    Ok(Some(left))
}

/// Cancel each unfinished transfer for the plan's imports, as Cancel does,
/// so the transfer worker does not prepare files for an import that is gone.
fn cancel_transfers(tx: &Connection, plan: &str) -> Result<(), Error> {
    let mut statement = tx.prepare(
        "SELECT t.id,t.revision,t.payload FROM collaboration_transfer t
         JOIN collaboration_transfer_definition d ON d.id=t.id
         WHERE json_extract(t.payload,'$.state') NOT IN ('complete','cancelled')
           AND json_extract(d.request,'$.selection.import_id') IN
               (SELECT id FROM collaboration_import WHERE project_id=?1)",
    )?;
    let rows = statement
        .query_map([plan], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    drop(statement);
    const ERROR: &str = LEFT_COLLABORATION;
    for (id, revision, payload) in rows {
        let mut job: super::transfer::Job =
            serde_json::from_str(&payload).map_err(|_| Error::CorruptDatabase)?;
        let next = revision.checked_add(1).ok_or(Error::CorruptDatabase)?;
        job.state = super::transfer::State::Cancelled;
        job.revision = u64::try_from(next).map_err(|_| Error::CorruptDatabase)?;
        job.error = Some(ERROR.into());
        job.next_attempt_ms = 0;
        let payload = serde_json::to_string(&job).map_err(|_| Error::InvalidInput)?;
        tx.execute(
            "UPDATE collaboration_transfer SET revision=?2,payload=?3 WHERE id=?1 AND revision=?4",
            params![id, next, payload, revision],
        )?;
        tx.execute(
            "UPDATE collaboration_transfer_summary
             SET payload=json_set(payload,'$.state','cancelled','$.revision',?2,'$.error',?3,'$.next_attempt_ms',0)
             WHERE id=?1",
            params![id, next, ERROR],
        )?;
    }
    Ok(())
}
