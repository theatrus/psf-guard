//! Opt-in session recovery IPC. Storage paths are launcher-owned, never supplied
//! by a message. Issued operations still require the native owner's safety gates.
use crate::{storage, ProtocolError};
use psf_guard_director_core::{recovery as core, State};
use psf_guard_director_ledger::recovery::{self as ledger, SessionStore};
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
};

pub const CONTRACT_VERSION: u32 = 2;
pub const MAX_EVENT_PAGE: usize = 16;

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub recovery_version: u32,
    pub operation: Operation,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    Open {
        identity: core::Identity,
        policy: core::Policy,
        now_ms: u64,
    },
    Current {},
    Apply {
        request: ledger::Request,
    },
    Events {
        night_id: String,
        after: u64,
        limit: usize,
    },
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum Issued {
    Probe {
        attempt_id: String,
        deadline_ms: u64,
    },
    Park {
        attempt_id: String,
        deadline_ms: u64,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum Reply {
    Opened {
        created: bool,
        record: Box<ledger::Record>,
    },
    Current {
        #[serde(deserialize_with = "Option::deserialize")]
        record: Option<Box<ledger::Record>>,
    },
    Applied {
        newly_applied: bool,
        record: Box<ledger::Record>,
        /// Never present on replay or preempted input. Not equipment permission.
        #[serde(deserialize_with = "Option::deserialize")]
        issued: Option<Issued>,
    },
    Events {
        events: Vec<ledger::JournalEvent>,
        next_cursor: u64,
    },
    Error {
        code: Error,
    },
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Error {
    Disabled,
    UnsupportedVersion,
    InvalidInput,
    InvalidDirectory,
    Busy,
    Unavailable,
    WrongScope,
    Conflict,
    Corrupt,
    UnsupportedSchema,
    ForeignDatabase,
    LimitReached,
    WrongPhase,
    StaleEvidence,
    ChangedReference,
    ClockReversed,
    NotAdmitted,
    AcquisitionBlocked,
}

impl From<ledger::Error> for Error {
    fn from(error: ledger::Error) -> Self {
        match error {
            ledger::Error::Storage(error) => match error.sqlite_error_code() {
                Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked) => {
                    Self::Busy
                }
                _ => Self::Unavailable,
            },
            ledger::Error::Encoding(_) | ledger::Error::Corrupt => Self::Corrupt,
            ledger::Error::InvalidInput => Self::InvalidInput,
            ledger::Error::ForeignDatabase => Self::ForeignDatabase,
            ledger::Error::UnsupportedSchema => Self::UnsupportedSchema,
            ledger::Error::WrongScope => Self::WrongScope,
            ledger::Error::Conflict => Self::Conflict,
            ledger::Error::LimitReached => Self::LimitReached,
            ledger::Error::Policy(error) => match error {
                core::Error::InvalidInput => Self::InvalidInput,
                core::Error::WrongPhase => Self::WrongPhase,
                core::Error::StaleEvidence => Self::StaleEvidence,
                core::Error::ChangedReference => Self::ChangedReference,
                core::Error::ClockReversed => Self::ClockReversed,
                core::Error::LimitReached => Self::LimitReached,
            },
        }
    }
}

pub struct Storage {
    path: PathBuf,
    store: Option<SessionStore>,
    rig_id: Option<String>,
    // Release the lease after SQLite, including when a blocking task outlives IPC.
    _lease: File,
}

impl Storage {
    pub async fn acquire_for_startup(directory: &Path) -> Result<Self, Error> {
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                match Self::acquire(directory) {
                    Err(Error::Busy) => {
                        tokio::time::sleep(std::time::Duration::from_millis(25)).await
                    }
                    result => return result,
                }
            }
        })
        .await
        .unwrap_or(Err(Error::Busy))
    }

    pub fn acquire(directory: &Path) -> Result<Self, Error> {
        if !directory.is_absolute() || !directory.is_dir() {
            return Err(Error::InvalidDirectory);
        }
        let directory = directory
            .canonicalize()
            .map_err(|_| Error::InvalidDirectory)?;
        let lease = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(directory.join("director-recovery.lock"))
            .map_err(|_| Error::Unavailable)?;
        lease.try_lock().map_err(|error| match error {
            std::fs::TryLockError::WouldBlock => Error::Busy,
            std::fs::TryLockError::Error(_) => Error::Unavailable,
        })?;
        Ok(Self {
            path: directory.join("recovery.sqlite"),
            store: None,
            rig_id: None,
            _lease: lease,
        })
    }

    fn store(&mut self, rig_id: &str) -> Result<&mut SessionStore, Error> {
        if self.rig_id.as_ref().is_some_and(|rig| rig != rig_id) {
            return Err(Error::WrongScope);
        }
        if self.store.is_none() {
            self.store = Some(SessionStore::open(&self.path, rig_id)?);
            self.rig_id = Some(rig_id.to_owned());
        }
        Ok(self.store.as_mut().expect("initialized above"))
    }

    fn handle(&mut self, request: Request, rig_id: &str) -> Result<Reply, Error> {
        if request.recovery_version != CONTRACT_VERSION {
            return Err(Error::UnsupportedVersion);
        }
        // Reject cross-rig input before creating or touching a recovery database.
        match &request.operation {
            Operation::Open { identity, .. } if identity.rig_id != rig_id => {
                return Err(Error::WrongScope)
            }
            Operation::Apply { request } => {
                let sample = match &request.event {
                    core::Event::Quality { sample } => Some(sample),
                    core::Event::RecoveryCompleted {
                        result: core::RecoveryResult::Quality { sample },
                        ..
                    } => Some(sample.as_ref()),
                    _ => None,
                };
                if sample.is_some_and(|sample| {
                    sample.rig_id != rig_id || sample.configuration_id != request.configuration_id
                }) {
                    return Err(Error::WrongScope);
                }
            }
            _ => {}
        }
        let store = self.store(rig_id)?;
        Ok(match request.operation {
            Operation::Open {
                identity,
                policy,
                now_ms,
            } => {
                let applied = store.begin_night(identity, policy, now_ms)?;
                Reply::Opened {
                    created: applied.newly_applied,
                    record: Box::new(applied.record),
                }
            }
            Operation::Current {} => Reply::Current {
                record: store.current()?.map(Box::new),
            },
            Operation::Apply { request } => {
                let applied = store.apply(&request)?;
                let issued = if applied.newly_applied {
                    match (&request.event, &applied.record.snapshot.phase) {
                        (
                            core::Event::BeginRecovery { attempt_id },
                            core::Phase::Recovering {
                                attempt_id: actual,
                                deadline_ms,
                                ..
                            },
                        ) if attempt_id == actual => Some(Issued::Probe {
                            attempt_id: actual.clone(),
                            deadline_ms: *deadline_ms,
                        }),
                        (
                            core::Event::BeginPark { attempt_id },
                            core::Phase::Stopping {
                                park_attempt_id: Some(actual),
                                deadline_ms,
                                ..
                            },
                        ) if attempt_id == actual => Some(Issued::Park {
                            attempt_id: actual.clone(),
                            deadline_ms: *deadline_ms,
                        }),
                        _ => None,
                    }
                } else {
                    None
                };
                Reply::Applied {
                    newly_applied: applied.newly_applied,
                    record: Box::new(applied.record),
                    issued,
                }
            }
            Operation::Events {
                night_id,
                after,
                limit,
            } => {
                if !(1..=MAX_EVENT_PAGE).contains(&limit) {
                    return Err(Error::InvalidInput);
                }
                let mut events = store.events(&night_id, after, limit)?;
                // Terminal-state evidence may be much larger than ordinary
                // samples. Preserve a complete prefix that fits one IPC frame.
                let mut bytes = 0;
                let mut count = 0;
                for event in &events {
                    bytes += serde_json::to_vec(event).map_err(|_| Error::Corrupt)?.len() + 1;
                    if bytes > psf_guard_director_core::MAX_REQUEST_BYTES - 1024 {
                        break;
                    }
                    count += 1;
                }
                if !events.is_empty() && count == 0 {
                    return Err(Error::LimitReached);
                }
                events.truncate(count);
                let next_cursor = events.last().map_or(after, |event| event.revision);
                Reply::Events {
                    events,
                    next_cursor,
                }
            }
        })
    }

    fn gate(&mut self, rig_id: &str, state: &State) -> Result<u64, Error> {
        if state.rig_id != rig_id {
            return Err(Error::WrongScope);
        }
        let store = self.store(rig_id)?;
        let record = store.current()?.ok_or(Error::NotAdmitted)?;
        let snapshot = record.snapshot;
        if state.rig_id != snapshot.identity.rig_id
            || state.configuration_id != snapshot.identity.configuration_id
        {
            return Err(Error::WrongScope);
        }
        if state.now_ms < snapshot.last_event_ms {
            return Err(Error::ClockReversed);
        }
        if state.now_ms >= snapshot.identity.ends_at_ms
            || state.now_ms - snapshot.last_event_ms > snapshot.policy.evidence_max_age_ms
            || !matches!(snapshot.phase, core::Phase::Acquiring {})
        {
            return Err(Error::AcquisitionBlocked);
        }
        let after = record
            .revision
            .checked_sub(1)
            .ok_or(Error::AcquisitionBlocked)?;
        let latest = store.events(&snapshot.identity.night_id, after, 1)?;
        let latest = latest.first().ok_or(Error::Corrupt)?;
        if latest.revision != record.revision || latest.request.now_ms != snapshot.last_event_ms {
            return Err(Error::Corrupt);
        }
        if latest.request.conditions.safety != psf_guard_director_core::Safety::Safe
            || latest.request.conditions.motion != core::Motion::Permitted
        {
            return Err(Error::AcquisitionBlocked);
        }
        Ok(snapshot.identity.ends_at_ms)
    }
}

