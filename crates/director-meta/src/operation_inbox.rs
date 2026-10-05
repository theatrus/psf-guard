//! Preparation receipts have their own cursor and never update live status or
//! capture credit. Delivery is append-only, transactional and replayable.
use super::*;
use crate::inbox::{FeedCursor, Stored};
pub use psf_guard_director_ledger::preparation::Event;
use psf_guard_director_ledger::preparation::EventKind;

pub const MAX_PAGE: usize = 32;

pub(crate) fn create_tables(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS rig_operation_feed(
            ledger_id TEXT PRIMARY KEY NOT NULL, rig_id TEXT NOT NULL REFERENCES rig(id),
            identity TEXT NOT NULL, highest_contiguous INTEGER NOT NULL, last_checkin_ms INTEGER NOT NULL);
         CREATE TABLE IF NOT EXISTS rig_operation_event(
            ledger_id TEXT NOT NULL REFERENCES rig_operation_feed(ledger_id),
            sequence INTEGER NOT NULL, rig_id TEXT NOT NULL REFERENCES rig(id),
            completed INTEGER NOT NULL, payload TEXT NOT NULL, received_at_ms INTEGER NOT NULL,
            PRIMARY KEY(ledger_id,sequence));
         CREATE INDEX IF NOT EXISTS rig_operation_history ON rig_operation_event(rig_id,completed,received_at_ms DESC,sequence DESC);"
    )?;
    Ok(())
}

#[derive(Debug, Serialize, Deserialize)]
pub struct OperationReceipt {
    pub event: Event,
    pub received_at_ms: u64,
}

fn identity(event: &Event) -> Result<String, Error> {
    serde_json::to_string(&(
        &event.rig_id,
        &event.ledger_id,
        event.contract_version,
        &event.engine_version,
        &event.assignment_id,
        event.assignment_revision,
        &event.configuration_id,
    ))
    .map_err(|_| Error::InvalidInput)
}

fn valid_event(event: &Event) -> bool {
    use psf_guard_director_core::{
        preparation::{Command, Operation, Outcome},
        Decision,
    };
    let id = |s: &str| !s.is_empty() && s.len() <= 128 && s.bytes().all(|c| c.is_ascii_graphic());
    let command_valid = |command: &Command| {
        command.preparation_id == event.preparation_id
            && command.ordinal > 0
            && [&command.goal_id, &command.target_id, &command.recipe_id]
                .into_iter()
                .all(|s| id(s))
            && match &command.operation {
                Operation::SwitchFilter { filter_id } => id(filter_id),
                Operation::SetReadoutMode { mode } => *mode >= 0,
                _ => true,
            }
    };
    event.schema_version == 1
        && event.contract_version == psf_guard_director_core::CONTRACT_VERSION
        && id(&event.engine_version)
        && event.sequence > 0
        && event.sequence <= i64::MAX as u64
        && event.assignment_revision > 0
        && [
            &event.assignment_id,
            &event.configuration_id,
            &event.preparation_id,
        ]
        .into_iter()
        .all(|s| id(s))
        && match &event.event {
            EventKind::Started { context, .. } => {
                [
                    &context.goal_id,
                    &context.target_id,
                    &context.recipe_id,
                    &context.filter_id,
                ]
                .into_iter()
                .all(|s| id(s))
                    && context.previous_target_id.as_deref().is_none_or(id)
                    && context.readout_mode >= 0
            }
            EventKind::Issued {
                command,
                issued_at_ms,
            } => command_valid(command) && *issued_at_ms <= 4_102_444_800_000,
            EventKind::Completed { observation } => {
                command_valid(&observation.command)
                    && observation.completion.preparation_id == event.preparation_id
                    && observation.command.ordinal > 0
                    && observation.command.ordinal == observation.completion.ordinal
                    && observation.completion.ended_at_ms >= observation.issued_at_ms
                    && observation.completion.ended_at_ms <= 4_102_444_800_000
                    && observation.completion.elapsed_ms <= 4_102_444_800_000
                    && match &observation.completion.outcome {
                        Outcome::Succeeded => true,
                        Outcome::Failed { reason } | Outcome::Uncertain { reason } => id(reason),
                    }
            }
            EventKind::Halted { decision } => match decision {
                Decision::Acquire { .. } | Decision::Continue { .. } => false,
                Decision::Stop { reason }
                | Decision::Wait { reason }
                | Decision::Complete { reason }
                | Decision::CheckIn { reason } => id(reason),
            },
            EventKind::Captured { capture_id } => id(capture_id),
            EventKind::Closed => true,
        }
}

