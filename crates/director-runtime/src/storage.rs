//! Opt-in local storage. Paths come only from the launcher, never IPC commands.
use crate::ProtocolError;
use psf_guard_director_core::preparation::{
    Completion, Context, Error as PreparationError, Estimates, Next,
};
use psf_guard_director_core::{Request, State};
use psf_guard_director_ledger::preparation::{
    Event as PreparationEvent, Record as PreparationRecord,
};
use psf_guard_director_ledger::{
    Attempt, Error, Evidence, ExecutionEvent, Ledger, LedgerInfo, Reservation,
};
use serde::{Deserialize, Serialize};
use std::{
    fs::{File, OpenOptions},
    path::{Path, PathBuf},
    time::Duration,
};

pub const MAX_EVENT_PAGE: usize = 64;
pub const MAX_PREPARATION_EVENT_PAGE: usize = 32;

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    Evaluate {
        state: State,
    },
    UnresolvedAttempt {},
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
    BeginPreparation {
        preparation_id: String,
        context: Context,
        estimates: Estimates,
        state: State,
    },
    AdvancePreparation {
        preparation_id: String,
        state: State,
    },
    CompletePreparation {
        completion: Completion,
    },
    Preparation {
        preparation_id: String,
    },
    ActivePreparation {},
    ClosePreparation {
        preparation_id: String,
    },
    ReservePrepared {
        preparation_id: String,
        capture_id: String,
        state: State,
    },
    PreparationEvents {
        after: u64,
        limit: usize,
    },
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum StorageReply {
    Evaluated {
        decision: psf_guard_director_core::Decision,
    },
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
        #[serde(deserialize_with = "Option::deserialize")]
        attempt: Option<Attempt>,
    },
    Events {
        events: Vec<ExecutionEvent>,
        next_cursor: u64,
    },
    PreparationStarted {
        created: bool,
        record: PreparationRecord,
    },
    PreparationAdvanced {
        next: Next,
    },
    PreparationRecorded {
        record: PreparationRecord,
    },
    PreparationFound {
        #[serde(deserialize_with = "Option::deserialize")]
        record: Option<PreparationRecord>,
    },
    PreparationClosed {
        record: PreparationRecord,
    },
    PreparationEvents {
        events: Vec<PreparationEvent>,
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
    PreparationNotSelected,
    InvalidCompletion,
    ClockRegression,
}

impl From<Error> for StorageError {
    fn from(error: Error) -> Self {
        match error {
            Error::InvalidInput => Self::InvalidInput,
            Error::Planner(_) => Self::InvalidSnapshot,
            Error::Program(_) => Self::InvalidSnapshot,
            Error::Preparation(error) => match error {
                PreparationError::NotSelected => Self::PreparationNotSelected,
                PreparationError::InvalidCompletion => Self::InvalidCompletion,
                PreparationError::ConflictingCompletion => Self::ConflictingEvidence,
                PreparationError::ClockRegression => Self::ClockRegression,
                PreparationError::InvalidCheckpoint => Self::CorruptLedger,
                _ => Self::InvalidSnapshot,
            },
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
            .map_err(acquisition_io_error)?;
        lease.try_lock().map_err(|error| match error {
            std::fs::TryLockError::WouldBlock => StorageError::Busy,
            std::fs::TryLockError::Error(error) => acquisition_io_error(error),
        })?;
        Ok(Self {
            path: directory.join("execution.sqlite"),
            _lease: lease,
            ledger: None,
        })
    }

    /// A previous process may have exited before the OS finishes releasing its
    /// handles. Wait only for contention, never steal ownership or retry I/O errors.
    pub async fn acquire_for_startup(directory: &Path) -> Result<Self, StorageError> {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                match Self::acquire(directory) {
                    Err(StorageError::Busy) => tokio::time::sleep(Duration::from_millis(25)).await,
                    result => return result,
                }
            }
        })
        .await
        .unwrap_or(Err(StorageError::Busy))
    }

    pub fn handle(
        &mut self,
        operation: Operation,
        rig_id: &str,
    ) -> Result<StorageReply, ProtocolError> {
        if self
            .ledger
            .as_ref()
            .is_some_and(|ledger| ledger.info().rig_id != rig_id)
        {
            return Err(ProtocolError::WrongRig);
        }
        match &operation {
            Operation::Open { request }
                if request.assignment.rig_id != rig_id || request.state.rig_id != rig_id =>
            {
                return Err(ProtocolError::WrongRig)
            }
            Operation::Reserve { state, .. }
            | Operation::Evaluate { state }
            | Operation::BeginPreparation { state, .. }
            | Operation::AdvancePreparation { state, .. }
            | Operation::ReservePrepared { state, .. }
                if state.rig_id != rig_id =>
            {
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
            Operation::Evaluate { state } => StorageReply::Evaluated {
                decision: ledger.evaluate(state)?,
            },
            Operation::UnresolvedAttempt {} => StorageReply::Found {
                attempt: ledger.unresolved_attempt()?,
            },
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
            Operation::BeginPreparation {
                preparation_id,
                context,
                estimates,
                state,
            } => {
                let started =
                    ledger.begin_preparation(&preparation_id, context, estimates, state)?;
                StorageReply::PreparationStarted {
                    created: started.created,
                    record: started.record,
                }
            }
            Operation::AdvancePreparation {
                preparation_id,
                state,
            } => StorageReply::PreparationAdvanced {
                next: ledger.advance_preparation(&preparation_id, state)?,
            },
            Operation::CompletePreparation { completion } => StorageReply::PreparationRecorded {
                record: ledger.complete_preparation(completion)?,
            },
            Operation::Preparation { preparation_id } => StorageReply::PreparationFound {
                record: ledger.preparation(&preparation_id)?,
            },
            Operation::ActivePreparation {} => StorageReply::PreparationFound {
                record: ledger.active_preparation()?,
            },
            Operation::ClosePreparation { preparation_id } => StorageReply::PreparationClosed {
                record: ledger.close_preparation(&preparation_id)?,
            },
            Operation::ReservePrepared {
                preparation_id,
                capture_id,
                state,
            } => StorageReply::Reserved {
                outcome: ledger.reserve_prepared(&preparation_id, &capture_id, state)?,
            },
            Operation::PreparationEvents { after, limit } => {
                if limit == 0 || limit > MAX_PREPARATION_EVENT_PAGE {
                    return Err(StorageError::InvalidInput);
                }
                let events = ledger.preparation_events_after(after, limit)?;
                let next_cursor = events.last().map_or(after, |event| event.sequence);
                StorageReply::PreparationEvents {
                    events,
                    next_cursor,
                }
            }
        })
    }
}

fn acquisition_io_error(error: std::io::Error) -> StorageError {
    #[cfg(windows)]
    if matches!(error.raw_os_error(), Some(32 | 33)) {
        return StorageError::Busy;
    }
    if error.kind() == std::io::ErrorKind::WouldBlock {
        StorageError::Busy
    } else {
        StorageError::Unavailable
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
