//! Local execution evidence, separate from both the pure planner and TS catalogs.
//! This first contract binds one immutable allocation to a ledger. It deliberately
//! has no assignment replacement or grading API until acknowledged event cursors
//! can prevent a server snapshot from counting the same capture twice.

use psf_guard_director_core::{
    Assignment, Decision, Request, State, CONTRACT_VERSION, ENGINE_VERSION, MAX_REQUEST_BYTES,
};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::{fmt, path::Path, time::Duration};

const APPLICATION_ID: i32 = 0x5047444c;
const SCHEMA_VERSION: i32 = 1;
const MAX_EVENT_PAGE: usize = 256;

#[derive(Debug)]
pub enum Error {
    Sqlite(rusqlite::Error),
    Json(serde_json::Error),
    Planner(psf_guard_director_core::Error),
    InvalidInput,
    ForeignDatabase,
    UnsupportedSchema,
    UnsupportedEngine,
    UnsupportedStorage,
    AssignmentMismatch,
    UnknownCapture,
    ConflictingEvidence,
    CorruptLedger,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Do not include stored assignments or image identities in ordinary logs.
        match self {
            Self::Sqlite(_) => f.write_str("Director ledger database operation failed"),
            Self::Json(_) => f.write_str("Director ledger serialization failed"),
            Self::Planner(code) => write!(f, "Director planning failed: {code:?}"),
            other => write!(f, "Director ledger: {other:?}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Sqlite(error) => Some(error),
            Self::Json(error) => Some(error),
            _ => None,
        }
    }
}
impl From<rusqlite::Error> for Error {
    fn from(value: rusqlite::Error) -> Self {
        Self::Sqlite(value)
    }
}
impl From<serde_json::Error> for Error {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum Evidence {
    Reserved,
    /// Verified final image-save receipt, not admission to NINA's save queue.
    Saved {
        image_id: String,
        elapsed_ms: u64,
    },
    /// Positive evidence that this attempt produced no image. Never a timeout.
    Failed {
        reason: String,
    },
    /// Capture/save outcome is unknown; retain the reservation until reconciled.
    Uncertain {
        reason: String,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Attempt {
    pub capture_id: String,
    pub goal_id: String,
    pub reserved_at_ms: u64,
    pub evidence: Evidence,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ExecutionEvent {
    pub schema_version: u32,
    pub ledger_id: String,
    pub sequence: u64,
    pub contract_version: u32,
    pub engine_version: String,
    pub assignment_id: String,
    pub assignment_revision: u64,
    pub rig_id: String,
    pub configuration_id: String,
    pub attempt: Attempt,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Reservation {
    /// Newly committed reservation only. Local safety and ownership must still
    /// be revalidated at the actual dispatch boundary; this is not a permit.
    Created(Attempt),
    /// A retry found evidence. NEVER dispatch again on this response.
    Existing(Attempt),
    /// A previous reservation or ambiguous capture blocks all new rig work.
    RecoveryRequired(Attempt),
    Decision(Decision),
}

pub struct Ledger {
    connection: Connection,
    assignment: Assignment,
    ledger_id: String,
}

fn valid_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= 128 && value.bytes().all(|c| c.is_ascii_graphic())
}

fn status(evidence: &Evidence) -> &'static str {
    match evidence {
        Evidence::Reserved => "reserved",
        Evidence::Saved { .. } => "saved",
        Evidence::Failed { .. } => "failed",
        Evidence::Uncertain { .. } => "uncertain",
    }
}

impl Ledger {
    /// Open only a Director-owned local database. The input allocation must be
    /// authenticated by the host. Validation uses the real shared core, including
    /// its geometry contract; it does not grant permission to dispatch.
    ///
    /// Reopening requires exactly the original allocation, including its baseline
    /// accepted/pending counters. Do not create another ledger on revision changes:
    /// revision activation and cursor reconciliation are not implemented yet.
    pub fn open(path: &Path, assignment: Assignment, state: State) -> Result<Self, Error> {
        // Absolute filesystem paths exclude SQLite's temporary/":memory:" names
        // and URI connection parameters that can silently disable persistence.
        if !path.is_absolute() {
            return Err(Error::InvalidInput);
        }
        let request = Request {
            contract_version: CONTRACT_VERSION,
            assignment,
            state,
        };
        if serde_json::to_vec(&request)?.len() > MAX_REQUEST_BYTES {
            return Err(Error::InvalidInput);
        }
        psf_guard_director_core::evaluate(&request).map_err(Error::Planner)?;
        if request.assignment.rig_id != request.state.rig_id
            || request.assignment.configuration_id != request.state.configuration_id
        {
            return Err(Error::AssignmentMismatch);
        }
        let encoded = serde_json::to_string(&request.assignment)?;
        let mut connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        connection.busy_timeout(Duration::from_secs(2))?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let application: i32 = tx.pragma_query_value(None, "application_id", |r| r.get(0))?;
        let version: i32 = tx.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if application == 0 && version == 0 {
            let objects: i64 = tx.query_row(
                "SELECT count(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
                [],
                |r| r.get(0),
            )?;
            if objects != 0 {
                return Err(Error::ForeignDatabase);
            }
            tx.execute_batch(
                "CREATE TABLE allocation (
                    singleton INTEGER PRIMARY KEY CHECK(singleton=1), payload TEXT NOT NULL,
                    ledger_id TEXT NOT NULL, contract_version INTEGER NOT NULL, engine_version TEXT NOT NULL
                 );
                 CREATE TABLE attempt (
                    capture_id TEXT PRIMARY KEY NOT NULL,
                    goal_id TEXT NOT NULL,
                    status TEXT NOT NULL CHECK(status IN ('reserved','saved','failed','uncertain')),
                    payload TEXT NOT NULL,
                    image_id TEXT UNIQUE
                 );
                 CREATE INDEX attempt_progress ON attempt(goal_id, status);
                 CREATE UNIQUE INDEX one_unresolved_attempt ON attempt((1))
                    WHERE status IN ('reserved','uncertain');
                 CREATE TABLE event (sequence INTEGER PRIMARY KEY AUTOINCREMENT, payload TEXT NOT NULL);"
            )?;
            tx.execute(
                "INSERT INTO allocation VALUES (1, ?1, ?2, ?3, ?4)",
                params![
                    encoded,
                    uuid::Uuid::new_v4().to_string(),
                    CONTRACT_VERSION,
                    ENGINE_VERSION
                ],
            )?;
            tx.pragma_update(None, "application_id", APPLICATION_ID)?;
            tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        } else if application != APPLICATION_ID {
            return Err(Error::ForeignDatabase);
        } else if version != SCHEMA_VERSION {
            return Err(Error::UnsupportedSchema);
        }
        let (stored, ledger_id, contract, engine): (String, String, u32, String) = tx.query_row(
            "SELECT payload,ledger_id,contract_version,engine_version FROM allocation WHERE singleton=1",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?;
        if contract != CONTRACT_VERSION || engine != ENGINE_VERSION {
            return Err(Error::UnsupportedEngine);
        }
        if uuid::Uuid::parse_str(&ledger_id).is_err() {
            return Err(Error::CorruptLedger);
        }
        if stored != encoded {
            return Err(Error::AssignmentMismatch);
        }
        tx.commit()?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        let journal_mode: String =
            connection.pragma_query_value(None, "journal_mode", |r| r.get(0))?;
        if journal_mode != "wal" {
            return Err(Error::UnsupportedStorage);
        }
        Ok(Self {
            connection,
            assignment: request.assignment,
            ledger_id,
        })
    }

    /// Select with shared Rust policy and commit both the attempt and its event
    /// under one SQLite writer transaction. No network/hardware work runs here.
    pub fn reserve(&mut self, capture_id: &str, state: State) -> Result<Reservation, Error> {
        if !valid_id(capture_id) {
            return Err(Error::InvalidInput);
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(attempt) = read_attempt(&tx, capture_id)? {
            return Ok(Reservation::Existing(attempt));
        }
        let unresolved: Option<String> = tx
            .query_row(
                "SELECT capture_id FROM attempt WHERE status IN ('reserved','uncertain') LIMIT 1",
                [],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(id) = unresolved {
            return Ok(Reservation::RecoveryRequired(
                read_attempt(&tx, &id)?.ok_or(Error::CorruptLedger)?,
            ));
        }
        let mut assignment = self.assignment.clone();
        let mut counts = tx.prepare(
            "SELECT goal_id, count(*), sum(status='saved') FROM attempt GROUP BY goal_id",
        )?;
        let rows = counts.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, u32>(1)?,
                r.get::<_, u32>(2)?,
            ))
        })?;
        for row in rows {
            let (goal_id, used, saved) = row?;
            let goal = assignment
                .goals
                .iter_mut()
                .find(|g| g.id == goal_id)
                .ok_or(Error::CorruptLedger)?;
            goal.attempts_remaining = goal
                .attempts_remaining
                .checked_sub(used)
                .ok_or(Error::CorruptLedger)?;
            goal.pending = goal
                .pending
                .checked_add(saved)
                .ok_or(Error::CorruptLedger)?;
        }
        drop(counts);
        let now_ms = state.now_ms;
        let decision = psf_guard_director_core::evaluate(&Request {
            contract_version: CONTRACT_VERSION,
            assignment,
            state,
        })
        .map_err(Error::Planner)?;
        let Decision::Acquire { goal_id, .. } = decision else {
            return Ok(Reservation::Decision(decision));
        };
        let attempt = Attempt {
            capture_id: capture_id.into(),
            goal_id,
            reserved_at_ms: now_ms,
            evidence: Evidence::Reserved,
        };
        tx.execute(
            "INSERT INTO attempt(capture_id,goal_id,status,payload) VALUES (?1,?2,'reserved',?3)",
            params![
                attempt.capture_id,
                attempt.goal_id,
                serde_json::to_string(&attempt)?
            ],
        )?;
        append_event(&tx, &self.ledger_id, &self.assignment, &attempt)?;
        tx.commit()?;
        Ok(Reservation::Created(attempt))
    }

