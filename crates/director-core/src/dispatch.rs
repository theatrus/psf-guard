//! Short-lived feasibility bounds for an already selected operation.
//! These bounds reserve nothing and are never hardware or recovery authority.

use crate::{evaluate, validate, Decision, Error, Request};
use serde::Serialize;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DispatchCheck {
    pub decision: Decision,
    pub evaluated_at_ms: u64,
    /// Inclusive latest start for the selected work in its current safe window.
    /// Only Acquire has a bound. All other decisions must refuse new dispatch.
    pub latest_start_ms: Option<u64>,
}

/// Evaluate with an explicit bound on time spent between checking and dispatch.
/// Fresh configuration, safety, ownership and one-shot authority are still required.
/// The bound preserves feasibility of this goal, not its future priority rank.
pub fn evaluate_dispatch(request: &Request) -> Result<DispatchCheck, Error> {
    for_decision(request, evaluate(request)?)
}

pub(crate) fn for_decision(request: &Request, decision: Decision) -> Result<DispatchCheck, Error> {
    let latest_start_ms = if let Decision::Acquire { goal_id, .. } = &decision {
        let windows = validate(request)?;
        let (index, goal) = request
            .assignment
            .goals
            .iter()
            .enumerate()
            .find(|(_, goal)| &goal.id == goal_id)
            .ok_or(Error::InvalidGoal)?;
        let duration = goal
            .exposure_ms
            .checked_add(goal.overhead_ms)
            .ok_or(Error::InvalidGoal)?;
        let latest = windows[index]
            .iter()
            .find_map(|window| {
                let latest = window.end_ms.checked_sub(duration)?;
                (window.start_ms <= request.state.now_ms && request.state.now_ms <= latest)
                    .then_some(latest)
            })
            .ok_or(Error::InvalidState)?;
        // Conditions expire exclusively; completion at the window end is legal.
        // Never borrow time from a later disconnected observing window.
        let conditions = request
            .state
            .conditions_valid_until_ms
            .checked_sub(1)
            .ok_or(Error::InvalidState)?;
        let latest = latest.min(conditions);
        if latest < request.state.now_ms {
            return Err(Error::InvalidState);
        }
        Some(latest)
    } else {
        None
    };
    Ok(DispatchCheck {
        decision,
        evaluated_at_ms: request.state.now_ms,
        latest_start_ms,
    })
}