pub async fn execute(
    storage: &mut Option<Storage>,
    request: Request,
    rig_id: &str,
) -> Result<Reply, ProtocolError> {
    let Some(mut owned) = storage.take() else {
        return Ok(Reply::Error {
            code: Error::Disabled,
        });
    };
    let rig = rig_id.to_owned();
    let (owned, result) = tokio::task::spawn_blocking(move || {
        let result = owned.handle(request, &rig);
        (owned, result)
    })
    .await
    .map_err(|_| ProtocolError::StorageFailure)?;
    *storage = Some(owned);
    Ok(result.unwrap_or_else(|code| Reply::Error { code }))
}

/// Read the durable latch before each operation that might select or issue work.
/// Readback and completion paths remain usable to reconcile interrupted work.
pub async fn gate(
    storage: &mut Option<Storage>,
    operation: &mut storage::Operation,
    rig_id: &str,
) -> Result<Result<(), Error>, ProtocolError> {
    let Some(state) = operation.acquisition_state_mut() else {
        return Ok(Ok(()));
    };
    let Some(mut owned) = storage.take() else {
        return Ok(Ok(()));
    };
    let query = state.clone();
    let rig = rig_id.to_owned();
    let (owned, result) = tokio::task::spawn_blocking(move || {
        let result = owned.gate(&rig, &query);
        (owned, result)
    })
    .await
    .map_err(|_| ProtocolError::StorageFailure)?;
    *storage = Some(owned);
    Ok(result.map(|deadline| {
        // The existing planner enforces this bound across the complete operation,
        // not only its start. Never let recovery expand the caller's validity.
        state.conditions_valid_until_ms = state.conditions_valid_until_ms.min(deadline);
        state.completion_deadline_ms = Some(
            state
                .completion_deadline_ms
                .map_or(deadline, |end| end.min(deadline)),
        );
    }))
}