    /// Idempotently record evidence. Repeated identical delivery is a no-op.
    /// Uncertain work can resolve to a verified saved image, but cannot be reset
    /// or refunded. Saved/failed results are immutable in this first contract.
    pub fn record(&mut self, capture_id: &str, evidence: Evidence) -> Result<Attempt, Error> {
        if !valid_id(capture_id) {
            return Err(Error::InvalidInput);
        }
        match &evidence {
            Evidence::Reserved => return Err(Error::InvalidInput),
            Evidence::Saved { image_id, .. } if !valid_id(image_id) => {
                return Err(Error::InvalidInput)
            }
            Evidence::Failed { reason } | Evidence::Uncertain { reason } if !valid_id(reason) => {
                return Err(Error::InvalidInput)
            }
            _ => {}
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut attempt = read_attempt(&tx, capture_id)?.ok_or(Error::UnknownCapture)?;
        if attempt.evidence == evidence {
            return Ok(attempt);
        }
        match (&attempt.evidence, &evidence) {
            (Evidence::Reserved, _) | (Evidence::Uncertain { .. }, Evidence::Saved { .. }) => {}
            _ => return Err(Error::ConflictingEvidence),
        }
        attempt.evidence = evidence;
        let image_id = match &attempt.evidence {
            Evidence::Saved { image_id, .. } => Some(image_id),
            _ => None,
        };
        tx.execute(
            "UPDATE attempt SET status=?1,payload=?2,image_id=?3 WHERE capture_id=?4",
            params![
                status(&attempt.evidence),
                serde_json::to_string(&attempt)?,
                image_id,
                capture_id
            ],
        )?;
        append_event(&tx, &self.ledger_id, &self.assignment, &attempt)?;
        tx.commit()?;
        Ok(attempt)
    }

    pub fn attempt(&self, capture_id: &str) -> Result<Option<Attempt>, Error> {
        if !valid_id(capture_id) {
            return Err(Error::InvalidInput);
        }
        read_attempt(&self.connection, capture_id)
    }

    /// Durable ordered outbox. Cursor acknowledgement/deletion is intentionally
    /// absent until the server inbox contract exists. Pages can be replayed.
    pub fn events_after(&self, cursor: u64, limit: usize) -> Result<Vec<ExecutionEvent>, Error> {
        if limit == 0 || limit > MAX_EVENT_PAGE || cursor > i64::MAX as u64 {
            return Err(Error::InvalidInput);
        }
        let mut query = self.connection.prepare(
            "SELECT sequence,payload FROM event WHERE sequence>?1 ORDER BY sequence LIMIT ?2",
        )?;
        let rows = query.query_map(params![cursor as i64, limit as i64], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?;
        rows.map(|row| {
            let (sequence, payload) = row?;
            let mut event: ExecutionEvent = serde_json::from_str(&payload)?;
            event.sequence = sequence.try_into().map_err(|_| Error::CorruptLedger)?;
            Ok(event)
        })
        .collect()
    }
}

fn read_attempt(connection: &Connection, id: &str) -> Result<Option<Attempt>, Error> {
    let row: Option<(String, String, String)> = connection
        .query_row(
            "SELECT goal_id,status,payload FROM attempt WHERE capture_id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    row.map(|(goal_id, stored_status, payload)| {
        let attempt: Attempt = serde_json::from_str(&payload)?;
        if attempt.capture_id != id
            || attempt.goal_id != goal_id
            || status(&attempt.evidence) != stored_status
        {
            return Err(Error::CorruptLedger);
        }
        Ok(attempt)
    })
    .transpose()
}

fn append_event(
    connection: &Connection,
    ledger_id: &str,
    assignment: &Assignment,
    attempt: &Attempt,
) -> Result<(), Error> {
    let event = ExecutionEvent {
        schema_version: SCHEMA_VERSION as u32,
        ledger_id: ledger_id.into(),
        sequence: 0,
        contract_version: CONTRACT_VERSION,
        engine_version: ENGINE_VERSION.into(),
        assignment_id: assignment.id.clone(),
        assignment_revision: assignment.revision,
        rig_id: assignment.rig_id.clone(),
        configuration_id: assignment.configuration_id.clone(),
        attempt: attempt.clone(),
    };
    connection.execute(
        "INSERT INTO event(payload) VALUES (?1)",
        [serde_json::to_string(&event)?],
    )?;
    Ok(())
}
