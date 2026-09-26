//! Immutable observing programs. Bound and unbound ledgers never switch modes.

use super::*;
use psf_guard_director_core::preparation::{Estimates, Next};
use psf_guard_director_core::program::{Configuration, LocalState, Recipe, Target};
use sha2::{Digest, Sha256};

pub(super) fn create_table(connection: &Connection) -> Result<(), Error> {
    connection.execute_batch(
        "CREATE TABLE execution_program (
            singleton INTEGER PRIMARY KEY CHECK(singleton=1),
            payload TEXT NOT NULL, digest BLOB NOT NULL CHECK(length(digest)=32)
         );",
    )?;
    Ok(())
}

pub(super) fn insert(connection: &Connection, program: Option<&BoundProgram>) -> Result<(), Error> {
    if let Some(program) = program {
        let payload = serde_json::to_string(program.snapshot())?;
        connection.execute(
            "INSERT INTO execution_program VALUES (1,?1,?2)",
            params![payload, Sha256::digest(payload.as_bytes()).as_slice()],
        )?;
    }
    Ok(())
}

pub(super) fn verify(
    connection: &Connection,
    expected: Option<&BoundProgram>,
) -> Result<(), Error> {
    let required: bool = connection.query_row(
        "SELECT program_required FROM allocation WHERE singleton=1",
        [],
        |row| row.get(0),
    )?;
    let stored: Option<(String, Vec<u8>)> = connection
        .query_row(
            "SELECT payload,digest FROM execution_program WHERE singleton=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    if required != stored.is_some() {
        return Err(Error::CorruptLedger);
    }
    if let Some((payload, digest)) = &stored
        && (payload.len() > MAX_REQUEST_BYTES
            || Sha256::digest(payload.as_bytes()).as_slice() != digest)
    {
        return Err(Error::CorruptLedger);
    }
    match (stored, expected) {
        (None, None) => Ok(()),
        (Some((payload, _)), Some(expected))
            if payload == serde_json::to_string(expected.snapshot())? =>
        {
            Ok(())
        }
        _ => Err(Error::AssignmentMismatch),
    }
}

/// Saved binding and capture evidence, never a dispatch permit or a retry grant.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CaptureBinding {
    pub ledger: LedgerInfo,
    pub attempt: Attempt,
    pub target: Target,
    pub recipe: Recipe,
    pub configuration: Configuration,
}

impl Ledger {
    pub fn program(&self) -> Option<&Program> {
        self.program.as_ref().map(BoundProgram::snapshot)
    }

    pub(super) fn check_program_configuration(
        &self,
        configuration: &Configuration,
    ) -> Result<&BoundProgram, Error> {
        let program = self.program.as_ref().ok_or(Error::ConflictingEvidence)?;
        if &program.snapshot().configuration != configuration {
            return Err(Error::AssignmentMismatch);
        }
        Ok(program)
    }

    pub fn begin_program_preparation(
        &mut self,
        id: &str,
        goal_id: &str,
        local: LocalState,
        estimates: Estimates,
        state: State,
    ) -> Result<preparation::Started, Error> {
        let context = self
            .check_program_configuration(&local.configuration)?
            .preparation_context(goal_id, local)
            .map_err(Error::Program)?;
        self.begin_preparation_inner(id, context, estimates, state, None)
    }

    pub fn advance_program_preparation(
        &mut self,
        id: &str,
        state: State,
        configuration: &Configuration,
    ) -> Result<Next, Error> {
        self.check_program_configuration(configuration)?;
        self.advance_preparation_inner(id, state, None)
    }

    pub fn reserve_program_prepared(
        &mut self,
        id: &str,
        capture_id: &str,
        state: State,
        configuration: &Configuration,
    ) -> Result<Reservation, Error> {
        self.check_program_configuration(configuration)?;
        self.reserve_prepared_inner(id, capture_id, state, None)
    }

    /// Resolve the exact persisted settings for a capture after reservation or
    /// recovery. Reads do not re-select a goal or consume another attempt.
    pub fn capture_binding(&self, capture_id: &str) -> Result<Option<CaptureBinding>, Error> {
        if !valid_id(capture_id) {
            return Err(Error::InvalidInput);
        }
        let program = self.program.as_ref().ok_or(Error::ConflictingEvidence)?;
        let Some(attempt) = read_attempt(&self.connection, capture_id)? else {
            return Ok(None);
        };
        let resolved = program
            .resolve(&attempt.goal_id)
            .map_err(|_| Error::CorruptLedger)?;
        Ok(Some(CaptureBinding {
            ledger: self.info(),
            target: resolved.target.clone(),
            recipe: resolved.recipe.clone(),
            configuration: resolved.configuration.clone(),
            attempt,
        }))
    }
}
