//! Persist the original geometry binding, never caller-supplied computed windows.

use super::*;
use psf_guard_director_core::preparation::{Command, Estimates, Next};
use psf_guard_director_core::program::{Configuration, LocalState};
use sha2::{Digest, Sha256};

const MAX_GEOMETRY_BYTES: usize = 4 * MAX_REQUEST_BYTES;

pub(super) fn create_table(connection: &Connection) -> Result<(), Error> {
    connection.execute_batch(
        "ALTER TABLE allocation ADD COLUMN geometry_required INTEGER NOT NULL DEFAULT 0 CHECK(geometry_required IN (0,1));
         CREATE TABLE execution_geometry (
            singleton INTEGER PRIMARY KEY CHECK(singleton=1),
            payload TEXT NOT NULL, digest BLOB NOT NULL CHECK(length(digest)=32)
         );",
    )?;
    Ok(())
}

pub(super) fn insert(
    connection: &Connection,
    geometry: Option<&BoundGeometry>,
) -> Result<(), Error> {
    if let Some(geometry) = geometry {
        let payload = serde_json::to_string(geometry.constraints())?;
        if payload.len() > MAX_GEOMETRY_BYTES {
            return Err(Error::InvalidInput);
        }
        connection.execute(
            "INSERT INTO execution_geometry VALUES (1,?1,?2)",
            params![payload, Sha256::digest(payload.as_bytes()).as_slice()],
        )?;
        connection.execute(
            "UPDATE allocation SET geometry_required=1 WHERE singleton=1",
            [],
        )?;
    }
    Ok(())
}

pub(super) fn verify(
    connection: &Connection,
    expected: Option<&BoundGeometry>,
) -> Result<(), Error> {
    let (required, program_required): (bool, bool) = connection.query_row(
        "SELECT geometry_required,program_required FROM allocation WHERE singleton=1",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let stored: Option<(String, Vec<u8>)> = connection
        .query_row(
            "SELECT payload,digest FROM execution_geometry WHERE singleton=1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    if required != stored.is_some() || (required && !program_required) {
        return Err(Error::CorruptLedger);
    }
    if let Some((payload, digest)) = &stored
        && (payload.len() > MAX_GEOMETRY_BYTES
            || Sha256::digest(payload.as_bytes()).as_slice() != digest)
    {
        return Err(Error::CorruptLedger);
    }
    match (stored, expected) {
        (None, None) => Ok(()),
        (Some((payload, _)), Some(expected))
            if payload == serde_json::to_string(expected.constraints())? =>
        {
            Ok(())
        }
        _ => Err(Error::AssignmentMismatch),
    }
}

impl Ledger {
    /// Recompile original geometry before opening the writer transaction. No
    /// existing ledger can change modes or adopt a different constraint snapshot.
    pub fn open_geometry(
        path: &Path,
        program: Program,
        constraints: Constraints,
        state: State,
    ) -> Result<Self, Error> {
        let program = BoundProgram::new(program, &state).map_err(Error::Program)?;
        let geometry =
            BoundGeometry::new(program.clone(), constraints, &state).map_err(Error::Geometry)?;
        Self::open_internal(
            path,
            program.snapshot().assignment.clone(),
            state,
            Some(program),
            Some(geometry),
        )
    }

    pub(super) fn check_geometry_mode(&self, requested: bool) -> Result<(), Error> {
        if requested != self.geometry.is_some() {
            return Err(Error::ConflictingEvidence);
        }
        Ok(())
    }

    pub fn evaluate_geometry(
        &mut self,
        state: State,
        current: &Constraints,
    ) -> Result<Decision, Error> {
        self.evaluate_inner(state, Some(current))
    }

    pub fn begin_geometry_preparation(
        &mut self,
        id: &str,
        goal_id: &str,
        local: LocalState,
        estimates: Estimates,
        state: State,
        current: &Constraints,
    ) -> Result<preparation::Started, Error> {
        self.check_geometry_mode(true)?;
        let context = self
            .check_program_configuration(&local.configuration)?
            .preparation_context(goal_id, local.clone())
            .map_err(Error::Program)?;
        self.begin_preparation_inner(id, context, estimates, state, Some((current, local)))
    }

    pub fn advance_geometry_preparation(
        &mut self,
        id: &str,
        state: State,
        configuration: &Configuration,
        current: &Constraints,
    ) -> Result<Next, Error> {
        self.check_geometry_mode(true)?;
        self.check_program_configuration(configuration)?;
        self.advance_preparation_inner(id, state, Some(current))
    }

    /// Fresh feasibility for one linked reservation, never a recovery replay grant.
    pub fn check_geometry_capture_dispatch(
        &mut self,
        preparation_id: &str,
        capture_id: &str,
        state: State,
        configuration: &Configuration,
        current: &Constraints,
    ) -> Result<Decision, Error> {
        self.check_geometry_mode(true)?;
        self.check_program_configuration(configuration)?;
        self.check_capture_dispatch_inner(preparation_id, capture_id, state, Some(current))
    }

    /// Fresh feasibility only; never issues a command or authorizes recovery replay.
    pub fn check_geometry_pending_dispatch(
        &mut self,
        command: &Command,
        state: State,
        configuration: &Configuration,
        current: &Constraints,
    ) -> Result<Decision, Error> {
        self.check_geometry_mode(true)?;
        self.check_program_configuration(configuration)?;
        self.check_pending_dispatch_inner(command, state, Some(current))
    }

    pub fn reserve_geometry_prepared(
        &mut self,
        id: &str,
        capture_id: &str,
        state: State,
        configuration: &Configuration,
        current: &Constraints,
    ) -> Result<Reservation, Error> {
        self.check_geometry_mode(true)?;
        self.check_program_configuration(configuration)?;
        self.reserve_prepared_inner(id, capture_id, state, Some(current))
    }
}