impl MetaStore {
    pub fn store_operation_receipts(
        &mut self,
        rig: Uuid,
        events: &[Event],
        now: u64,
    ) -> Result<(Vec<Stored>, FeedCursor), Error> {
        valid_id(rig)?;
        if events.is_empty() || events.len() > MAX_PAGE || now > 4_102_444_800_000 {
            return Err(Error::InvalidInput);
        }
        let first = &events[0];
        let ledger = Uuid::parse_str(&first.ledger_id).map_err(|_| Error::InvalidInput)?;
        valid_id(ledger)?;
        if ledger.to_string() != first.ledger_id {
            return Err(Error::InvalidInput);
        }
        let expected = identity(first)?;
        let mut previous = None;
        let mut payloads = Vec::with_capacity(events.len());
        for event in events {
            if !valid_event(event)
                || event.rig_id != rig.to_string()
                || identity(event)? != expected
                || previous.is_some_and(|n| event.sequence != n + 1)
            {
                return Err(Error::InvalidInput);
            }
            previous = Some(event.sequence);
            payloads.push(serde_json::to_string(event).map_err(|_| Error::InvalidInput)?);
        }
        if payloads.iter().map(String::len).sum::<usize>()
            > psf_guard_director_core::MAX_REQUEST_BYTES
        {
            return Err(Error::InvalidInput);
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if read_named(&tx, Kind::Rig, rig)?.is_none() {
            return Err(Error::NotFound);
        }
        let foreign: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM rig_feed WHERE ledger_id=?1 AND rig_id!=?2)
                 OR EXISTS(SELECT 1 FROM rig_event WHERE ledger_id=?1 AND rig_id!=?2)
                 OR EXISTS(SELECT 1 FROM execution_start WHERE ledger_id=?1 AND rig_id!=?2)",
            params![first.ledger_id, rig.to_string()],
            |r| r.get(0),
        )?;
        if foreign {
            return Err(Error::Conflict);
        }
        let stored: Option<(String, i64)> = tx
            .query_row(
                "SELECT identity,highest_contiguous FROM rig_operation_feed WHERE ledger_id=?1",
                [&first.ledger_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if stored.as_ref().is_some_and(|(saved, _)| saved != &expected) {
            return Err(Error::Conflict);
        }
        let mut through = match stored.as_ref() {
            Some((_, n)) => u64::try_from(*n).map_err(|_| Error::CorruptDatabase)?,
            None => 0,
        };
        if first.sequence > through + 1 {
            return Err(Error::Conflict);
        }
        tx.execute(
            "INSERT OR IGNORE INTO rig_operation_feed VALUES(?1,?2,?3,0,?4)",
            params![first.ledger_id, rig.to_string(), expected, now as i64],
        )?;
        let mut outcomes = Vec::with_capacity(events.len());
        for (event, payload) in events.iter().zip(payloads) {
            if event.sequence <= through {
                let old: String = tx.query_row(
                    "SELECT payload FROM rig_operation_event WHERE ledger_id=?1 AND sequence=?2",
                    params![event.ledger_id, event.sequence as i64],
                    |r| r.get(0),
                )?;
                outcomes.push(if old == payload {
                    Stored::Duplicate
                } else {
                    Stored::Conflict
                });
            } else {
                if event.sequence != through + 1 {
                    return Err(Error::Conflict);
                }
                tx.execute(
                    "INSERT INTO rig_operation_event VALUES(?1,?2,?3,?4,?5,?6)",
                    params![
                        event.ledger_id,
                        event.sequence as i64,
                        rig.to_string(),
                        matches!(event.event, EventKind::Completed { .. }),
                        payload,
                        now as i64
                    ],
                )?;
                through = event.sequence;
                outcomes.push(Stored::Applied);
            }
        }
        tx.execute("UPDATE rig_operation_feed SET highest_contiguous=?2,last_checkin_ms=?3 WHERE ledger_id=?1",
            params![first.ledger_id, through as i64, now as i64])?;
        tx.commit()?;
        Ok((
            outcomes,
            FeedCursor {
                rig_id: rig,
                ledger_id: first.ledger_id.clone(),
                highest_contiguous: through,
                highest_seen: through,
                last_checkin_ms: now,
            },
        ))
    }

    pub fn recent_operations(&self, rig: Uuid) -> Result<Vec<OperationReceipt>, Error> {
        valid_id(rig)?;
        let mut query = self.connection.prepare(
            "SELECT payload,received_at_ms FROM rig_operation_event WHERE rig_id=?1 AND completed=1 ORDER BY received_at_ms DESC,sequence DESC,ledger_id LIMIT 20")?;
        let rows = query
            .query_map([rig.to_string()], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(|(payload, time)| {
                let event: Event =
                    serde_json::from_str(&payload).map_err(|_| Error::CorruptDatabase)?;
                if !matches!(event.event, EventKind::Completed { .. }) {
                    return Err(Error::CorruptDatabase);
                }
                Ok(OperationReceipt {
                    event,
                    received_at_ms: u64::try_from(time).map_err(|_| Error::CorruptDatabase)?,
                })
            })
            .collect()
    }
}
