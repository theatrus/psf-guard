//! Immutable first allocation. Expiry/revocation never refunds work or permits
//! replacement; successor accounting is a separate, not yet exposed operation.
use super::*;
use psf_guard_director_core::{
    program::{BoundProgram, Program},
    Safety, State,
};

// Leave room for the HTTP API wrapper inside the client's 1 MiB response limit.
pub(crate) const MAX_ALLOCATION_BYTES: usize = 1_048_000;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Allocation {
    pub schema_version: u32,
    pub allocation_id: Uuid,
    pub coordinator_instance_id: Uuid,
    pub catalog_id: Uuid,
    pub rig_id: Uuid,
    pub client_id: Uuid,
    pub profile_id: Uuid,
    pub preview_revision: String,
    pub admitted_at_ms: u64,
    /// Complete issued envelope, including links and rig context. Never rebuilt
    /// from mutable catalog rows when a client reconnects.
    pub snapshot: serde_json::Value,
}

pub(crate) fn create_table(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch(
        "CREATE TABLE execution_allocation(
        rig_id TEXT PRIMARY KEY NOT NULL REFERENCES rig(id),
        allocation_id TEXT UNIQUE NOT NULL, payload TEXT NOT NULL);",
    )?;
    Ok(())
}

impl Allocation {
    pub(crate) fn validate(&self, instance: Uuid) -> Result<(), Error> {
        for id in [
            self.allocation_id,
            self.catalog_id,
            self.rig_id,
            self.client_id,
            self.profile_id,
        ] {
            valid_id(id)?;
        }
        if self.schema_version != 1
            || self.coordinator_instance_id != instance
            || self.preview_revision.len() != 64
            || !self
                .preview_revision
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            || self.snapshot["coordinator_instance_id"] != instance.to_string()
            || self.snapshot["catalog_id"] != self.catalog_id.to_string()
            || self.snapshot["rig_id"] != self.rig_id.to_string()
        {
            return Err(Error::InvalidInput);
        }
        if serde_json::to_vec(&self.snapshot["program"])
            .map_err(|_| Error::InvalidInput)?
            .len()
            > psf_guard_director_core::MAX_REQUEST_BYTES
        {
            return Err(Error::InvalidInput);
        }
        let program: Program = serde_json::from_value(self.snapshot["program"].clone())
            .map_err(|_| Error::InvalidInput)?;
        let a = &program.assignment;
        if a.id != format!("allocation-{}", self.allocation_id)
            || a.rig_id != self.rig_id.to_string()
            || self.admitted_at_ms < a.valid_from_ms
            || self.admitted_at_ms >= a.expires_at_ms
            || a.expires_at_ms - a.valid_from_ms > 86_400_000
        {
            return Err(Error::InvalidInput);
        }
        let state = State {
            rig_id: a.rig_id.clone(),
            configuration_id: a.configuration_id.clone(),
            now_ms: self.admitted_at_ms,
            conditions_valid_until_ms: a.expires_at_ms,
            safety: Safety::Unknown,
            at_boundary: true,
            operator_stop: false,
            meridian_exclusion: psf_guard_director_core::windows::MeridianExclusion {
                before_ms: 0,
                after_ms: 0,
            },
        };
        BoundProgram::new(program, &state).map_err(|_| Error::InvalidInput)?;
        Ok(())
    }
}

