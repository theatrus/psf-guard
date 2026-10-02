//! Durable per-rig recovery state. This store lives outside per-allocation run
//! directories. It does not authorize capture or replace the capture ledger.

use psf_guard_director_core::recovery::{
    Conditions, Event, Identity, Phase, Policy, RecoveryResult, Snapshot,
};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{path::Path, time::Duration};

const APPLICATION_ID: i32 = 0x50475253;
const SCHEMA: u32 = 1;
const MAX_BYTES: usize = 131_072;
const MAX_EVENTS: u64 = 100_000;

#[derive(Debug)]
pub enum Error {
    Storage(rusqlite::Error),
    Encoding(serde_json::Error),
    Policy(psf_guard_director_core::recovery::Error),
    InvalidInput,
    ForeignDatabase,
    UnsupportedSchema,
    WrongScope,
    Conflict,
    Corrupt,
    LimitReached,
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Storage(_) => "Director recovery storage failed",
            Self::Encoding(_) => "Director recovery encoding failed",
            Self::Policy(_) => "Director recovery transition refused",
            Self::InvalidInput => "Invalid Director recovery input",
            Self::ForeignDatabase => "Not a Director recovery database",
            Self::UnsupportedSchema => "Unsupported Director recovery schema",
            Self::WrongScope => "Director recovery scope changed",
            Self::Conflict => "Director recovery state changed or request conflicts",
            Self::Corrupt => "Director recovery state is corrupt",
            Self::LimitReached => "Director recovery journal limit reached",
        })
    }
}
impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Storage(error) => Some(error),
            Self::Encoding(error) => Some(error),
            Self::Policy(error) => Some(error),
            _ => None,
        }
    }
}
impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Self::Storage(e)
    }
}
impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Self::Encoding(e)
    }
}
impl From<psf_guard_director_core::recovery::Error> for Error {
    fn from(e: psf_guard_director_core::recovery::Error) -> Self {
        Self::Policy(e)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub revision: u64,
    pub snapshot: Snapshot,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub night_id: String,
    pub configuration_id: String,
    pub event_id: String,
    pub expected_revision: u64,
    pub now_ms: u64,
    pub conditions: Conditions,
    pub event: Event,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Applied {
    /// False on replay. A replay or readback can never dispatch an operation.
    pub newly_applied: bool,
    pub record: Record,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct JournalEvent {
    pub revision: u64,
    pub request: Request,
}

pub struct SessionStore {
    db: Connection,
    rig_id: String,
}

fn valid_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && s.bytes().all(|b| b.is_ascii_graphic())
}
fn encode<T: Serialize>(value: &T) -> Result<String, Error> {
    let payload = serde_json::to_string(value)?;
    if payload.len() > MAX_BYTES {
        return Err(Error::LimitReached);
    }
    Ok(payload)
}

impl SessionStore {
    pub fn open(path: &Path, rig_id: &str) -> Result<Self, Error> {
        if !path.is_absolute() || !valid_id(rig_id) {
            return Err(Error::InvalidInput);
        }
        let mut db = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        db.busy_timeout(Duration::from_secs(2))?;
        db.pragma_update(None, "synchronous", "FULL")?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let application: i32 = tx.pragma_query_value(None, "application_id", |r| r.get(0))?;
        let version: u32 = tx.pragma_query_value(None, "user_version", |r| r.get(0))?;
        if application == 0 && version == 0 {
            let count: i64 = tx.query_row(
                "SELECT count(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
                [],
                |r| r.get(0),
            )?;
            if count != 0 {
                return Err(Error::ForeignDatabase);
            }
            tx.execute_batch("CREATE TABLE owner(singleton INTEGER PRIMARY KEY CHECK(singleton=1),rig_id TEXT NOT NULL,current_night TEXT);
                CREATE TABLE night(night_id TEXT PRIMARY KEY NOT NULL,revision INTEGER NOT NULL,payload TEXT NOT NULL,digest BLOB NOT NULL);
                CREATE TABLE event(night_id TEXT NOT NULL,event_id TEXT NOT NULL,revision INTEGER NOT NULL,request TEXT NOT NULL,
                    PRIMARY KEY(night_id,event_id),UNIQUE(night_id,revision));
                CREATE TABLE evidence(night_id TEXT NOT NULL,kind TEXT NOT NULL,identity TEXT NOT NULL,PRIMARY KEY(night_id,kind,identity));")?;
            tx.execute("INSERT INTO owner VALUES(1,?1,NULL)", [rig_id])?;
            tx.pragma_update(None, "application_id", APPLICATION_ID)?;
            tx.pragma_update(None, "user_version", SCHEMA)?;
        } else if application != APPLICATION_ID {
            return Err(Error::ForeignDatabase);
        } else if version != SCHEMA {
            return Err(Error::UnsupportedSchema);
        }
        let stored: String =
            tx.query_row("SELECT rig_id FROM owner WHERE singleton=1", [], |r| {
                r.get(0)
            })?;
        if stored != rig_id {
            return Err(Error::WrongScope);
        }
        tx.commit()?;
        let journal: String = db.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))?;
        if !journal.eq_ignore_ascii_case("wal") {
            return Err(Error::InvalidInput);
        }
        Ok(Self {
            db,
            rig_id: rig_id.to_owned(),
        })
    }

    /// Explicit admission only. Reopening the same night restores its latch.
    /// A different night cannot replace an unfinished or overlapping night.
    /// Hosts must still obtain fresh allocation, safety and operator authority.
    pub fn begin_night(
        &mut self,
        identity: Identity,
        policy: Policy,
        now: u64,
    ) -> Result<Applied, Error> {
        if identity.rig_id != self.rig_id {
            return Err(Error::WrongScope);
        }
        identity.validate()?;
        policy.validate(&identity)?;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(current) = read_current(&tx)? {
            if current.snapshot.identity.night_id == identity.night_id {
                if current.snapshot.identity != identity || current.snapshot.policy != policy {
                    return Err(Error::Conflict);
                }
                return Ok(Applied {
                    newly_applied: false,
                    record: current,
                });
            }
            if !matches!(current.snapshot.phase, Phase::Stopped { .. })
                || identity.starts_at_ms < current.snapshot.identity.ends_at_ms
            {
                return Err(Error::Conflict);
            }
        }
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM night WHERE night_id=?1)",
            [&identity.night_id],
            |r| r.get(0),
        )?;
        if exists {
            return Err(Error::Conflict);
        }
        let initial = Snapshot::new(identity.clone(), policy, now)?;
        let payload = encode(&initial)?;
        tx.execute(
            "INSERT INTO night VALUES(?1,0,?2,?3)",
            params![
                identity.night_id,
                payload,
                Sha256::digest(payload.as_bytes()).as_slice()
            ],
        )?;
        tx.execute(
            "UPDATE owner SET current_night=?1 WHERE singleton=1",
            [&identity.night_id],
        )?;
        tx.commit()?;
        Ok(Applied {
            newly_applied: true,
            record: Record {
                revision: 0,
                snapshot: initial,
            },
        })
    }

    pub fn current(&self) -> Result<Option<Record>, Error> {
        let tx = self.db.unchecked_transaction()?;
        let current = read_current(&tx)?;
        tx.commit()?;
        Ok(current)
    }

    /// Bounded durable evidence for future batch check-in, including old nights.
    /// Reading a recorded BeginRecovery/BeginPark never permits replay.
    pub fn events(
        &self,
        night: &str,
        after: u64,
        limit: usize,
    ) -> Result<Vec<JournalEvent>, Error> {
        if !valid_id(night) || after > MAX_EVENTS || !(1..=256).contains(&limit) {
            return Err(Error::InvalidInput);
        }
        let mut statement = self.db.prepare("SELECT revision,request FROM event WHERE night_id=?1 AND revision>?2 ORDER BY revision LIMIT ?3")?;
        let rows = statement.query_map(params![night, after as i64, limit as i64], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?;
        rows.map(|row| {
            let (revision, payload) = row?;
            let revision = u64::try_from(revision).map_err(|_| Error::Corrupt)?;
            if payload.len() > MAX_BYTES {
                return Err(Error::Corrupt);
            }
            let request: Request = serde_json::from_str(&payload)?;
            if request.night_id != night
                || request.expected_revision.checked_add(1) != Some(revision)
            {
                return Err(Error::Corrupt);
            }
            Ok(JournalEvent { revision, request })
        })
        .collect()
    }

    pub fn apply(&mut self, request: &Request) -> Result<Applied, Error> {
        if !valid_id(&request.event_id)
            || !valid_id(&request.night_id)
            || !valid_id(&request.configuration_id)
        {
            return Err(Error::InvalidInput);
        }
        let encoded = encode(request)?;
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = read_current(&tx)?.ok_or(Error::WrongScope)?;
        if current.snapshot.identity.night_id != request.night_id
            || current.snapshot.identity.configuration_id != request.configuration_id
        {
            return Err(Error::WrongScope);
        }
        let previous: Option<String> = tx
            .query_row(
                "SELECT request FROM event WHERE night_id=?1 AND event_id=?2",
                params![request.night_id, request.event_id],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(previous) = previous {
            if previous != encoded {
                return Err(Error::Conflict);
            }
            return Ok(Applied {
                newly_applied: false,
                record: current,
            });
        }
        if current.revision != request.expected_revision {
            return Err(Error::Conflict);
        }
        if current.revision >= MAX_EVENTS {
            return Err(Error::LimitReached);
        }
        let snapshot =
            current
                .snapshot
                .apply(request.now_ms, request.conditions, &request.event)?;
        // New request IDs cannot reuse the same image, failure receipt or native
        // attempt to grow confidence, reset counters or obtain another dispatch.
        for (kind, identity) in evidence_keys(&request.event) {
            if !valid_id(identity) {
                return Err(Error::InvalidInput);
            }
            let inserted = tx.execute(
                "INSERT OR IGNORE INTO evidence VALUES(?1,?2,?3)",
                params![request.night_id, kind, identity],
            )?;
            if inserted != 1 {
                return Err(Error::Conflict);
            }
        }
        let revision = current.revision + 1;
        let payload = encode(&snapshot)?;
        tx.execute(
            "UPDATE night SET revision=?2,payload=?3,digest=?4 WHERE night_id=?1",
            params![
                request.night_id,
                revision as i64,
                payload,
                Sha256::digest(payload.as_bytes()).as_slice()
            ],
        )?;
        tx.execute(
            "INSERT INTO event VALUES(?1,?2,?3,?4)",
            params![request.night_id, request.event_id, revision as i64, encoded],
        )?;
        tx.commit()?;
        Ok(Applied {
            newly_applied: true,
            record: Record { revision, snapshot },
        })
    }
}

fn evidence_keys(event: &Event) -> Vec<(&'static str, &str)> {
    match event {
        Event::Quality { sample } => vec![("capture", &sample.capture_id)],
        Event::RecoveryCompleted {
            result: RecoveryResult::Quality { sample },
            ..
        } => vec![("capture", &sample.capture_id)],
        Event::Failure { failure } => vec![("failure", &failure.attempt_id)],
        Event::BeginRecovery { attempt_id } | Event::BeginPark { attempt_id } => {
            vec![("operation", attempt_id)]
        }
        _ => vec![],
    }
}

fn read_current(db: &Connection) -> Result<Option<Record>, Error> {
    let night: Option<String> = db.query_row(
        "SELECT current_night FROM owner WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    let Some(night) = night else {
        return Ok(None);
    };
    let (revision, payload, digest): (i64, String, Vec<u8>) = db.query_row(
        "SELECT revision,payload,digest FROM night WHERE night_id=?1",
        [&night],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let revision = u64::try_from(revision).map_err(|_| Error::Corrupt)?;
    if revision > MAX_EVENTS
        || payload.len() > MAX_BYTES
        || Sha256::digest(payload.as_bytes()).as_slice() != digest
    {
        return Err(Error::Corrupt);
    }
    let snapshot: Snapshot = serde_json::from_str(&payload)?;
    let rig: String = db.query_row("SELECT rig_id FROM owner WHERE singleton=1", [], |r| {
        r.get(0)
    })?;
    if snapshot.schema_version != psf_guard_director_core::recovery::VERSION
        || snapshot.identity.night_id != night
        || snapshot.identity.rig_id != rig
    {
        return Err(Error::Corrupt);
    }
    snapshot.validate()?;
    Ok(Some(Record { revision, snapshot }))
}
