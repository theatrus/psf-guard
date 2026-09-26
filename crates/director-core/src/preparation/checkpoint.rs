use super::*;

const FORMAT_VERSION: u32 = 1;
const MAX_CHECKPOINT_BYTES: usize = crate::MAX_REQUEST_BYTES + 65_536;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Checkpoint {
    format_version: u32,
    engine_version: String,
    id: String,
    initial: Request,
    context: Context,
    estimates: Estimates,
    observations: Vec<Observation>,
    #[serde(deserialize_with = "Option::deserialize")]
    pending_issued_at_ms: Option<u64>,
    #[serde(deserialize_with = "Option::deserialize")]
    halted: Option<Decision>,
    last_time_ms: u64,
}

impl Preparation {
    /// Opaque, versioned persistence data. Hosts must commit it before returning
    /// a Run command, not after dispatch. It is not an IPC request or a permit.
    pub fn checkpoint(&self) -> Result<Vec<u8>, Error> {
        let checkpoint = Checkpoint {
            format_version: FORMAT_VERSION,
            engine_version: crate::ENGINE_VERSION.into(),
            id: self.id.clone(),
            initial: self.initial.clone(),
            context: self.context.clone(),
            estimates: self.estimates,
            observations: self.observations.clone(),
            pending_issued_at_ms: self.pending.as_ref().map(|(_, time)| *time),
            halted: self.halted.clone(),
            last_time_ms: self.last_time_ms,
        };
        let bytes = serde_json::to_vec(&checkpoint).map_err(|_| Error::InvalidCheckpoint)?;
        if bytes.len() > MAX_CHECKPOINT_BYTES {
            return Err(Error::InvalidCheckpoint);
        }
        Ok(bytes)
    }

    /// Restore validated state, retaining an in-flight operation as in-flight.
    /// No Run command is returned or replayed by restoration.
    pub fn restore(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > MAX_CHECKPOINT_BYTES {
            return Err(Error::InvalidCheckpoint);
        }
        let saved: Checkpoint =
            serde_json::from_slice(bytes).map_err(|_| Error::InvalidCheckpoint)?;
        if saved.format_version != FORMAT_VERSION || saved.engine_version != crate::ENGINE_VERSION {
            return Err(Error::InvalidCheckpoint);
        }
        let mut restored = Self::new(saved.id, &saved.initial, saved.context, saved.estimates)
            .map_err(|_| Error::InvalidCheckpoint)?;
        if saved.observations.len() > restored.steps.len() {
            return Err(Error::InvalidCheckpoint);
        }
        for observation in saved.observations {
            restored.restore_pending(observation.issued_at_ms)?;
            if restored.pending() != Some(&observation.command) {
                return Err(Error::InvalidCheckpoint);
            }
            restored
                .complete(observation.completion)
                .map_err(|_| Error::InvalidCheckpoint)?;
        }
        if let Some(issued_at_ms) = saved.pending_issued_at_ms {
            restored.restore_pending(issued_at_ms)?;
        }
        // Failures cannot be erased by a damaged snapshot. Other terminal
        // recommendations only stop work, so never restore an Acquire/Continue.
        if restored.halted.is_some() && saved.halted.is_none()
            || saved.last_time_ms < restored.last_time_ms
            || matches!(
                saved.halted,
                Some(Decision::Acquire { .. } | Decision::Continue { .. })
            )
        {
            return Err(Error::InvalidCheckpoint);
        }
        restored.halted = saved.halted;
        restored.last_time_ms = saved.last_time_ms;
        Ok(restored)
    }

    fn restore_pending(&mut self, issued_at_ms: u64) -> Result<(), Error> {
        let step = self
            .steps
            .get(self.cursor)
            .ok_or(Error::InvalidCheckpoint)?;
        if self.halted.is_some() || self.pending.is_some() || issued_at_ms < self.last_time_ms {
            return Err(Error::InvalidCheckpoint);
        }
        self.pending = Some((
            Command {
                preparation_id: self.id.clone(),
                ordinal: self.cursor as u32 + 1,
                goal_id: self.context.goal_id.clone(),
                target_id: self.context.target_id.clone(),
                recipe_id: self.context.recipe_id.clone(),
                operation: step.operation.clone(),
            },
            issued_at_ms,
        ));
        self.last_time_ms = issued_at_ms;
        Ok(())
    }
}
