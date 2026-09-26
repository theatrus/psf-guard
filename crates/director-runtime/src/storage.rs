//! Opt-in local storage. Paths come only from the launcher, never IPC commands.
use crate::ProtocolError;
use psf_guard_director_core::{Request, State};
use psf_guard_director_ledger::{
    Attempt, Error, Evidence, ExecutionEvent, Ledger, LedgerInfo, Reservation,
};
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
};

pub const MAX_EVENT_PAGE: usize = 64;

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    Open {
        request: Request,
    },
    Reserve {
        capture_id: String,
        state: State,
    },
    Record {
        capture_id: String,
        evidence: Evidence,
    },
    Attempt {
        capture_id: String,
    },
    Events {
        after: u64,
        limit: usize,
    },
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum StorageReply {
    Opened {
        info: LedgerInfo,
    },
    Reserved {
        outcome: Reservation,
    },
    Recorded {
        attempt: Attempt,
    },
    Found {
        attempt: Option<Attempt>,
    },
    Events {
        events: Vec<ExecutionEvent>,
        next_cursor: u64,
    },
    Error {
        code: StorageError,
    },
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StorageError {
    Disabled,
    NotOpen,
    AlreadyOpen,
    InvalidInput,
    InvalidSnapshot,
    InvalidDirectory,
    Unavailable,
    Busy,
    ForeignDatabase,
    UnsupportedSchema,
    UnsupportedEngine,
    UnsupportedStorage,
    AssignmentMismatch,
    UnknownCapture,
    ConflictingEvidence,
    CorruptLedger,
}

impl From<Error> for StorageError {
    fn from(error: Error) -> Self {
        match error {
            Error::InvalidInput => Self::InvalidInput,
            Error::Planner(_) => Self::InvalidSnapshot,
            Error::ForeignDatabase => Self::ForeignDatabase,
            Error::UnsupportedSchema => Self::UnsupportedSchema,
            Error::UnsupportedEngine => Self::UnsupportedEngine,
            Error::UnsupportedStorage => Self::UnsupportedStorage,
            Error::AssignmentMismatch => Self::AssignmentMismatch,
            Error::UnknownCapture => Self::UnknownCapture,
            Error::ConflictingEvidence => Self::ConflictingEvidence,
            Error::CorruptLedger | Error::Json(_) => Self::CorruptLedger,
            Error::Sqlite(error) => match error.sqlite_error_code() {
                Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked) => {
                    Self::Busy
                }
                _ => Self::Unavailable,
            },
        }
    }
}

pub struct Storage {
    path: PathBuf,
    ledger: Option<Ledger>,
    // Hold the OS lock until this storage owner (including any blocking task) ends.
    // Process death releases it. Never unlink the lock file: that permits split locks.
    // Fields drop in declaration order: release this only after SQLite closes.
    _lease: File,
}

impl Storage {
    pub(crate) fn is_open(&self) -> bool {
        self.ledger.is_some()
    }
    /// The host supplies a private, existing absolute directory for one rig/profile.
    /// No directory creation, catalog discovery, or wire-supplied paths are allowed.
    pub fn acquire(directory: &Path) -> Result<Self, StorageError> {
        if !directory.is_absolute() || !directory.is_dir() {
            return Err(StorageError::InvalidDirectory);
        }
        let directory = directory
            .canonicalize()
            .map_err(|_| StorageError::InvalidDirectory)?;
        let lease = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(directory.join("director-runtime.lock"))
            .map_err(|_| StorageError::Unavailable)?;
        lease.try_lock().map_err(|error| match error {
            std::fs::TryLockError::WouldBlock => StorageError::Busy,
            std::fs::TryLockError::Error(_) => StorageError::Unavailable,
        })?;
        Ok(Self {
            path: directory.join("execution.sqlite"),
            _lease: lease,
            ledger: None,
        })
    }

    pub fn handle(
        &mut self,
        operation: Operation,
        rig_id: &str,
    ) -> Result<StorageReply, ProtocolError> {
        match &operation {
            Operation::Open { request }
                if request.assignment.rig_id != rig_id || request.state.rig_id != rig_id =>
            {
                return Err(ProtocolError::WrongRig)
            }
            Operation::Reserve { state, .. } if state.rig_id != rig_id => {
                return Err(ProtocolError::WrongRig)
            }
            _ => {}
        }
        Ok(self
            .apply(operation)
            .unwrap_or_else(|code| StorageReply::Error { code }))
    }

    fn apply(&mut self, operation: Operation) -> Result<StorageReply, StorageError> {
        if let Operation::Open { request } = operation {
            if self.ledger.is_some() {
                return Err(StorageError::AlreadyOpen);
            }
            if request.contract_version != psf_guard_director_core::CONTRACT_VERSION {
                return Err(StorageError::InvalidSnapshot);
            }
            let ledger = Ledger::open(&self.path, request.assignment, request.state)?;
            let info = ledger.info();
            self.ledger = Some(ledger);
            return Ok(StorageReply::Opened { info });
        }
        let ledger = self.ledger.as_mut().ok_or(StorageError::NotOpen)?;
        Ok(match operation {
            Operation::Open { .. } => unreachable!("handled above"),
            Operation::Reserve { capture_id, state } => StorageReply::Reserved {
                outcome: ledger.reserve(&capture_id, state)?,
            },
            Operation::Record {
                capture_id,
                evidence,
            } => StorageReply::Recorded {
                attempt: ledger.record(&capture_id, evidence)?,
            },
            Operation::Attempt { capture_id } => StorageReply::Found {
                attempt: ledger.attempt(&capture_id)?,
            },
            Operation::Events { after, limit } => {
                if limit == 0 || limit > MAX_EVENT_PAGE {
                    return Err(StorageError::InvalidInput);
                }
                let events = ledger.events_after(after, limit)?;
                let next_cursor = events.last().map_or(after, |event| event.sequence);
                StorageReply::Events {
                    events,
                    next_cursor,
                }
            }
        })
    }
}

/// Run SQLite off the async pipe executor. An abandoned request may have committed:
/// keep evidence, release ownership only when the operation finishes, and require
/// the next session to recover by capture ID instead of replaying dispatch.
pub async fn execute(
    storage: &mut Option<Storage>,
    operation: Operation,
    rig_id: &str,
) -> Result<StorageReply, ProtocolError> {
    let Some(mut owned) = storage.take() else {
        return Ok(StorageReply::Error {
            code: StorageError::Disabled,
        });
    };
    let rig_id = rig_id.to_owned();
    let (owned, response) = tokio::task::spawn_blocking(move || {
        let response = owned.handle(operation, &rig_id);
        (owned, response)
    })
    .await
    .map_err(|_| ProtocolError::StorageFailure)?;
    *storage = Some(owned);
    response
}
