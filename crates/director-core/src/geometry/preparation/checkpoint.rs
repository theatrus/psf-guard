use super::*;
use crate::program::Program;
use serde::{Deserialize, Serialize};

const FORMAT_VERSION: u32 = 1;
// Program, original request, full horizon, and escaped inner journal are bounded
// together before decoding. This is local persistence, not an IPC frame.
const MAX_CHECKPOINT_BYTES: usize = 8 * crate::MAX_REQUEST_BYTES;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Checkpoint {
    format_version: u32,
    engine_version: String,
    program: Program,
    constraints: Constraints,
    initial: Request,
    local: LocalState,
    preparation: String,
}

impl GeometryPreparation<'_> {
    /// Opaque local persistence. Commit with an integrity digest before returning
    /// any Run command. These bytes neither authenticate intent nor grant dispatch.
    pub fn checkpoint(&self) -> Result<Vec<u8>, Error> {
        let saved = Checkpoint {
            format_version: FORMAT_VERSION,
            engine_version: crate::ENGINE_VERSION.into(),
            program: self.geometry.source.snapshot().clone(),
            constraints: self.geometry.constraints.clone(),
            initial: self.initial.clone(),
            local: self.local.clone(),
            preparation: String::from_utf8(
                self.inner
                    .checkpoint()
                    .map_err(|_| Error::InvalidCheckpoint)?,
            )
            .map_err(|_| Error::InvalidCheckpoint)?,
        };
        let bytes = serde_json::to_vec(&saved).map_err(|_| Error::InvalidCheckpoint)?;
        if bytes.len() > MAX_CHECKPOINT_BYTES {
            return Err(Error::InvalidCheckpoint);
        }
        Ok(bytes)
    }
}

impl BoundGeometry {
    /// Recompile the original trusted program and constraints before restoring.
    /// Compare the complete binding, then recompute the initial narrowed request;
    /// saved diagnostic windows cannot replace computed geometric evidence.
    /// Restoration returns no command. Call next with fresh state and constraints.
    pub fn restore_preparation(&self, bytes: &[u8]) -> Result<GeometryPreparation<'_>, Error> {
        if bytes.len() > MAX_CHECKPOINT_BYTES {
            return Err(Error::InvalidCheckpoint);
        }
        let saved: Checkpoint =
            serde_json::from_slice(bytes).map_err(|_| Error::InvalidCheckpoint)?;
        if saved.format_version != FORMAT_VERSION
            || saved.engine_version != crate::ENGINE_VERSION
            || saved.program != *self.source.snapshot()
            || saved.constraints != self.constraints
        {
            return Err(Error::InvalidCheckpoint);
        }
        let inner = Preparation::restore(saved.preparation.as_bytes())
            .map_err(|_| Error::InvalidCheckpoint)?;
        let mut restored = self
            .preparation(
                inner.id().into(),
                &saved.initial,
                &self.constraints,
                &inner.context().goal_id,
                saved.local,
                inner.estimates(),
            )
            .map_err(|_| Error::InvalidCheckpoint)?;
        if !inner.same_inputs(&restored.inner) {
            return Err(Error::InvalidCheckpoint);
        }
        restored.inner = inner;
        Ok(restored)
    }
}