pub(crate) fn read(
    conn: &Connection,
    rig: Uuid,
    instance: Uuid,
) -> Result<Option<Allocation>, Error> {
    let row: Option<(String, String)> = conn
        .query_row(
            "SELECT allocation_id,payload FROM execution_allocation WHERE rig_id=?1",
            [rig.to_string()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    row.map(|(id, payload)| {
        if payload.len() > MAX_ALLOCATION_BYTES {
            return Err(Error::CorruptDatabase);
        }
        let value: Allocation =
            serde_json::from_str(&payload).map_err(|_| Error::CorruptDatabase)?;
        value
            .validate(instance)
            .map_err(|_| Error::CorruptDatabase)?;
        if value.rig_id != rig || value.allocation_id.to_string() != id {
            return Err(Error::CorruptDatabase);
        }
        Ok(value)
    })
    .transpose()
}

impl MetaStore {
    /// One-shot launch: even an identical retry is refused because its original
    /// caller may already be operating offline. No timeout refunds this claim.
    pub fn start_allocation(
        &mut self,
        rig: Uuid,
        allocation: Uuid,
        client: Uuid,
        ledger: Uuid,
        now: u64,
    ) -> Result<(), Error> {
        valid_id(ledger)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let grant = read(&tx, rig, self.instance_id)?.ok_or(Error::Conflict)?;
        let live: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM director_client WHERE id=?1 AND rig_id=?2)",
            params![client.to_string(), rig.to_string()],
            |r| r.get(0),
        )?;
        let expires = grant.snapshot["program"]["assignment"]["expires_at_ms"]
            .as_u64()
            .ok_or(Error::CorruptDatabase)?;
        if !live
            || grant.allocation_id != allocation
            || grant.client_id != client
            || now < grant.admitted_at_ms
            || now >= expires
        {
            return Err(Error::Conflict);
        }
        let used_ledger: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM workload_history WHERE json_extract(payload,'$.ledger_id')=?1)", [ledger.to_string()], |r| r.get(0))?;
        let released: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM workload_history WHERE allocation_id=?1 AND json_extract(payload,'$.released')=1)", [allocation.to_string()], |r| r.get(0))?;
        if used_ledger || released {
            return Err(Error::Conflict);
        }
        let changed = tx.execute("INSERT OR IGNORE INTO execution_start(rig_id,allocation_id,ledger_id,started_at_ms) VALUES(?1,?2,?3,?4)",
            params![rig.to_string(), allocation.to_string(), ledger.to_string(), i64::try_from(now).map_err(|_| Error::InvalidInput)?])?;
        if changed != 1 {
            return Err(Error::Conflict);
        }
        tx.commit()?;
        Ok(())
    }

    pub fn allocation(&self, rig: Uuid) -> Result<Option<Allocation>, Error> {
        valid_id(rig)?;
        read(&self.connection, rig, self.instance_id)
    }

    /// The operator selects one live paired client. There is intentionally no
    /// delete/replace API: a revoked or expired allocation retains its budget.
    pub fn admit_allocation(&mut self, allocation: &Allocation) -> Result<Allocation, Error> {
        allocation.validate(self.instance_id)?;
        let payload = serde_json::to_string(allocation).map_err(|_| Error::InvalidInput)?;
        if payload.len() > MAX_ALLOCATION_BYTES {
            return Err(Error::InvalidInput);
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let permitted: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM director_client c
            JOIN catalog_rig b ON b.rig_id=c.rig_id AND b.catalog_id=c.catalog_id
            WHERE c.id=?1 AND c.rig_id=?2 AND c.catalog_id=?3 AND c.profile_id=?4)",
            params![
                allocation.client_id.to_string(),
                allocation.rig_id.to_string(),
                allocation.catalog_id.to_string(),
                allocation.profile_id.to_string()
            ],
            |r| r.get(0),
        )?;
        if !permitted {
            return Err(Error::Conflict);
        }
        if let Some(old) = read(&tx, allocation.rig_id, self.instance_id)? {
            if old != *allocation {
                return Err(Error::Conflict);
            }
            return Ok(old);
        }
        let reused_id: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM execution_allocation WHERE allocation_id=?1)",
            [allocation.allocation_id.to_string()],
            |r| r.get(0),
        )?;
        if reused_id {
            return Err(Error::Conflict);
        }
        // Legacy/test receipts cannot be assumed settled just because they did
        // not come from this admission protocol.
        let prior_work: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM rig_feed WHERE rig_id=?1)",
            [allocation.rig_id.to_string()],
            |r| r.get(0),
        )?;
        if prior_work {
            return Err(Error::Conflict);
        }
        tx.execute(
            "INSERT INTO execution_allocation VALUES(?1,?2,?3)",
            params![
                allocation.rig_id.to_string(),
                allocation.allocation_id.to_string(),
                payload
            ],
        )?;
        tx.commit()?;
        Ok(allocation.clone())
    }
}
