//! Durable native-operation boundaries. Capture and preparation outboxes have
//! independent cursors; their shared ledger ID and capture link preserve identity.

use super::*;
use psf_guard_director_core::preparation::{
    Command, Completion, Context, Estimates, Next, Observation, Outcome, Preparation,
};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Lifecycle {
    Active,
    Closed,
    Captured,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub preparation_id: String,
    pub lifecycle: Lifecycle,
    pub goal_id: String,
    pub pending: Option<Command>,
    pub halted: Option<Decision>,
    pub observations: Vec<Observation>,
    pub capture_id: Option<String>,
}

pub struct Started {
    /// False returns existing evidence, never a new operation to dispatch.
    pub created: bool,
    pub record: Record,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum EventKind {
    Started {
        context: Context,
        estimates: Estimates,
    },
    Issued {
        command: Command,
        issued_at_ms: u64,
    },
    Completed {
        observation: Observation,
    },
    Halted {
        decision: Decision,
    },
    Closed,
    Captured {
        capture_id: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Event {
    pub schema_version: u32,
    pub ledger_id: String,
    pub sequence: u64,
    pub contract_version: u32,
    pub engine_version: String,
    pub assignment_id: String,
    pub assignment_revision: u64,
    pub rig_id: String,
    pub configuration_id: String,
    pub preparation_id: String,
    pub event: EventKind,
}

pub(super) fn create_tables(connection: &Connection) -> Result<(), Error> {
    connection.execute_batch(
        "CREATE TABLE preparation (
            id TEXT PRIMARY KEY NOT NULL,
            status TEXT NOT NULL CHECK(status IN ('active','closed','captured')),
            checkpoint BLOB NOT NULL,
            checkpoint_digest BLOB NOT NULL CHECK(length(checkpoint_digest)=32),
            capture_id TEXT UNIQUE,
            CHECK((status='captured') = (capture_id IS NOT NULL))
         );
         CREATE UNIQUE INDEX one_active_preparation ON preparation((1)) WHERE status='active';
         CREATE TABLE preparation_event (sequence INTEGER PRIMARY KEY AUTOINCREMENT, payload TEXT NOT NULL);"
    )?;
    Ok(())
}

pub(super) fn has_active(connection: &Connection) -> Result<bool, Error> {
    Ok(connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM preparation WHERE status='active')",
        [],
        |r| r.get(0),
    )?)
}

struct Stored {
    preparation: Preparation,
    lifecycle: Lifecycle,
    capture_id: Option<String>,
}

impl Stored {
    fn record(&self) -> Record {
        Record {
            preparation_id: self.preparation.id().into(),
            lifecycle: self.lifecycle.clone(),
            goal_id: self.preparation.context().goal_id.clone(),
            pending: self.preparation.pending().cloned(),
            halted: self.preparation.halted().cloned(),
            observations: self.preparation.observations().to_vec(),
            capture_id: self.capture_id.clone(),
        }
    }
}

