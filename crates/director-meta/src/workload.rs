//! Reviewed authority for automatic intake. Started work never expires into a
//! fresh budget: only an executor's quiescent, fully delivered terminal ledger
//! can release it. Immutable history survives client revocation.
use super::*;
use allocation::Allocation;
use psf_guard_director_core::program::Program;
use psf_guard_director_ledger::{Evidence, ExecutionEvent};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub rig_id: Uuid,
    pub catalog_id: Uuid,
    pub client_id: Uuid,
    pub profile_id: Uuid,
    pub profile_revision: u64,
    pub configuration_id: String,
    pub project_ids: Vec<Uuid>,
    pub enabled: bool,
    pub revision: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Workload {
    pub allocation: Allocation,
    pub released: bool,
    pub ledger_id: Option<Uuid>,
    pub terminal_sequence: Option<u64>,
}

pub(crate) fn create_tables(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch("CREATE TABLE workload_policy(rig_id TEXT PRIMARY KEY NOT NULL REFERENCES rig(id), payload TEXT NOT NULL);
        CREATE TABLE workload_history(allocation_id TEXT PRIMARY KEY NOT NULL, rig_id TEXT NOT NULL REFERENCES rig(id), payload TEXT NOT NULL);
        CREATE TABLE workload_budget(rig_id TEXT NOT NULL REFERENCES rig(id),goal_id TEXT NOT NULL,initial_limit INTEGER NOT NULL CHECK(initial_limit>=0),spent INTEGER NOT NULL CHECK(spent>=0 AND spent<=initial_limit),PRIMARY KEY(rig_id,goal_id));
        CREATE INDEX workload_history_rig ON workload_history(rig_id);
        CREATE UNIQUE INDEX workload_history_ledger ON workload_history(json_extract(payload,'$.ledger_id'));
        CREATE INDEX IF NOT EXISTS rig_event_capture ON rig_event(rig_id,capture_id,ledger_id);")?;
    Ok(())
}

fn policy(conn: &Connection, rig: Uuid) -> Result<Option<Policy>, Error> {
    let text: Option<String> = conn
        .query_row(
            "SELECT payload FROM workload_policy WHERE rig_id=?1",
            [rig.to_string()],
            |r| r.get(0),
        )
        .optional()?;
    text.map(|v| serde_json::from_str(&v).map_err(|_| Error::CorruptDatabase))
        .transpose()
}

fn history(conn: &Connection, id: Uuid) -> Result<Option<Workload>, Error> {
    let text: Option<String> = conn
        .query_row(
            "SELECT payload FROM workload_history WHERE allocation_id=?1",
            [id.to_string()],
            |r| r.get(0),
        )
        .optional()?;
    text.map(|v| serde_json::from_str(&v).map_err(|_| Error::CorruptDatabase))
        .transpose()
}

fn live(conn: &Connection, p: &Policy) -> Result<bool, Error> {
    Ok(conn.query_row("SELECT EXISTS(SELECT 1 FROM director_client c JOIN catalog_rig b ON b.rig_id=c.rig_id AND b.catalog_id=c.catalog_id WHERE c.id=?1 AND c.rig_id=?2 AND c.catalog_id=?3 AND c.profile_id=?4)",
        params![p.client_id.to_string(), p.rig_id.to_string(), p.catalog_id.to_string(), p.profile_id.to_string()], |r| r.get(0))?)
}

fn profile_matches(conn: &Connection, p: &Policy) -> Result<bool, Error> {
    Ok(profile::read_profile(conn, p.rig_id)?.is_some_and(|r| {
        r.revision == p.profile_revision
            && r.configuration
                .is_some_and(|c| c.value.id == p.configuration_id)
    }))
}

fn carry_budget(conn: &Connection, rig: Uuid, program: &mut Program) -> Result<(), Error> {
    let mut stmt =
        conn.prepare("SELECT goal_id,initial_limit,spent FROM workload_budget WHERE rig_id=?1")?;
    let budgets = stmt
        .query_map([rig.to_string()], |r| {
            Ok((
                r.get::<_, String>(0)?,
                (r.get::<_, u32>(1)?, r.get::<_, u32>(2)?),
            ))
        })?
        .collect::<Result<BTreeMap<_, _>, _>>()?;
    for g in &mut program.assignment.goals {
        if let Some((cap, spent)) = budgets.get(&g.id) {
            g.carry_attempt_budget(*cap, *spent);
        }
    }
    Ok(())
}

impl MetaStore {
    pub fn workload_policy(&self, rig: Uuid) -> Result<Option<Policy>, Error> {
        valid_id(rig)?;
        policy(&self.connection, rig)
    }

    /// Compare-and-set commissioning is interactive; paired clients cannot
    /// broaden project scope or silently approve new equipment.
    pub fn save_workload_policy(&mut self, value: &Policy, expected: u64) -> Result<Policy, Error> {
        for id in [
            value.rig_id,
            value.catalog_id,
            value.client_id,
            value.profile_id,
        ] {
            valid_id(id)?;
        }
        if value.revision != expected.checked_add(1).ok_or(Error::InvalidInput)?
            || value.profile_revision == 0
            || value.configuration_id.is_empty()
            || value.configuration_id.len() > 128
            || value.project_ids.is_empty()
            || value.project_ids.len() > 128
            || value.project_ids.iter().any(Uuid::is_nil)
            || value.project_ids.iter().collect::<BTreeSet<_>>().len() != value.project_ids.len()
        {
            return Err(Error::InvalidInput);
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if policy(&tx, value.rig_id)?.map_or(0, |p| p.revision) != expected
            || !live(&tx, value)?
            || (value.enabled && !profile_matches(&tx, value)?)
        {
            return Err(Error::Conflict);
        }
        for id in &value.project_ids {
            let exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM activation WHERE project_id=?1)",
                [id.to_string()],
                |r| r.get(0),
            )?;
            if !exists {
                return Err(Error::Conflict);
            }
        }
        tx.execute("INSERT INTO workload_policy VALUES(?1,?2) ON CONFLICT(rig_id) DO UPDATE SET payload=excluded.payload", params![value.rig_id.to_string(), serde_json::to_string(value).map_err(|_| Error::InvalidInput)?])?;
        tx.commit()?;
        Ok(value.clone())
    }

    pub fn workload(&self, rig: Uuid, client: Uuid, id: Uuid) -> Result<Option<Workload>, Error> {
        valid_id(id)?;
        let result = history(&self.connection, id)?;
        if result
            .as_ref()
            .is_some_and(|w| w.allocation.rig_id != rig || w.allocation.client_id != client)
        {
            return Err(Error::Conflict);
        }
        Ok(result)
    }

    /// Fresh compiler counters are capped by the first grant's budget. Failed
    /// attempts also consume authority; pending credit is not accepted credit.
    pub fn carry_workload_budget(&self, rig: Uuid, program: &mut Program) -> Result<(), Error> {
        carry_budget(&self.connection, rig, program)
    }

    pub fn admit_workload(
        &mut self,
        grant: &Allocation,
        policy_revision: u64,
    ) -> Result<Workload, Error> {
        grant.validate(self.instance_id)?;
        if serde_json::to_vec(grant)
            .map_err(|_| Error::InvalidInput)?
            .len()
            > allocation::MAX_ALLOCATION_BYTES
        {
            return Err(Error::InvalidInput);
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let p = policy(&tx, grant.rig_id)?.ok_or(Error::Conflict)?;
        if !live(&tx, &p)?
            || p.client_id != grant.client_id
            || p.catalog_id != grant.catalog_id
            || p.profile_id != grant.profile_id
        {
            return Err(Error::Conflict);
        }
        if let Some(old) = history(&tx, grant.allocation_id)? {
            if old.allocation != *grant {
                return Err(Error::Conflict);
            }
            return Ok(old);
        }
        if !p.enabled || p.revision != policy_revision || !profile_matches(&tx, &p)? {
            return Err(Error::Conflict);
        }
        let mut bounded: Program = serde_json::from_value(grant.snapshot["program"].clone())
            .map_err(|_| Error::InvalidInput)?;
        let original = bounded.clone();
        carry_budget(&tx, grant.rig_id, &mut bounded)?;
        if original != bounded {
            return Err(Error::Conflict);
        }
        let links = grant.snapshot["links"]
            .as_array()
            .ok_or(Error::InvalidInput)?;
        if links.is_empty()
            || links.iter().any(|l| {
                serde_json::from_value::<Uuid>(l["project_id"].clone())
                    .map_or(true, |id| !p.project_ids.contains(&id))
            })
            || grant.snapshot["program"]["configuration"]["id"] != p.configuration_id
            || grant.snapshot["rig"]["profile_revision"] != p.profile_revision
        {
            return Err(Error::Conflict);
        }
        if let Some(old) = allocation::read(&tx, grant.rig_id, self.instance_id)? {
            if !history(&tx, old.allocation_id)?.is_some_and(|w| w.released) {
                return Err(Error::Conflict);
            }
        } else {
            let prior: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM rig_feed WHERE rig_id=?1)",
                [grant.rig_id.to_string()],
                |r| r.get(0),
            )?;
            if prior {
                return Err(Error::Conflict);
            }
        }
        let count: i64 = tx.query_row(
            "SELECT COUNT(*) FROM workload_history WHERE rig_id=?1",
            [grant.rig_id.to_string()],
            |r| r.get(0),
        )?;
        if count >= 4096 {
            return Err(Error::Conflict);
        }
        let result = Workload {
            allocation: grant.clone(),
            released: false,
            ledger_id: None,
            terminal_sequence: None,
        };
        for g in &original.assignment.goals {
            tx.execute("INSERT INTO workload_budget VALUES(?1,?2,?3,0) ON CONFLICT(rig_id,goal_id) DO NOTHING", params![grant.rig_id.to_string(),g.id,g.attempts_remaining])?;
        }
        tx.execute(
            "DELETE FROM execution_start WHERE rig_id=?1",
            [grant.rig_id.to_string()],
        )?;
        tx.execute("INSERT INTO execution_allocation VALUES(?1,?2,?3) ON CONFLICT(rig_id) DO UPDATE SET allocation_id=excluded.allocation_id,payload=excluded.payload", params![grant.rig_id.to_string(), grant.allocation_id.to_string(), serde_json::to_string(grant).map_err(|_| Error::InvalidInput)?])?;
        tx.execute(
            "INSERT INTO workload_history VALUES(?1,?2,?3)",
            params![
                grant.allocation_id.to_string(),
                grant.rig_id.to_string(),
                serde_json::to_string(&result).map_err(|_| Error::InvalidInput)?
            ],
        )?;
        tx.commit()?;
        Ok(result)
    }

    /// The authenticated executor attests it has stopped all operations and
    /// parked. We additionally require a complete, settled capture feed. An
    /// uncertain preparation must never call this API; it needs reconciliation.
    pub fn release_workload(
        &mut self,
        rig: Uuid,
        client: Uuid,
        id: Uuid,
        ledger: Uuid,
        through: u64,
    ) -> Result<Workload, Error> {
        valid_id(ledger)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut w = history(&tx, id)?.ok_or(Error::Conflict)?;
        if w.allocation.rig_id != rig || w.allocation.client_id != client {
            return Err(Error::Conflict);
        }
        if w.released {
            if w.ledger_id != Some(ledger) || w.terminal_sequence != Some(through) {
                return Err(Error::Conflict);
            }
            return Ok(w);
        }
        let started: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM execution_start WHERE rig_id=?1 AND allocation_id=?2 AND ledger_id=?3)", params![rig.to_string(), id.to_string(), ledger.to_string()], |r| r.get(0))?;
        if !started {
            return Err(Error::Conflict);
        }
        let cursor: Option<(i64,i64)> = tx.query_row("SELECT highest_contiguous,highest_seen FROM rig_feed WHERE rig_id=?1 AND ledger_id=?2", params![rig.to_string(), ledger.to_string()], |r| Ok((r.get(0)?,r.get(1)?))).optional()?;
        if through > 200_000 || cursor.unwrap_or((0, 0)) != (through as i64, through as i64) {
            return Err(Error::Conflict);
        }
        let program: Program = serde_json::from_value(w.allocation.snapshot["program"].clone())
            .map_err(|_| Error::CorruptDatabase)?;
        let reused: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM rig_event a JOIN rig_event b ON a.capture_id=b.capture_id AND a.rig_id=b.rig_id WHERE a.ledger_id=?1 AND b.ledger_id!=a.ledger_id)", [ledger.to_string()], |r| r.get(0))?;
        if reused {
            return Err(Error::Conflict);
        }
        let mut attempts = BTreeMap::<String, ExecutionEvent>::new();
        let mut stmt =
            tx.prepare("SELECT payload,goal_id,capture_id,state FROM rig_event WHERE ledger_id=?1 ORDER BY sequence")?;
        let mut sequence = 0;
        for row in stmt.query_map([ledger.to_string()], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
            ))
        })? {
            let (payload, goal, capture, state) = row?;
            let e: ExecutionEvent = serde_json::from_str(&payload).map_err(|_| Error::Conflict)?;
            let evidence_state = match &e.attempt.evidence {
                Evidence::Reserved => "reserved",
                Evidence::Saved { image_id, .. } => {
                    if image_id.is_empty()
                        || image_id.len() > 128
                        || !image_id.bytes().all(|b| b.is_ascii_graphic())
                    {
                        return Err(Error::Conflict);
                    }
                    "saved"
                }
                Evidence::Failed { reason } => {
                    if reason.is_empty()
                        || reason.len() > 128
                        || !reason.bytes().all(|b| b.is_ascii_graphic())
                    {
                        return Err(Error::Conflict);
                    }
                    "failed"
                }
                Evidence::Uncertain { .. } => "uncertain",
            };
            if goal != e.attempt.goal_id
                || capture != e.attempt.capture_id
                || state != evidence_state
            {
                return Err(Error::Conflict);
            }
            sequence += 1;
            if e.sequence != sequence
                || e.schema_version != 1
                || e.ledger_id != ledger.to_string()
                || e.rig_id != rig.to_string()
                || e.assignment_id != program.assignment.id
                || e.assignment_revision != program.assignment.revision
                || e.configuration_id != program.configuration.id
                || e.contract_version != psf_guard_director_core::CONTRACT_VERSION
                || e.engine_version != psf_guard_director_core::ENGINE_VERSION
                || e.attempt.reserved_at_ms < program.assignment.valid_from_ms
                || e.attempt.reserved_at_ms >= program.assignment.expires_at_ms
                || !program
                    .assignment
                    .goals
                    .iter()
                    .any(|g| g.id == e.attempt.goal_id)
            {
                return Err(Error::Conflict);
            }
            match attempts.get(&e.attempt.capture_id) {
                None if matches!(e.attempt.evidence, Evidence::Reserved) => {}
                Some(old)
                    if matches!(old.attempt.evidence, Evidence::Reserved)
                        && !matches!(e.attempt.evidence, Evidence::Reserved)
                        && old.attempt.goal_id == e.attempt.goal_id
                        && old.attempt.reserved_at_ms == e.attempt.reserved_at_ms => {}
                _ => return Err(Error::Conflict),
            }
            attempts.insert(e.attempt.capture_id.clone(), e);
        }
        drop(stmt);
        if sequence != through
            || attempts.values().any(|e| {
                !matches!(
                    e.attempt.evidence,
                    Evidence::Saved { .. } | Evidence::Failed { .. }
                )
            })
        {
            return Err(Error::Conflict);
        }
        for g in &program.assignment.goals {
            let spent = attempts
                .values()
                .filter(|e| e.attempt.goal_id == g.id)
                .count();
            if spent > g.attempts_remaining as usize {
                return Err(Error::Conflict);
            }
            if tx.execute(
                "UPDATE workload_budget SET spent=spent+?1 WHERE rig_id=?2 AND goal_id=?3",
                params![spent as i64, rig.to_string(), g.id],
            )? != 1
            {
                return Err(Error::CorruptDatabase);
            }
        }
        w.released = true;
        w.ledger_id = Some(ledger);
        w.terminal_sequence = Some(through);
        tx.execute(
            "UPDATE workload_history SET payload=?1 WHERE allocation_id=?2",
            params![
                serde_json::to_string(&w).map_err(|_| Error::InvalidInput)?,
                id.to_string()
            ],
        )?;
        tx.commit()?;
        Ok(w)
    }
}
