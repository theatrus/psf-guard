use super::*;

impl Ledger {
    /// Revalidate an already reserved exposure after native before-hooks.
    /// This is not a reservation, replay grant, or hardware dispatch permit.
    pub fn check_prepared_capture_dispatch(
        &mut self,
        preparation_id: &str,
        capture_id: &str,
        state: State,
    ) -> Result<Decision, Error> {
        if self.program.is_some() {
            return Err(Error::ConflictingEvidence);
        }
        self.check_capture_dispatch_inner(preparation_id, capture_id, state, None)
    }

    pub(crate) fn check_capture_dispatch_inner(
        &mut self,
        id: &str,
        capture_id: &str,
        state: State,
        current: Option<&Constraints>,
    ) -> Result<Decision, Error> {
        self.check_geometry_mode(current.is_some())?;
        if !valid_id(capture_id) {
            return Err(Error::InvalidInput);
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut stored = read(&tx, id, self.geometry.as_ref())?.ok_or(Error::InvalidInput)?;
        if stored.lifecycle != Lifecycle::Captured
            || stored.capture_id.as_deref() != Some(capture_id)
        {
            return Err(Error::ConflictingEvidence);
        }
        let attempt = read_attempt(&tx, capture_id)?.ok_or(Error::CorruptLedger)?;
        if attempt.goal_id != stored.preparation.context().goal_id
            || stored.preparation.pending().is_some()
        {
            return Err(Error::CorruptLedger);
        }
        if attempt.evidence != Evidence::Reserved {
            return Err(Error::ConflictingEvidence);
        }
        let mut request = snapshot(&tx, &self.assignment, state)?;
        // The core evaluates this exposure, not a second one. Restore only its
        // already-spent attempt in this temporary projection; never edit storage.
        let goal = request
            .assignment
            .goals
            .iter_mut()
            .find(|goal| goal.id == attempt.goal_id)
            .ok_or(Error::CorruptLedger)?;
        goal.attempts_remaining = goal
            .attempts_remaining
            .checked_add(1)
            .ok_or(Error::CorruptLedger)?;
        let old_halt = stored.preparation.halted().cloned();
        let decision = match stored.preparation.next(&request, current)? {
            Next::ReadyToReserve { goal_id } if goal_id == attempt.goal_id => Decision::Acquire {
                goal_id,
                reason: "reserved_capture_ready".into(),
            },
            Next::Decision(decision) => decision,
            // A captured preparation must never issue another native operation.
            _ => return Err(Error::CorruptLedger),
        };
        persist(&tx, &stored.preparation)?;
        if old_halt.as_ref() != stored.preparation.halted()
            && let Some(halted) = stored.preparation.halted()
        {
            append(
                &tx,
                &self.ledger_id,
                &self.assignment,
                id,
                EventKind::Halted {
                    decision: halted.clone(),
                },
            )?;
        }
        tx.commit()?;
        Ok(decision)
    }
}