fn read(connection: &Connection, id: &str) -> Result<Option<Stored>, Error> {
    if !valid_id(id) {
        return Err(Error::InvalidInput);
    }
    type StoredRow = (String, Vec<u8>, Vec<u8>, Option<String>);
    let row: Option<StoredRow> = connection
        .query_row(
            "SELECT status,checkpoint,checkpoint_digest,capture_id FROM preparation WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .optional()?;
    row.map(|(status, checkpoint, digest, capture_id)| {
        // Integrity check for damaged local storage, not authentication against
        // someone who can rewrite both the checkpoint and its digest.
        if Sha256::digest(&checkpoint).as_slice() != digest {
            return Err(Error::CorruptLedger);
        }
        let preparation = Preparation::restore(&checkpoint).map_err(|_| Error::CorruptLedger)?;
        let lifecycle = match status.as_str() {
            "active" => Lifecycle::Active,
            "closed" => Lifecycle::Closed,
            "captured" => Lifecycle::Captured,
            _ => return Err(Error::CorruptLedger),
        };
        if preparation.id() != id || (lifecycle == Lifecycle::Captured) != capture_id.is_some() {
            return Err(Error::CorruptLedger);
        }
        Ok(Stored {
            preparation,
            lifecycle,
            capture_id,
        })
    })
    .transpose()
}

fn persist(connection: &Connection, preparation: &Preparation) -> Result<(), Error> {
    let checkpoint = preparation.checkpoint().map_err(|_| Error::CorruptLedger)?;
    connection.execute(
        "UPDATE preparation SET checkpoint=?1,checkpoint_digest=?2 WHERE id=?3",
        params![
            checkpoint,
            Sha256::digest(&checkpoint).as_slice(),
            preparation.id()
        ],
    )?;
    Ok(())
}

fn append(
    connection: &Connection,
    ledger_id: &str,
    assignment: &Assignment,
    id: &str,
    event: EventKind,
) -> Result<(), Error> {
    let event = Event {
        schema_version: 1,
        ledger_id: ledger_id.into(),
        sequence: 0,
        contract_version: CONTRACT_VERSION,
        engine_version: ENGINE_VERSION.into(),
        assignment_id: assignment.id.clone(),
        assignment_revision: assignment.revision,
        rig_id: assignment.rig_id.clone(),
        configuration_id: assignment.configuration_id.clone(),
        preparation_id: id.into(),
        event,
    };
    connection.execute(
        "INSERT INTO preparation_event(payload) VALUES (?1)",
        [serde_json::to_string(&event)?],
    )?;
    Ok(())
}

fn snapshot(
    connection: &Connection,
    assignment: &Assignment,
    state: State,
) -> Result<Request, Error> {
    Ok(Request {
        contract_version: CONTRACT_VERSION,
        assignment: current_assignment(connection, assignment)?,
        state,
    })
}

impl Ledger {
    /// Begin one resolved preparation. Does not issue any native operation.
    pub fn begin_preparation(
        &mut self,
        id: &str,
        context: Context,
        estimates: Estimates,
        state: State,
    ) -> Result<Started, Error> {
        if !valid_id(id) {
            return Err(Error::InvalidInput);
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(stored) = read(&tx, id)? {
            if stored.preparation.context() != &context
                || stored.preparation.estimates() != estimates
            {
                return Err(Error::ConflictingEvidence);
            }
            return Ok(Started {
                created: false,
                record: stored.record(),
            });
        }
        let unresolved: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM attempt WHERE status IN ('reserved','uncertain'))",
            [],
            |r| r.get(0),
        )?;
        if unresolved || has_active(&tx)? {
            return Err(Error::ConflictingEvidence);
        }
        let request = snapshot(&tx, &self.assignment, state)?;
        let preparation = Preparation::new(id.into(), &request, context.clone(), estimates)
            .map_err(Error::Preparation)?;
        let checkpoint = preparation.checkpoint().map_err(|_| Error::InvalidInput)?;
        tx.execute(
            "INSERT INTO preparation(id,status,checkpoint,checkpoint_digest) VALUES (?1,'active',?2,?3)",
            params![id, checkpoint, Sha256::digest(&checkpoint).as_slice()],
        )?;
        append(
            &tx,
            &self.ledger_id,
            &self.assignment,
            id,
            EventKind::Started { context, estimates },
        )?;
        let record = Stored {
            preparation,
            lifecycle: Lifecycle::Active,
            capture_id: None,
        }
        .record();
        tx.commit()?;
        Ok(Started {
            created: true,
            record,
        })
    }

    /// Commit an issued operation before returning Run. A lost response or reopen
    /// returns InFlight, never a second dispatch of the same native action.
    pub fn advance_preparation(&mut self, id: &str, state: State) -> Result<Next, Error> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut stored = read(&tx, id)?.ok_or(Error::InvalidInput)?;
        if stored.lifecycle != Lifecycle::Active {
            return Err(Error::ConflictingEvidence);
        }
        let old_halt = stored.preparation.halted().cloned();
        let now = state.now_ms;
        let next = stored
            .preparation
            .next(&snapshot(&tx, &self.assignment, state)?)
            .map_err(Error::Preparation)?;
        persist(&tx, &stored.preparation)?;
        if let Next::Run(command) = &next {
            append(
                &tx,
                &self.ledger_id,
                &self.assignment,
                id,
                EventKind::Issued {
                    command: command.clone(),
                    issued_at_ms: now,
                },
            )?;
        }
        if old_halt.as_ref() != stored.preparation.halted()
            && let Some(decision) = stored.preparation.halted()
        {
            append(
                &tx,
                &self.ledger_id,
                &self.assignment,
                id,
                EventKind::Halted {
                    decision: decision.clone(),
                },
            )?;
        }
        tx.commit()?;
        Ok(next)
    }

    pub fn complete_preparation(&mut self, completion: Completion) -> Result<Record, Error> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let id = completion.preparation_id.clone();
        let mut stored = read(&tx, &id)?.ok_or(Error::InvalidInput)?;
        let before = stored.preparation.observations().len();
        stored
            .preparation
            .complete(completion)
            .map_err(Error::Preparation)?;
        if stored.preparation.observations().len() != before {
            if stored.lifecycle != Lifecycle::Active {
                return Err(Error::ConflictingEvidence);
            }
            persist(&tx, &stored.preparation)?;
            let observation = stored
                .preparation
                .observations()
                .last()
                .ok_or(Error::CorruptLedger)?
                .clone();
            append(
                &tx,
                &self.ledger_id,
                &self.assignment,
                &id,
                EventKind::Completed { observation },
            )?;
        }
        let record = stored.record();
        tx.commit()?;
        Ok(record)
    }

    pub fn preparation(&self, id: &str) -> Result<Option<Record>, Error> {
        Ok(read(&self.connection, id)?.map(|stored| stored.record()))
    }

    /// Explicitly end unused preparation; never discard an unknown operation.
    /// A pending or uncertain action requires positive recovery evidence first.
    pub fn close_preparation(&mut self, id: &str) -> Result<Record, Error> {
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut stored = read(&tx, id)?.ok_or(Error::InvalidInput)?;
        if stored.lifecycle != Lifecycle::Active {
            return Ok(stored.record());
        }
        if stored.preparation.pending().is_some()
            || stored
                .preparation
                .observations()
                .iter()
                .any(|o| matches!(o.completion.outcome, Outcome::Uncertain { .. }))
        {
            return Err(Error::ConflictingEvidence);
        }
        tx.execute("UPDATE preparation SET status='closed' WHERE id=?1", [id])?;
        append(
            &tx,
            &self.ledger_id,
            &self.assignment,
            id,
            EventKind::Closed,
        )?;
        stored.lifecycle = Lifecycle::Closed;
        tx.commit()?;
        Ok(stored.record())
    }

    /// Freshly check readiness and atomically link the resulting reservation.
    /// Existing reservations are evidence only and must never be redispatched.
    pub fn reserve_prepared(
        &mut self,
        id: &str,
        capture_id: &str,
        state: State,
    ) -> Result<Reservation, Error> {
        if !valid_id(capture_id) {
            return Err(Error::InvalidInput);
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut stored = read(&tx, id)?.ok_or(Error::InvalidInput)?;
        if stored.lifecycle == Lifecycle::Captured {
            if stored.capture_id.as_deref() != Some(capture_id) {
                return Err(Error::ConflictingEvidence);
            }
            return Ok(Reservation::Existing(
                read_attempt(&tx, capture_id)?.ok_or(Error::CorruptLedger)?,
            ));
        }
        if stored.lifecycle != Lifecycle::Active || !stored.preparation.steps_completed() {
            return Err(Error::ConflictingEvidence);
        }
        if read_attempt(&tx, capture_id)?.is_some() {
            return Err(Error::ConflictingEvidence);
        }
        let now = state.now_ms;
        let next = stored
            .preparation
            .next(&snapshot(&tx, &self.assignment, state)?)
            .map_err(Error::Preparation)?;
        persist(&tx, &stored.preparation)?;
        let Next::ReadyToReserve { goal_id } = next else {
            let Next::Decision(decision) = next else {
                return Err(Error::CorruptLedger);
            };
            if stored.preparation.halted().is_some() {
                append(
                    &tx,
                    &self.ledger_id,
                    &self.assignment,
                    id,
                    EventKind::Halted {
                        decision: decision.clone(),
                    },
                )?;
            }
            tx.commit()?;
            return Ok(Reservation::Decision(decision));
        };
        let attempt = insert_attempt(
            &tx,
            &self.ledger_id,
            &self.assignment,
            capture_id,
            goal_id,
            now,
        )?;
        tx.execute(
            "UPDATE preparation SET status='captured',capture_id=?1 WHERE id=?2",
            params![capture_id, id],
        )?;
        append(
            &tx,
            &self.ledger_id,
            &self.assignment,
            id,
            EventKind::Captured {
                capture_id: capture_id.into(),
            },
        )?;
        tx.commit()?;
        Ok(Reservation::Created(attempt))
    }

    /// Separate, replayable preparation outbox. Do not mix its cursor with the
    /// existing capture-event cursor. Neither feed is acknowledged/deleted yet.
    pub fn preparation_events_after(&self, cursor: u64, limit: usize) -> Result<Vec<Event>, Error> {
        if limit == 0 || limit > MAX_EVENT_PAGE || cursor > i64::MAX as u64 {
            return Err(Error::InvalidInput);
        }
        let mut query = self.connection.prepare("SELECT sequence,payload FROM preparation_event WHERE sequence>?1 ORDER BY sequence LIMIT ?2")?;
        let rows = query.query_map(params![cursor as i64, limit as i64], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?;
        rows.map(|row| {
            let (sequence, payload) = row?;
            let mut event: Event = serde_json::from_str(&payload)?;
            event.sequence = sequence.try_into().map_err(|_| Error::CorruptLedger)?;
            Ok(event)
        })
        .collect()
    }
}
